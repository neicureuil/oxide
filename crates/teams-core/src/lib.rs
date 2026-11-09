//! Headless Teams client: state machine, network loading and view-models; Swift sends [`Action`]s and gets changed parts as [`Update`]s.
uniffi::setup_scaffolding!();

mod app;
mod attachments;
mod avatars;
#[cfg(feature = "demo")]
mod calendar_mock;
#[cfg(feature = "demo")]
mod calls_mock;
mod cards;
mod conversations;
mod disk_cache;
mod emoji;
mod error;
#[cfg(feature = "demo")]
mod files_mock;
mod html;
mod login;
mod media;
mod memory;
#[cfg(feature = "demo")]
mod manage_mock;
#[cfg(feature = "demo")]
mod mock;
#[cfg(feature = "demo")]
mod notifications_mock;
mod session;
mod stickers;
mod task;
#[cfg(test)]
mod timing;

pub use app::calendar::{
    CalendarAction, CalendarDraft, CalendarEventView, CalendarPerson, CalendarView,
    RecurrenceDraft, Repeat, RepeatUnit,
};
pub use app::call::{ActiveCallView, CallAction, CallPart, CallPhaseView, IceServerView};
pub use app::calls::{
    CallDirectionView, CallLogView, CallOutcome, CallsAction, CallsView, VoicemailAudio,
    VoicemailView,
};
use app::conversation::ConversationMsg;
pub use app::conversation::Importance;
pub use app::files::{
    BrowserPlace, BrowserView, EntryView, FileAction, PendingFileView, SharedFilter,
};
pub use app::formatting::{ComposerBlock, ComposerFormat};
pub use app::chat_admin::{AddPeopleView, ChatAdminAction, ChatAdminView, ShareHistory};
pub use app::teams_admin::{
    GalleryJoin, GalleryTeamView, GalleryView, JoinCodeView, NewTeamVisibility, TeamsAdminAction,
    TeamsAdminView,
};
pub use app::extensions::{
    ComposerCardView, ExtensionAppView, ExtensionCommandView, ExtensionFormView, ExtensionLinkView,
    ExtensionResultView, ExtensionsAction, ExtensionsView,
};
pub use app::link_preview::{LinkPreviewAction, LinkPreviewView};
pub use app::gifs::{GifView, GifsAction, GifsView, StickerCategory, StickerView};
pub use app::search::{
    SearchAction, SearchDate, SearchFiltersView, SearchHitView, SearchKind, SearchView,
};
pub use app::me::{
    MeAction, MeView, MyStatus, OutOfOffice, OutOfOfficeView, OutsideAudience, StatusDuration,
};
pub use app::notifications::{
    ChannelChoice, ChatDesktop, EmailDelay, MeetingChats, NotificationChoice,
    NotificationEditorView, NotificationPermission, NotificationPreset, NotificationRow,
    NotificationRowView, NotificationSection, NotificationSound, NotificationToggle,
    NotificationsAction, NotificationsView,
};
pub use app::receipts::ReceiptView;
pub use app::team_list::{TeamListAction, TeamListControls, TeamSort};
pub use app::view::*;
pub use app::{Filter, Nav, RefreshMode, Tab};
pub use task::Effect;

use app::view::{Parts, Snapshot};
use app::{App, Msg};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak, mpsc};
use std::time::{Duration, Instant};
use task::{Job, Task};
use tokio::sync::watch;

/// Refresh cadence while the window is in front, and while it is hidden or in the background.
const ACTIVE_REFRESH: Duration = Duration::from_secs(5);
const IDLE_REFRESH: Duration = Duration::from_secs(60);

