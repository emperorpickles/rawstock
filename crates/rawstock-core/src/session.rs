//! Cached develop session for the live tuner.
//!
//! The session holds one full-resolution ACEScg image and a fit proxy. It
//! does not open files, draw a window, or start a thread. The camera
//! orientation, and any manual quarter-turn, is baked in after demosaic and
//! before the proxy. A tweak dirties the first stage it affects; earlier
//! buffers stay. The fit view's dye scan can use a lookup table. Export and
//! [`Self::render_full`] use the exact scan. The tuner asks for that full
//! frame once per settings state and pans the bitmap in the window.

use crate::color::{DevelopError, LinearImage};
use crate::decode::{FrameOrientation, Orientation};
use crate::render::{self, Cancelled, DevelopControl, Fidelity, Parameters, SrgbImage, Stage};
use crate::stock::{FilmModel, Grain, Halation, ModelError};

/// Long edge of the fit proxy while a control is moving.
pub const DRAG_LONG_EDGE: u32 = 720;

/// Long edge of the fit proxy once the worker has been idle.
pub const SETTLED_LONG_EDGE: u32 = 1440;

/// Lattice edge of the dye-scan lookup table.
const DYE_LUT_EDGE: usize = 48;

/// Display-space rectangle, in pixels of the oriented image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRect {
    /// Left edge in the oriented image.
    pub x: u32,
    /// Top edge in the oriented image.
    pub y: u32,
    /// Width in the oriented image.
    pub width: u32,
    /// Height in the oriented image.
    pub height: u32,
}

impl DisplayRect {
    /// Intersection with an image of `width` by `height`. Empty when it misses.
    pub fn clamped(self, width: u32, height: u32) -> Self {
        if width == 0
            || height == 0
            || self.width == 0
            || self.height == 0
            || self.x >= width
            || self.y >= height
        {
            return Self {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        }
        Self {
            x: self.x,
            y: self.y,
            width: self.width.min(width - self.x),
            height: self.height.min(height - self.y),
        }
    }
}

/// How a render treats spectral dye curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Lookup table. The fit view, and a 100% crop while a control is moving.
    Preview,
    /// Exact `printing_density`. Settled 100% and every export.
    Exact,
}

/// Highlight halation as the Milestone 1 controls show it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HalationControls {
    /// The stock currently applies halation.
    pub enabled: bool,
    /// Scene-linear peak. The slider is logarithmic.
    pub threshold: f32,
    /// Gaussian sigma as a fraction of the shorter full-frame side.
    pub radius: f32,
    /// Red, green, and blue add-back. Milestone 1 edits red only.
    pub weight: [f32; 3],
}

/// One open file and the stock being edited.
///
/// `new` does not copy a stock in. Call [`Self::set_parameters`] before
/// rendering. The full ACEScg buffer stays until the session is dropped;
/// switching stocks does not decode again. Turning the image reuses that
/// buffer and rebuilds the proxy.
pub struct PreviewSession {
    /// Demosaic size, kept so a turn can report the sensor frame.
    sensor_width: u32,
    sensor_height: u32,
    /// `true` while `full` is a quick develop standing in for the full one.
    /// See [`Self::staged`].
    staged: bool,
    /// Camera tag plus manual quarter-turns. Already baked into `full`.
    view: FrameOrientation,
    full: LinearImage,
    proxy_long_edge: u32,
    proxy: LinearImage,
    current: Option<LoadedStock>,
    baseline: Option<LoadedStock>,
    proxy_cache: FrameCache,
    baseline_proxy: FrameCache,
    region: Option<RegionCache>,
    baseline_region: Option<RegionCache>,
    dye_lut: Option<DyeLut>,
    /// Renders on this device when set, and on the CPU when it fails.
    #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
    gpu: Option<crate::gpu::Gpu>,
}

#[derive(Clone)]
struct LoadedStock {
    parameters: Parameters,
    base_grain: Option<Grain>,
    grain_amount: f32,
    base_film: FilmModel,
    dye_scales: [f32; 3],
    halation_memory: Option<Halation>,
}

impl LoadedStock {
    fn new(parameters: Parameters) -> Self {
        Self {
            base_grain: parameters.grain().cloned(),
            grain_amount: if parameters.grain().is_some() {
                1.0
            } else {
                0.0
            },
            base_film: parameters.film().clone(),
            dye_scales: [1.0; 3],
            halation_memory: parameters.halation().cloned(),
            parameters,
        }
    }
}

/// Splatted dye-cloud field. RMS is not part of the key: an amount change
/// reuses the clouds and only remultiplies density.
struct CloudField {
    frame_mm: u32,
    seed: u32,
    size_mm: [u32; 3],
    origin_x: u32,
    origin_y: u32,
    width: u32,
    rows: u32,
    frame_width: u32,
    samples: Vec<f32>,
}

impl CloudField {
    fn matches(&self, grain: &Grain, region: crate::stock::GrainRegion, rows: usize) -> bool {
        self.frame_mm == grain.frame_mm().to_bits()
            && self.seed == grain.seed()
            && self.size_mm == grain.size_mm().map(f32::to_bits)
            && self.origin_x == region.origin_x
            && self.origin_y == region.origin_y
            && self.width == region.width
            && self.rows == rows as u32
            && self.frame_width == region.frame_width
    }

    fn store(
        grain: &Grain,
        region: crate::stock::GrainRegion,
        rows: usize,
        samples: Vec<f32>,
    ) -> Self {
        Self {
            frame_mm: grain.frame_mm().to_bits(),
            seed: grain.seed(),
            size_mm: grain.size_mm().map(f32::to_bits),
            origin_x: region.origin_x,
            origin_y: region.origin_y,
            width: region.width,
            rows: rows as u32,
            frame_width: region.frame_width,
            samples,
        }
    }
}

