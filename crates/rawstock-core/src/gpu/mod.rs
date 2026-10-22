//! GPU film pipeline on wgpu, behind the `gpu` feature.
//!
//! The CPU path in [`render`](crate::render) is the reference. Each WGSL
//! entry point mirrors one CPU stage function, and the parity tests hold the
//! two together within a tolerance written in that stage's own units. See
//! `docs/GPU.md`.
//!
//! Setup and readback are async because WebGPU in a browser cannot block.
//! Native callers can use the `_blocking` wrappers.
//!
//! Exposure, both film models, the exact dye scan, print/scan, and the sRGB
//! encode run here. A stock with halation or grain is refused with
//! [`GpuError::Unsupported`] until those stages are ported.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::color::LinearImage;
use crate::render::{Fidelity, Parameters, SrgbImage, Stage};
use crate::stock::{Curve, FilmModel};

#[cfg(test)]
mod tests;

/// Threads per workgroup in `pixel.wgsl`.
const WORKGROUP: u32 = 256;

/// Bytes in one interleaved RGB f32 pixel.
const PIXEL_BYTES: u64 = 12;

/// Why the GPU path could not run.
#[derive(Debug, Error)]
pub enum GpuError {
    /// No adapter matched. Headless machines without a GPU or a software
    /// Vulkan driver land here.
    #[error("no GPU adapter: {0}")]
    NoAdapter(String),
    /// The adapter refused a device.
    #[error("GPU device request failed: {0}")]
    Device(String),
    /// The parameters use a stage the GPU path does not run yet.
    #[error("the GPU path does not run the {} stage yet", .0.name())]
    Unsupported(Stage),
    /// Reading the result back failed.
    #[error("GPU readback failed: {0}")]
    Readback(String),
}

/// A GPU device with the film kernels compiled.
///
/// Cheap to clone. One per process is enough.
#[derive(Clone)]
pub struct Gpu {
    inner: Arc<Inner>,
}

struct Inner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    kernels: [wgpu::ComputePipeline; KERNEL_COUNT],
    adapter: String,
    max_chunk_pixels: u32,
    /// The Jakob-Hanika table, uploaded on the first spectral stock.
    rgb2spec: OnceLock<wgpu::Buffer>,
    /// Bound in place of a table a stage does not read.
    empty: wgpu::Buffer,
}

/// One entry point in `pixel.wgsl`, in pipeline order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kernel {
    Exposure,
    DensityParametric,
    DensitySpectral,
    DyeScan,
    Print,
    Encode,
}

const KERNEL_COUNT: usize = 6;

impl Kernel {
    const ALL: [Self; KERNEL_COUNT] = [
        Self::Exposure,
        Self::DensityParametric,
        Self::DensitySpectral,
        Self::DyeScan,
        Self::Print,
        Self::Encode,
    ];

    fn entry_point(self) -> &'static str {
        match self {
            Self::Exposure => "exposure",
            Self::DensityParametric => "density_parametric",
            Self::DensitySpectral => "density_spectral",
            Self::DyeScan => "dye_scan",
            Self::Print => "print_scan",
            Self::Encode => "encode",
        }
    }
}

impl std::fmt::Debug for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gpu")
            .field("adapter", &self.inner.adapter)
            .finish()
    }
}

