//! Scanner-filtered film grain: dye clouds for color film, silver clumps
//! for black and white.
//!
//! Real color-negative grain is a Poisson field of soft dye clouds in each
//! emulsion layer. At a scan of Portra the clouds are a few pixels across,
//! they overlap, and the three layers are read together, so the grain is
//! mostly luminance with a fine chroma residual. Developed silver is
//! different: opaque, hard-edged clumps of filament that overlap less, so a
//! black and white scan is crisper and skewed toward dense specks. This
//! module is both fields: shot noise in the spirit of the Boolean model in
//! Newson, Delon, and Galerne (Computer Graphics Forum 36(8), 2017),
//! evaluated in millimetres, then filtered by a scanner aperture.
//!
//! The field is calibrated against the 48 µm aperture that granularity
//! tables are measured through, not against a pixel. A pixel's grain then
//! follows from cloud size, the scanner, and the pitch: a fine scan sees more
//! than the datasheet RMS, a small preview less, the way averaging an export
//! down to that size does.
//!
//! The implementation is original. It does not copy darktable, spektrafilm,
//! or the Newson reference renderer.

use rayon::prelude::*;

use crate::render::{Cancelled, DevelopControl};
use crate::workers::Workers;

/// Grain field for one frame.
///
/// `size_mm` is the primary cloud sigma of each layer, red, green, then
/// blue. Shared clouds use the geometric mean, so a clump is the same shape
/// in every layer. Each color layer also keeps a small private field at its
/// own size. That is the chroma.
///
/// A monochrome emulsion has one silver layer. It draws silver clumps from
/// [`SILVER_KINDS`], and every process lands in all three channels, so the
/// field is the same in each and the grain has no chroma.
pub(crate) struct FilmGrain {
    frame_mm: f32,
    seed: u32,
    size_mm: [f32; 3],
    monochrome: bool,
}

impl FilmGrain {
    pub(crate) fn new(frame_mm: f32, seed: u32, size_mm: [f32; 3], monochrome: bool) -> Self {
        Self {
            frame_mm,
            seed,
            size_mm,
            monochrome,
        }
    }

    /// Zero-mean grain for `plane`, one sample per density, in units of the
    /// field's standard deviation through the 48 µm aperture.
    ///
    /// A pixel's own deviation is [`Self::pixel_ratio`]. The pattern does not
    /// depend on the picture. RMS is applied later, per pixel, so a hard edge
    /// does not grow extra grain. A crop uses the full frame's pitch and is
    /// padded by the aperture kernel, so those pixels match the full frame.
    pub(crate) fn render(
        &self,
        plane: Plane,
        control: &mut dyn DevelopControl,
    ) -> Result<Vec<f32>, Cancelled> {
        let pitch = self.frame_mm / plane.frame_width as f32;
        let processes = Process::table(self.size_mm, self.monochrome);
        let aperture = Aperture::for_frame(&processes, pitch);
        let pad = aperture.map_or(0, Aperture::pad);
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
            origin_x,
            origin_y,
            pitch,
        };
        let mut mean = [0.0_f32; 3];
        let mut reference = [0.0_f32; 3];
        for process in &processes {
            let splat = process.splat(&mut field, target, self.seed, aperture, control)?;
            for layer in 0..3 {
                mean[layer] += splat.mean[layer];
                reference[layer] += splat.reference[layer];
            }
        }
        if let Some(aperture) = aperture {
            aperture.blur(&mut field, padded_w, padded_h, control)?;
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
            let spread = reference[index % 3].sqrt();
            if spread == 0.0 {
                *sample = 0.0;
            } else {
                *sample = (*sample - mean[index % 3]) / spread;
            }
        }
        Ok(out)
    }

    /// Standard deviation of one pixel of [`Self::render`] at
    /// `frame_width`, per layer, as a multiple of the 48 µm reading.
    ///
    /// Above 1 when pixels are finer than the clouds' reach through that
    /// aperture, below 1 for a small preview.
    pub(crate) fn pixel_ratio(&self, frame_width: u32) -> [f32; 3] {
        if frame_width == 0 {
            return [0.0; 3];
        }
        let pitch = self.frame_mm / frame_width as f32;
        let processes = Process::table(self.size_mm, self.monochrome);
        let aperture = Aperture::for_frame(&processes, pitch);
        let mut variance = [0.0_f32; 3];
        let mut reference = [0.0_f32; 3];
        for process in processes.iter().filter(|process| process.active()) {
            let splat = process.constants(pitch, aperture);
            for layer in 0..3 {
                variance[layer] += splat.variance[layer];
                reference[layer] += splat.reference[layer];
            }
        }
        std::array::from_fn(|layer| {
            if reference[layer] == 0.0 {
                0.0
            } else {
                (variance[layer] / reference[layer]).sqrt()
            }
        })
    }
}

/// What the GPU needs to evaluate [`FilmGrain::render`] for one frame width.
#[cfg(feature = "gpu")]
pub(crate) struct GpuPlan {
    pub pitch: f32,
    /// Whether the aperture blurs the field: the scanner when every process
    /// is resolved, a wider pixel aperture when some are not.
    pub blur: bool,
    /// Rows and columns of field computed past each crop edge for the blur.
    pub pad: u32,
    /// Normalized aperture taps, `2 * pad + 1` of them, when `blur`.
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
    /// [`Kernel::code`].
    pub kernel: u32,
    /// The per-pixel draw when the process is not resolved.
    pub aggregate: Option<Aggregate>,
}

