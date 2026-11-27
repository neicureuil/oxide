//! Controls the settings pages share: the segmented picker (`SegmentedPicker` in Theme.swift), menu pickers
//! (SwiftUI's `Picker` in menu style), switches with their label, and the date-and-time stepper.
use crate::icons::{Icon, icon};
use crate::oxide::Oxide;
use crate::theme::Theme;
use crate::widgets;
use gpui::{
    AnyElement, Context, Corner, Div, FontWeight, SharedString, Window, div, prelude::*, px,
};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub type Handler = Rc<dyn Fn(&mut Oxide, &mut Window, &mut Context<Oxide>)>;

/// Wraps a click handler for `Choice` and friends.
pub fn handler(f: impl Fn(&mut Oxide, &mut Window, &mut Context<Oxide>) + 'static) -> Handler {
    Rc::new(f)
}

/// One option of a menu picker.
pub struct Choice {
    pub label: String,
    pub selected: bool,
    pub pick: Handler,
}
impl Choice {
    pub fn new(
        label: impl Into<String>,
        selected: bool,
        pick: impl Fn(&mut Oxide, &mut Window, &mut Context<Oxide>) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            selected,
            pick: Rc::new(pick),
        }
    }
}

/// Segmented control in the app palette: the chosen option on the accent tint.
pub fn segmented<T: Copy + PartialEq + 'static>(
    id: &str,
    options: Vec<(T, String)>,
    selected: T,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    pick: impl Fn(&mut Oxide, T, &mut Window, &mut Context<Oxide>) + 'static,
) -> Div {
    let pick = Rc::new(pick);
    // The chosen segment's tint slides to the new one (`withAnimation(Motion.select)`).
    let accent = theme.accent_soft;
    let (highlight, slid, measure) = crate::motion::sliding_selection(
        &format!("segmented-{id}"),
        options.iter().position(|(v, _)| *v == selected),
        3.,
        move |d| d.top(px(3.)).bottom(px(3.)).rounded(px(6.)).bg(accent),
    );
    let mut row = div()
        .relative()
        .flex()
        .flex_none()
        .gap(px(2.))
        .p(px(3.))
        .rounded(px(8.))
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border_strong)
        .on_children_prepainted(measure)
        .child(highlight.unwrap_or_else(|| div().into_any_element()));
    for (ix, (value, label)) in options.into_iter().enumerate() {
        let active = value == selected;
        let pick = pick.clone();
        row = row.child(
            div()
                .id(SharedString::from(format!("{id}-{ix}")))
                .min_h(px(26.))
                .px(px(14.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(active, |d| {
                    d.when(!slid, |d| d.bg(theme.accent_soft))
                        .text_color(theme.accent_text)
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!active, |d| d.text_color(theme.text_soft))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !active {
                        pick(this, value, window, cx);
                        cx.notify();
                    }
                })),
        );
    }
    row
}

/// A popover closed by a click outside it must not reopen when that click lands on its own trigger.
fn closed_just_now(this: &Oxide, key: &str) -> bool {
    this.ui
        .settings
        .just_closed
        .as_ref()
        .is_some_and(|(k, at)| k.as_ref() == key && at.elapsed() < Duration::from_millis(400))
}

/// Opens or closes the menu `key`, minding the click that just closed it.
pub fn toggle(this: &mut Oxide, key: &str, cx: &mut Context<Oxide>) {
    if !this.popup_open(key) && closed_just_now(this, key) {
        return;
    }
    this.toggle_popup(SharedString::from(key.to_owned()), cx);
}

/// A floating panel under a trigger of key `key`; a click outside closes it.
pub fn popover(
    this: &mut Oxide,
    key: &str,
    corner: Corner,
    content: impl IntoElement,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let entity = cx.entity().downgrade();
    let key: SharedString = key.to_owned().into();
    widgets::popover(corner, content, &this.theme, move |_, cx| {
        let key = key.clone();
        let _ = entity.update(cx, |this, cx| {
            if this.popup_open(&key) {
                this.ui.settings.just_closed = Some((key, Instant::now()));
                this.close_popup(cx);
            }
        });
    })
}

