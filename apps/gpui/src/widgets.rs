//! Shared building blocks, styled like the macOS app's `PrimaryButtonStyle`, `IconButton`, cards and menus.
use crate::icons::{Icon, icon};
use crate::input::{InputColors, TextInput, field};
use crate::theme::Theme;
use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, Context, Corner, Div, ElementId, Entity,
    FontWeight, Hsla, IntoElement, MouseButton, Pixels, SharedString, Stateful, Transformation,
    Window, anchored, deferred, div, percentage, prelude::*, px,
};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Outline,
    Ghost,
    Danger,
}

/// A text button; add `.on_click` to it.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, kind: ButtonKind, theme: &Theme) -> Stateful<Div> {
    let (bg, fg, border) = match kind {
        ButtonKind::Primary => (Some(theme.accent), gpui::white(), None),
        ButtonKind::Danger => (Some(theme.danger), gpui::white(), None),
        ButtonKind::Outline => (Some(theme.surface), theme.text, Some(theme.border_strong)),
        ButtonKind::Ghost => (None, theme.accent_text, None),
    };
    let id: ElementId = id.into();
    let key = format!("{id}");
    let hover = match kind {
        // Filled buttons lighten a touch; the others take the input tint.
        ButtonKind::Primary | ButtonKind::Danger => gpui::white().opacity(0.12),
        _ => theme.input,
    };
    let button = div()
        .id(id)
        .h(px(32.))
        .px_3()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap_1p5()
        .rounded(px(7.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .cursor_pointer()
        .when_some(bg, |d, bg| d.bg(bg))
        .when_some(border, |d, b| d.border_1().border_color(b));
    crate::motion::hover_fill(button, key, hover, px(7.)).child(label.into())
}

/// A button that is disabled: drawn faded, clicks ignored by the caller.
pub fn disabled(button: Stateful<Div>, disabled: bool) -> Stateful<Div> {
    if disabled { button.opacity(0.45).cursor_default() } else { button }
}

/// An icon-only button, `side` points square; `active` fills it with the accent tint.
pub fn icon_button(id: impl Into<ElementId>, glyph: Icon, theme: &Theme) -> Stateful<Div> {
    icon_button_sized(id, glyph, 30., 16., false, theme)
}
pub fn icon_button_sized(
    id: impl Into<ElementId>,
    glyph: Icon,
    side: f32,
    glyph_size: f32,
    active: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let key = format!("{id}");
    let button = div()
        .id(id)
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor_pointer()
        .when(active, |d| d.bg(theme.accent_soft));
    let button = if active { button } else { crate::motion::hover_fill(button, key, theme.input, px(7.)) };
    button.child(icon(glyph, px(glyph_size), if active { theme.accent_text } else { theme.muted }))
}

/// A pill, as the chat list's filters.
pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool, theme: &Theme) -> Stateful<Div> {
    let id: ElementId = id.into();
    let key = format!("{id}");
    let chip = div()
        .id(id)
        .h(px(26.))
        .px_3()
        .flex()
        .items_center()
        .rounded_full()
        .text_size(px(12.))
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.accent_soft).text_color(theme.accent_text).font_weight(FontWeight::MEDIUM))
        .when(!selected, move |d| d.text_color(theme.muted).border_1().border_color(theme.border));
    let chip = if selected { chip } else { crate::motion::hover_fill(chip, key, theme.input, px(13.)) };
    chip.child(label.into())
}

