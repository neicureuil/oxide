//! The call's people and shared screen, arranged by the chosen layout (`CallStage.swift`).
use crate::icons::{Icon, icon};
use crate::media_video::{self, VideoFeed};
use crate::oxide::{Oxide, tr, trf};
use crate::widgets;
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Corner, FontWeight, MouseButton, Pixels, Point,
    Window, anchored, deferred, div, ease_in_out, prelude::*, px,
};
use std::rc::Rc;
use std::time::Duration;
use teams_core::{ActiveCallView, AvatarShape};

/// How the call window arranges people and a shared screen; without a share, `Sharing` and `Focus` enlarge the
/// speaker or pinned person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallLayout {
    Gallery,
    Speaker,
    Sharing,
    Focus,
}

impl CallLayout {
    pub const ALL: [CallLayout; 4] = [
        CallLayout::Gallery,
        CallLayout::Speaker,
        CallLayout::Sharing,
        CallLayout::Focus,
    ];

    pub fn title(self) -> String {
        tr(match self {
            CallLayout::Gallery => "ui.call_stage.gallery",
            CallLayout::Speaker => "ui.call_stage.speaker",
            CallLayout::Sharing => "ui.call_stage.content_and_column",
            CallLayout::Focus => "ui.call_stage.full_frame",
        })
    }

    pub fn icon(self) -> Icon {
        match self {
            CallLayout::Gallery => Icon::Grid,
            CallLayout::Speaker => Icon::LayoutSpeaker,
            CallLayout::Sharing => Icon::LayoutSide,
            CallLayout::Focus => Icon::LayoutFocus,
        }
    }

    fn key(self) -> &'static str {
        match self {
            CallLayout::Gallery => "gallery",
            CallLayout::Speaker => "speaker",
            CallLayout::Sharing => "sharing",
            CallLayout::Focus => "focus",
        }
    }

    fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|l| l.key() == key)
            .unwrap_or(CallLayout::Gallery)
    }
}

/// The remembered layout pick; a screen share switches to `Sharing` until it ends or another layout is picked.
pub struct LayoutState {
    base: CallLayout,
    while_sharing: Option<CallLayout>,
    /// Participant kept in the big area instead of whoever speaks.
    pub pinned: Option<String>,
    /// Whether a screen was shared at the last draw.
    shared: Option<bool>,
}

impl LayoutState {
    pub fn new(saved: &str) -> Self {
        Self {
            base: CallLayout::from_key(saved),
            while_sharing: None,
            pinned: None,
            shared: None,
        }
    }

    pub fn current(&self) -> CallLayout {
        self.while_sharing.unwrap_or(self.base)
    }

    pub fn pick(&mut self, layout: CallLayout, prefs: &mut crate::call_prefs::CallPrefs) {
        self.base = layout;
        self.while_sharing = None;
        prefs.layout = layout.key().to_owned();
        prefs.save();
    }

    fn screen_shared(&mut self, shared: bool) {
        if self.shared == Some(shared) {
            return;
        }
        self.shared = Some(shared);
        self.while_sharing = shared.then_some(CallLayout::Sharing);
    }

    fn toggle_pin(&mut self, id: &str) {
        self.pinned = if self.pinned.as_deref() == Some(id) {
            None
        } else {
            Some(id.to_owned())
        };
    }
}

/// One tile: a person (camera or avatar) or the shared screen.
#[derive(Clone)]
struct Tile {
    id: String,
    name: String,
    muted: bool,
    speaking: bool,
    video: Option<Rc<VideoFeed>>,
    is_me: bool,
    is_screen: bool,
}

/// A right-click menu on a tile.
pub struct TileMenu {
    id: String,
    is_me: bool,
    is_screen: bool,
    position: Point<Pixels>,
}

