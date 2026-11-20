//! Mentions typed in the draft (`DraftInput.swift`, `RichDraft.swift`): the `@` being typed, the people it can
//! name, and the list floating above it, moved in with the editor's keys.
use crate::composer::{self, Ui2};
use crate::oxide::{Oxide, tr, trf};
use crate::widgets;
use gpui::{
    AnyElement, Context, Corner, FontWeight, anchored, deferred, div, point, prelude::*, px,
};
use std::ops::Range;
use teams_core::{Action, PersonView};

/// A mention being typed: what follows the `@` and the byte range from the `@` to the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionQuery {
    pub query: String,
    pub range: Range<usize>,
}

impl MentionQuery {
    /// An `@` at a word start before the caret, with no line break or other `@` up to it.
    pub fn detect(value: &str, selection: Range<usize>) -> Option<Self> {
        if !selection.is_empty()
            || selection.start > value.len()
            || !value.is_char_boundary(selection.start)
        {
            return None;
        }
        let prefix = &value[..selection.start];
        let at = prefix.rfind('@')?;
        let before = prefix[..at].chars().last();
        if !before.is_none_or(|c| c.is_whitespace() || "([{>".contains(c)) {
            return None;
        }
        let query = &prefix[at + 1..];
        if query.chars().count() > 64 || query.contains(['\n', '\r', '@']) {
            return None;
        }
        Some(Self {
            query: query.to_owned(),
            range: at..selection.start,
        })
    }
}

/// What the list knows between keystrokes, as `DraftInput`.
#[derive(Default)]
pub struct Mentions {
    pub current: Option<MentionQuery>,
    pub highlighted: usize,
    suppressed: Option<MentionQuery>,
    value: String,
    selection: Range<usize>,
    skip_core_mention: bool,
}

impl Mentions {
    pub fn dismiss(&mut self, after_insertion: bool) {
        self.suppressed = self.current.take();
        self.skip_core_mention = after_insertion;
    }
    /// The text or caret changed: finds the mention typed there, unless it was just dismissed or is already one.
    /// Returns whether the mention changed.
    pub fn capture(
        &mut self,
        value: &str,
        selection: Range<usize>,
        mentions: &[Range<usize>],
        core_change: bool,
    ) -> bool {
        let before = self.current.clone();
        self.selection = selection.clone();
        let found = MentionQuery::detect(value, selection).filter(|q| {
            !mentions
                .iter()
                .any(|m| m.start < q.range.end.max(q.range.start + 1) && q.range.start < m.end)
        });
        if core_change && self.skip_core_mention {
            self.value = value.to_owned();
            self.suppressed = found;
            self.current = None;
            self.skip_core_mention = false;
            return before != self.current;
        }
        if self.value != value {
            self.suppressed = None;
            if !core_change {
                self.skip_core_mention = false;
            }
        }
        self.value = value.to_owned();
        self.current = if found == self.suppressed {
            None
        } else {
            found
        };
        if before != self.current {
            self.highlighted = 0;
            return true;
        }
        false
    }
    /// Whether `capture` has seen this text and caret already.
    pub fn seen(&self, value: &str, selection: &Range<usize>) -> bool {
        self.value == value && &self.selection == selection
    }
}

/// Case- and accent-insensitive, as `localizedStandardContains`.
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// The people the mention being typed can still name, eight at most.
pub fn people(this: &Oxide) -> Vec<PersonView> {
    let Some(mention) = &this.ui.composer.mentions.current else {
        return Vec::new();
    };
    let query = fold(&mention.query);
    this.mention_people
        .iter()
        .filter(|p| query.is_empty() || fold(&format!("{} {}", p.name, p.email)).contains(&query))
        .take(8)
        .cloned()
        .collect()
}

pub fn choose(this: &mut Oxide, person: &PersonView, cx: &mut Context<Oxide>) {
    let Some(mention) = this.ui.composer.mentions.current.clone() else {
        return;
    };
    this.ui.composer.mentions.dismiss(true);
    composer::replace(
        this,
        &format!("@{} ", person.name),
        Some(mention.range),
        Some(person.mri.clone()),
        None,
        cx,
    );
}

/// The editor's keys while a mention is typed: they move in and pick from the list. False when none is typed.
pub fn key(this: &mut Oxide, event: &crate::input::InputEvent, cx: &mut Context<Oxide>) -> bool {
    use crate::input::InputEvent;
    if this.ui.composer.mentions.current.is_none() {
        return false;
    }
    let people = people(this);
    let m = &mut this.ui.composer.mentions;
    match event {
        InputEvent::Cancel => m.dismiss(false),
        InputEvent::Down => m.highlighted = (m.highlighted + 1).min(people.len().saturating_sub(1)),
        InputEvent::Up => m.highlighted = m.highlighted.saturating_sub(1),
        InputEvent::Submit | InputEvent::Tab => {
            if let Some(person) = people
                .get(m.highlighted.min(people.len().saturating_sub(1)))
                .cloned()
            {
                choose(this, &person, cx);
            }
        }
        InputEvent::Changed => return false,
    }
    composer::sync_completing(this, cx);
    cx.notify();
    true
}

