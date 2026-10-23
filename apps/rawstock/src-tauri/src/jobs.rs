//! In-memory stand-in for the develop service.
//!
//! Job ids, cancellation, progress, and the cache events are real. The
//! bitmaps are a neutral placeholder, not a decode or a film render. The
//! cache stores that placeholder only. When #31's session cache lands,
//! `develop_full` should fill that cache instead of this map. `get_region`
//! is the fallback for a frame the webview should not hold; the UI takes
//! the full frame once and pans it locally.

use std::collections::{HashMap, VecDeque};

use rawstock_core::render::Stage;

use crate::contract::{
    AppError, CacheEvictedEvent, CacheReadyEvent, CacheReason, CancelReason, CancelledEvent,
    ContractEvent, DoneEvent, EditSettings, ExportOptions, FailedEvent, ImageInfo, JobKind,
    JobStarted, ProgressEvent, SaveResult, ThumbnailReadyEvent, settings_hash,
};
use crate::frame::{self, placeholder, stub_size};

#[derive(Debug)]
struct CachedFrame {
    image_id: String,
    settings_hash: String,
    packed: Vec<u8>,
}

#[derive(Debug)]
struct Job {
    image_id: String,
    kind: JobKind,
    settings_hash: String,
    stages: Vec<String>,
    stock_ids: Vec<String>,
    output_path: Option<String>,
    /// Stages already entered. `None` means the export is still queued.
    completed: Option<usize>,
    terminal: bool,
    cancel: Option<CancelReason>,
    long_edge: u32,
    compare: bool,
}

struct StartArgs<'a> {
    image_id: &'a str,
    kind: JobKind,
    settings: &'a EditSettings,
    long_edge: u32,
    compare: bool,
    stock_ids: Vec<String>,
    output_path: Option<String>,
}

/// The command-side job board. Tests drive [`Self::tick`] directly.
#[derive(Debug, Default)]
pub struct ContractStub {
    images: HashMap<String, ImageInfo>,
    active_image: Option<String>,
    next_id: u64,
    jobs: HashMap<u64, Job>,
    active: HashMap<(String, JobKind), u64>,
    export_order: VecDeque<u64>,
    running_export: Option<u64>,
    pending_frames: HashMap<(u64, Option<String>), Vec<u8>>,
    cache: Option<CachedFrame>,
    /// Thumbnail jobs are rendered by [`crate::thumbnails::Thumbnailer`]
    /// and reported through [`Self::deliver_thumbnail`], not by `tick`.
    external_thumbnails: bool,
    /// Preview and full develops run in [`crate::develop::Developer`] and
    /// finish through [`Self::finish_develop`], not by `tick`.
    external_develops: bool,
}

impl ContractStub {
    pub fn insert_image(&mut self, info: ImageInfo) {
        self.images.insert(info.image_id.clone(), info);
    }

    pub fn has_running(&self) -> bool {
        self.jobs
            .values()
            .any(|job| self.ticks(job) && !job.terminal && job.completed.is_some())
    }

    /// Hand thumbnail jobs to the real renderer from now on.
    pub fn render_thumbnails_elsewhere(&mut self) {
        self.external_thumbnails = true;
    }

    /// Hand preview and full develops to the real develop thread.
    pub fn develop_elsewhere(&mut self) {
        self.external_develops = true;
    }

    fn ticks(&self, job: &Job) -> bool {
        match job.kind {
            JobKind::Thumbnails => !self.external_thumbnails,
            JobKind::Preview | JobKind::Full => !self.external_develops,
            JobKind::Export => true,
        }
    }

    /// Whether a running job should keep going. Same rule as
    /// [`Self::thumbnail_check`], for any kind.
    pub fn job_check(&mut self, job_id: u64) -> (bool, Vec<ContractEvent>) {
        self.thumbnail_check(job_id)
    }

    /// `true` while `job_id` runs with no cancel request. Read-only, for the
    /// develop's per-row cancel check.
    pub fn job_live(&self, job_id: u64) -> bool {
        self.jobs
            .get(&job_id)
            .is_some_and(|job| !job.terminal && job.cancel.is_none())
    }

    /// Finish a preview or full develop with its real frame. A full frame
    /// also becomes the pan cache.
    pub fn finish_develop(
        &mut self,
        job_id: u64,
        packed: Vec<u8>,
        width: u32,
        height: u32,
    ) -> Vec<ContractEvent> {
        if self
            .jobs
            .get(&job_id)
            .is_none_or(|job| !matches!(job.kind, JobKind::Preview | JobKind::Full))
        {
            return Vec::new();
        }
        self.finish_with(job_id, Some((width, height)), Some(packed))
    }