/// Implemented in Swift. Calls arrive on core threads, in order.
#[uniffi::export(with_foreign)]
pub trait CoreListener: Send + Sync {
    fn update(&self, update: Update);
    fn effect(&self, effect: Effect);
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum Action {
    TeamList {
        action: TeamListAction,
    },
    OpenActivity {
        id: String,
    },
    MoreActivity,
    LoadAvatar {
        id: String,
        name: String,
    },
    Calendar {
        action: CalendarAction,
    },
    Calls {
        action: CallsAction,
    },
    Call {
        action: CallAction,
    },
    Files {
        action: FileAction,
    },
    Notifications {
        action: NotificationsAction,
    },
    Me {
        action: MeAction,
    },
    ChatAdmin {
        action: ChatAdminAction,
    },
    TeamsAdmin {
        action: TeamsAdminAction,
    },
    Gifs {
        action: GifsAction,
    },
    /// The compose box's message extensions.
    Extensions {
        action: ExtensionsAction,
    },
    /// The compose box's link preview.
    LinkPreview {
        action: LinkPreviewAction,
    },
    FullSearch {
        action: SearchAction,
    },
    /// A button of a bot card, with the values typed into the card's inputs.
    CardAction {
        message_id: String,
        card: u32,
        action: u32,
        inputs: Vec<CardInputValue>,
    },
    SignIn,
    SignInBrowser,
    CopyLoginUrl,
    CallbackChanged {
        value: String,
    },
    CompleteSignIn,
    LoginReturned {
        url: String,
    },
    LoginCancelled,
    /// The invisible sign-in did not reach the Teams return URL in time.
    SilentLoginFailed,
    SignOut,
    Refresh,
    /// Chosen in Settings: every part of the screen is rebuilt in that language.
    SetLanguage {
        language: Language,
    },
    /// Kept by Swift and sent at launch and on every change.
    SetRefreshMode {
        mode: RefreshMode,
    },
    Navigate {
        nav: Nav,
    },
    SetFilter {
        filter: Filter,
    },
    SetTab {
        tab: Tab,
    },
    Search {
        query: String,
    },
    DismissError,
    /// Closes one notice banner, by its text.
    DismissNotice {
        text: String,
    },
    Select {
        target: Target,
    },
    ToggleTeam {
        id: String,
    },
    LoadChannels {
        id: String,
    },
    ChannelMembers,
    MoreFeed,
    MoreChats,
    OpenReply {
        post: String,
    },
    CloseReply,
    MoreReplies,
    DraftChanged {
        value: String,
    },
    RichDraftChanged {
        value: String,
        formats: Vec<ComposerFormat>,
    },
    SubjectChanged {
        value: String,
    },
    /// Delivery option of the open draft.
    SetImportance {
        importance: Importance,
    },
    InsertEmoji {
        emoji: String,
    },
    InsertMention,
    MentionMembers,
    ReplaceDraft {
        start: u32,
        length: u32,
        text: String,
        mention: Option<String>,
        emoji: Option<String>,
        style: Option<ComposerFormat>,
    },
    Send,
    NewChat,
    CancelNewChat,
    PeopleQuery {
        query: String,
    },
    PeopleSearch,
    ChoosePerson {
        mri: String,
    },
    RemovePerson {
        mri: String,
    },
    NewTitle {
        title: String,
    },
    CreateChat,
    React {
        id: String,
        key: String,
    },
    Edit {
        id: String,
    },
    CancelEdit,
    SaveEdit,
    Delete {
        id: String,
    },
    RetryImage {
        src: String,
    },
    /// Ids of the messages and posts on screen and about a screen around it, whose previews load first.
    ShowMessages {
        ids: Vec<String>,
    },
    OpenUrl {
        url: String,
    },
}

fn message(app: &App, action: Action) -> Option<Msg> {
    Some(match action {
        Action::TeamList { action } => Msg::TeamList(app::team_list::TeamListMsg::Action(action)),
        Action::Calendar { action } => Msg::Calendar(app::calendar::CalendarMsg::Action(action)),
        Action::Calls { action } => Msg::Calls(app::calls::CallsMsg::Action(action)),
        Action::Call { action } => Msg::Call(app::call::CallMsg::Action(action)),
        Action::Files { action } => Msg::Files(app::files::FilesMsg::Action(action)),
        Action::Notifications { action } => {
            Msg::Notifications(app::notifications::NotificationsMsg::Action(action))
        }
        Action::Me { action } => Msg::Me(app::me::MeMsg::Action(action)),
        Action::Gifs { action } => Msg::Gifs(app::gifs::GifsMsg::Action(action)),
        Action::Extensions { action } => {
            Msg::Extensions(app::extensions::ExtensionsMsg::Action(action))
        }
        Action::LinkPreview { action } => {
            Msg::LinkPreview(app::link_preview::LinkPreviewMsg::Action(action))
        }
        Action::FullSearch { action } => {
            Msg::FullSearch(app::search::SearchMsg::Action(action))
        }
        Action::CardAction {
            message_id,
            card,
            action,
            inputs,
        } => Msg::Card(app::card_actions::CardMsg::Action {
            message: message_id,
            card,
            action,
            inputs,
        }),
        Action::TeamsAdmin { action } => {
            Msg::TeamsAdmin(app::teams_admin::TeamsAdminMsg::Action(action))
        }
        Action::ChatAdmin { action } => {
            Msg::ChatAdmin(app::chat_admin::ChatAdminMsg::Action(action))
        }
        Action::SignIn => Msg::SignIn,
        Action::SignInBrowser => Msg::SignInBrowser,
        Action::CopyLoginUrl => Msg::CopyLoginUrl,
        Action::CallbackChanged { value } => Msg::CallbackChanged(value),
        Action::CompleteSignIn => Msg::CompleteSignIn,
        Action::LoginReturned { url } => Msg::LoginReturned(url),
        Action::LoginCancelled => Msg::LoginCancelled,
        Action::SilentLoginFailed => Msg::SilentLoginFailed,
        Action::SignOut => Msg::SignOut,
        Action::Refresh => Msg::Refresh,
        // Applied by `Inner::handle`, which rebuilds the whole screen.
        Action::SetLanguage { .. } => return None,
        Action::SetRefreshMode { mode } => Msg::SetRefreshMode(mode),
        Action::Navigate { nav } => Msg::Navigate(nav),
        Action::SetFilter { filter } => Msg::SetFilter(filter),
        Action::OpenActivity { id } => Msg::Activity(app::activity::ActivityMsg::Open(id)),
        Action::MoreActivity => Msg::Activity(app::activity::ActivityMsg::More),
        Action::LoadAvatar { id, name } => Msg::LoadAvatar(id, name),
        Action::SetTab { tab } => Msg::SetTab(tab),
        Action::Search { query } => Msg::Search(query),
        Action::DismissError => Msg::DismissError,
        Action::DismissNotice { text } => Msg::DismissNotice(text),
        Action::Select { target } => Msg::Select(target.into()),
        Action::ToggleTeam { id } => Msg::ToggleTeam(id),
        Action::LoadChannels { id } => Msg::LoadChannels(id),
        Action::ChannelMembers => Msg::ChannelMembers,
        Action::MoreFeed => ConversationMsg::MoreFeed.into(),
        Action::MoreChats => Msg::MoreChats,
        Action::OpenReply { post } => ConversationMsg::OpenReply(post).into(),
        Action::CloseReply => ConversationMsg::CloseReply.into(),
        Action::MoreReplies => ConversationMsg::MoreReplies.into(),
        Action::DraftChanged { value } => Msg::DraftChanged(value),
        Action::RichDraftChanged { value, formats } => Msg::RichDraftChanged(value, formats),
        Action::SubjectChanged { value } => ConversationMsg::SubjectChanged(value).into(),
        Action::SetImportance { importance } => ConversationMsg::SetImportance(importance).into(),
        Action::InsertEmoji { emoji } => ConversationMsg::InsertEmoji(emoji).into(),
        Action::InsertMention => ConversationMsg::InsertMention.into(),
        Action::MentionMembers => {
            if app.mention_members_cached() {
                return None;
            }
            Msg::ChannelMembers
        }
        Action::ReplaceDraft {
            start,
            length,
            text,
            mention,
            emoji,
            style,
        } => Msg::ReplaceDraft {
            start,
            length,
            text,
            mention,
            emoji,
            style,
        },
        Action::Send => Msg::Send,
        Action::NewChat => Msg::NewChat,
        Action::CancelNewChat => Msg::CancelNewChat,
        Action::PeopleQuery { query } => Msg::PeopleQuery(query),
        Action::PeopleSearch => Msg::PeopleSearch,
        Action::ChoosePerson { mri } => {
            Msg::ChoosePerson(app.candidates().into_iter().find(|p| p.mri == mri)?)
        }
        Action::RemovePerson { mri } => Msg::RemovePerson(mri),
        Action::NewTitle { title } => Msg::NewTitle(title),
        Action::CreateChat => Msg::CreateChat,
        Action::React { id, key } => ConversationMsg::React { id, key }.into(),
        Action::Edit { id } => Msg::Edit(id),
        Action::CancelEdit => Msg::CancelEdit,
        Action::SaveEdit => Msg::SaveEdit,
        Action::Delete { id } => ConversationMsg::Delete(id).into(),
        Action::RetryImage { src } => ConversationMsg::RetryImage(src).into(),
        Action::ShowMessages { ids } => ConversationMsg::Shown(ids).into(),
        Action::OpenUrl { url } => Msg::OpenUrl(url),
    })
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    /// The picture file as downloaded (PNG, JPEG, GIF or WebP), to decode at `width` × `height`.
    pub data: Vec<u8>,
}

struct State {
    app: App,
    snapshot: Snapshot,
    running: Running,
}
/// Network work of the current generation, aborted (not just ignored) on sign-out or recovery.
#[derive(Default)]
struct Running {
    generation: u64,
    tasks: tokio::task::JoinSet<()>,
}
impl Running {
    /// Aborts what an earlier generation left running, then forgets finished tasks.
    fn enter(&mut self, generation: u64) {
        if generation != self.generation {
            self.tasks.abort_all();
            self.generation = generation;
        }
        while self.tasks.try_join_next().is_some() {}
    }
}
/// `TEAMS_CORE_TIMING=1` logs how long each diff and its delivery to Swift take.
fn timing() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TEAMS_CORE_TIMING").is_some())
}
/// Work for the core thread: what Swift asked for, or what a network task brought back.
enum Event {
    Action(Action),
    Msg(Msg),
    /// The emoticon catalog was installed: reactions are drawn again.
    EmojiCatalog,
    /// The sticker catalog was installed: the picker asks for it again.
    StickerCatalog,
}
struct Inner {
    state: Mutex<State>,
    runtime: tokio::runtime::Handle,
    listener: Arc<dyn CoreListener>,
    /// Feeds the core thread, which applies events in order so neither Swift nor network workers wait for a diff.
    events: mpsc::Sender<Event>,
    /// The app's photo cache, taken from without the state lock.
    avatars: Arc<Mutex<avatars::Cache>>,
    /// Ready previews and voicemail audio, also taken from without the state lock.
    handoff: Arc<Mutex<media::Handoff>>,
    demo: bool,
}
/// A panic is caught and the state rebuilt, so a poisoned lock still guards usable data.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
impl Inner {
    fn queue(&self, event: Event) {
        // Fails only once the core thread has stopped, when the app is shutting down.
        let _ = self.events.send(event);
    }
    fn handle(self: &Arc<Self>, event: Event) {
        let mut state = lock(&self.state);
        let applied = catch_unwind(AssertUnwindSafe(|| {
            let msg = match event {
                Event::Action(Action::SetLanguage { language }) => {
                    set_language(language);
                    state.app.relocalize();
                    let _ = state.app.take_changes();
                    self.publish(&mut state, Task::none(), Parts::ALL);
                    self.load_emoji();
                    self.load_stickers();
                    return;
                }
                Event::EmojiCatalog => {
                    let task = Task::effect(Effect::EmojiCatalogChanged);
                    self.publish(&mut state, task, Parts::ALL);
                    return;
                }
                Event::StickerCatalog => {
                    let task = Task::effect(Effect::StickerCatalogChanged);
                    self.publish(&mut state, task, Parts::NONE);
                    return;
                }
                Event::Msg(msg) => msg,
                Event::Action(action) => match message(&state.app, action) {
                    Some(msg) => msg,
                    None => return,
                },
            };
            let task = state.app.update(msg);
            let parts = state.app.take_changes();
            self.publish(&mut state, task, parts);
        }));
        if applied.is_err() {
            // Already logged by the panic hook; without recovering, the core thread would stop reacting.
            self.recover(&mut state);
        }
    }
    fn recover(self: &Arc<Self>, state: &mut State) {
        let rebuilt = catch_unwind(AssertUnwindSafe(|| {
            state.running.tasks.abort_all();
            let task = state.app.recover(self.demo);
            state.snapshot = Snapshot::default();
            let _ = state.app.take_changes();
            self.publish(state, task, Parts::ALL);
        }));
        if rebuilt.is_err() {
            eprintln!("teams-core: recovery after a panic failed");
        }
    }
    /// Sends the changed parts, then starts the task, under the state lock so Swift sees updates in order.
    fn publish(self: &Arc<Self>, state: &mut State, task: Task<Msg>, parts: Parts) {
        let State {
            app,
            snapshot,
            running,
        } = state;
        running.enter(app.generation());
        let started = timing().then(Instant::now);
        let update = snapshot.diff(app, parts);
        let diffed = started.map(|s| s.elapsed());
        if !update.is_empty() {
            self.listener.update(update);
        }
        if let (Some(started), Some(diffed)) = (started, diffed) {
            eprintln!(
                "teams-core: diff {diffed:?}, delivery {:?}",
                started.elapsed() - diffed
            );
        }
        for job in task.0 {
            match job {
                Job::Effect(effect) => self.listener.effect(effect),
                // `App::update` applies its own follow-ups; one from boot waits its turn.
                Job::Done(msg) => self.queue(Event::Msg(msg)),
                Job::Subscribe(start) => {
                    let inner: Weak<Self> = Arc::downgrade(self);
                    let sink = task::Sink::new(move |msg| {
                        if let Some(inner) = inner.upgrade() {
                            inner.queue(Event::Msg(msg));
                        }
                    });
                    running.tasks.spawn_on(start(sink), &self.runtime);
                }
                Job::Future(future) => {
                    let inner: Weak<Self> = Arc::downgrade(self);
                    let run = async move {
                        let msg = future.await;
                        if let Some(inner) = inner.upgrade() {
                            inner.queue(Event::Msg(msg));
                        }
                    };
                    running.tasks.spawn_on(run, &self.runtime);
                }
            }
        }
    }
    fn signed_in(&self) -> bool {
        lock(&self.state).app.signed_in()
    }
    fn load_emoji(self: &Arc<Self>) {
        self.load_catalog(emoji::has, emoji::load, Event::EmojiCatalog);
    }
    fn load_stickers(self: &Arc<Self>) {
        self.load_catalog(stickers::has, stickers::load, Event::StickerCatalog);
    }
    /// Installs a catalog of the app's language, retrying while unreachable; outlives sign-outs.
    fn load_catalog<L, F>(self: &Arc<Self>, has: fn(&str) -> bool, load: L, done: Event)
    where
        L: Fn(&'static str) -> F + Send + 'static,
        F: Future<Output = bool> + Send,
    {
        let locale = emoji::locale();
        if has(locale) {
            return;
        }
        let inner = Arc::downgrade(self);
        self.runtime.spawn(async move {
            let mut wait = Duration::from_secs(30);
            while !load(locale).await {
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(600));
                if inner.strong_count() == 0 {
                    return;
                }
            }
            if let Some(inner) = inner.upgrade() {
                inner.queue(done);
            }
        });
    }
}

