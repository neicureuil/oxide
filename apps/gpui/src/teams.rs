//! Teams and channels column (`TeamsList.swift`, the Teams part of `SidebarView.swift`) and the
//! teams overview (`TeamsOverview`); the team dialogs are in `teams_admin.rs` (owner: teams agent).
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr, trf};
use crate::teams_admin;
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, ListAlignment, ListState, MouseButton,
    MouseDownEvent, Pixels, Point, SharedString, Window, anchored, deferred, div, list, point,
    prelude::*, px,
};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};
use teams_core::{
    Action, AvatarShape, ChannelRow, NotificationsAction, Sidebar, Target, TeamListAction, TeamRow,
    TeamSort,
};

pub struct Ui {
    /// The column's lines; spliced by key so the scroll position survives updates.
    list: ListState,
    keys: Vec<String>,
    overview: ListState,
    overview_keys: Vec<String>,
    /// Where the open menu of this module appears (the click that opened it).
    menu_at: Point<Pixels>,
    /// The menu a click outside just closed: a click on its own trigger then must not reopen it.
    just_closed: Option<(SharedString, Instant)>,
    pub admin: teams_admin::Ui,
}
impl Ui {
    pub fn new(_window: &mut Window, _cx: &mut Context<Oxide>) -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Top, px(300.)),
            keys: Vec::new(),
            overview: ListState::new(0, ListAlignment::Top, px(300.)),
            overview_keys: Vec::new(),
            menu_at: point(px(0.), px(0.)),
            just_closed: None,
            admin: teams_admin::Ui::default(),
        }
    }
}

/// One row of the column (`TeamLine` plus the notes around the teams), by index into `Sidebar::teams`.
#[derive(Clone, Debug, PartialEq)]
enum Line {
    Team(usize),
    Channel(usize, usize),
    HiddenChannels(usize),
    LoadingChannels(usize),
    FailedChannels(usize),
    NoChannels(usize),
    AllHidden,
    HiddenTeams,
    Skeleton,
    Note(String),
    Problem(String),
    SavingOrder,
}

fn searching(this: &Oxide) -> bool {
    this.shell
        .as_ref()
        .is_some_and(|s| !s.search.trim().is_empty())
}
fn loading(this: &Oxide) -> bool {
    this.shell.as_ref().is_some_and(|s| s.loading_workspace)
}

/// Each team and its channels as lines of their own, so the list builds only the rows in sight.
fn lines(sidebar: &Sidebar, searching: bool, loading: bool) -> Vec<Line> {
    let controls = &sidebar.team_controls;
    let team_lines = |hidden: bool, out: &mut Vec<Line>| {
        for (t, team) in sidebar
            .teams
            .iter()
            .enumerate()
            .filter(|(_, t)| t.hidden == hidden)
        {
            out.push(Line::Team(t));
            if !team.show_channels {
                continue;
            }
            let channels = |hidden: bool| {
                team.channels
                    .iter()
                    .enumerate()
                    .filter(move |(_, c)| c.hidden == hidden)
                    .map(move |(c, _)| Line::Channel(t, c))
            };
            out.extend(channels(false));
            if team.hidden_channel_count > 0 && !searching {
                out.push(Line::HiddenChannels(t));
            }
            out.extend(channels(true));
            if team.loading {
                out.push(Line::LoadingChannels(t));
            }
            if team.failed {
                out.push(Line::FailedChannels(t));
            } else if team.channels.is_empty() && !team.loading && team.hidden_channel_count == 0 {
                out.push(Line::NoChannels(t));
            }
        }
    };
    let mut out = Vec::new();
    team_lines(false, &mut out);
    if !searching && controls.shown_count == 0 && controls.hidden_count > 0 {
        out.push(Line::AllHidden);
    }
    if controls.hidden_count > 0 && !searching {
        out.push(Line::HiddenTeams);
    }
    team_lines(true, &mut out);
    if sidebar.team_count == 0 && loading && !searching {
        out.push(Line::Skeleton);
    } else if sidebar.team_count == 0 || (searching && sidebar.teams.is_empty()) {
        out.push(Line::Note(if loading {
            tr("ui.teams_list.loading_teams")
        } else if sidebar.teams_problem.is_some() {
            tr("ui.teams_list.couldn_t_load_teams")
        } else if searching {
            tr("ui.teams_list.no_matching_teams_or_channels")
        } else {
            tr("ui.teams_list.no_teams_available")
        }));
    }
    if let Some(problem) = &sidebar.teams_problem {
        out.push(Line::Problem(problem.clone()));
    }
    if controls.saving_order {
        out.push(Line::SavingOrder);
    }
    out
}

/// Stable identity of a line, as Swift's `TeamLine.id`.
fn key(line: &Line, sidebar: &Sidebar) -> String {
    let team = |t: usize| sidebar.teams.get(t).map(|t| t.id.as_str()).unwrap_or("");
    match line {
        Line::Team(t) => format!("team:{}", team(*t)),
        Line::Channel(t, c) => {
            let channel = sidebar.teams.get(*t).and_then(|t| t.channels.get(*c));
            format!(
                "channel:{}:{}",
                team(*t),
                channel.map(|c| c.id.as_str()).unwrap_or("")
            )
        }
        Line::HiddenChannels(t) => format!("hidden:{}", team(*t)),
        Line::LoadingChannels(t) => format!("loading:{}", team(*t)),
        Line::FailedChannels(t) => format!("failed:{}", team(*t)),
        Line::NoChannels(t) => format!("empty:{}", team(*t)),
        Line::Note(text) => format!("note:{text}"),
        Line::Problem(text) => format!("problem:{text}"),
        other => format!("{other:?}"),
    }
}