#[cfg(feature = "gpu")]
impl FilmGrain {
    pub(crate) fn gpu_plan(&self, frame_width: u32) -> GpuPlan {
        let pitch = self.frame_mm / frame_width as f32;
        let processes = Process::table(self.size_mm, self.monochrome);
        let aperture = Aperture::for_frame(&processes, pitch);
        let mut mean = [0.0_f32; 3];
        let mut reference = [0.0_f32; 3];
        let mut active = Vec::new();
        for process in processes.iter().filter(|process| process.active()) {
            let splat = process.constants(pitch, aperture);
            for layer in 0..3 {
                mean[layer] += splat.mean[layer];
                reference[layer] += splat.reference[layer];
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
                kernel: process.kernel.code(),
                aggregate: (!splat.resolved).then(|| process.aggregate(splat.amplitude, pitch)),
            });
        }
        let (pad, kernel) = match aperture {
            Some(aperture) => (aperture.pad() as u32, aperture.kernel()),
            None => (0, Vec::new()),
        };
        GpuPlan {
            pitch,
            blur: aperture.is_some(),
            pad,
            kernel,
            processes: active,
            mean,
            spread: reference.map(f32::sqrt),
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
/// so the aperture kernel can read grains that sit just outside the crop.
#[derive(Clone, Copy)]
struct Target {
    width: usize,
    origin_x: i32,
    origin_y: i32,
    pitch: f32,
}

/// The profile of one cloud.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kernel {
    /// A soft gaussian dye cloud.
    Dye,
    /// A silver clump: a flat-topped disk with a short rim, `(1 - q²)²` for
    /// `q = d² / R²`, `R` being [`Kernel::SILVER_RADIUS`] sigmas. Its
    /// equivalent gaussian sigma is within 0.5% of the cloud's sigma, so
    /// `size_mm` means the same thing for both kernels.
    Silver,
}

impl Kernel {
    /// Silver clump radius, in sigmas.
    const SILVER_RADIUS: f32 = 2.4;
    /// [`Self::SILVER_RADIUS`] squared. WGSL uses the same literal.
    const SILVER_R2: f32 = 5.76;

    /// How far the profile reaches, in sigmas. A dye cloud's tail past 3.5
    /// sigmas is about 0.2% of a gaussian.
    fn support(self) -> f32 {
        match self {
            Self::Dye => 3.5,
            Self::Silver => Self::SILVER_RADIUS,
        }
    }

    /// `∫k / (amplitude · σ²)` over the plane, for axes whose product is σ².
    fn mass(self) -> f32 {
        match self {
            Self::Dye => std::f32::consts::TAU,
            // 2πR² ∫₀¹ (1 - t⁴)² t dt = 2πR² · 4/15.
            Self::Silver => std::f32::consts::TAU * Self::SILVER_R2 * 4.0 / 15.0,
        }
    }

    /// `∫k² / (amplitude² · σ²)` over the plane.
    fn energy(self) -> f32 {
        match self {
            Self::Dye => std::f32::consts::PI,
            // 2πR² ∫₀¹ (1 - t⁴)⁴ t dt = 2πR² · 64/315.
            Self::Silver => std::f32::consts::TAU * Self::SILVER_R2 * 64.0 / 315.0,
        }
    }

    /// Variance of the gaussian with this profile's mass and energy, as a
    /// multiple of σ². 1 for a dye cloud.
    fn sigma2_factor(self) -> f32 {
        self.mass().powi(2) / (4.0 * std::f32::consts::PI * self.energy())
    }

    /// The silver profile at squared distance `d2`, in sigmas.
    fn silver(d2: f32) -> f32 {
        let q = d2 / Self::SILVER_R2;
        if q >= 1.0 {
            return 0.0;
        }
        let rim = 1.0 - q * q;
        rim * rim
    }

    /// The GPU's process word for this kernel.
    #[cfg(feature = "gpu")]
    fn code(self) -> u32 {
        match self {
            Self::Dye => 0,
            Self::Silver => 1,
        }
    }
}

/// One Poisson process. `layers` is a bitmask, red = 1, green = 2, blue = 4.
struct Process {
    cell_mm: f32,
    mu: f32,
    salt: u32,
    sigma_lo: f32,
    sigma_hi: f32,
    aspect_max: f32,
    /// This process's share of a layer's variance through the 48 µm
    /// aperture.
    variance: f32,
    layers: u8,
    kernel: Kernel,
}

impl Process {
    /// With `monochrome`, silver clumps in every layer.
    fn table(size_mm: [f32; 3], monochrome: bool) -> Vec<Self> {
        let shared = (size_mm[0] * size_mm[1] * size_mm[2]).cbrt();
        let kinds: &[ProcessKind] = if monochrome {
            &SILVER_KINDS
        } else {
            &DYE_KINDS
        };
        kinds
            .iter()
            .map(|kind| {
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
                    kernel: kind.kernel,
                }
            })
            .collect()
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
        self.kernel.support() * self.sigma_hi * self.aspect_max.sqrt()
    }

    /// Variance of one pixel at `pitch` over variance through the 48 µm
    /// aperture, for this process's clouds.
    ///
    /// A pixel averages the scanned film over its area: each cloud is
    /// blurred by the scanner, then by the pixel's box. The aperture reads
    /// the film directly. A cloud's mass goes as σ², so sizes weigh by σ⁴.
    /// Aspect is left out: both sides see the same ellipses.
    fn pixel_share(&self, pitch: f32) -> f32 {
        const STEPS: usize = 32;
        let factor = f64::from(self.kernel.sigma2_factor());
        let scanner2 = f64::from(Aperture::SCANNER_MM).powi(2);
        // A box of the 48 µm circle's area.
        let side = f64::from(crate::stock::Grain::APERTURE_MM) * std::f64::consts::PI.sqrt() / 2.0;
        let pitch = f64::from(pitch);
        let lo = f64::from(self.sigma_lo);
        let ratio = f64::from(self.sigma_hi) / lo;
        let mut pixel = 0.0;
        let mut reference = 0.0;
        for step in 0..STEPS {
            let unit = (step as f64 + 0.5) / STEPS as f64;
            let sigma = lo * ratio.powf(unit);
            let sigma2 = factor * sigma * sigma;
            let weight = sigma.powi(4);
            pixel += weight * box_energy(pitch, (sigma2 + scanner2).sqrt()).powi(2);
            reference += weight * box_energy(side, sigma2.sqrt()).powi(2);
        }
        (pixel / reference) as f32
    }

