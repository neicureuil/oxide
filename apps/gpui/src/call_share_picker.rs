//! Share sheet (`ScreenSharePicker.swift`): pick a screen or a window from the engine's capture lists, then share it.
//! The lists and their thumbnails (up to 2 s each) are read on a thread of their own, one source at a time.
use crate::icons::{Icon, icon};
use crate::media_devices::permissions;
use crate::media_engine::MediaMsg;
use crate::oxide::{Oxide, tr, trf};
use crate::widgets::{self, ButtonKind};
use futures::channel::mpsc::UnboundedSender;
use gpui::{AnyElement, Context, FontWeight, RenderImage, Window, div, img, prelude::*, px};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use teams_core::{Action, CallAction};
use teams_media::{ScreenSource, ScreenSourceKind};

/// Thumbnail width, in pixels.
const THUMBNAIL_WIDTH: u32 = 320;

/// A screen or window that can be shared.
pub struct Source {
    pub source: ScreenSource,
    pub id: String,
    pub name: String,
    pub window: bool,
    pub thumbnail: Option<Arc<RenderImage>>,
}

/// What the sources thread finds, for the open sheet.
pub enum PickerMsg {
    Sources {
        sheet: u64,
        screens: Vec<ScreenSource>,
        windows: Vec<ScreenSource>,
    },
    Thumbnail {
        sheet: u64,
        source: ScreenSource,
        image: Arc<RenderImage>,
    },
}

/// The open share sheet.
pub struct ScreenPicker {
    /// Which sheet the thread's messages belong to.
    sheet: u64,
    pub screens: Vec<Source>,
    pub windows: Vec<Source>,
    pub selected: Option<String>,
    pub allowed: bool,
    /// The sources are still being listed.
    loading: bool,
    /// Tells the sources thread to stop once the sheet closes.
    cancelled: Arc<AtomicBool>,
    /// The window drawing the thumbnails, whose atlas frees them once the sheet closes.
    drawn_in: Option<gpui::WindowId>,
}

impl ScreenPicker {
    fn all(&self) -> impl Iterator<Item = &Source> {
        self.screens.iter().chain(self.windows.iter())
    }
}

impl Drop for ScreenPicker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(window) = self.drawn_in {
            for source in self.screens.iter_mut().chain(self.windows.iter_mut()) {
                if let Some(image) = source.thumbnail.take() {
                    crate::media_video::retire(window, image);
                }
            }
        }
    }
}

fn source_id(source: &ScreenSource) -> String {
    let kind = match source.kind {
        ScreenSourceKind::Screen => "screen",
        ScreenSourceKind::Window => "window",
    };
    format!("{kind}-{}", source.id)
}

/// RGBA rows into the BGRA GPUI draws.
fn render_image(width: u32, height: u32, mut rgba: Vec<u8>) -> Option<Arc<RenderImage>> {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(width, height, rgba)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(image)])))
}

static NEXT_SHEET: AtomicU64 = AtomicU64::new(1);

/// Lists screens and windows, then makes their thumbnails one by one, until the sheet closes.
fn load(sheet: u64, cancelled: Arc<AtomicBool>, tx: UnboundedSender<MediaMsg>) {
    let spawned = std::thread::Builder::new()
        .name("oxide.share-sources".into())
        .spawn(move || {
            let screens = teams_media::screen_sources(ScreenSourceKind::Screen);
            let own = own_window_ids();
            let windows: Vec<ScreenSource> = teams_media::screen_sources(ScreenSourceKind::Window)
                .into_iter()
                // Not our own windows (the macOS app matches its name; ours are named after the chat or call).
                .filter(|w| !w.title.trim().is_empty() && !own.contains(&w.id))
                .collect();
            let all: Vec<ScreenSource> = screens.iter().chain(windows.iter()).cloned().collect();
            let _ = tx.unbounded_send(MediaMsg::Picker(PickerMsg::Sources {
                sheet,
                screens,
                windows,
            }));
            for source in all {
                if cancelled.load(Ordering::Acquire) {
                    return;
                }
                let Some(image) = source
                    .thumbnail(THUMBNAIL_WIDTH)
                    .and_then(|(w, h, rgba)| render_image(w, h, rgba))
                else {
                    continue;
                };
                let _ = tx.unbounded_send(MediaMsg::Picker(PickerMsg::Thumbnail {
                    sheet,
                    source,
                    image,
                }));
            }
        });
    if let Err(error) = spawned {
        crate::media_engine::trace(&format!("share sources: {error}"));
    }
}

