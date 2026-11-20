//! Pieces of the file screens other screens reuse (`FilesScreens.swift`): extension badges, file cards,
//! the draft's attachment chips, and the search box and menus they share.
use crate::icons::{Icon, icon};
use crate::input::TextInput;
use crate::oxide::{Oxide, tr, trf};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, ClickEvent, Context, Div, Entity, FontWeight, Hsla, ObjectFit, PathPromptOptions,
    Pixels, Point, SharedString, Stateful, Window, anchored, deferred, div, img, prelude::*, px,
    rgb,
};
use teams_core::{Action, EntryView, FileAction, FileView, ImageStatus, PendingFileView};

/// Popup key of the file menus: one is open at a time, like any other popup.
pub const MENU: &str = "files:menu";

pub fn send(this: &Oxide, action: FileAction) {
    this.send(Action::Files { action });
}

/// OneDrive's folder colours, indexed by `_ColorHex`; 0 keeps the app's own look.
pub const FOLDER_COLORS: u8 = 16;
const FOLDER_HEX: [u32; 16] = [
    0x000000, 0xA4262C, 0xCA5010, 0x0B6A0B, 0x038387, 0x004E8C, 0x5C2E91, 0xBF0077, 0x7A7574,
    0xF1707B, 0xFFAA44, 0x6BB700, 0x00B7C3, 0x73B3EB, 0x8764B8, 0xEE5FB7,
];
pub fn folder_color(index: u8, theme: &Theme) -> Hsla {
    if index == 0 || index >= FOLDER_COLORS {
        theme.accent_text
    } else {
        rgb(FOLDER_HEX[index as usize]).into()
    }
}
pub fn folder_color_name(index: u8) -> String {
    tr(&format!("ui.files_screens.folder_color_{index}"))
}

/// `Theme.pdfBackground` / `Theme.pdfText`, which only the badges use.
fn pdf_colors(theme: &Theme) -> (Hsla, Hsla) {
    if theme.dark {
        (rgb(0x4A2029).into(), rgb(0xFFB0BD).into())
    } else {
        (rgb(0xFCE8EC).into(), rgb(0xA3243B).into())
    }
}

/// Extension badge, PDF in its own colours like Teams.
pub fn file_badge(
    ext: &str,
    folder: bool,
    folder_color_index: u8,
    side: f32,
    theme: &Theme,
) -> Div {
    let tinted = folder && folder_color_index != 0;
    let (pdf_bg, pdf_fg) = pdf_colors(theme);
    let (fg, bg) = if ext == "PDF" {
        (pdf_fg, pdf_bg)
    } else if tinted {
        let c = folder_color(folder_color_index, theme);
        (c, c.opacity(0.16))
    } else {
        (theme.accent_text, theme.accent_soft)
    };
    let content: AnyElement = if folder {
        icon(Icon::FolderFill, px(side * 0.5), fg).into_any_element()
    } else if ext == "URL" {
        icon(Icon::Link, px(side * 0.46), fg).into_any_element()
    } else {
        div()
            .text_size(px(side * 0.28))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(fg)
            .child(ext.to_owned())
            .into_any_element()
    };
    div()
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(side * 0.22))
        .bg(bg)
        .child(content)
}

/// A thin bar filled to `value` percent, as `ProgressView(value:total:)`.
pub fn progress_bar(value: u8, width: f32, theme: &Theme) -> Div {
    let fraction = f32::from(value.min(100)) / 100.;
    div()
        .w(px(width))
        .h(px(4.))
        .flex_none()
        .rounded_full()
        .bg(theme.border)
        .child(
            div()
                .h_full()
                .w(px(width * fraction))
                .rounded_full()
                .bg(theme.accent),
        )
}

/// An outline button as `OutlineButtonStyle`, with an optional leading icon.
pub fn outline_button(
    id: impl Into<gpui::ElementId>,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
    enabled: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let hover = theme.input;
    let fg = if enabled {
        theme.text_soft
    } else {
        theme.subtle
    };
    div()
        .id(id)
        .h(px(32.))
        .px_3()
        .flex()
        .flex_none()
        .items_center()
        .gap_1p5()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.surface)
        .text_size(px(13.))
        .text_color(fg)
        .when(enabled, move |d| {
            d.cursor_pointer().hover(move |s| s.bg(hover))
        })
        .when_some(glyph, |d, g| d.child(icon(g, px(14.), fg)))
        .child(label.into())
}