/// The participant's camera line, or the only remote video of a direct 1:1 call.
fn camera(this: &Oxide, call: &ActiveCallView, id: &str, alone: bool) -> Option<Rc<VideoFeed>> {
    let media = &this.ui.call.media;
    if let Some(view) = call
        .videos
        .iter()
        .find(|v| !v.sharing && v.participant_id == id)
    {
        return media.remote_videos.get(&view.mid).cloned();
    }
    (!call.is_meeting && alone && call.videos.is_empty())
        .then(|| media.remote_video.clone())
        .flatten()
}

/// Speaking people first, so they stay in view in a short column.
fn ordered(tiles: Vec<Tile>) -> Vec<Tile> {
    let (speaking, rest): (Vec<Tile>, Vec<Tile>) = tiles.into_iter().partition(|t| t.speaking);
    speaking.into_iter().chain(rest).collect()
}

/// The person for the big area: pinned, else the last lasting speaker, else someone else.
fn focus(this: &Oxide, people: &[Tile]) -> Option<Tile> {
    let others: Vec<&Tile> = people.iter().filter(|t| !t.is_me).collect();
    let ui = &this.ui.call;
    others
        .iter()
        .find(|t| ui.layout.pinned.as_deref() == Some(t.id.as_str()))
        .or_else(|| {
            others
                .iter()
                .find(|t| ui.active_speaker.as_deref() == Some(t.id.as_str()))
        })
        .or_else(|| others.iter().find(|t| t.video.is_some()))
        .or_else(|| others.first())
        .map(|t| (*t).clone())
        .or_else(|| people.first().cloned())
}

