//! Command and event contract for the rawstock 1.0 app.
//!
//! Engineering approved this shape on issue #36. Field names are camelCase
//! on the wire. A sidecar is this edit as TOML plus `rawstockVersion` and
//! `savedAt`, next to the RAW (`photo.RAF.rawstock.toml`). The RAW is never
//! written. Older sidecars load through `#[serde(default)]`: grain
//! multipliers default to 1, and halation overrides default to unset.
//!
//! White balance keeps the `custom` variant. The develop service will apply
//! it later; this build records it and develops as-shot only.
//!
//! Pixel bytes are not JSON. See [`crate::frame`].

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Schema written by this build. Older files that omit it are treated as 1.
pub fn current_schema() -> u32 {
    1
}

/// One opened RAW file, before any develop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub image_id: String,
    pub path: String,
    pub file_name: String,
    pub camera: CameraInfo,
    pub support: SupportLevel,
    pub exif: ExifInfo,
    pub width: u32,
    pub height: u32,
    pub sidecar: SidecarInfo,
}

/// Make and model shown in an unsupported-camera message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraInfo {
    pub make: String,
    pub model: String,
}

/// How far rawstock can take a camera. `decodes` is an uncalibrated file
/// the decoder accepts. Calibration is decided by the develop service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportLevel {
    Calibrated,
    Decodes,
    Unsupported,
}

/// EXIF the shell may show. Empty until the develop service reads the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ExifInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
}

/// Sidecar next to an opened file. Reading and writing the file is #37;
/// this is the status the open command will report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarInfo {
    pub status: SidecarStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<EditSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_revision: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_revision: Option<u32>,
}

impl SidecarInfo {
    /// No sidecar was consulted. The open stub uses this until #37.
    pub fn none() -> Self {
        Self {
            status: SidecarStatus::None,
            settings: None,
            saved_revision: None,
            installed_revision: None,
        }
    }
}

/// Whether the saved stock revision still matches the installed stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SidecarStatus {
    None,
    Loaded,
    Invalid,
    StockVersionMismatch,
}

/// Stock identity saved with an edit. `revision` travels with the sidecar
/// so a later stock update can warn that the render may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StockRef {
    pub id: String,
    pub revision: u32,
}

/// As-shot white balance, or a custom temperature the backend does not
/// apply yet. `custom` stays in the type so #39 can send it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WhiteBalance {
    #[default]
    AsShot,
    Custom {
        temperature_k: f64,
        tint: f64,
    },
}

/// Spectral dye-scale overrides. Absent on parametric stocks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DyeEdit {
    pub scales: [f64; 3],
}

/// Multipliers on the stock's calibrated grain. 1 is the stock as it ships.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrainEdit {
    pub amount: f64,
    pub size: f64,
}

impl Default for GrainEdit {
    fn default() -> Self {
        Self {
            amount: 1.0,
            size: 1.0,
        }
    }
}

/// Halation switch, plus optional overrides of the stock's own defaults.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct HalationEdit {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<[f64; 3]>,
}

/// Scan or print look preset id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanEdit {
    pub look: String,
}

impl Default for ScanEdit {
    fn default() -> Self {
        Self {
            look: "default".to_string(),
        }
    }
}

/// Crop in normalised 0..1 coordinates of the oriented image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CropRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Lens corrections applied while the RAW file is developed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LensEdit {
    /// Chromatic aberration correction: red and blue registered onto green,
    /// then the blue/purple glow beside bright edges desaturated. On unless
    /// the edit turns it off.
    pub ca_correction: bool,
}

impl Default for LensEdit {
    fn default() -> Self {
        Self {
            ca_correction: true,
        }
    }
}

/// Rotation, straighten, and an optional crop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct GeometryEdit {
    pub quarter_turns: u8,
    pub straighten_deg: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop: Option<CropRect>,
}

/// Everything an edit sidecar stores about one photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EditSettings {
    #[serde(default = "current_schema")]
    pub schema: u32,
    pub stock: StockRef,
    pub exposure_ev: f64,
    pub white_balance: WhiteBalance,
    pub print_exposure: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dye: Option<DyeEdit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f64>,
    pub grain: GrainEdit,
    pub halation: HalationEdit,
    pub scan: ScanEdit,
    pub geometry: GeometryEdit,
    pub lens: LensEdit,
}

