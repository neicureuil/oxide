//! The window's frame (`MainView.swift`): top bar, navigation rail, the column of the section, banners and the screen.
use crate::icons::{Icon, icon};
use crate::oxide::{FONT, Oxide, tr};
use crate::widgets::{self, ButtonKind};
use crate::motion;
use crate::{
    activity, calendar, calls_screen, conversation, files, new_chat, screens, search, settings,
    sidebar, signin, teams,
};
use gpui::{
    AnimationExt, AnyElement, App, Context, FontWeight, KeyBinding, SharedString, Window, actions, div,
    prelude::*, px,
};
use std::sync::Arc;
use teams_core::{Action, Nav, OxideCore, Shell};

actions!(shell, [CopySelection, SelectMessage, DismissModal, FocusSearch, NewChat, Refresh, GoActivity, GoChat, GoTeams, GoCalendar, GoCalls, GoFiles, GoSettings]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        // Text fields bind their own Escape and ⌘C, which win while one has focus.
        KeyBinding::new("escape", DismissModal, None),
        KeyBinding::new("secondary-c", CopySelection, None),
        KeyBinding::new("secondary-a", SelectMessage, None),
        KeyBinding::new("secondary-e", FocusSearch, None),
        KeyBinding::new("secondary-n", NewChat, None),
        KeyBinding::new("secondary-r", Refresh, None),
        KeyBinding::new("secondary-1", GoActivity, None),
        KeyBinding::new("secondary-2", GoChat, None),
        KeyBinding::new("secondary-3", GoTeams, None),
        KeyBinding::new("secondary-4", GoCalendar, None),
        KeyBinding::new("secondary-5", GoCalls, None),
        KeyBinding::new("secondary-6", GoFiles, None),
        KeyBinding::new("secondary-,", GoSettings, None),
    ]);
}

/// The rail's sections, in order, as `RailCommands.items`.
pub fn rail_items() -> [(Nav, Icon, String); 6] {
    [
        (Nav::Activity, Icon::Bell, tr("ui.app.activity")),
        (Nav::Chat, Icon::Chat, tr("ui.app.chat")),
        (Nav::Teams, Icon::People, tr("ui.app.teams")),
        (Nav::Calendar, Icon::Calendar, tr("ui.app.calendar")),
        (Nav::Calls, Icon::Phone, tr("ui.app.calls")),
        (Nav::Files, Icon::File, tr("ui.app.files")),
    ]
}