struct FrameCache {
    exposure: Option<Vec<f32>>,
    halation: Option<Vec<f32>>,
    density: Option<Vec<f32>>,
    grain: Option<Vec<f32>>,
    grain_field: Option<CloudField>,
    dye: Option<Vec<f32>>,
    dye_preview: bool,
    printed: Option<Vec<f32>>,
    image: Option<SrgbImage>,
}

impl FrameCache {
    fn fresh() -> Self {
        Self {
            exposure: None,
            halation: None,
            density: None,
            grain: None,
            grain_field: None,
            dye: None,
            dye_preview: false,
            printed: None,
            image: None,
        }
    }

    fn invalidate(&mut self, stage: Stage) {
        if stage <= Stage::Exposure {
            self.exposure = None;
        }
        if stage <= Stage::Halation {
            self.halation = None;
        }
        if stage <= Stage::Density {
            self.density = None;
        }
        if stage <= Stage::Grain {
            self.grain = None;
        }
        if stage <= Stage::DyeScan {
            self.dye = None;
        }
        if stage <= Stage::Print {
            self.printed = None;
        }
        self.image = None;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RegionKey {
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
    pad_x: u32,
    pad_y: u32,
    padded_w: u32,
    padded_h: u32,
}

struct RegionCache {
    key: RegionKey,
    padded_exposure: Option<Vec<f32>>,
    visible: Option<Vec<f32>>,
    density: Option<Vec<f32>>,
    grain: Option<Vec<f32>>,
    grain_field: Option<CloudField>,
    dye: Option<Vec<f32>>,
    dye_preview: bool,
    printed: Option<Vec<f32>>,
    image: Option<SrgbImage>,
}

impl RegionCache {
    fn fresh(key: RegionKey) -> Self {
        Self {
            key,
            padded_exposure: None,
            visible: None,
            density: None,
            grain: None,
            grain_field: None,
            dye: None,
            dye_preview: false,
            printed: None,
            image: None,
        }
    }

    fn matches(&self, key: RegionKey) -> bool {
        self.key == key
    }

    fn invalidate(&mut self, stage: Stage) {
        if stage <= Stage::Exposure {
            self.padded_exposure = None;
        }
        if stage <= Stage::Halation {
            self.visible = None;
        }
        if stage <= Stage::Density {
            self.density = None;
        }
        if stage <= Stage::Grain {
            self.grain = None;
        }
        if stage <= Stage::DyeScan {
            self.dye = None;
        }
        if stage <= Stage::Print {
            self.printed = None;
        }
        self.image = None;
    }
}

impl PreviewSession {
    /// Cache `linear` and build the fit proxy.
    ///
    /// `orientation` is the camera EXIF tag. It is applied to `linear`
    /// before the proxy, so portrait files are already upright and grain
    /// is generated in that frame. `proxy_long_edge` is the proxy's longer
    /// side in those oriented pixels.
    pub fn new(
        linear: LinearImage,
        orientation: Orientation,
        proxy_long_edge: u32,
    ) -> Result<Self, DevelopError> {
        let sensor_width = linear.width();
        let sensor_height = linear.height();
        let view = FrameOrientation::new(orientation);
        let mut full = linear;
        full.apply_orientation(view.composed());
        let proxy = full.area_downsample(proxy_long_edge)?;
        Ok(Self {
            sensor_width,
            sensor_height,
            staged: false,
            view,
            full,
            proxy_long_edge,
            proxy,
            current: None,
            baseline: None,
            proxy_cache: FrameCache::fresh(),
            baseline_proxy: FrameCache::fresh(),
            region: None,
            baseline_region: None,
            dye_lut: None,
            #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
            gpu: None,
        })
    }

    /// Session on a quick develop while the full one is still running.
    ///
    /// The fit proxy is built from `quick` right away, so the first preview
    /// does not wait for the demosaic. Dimensions already report the full
    /// frame. [`Self::render_region`] and [`Self::render_full`] need the full
    /// image: install it with [`Self::install_full`] first. Until then they
    /// return [`Cancelled`].
    pub fn staged(
        quick: crate::color::QuickDevelop,
        orientation: Orientation,
        proxy_long_edge: u32,
    ) -> Result<Self, DevelopError> {
        let mut session = Self::new(quick.image, orientation, proxy_long_edge)?;
        session.sensor_width = quick.full_width;
        session.sensor_height = quick.full_height;
        session.staged = true;
        Ok(session)
    }

    /// `true` until [`Self::install_full`] replaces a quick develop.
    pub fn is_staged(&self) -> bool {
        self.staged
    }

    /// Replace the quick develop with the full one, keeping the stock,
    /// edits, A/B baseline, and turns. The proxy is rebuilt from it.
    pub fn install_full(&mut self, mut linear: LinearImage) -> Result<(), DevelopError> {
        if (linear.width(), linear.height()) != (self.sensor_width, self.sensor_height) {
            return Err(DevelopError::FrameMismatch {
                width: self.sensor_width,
                height: self.sensor_height,
                actual_width: linear.width(),
                actual_height: linear.height(),
            });
        }
        linear.apply_orientation(self.view.composed());
        let proxy = linear.area_downsample(self.proxy_long_edge)?;
        self.full = linear;
        self.proxy = proxy;
        self.staged = false;
        self.proxy_cache = FrameCache::fresh();
        self.baseline_proxy = FrameCache::fresh();
        self.region = None;
        self.baseline_region = None;
        Ok(())
    }

    /// Render on `gpu` from now on, or on the CPU with `None`.
    ///
    /// The GPU path runs every stage on each render and keeps no stage
    /// cache. Its dye scan is always exact, so every image it returns is
    /// [`Fidelity::Exact`]. A GPU error falls back to the CPU for that
    /// render.
    #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
    pub fn set_gpu(&mut self, gpu: Option<crate::gpu::Gpu>) {
        self.gpu = gpu;
    }

    /// The device renders run on, when there is one.
    #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
    pub fn gpu(&self) -> Option<&crate::gpu::Gpu> {
        self.gpu.as_ref()
    }

    /// Width and height of the oriented working image. While staged, the
    /// full frame's.
    pub fn dimensions(&self) -> (u32, u32) {
        if self.staged {
            return self
                .view
                .composed()
                .dimensions(self.sensor_width, self.sensor_height);
        }
        (self.full.width(), self.full.height())
    }

    /// Width and height of the demosaiced frame, before orientation.
    pub fn sensor_dimensions(&self) -> (u32, u32) {
        (self.sensor_width, self.sensor_height)
    }

    /// Width and height of the oriented image the preview and export share.
    pub fn display_dimensions(&self) -> (u32, u32) {
        self.dimensions()
    }

    /// Camera tag composed with the manual quarter-turns.
    pub fn orientation(&self) -> Orientation {
        self.view.composed()
    }

    /// EXIF orientation stored in the file.
    pub fn camera_orientation(&self) -> Orientation {
        self.view.camera()
    }

    /// Clockwise quarter-turns added on top of the camera tag, in `0..4`.
    pub fn quarter_turns(&self) -> u8 {
        self.view.turns()
    }

    /// Turn the working image 90° clockwise and rebuild the proxy.
    ///
    /// The demosaic buffer is not decoded again. Stage caches are dropped
    /// because every pixel moved.
    pub fn rotate_clockwise(&mut self) -> Result<(), DevelopError> {
        self.reorient(self.view.rotate_clockwise())
    }

    /// Turn the working image 90° counter-clockwise and rebuild the proxy.
    pub fn rotate_counterclockwise(&mut self) -> Result<(), DevelopError> {
        self.reorient(self.view.rotate_counterclockwise())
    }

    fn reorient(&mut self, next: FrameOrientation) -> Result<(), DevelopError> {
        let delta = self.view.delta_to(next);
        self.full.apply_orientation(delta);
        self.view = next;
        self.proxy = self.full.area_downsample(self.proxy_long_edge)?;
        self.proxy_cache = FrameCache::fresh();
        self.baseline_proxy = FrameCache::fresh();
        self.region = None;
        self.baseline_region = None;
        Ok(())
    }

    /// Proxy width and height at the current long edge.
    pub fn proxy_dimensions(&self) -> (u32, u32) {
        (self.proxy.width(), self.proxy.height())
    }

    /// The proxy's longer side.
    pub fn proxy_long_edge(&self) -> u32 {
        self.proxy_long_edge
    }

    /// Replace the fit proxy. The full ACEScg image is unchanged.
    pub fn set_proxy_long_edge(&mut self, long_edge: u32) -> Result<(), DevelopError> {
        if long_edge == self.proxy_long_edge {
            return Ok(());
        }
        self.proxy = self.full.area_downsample(long_edge)?;
        self.proxy_long_edge = long_edge;
        self.proxy_cache = FrameCache::fresh();
        self.baseline_proxy = FrameCache::fresh();
        Ok(())
    }

    /// Load a stock. The A/B baseline becomes this stock, and dye scales and
    /// grain amount reset to 1. Decode is not repeated.
    pub fn set_parameters(&mut self, parameters: Parameters) -> Stage {
        let stage = self
            .current
            .as_ref()
            .map(|loaded| first_dirty(&loaded.parameters, &parameters))
            .unwrap_or(Stage::Exposure);
        let loaded = LoadedStock::new(parameters);
        self.baseline = Some(loaded.clone());
        self.current = Some(loaded);
        self.proxy_cache.invalidate(stage);
        self.baseline_proxy = FrameCache::fresh();
        self.invalidate_region(stage);
        self.baseline_region = None;
        stage
    }

    /// Parameters currently being edited.
    pub fn parameters(&self) -> &Parameters {
        &self.loaded().parameters
    }

    /// Parameters of the A/B "before" image: the stock as opened, or as last
    /// committed with [`Self::commit_edit`].
    pub fn baseline_parameters(&self) -> &Parameters {
        &self.baseline().parameters
    }

    /// Exposure in stops.
    pub fn set_exposure_ev(&mut self, exposure_ev: f32) -> Result<Stage, ModelError> {
        let stage = self.loaded_mut().parameters.set_exposure_ev(exposure_ev)?;
        self.invalidate_current(stage);
        Ok(stage)
    }

    /// Print log-exposure offset.
    pub fn set_print_exposure(&mut self, print_exposure: f32) -> Result<Stage, ModelError> {
        let stage = self
            .loaded_mut()
            .parameters
            .set_print_exposure(print_exposure)?;
        self.invalidate_current(stage);
        Ok(stage)
    }

    /// Replace the print curve.
    pub fn set_print_curve(&mut self, curve: crate::stock::Curve) -> Result<Stage, ModelError> {
        let stage = self.loaded_mut().parameters.set_print_curve(curve)?;
        self.invalidate_current(stage);
        Ok(stage)
    }

    /// Replace the film model and reset the dye-scale base to it.
    pub fn set_film(&mut self, film: FilmModel) -> Result<Stage, ModelError> {
        let stage = {
            let loaded = self.loaded_mut();
            let stage = loaded.parameters.set_film(film.clone())?;
            loaded.base_film = film;
            loaded.dye_scales = [1.0; 3];
            stage
        };
        self.invalidate_current(stage);
        Ok(stage)
    }

    /// Multiply the loaded dye curves by `scales`.
    ///
    /// Scales start at 1, meaning the file as loaded. A parametric stock, or
    /// a spectral stock without dye curves, returns [`ModelError::DyeDensity`].
    pub fn set_dye_scales(&mut self, scales: [f32; 3]) -> Result<Stage, ModelError> {
        let stage = {
            let loaded = self.loaded_mut();
            let FilmModel::Spectral(base) = &loaded.base_film else {
                return Err(ModelError::DyeDensity);
            };
            let scaled = FilmModel::Spectral(Box::new(base.with_dye_scales(scales)?));
            let stage = loaded.parameters.set_film(scaled)?;
            loaded.dye_scales = scales;
            stage
        };
        self.invalidate_current(stage);
        Ok(stage)
    }

    /// Dye-scale multipliers. `None` when this stock has no dye curves.
    pub fn dye_scales(&self) -> Option<[f32; 3]> {
        let loaded = self.loaded();
        match loaded.parameters.film() {
            FilmModel::Spectral(model) if model.dye_density().is_some() => Some(loaded.dye_scales),
            _ => None,
        }
    }

    /// Scale the loaded grain. `0` omits grain. `1` is the file.
    pub fn set_grain_amount(&mut self, amount: f32) -> Result<Stage, ModelError> {
        if !amount.is_finite() || amount < 0.0 {
            return Err(ModelError::GranularitySample);
        }
        let stage = {
            let loaded = self.loaded_mut();
            loaded.grain_amount = amount;
            let grain = match &loaded.base_grain {
                Some(base) if amount > 0.0 => Some(base.scaled(amount)?),
                _ => None,
            };
            loaded.parameters.set_grain(grain)?
        };
        self.invalidate_current(stage);
        Ok(stage)
    }

    /// Grain amount multiplier. `0` when the stock has no grain.
    pub fn grain_amount(&self) -> f32 {
        self.loaded().grain_amount
    }

    /// Whether the loaded stock has a grain table to scale.
    pub fn grain_available(&self) -> bool {
        self.loaded().base_grain.is_some()
    }

    /// Turn halation on or off. Off omits the table. On restores the last
    /// values, or a faint red fringe when the stock never had one.
    pub fn set_halation_enabled(&mut self, enabled: bool) -> Result<Stage, ModelError> {
        let stage = {
            let loaded = self.loaded_mut();
            let next = if enabled {
                Some(
                    loaded
                        .halation_memory
                        .clone()
                        .unwrap_or_else(default_halation),
                )
            } else {
                if let Some(current) = loaded.parameters.halation().cloned() {
                    loaded.halation_memory = Some(current);
                }
                None
            };
            loaded.parameters.set_halation(next)?
        };
        self.invalidate_current(stage);
        Ok(stage)
    }

    /// Set the halation threshold, keeping radius and weights.
    pub fn set_halation_threshold(&mut self, threshold: f32) -> Result<Stage, ModelError> {
        self.edit_halation(|halation| {
            Halation::new(threshold, halation.radius(), halation.weight())
        })
    }

    /// Set the halation radius as a fraction of the shorter full-frame side.
    pub fn set_halation_radius(&mut self, radius: f32) -> Result<Stage, ModelError> {
        self.edit_halation(|halation| {
            Halation::new(halation.threshold(), radius, halation.weight())
        })
    }

    /// Set the red halation weight. Green and blue stay as loaded.
    pub fn set_halation_red(&mut self, red: f32) -> Result<Stage, ModelError> {
        self.edit_halation(|halation| {
            let mut weight = halation.weight();
            weight[0] = red;
            Halation::new(halation.threshold(), halation.radius(), weight)
        })
    }

    /// Halation controls, including the values remembered while the stage is off.
    pub fn halation_controls(&self) -> HalationControls {
        let loaded = self.loaded();
        let enabled = loaded.parameters.halation().is_some();
        let source = loaded
            .parameters
            .halation()
            .cloned()
            .or_else(|| loaded.halation_memory.clone())
            .unwrap_or_else(default_halation);
        HalationControls {
            enabled,
            threshold: source.threshold(),
            radius: source.radius(),
            weight: source.weight(),
        }
    }

    /// Make the current edit the A/B baseline.
    ///
    /// Dye scales reset to 1 and the baked curves become the new base, so a
    /// second save does not square the scale. Grain amount resets the same way.
    pub fn commit_edit(&mut self) {
        let mut loaded = self.loaded().clone();
        loaded.base_film = loaded.parameters.film().clone();
        loaded.dye_scales = [1.0; 3];
        loaded.base_grain = loaded.parameters.grain().cloned();
        loaded.grain_amount = if loaded.base_grain.is_some() {
            1.0
        } else {
            0.0
        };
        if let Some(halation) = loaded.parameters.halation().cloned() {
            loaded.halation_memory = Some(halation);
        }
        self.baseline = Some(loaded.clone());
        self.current = Some(loaded);
        self.baseline_proxy = FrameCache::fresh();
        self.baseline_region = None;
    }

    /// Fit proxy of the current edit. Spectral dye curves use the lookup table.
    pub fn render_proxy(
        &mut self,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        self.render_fit(false, control)
    }

    /// Fit proxy of the A/B baseline, with the same lookup rule as [`Self::render_proxy`].
    pub fn render_baseline_proxy(
        &mut self,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        self.render_fit(true, control)
    }

    /// 100% crop. `rect` is in the oriented image. `scan` selects the dye path.
    ///
    /// Halation's sigma and grain's cell come from the oriented full frame.
    /// Grain's hash uses the crop origin in that frame, so the crop matches
    /// the same pixels of an export.
    pub fn render_region(
        &mut self,
        rect: DisplayRect,
        scan: ScanMode,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        self.render_crop(false, rect, scan, control)
    }

    /// 100% crop of the A/B baseline.
    pub fn render_baseline_region(
        &mut self,
        rect: DisplayRect,
        scan: ScanMode,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        self.render_crop(true, rect, scan, control)
    }

    /// Exact full frame, the image export writes. The lookup table is not used.
    ///
    /// Nothing from this call is kept on the session. The caller holds the
    /// encoded frame if it wants to pan it.
    pub fn render_full(
        &mut self,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        if self.staged {
            return Err(Cancelled);
        }
        let parameters = self.loaded().parameters.clone();
        #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
        if let Some(image) = self.gpu_render(&parameters, &self.full, None, control)? {
            return Ok(image);
        }
        render::Pipeline::new(parameters).develop_controlled(&self.full, control)
    }

    /// Exact full frame of the A/B baseline. Same path as [`Self::render_full`].
    pub fn render_baseline_full(
        &mut self,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        if self.staged {
            return Err(Cancelled);
        }
        let parameters = self.baseline().parameters.clone();
        #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
        if let Some(image) = self.gpu_render(&parameters, &self.full, None, control)? {
            return Ok(image);
        }
        render::Pipeline::new(parameters).develop_controlled(&self.full, control)
    }

    /// `frame` on the GPU, or `None` when there is no device or it failed.
    ///
    /// `crop` is the full-frame halation sigma, the padded crop's origin in
    /// the full frame, and the full frame's width.
    #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
    fn gpu_render(
        &self,
        parameters: &Parameters,
        frame: &LinearImage,
        crop: Option<(Option<f32>, u32, u32, u32)>,
        control: &mut dyn DevelopControl,
    ) -> Result<Option<SrgbImage>, Cancelled> {
        let Some(gpu) = &self.gpu else {
            return Ok(None);
        };
        let (width, height) = (frame.width(), frame.height());
        let (sigma, origin_x, origin_y, frame_width) = crop.unwrap_or((None, 0, 0, width));
        render::begin(Stage::Exposure, height, control)?;
        let pipeline = crate::gpu::GpuPipeline::new(gpu, parameters);
        let region = crate::stock::GrainRegion {
            width,
            height,
            origin_x,
            origin_y,
            frame_width,
        };
        let developed = pollster::block_on(pipeline.develop_frame(
            frame.pixels(),
            width,
            height,
            sigma,
            region,
        ));
        let Ok(pixels) = developed else {
            return Ok(None);
        };
        control.on_progress(Stage::Encode, height, height);
        if control.cancelled() {
            return Err(Cancelled);
        }
        Ok(Some(SrgbImage::from_encoded(
            width,
            height,
            pixels,
            Fidelity::Exact,
        )))
    }

    fn render_fit(
        &mut self,
        baseline: bool,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        let parameters = self.stock(baseline).parameters.clone();
        #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
        if let Some(image) = self.gpu_render(&parameters, &self.proxy, None, control)? {
            return Ok(image);
        }
        let lut = self.prepare_lut(&parameters, ScanMode::Preview, control)?;
        if baseline {
            render_proxy_cache(
                &self.proxy,
                &parameters,
                &mut self.baseline_proxy,
                lut.as_ref(),
                control,
            )
        } else {
            render_proxy_cache(
                &self.proxy,
                &parameters,
                &mut self.proxy_cache,
                lut.as_ref(),
                control,
            )
        }
    }

    fn render_crop(
        &mut self,
        baseline: bool,
        rect: DisplayRect,
        scan: ScanMode,
        control: &mut dyn DevelopControl,
    ) -> Result<SrgbImage, Cancelled> {
        if self.staged {
            return Err(Cancelled);
        }
        let parameters = self.stock(baseline).parameters.clone();
        let rect = rect.clamped(self.full.width(), self.full.height());
        let (origin_x, origin_y, width, height) = (rect.x, rect.y, rect.width, rect.height);
        if width == 0 || height == 0 {
            return Ok(SrgbImage::from_encoded(0, 0, Vec::new(), Fidelity::Exact));
        }
        let pad = parameters
            .halation()
            .map(|halation| halation.support_px(self.full.width(), self.full.height()))
            .unwrap_or(0);
        let pad_x = origin_x.min(pad);
        let pad_y = origin_y.min(pad);
        let pad_right = (self.full.width() - (origin_x + width)).min(pad);
        let pad_bottom = (self.full.height() - (origin_y + height)).min(pad);
        let padded_x = origin_x - pad_x;
        let padded_y = origin_y - pad_y;
        let padded_w = width + pad_x + pad_right;
        let padded_h = height + pad_y + pad_bottom;
        #[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
        if self.gpu.is_some() {
            // Grain depends only on the full-frame coordinate, so the padded
            // crop's grain is the crop's grain, and the padding is cut after.
            let padded = LinearImage::new(
                padded_w,
                padded_h,
                extract(&self.full, padded_x, padded_y, padded_w, padded_h),
            )
            .expect("extract matches its size");
            let sigma = parameters
                .halation()
                .map(|halation| halation.sigma_px(self.full.width(), self.full.height()));
            let crop = Some((sigma, padded_x, padded_y, self.full.width()));
            if let Some(image) = self.gpu_render(&parameters, &padded, crop, control)? {
                let pixels = crop_buffer(image.pixels(), padded_w, pad_x, pad_y, width, height);
                return Ok(SrgbImage::from_encoded(
                    width,
                    height,
                    pixels,
                    Fidelity::Exact,
                ));
            }
        }
        let key = RegionKey {
            origin_x,
            origin_y,
            width,
            height,
            pad_x,
            pad_y,
            padded_w,
            padded_h,
        };
        let mismatch = self
            .region_slot(baseline)
            .as_ref()
            .is_none_or(|cached| !cached.matches(key));
        if mismatch {
            *self.region_slot(baseline) = Some(RegionCache::fresh(key));
        }
        let needs_exposure = self
            .region_slot(baseline)
            .as_ref()
            .expect("region")
            .padded_exposure
            .is_none();
        let extracted = if needs_exposure {
            Some(extract(&self.full, padded_x, padded_y, padded_w, padded_h))
        } else {
            None
        };
        let sigma = parameters
            .halation()
            .map(|halation| halation.sigma_px(self.full.width(), self.full.height()));
        let full_width = self.full.width();
        let lut = self.prepare_lut(&parameters, scan, control)?;
        let region = if baseline {
            self.baseline_region.as_mut().expect("region")
        } else {
            self.region.as_mut().expect("region")
        };
        if let Some(extracted) = extracted {
            region.padded_exposure = Some(render::exposure_buffer(
                &extracted,
                parameters.exposure_ev(),
                control,
            )?);
            region.visible = None;
        }
        if region.visible.is_none() {
            let padded = region.padded_exposure.clone().expect("padded exposure");
            let blurred = render::halation_buffer(
                padded,
                parameters.halation(),
                padded_w,
                padded_h,
                sigma,
                control,
            )?;
            region.visible = Some(crop_buffer(&blurred, padded_w, pad_x, pad_y, width, height));
            region.density = None;
        }
        let visible = region.visible.clone().expect("visible crop");
        render_tail(
            TailRequest {
                exposure: visible,
                width,
                height,
                parameters: &parameters,
                origin_x,
                origin_y,
                frame_width: full_width,
                scan,
                lut: lut.as_ref(),
            },
            TailCache {
                density: &mut region.density,
                grain: &mut region.grain,
                grain_field: &mut region.grain_field,
                dye: &mut region.dye,
                dye_preview: &mut region.dye_preview,
                printed: &mut region.printed,
                image: &mut region.image,
            },
            control,
        )
    }

    fn prepare_lut(
        &mut self,
        parameters: &Parameters,
        scan: ScanMode,
        control: &mut dyn DevelopControl,
    ) -> Result<Option<DyeLut>, Cancelled> {
        if scan == ScanMode::Exact {
            return Ok(None);
        }
        let key = DyeKey::from_film(parameters.film());
        let Some(key) = key else {
            return Ok(None);
        };
        if self.dye_lut.as_ref().is_none_or(|lut| lut.key != key) {
            self.dye_lut = Some(DyeLut::build(parameters.film(), key, control)?);
        }
        Ok(self.dye_lut.clone())
    }

    fn edit_halation(
        &mut self,
        edit: impl FnOnce(&Halation) -> Result<Halation, ModelError>,
    ) -> Result<Stage, ModelError> {
        let (stage, enabled) = {
            let loaded = self.loaded_mut();
            let enabled = loaded.parameters.halation().is_some();
            let base = loaded
                .parameters
                .halation()
                .cloned()
                .or_else(|| loaded.halation_memory.clone())
                .unwrap_or_else(default_halation);
            let next = edit(&base)?;
            loaded.halation_memory = Some(next.clone());
            let stage = if enabled {
                loaded.parameters.set_halation(Some(next))?
            } else {
                Stage::Halation
            };
            (stage, enabled)
        };
        if enabled {
            self.invalidate_current(stage);
        }
        Ok(stage)
    }

    fn region_slot(&mut self, baseline: bool) -> &mut Option<RegionCache> {
        if baseline {
            &mut self.baseline_region
        } else {
            &mut self.region
        }
    }

    fn invalidate_current(&mut self, stage: Stage) {
        self.proxy_cache.invalidate(stage);
        self.invalidate_region(stage);
    }

    fn invalidate_region(&mut self, stage: Stage) {
        if let Some(region) = &mut self.region {
            region.invalidate(stage);
        }
    }

    fn loaded(&self) -> &LoadedStock {
        self.current
            .as_ref()
            .expect("load a stock before rendering")
    }

    fn loaded_mut(&mut self) -> &mut LoadedStock {
        self.current.as_mut().expect("load a stock before editing")
    }

    fn stock(&self, baseline: bool) -> &LoadedStock {
        if baseline {
            self.baseline()
        } else {
            self.loaded()
        }
    }

    fn baseline(&self) -> &LoadedStock {
        self.baseline
            .as_ref()
            .expect("load a stock before comparing")
    }
}

fn render_proxy_cache(
    source: &LinearImage,
    parameters: &Parameters,
    cache: &mut FrameCache,
    lut: Option<&DyeLut>,
    control: &mut dyn DevelopControl,
) -> Result<SrgbImage, Cancelled> {
    if cache.exposure.is_none() {
        cache.exposure = Some(render::exposure_buffer(
            source.pixels(),
            parameters.exposure_ev(),
            control,
        )?);
        cache.halation = None;
        cache.density = None;
    }
    if cache.halation.is_none() {
        let exposure = cache.exposure.clone().expect("exposure");
        cache.halation = Some(render::halation_buffer(
            exposure,
            parameters.halation(),
            source.width(),
            source.height(),
            None,
            control,
        )?);
        cache.density = None;
    }
    let halation = cache.halation.clone().expect("halation");
    render_tail(
        TailRequest {
            exposure: halation,
            width: source.width(),
            height: source.height(),
            parameters,
            origin_x: 0,
            origin_y: 0,
            frame_width: source.width(),
            scan: ScanMode::Preview,
            lut,
        },
        TailCache {
            density: &mut cache.density,
            grain: &mut cache.grain,
            grain_field: &mut cache.grain_field,
            dye: &mut cache.dye,
            dye_preview: &mut cache.dye_preview,
            printed: &mut cache.printed,
            image: &mut cache.image,
        },
        control,
    )
}

struct TailRequest<'a> {
    exposure: Vec<f32>,
    width: u32,
    height: u32,
    parameters: &'a Parameters,
    origin_x: u32,
    origin_y: u32,
    frame_width: u32,
    scan: ScanMode,
    lut: Option<&'a DyeLut>,
}

struct TailCache<'a> {
    density: &'a mut Option<Vec<f32>>,
    grain: &'a mut Option<Vec<f32>>,
    grain_field: &'a mut Option<CloudField>,
    dye: &'a mut Option<Vec<f32>>,
    dye_preview: &'a mut bool,
    printed: &'a mut Option<Vec<f32>>,
    image: &'a mut Option<SrgbImage>,
}