/// The part of the old list to replace, and by how many items, to turn `old` into `new`.
pub(crate) fn splice_range(old: &[String], new: &[String]) -> Option<(Range<usize>, usize)> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    Some((prefix..old.len() - suffix, new.len() - prefix - suffix))
}
fn sync(state: &ListState, old: &mut Vec<String>, new: Vec<String>) {
    if let Some((range, count)) = splice_range(old, &new) {
        state.splice(range, count);
        *old = new;
    }
}

/// Teams can be reordered: the service gave an order, none is being saved, and the list is in its own order.
fn can_move(sidebar: &Sidebar, searching: bool) -> bool {
    let c = &sidebar.team_controls;
    c.order_available && !c.saving_order && c.sort == TeamSort::Teams && !searching
}
fn can_shift(sidebar: &Sidebar, team: &TeamRow, up: bool, searching: bool) -> bool {
    if !can_move(sidebar, searching) {
        return false;
    }
    let mut peers = sidebar.teams.iter().filter(|t| t.hidden == team.hidden);
    let end = if up { peers.next() } else { peers.next_back() };
    end.is_some_and(|t| t.id != team.id)
}

pub fn sidebar(this: &mut Oxide, _window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let loading = loading(this);
    let core = this.core.clone();
    let header = div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .pt(px(16.))
        .pl(px(16.))
        .pr(px(12.))
        .pb(px(10.))
        .child(
            div()
                .flex_1()
                .text_size(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.sidebar_view.teams")),
        )
        .child(
            widgets::icon_button_sized(
                "teams-add",
                Icon::Plus,
                34.,
                15.,
                this.popup_open(ADD_MENU),
                &theme,
            )
            .tooltip(widgets::tooltip(tr("ui.teams_admin.join_or_create"), theme))
            .on_click(cx.listener(|this, e: &ClickEvent, _, cx| {
                open_menu(this, ADD_MENU, e.position(), cx)
            })),
        )
        .child(
            div()
                .id("teams-refresh")
                .tooltip(widgets::tooltip(tr("ui.sidebar_view.refresh_teams"), theme))
                .child(if loading {
                    div()
                        .size(px(34.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(widgets::spinner("teams-refresh-spin", 16., theme.muted))
                        .into_any_element()
                } else {
                    widgets::icon_button_sized(
                        "teams-refresh-button",
                        Icon::Refresh,
                        34.,
                        16.,
                        false,
                        &theme,
                    )
                    .on_click(move |_, _, _| core.send(Action::Refresh))
                    .into_any_element()
                }),
        );
    let column = div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme.surface)
        .child(header);
    let Some(sidebar) = this.sidebar.as_ref() else {
        return column.into_any_element();
    };
    let controls = sidebar.team_controls.clone();
    let lines = lines(sidebar, searching(this), loading);
    let keys = lines.iter().map(|l| key(l, sidebar)).collect();
    sync(&this.ui.teams.list, &mut this.ui.teams.keys, keys);

    let counts = div()
        .flex_none()
        .flex()
        .items_center()
        .px_4()
        .child(
            div()
                .flex_1()
                .text_size(px(11.))
                .text_color(theme.muted)
                .child(trf(
                    "ui.teams_list.shown_hidden",
                    &[
                        ("shownCount", &controls.shown_count.to_string()),
                        ("hiddenCount", &controls.hidden_count.to_string()),
                    ],
                )),
        )
        .child(
            widgets::icon_button_sized(
                "teams-sort",
                Icon::Sort,
                28.,
                14.,
                this.popup_open(SORT_MENU),
                &theme,
            )
            .tooltip(widgets::tooltip(tr("ui.teams_list.sort_teams"), theme))
            .on_click(cx.listener(|this, e: &ClickEvent, _, cx| {
                open_menu(this, SORT_MENU, e.position(), cx)
            })),
        );
    let entity = cx.entity();
    let lines = Rc::new(lines);
    let rows = list(this.ui.teams.list.clone(), move |ix, window, cx| {
        let lines = lines.clone();
        entity.update(cx, |this, cx| {
            let body = match lines.get(ix) {
                Some(line) => line_element(this, line, window, cx),
                None => div().into_any_element(),
            };
            // The list's own padding, and the stack's spacing; the last line closes the list.
            div()
                .px_2()
                .pb(px(2.))
                .when(ix == 0, |d| d.pt(px(10.)))
                .when(ix + 1 == lines.len(), |d| d.pb(px(10.)))
                .child(body)
                .into_any_element()
        })
    })
    .flex_1();
    column.child(counts).child(rows).into_any_element()
}

fn line_element(
    this: &mut Oxide,
    line: &Line,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let Some(sidebar) = this.sidebar.clone() else {
        return div().into_any_element();
    };
    let searching = searching(this);
    match line {
        Line::Team(t) => match sidebar.teams.get(*t) {
            Some(team) => div()
                .pt(px(1.))
                .child(team_row(this, &sidebar, team, searching, cx))
                .into_any_element(),
            None => div().into_any_element(),
        },
        Line::Channel(t, c) => match sidebar.teams.get(*t).and_then(|t| t.channels.get(*c)) {
            Some(channel) => channel_row(this, channel, cx),
            None => div().into_any_element(),
        },
        Line::HiddenChannels(t) => {
            let Some(team) = sidebar.teams.get(*t) else {
                return div().into_any_element();
            };
            let count = team.hidden_channel_count.to_string();
            let label = if team.show_hidden_channels {
                trf(
                    "ui.teams_list.hide_hidden_channels",
                    &[("hiddenChannelCount", &count)],
                )
            } else {
                trf(
                    "ui.teams_list.see_hidden_channels",
                    &[("hiddenChannelCount", &count)],
                )
            };
            let action = TeamListAction::HiddenChannels {
                team: team.id.clone(),
                expanded: !team.show_hidden_channels,
            };
            let core = this.core.clone();
            div()
                .pl(px(36.))
                .py(px(5.))
                .flex()
                .child(
                    ghost(
                        SharedString::from(format!("hidden-channels-{}", team.id)),
                        label,
                        11.,
                        true,
                        &theme,
                    )
                    .on_click(move |_, _, _| {
                        core.send(Action::TeamList {
                            action: action.clone(),
                        })
                    }),
                )
                .into_any_element()
        }
        Line::LoadingChannels(t) => div()
            .pl(px(36.))
            .py(px(6.))
            .text_size(px(11.))
            .child(widgets::loading(
                SharedString::from(format!("channels-loading-{t}")),
                tr("ui.teams_list.loading_channels"),
                &theme,
            ))
            .into_any_element(),
        Line::FailedChannels(t) => {
            let id = sidebar
                .teams
                .get(*t)
                .map(|t| t.id.clone())
                .unwrap_or_default();
            let core = this.core.clone();
            div()
                .pl(px(36.))
                .py(px(6.))
                .flex()
                .flex_col()
                .items_start()
                .gap_1()
                .child(widgets::muted(
                    tr("ui.teams_list.channels_couldn_t_be_refreshed"),
                    11.,
                    &theme,
                ))
                .child(
                    ghost(
                        SharedString::from(format!("channels-retry-{id}")),
                        tr("ui.teams_list.try_again"),
                        13.,
                        true,
                        &theme,
                    )
                    .on_click(move |_, _, _| core.send(Action::LoadChannels { id: id.clone() })),
                )
                .into_any_element()
        }
        Line::NoChannels(_) => div()
            .pl(px(36.))
            .py(px(6.))
            .child(widgets::muted(
                if searching {
                    tr("ui.teams_list.no_matching_channels")
                } else {
                    tr("ui.teams_list.no_channels_available")
                },
                11.,
                &theme,
            ))
            .into_any_element(),
        Line::AllHidden => div()
            .p_3()
            .child(widgets::muted(
                tr("ui.teams_list.all_your_teams_are_hidden"),
                12.,
                &theme,
            ))
            .into_any_element(),
        Line::HiddenTeams => {
            let controls = &sidebar.team_controls;
            let show = controls.show_hidden;
            let core = this.core.clone();
            let hover = theme.input;
            div()
                .pt_2()
                .child(
                    div()
                        .id("hidden-teams")
                        .p(px(10.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_color(theme.muted)
                        .hover(move |s| s.bg(hover))
                        .child(icon(
                            if show {
                                Icon::ChevronDown
                            } else {
                                Icon::ChevronRight
                            },
                            px(11.),
                            theme.muted,
                        ))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(trf(
                                    "ui.teams_list.hidden_teams",
                                    &[("hiddenCount", &controls.hidden_count.to_string())],
                                )),
                        )
                        .on_click(move |_, _, _| {
                            core.send(Action::TeamList {
                                action: TeamListAction::HiddenTeams { expanded: !show },
                            })
                        }),
                )
                .into_any_element()
        }
        Line::Skeleton => skeleton(6, 28., &theme),
        Line::Note(text) => div()
            .p_4()
            .child(widgets::muted(text.clone(), 13., &theme))
            .into_any_element(),
        Line::Problem(problem) => {
            let core = this.core.clone();
            div()
                .id("teams-problem")
                .p_3()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(6.))
                .tooltip(widgets::tooltip(problem.clone(), theme))
                .child(widgets::muted(
                    tr("ui.teams_list.the_team_list_couldn_t_be"),
                    11.,
                    &theme,
                ))
                .child(
                    widgets::button(
                        "teams-problem-retry",
                        tr("ui.teams_list.try_again"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(move |_, _, _| core.send(Action::Refresh)),
                )
                .into_any_element()
        }
        Line::SavingOrder => div()
            .p_3()
            .text_size(px(11.))
            .child(widgets::loading(
                "teams-saving-order",
                tr("ui.teams_list.saving_order"),
                &theme,
            ))
            .into_any_element(),
    }
}

/// A team being dragged to a new place in the list.
#[derive(Clone)]
struct TeamDrag {
    id: String,
    name: String,
    hidden: bool,
    theme: Theme,
}
impl Render for TeamDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1p5()
            .rounded(px(7.))
            .bg(self.theme.surface)
            .border_1()
            .border_color(self.theme.border_strong)
            .shadow_md()
            .font_family(crate::oxide::FONT)
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(self.theme.text)
            .child(self.name.clone())
    }
}

fn team_description(team: &TeamRow) -> String {
    let mut values = vec![team.name.clone()];
    if team.hidden {
        values.push(tr("ui.teams_list.hidden"));
    }
    if team.unread {
        values.push(tr("ui.teams_list.unread"));
    }
    if team.archived {
        values.push(tr("ui.teams_list.archived"));
    }
    values.join(" · ")
}

fn team_row(
    this: &mut Oxide,
    sidebar: &Sidebar,
    team: &TeamRow,
    searching: bool,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let avatar = this.avatar(&team.id, &team.name, 30., AvatarShape::Square, None);
    let menu_key: SharedString = format!("{TEAM_MENU}{}", team.id).into();
    let menu_open = this.popup_open(&menu_key);
    let group: SharedString = format!("team-row-{}", team.id).into();
    let movable = can_move(sidebar, searching);
    let core = this.core.clone();
    let id = team.id.clone();
    let main = div()
        .id(SharedString::from(format!("team-{}", team.id)))
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .py_2()
        .pl(px(6.))
        .cursor_pointer()
        .child(div().w(px(12.)).flex().justify_center().child(icon(
            if team.show_channels {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            px(11.),
            theme.subtle,
        )))
        .child(avatar)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .font_weight(if team.unread {
                    FontWeight::BOLD
                } else {
                    FontWeight::SEMIBOLD
                })
                .child(team.name.clone()),
        )
        // Collapsed: the channels' dots are out of sight.
        .when(team.unread && !team.show_channels, |d| {
            d.child(
                div()
                    .flex_none()
                    .size(px(8.))
                    .rounded_full()
                    .bg(theme.accent),
            )
        })
        .on_click(move |_, _, _| core.send(Action::ToggleTeam { id: id.clone() }));
    let more = more_button(
        SharedString::from(format!("team-more-{}", team.id)),
        26.,
        28.,
        &group,
        menu_open,
        team.pending,
        trf("ui.teams_list.options_for_team", &[("name", &team.name)]),
        &theme,
    )
    .when(!team.pending, |d| {
        let key = menu_key.clone();
        d.on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
            open_menu(this, key.clone(), e.position(), cx)
        }))
    });
    let hover = theme.input;
    let drop_bg = theme.accent_soft;
    let key = menu_key.clone();
    let pending = team.pending;
    div()
        .id(SharedString::from(format!("team-line-{}", team.id)))
        .group(group)
        .flex()
        .items_center()
        .rounded(px(7.))
        .when(menu_open, |d| d.bg(hover))
        .map(|d| crate::motion::hover_fill(d, format!("team-line-{}", team.id), hover, px(7.)))
        .tooltip(widgets::tooltip(team_description(team), theme))
        .child(main)
        .child(more)
        .when(!pending, |d| {
            d.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    open_menu(this, key.clone(), e.position, cx)
                }),
            )
        })
        .when(movable, |d| {
            let drag = TeamDrag {
                id: team.id.clone(),
                name: team.name.clone(),
                hidden: team.hidden,
                theme,
            };
            let before = team.id.clone();
            let hidden = team.hidden;
            d.on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                .drag_over::<TeamDrag>(move |s, drag, _, _| {
                    if drag.hidden == hidden {
                        s.bg(drop_bg)
                    } else {
                        s
                    }
                })
                .on_drop(cx.listener(move |this, drag: &TeamDrag, _, _| {
                    // Only among teams of the same kind, as Swift's `peers`.
                    if drag.id != before && drag.hidden == hidden {
                        this.send(Action::TeamList {
                            action: TeamListAction::Move {
                                id: drag.id.clone(),
                                before: Some(before.clone()),
                            },
                        });
                    }
                }))
        })
        .into_any_element()
}