#[derive(uniffi::Object)]
pub struct OxideCore {
    inner: Arc<Inner>,
    active: watch::Sender<bool>,
    /// Kept last so tasks are aborted after the state they refer to is unreachable.
    _runtime: tokio::runtime::Runtime,
}

#[uniffi::export]
impl OxideCore {
    /// Boots the client; `demo` swaps in an in-process fake service when built with the `demo` feature.
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn CoreListener>, demo: bool) -> Arc<Self> {
        if demo && !cfg!(feature = "demo") {
            eprintln!("teams-core: --demo needs a core built with the `demo` feature");
        }
        let demo = demo && cfg!(feature = "demo");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("teams-core")
            .enable_all()
            .build()
            .expect("tokio runtime");
        let (app, task) = {
            // Demo mode binds its fake server from a runtime context.
            let _guard = runtime.enter();
            App::boot(demo)
        };
        let (events, received) = mpsc::channel();
        let avatars = app.avatars.clone();
        let handoff = app.handoff.clone();
        let inner = Arc::new(Inner {
            avatars,
            handoff,
            demo,
            state: Mutex::new(State {
                app,
                snapshot: Snapshot::default(),
                running: Running::default(),
            }),
            runtime: runtime.handle().clone(),
            listener,
            events,
        });
        {
            let mut state = lock(&inner.state);
            let _ = state.app.take_changes();
            inner.publish(&mut state, task, Parts::ALL);
        }
        inner.load_emoji();
        inner.load_stickers();
        let weak = Arc::downgrade(&inner);
        let handle = runtime.handle().clone();
        // Stops when `Inner`, which holds the only sender, is dropped.
        std::thread::Builder::new()
            .name("teams-core-state".into())
            .spawn(move || {
                // Updates may create timers or sockets, which need the runtime's context.
                let _guard = handle.enter();
                while let Ok(event) = received.recv() {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    inner.handle(event);
                }
            })
            .expect("core thread");
        let (active, mut changes) = watch::channel(true);
        let weak = Arc::downgrade(&inner);
        runtime.spawn(async move {
            loop {
                let wait = if *changes.borrow_and_update() {
                    ACTIVE_REFRESH
                } else {
                    IDLE_REFRESH
                };
                let msg = tokio::select! {
                    _ = tokio::time::sleep(wait) => Some(Msg::Tick),
                    changed = changes.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        // Coming back to the front refreshes at once.
                        changes.borrow().then_some(Msg::Foreground)
                    }
                };
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                if let Some(msg) = msg
                    && inner.signed_in()
                {
                    inner.queue(Event::Msg(msg));
                }
            }
        });
        Arc::new(Self {
            inner,
            active,
            _runtime: runtime,
        })
    }
    /// Returns at once: the action is applied on the core thread, after the ones before it.
    pub fn send(&self, action: Action) {
        self.inner.queue(Event::Action(action));
    }
    /// The window came to the front (`true`) or went to the background.
    pub fn set_active(&self, active: bool) {
        // Banners are not shown for the conversation in front of the user.
        self.inner.queue(Event::Msg(Msg::Notifications(
            app::notifications::NotificationsMsg::Active(active),
        )));
        self.active.send_if_modified(|current| {
            let changed = *current != active;
            *current = active;
            changed
        });
    }
    /// File of a ready preview, handed over once: Swift keeps the only decoded copy.
    pub fn take_image(&self, src: String) -> Option<ImageData> {
        let (width, height, data) = lock(&self.inner.handoff).take_image(&src)?;
        Some(ImageData {
            width,
            height,
            data,
        })
    }
    /// Audio of the voicemail named by `CallsView.audio_ready`, handed over once.
    pub fn take_voicemail_audio(&self, id: String) -> Option<VoicemailAudio> {
        let (content_type, data) = lock(&self.inner.handoff).take_voicemail(&id)?;
        Some(VoicemailAudio { content_type, data })
    }
    pub fn take_avatar(&self, id: String) -> Option<ImageData> {
        let (width, height, data) = lock(&self.inner.avatars).take(&id)?;
        Some(ImageData {
            width,
            height,
            data,
        })
    }
}

