//! The top bar's search field (`GlobalSearchField` in `MainView.swift`) and the full results screen
//! (`SearchResults.swift`): messages found on the server, with the web client's filters.
use crate::chats_parts::{SyncedList, matches, menu_separator, sig, terms};
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput, field};
use crate::oxide::{Oxide, tr, trf};
use crate::theme::Theme;
use crate::widgets;
use gpui::{
    AnyElement, Context, Corner, Entity, FontWeight, HighlightStyle, SharedString, StyledText,
    Task, Window, div, list, prelude::*, px,
};
use std::time::Duration;
use teams_core::{
    Action, AvatarShape, SearchAction, SearchDate, SearchFiltersView, SearchHitView, SearchKind,
    SearchView,
};

pub struct Ui {
    /// The top bar's field.
    query: Entity<TextInput>,
    /// The core's query when last seen: it changes on its own only when cleared.
    current: Option<String>,
    /// Typing pauses 150 ms before the chat list is filtered.
    debounce: Option<Task<()>>,
    /// "Person" field of the "From" filter, searched 300 ms after typing stops.
    person: Entity<TextInput>,
    person_debounce: Option<Task<()>>,
    /// Custom date range, as `YYYY-MM-DD`.
    start: Entity<TextInput>,
    end: Entity<TextInput>,
    list: SyncedList,
    items: Vec<Item>,
    /// Words to highlight, worked out once per render for all hits.
    terms: Vec<String>,
}

#[derive(Clone, Copy, Hash)]
enum Item {
    Error,
    Hit(usize),
    Status,
    More,
}

const FROM: &str = "search-from";
const DATE: &str = "search-date";
const KIND: &str = "search-kind";

impl Ui {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let query =
            cx.new(|cx| TextInput::new(cx, tr("ui.main_view.search_chats_channels_and_messages")));
        cx.subscribe_in(
            &query,
            window,
            |this: &mut Oxide, input, event: &InputEvent, window, cx| {
                let text = input.read(cx).text().to_owned();
                match event {
                    InputEvent::Changed => {
                        let empty = text.is_empty();
                        this.ui.search.debounce = Some(cx.spawn(async move |this, cx| {
                            if !empty {
                                cx.background_executor()
                                    .timer(Duration::from_millis(150))
                                    .await;
                            }
                            let _ = this.update(cx, |this, _| send_query(this, text));
                        }));
                    }
                    // Return searches every message on the server, as Teams does.
                    InputEvent::Submit => {
                        this.ui.search.debounce = None;
                        send_query(this, text.clone());
                        let filters = this
                            .search
                            .as_ref()
                            .map(|s| s.filters.clone())
                            .unwrap_or_default();
                        this.send(Action::FullSearch {
                            action: SearchAction::Run {
                                query: text,
                                filters,
                            },
                        });
                    }
                    // Escape clears the field, then leaves it.
                    InputEvent::Cancel => {
                        if text.is_empty() {
                            window.blur();
                        } else {
                            clear(this, cx);
                        }
                    }
                    _ => {}
                }
            },
        )
        .detach();

        let person = cx.new(|cx| TextInput::new(cx, tr("ui.search.person")));
        cx.subscribe(
            &person,
            |this: &mut Oxide, input, event: &InputEvent, cx| {
                if *event == InputEvent::Changed {
                    let text = input.read(cx).text().to_owned();
                    this.ui.search.person_debounce = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(300))
                            .await;
                        let _ = this.update(cx, |this, _| {
                            this.send(Action::FullSearch {
                                action: SearchAction::FindPeople { query: text },
                            })
                        });
                    }));
                }
            },
        )
        .detach();

        let start = cx.new(|cx| TextInput::new(cx, "AAAA-MM-JJ"));
        let end = cx.new(|cx| TextInput::new(cx, "AAAA-MM-JJ"));
        for (input, is_start) in [(&start, true), (&end, false)] {
            cx.subscribe(
                input,
                move |this: &mut Oxide, input, event: &InputEvent, cx| {
                    if *event == InputEvent::Submit {
                        let text = input.read(cx).text().trim().to_owned();
                        set_day(this, is_start, text);
                    }
                },
            )
            .detach();
        }

        Self {
            query,
            current: None,
            debounce: None,
            person,
            person_debounce: None,
            start,
            end,
            list: SyncedList::new(),
            items: Vec::new(),
            terms: Vec::new(),
        }
    }
}

