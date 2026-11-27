//! The window's root view and store: keeps every part the core sent (as `Store.swift`) and lays out the window.
use crate::avatars::{self, Avatars};
use crate::feed::FeedList;
use crate::prefs::{Appearance, LanguagePref, Prefs};
use crate::theme::Theme;
use crate::{SignOut, screens, shell};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    AnyElement, Context, FontWeight, ListAlignment, RenderImage, SharedString, Window, div, img,
    prelude::*, px,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use teams_core::{
    Action, AvatarShape, BrowserView, CallPart, CallsView, ConversationView, CoreListener, Effect,
    MembersView, MessageView, NewChatView, OxideCore, PersonView, PostView, PresenceView,
    SearchView, Shell, Sidebar, Update,
};

pub const FONT: &str = "Figtree";
pub const MONO: &str = "Menlo";

thread_local! {
    static TABLE: RefCell<HashMap<String, String>> = RefCell::new(teams_core::translations());
}
/// Text of the core's catalog, in the app's language.
pub fn tr(key: &str) -> String {
    TABLE.with(|t| t.borrow().get(key).cloned().unwrap_or_else(|| key.to_owned()))
}
/// Same, with `{name}` placeholders filled.
pub fn trf(key: &str, args: &[(&str, &str)]) -> String {
    let mut s = tr(key);
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}
/// `key.one` or `key.other` for `n`, with `{n}` filled; French takes the singular for 0 too (Swift's `Lp`).
pub fn trp(key: &str, n: i64, args: &[(&str, &str)]) -> String {
    let one = if english() { n == 1 } else { n <= 1 };
    let n_text = n.to_string();
    let mut all: Vec<(&str, &str)> = args.to_vec();
    all.push(("n", &n_text));
    trf(&format!("{key}.{}", if one { "one" } else { "other" }), &all)
}
fn reload_translations() {
    TABLE.with(|t| *t.borrow_mut() = teams_core::translations());
}
pub fn english() -> bool {
    tr("ui.sign_in_view.welcome") == "Welcome"
}
/// Texts only this app shows, in the app's language.
pub fn text(fr: &'static str, en: &'static str) -> &'static str {
    if english() { en } else { fr }
}

/// The system's first preferred language, as Swift's `LanguagePreference.system` resolves it.
fn system_language() -> teams_core::Language {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let first = out
        .split(['(', ',', ')', '"', '\n', ' '])
        .find(|s| !s.trim().is_empty())
        .unwrap_or("fr");
    if first.starts_with("en") {
        teams_core::Language::English
    } else {
        teams_core::Language::French
    }
}

/// Applies the language before the core boots or when Settings changes it.
pub fn apply_language(prefs: &Prefs) -> teams_core::Language {
    let language = prefs.core_language().unwrap_or_else(system_language);
    teams_core::set_language(language);
    reload_translations();
    language
}

enum CoreEvent {
    Update(Box<Update>),
    Effect(Effect),
}
/// Core threads hand their updates to the UI thread, in order.
struct Listener(UnboundedSender<CoreEvent>);
impl CoreListener for Listener {
    fn update(&self, update: Update) {
        let _ = self.0.unbounded_send(CoreEvent::Update(Box::new(update)));
    }
    fn effect(&self, effect: Effect) {
        let _ = self.0.unbounded_send(CoreEvent::Effect(effect));
    }
}

/// Decoded message previews, by source: dropped as soon as the core stops counting them.
#[derive(Default)]
pub struct Images {
    pub images: HashMap<String, Arc<RenderImage>>,
    kept: HashSet<String>,
    decoding: HashSet<String>,
}

pub struct Oxide {
    pub core: Arc<OxideCore>,
    pub demo: bool,
    pub prefs: Prefs,
    pub theme: Theme,
    system_dark: bool,
    pub active: bool,