    /// Path of an opened image, for the renderer.
    pub fn image_path(&self, image_id: &str) -> Option<String> {
        self.images.get(image_id).map(|info| info.path.clone())
    }

    /// Whether a thumbnail job should keep rendering. A cancel request
    /// ends the job here, on the boundary between two stocks.
    pub fn thumbnail_check(&mut self, job_id: u64) -> (bool, Vec<ContractEvent>) {
        let Some(job) = self.jobs.get(&job_id) else {
            return (false, Vec::new());
        };
        if job.terminal {
            return (false, Vec::new());
        }
        if let Some(reason) = job.cancel {
            return (false, self.finish_cancel(job_id, reason));
        }
        (true, Vec::new())
    }

    /// Store one rendered stock for `take_frame` and announce it. Nothing
    /// is kept for a job that has already ended.
    pub fn deliver_thumbnail(
        &mut self,
        job_id: u64,
        stock_id: &str,
        packed: Vec<u8>,
    ) -> Vec<ContractEvent> {
        let Some(job) = self.jobs.get_mut(&job_id) else {
            return Vec::new();
        };
        if job.terminal || job.kind != JobKind::Thumbnails {
            return Vec::new();
        }
        let Some(index) = job.stock_ids.iter().position(|id| id == stock_id) else {
            return Vec::new();
        };
        job.completed = Some((index + 1).min(job.stages.len().saturating_sub(1)));
        let ready = ContractEvent::ThumbnailReady(ThumbnailReadyEvent {
            job_id,
            image_id: job.image_id.clone(),
            stock_id: stock_id.to_string(),
            settings_hash: job.settings_hash.clone(),
        });
        self.pending_frames
            .insert((job_id, Some(stock_id.to_string())), packed);
        vec![ready, self.progress_event(job_id)]
    }

    /// End a thumbnail job once every stock has been delivered.
    pub fn finish_thumbnails(
        &mut self,
        job_id: u64,
        width: u32,
        height: u32,
    ) -> Vec<ContractEvent> {
        if self
            .jobs
            .get(&job_id)
            .is_none_or(|job| job.kind != JobKind::Thumbnails)
        {
            return Vec::new();
        }
        self.finish_sized(job_id, Some((width, height)))
    }

