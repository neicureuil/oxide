//! A text editor: single or multi-line, wrapping, mouse and keyboard selection, IME, clipboard, undo.
//! Owners style ranges through `highlights` (mentions) and listen to [`InputEvent`]s. A `rich` editor also keeps
//! the composer's formats ([`ComposerFormat`] runs) as AppKit's text storage does in the macOS app: they follow
//! edits, typed text takes the format around it, paragraphs show their size, indent and list marker.
use gpui::{
    App, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    FontStyle, FontWeight, GlobalElementId, HighlightStyle, Hsla, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px, size,
};
use std::ops::Range;
use teams_core::{ComposerBlock, ComposerFormat};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    text_input,
    [
        Backspace,
        BackspaceWord,
        BackspaceLine,
        Delete,
        Left,
        Right,
        Up,
        Down,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocStart,
        DocEnd,
        Enter,
        Newline,
        SubmitNow,
        Escape,
        Tab,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        FormatBold,
        FormatItalic,
        FormatUnderline,
        FormatStrike,
        FormatLink,
    ]
);

const CONTEXT: &str = "TextInput";
/// Inset of an indent step, and of a list or quote's text after its marker, as in `ComposerText.attributes`.
const INDENT: f32 = 24.;
/// The character a pasted image takes in the draft (AppKit's attachment character).
pub const IMAGE_CHARACTER: char = '\u{FFFC}';
/// Drawn in its place: same UTF-8 length, so offsets still match the text.
const IMAGE_STAND_IN: char = '\u{25A3}';

/// `secondary` is ⌘ on macOS and Ctrl elsewhere (GPUI reads `cmd` as the Windows key there). Moving by word or
/// line follows each system: ⌥ / ⌘ arrows on macOS, Ctrl arrows and Home / End elsewhere.
pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("shift-enter", Newline, c),
        KeyBinding::new("alt-enter", Newline, c),
        KeyBinding::new("secondary-enter", SubmitNow, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("secondary-v", Paste, c),
        KeyBinding::new("secondary-c", Copy, c),
        KeyBinding::new("secondary-x", Cut, c),
        KeyBinding::new("secondary-z", Undo, c),
        KeyBinding::new("secondary-shift-z", Redo, c),
        KeyBinding::new("secondary-b", FormatBold, c),
        KeyBinding::new("secondary-i", FormatItalic, c),
        KeyBinding::new("secondary-u", FormatUnderline, c),
        KeyBinding::new("secondary-shift-x", FormatStrike, c),
        KeyBinding::new("secondary-k", FormatLink, c),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("alt-backspace", BackspaceWord, c),
        KeyBinding::new("cmd-backspace", BackspaceLine, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("cmd-left", Home, c),
        KeyBinding::new("cmd-right", End, c),
        KeyBinding::new("cmd-shift-left", SelectHome, c),
        KeyBinding::new("cmd-shift-right", SelectEnd, c),
        KeyBinding::new("cmd-up", DocStart, c),
        KeyBinding::new("cmd-down", DocEnd, c),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-backspace", BackspaceWord, c),
        KeyBinding::new("ctrl-left", WordLeft, c),
        KeyBinding::new("ctrl-right", WordRight, c),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("ctrl-home", DocStart, c),
        KeyBinding::new("ctrl-end", DocEnd, c),
        KeyBinding::new("ctrl-y", Redo, c),
    ]);
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    /// The text changed by typing, pasting or undo (not by `set_text`); in a rich editor, its formats too.
    Changed,
    /// Return in a field that submits, or ⌘ Return.
    Submit,
    /// Escape.
    Cancel,
    /// Up or Down with the cursor already on the first or last line, or Tab: for completion lists.
    Up,
    Down,
    Tab,
}

/// What only a rich editor reports, apart from [`InputEvent`] so its matches stay as they are.
#[derive(Clone, Debug)]
pub enum RichEvent {
    /// ⌘K: the owner asks for the link.
    Link,
    /// An image pasted on its own (a screenshot): the owner uploads it and inserts its placeholder.
    PasteImage(gpui::Image),
}

/// A snapshot for undo: text, selection and formats.
type Snapshot = (String, Range<usize>, Vec<(Range<usize>, ComposerFormat)>);

