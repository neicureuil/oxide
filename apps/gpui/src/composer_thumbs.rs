//! Public CDN pictures of the pickers (Giphy, Teams stickers and emoji, app icons), loaded without credentials and
//! decoded small (`FunPicker.swift`'s `RemoteThumbnail`). Decoded pictures are kept only while a picker is
//! open; small files stay on disk, as the Swift app's URL cache keeps them, so the next opening shows them
//! at once instead of downloading them again one by one.
use crate::oxide::Oxide;
use gpui::{AppContext, Context, RenderImage};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Downloads at once; the rest wait their turn.
const PARALLEL: usize = 6;
/// The longest side a thumbnail is decoded to, in pixels.
const MAX_SIDE: u32 = 220;
/// Files kept on disk at most this big: emoji, stickers and icons, not GIF previews.
const DISK_MAX_FILE: usize = 256 * 1024;
/// Arrivals are shown together, at most this long after the first of a batch.
const REVEAL_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Clone)]
pub enum Thumb {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

#[derive(Default)]
pub struct Thumbs {
    pictures: HashMap<String, Thumb>,
    queue: VecDeque<String>,
    running: usize,
    /// Downloaded and decoded, waiting for the batch to be shown.
    arrived: Vec<(String, Thumb)>,
    reveal_scheduled: bool,
    /// When each picture was shown, for its fade in.
    revealed: HashMap<String, std::time::Instant>,
    /// Bumped by `clear`, so downloads still running are dropped when they end.
    generation: u64,
}

impl Thumbs {
    /// Forgets every picture; returns the decoded ones, to free on the GPU.
    pub fn clear(&mut self) -> Vec<Arc<RenderImage>> {
        if self.pictures.is_empty() && self.queue.is_empty() {
            return Vec::new();
        }
        self.queue.clear();
        self.running = 0;
        self.generation += 1;
        self.revealed.clear();
        self.reveal_scheduled = false;
        let arrived: Vec<Arc<RenderImage>> = self
            .arrived
            .drain(..)
            .filter_map(|(_, thumb)| match thumb {
                Thumb::Ready(image) => Some(image),
                _ => None,
            })
            .collect();
        self.pictures
            .drain()
            .filter_map(|(_, thumb)| match thumb {
                Thumb::Ready(image) => Some(image),
                _ => None,
            })
            .chain(arrived)
            .collect()
    }

    /// Shown in the last moment: drawn with a fade in.
    pub fn fresh(&self, url: &str) -> bool {
        self.revealed
            .get(url)
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400))
    }
}

/// The picture at `url`, asked for the first time it is drawn.
pub fn thumb(this: &mut Oxide, url: &str, cx: &mut Context<Oxide>) -> Thumb {
    let thumbs = &mut this.ui.composer.thumbs;
    if let Some(found) = thumbs.pictures.get(url) {
        return found.clone();
    }
    // Kept on disk from an earlier opening: decoded here and now, so it shows with the picker.
    if let Some(picture) = disk::read(url).and_then(|bytes| decode(&bytes)) {
        let thumb = Thumb::Ready(picture);
        thumbs.pictures.insert(url.to_owned(), thumb.clone());
        return thumb;
    }
    thumbs.pictures.insert(url.to_owned(), Thumb::Loading);
    thumbs.queue.push_back(url.to_owned());
    pump(this, cx);
    Thumb::Loading
}

/// Shows the pictures that arrived since the last batch, together.
fn reveal(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let thumbs = &mut this.ui.composer.thumbs;
    thumbs.reveal_scheduled = false;
    if thumbs.arrived.is_empty() {
        return;
    }
    let now = std::time::Instant::now();
    for (url, thumb) in thumbs.arrived.drain(..) {
        if matches!(thumb, Thumb::Ready(_)) {
            thumbs.revealed.insert(url.clone(), now);
        }
        thumbs.pictures.insert(url, thumb);
    }
    cx.notify();
}

