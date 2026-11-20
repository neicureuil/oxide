//! The open conversation's lists (chat messages, channel posts, thread replies) and their rows (`FeedTable.swift`).
use crate::feed_message::{self, RowKind};
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr, trf};
use crate::widgets;
use gpui::{
    AnyElement, Context, FontWeight, ListAlignment, ListOffset, ListState, SharedString, Window,
    div, prelude::*, px,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use teams_core::{Action, MessageView, NotificationsAction, OxideCore, PostView};

/// What a list needs to know of its items: mine follow to the bottom, others' are counted while scrolled up.
pub trait FeedEntry: Clone + 'static {
    fn mine(&self) -> bool;
    fn key(&self) -> String;
    /// Drops what the app never reads: the flat `runs` repeat the body's text for Swift's copy and
    /// height estimates; here the text comes from the blocks.
    fn strip(&mut self);
}
impl FeedEntry for MessageView {
    fn mine(&self) -> bool {
        self.mine
    }
    fn key(&self) -> String {
        self.id.clone()
    }
    fn strip(&mut self) {
        self.runs = Vec::new();
    }
}
impl FeedEntry for PostView {
    fn mine(&self) -> bool {
        self.root.mine
    }
    fn key(&self) -> String {
        self.id.clone()
    }
    fn strip(&mut self) {
        self.root.runs = Vec::new();
    }
}

/// Items kept on each side of what is on screen; the others are asked of the core again when drawn.
const KEEP: usize = 30;

thread_local! {
    /// Messages that just arrived at the bottom, and when: their rows fade in once (`insertRows` with
    /// `.effectFade`), and not again when scrolled back into view.
    static FRESH: RefCell<HashMap<String, std::time::Instant>> = RefCell::default();
}

thread_local! {
    /// Width of each feed (main list, reply pane) at its last layout, and the feed whose rows are being drawn.
    static WIDTHS: RefCell<HashMap<&'static str, f32>> = RefCell::default();
    static DRAWING: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// Room around a message's content in a row: padding, avatar and gap, the bubble's own padding and the
/// space kept on the far side. Pictures and cards fit in what is left.
const ROW_CHROME: f32 = 170.;

/// The rows drawn next belong to feed `id` (`None` when done).
pub fn drawing_feed(id: Option<&'static str>) {
    DRAWING.with(|d| d.set(id));
}
/// Records feed `id`'s width; true when it changed, so the rows are drawn again to fit.
pub fn set_feed_width(id: &'static str, width: f32) -> bool {
    WIDTHS.with(|w| {
        let mut w = w.borrow_mut();
        let changed = w.get(id).is_none_or(|old| (old - width).abs() > 0.5);
        w.insert(id, width);
        changed
    })
}
/// The widest a picture or card may be in the feed being drawn: a fixed size there would push the whole
/// message wider than its column, as the bubble takes its content's width.
pub fn content_limit() -> f32 {
    let width = DRAWING.with(|d| d.get()).and_then(|id| WIDTHS.with(|w| w.borrow().get(id).copied()));
    width.map_or(f32::MAX, |w| (w - ROW_CHROME).max(120.))
}

/// True for a message that arrived in the last moment, while its row may still be fading in.
pub fn is_fresh(id: &str) -> bool {
    FRESH.with(|f| f.borrow().get(id).is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400)))
}

/// One list of the open conversation, merged from the core's differences, drawn virtualised.
///
/// The core keeps every message (parsed once); this keeps only the views of items on screen and around it,
/// and builds the others again from the core when they scroll back in: a Rust front end shares the core's
/// memory, so it need not hold a second copy of the whole conversation as the Swift app had to.
pub struct FeedList<T> {
    ids: Rc<RefCell<Vec<String>>>,
    views: Rc<RefCell<HashMap<String, T>>>,
    /// Builds views of the open conversation by id, from the core's state.
    fetch: Rc<dyn Fn(&[String]) -> Vec<T>>,
    /// Index range whose views are kept: what is on screen and `KEEP` on each side.
    near: Rc<Cell<(usize, usize)>>,
    pub list: ListState,
    /// First id the list was built with, to tell older items (before) from newer ones (after).
    first: Option<String>,
    can_load_more: Rc<Cell<bool>>,
    /// Messages from others that arrived below while the reader was scrolled up (the pill's count).
    unseen: Cell<usize>,
}

