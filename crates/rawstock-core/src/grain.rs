//! Scanner-filtered dye-cloud grain.
//!
//! Real color-negative grain is a Poisson field of soft dye clouds in each
//! emulsion layer. At a scan of Portra the clouds are a few pixels across,
//! they overlap, and the three layers are read together, so the grain is
//! mostly luminance with a fine chroma residual. This module is that field:
//! the high-overlap limit of the Boolean model in Newson, Delon, and Galerne
//! (Computer Graphics Forum 36(8), 2017), evaluated in millimetres, then
//! filtered by a scanner aperture.
//!
//! The implementation is original. It does not copy darktable, spektrafilm,
//! or the Newson reference renderer.

use crate::render::{Cancelled, DevelopControl};

/// Gaussian dye-cloud field for one frame.
///
/// `size_mm` is the primary dye-cloud sigma of each layer, red, green, then
/// blue. Shared clouds use the geometric mean, so a clump is the same shape
/// in every layer. Each layer also keeps a small private field at its own
/// size. That is the chroma.
pub(crate) struct FilmGrain {
    frame_mm: f32,
    seed: u32,
    size_mm: [f32; 3],
}

impl FilmGrain {
    pub(crate) fn new(frame_mm: f32, seed: u32, size_mm: [f32; 3]) -> Self {
        Self {
            frame_mm,
            seed,
            size_mm,
        }
    }

    /// Zero-mean unit-variance grain for `plane`, one sample per density.
    ///
    /// The pattern does not depend on the picture. RMS is applied later, per
    /// pixel, so a hard edge does not grow extra grain. A crop uses the full
    /// frame's pitch and is padded by the scanner kernel, so those pixels
    /// match the full frame.
    pub(crate) fn render(
        &self,
        plane: Plane,
        control: &mut dyn DevelopControl,
    ) -> Result<Vec<f32>, Cancelled> {
        let pitch = self.frame_mm / plane.frame_width as f32;
        let processes = Process::table(self.size_mm);
        let scanner = Scanner::at_pitch(pitch);
        let blur = scanner.active() && processes.iter().all(|process| process.resolved(pitch));
        let pad = if blur { scanner.pad() } else { 0 };
        let origin_x = plane.origin_x as i32 - pad;
        let origin_y = plane.origin_y as i32 - pad;
        let padded_w = plane.width as usize + pad as usize * 2;
        let padded_h = plane.rows + pad as usize * 2;
        // f64: a coarse pixel (a tiny preview, or a 1×1 measurement) holds
        // every cloud in the frame. Those masses are far below the ulp of an
        // f32 running sum, and the lost mass is a density bias.
        let mut field = vec![0.0_f64; padded_h * padded_w * 3];
        let target = Target {
            width: padded_w,
            rows: padded_h,
            origin_x,
            origin_y,
            pitch,
        };
        let mut mean = [0.0_f32; 3];
        let mut variance = [0.0_f32; 3];
        for process in &processes {
            let (process_mean, process_variance) = process.splat(
                &mut field,
                target,
                self.seed,
                blur.then_some(scanner),
                control,
            )?;
            for layer in 0..3 {
                mean[layer] += process_mean[layer];
                variance[layer] += process_variance[layer];
            }
        }
        if blur {
            scanner.blur(&mut field, padded_w, padded_h, control)?;
        }
        let mut out = vec![0.0_f32; plane.rows * plane.width as usize * 3];
        for y in 0..plane.rows {
            let src = ((y + pad as usize) * padded_w + pad as usize) * 3;
            let dst = y * plane.width as usize * 3;
            let count = plane.width as usize * 3;
            for i in 0..count {
                out[dst + i] = field[src + i] as f32;
            }
        }
        for (index, sample) in out.iter_mut().enumerate() {
            let spread = variance[index % 3].sqrt();
            if spread == 0.0 {
                *sample = 0.0;
            } else {
                *sample = (*sample - mean[index % 3]) / spread;
            }
        }
        Ok(out)
    }
}

