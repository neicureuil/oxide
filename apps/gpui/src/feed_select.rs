//! Selecting message text with the mouse, across paragraphs and messages (`FeedSelection.swift`).
//!
//! Each text piece of a message (its author line, then each paragraph of its body) is a unit. Units register
//! themselves while drawn, with their laid-out text and where it landed; one listener for the whole feed follows
//! the press and the drag across them. The selected part of each unit is drawn as a highlight in its text, and
//! ⌘C copies it, a blank line between messages. A click without a drag on a link opens it.
use gpui::{
    App, Bounds, ClipboardItem, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, TextLayout, TextRun, canvas, prelude::*,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use unicode_segmentation::UnicodeSegmentation;

/// The author line's segment, before the body's paragraphs (0, 1, …).
pub const AUTHOR: i32 = -1;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Unit {
    pub message: String,
    pub segment: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Spot {
    unit: Unit,
    index: usize,
}

/// A unit drawn this frame: its text, layout, links, and its bounds once prepainted.
struct Drawn {
    unit: Unit,
    text: String,
    layout: TextLayout,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    links: Vec<(Range<usize>, String)>,
}

#[derive(Default)]
struct State {
    anchor: Option<Spot>,
    focus: Option<Spot>,
    dragging: bool,
    /// Feed position of each message, read when a press starts: (list, row).
    order: HashMap<String, (u8, usize)>,
    /// Plain text of each message by id, for messages between both ends that are off screen.
    plain: Option<Rc<dyn Fn(&str) -> Vec<String>>>,
    drawn: Vec<Drawn>,
    /// The message whose body is being drawn and its next segment, so `register` needs no plumbing.
    current: Option<(String, i32)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

/// Called before the feed is drawn: the units of the previous frame are forgotten.
pub fn begin_frame() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.drawn.clear();
        s.current = None;
    });
}

/// The next texts drawn belong to `message`'s body, from its first paragraph; `None` when done.
pub fn drawing_message(message: Option<&str>) {
    STATE.with(|s| s.borrow_mut().current = message.map(|m| (m.to_owned(), 0)));
}

/// Leaves the message's selection for a while (a card's texts select on their own, as in Swift).
pub fn suspend() -> Option<(String, i32)> {
    STATE.with(|s| s.borrow_mut().current.take())
}
pub fn resume(saved: Option<(String, i32)>) {
    STATE.with(|s| s.borrow_mut().current = saved);
}

/// Takes the next unit of the message being drawn, if any.
pub fn next_unit() -> Option<Unit> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let (message, segment) = s.current.as_mut()?;
        let unit = Unit {
            message: message.clone(),
            segment: *segment,
        };
        *segment += 1;
        Some(unit)
    })
}

/// Records a unit drawn this frame; `bounds` is filled when its text is prepainted.
pub fn register(
    unit: Unit,
    text: String,
    layout: TextLayout,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    links: Vec<(Range<usize>, String)>,
) {
    STATE.with(|s| {
        s.borrow_mut().drawn.push(Drawn {
            unit,
            text,
            layout,
            bounds,
            links,
        })
    });
}

/// The selected byte range of `unit`, whose text is `len` bytes long.
pub fn selected(unit: &Unit, len: usize) -> Option<Range<usize>> {
    STATE.with(|s| s.borrow().range(unit, len))
}

pub fn has_selection() -> bool {
    STATE.with(|s| {
        let s = s.borrow();
        matches!((&s.anchor, &s.focus), (Some(a), Some(f)) if a != f)
    })
}

pub fn clear() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.anchor = None;
        s.focus = None;
        s.dragging = false;
    });
}

/// ⌘A after a click in a message: all of that message's text, author line included.
pub fn select_message() -> bool {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(message) = s.anchor.as_ref().map(|a| a.unit.message.clone()) else {
            return false;
        };
        let mut units: Vec<(i32, usize)> = s
            .drawn
            .iter()
            .filter(|d| d.unit.message == message)
            .map(|d| (d.unit.segment, d.text.len()))
            .collect();
        units.sort_unstable();
        let (Some(first), Some(last)) = (units.first().copied(), units.last().copied()) else {
            return false;
        };
        s.anchor = Some(Spot { unit: Unit { message: message.clone(), segment: first.0 }, index: 0 });
        s.focus = Some(Spot { unit: Unit { message, segment: last.0 }, index: last.1 });
        true
    })
}

