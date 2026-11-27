//! The GIF and sticker picker (`FunPicker.swift`), as the web client's "fun stuff" panel: Giphy searched through
//! the core, Teams stickers by category or name; a pick is sent at once.
use crate::composer_mention::fold;
use crate::composer_thumbs;
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput, field};
use crate::oxide::{Oxide, tr};
use crate::widgets;
use gpui::{
    AnyElement, Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px,
    uniform_list,
};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;
use teams_core::{Action, GifsAction};

const GIF_COLUMNS: usize = 3;
const STICKER_COLUMNS: usize = 4;
/// Three tiles across the 356 points inside the panel.
const GIF_WIDTH: f32 = 114.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Gifs,
    Stickers,
}

pub struct Picker {
    tab: Tab,
    query: Entity<TextInput>,
    category: Option<String>,
    /// Typing restarts the pause before a Giphy search.
    pending: u64,
    _subscription: Subscription,
}

impl Picker {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let query = cx.new(|cx| TextInput::new(cx, tr("ui.fun_picker.search_giphy")));
        let subscription = cx.subscribe_in(
            &query,
            window,
            |this: &mut Oxide, _, event: &InputEvent, _, cx| match event {
                InputEvent::Changed => changed(this, cx),
                InputEvent::Submit => search_now(this, cx),
                InputEvent::Cancel => {
                    let query = this.ui.composer.fun.query.clone();
                    query.update(cx, |q, cx| q.set_text("", None, cx));
                    changed(this, cx);
                }
                _ => {}
            },
        );
        Self {
            tab: Tab::Gifs,
            query,
            category: None,
            pending: 0,
            _subscription: subscription,
        }
    }
}

/// The panel opened: popular GIFs if none are shown yet.
pub fn opened(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if this.gifs.as_ref().is_none_or(|g| g.results.is_empty()) {
        this.send(Action::Gifs {
            action: GifsAction::Search {
                query: String::new(),
            },
        });
    }
    let _ = cx;
}

fn changed(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let picker = &mut this.ui.composer.fun;
    if picker.tab == Tab::Stickers {
        return cx.notify();
    }
    picker.pending += 1;
    let pending = picker.pending;
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(400))
            .await;
        let _ = this.update(cx, |this, cx| {
            if this.ui.composer.fun.pending == pending {
                search_now(this, cx);
            }
        });
    })
    .detach();
}

fn search_now(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let picker = &mut this.ui.composer.fun;
    picker.pending += 1;
    if picker.tab == Tab::Gifs {
        let query = picker.query.read(cx).text().to_owned();
        this.send(Action::Gifs {
            action: GifsAction::Search { query },
        });
    }
    cx.notify();
}

fn set_tab(this: &mut Oxide, tab: Tab, cx: &mut Context<Oxide>) {
    let picker = &mut this.ui.composer.fun;
    if picker.tab == tab {
        return;
    }
    picker.tab = tab;
    picker.pending += 1;
    let placeholder = if tab == Tab::Gifs {
        tr("ui.fun_picker.search_giphy")
    } else {
        tr("ui.fun_picker.search_stickers")
    };
    picker.query.update(cx, |q, cx| {
        q.set_text("", None, cx);
        q.set_placeholder(placeholder);
    });
    cx.notify();
}

pub fn show_stickers(this: &mut Oxide, cx: &mut Context<Oxide>) {
    set_tab(this, Tab::Stickers, cx);
}

fn segment(
    id: &'static str,
    label: String,
    selected: bool,
    theme: &crate::theme::Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded(px(5.))
        .text_size(px(12.))
        .cursor_pointer()
        .when(selected, |d| {
            d.bg(theme.surface)
                .shadow_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
        })
        .when(!selected, |d| d.text_color(theme.muted))
        .child(label)
}

pub fn picker(this: &mut Oxide, _: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let tab = this.ui.composer.fun.tab;
    let query = this.ui.composer.fun.query.clone();
    let sending = this.gifs.as_ref().is_some_and(|g| g.sending);
    let header = div()
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .p(px(2.))
                .gap(px(2.))
                .rounded(px(7.))
                .bg(theme.input)
                .child(
                    segment(
                        "fun-gifs",
                        tr("ui.fun_picker.gif"),
                        tab == Tab::Gifs,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| set_tab(this, Tab::Gifs, cx))),
                )
                .child(
                    segment(
                        "fun-stickers",
                        tr("ui.fun_picker.stickers"),
                        tab == Tab::Stickers,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| set_tab(this, Tab::Stickers, cx))),
                ),
        )
        .child(div().flex_1())
        .when(sending, |d| {
            d.child(widgets::spinner("fun-sending", 14., theme.muted))
        });
    let has_query = !query.read(cx).is_empty();
    let search = div()
        .h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(7.))
        .bg(theme.input)
        .border_1()
        .border_color(theme.border)
        .child(icon(Icon::Search, px(14.), theme.subtle))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .line_height(px(18.))
                .child(field(&query, widgets::input_colors(&theme), cx)),
        )
        .when(has_query, |d| {
            d.child(
                widgets::icon_button_sized("fun-clear", Icon::Close, 22., 10., false, &theme)
                    .tooltip(widgets::tooltip(tr("ui.files_screens.clear_search"), theme))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let query = this.ui.composer.fun.query.clone();
                        query.update(cx, |q, cx| q.set_text("", None, cx));
                        changed(this, cx);
                    })),
            )
        });
    let body = match tab {
        Tab::Gifs => gifs(this, cx),
        Tab::Stickers => stickers(this, cx),
    };
    div()
        .w(px(380.))
        .h(px(420.))
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(header)
        .child(search)
        .child(body)
        .into_any_element()
}

