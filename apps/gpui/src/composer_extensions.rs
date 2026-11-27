//! The compose box's apps (`ExtensionsFlyout.swift`), as the web client's message extension flyout: pick an app,
//! search or fill its form, insert a card; and the cards waiting in the draft (`ComposerCards`).
use crate::composer::APPS;
use crate::composer_thumbs;
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput, field};
use crate::oxide::{Oxide, tr, trf};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div, img,
    prelude::*, px,
};
use std::collections::HashMap;
use std::time::Duration;
use teams_core::{
    Action, BlockView, CardActionKind, CardInputKind, CardInputValue, CardInputView, CardItemView,
    CardView, ExtensionAppView, ExtensionsAction, ExtensionsView, ImageStatus, RunView,
};

pub struct Flyout {
    /// `inserted` when the flyout opened: a higher one means a card went in, and the flyout closes.
    opened: Option<u32>,
    search: Entity<TextInput>,
    /// The command the search field belongs to: another one starts it empty.
    command: Option<String>,
    pending: u64,
    /// The open form's typed values, by input id; dropped with the form.
    inputs: HashMap<String, Entity<TextInput>>,
    choices: HashMap<String, String>,
    _subscription: Subscription,
}

impl Flyout {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let search = cx.new(|cx| TextInput::new(cx, tr("ui.extensions.search")));
        let subscription = cx.subscribe_in(
            &search,
            window,
            |this: &mut Oxide, _, event: &InputEvent, _, cx| match event {
                InputEvent::Changed => {
                    let flyout = &mut this.ui.composer.apps;
                    flyout.pending += 1;
                    let pending = flyout.pending;
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(400))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if this.ui.composer.apps.pending == pending {
                                search_now(this, cx);
                            }
                        });
                    })
                    .detach();
                }
                InputEvent::Submit => search_now(this, cx),
                InputEvent::Cancel => {
                    let search = this.ui.composer.apps.search.clone();
                    search.update(cx, |s, cx| s.set_text("", None, cx));
                }
                _ => {}
            },
        );
        Self {
            opened: None,
            search,
            command: None,
            pending: 0,
            inputs: HashMap::new(),
            choices: HashMap::new(),
            _subscription: subscription,
        }
    }
}

fn send(this: &Oxide, action: ExtensionsAction) {
    this.send(Action::Extensions { action });
}

fn search_now(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.composer.apps.pending += 1;
    let query = this.ui.composer.apps.search.read(cx).text().to_owned();
    send(this, ExtensionsAction::Search { query });
}

/// The flyout opened: the apps are listed once.
pub fn opened(this: &mut Oxide) {
    this.ui.composer.apps.opened = Some(this.extensions.as_ref().map_or(0, |v| v.inserted));
    send(this, ExtensionsAction::Load);
}

/// A new core update: a card went into the draft closes the flyout; a closed form forgets its values.
pub fn changed(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let inserted = this.extensions.as_ref().map(|v| v.inserted);
    if let (Some(inserted), Some(opened)) = (inserted, this.ui.composer.apps.opened)
        && inserted > opened
        && this.popup_open(APPS)
    {
        this.ui.composer.apps.opened = None;
        this.close_popup(cx);
    }
    if this.extensions.as_ref().is_none_or(|v| v.form.is_none()) {
        let flyout = &mut this.ui.composer.apps;
        flyout.inputs.clear();
        flyout.choices.clear();
    }
}

pub fn debug_choose(this: &mut Oxide, n: usize) {
    if let Some(app) = this.extensions.as_ref().and_then(|v| v.apps.get(n))
        && let Some(first) = app.commands.first()
    {
        send(
            this,
            ExtensionsAction::Choose {
                app_id: app.id.clone(),
                command_id: first.id.clone(),
            },
        );
    }
}

/// An app's catalog icon, or a generic one.
fn app_icon(
    this: &mut Oxide,
    url: Option<&str>,
    side: f32,
    id: SharedString,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let inner = match url {
        Some(url) => composer_thumbs::view(this, url, id, cx),
        None => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(icon(Icon::Apps, px(side * 0.6), theme.muted))
            .into_any_element(),
    };
    div()
        .size(px(side))
        .flex_none()
        .rounded(px(6.))
        .overflow_hidden()
        .child(inner)
        .into_any_element()
}