impl Default for EditSettings {
    fn default() -> Self {
        Self {
            schema: current_schema(),
            stock: StockRef::default(),
            exposure_ev: 0.0,
            white_balance: WhiteBalance::AsShot,
            print_exposure: 0.0,
            dye: None,
            contrast: None,
            grain: GrainEdit::default(),
            halation: HalationEdit::default(),
            scan: ScanEdit::default(),
            geometry: GeometryEdit::default(),
            lens: LensEdit::default(),
        }
    }
}

impl EditSettings {
    /// Reject a document this build cannot treat as schema 1.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema != current_schema() {
            return Err(AppError::sidecar_invalid(
                "This edit was written by a newer rawstock. Update the app, or start again from the stock defaults. The photo was not changed.",
            ));
        }
        if self.geometry.quarter_turns > 3 {
            return Err(AppError::sidecar_invalid(
                "The rotation in the edit file is not valid. The photo was not changed.",
            ));
        }
        if !self.exposure_ev.is_finite() || !self.print_exposure.is_finite() {
            return Err(AppError::sidecar_invalid(
                "The exposure in the edit file is not valid. The photo was not changed.",
            ));
        }
        if !self.grain.amount.is_finite()
            || self.grain.amount < 0.0
            || !self.grain.size.is_finite()
            || self.grain.size <= 0.0
        {
            return Err(AppError::sidecar_invalid(
                "The grain settings in the edit file are not valid. The photo was not changed.",
            ));
        }
        if let WhiteBalance::Custom {
            temperature_k,
            tint,
        } = self.white_balance
            && (!temperature_k.is_finite()
                || !(1_000.0..=40_000.0).contains(&temperature_k)
                || !tint.is_finite())
        {
            return Err(AppError::sidecar_invalid(
                "The white balance in the edit file is not valid. The photo was not changed.",
            ));
        }
        if let Some(crop) = self.geometry.crop {
            let inside = [crop.x, crop.y, crop.w, crop.h]
                .iter()
                .all(|value| value.is_finite());
            let covers = (0.0..=1.0).contains(&crop.x)
                && (0.0..=1.0).contains(&crop.y)
                && crop.w > 0.0
                && crop.h > 0.0
                && crop.x + crop.w <= 1.0 + 1.0e-6
                && crop.y + crop.h <= 1.0 + 1.0e-6;
            if !inside || !covers {
                return Err(AppError::sidecar_invalid(
                    "The crop in the edit file is not valid. The photo was not changed.",
                ));
            }
        }
        Ok(())
    }
}

/// Sidecar document: the edit, the app version, and when it was saved.
///
/// The open and save flows that use this type belong to #37. The tests
/// parse it so the wire format stays fixed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[allow(dead_code)]
pub struct SidecarDocument {
    pub rawstock_version: String,
    pub saved_at: String,
    #[serde(flatten)]
    pub settings: EditSettings,
}

impl Default for SidecarDocument {
    fn default() -> Self {
        Self {
            rawstock_version: env!("CARGO_PKG_VERSION").to_string(),
            saved_at: String::new(),
            settings: EditSettings::default(),
        }
    }
}

/// Parse a sidecar. The TOML error stays out of [`AppError::message`].
#[allow(dead_code)]
pub fn parse_sidecar(text: &str) -> Result<SidecarDocument, AppError> {
    let document: SidecarDocument = toml::from_str(text).map_err(|_| {
        AppError::sidecar_invalid(
            "The edit file next to this photo could not be read. rawstock left the photo unchanged.",
        )
    })?;
    document.settings.validate()?;
    Ok(document)
}

/// Compare a saved stock revision with the one that is installed.
///
/// An unknown stock stays `loaded` here. `stockMissing` is an [`AppError`]
/// the open flow raises once it has looked the stock up (#37).
#[allow(dead_code)]
pub fn sidecar_status(settings: &EditSettings, installed_revision: Option<u32>) -> SidecarStatus {
    if settings.stock.id.is_empty() {
        return SidecarStatus::Invalid;
    }
    match installed_revision {
        Some(installed) if installed != settings.stock.revision => {
            SidecarStatus::StockVersionMismatch
        }
        _ => SidecarStatus::Loaded,
    }
}

/// `<file>.rawstock.toml` beside the RAW. The extension is kept.
pub fn sidecar_path(raw: &Path) -> PathBuf {
    let mut name = raw.as_os_str().to_owned();
    name.push(".rawstock.toml");
    PathBuf::from(name)
}