fn render_tail(
    request: TailRequest<'_>,
    cache: TailCache<'_>,
    control: &mut dyn DevelopControl,
) -> Result<SrgbImage, Cancelled> {
    let TailRequest {
        exposure,
        width,
        height,
        parameters,
        origin_x,
        origin_y,
        frame_width,
        scan,
        lut,
    } = request;
    let TailCache {
        density,
        grain,
        grain_field,
        dye,
        dye_preview,
        printed,
        image,
    } = cache;
    if density.is_none() {
        *density = Some(render::density_buffer(
            &exposure,
            parameters.film(),
            width,
            control,
        )?);
        *grain = None;
    }
    if grain.is_none() {
        let base = density.clone().expect("density");
        let region = crate::stock::GrainRegion {
            width,
            height,
            origin_x,
            origin_y,
            frame_width,
        };
        let rows = region.rows(base.len());
        *grain = Some(match parameters.grain() {
            Some(model) if rows > 0 => {
                let cached = grain_field
                    .as_ref()
                    .is_some_and(|field| field.matches(model, region, rows));
                if !cached {
                    let samples = model.unit_field(region, rows, control)?;
                    *grain_field = Some(CloudField::store(model, region, rows, samples));
                } else {
                    render::begin(Stage::Grain, rows as u32, control)?;
                    control.on_progress(Stage::Grain, rows as u32, rows as u32);
                    if control.cancelled() {
                        return Err(Cancelled);
                    }
                }
                let mut grained = base;
                model.mix_field(
                    &mut grained,
                    &grain_field.as_ref().expect("cloud field").samples,
                    frame_width,
                );
                grained
            }
            _ => {
                *grain_field = None;
                base
            }
        });
        *dye = None;
    }
    let want_preview = scan == ScanMode::Preview && lut.is_some();
    if dye.is_none() || *dye_preview != want_preview {
        let grained = grain.as_ref().expect("grain");
        if let Some(lut) = lut.filter(|_| want_preview) {
            *dye = Some(lut.apply(grained, width, control)?);
            *dye_preview = true;
        } else {
            let (scanned, _) = render::dye_buffer(grained, parameters.film(), width, control)?;
            *dye = Some(scanned);
            *dye_preview = false;
        }
        *printed = None;
    }
    if printed.is_none() {
        let scanned = dye.as_ref().expect("dye");
        *printed = Some(render::print_buffer(scanned, parameters, control)?);
        *image = None;
    }
    if image.is_none() {
        let linear = printed.clone().expect("print");
        let fidelity = if *dye_preview {
            Fidelity::Preview
        } else {
            Fidelity::Exact
        };
        *image = Some(render::encode_buffer(
            linear, width, height, fidelity, control,
        )?);
    }
    Ok(image.clone().expect("encoded"))
}