    /// Per-cloud amplitude, the field's analytic mean, its variance at
    /// `pitch` after `aperture`, and its 48 µm variance. Shared by the CPU
    /// splat and the GPU upload.
    fn constants(&self, pitch: f32, aperture: Option<Aperture>) -> Splat {
        let resolved = self.resolved(pitch);
        let sigma2 = log_moment(self.sigma_lo, self.sigma_hi, 2);
        let sigma4 = log_moment(self.sigma_lo, self.sigma_hi, 4);
        let (strength_mean, strength_second) = strength_moments();
        let lambda = self.mu / (self.cell_mm * self.cell_mm);
        let variance = self.variance * self.pixel_share(pitch);
        // What the aperture leaves of the drawn field's variance. Resolved
        // clouds widen; per-pixel draws are white, so the taps decide.
        let kept = aperture.map_or(1.0, |aperture| {
            if resolved {
                aperture.cloud_scale(self.kernel.sigma2_factor() * sigma2)
            } else {
                aperture.white_scale()
            }
        });
        let drawn = variance / kept;
        let amplitude = if resolved {
            (drawn / (lambda * strength_second * self.kernel.energy() * sigma2)).sqrt()
        } else {
            (drawn * pitch * pitch
                / (lambda * strength_second * self.kernel.mass().powi(2) * sigma4))
                .sqrt()
        };
        let mean_density = lambda * amplitude * strength_mean * self.kernel.mass() * sigma2;
        let mut splat = Splat {
            resolved,
            amplitude,
            mean: [0.0; 3],
            variance: [0.0; 3],
            reference: [0.0; 3],
        };
        for layer in 0..3 {
            if self.layers & (1 << layer) != 0 {
                splat.mean[layer] = mean_density;
                splat.variance[layer] = variance;
                splat.reference[layer] = self.variance;
            }
        }
        splat
    }

    /// Per-pixel distribution of this process when its clouds are smaller
    /// than a pixel. Shared by the CPU fill and the GPU upload.
    ///
    /// Each sub-pixel cloud lands whole in the pixel that holds its center,
    /// so where a cloud sits inside the pixel does not matter. A pixel's sum
    /// is a compound Poisson: `N` clouds, `N ~ Poisson(λ · pitch²)`, each
    /// adding `amplitude · strength · mass · σ² / pitch²`.
    fn aggregate(&self, amplitude: f32, pitch: f32) -> Aggregate {
        let clouds = f64::from(self.mu) * f64::from(pitch / self.cell_mm).powi(2);
        let (strength_mean, strength_second) = strength_moments();
        let sigma2 = f64::from(log_moment(self.sigma_lo, self.sigma_hi, 2));
        let sigma4 = f64::from(log_moment(self.sigma_lo, self.sigma_hi, 4));
        let scale = f64::from(amplitude) * f64::from(self.kernel.mass()) / f64::from(pitch).powi(2);
        let mass_mean = scale * f64::from(strength_mean) * sigma2;
        let mass_second = scale * scale * f64::from(strength_second) * sigma4;
        let mass_spread = (mass_second - mass_mean * mass_mean).max(0.0).sqrt();
        if clouds > AGGREGATE_TABLE_CLOUDS {
            return Aggregate {
                cdf: Vec::new(),
                mass_mean: (clouds * mass_mean) as f32,
                mass_spread: (clouds * mass_second).sqrt() as f32,
            };
        }
        // P(N <= k), until it rounds to 1 in f32.
        let mut cdf = Vec::new();
        let mut term = (-clouds).exp();
        let mut total = term;
        while cdf.len() < AGGREGATE_TABLE_LEN {
            let value = total as f32;
            cdf.push(value);
            if value >= 1.0 {
                break;
            }
            let next = cdf.len() as f64;
            term *= clouds / next;
            total += term;
        }
        Aggregate {
            cdf,
            mass_mean: mass_mean as f32,
            mass_spread: mass_spread as f32,
        }
    }

    fn splat(
        &self,
        field: &mut [f64],
        target: Target,
        seed: u32,
        aperture: Option<Aperture>,
        control: &mut dyn DevelopControl,
    ) -> Result<Splat, Cancelled> {
        if !self.active() {
            return Ok(Splat::default());
        }
        let pitch = target.pitch;
        let splat = self.constants(pitch, aperture);
        let seed = seed.wrapping_add(self.salt);
        if splat.resolved {
            Band::split(field, target, control, |band| {
                self.splat_band(band, target, seed, splat.amplitude);
            })?;
        } else {
            self.aggregate(splat.amplitude, pitch).fill(
                field,
                target,
                seed,
                self.layers,
                control,
            )?;
        }
        Ok(splat)
    }