    // Parts of the core, as last sent.
    pub shell: Option<Shell>,
    pub sidebar: Option<Sidebar>,
    pub composer: Option<teams_core::ComposerView>,
    pub mention_people: Vec<PersonView>,
    pub conversation: Option<ConversationView>,
    pub typing: Option<String>,
    pub members: Option<MembersView>,
    pub files: Option<teams_core::FilesView>,
    pub browser: Option<BrowserView>,
    pub new_chat: Option<NewChatView>,
    pub notifications: Option<teams_core::NotificationsView>,
    pub me: Option<teams_core::MeView>,
    pub chat_admin: Option<teams_core::ChatAdminView>,
    pub teams_admin: Option<teams_core::TeamsAdminView>,
    pub gifs: Option<teams_core::GifsView>,
    pub extensions: Option<teams_core::ExtensionsView>,
    pub search: Option<SearchView>,
    pub calendar: Option<teams_core::CalendarView>,
    pub calls: Option<CallsView>,
    pub call: Option<CallPart>,
    /// The emoticon catalog, fetched again on `EmojiCatalogChanged`.
    pub emoji: Vec<teams_core::EmojiCategory>,
    pub stickers: Vec<teams_core::StickerCategory>,

    /// Chat messages, channel posts and the open thread's replies.
    pub messages: FeedList<MessageView>,
    pub posts: FeedList<PostView>,
    pub replies: FeedList<MessageView>,

    pub avatars: Avatars,
    pub images: Images,
    /// The open menu or popover, by key: one at a time.
    pub popup: Option<SharedString>,
    /// The popover last closed and when, so the click that closed it does not reopen it.
    popup_closed: Option<(SharedString, std::time::Instant)>,
    /// A message picture shown full size over the window.
    pub zoomed: Option<String>,
    /// Network reachable (the offline banner otherwise).
    pub online: bool,

    pub ui: screens::Ui,
    /// The main window, for effects that need one (the hidden sign-in view).
    pub main_window: gpui::AnyWindowHandle,
}

impl Oxide {
    pub fn new(demo: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = Prefs::load();
        apply_language(&prefs);
        let (sender, mut events) = unbounded();
        let core = OxideCore::new(Arc::new(Listener(sender)), demo);
        core.send(Action::SetRefreshMode {
            mode: prefs.core_refresh(),
        });
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this.update(cx, |this, cx| this.apply(event, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();

        cx.observe_window_activation(window, |this, window, cx| {
            this.active = window.is_window_active();
            // In the background, what was freed goes back to the system at once.
            #[cfg(target_os = "macos")]
            if !this.active {
                cx.background_spawn(async { crate::platform_memory::relieve() }).detach();
            }
            // Reduce motion may have changed in System Settings meanwhile.
            crate::motion::refresh_reduced();
            this.core.set_active(this.active);
            cx.notify();
        })
        .detach();
        cx.observe_window_appearance(window, |this, window, cx| {
            this.system_dark = is_dark(window.appearance());
            this.theme = this.compute_theme();
            cx.notify();
        })
        .detach();
        // Notifications, network watcher, quitting mid-call, voicemail events.
        crate::platform::start(window, cx);
        // In front, freed memory is given back every 30 s (conversation switches, pictures).
        #[cfg(target_os = "macos")]
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor().timer(crate::platform_memory::EVERY).await;
                // Unconditionally: malloc's statistics leave out its cache of freed large blocks (tens of MB
                // after a conversation with pictures), and a pass every 30 s costs next to nothing.
                cx.background_executor()
                    .spawn(async { crate::platform_memory::relieve() })
                    .await;
            }
        })
        .detach();
        let signed_out = core.clone();
        // From the app menu, whatever has focus.
        gpui::App::on_action(cx, move |_: &SignOut, _| signed_out.send(Action::SignOut));