/// Window numbers of this process, as the capturer names window sources on macOS.
#[cfg(target_os = "macos")]
fn own_window_ids() -> Vec<u64> {
    use crate::objc_util;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *mut AnyObject;
    }
    // kCGWindowListOptionAll
    let list = unsafe { CGWindowListCopyWindowInfo(0, 0) };
    let Some(list) = (unsafe { Retained::from_raw(list) }) else {
        return Vec::new();
    };
    let pid = std::process::id() as f64;
    objc_util::array(Some(&list))
        .iter()
        .filter(|w| objc_util::double(objc_util::get(w, "kCGWindowOwnerPID").as_deref()) == pid)
        .map(|w| objc_util::double(objc_util::get(w, "kCGWindowNumber").as_deref()) as u64)
        .collect()
}

/// Our windows are named after a chat or call: nothing tells them apart from the outside here.
#[cfg(not(target_os = "macos"))]
fn own_window_ids() -> Vec<u64> {
    Vec::new()
}

/// What the sources thread found, into the sheet it was started for.
pub fn on_msg(this: &mut Oxide, msg: PickerMsg) {
    let Some(picker) = this.ui.call.picker.as_mut() else {
        return;
    };
    match msg {
        PickerMsg::Sources {
            sheet,
            screens,
            windows,
        } if sheet == picker.sheet => {
            let source = |source: ScreenSource| Source {
                id: source_id(&source),
                // Screens have no title: they're numbered.
                name: source.title.clone(),
                window: source.kind == ScreenSourceKind::Window,
                thumbnail: None,
                source,
            };
            picker.screens = screens.into_iter().map(source).collect();
            for (n, screen) in picker.screens.iter_mut().enumerate() {
                screen.name = trf(
                    "ui.screen_share_picker.screen_number",
                    &[("n", &(n + 1).to_string())],
                );
            }
            picker.windows = windows.into_iter().map(source).collect();
            picker.loading = false;
            if picker.selected.is_none() {
                picker.selected = picker.screens.first().map(|s| s.id.clone());
            }
        }
        PickerMsg::Thumbnail {
            sheet,
            source,
            image,
        } if sheet == picker.sheet => {
            let id = source_id(&source);
            if let Some(found) = picker
                .screens
                .iter_mut()
                .chain(picker.windows.iter_mut())
                .find(|s| s.id == id)
            {
                found.thumbnail = Some(image);
            }
        }
        _ => {}
    }
}

pub fn open(this: &mut Oxide) {
    let mut allowed = permissions::screen_capture(false);
    if !allowed {
        allowed = permissions::screen_capture(true);
    }
    let sheet = NEXT_SHEET.fetch_add(1, Ordering::Relaxed);
    let cancelled = Arc::new(AtomicBool::new(false));
    if allowed {
        load(sheet, cancelled.clone(), this.ui.call.media.tx.clone());
    }
    this.ui.call.picker = Some(ScreenPicker {
        sheet,
        screens: Vec::new(),
        windows: Vec::new(),
        selected: None,
        allowed,
        loading: allowed,
        cancelled,
        drawn_in: None,
    });
}

pub fn cancel(this: &mut Oxide) {
    this.ui.call.picker = None;
}

/// Hands the choice to the engine and asks the core to share.
pub fn share(this: &mut Oxide) {
    let Some(picker) = this.ui.call.picker.take() else {
        return;
    };
    let Some(source) = picker
        .selected
        .as_deref()
        .and_then(|id| picker.all().find(|s| s.id == id))
        .map(|s| s.source.clone())
    else {
        return;
    };
    this.ui.call.media.choose_screen(Some(source));
    this.send(Action::Call {
        action: CallAction::SetScreenShare { on: true },
    });
}