/// A file attached to a message, or listed in a "Partagé" tab.
pub fn file_card(this: &mut Oxide, file: &FileView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let key = file.key.clone();
    let thumbnail = file
        .thumbnail
        .as_ref()
        .filter(|t| matches!(t.status, ImageStatus::Ready { .. }))
        .and_then(|t| {
            this.images
                .images
                .get(&t.src)
                .map(|image| (t.src.clone(), image.clone()))
        });
    let actions: AnyElement = if let Some(progress) = file.progress {
        let cancel = key.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id(SharedString::from(format!("progress-{key}")))
                    .child(progress_bar(progress, 60., &theme))
                    .tooltip(widgets::tooltip(
                        trf(
                            "ui.files_screens.downloading",
                            &[("progress", &progress.to_string())],
                        ),
                        theme,
                    )),
            )
            .child(
                widgets::icon_button_sized(
                    SharedString::from(format!("cancel-{key}")),
                    Icon::Close,
                    28.,
                    13.,
                    false,
                    &theme,
                )
                .tooltip(widgets::tooltip(
                    tr("ui.files_screens.cancel_download"),
                    theme,
                ))
                .on_click(cx.listener(move |this, _, _, _| {
                    send(
                        this,
                        FileAction::Cancel {
                            key: cancel.clone(),
                        },
                    )
                })),
            )
            .into_any_element()
    } else {
        let open = key.clone();
        let can_open = file.url.is_some();
        let target = file.clone();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                outline_button(
                    SharedString::from(format!("open-{key}")),
                    None,
                    tr("ui.files_screens.open"),
                    can_open,
                    &theme,
                )
                .when(can_open, |d| {
                    d.on_click(cx.listener(move |this, _, _, _| {
                        send(this, FileAction::Open { key: open.clone() })
                    }))
                }),
            )
            .child(
                widgets::disabled(
                    widgets::icon_button_sized(
                        SharedString::from(format!("more-{key}")),
                        Icon::More,
                        28.,
                        16.,
                        false,
                        &theme,
                    )
                    .tooltip(widgets::tooltip(tr("ui.files_screens.more_actions"), theme)),
                    file.deleted,
                )
                .when(!file.deleted, |d| {
                    d.on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        open_menu(this, MenuTarget::File(target.clone()), event.position(), cx)
                    }))
                }),
            )
            .into_any_element()
    };
    let mut meta = div()
        .flex()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(theme.muted)
        .child(div().truncate().child(file.description.clone()));
    if !file.date.is_empty() {
        meta = meta.child(div().flex_none().child(format!("· {}", file.date)));
    }
    let context = file.clone();
    div()
        .id(SharedString::from(format!("card-{key}")))
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface)
        .when_some(thumbnail, |d, (src, image)| {
            let size = image.size(0);
            let (w, h) = (size.width.0.max(1) as f32, size.height.0.max(1) as f32);
            let width = (150. * w / h).min(620.);
            d.child(
                div().px_2().pt_2().child(
                    img(image)
                        .id(SharedString::from(format!("thumb-{key}")))
                        .w(px(width))
                        .max_w_full()
                        .h(px(150.))
                        .object_fit(ObjectFit::Cover)
                        .rounded(px(8.))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.zoomed = Some(src.clone());
                            cx.notify();
                        })),
                ),
            )
        })
        .child(
            div()
                .px_3()
                .py(px(10.))
                .flex()
                .items_center()
                .gap_3()
                .child(file_badge(&file.ext, false, 0, 36., &theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(file.name.clone()),
                        )
                        .child(meta),
                )
                .child(actions),
        )
        .on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                if !context.deleted {
                    open_menu(this, MenuTarget::File(context.clone()), event.position, cx);
                }
            }),
        )
        .into_any_element()
}