/// Filters the chat list, once the typed text differs from the core's.
fn send_query(this: &mut Oxide, query: String) {
    if this.shell.as_ref().is_none_or(|s| s.search != query) {
        this.send(Action::Search { query });
    }
}

fn clear(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.search.debounce = None;
    this.ui
        .search
        .query
        .update(cx, |i, cx| i.set_text("", None, cx));
    send_query(this, String::new());
}

/// The top bar's search: filters the chat list once typing pauses (150 ms); Return searches all messages.
pub fn top_field(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let current = this
        .shell
        .as_ref()
        .map(|s| s.search.clone())
        .unwrap_or_default();
    let ui = &mut this.ui.search;
    if ui.current.as_ref() != Some(&current) {
        ui.current = Some(current.clone());
        if !ui.query.read(cx).is_focused(window) && ui.query.read(cx).text() != current {
            ui.query.update(cx, |i, cx| i.set_text(current, None, cx));
        }
    }
    let empty = ui.query.read(cx).is_empty();
    div()
        .id("search-field")
        .w_full()
        .min_h(px(34.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(8.))
        .bg(theme.input)
        .text_size(px(13.))
        .line_height(px(18.))
        .child(icon(Icon::Search, px(15.), theme.muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h(px(18.))
                .overflow_hidden()
                .child(field(
                    &this.ui.search.query,
                    widgets::input_colors(&theme),
                    cx,
                )),
        )
        .when(!empty, |d| {
            d.child(
                div()
                    .id("search-clear")
                    .child(widgets::icon_button_sized(
                        "search-clear-button",
                        Icon::Close,
                        22.,
                        12.,
                        false,
                        &theme,
                    ))
                    .tooltip(widgets::tooltip(tr("ui.main_view.clear_search"), theme))
                    .on_click(cx.listener(|this, _, window, cx| {
                        clear(this, cx);
                        this.ui.search.query.read(cx).focus(window);
                    })),
            )
        })
        .child(
            div()
                .flex_none()
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .bg(theme.surface)
                .text_size(px(11.))
                .line_height(px(14.))
                .text_color(theme.muted)
                .child("⌘E"),
        )
        .tooltip(widgets::tooltip(tr("ui.search.press_return"), theme))
        .into_any_element()
}

/// ⌘E: puts the cursor in the top bar search field.
pub fn focus(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    let input = this.ui.search.query.clone();
    input.update(cx, |i, cx| {
        let len = i.text().len();
        i.select(0..len, cx);
    });
    input.read(cx).focus(window);
}

fn run(this: &mut Oxide, change: impl FnOnce(&mut SearchFiltersView)) {
    let Some(view) = this.search.as_ref() else {
        return;
    };
    let mut filters = view.filters.clone();
    change(&mut filters);
    let query = view.query.clone();
    this.send(Action::FullSearch {
        action: SearchAction::Run { query, filters },
    });
}

/// A typed custom day: searches again once it reads as a date, filling the other end if it is empty.
fn set_day(this: &mut Oxide, start: bool, text: String) {
    let valid = text.len() == 10
        && text.char_indices().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !valid {
        return;
    }
    run(this, |f| {
        if start {
            f.start = text.clone();
        } else {
            f.end = text.clone();
        }
        if f.start.is_empty() {
            f.start = text.clone();
        }
        if f.end.is_empty() {
            f.end = text;
        }
    });
}

pub fn screen(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let Some(view) = this.search.clone() else {
        return div().into_any_element();
    };
    let theme = this.theme;
    div()
        .id("search-screen")
        .size_full()
        .flex()
        .flex_col()
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" && this.popup.is_none() {
                this.send(Action::FullSearch {
                    action: SearchAction::Close,
                });
                cx.stop_propagation();
            }
        }))
        .child(header(this, &view, cx))
        .child(widgets::divider(&theme))
        .child(filters(this, &view, window, cx))
        .child(widgets::divider(&theme))
        .child(results(this, &view, cx))
        .into_any_element()
}