impl Gpu {
    /// Pick the highest-performance adapter and compile the kernels.
    pub async fn new() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|error| GpuError::NoAdapter(error.to_string()))?;
        let available = adapter.limits();
        let required_limits = wgpu::Limits {
            max_storage_buffer_binding_size: available.max_storage_buffer_binding_size.min(1 << 30),
            max_buffer_size: available.max_buffer_size.min(1 << 30),
            ..wgpu::Limits::defaults()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rawstock"),
                required_limits: required_limits.clone(),
                ..Default::default()
            })
            .await
            .map_err(|error| GpuError::Device(error.to_string()))?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pixel.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pixel.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("film stages"),
            entries: &[
                buffer_entry(0, wgpu::BufferBindingType::Uniform),
                buffer_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                buffer_entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer_entry(3, wgpu::BufferBindingType::Storage { read_only: true }),
                buffer_entry(4, wgpu::BufferBindingType::Storage { read_only: true }),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("film stages"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let kernels = Kernel::ALL.map(|kernel| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(kernel.entry_point()),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(kernel.entry_point()),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let binding = required_limits
            .max_storage_buffer_binding_size
            .min(required_limits.max_buffer_size);
        // Two dispatch dimensions of 65535 workgroups cover far more than a
        // binding holds, so the binding size is the only chunk limit.
        let max_chunk_pixels = (binding / PIXEL_BYTES).min(u64::from(u32::MAX)) as u32;
        let empty = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("unused table"),
            contents: bytemuck::bytes_of(&0.0f32),
            usage: wgpu::BufferUsages::STORAGE,
        });
        Ok(Self {
            inner: Arc::new(Inner {
                device,
                queue,
                layout,
                kernels,
                adapter: adapter.get_info().name,
                max_chunk_pixels,
                rgb2spec: OnceLock::new(),
                empty,
            }),
        })
    }

    /// [`Self::new`], blocking the calling thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_blocking() -> Result<Self, GpuError> {
        pollster::block_on(Self::new())
    }

    /// The adapter's name, for logs and the CLI.
    pub fn adapter_name(&self) -> &str {
        &self.inner.adapter
    }

    /// The rgb2spec z scale, then its coefficient cube.
    fn rgb2spec(&self) -> &wgpu::Buffer {
        self.inner.rgb2spec.get_or_init(|| {
            let (_, scale, data, _) = crate::spectrum::gpu_upsample();
            let mut table = Vec::with_capacity(scale.len() + data.len());
            table.extend_from_slice(scale);
            table.extend_from_slice(data);
            self.inner
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("rgb2spec"),
                    contents: bytemuck::cast_slice(&table),
                    usage: wgpu::BufferUsages::STORAGE,
                })
        })
    }

    #[cfg(test)]
    fn with_chunk_limit(&self, pixels: u32) -> Self {
        let inner = &self.inner;
        Self {
            inner: Arc::new(Inner {
                device: inner.device.clone(),
                queue: inner.queue.clone(),
                layout: inner.layout.clone(),
                kernels: inner.kernels.clone(),
                adapter: inner.adapter.clone(),
                max_chunk_pixels: pixels,
                rgb2spec: inner.rgb2spec.clone(),
                empty: inner.empty.clone(),
            }),
        }
    }
}

fn buffer_entry(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// `Params` in `pixel.wgsl`. Every member is 16 bytes, so the layout is the
/// same under WGSL uniform rules.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct StageParams {
    counts: [u32; 4],
    scalars: [f32; 4],
    coupling: [[f32; 4]; 3],
    srgb: [[f32; 4]; 3],
    curve_offset: [u32; 4],
    curve_len: [u32; 4],
    spectral: [u32; 4],
    scan_balance: [f32; 4],
    ap1_to_ap0: [[f32; 4]; 3],
}

/// The film pipeline on a [`Gpu`], for one set of [`Parameters`].
pub struct GpuPipeline {
    gpu: Gpu,
    knots: wgpu::Buffer,
    /// Spectral stock tables, or the empty buffer.
    spectral: wgpu::Buffer,
    /// The rgb2spec table, or the empty buffer.
    rgb2spec: wgpu::Buffer,
    params: StageParams,
    sequence: Vec<Kernel>,
}

impl std::fmt::Debug for GpuPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuPipeline")
            .field("gpu", &self.gpu)
            .finish_non_exhaustive()
    }
}