/// Files of the draft, above the editor: progress, failure, removal.
pub fn attachment_chips(
    this: &mut Oxide,
    attachments: &[PendingFileView],
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let chips = attachments.iter().map(|file| {
        let status: AnyElement = if let Some(error) = &file.error {
            div()
                .id(SharedString::from(format!("chip-error-{}", file.id)))
                .truncate()
                .text_size(px(11.))
                .text_color(theme.danger)
                .child(error.clone())
                .tooltip(widgets::tooltip(error.clone(), theme))
                .into_any_element()
        } else if !file.ready {
            progress_bar(file.progress, 120., &theme).into_any_element()
        } else {
            widgets::muted(tr("ui.files_screens.ready"), 11., &theme).into_any_element()
        };
        let id = file.id.clone();
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .p(px(6.))
            .rounded(px(8.))
            .bg(theme.input)
            .child(file_badge(&file.ext, false, 0, 26., &theme))
            .child(
                div()
                    .max_w(px(180.))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(file.name.clone()),
                    )
                    .child(status),
            )
            .child(
                widgets::icon_button_sized(
                    SharedString::from(format!("chip-remove-{id}")),
                    Icon::Close,
                    22.,
                    10.,
                    false,
                    &theme,
                )
                .tooltip(widgets::tooltip(
                    trf("ui.files_screens.remove", &[("name", &file.name)]),
                    theme,
                ))
                .on_click(cx.listener(move |this, _, _, _| {
                    send(this, FileAction::RemoveAttachment { id: id.clone() })
                })),
            )
    });
    div()
        .id("attachment-chips")
        .w_full()
        .overflow_x_scroll()
        .child(div().flex().gap_2().children(chips))
        .into_any_element()
}

/// Search box of the file screens, as `FileSearchField`; the module keeps the entity and its events.
pub fn search_box(
    input: &Entity<TextInput>,
    clear: impl Fn(&mut Oxide, &mut Window, &mut Context<Oxide>) + 'static,
    this: &Oxide,
    window: &Window,
    cx: &mut Context<Oxide>,
) -> Div {
    let theme = this.theme;
    let state = input.read(cx);
    let focused = state.is_focused(window);
    let empty = state.is_empty();
    div()
        .h(px(32.))
        .w_full()
        .max_w(px(420.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(8.))
        .bg(theme.input)
        .border_1()
        .border_color(if focused {
            theme.accent
        } else {
            gpui::transparent_black()
        })
        .text_size(px(13.))
        .line_height(px(18.))
        .text_color(theme.text)
        .child(icon(Icon::Search, px(14.), theme.subtle))
        .child(div().flex_1().min_w_0().child(crate::input::field(
            input,
            widgets::input_colors(&theme),
            cx,
        )))
        .when(!empty, |d| {
            d.child(
                widgets::icon_button_sized("search-clear", Icon::Close, 22., 12., false, &theme)
                    .tooltip(widgets::tooltip(tr("ui.files_screens.clear_search"), theme))
                    .on_click(cx.listener(move |this, _, window, cx| clear(this, window, cx))),
            )
        })
}

/// What a file menu acts on.
#[derive(Clone)]
pub enum MenuTarget {
    Entry(EntryView),
    File(FileView),
}
pub struct Menu {
    pub target: MenuTarget,
    pub at: Point<Pixels>,
    /// The folder colour list replaces the menu.
    pub colors: bool,
}

pub fn open_menu(this: &mut Oxide, target: MenuTarget, at: Point<Pixels>, cx: &mut Context<Oxide>) {
    this.ui.files.menu = Some(Menu {
        target,
        at,
        colors: false,
    });
    this.popup = Some(MENU.into());
    cx.notify();
}

/// The open file menu, at the click that opened it.
pub fn menu_overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    if !this.popup_open(MENU) {
        return None;
    }
    let menu = this.ui.files.menu.as_ref()?;
    let (target, colors, at) = (menu.target.clone(), menu.colors, menu.at);
    let theme = this.theme;
    let items: Vec<AnyElement> = match (&target, colors) {
        (MenuTarget::Entry(entry), true) => color_items(entry, &theme, cx),
        (MenuTarget::Entry(entry), false) => crate::files::entry_menu(this, entry, window, cx),
        (MenuTarget::File(file), _) => file_menu_items(file, &theme, cx),
    };
    let close = cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.close_popup(cx));
    Some(
        deferred(
            anchored()
                .position(at)
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
                        .text_color(theme.text)
                        .on_mouse_down_out(close)
                        .children(items),
                ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

/// A menu line that runs `action` and closes the menu; disabled lines are drawn faded.
pub fn item(
    id: impl Into<SharedString>,
    glyph: Option<Icon>,
    label: impl Into<SharedString>,
    enabled: bool,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    action: impl Fn(&mut Oxide, &mut Window, &mut Context<Oxide>) + 'static,
) -> AnyElement {
    let id: SharedString = id.into();
    widgets::disabled(widgets::menu_item(id, glyph, label, theme), !enabled)
        .when(enabled, |d| {
            d.on_click(cx.listener(move |this, _, window, cx| {
                this.close_popup(cx);
                action(this, window, cx);
            }))
        })
        .into_any_element()
}

pub fn menu_divider(theme: &Theme) -> AnyElement {
    div().my_1().h(px(1.)).bg(theme.border).into_any_element()
}

fn file_menu_items(file: &FileView, theme: &Theme, cx: &mut Context<Oxide>) -> Vec<AnyElement> {
    let has_url = file.url.is_some();
    let mut out = Vec::new();
    let key = file.key.clone();
    out.push(item(
        "m-open",
        Some(Icon::External),
        tr("ui.files_screens.open_in_browser"),
        has_url,
        theme,
        cx,
        {
            let key = key.clone();
            move |this, _, _| send(this, FileAction::Open { key: key.clone() })
        },
    ));
    if file.can_download {
        out.push(item(
            "m-preview",
            Some(Icon::Eye),
            tr("ui.files_screens.preview"),
            true,
            theme,
            cx,
            {
                let key = key.clone();
                move |this, _, _| send(this, FileAction::Preview { key: key.clone() })
            },
        ));
        let name = file.name.clone();
        out.push(item(
            "m-download",
            Some(Icon::Download),
            tr("ui.files_screens.download"),
            true,
            theme,
            cx,
            {
                let key = key.clone();
                move |_, _, cx| download(key.clone(), name.clone(), cx)
            },
        ));
    }
    out.push(item(
        "m-copy",
        Some(Icon::Copy),
        tr("ui.files_screens.copy_link"),
        has_url,
        theme,
        cx,
        move |this, _, _| send(this, FileAction::CopyLink { key: key.clone() }),
    ));
    out
}

/// The folder colour list, the current one checked.
fn color_items(entry: &EntryView, theme: &Theme, cx: &mut Context<Oxide>) -> Vec<AnyElement> {
    let hover = theme.input;
    (0..FOLDER_COLORS)
        .map(|index| {
            let key = entry.key.clone();
            div()
                .id(SharedString::from(format!("color-{index}")))
                .min_w(px(200.))
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
                .child(div().w(px(14.)).child(if entry.color == index {
                    icon(Icon::Check, px(13.), theme.accent_text).into_any_element()
                } else {
                    div().into_any_element()
                }))
                .child(
                    div()
                        .size(px(12.))
                        .rounded_full()
                        .bg(folder_color(index, theme)),
                )
                .child(folder_color_name(index))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_popup(cx);
                    send(
                        this,
                        FileAction::SetColor {
                            key: key.clone(),
                            color: index,
                        },
                    );
                }))
                .into_any_element()
        })
        .collect()
}

