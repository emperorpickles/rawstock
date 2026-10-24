//! Filmstrip frames: one small picture per open photo.
//!
//! One thread works through the photos the UI sent last, in that order.
//! Every photo first gets the camera's embedded JPEG preview, which takes
//! milliseconds, so the whole strip fills at once. Then each photo is
//! developed with its own edit, as the stock thumbnails are, and that frame
//! replaces the preview. Frames are kept per photo by settings hash, so a
//! changed edit redraws only that photo.
//!
//! The develop pass uses the core's capped worker pool. This thread adds
//! one more, and it is idle whenever the queue is empty.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use rawstock_core::color::LinearImage;
use rawstock_core::decode::EmbeddedPreview;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::contract::EditSettings;
use crate::frame;
use crate::state::{self, AppState, lock};
use crate::thumbnails::{Source, ThumbnailRender};

/// Long edge of a strip frame in pixels: the 96 px box at 2x.
pub const STRIP_EDGE: u32 = 192;

/// Embedded previews are read larger than the frame so a crop still fills it.
const PREVIEW_EDGE: u32 = STRIP_EDGE * 2;

/// What the UI asks for: a photo and the edit to draw it with.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StripRequest {
    pub image_id: String,
    pub settings: EditSettings,
}

/// Payload of `strip:ready`. Fetch the pixels with `strip_frame`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StripReady {
    pub image_id: String,
    pub settings_hash: String,
    /// True for a develop with the photo's edit, false for the camera preview.
    pub exact: bool,
}

/// One queued photo, with the path and hash filled in.
#[derive(Debug, Clone)]
pub struct StripItem {
    pub image_id: String,
    pub path: PathBuf,
    pub settings: EditSettings,
    pub hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StripPass {
    Embedded,
    Develop,
}

#[derive(Debug, Clone)]
struct StripFrame {
    hash: String,
    exact: bool,
    packed: Vec<u8>,
}

/// The queue and the kept frames. No I/O, so tests drive it directly.
#[derive(Debug, Default)]
pub struct StripQueue {
    items: Vec<StripItem>,
    frames: HashMap<String, StripFrame>,
    /// `(image, hash, pass)` that failed, so a bad file is not retried
    /// until its edit changes.
    failed: HashSet<(String, String, StripPass)>,
}

impl StripQueue {
    /// Replace the queue. Frames of photos no longer listed are dropped.
    pub fn replace(&mut self, items: Vec<StripItem>) {
        let listed: HashSet<&str> = items.iter().map(|item| item.image_id.as_str()).collect();
        self.frames.retain(|id, _| listed.contains(id.as_str()));
        self.failed
            .retain(|(id, _, _)| listed.contains(id.as_str()));
        self.items = items;
    }

    /// Embedded previews for every photo without a frame come first, in
    /// queue order. Then develops, for photos whose frame is a preview or
    /// was drawn with an older edit.
    pub fn next(&self) -> Option<(StripItem, StripPass)> {
        let tried = |item: &StripItem, pass| {
            self.failed
                .contains(&(item.image_id.clone(), item.hash.clone(), pass))
        };
        let embedded = self.items.iter().find(|item| {
            !self.frames.contains_key(&item.image_id) && !tried(item, StripPass::Embedded)
        });
        if let Some(item) = embedded {
            return Some((item.clone(), StripPass::Embedded));
        }
        self.items
            .iter()
            .find(|item| {
                let current = self
                    .frames
                    .get(&item.image_id)
                    .is_some_and(|kept| kept.exact && kept.hash == item.hash);
                !current && !item.settings.stock.id.is_empty() && !tried(item, StripPass::Develop)
            })
            .map(|item| (item.clone(), StripPass::Develop))
    }

    /// Keep a finished frame. Returns false when it is no longer wanted:
    /// the photo left the queue, or a preview arrived after a develop.
    pub fn store(&mut self, item: &StripItem, pass: StripPass, packed: Vec<u8>) -> bool {
        if !self
            .items
            .iter()
            .any(|queued| queued.image_id == item.image_id)
        {
            return false;
        }
        let exact = pass == StripPass::Develop;
        if !exact && self.frames.contains_key(&item.image_id) {
            return false;
        }
        self.frames.insert(
            item.image_id.clone(),
            StripFrame {
                hash: item.hash.clone(),
                exact,
                packed,
            },
        );
        true
    }

    pub fn fail(&mut self, item: &StripItem, pass: StripPass) {
        self.failed
            .insert((item.image_id.clone(), item.hash.clone(), pass));
    }