pub struct TextInput {
    focus: FocusHandle,
    text: String,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    multiline: bool,
    /// Return emits `Submit` (Shift-Return then breaks the line); otherwise Return breaks the line.
    submits: bool,
    disabled: bool,
    /// Up, Down, Return, Tab and Escape go to the owner's list (a mention being typed) instead of the text.
    completing: bool,
    /// Styles over byte ranges, drawn on top of the field's text style.
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Keeps and draws the composer's formats.
    rich: bool,
    /// Formats over byte ranges, sorted and apart; text outside them is plain.
    runs: Vec<(Range<usize>, ComposerFormat)>,
    /// The format the next typed text takes, chosen with nothing selected (AppKit's typing attributes).
    typing: Option<ComposerFormat>,
    /// Laid out at the last paint, one per paragraph.
    lines: Vec<Line>,
    bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    selecting: bool,
    /// Horizontal position kept while moving up and down.
    goal_x: Option<Pixels>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

/// A laid-out paragraph: byte start, top, left inset, line height, its wrapped text and list marker.
#[derive(Clone)]
struct Line {
    start: usize,
    top: Pixels,
    x: Pixels,
    height: Pixels,
    wrapped: WrappedLine,
    marker: Option<(Pixels, ShapedLine)>,
}

impl EventEmitter<InputEvent> for TextInput {}
impl EventEmitter<RichEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            focus: cx.focus_handle(),
            text: String::new(),
            selected: 0..0,
            reversed: false,
            marked: None,
            placeholder: placeholder.into(),
            multiline: false,
            submits: true,
            disabled: false,
            completing: false,
            highlights: Vec::new(),
            rich: false,
            runs: Vec::new(),
            typing: None,
            lines: Vec::new(),
            bounds: None,
            line_height: px(18.),
            selecting: false,
            goal_x: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
    /// Several lines; `submits` keeps Return for sending (Shift-Return breaks the line).
    pub fn multiline(mut self, submits: bool) -> Self {
        self.multiline = true;
        self.submits = submits;
        self
    }
    /// Keeps composer formats: see [`TextInput::set_rich_text`] and [`TextInput::formats`].
    pub fn rich(mut self) -> Self {
        self.rich = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }
    pub fn selection(&self) -> Range<usize> {
        self.selected.clone()
    }
    pub fn has_marked_text(&self) -> bool {
        self.marked.is_some()
    }
    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }
    pub fn set_submits(&mut self, submits: bool) {
        self.submits = submits;
    }
    pub fn set_completing(&mut self, completing: bool) {
        self.completing = completing;
    }
    /// Replaces the whole text from outside (no `Changed`), putting the cursor at `cursor` (a byte offset) or the end.
    pub fn set_text(
        &mut self,
        text: impl Into<String>,
        cursor: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        if text == self.text && cursor.is_none() {
            return;
        }
        let at = cursor.unwrap_or(text.len()).min(text.len());
        let at = floor_char(&text, at);
        self.text = text;
        self.selected = at..at;
        self.reversed = false;
        self.marked = None;
        self.runs.clear();
        self.typing = None;
        self.highlights.retain(|(r, _)| r.end <= self.text.len());
        cx.notify();
    }
    /// Shows the draft the core holds, with its formats, the caret at `cursor` (UTF-16, as the core counts).
    pub fn set_rich_text(
        &mut self,
        text: impl Into<String>,
        formats: &[ComposerFormat],
        cursor: u32,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        let at = byte_offset(&text, cursor as usize);
        self.runs = formats
            .iter()
            .filter_map(|f| {
                let start = byte_offset(&text, f.start as usize);
                let end = byte_offset(&text, (f.start + f.length) as usize);
                (start < end && !is_plain(f)).then(|| (start..end, f.clone()))
            })
            .collect();
        self.runs.sort_by_key(|(r, _)| r.start);
        self.text = text;
        self.selected = at..at;
        self.reversed = false;
        self.marked = None;
        self.typing = None;
        self.goal_x = None;
        self.highlights.retain(|(r, _)| r.end <= self.text.len());
        cx.notify();
    }
    /// The formats over the text, in UTF-16 units (as `RichDraftChanged` takes them).
    pub fn formats(&self) -> Vec<ComposerFormat> {
        self.runs
            .iter()
            .map(|(r, f)| {
                let mut f = f.clone();
                f.start = self.utf8_to_utf16(r.start) as u32;
                f.length = self.text[r.clone()].encode_utf16().count() as u32;
                f
            })
            .collect()
    }
    pub fn select(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let len = self.text.len();
        self.selected = floor_char(&self.text, range.start.min(len))
            ..floor_char(&self.text, range.end.min(len));
        self.reversed = false;
        self.typing = None;
        cx.notify();
    }
    /// Replaces the selection as if typed (emits `Changed`).
    pub fn insert(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, text, window, cx);
    }
    /// Same, the new text taking `style` (a pasted image's placeholder).
    pub fn insert_styled(
        &mut self,
        text: &str,
        style: ComposerFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.typing = Some(style);
        self.replace_text_in_range(None, text, window, cx);
        self.typing = None;
    }
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }
    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// A byte offset as UTF-16 units, and back.
    pub fn utf16(&self, offset: usize) -> u32 {
        self.utf8_to_utf16(offset) as u32
    }
    pub fn utf8(&self, offset: u32) -> usize {
        byte_offset(&self.text, offset as usize)
    }
    /// The selection in UTF-16 units.
    pub fn selection_utf16(&self) -> Range<u32> {
        self.utf16(self.selected.start)..self.utf16(self.selected.end)
    }
    /// Where the character at `offset` is drawn, in window coordinates, after the last paint.
    pub fn bounds_for(&self, offset: usize) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds?;
        let (p, height) = self
            .position_for(offset.min(self.text.len()))
            .unwrap_or((point(px(0.), px(0.)), self.line_height));
        Some(Bounds::new(bounds.origin + p, size(px(1.), height)))
    }

    /// The format at the selection: its first character's, or what typing would take (Swift's `refreshFormat`).
    pub fn current_format(&self) -> ComposerFormat {
        if !self.selected.is_empty() {
            return self
                .run_at(self.selected.start)
                .cloned()
                .unwrap_or_default();
        }
        self.typing
            .clone()
            .unwrap_or_else(|| self.inherited(self.selected.clone()))
    }
    /// Changes the formats of the selection, or of its paragraphs; with nothing to change, of the text typed next.
    pub fn change_format(
        &mut self,
        paragraphs: bool,
        transform: impl Fn(&mut ComposerFormat),
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = if paragraphs {
            self.paragraph_range(self.selected.clone())
        } else {
            self.selected.clone()
        };
        if range.is_empty() {
            let mut style = self.current_format();
            transform(&mut style);
            self.typing = Some(style);
            cx.notify();
            return;
        }
        self.push_undo();
        let mut out = Vec::new();
        let mut at = range.start;
        for (r, f) in &self.runs {
            if r.end <= range.start || r.start >= range.end {
                out.push((r.clone(), f.clone()));
                continue;
            }
            if r.start < range.start {
                out.push((r.start..range.start, f.clone()));
            }
            let (s, e) = (r.start.max(range.start), r.end.min(range.end));
            if at < s {
                let mut plain = ComposerFormat::default();
                transform(&mut plain);
                out.push((at..s, plain));
            }
            let mut changed = f.clone();
            transform(&mut changed);
            out.push((s..e, changed));
            at = e;
            if r.end > range.end {
                out.push((range.end..r.end, f.clone()));
            }
        }
        if at < range.end {
            let mut plain = ComposerFormat::default();
            transform(&mut plain);
            out.push((at..range.end, plain));
        }
        self.runs = tidy(out);
        self.typing = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }
    /// The paragraphs `range` touches, with their line breaks, as `NSString.paragraphRange`.
    pub fn paragraph_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.text[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[range.end..]
            .find('\n')
            .map_or(self.text.len(), |i| range.end + i + 1);
        start..end
    }
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.undo.pop() {
            let current = self.snapshot();
            self.redo.push(current);
            self.restore(snapshot, cx);
        }
    }
    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.redo.pop() {
            let current = self.snapshot();
            self.undo.push(current);
            self.restore(snapshot, cx);
        }
    }

    fn snapshot(&self) -> Snapshot {
        (self.text.clone(), self.selected.clone(), self.runs.clone())
    }
    fn restore(&mut self, (text, selected, runs): Snapshot, cx: &mut Context<Self>) {
        self.text = text;
        self.selected = selected;
        self.runs = runs;
        self.typing = None;
        self.highlights.retain(|(r, _)| r.end <= self.text.len());
        cx.emit(InputEvent::Changed);
        cx.notify();
    }
    fn push_undo(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    fn run_at(&self, offset: usize) -> Option<&ComposerFormat> {
        self.runs
            .iter()
            .find(|(r, _)| r.start <= offset && offset < r.end)
            .map(|(_, f)| f)
    }
    /// The format text replacing `range` takes: its first character's, else the one before it, except at a paragraph
    /// start, where the paragraph's own; like AppKit, neither an image nor a link carries on.
    fn inherited(&self, range: Range<usize>) -> ComposerFormat {
        let at = if !range.is_empty() {
            Some(range.start)
        } else if range.start > 0 && self.text.as_bytes()[range.start - 1] != b'\n' {
            Some(self.previous_boundary(range.start))
        } else if range.start < self.text.len() {
            Some(range.start)
        } else if range.start > 0 {
            Some(range.start - 1)
        } else {
            None
        };
        let mut style = at.and_then(|i| self.run_at(i)).cloned().unwrap_or_default();
        style.image = None;
        style.link = None;
        style
    }

    /// Replaces `range` with `new`, moving highlights and formats with their text; the new text takes `style`.
    fn splice(&mut self, range: Range<usize>, new: &str, style: Option<ComposerFormat>) {
        // Styles after the edit move with their text; those inside it stretch or shrink.
        let delta = new.len() as isize - range.len() as isize;
        let shift = |x: usize| -> usize {
            if x >= range.end {
                (x as isize + delta) as usize
            } else if x > range.start {
                range.start + new.len().min(x - range.start)
            } else {
                x
            }
        };
        for (r, _) in &mut self.highlights {
            *r = shift(r.start)..shift(r.end);
        }
        self.highlights.retain(|(r, _)| !r.is_empty());
        if self.rich {
            let moved = |x: usize| (x as isize + delta) as usize;
            let mut out = Vec::new();
            for (r, f) in self.runs.drain(..) {
                if r.start < range.start {
                    out.push((r.start..r.end.min(range.start), f.clone()));
                }
                if r.end > range.end {
                    out.push((moved(r.start.max(range.end))..moved(r.end), f));
                }
            }
            if let Some(style) = style.filter(|_| !new.is_empty()) {
                out.push((range.start..range.start + new.len(), style));
            }
            out.sort_by_key(|(r, _)| r.start);
            self.runs = tidy(out);
        }
        self.text.replace_range(range, new);
    }

    fn edit(&mut self, range: Range<usize>, new: &str, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        self.push_undo();
        let new = if self.multiline {
            new.replace("\r\n", "\n")
        } else {
            new.replace(['\n', '\r'], " ")
        };
        let style = self.rich.then(|| {
            self.typing
                .take()
                .unwrap_or_else(|| self.inherited(range.clone()))
        });
        self.splice(range.clone(), &new, style);
        let at = range.start + new.len();
        self.selected = at..at;
        self.reversed = false;
        self.goal_x = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        self.typing = None;
        cx.notify();
    }
    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        self.typing = None;
        cx.notify();
    }
    fn previous_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(i, _)| (i < offset).then_some(i))
            .unwrap_or(0)
    }
    fn next_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(i, _)| (i > offset).then_some(i))
            .unwrap_or(self.text.len())
    }
    fn previous_word(&self, offset: usize) -> usize {
        self.text
            .unicode_word_indices()
            .rev()
            .find_map(|(i, _)| (i < offset).then_some(i))
            .unwrap_or(0)
    }
    fn next_word(&self, offset: usize) -> usize {
        self.text
            .unicode_word_indices()
            .find_map(|(i, w)| (i + w.len() > offset).then_some(i + w.len()))
            .unwrap_or(self.text.len())
    }
    fn line_start(&self, offset: usize) -> usize {
        // The start of the visual line holding `offset`, else of its paragraph.
        if let Some((start, ix)) = self.visual_line_of(offset) {
            return start + ix;
        }
        self.text[..offset].rfind('\n').map_or(0, |i| i + 1)
    }
    fn line_end(&self, offset: usize) -> usize {
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |i| offset + i)
    }
    fn line_of(&self, offset: usize) -> Option<&Line> {
        self.lines.iter().rev().find(|l| l.start <= offset)
    }
    /// The paragraph start and the offset (inside it) where the visual line holding `offset` starts.
    fn visual_line_of(&self, offset: usize) -> Option<(usize, usize)> {
        let line = self.line_of(offset)?;
        let local = offset - line.start;
        let mut begin = 0;
        for b in line.wrapped.wrap_boundaries.iter() {
            let run = &line.wrapped.unwrapped_layout.runs[b.run_ix];
            let ix = run.glyphs[b.glyph_ix].index;
            if ix <= local {
                begin = ix;
            }
        }
        Some((line.start, begin))
    }

    /// Where `offset` is drawn inside the field, with the height of its line.
    fn position_for(&self, offset: usize) -> Option<(Point<Pixels>, Pixels)> {
        let line = self.line_of(offset)?;
        let p = line
            .wrapped
            .position_for_index(offset - line.start, line.height)?;
        Some((point(line.x + p.x, line.top + p.y), line.height))
    }
    fn offset_for(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds else { return 0 };
        let local = position - bounds.origin;
        if local.y < px(0.) {
            return 0;
        }
        let Some(line) = self
            .lines
            .iter()
            .rev()
            .find(|l| l.top <= local.y)
            .or(self.lines.first())
        else {
            return 0;
        };
        let ix = match line
            .wrapped
            .closest_index_for_position(point(local.x - line.x, local.y - line.top), line.height)
        {
            Ok(i) | Err(i) => i,
        };
        (line.start + ix).min(self.text.len())
    }
    fn vertical(&mut self, down: bool) -> Option<usize> {
        let (at, height) = self.position_for(self.cursor())?;
        let x = *self.goal_x.get_or_insert(at.x);
        let y = if down {
            at.y + height + px(1.)
        } else {
            at.y - px(1.)
        };
        let bounds = self.bounds?;
        let total: Pixels = self
            .lines
            .last()
            .map(|l| l.top + l.wrapped.size(l.height).height)
            .unwrap_or_default();
        if y < px(0.) || y >= total {
            return None;
        }
        Some(self.offset_for(bounds.origin + point(x, y)))
    }

    // Actions.
    fn backspace(&mut self, _: &Backspace, w: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.previous_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", w, cx);
    }
    fn backspace_word(&mut self, _: &BackspaceWord, w: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.previous_word(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", w, cx);
    }
    fn backspace_line(&mut self, _: &BackspaceLine, w: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.line_start(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", w, cx);
    }
    fn delete(&mut self, _: &Delete, w: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.next_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", w, cx);
    }
    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        let to = if self.selected.is_empty() {
            self.previous_boundary(self.cursor())
        } else {
            self.selected.start
        };
        self.move_to(to, cx);
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        let to = if self.selected.is_empty() {
            self.next_boundary(self.cursor())
        } else {
            self.selected.end
        };
        self.move_to(to, cx);
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.completing && self.marked.is_none() {
            return cx.emit(InputEvent::Up);
        }
        match self.multiline.then(|| self.vertical(false)).flatten() {
            Some(to) => self.move_to(to, cx),
            None => cx.emit(InputEvent::Up),
        }
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.completing && self.marked.is_none() {
            return cx.emit(InputEvent::Down);
        }
        match self.multiline.then(|| self.vertical(true)).flatten() {
            Some(to) => self.move_to(to, cx),
            None => cx.emit(InputEvent::Down),
        }
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(false).unwrap_or(0);
        self.select_to(to, cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(true).unwrap_or(self.text.len());
        self.select_to(to, cx);
    }
    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor()), cx);
    }
    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor()), cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor()), cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word(self.cursor()), cx);
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word(self.cursor()), cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.text.len();
        self.reversed = false;
        self.typing = None;
        cx.notify();
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start(self.cursor()), cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(self.cursor()), cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_start(self.cursor()), cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(self.cursor()), cx);
    }
    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }
    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }
    fn enter(&mut self, _: &Enter, w: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        if self.submits || !self.multiline || self.completing {
            cx.emit(InputEvent::Submit);
        } else {
            self.replace_text_in_range(None, "\n", w, cx);
        }
    }
    fn newline(&mut self, _: &Newline, w: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", w, cx);
        } else {
            cx.emit(InputEvent::Submit);
        }
    }
    fn submit_now(&mut self, _: &SubmitNow, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_none() {
            cx.emit(InputEvent::Submit);
        }
    }
    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }
    fn tab(&mut self, _: &Tab, w: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Tab);
        if !self.completing {
            w.focus_next();
        }
    }
    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        w: &mut Window,
        _: &mut Context<Self>,
    ) {
        w.show_character_palette();
    }
    fn paste(&mut self, _: &Paste, w: &mut Window, cx: &mut Context<Self>) {
        let item = cx.read_from_clipboard();
        if let Some(text) = item.as_ref().and_then(|i| i.text()) {
            self.replace_text_in_range(None, &text, w, cx);
            return;
        }
        if !self.rich {
            return;
        }
        let image = item.as_ref().and_then(|item| {
            item.entries().iter().find_map(|e| match e {
                ClipboardEntry::Image(image) => Some(image.clone()),
                _ => None,
            })
        });
        // Windows: a picture offered only as a bitmap, which GPUI doesn't read.
        #[cfg(windows)]
        let image = image.or_else(crate::clipboard_bitmap::read_png);
        if let Some(image) = image {
            cx.emit(RichEvent::PasteImage(image));
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selected.clone()].to_owned(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, w: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selected.clone()].to_owned(),
            ));
            self.replace_text_in_range(None, "", w, cx);
        }
    }
    fn undo_action(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }
    fn redo_action(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }
    /// ⌘B, ⌘I, ⌘U, ⌘⇧X: the composer's shortcuts; other fields let the key go on.
    fn toggle(&mut self, flag: fn(&mut ComposerFormat) -> &mut bool, cx: &mut Context<Self>) {
        if !self.rich {
            return cx.propagate();
        }
        let enabled = !*flag(&mut self.current_format());
        self.change_format(false, |f| *flag(f) = enabled, cx);
    }
    fn bold(&mut self, _: &FormatBold, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle(|f| &mut f.bold, cx);
    }
    fn italic(&mut self, _: &FormatItalic, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle(|f| &mut f.italic, cx);
    }
    fn underline(&mut self, _: &FormatUnderline, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle(|f| &mut f.underline, cx);
    }
    fn strike(&mut self, _: &FormatStrike, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle(|f| &mut f.strike, cx);
    }
    fn link(&mut self, _: &FormatLink, _: &mut Window, cx: &mut Context<Self>) {
        if !self.rich {
            return cx.propagate();
        }
        cx.emit(RichEvent::Link);
    }
    fn mouse_down(&mut self, e: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        w.focus(&self.focus);
        self.selecting = true;
        self.goal_x = None;
        let at = self.offset_for(e.position);
        if e.click_count == 2 {
            let start = self.previous_word(self.next_boundary(at));
            let end = self.next_word(start);
            self.selected = start..end;
            self.reversed = false;
            self.typing = None;
            cx.notify();
        } else if e.click_count >= 3 {
            self.selected = self.text[..at].rfind('\n').map_or(0, |i| i + 1)..self.line_end(at);
            self.typing = None;
            cx.notify();
        } else if e.modifiers.shift {
            self.select_to(at, cx);
        } else {
            self.move_to(at, cx);
        }
    }
    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }
    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.offset_for(e.position), cx);
        }
    }

    fn utf16_to_utf8(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.text.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }
    fn utf8_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .encode_utf16()
            .count()
    }
    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf8_to_utf16(r.start)..self.utf8_to_utf16(r.end)
    }
    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.utf16_to_utf8(r.start)..self.utf16_to_utf8(r.end)
    }
}