fn pump(this: &mut Oxide, cx: &mut Context<Oxide>) {
    while this.ui.composer.thumbs.running < PARALLEL {
        let Some(url) = this.ui.composer.thumbs.queue.pop_front() else {
            return;
        };
        this.ui.composer.thumbs.running += 1;
        let generation = this.ui.composer.thumbs.generation;
        let task = cx.background_spawn({
            let url = url.clone();
            async move { download(&url).and_then(|bytes| decode(&bytes)) }
        });
        cx.spawn(async move |this, cx| {
            let picture = task.await;
            let _ = this.update(cx, |this, cx| {
                let thumbs = &mut this.ui.composer.thumbs;
                if thumbs.generation != generation {
                    return;
                }
                thumbs.running = thumbs.running.saturating_sub(1);
                // Not shown one by one as they come (each would replace its glyph in turn): together, when the
                // batch is done or a moment after the first of it arrived.
                thumbs.arrived.push((url, picture.map_or(Thumb::Failed, Thumb::Ready)));
                let done = thumbs.running == 0 && thumbs.queue.is_empty();
                pump(this, cx);
                if done {
                    reveal(this, cx);
                } else if !this.ui.composer.thumbs.reveal_scheduled {
                    this.ui.composer.thumbs.reveal_scheduled = true;
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(REVEAL_EVERY).await;
                        let _ = this.update(cx, |this, cx| {
                            if this.ui.composer.thumbs.generation == generation {
                                reveal(this, cx);
                            }
                        });
                    })
                    .detach();
                }
            });
        })
        .detach();
    }
}

/// One client for every picture: its connections to the CDNs stay open between files.
fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_default()
    })
}

/// The file at a public address, from disk if kept there, else downloaded (and kept if small).
fn download(url: &str) -> Option<Vec<u8>> {
    if !url.starts_with("https://") {
        return None;
    }
    if let Some(bytes) = disk::read(url) {
        return Some(bytes);
    }
    let response = client().get(url).send().ok()?.error_for_status().ok()?;
    let bytes = response.bytes().ok()?;
    if bytes.is_empty() || bytes.len() > 20_000_000 {
        return None;
    }
    if bytes.len() <= DISK_MAX_FILE {
        disk::write(url, &bytes);
    }
    Some(bytes.to_vec())
}

/// Small picker files kept between openings, in the app's cache folder.
mod disk {
    use std::hash::{Hash, Hasher};
    use std::path::PathBuf;

    /// Files kept at most: the oldest go past this.
    const MAX_FILES: usize = 4000;

    fn dir() -> Option<PathBuf> {
        Some(crate::paths::caches()?.join("pictures"))
    }
    fn path(url: &str) -> Option<PathBuf> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        url.hash(&mut hasher);
        Some(dir()?.join(format!("{:016x}", hasher.finish())))
    }
    pub fn read(url: &str) -> Option<Vec<u8>> {
        std::fs::read(path(url)?).ok().filter(|b| !b.is_empty())
    }
    pub fn write(url: &str, bytes: &[u8]) {
        let (Some(dir), Some(path)) = (dir(), path(url)) else { return };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        // Written aside then renamed: a reader never sees half a file.
        let partial = path.with_extension("part");
        if std::fs::write(&partial, bytes).is_ok() {
            let _ = std::fs::rename(&partial, &path);
        }
        trim(&dir);
    }
    /// Keeps the newest `MAX_FILES`, checked now and then rather than on every write.
    fn trim(dir: &PathBuf) {
        static WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 200 != 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        if files.len() <= MAX_FILES {
            return;
        }
        files.sort();
        for (_, path) in &files[..files.len() - MAX_FILES] {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The first frame, at most `MAX_SIDE` pixels on its longest side, in the BGRA GPUI uploads.
fn decode(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let picture = image::load_from_memory(bytes).ok()?;
    let picture = if picture.width() > MAX_SIDE || picture.height() > MAX_SIDE {
        picture.thumbnail(MAX_SIDE, MAX_SIDE)
    } else {
        picture
    };
    let mut pixels = picture.into_rgba8();
    for px in pixels.pixels_mut() {
        px.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

/// A thumbnail filling its box: the picture, a spinner while it loads, a placeholder if it failed.
pub fn view(
    this: &mut Oxide,
    url: &str,
    id: impl Into<gpui::ElementId>,
    cx: &mut Context<Oxide>,
) -> gpui::AnyElement {
    use gpui::{ObjectFit, div, img, prelude::*, px};
    let theme = this.theme;
    let frame = div()
        .size_full()
        .rounded(px(6.))
        .overflow_hidden()
        .bg(theme.input)
        .flex()
        .items_center()
        .justify_center();
    match thumb(this, url, cx) {
        Thumb::Ready(picture) => {
            let picture = img(picture).size_full().object_fit(ObjectFit::Contain);
            let picture = if this.ui.composer.thumbs.fresh(url) {
                crate::motion::fade_in(picture, gpui::SharedString::from(format!("thumb-in-{url}")))
            } else {
                picture.into_any_element()
            };
            frame.child(picture).into_any_element()
        }
        Thumb::Loading => frame
            .child(crate::widgets::spinner(id, 12., theme.muted))
            .into_any_element(),
        Thumb::Failed => frame
            .child(crate::icons::icon(
                crate::icons::Icon::Image,
                px(16.),
                theme.subtle,
            ))
            .into_any_element(),
    }
}