pub fn flyout(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let view = this.extensions.clone().unwrap_or_default();
    let app = view
        .app_id
        .as_ref()
        .and_then(|id| view.apps.iter().find(|a| &a.id == id))
        .cloned();
    let content = match app {
        Some(app) => chosen(this, &view, &app, window, cx),
        None => apps(this, &view, cx),
    };
    div()
        .w(px(400.))
        .h(px(440.))
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(content)
        .into_any_element()
}

fn apps(this: &mut Oxide, view: &ExtensionsView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let mut column = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.extensions.apps")),
        );
    if view.apps_loading || this.extensions.is_none() {
        column = column.child(
            widgets::loading("apps-loading", tr("ui.extensions.loading_apps"), &theme)
                .text_size(px(12.)),
        );
    } else if let Some(error) = &view.apps_error {
        column = column.child(widgets::muted(error.clone(), 12., &theme));
    } else if view.apps.is_empty() {
        column = column.child(widgets::muted(tr("ui.extensions.no_apps"), 12., &theme));
        if let Some(note) = &view.apps_note {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.subtle)
                    .child(note.clone()),
            );
        }
    }
    let mut list = div()
        .id("apps-list")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(2.));
    for (ix, app) in view.apps.iter().enumerate() {
        let face = app_icon(
            this,
            app.icon.as_deref(),
            28.,
            format!("app-icon-{ix}").into(),
            cx,
        );
        let commands = app
            .commands
            .iter()
            .map(|c| c.title.clone())
            .collect::<Vec<_>>()
            .join(" · ");
        let (app_id, first) = (app.id.clone(), app.commands.first().map(|c| c.id.clone()));
        let hover = theme.input;
        list = list.child(
            div()
                .id(("app", ix))
                .p(px(6.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(face)
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(app.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.muted)
                                .truncate()
                                .child(commands),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    if let Some(command_id) = &first {
                        send(
                            this,
                            ExtensionsAction::Choose {
                                app_id: app_id.clone(),
                                command_id: command_id.clone(),
                            },
                        );
                    }
                })),
        );
    }
    column.child(list).into_any_element()
}

