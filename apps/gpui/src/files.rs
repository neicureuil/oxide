//! The Files screen, the folder browser and a conversation's Shared tab (owner: files agent), as `FilesScreens.swift`.
use crate::files_parts::{self as parts, Menu, MenuTarget, send};
/// For the feed (message attachments) and the composer (draft attachments).
#[allow(unused_imports)]
pub use crate::files_parts::{attachment_chips, file_badge, file_card};
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput};
use crate::oxide::{Oxide, tr, trf};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, ClickEvent, Context, Entity, ExternalPaths, FocusHandle, FontWeight, KeyDownEvent,
    ListAlignment, ListState, ScrollStrategy, SharedString, Task, UniformListScrollHandle, Window,
    div, list, prelude::*, px, uniform_list,
};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::time::Duration;
use teams_core::{BrowserPlace, BrowserView, EntryView, FileAction, FileView, SharedFilter};

/// Pause in typing before searching OneDrive or the channel's folder; the Shared tab filters at once.
const SEARCH_DELAY: Duration = Duration::from_millis(350);

/// A name asked for: a new folder, or a new name for an entry.
#[derive(Clone)]
pub enum Naming {
    Folder,
    Rename(EntryView),
}

pub struct Ui {
    browser_search: Entity<TextInput>,
    shared_search: Entity<TextInput>,
    name: Entity<TextInput>,
    /// The core's queries last seen: a field empties when its query ends (unless it is being typed in).
    browser_seen: String,
    shared_seen: String,
    /// The pending OneDrive search, dropped (cancelled) by the next keystroke.
    search_task: Option<Task<()>>,
    /// Row chosen with a click or the arrows: Return opens it, Space previews it.
    selected: Option<String>,
    table_focus: FocusHandle,
    table: UniformListScrollHandle,
    shared_list: ListState,
    /// A hash of each Shared card as last measured, to measure again only those that changed.
    shared_rows: Vec<u64>,
    pub(crate) menu: Option<Menu>,
    naming: Option<Naming>,
    deleting: Option<EntryView>,
}

impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let browser_search = cx.new(|cx| TextInput::new(cx, ""));
        let shared_search = cx.new(|cx| TextInput::new(cx, ""));
        let name = cx.new(|cx| TextInput::new(cx, tr("ui.files_screens.name")));
        cx.subscribe(
            &browser_search,
            |this: &mut Oxide, input, event: &InputEvent, cx| {
                let query = input.read(cx).text().to_owned();
                match event {
                    InputEvent::Changed => browser_query_changed(this, query, cx),
                    InputEvent::Submit => {
                        this.ui.files.search_task = None;
                        send(this, FileAction::SearchFiles { query });
                    }
                    InputEvent::Cancel => clear_browser_search(this, cx),
                    _ => {}
                }
            },
        )
        .detach();
        cx.subscribe(
            &shared_search,
            |this: &mut Oxide, input, event: &InputEvent, cx| {
                let query = input.read(cx).text().to_owned();
                match event {
                    InputEvent::Changed | InputEvent::Submit => {
                        let current = this.files.as_ref().map(|f| f.query.as_str()).unwrap_or("");
                        if query.trim() != current || matches!(event, InputEvent::Submit) {
                            send(this, FileAction::SharedSearch { query });
                        }
                    }
                    InputEvent::Cancel => clear_shared_search(this, cx),
                    _ => {}
                }
            },
        )
        .detach();
        cx.subscribe(
            &name,
            |this: &mut Oxide, _, event: &InputEvent, cx| match event {
                InputEvent::Submit => submit_name(this, cx),
                InputEvent::Cancel => {
                    this.ui.files.naming = None;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        Self {
            browser_search,
            shared_search,
            name,
            browser_seen: String::new(),
            shared_seen: String::new(),
            search_task: None,
            selected: None,
            table_focus: cx.focus_handle(),
            table: UniformListScrollHandle::new(),
            shared_list: ListState::new(0, ListAlignment::Top, px(300.)),
            shared_rows: Vec::new(),
            menu: None,
            naming: None,
            deleting: None,
        }
    }
}

/// The Files destination: OneDrive places on the left, the browser on the right.
pub fn screen(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    div()
        .size_full()
        .flex()
        .bg(theme.surface)
        .child(places(this, cx))
        .child(browser(this, window, cx))
        .into_any_element()
}

/// A conversation's files tab: a channel's SharePoint folder, or a chat's "Partagé" list.
pub fn conversation_tab(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    if this.conversation.as_ref().is_some_and(|c| c.is_channel) {
        browser(this, window, cx)
    } else {
        shared(this, window, cx)
    }
}

// MARK: OneDrive places

fn places(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let current = this.browser.as_ref().map(|b| b.place);
    let place = |id: &'static str,
                 label: String,
                 glyph: Icon,
                 value: BrowserPlace,
                 cx: &mut Context<Oxide>| {
        let selected = current == Some(value);
        let hover = theme.input;
        div()
            .id(id)
            .w_full()
            .py(px(7.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(6.))
            .text_size(px(13.))
            .cursor_pointer()
            .when(selected, |d| {
                d.bg(theme.accent_soft).font_weight(FontWeight::SEMIBOLD)
            })
            .when(!selected, move |d| d.hover(move |s| s.bg(hover)))
            .child(icon(
                glyph,
                px(15.),
                if selected {
                    theme.accent_text
                } else {
                    theme.muted
                },
            ))
            .child(label)
            .on_click(
                cx.listener(move |this, _, _, _| send(this, FileAction::Browse { place: value })),
            )
    };
    div()
        .w(px(200.))
        .flex_none()
        .h_full()
        .p_4()
        .pt(px(40.))
        .flex()
        .flex_col()
        .gap_1()
        .border_r_1()
        .border_color(theme.border)
        .child(widgets::title(tr("ui.files_screens.files"), 20.).pb(px(10.)))
        .child(place(
            "place-recent",
            tr("ui.files_screens.recent"),
            Icon::Clock,
            BrowserPlace::Recent,
            cx,
        ))
        .child(place(
            "place-mine",
            tr("ui.files_screens.my_files"),
            Icon::Folder,
            BrowserPlace::MyFiles,
            cx,
        ))
        .child(place(
            "place-shared",
            tr("ui.files_screens.shared_with_me"),
            Icon::People,
            BrowserPlace::SharedWithMe,
            cx,
        ))
        .into_any_element()
}

// MARK: Folder browser

/// OneDrive (global Files) or a channel's SharePoint folder.
fn browser(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let view = this.browser.clone().unwrap_or_default();
    follow_core_query(this, true, &view.query, window, cx);
    let placeholder = if view.place == BrowserPlace::Channel {
        tr("ui.files_screens.search_channel_files")
    } else {
        tr("ui.files_screens.search_my_files")
    };
    this.ui
        .files
        .browser_search
        .update(cx, |input, _| input.set_placeholder(placeholder));
    let input = this.ui.files.browser_search.clone();
    let search = parts::search_box(
        &input,
        |this, _, cx| clear_browser_search(this, cx),
        this,
        window,
        cx,
    );

    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .child(crumbs(&view, &theme, cx))
        .child(div().flex_1())
        .when(view.loading || view.busy, |d| {
            d.child(parts::small_spinner("browser-busy", &theme))
        })
        .when(view.can_write, |d| {
            d.child(
                parts::outline_button(
                    "new-folder",
                    Some(Icon::FolderPlus),
                    tr("ui.files_screens.new_folder"),
                    true,
                    &theme,
                )
                .on_click(
                    cx.listener(|this, _, window, cx| ask_name(this, Naming::Folder, window, cx)),
                ),
            )
            .child(
                parts::outline_button(
                    "upload",
                    Some(Icon::Upload),
                    tr("ui.files_screens.upload"),
                    true,
                    &theme,
                )
                .on_click(cx.listener(|_, _, _, cx| parts::choose_uploads(cx))),
            )
        })
        .child(
            widgets::icon_button_sized("browser-refresh", Icon::Refresh, 34., 16., false, &theme)
                .tooltip(widgets::tooltip(tr("ui.files_screens.refresh"), theme))
                .on_click(cx.listener(|this, _, _, _| send(this, FileAction::RefreshBrowser))),
        );

    let can_write = view.can_write;
    let accent = theme.accent;
    div()
        .id("browser")
        .flex_1()
        .min_w_0()
        .h_full()
        .p_6()
        .flex()
        .flex_col()
        .gap_3()
        .rounded(px(12.))
        .border_2()
        .border_dashed()
        .border_color(gpui::transparent_black())
        .when(can_write, |d| {
            d.drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(accent))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, _| {
                    parts::upload(this, paths.paths())
                }))
        })
        .child(header)
        .child(search)
        .when_some(view.error.clone(), |d, error| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(error),
            )
        })
        .child(table(this, &view, cx))
        .into_any_element()
}