/// What the GPU needs to evaluate [`FilmGrain::render`] for one frame width.
#[cfg(feature = "gpu")]
pub(crate) struct GpuPlan {
    pub pitch: f32,
    /// Whether the scanner aperture blurs the field. Implies every process
    /// is resolved.
    pub blur: bool,
    /// Rows and columns of field computed past each crop edge for the blur.
    pub pad: u32,
    /// Normalized scanner taps, `2 * pad + 1` of them, when `blur`.
    pub kernel: Vec<f32>,
    pub processes: Vec<GpuProcess>,
    /// Subtracted from the summed field, then divided by `spread`.
    pub mean: [f32; 3],
    pub spread: [f32; 3],
}

/// One active Poisson process, with its constants already evaluated.
#[cfg(feature = "gpu")]
pub(crate) struct GpuProcess {
    pub cell_mm: f32,
    /// `exp(-mu)`, so Knuth's generator compares against the CPU's bits.
    pub poisson_limit: f32,
    pub sigma_lo: f32,
    pub sigma_hi: f32,
    pub aspect_max: f32,
    pub amplitude: f32,
    pub reach_mm: f32,
    /// The frame seed plus this process's salt.
    pub seed: u32,
    pub layers: u32,
    pub resolved: bool,
}

#[cfg(feature = "gpu")]
impl FilmGrain {
    pub(crate) fn gpu_plan(&self, frame_width: u32) -> GpuPlan {
        let pitch = self.frame_mm / frame_width as f32;
        let processes = Process::table(self.size_mm);
        let scanner = Scanner::at_pitch(pitch);
        let blur = scanner.active() && processes.iter().all(|process| process.resolved(pitch));
        let mut mean = [0.0_f32; 3];
        let mut variance = [0.0_f32; 3];
        let mut active = Vec::new();
        for process in processes.iter().filter(|process| process.active()) {
            let splat = process.constants(pitch, blur.then_some(scanner));
            for layer in 0..3 {
                mean[layer] += splat.mean[layer];
                variance[layer] += splat.variance[layer];
            }
            active.push(GpuProcess {
                cell_mm: process.cell_mm,
                poisson_limit: (-process.mu).exp(),
                sigma_lo: process.sigma_lo,
                sigma_hi: process.sigma_hi,
                aspect_max: process.aspect_max,
                amplitude: splat.amplitude,
                reach_mm: process.reach_mm(),
                seed: self.seed.wrapping_add(process.salt),
                layers: u32::from(process.layers),
                resolved: splat.resolved,
            });
        }
        let (pad, kernel) = if blur {
            (scanner.pad() as u32, scanner.kernel())
        } else {
            (0, Vec::new())
        };
        GpuPlan {
            pitch,
            blur,
            pad,
            kernel,
            processes: active,
            mean,
            spread: variance.map(f32::sqrt),
        }
    }
}

/// Where the field is evaluated. Origins are full-frame pixels.
pub(crate) struct Plane {
    pub width: u32,
    pub rows: usize,
    pub origin_x: u32,
    pub origin_y: u32,
    pub frame_width: u32,
}

/// Pixel rectangle a cloud field is splatted into. Origins may be negative
/// so the scanner kernel can read grains that sit just outside the crop.
#[derive(Clone, Copy)]
struct Target {
    width: usize,
    rows: usize,
    origin_x: i32,
    origin_y: i32,
    pitch: f32,
}

/// One Poisson process. `layers` is a bitmask, red = 1, green = 2, blue = 4.
struct Process {
    cell_mm: f32,
    mu: f32,
    salt: u32,
    sigma_lo: f32,
    sigma_hi: f32,
    aspect_max: f32,
    variance: f32,
    layers: u8,
}

impl Process {
    fn table(size_mm: [f32; 3]) -> [Self; PROCESS_COUNT] {
        let shared = (size_mm[0] * size_mm[1] * size_mm[2]).cbrt();
        std::array::from_fn(|index| {
            let kind = &PROCESS_KINDS[index];
            let size = if kind.layers == SHARED_LAYERS {
                shared
            } else {
                size_mm[kind.layers.trailing_zeros() as usize]
            };
            Self {
                cell_mm: kind.cell_over_size * size,
                mu: kind.mu,
                salt: kind.salt,
                sigma_lo: kind.sigma_scale[0] * size,
                sigma_hi: kind.sigma_scale[1] * size,
                aspect_max: kind.aspect_max,
                variance: kind.variance,
                layers: kind.layers,
            }
        })
    }