    pub fn select_image(
        &mut self,
        image_id: &str,
    ) -> Result<(ImageInfo, Vec<ContractEvent>), AppError> {
        let info = self
            .images
            .get(image_id)
            .cloned()
            .ok_or_else(|| AppError::internal("Open a photo before switching to it."))?;
        if self.active_image.as_deref() == Some(image_id) {
            return Ok((info, Vec::new()));
        }
        let mut events = Vec::new();
        if let Some(previous) = self.active_image.clone() {
            let ids: Vec<u64> = self
                .jobs
                .iter()
                .filter(|(_, job)| {
                    job.image_id == previous && job.kind != JobKind::Export && !job.terminal
                })
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                events.extend(self.finish_cancel(id, CancelReason::ImageClosed));
            }
            events.extend(self.evict_image(&previous, CacheReason::ImageSwitched));
        }
        self.active_image = Some(image_id.to_string());
        Ok((info, events))
    }

    pub fn develop_preview(
        &mut self,
        image_id: &str,
        settings: &EditSettings,
        long_edge: u32,
        compare: bool,
    ) -> Result<(JobStarted, Vec<ContractEvent>), AppError> {
        if long_edge == 0 {
            return Err(AppError::internal(
                "Choose a preview size larger than zero.",
            ));
        }
        self.start(StartArgs {
            image_id,
            kind: JobKind::Preview,
            settings,
            long_edge,
            compare,
            stock_ids: Vec::new(),
            output_path: None,
        })
    }

    pub fn develop_full(
        &mut self,
        image_id: &str,
        settings: &EditSettings,
    ) -> Result<(JobStarted, Vec<ContractEvent>), AppError> {
        self.start(StartArgs {
            image_id,
            kind: JobKind::Full,
            settings,
            long_edge: 48,
            compare: false,
            stock_ids: Vec::new(),
            output_path: None,
        })
    }

    pub fn render_thumbnails(
        &mut self,
        image_id: &str,
        settings: &EditSettings,
        stock_ids: Vec<String>,
        long_edge: u32,
    ) -> Result<(JobStarted, Vec<ContractEvent>), AppError> {
        if stock_ids.is_empty() {
            return Err(AppError::internal("Choose at least one stock."));
        }
        if long_edge == 0 {
            return Err(AppError::internal(
                "Choose a thumbnail size larger than zero.",
            ));
        }
        self.start(StartArgs {
            image_id,
            kind: JobKind::Thumbnails,
            settings,
            long_edge,
            compare: false,
            stock_ids,
            output_path: None,
        })
    }

    pub fn export_image(
        &mut self,
        image_id: &str,
        settings: &EditSettings,
        options: &ExportOptions,
    ) -> Result<(JobStarted, Vec<ContractEvent>), AppError> {
        options.validate()?;
        self.start(StartArgs {
            image_id,
            kind: JobKind::Export,
            settings,
            long_edge: 48,
            compare: false,
            stock_ids: Vec::new(),
            output_path: Some(options.path.clone()),
        })
    }

    /// `false` when `job_id` is unknown or already finished.
    ///
    /// A running job records the request and ends on the next stage
    /// boundary, which is the core `DevelopControl::cancelled` check.
    /// A queued export ends immediately and does not leave a file.
    pub fn cancel(&mut self, job_id: u64) -> (bool, Vec<ContractEvent>) {
        let queued = match self.jobs.get(&job_id) {
            None => return (false, Vec::new()),
            Some(job) if job.terminal => return (false, Vec::new()),
            Some(job) => job.completed.is_none(),
        };
        if queued {
            return (true, self.finish_cancel(job_id, CancelReason::Explicit));
        }
        if let Some(job) = self.jobs.get_mut(&job_id) {
            job.cancel = Some(CancelReason::Explicit);
        }
        (true, Vec::new())
    }

    /// Advance every running job by one stage.
    pub fn tick(&mut self) -> Vec<ContractEvent> {
        let ids: Vec<u64> = self
            .jobs
            .iter()
            .filter(|(_, job)| self.ticks(job) && !job.terminal && job.completed.is_some())
            .map(|(id, _)| *id)
            .collect();
        let mut events = Vec::new();
        for id in ids {
            events.extend(self.advance(id));
        }
        events
    }

    pub fn take_frame(&mut self, job_id: u64, stock_id: Option<&str>) -> Result<Vec<u8>, AppError> {
        let key = (job_id, stock_id.map(str::to_string));
        self.pending_frames.remove(&key).ok_or_else(|| {
            AppError::internal("That frame has already been delivered, or it is not ready.")
        })
    }

    pub fn get_region(
        &self,
        image_id: &str,
        settings_hash: &str,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, AppError> {
        let Some(cache) = &self.cache else {
            return Err(AppError::cache_miss());
        };
        if cache.image_id != image_id || cache.settings_hash != settings_hash {
            return Err(AppError::cache_miss());
        }
        frame::crop(&cache.packed, x, y, width, height)
    }

    /// The UI calls this when it drops the texture. Nothing in the shell
    /// does yet; the unit tests cover the event.
    #[allow(dead_code)]
    pub fn evict_for_memory(&mut self) -> Vec<ContractEvent> {
        let Some(cache) = self.cache.take() else {
            return Vec::new();
        };
        vec![ContractEvent::CacheEvicted(CacheEvictedEvent {
            image_id: cache.image_id,
            settings_hash: Some(cache.settings_hash),
            reason: CacheReason::MemoryBudget,
        })]
    }

    /// Test hook for a stage failure. The develop service will call the
    /// same path when a real stage returns an error.
    #[allow(dead_code)]
    pub fn fail_job(
        &mut self,
        job_id: u64,
        error: AppError,
    ) -> Result<Vec<ContractEvent>, AppError> {
        let known = self.jobs.get(&job_id).is_some_and(|job| !job.terminal);
        if !known {
            return Err(AppError::internal("That develop is no longer running."));
        }
        let job = self.jobs.get_mut(&job_id).expect("job");
        job.terminal = true;
        let event = ContractEvent::Failed(FailedEvent {
            job_id,
            image_id: job.image_id.clone(),
            kind: job.kind,
            error,
        });
        let mut events = vec![event];
        events.extend(self.detach(job_id));
        Ok(events)
    }

    /// Atomic write of `<file>.rawstock.toml`. The RAW is not opened for
    /// writing. An unreadable sidecar is refused so it cannot be replaced.
    pub fn save_sidecar(
        &mut self,
        image_id: &str,
        settings: &EditSettings,
    ) -> Result<SaveResult, crate::sidecar::Failure> {
        let info = self
            .images
            .get(image_id)
            .ok_or_else(|| crate::sidecar::Failure {
                error: AppError::internal("Open a photo before saving its edit."),
                detail: format!("save_sidecar unknown image {image_id}"),
            })?;
        if info.error.is_some() {
            return Err(crate::sidecar::Failure {
                error: AppError::internal(
                    "That photo could not be opened, so its edit was not saved. The photo was not changed.",
                ),
                detail: format!("save_sidecar skipped {}", info.path),
            });
        }
        let raw = std::path::PathBuf::from(&info.path);
        let saved = crate::sidecar::write(&raw, settings)?;
        let revisions = crate::sidecar::revision_map();
        if let Some(image) = self.images.get_mut(image_id) {
            image.sidecar = crate::sidecar::describe(settings, &revisions);
        }
        Ok(saved)
    }

    /// Move a bad sidecar to `.bak`. The image then opens at defaults.
    pub fn reset_sidecar(
        &mut self,
        image_id: &str,
    ) -> Result<crate::contract::ImageInfo, crate::sidecar::Failure> {
        let info = self
            .images
            .get(image_id)
            .cloned()
            .ok_or_else(|| crate::sidecar::Failure {
                error: AppError::internal("Open a photo before resetting its edit."),
                detail: format!("reset_sidecar unknown image {image_id}"),
            })?;
        let raw = std::path::PathBuf::from(&info.path);
        let _bak = crate::sidecar::reset(&raw)?;
        let image = self
            .images
            .get_mut(image_id)
            .expect("image stays in the session");
        image.sidecar = crate::contract::SidecarInfo::none();
        Ok(image.clone())
    }

    fn start(&mut self, args: StartArgs<'_>) -> Result<(JobStarted, Vec<ContractEvent>), AppError> {
        let StartArgs {
            image_id,
            kind,
            settings,
            long_edge,
            compare,
            stock_ids,
            output_path,
        } = args;
        let Some(info) = self.images.get(image_id) else {
            return Err(AppError::internal("Open a photo before developing it."));
        };
        if let Some(error) = info.error.clone() {
            return Err(error);
        }
        settings.validate()?;
        let mut events = Vec::new();
        if kind != JobKind::Export
            && let Some(previous) = self.active.get(&(image_id.to_string(), kind)).copied()
        {
            events.extend(self.finish_cancel(previous, CancelReason::Superseded));
        }
        self.next_id += 1;
        let id = self.next_id;
        let stages = stages_for(kind, stock_ids.len());
        let queued = kind == JobKind::Export && self.running_export.is_some();
        let job = Job {
            image_id: image_id.to_string(),
            kind,
            settings_hash: settings_hash(settings),
            stages,
            stock_ids,
            output_path,
            completed: if queued { None } else { Some(0) },
            terminal: false,
            cancel: None,
            long_edge,
            compare,
        };
        self.jobs.insert(id, job);
        if kind == JobKind::Export {
            if queued {
                self.export_order.push_back(id);
            } else {
                self.running_export = Some(id);
                events.push(self.progress_event(id));
            }
        } else {
            self.active.insert((image_id.to_string(), kind), id);
            events.push(self.progress_event(id));
        }
        Ok((JobStarted { job_id: id }, events))
    }

    fn advance(&mut self, id: u64) -> Vec<ContractEvent> {
        let (cancel, completed, len) = {
            let Some(job) = self.jobs.get(&id) else {
                return Vec::new();
            };
            if job.terminal || job.completed.is_none() {
                return Vec::new();
            }
            (
                job.cancel,
                job.completed.expect("running"),
                job.stages.len(),
            )
        };
        if let Some(reason) = cancel {
            return self.finish_cancel(id, reason);
        }
        let mut events = self.note_thumbnail(id, completed);
        let next = completed + 1;
        if next >= len {
            events.extend(self.finish_done(id));
            return events;
        }
        if let Some(job) = self.jobs.get_mut(&id) {
            job.completed = Some(next);
        }
        events.push(self.progress_event(id));
        events
    }

    fn note_thumbnail(&mut self, id: u64, completed_index: usize) -> Vec<ContractEvent> {
        if completed_index == 0 {
            return Vec::new();
        }
        let Some(job) = self.jobs.get(&id) else {
            return Vec::new();
        };
        if job.kind != JobKind::Thumbnails {
            return Vec::new();
        }
        let Some(stock_id) = job.stock_ids.get(completed_index - 1).cloned() else {
            return Vec::new();
        };
        let image_id = job.image_id.clone();
        let settings_hash = job.settings_hash.clone();
        let long_edge = job.long_edge;
        let salt = if job.compare { 40 } else { 0 };
        let (width, height) = stub_size(long_edge);
        self.pending_frames.insert(
            (id, Some(stock_id.clone())),
            placeholder(width, height, salt),
        );
        vec![ContractEvent::ThumbnailReady(ThumbnailReadyEvent {
            job_id: id,
            image_id,
            stock_id,
            settings_hash,
        })]
    }

    fn finish_done(&mut self, id: u64) -> Vec<ContractEvent> {
        self.finish_with(id, None, None)
    }

    fn finish_sized(&mut self, id: u64, size: Option<(u32, u32)>) -> Vec<ContractEvent> {
        self.finish_with(id, size, None)
    }

    /// `size` and `frame` are the real develop's. `None` uses the stub
    /// bitmap.
    fn finish_with(
        &mut self,
        id: u64,
        size: Option<(u32, u32)>,
        frame: Option<Vec<u8>>,
    ) -> Vec<ContractEvent> {
        let Some(job) = self.jobs.get_mut(&id) else {
            return Vec::new();
        };
        if job.terminal {
            return Vec::new();
        }
        job.terminal = true;
        let kind = job.kind;
        let image_id = job.image_id.clone();
        let settings_hash = job.settings_hash.clone();
        let output_path = job.output_path.clone();
        let long_edge = job.long_edge;
        let salt = u8::try_from(id % 64).unwrap_or(0) + u8::from(job.compare) * 40;
        let (width, height) = size.unwrap_or_else(|| stub_size(long_edge));
        let packed = frame.unwrap_or_else(|| placeholder(width, height, salt));
        if kind != JobKind::Thumbnails {
            self.pending_frames.insert((id, None), packed.clone());
        }
        let mut events = vec![ContractEvent::Done(DoneEvent {
            job_id: id,
            image_id: image_id.clone(),
            kind,
            settings_hash: settings_hash.clone(),
            width,
            height,
            exact: matches!(kind, JobKind::Full | JobKind::Export),
            output_path,
        })];
        if kind == JobKind::Full {
            events.extend(self.store_cache(image_id, settings_hash, packed, width, height));
        }
        events.extend(self.detach(id));
        events
    }

    fn finish_cancel(&mut self, id: u64, reason: CancelReason) -> Vec<ContractEvent> {
        let Some(job) = self.jobs.get_mut(&id) else {
            return Vec::new();
        };
        if job.terminal {
            return Vec::new();
        }
        job.terminal = true;
        job.cancel = Some(reason);
        let image_id = job.image_id.clone();
        let kind = job.kind;
        self.pending_frames.retain(|(job_id, _), _| *job_id != id);
        let event = ContractEvent::Cancelled(CancelledEvent {
            job_id: id,
            image_id,
            kind,
            reason,
        });
        let mut events = vec![event];
        events.extend(self.detach(id));
        events
    }

    fn store_cache(
        &mut self,
        image_id: String,
        settings_hash: String,
        packed: Vec<u8>,
        width: u32,
        height: u32,
    ) -> Vec<ContractEvent> {
        let mut events = Vec::new();
        if let Some(old) = &self.cache
            && (old.image_id != image_id || old.settings_hash != settings_hash)
        {
            let reason = if old.image_id == image_id {
                CacheReason::SettingsChanged
            } else {
                CacheReason::ImageSwitched
            };
            events.push(ContractEvent::CacheEvicted(CacheEvictedEvent {
                image_id: old.image_id.clone(),
                settings_hash: Some(old.settings_hash.clone()),
                reason,
            }));
        }
        self.cache = Some(CachedFrame {
            image_id: image_id.clone(),
            settings_hash: settings_hash.clone(),
            packed,
        });
        events.push(ContractEvent::CacheReady(CacheReadyEvent {
            image_id,
            settings_hash,
            width,
            height,
        }));
        events
    }

    fn evict_image(&mut self, image_id: &str, reason: CacheReason) -> Vec<ContractEvent> {
        let Some(cache) = &self.cache else {
            return Vec::new();
        };
        if cache.image_id != image_id {
            return Vec::new();
        }
        let event = ContractEvent::CacheEvicted(CacheEvictedEvent {
            image_id: cache.image_id.clone(),
            settings_hash: Some(cache.settings_hash.clone()),
            reason,
        });
        self.cache = None;
        vec![event]
    }

    fn detach(&mut self, id: u64) -> Vec<ContractEvent> {
        let Some(job) = self.jobs.get(&id) else {
            return Vec::new();
        };
        let key = (job.image_id.clone(), job.kind);
        if self.active.get(&key) == Some(&id) {
            self.active.remove(&key);
        }
        self.export_order.retain(|queued| *queued != id);
        if self.running_export == Some(id) {
            self.running_export = None;
            return self.promote_export();
        }
        Vec::new()
    }

    fn promote_export(&mut self) -> Vec<ContractEvent> {
        if self.running_export.is_some() {
            return Vec::new();
        }
        while let Some(id) = self.export_order.pop_front() {
            let Some(job) = self.jobs.get_mut(&id) else {
                continue;
            };
            if job.terminal {
                continue;
            }
            job.completed = Some(0);
            self.running_export = Some(id);
            return vec![self.progress_event(id)];
        }
        Vec::new()
    }

    fn progress_event(&self, id: u64) -> ContractEvent {
        let job = &self.jobs[&id];
        let index = job.completed.unwrap_or(0);
        let stage = job
            .stages
            .get(index)
            .cloned()
            .unwrap_or_else(|| "encode".to_string());
        ContractEvent::Progress(ProgressEvent {
            job_id: id,
            image_id: job.image_id.clone(),
            kind: job.kind,
            stage,
            done: u32::try_from(index).unwrap_or(u32::MAX),
            total: u32::try_from(job.stages.len()).unwrap_or(u32::MAX),
        })
    }
}