fn crumbs(view: &BrowserView, theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    if view.crumbs.is_empty() {
        let title = if view.title.is_empty() {
            tr("ui.files_screens.files")
        } else {
            view.title.clone()
        };
        return widgets::title(title, 20.).truncate().into_any_element();
    }
    let last = view.crumbs.len() - 1;
    let mut row = div().min_w_0().flex().items_center().gap_1();
    for (index, name) in view.crumbs.iter().enumerate() {
        if index > 0 {
            row = row.child(icon(Icon::ChevronRight, px(11.), theme.subtle));
        }
        let current = index == last;
        row = row.child(
            div()
                .id(("crumb", index))
                .when(current, |d| d.min_w_0().truncate())
                .flex_none()
                .text_size(px(if current { 20. } else { 15. }))
                .when(current, |d| d.font_weight(FontWeight::SEMIBOLD))
                .text_color(if current { theme.text } else { theme.muted })
                .child(name.clone())
                .when(!current, |d| {
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, _| {
                            send(
                                this,
                                FileAction::Crumb {
                                    index: index as u32,
                                },
                            )
                        }))
                }),
        );
    }
    row.into_any_element()
}

const NAME_COLUMNS: [f32; 3] = [90., 140., 90.];

fn table(this: &mut Oxide, view: &BrowserView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    if view.entries.is_empty() && !view.loading {
        let note = if view.query.is_empty() {
            tr("ui.files_screens.this_folder_is_empty")
        } else {
            tr("ui.files_screens.no_results")
        };
        return empty_state(Icon::Folder, note, &theme);
    }
    let heading = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .pb(px(6.))
        .border_b_1()
        .border_color(theme.border)
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted)
        .child(div().flex_1().child(tr("ui.files_screens.name")))
        .child(
            div()
                .w(px(NAME_COLUMNS[0]))
                .child(tr("ui.files_screens.modified")),
        )
        .child(
            div()
                .w(px(NAME_COLUMNS[1]))
                .child(tr("ui.files_screens.modified_by")),
        )
        .child(
            div()
                .w(px(NAME_COLUMNS[2]))
                .flex()
                .justify_end()
                .child(tr("ui.files_screens.size")),
        )
        .child(div().w(px(24.)));
    // Search results show their folder under the name: every row is then taller, so the list stays uniform.
    let tall = view.entries.iter().any(|e| !e.location.is_empty());
    let entries = Rc::new(view.entries.clone());
    let rows = entries.clone();
    let focus = this.ui.files.table_focus.clone();
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(heading)
        .child(
            uniform_list(
                "entries",
                entries.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|ix| entry_row(this, &rows[ix], tall, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(this.ui.files.table.clone())
            .flex_1(),
        )
        .track_focus(&focus)
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if table_key(this, &entries, event.keystroke.key.as_str(), window, cx) {
                cx.stop_propagation();
            }
        }))
        .into_any_element()
}

/// Arrows move the selection, Return opens, Space previews; true when the key was used.
fn table_key(
    this: &mut Oxide,
    entries: &[EntryView],
    key: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let position = this
        .ui
        .files
        .selected
        .as_ref()
        .and_then(|s| entries.iter().position(|e| &e.key == s));
    match key {
        "up" | "down" => {
            let Some(index) =
                step_selection(position, if key == "up" { -1 } else { 1 }, entries.len())
            else {
                return false;
            };
            this.ui.files.selected = Some(entries[index].key.clone());
            this.ui
                .files
                .table
                .scroll_to_item(index, ScrollStrategy::Top);
            cx.notify();
            true
        }
        "enter" => {
            let Some(entry) = position.map(|i| entries[i].clone()) else {
                return false;
            };
            open_entry(this, &entry, window);
            true
        }
        "space" => {
            let Some(entry) = position.map(|i| &entries[i]).filter(|e| !e.folder) else {
                return false;
            };
            send(
                this,
                FileAction::Preview {
                    key: entry.key.clone(),
                },
            );
            true
        }
        _ => false,
    }
}

/// The row an arrow lands on: from none, the first going down or the last going up; clamped at the ends.
fn step_selection(current: Option<usize>, offset: isize, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let target = match current {
        Some(i) => i as isize + offset,
        None if offset > 0 => 0,
        None => count as isize - 1,
    };
    Some(target.clamp(0, count as isize - 1) as usize)
}

