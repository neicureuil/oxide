//! Share sheet (`ScreenSharePicker.swift`): pick a screen or a window from libwebrtc's desktop lists, then share it.
use crate::icons::{Icon, icon};
use crate::media_devices::permissions;
use crate::media_engine::{MediaMsg, ScreenChoice};
use crate::media_objc::{self, Obj, string};
use crate::media_webrtc::release_native_self_reference;
use crate::oxide::{Oxide, tr};
use crate::widgets::{self, ButtonKind};
use futures::channel::mpsc::UnboundedSender;
use gpui::{AnyElement, Context, FontWeight, Image, ImageFormat, Window, div, img, prelude::*, px};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use teams_core::{Action, CallAction};

/// A screen or window that can be shared.
pub struct Source {
    pub obj: Obj,
    pub id: String,
    pub name: String,
    pub window: bool,
    pub thumbnail: Option<Arc<Image>>,
}

/// Screens and windows that can be shared, with their thumbnails (libwebrtc's desktop lists).
pub struct DesktopSources {
    lists: RefCell<Vec<Obj>>,
    pub screens: RefCell<Vec<Source>>,
    pub windows: RefCell<Vec<Source>>,
    delegate: Retained<ListDelegate>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the sender is thread-safe.
    #[unsafe(super(NSObject))]
    #[name = "OxideDesktopListDelegate"]
    #[ivars = UnboundedSender<MediaMsg>]
    struct ListDelegate;

    impl ListDelegate {
        #[unsafe(method(didDesktopSourceAdded:))]
        fn added(&self, _source: *mut AnyObject) {}

        #[unsafe(method(didDesktopSourceRemoved:))]
        fn removed(&self, _source: *mut AnyObject) {}

        #[unsafe(method(didDesktopSourceNameChanged:))]
        fn renamed(&self, _source: *mut AnyObject) {}

        #[unsafe(method(didDesktopSourceThumbnailChanged:))]
        fn thumbnail(&self, _source: *mut AnyObject) {
            let _ = self.ivars().unbounded_send(MediaMsg::Desktop);
        }
    }
);

/// An `NSImage` as PNG, for GPUI.
fn png(image: &AnyObject) -> Option<Arc<Image>> {
    let tiff: Option<Obj> = unsafe { msg_send![image, TIFFRepresentation] };
    let rep_class = media_objc::class(c"NSBitmapImageRep")?;
    let rep: Option<Obj> = unsafe { msg_send![rep_class, imageRepWithData: &*tiff?] };
    let dictionary = media_objc::new(c"NSDictionary")?;
    // NSBitmapImageFileTypePNG
    let data: Option<Obj> =
        unsafe { msg_send![&*rep?, representationUsingType: 4usize, properties: &*dictionary] };
    let data = data?;
    let length: usize = unsafe { msg_send![&*data, length] };
    let bytes: *const std::ffi::c_void = unsafe { msg_send![&*data, bytes] };
    let bytes = bytes.cast::<u8>();
    if bytes.is_null() || length == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec();
    Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
}

impl DesktopSources {
    pub fn new(tx: UnboundedSender<MediaMsg>) -> Rc<Self> {
        let delegate = {
            let this = ListDelegate::alloc().set_ivars(tx);
            let delegate: Retained<ListDelegate> = unsafe { msg_send![super(this), init] };
            delegate
        };
        Rc::new(Self {
            lists: RefCell::default(),
            screens: RefCell::default(),
            windows: RefCell::default(),
            delegate,
        })
    }

    pub fn load(&self) {
        if self.lists.borrow().is_empty() {
            let make = |kind: isize| -> Option<Obj> {
                unsafe {
                    msg_send![media_objc::alloc(c"LKRTCDesktopMediaList")?, initWithType: kind, delegate: &*self.delegate]
                }
            };
            *self.lists.borrow_mut() = [make(0), make(1)].into_iter().flatten().collect();
        }
        for list in self.lists.borrow().iter() {
            let _: i32 =
                unsafe { msg_send![&**list, UpdateSourceList: true, updateAllThumbnails: true] };
        }
        self.refresh();
    }