fn stages_for(kind: JobKind, stocks: usize) -> Vec<String> {
    match kind {
        JobKind::Preview | JobKind::Full => vec![
            "decode".to_string(),
            Stage::Exposure.name().to_string(),
            Stage::Halation.name().to_string(),
            Stage::Density.name().to_string(),
            Stage::Grain.name().to_string(),
            Stage::DyeScan.name().to_string(),
            Stage::Print.name().to_string(),
            "encode".to_string(),
        ],
        JobKind::Thumbnails => {
            let mut stages = vec!["decode".to_string()];
            for _ in 0..stocks {
                stages.push(Stage::Encode.name().to_string());
            }
            stages
        }
        JobKind::Export => vec![
            "decode".to_string(),
            Stage::Exposure.name().to_string(),
            Stage::Density.name().to_string(),
            Stage::Print.name().to_string(),
            "encode".to_string(),
            "write".to_string(),
        ],
    }
}

#[cfg(test)]
fn run_to_done(stub: &mut ContractStub, id: u64) -> Vec<ContractEvent> {
    let mut all = Vec::new();
    for _ in 0..32 {
        let events = stub.tick();
        let done = events.iter().any(|event| {
            matches!(event, ContractEvent::Done(done) if done.job_id == id)
                || matches!(event, ContractEvent::Cancelled(cancelled) if cancelled.job_id == id)
                || matches!(event, ContractEvent::Failed(failed) if failed.job_id == id)
        });
        all.extend(events);
        if done {
            break;
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{CacheReason, CancelReason, ErrorCode, StockRef};

    fn photo(id: &str) -> ImageInfo {
        ImageInfo {
            image_id: id.to_string(),
            path: format!("/photos/{id}.RAF"),
            file_name: format!("{id}.RAF"),
            camera: crate::contract::CameraInfo {
                make: "Fujifilm".to_string(),
                model: "X-T2".to_string(),
            },
            support: crate::contract::SupportLevel::Decodes,
            exif: crate::contract::ExifInfo::default(),
            width: 0,
            height: 0,
            sidecar: crate::contract::SidecarInfo::none(),
            error: None,
        }
    }

    fn settings(revision: u32) -> EditSettings {
        EditSettings {
            stock: StockRef {
                id: "portra400".to_string(),
                revision,
            },
            ..EditSettings::default()
        }
    }

    fn terminals(events: &[ContractEvent]) -> Vec<&ContractEvent> {
        events.iter().filter(|event| event.is_terminal()).collect()
    }

    #[test]
    fn supersede_cancels_the_older_preview_and_keeps_one_terminal_event() {
        let mut stub = ContractStub::default();
        stub.insert_image(photo("a"));
        let (first, _) = stub.develop_preview("a", &settings(2), 720, false).unwrap();
        let (_second, events) = stub.develop_preview("a", &settings(2), 720, false).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            ContractEvent::Cancelled(cancelled)
                if cancelled.job_id == first.job_id && cancelled.reason == CancelReason::Superseded
        )));
        let rest = run_to_done(&mut stub, first.job_id);
        assert!(!rest.iter().any(
            |event| matches!(event, ContractEvent::Done(done) if done.job_id == first.job_id)
        ));
        let done = rest.iter().find_map(|event| match event {
            ContractEvent::Done(done) => Some(done),
            _ => None,
        });
        let done = done.expect("replacement finishes");
        assert_eq!(terminals(&rest).len(), 1);
        assert!(!done.exact);
        assert!(rest.iter().any(|event| matches!(
            event,
            ContractEvent::Progress(progress) if progress.stage == "decode" || progress.stage == "Exposure"
        )));
    }

    #[test]
    fn explicit_cancel_waits_for_the_next_stage_and_publishes_no_frame() {
        let mut stub = ContractStub::default();
        stub.insert_image(photo("a"));
        let (job, _) = stub.develop_full("a", &settings(2)).unwrap();
        let (cancelled, immediate) = stub.cancel(job.job_id);
        assert!(cancelled);
        assert!(immediate.is_empty());
        let events = stub.tick();
        assert!(events.iter().any(|event| matches!(
            event,
            ContractEvent::Cancelled(cancelled)
                if cancelled.reason == CancelReason::Explicit && cancelled.job_id == job.job_id
        )));
        assert!(stub.take_frame(job.job_id, None).is_err());
        assert!(stub.cache.is_none());
        assert!(!stub.cancel(job.job_id).0);
        assert!(!stub.cancel(999).0);
    }

    #[test]
    fn image_switch_cancels_previews_and_leaves_the_export_queue() {
        let mut stub = ContractStub::default();
        stub.insert_image(photo("a"));
        stub.insert_image(photo("b"));
        stub.select_image("a").unwrap();
        let (preview, _) = stub.develop_preview("a", &settings(2), 32, false).unwrap();
        let export = ExportOptions {
            format: "jpeg".to_string(),
            quality: 90,
            path: "/tmp/out.jpg".to_string(),
        };
        let (export_job, _) = stub.export_image("a", &settings(2), &export).unwrap();
        let (_info, events) = stub.select_image("b").unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            ContractEvent::Cancelled(cancelled)
                if cancelled.job_id == preview.job_id && cancelled.reason == CancelReason::ImageClosed
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ContractEvent::Cancelled(cancelled) if cancelled.job_id == export_job.job_id))
        );
        let finished = run_to_done(&mut stub, export_job.job_id);
        assert!(finished.iter().any(|event| matches!(
            event,
            ContractEvent::Done(done) if done.job_id == export_job.job_id && done.output_path.as_deref() == Some("/tmp/out.jpg")
        )));
    }

    #[test]
    fn exports_queue_and_are_not_superseded() {
        let mut stub = ContractStub::default();
        stub.insert_image(photo("a"));
        let options = ExportOptions {
            format: "jpeg".to_string(),
            quality: 80,
            path: "/tmp/a.jpg".to_string(),
        };
        let (first, _) = stub.export_image("a", &settings(2), &options).unwrap();
        let second_options = ExportOptions {
            path: "/tmp/b.jpg".to_string(),
            ..options
        };
        let (second, events) = stub
            .export_image("a", &settings(2), &second_options)
            .unwrap();
        assert!(events.iter().all(|event| !event.is_terminal()));
        let (cancelled, immediate) = stub.cancel(second.job_id);
        assert!(cancelled);
        assert!(immediate.iter().any(|event| matches!(
            event,
            ContractEvent::Cancelled(cancelled)
                if cancelled.job_id == second.job_id && cancelled.reason == CancelReason::Explicit
        )));
        let finished = run_to_done(&mut stub, first.job_id);
        assert!(finished.iter().any(|event| {
            matches!(event, ContractEvent::Done(done) if done.job_id == first.job_id)
        }));
        assert!(!finished.iter().any(
            |event| matches!(event, ContractEvent::Done(done) if done.job_id == second.job_id)
        ));
    }

    #[test]
    fn full_frame_is_taken_once_and_get_region_is_the_fallback() {
        let mut stub = ContractStub::default();
        stub.insert_image(photo("a"));
        let (job, _) = stub.develop_full("a", &settings(2)).unwrap();
        let events = run_to_done(&mut stub, job.job_id);
        let ready = events.iter().find_map(|event| match event {
            ContractEvent::CacheReady(ready) => Some(ready.clone()),
            _ => None,
        });
        let ready = ready.expect("cache ready");
        assert!(events.iter().any(|event| {
            matches!(event, ContractEvent::Done(done) if done.exact && done.job_id == job.job_id)
        }));
        let frame = stub.take_frame(job.job_id, None).unwrap();
        assert!(stub.take_frame(job.job_id, None).is_err());
        let region = stub
            .get_region("a", &ready.settings_hash, 0, 0, 2, 2)
            .unwrap();
        assert_eq!(frame::dimensions(&region), Some((2, 2)));
        assert_eq!(frame::dimensions(&frame).unwrap().0, ready.width);

        let (next, _) = stub.develop_full("a", &settings(3)).unwrap();
        let replaced = run_to_done(&mut stub, next.job_id);
        assert!(replaced.iter().any(|event| matches!(
            event,
            ContractEvent::CacheEvicted(evicted)
                if evicted.reason == CacheReason::SettingsChanged && evicted.settings_hash.as_deref() == Some(ready.settings_hash.as_str())
        )));
        assert_eq!(
            stub.get_region("a", &ready.settings_hash, 0, 0, 2, 2)
                .unwrap_err()
                .code,
            ErrorCode::CacheMiss
        );
    }

    #[test]
    fn switching_images_evicts_the_cache_and_memory_pressure_does_too() {
        let mut stub = ContractStub::default();
        stub.insert_image(photo("a"));
        stub.insert_image(photo("b"));
        stub.select_image("a").unwrap();
        let (job, _) = stub.develop_full("a", &settings(2)).unwrap();
        run_to_done(&mut stub, job.job_id);
        let (_info, events) = stub.select_image("b").unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            ContractEvent::CacheEvicted(evicted) if evicted.reason == CacheReason::ImageSwitched
        )));
        assert_eq!(
            stub.get_region("a", "missing", 0, 0, 1, 1)
                .unwrap_err()
                .code,
            ErrorCode::CacheMiss
        );

        stub.select_image("a").unwrap();
        let (again, _) = stub.develop_full("a", &settings(2)).unwrap();
        run_to_done(&mut stub, again.job_id);
        let evicted = stub.evict_for_memory();
        assert!(evicted.iter().any(|event| matches!(
            event,
            ContractEvent::CacheEvicted(evicted) if evicted.reason == CacheReason::MemoryBudget
        )));
    }

    #[test]
    fn thumbnails_emit_one_ready_event_per_stock() {
        let mut stub = ContractStub::default();
        stub.insert_image(photo("a"));
        let (job, _) = stub
            .render_thumbnails(
                "a",
                &settings(2),
                vec!["portra400".to_string(), "velvia".to_string()],
                24,
            )
            .unwrap();
        let events = run_to_done(&mut stub, job.job_id);
        let thumbs: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ContractEvent::ThumbnailReady(ready) => Some(ready.stock_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(thumbs, ["portra400", "velvia"]);
        assert!(stub.take_frame(job.job_id, Some("portra400")).is_ok());
        assert!(stub.take_frame(job.job_id, Some("portra400")).is_err());
    }

    #[test]
    fn a_failed_job_is_the_only_terminal_event_and_leaves_no_frame() {
        let mut stub = ContractStub::default();
        stub.insert_image(photo("a"));
        let (job, _) = stub.develop_preview("a", &settings(2), 16, true).unwrap();
        let events = stub
            .fail_job(
                job.job_id,
                AppError::internal(
                    "The preview could not be finished. The last preview is still on screen.",
                ),
            )
            .unwrap();
        assert_eq!(terminals(&events).len(), 1);
        assert!(stub.take_frame(job.job_id, None).is_err());
    }

    #[test]
    fn save_sidecar_writes_beside_the_raw_and_a_bad_file_is_not_developed() {
        let dir = std::env::temp_dir().join(format!("rawstock-job-sidecar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("frame.RAF");
        let raw_bytes = b"RAWSTOCK-FIXTURE\nsupport: decodes\nmake: Fujifilm\nmodel: X-T2\n";
        std::fs::write(&raw, raw_bytes).unwrap();
        let before = std::fs::read(&raw).unwrap();

        let mut info = photo("frame");
        info.path = raw.display().to_string();
        let mut stub = ContractStub::default();
        stub.insert_image(info);
        let saved = stub.save_sidecar("frame", &settings(2)).unwrap();
        assert!(std::path::Path::new(&saved.path).is_file());
        assert!(saved.path.ends_with("frame.RAF.rawstock.toml"));
        assert_eq!(std::fs::read(&raw).unwrap(), before);
        let stored = stub.select_image("frame").unwrap().0;
        let expected = crate::sidecar::describe(&settings(2), &crate::sidecar::revision_map());
        assert_eq!(stored.sidecar.status, expected.status);
        assert_eq!(
            stored
                .sidecar
                .settings
                .as_ref()
                .map(|edit| edit.stock.revision),
            Some(2)
        );

        let mut broken = photo("bad");
        broken.error = Some(AppError::corrupt_file(std::path::Path::new("bad.RAF")));
        stub.insert_image(broken);
        let develop = stub
            .develop_preview("bad", &settings(2), 16, false)
            .unwrap_err();
        assert_eq!(develop.code, ErrorCode::CorruptFile);
        assert!(stub.select_image("bad").is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