fn entry_row(
    this: &mut Oxide,
    entry: &EntryView,
    tall: bool,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let selected = this.ui.files.selected.as_ref() == Some(&entry.key);
    let clicked = entry.clone();
    let menu_target = entry.clone();
    let context_target = entry.clone();
    let mut name = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(
            div()
                .truncate()
                .text_size(px(13.))
                .child(entry.name.clone()),
        );
    if !entry.location.is_empty() {
        name = name.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(11.))
                .text_color(theme.muted)
                .child(icon(Icon::Folder, px(11.), theme.muted))
                .child(div().truncate().child(entry.location.clone())),
        );
    }
    div()
        .id(SharedString::from(format!("entry-{}", entry.key)))
        .w_full()
        .h(px(if tall { 50. } else { 45. }))
        .border_b_1()
        .border_color(theme.border)
        .flex()
        .items_center()
        .child(
            div()
                .w_full()
                .px(px(10.))
                .py(px(8.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(6.))
                .when(selected, |d| d.bg(theme.accent_soft))
                .text_size(px(12.))
                .text_color(theme.text)
                .child(file_badge(
                    &entry.ext,
                    entry.folder,
                    entry.color,
                    28.,
                    &theme,
                ))
                .child(name)
                .when_some(entry.progress, |d, p| {
                    d.child(parts::progress_bar(p, 80., &theme))
                })
                .child(
                    div()
                        .w(px(NAME_COLUMNS[0]))
                        .flex_none()
                        .truncate()
                        .child(entry.modified.clone()),
                )
                .child(
                    div()
                        .w(px(NAME_COLUMNS[1]))
                        .flex_none()
                        .truncate()
                        .child(entry.modified_by.clone()),
                )
                .child(
                    div()
                        .w(px(NAME_COLUMNS[2]))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .child(entry.size.clone()),
                )
                .child(
                    widgets::icon_button_sized(
                        SharedString::from(format!("entry-more-{}", entry.key)),
                        Icon::More,
                        24.,
                        15.,
                        false,
                        &theme,
                    )
                    .tooltip(widgets::tooltip(
                        trf("ui.files_screens.actions_for", &[("name", &entry.name)]),
                        theme,
                    ))
                    .on_click(cx.listener(
                        move |this, event: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            parts::open_menu(
                                this,
                                MenuTarget::Entry(menu_target.clone()),
                                event.position(),
                                cx,
                            )
                        },
                    )),
                ),
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.ui.files.selected = Some(clicked.key.clone());
            window.focus(&this.ui.files.table_focus);
            if event.click_count() >= 2 {
                open_entry(this, &clicked, window);
            }
            cx.notify();
        }))
        .on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                this.ui.files.selected = Some(context_target.key.clone());
                parts::open_menu(
                    this,
                    MenuTarget::Entry(context_target.clone()),
                    event.position,
                    cx,
                );
            }),
        )
        .into_any_element()
}

fn open_entry(this: &mut Oxide, entry: &EntryView, _window: &mut Window) {
    let key = entry.key.clone();
    if entry.can_open_folder {
        this.ui.files.selected = None;
        send(this, FileAction::OpenFolder { key });
    } else {
        send(this, FileAction::Open { key });
    }
}