pub fn stage(
    this: &mut Oxide,
    call: &ActiveCallView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let media = &this.ui.call.media;
    let my_id = this
        .shell
        .as_ref()
        .map_or("me".to_owned(), |s| s.me.clone());
    let me = Tile {
        id: my_id,
        name: tr("ui.call_stage.you"),
        muted: call.muted,
        speaking: !call.muted && media.local_speaking,
        video: if call.camera {
            media.local_video.clone()
        } else {
            None
        },
        is_me: true,
        is_screen: false,
    };
    let heard = &media.speaking_sources;
    let present: Vec<_> = call.participants.iter().filter(|p| !p.in_lobby).collect();
    let others: Vec<Tile> = present
        .iter()
        .map(|p| Tile {
            id: p.id.clone(),
            name: p.name.clone(),
            muted: p.muted,
            speaking: !p.muted && p.audio_sources.iter().any(|s| heard.contains(s)),
            video: camera(this, call, &p.id, present.len() == 1),
            is_me: false,
            is_screen: false,
        })
        .collect();
    let screen = call.videos.iter().find(|v| v.sharing).and_then(|view| {
        let feed = this.ui.call.media.remote_videos.get(&view.mid)?.clone();
        let name = call
            .participants
            .iter()
            .find(|p| p.id == view.participant_id)
            .map_or_else(|| tr("ui.call_stage.a_participant"), |p| p.name.clone());
        Some(Tile {
            id: format!("screen:{}", view.mid),
            name: trf("ui.call_stage.is_sharing_their_screen", &[("name", &name)]),
            muted: false,
            speaking: false,
            video: Some(feed),
            is_me: false,
            is_screen: true,
        })
    });
    this.ui.call.layout.screen_shared(screen.is_some());
    track_speaker(
        this,
        others
            .iter()
            .filter(|t| t.speaking)
            .map(|t| t.id.clone())
            .collect(),
        cx,
    );
    let empty = others.is_empty() && screen.is_none() && call.is_meeting;
    let mut people = vec![me.clone()];
    people.extend(others);
    let body = match this.ui.call.layout.current() {
        CallLayout::Gallery => {
            let tiles: Vec<Tile> = screen.into_iter().chain(people).collect();
            grid(this, tiles, window, cx)
        }
        CallLayout::Speaker => {
            let main = focus(this, &people);
            let main_id = main.as_ref().map(|t| t.id.clone());
            let rest: Vec<Tile> = screen
                .into_iter()
                .chain(
                    people
                        .into_iter()
                        .filter(|t| Some(&t.id) != main_id.as_ref()),
                )
                .collect();
            strip(this, main, rest, false, window, cx)
        }
        CallLayout::Sharing => {
            let main = screen.or_else(|| focus(this, &people));
            let main_id = main.as_ref().map(|t| t.id.clone());
            let rest = ordered(
                people
                    .into_iter()
                    .filter(|t| Some(&t.id) != main_id.as_ref())
                    .collect(),
            );
            strip(this, main, rest, true, window, cx)
        }
        CallLayout::Focus => {
            let shared = screen.is_some();
            let main = screen.or_else(|| focus(this, &people));
            let corner = if shared {
                focus(this, &people)
            } else if main.as_ref().is_some_and(|m| m.is_me) {
                None
            } else {
                Some(me)
            };
            let main = main.map(|t| tile(this, t, false, window, cx));
            let corner = corner.map(|t| tile(this, t, true, window, cx));
            div()
                .size_full()
                .relative()
                .children(main)
                .when_some(corner, |d, corner| {
                    d.child(
                        div()
                            .absolute()
                            .right(px(12.))
                            .bottom(px(12.))
                            .w(px(200.))
                            .h(px(120.))
                            .shadow_lg()
                            .child(corner),
                    )
                })
                .into_any_element()
        }
    };
    let menu = tile_menu(this, cx);
    div()
        .size_full()
        .relative()
        .child(body)
        .when(empty, |d| {
            d.child(
                div()
                    .absolute()
                    .bottom(px(14.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(widgets::muted(
                        tr("ui.call_stage.no_one_else_is_here_yet"),
                        13.,
                        &this.theme,
                    )),
            )
        })
        .children(menu)
        .into_any_element()
}

/// Who spoke last for long enough (1.5 s), so the big area does not jump on every "hm".
fn track_speaker(this: &mut Oxide, speaking_now: Vec<String>, cx: &mut Context<Oxide>) {
    let ui = &mut this.ui.call;
    if ui.speaking_now == speaking_now {
        return;
    }
    ui.speaking_now = speaking_now.clone();
    let Some(first) = speaking_now.first().cloned() else {
        ui.speaker_timer = None;
        return;
    };
    ui.speaker_timer = Some(cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(1500))
            .await;
        let _ = this.update(cx, |this, cx| {
            let ui = &mut this.ui.call;
            if !ui
                .active_speaker
                .as_ref()
                .is_some_and(|a| speaking_now.contains(a))
            {
                ui.active_speaker = Some(first);
            }
            crate::call_window::redraw(this, cx);
        });
    }));
}

fn grid(
    this: &mut Oxide,
    tiles: Vec<Tile>,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let count = tiles.len().max(1);
    let columns = (count as f64).sqrt().ceil().max(1.) as usize;
    let mut rows = Vec::new();
    let mut tiles = tiles.into_iter().peekable();
    while tiles.peek().is_some() {
        let row: Vec<AnyElement> = tiles
            .by_ref()
            .take(columns)
            .map(|t| {
                div()
                    .flex_1()
                    .h_full()
                    .child(tile(this, t, false, window, cx))
                    .into_any_element()
            })
            .collect();
        rows.push(
            div()
                .flex_1()
                .min_h(px(80.))
                .w_full()
                .flex()
                .gap(px(10.))
                .children(row)
                .into_any_element(),
        );
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(10.))
        .children(rows)
        .into_any_element()
}