fn header(this: &mut Oxide, view: &SearchView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    div()
        .flex()
        .items_center()
        .px(px(20.))
        .py(px(12.))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(widgets::title(tr("ui.search.title"), 20.))
                .when(view.searched, |d| {
                    d.child(widgets::muted(
                        trf(
                            "ui.search.results_count",
                            &[("count", &view.total.to_string())],
                        ),
                        12.,
                        &theme,
                    ))
                }),
        )
        .child(
            widgets::icon_button_sized("search-close", Icon::Close, 34., 16., false, &theme)
                .tooltip(widgets::tooltip(tr("ui.search.close"), theme))
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(Action::FullSearch {
                        action: SearchAction::Close,
                    })
                })),
        )
        .into_any_element()
}

fn date_label(date: SearchDate) -> String {
    tr(match date {
        SearchDate::Any => "ui.search.any_time",
        SearchDate::Today => "ui.search.today",
        SearchDate::Yesterday => "ui.search.yesterday",
        SearchDate::ThisWeek => "ui.search.this_week",
        SearchDate::LastWeek => "ui.search.last_week",
        SearchDate::ThisMonth => "ui.search.this_month",
        SearchDate::LastMonth => "ui.search.last_month",
        SearchDate::ThisYear => "ui.search.this_year",
        SearchDate::LastYear => "ui.search.last_year",
        SearchDate::Custom => "ui.search.custom_range",
    })
}

fn kind_label(kind: SearchKind) -> String {
    tr(match kind {
        SearchKind::Everywhere => "ui.search.everywhere",
        SearchKind::Chats => "ui.search.chats",
        SearchKind::Channels => "ui.search.channels",
    })
}

/// A pop-up button, as AppKit draws a `Picker` or `Menu`: its value and a chevron, a menu under it.
fn menu_button(
    this: &mut Oxide,
    key: &'static str,
    glyph: Option<Icon>,
    label: String,
    menu: Vec<AnyElement>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let open = this.popup_open(key);
    let hover = theme.input;
    let entity = cx.entity().downgrade();
    div()
        .flex_none()
        .child(
            div()
                .id(key)
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border_strong)
                .bg(theme.surface)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .when_some(glyph, |d, g| d.child(icon(g, px(13.), theme.muted)))
                .child(label)
                .child(icon(Icon::ChevronDown, px(10.), theme.muted))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_popup(key, cx))),
        )
        .when(open, |d| {
            d.child(widgets::popover(
                Corner::TopLeft,
                div()
                    .id(SharedString::from(format!("{key}-menu")))
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .children(menu),
                &theme,
                move |_, cx| {
                    let _ = entity.update(cx, |this, cx| this.close_popup(cx));
                },
            ))
        })
        .into_any_element()
}