impl GpuPipeline {
    /// Upload `parameters` to `gpu`.
    ///
    /// Fails with [`GpuError::Unsupported`] when the parameters need a stage
    /// this slice has not ported.
    pub fn new(gpu: &Gpu, parameters: &Parameters) -> Result<Self, GpuError> {
        if parameters.halation().is_some() {
            return Err(GpuError::Unsupported(Stage::Halation));
        }
        if parameters.grain().is_some() {
            return Err(GpuError::Unsupported(Stage::Grain));
        }
        let (film_curves, coupling) = match parameters.film() {
            FilmModel::Parametric(film) => (film.curves(), film.coupling()),
            FilmModel::Spectral(film) => (film.curves(), film.coupling()),
        };
        let curves = [
            &film_curves[0],
            &film_curves[1],
            &film_curves[2],
            parameters.print_curve(),
        ];
        let mut knots = Vec::new();
        let mut curve_offset = [0u32; 4];
        let mut curve_len = [0u32; 4];
        for (index, curve) in curves.into_iter().enumerate() {
            curve_offset[index] = knots.len() as u32;
            curve_len[index] = append_curve(&mut knots, curve);
        }
        let matrix = coupling.unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let srgb = crate::matrices::acescg_to_linear_srgb();
        let mut params = StageParams {
            counts: [0, u32::from(coupling.is_some()), 0, 0],
            scalars: [
                2.0f32.powf(parameters.exposure_ev()),
                parameters.print_exposure(),
                0.0,
                0.0,
            ],
            coupling: matrix.map(|row| [row[0], row[1], row[2], 0.0]),
            srgb: srgb.map(|row| [row[0] as f32, row[1] as f32, row[2] as f32, 0.0]),
            curve_offset,
            curve_len,
            spectral: [0; 4],
            scan_balance: [0.0; 4],
            ap1_to_ap0: [[0.0; 4]; 3],
        };
        let device = &gpu.inner.device;
        let mut sequence = vec![Kernel::Exposure];
        let (spectral, rgb2spec) = match parameters.film() {
            FilmModel::Parametric(_) => {
                sequence.push(Kernel::DensityParametric);
                (gpu.inner.empty.clone(), gpu.inner.empty.clone())
            }
            FilmModel::Spectral(film) => {
                let tables = film.gpu_tables();
                let count = tables.wavelengths.len();
                let mut values = Vec::with_capacity(count * 10);
                values.extend_from_slice(tables.wavelengths);
                for layer in tables.weighted_sensitivity {
                    values.extend_from_slice(layer);
                }
                match tables.scan {
                    Some(scan) => {
                        for layer in scan.dyes.iter().chain(scan.observer) {
                            values.extend_from_slice(layer);
                        }
                        let balance = scan.balance;
                        params.scan_balance = [balance[0], balance[1], balance[2], 0.0];
                    }
                    None => values.resize(count * 10, 0.0),
                }
                let (res, _, _, ap0) = crate::spectrum::gpu_upsample();
                params.spectral = [count as u32, u32::from(tables.scan.is_some()), res, 0];
                params.ap1_to_ap0 = ap0.map(|row| [row[0], row[1], row[2], 0.0]);
                sequence.push(Kernel::DensitySpectral);
                if tables.scan.is_some() {
                    sequence.push(Kernel::DyeScan);
                }
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("spectral tables"),
                    contents: bytemuck::cast_slice(&values),
                    usage: wgpu::BufferUsages::STORAGE,
                });
                (buffer, gpu.rgb2spec().clone())
            }
        };
        sequence.extend([Kernel::Print, Kernel::Encode]);
        let knots = gpu
            .inner
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("curve knots"),
                contents: bytemuck::cast_slice(&knots),
                usage: wgpu::BufferUsages::STORAGE,
            });
        Ok(Self {
            gpu: gpu.clone(),
            knots,
            spectral,
            rgb2spec,
            params,
            sequence,
        })
    }

    /// Exposure, the film model, print/scan, and the sRGB encode.
    ///
    /// The same stages and order as [`Pipeline::develop`](crate::render::Pipeline::develop),
    /// within the documented tolerance. The result is always
    /// [`Fidelity::Exact`].
    pub async fn develop(&self, image: &LinearImage) -> Result<SrgbImage, GpuError> {
        let pixels = self.run(&self.sequence, image.pixels()).await?;
        Ok(SrgbImage::from_encoded(
            image.width(),
            image.height(),
            pixels,
            Fidelity::Exact,
        ))
    }

    /// [`Self::develop`], blocking the calling thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn develop_blocking(&self, image: &LinearImage) -> Result<SrgbImage, GpuError> {
        pollster::block_on(self.develop(image))
    }

    /// Run `kernels`, in order, over interleaved RGB `samples`.
    ///
    /// Per-pixel stages do not read neighbours, so the frame is split into
    /// chunks that fit one storage binding.
    pub(crate) async fn run(
        &self,
        kernels: &[Kernel],
        samples: &[f32],
    ) -> Result<Vec<f32>, GpuError> {
        let gpu = &self.gpu.inner;
        let pixel_count = samples.len() / 3;
        let chunk_pixels = (gpu.max_chunk_pixels as usize).max(1);
        let mut out = Vec::with_capacity(pixel_count * 3);
        for chunk in samples[..pixel_count * 3].chunks(chunk_pixels * 3) {
            out.extend(self.run_chunk(kernels, chunk).await?);
        }
        Ok(out)
    }

    async fn run_chunk(&self, kernels: &[Kernel], samples: &[f32]) -> Result<Vec<f32>, GpuError> {
        let gpu = &self.gpu.inner;
        let count = (samples.len() / 3) as u32;
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut params = self.params;
        params.counts[0] = count;
        let uniform = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("stage params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let data = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("pixels"),
                contents: bytemuck::cast_slice(samples),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            });
        let size = data.size();
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("film stages"),
            layout: &gpu.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.knots.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: data.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.spectral.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.rgb2spec.as_entire_binding(),
                },
            ],
        });
        let (groups_x, groups_y) = dispatch_size(count);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("develop"),
            });
        for kernel in kernels {
            // One pass per stage, so each stage sees the previous one's writes.
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(kernel.entry_point()),
                timestamp_writes: None,
            });
            pass.set_pipeline(&gpu.kernels[*kernel as usize]);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(groups_x, groups_y, 1);
        }
        encoder.copy_buffer_to_buffer(&data, 0, &readback, 0, size);
        gpu.queue.submit([encoder.finish()]);
        read_back(&gpu.device, &readback).await
    }
}