/// A big tile and the others in a bottom row (`vertical: false`) or a right column.
fn strip(
    this: &mut Oxide,
    main: Option<Tile>,
    rest: Vec<Tile>,
    vertical: bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let main = main.map(|t| {
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(tile(this, t, false, window, cx))
    });
    let small: Vec<AnyElement> = rest
        .into_iter()
        .map(|t| {
            div()
                .flex_none()
                .w(px(if vertical { 200. } else { 180. }))
                .h(px(if vertical { 120. } else { 110. }))
                .child(tile(this, t, true, window, cx))
                .into_any_element()
        })
        .collect();
    let has_rest = !small.is_empty();
    let rest = div()
        .id(if vertical {
            "stage-column"
        } else {
            "stage-row"
        })
        .flex()
        .gap(px(8.))
        .flex_none()
        .map(|d| {
            if vertical {
                d.flex_col().w(px(200.)).h_full().overflow_y_scroll()
            } else {
                d.h(px(110.)).w_full().overflow_x_scroll()
            }
        })
        .children(small);
    div()
        .size_full()
        .flex()
        .gap(px(10.))
        .map(|d| if vertical { d.flex_row() } else { d.flex_col() })
        .children(main)
        .when(has_rest, |d| d.child(rest))
        .into_any_element()
}

/// Double-click: the shared screen toggles `Sharing` / `Focus`; a person goes big in `Speaker`.
fn enlarge(this: &mut Oxide, id: &str, is_me: bool, is_screen: bool) {
    let ui = &mut this.ui.call;
    if is_screen {
        let next = if ui.layout.current() == CallLayout::Focus {
            CallLayout::Sharing
        } else {
            CallLayout::Focus
        };
        ui.layout.pick(next, &mut ui.prefs);
    } else {
        if !is_me {
            ui.layout.pinned = Some(id.to_owned());
        }
        ui.layout.pick(CallLayout::Speaker, &mut ui.prefs);
    }
}

fn tile(
    this: &mut Oxide,
    tile: Tile,
    small: bool,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let (id, is_me, is_screen) = (tile.id.clone(), tile.is_me, tile.is_screen);
    let element_id = gpui::SharedString::from(format!("tile-{}-{}", small, tile.id));
    let click_id = id.clone();
    let menu_id = id.clone();
    let body = if tile.is_screen {
        let video = tile
            .video
            .as_ref()
            .map(|v| media_video::view(&v.slot, true));
        div()
            .size_full()
            .relative()
            .rounded(px(10.))
            .overflow_hidden()
            .bg(gpui::black())
            .children(video)
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(8.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded_full()
                    .bg(theme.surface.opacity(0.85))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(px(if small { 11. } else { 12. }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon(Icon::Screen, px(12.), theme.text))
                    .child(tile.name.clone()),
            )
            .into_any_element()
    } else {
        let pinned = this.ui.call.layout.pinned.as_deref() == Some(tile.id.as_str());
        participant(this, &tile, small, pinned)
    };
    div()
        .id(element_id)
        .size_full()
        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
            if event.click_count() == 2 {
                enlarge(this, &click_id, is_me, is_screen);
                crate::call_window::redraw(this, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                this.ui.call.tile_menu = Some(TileMenu {
                    id: menu_id.clone(),
                    is_me,
                    is_screen,
                    position: event.position,
                });
                crate::call_window::redraw(this, cx);
            }),
        )
        .child(body)
        .into_any_element()
}