/// The list just above the mention being typed; it never takes the focus, the editor keeps the keys.
pub fn picker(this: &mut Oxide, ui: &Ui2, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let mention = ui.mention.clone()?;
    let anchor = ui.input.read(cx).bounds_for(mention.range.start)?;
    let theme = this.theme;
    let people = people(this);
    let highlighted = ui.highlighted.min(people.len().saturating_sub(1));
    let composer = this.composer.clone().unwrap_or_else(composer::empty_view);
    let mut list = div()
        .w(px(300.))
        .p(px(4.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .rounded(px(12.))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.surface)
        .shadow_lg()
        .occlude()
        .child(
            div()
                .p(px(6.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted)
                .child(tr("ui.composer.mention_someone")),
        );
    for (ix, person) in people.iter().enumerate() {
        let avatar = this.avatar(
            &person.mri,
            &person.name,
            28.,
            teams_core::AvatarShape::Round,
            None,
        );
        let chosen = person.clone();
        list = list.child(
            div()
                .id(("mention", ix))
                .p(px(6.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(ix == highlighted, |d| d.bg(theme.accent_soft))
                .when(ix != highlighted, |d| d.hover(|s| s.bg(theme.input)))
                .child(avatar)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(person.name.clone()),
                        )
                        .when(!person.email.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted)
                                    .truncate()
                                    .child(person.email.clone()),
                            )
                        }),
                )
                .tooltip(widgets::tooltip(
                    trf("ui.composer.mention", &[("name", &person.name)]),
                    theme,
                ))
                .on_click(cx.listener(move |this, _, window, cx| {
                    choose(this, &chosen, cx);
                    this.ui.composer.input.read(cx).focus(window);
                })),
        );
    }
    let note = |text: String| div().p(px(6.)).text_size(px(12.)).child(text);
    if composer.mention_loading {
        list = list.child(
            div()
                .p(px(6.))
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .child(widgets::spinner("mention-loading", 12., theme.muted))
                .child(tr("ui.composer.loading_participants")),
        );
    } else if let Some(error) = composer.mention_error.clone() {
        list = list.child(
            div()
                .id("mention-error")
                .p(px(6.))
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .tooltip(widgets::tooltip(error, theme))
                .child(tr("ui.composer.couldn_t_load_all_participants"))
                .child(
                    widgets::button(
                        "mention-retry",
                        tr("ui.composer.try_again"),
                        widgets::ButtonKind::Ghost,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, _| this.send(Action::ChannelMembers))),
                ),
        );
    } else if people.is_empty() {
        list = list.child(note(tr("ui.composer.no_matching_participants")).text_color(theme.muted));
    }
    let at = point(anchor.origin.x - px(4.), anchor.origin.y - px(4.));
    Some(
        deferred(
            anchored()
                .position(at)
                .anchor(Corner::BottomLeft)
                .snap_to_window_with_margin(px(8.))
                .child(list),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_an_at_sign_starting_a_word() {
        assert_eq!(
            MentionQuery::detect("hi @al", 6..6).map(|m| (m.query, m.range)),
            Some(("al".to_owned(), 3..6))
        );
        assert_eq!(
            MentionQuery::detect("(@", 2..2).map(|m| m.range),
            Some(1..2)
        );
        assert!(MentionQuery::detect("mail@al", 7..7).is_none());
        assert!(MentionQuery::detect("@a\nb", 4..4).is_none());
        assert!(MentionQuery::detect("@al", 1..3).is_none());
    }

    #[test]
    fn a_dismissed_mention_stays_hidden_until_the_text_changes() {
        let mut m = Mentions::default();
        assert!(m.capture("@a", 2..2, &[], false));
        m.dismiss(false);
        assert!(!m.capture("@a", 2..2, &[], false));
        assert!(m.current.is_none());
        assert!(m.capture("@al", 3..3, &[], false));
        // A mention already made is not offered again; nor right after one was inserted.
        assert!(m.capture("@Alice ", 6..6, &[0..6], false));
        assert!(m.current.is_none());
        m.capture("@al", 3..3, &[], false);
        m.dismiss(true);
        m.capture("@Alice x", 8..8, &[], true);
        assert!(m.current.is_none());
        assert_eq!(fold("Élodie Ça"), "elodie ca");
    }
}
