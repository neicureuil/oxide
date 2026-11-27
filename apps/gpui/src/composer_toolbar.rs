//! The formatting toolbar over the draft (`FormattingToolbar.swift`, `DraftFormatting.swift`): inline styles,
//! colours, size, paragraph styles, lists, indent, quote, link, code, alignment, rule, undo.
use crate::composer::{self, with_popover};
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput, hex_color};
use crate::oxide::{Oxide, tr};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, Context, Entity, FontWeight, SharedString, Stateful, Subscription, Window, div,
    prelude::*, px,
};
use teams_core::{ComposerBlock, ComposerFormat};

const COLOR: &str = "composer:color";
const HIGHLIGHT: &str = "composer:highlight";
const SIZE: &str = "composer:size";
const STYLE: &str = "composer:style";
const MORE: &str = "composer:more";
pub const LINK: &str = "composer:link";

/// The link editor's fields.
pub struct Toolbar {
    title: Entity<TextInput>,
    url: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl Toolbar {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let title = cx.new(|cx| TextInput::new(cx, tr("ui.formatting_toolbar.text_to_display")));
        let url = cx.new(|cx| TextInput::new(cx, "https://…"));
        let submit = |this: &mut Oxide,
                      _: &Entity<TextInput>,
                      event: &InputEvent,
                      window: &mut Window,
                      cx: &mut Context<Oxide>| match event {
            InputEvent::Submit => insert_link(this, window, cx),
            InputEvent::Cancel => close_link(this, window, cx),
            _ => cx.notify(),
        };
        let subscriptions = vec![
            cx.subscribe_in(&title, window, submit),
            cx.subscribe_in(&url, window, submit),
        ];
        Self {
            title,
            url,
            _subscriptions: subscriptions,
        }
    }
}

fn input(this: &Oxide) -> Entity<TextInput> {
    this.ui.composer.input.clone()
}

/// Changes the selection's format (or its paragraphs'), then gives the editor the focus back.
fn change(
    this: &mut Oxide,
    paragraphs: bool,
    transform: impl Fn(&mut ComposerFormat),
    window: &mut Window,
    cx: &mut Context<Oxide>,
) {
    let input = input(this);
    input.update(cx, |i, cx| {
        i.focus(window);
        i.change_format(paragraphs, transform, cx);
    });
    this.close_popup(cx);
}

fn toggle_flag(
    this: &mut Oxide,
    flag: fn(&mut ComposerFormat) -> &mut bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) {
    let on = !*flag(&mut input(this).read(cx).current_format());
    change(this, false, move |f| *flag(f) = on, window, cx);
}

/// A paragraph style, or back to a plain paragraph when it already has it; headings set their size and weight.
pub fn set_block(
    this: &mut Oxide,
    block: ComposerBlock,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) {
    let current = input(this).read(cx).current_format().block;
    let chosen = if current == block {
        ComposerBlock::Paragraph
    } else {
        block
    };
    change(
        this,
        true,
        move |style| {
            style.block = chosen;
            (style.size, style.bold) = match chosen {
                ComposerBlock::Heading1 => (26, true),
                ComposerBlock::Heading2 => (22, true),
                ComposerBlock::Heading3 => (18, true),
                _ => (0, false),
            };
        },
        window,
        cx,
    );
}

fn indent(this: &mut Oxide, delta: i32, window: &mut Window, cx: &mut Context<Oxide>) {
    change(
        this,
        true,
        move |f| f.indent = (f.indent as i32 + delta).clamp(0, 8) as u32,
        window,
        cx,
    );
}

fn clear_formatting(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    change(
        this,
        false,
        |style| {
            *style = ComposerFormat {
                alignment: "left".into(),
                ..Default::default()
            }
        },
        window,
        cx,
    );
}

/// ⌘K or the link button: the toolbar opens with the link editor, filled from the selection.
pub fn request_link(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    this.ui.composer.formatting = true;
    let input = input(this);
    let state = input.read(cx);
    let selected = state.text()[state.selection()].to_owned();
    let link = state.current_format().link.unwrap_or_default();
    let (title, url) = (
        this.ui.composer.toolbar.title.clone(),
        this.ui.composer.toolbar.url.clone(),
    );
    title.update(cx, |t, cx| t.set_text(selected, None, cx));
    url.update(cx, |u, cx| {
        u.set_text(link, None, cx);
        u.focus(window);
    });
    this.popup = Some(LINK.into());
    cx.notify();
}