fn first_dirty(previous: &Parameters, next: &Parameters) -> Stage {
    if previous.exposure_ev() != next.exposure_ev() {
        return Stage::Exposure;
    }
    if !halation_eq(previous.halation(), next.halation()) {
        return Stage::Halation;
    }
    let film_stage = previous.film().edit_stage(next.film());
    if film_stage <= Stage::Density {
        return Stage::Density;
    }
    if !grain_eq(previous.grain(), next.grain()) {
        return Stage::Grain;
    }
    if film_stage == Stage::DyeScan {
        return Stage::DyeScan;
    }
    if film_stage == Stage::Print
        || previous.print_exposure() != next.print_exposure()
        || !curve_eq(previous.print_curve(), next.print_curve())
    {
        return Stage::Print;
    }
    Stage::Encode
}

fn halation_eq(left: Option<&Halation>, right: Option<&Halation>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.threshold() == right.threshold()
                && left.radius() == right.radius()
                && left.weight() == right.weight()
        }
        _ => false,
    }
}

fn grain_eq(left: Option<&Grain>, right: Option<&Grain>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.frame_mm() == right.frame_mm()
                && left.seed() == right.seed()
                && left.size_mm() == right.size_mm()
                && left
                    .layers()
                    .iter()
                    .zip(right.layers())
                    .all(|(a, b)| a.points().eq(b.points()))
        }
        _ => false,
    }
}