    fn resolved(&self, pitch: f32) -> bool {
        self.sigma_lo / pitch >= RESOLVED_SIGMA_PX
    }

    /// `false` when this process adds nothing to any layer.
    fn active(&self) -> bool {
        self.variance > 0.0 && self.layers != 0
    }

    /// How far a cloud's kernel reaches from its center, in millimetres.
    fn reach_mm(&self) -> f32 {
        SUPPORT_SIGMAS * self.sigma_hi * self.aspect_max.sqrt()
    }

    /// Per-cloud amplitude and the field's analytic mean and variance at
    /// `pitch`. Shared by the CPU splat and the GPU upload.
    fn constants(&self, pitch: f32, scanner: Option<Scanner>) -> Splat {
        let resolved = self.resolved(pitch);
        let sigma2 = log_moment(self.sigma_lo, self.sigma_hi, 2);
        let sigma4 = log_moment(self.sigma_lo, self.sigma_hi, 4);
        let (strength_mean, strength_second) = strength_moments();
        let lambda = self.mu / (self.cell_mm * self.cell_mm);
        let amplitude = if resolved {
            (self.variance / (lambda * strength_second * std::f32::consts::PI * sigma2)).sqrt()
        } else {
            let pixel_area = pitch * pitch;
            (self.variance * pixel_area
                / (lambda
                    * strength_second
                    * 4.0
                    * std::f32::consts::PI
                    * std::f32::consts::PI
                    * sigma4))
                .sqrt()
        };
        let mean_density = lambda * amplitude * strength_mean * 2.0 * std::f32::consts::PI * sigma2;
        let mut variance = self.variance;
        if resolved && let Some(scanner) = scanner {
            variance *= scanner.variance_scale(sigma2);
        }
        let mut mean = [0.0_f32; 3];
        let mut layer_variance = [0.0_f32; 3];
        for layer in 0..3 {
            if self.layers & (1 << layer) != 0 {
                mean[layer] = mean_density;
                layer_variance[layer] = variance;
            }
        }
        Splat {
            resolved,
            amplitude,
            mean,
            variance: layer_variance,
        }
    }

    fn splat(
        &self,
        field: &mut [f64],
        target: Target,
        seed: u32,
        scanner: Option<Scanner>,
        control: &mut dyn DevelopControl,
    ) -> Result<([f32; 3], [f32; 3]), Cancelled> {
        if !self.active() {
            return Ok(([0.0; 3], [0.0; 3]));
        }
        let pitch = target.pitch;
        let Splat {
            resolved,
            amplitude,
            mean,
            variance: layer_variance,
        } = self.constants(pitch, scanner);

        let margin = self.reach_mm() + self.cell_mm;
        let origin_x_mm = target.origin_x as f32 * pitch;
        let origin_y_mm = target.origin_y as f32 * pitch;
        let width_mm = target.width as f32 * pitch;
        let height_mm = target.rows as f32 * pitch;
        let cell = self.cell_mm;
        let x_first = ((origin_x_mm - margin) / cell).floor() as i32;
        let y_first = ((origin_y_mm - margin) / cell).floor() as i32;
        let x_last = ((origin_x_mm + width_mm + margin) / cell).floor() as i32;
        let y_last = ((origin_y_mm + height_mm + margin) / cell).floor() as i32;
        let seed = seed.wrapping_add(self.salt);
        for cy in y_first..=y_last {
            if control.cancelled() {
                return Err(Cancelled);
            }
            for cx in x_first..=x_last {
                let mut draw = CellDraw::new(cx as u32, cy as u32, seed);
                let count = draw.poisson(self.mu);
                for _ in 0..count {
                    let ux = draw.unit();
                    let uy = draw.unit();
                    let ur = draw.unit();
                    let ua = draw.unit();
                    let ut = draw.unit();
                    let uf = draw.unit();
                    let x_mm = (cx as f32 + ux) * cell;
                    let y_mm = (cy as f32 + uy) * cell;
                    let sigma = log_lerp(self.sigma_lo, self.sigma_hi, ur);
                    let strength = STRENGTH_LO + (STRENGTH_HI - STRENGTH_LO) * uf;
                    let amplitude = amplitude * strength;
                    if resolved {
                        let aspect = log_lerp(1.0, self.aspect_max, ua);
                        let axis = aspect.sqrt();
                        let (sin, cos) = (ut * std::f32::consts::TAU).sin_cos();
                        paint_gaussian(
                            field,
                            target,
                            Gaussian {
                                x_mm,
                                y_mm,
                                sx: sigma / axis,
                                sy: sigma * axis,
                                sin,
                                cos,
                                amplitude,
                                layers: self.layers,
                            },
                        );
                    } else {
                        deposit_mass(field, target, x_mm, y_mm, amplitude, sigma, self.layers);
                    }
                }
            }
        }
        Ok((mean, layer_variance))
    }
}