fn gifs(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let view = this.gifs.clone().unwrap_or_default();
    let rows: Rc<Vec<Vec<teams_core::GifView>>> = Rc::new(
        view.results
            .chunks(GIF_COLUMNS)
            .map(|c| c.to_vec())
            .collect(),
    );
    let count = rows.len();
    let list = uniform_list(
        "gif-grid",
        count,
        cx.processor(move |this: &mut Oxide, range: Range<usize>, _, cx| {
            range
                .map(|ix| {
                    let mut line = div().h(px(94.)).flex().gap(px(6.));
                    for (n, gif) in rows[ix].iter().enumerate() {
                        let picture = composer_thumbs::view(
                            this,
                            &gif.preview,
                            ("gif-loading", ix * GIF_COLUMNS + n),
                            cx,
                        );
                        let url = gif.url.clone();
                        let title = if gif.title.is_empty() {
                            tr("ui.fun_picker.gif")
                        } else {
                            gif.title.clone()
                        };
                        line = line.child(
                            div()
                                .id(("gif", ix * GIF_COLUMNS + n))
                                .w(px(GIF_WIDTH))
                                .flex_none()
                                .h(px(88.))
                                .cursor_pointer()
                                .tooltip(widgets::tooltip(title, this.theme))
                                .child(picture)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.send(Action::Gifs {
                                        action: GifsAction::SendGif { url: url.clone() },
                                    });
                                    this.close_popup(cx);
                                    this.ui.composer.input.read(cx).focus(window);
                                })),
                        );
                    }
                    line.into_any_element()
                })
                .collect::<Vec<_>>()
        }),
    )
    .flex_1()
    .w_full();
    let note = |text: String| {
        div()
            .p_2()
            .text_size(px(12.))
            .text_color(theme.muted)
            .child(text)
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(list)
        .when(view.loading, |d| {
            d.child(div().p_2().flex().justify_center().child(widgets::spinner(
                "gifs-loading",
                16.,
                theme.muted,
            )))
        })
        .when_some(view.error.clone(), |d, e| d.child(note(e)))
        .when(
            !view.loading && view.results.is_empty() && view.error.is_none(),
            |d| d.child(note(tr("ui.fun_picker.no_gifs"))),
        )
        .child(
            div()
                .pt_1()
                .text_size(px(10.))
                .text_color(theme.muted)
                .child(tr("ui.fun_picker.powered_by_giphy")),
        )
        .into_any_element()
}

fn stickers(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let query = this.ui.composer.fun.query.read(cx).text().trim().to_owned();
    let chosen = this
        .ui
        .composer
        .fun
        .category
        .clone()
        .or_else(|| this.stickers.first().map(|c| c.id.clone()));
    let filtered: Vec<teams_core::StickerView> = if query.is_empty() {
        this.stickers
            .iter()
            .find(|c| Some(&c.id) == chosen.as_ref())
            .map(|c| c.stickers.clone())
            .unwrap_or_default()
    } else {
        let q = fold(&query);
        this.stickers
            .iter()
            .flat_map(|c| &c.stickers)
            .filter(|s| fold(&s.name).contains(&q) || fold(&s.keywords).contains(&q))
            .cloned()
            .collect()
    };
    let mut column = div().flex_1().min_h_0().flex().flex_col().gap(px(8.));
    if this.stickers.is_empty() {
        column = column.child(
            widgets::loading(
                "stickers-loading",
                tr("ui.fun_picker.loading_stickers"),
                &theme,
            )
            .text_size(px(12.)),
        );
    }
    if query.is_empty() && !this.stickers.is_empty() {
        let mut chips = div()
            .id("sticker-categories")
            .flex()
            .gap_1()
            .overflow_x_scroll();
        for (ix, category) in this.stickers.iter().enumerate() {
            let id = category.id.clone();
            chips = chips.child(
                widgets::chip(
                    ("sticker-category", ix),
                    category.id.clone(),
                    Some(&category.id) == chosen.as_ref(),
                    &theme,
                )
                .flex_none()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ui.composer.fun.category = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        column = column.child(chips);
    }
    let rows: Rc<Vec<Vec<teams_core::StickerView>>> = Rc::new(
        filtered
            .chunks(STICKER_COLUMNS)
            .map(|c| c.to_vec())
            .collect(),
    );
    let count = rows.len();
    let list = uniform_list(
        "sticker-grid",
        count,
        cx.processor(move |this: &mut Oxide, range: Range<usize>, _, cx| {
            range
                .map(|ix| {
                    let mut line = div().h(px(86.)).flex().gap(px(6.));
                    for (n, sticker) in rows[ix].iter().enumerate() {
                        let picture = composer_thumbs::view(
                            this,
                            &sticker.url,
                            ("sticker-loading", ix * STICKER_COLUMNS + n),
                            cx,
                        );
                        let id = sticker.id.clone();
                        line = line.child(
                            div()
                                .id(("sticker", ix * STICKER_COLUMNS + n))
                                .size(px(80.))
                                .flex_none()
                                .cursor_pointer()
                                .tooltip(widgets::tooltip(sticker.name.clone(), this.theme))
                                .child(picture)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.send(Action::Gifs {
                                        action: GifsAction::SendSticker { id: id.clone() },
                                    });
                                    this.close_popup(cx);
                                    this.ui.composer.input.read(cx).focus(window);
                                })),
                        );
                    }
                    line.into_any_element()
                })
                .collect::<Vec<_>>()
        }),
    )
    .flex_1()
    .w_full();
    column.child(list).into_any_element()
}