fn floor_char(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}
/// The byte offset of `units` UTF-16 units into `text`, kept on a character boundary.
pub fn byte_offset(text: &str, units: usize) -> usize {
    let mut count = 0;
    for (byte, ch) in text.char_indices() {
        if count >= units {
            return byte;
        }
        count += ch.len_utf16();
    }
    text.len()
}
/// A format that changes nothing (the core leaves such text unstyled).
pub fn is_plain(f: &ComposerFormat) -> bool {
    let mut f = f.clone();
    f.start = 0;
    f.length = 0;
    if f.alignment == "left" {
        f.alignment.clear();
    }
    f == ComposerFormat::default()
}
fn same_style(a: &ComposerFormat, b: &ComposerFormat) -> bool {
    let (mut a, mut b) = (a.clone(), b.clone());
    (a.start, a.length, b.start, b.length) = (0, 0, 0, 0);
    a == b && a.image.is_none()
}
/// Drops empty and plain runs and joins neighbours of the same style.
fn tidy(runs: Vec<(Range<usize>, ComposerFormat)>) -> Vec<(Range<usize>, ComposerFormat)> {
    let mut out: Vec<(Range<usize>, ComposerFormat)> = Vec::new();
    for (r, f) in runs {
        if r.is_empty() || is_plain(&f) {
            continue;
        }
        if let Some((last, lf)) = out.last_mut()
            && last.end == r.start
            && same_style(lf, &f)
        {
            last.end = r.end;
            continue;
        }
        out.push((r, f));
    }
    out
}
/// `#rgb` or `#rrggbb`.
pub fn hex_color(hex: &str) -> Option<Hsla> {
    let digits = hex.strip_prefix('#')?;
    let full: String = if digits.len() == 3 {
        digits.chars().flat_map(|c| [c, c]).collect()
    } else {
        digits.to_owned()
    };
    (full.len() == 6)
        .then(|| u32::from_str_radix(&full, 16).ok())
        .flatten()
        .map(|v| gpui::rgb(v).into())
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
        Some(self.text[range].to_owned())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected),
            reversed: self.reversed,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.marked = None;
        self.edit(range, new, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        let style = self.rich.then(|| {
            self.typing
                .clone()
                .unwrap_or_else(|| self.inherited(range.clone()))
        });
        self.splice(range.clone(), new, style);
        self.marked = (!new.is_empty()).then(|| range.start..range.start + new.len());
        self.selected = new_selected
            .as_ref()
            .map(|r| {
                let local = new.encode_utf16().collect::<Vec<_>>();
                let to8 = |u: usize| String::from_utf16_lossy(&local[..u.min(local.len())]).len();
                range.start + to8(r.start)..range.start + to8(r.end)
            })
            .unwrap_or_else(|| range.start + new.len()..range.start + new.len());
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range);
        let (start, height) = self.position_for(range.start)?;
        let end = self
            .position_for(range.end)
            .map(|(p, _)| p)
            .unwrap_or(start);
        Some(Bounds::from_corners(
            bounds.origin + start,
            bounds.origin + point(end.x.max(start.x + px(1.)), start.y + height),
        ))
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.utf8_to_utf16(self.offset_for(p)))
    }
}