/// A menu picker: the current choice on an outlined button; the list opens under it. `nested` (inside a
/// popover or a dialog, where GPUI cannot defer another layer) floats it over that container instead.
pub fn dropdown(
    this: &mut Oxide,
    key: &str,
    width: f32,
    disabled: bool,
    nested: bool,
    choices: Vec<Choice>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let open = !disabled
        && if nested {
            this.ui.settings.inline_open.as_deref() == Some(key)
        } else {
            this.popup_open(key)
        };
    let current = choices
        .iter()
        .find(|c| c.selected)
        .map(|c| c.label.clone())
        .unwrap_or_default();
    let trigger_key = key.to_owned();
    let hover = theme.input;
    let trigger = div()
        .id(SharedString::from(format!("{key}-trigger")))
        .w(px(width))
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .rounded(px(7.))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.surface)
        .text_size(px(13.))
        .text_color(theme.text)
        .when(disabled, |d| d.opacity(0.45))
        .when(!disabled, move |d| {
            d.cursor_pointer().hover(move |s| s.bg(hover))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(current),
        )
        .child(icon(Icon::ChevronDown, px(12.), theme.muted))
        .on_click(cx.listener(move |this, _, _, cx| {
            if disabled {
                return;
            }
            if nested {
                let inline = &mut this.ui.settings.inline_open;
                *inline = if inline.as_deref() == Some(trigger_key.as_str()) {
                    None
                } else {
                    Some(trigger_key.clone())
                };
                cx.notify();
            } else {
                toggle(this, &trigger_key, cx);
            }
        }));
    let anchor_key = key.to_owned();
    let mut container = div()
        .relative()
        .flex_none()
        // Where the trigger is, for its list floating under it.
        .on_children_prepainted(move |bounds, window, _| {
            if let Some(b) = bounds.first()
                && widgets::set_anchor(&anchor_key, *b)
            {
                window.refresh();
            }
        })
        .child(trigger);
    if open {
        let mut menu = div().flex().flex_col().min_w(px(width));
        for (ix, choice) in choices.into_iter().enumerate() {
            let pick = choice.pick.clone();
            menu = menu.child(
                widgets::menu_item(
                    SharedString::from(format!("{key}-choice-{ix}")),
                    None,
                    choice.label,
                    &theme,
                )
                .child(div().flex_1())
                .when(choice.selected, |d| {
                    d.child(icon(Icon::Check, px(12.), theme.accent))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if nested {
                        this.ui.settings.inline_open = None;
                        cx.notify();
                    } else {
                        this.close_popup(cx);
                    }
                    pick(this, window, cx);
                })),
            );
        }
        if nested {
            // Floats over the popover or dialog it sits in, under its trigger, instead of pushing what
            // follows down (the container draws it last; see `widgets::float_under`).
            if let Some(trigger) = widgets::anchor_of(key) {
                let close_key = key.to_owned();
                widgets::float_under(
                    trigger,
                    div()
                        .py_1()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface)
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                            // A press on the trigger toggles it closed by itself.
                            let on_trigger = widgets::anchor_of(&close_key).is_some_and(|b| b.contains(&e.position));
                            if !on_trigger && this.ui.settings.inline_open.as_deref() == Some(close_key.as_str()) {
                                this.ui.settings.inline_open = None;
                                cx.notify();
                            }
                        }))
                        .child(crate::motion::menu_in(menu, SharedString::from(format!("{key}-float-in")))),
                );
            }
        } else {
            let pop = popover(this, key, Corner::TopRight, menu, cx);
            container = container.child(div().absolute().top_full().right_0().child(pop));
        }
    }
    container.into_any_element()
}

/// A switch with its label on the left (SwiftUI's `Toggle` in switch style).
pub fn switch_row(
    id: &str,
    title: impl Into<SharedString>,
    detail: Option<String>,
    on: bool,
    disabled: bool,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    flip: impl Fn(&mut Oxide, bool, &mut Context<Oxide>) + 'static,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .when(disabled, |d| d.opacity(0.45))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(13.)).child(title.into()))
                .when_some(detail, |d, text| d.child(widgets::muted(text, 11., theme))),
        )
        .child(
            widgets::toggle(SharedString::from(id.to_owned()), on, theme).on_click(cx.listener(
                move |this, _, _, cx| {
                    if !disabled {
                        flip(this, !on, cx);
                        cx.notify();
                    }
                },
            )),
        )
}

/// A row title on the left, a control on the right.
pub fn labelled(title: impl Into<SharedString>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .child(title.into()),
        )
        .child(control)
}

/// A settings card with the macOS app's 18-point padding.
pub fn card(theme: &Theme) -> Div {
    widgets::card(theme).p(px(18.)).flex().flex_col()
}

/// "Section" heading above a card.
pub fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A card's own title.
pub fn card_title(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A small error line with a close button (the status card's and out of office's).
pub fn error_line(
    id: &str,
    text: String,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    close: impl Fn(&mut Oxide) + 'static,
) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .flex_1()
                .text_size(px(12.))
                .text_color(theme.error_text)
                .child(text),
        )
        .child(
            widgets::icon_button_sized(
                SharedString::from(id.to_owned()),
                Icon::Close,
                20.,
                10.,
                false,
                theme,
            )
            .tooltip(widgets::tooltip(
                crate::oxide::tr("ui.status_menu.close"),
                *theme,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                close(this);
                cx.notify();
            })),
        )
}

/// A date and time picked with steppers: day back or forward, half hour back or forward (the 30-minute
/// grid the web client offers). `set` gets the new epoch milliseconds.
pub fn date_time_stepper(
    id: &str,
    epoch_ms: i64,
    min_ms: Option<i64>,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    set: impl Fn(&mut Oxide, i64) + 'static,
) -> Div {
    use crate::settings_time::{local, long_date, short_time};
    let set = Rc::new(set);
    let l = local(epoch_ms.div_euclid(1000));
    let date = format!("{} {}", long_date(l), l.year);
    let step = |suffix: &str, glyph: Icon, delta_ms: i64, cx: &mut Context<Oxide>| {
        let set = set.clone();
        widgets::icon_button_sized(
            SharedString::from(format!("{id}-{suffix}")),
            glyph,
            24.,
            12.,
            false,
            theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            let step = 30 * 60 * 1000;
            // Snapped to the grid, as the macOS app's date binding does.
            let next = (epoch_ms + delta_ms + step / 2).div_euclid(step) * step;
            if min_ms.is_none_or(|min| next >= min) {
                set(this, next);
                cx.notify();
            }
        }))
    };
    let field = |text: String| {
        div()
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(7.))
            .border_1()
            .border_color(theme.border_strong)
            .text_size(px(13.))
            .child(text)
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(step("day-back", Icon::ChevronLeft, -86_400_000, cx))
        .child(field(date))
        .child(step("day-forward", Icon::ChevronRight, 86_400_000, cx))
        .child(div().w_2())
        .child(step("time-back", Icon::ChevronLeft, -30 * 60 * 1000, cx))
        .child(field(short_time(l)))
        .child(step("time-forward", Icon::ChevronRight, 30 * 60 * 1000, cx))
}