fn channel_description(channel: &ChannelRow) -> String {
    let kind = channel.kind.to_lowercase();
    let mut values = vec![channel.name.clone()];
    if kind == "private" {
        values.push(tr("ui.teams_list.private_channel"));
    }
    if kind == "shared" {
        values.push(tr("ui.teams_list.shared_channel"));
    }
    if channel.hidden {
        values.push(tr("ui.teams_list.hidden_2"));
    }
    if channel.unread {
        values.push(tr("ui.teams_list.unread"));
    }
    if channel.archived {
        values.push(tr("ui.teams_list.archived_2"));
    }
    values.join(" · ")
}

fn channel_row(this: &mut Oxide, channel: &ChannelRow, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let menu_key: SharedString = format!("{CHANNEL_MENU}{}|{}", channel.team, channel.id).into();
    let menu_open = this.popup_open(&menu_key);
    let group: SharedString = format!("channel-row-{}-{}", channel.team, channel.id).into();
    let kind = channel.kind.to_lowercase();
    let glyph = match kind.as_str() {
        "private" => Icon::Lock,
        "shared" => Icon::Branch,
        _ => Icon::Hash,
    };
    let core = this.core.clone();
    let target = Target {
        team: Some(channel.team.clone()),
        id: channel.id.clone(),
    };
    let main = div()
        .id(SharedString::from(format!(
            "channel-{}-{}",
            channel.team, channel.id
        )))
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(7.))
        .pl_2()
        .py(px(7.))
        .cursor_pointer()
        .text_color(if channel.hidden {
            theme.muted
        } else {
            theme.text
        })
        .child(div().w(px(14.)).flex().justify_center().child(icon(
            glyph,
            px(13.),
            if channel.selected {
                theme.accent
            } else {
                theme.subtle
            },
        )))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .when(channel.unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                .child(channel.name.clone()),
        )
        .when(channel.muted, |d| {
            d.child(icon(Icon::BellOff, px(11.), theme.subtle))
        })
        .when(channel.unread, |d| {
            d.child(
                div()
                    .flex_none()
                    .size(px(8.))
                    .rounded_full()
                    .bg(theme.accent),
            )
        })
        .on_click(move |_, _, _| {
            core.send(Action::Select {
                target: target.clone(),
            })
        });
    let more = more_button(
        SharedString::from(format!("channel-more-{}-{}", channel.team, channel.id)),
        24.,
        26.,
        &group,
        menu_open,
        channel.pending,
        trf(
            "ui.teams_list.options_for_channel",
            &[("name", &channel.name)],
        ),
        &theme,
    )
    .when(!channel.pending, |d| {
        let key = menu_key.clone();
        d.on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
            open_menu(this, key.clone(), e.position(), cx)
        }))
    });
    let hover = theme.input;
    let key = menu_key.clone();
    div()
        .pl(px(28.))
        .child(
            div()
                .id(SharedString::from(format!(
                    "channel-line-{}-{}",
                    channel.team, channel.id
                )))
                .group(group)
                .flex()
                .items_center()
                .rounded(px(7.))
                .when(channel.selected, |d| d.bg(theme.accent_soft))
                .when(!channel.selected, |d| d.when(menu_open, |d| d.bg(hover)))
                .map(|d| {
                    if channel.selected {
                        d
                    } else {
                        let key = format!("channel-line-{}-{}", channel.team, channel.id);
                        crate::motion::hover_fill(d, key, hover, px(7.))
                    }
                })
                .tooltip(widgets::tooltip(channel_description(channel), theme))
                .child(main)
                .child(more)
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        open_menu(this, key.clone(), e.position, cx)
                    }),
                ),
        )
        .into_any_element()
}