    pub fn frame(&self, image_id: &str) -> Option<Vec<u8>> {
        self.frames.get(image_id).map(|kept| kept.packed.clone())
    }
}

/// Owns the strip thread.
pub struct Strip {
    queue: Arc<Mutex<StripQueue>>,
    wake: mpsc::Sender<()>,
}

impl Strip {
    pub fn spawn(app: AppHandle) -> Self {
        let queue = Arc::new(Mutex::new(StripQueue::default()));
        let (wake, woken) = mpsc::channel::<()>();
        let shared = Arc::clone(&queue);
        thread::Builder::new()
            .name("rawstock-strip".to_string())
            .spawn(move || {
                while woken.recv().is_ok() {
                    while let Some((item, pass)) = lock(&shared).next() {
                        let drawn = match pass {
                            StripPass::Embedded => StripRender::embedded(&item),
                            StripPass::Develop => StripRender::developed(&item),
                        };
                        let mut queue = lock(&shared);
                        match drawn {
                            Ok(packed) => {
                                if queue.store(&item, pass, packed) {
                                    drop(queue);
                                    Self::announce(&app, &item, pass);
                                }
                            }
                            Err(detail) => {
                                queue.fail(&item, pass);
                                drop(queue);
                                state::log_detail(
                                    &app.state::<AppState>().log_dir,
                                    &format!(
                                        "strip {pass:?} for {}: {detail}",
                                        item.path.display()
                                    ),
                                );
                            }
                        }
                    }
                }
            })
            .expect("spawn the filmstrip thread");
        Self { queue, wake }
    }

    pub fn submit(&self, items: Vec<StripItem>) {
        lock(&self.queue).replace(items);
        let _ = self.wake.send(());
    }

    pub fn frame(&self, image_id: &str) -> Option<Vec<u8>> {
        lock(&self.queue).frame(image_id)
    }

    fn announce(app: &AppHandle, item: &StripItem, pass: StripPass) {
        let ready = StripReady {
            image_id: item.image_id.clone(),
            settings_hash: item.hash.clone(),
            exact: pass == StripPass::Develop,
        };
        if let Err(error) = app.emit("strip:ready", ready) {
            state::log_detail(
                &app.state::<AppState>().log_dir,
                &format!("emit strip:ready failed: {error}"),
            );
        }
    }
}

/// Draws one strip frame, packed for `strip_frame`.
pub struct StripRender;

impl StripRender {
    /// The camera's preview, turned and cropped like the edit.
    pub fn embedded(item: &StripItem) -> Result<Vec<u8>, String> {
        let preview = EmbeddedPreview::read(&item.path, PREVIEW_EDGE)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the file has no embedded preview".to_string())?;
        Self::framed_preview(&preview, &item.settings)
    }

    /// The photo developed with its own stock and edit, grain and halation off.
    pub fn developed(item: &StripItem) -> Result<Vec<u8>, String> {
        let source = Source::decode(&item.path, item.settings.lens.ca_correction)?;
        let base = source.framed(&item.settings, STRIP_EDGE)?;
        ThumbnailRender::stock(
            &crate::stocks::directory(),
            &item.settings.stock.id,
            &item.settings,
            &base,
        )
        .map_err(|error| error.message)
    }