fn close_link(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    this.close_popup(cx);
    input(this).read(cx).focus(window);
}

/// An http(s) address with a host, or a mailto with an address.
fn valid_link(url: &str) -> bool {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    match scheme.to_lowercase().as_str() {
        "http" | "https" => rest.strip_prefix("//").is_some_and(|r| {
            r.split(['/', '?', '#'])
                .next()
                .is_some_and(|h| !h.is_empty())
        }),
        "mailto" => !rest.is_empty(),
        _ => false,
    }
}

/// Puts the link on the selection, after writing its text in place of it when a different one was given.
fn insert_link(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    let title = this.ui.composer.toolbar.title.read(cx).text().to_owned();
    let url = this
        .ui
        .composer
        .toolbar
        .url
        .read(cx)
        .text()
        .trim()
        .to_owned();
    if !valid_link(&url) {
        return;
    }
    this.close_popup(cx);
    let input = input(this);
    input.update(cx, |i, cx| {
        i.focus(window);
        let chosen = if title.is_empty() { url.clone() } else { title };
        let selection = i.selection();
        if chosen != i.text()[selection.clone()] {
            i.insert(&chosen, window, cx);
            i.select(selection.start..selection.start + chosen.len(), cx);
        }
        let link = url.clone();
        i.change_format(false, move |f| f.link = Some(link.clone()), cx);
    });
}

/// A horizontal line on a paragraph of its own, the caret after it.
fn insert_rule(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    this.close_popup(cx);
    let input = input(this);
    input.update(cx, |i, cx| {
        i.focus(window);
        let start = i.selection().start;
        let prefix = if start > 0 && !i.text()[..start].ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let rule = "────────";
        i.insert(&format!("{prefix}{rule}\n"), window, cx);
        let at = start + prefix.len();
        i.select(at..at + rule.len(), cx);
        i.change_format(
            true,
            |f| {
                f.block = ComposerBlock::Rule;
                f.size = 0;
                f.bold = false;
            },
            cx,
        );
        let after = at + rule.len() + 1;
        i.select(after..after, cx);
        i.change_format(false, |f| *f = ComposerFormat::default(), cx);
    });
}