/// The "…" of a row: shown while the row is hovered or its menu is open, as Swift's `showOptions`.
#[allow(clippy::too_many_arguments)]
fn more_button(
    id: SharedString,
    width: f32,
    height: f32,
    group: &SharedString,
    open: bool,
    disabled: bool,
    help: String,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let hover = theme.border;
    div()
        .id(id)
        .w(px(width))
        .h(px(height))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .when(!open, |d| {
            d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.))
        })
        .when(disabled, |d| {
            d.group_hover(group.clone(), |s| s.opacity(0.4))
        })
        .when(!disabled, |d| {
            d.cursor_pointer().hover(move |s| s.bg(hover))
        })
        .tooltip(widgets::tooltip(help, *theme))
        .child(icon(Icon::More, px(15.), theme.muted))
}

/// A borderless text button (`GhostButtonStyle`), in the accent colour or muted.
pub(crate) fn ghost(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    size: f32,
    accent: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let hover = theme.input;
    div()
        .id(id)
        .px_1p5()
        .py(px(3.))
        .rounded(px(8.))
        .text_size(px(size))
        .text_color(if accent {
            theme.accent_text
        } else {
            theme.muted
        })
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(label.into())
}

/// Grey bars standing in for list rows while the first page loads (`SkeletonList`).
pub(crate) fn skeleton(rows: usize, avatar: f32, theme: &Theme) -> AnyElement {
    crate::widgets::skeleton("teams-skeleton", rows, avatar, theme).into_any_element()
}