/// Workgroups for `count` pixels. `pixel_index` in `pixel.wgsl` undoes this.
fn dispatch_size(count: u32) -> (u32, u32) {
    let groups = count.div_ceil(WORKGROUP);
    let x = groups.min(65_535);
    (x, groups.div_ceil(x))
}

/// Log-exposures, then densities. Returns the knot count.
fn append_curve(knots: &mut Vec<f32>, curve: &Curve) -> u32 {
    let points: Vec<(f32, f32)> = curve.points().collect();
    knots.extend(points.iter().map(|point| point.0));
    knots.extend(points.iter().map(|point| point.1));
    points.len() as u32
}

async fn read_back(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<f32>, GpuError> {
    let mapped = MapFuture::default();
    let signal = mapped.clone();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        signal.complete(result)
    });
    // Native backends need a poll to run the callback. On the web the
    // browser runs it, and this poll does nothing.
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| GpuError::Readback(error.to_string()))?;
    mapped
        .await
        .map_err(|error| GpuError::Readback(error.to_string()))?;
    let samples = {
        let view = buffer
            .get_mapped_range(..)
            .map_err(|error| GpuError::Readback(error.to_string()))?;
        bytemuck::cast_slice::<u8, f32>(&view).to_vec()
    };
    buffer.unmap();
    Ok(samples)
}

/// Resolves when a `map_async` callback fires.
#[derive(Clone, Default)]
struct MapFuture {
    state: Arc<Mutex<MapState>>,
}

#[derive(Default)]
struct MapState {
    result: Option<Result<(), wgpu::BufferAsyncError>>,
    waker: Option<Waker>,
}

impl MapFuture {
    fn complete(&self, result: Result<(), wgpu::BufferAsyncError>) {
        let mut state = self.state.lock().expect("map state lock");
        state.result = Some(result);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

impl Future for MapFuture {
    type Output = Result<(), wgpu::BufferAsyncError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.lock().expect("map state lock");
        match state.result.take() {
            Some(result) => Poll::Ready(result),
            None => {
                state.waker = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}