/// Asks where to save `name`, in Downloads first, then downloads it there.
pub fn download(key: String, name: String, cx: &mut Context<Oxide>) {
    let dir = std::env::var_os("HOME")
        .map(|home| std::path::Path::new(&home).join("Downloads"))
        .unwrap_or_else(std::env::temp_dir);
    let chosen = cx.prompt_for_new_path(&dir, Some(&name));
    cx.spawn(async move |this, cx| {
        if let Ok(Ok(Some(path))) = chosen.await {
            let path = path.to_string_lossy().into_owned();
            let _ = this.update(cx, |this, _| send(this, FileAction::Download { key, path }));
        }
    })
    .detach();
}

/// Lets the user pick files, then uploads them into the folder shown.
pub fn choose_uploads(cx: &mut Context<Oxide>) {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some(tr("ui.files_screens.upload").into()),
    });
    cx.spawn(async move |this, cx| {
        if let Ok(Ok(Some(paths))) = chosen.await {
            let _ = this.update(cx, |this, _| upload(this, &paths));
        }
    })
    .detach();
}

pub fn upload(this: &Oxide, paths: &[std::path::PathBuf]) {
    let paths: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if !paths.is_empty() {
        send(this, FileAction::UploadHere { paths });
    }
}

/// A small spinner, as `ProgressView().controlSize(.small)`.
pub fn small_spinner(id: &'static str, theme: &Theme) -> AnyElement {
    widgets::spinner(id, 14., theme.muted)
}

/// A dialog button, faded and inert when disabled.
pub fn dialog_button(
    id: &'static str,
    label: String,
    kind: ButtonKind,
    enabled: bool,
    theme: &Theme,
) -> Stateful<Div> {
    widgets::disabled(widgets::button(id, label, kind, theme), !enabled)
}