// Menus: one at a time through `Oxide::popup`, drawn at the click that opened them.
const ADD_MENU: &str = "teams:add";
const SORT_MENU: &str = "teams:sort";
const TEAM_MENU: &str = "teams:team:";
const CHANNEL_MENU: &str = "teams:channel:";

fn open_menu(
    this: &mut Oxide,
    key: impl Into<SharedString>,
    at: Point<Pixels>,
    cx: &mut Context<Oxide>,
) {
    let key = key.into();
    // The click outside that closed this very menu landed on its trigger: leave it closed.
    if let Some((closed, when)) = this.ui.teams.just_closed.take()
        && closed == key
        && when.elapsed() < Duration::from_millis(400)
    {
        return;
    }
    this.ui.teams.menu_at = at;
    this.popup = Some(key);
    cx.notify();
}

fn close_menu(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if let Some(key) = this.popup.clone()
        && key.starts_with("teams:")
    {
        this.ui.teams.just_closed = Some((key, Instant::now()));
        this.close_popup(cx);
    }
}

fn menu(this: &Oxide, items: Vec<AnyElement>, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    deferred(
        anchored()
            .position(this.ui.teams.menu_at)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("teams-menu")
                    .occlude()
                    .mt_1()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .shadow_lg()
                    .py_1()
                    .font_family(crate::oxide::FONT)
                    .text_color(theme.text)
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| close_menu(this, cx)))
                    .children(items),
            ),
    )
    .with_priority(1)
    .into_any_element()
}

/// A menu line that closes the menu, then runs `run`; faded and inert when disabled.
#[allow(clippy::too_many_arguments)]
fn item(
    id: impl Into<SharedString>,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
    enabled: bool,
    danger: bool,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    run: impl Fn(&mut Oxide, &mut Window, &mut Context<Oxide>) + 'static,
) -> AnyElement {
    let id: SharedString = id.into();
    widgets::menu_item(id, glyph, label, theme)
        .when(danger, |d| d.text_color(theme.danger))
        .when(!enabled, |d| d.opacity(0.45).cursor_default())
        .when(enabled, |d| {
            d.on_click(cx.listener(move |this, _, window, cx| {
                this.close_popup(cx);
                run(this, window, cx);
            }))
        })
        .into_any_element()
}
fn separator(theme: &Theme) -> AnyElement {
    div().my_1().h(px(1.)).bg(theme.border).into_any_element()
}
fn send(action: Action) -> impl Fn(&mut Oxide, &mut Window, &mut Context<Oxide>) + 'static {
    move |this, _, _| this.send(action.clone())
}
fn list_action(
    action: TeamListAction,
) -> impl Fn(&mut Oxide, &mut Window, &mut Context<Oxide>) + 'static {
    send(Action::TeamList { action })
}

fn open_menu_element(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let key = this.popup.clone()?;
    if !key.starts_with("teams:") {
        return None;
    }
    let theme = this.theme;
    let sidebar = this.sidebar.clone()?;
    let items = if key == ADD_MENU {
        vec![
            item(
                "teams-add-create",
                Some(Icon::Plus),
                tr("ui.teams_admin.create_team"),
                true,
                false,
                &theme,
                cx,
                teams_admin::open_new_team,
            ),
            item(
                "teams-add-gallery",
                Some(Icon::People),
                tr("ui.teams_admin.join_or_create"),
                true,
                false,
                &theme,
                cx,
                send(Action::TeamsAdmin {
                    action: teams_core::TeamsAdminAction::OpenGallery,
                }),
            ),
        ]
    } else if key == SORT_MENU {
        let sort = sidebar.team_controls.sort;
        vec![
            item(
                "teams-sort-teams",
                Some(if sort == TeamSort::Teams {
                    Icon::Check
                } else {
                    Icon::ListBullet
                }),
                tr("ui.teams_list.teams_order"),
                true,
                false,
                &theme,
                cx,
                list_action(TeamListAction::Sort {
                    sort: TeamSort::Teams,
                }),
            ),
            item(
                "teams-sort-alpha",
                Some(if sort == TeamSort::Alphabetical {
                    Icon::Check
                } else {
                    Icon::Format
                }),
                tr("ui.teams_list.a_to_z"),
                true,
                false,
                &theme,
                cx,
                list_action(TeamListAction::Sort {
                    sort: TeamSort::Alphabetical,
                }),
            ),
        ]
    } else if let Some(id) = key.strip_prefix(TEAM_MENU) {
        let team = sidebar.teams.iter().find(|t| t.id == id)?.clone();
        team_menu(this, &sidebar, &team, cx)
    } else if let Some(rest) = key.strip_prefix(CHANNEL_MENU) {
        let (team, channel) = rest.split_once('|')?;
        let channel = sidebar
            .teams
            .iter()
            .find(|t| t.id == team)?
            .channels
            .iter()
            .find(|c| c.id == channel)?
            .clone();
        channel_menu(&channel, &theme, cx)
    } else {
        return None;
    };
    Some(menu(this, items, cx))
}