        // Views far from the screen are built again by the core when needed (see `FeedList`).
        let fetch_messages = {
            let core = core.clone();
            move |ids: &[String]| core.message_views(ids)
        };
        let fetch_posts = {
            let core = core.clone();
            move |ids: &[String]| core.post_views(ids)
        };
        let fetch_replies = {
            let core = core.clone();
            move |ids: &[String]| core.message_views(ids)
        };
        let messages = FeedList::new(&core, Action::MoreFeed, ListAlignment::Bottom, fetch_messages);
        let posts = FeedList::new(&core, Action::MoreFeed, ListAlignment::Bottom, fetch_posts);
        let replies = FeedList::new(&core, Action::MoreReplies, ListAlignment::Bottom, fetch_replies);
        let system_dark = is_dark(window.appearance());
        let mut this = Self {
            core,
            demo,
            theme: Theme::for_dark(system_dark),
            system_dark,
            prefs,
            active: true,
            shell: None,
            sidebar: None,
            composer: None,
            mention_people: Vec::new(),
            conversation: None,
            typing: None,
            members: None,
            files: None,
            browser: None,
            new_chat: None,
            notifications: None,
            me: None,
            chat_admin: None,
            teams_admin: None,
            gifs: None,
            extensions: None,
            search: None,
            calendar: None,
            calls: None,
            call: None,
            emoji: teams_core::composer_emojis(),
            stickers: teams_core::sticker_categories(),
            messages,
            posts,
            replies,
            avatars: Avatars::default(),
            images: Images::default(),
            popup: None,
            popup_closed: None,
            zoomed: None,
            online: true,
            ui: screens::Ui::new(window, cx),
            main_window: window.window_handle(),
        };
        this.theme = this.compute_theme();
        this
    }

    fn compute_theme(&self) -> Theme {
        Theme::for_dark(match self.prefs.appearance {
            Appearance::System => self.system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        })
    }
    pub fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        self.prefs.appearance = appearance;
        self.prefs.save();
        self.theme = self.compute_theme();
        cx.notify();
    }
    pub fn set_language(&mut self, language: LanguagePref, cx: &mut Context<Self>) {
        self.prefs.language = language;
        self.prefs.save();
        let resolved = apply_language(&self.prefs);
        self.core.send(Action::SetLanguage { language: resolved });
        self.emoji = teams_core::composer_emojis();
        self.stickers = teams_core::sticker_categories();
        cx.notify();
    }
    pub fn set_refresh(&mut self, refresh: crate::prefs::RefreshPref) {
        self.prefs.refresh = refresh;
        self.prefs.save();
        self.core.send(Action::SetRefreshMode {
            mode: self.prefs.core_refresh(),
        });
    }

    pub fn send(&self, action: Action) {
        self.core.send(action);
    }
    pub fn toggle_popup(&mut self, key: impl Into<SharedString>, cx: &mut Context<Self>) {
        let key = key.into();
        // A click on the trigger of an open popover first closes it on mouse down (click outside):
        // the click itself must then leave it closed instead of opening it again.
        if self.popup.is_none()
            && self
                .popup_closed
                .as_ref()
                .is_some_and(|(k, at)| *k == key && at.elapsed() < std::time::Duration::from_millis(400))
        {
            self.popup_closed = None;
            return;
        }
        self.popup = if self.popup.as_ref() == Some(&key) { None } else { Some(key) };
        cx.notify();
    }
    pub fn popup_open(&self, key: &str) -> bool {
        self.popup.as_ref().is_some_and(|p| p.as_ref() == key)
    }
    pub fn close_popup(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.popup.take() {
            self.popup_closed = Some((key, std::time::Instant::now()));
            cx.notify();
        }
    }
    pub fn in_call(&self) -> bool {
        self.call
            .as_ref()
            .and_then(|c| c.active.as_ref())
            .is_some_and(|a| a.phase != teams_core::CallPhaseView::Ended)
    }

    fn apply(&mut self, event: CoreEvent, cx: &mut Context<Self>) {
        match event {
            CoreEvent::Update(update) => self.apply_update(*update, cx),
            CoreEvent::Effect(effect) => self.apply_effect(effect, cx),
        }
        cx.notify();
    }

    fn apply_update(&mut self, update: Update, cx: &mut Context<Self>) {
        if let Some(ids) = update.avatar_ids {
            let (wanted, dropped) = self.avatars.keep(ids);
            release_images(dropped, None, cx);
            for id in wanted {
                let core = self.core.clone();
                let decoded = cx.background_spawn(async move {
                    let image = core.take_avatar(id.clone()).and_then(avatars::decode);
                    (id, image)
                });
                cx.spawn(async move |this, cx| {
                    let (id, image) = decoded.await;
                    let _ = this.update(cx, |this, cx| {
                        this.avatars.decoding.remove(&id);
                        if let Some(image) = image
                            && this.avatars.kept.contains(&id)
                        {
                            if let Some(old) = this.avatars.images.insert(id, image) {
                                release_images(vec![old], None, cx);
                            }
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        if let Some(srcs) = update.image_srcs {
            self.keep_images(srcs, cx);
        }
        if let Some(shell) = update.shell {
            if shell.signed_in && !self.shell.as_ref().is_some_and(|s| s.signed_in) {
                screens::signed_in(self, cx);
            }
            crate::platform::dock_badge(
                shell
                    .signed_in
                    .then_some((shell.unread, shell.activity_unread)),
            );
            self.shell = Some(shell);
        }
        if let Some(composer) = update.composer {
            self.composer = Some(composer);
            screens::composer_changed(self, cx);
        }
        if let Some(people) = update.mention_people {
            self.mention_people = people;
        }
        if let Some(sidebar) = update.sidebar {
            self.sidebar = Some(sidebar);
        }
        if let Some(part) = update.conversation {
            let key = |c: &ConversationView| (c.target.clone(), c.is_channel);
            if self.conversation.as_ref().map(key) != part.view.as_ref().map(key) {
                self.messages.clear();
                self.posts.clear();
                self.replies.clear();
                self.zoomed = None;
            }
            let thread = |c: &Option<ConversationView>| {
                c.as_ref()
                    .and_then(|c| c.thread.as_ref().map(|t| t.post.clone()))
            };
            if thread(&self.conversation) != thread(&part.view) {
                self.replies.clear();
            }
            self.conversation = part.view;
        }
        if let Some(typing) = update.typing {
            self.typing = typing.text;
        }
        if let Some(feed) = update.feed {
            self.messages
                .apply(feed.messages.ids, feed.messages.changed);
            self.posts
                .apply(feed.posts.ids, feed.posts.changed);
            self.replies
                .apply(feed.replies.ids, feed.replies.changed);
        }
        let (more_feed, more_replies) = match &self.conversation {
            Some(c) => (
                c.has_more && !c.loading,
                c.thread.as_ref().is_some_and(|t| t.has_more && !t.loading),
            ),
            None => (false, false),
        };
        self.messages.set_can_load_more(more_feed);
        self.posts.set_can_load_more(more_feed);
        self.replies.set_can_load_more(more_replies);
        if let Some(v) = update.members {
            self.members = Some(v);
        }
        if let Some(v) = update.files {
            self.files = Some(v);
        }
        if let Some(v) = update.browser {
            self.browser = Some(v);
        }
        if let Some(v) = update.new_chat {
            self.new_chat = v.dialog;
        }
        if let Some(v) = update.notifications {
            self.notifications = Some(v);
        }
        if let Some(v) = update.me {
            self.me = Some(v);
        }
        if let Some(v) = update.chat_admin {
            self.chat_admin = Some(v);
        }
        if let Some(v) = update.teams_admin {
            self.teams_admin = Some(v);
        }
        if let Some(v) = update.gifs {
            self.gifs = Some(v);
        }
        if let Some(v) = update.extensions {
            self.extensions = Some(v);
        }
        if let Some(v) = update.search {
            self.search = Some(v);
        }
        if let Some(v) = update.calendar {
            self.calendar = Some(v);
        }
        if let Some(v) = update.calls {
            if let Some(id) = v.audio_ready.clone()
                && let Some(audio) = self.core.take_voicemail_audio(id.clone())
            {
                crate::platform::play_voicemail(&id, audio.data);
            }
            self.calls = Some(v);
        }
        if let Some(v) = update.call {
            self.call = Some(v);
            screens::call_changed(self, cx);
        }
    }

    fn keep_images(&mut self, srcs: Vec<String>, cx: &mut Context<Self>) {
        self.images.kept = srcs.into_iter().collect();
        let gone: Vec<String> = self
            .images
            .images
            .keys()
            .filter(|src| !self.images.kept.contains(*src))
            .cloned()
            .collect();
        let dropped = gone.iter().filter_map(|src| self.images.images.remove(src)).collect();
        release_images(dropped, None, cx);
        let wanted: Vec<String> = self
            .images
            .kept
            .iter()
            .filter(|s| !self.images.images.contains_key(*s))
            .cloned()
            .collect();
        for src in wanted {
            if !self.images.decoding.insert(src.clone()) {
                continue;
            }
            let core = self.core.clone();
            let decoded = cx.background_spawn(async move {
                let image = core.take_image(src.clone()).and_then(avatars::decode);
                (src, image)
            });
            cx.spawn(async move |this, cx| {
                let (src, image) = decoded.await;
                let _ = this.update(cx, |this, cx| {
                    this.images.decoding.remove(&src);
                    if let Some(image) = image
                        && this.images.kept.contains(&src)
                    {
                        if let Some(old) = this.images.images.insert(src, image) {
                            release_images(vec![old], None, cx);
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn apply_effect(&mut self, effect: Effect, cx: &mut Context<Self>) {
        match effect {
            Effect::Copy { value } => cx.write_to_clipboard(gpui::ClipboardItem::new_string(value)),
            Effect::Notify {
                id,
                conversation_id,
                reply_chain_id,
                title,
                subtitle,
                body,
                sound,
                urgent,
                can_reply,
            } => crate::platform::notify(crate::platform::Banner {
                id,
                conversation_id,
                reply_chain_id,
                title,
                subtitle,
                body,
                sound,
                urgent,
                can_reply,
            }),
            Effect::WithdrawNotifications { conversation_id } => {
                crate::platform::withdraw(&conversation_id)
            }
            Effect::OpenLogin { url } => crate::login_window::open(self, url, cx),
            Effect::CloseLogin => crate::login_window::close(self, cx),
            // These need the main window, reached once this update is over.
            Effect::SilentLogin { url } => self.in_main_window(cx, move |this, window, cx| {
                crate::login_window::silent(this, url, window, cx)
            }),
            Effect::ForgetLogin => {
                self.in_main_window(cx, |this, window, _| crate::login_window::forget(this, window))
            }
            Effect::EmojiCatalogChanged => self.emoji = teams_core::composer_emojis(),
            Effect::StickerCatalogChanged => self.stickers = teams_core::sticker_categories(),
            Effect::QuickLook { path } => crate::platform::quick_look(&path),
            Effect::Downloaded { path } => crate::platform::downloaded(&path),
            media => screens::media_effect(self, media, cx),
        }
    }

    /// Runs `f` with the main window, after the current update.
    fn in_main_window(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let handle = self.main_window;
        cx.spawn(async move |this, cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| f(this, window, cx));
            });
        })
        .detach();
    }

    /// Asks the core for a photo once per id and name.
    pub fn request_avatar(&mut self, id: &str, name: &str) {
        if self.avatars.images.contains_key(id) {
            return;
        }
        if self.avatars.requested.insert(format!("{id}|{name}")) {
            self.core.send(Action::LoadAvatar {
                id: id.to_owned(),
                name: name.to_owned(),
            });
        }
    }

    /// The person's or chat's photo, else initials on a colour of their id; with the presence badge.
    pub fn avatar(
        &mut self,
        id: &str,
        name: &str,
        size: f32,
        shape: AvatarShape,
        presence: Option<&PresenceView>,
    ) -> AnyElement {
        self.request_avatar(id, name);
        let radius = if shape == AvatarShape::Square {
            size * 0.25
        } else {
            size / 2.
        };
        let face = match self.avatars.images.get(id) {
            Some(image) => img(image.clone())
                .size(px(size))
                .rounded(px(radius))
                .into_any_element(),
            None => div()
                .size(px(size))
                .rounded(px(radius))
                .bg(Theme::avatar(id))
                .flex()
                .items_center()
                .justify_center()
                .text_color(gpui::white())
                .text_size(px(size * 0.32))
                .font_weight(FontWeight::SEMIBOLD)
                .child(initials(name))
                .into_any_element(),
        };
        let badge = presence.and_then(|p| Theme::presence(p.status, p.out_of_office));
        let badge_size = (size * 0.16).round().max(5.) * 2.;
        div()
            .relative()
            .flex_none()
            .size(px(size))
            .child(face)
            .when_some(badge, |this, color| {
                this.child(
                    div()
                        .absolute()
                        .right(px(-2.))
                        .bottom(px(-2.))
                        .size(px(badge_size))
                        .rounded_full()
                        .border_2()
                        .border_color(self.theme.surface)
                        .bg(color),
                )
            })
            .into_any_element()
    }
}

/// Frees pictures the app no longer holds from the windows' GPU atlases: GPUI keeps an uploaded image
/// there until told, so dropping our `Arc` alone would leave its texture behind for good. `window` is the
/// window being drawn, if any: GPUI cannot reach it through the app while it is.
pub fn release_images(images: Vec<Arc<RenderImage>>, mut window: Option<&mut Window>, cx: &mut gpui::App) {
    for image in images {
        cx.drop_image(image, window.as_deref_mut());
    }
}

fn is_dark(appearance: gpui::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}

pub fn initials(name: &str) -> SharedString {
    let words: Vec<&str> = name.split_whitespace().collect();
    let value: String = match (words.first(), words.last()) {
        (Some(first), Some(last)) if words.len() > 1 => first
            .chars()
            .take(1)
            .chain(last.chars().take(1))
            .collect(),
        _ => name.chars().take(2).collect(),
    };
    value.to_uppercase().into()
}

impl Render for Oxide {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::media_video::sweep(window);
        shell::root(self, window, cx)
    }
}