pub fn root(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    widgets::reset_modals();
    let theme = this.theme;
    let base = div()
        .id("root")
        .relative()
        .size_full()
        .font_family(FONT)
        .text_size(px(13.))
        .line_height(px(18.))
        .text_color(theme.text)
        .bg(theme.background);
    let Some(shell) = this.shell.clone() else {
        return base.into_any_element();
    };
    if !shell.signed_in {
        let screen = signin::screen(this, &shell, cx);
        return base.child(motion::fade_in(div().size_full().child(screen), "signed-out")).into_any_element();
    }
    let main = div()
        .id("signed-in")
        .size_full()
        .flex()
        .flex_col()
        .child(top_bar(this, &shell, window, cx))
        .child(widgets::divider(&theme))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(rail(this, &shell))
                .child(div().w(px(1.)).h_full().bg(theme.border))
                .when(
                    matches!(shell.nav, Nav::Chat | Nav::Teams | Nav::Activity),
                    |d| {
                        let column = match shell.nav {
                            Nav::Teams => teams::sidebar(this, window, cx),
                            Nav::Activity => activity::sidebar(this, window, cx),
                            _ => sidebar::chats(this, cx),
                        };
                        let key = SharedString::from(format!("column-{:?}", shell.nav));
                        d.child(motion::fade_in(div().w(px(316.)).flex_none().h_full().child(column), key))
                            .child(div().w(px(1.)).h_full().bg(theme.border))
                    },
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .bg(theme.conversation)
                        .children(banners(this, &shell))
                        .child({
                            // Screens cross in with a fade (`ContentRouter`'s `.transition(.opacity)`); a
                            // conversation changing within Chat or Teams updates in place, as in Swift.
                            let key = SharedString::from(format!(
                                "content-{:?}-{}-{}",
                                shell.nav,
                                this.search.as_ref().is_some_and(|s| s.open),
                                this.new_chat.is_some()
                            ));
                            let screen = content(this, &shell, window, cx);
                            motion::fade_in(div().flex_1().min_h_0().child(screen), key)
                        }),
                ),
        );
    let overlays = screens::overlays(this, window, cx);
    let new_chat_core = this.core.clone();
    let refresh_core = this.core.clone();
    base.on_action(|_: &CopySelection, _, cx| {
        crate::feed_select::copy(cx);
    })
    .on_action(cx.listener(|_, _: &SelectMessage, _, cx| {
        if crate::feed_select::select_message() {
            cx.notify();
        }
    }))
    .on_action(cx.listener(|this, _: &DismissModal, window, cx| {
        if !widgets::dismiss_modal(window, cx) {
            this.close_popup(cx);
            // Escape also drops a text selection in the feed.
            if crate::feed_select::has_selection() {
                crate::feed_select::clear();
                cx.notify();
            }
        }
    }))
    .on_action(cx.listener(|this, _: &FocusSearch, window, cx| search::focus(this, window, cx)))
        .on_action(move |_: &NewChat, _, _| new_chat_core.send(Action::NewChat))
        .on_action(move |_: &Refresh, _, _| refresh_core.send(Action::Refresh))
        .on_action(nav_listener::<GoActivity>(this.core.clone(), Nav::Activity))
        .on_action(nav_listener::<GoChat>(this.core.clone(), Nav::Chat))
        .on_action(nav_listener::<GoTeams>(this.core.clone(), Nav::Teams))
        .on_action(nav_listener::<GoCalendar>(this.core.clone(), Nav::Calendar))
        .on_action(nav_listener::<GoCalls>(this.core.clone(), Nav::Calls))
        .on_action(nav_listener::<GoFiles>(this.core.clone(), Nav::Files))
        .on_action(nav_listener::<GoSettings>(this.core.clone(), Nav::Settings))
        .child(motion::fade_in(main, "signed-in-in"))
        .children(overlays)
        .into_any_element()
}

fn nav_listener<A: gpui::Action>(core: Arc<OxideCore>, nav: Nav) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    move |_, _, _| core.send(Action::Navigate { nav })
}

fn top_bar(this: &mut Oxide, shell: &Shell, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let core = this.core.clone();
    div()
        .h(px(52.))
        .flex_none()
        .flex()
        .items_center()
        .gap_4()
        // Room for the traffic lights, drawn over the transparent title bar (macOS only).
        .pl(px(if cfg!(target_os = "macos") { 84. } else { 16. }))
        .pr_4()
        .bg(theme.surface)
        .child(
            div()
                .w(px(288.))
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .child(logo(&theme))
                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Oxide")),
        )
        .child(div().flex_1().max_w(px(560.)).child(search::top_field(this, window, cx)))
        .child(div().flex_1())
        .child(
            div()
                .id("refresh")
                .child(if shell.loading_workspace {
                    div()
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(widgets::spinner("refresh-spin", 17., theme.muted))
                        .into_any_element()
                } else {
                    widgets::icon_button_sized("refresh-button", Icon::Refresh, 36., 17., false, &theme)
                        .on_click(move |_, _, _| core.send(Action::Refresh))
                        .into_any_element()
                })
                .tooltip(widgets::tooltip(tr("ui.main_view.refresh"), theme)),
        )
        .child(settings::account_button(this, window, cx))
        .into_any_element()
}

/// The app icon's motif, as Swift's `OxideMark` (28 × 26 in the top bar).
pub fn logo(_theme: &crate::theme::Theme) -> impl IntoElement {
    gpui::img("brand/mark-28x26.svg").w(px(28.)).h(px(26.)).flex_none()
}

/// Pitch of the rail's items: 56 pt tall with 4 pt between them, under an 8 pt top inset.
const RAIL_PITCH: f32 = 60.;
const RAIL_TOP: f32 = 8.;