/// `TeamsList.teamMenu`.
fn team_menu(
    this: &Oxide,
    sidebar: &Sidebar,
    team: &TeamRow,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let theme = this.theme;
    let searching = searching(this);
    let id = team.id.clone();
    let mut items = vec![
        item(
            "team-menu-show",
            Some(if team.hidden { Icon::Eye } else { Icon::Hide }),
            if team.hidden {
                tr("ui.teams_list.show_team")
            } else {
                tr("ui.teams_list.hide_team")
            },
            !team.pending,
            false,
            &theme,
            cx,
            list_action(TeamListAction::Show {
                team: id.clone(),
                channel: None,
                shown: team.hidden,
            }),
        ),
        {
            let (show, expanded, id) = (team.show_hidden_channels, team.expanded, id.clone());
            item(
                "team-menu-hidden-channels",
                None,
                if show {
                    tr("ui.teams_list.hide_the_list_of_hidden_channels")
                } else {
                    tr("ui.teams_list.see_all_hidden_channels")
                },
                true,
                false,
                &theme,
                cx,
                move |this, _, _| {
                    if !show && !expanded {
                        this.send(Action::ToggleTeam { id: id.clone() });
                    }
                    this.send(Action::TeamList {
                        action: TeamListAction::HiddenChannels {
                            team: id.clone(),
                            expanded: !show,
                        },
                    });
                },
            )
        },
        separator(&theme),
        item(
            "team-menu-up",
            Some(Icon::ChevronUp),
            tr("ui.teams_list.move_up"),
            can_shift(sidebar, team, true, searching),
            false,
            &theme,
            cx,
            list_action(TeamListAction::Shift {
                id: id.clone(),
                up: true,
            }),
        ),
        item(
            "team-menu-down",
            Some(Icon::ChevronDown),
            tr("ui.teams_list.move_down"),
            can_shift(sidebar, team, false, searching),
            false,
            &theme,
            cx,
            list_action(TeamListAction::Shift {
                id: id.clone(),
                up: false,
            }),
        ),
        separator(&theme),
        item(
            "team-menu-refresh",
            Some(Icon::Refresh),
            tr("ui.teams_list.refresh_channels"),
            true,
            false,
            &theme,
            cx,
            send(Action::LoadChannels { id: id.clone() }),
        ),
    ];
    if !team.archived {
        let (new_id, new_name) = (team.id.clone(), team.name.clone());
        let (leave_id, leave_name) = (team.id.clone(), team.name.clone());
        items.extend([
            separator(&theme),
            item(
                "team-menu-channel",
                Some(Icon::Plus),
                tr("ui.teams_admin.add_channel"),
                true,
                false,
                &theme,
                cx,
                move |this, window, cx| {
                    teams_admin::open_new_channel(this, &new_id, &new_name, window, cx)
                },
            ),
            item(
                "team-menu-code",
                Some(Icon::Link),
                tr("ui.teams_admin.team_code"),
                true,
                false,
                &theme,
                cx,
                send(Action::TeamsAdmin {
                    action: teams_core::TeamsAdminAction::ShowJoinCode {
                        team_id: id.clone(),
                    },
                }),
            ),
            item(
                "team-menu-leave",
                Some(Icon::Leave),
                tr("ui.teams_admin.leave_team"),
                true,
                true,
                &theme,
                cx,
                move |this, _, cx| {
                    this.ui.teams.admin.leaving = Some((leave_id.clone(), leave_name.clone()));
                    cx.notify();
                },
            ),
        ]);
    }
    items
}

/// `TeamChannelRow.options`.
fn channel_menu(channel: &ChannelRow, theme: &Theme, cx: &mut Context<Oxide>) -> Vec<AnyElement> {
    let mut items = Vec::new();
    if channel.archived {
        items.push(
            div()
                .mx_1()
                .px_2()
                .h(px(26.))
                .flex()
                .items_center()
                .text_size(px(12.))
                .text_color(theme.muted)
                .child(tr("ui.teams_list.archived_channel"))
                .into_any_element(),
        );
    }
    let (team, id) = (channel.team.clone(), channel.id.clone());
    items.push(item(
        "channel-menu-show",
        Some(if channel.hidden {
            Icon::Eye
        } else {
            Icon::Hide
        }),
        if channel.hidden {
            tr("ui.teams_list.show_channel")
        } else {
            tr("ui.teams_list.hide_channel")
        },
        !channel.pending,
        false,
        theme,
        cx,
        list_action(TeamListAction::Show {
            team: team.clone(),
            channel: Some(id.clone()),
            shown: channel.hidden,
        }),
    ));
    items.push(separator(theme));
    // `ChannelNotificationMenu`.
    items.push(item(
        "channel-menu-notifications",
        Some(Icon::Bell),
        tr("ui.notification_settings_view.channel_notifications"),
        true,
        false,
        theme,
        cx,
        send(Action::Notifications {
            action: NotificationsAction::EditChannel {
                team: team.clone(),
                channel: id.clone(),
            },
        }),
    ));
    if !channel.muted {
        items.push(item(
            "channel-menu-mute",
            Some(Icon::BellOff),
            tr("ui.notification_settings_view.turn_off_channel_notifications"),
            true,
            false,
            theme,
            cx,
            send(Action::Notifications {
                action: NotificationsAction::MuteChannel { team, channel: id },
            }),
        ));
    }
    items
}