fn tile(this: &Oxide, source: &Source, selected: bool, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let id = source.id.clone();
    div()
        .id(gpui::SharedString::from(format!(
            "share-{}-{}",
            source.window, source.id
        )))
        .w(px(180.))
        .p(px(6.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .rounded(px(10.))
        .cursor_pointer()
        .border(px(if selected { 2. } else { 1. }))
        .border_color(if selected { theme.accent } else { theme.border })
        .when(selected, |d| d.bg(theme.accent_soft))
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(picker) = this.ui.call.picker.as_mut() {
                picker.selected = Some(id.clone());
            }
            crate::call_window::redraw(this, cx);
        }))
        .child(
            div()
                .h(px(96.))
                .w_full()
                .rounded(px(8.))
                .bg(gpui::black().opacity(0.85))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .map(|d| match &source.thumbnail {
                    Some(image) => d.child(img(image.clone()).max_w_full().max_h_full()),
                    None => d.child(icon(
                        if source.window {
                            Icon::AppWindow
                        } else {
                            Icon::Screen
                        },
                        px(28.),
                        gpui::white().opacity(0.7),
                    )),
                }),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_ellipsis()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(source.name.clone()),
        )
        .into_any_element()
}

fn section(
    this: &Oxide,
    title: String,
    items: &[Source],
    selected: Option<&str>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(this.theme.muted)
                .child(title),
        )
        .child(
            div().flex().flex_wrap().gap(px(12.)).children(
                items
                    .iter()
                    .map(|s| tile(this, s, selected == Some(s.id.as_str()), cx)),
            ),
        )
        .into_any_element()
}

/// The sheet over the call window.
pub fn view(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let picker = this.ui.call.picker.as_mut()?;
    picker.drawn_in = Some(window.window_handle().window_id());
    let picker = this.ui.call.picker.as_ref()?;
    let theme = this.theme;
    let allowed = picker.allowed;
    let loading = picker.loading;
    let selected = picker.selected.clone();
    let body = if allowed && loading {
        // The lists come within a moment; thumbnails fill in after.
        div().flex_1().into_any_element()
    } else if allowed {
        let (screens, windows) = (&picker.screens, &picker.windows);
        div()
            .id("share-sources")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section(
                this,
                tr("ui.screen_share_picker.screens"),
                screens,
                selected.as_deref(),
                cx,
            ))
            .when(!windows.is_empty(), |d| {
                d.child(section(
                    this,
                    tr("ui.screen_share_picker.windows"),
                    windows,
                    selected.as_deref(),
                    cx,
                ))
            })
            .into_any_element()
    } else {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .child(widgets::muted(tr("ui.screen_share_picker.oxide_must_be_allowed_in_system"), 13., &theme))
            .child(
                widgets::button("share-settings", tr("ui.screen_share_picker.open_settings"), ButtonKind::Outline, &theme)
                    .on_click(cx.listener(|_, _, _, cx| {
                        permissions::screen_capture(true);
                        cx.open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");
                    })),
            )
            .into_any_element()
    };
    let can_share = allowed && selected.is_some();
    let content = div()
        .h(px(520.))
        .p(px(22.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(widgets::title(
            tr("ui.screen_share_picker.share_screen"),
            18.,
        ))
        .child(body)
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    widgets::button(
                        "share-cancel",
                        tr("ui.screen_share_picker.cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cancel(this);
                        crate::call_window::redraw(this, cx);
                    })),
                )
                .child(
                    widgets::disabled(
                        widgets::button(
                            "share-go",
                            tr("ui.screen_share_picker.share"),
                            ButtonKind::Primary,
                            &theme,
                        ),
                        !can_share,
                    )
                    .when(can_share, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| {
                            share(this);
                            crate::call_window::redraw(this, cx);
                        }))
                    }),
                ),
        );
    let close = cx.listener(|this, _: &(), _, cx| {
        cancel(this);
        crate::call_window::redraw(this, cx);
    });
    Some(widgets::modal(
        "share-picker",
        px(640.),
        content,
        &theme,
        move |window, cx| close(&(), window, cx),
    ))
}