/// The menu of a browser row (`FileBrowserScreen.menu`).
pub(crate) fn entry_menu(
    this: &mut Oxide,
    entry: &EntryView,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let theme = this.theme;
    let can_write = this.browser.as_ref().is_some_and(|b| b.can_write);
    let key = entry.key.clone();
    let mut out = Vec::new();
    let label = if entry.folder {
        tr("ui.files_screens.open")
    } else {
        tr("ui.files_screens.open_in_browser")
    };
    out.push(parts::item(
        "m-open",
        Some(if entry.folder {
            Icon::Folder
        } else {
            Icon::External
        }),
        label,
        true,
        &theme,
        cx,
        {
            let entry = entry.clone();
            move |this, window, _| open_entry(this, &entry, window)
        },
    ));
    if !entry.folder {
        out.push(parts::item(
            "m-preview",
            Some(Icon::Eye),
            tr("ui.files_screens.preview"),
            true,
            &theme,
            cx,
            {
                let key = key.clone();
                move |this, _, _| send(this, FileAction::Preview { key: key.clone() })
            },
        ));
        out.push(parts::item(
            "m-download",
            Some(Icon::Download),
            tr("ui.files_screens.download"),
            true,
            &theme,
            cx,
            {
                let (key, name) = (key.clone(), entry.name.clone());
                move |_, _, cx| parts::download(key.clone(), name.clone(), cx)
            },
        ));
    }
    out.push(parts::item(
        "m-copy",
        Some(Icon::Copy),
        tr("ui.files_screens.copy_link"),
        true,
        &theme,
        cx,
        {
            let key = key.clone();
            move |this, _, _| send(this, FileAction::CopyLink { key: key.clone() })
        },
    ));
    if can_write {
        out.push(parts::menu_divider(&theme));
        out.push(parts::item(
            "m-rename",
            Some(Icon::Pencil),
            tr("ui.files_screens.rename_2"),
            true,
            &theme,
            cx,
            {
                let entry = entry.clone();
                move |this, window, cx| ask_name(this, Naming::Rename(entry.clone()), window, cx)
            },
        ));
        if entry.folder {
            // A submenu in Swift: here the colour list takes the menu's place.
            let hover = theme.input;
            out.push(
                div()
                    .id("m-color")
                    .min_w(px(200.))
                    .h(px(30.))
                    .mx_1()
                    .px_2()
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(
                        div()
                            .size(px(15.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(11.))
                                    .rounded_full()
                                    .bg(parts::folder_color(entry.color, &theme)),
                            ),
                    )
                    .child(div().flex_1().child(tr("ui.files_screens.folder_color")))
                    .child(icon(Icon::ChevronRight, px(12.), theme.muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(menu) = this.ui.files.menu.as_mut() {
                            menu.colors = true;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let entry = entry.clone();
        let delete = widgets::menu_item(
            "m-delete",
            Some(Icon::Trash),
            tr("ui.files_screens.delete_2"),
            &theme,
        )
        .text_color(theme.danger)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.close_popup(cx);
            this.ui.files.deleting = Some(entry.clone());
        }));
        out.push(delete.into_any_element());
    }
    out
}

// MARK: Shared tab

/// "Partagé" tab of a chat: files and links shared in the conversation.
fn shared(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let view = this.files.clone();
    let query = view.as_ref().map(|v| v.query.clone()).unwrap_or_default();
    follow_core_query(this, false, &query, window, cx);
    let loading = view.as_ref().is_some_and(|v| v.loading);
    let filter = view.as_ref().map(|v| v.filter).unwrap_or_default();
    let input = this.ui.files.shared_search.clone();
    input.update(cx, |i, _| {
        i.set_placeholder(tr("ui.files_screens.search_for_a_file_link_or"))
    });
    let search = parts::search_box(
        &input,
        |this, _, cx| clear_shared_search(this, cx),
        this,
        window,
        cx,
    );

    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .child(widgets::title(tr("ui.files_screens.shared"), 22.).flex_1())
        .when(loading, |d| {
            d.child(parts::small_spinner("shared-loading", &theme))
        })
        .child(
            widgets::icon_button_sized("shared-refresh", Icon::Refresh, 34., 16., false, &theme)
                .tooltip(widgets::tooltip(tr("ui.files_screens.refresh"), theme))
                .on_click(cx.listener(|this, _, _, _| send(this, FileAction::RefreshShared))),
        );
    let picker = segmented(
        &[
            (SharedFilter::All, tr("ui.files_screens.all")),
            (SharedFilter::Files, tr("ui.files_screens.files")),
            (SharedFilter::Links, tr("ui.files_screens.links")),
        ],
        filter,
        &theme,
        cx,
    );

    let files: Vec<FileView> = view.as_ref().map(|v| v.files.clone()).unwrap_or_default();
    let hashes: Vec<u64> = files.iter().map(|f| card_hash(this, f)).collect();
    if let Some(changed) = changed_rows(&this.ui.files.shared_rows, &hashes) {
        for ix in changed {
            this.ui.files.shared_list.splice(ix..ix + 1, 1);
        }
    } else {
        this.ui.files.shared_list.reset(hashes.len());
    }
    this.ui.files.shared_rows = hashes;

    let top = div()
        .px(px(28.))
        .pt(px(28.))
        .pb_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(header)
        .child(div().flex().child(picker))
        .child(search)
        .when_some(view.as_ref().and_then(|v| v.error.clone()), |d, error| {
            d.child(widgets::muted(
                trf(
                    "ui.files_screens.full_list_unavailable_files_from_loaded",
                    &[("error", &error)],
                ),
                12.,
                &theme,
            ))
        });
    let body = if files.is_empty() && !loading {
        let (glyph, note) = if query.is_empty() {
            (
                Icon::Copy,
                tr("ui.files_screens.nothing_has_been_shared_in_this"),
            )
        } else {
            (Icon::Search, tr("ui.files_screens.no_results"))
        };
        empty_state(glyph, note, &theme)
    } else {
        let files = Rc::new(files);
        list(
            this.ui.files.shared_list.clone(),
            cx.processor(move |this, ix: usize, _, cx| {
                div()
                    .px(px(28.))
                    .pb_3()
                    .child(file_card(this, &files[ix], cx))
                    .into_any_element()
            }),
        )
        .flex_1()
        .into_any_element()
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(top)
        .child(body)
        .into_any_element()
}

/// What a card shows, its thumbnail's presence included: a change means measuring it again.
fn card_hash(this: &Oxide, file: &FileView) -> u64 {
    let mut hasher = DefaultHasher::new();
    file.hash(&mut hasher);
    file.thumbnail
        .as_ref()
        .is_some_and(|t| this.images.images.contains_key(&t.src))
        .hash(&mut hasher);
    hasher.finish()
}

/// Rows whose content changed, or None when rows came or went (the list is then measured anew).
fn changed_rows(old: &[u64], new: &[u64]) -> Option<Vec<usize>> {
    (old.len() == new.len()).then(|| (0..new.len()).filter(|&i| old[i] != new[i]).collect())
}

/// `SegmentedPicker`: the choices in a bordered strip, the chosen one tinted.
fn segmented(
    options: &[(SharedFilter, String)],
    current: SharedFilter,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    // The chosen segment's tint slides to the new one (`withAnimation(Motion.select)`).
    let accent = theme.accent_soft;
    let (highlight, slid, measure) = crate::motion::sliding_selection(
        "segmented-shared-filter",
        options.iter().position(|(v, _)| *v == current),
        3.,
        move |d| d.top(px(3.)).bottom(px(3.)).rounded(px(6.)).bg(accent),
    );
    let mut strip = div()
        .relative()
        .flex()
        .gap(px(2.))
        .p(px(3.))
        .rounded(px(8.))
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border_strong)
        .on_children_prepainted(measure)
        .child(highlight.unwrap_or_else(|| div().into_any_element()));
    for (ix, (value, label)) in options.iter().enumerate() {
        let active = *value == current;
        let value = *value;
        strip = strip.child(
            div()
                .id(("filter", ix))
                .min_h(px(26.))
                .px(px(14.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(active, |d| {
                    d.when(!slid, |d| d.bg(theme.accent_soft))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.accent_text)
                })
                .when(!active, |d| d.text_color(theme.text_soft))
                .child(label.clone())
                .on_click(cx.listener(move |this, _, _, _| {
                    send(this, FileAction::SharedFilter { filter: value })
                })),
        );
    }
    strip.into_any_element()
}

fn empty_state(glyph: Icon, note: String, theme: &Theme) -> AnyElement {
    div()
        .w_full()
        .p(px(40.))
        .flex()
        .flex_col()
        .items_center()
        .gap_3()
        .child(icon(glyph, px(38.), theme.subtle))
        .child(widgets::muted(note, 14., theme))
        .into_any_element()
}

// MARK: Search

/// Empties a search field when the core ends its query, unless the user is typing in it.
fn follow_core_query(
    this: &mut Oxide,
    browser: bool,
    current: &str,
    window: &Window,
    cx: &mut Context<Oxide>,
) {
    let ui = &mut this.ui.files;
    let (input, seen) = if browser {
        (&ui.browser_search, &mut ui.browser_seen)
    } else {
        (&ui.shared_search, &mut ui.shared_seen)
    };
    if seen != current {
        if current.is_empty() && !input.read(cx).is_focused(window) {
            input.update(cx, |i, cx| i.set_text("", None, cx));
        }
        *seen = current.to_owned();
    }
}

fn browser_query_changed(this: &mut Oxide, query: String, cx: &mut Context<Oxide>) {
    let current = this
        .browser
        .as_ref()
        .map(|b| b.query.as_str())
        .unwrap_or("");
    if query.trim() == current {
        this.ui.files.search_task = None;
        return;
    }
    if query.is_empty() {
        this.ui.files.search_task = None;
        send(this, FileAction::SearchFiles { query });
        return;
    }
    this.ui.files.search_task = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(SEARCH_DELAY).await;
        let _ = this.update(cx, |this, _| send(this, FileAction::SearchFiles { query }));
    }));
}