/// Hex SHA-256 of the canonical JSON of `settings`. Key order is the
/// struct field order, and omitted optional fields stay omitted.
pub fn settings_hash(settings: &EditSettings) -> String {
    let bytes = serde_json::to_vec(settings).expect("edit settings serialize");
    sha256_hex(&bytes)
}

/// Stable id for one path. `path` should already be canonical.
pub fn image_id_for(canonical: &str) -> String {
    sha256_hex(canonical.as_bytes())
}

/// Canonical path when the file exists, otherwise `path` unchanged.
pub fn canonical_path(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// What a long command returns at once. The result arrives as events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStarted {
    pub job_id: u64,
}

/// `preview`, `full`, `thumbnails`, or `export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    Preview,
    Full,
    Thumbnails,
    Export,
}

/// Why a job stopped before a frame existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancelReason {
    Superseded,
    Explicit,
    ImageClosed,
}

/// Why a cached full frame was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CacheReason {
    ImageSwitched,
    SettingsChanged,
    MemoryBudget,
}

/// JPEG export. TIFF, PNG, and the other M2 options belong to #41.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: String,
    pub quality: u32,
    pub path: String,
}

impl ExportOptions {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.format != "jpeg" {
            return Err(AppError::internal(
                "This build exports JPEG only. Choose JPEG and try again.",
            ));
        }
        if !(1..=100).contains(&self.quality) {
            return Err(AppError::internal("Choose a JPEG quality from 1 to 100."));
        }
        if self.path.trim().is_empty() {
            return Err(AppError::internal("Choose a destination for the export."));
        }
        Ok(())
    }
}

/// Where a sidecar would be written. This build does not write it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub path: String,
    pub saved_at: String,
}

/// Files accepted or refused by `open_paths`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOutcome {
    pub images: Vec<ImageInfo>,
    pub rejected: Vec<RejectedPath>,
}

/// One path `open_paths` did not open, with a message safe to show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedPath {
    pub path: String,
    pub error: AppError,
}

/// A browsable stock. `category` is not in the stock file yet; the shell
/// maps the known 1.0 names and treats anything else as a color negative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockSummary {
    pub id: String,
    pub name: String,
    pub category: StockCategory,
    pub model: String,
    pub revision: u32,
}

/// Stock browser groups from the 1.0 spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StockCategory {
    ColorNegative,
    Slide,
    Bw,
    FujiSim,
}

/// Category for a stock file stem and display name.
pub fn category_for(stem: &str, name: &str) -> StockCategory {
    let stem = stem.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let key = if stem.is_empty() {
        name.as_str()
    } else {
        stem.as_str()
    };
    if key.contains("portra") || name.contains("portra") {
        StockCategory::ColorNegative
    } else if key.contains("acros") || key.contains("mono") || name.contains("monochrome") {
        StockCategory::Bw
    } else if matches!(key, "velvia" | "provia" | "astia")
        || key.contains("classic")
        || name.contains("velvia")
        || name.contains("provia")
        || name.contains("astia")
        || name.contains("classic chrome")
    {
        StockCategory::FujiSim
    } else {
        StockCategory::ColorNegative
    }
}

/// User-facing failure. `message` is the whole sentence the UI shows.
/// Rust errors and OS errors stay in the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Stable codes the UI can branch on. The sentence is [`AppError::message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    UnsupportedCamera,
    CorruptFile,
    UnsupportedFormat,
    Io,
    SidecarInvalid,
    StockMissing,
    CacheMiss,
    Internal,
}

impl AppError {
    pub fn unsupported_camera(path: &Path, make: &str, model: &str) -> Self {
        let file = file_label(path);
        Self {
            code: ErrorCode::UnsupportedCamera,
            message: format!(
                "{file} is from a {make} {model}, which rawstock cannot develop. Open a Fuji X-Trans RAF or an iPhone ProRAW DNG instead."
            ),
            path: Some(path.display().to_string()),
        }
    }

    pub fn corrupt_file(path: &Path) -> Self {
        let file = file_label(path);
        Self {
            code: ErrorCode::CorruptFile,
            message: format!(
                "{file} is damaged or incomplete, so it could not be opened. Export it from the camera again, or choose a different file."
            ),
            path: Some(path.display().to_string()),
        }
    }