/// A rounded surface with a hairline border.
pub fn card(theme: &Theme) -> Div {
    div()
        .rounded(px(12.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface)
}

pub fn muted(text: impl Into<SharedString>, size: f32, theme: &Theme) -> Div {
    div().text_size(px(size)).text_color(theme.muted).child(text.into())
}

pub fn title(text: impl Into<SharedString>, size: f32) -> Div {
    div().text_size(px(size)).font_weight(FontWeight::SEMIBOLD).child(text.into())
}

pub fn divider(theme: &Theme) -> Div {
    div().h(px(1.)).w_full().flex_none().bg(theme.border)
}

/// A count badge, as on the rail.
pub fn badge(count: u32, theme: &Theme) -> Div {
    div()
        .min_w(px(16.))
        .h(px(16.))
        .px(px(4.))
        .rounded_full()
        .bg(theme.danger)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .child(if count > 99 { "99+".to_owned() } else { count.to_string() })
}

/// Grey bars standing in for list rows while the first page loads (`SkeletonList`), with the shimmer.
pub fn skeleton(id: &str, rows: usize, avatar: f32, theme: &Theme) -> Div {
    const TITLE: [f32; 6] = [120., 90., 140., 105., 80., 130.];
    const INSET: [f32; 6] = [30., 70., 10., 50., 90., 40.];
    // The column the skeletons sit in (316 pt), so the band crosses it in one sweep.
    const SWEEP: f32 = 316.;
    let tint = if theme.dark { gpui::white().opacity(0.10) } else { gpui::white().opacity(0.55) };
    let text_x = 10. + avatar + 12.;
    let mut column = div().flex().flex_col();
    for ix in 0..rows {
        let shape = |w: Div, part: &str, x: f32| {
            crate::motion::shimmer_shape(w.bg(theme.input), SharedString::from(format!("{id}-{ix}-{part}")), x, SWEEP, tint)
        };
        column = column.child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(10.))
                .py(px(8.))
                // Fades towards the bottom, so the list reads as "more to come".
                .opacity(1. - ix as f32 / rows as f32 * 0.6)
                .child(shape(div().size(px(avatar)).flex_none().rounded_full(), "avatar", 10.))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(7.))
                        .child(shape(div().w(px(TITLE[ix % 6])).h(px(11.)).rounded(px(4.)), "title", text_x))
                        .child(div().pr(px(INSET[ix % 6])).child(shape(div().h(px(9.)).rounded(px(4.)), "preview", text_x))),
                ),
        );
    }
    column
}

/// A turning ring, for loading states.
pub fn spinner(id: impl Into<ElementId>, size: f32, color: Hsla) -> AnyElement {
    icon(Icon::Spinner, px(size), color)
        .with_animation(
            id,
            Animation::new(Duration::from_millis(900)).repeat(),
            |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
        )
        .into_any_element()
}

/// "Loading…" next to a spinner.
/// Shown only once loading lasts past a blink, so fast loads do not flash (`LoadingLabel`).
pub fn loading(id: impl Into<ElementId>, label: impl Into<SharedString>, theme: &Theme) -> Div {
    let id: ElementId = id.into();
    let fade = ElementId::Name(format!("{id}-in").into());
    div().child(crate::motion::fade_in_late(
        div()
            .flex()
            .items_center()
            .gap_2()
            .text_color(theme.muted)
            .child(spinner(id, 14., theme.muted))
            .child(label.into()),
        fade,
    ))
}

pub fn input_colors(theme: &Theme) -> InputColors {
    InputColors {
        text: theme.text,
        placeholder: theme.subtle,
        cursor: theme.accent,
        selection: theme.accent.opacity(0.28),
    }
}

/// A one-line text field in the app's input style.
pub fn text_field(input: &Entity<TextInput>, theme: &Theme, cx: &App) -> Div {
    div()
        .min_h(px(32.))
        .px_3()
        .py(px(7.))
        .rounded(px(7.))
        .bg(theme.input)
        .border_1()
        .border_color(theme.border)
        .text_size(px(13.))
        .line_height(px(18.))
        .text_color(theme.text)
        .child(field(input, input_colors(theme), cx))
}

/// A labelled row of a form.
pub fn form_row(label: impl Into<SharedString>, control: impl IntoElement, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(theme.text_soft).child(label.into()))
        .child(control)
}

/// A switch, on or off; add `.on_click`.
pub fn toggle(id: impl Into<ElementId>, on: bool, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(34.))
        .h(px(20.))
        .flex_none()
        .rounded_full()
        .p(px(2.))
        .cursor_pointer()
        .bg(if on { theme.accent } else { theme.border_strong })
        .flex()
        .when(on, |d| d.justify_end())
        .child(div().size(px(16.)).rounded_full().bg(gpui::white()))
}