fn filters(
    this: &mut Oxide,
    view: &SearchView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let f = view.filters.clone();

    let mut from_menu = vec![
        widgets::menu_item("from-anyone", None, tr("ui.search.anyone"), &theme)
            .on_click(cx.listener(|this, _, _, cx| {
                run(this, |f| f.from = None);
                this.close_popup(cx);
            }))
            .into_any_element(),
        menu_separator(&theme).into_any_element(),
    ];
    for person in &view.people {
        let label = if person.email.is_empty() {
            person.name.clone()
        } else {
            format!("{} · {}", person.name, person.email)
        };
        let chosen = person.clone();
        from_menu.push(
            widgets::menu_item(
                SharedString::from(format!("from-{}", person.mri)),
                None,
                label,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let chosen = chosen.clone();
                run(this, |f| f.from = Some(chosen));
                this.close_popup(cx);
            }))
            .into_any_element(),
        );
    }
    let from_label = match &f.from {
        Some(p) => format!("{} : {}", tr("ui.search.from"), p.name),
        None => tr("ui.search.from"),
    };
    let from = menu_button(this, FROM, Some(Icon::Person), from_label, from_menu, cx);

    let dates = [
        SearchDate::Any,
        SearchDate::Today,
        SearchDate::Yesterday,
        SearchDate::ThisWeek,
        SearchDate::LastWeek,
        SearchDate::ThisMonth,
        SearchDate::LastMonth,
        SearchDate::ThisYear,
        SearchDate::LastYear,
        SearchDate::Custom,
    ];
    let date_menu = dates
        .into_iter()
        .map(|date| {
            choice(
                SharedString::from(format!("date-{date:?}")),
                date_label(date),
                f.date == date,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                run(this, |f| f.date = date);
                this.close_popup(cx);
            }))
            .into_any_element()
        })
        .collect();
    let date = menu_button(this, DATE, None, date_label(f.date), date_menu, cx);

    let kind_menu = [
        SearchKind::Everywhere,
        SearchKind::Chats,
        SearchKind::Channels,
    ]
    .into_iter()
    .map(|kind| {
        choice(
            SharedString::from(format!("kind-{kind:?}")),
            kind_label(kind),
            f.kind == kind,
            &theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            run(this, |f| f.kind = kind);
            this.close_popup(cx);
        }))
        .into_any_element()
    })
    .collect();
    let kind = menu_button(this, KIND, None, kind_label(f.kind), kind_menu, cx);

    let ui = &this.ui.search;
    let small_field = |input: &Entity<TextInput>, width: f32, cx: &mut Context<Oxide>| {
        div()
            .w(px(width))
            .flex_none()
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(theme.input)
            .border_1()
            .border_color(theme.border)
            .child(div().w_full().h(px(16.)).overflow_hidden().child(field(
                input,
                widgets::input_colors(&theme),
                cx,
            )))
    };
    let person = small_field(&ui.person.clone(), 150., cx);
    if f.date == SearchDate::Custom {
        for (input, value) in [
            (ui.start.clone(), f.start.clone()),
            (ui.end.clone(), f.end.clone()),
        ] {
            if !input.read(cx).is_focused(window) && input.read(cx).text() != value {
                input.update(cx, |i, cx| i.set_text(value, None, cx));
            }
        }
    }
    let ui = &this.ui.search;
    let (start, end) = (ui.start.clone(), ui.end.clone());

    let check = |id: &'static str,
                 on: bool,
                 label: String,
                 set: fn(&mut SearchFiltersView, bool),
                 cx: &mut Context<Oxide>| {
        widgets::checkbox(id, on, label, &theme)
            .flex_none()
            .on_click(cx.listener(move |this, _, _, _| run(this, |f| set(f, !on))))
            .into_any_element()
    };
    let attachment = check(
        "filter-attachment",
        f.has_attachment,
        tr("ui.search.has_attachment"),
        |f, v| f.has_attachment = v,
        cx,
    );
    let mentions = check(
        "filter-mentions",
        f.mentions_me,
        tr("ui.search.mentions_me"),
        |f, v| f.mentions_me = v,
        cx,
    );
    let bots = check(
        "filter-bots",
        f.hide_bots,
        tr("ui.search.hide_bots"),
        |f, v| f.hide_bots = v,
        cx,
    );
    let here = view.here_name.as_ref().map(|name| {
        check(
            "filter-here",
            f.here,
            trf("ui.search.only_in", &[("name", name)]),
            |f, v| f.here = v,
            cx,
        )
    });

    let labelled = |label: String, control: AnyElement| {
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1p5()
            .child(label)
            .child(control)
            .into_any_element()
    };
    div()
        .id("search-filters")
        .flex_none()
        .overflow_x_scroll()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(20.))
        .py(px(10.))
        .text_size(px(12.))
        .line_height(px(16.))
        .child(from)
        .child(person)
        .child(labelled(tr("ui.search.date"), date))
        .when(f.date == SearchDate::Custom, |d| {
            d.child(small_field(&start, 104., cx))
                .child("–")
                .child(small_field(&end, 104., cx))
        })
        .child(labelled(tr("ui.search.type"), kind))
        .child(attachment)
        .child(mentions)
        .child(bots)
        .children(here)
        .into_any_element()
}

