//! A message picture shown full size over the window (`ImageViewer` in `MessageRow.swift`): a bar naming who sent it,
//! with the actions, above the picture fitted to the rest of the window. Escape, the back arrow or a click around
//! the picture closes it; the wheel zooms around the pointer, a drag pans once zoomed.
use crate::feed_blocks::Caption;
use crate::icons::Icon;
use crate::oxide::{Oxide, tr};
use crate::widgets;
use gpui::{
    AnyElement, ClipboardItem, Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, RenderImage, ScrollWheelEvent, Size, Window, div, img, point,
    prelude::*, px, size,
};
use std::sync::Arc;

/// Below the top bar and its rule, as in the macOS app.
const TOP: f32 = 53.;
const BAR: f32 = 56.;
const ZOOMS: (f32, f32) = (0.25, 8.);
const STEP: f32 = 1.5;

pub struct Ui {
    alt: String,
    caption: Option<Caption>,
    /// False for a file's thumbnail, which is not the file.
    savable: bool,
    /// Scale over the fitted size, around the picture's centre.
    zoom: f32,
    offset: Point<Pixels>,
    /// Where the last drag event was, while panning.
    drag: Option<Point<Pixels>>,
    focus: FocusHandle,
    /// Takes the keyboard on its first frame, so typing does not reach the composer underneath.
    wants_focus: bool,
}
impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        Self {
            alt: String::new(),
            caption: None,
            savable: true,
            zoom: 1.,
            offset: Point::default(),
            drag: None,
            focus: cx.focus_handle(),
            wants_focus: false,
        }
    }
}

/// Opens `src` (a decoded picture of `this.images`) full size.
pub fn open(
    this: &mut Oxide,
    src: String,
    alt: String,
    caption: Option<Caption>,
    savable: bool,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) {
    let ui = &mut this.ui.image_viewer;
    ui.alt = alt;
    ui.caption = caption;
    ui.savable = savable;
    ui.zoom = 1.;
    ui.offset = Point::default();
    ui.drag = None;
    ui.wants_focus = true;
    this.zoomed = Some(src);
    cx.notify();
}

fn close(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.zoomed = None;
    this.ui.image_viewer.drag = None;
    cx.notify();
}

/// Teams' default alt texts say nothing: only a real name is worth showing.
fn meaningful(alt: &str) -> bool {
    !["image", "images", "image partagée", ""].contains(&alt.to_lowercase().as_str())
}

/// Named after the alt text Teams gives a pasted file, less its extension and the path separators a name cannot hold.
pub fn file_name(alt: &str) -> String {
    let stem = std::path::Path::new(alt)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
        .replace(['/', ':'], "-");
    format!(
        "{}.png",
        if meaningful(alt) && !stem.is_empty() {
            stem
        } else {
            "Image".into()
        }
    )
}

/// Native size, shrunk to the area less its side margins if needed (never enlarged past 2×).
pub fn fitted_size(native: Size<f32>, area: Size<f32>) -> Size<f32> {
    let bounds = size((area.width - 128.).max(1.), (area.height - 48.).max(1.));
    if native.width <= 0. || native.height <= 0. {
        return bounds;
    }
    let scale = (bounds.width / native.width)
        .min(bounds.height / native.height)
        .min(2.);
    size(native.width * scale, native.height * scale)
}

/// `value` within the zoom range, and the offset that keeps `anchor` (from the area's centre) in place.
pub fn zoomed(
    value: f32,
    previous: f32,
    offset: Point<f32>,
    anchor: Point<f32>,
) -> (f32, Point<f32>) {
    let next = value.clamp(ZOOMS.0, ZOOMS.1);
    if next <= 1. {
        return (next, point(0., 0.));
    }
    let ratio = next / previous;
    (
        next,
        point(
            anchor.x - ratio * (anchor.x - offset.x),
            anchor.y - ratio * (anchor.y - offset.y),
        ),
    )
}

fn set_zoom(this: &mut Oxide, value: f32, anchor: Point<f32>, cx: &mut Context<Oxide>) {
    let ui = &mut this.ui.image_viewer;
    if value == 1. {
        ui.zoom = 1.;
        ui.offset = Point::default();
    } else {
        let offset = point(f32::from(ui.offset.x), f32::from(ui.offset.y));
        let (zoom, offset) = zoomed(value, ui.zoom, offset, anchor);
        ui.zoom = zoom;
        ui.offset = point(px(offset.x), px(offset.y));
    }
    cx.notify();
}

/// PNG bytes of a decoded picture (BGRA frames), for the clipboard and saving.
fn png(picture: &RenderImage) -> Option<Vec<u8>> {
    let size = picture.size(0);
    let (w, h) = (size.width.0 as u32, size.height.0 as u32);
    let mut pixels = picture.as_bytes(0)?.to_vec();
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let image = image::RgbaImage::from_raw(w, h, pixels)?;
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

fn copy(picture: &RenderImage, cx: &mut Context<Oxide>) {
    if let Some(bytes) = png(picture) {
        cx.write_to_clipboard(ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            bytes,
        )));
    }
}

