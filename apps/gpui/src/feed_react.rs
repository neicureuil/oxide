//! The reaction picker under a message's smiley (`EmojiPicker.swift` as `MessageRow` uses it): Teams' catalog
//! by category, a search, and the reactions used lately.
use crate::oxide::{Oxide, tr, trf};
use crate::widgets;
use gpui::{
    AnyElement, Context, Corner, FontWeight, SharedString, Window, div, prelude::*, px,
    uniform_list,
};
use teams_core::{Action, ComposerEmoji, MessageView};

const COLUMNS: usize = 8;

/// Case- and accent-insensitive search text, as `EmojiPicker.searchKey`.
pub fn search_key(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'û' | 'ü' | 'ú' | 'ù' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

/// What the grid shows: the search's results, else the chosen category.
fn results(this: &Oxide, query: &str) -> Vec<ComposerEmoji> {
    let query = query.trim();
    if query.is_empty() {
        let category = this.ui.conversation.react_category;
        return this
            .emoji
            .get(category)
            .map(|c| c.items.clone())
            .unwrap_or_default();
    }
    let folded = search_key(query);
    this.emoji
        .iter()
        .flat_map(|c| c.items.iter())
        .filter(|e| {
            search_key(&format!("{} {} {} {}", e.name, e.keywords, e.id, e.text)).contains(&folded)
        })
        .cloned()
        .collect()
}

/// The picker, anchored under the smiley of `msg`.
pub fn popover(
    this: &mut Oxide,
    msg: &MessageView,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let query = this.ui.conversation.react_search.read(cx).text().to_owned();
    let items = results(this, &query);
    let count = items.len();
    let recent: Vec<ComposerEmoji> = if query.trim().is_empty() {
        let ids = &this.ui.conversation.recent_reactions;
        ids.iter()
            .filter_map(|id| {
                this.emoji
                    .iter()
                    .flat_map(|c| c.items.iter())
                    .find(|e| e.id == *id)
                    .cloned()
            })
            .collect()
    } else {
        Vec::new()
    };
    let heading = if !query.trim().is_empty() {
        trf(
            "ui.emoji_picker.results",
            &[("resultsCount", &count.to_string())],
        )
    } else {
        this.emoji
            .get(this.ui.conversation.react_category)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| tr("ui.emoji_picker.emoji"))
    };
    let mut tabs = div().flex().gap(px(1.));
    for (ix, category) in this.emoji.iter().enumerate() {
        let selected = ix == this.ui.conversation.react_category;
        let glyph = category
            .items
            .first()
            .map(|e| e.text.clone())
            .unwrap_or_else(|| "•".into());
        let hover = theme.input;
        tabs = tabs.child(
            div()
                .id(SharedString::from(format!("react-tab-{ix}")))
                .w(px(34.))
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(19.))
                .when(selected, |d| d.bg(theme.accent.opacity(0.15)))
                .when(!selected, move |d| d.hover(move |s| s.bg(hover)))
                .child(glyph)
                .tooltip(widgets::tooltip(category.name.clone(), theme))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ui.conversation.react_category = ix;
                    this.ui
                        .conversation
                        .react_search
                        .update(cx, |i, cx| i.set_text("", None, cx));
                    cx.notify();
                })),
        );
    }
    let message = msg.clone();
    let entity = cx.entity();
    // One emoji button; a click reacts and closes the picker.
    let cell = move |item: &ComposerEmoji| {
        let (item, message, entity) = (item.clone(), message.clone(), entity.clone());
        let hover = theme.input;
        div()
            .id(SharedString::from(format!("react-{}", item.id)))
            .size(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(24.))
            .hover(move |s| s.bg(hover))
            .child(item.text.clone())
            .tooltip(widgets::tooltip(item.name.clone(), theme))
            .on_click(move |_, _, cx| {
                entity.update(cx, |this, cx| choose(this, &message, &item, cx))
            })
    };
    let label = |text: String| {
        div()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted)
            .child(text)
    };
    let mut body = div().h(px(260.)).flex().flex_col().gap(px(10.));
    if !recent.is_empty() {
        body = body.child(label(tr("ui.emoji_picker.recent"))).child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(3.))
                .children(recent.iter().take(2 * COLUMNS).map(&cell)),
        );
    }
    body = body.child(label(heading));
    if this.emoji.is_empty() {
        body = body.child(widgets::muted(
            tr("ui.emoji_picker.downloading"),
            13.,
            &theme,
        ));
    } else if count == 0 {
        body = body.child(widgets::muted(
            tr("ui.emoji_picker.no_emoji_found"),
            13.,
            &theme,
        ));
    }
    // Rows of eight, only those on screen drawn: a category holds hundreds.
    let rows = items.len().div_ceil(COLUMNS);
    body = body.child(
        uniform_list("react-grid", rows, move |range, _, _| {
            range
                .map(|row| {
                    div()
                        .flex()
                        .gap(px(3.))
                        .pb(px(3.))
                        .children(items.iter().skip(row * COLUMNS).take(COLUMNS).map(&cell))
                        .into_any_element()
                })
                .collect()
        })
        .flex_1(),
    );
    let content = div()
        .w(px(340.))
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .text_color(theme.text)
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.emoji_picker.emoji")),
        )
        .child(widgets::text_field(
            &this.ui.conversation.react_search,
            &theme,
            cx,
        ))
        .child(tabs)
        .child(body);
    let close = cx.listener(|this, _: &(), _, cx| this.close_popup(cx));
    widgets::popover(Corner::TopLeft, content, &theme, move |w, cx| {
        close(&(), w, cx)
    })
}

/// Reacts with `item`; picking a reaction of mine again removes it, through its existing wire key.
fn choose(this: &mut Oxide, msg: &MessageView, item: &ComposerEmoji, cx: &mut Context<Oxide>) {
    let key = msg
        .reactions
        .iter()
        .find(|r| r.mine && r.emoji_id.as_deref() == Some(item.id.as_str()))
        .map(|r| r.key.clone())
        .unwrap_or_else(|| item.id.clone());
    let recent = &mut this.ui.conversation.recent_reactions;
    recent.retain(|id| *id != item.id);
    recent.insert(0, item.id.clone());
    recent.truncate(24);
    this.popup = None;
    this.ui
        .conversation
        .react_search
        .update(cx, |i, cx| i.set_text("", None, cx));
    this.send(Action::React {
        id: msg.id.clone(),
        key,
    });
    cx.notify();
}

#[cfg(test)]
mod tests {
    #[test]
    fn search_ignores_case_and_accents() {
        assert_eq!(super::search_key("Cœur ÉTÉ"), "cœur ete");
    }
}