    /// Frees the lists and their streams; sources keep name and thumbnail but can no longer start a capture.
    pub fn close(&self) {
        self.screens.borrow_mut().clear();
        self.windows.borrow_mut().clear();
        for list in self.lists.borrow_mut().drain(..) {
            release_native_self_reference(&list);
        }
    }

    pub fn refresh(&self) {
        let lists = self.lists.borrow();
        if lists.len() != 2 {
            return;
        }
        let sources = |list: &AnyObject, window: bool| -> Vec<Source> {
            let all: Option<Obj> = unsafe { msg_send![list, getSources] };
            media_objc::array(all.as_deref())
                .into_iter()
                .map(|obj| {
                    // libwebrtc can leave a source without name or thumbnail, though its header says otherwise.
                    let id: Option<Obj> = unsafe { msg_send![&*obj, sourceId] };
                    let name: Option<Obj> = unsafe { msg_send![&*obj, name] };
                    let thumbnail: Option<Obj> = unsafe { msg_send![&*obj, thumbnail] };
                    Source {
                        id: string(id.as_deref()),
                        name: string(name.as_deref()),
                        window,
                        thumbnail: thumbnail.as_deref().and_then(png),
                        obj,
                    }
                })
                .collect()
        };
        *self.screens.borrow_mut() = sources(&lists[0], false);
        // Not our own windows (the macOS app matches its name; ours are named after the chat or call).
        let own = own_window_ids();
        *self.windows.borrow_mut() = sources(&lists[1], true)
            .into_iter()
            .filter(|s| !s.name.trim().is_empty() && !own.contains(&s.id))
            .collect();
    }

    fn find(&self, id: &str) -> Option<Obj> {
        let screens = self.screens.borrow();
        let windows = self.windows.borrow();
        screens
            .iter()
            .chain(windows.iter())
            .find(|s| s.id == id)
            .map(|s| s.obj.clone())
    }
}

/// Window numbers of this process, as libwebrtc names window sources.
fn own_window_ids() -> Vec<String> {
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *mut AnyObject;
    }
    // kCGWindowListOptionAll
    let list = unsafe { CGWindowListCopyWindowInfo(0, 0) };
    let Some(list) = (unsafe { Retained::from_raw(list) }) else {
        return Vec::new();
    };
    let pid = std::process::id() as f64;
    media_objc::array(Some(&list))
        .iter()
        .filter(|w| media_objc::double(media_objc::get(w, "kCGWindowOwnerPID").as_deref()) == pid)
        .map(|w| {
            (media_objc::double(media_objc::get(w, "kCGWindowNumber").as_deref()) as u64)
                .to_string()
        })
        .collect()
}

/// The open share sheet.
pub struct ScreenPicker {
    pub sources: Rc<DesktopSources>,
    pub selected: Option<String>,
    pub allowed: bool,
}

impl ScreenPicker {
    pub fn refresh(&mut self) {
        self.sources.refresh();
    }
}

pub fn open(this: &mut Oxide) {
    let mut allowed = permissions::screen_capture(false);
    if !allowed {
        allowed = permissions::screen_capture(true);
    }
    let sources = DesktopSources::new(this.ui.call.media.tx.clone());
    let mut selected = None;
    if allowed && media_objc::available() {
        sources.load();
        selected = sources.screens.borrow().first().map(|s| s.id.clone());
    }
    this.ui.call.picker = Some(ScreenPicker {
        sources,
        selected,
        allowed,
    });
}

pub fn cancel(this: &mut Oxide) {
    if let Some(picker) = this.ui.call.picker.take() {
        picker.sources.close();
    }
}

/// Hands the choice, with its lists, to the engine and asks the core to share.
pub fn share(this: &mut Oxide) {
    let Some(picker) = this.ui.call.picker.take() else {
        return;
    };
    let Some(source) = picker
        .selected
        .as_deref()
        .and_then(|id| picker.sources.find(id))
    else {
        picker.sources.close();
        return;
    };
    this.ui.call.media.choose_screen(Some(ScreenChoice {
        source,
        lists: picker.sources,
    }));
    this.send(Action::Call {
        action: CallAction::SetScreenShare { on: true },
    });
}