/// Saved as PNG: the viewer holds the decoded picture, not the bytes Teams served.
fn save(picture: Arc<RenderImage>, alt: &str, cx: &mut Context<Oxide>) {
    let dir = std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join("Downloads"))
        .unwrap_or_default();
    let answer = cx.prompt_for_new_path(&dir, Some(&file_name(alt)));
    cx.spawn(async move |_, _| {
        if let Ok(Ok(Some(path))) = answer.await
            && let Some(bytes) = png(&picture)
            && let Err(error) = std::fs::write(&path, bytes)
        {
            eprintln!("saving {}: {error}", path.display());
        }
    })
    .detach();
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    let src = this.zoomed.clone()?;
    // The picture left the core's cache (the conversation changed): nothing to show.
    let Some(picture) = this.images.images.get(&src).cloned() else {
        this.zoomed = None;
        return None;
    };
    let theme = this.theme;
    let ui = &mut this.ui.image_viewer;
    if ui.wants_focus {
        ui.wants_focus = false;
        window.focus(&ui.focus);
    }
    let viewport = window.viewport_size();
    let area = size(
        f32::from(viewport.width),
        (f32::from(viewport.height) - TOP - BAR).max(1.),
    );
    let native = picture.size(0);
    let fitted = fitted_size(size(native.width.0 as f32, native.height.0 as f32), area);
    let (w, h) = (fitted.width * ui.zoom, fitted.height * ui.zoom);
    let left = (area.width - w) / 2. + f32::from(ui.offset.x);
    let top = (area.height - h) / 2. + f32::from(ui.offset.y);
    let zoom = ui.zoom;
    let moved = zoom != 1. || ui.offset != Point::default();
    let alt = ui.alt.clone();
    let title = ui
        .caption
        .as_ref()
        .map(|c| c.sender.clone())
        .unwrap_or_else(|| {
            if meaningful(&alt) {
                alt.clone()
            } else {
                tr("ui.message_row.image")
            }
        });
    // « image | 10:00 » under the sender, as Teams words it.
    let subtitle = ui.caption.as_ref().map(|c| {
        format!(
            "{} | {}",
            if meaningful(&alt) {
                alt.clone()
            } else {
                tr("ui.message_row.image")
            },
            c.time
        )
    });
    let savable = ui.savable;
    let focus = ui.focus.clone();

    let button = |id: &'static str, glyph: Icon, help: String| {
        widgets::icon_button_sized(id, glyph, 30., 14., false, &theme)
            .tooltip(widgets::tooltip(help, theme))
    };
    let bar = div()
        .h(px(BAR))
        .flex_none()
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(6.))
        .bg(theme.surface)
        .border_b_1()
        .border_color(theme.border)
        .child(
            button("viewer-close", Icon::ArrowLeft, tr("ui.message_row.close"))
                .on_click(cx.listener(|this, _, _, cx| close(this, cx))),
        )
        .child(
            div()
                .pl(px(6.))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .children(subtitle.map(|s| {
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(theme.muted)
                        .child(s)
                })),
        )
        .when(moved, |d| {
            d.child(
                button("viewer-fit", Icon::Scope, tr("ui.message_row.fit_image"))
                    .on_click(cx.listener(|this, _, _, cx| set_zoom(this, 1., point(0., 0.), cx))),
            )
        })
        .child(widgets::disabled(
            button(
                "viewer-zoom-out",
                Icon::ZoomOut,
                tr("ui.message_row.zoom_out"),
            )
            .on_click(
                cx.listener(move |this, _, _, cx| set_zoom(this, zoom / STEP, point(0., 0.), cx)),
            ),
            zoom <= ZOOMS.0,
        ))
        .child(widgets::disabled(
            button("viewer-zoom-in", Icon::ZoomIn, tr("ui.message_row.zoom_in")).on_click(
                cx.listener(move |this, _, _, cx| set_zoom(this, zoom * STEP, point(0., 0.), cx)),
            ),
            zoom >= ZOOMS.1,
        ))
        .child(div().mx_1().w(px(1.)).h(px(18.)).bg(theme.border))
        .child({
            let picture = picture.clone();
            button("viewer-copy", Icon::Copy, tr("ui.message_row.copy_image"))
                .on_click(cx.listener(move |_, _, _, cx| copy(&picture, cx)))
        })
        .when(savable, |d| {
            let picture = picture.clone();
            let alt = alt.clone();
            d.child(
                button(
                    "viewer-save",
                    Icon::Download,
                    tr("ui.message_row.save_image"),
                )
                .on_click(cx.listener(move |_, _, _, cx| save(picture.clone(), &alt, cx))),
            )
        });

    let image = div()
        .id("viewer-picture")
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(w))
        .h(px(h))
        .when(zoom > 1., |d| d.cursor_grab())
        .child(img(picture.clone()).size_full())
        // A click on the picture is not one around it.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if this.ui.image_viewer.zoom > 1. {
                    this.ui.image_viewer.drag = Some(event.position);
                }
            }),
        );
    let stage = div()
        .id("viewer-stage")
        .relative()
        .flex_1()
        .w_full()
        .overflow_hidden()
        .bg(gpui::black().opacity(0.55))
        .child(image)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| close(this, cx)),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            let ui = &mut this.ui.image_viewer;
            let Some(last) = ui.drag else { return };
            if event.pressed_button != Some(MouseButton::Left) {
                ui.drag = None;
                return;
            }
            ui.offset = ui.offset + (event.position - last);
            ui.drag = Some(event.position);
            cx.notify();
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.ui.image_viewer.drag = None),
        )
        .on_scroll_wheel(
            cx.listener(move |this, event: &ScrollWheelEvent, window, cx| {
                let dy = f32::from(event.delta.pixel_delta(px(16.)).y);
                if dy == 0. {
                    return;
                }
                let speed = if event.delta.precise() {
                    0.01
                } else {
                    0.1 / 16.
                };
                let viewport = window.viewport_size();
                let anchor = point(
                    f32::from(event.position.x) - f32::from(viewport.width) / 2.,
                    f32::from(event.position.y)
                        - (TOP + BAR + (f32::from(viewport.height) - TOP - BAR) / 2.),
                );
                let zoom = this.ui.image_viewer.zoom;
                set_zoom(this, zoom * (dy * speed).exp(), anchor, cx);
            }),
        );
    Some(
        // Fades over the window, as Swift's viewer comes in with `.transition(.opacity)`.
        gpui::deferred(crate::motion::fade_in(
            div()
                .id("image-viewer")
                .occlude()
                .absolute()
                .top(px(TOP))
                .left_0()
                .size_full()
                .w(viewport.width)
                .h(px(f32::from(viewport.height) - TOP))
                .flex()
                .flex_col()
                .track_focus(&focus)
                .font_family(crate::oxide::FONT)
                .text_color(theme.text)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    let key = event.keystroke.key.as_str();
                    let cmd = event.keystroke.modifiers.platform;
                    let zoom = this.ui.image_viewer.zoom;
                    match key {
                        "escape" => close(this, cx),
                        "0" if cmd => set_zoom(this, 1., point(0., 0.), cx),
                        "-" if cmd => set_zoom(this, zoom / STEP, point(0., 0.), cx),
                        "=" if cmd => set_zoom(this, zoom * STEP, point(0., 0.), cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
                .child(bar)
                .child(stage),
            "image-viewer-in",
        ))
        .with_priority(3)
        .into_any_element(),
    )
}

/// `zoom:<src index>` opens the n-th decoded picture of the open conversation; `zoom:in` zooms it.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(arg) = step.strip_prefix("zoom:") else {
        return false;
    };
    if arg == "in" {
        let zoom = this.ui.image_viewer.zoom;
        set_zoom(this, zoom * STEP, point(0., 0.), cx);
        return true;
    }
    let mut srcs: Vec<String> = this.images.images.keys().cloned().collect();
    srcs.sort();
    let Some(src) = arg.parse::<usize>().ok().and_then(|n| srcs.get(n).cloned()) else {
        return false;
    };
    let caption = Some(Caption {
        sender: "Bruno Lefèvre".into(),
        time: "12:00".into(),
    });
    open(
        this,
        src,
        "Image de démonstration".into(),
        caption,
        true,
        window,
        cx,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_within_margins_and_never_past_twice() {
        let area = size(1128., 648.);
        assert_eq!(fitted_size(size(100., 50.), area), size(200., 100.));
        let big = fitted_size(size(4000., 1000.), area);
        assert_eq!(big.width, 1000.);
        assert_eq!(big.height, 250.);
    }

    #[test]
    fn zoom_keeps_the_anchor_and_recentres_at_fit() {
        let (z, o) = zoomed(2., 1., point(0., 0.), point(100., 0.));
        assert_eq!(z, 2.);
        assert_eq!(o, point(-100., 0.));
        assert_eq!(
            zoomed(0.5, 2., point(30., 30.), point(0., 0.)),
            (0.5, point(0., 0.))
        );
        assert_eq!(zoomed(100., 1., point(0., 0.), point(0., 0.)).0, 8.);
    }

    #[test]
    fn file_names_drop_extensions_and_useless_alts() {
        assert_eq!(file_name("capture.png"), "capture.png");
        assert_eq!(file_name("image"), "Image.png");
        assert_eq!(file_name("a:b.jpg"), "a-b.png");
    }
}