/// The colours a field draws with, from the app theme.
#[derive(Clone, Copy)]
pub struct InputColors {
    pub text: Hsla,
    pub placeholder: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
}

/// Draws a [`TextInput`]: the owner wraps it in its own frame (padding, border, scrolling).
pub fn field(input: &Entity<TextInput>, colors: InputColors, cx: &App) -> impl IntoElement {
    let state = input.read(cx);
    div()
        .key_context(CONTEXT)
        .track_focus(&state.focus)
        .cursor(CursorStyle::IBeam)
        .w_full()
        .on_action(window_listener(input, TextInput::backspace))
        .on_action(window_listener(input, TextInput::backspace_word))
        .on_action(window_listener(input, TextInput::backspace_line))
        .on_action(window_listener(input, TextInput::delete))
        .on_action(window_listener(input, TextInput::left))
        .on_action(window_listener(input, TextInput::right))
        .on_action(window_listener(input, TextInput::up))
        .on_action(window_listener(input, TextInput::down))
        .on_action(window_listener(input, TextInput::word_left))
        .on_action(window_listener(input, TextInput::word_right))
        .on_action(window_listener(input, TextInput::select_left))
        .on_action(window_listener(input, TextInput::select_right))
        .on_action(window_listener(input, TextInput::select_up))
        .on_action(window_listener(input, TextInput::select_down))
        .on_action(window_listener(input, TextInput::select_word_left))
        .on_action(window_listener(input, TextInput::select_word_right))
        .on_action(window_listener(input, TextInput::select_all))
        .on_action(window_listener(input, TextInput::home))
        .on_action(window_listener(input, TextInput::end))
        .on_action(window_listener(input, TextInput::select_home))
        .on_action(window_listener(input, TextInput::select_end))
        .on_action(window_listener(input, TextInput::doc_start))
        .on_action(window_listener(input, TextInput::doc_end))
        .on_action(window_listener(input, TextInput::enter))
        .on_action(window_listener(input, TextInput::newline))
        .on_action(window_listener(input, TextInput::submit_now))
        .on_action(window_listener(input, TextInput::escape))
        .on_action(window_listener(input, TextInput::tab))
        .on_action(window_listener(input, TextInput::show_character_palette))
        .on_action(window_listener(input, TextInput::paste))
        .on_action(window_listener(input, TextInput::copy))
        .on_action(window_listener(input, TextInput::cut))
        .on_action(window_listener(input, TextInput::undo_action))
        .on_action(window_listener(input, TextInput::redo_action))
        .on_action(window_listener(input, TextInput::bold))
        .on_action(window_listener(input, TextInput::italic))
        .on_action(window_listener(input, TextInput::underline))
        .on_action(window_listener(input, TextInput::strike))
        .on_action(window_listener(input, TextInput::link))
        .on_mouse_down(
            MouseButton::Left,
            mouse_listener(input, TextInput::mouse_down),
        )
        .on_mouse_up(
            MouseButton::Left,
            mouse_listener(input, TextInput::mouse_up),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            mouse_listener(input, TextInput::mouse_up),
        )
        .on_mouse_move(mouse_listener(input, TextInput::mouse_move))
        .child(TextElement {
            input: input.clone(),
            colors,
        })
}