    /// Every cloud that reaches `band`'s rows, painted into them.
    ///
    /// Cells are visited in the same order for every band, and a pixel sums
    /// its clouds in that order, so the field does not depend on the banding.
    fn splat_band(&self, band: Band<'_>, target: Target, seed: u32, amplitude: f32) {
        let pitch = target.pitch;
        let margin = self.reach_mm() + self.cell_mm;
        let origin_x_mm = target.origin_x as f32 * pitch;
        let origin_y_mm = (target.origin_y + band.first_row as i32) as f32 * pitch;
        let width_mm = target.width as f32 * pitch;
        let height_mm = band.rows as f32 * pitch;
        let cell = self.cell_mm;
        let x_first = ((origin_x_mm - margin) / cell).floor() as i32;
        let y_first = ((origin_y_mm - margin) / cell).floor() as i32;
        let x_last = ((origin_x_mm + width_mm + margin) / cell).floor() as i32;
        let y_last = ((origin_y_mm + height_mm + margin) / cell).floor() as i32;
        let mut band = band;
        for cy in y_first..=y_last {
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
                    let aspect = log_lerp(1.0, self.aspect_max, ua);
                    let axis = aspect.sqrt();
                    let (sin, cos) = (ut * std::f32::consts::TAU).sin_cos();
                    paint_cloud(
                        &mut band,
                        target,
                        Cloud {
                            x_mm,
                            y_mm,
                            sx: sigma / axis,
                            sy: sigma * axis,
                            sin,
                            cos,
                            amplitude,
                            layers: self.layers,
                            kernel: self.kernel,
                        },
                    );
                }
            }
        }
    }
}

/// Rows `first_row..first_row + rows` of a target's field.
struct Band<'a> {
    field: &'a mut [f64],
    first_row: usize,
    rows: usize,
}

impl Band<'_> {
    /// Rows per band. Each band redraws the clouds in its margin, so a band
    /// stays several cloud reaches tall.
    const ROWS: usize = 64;

    /// Run `work` on every band of `field` across the thread pool. Cancel
    /// is checked between rounds of bands, on this thread, because
    /// `control` is not shared with the workers.
    fn split(
        field: &mut [f64],
        target: Target,
        control: &mut dyn DevelopControl,
        work: impl Fn(Band<'_>) + Sync,
    ) -> Result<(), Cancelled> {
        let stride = Self::ROWS * target.width * 3;
        if stride == 0 {
            return Ok(());
        }
        let round = Self::ROWS * Workers::threads() * 2;
        for (round_index, rows) in field.chunks_mut(round * target.width * 3).enumerate() {
            if control.cancelled() {
                return Err(Cancelled);
            }
            let first = round_index * round;
            Workers::install(|| {
                rows.par_chunks_mut(stride)
                    .enumerate()
                    .for_each(|(index, field)| {
                        let rows = field.len() / (target.width * 3);
                        work(Band {
                            field,
                            first_row: first + index * Self::ROWS,
                            rows,
                        });
                    });
            });
        }
        if control.cancelled() {
            return Err(Cancelled);
        }
        Ok(())
    }
}

/// What one process contributes at a given pitch.
#[derive(Default)]
struct Splat {
    resolved: bool,
    amplitude: f32,
    mean: [f32; 3],
    /// Per-pixel variance after the aperture.
    variance: [f32; 3],
    /// Variance through the 48 µm aperture.
    reference: [f32; 3],
}

/// One unresolved process, drawn per pixel instead of per cloud.
///
/// With a `cdf`, the cloud count is drawn exactly from that table and the
/// clouds' summed mass is gaussian given the count: a cloud's mass varies
/// little, so the count carries the skew. Without one, the pixel holds more
/// than [`AGGREGATE_TABLE_CLOUDS`] clouds on average and the whole sum is
/// gaussian, `mass_mean` and `mass_spread` being its mean and deviation.
///
/// The cost is per pixel, not per cloud, so a preview no longer draws every
/// cloud in the frame. The draws hash the pixel's full-frame position, so a
/// crop at the same pitch matches the full frame. The pixel aperture then
/// spreads each pixel's mass over its neighbors, as a cloud that straddles
/// a pixel edge does.
pub(crate) struct Aggregate {
    pub cdf: Vec<f32>,
    pub mass_mean: f32,
    pub mass_spread: f32,
}

impl Aggregate {
    /// This process's sum at full-frame pixel `(x, y)`.
    fn sample(&self, x: i32, y: i32, seed: u32) -> f32 {
        let mut draw = CellDraw::new(x as u32, y as u32, seed ^ AGGREGATE_SALT);
        let pick = draw.unit();
        // Box-Muller. `1 - unit` is exact and never zero.
        let radius = (-2.0 * (1.0 - draw.unit()).ln()).sqrt();
        let normal = radius * (draw.unit() * std::f32::consts::TAU).cos();
        if self.cdf.is_empty() {
            return self.mass_mean + self.mass_spread * normal;
        }
        let count = self.cdf.iter().take_while(|limit| **limit <= pick).count() as f32;
        count * self.mass_mean + count.sqrt() * self.mass_spread * normal
    }

    fn fill(
        &self,
        field: &mut [f64],
        target: Target,
        seed: u32,
        layers: u8,
        control: &mut dyn DevelopControl,
    ) -> Result<(), Cancelled> {
        Band::split(field, target, control, |band| {
            for row in 0..band.rows {
                let y = target.origin_y + (band.first_row + row) as i32;
                for column in 0..target.width {
                    let value = f64::from(self.sample(target.origin_x + column as i32, y, seed));
                    let index = (row * target.width + column) * 3;
                    for layer in 0..3 {
                        if layers & (1 << layer) != 0 {
                            band.field[index + layer] += value;
                        }
                    }
                }
            }
        })
    }
}

/// Mixed into an unresolved process's seed, so its per-pixel draws are not
/// the per-cell draws of the same coordinates.
pub(crate) const AGGREGATE_SALT: u32 = 0x6A09_E667;

/// Above this mean count per pixel, the count is gaussian too. Poisson skew
/// is `1 / sqrt(mean)`, 0.125 here.
const AGGREGATE_TABLE_CLOUDS: f64 = 64.0;

/// Longest count table. At a mean of 64 the CDF reaches 1 in f32 well before.
pub(crate) const AGGREGATE_TABLE_LEN: usize = 192;

struct ProcessKind {
    cell_over_size: f32,
    mu: f32,
    salt: u32,
    variance: f32,
    sigma_scale: [f32; 2],
    aspect_max: f32,
    layers: u8,
    kernel: Kernel,
}