fn chosen(
    this: &mut Oxide,
    view: &ExtensionsView,
    app: &ExtensionAppView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let face = app_icon(this, app.icon.as_deref(), 22., "chosen-app-icon".into(), cx);
    let header = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            widgets::icon_button_sized("apps-back", Icon::ChevronLeft, 26., 12., false, &theme)
                .tooltip(widgets::tooltip(tr("ui.extensions.back"), theme))
                .on_click(cx.listener(|this, _, _, _| send(this, ExtensionsAction::Back))),
        )
        .child(face)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(app.name.clone()),
        )
        .when(view.inserting, |d| {
            d.child(widgets::spinner("apps-inserting", 14., theme.muted))
        });
    let mut column = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(header);
    if app.commands.len() > 1 {
        let mut row = div().id("app-commands").flex().gap_1().overflow_x_scroll();
        for (ix, command) in app.commands.iter().enumerate() {
            let selected = view.command_id.as_ref() == Some(&command.id);
            let (app_id, command_id) = (app.id.clone(), command.id.clone());
            let help = if command.description.is_empty() {
                command.title.clone()
            } else {
                command.description.clone()
            };
            row = row.child(
                div()
                    .id(("app-command", ix))
                    .flex_none()
                    .px(px(8.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(selected, |d| {
                        d.bg(theme.accent_soft).font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(icon(
                        if command.action {
                            Icon::Compose
                        } else {
                            Icon::Search
                        },
                        px(12.),
                        theme.muted,
                    ))
                    .child(command.title.clone())
                    .tooltip(widgets::tooltip(help, theme))
                    .on_click(cx.listener(move |this, _, _, _| {
                        send(
                            this,
                            ExtensionsAction::Choose {
                                app_id: app_id.clone(),
                                command_id: command_id.clone(),
                            },
                        )
                    })),
            );
        }
        column = column.child(row);
    }
    let command = app
        .commands
        .iter()
        .find(|c| Some(&c.id) == view.command_id.as_ref())
        .cloned();
    if let Some(form) = &view.form {
        column = column.child(form_view(this, form, window, cx));
    } else if let Some(command) = command.filter(|c| !c.action) {
        // Another command starts its search empty.
        if this.ui.composer.apps.command.as_ref() != Some(&command.id) {
            this.ui.composer.apps.command = Some(command.id.clone());
            let search = this.ui.composer.apps.search.clone();
            let current = view.query.clone();
            search.update(cx, |s, cx| s.set_text(current, None, cx));
        }
        let placeholder = if command.description.is_empty() {
            tr("ui.extensions.search")
        } else {
            command.description.clone()
        };
        let search = this.ui.composer.apps.search.clone();
        search.update(cx, |s, _| s.set_placeholder(placeholder));
        column = column
            .child(
                div()
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
                            .child(field(&search, widgets::input_colors(&theme), cx)),
                    ),
            )
            .child(results(this, view, cx));
    } else {
        column = column.child(status(this, view, cx));
    }
    column.into_any_element()
}

fn status(this: &mut Oxide, view: &ExtensionsView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .when(view.loading || view.inserting, |d| {
            d.child(div().pt_2().flex().justify_center().child(widgets::spinner(
                "apps-loading-status",
                14.,
                theme.muted,
            )))
        })
        .when_some(view.message.clone(), |d, m| {
            d.child(widgets::muted(m, 12., &theme))
        })
        .when_some(view.link.clone(), |d, link| {
            let url = link.url.clone();
            d.child(
                widgets::button("apps-link", link.title.clone(), ButtonKind::Outline, &theme)
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.send(Action::OpenUrl { url: url.clone() })
                    })),
            )
        })
        .into_any_element()
}

fn results(this: &mut Oxide, view: &ExtensionsView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let status = status(this, view, cx);
    let mut list = div()
        .id("apps-results")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(status);
    if view.grid {
        let mut grid = div().flex().flex_wrap().gap(px(6.));
        for item in &view.results {
            let index = item.index;
            let picture = item.image.as_ref().map(|url| {
                div().h(px(80.)).w_full().child(composer_thumbs::view(
                    this,
                    url,
                    ("result-loading", index as usize),
                    cx,
                ))
            });
            grid = grid.child(
                div()
                    .id(("result", index as usize))
                    .w(px(114.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .cursor_pointer()
                    .children(picture)
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_clamp(2)
                            .child(item.title.clone()),
                    )
                    .tooltip(widgets::tooltip(item.text.clone(), theme))
                    .on_click(cx.listener(move |this, _, _, _| {
                        send(this, ExtensionsAction::Pick { index })
                    })),
            );
        }
        list = list.child(grid);
    } else {
        for item in &view.results {
            let index = item.index;
            let picture = item.image.as_ref().map(|url| {
                div().size(px(44.)).flex_none().child(composer_thumbs::view(
                    this,
                    url,
                    ("result-loading", index as usize),
                    cx,
                ))
            });
            let hover = theme.input;
            list = list.child(
                div()
                    .id(("result", index as usize))
                    .p(px(6.))
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .rounded(px(6.))
                    .when(!view.inserting, move |d| {
                        d.cursor_pointer().hover(move |s| s.bg(hover))
                    })
                    .children(picture)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .line_clamp(2)
                                    .child(item.title.clone()),
                            )
                            .when(!item.text.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme.muted)
                                        .line_clamp(2)
                                        .child(item.text.clone()),
                                )
                            }),
                    )
                    .when(!view.inserting, |d| {
                        d.on_click(cx.listener(move |this, _, _, _| {
                            send(this, ExtensionsAction::Pick { index })
                        }))
                    }),
            );
        }
    }
    list.into_any_element()
}

