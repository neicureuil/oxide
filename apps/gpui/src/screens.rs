//! Each screen's own UI state, and the hooks the store calls into them.
use crate::oxide::Oxide;
use crate::theme::Theme;
use crate::{
    activity, call_window, calendar, calls_screen, chat_admin, composer, conversation, files,
    image_viewer, members, new_chat, search, settings, sidebar, teams,
};
use gpui::{AnyElement, Context, FontWeight, Window, div, prelude::*, px};

pub struct Ui {
    pub sidebar: sidebar::Ui,
    pub conversation: conversation::Ui,
    pub composer: composer::Ui,
    pub teams: teams::Ui,
    pub members: members::Ui,
    pub activity: activity::Ui,
    pub search: search::Ui,
    pub new_chat: new_chat::Ui,
    pub chat_admin: chat_admin::Ui,
    pub calendar: calendar::Ui,
    pub files: files::Ui,
    pub calls: calls_screen::Ui,
    pub settings: settings::Ui,
    pub call: call_window::Ui,
    pub image_viewer: image_viewer::Ui,
    pub login: crate::login_window::Ui,
}

impl Ui {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        Self {
            sidebar: sidebar::Ui::new(window, cx),
            conversation: conversation::Ui::new(window, cx),
            composer: composer::Ui::new(window, cx),
            teams: teams::Ui::new(window, cx),
            members: members::Ui::new(window, cx),
            activity: activity::Ui::new(window, cx),
            search: search::Ui::new(window, cx),
            new_chat: new_chat::Ui::new(window, cx),
            chat_admin: chat_admin::Ui::new(window, cx),
            calendar: calendar::Ui::new(window, cx),
            files: files::Ui::new(window, cx),
            calls: calls_screen::Ui::new(window, cx),
            settings: settings::Ui::new(window, cx),
            call: call_window::Ui::new(window, cx),
            image_viewer: image_viewer::Ui::new(window, cx),
            login: crate::login_window::Ui::new(window, cx),
        }
    }
}

/// The account just signed in (also at launch with a saved session).
pub fn signed_in(this: &mut Oxide, cx: &mut Context<Oxide>) {
    run_script(this, cx);
}

/// For checking screens without clicking: `OXIDE_SCRIPT="nav:calendar;wait;select:1"` runs one step
/// every 1.5 s after sign-in. Built-in steps: `nav:<activity|chat|teams|calendar|calls|files|settings>`,
/// `select:<n>` (n-th chat), `action:<core action, e.g. NewChat, Refresh, MoreFeed>`, `popup:<key>`,
/// `resize:<w>x<h>`, `wait`. Any other step goes to each module's `debug_step` until one takes it.
fn run_script(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let Ok(script) = std::env::var("OXIDE_SCRIPT") else { return };
    let steps: Vec<String> = script.split(';').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
    let _ = this;
    cx.spawn(async move |this, cx| {
        for step in steps {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            let Some(window) = cx.update(|cx| cx.windows().first().copied()).ok().flatten() else { return };
            let _ = window.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    eprintln!("OXIDE_SCRIPT: {step}");
                    // GPUI stops drawing a covered window: keep this one in front so captures are current.
                    cx.activate(true);
                    window.activate_window();
                    if !builtin_step(this, &step, window, cx) && !module_step(this, &step, window, cx) {
                        eprintln!("OXIDE_SCRIPT: unknown step {step}");
                    }
                    cx.notify();
                });
            });
        }
    })
    .detach();
}

fn builtin_step(this: &mut Oxide, step: &str, _window: &mut Window, cx: &mut Context<Oxide>) -> bool {
    use teams_core::{Action, Nav, Target};
    let (verb, arg) = step.split_once(':').unwrap_or((step, ""));
    match verb {
        "wait" => {}
        "nav" => {
            let nav = match arg {
                "activity" => Nav::Activity,
                "chat" => Nav::Chat,
                "teams" => Nav::Teams,
                "calendar" => Nav::Calendar,
                "calls" => Nav::Calls,
                "files" => Nav::Files,
                "settings" => Nav::Settings,
                _ => return false,
            };
            this.send(Action::Navigate { nav });
        }
        "select" => {
            let Some(chat) = arg.parse::<usize>().ok().and_then(|n| this.sidebar.as_ref()?.chats.get(n).cloned()) else {
                return false;
            };
            this.send(Action::Select { target: Target { team: None, id: chat.id } });
        }
        "action" => {
            let action = match arg {
                "NewChat" => Action::NewChat,
                "CancelNewChat" => Action::CancelNewChat,
                "Refresh" => Action::Refresh,
                "MoreFeed" => Action::MoreFeed,
                "MoreChats" => Action::MoreChats,
                "CloseReply" => Action::CloseReply,
                "SignOut" => Action::SignOut,
                _ => return false,
            };
            this.send(action);
        }
        // Window size in points, as "resize:760x700", to check narrow layouts.
        "resize" => {
            let Some((w, h)) = arg.split_once('x').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)))
            else {
                return false;
            };
            _window.resize(gpui::size(px(w), px(h)));
        }
        "popup" => {
            this.popup = Some(arg.to_owned().into());
            cx.notify();
        }
        _ => return false,
    }
    true
}

fn module_step(this: &mut Oxide, step: &str, window: &mut Window, cx: &mut Context<Oxide>) -> bool {
    let all: [fn(&mut Oxide, &str, &mut Window, &mut Context<Oxide>) -> bool; 16] = [
        crate::login_window::debug_step,
        sidebar::debug_step,
        conversation::debug_step,
        composer::debug_step,
        teams::debug_step,
        members::debug_step,
        activity::debug_step,
        search::debug_step,
        new_chat::debug_step,
        chat_admin::debug_step,
        calendar::debug_step,
        files::debug_step,
        calls_screen::debug_step,
        settings::debug_step,
        call_window::debug_step,
        image_viewer::debug_step,
    ];
    all.iter().any(|f| f(this, step, window, cx))
}
pub fn composer_changed(this: &mut Oxide, cx: &mut Context<Oxide>) {
    composer::composer_changed(this, cx);
}
pub fn call_changed(this: &mut Oxide, cx: &mut Context<Oxide>) {
    call_window::call_changed(this, cx);
}
pub fn media_effect(this: &mut Oxide, effect: teams_core::Effect, cx: &mut Context<Oxide>) {
    call_window::media_effect(this, effect, cx);
}

/// Every module's dialogs and floating panels, drawn over the window.
pub fn overlays(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> Vec<AnyElement> {
    let all: [fn(&mut Oxide, &mut Window, &mut Context<Oxide>) -> Option<AnyElement>; 15] = [
        sidebar::overlay,
        conversation::overlay,
        composer::overlay,
        teams::overlay,
        members::overlay,
        activity::overlay,
        search::overlay,
        new_chat::overlay,
        chat_admin::overlay,
        calendar::overlay,
        files::overlay,
        calls_screen::overlay,
        settings::overlay,
        call_window::overlay,
        image_viewer::overlay,
    ];
    all.iter().filter_map(|f| f(this, window, cx)).collect()
}

/// Stands in for a screen not ported yet.
pub fn placeholder(name: &str, theme: &Theme) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .text_color(theme.muted)
        .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(name.to_owned()))
        .child("…")
        .into_any_element()
}