/// A person: camera, or avatar with a ring pulsing while they speak.
fn participant(this: &mut Oxide, tile: &Tile, small: bool, pinned: bool) -> AnyElement {
    let theme = this.theme;
    let avatar_size = if small { 44. } else { 80. };
    let content = match &tile.video {
        Some(video) => media_video::view(&video.slot, false),
        None => {
            let avatar = this.avatar(&tile.id, &tile.name, avatar_size, AvatarShape::Round, None);
            let ring = avatar_size + 8.;
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .relative()
                        .size(px(ring * 1.2))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(tile.speaking, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .size(px(ring))
                                    .rounded_full()
                                    .border(px(3.))
                                    .border_color(theme.accent)
                                    .with_animation(
                                        gpui::SharedString::from(format!("pulse-{}", tile.id)),
                                        Animation::new(Duration::from_millis(1200))
                                            .repeat()
                                            .with_easing(ease_in_out),
                                        move |d, delta| {
                                            // 0.6 s out and back, as the macOS app's autoreversing pulse.
                                            let t = if delta < 0.5 {
                                                delta * 2.
                                            } else {
                                                (1. - delta) * 2.
                                            };
                                            let side = ring * (1. + 0.18 * t);
                                            d.size(px(side)).opacity(0.9 - 0.55 * t)
                                        },
                                    ),
                            )
                        })
                        .child(avatar),
                )
                .into_any_element()
        }
    };
    div()
        .size_full()
        .relative()
        .rounded(px(12.))
        .overflow_hidden()
        .bg(theme.conversation)
        .border(px(if tile.speaking { 2. } else { 1. }))
        .border_color(if tile.speaking {
            theme.accent
        } else {
            theme.border
        })
        .child(content)
        .child(
            div()
                .absolute()
                .left(px(8.))
                .bottom(px(8.))
                .max_w(gpui::relative(0.9))
                .px(px(8.))
                .py(px(4.))
                .rounded_full()
                .bg(theme.surface.opacity(0.85))
                .flex()
                .items_center()
                .gap(px(5.))
                .when(pinned, |d| d.child(icon(Icon::Pin, px(10.), theme.accent)))
                .when(tile.muted, |d| {
                    d.child(icon(Icon::MicOff, px(10.), theme.muted))
                })
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(tile.name.clone()),
                ),
        )
        .into_any_element()
}

fn tile_menu(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let menu = this.ui.call.tile_menu.as_ref()?;
    let theme = this.theme;
    let (id, is_me, is_screen, position) =
        (menu.id.clone(), menu.is_me, menu.is_screen, menu.position);
    let layout = &this.ui.call.layout;
    let mut items: Vec<AnyElement> = Vec::new();
    let enlarge_label = if is_screen {
        if layout.current() == CallLayout::Focus {
            tr("ui.call_stage.show_participants")
        } else {
            tr("ui.call_stage.full_frame")
        }
    } else {
        tr("ui.call_stage.enlarge")
    };
    let enlarge_id = id.clone();
    items.push(
        widgets::menu_item("tile-enlarge", None, enlarge_label, &theme)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.ui.call.tile_menu = None;
                enlarge(this, &enlarge_id, is_me, is_screen);
                crate::call_window::redraw(this, cx);
            }))
            .into_any_element(),
    );
    if !is_screen && !is_me {
        let pinned = layout.pinned.as_deref() == Some(id.as_str());
        items.push(
            widgets::menu_item(
                "tile-pin",
                None,
                if pinned {
                    tr("ui.call_stage.unpin")
                } else {
                    tr("ui.call_stage.pin")
                },
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.ui.call.tile_menu = None;
                this.ui.call.layout.toggle_pin(&id);
                crate::call_window::redraw(this, cx);
            }))
            .into_any_element(),
        );
    }
    let close = cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
        this.ui.call.tile_menu = None;
        crate::call_window::redraw(this, cx);
    });
    Some(
        deferred(
            anchored()
                .position(position)
                .anchor(Corner::TopLeft)
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .occlude()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface)
                        .shadow_lg()
                        .py_1()
                        .on_mouse_down_out(close)
                        .children(items),
                ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_switches_to_sharing_until_another_pick() {
        let mut state = LayoutState::new("speaker");
        assert_eq!(state.current(), CallLayout::Speaker);
        state.screen_shared(true);
        assert_eq!(state.current(), CallLayout::Sharing);
        state.screen_shared(false);
        assert_eq!(state.current(), CallLayout::Speaker);
        state.toggle_pin("a");
        assert_eq!(state.pinned.as_deref(), Some("a"));
        state.toggle_pin("a");
        assert_eq!(state.pinned, None);
    }

    #[test]
    fn unknown_saved_layout_is_gallery() {
        assert_eq!(LayoutState::new("").current(), CallLayout::Gallery);
        assert_eq!(CallLayout::from_key("focus"), CallLayout::Focus);
    }
}