/// What one process contributes at a given pitch.
struct Splat {
    resolved: bool,
    amplitude: f32,
    mean: [f32; 3],
    variance: [f32; 3],
}

struct ProcessKind {
    cell_over_size: f32,
    mu: f32,
    salt: u32,
    variance: f32,
    sigma_scale: [f32; 2],
    aspect_max: f32,
    layers: u8,
}

const PROCESS_COUNT: usize = 6;
const SHARED_LAYERS: u8 = 0b111;

/// Fine shared clouds carry the grain. A little mid and coarse structure
/// keeps it from reading as filtered noise. The private remainder is chroma.
///
/// `cell_over_size` and `mu` are chosen so a point is covered by several
/// clouds. Variances on one layer sum to 1.
const PROCESS_KINDS: [ProcessKind; PROCESS_COUNT] = [
    ProcessKind {
        cell_over_size: 0.95,
        mu: 1.23,
        salt: 0x51C0_0101,
        variance: 0.72,
        sigma_scale: [0.80, 1.15],
        aspect_max: 1.55,
        layers: SHARED_LAYERS,
    },
    ProcessKind {
        cell_over_size: 2.0,
        mu: 1.16,
        salt: 0xA11C_0102,
        variance: 0.20,
        sigma_scale: [1.70, 2.50],
        aspect_max: 1.65,
        layers: SHARED_LAYERS,
    },
    ProcessKind {
        cell_over_size: 4.0,
        mu: 1.02,
        salt: 0xC0A5_0103,
        variance: 0.03,
        sigma_scale: [3.20, 4.60],
        aspect_max: 1.70,
        layers: SHARED_LAYERS,
    },
    ProcessKind {
        cell_over_size: 0.85,
        mu: 1.26,
        salt: 0xB1E7_0111,
        variance: 0.05,
        sigma_scale: [0.85, 1.25],
        aspect_max: 1.50,
        layers: 0b001,
    },
    ProcessKind {
        cell_over_size: 0.85,
        mu: 1.26,
        salt: 0xB1E7_0122,
        variance: 0.05,
        sigma_scale: [0.85, 1.25],
        aspect_max: 1.50,
        layers: 0b010,
    },
    ProcessKind {
        cell_over_size: 0.85,
        mu: 1.26,
        salt: 0xB1E7_0133,
        variance: 0.05,
        sigma_scale: [0.85, 1.25],
        aspect_max: 1.50,
        layers: 0b100,
    },
];

/// Below this many pixels of sigma, the photosite is the aperture: the whole
/// cloud is deposited in the pixel that contains its center.
const RESOLVED_SIGMA_PX: f32 = 0.75;

/// Kernel support, in sigmas. The tail past this is about 0.2% of a gaussian.
const SUPPORT_SIGMAS: f32 = 3.5;
const SUPPORT_D2: f32 = SUPPORT_SIGMAS * SUPPORT_SIGMAS;

/// Per-cloud strength. A narrow range keeps the overlap high.
const STRENGTH_LO: f32 = 0.72;
const STRENGTH_HI: f32 = 1.0;

fn strength_moments() -> (f32, f32) {
    let span = STRENGTH_HI - STRENGTH_LO;
    let mean = 0.5 * (STRENGTH_LO + STRENGTH_HI);
    let second = STRENGTH_LO * STRENGTH_LO + STRENGTH_LO * span + span * span / 3.0;
    (mean, second)
}