const SHARED_LAYERS: u8 = 0b111;

/// Color film. Fine shared clouds carry the grain. A little mid and coarse
/// structure keeps it from reading as filtered noise. The private remainder
/// is chroma.
///
/// `cell_over_size` and `mu` are chosen so a point is covered by several
/// clouds. Variances on one layer sum to 1.
const DYE_KINDS: [ProcessKind; 6] = [
    ProcessKind {
        cell_over_size: 0.95,
        mu: 1.23,
        salt: 0x51C0_0101,
        variance: 0.72,
        sigma_scale: [0.80, 1.15],
        aspect_max: 1.55,
        layers: SHARED_LAYERS,
        kernel: Kernel::Dye,
    },
    ProcessKind {
        cell_over_size: 2.0,
        mu: 1.16,
        salt: 0xA11C_0102,
        variance: 0.20,
        sigma_scale: [1.70, 2.50],
        aspect_max: 1.65,
        layers: SHARED_LAYERS,
        kernel: Kernel::Dye,
    },
    ProcessKind {
        cell_over_size: 4.0,
        mu: 1.02,
        salt: 0xC0A5_0103,
        variance: 0.03,
        sigma_scale: [3.20, 4.60],
        aspect_max: 1.70,
        layers: SHARED_LAYERS,
        kernel: Kernel::Dye,
    },
    ProcessKind {
        cell_over_size: 0.85,
        mu: 1.26,
        salt: 0xB1E7_0111,
        variance: 0.05,
        sigma_scale: [0.85, 1.25],
        aspect_max: 1.50,
        layers: 0b001,
        kernel: Kernel::Dye,
    },
    ProcessKind {
        cell_over_size: 0.85,
        mu: 1.26,
        salt: 0xB1E7_0122,
        variance: 0.05,
        sigma_scale: [0.85, 1.25],
        aspect_max: 1.50,
        layers: 0b010,
        kernel: Kernel::Dye,
    },
    ProcessKind {
        cell_over_size: 0.85,
        mu: 1.26,
        salt: 0xB1E7_0133,
        variance: 0.05,
        sigma_scale: [0.85, 1.25],
        aspect_max: 1.50,
        layers: 0b100,
        kernel: Kernel::Dye,
    },
];

/// Black and white film. Developed silver is opaque clumps, not dye clouds:
/// hard-edged, irregular, and sparse enough that a point sees only a couple
/// of them, so the field leans toward dense specks. Larger aggregates of
/// clumps give the grain its tooth, and a faint soft mottle stays from the
/// emulsion's uneven development. Variances sum to 1.
const SILVER_KINDS: [ProcessKind; 3] = [
    ProcessKind {
        cell_over_size: 2.6,
        mu: 1.0,
        salt: 0x5117_0201,
        variance: 0.75,
        sigma_scale: [0.70, 1.35],
        aspect_max: 2.2,
        layers: SHARED_LAYERS,
        kernel: Kernel::Silver,
    },
    ProcessKind {
        cell_over_size: 4.2,
        mu: 1.0,
        salt: 0x5117_0202,
        variance: 0.20,
        sigma_scale: [1.60, 2.60],
        aspect_max: 1.9,
        layers: SHARED_LAYERS,
        kernel: Kernel::Silver,
    },
    ProcessKind {
        cell_over_size: 4.0,
        mu: 1.02,
        salt: 0x5117_0203,
        variance: 0.05,
        sigma_scale: [3.20, 4.60],
        aspect_max: 1.70,
        layers: SHARED_LAYERS,
        kernel: Kernel::Dye,
    },
];

/// Below this many pixels of sigma, the photosite is the aperture: the whole
/// cloud lands in the pixel that contains its center, and the process is
/// drawn per pixel by [`Aggregate`].
const RESOLVED_SIGMA_PX: f32 = 0.75;

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

/// `∫ (g ∗ u)²` along one axis, where `g` is a unit-mass gaussian of
/// deviation `sigma` and `u` the unit-mass box of `width`: the energy one
/// cloud leaves after averaging over that box. The square of this is the 2D
/// figure for a square box.
fn box_energy(width: f64, sigma: f64) -> f64 {
    if sigma <= 0.0 {
        return 1.0 / width;
    }
    let ratio = width / (2.0 * sigma);
    if ratio < 1.0e-3 {
        // The box is a point: the gaussian's own energy, less a sliver.
        return (1.0 - ratio * ratio / 6.0) / (2.0 * std::f64::consts::PI.sqrt() * sigma);
    }
    let edge = 2.0 * sigma / std::f64::consts::PI.sqrt() * (1.0 - (-ratio * ratio).exp());
    (width * erf(ratio) - edge) / (width * width)
}

