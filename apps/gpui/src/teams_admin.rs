//! Team dialogs (`TeamsAdmin.swift`): create a team or a channel, the gallery to join one, the join
//! code, and the "leave the team" confirmation of `TeamsList.swift` (owner: teams agent).
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput};
use crate::oxide::{MONO, Oxide, tr, trf};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, FontWeight, SharedString, Subscription, Task,
    Window, div, prelude::*, px,
};
use std::time::Duration;
use teams_core::{
    Action, AvatarShape, GalleryJoin, GalleryTeamView, NewTeamVisibility, TeamsAdminAction,
};

#[derive(Default)]
pub struct Ui {
    new_team: Option<NewTeam>,
    new_channel: Option<NewChannel>,
    /// The team whose "Leave" waits for confirmation: (id, name).
    pub leaving: Option<(String, String)>,
    /// The gallery's fields, alive while the core has the gallery open.
    gallery: Option<Gallery>,
}

struct NewTeam {
    name: Entity<TextInput>,
    description: Entity<TextInput>,
    visibility: NewTeamVisibility,
    /// The core's creation count when the dialog opened: one more closes it (`CloseOnCreated`).
    start: u32,
    _subscriptions: Vec<Subscription>,
}

struct NewChannel {
    team_id: String,
    team_name: String,
    name: Entity<TextInput>,
    description: Entity<TextInput>,
    private: bool,
    show_for_everyone: bool,
    start: u32,
    _subscriptions: Vec<Subscription>,
}

