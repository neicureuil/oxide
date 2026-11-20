//! Public CDN pictures of the pickers (Giphy, Teams stickers and emoji, app icons), loaded without credentials and
//! decoded small (`FunPicker.swift`'s `RemoteThumbnail`). Kept only while a picker is open.
use crate::oxide::Oxide;
use gpui::{AppContext, Context, RenderImage};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Downloads at once; the rest wait their turn.
const PARALLEL: usize = 6;
/// The longest side a thumbnail is decoded to, in pixels.
const MAX_SIDE: u32 = 220;

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
    /// Bumped by `clear`, so downloads still running are dropped when they end.
    generation: u64,
}

impl Thumbs {
    pub fn clear(&mut self) {
        if !self.pictures.is_empty() || !self.queue.is_empty() {
            self.pictures.clear();
            self.queue.clear();
            self.running = 0;
            self.generation += 1;
        }
    }
}

/// The picture at `url`, asked for the first time it is drawn.
pub fn thumb(this: &mut Oxide, url: &str, cx: &mut Context<Oxide>) -> Thumb {
    let thumbs = &mut this.ui.composer.thumbs;
    if let Some(found) = thumbs.pictures.get(url) {
        return found.clone();
    }
    thumbs.pictures.insert(url.to_owned(), Thumb::Loading);
    thumbs.queue.push_back(url.to_owned());
    pump(this, cx);
    Thumb::Loading
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
                thumbs
                    .pictures
                    .insert(url, picture.map_or(Thumb::Failed, Thumb::Ready));
                pump(this, cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// The file at a public address, through the system's curl: this app has no HTTP client of its own.
fn download(url: &str) -> Option<Vec<u8>> {
    if !url.starts_with("https://") {
        return None;
    }
    let out = std::process::Command::new("/usr/bin/curl")
        .args([
            "-sfL",
            "--max-time",
            "20",
            "--max-filesize",
            "20000000",
            "--",
            url,
        ])
        .output()
        .ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
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
        Thumb::Ready(picture) => frame
            .child(img(picture).size_full().object_fit(ObjectFit::Contain))
            .into_any_element(),
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