/// `E[X^power]` for `X` log-uniform on `[lo, hi]`.
fn log_moment(lo: f32, hi: f32, power: u32) -> f32 {
    let ratio = hi / lo;
    let p = power as f32;
    if !(ratio.is_finite()) || ratio < 1.000_001 {
        return lo.powi(power as i32);
    }
    lo.powi(power as i32) * (ratio.powf(p) - 1.0) / (p * ratio.ln())
}

fn log_lerp(lo: f32, hi: f32, unit: f32) -> f32 {
    lo * (hi / lo).powf(unit)
}

struct CellDraw {
    x: u32,
    y: u32,
    seed: u32,
    seq: u32,
}

impl CellDraw {
    fn new(x: u32, y: u32, seed: u32) -> Self {
        Self { x, y, seed, seq: 0 }
    }

    fn unit(&mut self) -> f32 {
        self.seq = self.seq.wrapping_add(1);
        let hash = crate::stock::Grain::hash(self.x, self.y, self.seq, self.seed);
        (hash >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// Knuth's Poisson generator. `mu` stays near 1 for every process.
    fn poisson(&mut self, mu: f32) -> u32 {
        let limit = (-mu).exp();
        let mut product = 1.0_f32;
        let mut count = 0_u32;
        while count < 64 {
            product *= self.unit();
            if product <= limit {
                return count;
            }
            count += 1;
        }
        count
    }
}

struct Gaussian {
    x_mm: f32,
    y_mm: f32,
    sx: f32,
    sy: f32,
    sin: f32,
    cos: f32,
    amplitude: f32,
    layers: u8,
}

fn paint_gaussian(field: &mut [f64], target: Target, cloud: Gaussian) {
    let Target {
        width,
        rows,
        origin_x,
        origin_y,
        pitch,
    } = target;
    let px = cloud.x_mm / pitch - origin_x as f32;
    let py = cloud.y_mm / pitch - origin_y as f32;
    let sx = cloud.sx / pitch;
    let sy = cloud.sy / pitch;
    if sx <= 0.0 || sy <= 0.0 {
        return;
    }
    let rad = SUPPORT_SIGMAS * sx.max(sy);
    let x0 = ((px - rad - 0.5).ceil() as i32).max(0);
    let y0 = ((py - rad - 0.5).ceil() as i32).max(0);
    let x1 = ((px + rad - 0.5).floor() as i32).min(width as i32 - 1);
    let y1 = ((py + rad - 0.5).floor() as i32).min(rows as i32 - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }
    let inv_sx = 1.0 / sx;
    let inv_sy = 1.0 / sy;
    for y in y0 as usize..=y1 as usize {
        let dy = y as f32 + 0.5 - py;
        let row = y * width;
        for x in x0 as usize..=x1 as usize {
            let dx = x as f32 + 0.5 - px;
            let xr = dx * cloud.cos + dy * cloud.sin;
            let yr = -dx * cloud.sin + dy * cloud.cos;
            let d2 = xr * inv_sx * xr * inv_sx + yr * inv_sy * yr * inv_sy;
            if d2 >= SUPPORT_D2 {
                continue;
            }
            let weight = f64::from(cloud.amplitude * (-0.5 * d2).exp());
            let index = (row + x) * 3;
            for layer in 0..3 {
                if cloud.layers & (1 << layer) != 0 {
                    field[index + layer] += weight;
                }
            }
        }
    }
}

/// Photosite integral of a sub-pixel cloud. The whole mass lands in the pixel
/// that contains the center, which is what a large aperture does.
fn deposit_mass(
    field: &mut [f64],
    target: Target,
    x_mm: f32,
    y_mm: f32,
    amplitude: f32,
    sigma: f32,
    layers: u8,
) {
    let local_x = x_mm / target.pitch - target.origin_x as f32;
    let local_y = y_mm / target.pitch - target.origin_y as f32;
    let x = local_x.floor() as i32;
    let y = local_y.floor() as i32;
    if x < 0 || y < 0 || x >= target.width as i32 || y >= target.rows as i32 {
        return;
    }
    let sigma = f64::from(sigma);
    let pitch = f64::from(target.pitch);
    let integral = f64::from(amplitude) * 2.0 * std::f64::consts::PI * sigma * sigma;
    let mass = integral / (pitch * pitch);
    let index = (y as usize * target.width + x as usize) * 3;
    for layer in 0..3 {
        if layers & (1 << layer) != 0 {
            field[index + layer] += mass;
        }
    }
}

/// Scanner aperture. Sigma is a fixed length on the film, so a finer scan
/// does not invent a harder grain and a coarse preview does not blur twice.
#[derive(Clone, Copy)]
struct Scanner {
    sigma_px: f32,
}

impl Scanner {
    /// About 6 µm, a tight scan of a 35 mm frame. Smaller than a Portra dye
    /// cloud, large enough to take the pixel grid out of a resolved cloud.
    const SIGMA_MM: f32 = 0.006;

    fn at_pitch(pitch: f32) -> Self {
        Self {
            sigma_px: Self::SIGMA_MM / pitch,
        }
    }

    fn active(self) -> bool {
        self.sigma_px >= 0.40
    }

    fn pad(self) -> i32 {
        if self.active() {
            (self.sigma_px * 3.0).ceil() as i32
        } else {
            0
        }
    }

    /// How much of a gaussian cloud's variance survives this aperture.
    ///
    /// `sigma2` is `E[σ²]` of that cloud in millimetres. An isotropic blur
    /// multiplies `∫k²` by `σ² / (σ² + s²)`.
    fn variance_scale(self, sigma2_mm: f32) -> f32 {
        if !self.active() || sigma2_mm <= 0.0 {
            return 1.0;
        }
        let scanner2 = Self::SIGMA_MM * Self::SIGMA_MM;
        sigma2_mm / (sigma2_mm + scanner2)
    }

    /// Normalized Gaussian taps, `2 * pad + 1` of them.
    fn kernel(self) -> Vec<f32> {
        let radius = self.pad();
        let sigma = self.sigma_px;
        let mut kernel = Vec::with_capacity(radius as usize * 2 + 1);
        let mut weight = 0.0_f32;
        for offset in -radius..=radius {
            let tap = (-0.5 * (offset as f32 / sigma).powi(2)).exp();
            kernel.push(tap);
            weight += tap;
        }
        for tap in &mut kernel {
            *tap /= weight;
        }
        kernel
    }

    fn blur(
        self,
        field: &mut [f64],
        width: usize,
        rows: usize,
        control: &mut dyn DevelopControl,
    ) -> Result<(), Cancelled> {
        let radius = self.pad();
        if radius <= 0 || width == 0 || rows == 0 {
            return Ok(());
        }
        let kernel = self.kernel();
        let mut temp = vec![0.0_f64; field.len()];
        for y in 0..rows {
            if control.cancelled() {
                return Err(Cancelled);
            }
            for x in 0..width {
                let mut acc = [0.0_f64; 3];
                for (index, tap) in kernel.iter().enumerate() {
                    let src_x =
                        (x as i32 + index as i32 - radius).clamp(0, width as i32 - 1) as usize;
                    let src = (y * width + src_x) * 3;
                    let tap = f64::from(*tap);
                    acc[0] += field[src] * tap;
                    acc[1] += field[src + 1] * tap;
                    acc[2] += field[src + 2] * tap;
                }
                let dst = (y * width + x) * 3;
                temp[dst] = acc[0];
                temp[dst + 1] = acc[1];
                temp[dst + 2] = acc[2];
            }
        }
        for y in 0..rows {
            if control.cancelled() {
                return Err(Cancelled);
            }
            for x in 0..width {
                let mut acc = [0.0_f64; 3];
                for (index, tap) in kernel.iter().enumerate() {
                    let src_y =
                        (y as i32 + index as i32 - radius).clamp(0, rows as i32 - 1) as usize;
                    let src = (src_y * width + x) * 3;
                    let tap = f64::from(*tap);
                    acc[0] += temp[src] * tap;
                    acc[1] += temp[src + 1] * tap;
                    acc[2] += temp[src + 2] * tap;
                }
                let dst = (y * width + x) * 3;
                field[dst] = acc[0];
                field[dst + 1] = acc[1];
                field[dst + 2] = acc[2];
            }
        }
        Ok(())
    }
}