/// The error function. A Taylor series below 3, an asymptotic tail above,
/// both well under 1e-9 here. Only the calibration uses it.
fn erf(x: f64) -> f64 {
    if x < 0.0 {
        return -erf(-x);
    }
    if x < 3.0 {
        let mut term = x;
        let mut sum = x;
        for n in 1..80 {
            term *= -x * x / n as f64;
            let next = term / (2 * n + 1) as f64;
            sum += next;
            if next.abs() < 1.0e-17 {
                break;
            }
        }
        return 2.0 / std::f64::consts::PI.sqrt() * sum;
    }
    let inv2 = 1.0 / (x * x);
    let tail = (-x * x).exp() / (x * std::f64::consts::PI.sqrt())
        * (1.0 - 0.5 * inv2 + 0.75 * inv2 * inv2 - 1.875 * inv2 * inv2 * inv2);
    1.0 - tail
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

struct Cloud {
    x_mm: f32,
    y_mm: f32,
    sx: f32,
    sy: f32,
    sin: f32,
    cos: f32,
    amplitude: f32,
    layers: u8,
    kernel: Kernel,
}

/// Paint `cloud` into the rows of `band`.
///
/// Along a row a gaussian's exponent is quadratic in x, so the dye profile
/// steps by a ratio that itself steps by a constant factor: two `exp` per
/// row and one per cloud instead of one per pixel. The silver profile is a
/// polynomial in the distance already computed for the support test.
fn paint_cloud(band: &mut Band<'_>, target: Target, cloud: Cloud) {
    let Target {
        width,
        origin_x,
        origin_y,
        pitch,
        ..
    } = target;
    let px = cloud.x_mm / pitch - origin_x as f32;
    let py = cloud.y_mm / pitch - origin_y as f32;
    let sx = cloud.sx / pitch;
    let sy = cloud.sy / pitch;
    if sx <= 0.0 || sy <= 0.0 {
        return;
    }
    let support = cloud.kernel.support();
    let support_d2 = support * support;
    let rad = support * sx.max(sy);
    let last_row = band.first_row + band.rows - 1;
    // The row walk starts at the cloud's own left edge, clipped or not, so a
    // pixel's weight does not depend on where the frame or a crop begins.
    let left = (px - rad - 0.5).ceil() as i32;
    let x0 = left.max(0);
    let y0 = ((py - rad - 0.5).ceil() as i32).max(band.first_row as i32);
    let x1 = ((px + rad - 0.5).floor() as i32).min(width as i32 - 1);
    let y1 = ((py + rad - 0.5).floor() as i32).min(last_row as i32);
    if x0 > x1 || y0 > y1 {
        return;
    }
    let inv_sx = 1.0 / sx;
    let inv_sy = 1.0 / sy;
    // d2(dx) = a dx² + b dx + c along a row, with b and c set by the row.
    let (sin, cos) = (cloud.sin, cloud.cos);
    let a = cos * cos * inv_sx * inv_sx + sin * sin * inv_sy * inv_sy;
    let cross = 2.0 * sin * cos * (inv_sx * inv_sx - inv_sy * inv_sy);
    let across = sin * sin * inv_sx * inv_sx + cos * cos * inv_sy * inv_sy;
    let growth = (-a).exp();
    let start = left as f32 + 0.5 - px;
    for y in y0 as usize..=y1 as usize {
        let dy = y as f32 + 0.5 - py;
        let row = (y - band.first_row) * width;
        let b = cross * dy;
        let mut weight =
            cloud.amplitude * (-0.5 * (a * start * start + b * start + across * dy * dy)).exp();
        let mut step = (-0.5 * (a * (2.0 * start + 1.0) + b)).exp();
        for x in left..=x1 {
            if x >= x0 {
                let dx = x as f32 + 0.5 - px;
                let xr = dx * cos + dy * sin;
                let yr = -dx * sin + dy * cos;
                let d2 = xr * inv_sx * xr * inv_sx + yr * inv_sy * yr * inv_sy;
                if d2 < support_d2 {
                    let value = match cloud.kernel {
                        Kernel::Dye => weight,
                        Kernel::Silver => cloud.amplitude * Kernel::silver(d2),
                    };
                    let value = f64::from(value);
                    let index = (row + x as usize) * 3;
                    for layer in 0..3 {
                        if cloud.layers & (1 << layer) != 0 {
                            band.field[index + layer] += value;
                        }
                    }
                }
            }
            weight *= step;
            step *= growth;
        }
    }
}

/// The blur on the summed field.
///
/// When every cloud is resolved this is the scanner. Sigma is a fixed
/// length on the film, so a finer scan does not invent a harder grain and a
/// coarse preview does not blur twice. When some clouds are drawn per pixel,
/// it is the pixel's own spread: a cloud whose center sits in one pixel
/// still reaches its neighbors, so a preview keeps the neighbor correlation
/// an export shrunk to that size has, instead of reading as white noise.
#[derive(Clone, Copy)]
enum Aperture {
    /// A gaussian scanner spot of `sigma_px`.
    Scanner { sigma_px: f32, pitch: f32 },
    /// Where a scanned cloud of deviation `spread_px`, centered uniformly
    /// in a pixel, lands: that gaussian, averaged over the source pixel and
    /// binned into whole pixels.
    Pixel { spread_px: f32, pitch: f32 },
}

impl Aperture {
    /// About 6 µm, a tight scan of a 35 mm frame. Smaller than a Portra dye
    /// cloud, large enough to take the pixel grid out of a resolved cloud.
    const SCANNER_MM: f32 = 0.006;

    /// A scanner spot narrower than this is nearly the identity.
    const MIN_SIGMA_PX: f32 = 0.40;

    /// A pixel spread that keeps more than this of the mass at home is
    /// nearly the identity too.
    const MAX_CENTER_TAP: f32 = 0.995;

    fn for_frame(processes: &[Process], pitch: f32) -> Option<Self> {
        let scanner2 = Self::SCANNER_MM * Self::SCANNER_MM;
        let mut spread = 0.0_f32;
        let mut weight = 0.0_f32;
        for process in processes
            .iter()
            .filter(|process| process.active() && !process.resolved(pitch))
        {
            let share = process.variance * process.pixel_share(pitch);
            let sigma2 =
                process.kernel.sigma2_factor() * log_moment(process.sigma_lo, process.sigma_hi, 2);
            spread += share * (sigma2 + scanner2);
            weight += share;
        }
        if weight > 0.0 {
            let aperture = Self::Pixel {
                spread_px: (spread / weight).sqrt() / pitch,
                pitch,
            };
            let center = aperture.kernel()[aperture.pad() as usize];
            return (center < Self::MAX_CENTER_TAP).then_some(aperture);
        }
        let sigma_px = Self::SCANNER_MM / pitch;
        (sigma_px >= Self::MIN_SIGMA_PX).then_some(Self::Scanner { sigma_px, pitch })
    }

    fn pad(self) -> i32 {
        match self {
            Self::Scanner { sigma_px, .. } => (sigma_px * 3.0).ceil() as i32,
            Self::Pixel { spread_px, .. } => (spread_px * 3.0).ceil() as i32 + 1,
        }
    }

    /// Variance of the taps, in square pixels.
    fn variance_px2(self) -> f32 {
        match self {
            Self::Scanner { sigma_px, .. } => sigma_px * sigma_px,
            Self::Pixel { .. } => {
                let radius = self.pad();
                self.kernel()
                    .iter()
                    .enumerate()
                    .map(|(index, tap)| tap * (index as i32 - radius).pow(2) as f32)
                    .sum()
            }
        }
    }

    /// How much of a resolved cloud field's variance survives this blur.
    ///
    /// `sigma2_mm` is `E[σ²]` of the clouds' equivalent gaussian. An
    /// isotropic blur multiplies `∫k²` by `σ² / (σ² + s²)`.
    fn cloud_scale(self, sigma2_mm: f32) -> f32 {
        if sigma2_mm <= 0.0 {
            return 1.0;
        }
        let pitch = match self {
            Self::Scanner { pitch, .. } | Self::Pixel { pitch, .. } => pitch,
        };
        let blur2_mm = self.variance_px2() * pitch * pitch;
        sigma2_mm / (sigma2_mm + blur2_mm)
    }

    /// How much of a white field's variance survives this blur: the sum of
    /// squared taps, once per axis.
    fn white_scale(self) -> f32 {
        let taps: f32 = self.kernel().iter().map(|tap| tap * tap).sum();
        taps * taps
    }

    /// Normalized taps, `2 * pad + 1` of them.
    fn kernel(self) -> Vec<f32> {
        let radius = self.pad();
        let mut kernel: Vec<f32> = match self {
            Self::Scanner { sigma_px, .. } => (-radius..=radius)
                .map(|offset| (-0.5 * (offset as f32 / sigma_px).powi(2)).exp())
                .collect(),
            Self::Pixel { spread_px, .. } => {
                // The mass in pixel j is the second difference of
                // F(t) = tΦ(t) + φ(t), the integral of the normal CDF.
                let scale = 1.0 / f64::from(spread_px);
                let integral = |t: f64| {
                    let cdf = 0.5 * (1.0 + erf(t / std::f64::consts::SQRT_2));
                    let pdf = (-0.5 * t * t).exp() / (2.0 * std::f64::consts::PI).sqrt();
                    t * cdf + pdf
                };
                (-radius..=radius)
                    .map(|offset| {
                        let at = f64::from(offset) * scale;
                        let mass = integral(at + scale) - 2.0 * integral(at) + integral(at - scale);
                        (mass / scale).max(0.0) as f32
                    })
                    .collect()
            }
        };
        let weight: f32 = kernel.iter().sum();
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
        let stride = width * 3;
        let mut temp = vec![0.0_f64; field.len()];
        if control.cancelled() {
            return Err(Cancelled);
        }
        Workers::install(|| {
            temp.par_chunks_mut(stride)
                .zip(field.par_chunks(stride))
                .for_each(|(out, row)| {
                    for x in 0..width {
                        let mut acc = [0.0_f64; 3];
                        for (index, tap) in kernel.iter().enumerate() {
                            let src_x = (x as i32 + index as i32 - radius)
                                .clamp(0, width as i32 - 1)
                                as usize;
                            let tap = f64::from(*tap);
                            acc[0] += row[src_x * 3] * tap;
                            acc[1] += row[src_x * 3 + 1] * tap;
                            acc[2] += row[src_x * 3 + 2] * tap;
                        }
                        out[x * 3..x * 3 + 3].copy_from_slice(&acc);
                    }
                });
        });
        if control.cancelled() {
            return Err(Cancelled);
        }
        Workers::install(|| {
            field
                .par_chunks_mut(stride)
                .enumerate()
                .for_each(|(y, out)| {
                    for x in 0..width {
                        let mut acc = [0.0_f64; 3];
                        for (index, tap) in kernel.iter().enumerate() {
                            let src_y = (y as i32 + index as i32 - radius).clamp(0, rows as i32 - 1)
                                as usize;
                            let src = (src_y * width + x) * 3;
                            let tap = f64::from(*tap);
                            acc[0] += temp[src] * tap;
                            acc[1] += temp[src + 1] * tap;
                            acc[2] += temp[src + 2] * tap;
                        }
                        out[x * 3..x * 3 + 3].copy_from_slice(&acc);
                    }
                });
        });
        if control.cancelled() {
            return Err(Cancelled);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::NopControl;

    /// Mean and variance of `count` draws of `aggregate`.
    fn moments(aggregate: &Aggregate, count: i32) -> (f64, f64) {
        let mut sum = 0.0_f64;
        let mut square = 0.0_f64;
        for index in 0..count {
            let value = f64::from(aggregate.sample(index % 509, index / 509, 7));
            sum += value;
            square += value * value;
        }
        let mean = sum / f64::from(count);
        (mean, square / f64::from(count) - mean * mean)
    }

    #[test]
    fn aggregate_matches_the_compound_poisson_moments() {
        let process = &Process::table([0.010; 3], false)[0];
        let samples = 400_000;
        // A few clouds per pixel draws the count from the table. Hundreds is
        // gaussian.
        for (pitch, table) in [(0.012_f32, true), (0.15, false)] {
            assert!(!process.resolved(pitch));
            let amplitude = process.constants(pitch, None).amplitude;
            let aggregate = process.aggregate(amplitude, pitch);
            assert_eq!(!aggregate.cdf.is_empty(), table, "pitch {pitch}");
            let clouds = f64::from(process.mu) * f64::from(pitch / process.cell_mm).powi(2);
            let (strength_mean, strength_second) = strength_moments();
            let scale =
                f64::from(amplitude) * f64::from(process.kernel.mass()) / f64::from(pitch).powi(2);
            let expect_mean = clouds
                * scale
                * f64::from(strength_mean)
                * f64::from(log_moment(process.sigma_lo, process.sigma_hi, 2));
            let expect_variance = clouds
                * scale
                * scale
                * f64::from(strength_second)
                * f64::from(log_moment(process.sigma_lo, process.sigma_hi, 4));
            let (mean, variance) = moments(&aggregate, samples);
            let stderr = (expect_variance / f64::from(samples)).sqrt();
            assert!(
                (mean - expect_mean).abs() < 5.0 * stderr,
                "pitch {pitch}: mean {mean} expected {expect_mean}"
            );
            assert!(
                (variance / expect_variance - 1.0).abs() < 0.02,
                "pitch {pitch}: variance {variance} expected {expect_variance}"
            );
        }
    }

    #[test]
    fn unresolved_crop_matches_the_full_frame() {
        for monochrome in [false, true] {
            let grain = FilmGrain::new(35.0, 11, [0.018, 0.013, 0.010], monochrome);
            let frame_width = 360;
            let full = grain
                .render(
                    Plane {
                        width: frame_width,
                        rows: 240,
                        origin_x: 0,
                        origin_y: 0,
                        frame_width,
                    },
                    &mut NopControl,
                )
                .unwrap();
            let (x, y, width, rows) = (97_u32, 61_usize, 40_u32, 30_usize);
            let crop = grain
                .render(
                    Plane {
                        width,
                        rows,
                        origin_x: x,
                        origin_y: y as u32,
                        frame_width,
                    },
                    &mut NopControl,
                )
                .unwrap();
            for row in 0..rows {
                let src = ((y + row) * frame_width as usize + x as usize) * 3;
                let dst = row * width as usize * 3;
                let count = width as usize * 3;
                assert_eq!(crop[dst..dst + count], full[src..src + count], "row {row}");
            }
        }
    }

    #[test]
    fn erf_matches_known_values() {
        for (x, expect) in [
            (0.0, 0.0),
            (0.1, 0.112_462_916_018_284_9),
            (0.5, 0.520_499_877_813_046_5),
            (1.0, 0.842_700_792_949_714_9),
            (2.0, 0.995_322_265_018_952_7),
            (2.9, 0.999_958_902_121_900_6),
            (3.5, 0.999_999_256_901_627_7),
        ] {
            assert!((erf(x) - expect).abs() < 1.0e-9, "erf({x}) = {}", erf(x));
        }
    }

    #[test]
    fn box_energy_has_the_point_and_wide_limits() {
        let sigma = 0.01;
        let point = 1.0 / (2.0 * std::f64::consts::PI.sqrt() * sigma);
        assert!((box_energy(1.0e-6, sigma) / point - 1.0).abs() < 1.0e-6);
        assert!((box_energy(0.004, sigma) / point - 1.0).abs() < 0.01);
        // A box far wider than the cloud keeps its whole mass: 1 / width.
        let wide = 10.0;
        assert!((box_energy(wide, sigma) * wide - 1.0).abs() < 0.01);
        // The series and the closed form meet.
        let edge = 2.0 * sigma * 1.0e-3;
        let below = box_energy(edge * 0.999, sigma);
        let above = box_energy(edge * 1.001, sigma);
        assert!((below / above - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn silver_kernel_moments_match_the_profile() {
        // Integrate the profile on a fine grid in sigma units.
        let step = 0.01_f64;
        let reach = f64::from(Kernel::SILVER_RADIUS);
        let mut mass = 0.0;
        let mut energy = 0.0;
        let cells = (reach / step).ceil() as i32;
        for y in -cells..=cells {
            for x in -cells..=cells {
                let (dx, dy) = ((f64::from(x) + 0.5) * step, (f64::from(y) + 0.5) * step);
                let value = f64::from(Kernel::silver((dx * dx + dy * dy) as f32));
                mass += value * step * step;
                energy += value * value * step * step;
            }
        }
        assert!((mass / f64::from(Kernel::Silver.mass()) - 1.0).abs() < 0.005);
        assert!((energy / f64::from(Kernel::Silver.energy()) - 1.0).abs() < 0.005);
        assert!((Kernel::Silver.sigma2_factor() - 1.0).abs() < 0.01);
        assert!((Kernel::Dye.sigma2_factor() - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn a_pixel_sees_more_grain_than_the_aperture_and_a_preview_less() {
        let grain = FilmGrain::new(35.0, 3, [0.014, 0.011, 0.009], false);
        // 5.7 µm pixels: finer than the clouds' reach through 48 µm.
        let fine = grain.pixel_ratio(6160);
        // 49 µm pixels: about the aperture itself.
        let aperture = grain.pixel_ratio(714);
        // 0.35 mm pixels average many apertures.
        let coarse = grain.pixel_ratio(100);
        for layer in 0..3 {
            assert!(fine[layer] > 1.1, "fine {fine:?}");
            assert!(
                (aperture[layer] - 1.0).abs() < 0.12,
                "aperture {aperture:?}"
            );
            assert!(coarse[layer] < 0.2, "coarse {coarse:?}");
        }
        // Smaller clouds read stronger at a fine pitch for the same RMS.
        let small = FilmGrain::new(35.0, 3, [0.006; 3], true).pixel_ratio(6160);
        let large = FilmGrain::new(35.0, 3, [0.012; 3], true).pixel_ratio(6160);
        assert!(small[0] > large[0] * 1.2, "small {small:?} large {large:?}");
    }
}
