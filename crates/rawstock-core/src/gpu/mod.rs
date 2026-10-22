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
//! Every stage runs here: exposure, halation, both film models, dye-cloud
//! grain, the exact dye scan, print/scan, and the sRGB encode.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::color::LinearImage;
use crate::render::{Fidelity, Parameters, SrgbImage};
use crate::stock::{Curve, FilmModel, Grain, GrainRegion, Halation};

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
    /// Halation needs one storage binding for the whole frame, or a grain
    /// band for one row does not fit one binding.
    #[error("a {width}×{height} frame is too large for the GPU path")]
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
    grain_layout: wgpu::BindGroupLayout,
    /// `grain_field`, `scan_horizontal`, `scan_vertical`, `grain_mix`.
    grain_kernels: [wgpu::ComputePipeline; 4],
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
        let grain_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grain.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grain.wgsl").into()),
        });
        let grain_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grain"),
            entries: &[
                buffer_entry(0, wgpu::BufferBindingType::Uniform),
                buffer_entry(1, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer_entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer_entry(3, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer_entry(4, wgpu::BufferBindingType::Storage { read_only: true }),
            ],
        });
        let grain_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("grain"),
                bind_group_layouts: &[Some(&grain_layout)],
                immediate_size: 0,
            });
        let grain_kernels = [
            "grain_field",
            "scan_horizontal",
            "scan_vertical",
            "grain_mix",
        ]
        .map(|entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&grain_pipeline_layout),
                module: &grain_module,
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
                grain_layout,
                grain_kernels,
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
                grain_layout: inner.grain_layout.clone(),
                grain_kernels: inner.grain_kernels.clone(),
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
    grain: Option<Grain>,
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
    pub fn new(gpu: &Gpu, parameters: &Parameters) -> Self {
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
        Self {
            gpu: gpu.clone(),
            knots,
            spectral,
            rgb2spec,
            params,
            sequence,
            halation: parameters.halation().cloned(),
            grain: parameters.grain().cloned(),
        }
    }

    /// Exposure, halation, the film model, grain, print/scan, and the sRGB
    /// encode.
    ///
    /// The same stages and order as [`Pipeline::develop`](crate::render::Pipeline::develop),
    /// within the documented tolerance. The result is always
    /// [`Fidelity::Exact`].
    pub async fn develop(&self, image: &LinearImage) -> Result<SrgbImage, GpuError> {
        let (width, height) = (image.width(), image.height());
        let stages = Stages {
            pre: &self.sequence[..1],
            halation: self
                .halation
                .as_ref()
                .map(|halation| (halation, halation.sigma_px(width, height))),
            mid: &self.sequence[1..2],
            grain: self.grain.as_ref().map(|grain| {
                (
                    grain,
                    GrainRegion {
                        width,
                        height,
                        origin_x: 0,
                        origin_y: 0,
                        frame_width: width,
                    },
                )
            }),
            post: &self.sequence[2..],
        };
        let pixels = self
            .run_frame(image.pixels(), width, height, stages)
            .await?;
        Ok(SrgbImage::from_encoded(
            width,
            height,
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
        let chunks = self.upload(samples, gpu.max_chunk_pixels as usize);
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

    /// Run `stages` over a `width` × `height` frame of interleaved RGB.
    ///
    /// Chunks are whole rows. Halation reads the whole frame between `pre`
    /// and `mid`. Grain reads a padded band around each chunk between `mid`
    /// and `post`.
    pub(crate) async fn run_frame(
        &self,
        samples: &[f32],
        width: u32,
        height: u32,
        stages: Stages<'_>,
    ) -> Result<Vec<f32>, GpuError> {
        let gpu = &self.gpu.inner;
        let pixels = u64::from(width) * u64::from(height);
        if width == 0 || height == 0 || samples.len() as u64 != pixels * 3 {
            return self.run(stages.pre, samples).await;
        }
        let grain = stages
            .grain
            .filter(|(_, region)| region.frame_width > 0)
            .map(|(grain, region)| GrainUpload::new(gpu, grain, region));
        let pad = grain.as_ref().map_or(0, |grain| u64::from(grain.plan.pad));
        let band_row = (u64::from(width) + 2 * pad) * PIXEL_BYTES;
        let rows_by_band = (gpu.max_binding / band_row).saturating_sub(2 * pad);
        let rows_by_data = u64::from(gpu.max_chunk_pixels) / u64::from(width);
        let rows = rows_by_band.min(rows_by_data).clamp(1, u64::from(height));
        if stages.halation.is_some() && pixels * 4 > gpu.max_binding
            || (2 * pad + 1) * band_row > gpu.max_binding
        {
            return Err(GpuError::FrameTooLarge { width, height });
        }
        let chunks = self.upload(samples, (rows * u64::from(width)) as usize);
        let halation = stages
            .halation
            .map(|(halation, sigma)| HalationUpload::new(gpu, halation, sigma, width, height));
        let band = grain.as_ref().map(|_| {
            let size = (rows + 2 * pad) * band_row;
            let buffer = |label| {
                gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            };
            (buffer("grain field"), buffer("grain scratch"))
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("develop"),
            });
        for chunk in &chunks {
            self.encode_film(&mut encoder, chunk, stages.pre);
            if let Some(halation) = &halation {
                halation.threshold(gpu, &mut encoder, chunk);
            }
        }
        if let Some(halation) = &halation {
            halation.widen(gpu, &mut encoder, &chunks[0]);
        }
        for chunk in &chunks {
            if let Some(halation) = &halation {
                halation.add(gpu, &mut encoder, chunk);
            }
            self.encode_film(&mut encoder, chunk, stages.mid);
            if let (Some(grain), Some((field, scratch))) = (&grain, &band) {
                grain.apply(gpu, &mut encoder, chunk, width, field, scratch);
            }
            self.encode_film(&mut encoder, chunk, stages.post);
        }
        self.finish(encoder, &chunks).await
    }

    /// Split `samples` into chunks that fit one storage binding and upload
    /// them. Per-pixel stages do not read neighbours, so the split cannot
    /// change their result.
    fn upload(&self, samples: &[f32], chunk_pixels: usize) -> Vec<Chunk> {
        let gpu = &self.gpu.inner;
        let pixel_count = samples.len() / 3;
        let chunk_pixels = chunk_pixels.max(1);
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

/// Which stages [`GpuPipeline::run_frame`] runs, in order.
pub(crate) struct Stages<'a> {
    pub pre: &'a [Kernel],
    /// The halation and its sigma in pixels.
    pub halation: Option<(&'a Halation, f32)>,
    pub mid: &'a [Kernel],
    /// The grain and where this frame sits in the full frame.
    pub grain: Option<(&'a Grain, GrainRegion)>,
    pub post: &'a [Kernel],
}

/// Halation's full-frame planes and constants for one run.
struct HalationUpload {
    excess: wgpu::Buffer,
    scratch: wgpu::Buffer,
    params: HalationParams,
    sigma: f32,
}

impl HalationUpload {
    fn new(gpu: &Inner, halation: &Halation, sigma: f32, width: u32, height: u32) -> Self {
        let plane = |label| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (u64::from(width) * u64::from(height) * 4).max(4),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let weight = halation.weight();
        Self {
            excess: plane("halation excess"),
            scratch: plane("halation scratch"),
            params: HalationParams {
                dims: [width, height, 0, 0],
                scalars: [halation.threshold(), weight[0], weight[1], weight[2]],
                counts: [0; 4],
            },
            sigma,
        }
    }

    fn group(&self, gpu: &Inner, chunk: &Chunk, radius: u32) -> wgpu::BindGroup {
        let mut params = self.params;
        params.dims[2] = radius;
        params.dims[3] = chunk.offset;
        params.counts[0] = chunk.count;
        let uniform = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("halation params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        bind(
            gpu,
            &gpu.halation_layout,
            &[&uniform, &chunk.data, &self.excess, &self.scratch],
        )
    }

    /// The excess over the threshold, for one chunk.
    fn threshold(&self, gpu: &Inner, encoder: &mut wgpu::CommandEncoder, chunk: &Chunk) {
        let group = self.group(gpu, chunk, 0);
        dispatch(
            encoder,
            &gpu.halation_kernels[0],
            &group,
            dispatch_size(chunk.count),
        );
    }

    /// Kovesi's box passes over the whole excess plane. `widen` leaves the
    /// excess unblurred below half a pixel.
    fn widen(&self, gpu: &Inner, encoder: &mut wgpu::CommandEncoder, any: &Chunk) {
        let [width, height, ..] = self.params.dims;
        if !self.sigma.is_finite() || self.sigma < 0.5 {
            return;
        }
        let tiles = (width.div_ceil(16), height.div_ceil(16));
        for radius in crate::stock::box_radii(self.sigma) {
            if radius == 0 {
                continue;
            }
            let group = self.group(gpu, any, radius as u32);
            dispatch(encoder, &gpu.halation_kernels[1], &group, tiles);
            dispatch(encoder, &gpu.halation_kernels[2], &group, tiles);
        }
    }

    /// The widened excess, weighted, back onto one chunk.
    fn add(&self, gpu: &Inner, encoder: &mut wgpu::CommandEncoder, chunk: &Chunk) {
        let group = self.group(gpu, chunk, 0);
        dispatch(
            encoder,
            &gpu.halation_kernels[3],
            &group,
            dispatch_size(chunk.count),
        );
    }
}

/// `Params` in `grain.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GrainParams {
    band: [u32; 4],
    origin: [i32; 4],
    scalars: [f32; 4],
    mean: [f32; 4],
    spread: [f32; 4],
    counts: [u32; 4],
    rms_offset: [u32; 4],
    rms_len: [u32; 4],
}

/// The grain plan and its tables for one run.
struct GrainUpload {
    plan: crate::grain::GpuPlan,
    region: GrainRegion,
    tables: wgpu::Buffer,
    params: GrainParams,
}

impl GrainUpload {
    fn new(gpu: &Inner, grain: &Grain, region: GrainRegion) -> Self {
        let plan = crate::grain::FilmGrain::new(grain.frame_mm(), grain.seed(), grain.size_mm())
            .gpu_plan(region.frame_width);
        let mut tables = Vec::new();
        for process in &plan.processes {
            tables.extend([
                process.cell_mm,
                process.poisson_limit,
                process.sigma_lo,
                process.sigma_hi,
                process.aspect_max,
                process.amplitude,
                process.reach_mm,
                f32::from_bits(process.seed),
                f32::from_bits(process.layers),
                f32::from_bits(u32::from(process.resolved)),
                0.0,
                0.0,
            ]);
        }
        let kernel_offset = tables.len() as u32;
        tables.extend_from_slice(&plan.kernel);
        let mut rms_offset = [0u32; 4];
        let mut rms_len = [0u32; 4];
        for (layer, granularity) in grain.layers().iter().enumerate() {
            rms_offset[layer] = tables.len() as u32;
            let points: Vec<(f32, f32)> = granularity.points().collect();
            tables.extend(points.iter().map(|point| point.0));
            tables.extend(points.iter().map(|point| point.1));
            rms_len[layer] = points.len() as u32;
        }
        let pad = plan.pad;
        let params = GrainParams {
            band: [0, 0, pad, region.width],
            origin: [0, 0, plan.processes.len() as i32, i32::from(plan.blur)],
            scalars: [
                plan.pitch,
                grain.cell_px(region.frame_width).min(1.0).sqrt() / 1000.0,
                0.0,
                0.0,
            ],
            mean: [plan.mean[0], plan.mean[1], plan.mean[2], 0.0],
            spread: [plan.spread[0], plan.spread[1], plan.spread[2], 0.0],
            counts: [0, kernel_offset, 0, 0],
            rms_offset,
            rms_len,
        };
        let tables = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grain tables"),
                contents: bytemuck::cast_slice(&tables),
                usage: wgpu::BufferUsages::STORAGE,
            });
        Self {
            plan,
            region,
            tables,
            params,
        }
    }

    /// Field, scanner blur, and mix for one chunk of whole rows.
    fn apply(
        &self,
        gpu: &Inner,
        encoder: &mut wgpu::CommandEncoder,
        chunk: &Chunk,
        width: u32,
        field: &wgpu::Buffer,
        scratch: &wgpu::Buffer,
    ) {
        let pad = self.plan.pad;
        let rows = chunk.count / width;
        let first_row = chunk.offset / width;
        let mut params = self.params;
        params.band[0] = width + 2 * pad;
        params.band[1] = rows + 2 * pad;
        params.origin[0] = self.region.origin_x as i32 - pad as i32;
        params.origin[1] = (self.region.origin_y + first_row) as i32 - pad as i32;
        params.counts[0] = chunk.count;
        let uniform = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grain params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let group = bind(
            gpu,
            &gpu.grain_layout,
            &[&uniform, &chunk.data, field, scratch, &self.tables],
        );
        let tiles = (params.band[0].div_ceil(16), params.band[1].div_ceil(16));
        dispatch(encoder, &gpu.grain_kernels[0], &group, tiles);
        if self.plan.blur {
            dispatch(encoder, &gpu.grain_kernels[1], &group, tiles);
            dispatch(encoder, &gpu.grain_kernels[2], &group, tiles);
        }
        dispatch(
            encoder,
            &gpu.grain_kernels[3],
            &group,
            dispatch_size(chunk.count),
        );
    }
}

/// A bind group of whole buffers at bindings 0, 1, 2, and so on.
fn bind(gpu: &Inner, layout: &wgpu::BindGroupLayout, buffers: &[&wgpu::Buffer]) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .enumerate()
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding: binding as u32,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &entries,
    })
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
