//! Pieces shared by the chat list, activity, search, new chat and chat dialogs (owner: chats agent).
use crate::input::TextInput;
use crate::theme::Theme;
use gpui::{
    AnyElement, App, Entity, ListAlignment, ListState, Pixels, Point, Window, anchored, deferred,
    div, prelude::*, px,
};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

/// A virtualised list of rows that changes in place: only the rows whose content changed are measured
/// again, so the scroll position survives updates (a reset would jump to the top).
pub struct SyncedList {
    pub state: ListState,
    sigs: Vec<u64>,
}

impl SyncedList {
    pub fn new() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(600.)),
            sigs: Vec::new(),
        }
    }

    /// `sigs` holds one hash per row, of what it draws.
    pub fn sync(&mut self, sigs: Vec<u64>) {
        let old = std::mem::take(&mut self.sigs);
        if self.state.item_count() != old.len() {
            self.state.reset(sigs.len());
        } else {
            for (ix, (a, b)) in old.iter().zip(&sigs).enumerate() {
                if a != b {
                    self.state.splice(ix..ix + 1, 1);
                }
            }
            if sigs.len() > old.len() {
                self.state
                    .splice(old.len()..old.len(), sigs.len() - old.len());
            } else if sigs.len() < old.len() {
                self.state.splice(sigs.len()..old.len(), 0);
            }
        }
        self.sigs = sigs;
    }

    /// Forgets every row, back at the top: for a list that shows something else.
    pub fn clear(&mut self) {
        self.sigs.clear();
        self.state.reset(0);
    }
}

pub fn sig(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// A field that mirrors a core value, as Swift's `SyncedTextField`: the core's echo of typing is
/// ignored, only its own changes (cleared, reset) replace the text.
#[derive(Default)]
pub struct Echo {
    sent: Vec<String>,
    seen: Option<String>,
}

impl Echo {
    /// The text just typed, before it is sent to the core.
    pub fn typed(&mut self, text: &str) {
        self.sent.push(text.to_owned());
        if self.sent.len() > 64 {
            self.sent.remove(0);
        }
    }

    /// The core's value, at each render.
    pub fn incoming(&mut self, value: &str, input: &Entity<TextInput>, cx: &mut App) {
        if self.seen.as_deref() == Some(value) {
            return;
        }
        self.seen = Some(value.to_owned());
        if let Some(ix) = self.sent.iter().position(|s| s == value) {
            self.sent.drain(..=ix);
        } else if input.read(cx).text() != value {
            self.sent.clear();
            let value = value.to_owned();
            input.update(cx, |i, cx| i.set_text(value, None, cx));
        }
    }

    /// Starts over, for a dialog opened again.
    pub fn reset(&mut self) {
        self.sent.clear();
        self.seen = None;
    }
}

/// The app's text field taking the rest of a row: laid out on its own, a field measures its text
/// unwrapped and pushes past the row's other children, so it is pinned to its slot instead.
pub fn row_field(input: &Entity<TextInput>, theme: &Theme, cx: &App) -> gpui::Div {
    div().relative().flex_1().min_w_0().h(px(34.)).child(
        crate::widgets::text_field(input, theme, cx)
            .absolute()
            .inset_0()
            .overflow_hidden(),
    )
}

/// A menu at the pointer, as a right click opens; `close` runs on a click outside it or Escape.
pub fn context_menu(
    position: Point<Pixels>,
    content: impl IntoElement,
    theme: &Theme,
    close: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let close = std::rc::Rc::new(close);
    let escape = close.clone();
    deferred(
        anchored()
            .position(position)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("context-menu")
                    .occlude()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .shadow_lg()
                    .py_1()
                    .on_mouse_down_out(move |_, w, cx| close(w, cx))
                    .on_key_down(move |e, w, cx| {
                        if e.keystroke.key == "escape" {
                            escape(w, cx)
                        }
                    })
                    .child(content),
            ),
    )
    .with_priority(1)
    .into_any_element()
}

/// A hairline between groups of menu items.
pub fn menu_separator(theme: &Theme) -> impl IntoElement {
    div().my_1().mx_2().h(px(1.)).bg(theme.border)
}

/// Lowercase without accents, one char for one char so offsets map back (Swift's `.caseInsensitive, .diacriticInsensitive`).
fn fold(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match lower {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => 'i',
        'ł' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ř' => 'r',
        'ś' | 'š' | 'ş' => 's',
        'ť' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// Byte ranges of `text` matching any term, ignoring case and accents; sorted and merged.
pub fn matches(text: &str, terms: &[String]) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().map(|(i, c)| (i, fold(c))).collect();
    let mut found: Vec<Range<usize>> = Vec::new();
    for term in terms {
        let term: Vec<char> = term.chars().map(fold).collect();
        if term.is_empty() || term.len() > chars.len() {
            continue;
        }
        let mut ix = 0;
        while ix + term.len() <= chars.len() {
            if chars[ix..ix + term.len()]
                .iter()
                .map(|(_, c)| *c)
                .eq(term.iter().copied())
            {
                let start = chars[ix].0;
                let end = chars.get(ix + term.len()).map_or(text.len(), |(i, _)| *i);
                found.push(start..end);
                ix += term.len();
            } else {
                ix += 1;
            }
        }
    }
    found.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in found {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// The words to highlight in search results, worked out once for all hits: trimmed of punctuation, two letters or more.
pub fn terms<'a>(words: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in words {
        let word = word.trim_matches(|c: char| c.is_ascii_punctuation() || "«»“”‘’…".contains(c));
        if word.chars().count() > 1 && !out.iter().any(|w| w == word) {
            out.push(word.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ignore_case_and_accents() {
        let text = "Réunion décalée, voir la REUNION";
        let ranges = matches(text, &terms(["reunion", "decale"]));
        let found: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(found, ["Réunion", "décalé", "REUNION"]);
    }

    #[test]
    fn overlapping_matches_merge() {
        let text = "abcdef";
        assert_eq!(matches(text, &["abcd".into(), "cdef".into()]), [0..6]);
        assert!(matches(text, &[]).is_empty());
    }

    #[test]
    fn terms_drop_punctuation_and_short_words() {
        assert_eq!(
            terms(["«Rust»,", "a", "rust", "client."]),
            ["Rust", "rust", "client"]
        );
    }

    #[test]
    fn synced_list_keeps_count() {
        let mut list = SyncedList::new();
        list.sync(vec![1, 2, 3]);
        assert_eq!(list.state.item_count(), 3);
        list.sync(vec![1, 5, 3, 4]);
        assert_eq!(list.state.item_count(), 4);
        list.sync(vec![1]);
        assert_eq!(list.state.item_count(), 1);
        list.clear();
        assert_eq!(list.state.item_count(), 0);
    }

    #[gpui::test]
    fn echo_ignores_typing_echo_but_takes_core_changes(cx: &mut gpui::TestAppContext) {
        let input = cx.new(|cx| TextInput::new(cx, ""));
        let mut echo = Echo::default();
        cx.update(|cx| {
            input.update(cx, |i, cx| i.set_text("ab", None, cx));
            echo.typed("a");
            echo.typed("ab");
            // The core echoes the first keystroke late: the field keeps what was typed since.
            echo.incoming("a", &input, cx);
            assert_eq!(input.read(cx).text(), "ab");
            echo.incoming("ab", &input, cx);
            // Cleared by the core: the field follows.
            echo.incoming("", &input, cx);
            assert_eq!(input.read(cx).text(), "");
        });
    }
}