impl<T: FeedEntry> FeedList<T> {
    /// `more` is sent once the list is scrolled near its oldest item; visible ids go out as `ShowMessages`.
    pub fn new(
        core: &Arc<OxideCore>,
        more: Action,
        alignment: ListAlignment,
        fetch: impl Fn(&[String]) -> Vec<T> + 'static,
    ) -> Self {
        let core = core.clone();
        Self::with_sender(move |action| core.send(action), more, alignment, fetch)
    }

    /// Same, sending actions through `send` (tests use it without a core).
    fn with_sender(
        send: impl Fn(Action) + 'static,
        more: Action,
        alignment: ListAlignment,
        fetch: impl Fn(&[String]) -> Vec<T> + 'static,
    ) -> Self {
        let list = ListState::new(0, alignment, px(800.));
        let ids: Rc<RefCell<Vec<String>>> = Rc::default();
        let views: Rc<RefCell<HashMap<String, T>>> = Rc::default();
        let near = Rc::new(Cell::new((0, 0)));
        let can_load_more = Rc::new(Cell::new(false));
        let shown: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let ids = ids.clone();
            let views = views.clone();
            let near = near.clone();
            let can_load_more = can_load_more.clone();
            list.set_scroll_handler(move |event, _, _| {
                // Reaching the top loads one page, as Teams does; the next one waits for this one to land.
                if event.visible_range.start < 3 && can_load_more.replace(false) {
                    send(more.clone());
                }
                let ids = ids.borrow();
                // Views far from the screen go; they are built again if scrolled back to.
                let window = (
                    event.visible_range.start.saturating_sub(KEEP),
                    (event.visible_range.end + KEEP).min(ids.len()),
                );
                if near.replace(window) != window {
                    trim(&ids, &mut views.borrow_mut(), window);
                }
                // Previews of what is on screen and about a screen around it load first.
                let span = event.visible_range.len().max(1);
                let from = event.visible_range.start.saturating_sub(span);
                let to = (event.visible_range.end + span).min(ids.len());
                let near_ids: Vec<String> = ids.get(from..to).map(|s| s.to_vec()).unwrap_or_default();
                if *shown.borrow() != near_ids && !near_ids.is_empty() {
                    *shown.borrow_mut() = near_ids.clone();
                    send(Action::ShowMessages { ids: near_ids });
                }
            });
        }
        Self {
            ids,
            views,
            fetch: Rc::new(fetch),
            near,
            list,
            first: None,
            can_load_more,
            unseen: Cell::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.ids.borrow().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn id(&self, ix: usize) -> Option<String> {
        self.ids.borrow().get(ix).cloned()
    }
    pub fn ids(&self) -> Vec<String> {
        self.ids.borrow().clone()
    }
    pub fn contains(&self, id: &str) -> bool {
        self.ids.borrow().iter().any(|i| i == id)
    }
    /// Whether a kept view matches `f` (views far from the screen are not looked at).
    pub fn any_kept(&self, f: impl Fn(&T) -> bool) -> bool {
        self.views.borrow().values().any(f)
    }

    /// The item at `ix`, from the kept views or else built by the core (with its neighbours, kept if near).
    pub fn get(&self, ix: usize) -> Option<T> {
        let id = self.id(ix)?;
        if let Some(view) = self.views.borrow().get(&id) {
            return Some(view.clone());
        }
        // Rows come in runs as the list scrolls: ask for the missing ones around this one at once.
        let (from, to) = (ix.saturating_sub(8), (ix + 9).min(self.len()));
        let missing: Vec<String> = {
            let ids = self.ids.borrow();
            let views = self.views.borrow();
            ids[from..to].iter().filter(|i| !views.contains_key(*i)).cloned().collect()
        };
        let mut found = None;
        let (near_from, near_to) = self.near_window();
        let positions: HashMap<String, usize> =
            (from..to).filter_map(|i| self.id(i).map(|id| (id, i))).collect();
        for mut view in (self.fetch)(&missing) {
            view.strip();
            let key = view.key();
            if key == id {
                found = Some(view.clone());
            }
            if positions.get(&key).is_some_and(|i| (near_from..near_to).contains(i)) {
                self.views.borrow_mut().insert(key, view);
            }
        }
        found
    }

    /// Views of `ids` (in that order), kept or built by the core; those built are not kept.
    pub fn views_of(&self, ids: &[String]) -> Vec<T> {
        let missing: Vec<String> = {
            let views = self.views.borrow();
            ids.iter().filter(|i| !views.contains_key(*i)).cloned().collect()
        };
        let mut built: HashMap<String, T> = (self.fetch)(&missing)
            .into_iter()
            .map(|mut v| {
                v.strip();
                (v.key(), v)
            })
            .collect();
        let views = self.views.borrow();
        ids.iter()
            .filter_map(|id| views.get(id).cloned().or_else(|| built.remove(id)))
            .collect()
    }

    pub fn clear(&mut self) {
        self.ids.borrow_mut().clear();
        self.views.borrow_mut().clear();
        self.near.set((0, 0));
        self.list.reset(0);
        self.first = None;
        self.unseen.set(0);
    }
    pub fn set_can_load_more(&self, more: bool) {
        self.can_load_more.set(more);
    }

    /// The kept range; before the list first scrolls, the newest items (the lists open at the bottom).
    fn near_window(&self) -> (usize, usize) {
        let len = self.len();
        match self.near.get() {
            (0, 0) => (len.saturating_sub(2 * KEEP), len),
            window => window,
        }
    }

    /// Whether the newest item is in view: a bottom-aligned list then follows what arrives.
    pub fn at_bottom(&self) -> bool {
        self.list.logical_scroll_top().item_ix >= self.list.item_count()
    }
    /// Arrivals from others since the reader scrolled up; back to 0 once at the bottom.
    pub fn unseen(&self) -> usize {
        if self.at_bottom() {
            self.unseen.set(0);
        }
        self.unseen.get()
    }
    /// The pill's action: back to the newest item, following again.
    pub fn jump_to_bottom(&self) {
        self.list.scroll_to(ListOffset {
            item_ix: self.list.item_count(),
            offset_in_item: px(0.),
        });
        self.unseen.set(0);
    }

    /// Merges one difference and remeasures only the rows that changed; keeps the views near the screen.
    pub fn apply(&mut self, ids: Option<Vec<String>>, changed: Vec<T>) {
        let mut appended: Vec<String> = Vec::new();
        if let Some(new) = ids {
            let old = self.list.item_count();
            if old == 0 || new.len() < old {
                self.list.reset(new.len());
            } else if new.len() > old {
                // Older items come before, newer after: keep what is on screen in place.
                let added = new.len() - old;
                if self.first.as_ref() == new.get(added) {
                    self.list.splice(0..0, added);
                    // The kept window moves with the items it covers.
                    let (from, to) = self.near.get();
                    if (from, to) != (0, 0) {
                        self.near.set((from + added, to + added));
                    }
                } else {
                    self.list.splice(old..old, added);
                    appended = new[old..].to_vec();
                }
            }
            self.first = new.first().cloned();
            *self.ids.borrow_mut() = new;
        }
        let mine = changed.iter().any(|v| appended.contains(&v.key()) && v.mine());
        {
            let ids = self.ids.borrow();
            let mut views = self.views.borrow_mut();
            for mut item in changed {
                item.strip();
                let id = item.key();
                if let Some(ix) = ids.iter().position(|i| *i == id) {
                    self.list.splice(ix..ix + 1, 1);
                    // The next row may drop or gain its day separator.
                    if ix + 1 < ids.len() {
                        self.list.splice(ix + 1..ix + 2, 1);
                    }
                }
                views.insert(id, item);
            }
            let window = match self.near.get() {
                (0, 0) => (ids.len().saturating_sub(2 * KEEP), ids.len()),
                (from, to) => (from, to.min(ids.len())),
            };
            trim(&ids, &mut views, window);
        }
        if !appended.is_empty() {
            FRESH.with(|f| {
                let mut f = f.borrow_mut();
                f.retain(|_, at| at.elapsed() < std::time::Duration::from_secs(1));
                let now = std::time::Instant::now();
                for id in &appended {
                    f.insert(id.clone(), now);
                }
            });
        }
        if appended.is_empty() || self.at_bottom() {
            return;
        }
        if mine {
            // What I send shows, as in Teams, even when reading older messages.
            self.jump_to_bottom();
        } else {
            self.unseen.set(self.unseen.get() + appended.len());
        }
    }
}

/// Keeps only the views whose item sits in `window` (indices into `ids`).
fn trim<T>(ids: &[String], views: &mut HashMap<String, T>, (from, to): (usize, usize)) {
    let keep: HashSet<&String> = ids.get(from..to.min(ids.len())).unwrap_or_default().iter().collect();
    views.retain(|id, _| keep.contains(id));
}

/// Row padding of `FeedItemView`: 24 across, 8 above and below.
fn padded(child: impl IntoElement) -> gpui::Div {
    div().w_full().px(px(24.)).py(px(8.)).child(child)
}

/// « Scroll for earlier messages », or a spinner while they load; the same height either way keeps the feed still.
fn more_row(label: String, loading: bool, this: &Oxide) -> AnyElement {
    let theme = this.theme;
    padded(
        div()
            .h(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .child(if loading {
                widgets::spinner("feed-more-spin", 14., theme.muted)
            } else {
                div()
                    .text_size(px(12.))
                    .text_color(theme.subtle)
                    .child(label)
                    .into_any_element()
            }),
    )
    .into_any_element()
}

/// A day separator, or « Nouveaux messages » in the danger colour.
fn separator(label: String, danger: bool, this: &Oxide) -> AnyElement {
    let theme = this.theme;
    let color = if danger { theme.danger } else { theme.border };
    padded(
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().flex_1().h(px(1.)).bg(color))
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(if danger { theme.danger } else { theme.subtle })
                    .when(danger, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(label),
            )
            .child(div().flex_1().h(px(1.)).bg(color)),
    )
    .into_any_element()
}

/// One row of the chat feed: its message, after the "load earlier" row, a day separator or the unread marker.
pub fn chat_row(
    this: &mut Oxide,
    ix: usize,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let Some(msg) = this.messages.get(ix) else {
        return div().into_any_element();
    };
    let Some(view) = this.conversation.as_ref() else {
        return div().into_any_element();
    };
    let more = (ix == 0 && view.has_more)
        .then(|| (tr("ui.store.scroll_for_earlier_messages"), view.loading));
    let first_unread = view.first_unread.as_deref() == Some(msg.id.as_str());
    let previous_day = ix
        .checked_sub(1)
        .and_then(|p| this.messages.get(p))
        .map(|m| m.day.clone());
    let new_day = !msg.day.is_empty() && previous_day.as_deref() != Some(msg.day.as_str());
    let mut column = div().w_full().flex().flex_col();
    if let Some((label, loading)) = more {
        column = column.child(more_row(label, loading, this));
    }
    if new_day {
        column = column.child(separator(msg.day.clone(), false, this));
    }
    if first_unread {
        column = column.child(separator(tr("ui.feed_table.new_messages_2"), true, this));
    }
    column
        .child(padded(feed_message::message_row(
            this,
            &msg,
            RowKind::Bubble,
            window,
            cx,
        )))
        .into_any_element()
}

/// One row of a thread: a reply, with a rule under it marking where the next speaker starts.
pub fn reply_row(
    this: &mut Oxide,
    ix: usize,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let Some(msg) = this.replies.get(ix) else {
        return div().into_any_element();
    };
    let Some(thread) = this.conversation.as_ref().and_then(|c| c.thread.as_ref()) else {
        return div().into_any_element();
    };
    let more = (ix == 0 && thread.has_more)
        .then(|| (tr("ui.store.scroll_for_earlier_messages"), thread.loading));
    let previous_day = ix
        .checked_sub(1)
        .and_then(|p| this.replies.get(p))
        .map(|m| m.day.clone());
    let new_day = !msg.day.is_empty() && previous_day.as_deref() != Some(msg.day.as_str());
    let theme = this.theme;
    let mut column = div().w_full().flex().flex_col();
    if let Some((label, loading)) = more {
        column = column.child(more_row(label, loading, this));
    }
    if new_day {
        column = column.child(separator(msg.day.clone(), false, this));
    }
    column
        .child(padded(
            div()
                .pt(px(4.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(feed_message::message_row(
                    this,
                    &msg,
                    RowKind::Reply,
                    window,
                    cx,
                ))
                .child(div().ml(px(42.)).h(px(1.)).bg(theme.border)),
        ))
        .into_any_element()
}

/// One row of a channel: a post as a card, with its replies and follow toggle; a deleted one keeps a line.
pub fn post_row(
    this: &mut Oxide,
    ix: usize,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let Some(post) = this.posts.get(ix) else {
        return div().into_any_element();
    };
    let Some(view) = this.conversation.as_ref() else {
        return div().into_any_element();
    };
    let more =
        (ix == 0 && view.has_more).then(|| (tr("ui.store.scroll_for_earlier_posts"), view.loading));
    let error = (ix + 1 == this.posts.len())
        .then(|| view.posts_error.clone())
        .flatten();
    let theme = this.theme;
    let mut column = div().w_full().flex().flex_col();
    if let Some((label, loading)) = more {
        column = column.child(more_row(label, loading, this));
    }
    let replies_label = trf(
        "ui.feed_table.replies_open_thread",
        &[("replies", &post.replies.to_string())],
    );
    let card = if post.root.deleted {
        let core = this.core.clone();
        let id = post.id.clone();
        div()
            .w_full()
            .px(px(14.))
            .py(px(8.))
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(10.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface.opacity(0.6))
            .child(icon(Icon::Trash, px(11.), theme.subtle))
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .italic()
                    .text_color(theme.muted)
                    .child(tr("ui.message_row.this_message_has_been_deleted")),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(theme.subtle)
                    .child(format!("· {} · {}", post.root.sender, post.root.time)),
            )
            .child(div().flex_1().min_w(px(8.)))
            .when(post.replies > 0, |d| {
                d.child(
                    ghost(
                        SharedString::from(format!("post-open-{}", post.id)),
                        None,
                        replies_label.clone(),
                        true,
                        this,
                    )
                    .on_click(move |_, _, _| core.send(Action::OpenReply { post: id.clone() })),
                )
            })
            .into_any_element()
    } else {
        let message =
            feed_message::message_row(this, &post.root, RowKind::Post(post.id.clone()), window, cx);
        let open = {
            let core = this.core.clone();
            let id = post.id.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, _: &mut gpui::App| {
                core.send(Action::OpenReply { post: id.clone() })
            }
        };
        let follow = {
            let core = this.core.clone();
            let (id, following) = (post.id.clone(), post.following);
            move |_: &gpui::ClickEvent, _: &mut Window, _: &mut gpui::App| {
                core.send(Action::Notifications {
                    action: NotificationsAction::SetPostFollow {
                        post: id.clone(),
                        follow: !following,
                    },
                })
            }
        };
        let reply_label = if post.replies == 0 {
            tr("ui.feed_table.reply")
        } else {
            replies_label
        };
        div()
            .w_full()
            .p(px(18.))
            .rounded(px(12.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(message)
            .child(div().h(px(1.)).bg(theme.border))
            // In a narrow column "Follow" goes under "Reply" rather than past the card's edge.
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .child(
                        ghost(
                            SharedString::from(format!("post-reply-{}", post.id)),
                            Some(Icon::Chat),
                            reply_label,
                            true,
                            this,
                        )
                        .on_click(open),
                    )
                    .child(div().flex_1())
                    .child(
                        ghost(
                            SharedString::from(format!("post-follow-{}", post.id)),
                            Some(Icon::Bell),
                            if post.following {
                                tr("ui.feed_table.following")
                            } else {
                                tr("ui.feed_table.follow")
                            },
                            false,
                            this,
                        )
                        .tooltip(widgets::tooltip(
                            if post.following {
                                tr("ui.feed_table.stop_getting_notified_of_replies")
                            } else {
                                tr("ui.feed_table.get_notified_of_replies")
                            },
                            theme,
                        ))
                        .on_click(follow),
                    ),
            )
            .into_any_element()
    };
    column = column.child(padded(card));
    if let Some(error) = error {
        column = column.child(posts_error(error, this));
    }
    column.into_any_element()
}

/// Why posts could not load, with a way to try again.
pub fn posts_error(error: String, this: &Oxide) -> AnyElement {
    let theme = this.theme;
    let core = this.core.clone();
    padded(
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_2()
            .child(widgets::muted(error, 12., &theme))
            .child(
                outline("posts-retry", tr("ui.feed_table.retry_posts"), this)
                    .on_click(move |_, _, _| core.send(Action::Refresh)),
            ),
    )
    .into_any_element()
}

/// `GhostButtonStyle`: no frame, the input colour under the pointer; `accent` for the accent text.
pub fn ghost(
    id: impl Into<gpui::ElementId>,
    glyph: Option<Icon>,
    label: String,
    accent: bool,
    this: &Oxide,
) -> gpui::Stateful<gpui::Div> {
    let theme = this.theme;
    let fg = if accent {
        theme.accent_text
    } else {
        theme.muted
    };
    let hover = theme.input;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(6.))
        .py(px(4.))
        .rounded(px(8.))
        .text_size(px(12.))
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .when_some(glyph, |d, g| d.child(icon(g, px(13.), fg)))
        .child(label)
}

/// `OutlineButtonStyle`: 32 high, the strong border, soft text.
pub fn outline(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    this: &Oxide,
) -> gpui::Stateful<gpui::Div> {
    let theme = this.theme;
    widgets::button(id, label, widgets::ButtonKind::Outline, &theme)
        .rounded(px(8.))
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.text_soft)
}

#[cfg(test)]
mod keep_tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Item(String);
    impl FeedEntry for Item {
        fn mine(&self) -> bool {
            false
        }
        fn key(&self) -> String {
            self.0.clone()
        }
        fn strip(&mut self) {}
    }