/// `TeamsOverview`: the content while no channel is open.
pub fn overview(this: &mut Oxide, _window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let teams = this.sidebar.as_ref().map(|s| s.teams.len()).unwrap_or(0);
    // Item 0 is the heading (with the empty or failed state), then one card per team.
    let mut keys = vec![format!(
        "head:{}:{}",
        teams == 0,
        this.sidebar
            .as_ref()
            .and_then(|s| s.teams_problem.clone())
            .unwrap_or_default()
    )];
    if let Some(sidebar) = &this.sidebar {
        keys.extend(
            sidebar
                .teams
                .iter()
                .map(|t| format!("{}:{}", t.id, t.channels.len())),
        );
    }
    let count = keys.len();
    sync(
        &this.ui.teams.overview,
        &mut this.ui.teams.overview_keys,
        keys,
    );
    let entity = cx.entity();
    list(this.ui.teams.overview.clone(), move |ix, _, cx| {
        entity.update(cx, |this, _| {
            let body = if ix == 0 {
                overview_head(this)
            } else {
                overview_card(this, ix - 1)
            };
            div()
                .px(px(28.))
                .pb_4()
                .when(ix == 0, |d| d.pt(px(28.)))
                .when(ix + 1 == count, |d| d.pb(px(28.)))
                .child(body)
                .into_any_element()
        })
    })
    .size_full()
    .into_any_element()
}