struct Gallery {
    query: Entity<TextInput>,
    code: Entity<TextInput>,
    /// The pending search, sent once typing pauses; replaced (so cancelled) by each keystroke.
    search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn admin(action: TeamsAdminAction) -> Action {
    Action::TeamsAdmin { action }
}

/// Name and description fields of a creation dialog: typing redraws (for Create), Return creates, Escape cancels.
fn creation_fields(
    name_placeholder: String,
    window: &mut Window,
    cx: &mut Context<Oxide>,
    create: fn(&mut Oxide, &mut Context<Oxide>),
    cancel: fn(&mut Oxide, &mut Context<Oxide>),
) -> (Entity<TextInput>, Entity<TextInput>, Vec<Subscription>) {
    let name = cx.new(|cx| TextInput::new(cx, name_placeholder));
    let description =
        cx.new(|cx| TextInput::new(cx, tr("ui.teams_admin.description_optional")).multiline(false));
    let on_event = move |this: &mut Oxide,
                         _: Entity<TextInput>,
                         event: &InputEvent,
                         cx: &mut Context<Oxide>| match event {
        InputEvent::Submit => create(this, cx),
        InputEvent::Cancel => cancel(this, cx),
        _ => cx.notify(),
    };
    let subscriptions = vec![
        cx.subscribe(&name, on_event),
        cx.subscribe(&description, on_event),
    ];
    name.read(cx).focus(window);
    (name, description, subscriptions)
}

fn created(this: &Oxide) -> u32 {
    this.teams_admin.as_ref().map(|a| a.created).unwrap_or(0)
}
fn creating(this: &Oxide) -> bool {
    this.teams_admin.as_ref().is_some_and(|a| a.creating)
}

pub fn open_new_team(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    let (name, description, subscriptions) = creation_fields(
        tr("ui.teams_admin.team_name"),
        window,
        cx,
        create_team,
        |this, cx| {
            this.ui.teams.admin.new_team = None;
            cx.notify();
        },
    );
    this.ui.teams.admin.new_team = Some(NewTeam {
        name,
        description,
        visibility: NewTeamVisibility::Private,
        start: created(this),
        _subscriptions: subscriptions,
    });
    this.send(admin(TeamsAdminAction::DismissError));
    cx.notify();
}

pub fn open_new_channel(
    this: &mut Oxide,
    team_id: &str,
    team_name: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) {
    let (name, description, subscriptions) = creation_fields(
        tr("ui.teams_admin.channel_name"),
        window,
        cx,
        create_channel,
        |this, cx| {
            this.ui.teams.admin.new_channel = None;
            cx.notify();
        },
    );
    this.ui.teams.admin.new_channel = Some(NewChannel {
        team_id: team_id.to_owned(),
        team_name: team_name.to_owned(),
        name,
        description,
        private: false,
        show_for_everyone: false,
        start: created(this),
        _subscriptions: subscriptions,
    });
    this.send(admin(TeamsAdminAction::DismissError));
    cx.notify();
}

fn create_team(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let Some(form) = &this.ui.teams.admin.new_team else {
        return;
    };
    let name = form.name.read(cx).text().to_owned();
    if name.trim().is_empty() || creating(this) {
        return;
    }
    let action = TeamsAdminAction::CreateTeam {
        name,
        description: form.description.read(cx).text().to_owned(),
        visibility: form.visibility,
    };
    this.send(admin(action));
}

fn create_channel(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let Some(form) = &this.ui.teams.admin.new_channel else {
        return;
    };
    let name = form.name.read(cx).text().to_owned();
    if name.trim().is_empty() || creating(this) {
        return;
    }
    let action = TeamsAdminAction::CreateChannel {
        team_id: form.team_id.clone(),
        name,
        description: form.description.read(cx).text().to_owned(),
        private: form.private,
        show_for_everyone: form.show_for_everyone && !form.private,
    };
    this.send(admin(action));
}

/// The dialogs open now, front last.
pub fn overlays(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> Vec<AnyElement> {
    let created = created(this);
    let state = &mut this.ui.teams.admin;
    if state.new_team.as_ref().is_some_and(|f| created > f.start) {
        state.new_team = None;
    }
    if state
        .new_channel
        .as_ref()
        .is_some_and(|f| created > f.start)
    {
        state.new_channel = None;
    }
    let gallery_open = this
        .teams_admin
        .as_ref()
        .is_some_and(|a| a.gallery.is_some());
    if gallery_open != this.ui.teams.admin.gallery.is_some() {
        this.ui.teams.admin.gallery = gallery_open.then(|| gallery_fields(this, cx));
    }

    let mut out = Vec::new();
    if gallery_open {
        out.extend(gallery(this, window, cx));
    }
    if this
        .teams_admin
        .as_ref()
        .is_some_and(|a| a.join_code.is_some())
    {
        out.extend(join_code(this, cx));
    }
    if this.ui.teams.admin.new_team.is_some() {
        out.push(new_team(this, cx));
    }
    if this.ui.teams.admin.new_channel.is_some() {
        out.push(new_channel(this, cx));
    }
    if this.ui.teams.admin.leaving.is_some() {
        out.push(leave_confirmation(this, cx));
    }
    out
}

/// `AdminError`: the error of the team dialogs, with a close button.
fn admin_error(this: &Oxide) -> Option<AnyElement> {
    let theme = this.theme;
    let error = this.teams_admin.as_ref()?.error.clone()?;
    let core = this.core.clone();
    Some(
        div()
            .flex()
            .items_start()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(theme.error_text)
                    .child(error),
            )
            .child(
                widgets::icon_button_sized(
                    "teams-admin-error-close",
                    Icon::Close,
                    20.,
                    10.,
                    false,
                    &theme,
                )
                .tooltip(widgets::tooltip(tr("ui.status_menu.close"), theme))
                .on_click(move |_, _, _| core.send(admin(TeamsAdminAction::DismissError))),
            )
            .into_any_element(),
    )
}

/// `CreateButtons`: Cancel, and Create once a name is typed.
fn create_buttons(
    this: &Oxide,
    name: &Entity<TextInput>,
    cx: &mut Context<Oxide>,
    create: fn(&mut Oxide, &mut Context<Oxide>),
    cancel: fn(&mut Oxide, &mut Context<Oxide>),
) -> AnyElement {
    let theme = this.theme;
    let creating = creating(this);
    let empty = name.read(cx).text().trim().is_empty();
    div()
        .flex()
        .justify_end()
        .gap_2()
        .child(
            widgets::button(
                "teams-create-cancel",
                tr("ui.chat_admin.cancel"),
                ButtonKind::Outline,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| cancel(this, cx))),
        )
        .child(
            widgets::disabled(
                widgets::button(
                    "teams-create",
                    if creating {
                        tr("ui.teams_admin.creating")
                    } else {
                        tr("ui.teams_admin.create")
                    },
                    ButtonKind::Primary,
                    &theme,
                ),
                empty || creating,
            )
            .on_click(cx.listener(move |this, _, _, cx| create(this, cx))),
        )
        .into_any_element()
}