    pub fn unsupported_format(path: &Path) -> Self {
        let file = file_label(path);
        Self {
            code: ErrorCode::UnsupportedFormat,
            message: format!(
                "{file} is not a RAF or DNG file. Open a Fuji X-Trans RAF or an iPhone ProRAW DNG."
            ),
            path: Some(path.display().to_string()),
        }
    }

    pub fn io(path: &Path) -> Self {
        let file = file_label(path);
        Self {
            code: ErrorCode::Io,
            message: format!(
                "{file} could not be read. Check that it is still there and that you have permission to open it."
            ),
            path: Some(path.display().to_string()),
        }
    }

    pub fn sidecar_invalid(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::SidecarInvalid,
            message: message.into(),
            path: None,
        }
    }

    pub fn stock_missing(name: &str) -> Self {
        Self {
            code: ErrorCode::StockMissing,
            message: format!(
                "The stock \"{name}\" is not installed. Choose another stock. The photo was not changed."
            ),
            path: None,
        }
    }

    pub fn cache_miss() -> Self {
        Self {
            code: ErrorCode::CacheMiss,
            message: "The full-size preview is no longer in memory. Develop the photo again."
                .to_string(),
            path: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Internal,
            message: message.into(),
            path: None,
        }
    }

    /// True when `message` is safe to put on screen.
    #[allow(dead_code)]
    pub fn is_user_facing(&self) -> bool {
        let message = self.message.as_str();
        !message.contains("DecodeError")
            && !message.contains("Os {")
            && !message.contains("rawler")
            && !message.contains("backtrace")
    }
}

pub fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("That file")
        .to_string()
}

/// Stage progress. `stage` is a core `Stage` name, or `decode` / `encode` / `write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub job_id: u64,
    pub image_id: String,
    pub kind: JobKind,
    pub stage: String,
    pub done: u32,
    pub total: u32,
}

/// Terminal success. `exact` is true for a full develop and an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoneEvent {
    pub job_id: u64,
    pub image_id: String,
    pub kind: JobKind,
    pub settings_hash: String,
    pub width: u32,
    pub height: u32,
    pub exact: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
}

/// Terminal cancellation. No partial frame was published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledEvent {
    pub job_id: u64,
    pub image_id: String,
    pub kind: JobKind,
    pub reason: CancelReason,
}

/// Terminal failure. The previous frame stays on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedEvent {
    pub job_id: u64,
    pub image_id: String,
    pub kind: JobKind,
    pub error: AppError,
}

/// One stock thumbnail is ready to `take_frame`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailReadyEvent {
    pub job_id: u64,
    pub image_id: String,
    pub stock_id: String,
    pub settings_hash: String,
}

/// The full-frame cache entry is ready. The UI takes it once and pans locally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheReadyEvent {
    pub image_id: String,
    pub settings_hash: String,
    pub width: u32,
    pub height: u32,
}

/// A cache entry was dropped. `settings_hash` is set when one entry went away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEvictedEvent {
    pub image_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_hash: Option<String>,
    pub reason: CacheReason,
}

/// OS file open, a second instance, or a drop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRequested {
    pub paths: Vec<String>,
}

/// A native menu item or a shortcut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuAction {
    pub action: String,
}

/// One thing the shell emits after a command returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractEvent {
    Progress(ProgressEvent),
    Done(DoneEvent),
    Cancelled(CancelledEvent),
    /// Built by [`crate::jobs::ContractStub::fail_job`]. The shell does not
    /// fail a stub job on its own.
    #[allow(dead_code)]
    Failed(FailedEvent),
    ThumbnailReady(ThumbnailReadyEvent),
    CacheReady(CacheReadyEvent),
    CacheEvicted(CacheEvictedEvent),
}

impl ContractEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Progress(_) => "job:progress",
            Self::Done(_) => "job:done",
            Self::Cancelled(_) => "job:cancelled",
            Self::Failed(_) => "job:failed",
            Self::ThumbnailReady(_) => "thumbnail:ready",
            Self::CacheReady(_) => "cache:ready",
            Self::CacheEvicted(_) => "cache:evicted",
        }
    }

    #[allow(dead_code)]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done(_) | Self::Cancelled(_) | Self::Failed(_))
    }
}

/// RFC3339 UTC time. Used as `savedAt` on a sidecar the shell has not written.
pub fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_unix(secs)
}

pub fn format_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = tod / 3_600;
    let minute = (tod % 3_600) / 60;
    let second = tod % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's `civil_from_days`, public domain.
fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_sidecar_fills_defaults_and_keeps_the_stock_revision() {
        let text = r#"
rawstockVersion = "0.0.1"
savedAt = "2026-01-01T00:00:00Z"

[stock]
id = "portra400"
revision = 2
"#;
        let document = parse_sidecar(text).unwrap();
        assert_eq!(document.settings.schema, 1);
        assert_eq!(document.settings.stock.revision, 2);
        assert_eq!(document.settings.grain.amount, 1.0);
        assert_eq!(document.settings.grain.size, 1.0);
        assert!(!document.settings.halation.enabled);
        assert!(document.settings.halation.threshold.is_none());
        assert_eq!(document.settings.white_balance, WhiteBalance::AsShot);
        assert_eq!(document.settings.scan.look, "default");
        assert_eq!(
            sidecar_status(&document.settings, Some(3)),
            SidecarStatus::StockVersionMismatch
        );
        assert_eq!(
            sidecar_status(&document.settings, Some(2)),
            SidecarStatus::Loaded
        );
    }

    #[test]
    fn custom_white_balance_roundtrips_in_json_and_toml() {
        let settings = EditSettings {
            white_balance: WhiteBalance::Custom {
                temperature_k: 5200.0,
                tint: 2.5,
            },
            ..EditSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"mode\":\"custom\""));
        assert!(json.contains("\"temperatureK\":5200.0"));
        let back: EditSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.white_balance, settings.white_balance);

        let document = SidecarDocument {
            settings,
            ..SidecarDocument::default()
        };
        let toml = toml::to_string(&document).unwrap();
        let parsed = parse_sidecar(&toml).unwrap();
        assert_eq!(
            parsed.settings.white_balance,
            document.settings.white_balance
        );
    }

    #[test]
    fn hash_tracks_the_stock_revision_and_ignores_missing_defaults() {
        let mut settings = EditSettings {
            stock: StockRef {
                id: "portra400".to_string(),
                revision: 2,
            },
            ..EditSettings::default()
        };
        let hash = settings_hash(&settings);
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, settings_hash(&settings));
        settings.stock.revision = 3;
        assert_ne!(hash, settings_hash(&settings));

        let explicit = EditSettings::default();
        let from_empty: EditSettings = toml::from_str("").unwrap();
        assert_eq!(settings_hash(&explicit), settings_hash(&from_empty));
    }

    #[test]
    fn ca_correction_defaults_on_and_an_older_sidecar_keeps_it_on() {
        assert!(EditSettings::default().lens.ca_correction);
        let older: EditSettings = toml::from_str("exposureEv = 0.5").unwrap();
        assert!(older.lens.ca_correction);
        let off: EditSettings = toml::from_str("[lens]\ncaCorrection = false").unwrap();
        assert!(!off.lens.ca_correction);
        assert_ne!(settings_hash(&off), settings_hash(&EditSettings::default()));
    }

    #[test]
    fn sidecar_path_keeps_the_raw_extension() {
        let path = sidecar_path(Path::new("/photos/DSCF0001.RAF"));
        assert_eq!(path, PathBuf::from("/photos/DSCF0001.RAF.rawstock.toml"));
    }

    #[test]
    fn user_messages_name_the_camera_and_hide_rust_errors() {
        let path = Path::new("/photos/IMG.RAF");
        let error = AppError::unsupported_camera(path, "Canon", "EOS 5D Mark IV");
        assert_eq!(error.code, ErrorCode::UnsupportedCamera);
        assert!(error.message.contains("IMG.RAF"));
        assert!(error.message.contains("Canon"));
        assert!(error.message.contains("EOS 5D Mark IV"));
        assert!(error.is_user_facing());
        assert!(AppError::corrupt_file(path).message.contains("IMG.RAF"));
        assert!(AppError::corrupt_file(path).is_user_facing());
    }

    #[test]
    fn epoch_is_rfc3339() {
        assert_eq!(format_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix(86_400), "1970-01-02T00:00:00Z");
    }

    #[test]
    fn categories_follow_the_shipped_names() {
        assert_eq!(
            category_for("portra400", "Kodak Portra 400"),
            StockCategory::ColorNegative
        );
        assert_eq!(category_for("velvia", "Velvia"), StockCategory::FujiSim);
        assert_eq!(
            category_for("neutral", "Neutral"),
            StockCategory::ColorNegative
        );
        assert_eq!(category_for("acros", "Acros"), StockCategory::Bw);
    }
}