/// An action command's form: its card's items, whose buttons go to the app rather than to a message.
fn form_view(
    this: &mut Oxide,
    form: &teams_core::ExtensionFormView,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let mut column = div().flex_1().min_h_0().flex().flex_col().gap(px(8.));
    if !form.title.is_empty() {
        column = column.child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(form.title.clone()),
        );
    }
    if let Some(card) = &form.card {
        let items = items(this, &card.items, true, cx);
        column = column.child(
            div()
                .id("form-items")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(items),
        );
        let mut buttons = div().flex().items_center().gap(px(8.));
        for (ix, action) in card.actions.iter().enumerate() {
            let kind = if ix == 0 {
                ButtonKind::Primary
            } else {
                ButtonKind::Outline
            };
            let disabled = form.busy || action.kind == CardActionKind::Unsupported;
            let card_items = card.items.clone();
            buttons = buttons.child(
                widgets::disabled(
                    widgets::button(("form-action", ix), action.title.clone(), kind, &theme),
                    disabled,
                )
                .when(!disabled, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        let inputs = form_values(this, &card_items, cx);
                        send(
                            this,
                            ExtensionsAction::FormAction {
                                action: ix as u32,
                                inputs,
                            },
                        );
                    }))
                }),
            );
        }
        buttons = buttons.child(div().flex_1()).child(
            widgets::button(
                "form-cancel",
                tr("ui.extensions.cancel"),
                ButtonKind::Outline,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, _| send(this, ExtensionsAction::CloseForm))),
        );
        if form.busy {
            buttons = buttons.child(widgets::spinner("form-busy", 14., theme.muted));
        }
        column = column.child(buttons);
    } else if let Some(url) = form.url.clone() {
        column =
            column
                .child(widgets::muted(tr("ui.extensions.web_form"), 12., &theme))
                .child(
                    widgets::button(
                        "form-open",
                        tr("ui.extensions.open_in_browser"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.send(Action::OpenUrl { url: url.clone() })
                    })),
                );
    }
    if let Some(error) = &form.error {
        column = column.child(
            div()
                .text_size(px(12.))
                .text_color(theme.error_text)
                .child(error.clone()),
        );
    }
    column.into_any_element()
}

/// Every input of the items, nested ones included.
fn inputs_of(items: &[CardItemView]) -> Vec<&CardInputView> {
    items
        .iter()
        .flat_map(|item| match item {
            CardItemView::Input { input } => vec![input],
            CardItemView::Columns { columns } => {
                columns.iter().flat_map(|c| inputs_of(&c.items)).collect()
            }
            CardItemView::Emphasis { items } => inputs_of(items),
            _ => vec![],
        })
        .collect()
}

fn form_values(this: &Oxide, items: &[CardItemView], cx: &Context<Oxide>) -> Vec<CardInputValue> {
    let flyout = &this.ui.composer.apps;
    inputs_of(items)
        .into_iter()
        .map(|input| {
            let value = flyout
                .inputs
                .get(&input.id)
                .map(|e| e.read(cx).text().to_owned())
                .or_else(|| flyout.choices.get(&input.id).cloned())
                .unwrap_or_else(|| input.value.clone());
            CardInputValue {
                id: input.id.clone(),
                value,
            }
        })
        .collect()
}