fn dialog_title(text: String) -> gpui::Div {
    div()
        .text_size(px(17.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// A dialog's body: Swift's 20-point padding and 14-point spacing.
fn sheet() -> gpui::Div {
    div().p_5().flex().flex_col().gap(px(14.))
}

fn close_with(
    cx: &mut Context<Oxide>,
    close: fn(&mut Oxide, &mut Context<Oxide>),
) -> impl Fn(&mut Window, &mut App) + 'static {
    let entity = cx.entity();
    move |_, cx| entity.update(cx, close)
}

fn close_new_team(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.teams.admin.new_team = None;
    cx.notify();
}
fn close_new_channel(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.teams.admin.new_channel = None;
    cx.notify();
}

/// `NewTeamSheet`.
fn new_team(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(form) = &this.ui.teams.admin.new_team else {
        return div().into_any_element();
    };
    let (name, description, visibility) =
        (form.name.clone(), form.description.clone(), form.visibility);
    let choices = [
        (NewTeamVisibility::Private, "ui.teams_admin.private"),
        (NewTeamVisibility::Public, "ui.teams_admin.public"),
        (NewTeamVisibility::OrgWide, "ui.teams_admin.org_wide"),
    ];
    let radios = div()
        .flex()
        .flex_col()
        .gap_2()
        .children(choices.into_iter().map(|(value, key)| {
            widgets::radio(
                SharedString::from(format!("new-team-{key}")),
                visibility == value,
                tr(key),
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(form) = &mut this.ui.teams.admin.new_team {
                    form.visibility = value;
                    cx.notify();
                }
            }))
        }));
    let explanation = match visibility {
        NewTeamVisibility::Private => tr("ui.teams_admin.private_explanation"),
        NewTeamVisibility::Public => tr("ui.teams_admin.public_explanation"),
        NewTeamVisibility::OrgWide => tr("ui.teams_admin.org_wide_explanation"),
    };
    let content = sheet()
        .child(dialog_title(tr("ui.teams_admin.create_team")))
        .child(widgets::text_field(&name, &theme, cx))
        .child(widgets::text_field(&description, &theme, cx).min_h(px(72.)))
        .child(widgets::form_row(
            tr("ui.teams_admin.privacy"),
            radios,
            &theme,
        ))
        .child(widgets::muted(explanation, 12., &theme))
        .children(admin_error(this))
        .child(create_buttons(this, &name, cx, create_team, close_new_team));
    widgets::modal(
        "new-team",
        px(440.),
        content,
        &theme,
        close_with(cx, close_new_team),
    )
}

/// `NewChannelSheet`.
fn new_channel(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(form) = &this.ui.teams.admin.new_channel else {
        return div().into_any_element();
    };
    let (name, description, private, everyone) = (
        form.name.clone(),
        form.description.clone(),
        form.private,
        form.show_for_everyone,
    );
    let title = trf(
        "ui.teams_admin.new_channel_in",
        &[("team", &form.team_name)],
    );
    let radios = div().flex().flex_col().gap_2().children(
        [
            (false, "ui.teams_admin.standard_channel"),
            (true, "ui.teams_admin.private_channel"),
        ]
        .map(|(value, key)| {
            widgets::radio(
                SharedString::from(format!("new-channel-{key}")),
                private == value,
                tr(key),
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(form) = &mut this.ui.teams.admin.new_channel {
                    form.private = value;
                    cx.notify();
                }
            }))
        }),
    );
    let content = sheet()
        .child(dialog_title(title))
        .child(widgets::text_field(&name, &theme, cx))
        .child(widgets::text_field(&description, &theme, cx).min_h(px(54.)))
        .child(widgets::form_row(
            tr("ui.teams_admin.channel_type"),
            radios,
            &theme,
        ))
        .when(!private, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        widgets::toggle("new-channel-everyone", everyone, &theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(form) = &mut this.ui.teams.admin.new_channel {
                                    form.show_for_everyone = !form.show_for_everyone;
                                    cx.notify();
                                }
                            }),
                        ),
                    )
                    .child(tr("ui.teams_admin.show_for_everyone")),
            )
        })
        .children(admin_error(this))
        .child(create_buttons(
            this,
            &name,
            cx,
            create_channel,
            close_new_channel,
        ));
    widgets::modal(
        "new-channel",
        px(440.),
        content,
        &theme,
        close_with(cx, close_new_channel),
    )
}