/// For `OXIDE_SCRIPT`: selects from the middle of the second text drawn to the middle of the last one.
pub fn debug_select(lists: Vec<Vec<String>>) -> bool {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.order = lists
            .iter()
            .enumerate()
            .flat_map(|(list, ids)| ids.iter().enumerate().map(move |(row, id)| (id.clone(), (list as u8, row))))
            .collect();
        let mut drawn: Vec<(Unit, usize)> = s.drawn.iter().map(|d| (d.unit.clone(), d.text.len())).collect();
        drawn.sort_by_key(|(u, _)| s.order.get(&u.message).copied().map(|(l, r)| (l, r, u.segment)));
        let (Some(first), Some(last)) = (drawn.get(1).cloned(), drawn.last().cloned()) else {
            return false;
        };
        s.anchor = Some(Spot { index: first.1 / 2, unit: first.0 });
        s.focus = Some(Spot { index: last.1 / 2, unit: last.0 });
        true
    })
}

/// The selected text: each unit's selected part on its own line, a blank line between messages.
pub fn text() -> Option<String> {
    let text = STATE.with(|s| s.borrow().text());
    (!text.is_empty()).then_some(text)
}

/// Copies the selection; false when nothing is selected.
pub fn copy(cx: &mut App) -> bool {
    match text() {
        Some(text) => {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            true
        }
        None => false,
    }
}

/// A message's author line ("Alice · 11:25"), selectable along with its text (`MessageAuthorLine`).
/// `name` takes `name_style`, the rest `rest_style`; the selected part gets `selection` behind it.
pub fn author_line(
    message: &str,
    name: String,
    rest: String,
    name_style: gpui::HighlightStyle,
    rest_style: gpui::HighlightStyle,
    selection: Hsla,
) -> gpui::AnyElement {
    let unit = Unit {
        message: message.to_owned(),
        segment: AUTHOR,
    };
    let text = format!("{name}{rest}");
    let split = name.len();
    let selected = self::selected(&unit, text.len());
    let mut cuts = vec![0, split, text.len()];
    if let Some(r) = &selected {
        cuts.extend([r.start, r.end]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let highlights: Vec<(Range<usize>, gpui::HighlightStyle)> = cuts
        .windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| {
            let mut style = if w[0] < split { name_style } else { rest_style };
            if selected.as_ref().is_some_and(|r| r.start <= w[0] && w[1] <= r.end) {
                style.background_color = Some(selection);
            }
            (w[0]..w[1], style)
        })
        .collect();
    let styled = gpui::StyledText::new(text.clone()).with_highlights(highlights);
    let bounds = Rc::new(Cell::new(None));
    register(unit, text, styled.layout().clone(), bounds.clone(), Vec::new());
    // One line, cut with an ellipsis when the column is narrow (`lineBreakMode = .byTruncatingTail`).
    gpui::div()
        .min_w_0()
        .truncate()
        .cursor(gpui::CursorStyle::IBeam)
        .on_children_prepainted(move |b, _, _| bounds.set(b.first().copied()))
        .child(styled)
        .into_any_element()
}

/// Splits `runs` so `range` is drawn on the selection colour.
pub fn highlight(runs: Vec<TextRun>, range: Range<usize>, color: Hsla) -> Vec<TextRun> {
    if range.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let cuts = [start, range.start.clamp(start, end), range.end.clamp(start, end), end];
        for w in cuts.windows(2) {
            if w[1] > w[0] {
                let mut piece = run.clone();
                piece.len = w[1] - w[0];
                if w[0] >= range.start && w[1] <= range.end {
                    piece.background_color = Some(color);
                }
                out.push(piece);
            }
        }
        start = end;
    }
    out
}

/// What a press, drag or release is about: the feed's message order and plain texts, read at the press.
pub struct Feed {
    /// Message ids in feed order, per list (main feed, then the reply pane).
    pub lists: Vec<Vec<String>>,
    /// Plain text of each message (author line, then body), for copying ones scrolled away.
    /// A message's plain text (author line, then body), asked only for those copied whole.
    pub plain: Rc<dyn Fn(&str) -> Vec<String>>,
}