fn clear_browser_search(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui
        .files
        .browser_search
        .update(cx, |i, cx| i.set_text("", None, cx));
    browser_query_changed(this, String::new(), cx);
}

fn clear_shared_search(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui
        .files
        .shared_search
        .update(cx, |i, cx| i.set_text("", None, cx));
    if this.files.as_ref().is_some_and(|f| !f.query.is_empty()) {
        send(
            this,
            FileAction::SharedSearch {
                query: String::new(),
            },
        );
    }
}

// MARK: Dialogs

fn ask_name(this: &mut Oxide, naming: Naming, window: &mut Window, cx: &mut Context<Oxide>) {
    let initial = match &naming {
        Naming::Folder => String::new(),
        Naming::Rename(entry) => entry.name.clone(),
    };
    this.ui
        .files
        .name
        .update(cx, |i, cx| i.set_text(initial, None, cx));
    this.ui.files.name.read(cx).focus(window);
    this.ui.files.naming = Some(naming);
    cx.notify();
}

/// The trimmed name, if there is one.
fn valid_name(text: &str) -> Option<String> {
    let name = text.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn submit_name(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let Some(name) = valid_name(this.ui.files.name.read(cx).text()) else {
        return;
    };
    match this.ui.files.naming.take() {
        Some(Naming::Folder) => send(this, FileAction::NewFolder { name }),
        Some(Naming::Rename(entry)) => send(
            this,
            FileAction::Rename {
                key: entry.key,
                name,
            },
        ),
        None => {}
    }
    cx.notify();
}

fn name_dialog(this: &mut Oxide, naming: &Naming, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let title = match naming {
        Naming::Folder => tr("ui.files_screens.new_folder"),
        Naming::Rename(_) => tr("ui.files_screens.rename"),
    };
    let ok = valid_name(this.ui.files.name.read(cx).text()).is_some();
    let content = div()
        .p_5()
        .flex()
        .flex_col()
        .gap(px(14.))
        .text_color(theme.text)
        .child(widgets::title(title, 16.))
        .child(widgets::text_field(&this.ui.files.name, &theme, cx))
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    widgets::button(
                        "name-cancel",
                        tr("ui.files_screens.cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ui.files.naming = None;
                        cx.notify();
                    })),
                )
                .child(
                    parts::dialog_button(
                        "name-ok",
                        tr("ui.files_screens.ok"),
                        ButtonKind::Primary,
                        ok,
                        &theme,
                    )
                    .when(ok, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| submit_name(this, cx)))
                    }),
                ),
        );
    let entity = cx.entity();
    widgets::modal("files-name", px(360.), content, &theme, move |_, cx| {
        entity.update(cx, |this, cx| {
            this.ui.files.naming = None;
            cx.notify();
        })
    })
}