fn close_gallery(this: &mut Oxide, _: &mut Context<Oxide>) {
    this.send(admin(TeamsAdminAction::CloseGallery));
}

fn join_with_code(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let Some(fields) = &this.ui.teams.admin.gallery else {
        return;
    };
    let code = fields.code.read(cx).text().to_owned();
    let joining = this
        .teams_admin
        .as_ref()
        .and_then(|a| a.gallery.as_ref())
        .is_some_and(|g| g.joining_code);
    if !code.trim().is_empty() && !joining {
        this.send(admin(TeamsAdminAction::JoinWithCode { code }));
    }
}

fn gallery_fields(this: &Oxide, cx: &mut Context<Oxide>) -> Gallery {
    let current = this
        .teams_admin
        .as_ref()
        .and_then(|a| a.gallery.as_ref())
        .map(|g| g.query.clone())
        .unwrap_or_default();
    let query = cx.new(|cx| {
        let mut input = TextInput::new(cx, tr("ui.teams_admin.search_teams"));
        input.set_text(current, None, cx);
        input
    });
    let code = cx.new(|cx| TextInput::new(cx, tr("ui.teams_admin.enter_code")));
    let searched =
        cx.subscribe(
            &query,
            |this: &mut Oxide, input, event: &InputEvent, cx| match event {
                InputEvent::Changed => {
                    let query = input.read(cx).text().to_owned();
                    let task = cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(400))
                            .await;
                        let _ = this.update(cx, |this, _| {
                            this.send(admin(TeamsAdminAction::GallerySearch { query }))
                        });
                    });
                    if let Some(fields) = &mut this.ui.teams.admin.gallery {
                        fields.search = Some(task);
                    }
                }
                InputEvent::Submit => {
                    let query = input.read(cx).text().to_owned();
                    if let Some(fields) = &mut this.ui.teams.admin.gallery {
                        fields.search = None;
                    }
                    this.send(admin(TeamsAdminAction::GallerySearch { query }));
                }
                InputEvent::Cancel => close_gallery(this, cx),
                _ => {}
            },
        );
    let coded = cx.subscribe(
        &code,
        |this: &mut Oxide, _, event: &InputEvent, cx| match event {
            InputEvent::Submit => join_with_code(this, cx),
            InputEvent::Cancel => close_gallery(this, cx),
            _ => cx.notify(),
        },
    );
    Gallery {
        query,
        code,
        search: None,
        _subscriptions: vec![searched, coded],
    }
}