fn runs_text(runs: &[RunView]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}
fn blocks_text(blocks: &[BlockView]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            BlockView::Text { runs } => runs_text(runs),
            BlockView::List { items, .. } => items
                .iter()
                .map(|i| format!("• {}", blocks_text(&i.blocks)))
                .collect::<Vec<_>>()
                .join("\n"),
            BlockView::Quote { body, .. } => blocks_text(body),
            BlockView::Code { text, .. } => text.clone(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A card's items, top to bottom; inputs editable only in a form.
fn items(
    this: &mut Oxide,
    items: &[CardItemView],
    editable: bool,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let theme = this.theme;
    let mut out = Vec::new();
    for item in items {
        out.push(match item {
            CardItemView::Text { runs, muted } => {
                let bold = runs.first().is_some_and(|r| r.bold);
                div()
                    .text_size(px(13.))
                    .when(*muted, |d| d.text_color(theme.muted))
                    .when(bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(runs_text(runs))
                    .into_any_element()
            }
            CardItemView::Blocks { blocks } => div()
                .text_size(px(13.))
                .child(blocks_text(blocks))
                .into_any_element(),
            CardItemView::Gap { points, separator } => div()
                .h(px(*points as f32))
                .when(*separator, |d| d.border_t_1().border_color(theme.border))
                .into_any_element(),
            CardItemView::Facts { facts } => div()
                .flex()
                .flex_col()
                .gap_1()
                .children(facts.iter().map(|f| {
                    div()
                        .flex()
                        .gap_2()
                        .text_size(px(13.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(f.title.clone()),
                        )
                        .child(f.value.clone())
                }))
                .into_any_element(),
            CardItemView::Image { image } => {
                match (&image.status, this.images.images.get(&image.src)) {
                    (ImageStatus::Ready { width, height }, Some(picture)) => {
                        let scale = (360. / (*width).max(1) as f32).min(1.);
                        img(picture.clone())
                            .w(px(*width as f32 * scale))
                            .h(px(*height as f32 * scale))
                            .rounded(px(6.))
                            .into_any_element()
                    }
                    _ => div()
                        .h(px(60.))
                        .w(px(120.))
                        .rounded(px(6.))
                        .bg(theme.input)
                        .into_any_element(),
                }
            }
            CardItemView::Input { input } => input_view(this, input, editable, cx),
            CardItemView::Columns { columns } => {
                let mut row = div().flex().gap_3();
                for column in columns {
                    let inner = self::items(this, &column.items, editable, cx);
                    let cell = div().min_w_0().flex().flex_col().gap_2().children(inner);
                    row = row.child(match column.width {
                        teams_core::CardColumnWidth::Auto => cell.flex_none(),
                        teams_core::CardColumnWidth::Points { points } => {
                            cell.w(px(points as f32)).flex_none()
                        }
                        _ => cell.flex_1(),
                    });
                }
                row.into_any_element()
            }
            CardItemView::Emphasis { items } => {
                let inner = self::items(this, items, editable, cx);
                div()
                    .p_2()
                    .rounded(px(6.))
                    .bg(theme.input)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(inner)
                    .into_any_element()
            }
        });
    }
    out
}

fn input_view(
    this: &mut Oxide,
    input: &CardInputView,
    editable: bool,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let label = input
        .label
        .clone()
        .map(|l| if input.required { format!("{l} *") } else { l });
    let control: AnyElement = match &input.kind {
        CardInputKind::Toggle { title, on, off } => {
            let value = this
                .ui
                .composer
                .apps
                .choices
                .get(&input.id)
                .cloned()
                .unwrap_or_else(|| input.value.clone());
            let checked = value == *on;
            let (id, on, off) = (input.id.clone(), on.clone(), off.clone());
            widgets::checkbox(
                SharedString::from(format!("toggle-{}", input.id)),
                checked,
                title.clone(),
                &theme,
            )
            .text_size(px(13.))
            .when(editable, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    this.ui
                        .composer
                        .apps
                        .choices
                        .insert(id.clone(), if checked { off.clone() } else { on.clone() });
                    cx.notify();
                }))
            })
            .into_any_element()
        }
        CardInputKind::Choice { choices, multi } => {
            let value = this
                .ui
                .composer
                .apps
                .choices
                .get(&input.id)
                .cloned()
                .unwrap_or_else(|| input.value.clone());
            let picked: Vec<String> = value
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            let mut list = div().flex().flex_col().gap_1().text_size(px(13.));
            for (ix, choice) in choices.iter().enumerate() {
                let on = picked.contains(&choice.value);
                let element_id = SharedString::from(format!("choice-{}-{ix}", input.id));
                let (id, choice_value, multi, picked) = (
                    input.id.clone(),
                    choice.value.clone(),
                    *multi,
                    picked.clone(),
                );
                let pick = cx.listener(move |this: &mut Oxide, _: &gpui::ClickEvent, _, cx| {
                    let value = if multi {
                        let mut next = picked.clone();
                        if on {
                            next.retain(|v| *v != choice_value);
                        } else {
                            next.push(choice_value.clone());
                        }
                        next.join(",")
                    } else {
                        choice_value.clone()
                    };
                    this.ui.composer.apps.choices.insert(id.clone(), value);
                    cx.notify();
                });
                let control = if multi {
                    widgets::checkbox(element_id, on, choice.title.clone(), &theme)
                } else {
                    widgets::radio(element_id, on, choice.title.clone(), &theme)
                };
                list = list.child(control.when(editable, |d| d.on_click(pick)));
            }
            list.into_any_element()
        }
        CardInputKind::Text { placeholder, .. } | CardInputKind::Number { placeholder }
            if editable =>
        {
            text_input(this, input, placeholder.clone(), cx)
        }
        _ if editable => text_input(this, input, String::new(), cx),
        _ => div()
            .text_size(px(13.))
            .text_color(theme.muted)
            .child(input.value.clone())
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .when_some(label, |d, l| {
            d.child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_soft)
                    .child(l),
            )
        })
        .child(control)
        .into_any_element()
}