/// Language of the texts the core builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Language {
    French,
    English,
}
/// Sets the language before the core boots; `Action::SetLanguage` changes it afterwards.
#[uniffi::export]
pub fn set_language(language: Language) {
    teams_i18n::set_language(match language {
        Language::French => teams_i18n::Language::French,
        Language::English => teams_i18n::Language::English,
    });
}
/// Every text of the catalog in the current language, for the Swift side.
#[uniffi::export]
pub fn translations() -> std::collections::HashMap<String, String> {
    teams_i18n::table()
}
/// The app's version: the workspace's in `Cargo.toml`, also written to Info.plist by `build-app.sh`.
#[uniffi::export]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
/// Whether this core was built with the fake services behind `--demo`.
#[uniffi::export]
pub fn demo_available() -> bool {
    cfg!(feature = "demo")
}
/// The login window stops on this URL and hands it to the core.
#[uniffi::export]
pub fn is_login_callback(url: String) -> bool {
    login::is_callback(&url)
}
/// Navigation the login window may follow.
#[uniffi::export]
pub fn is_login_allowed(url: String) -> bool {
    login::is_allowed(&url)
}
/// The Teams emoticon catalog in the app's language; empty until downloaded, then `Effect::EmojiCatalogChanged`.
#[uniffi::export]
pub fn composer_emojis() -> Vec<emoji::EmojiCategory> {
    emoji::categories()
}
/// The stickers Teams bundles, by category, in the app's language.
#[uniffi::export]
pub fn sticker_categories() -> Vec<StickerCategory> {
    app::gifs::sticker_categories()
}
/// Where the core keeps its own downloads (emoticon and sticker catalogs, pictures); called before the core boots.
#[uniffi::export]
pub fn set_cache_directory(path: String) {
    disk_cache::set_dir(path.clone().into());
    emoji::set_cache_dir(path.clone().into());
    stickers::set_cache_dir(path.into());
}

