//! Microsoft sign-in in an embedded web view (`LoginWindow.swift`), through wry: WKWebView on macOS, WebView2
//! on Windows, WebKitGTK on Linux. The page is stopped on the Teams return URL, which goes to the core for its
//! PKCE exchange. Its own cookie store allows later `prompt=none` silent sign-ins in a hidden view.
use crate::oxide::{Oxide, tr};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{Context, Render, Task, Window, div, prelude::*};
#[cfg(not(windows))]
use gpui::{AppContext, Bounds, WindowBounds, WindowHandle, WindowOptions, px, size};
use std::time::Duration;
use teams_core::Action;
use wry::{Rect, WebContext, WebView, WebViewBuilder, dpi};

/// A silent sign-in is a few redirects; past this, Microsoft is waiting for a person.
const SILENT_LIMIT: Duration = Duration::from_secs(30);
/// The Swift app's store identifier: only the Microsoft sign-in uses it.
#[cfg(target_os = "macos")]
const STORE: [u8; 16] = [
    0x6D, 0x2A, 0x3C, 0x1E, 0x4B, 0x7F, 0x4E, 0x0A, 0x9C, 0x5D, 0x2F, 0x1B, 0x8A, 0x7E, 0x6D, 0x40,
];

/// What the web views report, handed to the UI thread.
enum Event {
    /// The Teams return URL was reached, by the window or the silent view.
    Returned { url: String, silent: bool },
    /// The silent view was sent elsewhere than Microsoft's pages, or took too long.
    SilentFailed,
    /// The person closed the sign-in window.
    Closed,
}

/// The open sign-in window: a GPUI window on macOS and Linux, a Win32 one on Windows (`login_native`).
#[cfg(not(windows))]
type Handle = WindowHandle<LoginView>;
#[cfg(windows)]
type Handle = crate::login_native::LoginWindow;

pub struct Ui {
    events: UnboundedSender<Event>,
    window: Option<Handle>,
    /// The return URL was reached or the core closed the window: closing it is not a cancellation.
    returned: bool,
    silent: Option<(WebView, Task<()>)>,
}

impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let (events, mut received) = unbounded();
        cx.spawn(async move |this, cx| {
            while let Some(event) = received.next().await {
                if this.update(cx, |this, cx| handle(this, event, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
        Self {
            events,
            window: None,
            returned: false,
            silent: None,
        }
    }
}

fn handle(this: &mut Oxide, event: Event, cx: &mut Context<Oxide>) {
    match event {
        Event::Returned { url, silent } => {
            if std::env::var("OXIDE_SCRIPT").is_ok() {
                eprintln!("OXIDE_SCRIPT: sign-in returned (silent: {silent})");
            }
            if silent {
                end_silent(this);
            } else {
                this.ui.login.returned = true;
                close(this, cx);
            }
            this.send(Action::LoginReturned { url });
        }
        Event::SilentFailed => {
            if this.ui.login.silent.is_some() {
                end_silent(this);
                this.send(Action::SilentLoginFailed);
            }
        }
        Event::Closed => {
            this.ui.login.window = None;
            if !std::mem::replace(&mut this.ui.login.returned, false) {
                this.send(Action::LoginCancelled);
            }
        }
    }
}

/// Decides each navigation: the return URL is kept from the page and reported; only https pages load.
fn navigation(events: UnboundedSender<Event>, silent: bool) -> impl Fn(String) -> bool + 'static {
    move |url: String| {
        if teams_core::is_login_callback(url.clone()) {
            let _ = events.unbounded_send(Event::Returned { url, silent });
            return false;
        }
        let allowed = teams_core::is_login_allowed(url);
        if silent && !allowed {
            let _ = events.unbounded_send(Event::SilentFailed);
        }
        allowed
    }
}

/// The sign-in's own browsing data. On Windows a folder of the app's (WebView2 would otherwise make one
/// next to the executable); macOS uses the store identifier instead.
fn context() -> WebContext {
    #[cfg(windows)]
    let folder = std::env::var_os("LOCALAPPDATA").map(|local| {
        std::path::PathBuf::from(local)
            .join("oxide-gpui")
            .join("WebView2")
    });
    #[cfg(not(windows))]
    let folder = None;
    WebContext::new(folder)
}

/// A builder on the sign-in's own cookie store.
fn builder<'a>(
    context: &'a mut WebContext,
    url: &str,
    events: UnboundedSender<Event>,
    silent: bool,
) -> WebViewBuilder<'a> {
    let builder = WebViewBuilder::new_with_web_context(context)
        .with_url(url)
        .with_navigation_handler(navigation(events, silent));
    #[cfg(target_os = "macos")]
    let builder = {
        use wry::WebViewBuilderExtDarwin;
        builder.with_data_store_identifier(STORE)
    };
    builder
}