fn overview_head(this: &mut Oxide) -> AnyElement {
    let theme = this.theme;
    let mut head = div()
        .flex()
        .flex_col()
        .items_start()
        .gap_4()
        .child(
            div()
                .text_size(px(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.screens.your_teams")),
        )
        .child(widgets::muted(
            tr("ui.screens.open_a_channel_to_see_its"),
            14.,
            &theme,
        ));
    if let Some(sidebar) = &this.sidebar
        && sidebar.teams.is_empty()
    {
        head = head.child(widgets::muted(
            if loading(this) {
                tr("ui.screens.loading_teams")
            } else if sidebar.teams_problem.is_some() {
                tr("ui.screens.the_teams_service_couldn_t_be")
            } else {
                tr("ui.screens.no_teams_available_for_this_account")
            },
            14.,
            &theme,
        ));
        if let Some(problem) = &sidebar.teams_problem {
            let core = this.core.clone();
            head = head
                .child(widgets::muted(problem.clone(), 13., &theme))
                .child(
                    widgets::button(
                        "overview-retry",
                        tr("ui.screens.try_again"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(move |_, _, _| core.send(Action::Refresh)),
                );
        }
    }
    head.into_any_element()
}

fn overview_card(this: &mut Oxide, ix: usize) -> AnyElement {
    let theme = this.theme;
    let Some(team) = this.sidebar.as_ref().and_then(|s| s.teams.get(ix)).cloned() else {
        return div().into_any_element();
    };
    widgets::card(&theme)
        .p(px(18.))
        .flex()
        .flex_col()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(team.name.clone()),
        )
        .when(!team.description.is_empty(), |d| {
            d.child(widgets::muted(team.description.clone(), 13., &theme))
        })
        .children(team.channels.iter().map(|channel| {
            let core = this.core.clone();
            let target = Target {
                team: Some(channel.team.clone()),
                id: channel.id.clone(),
            };
            div().py(px(2.)).child(
                ghost(
                    SharedString::from(format!("overview-{}-{}", channel.team, channel.id)),
                    channel.name.clone(),
                    14.,
                    true,
                    &theme,
                )
                .on_click(move |_, _, _| {
                    core.send(Action::Select {
                        target: target.clone(),
                    })
                }),
            )
        }))
        .into_any_element()
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    let mut layers: Vec<AnyElement> = teams_admin::overlays(this, window, cx);
    layers.extend(open_menu_element(this, cx));
    match layers.len() {
        0 => None,
        1 => layers.pop(),
        // Laid over the whole window, as each dialog's dimmed backdrop expects.
        _ => Some(
            div()
                .absolute()
                .inset_0()
                .children(layers)
                .into_any_element(),
        ),
    }
}

/// A step of `OXIDE_SCRIPT` this module understands (see `screens::run_script`); false if not its own.
/// `teams:toggle <n>` opens or closes the n-th team, `teams:open <team> <channel>` opens a channel,
/// `teams:menu <n>` / `teams:channel-menu <team> <channel>` / `teams:add` / `teams:sort` open a menu,
/// `teams:new-team`, `teams:new-channel <n>`, `teams:gallery`, `teams:code <n>`, `teams:leave <n>` open a dialog,
/// `teams:hidden` shows the hidden teams, `teams:hide <n>` hides the n-th team.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("teams:") else {
        return false;
    };
    let mut words = rest.split_whitespace();
    let verb = words.next().unwrap_or("");
    let numbers: Vec<usize> = words.filter_map(|w| w.parse().ok()).collect();
    let n = |i: usize| numbers.get(i).copied().unwrap_or(0);
    let team = this
        .sidebar
        .as_ref()
        .and_then(|s| s.teams.get(n(0)))
        .cloned();
    let at = point(px(420.), px(200.));
    match (verb, team) {
        ("toggle", Some(team)) => this.send(Action::ToggleTeam { id: team.id }),
        ("open", Some(team)) => {
            let Some(channel) = team.channels.get(n(1)) else {
                return false;
            };
            this.send(Action::Select {
                target: Target {
                    team: Some(team.id.clone()),
                    id: channel.id.clone(),
                },
            });
        }
        ("menu", Some(team)) => open_menu(this, format!("{TEAM_MENU}{}", team.id), at, cx),
        ("channel-menu", Some(team)) => {
            let Some(channel) = team.channels.get(n(1)) else {
                return false;
            };
            open_menu(
                this,
                format!("{CHANNEL_MENU}{}|{}", team.id, channel.id),
                at,
                cx,
            )
        }
        ("add", _) => open_menu(this, ADD_MENU, point(px(300.), px(100.)), cx),
        ("sort", _) => open_menu(this, SORT_MENU, point(px(300.), px(130.)), cx),
        ("new-team", _) => teams_admin::open_new_team(this, window, cx),
        ("new-channel", Some(team)) => {
            teams_admin::open_new_channel(this, &team.id, &team.name, window, cx)
        }
        ("gallery", _) => this.send(Action::TeamsAdmin {
            action: teams_core::TeamsAdminAction::OpenGallery,
        }),
        ("code", Some(team)) => this.send(Action::TeamsAdmin {
            action: teams_core::TeamsAdminAction::ShowJoinCode { team_id: team.id },
        }),
        ("leave", Some(team)) => this.ui.teams.admin.leaving = Some((team.id, team.name)),
        ("hide", Some(team)) => this.send(Action::TeamList {
            action: TeamListAction::Show {
                team: team.id,
                channel: None,
                shown: false,
            },
        }),
        ("hidden", _) => this.send(Action::TeamList {
            action: TeamListAction::HiddenTeams { expanded: true },
        }),
        // What the column has, on stderr.
        ("dump", _) => eprintln!(
            "teams: nav {:?}, selected {:?}, teams {:?}",
            this.shell.as_ref().map(|s| s.nav),
            this.shell.as_ref().map(|s| s.selected.clone()),
            this.sidebar.as_ref().map(|s| s
                .teams
                .iter()
                .map(|t| (t.name.clone(), t.channels.len()))
                .collect::<Vec<_>>())
        ),
        _ => return false,
    }
    cx.notify();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn splices_only_the_changed_middle() {
        let old = keys(&["a", "b", "c", "d"]);
        assert_eq!(splice_range(&old, &old), None);
        assert_eq!(
            splice_range(&old, &keys(&["a", "x", "y", "c", "d"])),
            Some((1..2, 2))
        );
        assert_eq!(splice_range(&old, &keys(&["a", "d"])), Some((1..3, 0)));
        assert_eq!(splice_range(&[], &keys(&["a"])), Some((0..0, 1)));
        assert_eq!(
            splice_range(&keys(&["a", "a"]), &keys(&["a"])),
            Some((1..2, 0))
        );
    }

    fn channel(id: &str, hidden: bool) -> ChannelRow {
        ChannelRow {
            team: "t".into(),
            id: id.into(),
            name: id.into(),
            selected: false,
            hidden,
            kind: "standard".into(),
            archived: false,
            pending: false,
            muted: false,
            unread: false,
        }
    }
    fn team(id: &str, hidden: bool, channels: Vec<ChannelRow>) -> TeamRow {
        TeamRow {
            hidden,
            pending: false,
            hidden_channel_count: channels.iter().filter(|c| c.hidden).count() as u32,
            show_hidden_channels: false,
            id: id.into(),
            name: id.into(),
            description: String::new(),
            archived: false,
            expanded: true,
            show_channels: true,
            channels,
            unread: false,
            loading: false,
            failed: false,
        }
    }
    fn sidebar(teams: Vec<TeamRow>, shown: u32, hidden: u32) -> Sidebar {
        Sidebar {
            activities: vec![],
            activity_loading: false,
            more_activity: false,
            activity_error: None,
            chats: vec![],
            team_count: teams.len() as u32,
            teams,
            team_controls: teams_core::TeamListControls {
                sort: TeamSort::Teams,
                shown_count: shown,
                hidden_count: hidden,
                show_hidden: false,
                saving_order: false,
                order_available: true,
            },
            more_chats: false,
            teams_problem: None,
        }
    }

    #[test]
    fn lines_follow_swift_order() {
        let s = sidebar(
            vec![
                team("a", false, vec![channel("c1", false), channel("c2", true)]),
                team("h", true, vec![]),
            ],
            1,
            1,
        );
        assert_eq!(
            lines(&s, false, false),
            vec![
                Line::Team(0),
                Line::Channel(0, 0),
                Line::HiddenChannels(0),
                Line::Channel(0, 1),
                Line::HiddenTeams,
                Line::Team(1),
                Line::NoChannels(1),
            ]
        );
        // A search drops the "hidden" toggles.
        assert_eq!(
            lines(&s, true, false),
            vec![
                Line::Team(0),
                Line::Channel(0, 0),
                Line::Channel(0, 1),
                Line::Team(1),
                Line::NoChannels(1)
            ]
        );
    }

    #[test]
    fn empty_and_loading_states() {
        let s = sidebar(vec![], 0, 0);
        assert_eq!(lines(&s, false, true), vec![Line::Skeleton]);
        assert_eq!(
            lines(&s, false, false),
            vec![Line::Note(tr("ui.teams_list.no_teams_available"))]
        );
        let hidden = sidebar(vec![team("h", true, vec![])], 0, 1);
        assert_eq!(
            lines(&hidden, false, false)[..2],
            [Line::AllHidden, Line::HiddenTeams]
        );
    }

    #[test]
    fn shifting_stays_among_peers() {
        let s = sidebar(
            vec![
                team("a", false, vec![]),
                team("b", false, vec![]),
                team("h", true, vec![]),
            ],
            2,
            1,
        );
        assert!(!can_shift(&s, &s.teams[0], true, false));
        assert!(can_shift(&s, &s.teams[0], false, false));
        assert!(!can_shift(&s, &s.teams[1], false, false));
        assert!(!can_shift(&s, &s.teams[2], true, false));
        assert!(!can_shift(&s, &s.teams[1], true, true));
    }
}