/// The feed's mouse listener: lay it over the conversation (it draws nothing). `feed` is read on a press;
/// `open` follows a link clicked without a drag; `blur` lets ⌘C reach the feed rather than the composer.
pub fn listener(
    feed: impl Fn(&mut App) -> Feed + 'static,
    open: impl Fn(String, &mut App) + 'static,
) -> impl IntoElement {
    let feed = Rc::new(feed);
    let open = Rc::new(open);
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let feed = feed.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if !phase.bubble() || event.button != MouseButton::Left {
                    return;
                }
                let Some(spot) = STATE.with(|s| s.borrow().hit(event.position, false)) else {
                    // A press away from any text drops the selection, as clicking elsewhere does.
                    if has_selection() {
                        clear();
                        window.refresh();
                    }
                    return;
                };
                let Feed { lists, plain } = feed(cx);
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    s.order = lists
                        .iter()
                        .enumerate()
                        .flat_map(|(list, ids)| {
                            ids.iter().enumerate().map(move |(row, id)| (id.clone(), (list as u8, row)))
                        })
                        .collect();
                    s.plain = Some(plain);
                    s.press(spot, event.click_count, event.modifiers.shift);
                });
                // The selection is the feed's now: ⌘C must not go to the composer.
                window.blur();
                window.refresh();
            });
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                if !phase.bubble() || event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let moved = STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    if !s.dragging {
                        return false;
                    }
                    let Some(spot) = s.hit(event.position, true) else { return false };
                    if s.focus.as_ref() == Some(&spot) {
                        return false;
                    }
                    s.focus = Some(spot);
                    true
                });
                if moved {
                    window.refresh();
                }
            });
            let open = open.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if !phase.bubble() || event.button != MouseButton::Left {
                    return;
                }
                let link = STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    if !s.dragging {
                        return None;
                    }
                    s.dragging = false;
                    // A plain click: no selection, but a link under it opens.
                    if s.anchor.is_some() && s.anchor == s.focus && event.click_count == 1 {
                        let spot = s.anchor.take()?;
                        s.focus = None;
                        return s.link_at(&spot);
                    }
                    None
                });
                if let Some(url) = link {
                    open(url, cx);
                }
                window.refresh();
            });
        },
    )
    .absolute()
    .size_full()
}

impl State {
    /// The text spot under `point`; while dragging, the nearest one above or below it instead.
    fn hit(&self, point: Point<Pixels>, nearest: bool) -> Option<Spot> {
        let mut painted: Vec<(&Drawn, Bounds<Pixels>)> =
            self.drawn.iter().filter_map(|d| d.bounds.get().map(|b| (d, b))).collect();
        let inside = painted.iter().find(|(_, b)| {
            // A little slack around lines, as text views take a press just beside a word.
            let b = b.dilate(gpui::px(2.));
            b.contains(&point)
        });
        if let Some((d, _)) = inside {
            return Some(Spot {
                unit: d.unit.clone(),
                index: index_in(d, point),
            });
        }
        if !nearest {
            return None;
        }
        painted.sort_by(|a, b| a.1.top().partial_cmp(&b.1.top()).unwrap_or(std::cmp::Ordering::Equal));
        // Above every text: its start; between two: the start of the next; below all: the end of the last.
        for (d, b) in &painted {
            if point.y < b.top() {
                return Some(Spot { unit: d.unit.clone(), index: 0 });
            }
            if point.y <= b.bottom() {
                let index = if point.x < b.left() { 0 } else if point.x > b.right() { d.text.len() } else { index_in(d, point) };
                return Some(Spot { unit: d.unit.clone(), index });
            }
        }
        painted.last().map(|(d, _)| Spot {
            unit: d.unit.clone(),
            index: d.text.len(),
        })
    }

    fn press(&mut self, spot: Spot, clicks: usize, extend: bool) {
        self.dragging = true;
        if extend && self.anchor.is_some() {
            self.focus = Some(spot);
            return;
        }
        let text = self.drawn.iter().find(|d| d.unit == spot.unit).map(|d| d.text.clone()).unwrap_or_default();
        match clicks {
            // A double click takes the word, a triple click the paragraph, as in a text view.
            2 => {
                let (start, end) = word_at(&text, spot.index);
                self.anchor = Some(Spot { unit: spot.unit.clone(), index: start });
                self.focus = Some(Spot { unit: spot.unit, index: end });
            }
            n if n >= 3 => {
                self.anchor = Some(Spot { unit: spot.unit.clone(), index: 0 });
                self.focus = Some(Spot { unit: spot.unit, index: text.len() });
            }
            _ => {
                self.anchor = Some(spot.clone());
                self.focus = Some(spot);
            }
        }
    }