fn delete_dialog(this: &mut Oxide, entry: &EntryView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let key = entry.key.clone();
    let content = div()
        .p_5()
        .flex()
        .flex_col()
        .gap_3()
        .text_color(theme.text)
        .child(widgets::title(
            trf("ui.files_screens.delete", &[("name", &entry.name)]),
            16.,
        ))
        .child(widgets::muted(
            tr("ui.files_screens.the_item_goes_to_the_sharepoint"),
            13.,
            &theme,
        ))
        .child(
            div()
                .pt_2()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    widgets::button(
                        "delete-cancel",
                        tr("ui.files_screens.cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ui.files.deleting = None;
                        cx.notify();
                    })),
                )
                .child(
                    widgets::button(
                        "delete-ok",
                        tr("ui.files_screens.move_to_recycle_bin"),
                        ButtonKind::Danger,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ui.files.deleting = None;
                        send(this, FileAction::Delete { key: key.clone() });
                        cx.notify();
                    })),
                ),
        );
    let entity = cx.entity();
    widgets::modal("files-delete", px(380.), content, &theme, move |_, cx| {
        entity.update(cx, |this, cx| {
            this.ui.files.deleting = None;
            cx.notify();
        })
    })
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    if let Some(naming) = this.ui.files.naming.clone() {
        return Some(name_dialog(this, &naming, cx));
    }
    if let Some(entry) = this.ui.files.deleting.clone() {
        return Some(delete_dialog(this, &entry, cx));
    }
    parts::menu_overlay(this, window, cx)
}

// MARK: Debug driver