    fn feed(asked: Rc<RefCell<Vec<String>>>) -> FeedList<Item> {
        FeedList::with_sender(|_| {}, Action::MoreFeed, ListAlignment::Bottom, move |ids: &[String]| {
            asked.borrow_mut().extend(ids.iter().cloned());
            ids.iter().map(|id| Item(id.clone())).collect()
        })
    }

    #[test]
    fn keeps_views_near_the_screen_and_asks_the_core_for_the_others() {
        let asked: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut list = feed(asked.clone());
        let ids: Vec<String> = (0..200).map(|i| format!("m{i}")).collect();
        let all: Vec<Item> = ids.iter().map(|i| Item(i.clone())).collect();
        list.apply(Some(ids), all);
        // Opened at the bottom: only the newest 2 × KEEP are kept.
        assert_eq!(list.views.borrow().len(), 2 * KEEP);
        assert!(list.views.borrow().contains_key("m199"));
        assert!(!list.views.borrow().contains_key("m0"));
        // An old one is built by the core when drawn, without being kept (it is far from the screen).
        assert_eq!(list.get(0), Some(Item("m0".into())));
        assert!(asked.borrow().contains(&"m0".to_owned()));
        assert!(!list.views.borrow().contains_key("m0"));
        // A kept one is not asked for again.
        asked.borrow_mut().clear();
        assert_eq!(list.get(190), Some(Item("m190".into())));
        assert!(asked.borrow().is_empty());
    }

    #[test]
    fn older_pages_shift_the_kept_window_and_changes_far_away_are_dropped() {
        let mut list = feed(Rc::default());
        let ids: Vec<String> = (100..200).map(|i| format!("m{i}")).collect();
        let all: Vec<Item> = ids.iter().map(|i| Item(i.clone())).collect();
        list.apply(Some(ids.clone()), all);
        // Scrolled near the top of the first page: its rows are drawn, so their views are built and kept.
        list.near.set((0, 40));
        for ix in 0..40 {
            list.get(ix);
        }
        let mut older: Vec<String> = (50..100).map(|i| format!("m{i}")).collect();
        older.extend(ids);
        let page: Vec<Item> = (50..100).map(|i| Item(format!("m{i}"))).collect();
        list.apply(Some(older), page);
        // The window followed the items it covered; the new page above it is not kept.
        assert_eq!(list.near.get(), (50, 90));
        assert!(list.views.borrow().contains_key("m100"));
        assert!(!list.views.borrow().contains_key("m50"));
        assert_eq!(list.views_of(&["m50".to_owned(), "m100".to_owned()]).len(), 2);
    }
}