    fn link_at(&self, spot: &Spot) -> Option<String> {
        let drawn = self.drawn.iter().find(|d| d.unit == spot.unit)?;
        drawn
            .links
            .iter()
            .find(|(r, _)| r.start <= spot.index && spot.index < r.end)
            .map(|(_, url)| url.clone())
    }

    /// Feed position of a unit: list, row, then segment within the message.
    fn rank(&self, unit: &Unit) -> Option<(u8, usize, i32)> {
        let (list, row) = self.order.get(&unit.message)?;
        Some((*list, *row, unit.segment))
    }

    fn ends(&self) -> Option<(Spot, Spot)> {
        let (a, f) = (self.anchor.clone()?, self.focus.clone()?);
        let (ra, rf) = (self.rank(&a.unit)?, self.rank(&f.unit)?);
        Some(if (rf, f.index) < (ra, a.index) { (f, a) } else { (a, f) })
    }

    fn range(&self, unit: &Unit, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.ends()?;
        if start == end {
            return None;
        }
        let r = self.rank(unit)?;
        let (rs, re) = (self.rank(&start.unit)?, self.rank(&end.unit)?);
        if r < rs || r > re {
            return None;
        }
        let lower = if r == rs { start.index.min(len) } else { 0 };
        let upper = if r == re { end.index.min(len) } else { len };
        (upper > lower).then(|| lower..upper)
    }

    fn text(&self) -> String {
        let Some((start, end)) = self.ends() else { return String::new() };
        if start == end {
            return String::new();
        }
        let (Some(rs), Some(re)) = (self.rank(&start.unit), self.rank(&end.unit)) else {
            return String::new();
        };
        let mut rows: Vec<(&String, (u8, usize))> = self
            .order
            .iter()
            .filter(|(_, r)| (r.0, r.1) >= (rs.0, rs.1) && (r.0, r.1) <= (re.0, re.1))
            .map(|(id, r)| (id, *r))
            .collect();
        rows.sort_by_key(|(_, r)| *r);
        let mut parts: Vec<String> = Vec::new();
        for (ix, (message, _)) in rows.iter().enumerate() {
            if ix > 0 {
                parts.push(String::new());
            }
            // Units drawn this frame give exact ranges; a message scrolled away is taken whole.
            let mut drawn: Vec<&Drawn> = self.drawn.iter().filter(|d| &d.unit.message == *message).collect();
            if drawn.is_empty() {
                parts.extend(self.plain.as_ref().map(|plain| plain(message)).unwrap_or_default());
                continue;
            }
            drawn.sort_by_key(|d| d.unit.segment);
            for d in drawn {
                if let Some(r) = self.range(&d.unit, d.text.len()) {
                    parts.push(d.text[r].to_owned());
                }
            }
        }
        while parts.last().is_some_and(|p| p.is_empty()) {
            parts.pop();
        }
        parts.join("\n")
    }
}