/// A check box with its label; add `.on_click`.
pub fn checkbox(id: impl Into<ElementId>, checked: bool, label: impl Into<SharedString>, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(
            div()
                .size(px(16.))
                .flex_none()
                .rounded(px(4.))
                .border_1()
                .border_color(if checked { theme.accent } else { theme.border_strong })
                .when(checked, |d| d.bg(theme.accent))
                .flex()
                .items_center()
                .justify_center()
                .when(checked, |d| d.child(icon(Icon::Check, px(12.), gpui::white()))),
        )
        .child(label.into())
}

/// A radio choice with its label; add `.on_click`.
pub fn radio(id: impl Into<ElementId>, selected: bool, label: impl Into<SharedString>, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(
            div()
                .size(px(16.))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(if selected { theme.accent } else { theme.border_strong })
                .flex()
                .items_center()
                .justify_center()
                .when(selected, |d| d.child(div().size(px(8.)).rounded_full().bg(theme.accent))),
        )
        .child(label.into())
}

/// A floating panel anchored under its trigger; `close` runs on a click outside it.
pub fn popover(
    corner: Corner,
    content: impl IntoElement,
    theme: &Theme,
    close: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let panel = div()
        .occlude()
        .mt_1()
        .rounded(px(10.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface)
        .shadow_lg()
        .py_1()
        .on_mouse_down_out(move |e, w, cx| {
            if !in_float(e.position) {
                close(w, cx)
            }
        })
        // Its content sits 4 pt below the panel's top (`py_1`): absolute children start from there.
        .on_children_prepainted(|bounds, _, _| {
            POPOVER_ORIGIN.with(|o| o.set(bounds.first().map(|b| b.origin - gpui::point(px(0.), px(4.)))))
        })
        .child(content);
    // Lists opened in the popover, drawn last so they cover what follows them (none on the first frame,
    // before the popover's place is known).
    let floats = POPOVER_ORIGIN.with(|o| o.get()).map(take_floats).unwrap_or_else(|| {
        FLOATS.with(|f| f.borrow_mut().clear());
        Vec::new()
    });
    let panel = panel.children(floats);
    deferred(
        anchored()
            .anchor(corner)
            .snap_to_window_with_margin(px(8.))
            .child(crate::motion::menu_in(panel, "popover-in")),
    )
    .with_priority(1)
    .into_any_element()
}

/// One line of a menu; add `.on_click`.
pub fn menu_item(id: impl Into<ElementId>, glyph: Option<Icon>, label: impl Into<SharedString>, theme: &Theme) -> Stateful<Div> {
    let id: ElementId = id.into();
    let key = format!("{id}");
    let item = div()
        .id(id)
        .min_w(px(200.))
        .h(px(30.))
        .mx_1()
        .px_2()
        .rounded(px(6.))
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(13.))
        .cursor_pointer();
    crate::motion::hover_fill(item, key, theme.input, px(6.))
        .when_some(glyph, |d, g| d.child(icon(g, px(15.), theme.muted)))
        .child(label.into())
}

/// A dialog over a dimmed window; `close` runs on Escape or a click on the dimmed part.
pub fn modal(
    id: impl Into<ElementId>,
    width: Pixels,
    content: impl IntoElement,
    theme: &Theme,
    close: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let close: std::rc::Rc<dyn Fn(&mut Window, &mut App)> = std::rc::Rc::new(close);
    let outside = close.clone();
    TOP_MODAL.with(|top| *top.borrow_mut() = Some(close.clone()));
    deferred(
        div()
            .id(id)
            .occlude()
            .absolute()
            .inset_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, move |_, w, cx| outside(w, cx))
            // The window dims while the card rises into place, as a sheet comes down.
            .child(crate::motion::fade_in(
                div().absolute().inset_0().bg(gpui::black().opacity(0.35)),
                "modal-scrim",
            ))
            .child(crate::motion::rise(
                div()
                    .id("modal-card")
                    .w(width)
                    .max_h(gpui::relative(0.86))
                    .overflow_y_scroll()
                    .rounded(px(14.))
                    .bg(theme.surface)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(content),
                "modal-card-in",
            ))
            // Lists opened in the dialog float over it, outside the card's scrolling.
            .children(take_floats(gpui::Point::default())),
    )
    .with_priority(2)
    .into_any_element()
}