/// A menu line with a check mark on the chosen value.
fn choice(
    id: SharedString,
    label: String,
    chosen: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    widgets::menu_item(id, None, label, theme)
        .child(div().flex_1())
        .when(chosen, |d| {
            d.child(icon(Icon::Check, px(13.), theme.accent_text))
        })
}

fn results(this: &mut Oxide, view: &SearchView, cx: &mut Context<Oxide>) -> AnyElement {
    let mut items = Vec::with_capacity(view.hits.len() + 3);
    if view.error.is_some() {
        items.push(Item::Error);
    }
    items.extend((0..view.hits.len()).map(Item::Hit));
    let status = view.loading || !view.searched || (view.hits.is_empty() && view.error.is_none());
    if status {
        items.push(Item::Status);
    }
    if view.has_more && !view.loading {
        items.push(Item::More);
    }
    let words = terms(
        view.terms
            .iter()
            .map(String::as_str)
            .chain(view.query.split(' ')),
    );
    let words_sig = sig(&words);
    let sigs = items
        .iter()
        .map(|item| match item {
            Item::Hit(i) => sig(&(&view.hits[*i], words_sig)),
            Item::Error => sig(&view.error),
            Item::Status => sig(&(view.loading, view.searched)),
            Item::More => sig(item),
        })
        .collect();
    this.ui.search.items = items;
    this.ui.search.terms = words;
    this.ui.search.list.sync(sigs);
    let entity = cx.entity();
    list(this.ui.search.list.state.clone(), move |ix, _, cx| {
        entity.update(cx, |this, cx| item(this, ix, cx))
    })
    .flex_1()
    .pt(px(20.))
    .pb(px(12.))
    .into_any_element()
}

fn item(this: &mut Oxide, ix: usize, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(view) = this.search.as_ref() else {
        return div().into_any_element();
    };
    let body = match this.ui.search.items.get(ix).copied() {
        Some(Item::Error) => div()
            .text_size(px(12.))
            .text_color(theme.error_text)
            .child(view.error.clone().unwrap_or_default())
            .into_any_element(),
        Some(Item::Hit(i)) => {
            let Some(hit) = view.hits.get(i).cloned() else {
                return div().into_any_element();
            };
            hit_card(this, &hit, cx)
        }
        Some(Item::Status) => {
            let note = if view.loading {
                widgets::loading("search-loading", tr("ui.search.searching"), &theme)
                    .text_size(px(12.))
                    .into_any_element()
            } else if view.searched {
                widgets::muted(tr("ui.search.no_results"), 13., &theme).into_any_element()
            } else {
                widgets::muted(tr("ui.search.type_or_filter"), 13., &theme).into_any_element()
            };
            div().p_4().child(note).into_any_element()
        }
        Some(Item::More) => {
            let hover = theme.input;
            div()
                .py(px(6.))
                .flex()
                .child(
                    div()
                        .id("search-more")
                        .px_2()
                        .py_1()
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_color(theme.accent_text)
                        .hover(move |s| s.bg(hover))
                        .child(tr("ui.search.more_results"))
                        .on_click(cx.listener(|this, _, _, _| {
                            this.send(Action::FullSearch {
                                action: SearchAction::More,
                            })
                        })),
                )
                .into_any_element()
        }
        None => return div().into_any_element(),
    };
    div()
        .px(px(20.))
        .pb(px(8.))
        .child(div().max_w(px(780.)).child(body))
        .into_any_element()
}