fn index_in(d: &Drawn, point: Point<Pixels>) -> usize {
    let index = match d.layout.index_for_position(point) {
        Ok(i) | Err(i) => i,
    };
    let mut index = index.min(d.text.len());
    while index > 0 && !d.text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The word around `index` (its bounds), or just that spot between words.
fn word_at(text: &str, index: usize) -> (usize, usize) {
    for (start, word) in text.unicode_word_indices() {
        let end = start + word.len();
        if start <= index && index <= end {
            return (start, end);
        }
    }
    (index, index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(m: &str, s: i32) -> Unit {
        Unit { message: m.into(), segment: s }
    }

    fn state() -> State {
        let mut s = State::default();
        s.order = [("a".to_owned(), (0, 0)), ("b".to_owned(), (0, 1)), ("c".to_owned(), (0, 2))].into();
        s.plain = Some(Rc::new(|id: &str| {
            if id == "b" { vec!["Bob".to_owned(), "middle message".to_owned()] } else { Vec::new() }
        }));
        s
    }

    #[test]
    fn ranges_cover_every_unit_between_both_ends_in_feed_order() {
        let mut s = state();
        // Dragged upwards: from the end of c's body to inside a's body.
        s.anchor = Some(Spot { unit: unit("c", 0), index: 3 });
        s.focus = Some(Spot { unit: unit("a", 0), index: 2 });
        assert_eq!(s.range(&unit("a", 0), 10), Some(2..10));
        assert_eq!(s.range(&unit("a", AUTHOR), 5), None);
        assert_eq!(s.range(&unit("b", AUTHOR), 3), Some(0..3));
        assert_eq!(s.range(&unit("c", 0), 10), Some(0..3));
        assert_eq!(s.range(&unit("c", 1), 10), None);
    }

    #[test]
    fn copied_text_takes_off_screen_messages_whole_with_blank_lines_between() {
        let mut s = state();
        s.anchor = Some(Spot { unit: unit("a", 0), index: 6 });
        s.focus = Some(Spot { unit: unit("c", 0), index: 5 });
        // a and c are on screen with exact ranges; b is scrolled away.
        for (u, text) in [(unit("a", 0), "Hello world"), (unit("c", 0), "Bye now")] {
            s.drawn.push(Drawn {
                unit: u,
                text: text.into(),
                layout: TextLayout::default(),
                bounds: Rc::default(),
                links: Vec::new(),
            });
        }
        assert_eq!(s.text(), "world\n\nBob\nmiddle message\n\nBye n");
    }

    #[test]
    fn a_click_selects_nothing_and_words_split_on_boundaries() {
        let mut s = state();
        s.anchor = Some(Spot { unit: unit("a", 0), index: 2 });
        s.focus = s.anchor.clone();
        assert_eq!(s.range(&unit("a", 0), 10), None);
        assert_eq!(s.text(), "");
        assert_eq!(word_at("salut tout le monde", 8), (6, 10));
        assert_eq!(word_at("salut tout", 5), (0, 5));
    }

    #[test]
    fn highlight_splits_runs_at_the_selection() {
        let run = |len| TextRun {
            len,
            font: gpui::font("Figtree"),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let out = highlight(vec![run(5), run(5)], 3..7, gpui::red());
        let lens: Vec<_> = out.iter().map(|r| (r.len, r.background_color.is_some())).collect();
        assert_eq!(lens, vec![(3, false), (2, true), (2, true), (3, false)]);
    }

    /// Drives the real listener with a mouse: press in one message, drag into the next, release.
    mod mouse {
        use super::super::*;
        use gpui::{Context, Modifiers, TestAppContext, Window, div, point, px};

        struct Host;
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                begin_frame();
                let style = gpui::HighlightStyle::default();
                let line = |m: &str, text: &str| author_line(m, text.into(), String::new(), style, style, gpui::red());
                div()
                    .relative()
                    .size_full()
                    .child(listener(
                        |_| Feed {
                            lists: vec![vec!["a".into(), "b".into()]],
                            plain: Rc::new(|_: &str| Vec::new()),
                        },
                        |_, _| {},
                    ))
                    .child(div().h(px(30.)).child(line("a", "Hello world")))
                    .child(div().h(px(30.)).child(line("b", "Second line")))
            }
        }

        fn bounds_of(message: &str) -> Bounds<Pixels> {
            STATE.with(|s| {
                s.borrow()
                    .drawn
                    .iter()
                    .find(|d| d.unit.message == message)
                    .and_then(|d| d.bounds.get())
                    .expect("drawn")
            })
        }

        #[gpui::test]
        fn dragging_from_one_message_into_the_next_selects_across_them(cx: &mut TestAppContext) {
            clear();
            let (_, cx) = cx.add_window_view(|_, _| Host);
            cx.run_until_parked();
            let a = bounds_of("a");
            let b = bounds_of("b");
            let start = point(a.left() + px(1.), a.center().y);
            cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            // Inside "Second line", a few letters in.
            let end = point(b.left() + px(20.), b.center().y);
            cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            let copied = text().expect("a selection");
            assert!(copied.starts_with("Hello world\n\nS"), "got {copied:?}");
            assert!(!copied.ends_with("line"), "stops under the pointer, got {copied:?}");
            // A plain click elsewhere in text drops it.
            cx.simulate_click(start, Modifiers::default());
            assert_eq!(text(), None);
        }
    }
}