/// Steps of `OXIDE_SCRIPT` for these screens (see `screens::run_script`):
/// `files:recent|mine|withme` (places), `files:open <n>` (n-th row: folder or file), `files:crumb <n>`,
/// `files:select <n>`, `files:menu <n>`, `files:colors <n>`, `files:rename <n>`, `files:delete <n>`,
/// `files:newfolder`, `files:search <text>`, `files:filter all|files|links`, `files:sharedsearch <text>`,
/// `files:cardmenu <n>` (n-th Shared card), `files:closemenu`, `files:tab` (the open conversation's files tab).
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("files:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    let n = arg.trim().parse::<usize>().ok();
    let entry = n.and_then(|n| this.browser.as_ref()?.entries.get(n).cloned());
    let menu_at = gpui::point(px(700.), px(200.));
    match verb {
        "recent" => send(
            this,
            FileAction::Browse {
                place: BrowserPlace::Recent,
            },
        ),
        "mine" => send(
            this,
            FileAction::Browse {
                place: BrowserPlace::MyFiles,
            },
        ),
        "withme" => send(
            this,
            FileAction::Browse {
                place: BrowserPlace::SharedWithMe,
            },
        ),
        "open" => {
            let Some(entry) = entry else { return false };
            open_entry(this, &entry, window);
        }
        "crumb" => send(
            this,
            FileAction::Crumb {
                index: n.unwrap_or(0) as u32,
            },
        ),
        "select" => {
            let Some(entry) = entry else { return false };
            this.ui.files.selected = Some(entry.key);
            window.focus(&this.ui.files.table_focus);
        }
        "menu" | "colors" => {
            let Some(entry) = entry else { return false };
            parts::open_menu(this, MenuTarget::Entry(entry), menu_at, cx);
            if verb == "colors"
                && let Some(menu) = this.ui.files.menu.as_mut()
            {
                menu.colors = true;
            }
        }
        "cardmenu" => {
            let Some(file) = n.and_then(|n| this.files.as_ref()?.files.get(n).cloned()) else {
                return false;
            };
            parts::open_menu(this, MenuTarget::File(file), menu_at, cx);
        }
        "closemenu" => this.close_popup(cx),
        "tab" => this.send(teams_core::Action::SetTab {
            tab: teams_core::Tab::Files,
        }),
        "rename" => {
            let Some(entry) = entry else { return false };
            ask_name(this, Naming::Rename(entry), window, cx);
        }
        "delete" => {
            let Some(entry) = entry else { return false };
            this.ui.files.deleting = Some(entry);
        }
        "newfolder" => ask_name(this, Naming::Folder, window, cx),
        "search" => {
            this.ui
                .files
                .browser_search
                .update(cx, |i, cx| i.set_text(arg, None, cx));
            send(
                this,
                FileAction::SearchFiles {
                    query: arg.to_owned(),
                },
            );
        }
        "sharedsearch" => {
            this.ui
                .files
                .shared_search
                .update(cx, |i, cx| i.set_text(arg, None, cx));
            send(
                this,
                FileAction::SharedSearch {
                    query: arg.to_owned(),
                },
            );
        }
        "filter" => {
            let filter = match arg {
                "files" => SharedFilter::Files,
                "links" => SharedFilter::Links,
                _ => SharedFilter::All,
            };
            send(this, FileAction::SharedFilter { filter });
        }
        _ => return false,
    }
    cx.notify();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_start_at_an_end_and_stop_at_the_other() {
        assert_eq!(step_selection(None, 1, 3), Some(0));
        assert_eq!(step_selection(None, -1, 3), Some(2));
        assert_eq!(step_selection(Some(2), 1, 3), Some(2));
        assert_eq!(step_selection(Some(0), -1, 3), Some(0));
        assert_eq!(step_selection(Some(1), 1, 3), Some(2));
        assert_eq!(step_selection(None, 1, 0), None);
    }

    #[test]
    fn only_changed_cards_are_measured_again() {
        assert_eq!(changed_rows(&[1, 2, 3], &[1, 9, 3]), Some(vec![1]));
        assert_eq!(changed_rows(&[1, 2], &[1, 2]), Some(vec![]));
        assert_eq!(changed_rows(&[1, 2], &[1, 2, 3]), None);
    }

    #[test]
    fn names_are_trimmed_and_never_empty() {
        assert_eq!(valid_name("  Projets "), Some("Projets".to_owned()));
        assert_eq!(valid_name("   "), None);
    }
}