fn curve_eq(left: &crate::stock::Curve, right: &crate::stock::Curve) -> bool {
    left.points().eq(right.points())
}

fn default_halation() -> Halation {
    Halation::new(0.25, 0.008, [0.08, 0.008, 0.0]).expect("default halation is valid")
}

fn extract(image: &LinearImage, x: u32, y: u32, width: u32, height: u32) -> Vec<f32> {
    let stride = image.width() as usize * 3;
    let row = width as usize * 3;
    let mut out = Vec::with_capacity(row * height as usize);
    let pixels = image.pixels();
    for line in 0..height as usize {
        let start = (y as usize + line) * stride + x as usize * 3;
        out.extend_from_slice(&pixels[start..start + row]);
    }
    out
}

fn crop_buffer(
    buffer: &[f32],
    stride_px: u32,
    origin_x: u32,
    origin_y: u32,
    width: u32,
    height: u32,
) -> Vec<f32> {
    let stride = stride_px as usize * 3;
    let row = width as usize * 3;
    let mut out = Vec::with_capacity(row * height as usize);
    for line in 0..height as usize {
        let start = (origin_y as usize + line) * stride + origin_x as usize * 3;
        out.extend_from_slice(&buffer[start..start + row]);
    }
    out
}

#[derive(Clone, PartialEq, Eq)]
struct DyeKey {
    process: crate::stock::Process,
    reference: Option<u32>,
    samples: Vec<u32>,
}