fn hit_card(this: &mut Oxide, hit: &SearchHitView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let avatar = this.avatar(&hit.sender_id, &hit.sender, 32., AvatarShape::Round, None);
    let highlight = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        color: Some(theme.accent_text),
        ..Default::default()
    };
    let ranges = matches(&hit.preview, &this.ui.search.terms);
    let preview = StyledText::new(hit.preview.clone())
        .with_highlights(ranges.into_iter().map(|r| (r, highlight)));
    let id = hit.message_id.clone();
    widgets::card(&theme)
        .id(SharedString::from(format!("hit-{}", hit.message_id)))
        .p(px(12.))
        .flex()
        .items_start()
        .gap(px(12.))
        .cursor_pointer()
        .child(avatar)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .line_clamp(1)
                                .text_ellipsis()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(hit.sender.clone()),
                        )
                        .child(widgets::muted(hit.time.clone(), 11., &theme)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .line_clamp(3)
                        .text_ellipsis()
                        .child(preview),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(11.))
                        .text_color(theme.muted)
                        .child(icon(
                            if hit.is_channel {
                                Icon::Hash
                            } else {
                                Icon::Chat
                            },
                            px(12.),
                            theme.muted,
                        ))
                        .child(hit.place.clone()),
                ),
        )
        .on_click(cx.listener(move |this, _, _, _| {
            this.send(Action::FullSearch {
                action: SearchAction::OpenHit {
                    message_id: id.clone(),
                },
            })
        }))
        .into_any_element()
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    _this: &mut Oxide,
    _window: &mut Window,
    _cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    None
}

/// A step of `OXIDE_SCRIPT` this module understands: `search:type <text>` (types in the top field),
/// `search:run <text>` (Return: full search), `search:menu <from|date|kind>`, `search:focus`, `search:open <n>` (n-th hit),
/// `search:filter <attachment|mentions|bots|custom>`.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("search:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "type" | "run" => {
            let text = arg.to_owned();
            this.ui
                .search
                .query
                .update(cx, |i, cx| i.set_text(text.clone(), None, cx));
            this.ui.search.query.read(cx).focus(window);
            send_query(this, text.clone());
            if verb == "run" {
                let filters = this
                    .search
                    .as_ref()
                    .map(|s| s.filters.clone())
                    .unwrap_or_default();
                this.send(Action::FullSearch {
                    action: SearchAction::Run {
                        query: text,
                        filters,
                    },
                });
            }
        }
        "focus" => focus(this, window, cx),
        "menu" => {
            let key = match arg {
                "from" => FROM,
                "date" => DATE,
                _ => KIND,
            };
            this.toggle_popup(key, cx);
        }
        "open" => {
            let Some(hit) = arg
                .parse::<usize>()
                .ok()
                .and_then(|n| this.search.as_ref()?.hits.get(n).cloned())
            else {
                return false;
            };
            this.send(Action::FullSearch {
                action: SearchAction::OpenHit {
                    message_id: hit.message_id,
                },
            });
        }
        "filter" => match arg {
            "attachment" => run(this, |f| f.has_attachment = !f.has_attachment),
            "mentions" => run(this, |f| f.mentions_me = !f.mentions_me),
            "bots" => run(this, |f| f.hide_bots = !f.hide_bots),
            "custom" => run(this, |f| f.date = SearchDate::Custom),
            _ => return false,
        },
        _ => return false,
    }
    true
}