/// A toolbar button, 27 × 28, tinted when its style is on.
fn tool(
    id: &'static str,
    glyph: Icon,
    label: String,
    active: bool,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    let hover = theme.input;
    div()
        .id(id)
        .w(px(27.))
        .h(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .when(active, |d| d.bg(theme.accent_soft))
        .when(!active, move |d| d.hover(move |s| s.bg(hover)))
        .child(icon(
            glyph,
            px(14.),
            if active {
                theme.accent_text
            } else {
                theme.text_soft
            },
        ))
        .tooltip(widgets::tooltip(label, *theme))
}

/// A menu button of the toolbar showing a label.
fn menu_label(
    id: &'static str,
    label: String,
    min_width: f32,
    help: String,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    let hover = theme.input;
    div()
        .id(id)
        .h(px(28.))
        .min_w(px(min_width))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .gap_1()
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .text_size(px(12.))
        .text_color(theme.text_soft)
        .child(label)
        .child(icon(Icon::ChevronDown, px(10.), theme.muted))
        .tooltip(widgets::tooltip(help, *theme))
}

fn menu(items: Vec<AnyElement>) -> AnyElement {
    div()
        .py_1()
        .flex()
        .flex_col()
        .children(items)
        .into_any_element()
}

/// One entry of a toolbar menu, running `run` then closing the menu.
fn entry(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    swatch: Option<gpui::Hsla>,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    run: impl Fn(&mut Oxide, &mut Window, &mut Context<Oxide>) + 'static,
) -> AnyElement {
    let hover = theme.input;
    div()
        .id(id)
        .min_w(px(160.))
        .h(px(28.))
        .mx_1()
        .px_2()
        .rounded(px(6.))
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(13.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .when_some(swatch, |d, color| {
            d.child(div().size(px(12.)).flex_none().rounded_full().bg(color))
        })
        .child(label.into())
        .on_click(cx.listener(move |this, _, window, cx| run(this, window, cx)))
        .into_any_element()
}

fn separator(theme: &Theme) -> AnyElement {
    div().my_1().h(px(1.)).bg(theme.border).into_any_element()
}

fn colors() -> Vec<(String, &'static str)> {
    vec![
        (tr("ui.formatting_toolbar.black"), "#202020"),
        (tr("ui.formatting_toolbar.gray"), "#666666"),
        (tr("ui.formatting_toolbar.red"), "#c4314b"),
        ("Orange".into(), "#c75200"),
        (tr("ui.formatting_toolbar.yellow"), "#a66900"),
        (tr("ui.formatting_toolbar.green"), "#237b4b"),
        (tr("ui.formatting_toolbar.blue"), "#0078d4"),
        (tr("ui.formatting_toolbar.purple"), "#744da9"),
    ]
}
fn highlights() -> Vec<(String, &'static str)> {
    vec![
        (tr("ui.formatting_toolbar.yellow"), "#fff100"),
        (tr("ui.formatting_toolbar.green"), "#92d050"),
        ("Cyan".into(), "#00ffff"),
        (tr("ui.formatting_toolbar.pink"), "#ff99cc"),
        (tr("ui.formatting_toolbar.lavender"), "#d9c2f0"),
    ]
}

fn open_menu(
    key: &'static str,
    cx: &mut Context<Oxide>,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static {
    cx.listener(move |this, _, _, cx| composer::toggle(this, key, cx))
}

pub fn toolbar(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let format = input(this).read(cx).current_format();
    let style_label = match format.block {
        ComposerBlock::Heading1 => tr("ui.formatting_toolbar.heading_1"),
        ComposerBlock::Heading2 => tr("ui.formatting_toolbar.heading_2"),
        ComposerBlock::Heading3 => tr("ui.formatting_toolbar.heading_3"),
        _ => tr("ui.formatting_toolbar.paragraph"),
    };
    let size = if format.size == 0 { 14 } else { format.size };

    let bold = tool(
        "fmt-bold",
        Icon::Bold,
        tr("ui.formatting_toolbar.bold_b"),
        format.bold,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| toggle_flag(this, |f| &mut f.bold, w, cx)));
    let italic = tool(
        "fmt-italic",
        Icon::Italic,
        tr("ui.formatting_toolbar.italic_i"),
        format.italic,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| toggle_flag(this, |f| &mut f.italic, w, cx)));
    let underline = tool(
        "fmt-underline",
        Icon::Underline,
        tr("ui.formatting_toolbar.underline_u"),
        format.underline,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| toggle_flag(this, |f| &mut f.underline, w, cx)));
    let strike = tool(
        "fmt-strike",
        Icon::Strike,
        tr("ui.formatting_toolbar.strikethrough_x"),
        format.strike,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| toggle_flag(this, |f| &mut f.strike, w, cx)));
    let color = with_popover(
        this,
        COLOR,
        tool(
            "fmt-color",
            Icon::FontColor,
            tr("ui.formatting_toolbar.font_color"),
            false,
            &theme,
        )
        .on_click(open_menu(COLOR, cx)),
        |this, _, cx| {
            let theme = this.theme;
            let mut items = vec![entry(
                "color-auto",
                tr("ui.formatting_toolbar.automatic"),
                None,
                &theme,
                cx,
                |this, w, cx| change(this, false, |f| f.color = None, w, cx),
            )];
            for (ix, (name, hex)) in colors().into_iter().enumerate() {
                items.push(entry(
                    ("color", ix),
                    name,
                    hex_color(hex),
                    &theme,
                    cx,
                    move |this, w, cx| {
                        change(this, false, move |f| f.color = Some(hex.to_owned()), w, cx)
                    },
                ));
            }
            menu(items)
        },
        window,
        cx,
    );
    let highlight = with_popover(
        this,
        HIGHLIGHT,
        tool(
            "fmt-highlight",
            Icon::Highlighter,
            tr("ui.formatting_toolbar.highlight_color"),
            false,
            &theme,
        )
        .on_click(open_menu(HIGHLIGHT, cx)),
        |this, _, cx| {
            let theme = this.theme;
            let mut items = vec![entry(
                "highlight-none",
                tr("ui.formatting_toolbar.no_highlight"),
                None,
                &theme,
                cx,
                |this, w, cx| change(this, false, |f| f.highlight = None, w, cx),
            )];
            for (ix, (name, hex)) in highlights().into_iter().enumerate() {
                items.push(entry(
                    ("highlight", ix),
                    name,
                    hex_color(hex),
                    &theme,
                    cx,
                    move |this, w, cx| {
                        change(
                            this,
                            false,
                            move |f| f.highlight = Some(hex.to_owned()),
                            w,
                            cx,
                        )
                    },
                ));
            }
            menu(items)
        },
        window,
        cx,
    );
    let sizes = with_popover(
        this,
        SIZE,
        menu_label(
            "fmt-size",
            format!("{size} pt"),
            44.,
            tr("ui.formatting_toolbar.font_size"),
            &theme,
        )
        .on_click(open_menu(SIZE, cx)),
        |this, _, cx| {
            let theme = this.theme;
            let items = [10u32, 12, 14, 18, 24, 32]
                .into_iter()
                .map(|size| {
                    entry(
                        ("size", size as usize),
                        format!("{size} pt"),
                        None,
                        &theme,
                        cx,
                        move |this, w, cx| change(this, false, move |f| f.size = size, w, cx),
                    )
                })
                .collect();
            menu(items)
        },
        window,
        cx,
    );
    let clear = tool(
        "fmt-clear",
        Icon::ClearFormat,
        tr("ui.formatting_toolbar.clear_all_formatting"),
        false,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| clear_formatting(this, w, cx)));

    let styles = with_popover(
        this,
        STYLE,
        menu_label(
            "fmt-style",
            style_label,
            70.,
            tr("ui.formatting_toolbar.paragraph_style"),
            &theme,
        )
        .on_click(open_menu(STYLE, cx)),
        |this, _, cx| {
            let theme = this.theme;
            let items = [
                (
                    "style-p",
                    tr("ui.formatting_toolbar.paragraph"),
                    ComposerBlock::Paragraph,
                ),
                (
                    "style-h1",
                    tr("ui.formatting_toolbar.heading_1"),
                    ComposerBlock::Heading1,
                ),
                (
                    "style-h2",
                    tr("ui.formatting_toolbar.heading_2"),
                    ComposerBlock::Heading2,
                ),
                (
                    "style-h3",
                    tr("ui.formatting_toolbar.heading_3"),
                    ComposerBlock::Heading3,
                ),
            ]
            .into_iter()
            .map(|(id, label, block)| {
                entry(id, label, None, &theme, cx, move |this, w, cx| {
                    set_block(this, block, w, cx)
                })
            })
            .collect();
            menu(items)
        },
        window,
        cx,
    );
    let bullet = tool(
        "fmt-bullet",
        Icon::ListBullet,
        tr("ui.formatting_toolbar.bulleted_list"),
        format.block == ComposerBlock::Bullet,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| set_block(this, ComposerBlock::Bullet, w, cx)));
    let numbered = tool(
        "fmt-numbered",
        Icon::ListNumber,
        tr("ui.formatting_toolbar.numbered_list"),
        format.block == ComposerBlock::Numbered,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| set_block(this, ComposerBlock::Numbered, w, cx)));
    let outdent = tool(
        "fmt-outdent",
        Icon::IndentLess,
        tr("ui.formatting_toolbar.decrease_indent"),
        false,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| indent(this, -1, w, cx)));
    let indent_more = tool(
        "fmt-indent",
        Icon::IndentMore,
        tr("ui.formatting_toolbar.increase_indent"),
        false,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| indent(this, 1, w, cx)));
    let quote = tool(
        "fmt-quote",
        Icon::Quote,
        tr("ui.formatting_toolbar.quote"),
        format.block == ComposerBlock::Quote,
        &theme,
    )
    .on_click(cx.listener(|this, _, w, cx| set_block(this, ComposerBlock::Quote, w, cx)));
    let link = with_popover(
        this,
        LINK,
        tool(
            "fmt-link",
            Icon::Link,
            tr("ui.formatting_toolbar.insert_link_k"),
            format.link.is_some(),
            &theme,
        )
        .on_click(cx.listener(|this, _, w, cx| {
            if this.popup_open(LINK) {
                close_link(this, w, cx)
            } else {
                request_link(this, w, cx)
            }
        })),
        link_editor,
        window,
        cx,
    );
    let more = with_popover(
        this,
        MORE,
        tool(
            "fmt-more",
            Icon::More,
            tr("ui.formatting_toolbar.more_formatting_options"),
            false,
            &theme,
        )
        .on_click(open_menu(MORE, cx)),
        |this, _, cx| {
            let theme = this.theme;
            let items = vec![
                entry(
                    "more-code",
                    tr("ui.formatting_toolbar.inline_code"),
                    None,
                    &theme,
                    cx,
                    |this, w, cx| toggle_flag(this, |f| &mut f.code, w, cx),
                ),
                entry(
                    "more-codeblock",
                    tr("ui.formatting_toolbar.code_block"),
                    None,
                    &theme,
                    cx,
                    |this, w, cx| set_block(this, ComposerBlock::CodeBlock, w, cx),
                ),
                separator(&theme),
                entry(
                    "more-left",
                    tr("ui.formatting_toolbar.align_left"),
                    None,
                    &theme,
                    cx,
                    |this, w, cx| change(this, true, |f| f.alignment = "left".into(), w, cx),
                ),
                entry(
                    "more-center",
                    tr("ui.formatting_toolbar.center"),
                    None,
                    &theme,
                    cx,
                    |this, w, cx| change(this, true, |f| f.alignment = "center".into(), w, cx),
                ),
                entry(
                    "more-right",
                    tr("ui.formatting_toolbar.align_right"),
                    None,
                    &theme,
                    cx,
                    |this, w, cx| change(this, true, |f| f.alignment = "right".into(), w, cx),
                ),
                separator(&theme),
                entry(
                    "more-rule",
                    tr("ui.formatting_toolbar.horizontal_line"),
                    None,
                    &theme,
                    cx,
                    insert_rule,
                ),
                entry(
                    "more-unlink",
                    tr("ui.formatting_toolbar.remove_link"),
                    None,
                    &theme,
                    cx,
                    |this, w, cx| change(this, false, |f| f.link = None, w, cx),
                ),
                separator(&theme),
                entry(
                    "more-undo",
                    tr("ui.formatting_toolbar.undo"),
                    None,
                    &theme,
                    cx,
                    |this, w, cx| {
                        this.close_popup(cx);
                        input(this).update(cx, |i, cx| {
                            i.focus(w);
                            i.undo(cx)
                        });
                    },
                ),
                entry(
                    "more-redo",
                    tr("ui.formatting_toolbar.redo"),
                    None,
                    &theme,
                    cx,
                    |this, w, cx| {
                        this.close_popup(cx);
                        input(this).update(cx, |i, cx| {
                            i.focus(w);
                            i.redo(cx)
                        });
                    },
                ),
            ];
            menu(items)
        },
        window,
        cx,
    );

    let row = || div().flex().items_center().gap(px(2.));
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .pb(px(4.))
        .border_b_1()
        .border_color(theme.border)
        .child(
            row()
                .child(bold)
                .child(italic)
                .child(underline)
                .child(strike)
                .child(div().w(px(1.)).h(px(18.)).mx(px(3.)).bg(theme.border))
                .child(color)
                .child(highlight)
                .child(sizes)
                .child(clear),
        )
        .child(
            row()
                .child(styles)
                .child(bullet)
                .child(numbered)
                .child(outdent)
                .child(indent_more)
                .child(quote)
                .child(link)
                .child(more),
        )
        .into_any_element()
}