/// `Effect::OpenLogin`: the sign-in window at Microsoft's authorization URL.
pub fn open(this: &mut Oxide, url: String, cx: &mut Context<Oxide>) {
    close(this, cx);
    this.ui.login.returned = false;
    let events = this.ui.login.events.clone();
    let title = tr("ui.login_window.microsoft_sign_in_oxide");
    #[cfg(windows)]
    {
        let closed = events.clone();
        let opened = crate::login_native::LoginWindow::open(
            &title,
            540.,
            740.,
            |parent| {
                builder(&mut context(), &url, events, false)
                    .with_focused(true)
                    .build(parent)
            },
            move || {
                let _ = closed.unbounded_send(Event::Closed);
            },
        );
        match opened {
            Ok(window) => this.ui.login.window = Some(window),
            Err(error) => {
                eprintln!("oxide: the sign-in window could not be opened: {error}");
                this.send(Action::LoginCancelled);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let bounds = Bounds::centered(None, size(px(540.), px(740.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| cx.new(|_| LoginView::new(window, &url, events)),
        );
        match opened {
            Ok(handle) => this.ui.login.window = Some(handle),
            Err(_) => this.send(Action::LoginCancelled),
        }
    }
}

/// `Effect::CloseLogin`: closes without reporting a cancellation (the core asked for it).
pub fn close(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if let Some(handle) = this.ui.login.window.take() {
        this.ui.login.returned = true;
        // Dropping the Win32 window closes it.
        #[cfg(not(windows))]
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        #[cfg(windows)]
        let _ = (handle, &cx);
    }
}

/// `Effect::SilentLogin`: loads a `prompt=none` URL in a hidden view; the return goes to the core as
/// `LoginReturned`, anything else as `SilentLoginFailed`.
pub fn silent(this: &mut Oxide, url: String, window: &mut Window, cx: &mut Context<Oxide>) {
    end_silent(this);
    let events = this.ui.login.events.clone();
    let built = builder(&mut context(), &url, events.clone(), true)
        .with_visible(false)
        .with_bounds(Rect {
            position: dpi::LogicalPosition::new(0., 0.).into(),
            size: dpi::LogicalSize::new(1., 1.).into(),
        })
        .build_as_child(&*window);
    let Ok(view) = built else {
        this.send(Action::SilentLoginFailed);
        return;
    };
    let timeout = cx.spawn(async move |_, cx| {
        cx.background_executor().timer(SILENT_LIMIT).await;
        let _ = events.unbounded_send(Event::SilentFailed);
    });
    this.ui.login.silent = Some((view, timeout));
}

/// `Effect::ForgetLogin` (signing out): no later silent sign-in may reuse this account.
pub fn forget(this: &mut Oxide, window: &mut Window) {
    end_silent(this);
    // Clearing goes through a view on the sign-in's store; a short-lived hidden one does it.
    let cleared = builder(
        &mut context(),
        "about:blank",
        this.ui.login.events.clone(),
        false,
    )
    .with_visible(false)
    .build_as_child(&*window)
    .and_then(|view| view.clear_all_browsing_data());
    if let Err(error) = cleared {
        eprintln!("oxide: could not clear the sign-in cookies: {error}");
    }
}

fn end_silent(this: &mut Oxide) {
    // Dropping the view stops its loading; dropping the task stops the timeout.
    this.ui.login.silent = None;
}

/// The sign-in window's content: the web view fills it and follows its size.
pub struct LoginView {
    web: Option<WebView>,
    events: UnboundedSender<Event>,
    size: Option<gpui::Size<gpui::Pixels>>,
}

impl LoginView {
    fn new(window: &mut Window, url: &str, events: UnboundedSender<Event>) -> Self {
        let web = builder(&mut context(), url, events.clone(), false)
            .with_focused(true)
            .build_as_child(&*window)
            .map_err(|error| eprintln!("oxide: the sign-in view could not be created: {error}"))
            .ok();
        Self {
            web,
            events,
            size: None,
        }
    }
}

impl Render for LoginView {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        if self.size != Some(viewport)
            && let Some(web) = &self.web
        {
            self.size = Some(viewport);
            let _ = web.set_bounds(Rect {
                position: dpi::LogicalPosition::new(0., 0.).into(),
                size: dpi::LogicalSize::new(
                    f64::from(f32::from(viewport.width)),
                    f64::from(f32::from(viewport.height)),
                )
                .into(),
            });
        }
        div().size_full().bg(gpui::white())
    }
}

impl Drop for LoginView {
    fn drop(&mut self) {
        // The view goes before its window; then the store learns whether this was a cancellation.
        self.web = None;
        let _ = self.events.unbounded_send(Event::Closed);
    }
}

/// `OXIDE_SCRIPT` steps: `login:open:<url>` shows the sign-in window there; `login:return` sends it to the
/// Teams return URL, which must be stopped, reported and close the window.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    if let Some(url) = step.strip_prefix("login:open:") {
        open(this, url.to_owned(), cx);
        return true;
    }
    if step == "login:return" {
        const RETURN: &str = "https://teams.microsoft.com/v2#code=oxide-test&state=x";
        let Some(handle) = &this.ui.login.window else {
            return false;
        };
        #[cfg(windows)]
        handle.with_web(|web| {
            let _ = web.load_url(RETURN);
        });
        #[cfg(not(windows))]
        let _ = handle.update(cx, |view, _, _| {
            if let Some(web) = &view.web {
                let _ = web.load_url(RETURN);
            }
        });
        return true;
    }
    false
}