    /// Orient and frame an embedded preview, then shrink it to the strip.
    /// The samples stay sRGB: this is a stand-in, not a develop.
    pub fn framed_preview(
        preview: &EmbeddedPreview,
        settings: &EditSettings,
    ) -> Result<Vec<u8>, String> {
        let (width, height, rgb) =
            preview
                .orientation
                .apply_rgb(preview.width, preview.height, &preview.rgb);
        let samples = rgb.iter().map(|&value| f32::from(value) / 255.0).collect();
        let image = LinearImage::new(width, height, samples).map_err(|error| error.to_string())?;
        let framed = ThumbnailRender::frame(&image, settings)?
            .area_downsample(STRIP_EDGE)
            .map_err(|error| error.to_string())?;
        let mut rgba = Vec::with_capacity(framed.pixels().len() / 3 * 4);
        for pixel in framed.pixels().as_chunks::<3>().0 {
            for &value in pixel {
                rgba.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            rgba.push(255);
        }
        Ok(frame::pack(framed.width(), framed.height(), &rgba))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawstock_core::decode::Orientation;

    fn item(id: &str, stock: &str, exposure: f64) -> StripItem {
        let mut settings = EditSettings::default();
        settings.stock.id = stock.to_string();
        settings.exposure_ev = exposure;
        StripItem {
            image_id: id.to_string(),
            path: PathBuf::from(format!("/photos/{id}.RAF")),
            hash: crate::contract::settings_hash(&settings),
            settings,
        }
    }

    #[test]
    fn previews_fill_the_strip_before_any_develop() {
        let mut queue = StripQueue::default();
        queue.replace(vec![item("b", "portra400", 0.0), item("a", "velvia", 0.0)]);
        let (first, pass) = queue.next().unwrap();
        assert_eq!((first.image_id.as_str(), pass), ("b", StripPass::Embedded));
        assert!(queue.store(&first, pass, frame::placeholder(2, 2, 0)));
        let (second, pass) = queue.next().unwrap();
        assert_eq!((second.image_id.as_str(), pass), ("a", StripPass::Embedded));
        queue.fail(&second, pass);
        let (third, pass) = queue.next().unwrap();
        assert_eq!((third.image_id.as_str(), pass), ("b", StripPass::Develop));
        assert!(queue.store(&third, pass, frame::placeholder(4, 4, 0)));
        let (fourth, pass) = queue.next().unwrap();
        assert_eq!((fourth.image_id.as_str(), pass), ("a", StripPass::Develop));
        assert!(queue.store(&fourth, pass, frame::placeholder(4, 4, 0)));
        assert!(queue.next().is_none());
    }

    #[test]
    fn a_changed_edit_redraws_only_that_photo() {
        let mut queue = StripQueue::default();
        let a = item("a", "portra400", 0.0);
        let b = item("b", "portra400", 0.0);
        queue.replace(vec![a.clone(), b.clone()]);
        queue.store(&a, StripPass::Develop, frame::placeholder(4, 4, 1));
        queue.store(&b, StripPass::Develop, frame::placeholder(4, 4, 2));
        assert!(queue.next().is_none());

        let brighter = item("b", "portra400", 1.0);
        queue.replace(vec![a, brighter]);
        let (next, pass) = queue.next().unwrap();
        assert_eq!((next.image_id.as_str(), pass), ("b", StripPass::Develop));
        assert_eq!(
            queue.frame("b"),
            Some(frame::placeholder(4, 4, 2)),
            "old frame stays up"
        );
    }

    #[test]
    fn late_previews_and_closed_photos_are_dropped() {
        let mut queue = StripQueue::default();
        let a = item("a", "portra400", 0.0);
        queue.replace(vec![a.clone()]);
        assert!(queue.store(&a, StripPass::Develop, frame::placeholder(4, 4, 1)));
        assert!(!queue.store(&a, StripPass::Embedded, frame::placeholder(2, 2, 0)));
        assert_eq!(queue.frame("a"), Some(frame::placeholder(4, 4, 1)));

        queue.replace(Vec::new());
        assert_eq!(queue.frame("a"), None);
        assert!(!queue.store(&a, StripPass::Develop, frame::placeholder(4, 4, 1)));
    }

    #[test]
    fn a_photo_with_no_stock_gets_only_the_preview() {
        let mut queue = StripQueue::default();
        let bare = item("a", "", 0.0);
        queue.replace(vec![bare.clone()]);
        let (_, pass) = queue.next().unwrap();
        queue.store(&bare, pass, frame::placeholder(2, 2, 0));
        assert!(queue.next().is_none());
    }

    #[test]
    fn a_preview_is_turned_and_cropped_like_the_edit() {
        // 4 x 2, left half red, right half blue, stored a quarter turn off.
        let mut rgb = Vec::new();
        for _ in 0..2 {
            rgb.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255]);
        }
        let preview = EmbeddedPreview {
            width: 4,
            height: 2,
            rgb,
            orientation: Orientation::Rotate90,
        };
        let settings = EditSettings::default();
        let packed = StripRender::framed_preview(&preview, &settings).unwrap();
        assert_eq!(frame::dimensions(&packed), Some((2, 4)));

        let mut cropped = EditSettings::default();
        cropped.geometry.crop = Some(crate::contract::CropRect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 0.5,
        });
        let packed = StripRender::framed_preview(&preview, &cropped).unwrap();
        assert_eq!(frame::dimensions(&packed), Some((2, 2)));
        let body = &packed[frame::HEADER..];
        assert!(
            body.chunks(4).all(|pixel| pixel[..3] == body[..3]),
            "one colour"
        );
    }
}