/// `TeamsGallerySheet`: "Join or create a team".
fn gallery(this: &mut Oxide, _window: &mut Window, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let theme = this.theme;
    let view = this.teams_admin.as_ref()?.gallery.clone()?;
    let fields = this.ui.teams.admin.gallery.as_ref()?;
    let (query, code) = (fields.query.clone(), fields.code.clone());
    let code_empty = code.read(cx).text().trim().is_empty();
    let header = div()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.teams_admin.join_or_create")),
        )
        .child(
            div()
                .w(px(240.))
                .child(widgets::text_field(&query, &theme, cx)),
        );
    let actions = div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            widgets::button(
                "gallery-create",
                tr("ui.teams_admin.create_team"),
                ButtonKind::Primary,
                &theme,
            )
            // The icon leads, as in Swift's `Label`.
            .flex_row_reverse()
            .child(icon(Icon::Plus, px(14.), gpui::white()))
            .on_click(cx.listener(|this, _, window, cx| {
                this.send(admin(TeamsAdminAction::CloseGallery));
                open_new_team(this, window, cx);
            })),
        )
        .child(div().w(px(1.)).h(px(24.)).bg(theme.border))
        .child(
            div()
                .w(px(180.))
                .child(widgets::text_field(&code, &theme, cx)),
        )
        .child(
            widgets::disabled(
                widgets::button(
                    "gallery-join-code",
                    if view.joining_code {
                        tr("ui.teams_admin.joining")
                    } else {
                        tr("ui.teams_admin.join_with_code")
                    },
                    ButtonKind::Outline,
                    &theme,
                ),
                code_empty || view.joining_code,
            )
            .on_click(cx.listener(|this, _, _, cx| join_with_code(this, cx))),
        );
    let core = this.core.clone();
    let mut grid = div().grid().grid_cols(3).gap_3();
    for team in &view.teams {
        grid = grid.child(gallery_card(this, team));
    }
    let results = div()
        .id("gallery-results")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .child(grid)
        .when(view.teams.is_empty() && !view.loading, |d| {
            d.child(div().py_5().child(widgets::muted(
                if view.query.is_empty() {
                    tr("ui.teams_admin.no_suggestions")
                } else {
                    tr("ui.teams_admin.no_teams_found")
                },
                13.,
                &theme,
            )))
        })
        .when(view.loading, |d| {
            d.child(div().p_4().flex().justify_center().child(widgets::spinner(
                "gallery-loading",
                16.,
                theme.muted,
            )))
        })
        .when(view.has_more && !view.loading, |d| {
            d.child(
                div().py_2().flex().child(
                    crate::teams::ghost(
                        "gallery-more",
                        tr("ui.teams_admin.more_teams"),
                        13.,
                        true,
                        &theme,
                    )
                    .on_click(move |_, _, _| core.send(admin(TeamsAdminAction::GalleryMore))),
                ),
            )
        });
    let content = sheet()
        .h(px(560.))
        .child(header)
        .child(actions)
        .when(view.code_pending, |d| {
            d.child(widgets::muted(
                tr("ui.teams_admin.code_pending"),
                12.,
                &theme,
            ))
        })
        .children(admin_error(this))
        .child(results)
        .child(
            div().flex().justify_end().child(
                widgets::button(
                    "gallery-close",
                    tr("ui.status_menu.close"),
                    ButtonKind::Outline,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, cx| close_gallery(this, cx))),
            ),
        );
    Some(widgets::modal(
        "teams-gallery",
        px(720.),
        content,
        &theme,
        close_with(cx, close_gallery),
    ))
}

fn gallery_card(this: &mut Oxide, team: &GalleryTeamView) -> AnyElement {
    let theme = this.theme;
    let avatar = this.avatar(&team.group_id, &team.name, 36., AvatarShape::Square, None);
    let core = this.core.clone();
    let group_id = team.group_id.clone();
    let join = match team.join {
        GalleryJoin::None => widgets::button(
            SharedString::from(format!("gallery-join-{}", team.group_id)),
            if team.private {
                tr("ui.teams_admin.request_to_join")
            } else {
                tr("ui.teams_admin.join")
            },
            ButtonKind::Outline,
            &theme,
        )
        .on_click(move |_, _, _| {
            core.send(admin(TeamsAdminAction::JoinTeam {
                group_id: group_id.clone(),
            }))
        })
        .into_any_element(),
        GalleryJoin::Joining => widgets::spinner(
            SharedString::from(format!("gallery-joining-{}", team.group_id)),
            14.,
            theme.muted,
        ),
        GalleryJoin::Joined => status(Icon::Check, tr("ui.teams_admin.joined"), theme.accent),
        GalleryJoin::Requested => {
            status(Icon::Clock, tr("ui.teams_admin.request_sent"), theme.muted)
        }
    };
    widgets::card(&theme)
        .p(px(14.))
        .min_h(px(150.))
        .flex()
        .flex_col()
        .items_start()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(avatar)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .line_clamp(2)
                                .child(team.name.clone()),
                        )
                        .child(widgets::muted(
                            if team.private {
                                tr("ui.teams_admin.private")
                            } else {
                                tr("ui.teams_admin.public")
                            },
                            11.,
                            &theme,
                        )),
                ),
        )
        .when(!team.description.is_empty(), |d| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.text_soft)
                    .line_clamp(3)
                    .child(team.description.clone()),
            )
        })
        .when(team.known_members > 0, |d| {
            d.child(widgets::muted(
                trf(
                    "ui.teams_admin.known_members",
                    &[("count", &team.known_members.to_string())],
                ),
                11.,
                &theme,
            ))
        })
        .child(div().flex_1())
        .child(join)
        .into_any_element()
}