/// Pictures kept on disk between launches, for Settings › Debug.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PictureCache {
    pub preview_files: u64,
    pub preview_bytes: u64,
    pub photo_files: u64,
    pub photo_bytes: u64,
}
/// Walks the picture folders: call it off the main thread.
#[uniffi::export]
pub fn picture_cache() -> PictureCache {
    let (preview_files, preview_bytes) = disk_cache::usage(disk_cache::Kind::Preview);
    let (photo_files, photo_bytes) = disk_cache::usage(disk_cache::Kind::Photo);
    PictureCache {
        preview_files,
        preview_bytes,
        photo_files,
        photo_bytes,
    }
}
/// Deletes the pictures kept on disk; the next ones shown are downloaded again.
#[uniffi::export]
pub fn clear_picture_cache() {
    disk_cache::clear_now();
}

#[cfg(test)]
mod running_tests {
    use super::Running;
    use std::time::Duration;

    #[tokio::test]
    async fn a_new_generation_aborts_the_work_of_the_previous_one() {
        let handle = tokio::runtime::Handle::current();
        let mut running = Running::default();
        let (dropped, mut gone) = tokio::sync::oneshot::channel::<()>();
        running.enter(0);
        running.tasks.spawn_on(
            async move {
                let _dropped = dropped;
                std::future::pending::<()>().await
            },
            &handle,
        );
        running.tasks.spawn_on(async {}, &handle);
        tokio::time::sleep(Duration::from_millis(20)).await;
        running.enter(0);
        assert_eq!(running.tasks.len(), 1, "finished work is forgotten");
        assert!(gone.try_recv().is_err(), "same generation keeps running");
        running.enter(1);
        assert!(
            gone.await.is_err(),
            "the pending task was aborted and dropped"
        );
    }
}