fn rail(this: &mut Oxide, shell: &Shell) -> AnyElement {
    let theme = this.theme;
    let items = rail_items();
    // The selection slides between items (`matchedGeometryEffect`), on the select spring.
    let selected = items.iter().position(|(nav, _, _)| *nav == shell.nav);
    let highlight = selected.map(|ix| {
        let to = ix as f32;
        let from = motion::previous("rail-selection", to);
        let place = move |d: gpui::Div, at: f32| d.top(px(RAIL_TOP + at * RAIL_PITCH));
        motion::animate(
            div()
                .absolute()
                .left(px(6.))
                .w(px(60.))
                .h(px(56.))
                .rounded(px(8.))
                .bg(theme.accent_soft),
            motion::move_id("rail-selection", from, to),
            motion::Curve::Select,
            move |d, t| place(d, motion::lerp(from, to, t)),
        )
    });
    let mut column = div()
        .relative()
        .w(px(72.))
        .flex_none()
        .h_full()
        .pt(px(RAIL_TOP))
        .pb_3()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .bg(theme.surface)
        .children(highlight);
    for (nav, glyph, name) in items {
        let active = shell.nav == nav;
        let badge = match nav {
            Nav::Activity => shell.activity_unread,
            Nav::Chat => shell.unread,
            _ => 0,
        };
        let dot = nav == Nav::Teams && shell.teams_unread;
        let core = this.core.clone();
        let fg = if active { theme.accent_text } else { theme.muted };
        let hover = theme.input;
        // The newly selected icon bounces once (`symbolEffect(.bounce)`).
        // The selected section's symbol is filled, as Swift's `.symbolVariant(.fill)`.
        let glyph_view = if active { crate::icons::icon_fill(glyph, px(20.), fg) } else { icon(glyph, px(20.), fg) };
        let glyph_view = if active {
            motion::animate(
                glyph_view,
                SharedString::from(format!("rail-bounce-{name}")),
                motion::Curve::Pop,
                |svg, t| {
                    let lift = (t * std::f32::consts::PI).sin().max(0.) * 0.18;
                    svg.with_transformation(gpui::Transformation::scale(gpui::size(1. + lift, 1. + lift)))
                },
            )
        } else {
            glyph_view.into_any_element()
        };
        let item = div()
            .id(SharedString::from(format!("rail-{name}")))
            .w(px(60.))
            .h(px(56.))
            .rounded(px(8.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .cursor_pointer()
            .text_color(fg)
            .when(!active, move |d| d.hover(move |s| s.text_color(theme.text)));
        let item = if active { item } else { motion::hover_fill(item, format!("rail-{name}"), hover, px(8.)) };
        column = column.child(
            item
                .child(
                    div()
                        .relative()
                        .child(glyph_view)
                        // A new count pops in (`numericText` + `pop`).
                        .when(badge > 0, |d| {
                            d.child(motion::pop(
                                div().absolute().top(px(-6.)).left(px(10.)).child(widgets::badge(badge, &theme)),
                                SharedString::from(format!("rail-badge-{name}-{badge}")),
                            ))
                        })
                        .when(dot && badge == 0, |d| {
                            d.child(motion::pop(
                                div().absolute().top(px(-2.)).right(px(-4.)).size(px(8.)).rounded_full().bg(theme.danger),
                                SharedString::from(format!("rail-dot-{name}")),
                            ))
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .child(name),
                )
                .on_click(move |_, _, _| core.send(Action::Navigate { nav })),
        );
    }
    let core = this.core.clone();
    column
        .child(div().flex_1())
        .child(
            widgets::icon_button_sized("rail-settings", Icon::Gear, 34., 19., shell.nav == Nav::Settings, &theme)
                .tooltip(widgets::tooltip(tr("ui.main_view.settings"), theme))
                .on_click(move |_, _, _| core.send(Action::Navigate { nav: Nav::Settings })),
        )
        .into_any_element()
}

/// Offline, error and notice banners above the screen.
fn banners(this: &mut Oxide, shell: &Shell) -> Vec<AnyElement> {
    let theme = this.theme;
    let mut out = Vec::new();
    let banner = |id: SharedString, text: String, fg: gpui::Hsla, bg: gpui::Hsla, close: Option<Action>, core: std::sync::Arc<teams_core::OxideCore>| {
        let row = div()
            .id(id.clone())
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py(px(6.))
            .bg(bg)
            .child(div().flex_1().text_size(px(12.)).text_color(fg).child(text))
            .when_some(close, |d, action| {
                d.child(
                    widgets::icon_button_sized(SharedString::from(format!("{id}-close")), Icon::Close, 24., 11., false, &theme)
                        .on_click(move |_, _, _| core.send(action.clone())),
                )
            });
        // Banners slide down from the top edge (`slideDown`).
        motion::slide_down(div().child(row), SharedString::from(format!("{id}-in")))
    };
    if !this.online {
        out.push(banner(
            "offline".into(),
            tr("ui.main_view.you_re_offline_messages_can_t"),
            theme.error_text,
            theme.error_background,
            Some(Action::Refresh),
            this.core.clone(),
        ));
    }
    if let Some(error) = &shell.error {
        out.push(banner("error".into(), error.clone(), theme.error_text, theme.error_background, Some(Action::DismissError), this.core.clone()));
    }
    if let Some(admin) = &this.teams_admin
        && let Some(error) = &admin.error
        && admin.gallery.is_none()
        && admin.join_code.is_none()
    {
        out.push(banner(
            "teams-admin-error".into(),
            error.clone(),
            theme.error_text,
            theme.error_background,
            Some(Action::TeamsAdmin { action: teams_core::TeamsAdminAction::DismissError }),
            this.core.clone(),
        ));
    }
    if let Some(error) = this.chat_admin.as_ref().and_then(|a| a.error.clone()) {
        out.push(banner(
            "chat-admin-error".into(),
            error,
            theme.error_text,
            theme.error_background,
            Some(Action::ChatAdmin { action: teams_core::ChatAdminAction::DismissError }),
            this.core.clone(),
        ));
    }
    for (ix, notice) in shell.notices.iter().enumerate() {
        out.push(banner(
            SharedString::from(format!("notice-{ix}")),
            notice.clone(),
            theme.text,
            theme.input,
            Some(Action::DismissNotice { text: notice.clone() }),
            this.core.clone(),
        ));
    }
    out
}

/// What the content area shows (`ContentRouter`).
fn content(this: &mut Oxide, shell: &Shell, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    if this.search.as_ref().is_some_and(|s| s.open) {
        return search::screen(this, window, cx);
    }
    if this.new_chat.is_some() {
        return new_chat::screen(this, window, cx);
    }
    match shell.nav {
        Nav::Settings => settings::screen(this, window, cx),
        Nav::Calendar => calendar::screen(this, window, cx),
        Nav::Calls => calls_screen::screen(this, window, cx),
        Nav::Files => files::screen(this, window, cx),
        Nav::Activity
            if !this
                .sidebar
                .as_ref()
                .is_some_and(|s| s.activities.iter().any(|a| a.selected && a.has_target)) =>
        {
            activity::overview(this, window, cx)
        }
        Nav::Teams if shell.selected.as_ref().is_none_or(|t| t.team.is_none()) => teams::overview(this, window, cx),
        _ if this.conversation.is_some() => conversation::screen(this, window, cx),
        _ => welcome(this, shell),
    }
}

fn welcome(this: &mut Oxide, shell: &Shell) -> AnyElement {
    let theme = this.theme;
    let core = this.core.clone();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_4()
        .p_8()
        .child({
            let glyph = icon(Icon::Chat, px(48.), theme.accent);
            // Pulses while the workspace loads (`symbolEffect(.pulse)`).
            if shell.loading_workspace && !motion::reduced() {
                glyph
                    .with_animation(
                        "welcome-pulse",
                        gpui::Animation::new(std::time::Duration::from_millis(1400))
                            .repeat()
                            .with_easing(gpui::pulsating_between(0.35, 1.)),
                        |svg, t| svg.opacity(t),
                    )
                    .into_any_element()
            } else {
                glyph.into_any_element()
            }
        })
        .child(div().text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).child(tr("ui.screens.your_workspace")))
        .child(if shell.loading_workspace {
            widgets::loading("welcome-loading", tr("ui.screens.loading_your_conversations"), &theme).into_any_element()
        } else {
            widgets::muted(tr("ui.screens.find_your_private_chats_and_your"), 14., &theme).into_any_element()
        })
        .child(
            widgets::button("welcome-new-chat", tr("ui.screens.new_chat"), ButtonKind::Primary, &theme)
                .on_click(move |_, _, _| core.send(Action::NewChat)),
        )
        .into_any_element()
}