fn text_input(
    this: &mut Oxide,
    input: &CardInputView,
    placeholder: String,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let multiline = matches!(
        input.kind,
        CardInputKind::Text {
            multiline: true,
            ..
        }
    );
    let entity = this
        .ui
        .composer
        .apps
        .inputs
        .entry(input.id.clone())
        .or_insert_with(|| {
            let value = input.value.clone();
            cx.new(|cx| {
                let mut field = TextInput::new(cx, placeholder);
                if multiline {
                    field = field.multiline(false);
                }
                field.set_text(value, None, cx);
                field
            })
        })
        .clone();
    widgets::text_field(&entity, &theme, cx).into_any_element()
}

/// Cards from message extensions waiting in the draft, sent with it.
pub fn cards(
    this: &mut Oxide,
    cards: &[teams_core::ComposerCardView],
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let mut column = div().flex().flex_col().gap(px(6.));
    for item in cards {
        let index = item.index;
        let body = card(this, &item.card, cx);
        column = column.child(
            div()
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
                                .text_size(px(11.))
                                .text_color(theme.muted)
                                .child(trf("ui.extensions.via", &[("app", &item.app_name)])),
                        )
                        .child(
                            widgets::icon_button_sized(
                                ("remove-card", index as usize),
                                Icon::Close,
                                22.,
                                10.,
                                false,
                                &theme,
                            )
                            .tooltip(widgets::tooltip(tr("ui.extensions.remove_card"), theme))
                            .on_click(cx.listener(
                                move |this, _, _, _| {
                                    send(this, ExtensionsAction::RemoveCard { index })
                                },
                            )),
                        ),
                )
                .child(body),
        );
    }
    column.into_any_element()
}

/// A card in the draft, as it will look: title, items, its buttons (they act once sent).
fn card(this: &mut Oxide, card: &CardView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme: Theme = this.theme;
    let inner = items(this, &card.items, false, cx);
    div()
        .relative()
        .max_w(px(440.))
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .rounded(px(10.))
        .overflow_hidden()
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border)
        .when_some(card.accent, |d, accent| {
            d.pt(px(15.)).child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(3.))
                    .bg(gpui::rgb(accent)),
            )
        })
        .when_some(card.title.clone(), |d, t| {
            d.child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t),
            )
        })
        .when_some(card.subtitle.clone(), |d, t| {
            d.child(widgets::muted(t, 13., &theme))
        })
        .children(inner)
        .when(!card.actions.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(card.actions.iter().enumerate().map(|(ix, a)| {
                        widgets::disabled(
                            widgets::button(
                                SharedString::from(format!("card-{}-{ix}", card.index)),
                                a.title.clone(),
                                ButtonKind::Outline,
                                &theme,
                            ),
                            true,
                        )
                    })),
            )
        })
        .into_any_element()
}