impl DyeKey {
    fn from_film(film: &FilmModel) -> Option<Self> {
        let FilmModel::Spectral(model) = film else {
            return None;
        };
        let dyes = model.dye_density()?;
        let mut samples = Vec::new();
        for layer in dyes {
            samples.extend(layer.iter().map(|sample| sample.to_bits()));
        }
        Some(Self {
            process: model.process(),
            reference: model.reference_log_exposure().map(f32::to_bits),
            samples,
        })
    }
}

/// 48³ trilinear table of the exact dye scan over the stock's density range.
#[derive(Clone)]
pub(crate) struct DyeLut {
    key: DyeKey,
    edge: usize,
    min: [f32; 3],
    max: [f32; 3],
    values: Vec<f32>,
}

impl DyeLut {
    fn build(
        film: &FilmModel,
        key: DyeKey,
        control: &mut dyn DevelopControl,
    ) -> Result<Self, Cancelled> {
        let FilmModel::Spectral(model) = film else {
            unreachable!("dye key exists only for a spectral stock with dyes");
        };
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for (layer, curve) in model.curves().iter().enumerate() {
            for (_, density) in curve.points() {
                min[layer] = min[layer].min(density);
                max[layer] = max[layer].max(density);
            }
            if max[layer] <= min[layer] {
                max[layer] = min[layer] + 1.0;
            }
        }
        let edge = DYE_LUT_EDGE;
        let last = (edge - 1) as f32;
        let mut values = Vec::with_capacity(edge * edge * edge * 3);
        let mut transmittance = vec![0.0f32; model.wavelengths().len()];
        if control.cancelled() {
            return Err(Cancelled);
        }
        for z in 0..edge {
            if control.cancelled() {
                return Err(Cancelled);
            }
            let blue = lerp(min[2], max[2], z as f32 / last);
            for y in 0..edge {
                let green = lerp(min[1], max[1], y as f32 / last);
                for x in 0..edge {
                    let red = lerp(min[0], max[0], x as f32 / last);
                    let scanned = model
                        .scan_pixel([red, green, blue], &mut transmittance)
                        .expect("dye curves scan");
                    values.extend(scanned);
                }
            }
        }
        Ok(Self {
            key,
            edge,
            min,
            max,
            values,
        })
    }