fn window_listener<A: 'static>(
    input: &Entity<TextInput>,
    f: fn(&mut TextInput, &A, &mut Window, &mut Context<TextInput>),
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let input = input.downgrade();
    move |a, w, cx| {
        let _ = input.update(cx, |i, cx| f(i, a, w, cx));
    }
}
fn mouse_listener<E: 'static>(
    input: &Entity<TextInput>,
    f: fn(&mut TextInput, &E, &mut Window, &mut Context<TextInput>),
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    window_listener(input, f)
}

struct TextElement {
    input: Entity<TextInput>,
    colors: InputColors,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub struct Prepaint {
    lines: Vec<Line>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

/// The field's text style, size and line height.
struct Metrics {
    style: gpui::TextStyle,
    size: Pixels,
    line_height: Pixels,
}
impl Metrics {
    fn of(window: &Window) -> Self {
        let style = window.text_style();
        Self {
            size: style.font_size.to_pixels(window.rem_size()),
            line_height: window.line_height(),
            style,
        }
    }
}

/// How a paragraph is laid out: from its format in a rich editor, else the field's text style.
struct Paragraph {
    font_size: Pixels,
    height: Pixels,
    inset: Pixels,
    marker: Option<(Pixels, SharedString)>,
}

impl TextElement {
    /// Runs of the field's text style with the formats, the owner's highlights and the IME's marked range on top.
    fn runs(&self, text: &str, placeholder: bool, m: &Metrics, cx: &App) -> Vec<TextRun> {
        let style = &m.style;
        let input = self.input.read(cx);
        let base = TextRun {
            len: text.len(),
            font: style.font(),
            color: if placeholder {
                self.colors.placeholder
            } else {
                self.colors.text
            },
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        if placeholder || text.is_empty() {
            return vec![base];
        }
        let mut cuts = vec![0, text.len()];
        let formats: Vec<(Range<usize>, HighlightStyle)> = input
            .runs
            .iter()
            .filter(|(r, _)| r.end <= text.len())
            .map(|(r, f)| (r.clone(), format_style(f, self.colors)))
            .collect();
        let mut styles: Vec<(Range<usize>, HighlightStyle)> = input
            .highlights
            .iter()
            .filter(|(r, _)| r.end <= text.len() && !r.is_empty())
            .cloned()
            .collect();
        if let Some(m) = &input.marked {
            styles.push((
                m.clone(),
                HighlightStyle {
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ));
        }
        let mono = |r: &Range<usize>| {
            input.runs.iter().any(|(f, s)| {
                f.start <= r.start
                    && r.end <= f.end
                    && (s.code || s.block == ComposerBlock::CodeBlock)
            })
        };
        for (r, _) in formats.iter().chain(&styles) {
            cuts.push(r.start.min(text.len()));
            cuts.push(r.end.min(text.len()));
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.windows(2)
            .map(|w| {
                let mut run = base.clone();
                run.len = w[1] - w[0];
                if mono(&(w[0]..w[1])) {
                    run.font.family = crate::oxide::MONO.into();
                }
                for (r, s) in formats.iter().chain(&styles) {
                    if r.start <= w[0] && w[1] <= r.end {
                        if let Some(c) = s.color {
                            run.color = c;
                        }
                        if let Some(weight) = s.font_weight {
                            run.font.weight = weight;
                        }
                        if let Some(st) = s.font_style {
                            run.font.style = st;
                        }
                        if s.background_color.is_some() {
                            run.background_color = s.background_color;
                        }
                        if s.underline.is_some() {
                            run.underline = s.underline;
                        }
                        if s.strikethrough.is_some() {
                            run.strikethrough = s.strikethrough;
                        }
                    }
                }
                run
            })
            .collect()
    }

    /// Each paragraph's size, inset and marker; numbered lists count on from the paragraph before.
    fn paragraphs(&self, text: &str, m: &Metrics, cx: &App) -> Vec<Paragraph> {
        let base_size = m.size;
        let base_height = m.line_height;
        let input = self.input.read(cx);
        let mut number = 0;
        let mut start = 0;
        let mut out = Vec::new();
        for paragraph in text.split('\n') {
            let end = start + paragraph.len();
            let plain = Paragraph {
                font_size: base_size,
                height: base_height,
                inset: px(0.),
                marker: None,
            };
            if !input.rich || text.is_empty() {
                out.push(plain);
                start = end + 1;
                continue;
            }
            // An empty last paragraph is styled by what typing there would take.
            let format = if start < text.len() {
                input.run_at(start).cloned().unwrap_or_default()
            } else {
                input
                    .typing
                    .clone()
                    .unwrap_or_else(|| input.inherited(start..start))
            };
            let largest = input
                .runs
                .iter()
                .filter(|(r, _)| r.start < end.max(start + 1) && r.end > start)
                .map(|(_, f)| f.size)
                .chain([format.size])
                .max()
                .unwrap_or(0);
            let font_size = if largest == 0 {
                base_size
            } else {
                px(largest as f32)
            };
            number = if format.block == ComposerBlock::Numbered {
                number + 1
            } else {
                0
            };
            let indent = format.indent as f32 * INDENT;
            let marker = match format.block {
                ComposerBlock::Bullet => Some("•".to_owned()),
                ComposerBlock::Numbered => Some(format!("{number}.")),
                ComposerBlock::Quote => Some("│".to_owned()),
                _ => None,
            };
            let extra = if marker.is_some() { INDENT } else { 0. };
            out.push(Paragraph {
                font_size,
                height: base_height * (font_size / base_size),
                inset: px(indent + extra),
                marker: marker.map(|m| (px(indent + 2.), m.into())),
            });
            start = end + 1;
        }
        out
    }

    fn shape(
        &self,
        width: Pixels,
        m: &Metrics,
        window: &mut Window,
        cx: &App,
    ) -> (Vec<Line>, Pixels) {
        let input = self.input.read(cx);
        let placeholder = input.text.is_empty();
        let text: SharedString = if placeholder {
            input.placeholder.clone()
        } else {
            input.text.clone().into()
        };
        let runs = self.runs(&text, placeholder, m, cx);
        let paragraphs = self.paragraphs(if placeholder { "" } else { &text }, m, cx);
        let multiline = input.multiline;
        let style = &m.style;
        let mut lines = Vec::new();
        let mut top = px(0.);
        let mut start = 0;
        for (ix, paragraph) in text.split('\n').enumerate() {
            let end = start + paragraph.len();
            let layout = paragraphs
                .get(ix)
                .unwrap_or_else(|| paragraphs.last().expect("one paragraph"));
            let shown: String = paragraph
                .chars()
                .map(|c| {
                    if c == IMAGE_CHARACTER {
                        IMAGE_STAND_IN
                    } else {
                        c
                    }
                })
                .collect();
            let wrap = multiline.then(|| (width - layout.inset).max(px(20.)));
            let wrapped = window
                .text_system()
                .shape_text(
                    shown.into(),
                    layout.font_size,
                    &slice_runs(&runs, start..end),
                    wrap,
                    None,
                )
                .ok()
                .and_then(|mut l| (!l.is_empty()).then(|| l.remove(0)))
                .unwrap_or_default();
            let marker = layout.marker.as_ref().map(|(x, label)| {
                let run = TextRun {
                    len: label.len(),
                    font: style.font(),
                    color: self.colors.placeholder,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                (
                    *x,
                    window
                        .text_system()
                        .shape_line(label.clone(), m.size, &[run], None),
                )
            });
            let height = wrapped.size(layout.height).height.max(layout.height);
            lines.push(Line {
                start,
                top,
                x: layout.inset,
                height: layout.height,
                wrapped,
                marker,
            });
            top += height;
            start = end + 1;
        }
        (lines, top)
    }
}

/// The runs covering `range` of the text, cut to it.
fn slice_runs(runs: &[TextRun], range: Range<usize>) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut at = 0;
    for run in runs {
        let (s, e) = (at, at + run.len);
        at = e;
        let (s, e) = (s.max(range.start), e.min(range.end));
        if s < e {
            let mut run = run.clone();
            run.len = e - s;
            out.push(run);
        }
    }
    out
}

/// A composer format as a text style: weight, slant, lines, colours; links in the accent colour, underlined.
fn format_style(f: &ComposerFormat, colors: InputColors) -> HighlightStyle {
    let line = || UnderlineStyle {
        thickness: px(1.),
        ..Default::default()
    };
    HighlightStyle {
        color: if f.link.is_some() {
            Some(colors.cursor)
        } else {
            f.color.as_deref().and_then(hex_color)
        },
        font_weight: f.bold.then_some(FontWeight::BOLD),
        font_style: f.italic.then_some(FontStyle::Italic),
        background_color: f.highlight.as_deref().and_then(hex_color),
        underline: (f.underline || f.link.is_some()).then(line),
        strikethrough: f.strike.then(|| StrikethroughStyle {
            thickness: px(1.),
            ..Default::default()
        }),
        ..Default::default()
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        let input = self.input.clone();
        let colors = self.colors;
        // The measure runs after this element's text style is gone: it keeps the one in force now.
        let metrics = Metrics::of(window);
        let layout = window.request_measured_layout(style, move |known, available, window, cx| {
            let element = TextElement {
                input: input.clone(),
                colors,
            };
            // Without a definite width (a field in a row), ask only for what the text needs, as text
            // does: a huge guess would push the row's other children out.
            let width = match (known.width, available.width) {
                (Some(w), _) | (None, gpui::AvailableSpace::Definite(w)) => w,
                (None, gpui::AvailableSpace::MinContent) => px(0.),
                (None, gpui::AvailableSpace::MaxContent) => {
                    let (lines, _) = element.shape(px(100_000.), &metrics, window, cx);
                    lines.iter().map(|l| l.x + l.wrapped.width()).fold(px(0.), |a, b| a.max(b)) + px(2.)
                }
            };
            let (_, height) = element.shape(width.max(px(1.)), &metrics, window, cx);
            size(width, height)
        });
        (layout, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let metrics = Metrics::of(window);
        let (lines, _) = self.shape(bounds.size.width, &metrics, window, cx);
        let line_height = metrics.line_height;
        let input = self.input.read(cx);
        let placeholder = input.text.is_empty();
        let locate = |offset: usize| -> (Point<Pixels>, Pixels) {
            let first = lines.first().map(|l| (point(l.x, px(0.)), l.height));
            if placeholder {
                return first.unwrap_or((point(px(0.), px(0.)), line_height));
            }
            lines
                .iter()
                .rev()
                .find(|l| l.start <= offset)
                .and_then(|l| {
                    l.wrapped
                        .position_for_index(offset - l.start, l.height)
                        .map(|p| (point(l.x + p.x, l.top + p.y), l.height))
                })
                .unwrap_or((point(px(0.), px(0.)), line_height))
        };
        let selected = input.selected.clone();
        let cursor_at = if input.reversed {
            selected.start
        } else {
            selected.end
        };
        let mut selection = Vec::new();
        let color = self.colors.selection;
        let cursor = if selected.is_empty() {
            let (p, height) = locate(cursor_at);
            Some(fill(
                Bounds::new(bounds.origin + p, size(px(1.5), height)),
                self.colors.cursor,
            ))
        } else {
            let (a, ha) = locate(selected.start);
            let (b, hb) = locate(selected.end);
            let o = bounds.origin;
            if a.y == b.y {
                selection.push(fill(
                    Bounds::from_corners(o + a, o + point(b.x, b.y + ha.max(hb))),
                    color,
                ));
            } else {
                selection.push(fill(
                    Bounds::from_corners(o + a, o + point(bounds.size.width, a.y + ha)),
                    color,
                ));
                if b.y > a.y + ha {
                    selection.push(fill(
                        Bounds::from_corners(
                            o + point(px(0.), a.y + ha),
                            o + point(bounds.size.width, b.y),
                        ),
                        color,
                    ));
                }
                selection.push(fill(
                    Bounds::from_corners(o + point(px(0.), b.y), o + point(b.x, b.y + hb)),
                    color,
                ));
            }
            None
        };
        Prepaint {
            lines,
            cursor,
            selection,
        }
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        for line in &prepaint.lines {
            let origin = bounds.origin + point(line.x, line.top);
            let _ = line.wrapped.paint_background(
                origin,
                line.height,
                TextAlign::Left,
                Some(bounds),
                window,
                cx,
            );
            let _ = line.wrapped.paint(
                origin,
                line.height,
                TextAlign::Left,
                Some(bounds),
                window,
                cx,
            );
            if let Some((x, marker)) = &line.marker {
                let _ = marker.paint(bounds.origin + point(*x, line.top), line.height, window, cx);
            }
        }
        if focus.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        let lines = std::mem::take(&mut prepaint.lines);
        let line_height = window.line_height();
        self.input.update(cx, |input, _| {
            let placeholder = input.text.is_empty();
            input.lines = if placeholder { Vec::new() } else { lines };
            input.bounds = Some(bounds);
            input.line_height = line_height;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, TestAppContext, VisualTestContext};

    struct Host {
        input: Entity<TextInput>,
        events: Vec<InputEvent>,
        links: usize,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let colors = InputColors {
                text: gpui::black(),
                placeholder: gpui::black(),
                cursor: gpui::black(),
                selection: gpui::black(),
            };
            div().w(px(300.)).child(field(&self.input, colors, cx))
        }
    }

    fn host_with(
        cx: &mut TestAppContext,
        multiline: bool,
        rich: bool,
    ) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(bind_keys);
        let (host, cx) = cx.add_window_view(|window, cx| {
            let input = cx.new(|cx| {
                let input = TextInput::new(cx, "placeholder");
                let input = if multiline {
                    input.multiline(true)
                } else {
                    input
                };
                if rich { input.rich() } else { input }
            });
            cx.subscribe(&input, |host: &mut Host, _, e: &InputEvent, _| {
                host.events.push(e.clone())
            })
            .detach();
            cx.subscribe(&input, |host: &mut Host, _, e: &RichEvent, _| {
                if matches!(e, RichEvent::Link) {
                    host.links += 1;
                }
            })
            .detach();
            input.read(cx).focus(window);
            Host {
                input,
                events: Vec::new(),
                links: 0,
            }
        });
        cx.run_until_parked();
        (host, cx)
    }
    fn host(cx: &mut TestAppContext, multiline: bool) -> (Entity<Host>, &mut VisualTestContext) {
        host_with(cx, multiline, false)
    }
    fn text(host: &Entity<Host>, cx: &mut VisualTestContext) -> String {
        host.read_with(cx, |h, cx| h.input.read(cx).text().to_owned())
    }
    fn formats(host: &Entity<Host>, cx: &mut VisualTestContext) -> Vec<ComposerFormat> {
        host.read_with(cx, |h, cx| h.input.read(cx).formats())
    }

    #[gpui::test]
    fn typing_editing_and_undo(cx: &mut TestAppContext) {
        let (host, cx) = host(cx, false);
        cx.simulate_input("hello world");
        assert_eq!(text(&host, cx), "hello world");
        cx.simulate_keystrokes("left left backspace");
        assert_eq!(text(&host, cx), "hello wold");
        cx.simulate_keystrokes("alt-backspace");
        assert_eq!(text(&host, cx), "hello ld");
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(text(&host, cx), "hello wold");
        cx.simulate_keystrokes("cmd-shift-z");
        assert_eq!(text(&host, cx), "hello ld");
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("é😀");
        assert_eq!(text(&host, cx), "é😀");
        cx.simulate_keystrokes("backspace");
        assert_eq!(text(&host, cx), "é");
        cx.simulate_keystrokes("enter escape");
        let events = host.read_with(cx, |h, _| h.events.clone());
        assert!(events.contains(&InputEvent::Changed));
        assert!(events.ends_with(&[InputEvent::Submit, InputEvent::Cancel]));
    }

    #[gpui::test]
    fn multiline_breaks_lines_and_moves_between_them(cx: &mut TestAppContext) {
        let (host, cx) = host(cx, true);
        cx.simulate_input("one");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("two");
        assert_eq!(text(&host, cx), "one\ntwo");
        cx.simulate_keystrokes("cmd-left");
        cx.simulate_input("x");
        assert_eq!(text(&host, cx), "one\nxtwo");
        // Up keeps the column: after "x" is column 1, so the cursor lands after "o".
        cx.simulate_keystrokes("up");
        cx.simulate_input("y");
        assert_eq!(text(&host, cx), "oyne\nxtwo");
        cx.simulate_keystrokes("up");
        let events = host.read_with(cx, |h, _| h.events.clone());
        assert_eq!(events.last(), Some(&InputEvent::Up));
        // Return submits; the text keeps no newline from it.
        cx.simulate_keystrokes("enter");
        assert_eq!(text(&host, cx), "oyne\nxtwo");
    }

    #[gpui::test]
    fn highlights_follow_edits(cx: &mut TestAppContext) {
        let (host, cx) = host(cx, false);
        cx.simulate_input("ab cd");
        host.update(cx, |h, cx| {
            h.input.update(cx, |i, _| {
                i.highlights = vec![(3..5, HighlightStyle::default())]
            });
        });
        cx.simulate_keystrokes("cmd-left");
        cx.simulate_input("xx");
        let ranges = host.read_with(cx, |h, cx| {
            h.input
                .read(cx)
                .highlights
                .iter()
                .map(|(r, _)| r.clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(ranges, vec![5..7]);
    }

    #[gpui::test]
    fn bold_shortcut_formats_the_selection_and_the_text_typed_next(cx: &mut TestAppContext) {
        let (host, cx) = host_with(cx, true, true);
        cx.simulate_input("ab cd");
        cx.simulate_keystrokes("shift-left shift-left cmd-b");
        let f = formats(&host, cx);
        assert_eq!(
            (f.len(), f[0].start, f[0].length, f[0].bold),
            (1, 3, 2, true)
        );
        // Typing at the end of the bold run goes on in bold; ⌘B again stops it for what follows.
        cx.simulate_keystrokes("right");
        cx.simulate_input("e");
        cx.simulate_keystrokes("cmd-b");
        cx.simulate_input("f");
        let f = formats(&host, cx);
        assert_eq!(text(&host, cx), "ab cdef");
        assert_eq!((f.len(), f[0].start, f[0].length), (1, 3, 3));
        // Undo takes back the typing, formats included.
        cx.simulate_keystrokes("cmd-z cmd-z");
        assert_eq!(text(&host, cx), "ab cd");
        assert_eq!(formats(&host, cx)[0].length, 2);
        cx.simulate_keystrokes("cmd-k");
        assert_eq!(host.read_with(cx, |h, _| h.links), 1);
    }

    #[gpui::test]
    fn rich_text_from_the_core_counts_utf16_and_paragraph_formats_carry_on(
        cx: &mut TestAppContext,
    ) {
        let (host, cx) = host_with(cx, true, true);
        let quote = ComposerFormat {
            start: 3,
            length: 2,
            block: ComposerBlock::Bullet,
            ..Default::default()
        };
        host.update(cx, |h, cx| {
            h.input
                .update(cx, |i, cx| i.set_rich_text("😀\nab", &[quote], 5, cx))
        });
        // A new line in a list stays in the list.
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("c");
        let f = formats(&host, cx);
        assert_eq!(text(&host, cx), "😀\nab\nc");
        assert_eq!(
            (f.len(), f[0].start, f[0].length, f[0].block),
            (1, 3, 4, ComposerBlock::Bullet)
        );
        // A paragraph change covers the whole line, its break too.
        host.update(cx, |h, cx| {
            h.input.update(cx, |i, cx| {
                i.change_format(true, |f| f.block = ComposerBlock::Paragraph, cx)
            })
        });
        let f = formats(&host, cx);
        assert_eq!((f.len(), f[0].start, f[0].length), (1, 3, 3));
    }

    #[gpui::test]
    fn completing_sends_list_keys_to_the_owner(cx: &mut TestAppContext) {
        let (host, cx) = host_with(cx, true, true);
        cx.simulate_input("one\n");
        host.update(cx, |h, cx| {
            h.input.update(cx, |i, _| i.set_completing(true))
        });
        cx.simulate_keystrokes("up down enter tab escape");
        let events = host.read_with(cx, |h, _| h.events.clone());
        assert!(events.ends_with(&[
            InputEvent::Up,
            InputEvent::Down,
            InputEvent::Submit,
            InputEvent::Tab,
            InputEvent::Cancel
        ]));
        assert_eq!(text(&host, cx), "one\n");
    }
}