fn status(glyph: Icon, label: String, color: gpui::Hsla) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(12.))
        .text_color(color)
        .child(icon(glyph, px(13.), color))
        .child(label)
        .into_any_element()
}

fn close_join_code(this: &mut Oxide, _: &mut Context<Oxide>) {
    this.send(admin(TeamsAdminAction::CloseJoinCode));
}

/// `JoinCodeSheet`: the team's join code, which owners can generate again.
fn join_code(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let theme = this.theme;
    let view = this.teams_admin.as_ref()?.join_code.clone()?;
    let body = if view.loading {
        div()
            .child(widgets::spinner("join-code-loading", 16., theme.muted))
            .into_any_element()
    } else if let Some(code) = view.code.clone() {
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_family(MONO)
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(code.clone()),
                    )
                    .child(
                        widgets::button(
                            "join-code-copy",
                            tr("ui.teams_admin.copy"),
                            ButtonKind::Outline,
                            &theme,
                        )
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                        }),
                    ),
            )
            .child(widgets::muted(
                tr("ui.teams_admin.code_explanation"),
                12.,
                &theme,
            ))
            .into_any_element()
    } else {
        widgets::muted(tr("ui.teams_admin.no_code"), 12., &theme).into_any_element()
    };
    let core = this.core.clone();
    let content = sheet()
        .child(dialog_title(trf(
            "ui.teams_admin.team_code_of",
            &[("team", &view.team_name)],
        )))
        .child(body)
        .children(admin_error(this))
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    widgets::disabled(
                        widgets::button(
                            "join-code-reset",
                            if view.code.is_none() {
                                tr("ui.teams_admin.generate")
                            } else {
                                tr("ui.teams_admin.reset_code")
                            },
                            ButtonKind::Outline,
                            &theme,
                        ),
                        view.loading,
                    )
                    .when(!view.loading, |d| {
                        d.on_click(move |_, _, _| core.send(admin(TeamsAdminAction::ResetJoinCode)))
                    }),
                )
                .child(div().flex_1())
                .child(
                    widgets::button(
                        "join-code-close",
                        tr("ui.status_menu.close"),
                        ButtonKind::Primary,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| close_join_code(this, cx))),
                ),
        );
    Some(widgets::modal(
        "join-code",
        px(400.),
        content,
        &theme,
        close_with(cx, close_join_code),
    ))
}

fn close_leave(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.teams.admin.leaving = None;
    cx.notify();
}

/// The confirmation of "Leave the team".
fn leave_confirmation(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let (id, name) = this.ui.teams.admin.leaving.clone().unwrap_or_default();
    confirmation(
        "leave-team",
        trf("ui.teams_admin.leave_question", &[("name", &name)]),
        None,
        tr("ui.teams_admin.leave_team"),
        &theme,
        cx,
        move |this, cx| {
            this.send(admin(TeamsAdminAction::LeaveTeam {
                team_id: id.clone(),
            }));
            close_leave(this, cx);
        },
        close_leave,
    )
}

/// A small dialog asking to confirm a destructive step (Swift's `confirmationDialog`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn confirmation(
    id: &'static str,
    title: String,
    message: Option<String>,
    confirm: String,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    run: impl Fn(&mut Oxide, &mut Context<Oxide>) + 'static,
    cancel: fn(&mut Oxide, &mut Context<Oxide>),
) -> AnyElement {
    let content = sheet()
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .children(message.map(|m| widgets::muted(m, 13., theme)))
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    widgets::button(
                        SharedString::from(format!("{id}-cancel")),
                        tr("ui.chat_admin.cancel"),
                        ButtonKind::Outline,
                        theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| cancel(this, cx))),
                )
                .child(
                    widgets::button(
                        SharedString::from(format!("{id}-confirm")),
                        confirm,
                        ButtonKind::Danger,
                        theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| run(this, cx))),
                ),
        );
    widgets::modal(id, px(360.), content, theme, close_with(cx, cancel))
}