    pub(crate) fn lookup(&self, density: [f32; 3]) -> [f32; 3] {
        let last = self.edge - 1;
        let mut index0 = [0usize; 3];
        let mut index1 = [0usize; 3];
        let mut weight = [0.0f32; 3];
        for channel in 0..3 {
            let span = self.max[channel] - self.min[channel];
            let unit = if span > 0.0 {
                ((density[channel] - self.min[channel]) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let scaled = unit * last as f32;
            index0[channel] = (scaled.floor() as usize).min(last);
            index1[channel] = (index0[channel] + 1).min(last);
            weight[channel] = if index0[channel] == index1[channel] {
                0.0
            } else {
                scaled - index0[channel] as f32
            };
        }
        let mut acc = [0.0f32; 3];
        for corner in 0..8 {
            let ix = if corner & 1 == 0 {
                index0[0]
            } else {
                index1[0]
            };
            let iy = if corner & 2 == 0 {
                index0[1]
            } else {
                index1[1]
            };
            let iz = if corner & 4 == 0 {
                index0[2]
            } else {
                index1[2]
            };
            let wx = if corner & 1 == 0 {
                1.0 - weight[0]
            } else {
                weight[0]
            };
            let wy = if corner & 2 == 0 {
                1.0 - weight[1]
            } else {
                weight[1]
            };
            let wz = if corner & 4 == 0 {
                1.0 - weight[2]
            } else {
                weight[2]
            };
            let sample = self.sample(ix, iy, iz);
            let corner_weight = wx * wy * wz;
            for channel in 0..3 {
                acc[channel] += sample[channel] * corner_weight;
            }
        }
        acc
    }

    fn sample(&self, x: usize, y: usize, z: usize) -> [f32; 3] {
        let index = ((z * self.edge + y) * self.edge + x) * 3;
        [
            self.values[index],
            self.values[index + 1],
            self.values[index + 2],
        ]
    }

    fn apply(
        &self,
        density: &[f32],
        width: u32,
        control: &mut dyn DevelopControl,
    ) -> Result<Vec<f32>, Cancelled> {
        let stride = width as usize * 3;
        let rows = density.len().checked_div(stride).unwrap_or(0) as u32;
        render::begin(Stage::DyeScan, rows, control)?;
        let mut out = Vec::with_capacity(density.len());
        for (y, row) in density.chunks(stride).take(rows as usize).enumerate() {
            if control.cancelled() {
                return Err(Cancelled);
            }
            for pixel in row.as_chunks::<3>().0 {
                out.extend(self.lookup(*pixel));
            }
            control.on_progress(Stage::DyeScan, y as u32 + 1, rows);
        }
        Ok(out)
    }
}

fn lerp(min: f32, max: f32, t: f32) -> f32 {
    min + (max - min) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Fidelity, NopControl};
    use crate::stock::Stock;

    #[test]
    fn dye_lookup_stays_within_one_code_value() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stocks/portra400.stock.toml");
        let stock = Stock::load(path).unwrap();
        let film = stock.parameters().film().clone();
        let key = DyeKey::from_film(&film).unwrap();
        let lut = DyeLut::build(&film, key, &mut NopControl).unwrap();
        let (min, max) = (lut.min, lut.max);
        let mut worst = 0i16;
        for step in 0..=48 {
            let t = step as f32 / 48.0;
            let density = [
                lerp(min[0], max[0], t),
                lerp(min[1], max[1], t),
                lerp(min[2], max[2], t),
            ];
            worst = worst.max(channel_error(&stock, &lut, density));
        }
        for z in 0..5 {
            for y in 0..5 {
                for x in 0..5 {
                    let density = [
                        lerp(min[0], max[0], x as f32 / 4.0),
                        lerp(min[1], max[1], y as f32 / 4.0),
                        lerp(min[2], max[2], z as f32 / 4.0),
                    ];
                    worst = worst.max(channel_error(&stock, &lut, density));
                }
            }
        }
        assert!(
            worst <= 1,
            "dye lookup missed the 8-bit tolerance by {worst} levels"
        );
    }

    fn channel_error(stock: &Stock, lut: &DyeLut, density: [f32; 3]) -> i16 {
        let exact = stock
            .parameters()
            .film()
            .printing_density(&density)
            .unwrap();
        let approx = lut.lookup(density);
        let exact = quantize(stock, &exact);
        let approx = quantize(stock, &approx);
        exact
            .into_iter()
            .zip(approx)
            .map(|(a, b)| (i16::from(a) - i16::from(b)).abs())
            .max()
            .unwrap()
    }

    fn quantize(stock: &Stock, density: &[f32]) -> [u8; 3] {
        let linear =
            crate::render::print_buffer(density, stock.parameters(), &mut NopControl).unwrap();
        let image =
            crate::render::encode_buffer(linear, 1, 1, Fidelity::Exact, &mut NopControl).unwrap();
        let rgb = image.to_rgb8();
        [rgb[0], rgb[1], rgb[2]]
    }
}