thread_local! {
    /// The close action of the last modal drawn this frame: Escape runs it (see `dismiss_modal`).
    static TOP_MODAL: std::cell::RefCell<Option<std::rc::Rc<dyn Fn(&mut Window, &mut App)>>> = Default::default();
}
/// Called at the start of each frame: modals register again while drawing.
pub fn reset_modals() {
    TOP_MODAL.with(|top| top.borrow_mut().take());
    FLOATS.with(|f| f.borrow_mut().clear());
}

thread_local! {
    /// Lists opened inside a popover or a dialog, drawn by that container after its content so they float
    /// over it: GPUI cannot draw a deferred layer inside another one, nor an anchored one inside a popover.
    static FLOATS: std::cell::RefCell<Vec<(gpui::Bounds<Pixels>, AnyElement)>> = Default::default();
    /// Where each floating list's trigger and the floating lists themselves were last laid out.
    static ANCHORS: std::cell::RefCell<std::collections::HashMap<String, gpui::Bounds<Pixels>>> = Default::default();
    static FLOAT_BOUNDS: std::cell::RefCell<Vec<gpui::Bounds<Pixels>>> = Default::default();
    /// The popover's content origin at its last layout, to place floating lists in it.
    static POPOVER_ORIGIN: std::cell::Cell<Option<gpui::Point<Pixels>>> = const { std::cell::Cell::new(None) };
}

/// Where the trigger `key` was laid out last frame, for a list floating under it.
pub fn anchor_of(key: &str) -> Option<gpui::Bounds<Pixels>> {
    ANCHORS.with(|a| a.borrow().get(key).copied())
}
/// Records where the trigger `key` lies; true when it moved (the list should follow).
pub fn set_anchor(key: &str, bounds: gpui::Bounds<Pixels>) -> bool {
    ANCHORS.with(|a| a.borrow_mut().insert(key.to_owned(), bounds) != Some(bounds))
}

/// Floats `list` under `trigger` (window coordinates), over the popover or dialog being drawn.
pub fn float_under(trigger: gpui::Bounds<Pixels>, list: impl IntoElement) {
    let element = div()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_children_prepainted(|bounds, _, _| FLOAT_BOUNDS.with(|f| f.borrow_mut().extend(bounds)))
        .child(list)
        .into_any_element();
    FLOATS.with(|f| f.borrow_mut().push((trigger, element)));
}

/// The floating lists registered while drawing a container's content, placed absolutely under their
/// triggers; `origin` is where the container's absolute children are measured from, in the window.
fn take_floats(origin: gpui::Point<Pixels>) -> Vec<AnyElement> {
    FLOAT_BOUNDS.with(|f| f.borrow_mut().clear());
    FLOATS.with(|f| std::mem::take(&mut *f.borrow_mut()))
        .into_iter()
        .map(|(trigger, list)| {
            div()
                .absolute()
                .left(trigger.left() - origin.x)
                .top(trigger.bottom() + px(4.) - origin.y)
                .child(list)
                .into_any_element()
        })
        .collect()
}

/// True for a press on a floating list: its popover must not take it for a click outside.
fn in_float(point: gpui::Point<Pixels>) -> bool {
    FLOAT_BOUNDS.with(|f| f.borrow().iter().any(|b| b.contains(&point)))
}
/// Escape outside a text field: closes the topmost modal; false when none is shown.
pub fn dismiss_modal(window: &mut Window, cx: &mut App) -> bool {
    let close = TOP_MODAL.with(|top| top.borrow().clone());
    match close {
        Some(close) => {
            close(window, cx);
            true
        }
        None => false,
    }
}

/// A plain text tooltip.
pub struct Tooltip {
    text: SharedString,
    theme: Theme,
}
impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .bg(self.theme.text)
            .text_color(self.theme.surface)
            .text_size(px(12.))
            .font_family(crate::oxide::FONT)
            .child(self.text.clone())
    }
}
pub fn tooltip(text: impl Into<SharedString>, theme: Theme) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        cx.new(|_| Tooltip {
            text: text.clone(),
            theme,
        })
        .into()
    }
}
