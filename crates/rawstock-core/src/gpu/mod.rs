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
//! Exposure, halation, both film models, the exact dye scan, print/scan, and
//! the sRGB encode run here. A stock with grain is refused with
//! [`GpuError::Unsupported`] until those stages are ported.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::color::LinearImage;
use crate::render::{Fidelity, Parameters, SrgbImage, Stage};
use crate::stock::{Curve, FilmModel, Halation};

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
    /// Halation needs one storage binding for the whole frame, and this
    /// frame does not fit.
    #[error("a {width}×{height} frame is too large for one GPU halation pass")]
    FrameTooLarge {
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
    },
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
    halation_layout: wgpu::BindGroupLayout,
    /// `threshold`, `box_horizontal`, `box_vertical`, `add_glow`.
    halation_kernels: [wgpu::ComputePipeline; 4],
    adapter: String,
    max_chunk_pixels: u32,
    /// Bytes in the largest storage binding.
    max_binding: u64,
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
        let halation_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("halation.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("halation.wgsl").into()),
        });
        let halation_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("halation"),
            entries: &[
                buffer_entry(0, wgpu::BufferBindingType::Uniform),
                buffer_entry(1, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer_entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer_entry(3, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let halation_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("halation"),
                bind_group_layouts: &[Some(&halation_layout)],
                immediate_size: 0,
            });
        let halation_kernels =
            ["threshold", "box_horizontal", "box_vertical", "add_glow"].map(|entry_point| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry_point),
                    layout: Some(&halation_pipeline_layout),
                    module: &halation_module,
                    entry_point: Some(entry_point),
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
                halation_layout,
                halation_kernels,
                adapter: adapter.get_info().name,
                max_chunk_pixels,
                max_binding: binding,
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
                halation_layout: inner.halation_layout.clone(),
                halation_kernels: inner.halation_kernels.clone(),
                adapter: inner.adapter.clone(),
                max_chunk_pixels: pixels,
                max_binding: inner.max_binding,
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
    halation: Option<Halation>,
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
            halation: parameters.halation().cloned(),
        })
    }

    /// Exposure, halation, the film model, print/scan, and the sRGB encode.
    ///
    /// The same stages and order as [`Pipeline::develop`](crate::render::Pipeline::develop),
    /// within the documented tolerance. The result is always
    /// [`Fidelity::Exact`].
    pub async fn develop(&self, image: &LinearImage) -> Result<SrgbImage, GpuError> {
        let pixels = match &self.halation {
            None => self.run(&self.sequence, image.pixels()).await?,
            Some(halation) => {
                let sigma = halation.sigma_px(image.width(), image.height());
                self.run_with_halation(halation, sigma, image, &self.sequence[1..])
                    .await?
            }
        };
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

    /// Run per-pixel `kernels`, in order, over interleaved RGB `samples`.
    pub(crate) async fn run(
        &self,
        kernels: &[Kernel],
        samples: &[f32],
    ) -> Result<Vec<f32>, GpuError> {
        let gpu = &self.gpu.inner;
        let chunks = self.upload(samples);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("develop"),
            });
        for chunk in &chunks {
            self.encode_film(&mut encoder, chunk, kernels);
        }
        self.finish(encoder, &chunks).await
    }

    /// Exposure, halation over the whole frame at `sigma` pixels, then the
    /// per-pixel `post` kernels.
    pub(crate) async fn run_with_halation(
        &self,
        halation: &Halation,
        sigma: f32,
        image: &LinearImage,
        post: &[Kernel],
    ) -> Result<Vec<f32>, GpuError> {
        let gpu = &self.gpu.inner;
        let (width, height) = (image.width(), image.height());
        let pixels = u64::from(width) * u64::from(height);
        if pixels * 4 > gpu.max_binding {
            return Err(GpuError::FrameTooLarge { width, height });
        }
        let chunks = self.upload(image.pixels());
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let plane = |label| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (pixels * 4).max(4),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let excess = plane("halation excess");
        let scratch = plane("halation scratch");
        let weight = halation.weight();
        let glow = |chunk: &Chunk, radius: u32| {
            let params = HalationParams {
                dims: [width, height, radius, chunk.offset],
                scalars: [halation.threshold(), weight[0], weight[1], weight[2]],
                counts: [chunk.count, 0, 0, 0],
            };
            let uniform = gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("halation params"),
                    contents: bytemuck::bytes_of(&params),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("halation"),
                layout: &gpu.halation_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: chunk.data.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: excess.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: scratch.as_entire_binding(),
                    },
                ],
            })
        };
        let pre = [Kernel::Exposure];
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("develop with halation"),
            });
        for chunk in &chunks {
            self.encode_film(&mut encoder, chunk, &pre);
            let group = glow(chunk, 0);
            let (x, y) = dispatch_size(chunk.count);
            dispatch(&mut encoder, &gpu.halation_kernels[0], &group, (x, y));
        }
        // `widen` leaves the excess unblurred below half a pixel.
        if sigma.is_finite() && sigma >= 0.5 && width > 0 && height > 0 {
            let tiles = (width.div_ceil(16), height.div_ceil(16));
            for radius in crate::stock::box_radii(sigma) {
                if radius == 0 {
                    continue;
                }
                let group = glow(&chunks[0], radius as u32);
                dispatch(&mut encoder, &gpu.halation_kernels[1], &group, tiles);
                dispatch(&mut encoder, &gpu.halation_kernels[2], &group, tiles);
            }
        }
        for chunk in &chunks {
            let group = glow(chunk, 0);
            let (x, y) = dispatch_size(chunk.count);
            dispatch(&mut encoder, &gpu.halation_kernels[3], &group, (x, y));
            self.encode_film(&mut encoder, chunk, post);
        }
        self.finish(encoder, &chunks).await
    }

    /// Split `samples` into chunks that fit one storage binding and upload
    /// them. Per-pixel stages do not read neighbours, so the split cannot
    /// change their result.
    fn upload(&self, samples: &[f32]) -> Vec<Chunk> {
        let gpu = &self.gpu.inner;
        let pixel_count = samples.len() / 3;
        let chunk_pixels = (gpu.max_chunk_pixels as usize).max(1);
        samples[..pixel_count * 3]
            .chunks(chunk_pixels * 3)
            .enumerate()
            .map(|(index, samples)| {
                let count = (samples.len() / 3) as u32;
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
                let film = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                Chunk {
                    data,
                    film,
                    count,
                    offset: (index * chunk_pixels) as u32,
                }
            })
            .collect()
    }

    fn encode_film(&self, encoder: &mut wgpu::CommandEncoder, chunk: &Chunk, kernels: &[Kernel]) {
        let size = dispatch_size(chunk.count);
        for kernel in kernels {
            let pipeline = &self.gpu.inner.kernels[*kernel as usize];
            dispatch(encoder, pipeline, &chunk.film, size);
        }
    }

    /// Submit `encoder` and read every chunk back, in order.
    async fn finish(
        &self,
        mut encoder: wgpu::CommandEncoder,
        chunks: &[Chunk],
    ) -> Result<Vec<f32>, GpuError> {
        let gpu = &self.gpu.inner;
        let readbacks: Vec<wgpu::Buffer> = chunks
            .iter()
            .map(|chunk| {
                let size = chunk.data.size();
                let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                encoder.copy_buffer_to_buffer(&chunk.data, 0, &readback, 0, size);
                readback
            })
            .collect();
        gpu.queue.submit([encoder.finish()]);
        let mut out = Vec::new();
        for readback in &readbacks {
            out.extend(read_back(&gpu.device, readback).await?);
        }
        Ok(out)
    }
}

/// One upload of interleaved RGB, at most one storage binding long.
struct Chunk {
    data: wgpu::Buffer,
    film: wgpu::BindGroup,
    count: u32,
    /// The chunk's first pixel in the frame.
    offset: u32,
}

/// `Params` in `halation.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HalationParams {
    dims: [u32; 4],
    scalars: [f32; 4],
    counts: [u32; 4],
}

/// One compute pass, so the next dispatch sees this one's writes.
fn dispatch(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    group: &wgpu::BindGroup,
    (x, y): (u32, u32),
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: None,
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.dispatch_workgroups(x, y, 1);
}

/// Workgroups for `count` pixels. `pixel_index` in `pixel.wgsl` undoes this.
fn dispatch_size(count: u32) -> (u32, u32) {
    let groups = count.div_ceil(WORKGROUP).max(1);
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