/// The link popover: text to show and address.
fn link_editor(this: &mut Oxide, _: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let (title, url) = (
        this.ui.composer.toolbar.title.clone(),
        this.ui.composer.toolbar.url.clone(),
    );
    let valid = valid_link(url.read(cx).text());
    div()
        .w(px(310.))
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.formatting_toolbar.insert_link")),
        )
        .child(widgets::text_field(&title, &theme, cx))
        .child(widgets::text_field(&url, &theme, cx))
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    widgets::button(
                        "link-cancel",
                        tr("ui.formatting_toolbar.cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, w, cx| close_link(this, w, cx))),
                )
                .child(
                    widgets::disabled(
                        widgets::button(
                            "link-insert",
                            tr("ui.formatting_toolbar.insert"),
                            ButtonKind::Primary,
                            &theme,
                        ),
                        !valid,
                    )
                    .when(valid, |d| {
                        d.on_click(cx.listener(|this, _, w, cx| insert_link(this, w, cx)))
                    }),
                ),
        )
        .into_any_element()
}

/// Nothing floats over the window: the toolbar's menus hang from their buttons.
pub fn overlay(_: &mut Oxide, _: &mut Window, _: &mut Context<Oxide>) -> Option<AnyElement> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn links_need_a_host_or_an_address() {
        assert!(super::valid_link("https://example.com/a"));
        assert!(super::valid_link("mailto:a@b.c"));
        assert!(!super::valid_link("https://"));
        assert!(!super::valid_link("javascript:alert(1)"));
        assert!(!super::valid_link("example.com"));
    }
}