fn tile(this: &Oxide, source: &Source, selected: bool, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let id = source.id.clone();
    div()
        .id(gpui::SharedString::from(format!(
            "share-{}-{}",
            source.window, source.id
        )))
        .w(px(180.))
        .p(px(6.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .rounded(px(10.))
        .cursor_pointer()
        .border(px(if selected { 2. } else { 1. }))
        .border_color(if selected { theme.accent } else { theme.border })
        .when(selected, |d| d.bg(theme.accent_soft))
        .on_click(cx.listener(move |this, _, _, cx| {
            if let Some(picker) = this.ui.call.picker.as_mut() {
                picker.selected = Some(id.clone());
            }
            crate::call_window::redraw(this, cx);
        }))
        .child(
            div()
                .h(px(96.))
                .w_full()
                .rounded(px(8.))
                .bg(gpui::black().opacity(0.85))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .map(|d| match &source.thumbnail {
                    Some(image) => d.child(img(image.clone()).max_w_full().max_h_full()),
                    None => d.child(icon(
                        if source.window {
                            Icon::AppWindow
                        } else {
                            Icon::Screen
                        },
                        px(28.),
                        gpui::white().opacity(0.7),
                    )),
                }),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_ellipsis()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(source.name.clone()),
        )
        .into_any_element()
}

fn section(
    this: &Oxide,
    title: String,
    items: &[Source],
    selected: Option<&str>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(this.theme.muted)
                .child(title),
        )
        .child(
            div().flex().flex_wrap().gap(px(12.)).children(
                items
                    .iter()
                    .map(|s| tile(this, s, selected == Some(s.id.as_str()), cx)),
            ),
        )
        .into_any_element()
}

/// The sheet over the call window.
pub fn view(this: &mut Oxide, _window: &mut Window, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let picker = this.ui.call.picker.as_ref()?;
    let theme = this.theme;
    let allowed = picker.allowed;
    let selected = picker.selected.clone();
    let sources = picker.sources.clone();
    let body = if allowed {
        let screens = sources.screens.borrow();
        let windows = sources.windows.borrow();
        div()
            .id("share-sources")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section(
                this,
                tr("ui.screen_share_picker.screens"),
                &screens,
                selected.as_deref(),
                cx,
            ))
            .when(!windows.is_empty(), |d| {
                d.child(section(
                    this,
                    tr("ui.screen_share_picker.windows"),
                    &windows,
                    selected.as_deref(),
                    cx,
                ))
            })
            .into_any_element()
    } else {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .child(widgets::muted(tr("ui.screen_share_picker.oxide_must_be_allowed_in_system"), 13., &theme))
            .child(
                widgets::button("share-settings", tr("ui.screen_share_picker.open_settings"), ButtonKind::Outline, &theme)
                    .on_click(cx.listener(|_, _, _, cx| {
                        permissions::screen_capture(true);
                        cx.open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");
                    })),
            )
            .into_any_element()
    };
    let can_share = allowed && selected.is_some();
    let content = div()
        .h(px(520.))
        .p(px(22.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(widgets::title(
            tr("ui.screen_share_picker.share_screen"),
            18.,
        ))
        .child(body)
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    widgets::button(
                        "share-cancel",
                        tr("ui.screen_share_picker.cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cancel(this);
                        crate::call_window::redraw(this, cx);
                    })),
                )
                .child(
                    widgets::disabled(
                        widgets::button(
                            "share-go",
                            tr("ui.screen_share_picker.share"),
                            ButtonKind::Primary,
                            &theme,
                        ),
                        !can_share,
                    )
                    .when(can_share, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| {
                            share(this);
                            crate::call_window::redraw(this, cx);
                        }))
                    }),
                ),
        );
    let close = cx.listener(|this, _: &(), _, cx| {
        cancel(this);
        crate::call_window::redraw(this, cx);
    });
    Some(widgets::modal(
        "share-picker",
        px(640.),
        content,
        &theme,
        move |window, cx| close(&(), window, cx),
    ))
}
