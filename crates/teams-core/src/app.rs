//! Client state machine, without any rendering.
pub(crate) use crate::error::CoreError;
#[cfg(feature = "demo")]
pub(crate) use crate::mock;
pub(crate) use crate::session::{AppAuthenticator, open_browser};
pub(crate) use crate::task::{Effect, Job, Task};
pub(crate) use crate::{conversations, html, media, session};
pub(crate) use std::collections::{HashMap, HashSet};
pub(crate) use std::sync::Arc;
pub(crate) use std::time::Duration;
pub(crate) use teams_auth::{LoginAttempt, TeamsAuthenticator};
#[cfg(feature = "demo")]
pub(crate) use teams_auth::{Service, StaticAuthenticator};
pub(crate) use teams_chat::{
    Channel, ChatClient, ChatsPage, Conversation, Message, MessagesPage, MessagesQuery,
    OutgoingMessage, Person, PostsPage, Presence, RepliesPage, ReplyChain, Startup, Team,
};
pub(crate) use url::Url;
pub(crate) type Client = ChatClient<AppAuthenticator>;
use crate::attachments;
use chrono::{DateTime, Local};
use teams_i18n::{t, tf, tp};
/// Builds the [`Ctx`] of `$app` from its fields alone, so the sub-state handed with it can be borrowed mutably too.
macro_rules! ctx {
    ($app:expr) => {
        Ctx {
            client: $app.client.as_ref(),
            generation: $app.generation,
            me: &$app.me,
            display_name: &$app.display_name,
            account_email: &$app.account_email,
            nav: *$app.nav,
            selected: $app.selected.as_ref(),
            chats: &$app.chats,
            teams: &$app.teams,
            handoff: &$app.handoff,
        }
    };
}
#[path = "activity.rs"]
pub(crate) mod activity;
#[path = "bot_apps.rs"]
pub(crate) mod bot_apps;
#[path = "calendar.rs"]
pub(crate) mod calendar;
#[path = "call.rs"]
pub(crate) mod call;
#[path = "calls.rs"]
pub(crate) mod calls;
#[path = "changes.rs"]
pub(crate) mod changes;
#[path = "card_actions.rs"]
pub(crate) mod card_actions;
#[path = "chat_admin.rs"]
pub(crate) mod chat_admin;
#[path = "composer.rs"]
pub(crate) mod composer;
#[path = "conversation.rs"]
pub(crate) mod conversation;
#[path = "files.rs"]
pub(crate) mod files;
#[path = "formatting.rs"]
pub(crate) mod formatting;
#[path = "extensions.rs"]
pub(crate) mod extensions;
#[path = "link_preview.rs"]
pub(crate) mod link_preview;
#[path = "gifs.rs"]
pub(crate) mod gifs;
#[path = "me.rs"]
pub(crate) mod me;
#[path = "notifications.rs"]
pub(crate) mod notifications;
#[path = "notify_rules.rs"]
pub(crate) mod notify_rules;
#[path = "push.rs"]
pub(crate) mod push;
#[path = "receipts.rs"]
pub(crate) mod receipts;
#[path = "teams_admin.rs"]
pub(crate) mod teams_admin;
#[path = "search.rs"]
pub(crate) mod search;
#[path = "team_list.rs"]
pub(crate) mod team_list;
use activity::{ActivityMsg, ActivityState};
#[path = "view.rs"]
pub mod view;
use calendar::{CalendarMsg, CalendarState};
use call::{CallMsg, CallState};
use calls::{CallsMsg, CallsState};
use changes::{Parts, Tracked, feeds};
use conversation::{ConversationMsg, ConversationState};
use files::{FilesMsg, FilesState};
use card_actions::{CardMsg, CardState};
use chat_admin::{ChatAdminMsg, ChatAdminState};
use extensions::{ExtensionsMsg, ExtensionsState};
use link_preview::{LinkPreviewMsg, LinkPreviewState};
use gifs::{GifsMsg, GifsState};
use search::{SearchMsg, SearchState};
use me::{MeMsg, MeState};
use teams_admin::{TeamsAdminMsg, TeamsAdminState};
use notifications::{NotificationsMsg, NotificationsState};
const PAGE_SIZE: u32 = 50;
/// Your own chat, which Teams shows as personal notes.
pub(crate) const NOTES: &str = "48:notes";
fn notes_title() -> &'static str {
    t("core.app.you_personal_notes")
}
/// `text` in the current language when it is, in either language, the fixed text of one of these keys.
pub(crate) fn relocalized(text: &str, keys: &[&str]) -> Option<&'static str> {
    keys.iter()
        .find(|key| {
            teams_i18n::catalog()
                .get(**key)
                .is_some_and(|entry| text == entry.fr || text == entry.en)
        })
        .map(|key| t(key))
}
/// The name of a team's General channel, which Teams localizes.
fn general_channel() -> &'static str {
    t("core.app.general")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Nav {
    Chat,
    Teams,
    Activity,
    Files,
    Calendar,
    Calls,
    Settings,
}
/// How the open conversation, the chat list and the activity stay current.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum RefreshMode {
    /// The chat push channel brings changes; the network is polled only while it is down or reconnecting.
    #[default]
    Push,
    /// Polled every 5 s in front (60 s otherwise), whatever the push channel does.
    Polling,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Filter {
    All,
    Unread,
    Meetings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Tab {
    Messages,
    Files,
    Members,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Selection {
    Chat(String),
    Channel { team: String, channel: String },
}
impl Selection {
    pub fn id(&self) -> &str {
        match self {
            Self::Chat(id) => id,
            Self::Channel { channel, .. } => channel,
        }
    }
    pub fn key(&self) -> String {
        match self {
            Self::Chat(id) => format!("chat:{id}"),
            Self::Channel { team, channel } => format!("channel:{team}:{channel}"),
        }
    }
    pub fn is_channel(&self) -> bool {
        matches!(self, Self::Channel { .. })
    }
}
/// What a sub-state reads from the rest of the app, without changing it.
pub(crate) struct Ctx<'a> {
    pub client: Option<&'a Client>,
    pub generation: u64,
    pub me: &'a str,
    pub display_name: &'a str,
    pub account_email: &'a str,
    pub nav: Nav,
    pub selected: Option<&'a Selection>,
    pub chats: &'a [Conversation],
    pub teams: &'a [Team],
    pub handoff: &'a std::sync::Mutex<media::Handoff>,
}
impl Ctx<'_> {
    /// The open chat or channel accepts no new messages, edits or reactions.
    pub(crate) fn read_only(&self) -> bool {
        read_only(self.selected, self.chats, self.teams)
    }
}
fn read_only(selected: Option<&Selection>, chats: &[Conversation], teams: &[Team]) -> bool {
    match selected {
        Some(Selection::Channel { team, channel }) => {
            teams.iter().find(|t| &t.id == team).is_some_and(|t| {
                t.is_archived || t.channels.iter().any(|c| &c.id == channel && c.is_archived)
            })
        }
        Some(Selection::Chat(id)) => chats
            .iter()
            .find(|c| &c.id == id)
            .is_some_and(|c| c.is_disabled()),
        None => true,
    }
}
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    chats: Vec<Conversation>,
    favorites: Option<HashSet<String>>,
    favorites_folder: Option<(String, i64)>,
    /// Pins of the `favorites` user property, in order, when there is no folder.
    favorite_order: Vec<String>,
    teams: Vec<Team>,
    more_chats: bool,
    teams_complete: bool,
    teams_problem: Option<String>,
    team_order: Option<Vec<team_list::TeamOrder>>,
    /// `GET v1/users/ME/properties`, read with the workspace.
    preferences: Option<serde_json::Value>,
    users: Vec<Person>,
    warning: Option<String>,
}
#[derive(Default)]
pub(crate) struct NewChat {
    query: String,
    title: String,
    people: Vec<Person>,
    results: Vec<Person>,
    searching: bool,
    searched: bool,
    creating: bool,
}
/// Each field a view reads says which parts of the screen it feeds (see [`Tracked`]); the others are bookkeeping.
pub(crate) struct App {
    team_list: Tracked<team_list::TeamListState, { feeds::SIDEBAR }>,
    composer: Tracked<composer::ComposerState, { feeds::COMPOSER }>,
    activity: Tracked<ActivityState, { feeds::ACTIVITY }>,
    /// Own lock so Swift's main thread never waits on an update to take photos; changes mark `Parts::AVATARS`.
    pub(crate) avatars: Arc<std::sync::Mutex<crate::avatars::Cache>>,
    /// Ready previews and voicemail audio, taken by Swift without the state lock.
    pub(crate) handoff: Arc<std::sync::Mutex<media::Handoff>>,
    calendar: Tracked<CalendarState, { feeds::CALENDAR }>,
    calls: Tracked<CallsState, { feeds::CALLS }>,
    call: Tracked<CallState, { feeds::CALL }>,
    files: Tracked<FilesState, { feeds::FILES }>,
    notifications: Tracked<NotificationsState, { feeds::NOTIFICATIONS }>,
    /// The user's own status and note.
    me_state: Tracked<MeState, { feeds::ME }>,
    chat_admin: Tracked<ChatAdminState, { feeds::MANAGE }>,
    teams_admin: Tracked<TeamsAdminState, { feeds::MANAGE }>,
    gifs: Tracked<GifsState, { feeds::GIFS }>,
    /// Message extensions: the flyout, and the cards they put in drafts.
    extensions: Tracked<ExtensionsState, { feeds::EXTENSIONS }>,
    /// The preview of the draft's first link.
    link_preview: Tracked<LinkPreviewState, { feeds::COMPOSER }>,
    /// The search results page.
    search_state: Tracked<SearchState, { feeds::RESULTS }>,
    /// Cards whose buttons were used, by message id and card number.
    card_states: Tracked<HashMap<(String, u32), CardState>, { feeds::MESSAGES }>,
    /// The CSA "Favorites" folder (id, hierarchy version), where pins go when it exists.
    favorites_folder: Option<(String, i64)>,
    /// Pinned chats in the order they were pinned, for the `favorites` property.
    pinned_order: Vec<String>,
    /// Chats hidden from this app, with when: they come back with a newer message.
    hidden_chats: Tracked<HashMap<String, i64>, { feeds::SHELL_SIDEBAR }>,
    /// Marked unread by hand: reading the open feed does not mark them read again.
    kept_unread: HashSet<String>,
    receipts: receipts::ReceiptsState,
    client: Tracked<Option<Client>, { feeds::ALL }>,
    saved_auth: Option<Arc<TeamsAuthenticator>>,
    me: Tracked<String, { feeds::ALL }>,
    display_name: Tracked<String, { feeds::ALL }>,
    account_email: Tracked<String, { feeds::SHELL }>,
    login: Tracked<Option<LoginAttempt>, { feeds::SHELL }>,
    /// Attempt whose return is awaited from the native login window.
    window_login: Option<LoginAttempt>,
    /// `prompt=none` attempt whose return is awaited from the invisible web view.
    silent_login: Option<LoginAttempt>,
    /// No new silent attempt before then, after one failed.
    renewal_retry: Option<std::time::Instant>,
    login_url: Tracked<String, { feeds::SHELL }>,
    callback: Tracked<String, { feeds::SHELL }>,
    signing_in: Tracked<bool, { feeds::SHELL }>,
    generation: u64,
    nav: Tracked<Nav, { feeds::ALL }>,
    filter: Tracked<Filter, { feeds::SHELL_SIDEBAR }>,
    tab: Tracked<Tab, { feeds::ALL }>,
    search: Tracked<String, { feeds::SEARCH }>,
    chats: Tracked<Vec<Conversation>, { feeds::ALL }>,
    favorites: Tracked<Option<HashSet<String>>, { feeds::ALL }>,
    /// Shared with the roster lookups in flight, which only read it.
    profiles: Tracked<Arc<HashMap<String, Person>>, { feeds::ALL }>,
    details: Tracked<HashMap<String, conversations::ChatDetails>, { feeds::ALL }>,
    resolved_ids: HashSet<String>,
    resolving_chats: bool,
    teams: Tracked<Vec<Team>, { feeds::ALL }>,
    teams_problem: Tracked<Option<String>, { feeds::SIDEBAR }>,
    channels_loading: Tracked<HashSet<String>, { feeds::SIDEBAR }>,
    channels_loaded: HashSet<String>,
    channel_errors: Tracked<HashMap<String, String>, { feeds::SIDEBAR }>,
    channel_members: Tracked<HashMap<String, Vec<teams_chat::Member>>, { feeds::ALL }>,
    members_loading: Tracked<HashSet<String>, { feeds::COMPOSER_MEMBERS }>,
    members_errors: Tracked<HashMap<String, String>, { feeds::COMPOSER_MEMBERS }>,
    /// Authors and reactors already looked up in the directory, found or not.
    profiles_requested: HashSet<String>,
    /// Bots' apps by app id: their catalog name and icon.
    bot_apps: Tracked<HashMap<String, bot_apps::Lookup>, { feeds::ALL }>,
    /// Reaction lists already scanned for names, by message id and raw `emotions`.
    reactions_scanned: HashSet<u64>,
    /// Wall-clock minute the time-dependent parts were last rebuilt for.
    clock_minute: i64,
    /// Presence of the people [`App::presence_targets`] names, by lowercase MRI.
    presence: Tracked<HashMap<String, Presence>, { feeds::PRESENCE }>,
    /// When presence was last asked for, while a request is out or after it.
    presence_asked: Option<std::time::Instant>,
    /// Whom that request asked for: anyone else is asked for at once.
    presence_asked_for: HashSet<String>,
    expanded: Tracked<HashSet<String>, { feeds::SIDEBAR }>,
    selected: Tracked<Option<Selection>, { feeds::ALL }>,
    last_chat: Option<String>,
    last_channel: Option<(String, String)>,
    conv: ConversationState,
    chat_cursor: Option<String>,
    more_chats: Tracked<bool, { feeds::SHELL_SIDEBAR }>,
    loading_workspace: Tracked<bool, { feeds::SHELL }>,
    ticks: u32,
    refresh_mode: RefreshMode,
    error: Tracked<Option<String>, { feeds::SHELL }>,
    notice: Tracked<Option<String>, { feeds::SHELL }>,
    participants_notice: Tracked<Option<String>, { feeds::SHELL }>,
    new_chat: Tracked<Option<NewChat>, { feeds::NEW_CHAT }>,
    /// Lookup cache for `known_name`, rebuilt lazily after every update.
    roster_names: std::sync::OnceLock<HashMap<String, Option<String>>>,
    /// Parts marked by hand: what changes outside the cells (avatar cache) or with the time.
    marked: Parts,
}
#[derive(Debug, Clone)]
pub(crate) enum Msg {
    TeamList(team_list::TeamListMsg),
    Conversation(ConversationMsg),
    /// New messages of this chat are merged: they update its row and mark it read.
    FeedMerged(Selection),
    ShowError(String),
    Activity(ActivityMsg),
    LoadAvatar(String, String),
    BotAppLoaded {
        generation: u64,
        id: String,
        result: Result<Vec<teams_chat::AppDefinition>, String>,
    },
    AvatarLoaded {
        generation: u64,
        id: String,
        result: Result<media::LoadedImage, ()>,
    },
    Calendar(CalendarMsg),
    Calls(CallsMsg),
    Call(CallMsg),
    Files(FilesMsg),
    Notifications(NotificationsMsg),
    Me(MeMsg),
    ChatAdmin(ChatAdminMsg),
    TeamsAdmin(TeamsAdminMsg),
    Gifs(GifsMsg),
    Extensions(ExtensionsMsg),
    LinkPreview(LinkPreviewMsg),
    Card(CardMsg),
    FullSearch(SearchMsg),
    Receipts(receipts::ReceiptsMsg),
    SignIn,
    LoginReturned(String),
    LoginCancelled,
    SilentLoginFailed,
    /// `Some`: signed in from scratch (at launch); `None`: the live session took the renewed credentials.
    SilentRenewed(u64, Result<Option<Arc<TeamsAuthenticator>>, CoreError>),
    SignInBrowser,
    CopyLoginUrl,
    BrowserOpened(Result<(), String>),
    CallbackChanged(String),
    CompleteSignIn,
    SignedIn(Result<Arc<TeamsAuthenticator>, CoreError>),
    SignOut,
    SessionRestored(u64, Result<Option<Arc<TeamsAuthenticator>>, CoreError>),
    SessionRemembered(u64, Result<(), String>),
    MyProfileLoaded(u64, Result<Person, String>),
    Tick,
    /// The window came back to the front.
    Foreground,
    SetRefreshMode(RefreshMode),
    Refresh,
    Navigate(Nav),
    SetFilter(Filter),
    SetTab(Tab),
    Search(String),
    DismissError,
    DismissNotice(String),
    WorkspaceLoaded(u64, Result<Workspace, CoreError>),
    ChatsResolved(u64, conversations::Resolution),
    Select(Selection),
    ToggleTeam(String),
    LoadChannels(String),
    ChannelMembers,
    MembersLoaded {
        generation: u64,
        /// `Selection::key` of the conversation.
        key: String,
        result: Result<(Vec<teams_chat::Member>, Vec<Person>), String>,
    },
    ChannelsLoaded {
        generation: u64,
        team: String,
        /// The channels, and why some may be missing.
        result: Result<(Vec<Channel>, Option<String>), String>,
    },
    MoreChats,
    MoreChatsLoaded(u64, Result<ChatsPage, String>),
    DraftChanged(String),
    RichDraftChanged(String, Vec<formatting::ComposerFormat>),
    ReplaceDraft {
        start: u32,
        length: u32,
        text: String,
        mention: Option<String>,
        emoji: Option<String>,
        style: Option<formatting::ComposerFormat>,
    },
    Send,
    Sent {
        generation: u64,
        selection: Selection,
        /// Root post of the thread it answers.
        reply: Option<String>,
        /// The draft sent.
        content: String,
        /// Attachments sent.
        ids: Vec<String>,
        result: Result<(), String>,
    },
    NewChat,
    CancelNewChat,
    PeopleQuery(String),
    PeopleSearch,
    PeopleLoaded {
        generation: u64,
        query: String,
        result: Result<Vec<Person>, String>,
    },
    ChoosePerson(Person),
    RemovePerson(String),
    NewTitle(String),
    CreateChat,
    ChatCreated {
        generation: u64,
        people: Vec<Person>,
        title: String,
        result: Result<String, String>,
    },
    OpenUrl(String),
    UrlOpened(Result<(), String>),
    MarkRead(Selection, String),
    ReadMarked(u64, Result<(), String>),
    ProfilesLoaded(u64, Result<Vec<Person>, String>),
    PresenceLoaded(u64, Result<Vec<Presence>, String>),
}
impl App {
    fn empty() -> Self {
        Self {
            team_list: Default::default(),
            composer: Default::default(),
            activity: Default::default(),
            avatars: Default::default(),
            handoff: Default::default(),
            calendar: Tracked::new(CalendarState::default()),
            calls: Tracked::new(CallsState::default()),
            call: Tracked::new(CallState::default()),
            files: Tracked::new(FilesState::default()),
            notifications: Tracked::new(NotificationsState::default()),
            me_state: Default::default(),
            chat_admin: Default::default(),
            teams_admin: Default::default(),
            gifs: Default::default(),
            extensions: Default::default(),
            link_preview: Default::default(),
            search_state: Default::default(),
            card_states: Default::default(),
            favorites_folder: None,
            pinned_order: vec![],
            hidden_chats: Default::default(),
            kept_unread: HashSet::new(),
            receipts: Default::default(),
            client: Tracked::new(None),
            saved_auth: None,
            me: Tracked::new(String::new()),
            display_name: Tracked::new(String::new()),
            account_email: Tracked::new(String::new()),
            login: Tracked::new(None),
            window_login: None,
            silent_login: None,
            renewal_retry: None,
            login_url: Tracked::new(String::new()),
            callback: Tracked::new(String::new()),
            signing_in: Tracked::new(false),
            generation: 0,
            nav: Tracked::new(Nav::Chat),
            filter: Tracked::new(Filter::All),
            tab: Tracked::new(Tab::Messages),
            search: Tracked::new(String::new()),
            chats: Tracked::new(vec![]),
            favorites: Tracked::new(None),
            profiles: Tracked::new(Arc::default()),
            bot_apps: Tracked::new(HashMap::new()),
            details: Tracked::new(HashMap::new()),
            resolved_ids: HashSet::new(),
            resolving_chats: false,
            teams: Tracked::new(vec![]),
            teams_problem: Tracked::new(None),
            channels_loading: Tracked::new(HashSet::new()),
            channels_loaded: HashSet::new(),
            channel_errors: Tracked::new(HashMap::new()),
            channel_members: Tracked::new(HashMap::new()),
            members_loading: Tracked::new(HashSet::new()),
            members_errors: Tracked::new(HashMap::new()),
            profiles_requested: HashSet::new(),
            reactions_scanned: HashSet::new(),
            clock_minute: 0,
            presence: Tracked::new(HashMap::new()),
            presence_asked: None,
            presence_asked_for: HashSet::new(),
            expanded: Tracked::new(HashSet::new()),
            selected: Tracked::new(None),
            last_chat: None,
            last_channel: None,
            conv: ConversationState::default(),
            chat_cursor: None,
            more_chats: Tracked::new(false),
            loading_workspace: Tracked::new(false),
            ticks: 0,
            refresh_mode: RefreshMode::Push,
            error: Tracked::new(None),
            notice: Tracked::new(None),
            participants_notice: Tracked::new(None),
            new_chat: Tracked::new(None),
            roster_names: Default::default(),
            marked: Parts::NONE,
        }
    }
    pub(crate) fn boot(demo: bool) -> (Self, Task<Msg>) {
        let mut app = Self::empty();
        let task = app.start(demo);
        (app, task)
    }
    /// Connects a signed-out app: to the demo service, or through the saved session.
    fn start(&mut self, demo: bool) -> Task<Msg> {
        let app = self;
        #[cfg(not(feature = "demo"))]
        let _ = demo;
        #[cfg(feature = "demo")]
        if demo {
            let addr = mock::spawn();
            let base = Url::parse(&format!("http://{addr}/")).expect("mock URL");
            let auth = Arc::new(AppAuthenticator::Demo(
                StaticAuthenticator::new()
                    .with_service(Service::ChatService, base.clone())
                    .with_service(Service::Csa, base.clone())
                    .with_service(Service::MiddleTier, base.clone())
                    .with_service(Service::Calendar, base.clone())
                    .with_service(Service::Outlook, base.clone())
                    .with_service(Service::Substrate, base.clone())
                    .with_service(Service::Uis, base.clone())
                    .with_service(Service::SubstrateSearch, base.clone())
                    .with_service(Service::UnifiedPresence, base.clone())
                    .with_resource(&base)
                    .with_service(Service::Trouter, crate::calls_mock::trouter().trouter_url())
                    .with_service(
                        Service::Registrar,
                        crate::calls_mock::trouter().registrar_url(),
                    )
                    .with_service(
                        Service::CallingConversation,
                        crate::calls_mock::conversation().url(),
                    )
                    .with_service(Service::UrlPreview, base.clone())
                    .with_service(Service::Ams, base),
            ));
            *app.client = Some(ChatClient::new(auth));
            *app.me = mock::ME.into();
            *app.display_name = "John Doe".into();
        }
        if app.client.is_some() {
            Task::batch([
                app.load_workspace(),
                app.sync_messaging(),
                app.publish_presence_endpoint(),
            ])
        } else {
            *app.signing_in = true;
            let generation = app.generation;
            Task::perform(session::restore(), move |r| {
                Msg::SessionRestored(generation, r)
            })
        }
    }
    /// Back to a signed-out state keeping device settings; the new generation drops earlier work's results.
    fn reset(&mut self) {
        let previous_conv = std::mem::take(&mut self.conv);
        let generation = self.generation + 1;
        let refresh_mode = self.refresh_mode;
        let permission = self.notifications_permission();
        // The core hands out this cache by reference: empty it rather than replace it.
        let avatars = self.avatars.clone();
        *avatars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Default::default();
        let handoff = self.handoff.clone();
        *self.handoff() = Default::default();
        *self = Self::empty();
        self.avatars = avatars;
        self.handoff = handoff;
        self.refresh_mode = refresh_mode;
        self.restore_notifications_permission(permission);
        self.generation = generation;
        self.conv = ConversationState::after(&previous_conv);
        // Every cell is new: Swift must see all of them again.
        self.mark(Parts::ALL);
    }
    /// Starts over after a panic left the state unknown, then signs back in from the saved session.
    pub(crate) fn recover(&mut self, demo: bool) -> Task<Msg> {
        self.reset();
        *self.error = Some(t("core.app.an_internal_error_occurred_the_app").into());
        self.start(demo)
    }
    pub(crate) fn handoff(&self) -> std::sync::MutexGuard<'_, media::Handoff> {
        self.handoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    #[cfg(test)]
    /// A preview is ready: its pixels wait in the hand-off for Swift, its size stays here.
    pub(crate) fn image_ready(&mut self, src: String, mut image: media::LoadedImage) {
        self.handoff().offer(src.clone(), &mut image);
        self.conv.image_ready(src, image);
    }
    #[cfg(test)]
    pub(crate) fn me_view(&self) -> me::MeView {
        self.me_state.view.clone()
    }
    #[cfg(test)]
    pub(crate) fn gifs_view(&self) -> gifs::GifsView {
        self.gifs.view.clone()
    }
    #[cfg(test)]
    pub(crate) fn extensions_view(&self) -> extensions::ExtensionsView {
        self.extensions.view.clone()
    }
    #[cfg(test)]
    pub(crate) fn teams_admin_view(&self) -> teams_admin::TeamsAdminView {
        self.teams_admin.view.clone()
    }
    #[cfg(test)]
    pub(crate) fn search_view(&self) -> search::SearchView {
        self.search_state.view.clone()
    }
    #[cfg(test)]
    pub(crate) fn chat_admin_error(&self) -> Option<String> {
        self.chat_admin.view.error.clone()
    }
    #[cfg(test)]
    pub(crate) fn error_text(&self) -> Option<String> {
        self.error.clone()
    }
    #[cfg(test)]
    pub(crate) fn first_chat_id(&self) -> String {
        self.chats.iter().find(|c| c.id != NOTES).map(|c| c.id.clone()).unwrap_or_default()
    }
    #[cfg(test)]
    pub(crate) fn measured(&self) -> (u64, usize) {
        (self.generation, self.conv.messages().len())
    }
    /// Whose photo an avatar shows: a person or bot by MRI, or a direct chat's peer.
    pub(crate) fn avatar_mri(&self, id: &str) -> Option<String> {
        let has_photo = |mri: &str| mri.starts_with("8:") || mri.starts_with("28:");
        if has_photo(id) {
            return Some(id.to_owned());
        }
        self.chats
            .iter()
            .find(|c| c.id == id && !c.is_group())
            .and_then(|c| {
                c.members
                    .iter()
                    .find(|m| !m.id.eq_ignore_ascii_case(&self.me))
            })
            .map(|m| m.id.clone())
            .filter(|mri| has_photo(mri))
    }
    /// Marks parts whose change no cell sees: the avatar cache, or what shows the time.
    pub(crate) fn mark(&mut self, parts: Parts) {
        self.marked |= parts;
    }
    /// Rebuilds the ready-made views in the new language; the caller then republishes every part.
    pub(crate) fn relocalize(&mut self) {
        self.conv.relocalize();
        self.calendar.relocalize();
        // The catalog names apps in the language asked for.
        self.bot_apps.clear();
        for team in self.teams.iter_mut() {
            let id = team.id.clone();
            for channel in team
                .channels
                .iter_mut()
                .filter(|c| c.is_general && c.id == id)
            {
                channel.display_name = general_channel().into();
            }
        }
        self.relocalize_calls();
        self.relocalize_files();
        self.relocalize_notifications();
        self.relocalize_me();
    }
    /// The parts changed since the previous call, for the next diff.
    pub(crate) fn take_changes(&mut self) -> Parts {
        // Exhaustive on purpose: a new field must say which parts it feeds, if any.
        let Self {
            team_list,
            composer,
            activity,
            avatars: _,
            handoff: _,
            calendar,
            calls,
            call,
            files,
            notifications,
            me_state,
            chat_admin,
            teams_admin,
            gifs,
            extensions,
            link_preview,
            search_state,
            card_states,
            favorites_folder: _,
            pinned_order: _,
            hidden_chats,
            kept_unread: _,
            receipts,
            client,
            saved_auth: _,
            me,
            display_name,
            account_email,
            login,
            window_login: _,
            silent_login: _,
            renewal_retry: _,
            login_url,
            callback,
            signing_in,
            generation: _,
            nav,
            filter,
            tab,
            search,
            chats,
            favorites,
            profiles,
            bot_apps,
            details,
            resolved_ids: _,
            resolving_chats: _,
            teams,
            teams_problem,
            channels_loading,
            channels_loaded: _,
            channel_errors,
            channel_members,
            members_loading,
            members_errors,
            profiles_requested: _,
            reactions_scanned: _,
            clock_minute: _,
            presence,
            presence_asked: _,
            presence_asked_for: _,
            expanded,
            selected,
            last_chat: _,
            last_channel: _,
            conv,
            chat_cursor: _,
            more_chats,
            loading_workspace,
            ticks: _,
            refresh_mode: _,
            error,
            notice,
            participants_notice,
            new_chat,
            roster_names: _,
            marked,
        } = self;
        let cells = [
            team_list.take_changes(),
            composer.take_changes(),
            activity.take_changes(),
            calendar.take_changes(),
            calls.take_changes(),
            call.take_changes(),
            files.take_changes(),
            notifications.take_changes(),
            me_state.take_changes(),
            chat_admin.take_changes(),
            teams_admin.take_changes(),
            gifs.take_changes(),
            extensions.take_changes(),
            link_preview.take_changes(),
            search_state.take_changes(),
            card_states.take_changes(),
            hidden_chats.take_changes(),
            receipts.take_changes(),
            client.take_changes(),
            me.take_changes(),
            display_name.take_changes(),
            account_email.take_changes(),
            login.take_changes(),
            login_url.take_changes(),
            callback.take_changes(),
            signing_in.take_changes(),
            nav.take_changes(),
            filter.take_changes(),
            tab.take_changes(),
            search.take_changes(),
            chats.take_changes(),
            favorites.take_changes(),
            profiles.take_changes(),
            bot_apps.take_changes(),
            details.take_changes(),
            teams.take_changes(),
            teams_problem.take_changes(),
            channels_loading.take_changes(),
            channel_errors.take_changes(),
            channel_members.take_changes(),
            members_loading.take_changes(),
            members_errors.take_changes(),
            presence.take_changes(),
            expanded.take_changes(),
            selected.take_changes(),
            more_chats.take_changes(),
            loading_workspace.take_changes(),
            error.take_changes(),
            notice.take_changes(),
            participants_notice.take_changes(),
            new_chat.take_changes(),
            conv.take_changes(),
        ];
        cells
            .into_iter()
            .fold(std::mem::take(marked), |all, parts| all | parts)
    }
    pub(crate) fn signed_in(&self) -> bool {
        self.client.is_some()
    }
    /// Bumped by sign-in, sign-out and recovery: work started before belongs to a session that is gone.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn update(&mut self, msg: impl Into<Msg>) -> Task<Msg> {
        // Follow-ups apply right after their message and before later ones: depth first, as nested calls would.
        let mut pending = vec![msg.into()];
        let mut jobs = Vec::new();
        while let Some(msg) = pending.pop() {
            // The name index must not outlive roster changes: drop it before each handler and before follow-up loads.
            self.roster_names.take();
            let mut follow_ups = Vec::new();
            for job in self.handle(msg).0 {
                match job {
                    Job::Done(msg) => follow_ups.push(msg),
                    job => jobs.push(job),
                }
            }
            pending.extend(follow_ups.into_iter().rev());
        }
        self.roster_names.take();
        self.calls.in_call.set(self.call_in_progress());
        if !self.call_in_progress() && self.notifications.muted_in_call() {
            // "Mute notifications during this meeting" ends with the call.
            self.notifications.end_call();
        }
        Task::batch([
            Task(jobs),
            self.refresh_link_preview(),
            self.load_images(),
            self.load_visible_channels(),
            self.resolve_people(),
            self.load_notification_settings(),
            self.load_receipts(),
            self.client
                .clone()
                .map(|client| self.avatars.lock().unwrap().jobs(client, self.generation))
                .unwrap_or_else(Task::none),
        ])
    }
    /// The open conversation, and the context it reads from the rest of the app.
    fn conversation(&mut self) -> (&mut ConversationState, Ctx<'_>) {
        (&mut self.conv, ctx!(self))
    }
    /// Whether ticks reload from the network: when polling, or while the chat push channel is not registered.
    fn polls(&self) -> bool {
        self.refresh_mode == RefreshMode::Polling || !self.notifications.push_live()
    }
    /// Starts a `prompt=none` sign-in in the invisible web view, unless one is running.
    fn renew_silently(&mut self) -> Task<Msg> {
        if self.silent_login.is_some() {
            return Task::none();
        }
        let hint = self.saved_auth.as_ref().and_then(|a| a.login_hint());
        // Restoring the session read the application ID, so this only fails without one.
        let Ok(login) = LoginAttempt::silent(hint) else {
            return self.silent_failed();
        };
        let url = login.authorization_url().to_string();
        self.silent_login = Some(login);
        Task::effect(Effect::SilentLogin { url })
    }
    /// Reads the application ID from the web client's configuration, then handles `then`; failures show as a failed sign-in.
    fn with_client_id(then: Msg) -> Task<Msg> {
        Task::perform(teams_auth::client_id::resolve(), move |r| match r {
            Ok(_) => then,
            Err(e) => Msg::SignedIn(Err(e.into())),
        })
    }
    /// A SPA refresh token dies 24 h after sign-in: renew it from the Microsoft cookies an hour before, or once refused.
    fn renew_if_due(&mut self) -> Task<Msg> {
        const MARGIN: Duration = Duration::from_secs(3600);
        const RETRY: Duration = Duration::from_secs(15 * 60);
        let now = std::time::Instant::now();
        let due = self.client.is_some()
            && self.window_login.is_none()
            && self.renewal_retry.is_none_or(|at| now >= at)
            && self
                .saved_auth
                .as_ref()
                .is_some_and(|a| a.needs_renewal(MARGIN));
        if !due {
            return Task::none();
        }
        self.renewal_retry = Some(now + RETRY);
        self.renew_silently()
    }
    fn silent_returned(&mut self, callback: String) -> Task<Msg> {
        let Some(login) = self.silent_login.take() else {
            return Task::none();
        };
        let current = self.saved_auth.clone().filter(|_| self.client.is_some());
        let generation = self.generation;
        Task::perform(
            async move {
                let renewed = login.complete(&callback).await.map_err(CoreError::from)?;
                match current {
                    Some(current) => {
                        current.adopt(renewed).await.map_err(CoreError::from)?;
                        Ok(None)
                    }
                    None => Ok(Some(Arc::new(renewed))),
                }
            },
            move |r| Msg::SilentRenewed(generation, r),
        )
    }
    /// At launch the sign-in page takes over; a live session retries later until the refresh token is gone.
    fn silent_failed(&mut self) -> Task<Msg> {
        let expired = t("core.app.your_microsoft_session_has_expired_please");
        if self.client.is_none() {
            *self.signing_in = false;
            *self.error = Some(expired.into());
        } else if self
            .saved_auth
            .as_ref()
            .is_some_and(|a| a.needs_renewal(Duration::ZERO))
        {
            self.reset();
            *self.error = Some(expired.into());
        }
        Task::none()
    }
    /// Forgets errors and lookups and reloads the chats, teams, open feed and own profile.
    fn reload_workspace(&mut self) -> Task<Msg> {
        *self.participants_notice = None;
        self.resolved_ids.clear();
        self.details.clear();
        *self.error = None;
        self.channels_loaded.clear();
        self.channel_errors.clear();
        self.members_errors.clear();
        Task::batch([
            self.load_workspace(),
            self.load_feed(false),
            self.load_my_profile(),
        ])
    }
    /// Periodic refresh; with a live push channel it only redraws the time, and coming to the front reloads the open conversation.
    fn tick(&mut self, foreground: bool) -> Task<Msg> {
        let renewal = self.renew_if_due();
        let active = self.notifications.active;
        Task::batch([
            renewal,
            self.presence_heartbeat(active),
            self.load_presence(),
            self.refresh_views(foreground),
        ])
    }
    /// Polls `getpresence` even with push, as the web client polls what Trouter cannot subscribe.
    /// People not asked for last time, such as the authors of a chat just opened, are asked for at once.
    fn load_presence(&mut self) -> Task<Msg> {
        const EVERY: Duration = Duration::from_secs(120);
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let mris = self.presence_targets();
        let recent = self.presence_asked.is_some_and(|at| at.elapsed() < EVERY);
        if mris.is_empty() || recent && mris.iter().all(|m| self.presence_asked_for.contains(m)) {
            return Task::none();
        }
        self.presence_asked = Some(std::time::Instant::now());
        self.presence_asked_for = mris.iter().cloned().collect();
        let generation = self.generation;
        Task::perform(
            async move { client.presences(&mris).await.map_err(|e| e.to_string()) },
            move |result| Msg::PresenceLoaded(generation, result),
        )
    }
    /// The other person of each direct chat, the user (the status menu shows them), and in the open
    /// conversation the authors of its messages and the members of a group chat. Channel rosters
    /// can hold a whole organisation: their members are left out.
    pub(crate) fn presence_targets(&self) -> Vec<String> {
        let open_chat = match &*self.selected {
            Some(Selection::Chat(id)) => self.chats.iter().find(|c| &c.id == id && c.is_group()),
            _ => None,
        };
        let mut mris: Vec<String> = self
            .chats
            .iter()
            .filter_map(|c| chat_peer(c, &self.me))
            .chain(std::iter::once(self.me.as_str()))
            .chain(self.conv.all_messages().map(|m| m.sender_mri()))
            .chain(
                open_chat
                    .into_iter()
                    .flat_map(|c| c.members.iter().map(|m| m.id.as_str())),
            )
            .filter(|mri| mri.starts_with("8:"))
            .map(str::to_ascii_lowercase)
            .collect();
        mris.sort();
        mris.dedup();
        mris
    }
    fn refresh_views(&mut self, foreground: bool) -> Task<Msg> {
        // What moves with time: a pause may have ended, a typer timed out, the calendar, calls and loading flags.
        self.mark(Parts::SHELL | Parts::TYPING | Parts::NOTIFICATIONS);
        // Times and dates shown in lists change by the minute: rebuilding them every tick only costs CPU.
        let minute = chrono::Utc::now().timestamp() / 60;
        if minute != self.clock_minute {
            self.clock_minute = minute;
            self.mark(Parts::SIDEBAR | Parts::CONVERSATION | Parts::CALENDAR | Parts::CALLS);
        }
        // With a live push channel these screens reload only on coming to the front; pushes refresh them in between.
        let reload = foreground || self.polls();
        match *self.nav {
            Nav::Calendar | Nav::Calls if !reload => return Task::none(),
            Nav::Calendar => return self.load_calendar(false),
            Nav::Calls => return self.load_calls(false),
            _ => {}
        }
        if let Some(error) = self.saved_auth.as_ref().and_then(|a| a.persistence_error()) {
            *self.notice = Some(error);
        }
        if !self.polls() {
            return if foreground {
                self.load_feed(false)
            } else {
                Task::none()
            };
        }
        self.ticks += 1;
        let feed = self.load_feed(false);
        if self.ticks % 6 == 0 {
            self.channels_loaded
                .retain(|id| !self.expanded.contains(id));
            Task::batch([feed, self.load_workspace(), self.load_activity(false)])
        } else {
            feed
        }
    }
    /// Reloads what pushes may have missed: open conversation, chat list, activity, and the open calendar or calls screen.
    pub(crate) fn catch_up(&mut self) -> Task<Msg> {
        Task::batch([
            self.load_feed(false),
            self.load_workspace(),
            self.load_activity(false),
            self.load_calendar(true),
            self.load_calls(true),
        ])
    }
    fn load_feed(&mut self, append: bool) -> Task<Msg> {
        let (conv, ctx) = self.conversation();
        conv.load_feed(&ctx, append)
    }
    fn load_images(&mut self) -> Task<Msg> {
        let draft_image = self.draft_link_image();
        let (conv, ctx) = self.conversation();
        conv.load_images(&ctx, draft_image)
    }
    fn draft_key(&self) -> String {
        self.conv.draft_key(self.selected.as_ref())
    }
    fn handle(&mut self, msg: impl Into<Msg>) -> Task<Msg> {
        let msg = msg.into();
        match msg {
            Msg::TeamList(msg) => self.update_team_list(msg),
            Msg::Conversation(msg) => {
                let (conv, ctx) = self.conversation();
                conv.update(&ctx, msg)
            }
            Msg::ShowError(error) => {
                *self.error = Some(error);
                Task::none()
            }
            Msg::Activity(msg) => self.update_activity(msg),
            Msg::Calendar(msg) => self.update_calendar(msg),
            Msg::Calls(msg) => self.update_calls(msg),
            Msg::Call(msg) => self.update_call(msg),
            Msg::Files(msg) => self.update_files(msg),
            Msg::Notifications(msg) => self.update_notifications(msg),
            Msg::Me(msg) => self.update_me(msg),
            Msg::ChatAdmin(msg) => self.update_chat_admin(msg),
            Msg::TeamsAdmin(msg) => self.update_teams_admin(msg),
            Msg::Gifs(msg) => self.update_gifs(msg),
            Msg::Extensions(msg) => self.update_extensions(msg),
            Msg::LinkPreview(msg) => self.update_link_preview(msg),
            Msg::Card(msg) => self.update_card(msg),
            Msg::FullSearch(msg) => self.update_search(msg),
            Msg::Receipts(msg) => self.update_receipts(msg),
            Msg::CopyLoginUrl => Task::effect(Effect::Copy {
                value: self.login_url.clone(),
            }),
            Msg::SignIn => {
                if *self.signing_in || self.client.is_some() {
                    return Task::none();
                }
                let Ok(login) = LoginAttempt::new() else {
                    return Self::with_client_id(Msg::SignIn);
                };
                *self.login = None;
                self.callback.clear();
                self.login_url.clear();
                *self.signing_in = true;
                *self.error = None;
                let url = login.authorization_url().to_string();
                self.window_login = Some(login);
                Task::effect(Effect::OpenLogin { url })
            }
            Msg::LoginReturned(callback) => {
                let Some(login) = self.window_login.take() else {
                    return self.silent_returned(callback);
                };
                Task::batch([Task::effect(Effect::CloseLogin), complete_login(login, callback)])
            }
            Msg::LoginCancelled => {
                if self.window_login.take().is_some() {
                    *self.signing_in = false;
                    *self.error = Some(t("core.app.sign_in_cancelled").into());
                }
                Task::none()
            }
            Msg::SilentLoginFailed => {
                if self.silent_login.take().is_some() {
                    return self.silent_failed();
                }
                Task::none()
            }
            Msg::SilentRenewed(g, _) if g != self.generation => Task::none(),
            Msg::SilentRenewed(_, Ok(Some(auth))) => Task::done(Msg::SignedIn(Ok(auth))),
            Msg::SilentRenewed(_, Ok(None)) => {
                self.renewal_retry = None;
                // Requests refused before the renewal left an error and missing names: start again from a clean state.
                self.reload_workspace()
            }
            Msg::SilentRenewed(_, Err(_)) => self.silent_failed(),
            Msg::SignInBrowser => {
                if *self.signing_in || self.client.is_some() {
                    return Task::none();
                }
                let Ok(login) = LoginAttempt::new() else {
                    return Self::with_client_id(Msg::SignInBrowser);
                };
                let url = login.authorization_url().to_string();
                *self.login_url = url.clone();
                *self.login = Some(login);
                self.callback.clear();
                *self.error = None;
                Task::perform(blocking(move || open_browser(&url)), Msg::BrowserOpened)
            }
            Msg::BrowserOpened(result) => {
                if self.client.is_none() {
                    *self.error = result.err();
                }
                Task::none()
            }
            Msg::CallbackChanged(value) => {
                *self.callback = value;
                Task::none()
            }
            Msg::CompleteSignIn => {
                let Some(login) = self.login.take() else {
                    return Task::none();
                };
                let callback = std::mem::take(&mut *self.callback);
                self.login_url.clear();
                *self.signing_in = true;
                *self.error = None;
                complete_login(login, callback)
            }
            Msg::SignedIn(result) => {
                *self.signing_in = false;
                match result {
                    Ok(auth) => {
                        self.generation += 1;
                        *self.me = auth.user_id().to_owned();
                        *self.display_name = auth.display_name().to_owned();
                        self.saved_auth = Some(auth.clone());
                        *self.client = Some(ChatClient::new(Arc::new(
                            AppAuthenticator::Microsoft(auth.clone()),
                        )));
                        *self.error = None;
                        let generation = self.generation;
                        let save = Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || session::remember(&auth))
                                    .await
                                    .map_err(|_| {
                                        t("core.app.session_saving_unavailable").to_owned()
                                    })?
                            },
                            move |r| Msg::SessionRemembered(generation, r),
                        );
                        Task::batch([
                            self.load_workspace(),
                            self.load_my_profile(),
                            self.load_activity(false),
                            save,
                            self.sync_realtime(),
                            self.sync_messaging(),
                            self.publish_presence_endpoint(),
                        ])
                    }
                    Err(error) => {
                        *self.error = Some(error.into());
                        Task::none()
                    }
                }
            }
            Msg::SignOut => {
                let auth = self.saved_auth.take();
                // The presence endpoint ends with the session (web client's onStop); its answer is dropped with this generation.
                let ended = self.generation;
                let endpoint = self
                    .client
                    .clone()
                    .zip(self.presence_endpoint())
                    .map(|(client, endpoint)| {
                        Task::perform(
                            async move { client.delete_presence_endpoint(&endpoint).await.ok() },
                            move |_| Msg::ReadMarked(ended, Ok(())),
                        )
                    })
                    .unwrap_or_else(Task::none);
                self.reset();
                crate::disk_cache::clear();
                let generation = self.generation;
                let forget = Task::batch([Task::effect(Effect::ForgetLogin), endpoint]);
                if let Some(auth) = auth {
                    let task = Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                auth.forget_session().map_err(|e| e.to_string())
                            })
                            .await
                            .map_err(|_| t("core.app.session_removal_unavailable").to_owned())?
                        },
                        move |r| Msg::SessionRemembered(generation, r),
                    );
                    Task::batch([forget, task])
                } else {
                    forget
                }
            }
            Msg::SessionRestored(g, _) | Msg::SessionRemembered(g, _) if g != self.generation => {
                Task::none()
            }
            Msg::SessionRestored(_, result) => {
                *self.signing_in = false;
                match result {
                    Ok(Some(auth)) => Task::done(Msg::SignedIn(Ok(auth))),
                    Ok(None) => Task::none(),
                    // The saved refresh token is over, but the Microsoft cookies may still sign in without a page, as the web client does.
                    Err(CoreError::SessionExpired(_)) => {
                        *self.signing_in = true;
                        self.renew_silently()
                    }
                    Err(e) => {
                        *self.error = Some(e.into());
                        Task::none()
                    }
                }
            }
            Msg::SessionRemembered(_, result) => {
                if let Err(e) = result {
                    *self.error = Some(e);
                }
                Task::none()
            }
            Msg::AvatarLoaded { generation: g, .. }
            | Msg::BotAppLoaded { generation: g, .. }
            | Msg::ChatsResolved(g, _)
            | Msg::WorkspaceLoaded(g, _)
            | Msg::ChannelsLoaded { generation: g, .. }
            | Msg::MembersLoaded { generation: g, .. }
            | Msg::MoreChatsLoaded(g, _)
            | Msg::Sent { generation: g, .. }
            | Msg::PeopleLoaded { generation: g, .. }
            | Msg::ChatCreated { generation: g, .. }
            | Msg::ReadMarked(g, _)
            | Msg::ProfilesLoaded(g, _)
            | Msg::PresenceLoaded(g, _)
            | Msg::MyProfileLoaded(g, _)
                if g != self.generation =>
            {
                Task::none()
            }
            Msg::Tick => self.tick(false),
            Msg::Foreground => self.tick(true),
            Msg::SetRefreshMode(mode) => {
                self.refresh_mode = mode;
                Task::none()
            }
            Msg::Refresh => {
                match *self.nav {
                    Nav::Activity => return self.load_activity(false),
                    Nav::Calendar => return self.load_calendar(true),
                    Nav::Calls => return self.load_calls(true),
                    Nav::Files => {
                        return self.update_files(FilesMsg::Action(files::FileAction::RefreshBrowser));
                    }
                    _ => {}
                }
                let files = match (*self.tab, &*self.selected) {
                    (Tab::Files, Some(Selection::Chat(_))) => self.load_shared(true),
                    (Tab::Files, Some(Selection::Channel { .. })) => {
                        self.update_files(FilesMsg::Action(files::FileAction::RefreshBrowser))
                    }
                    _ => Task::none(),
                };
                Task::batch([files, self.reload_workspace()])
            }
            Msg::Navigate(nav) => {
                if nav == Nav::Activity && *self.nav != Nav::Activity {
                    // The list opens at its top, on the newest items.
                    self.activity.paged = false;
                }
                *self.nav = nav;
                if nav == Nav::Activity {
                    *self.new_chat = None;
                    return self.load_activity(false);
                }
                if nav == Nav::Calendar {
                    return self.load_calendar(false);
                }
                if nav == Nav::Calls {
                    return self.load_calls(false);
                }
                if nav == Nav::Files {
                    *self.new_chat = None;
                    let place = match self.files.browser_place() {
                        files::BrowserPlace::Channel => files::BrowserPlace::Recent,
                        place => place,
                    };
                    return self.browse(place);
                }
                *self.new_chat = None;
                // The Files screen and a channel's files tab share the browser: take it back.
                if nav == Nav::Teams
                    && *self.tab == Tab::Files
                    && self.selected.as_ref().is_some_and(Selection::is_channel)
                    && self.files.browser_place() != files::BrowserPlace::Channel
                {
                    return self.open_channel_browser();
                }
                let target = match nav {
                    Nav::Teams if !self.selected.as_ref().is_some_and(Selection::is_channel) => {
                        self.last_channel
                            .clone()
                            .or_else(|| self.first_shown_channel())
                            .map(|(team, channel)| Selection::Channel { team, channel })
                    }
                    Nav::Chat if self.selected.as_ref().is_some_and(Selection::is_channel) => self
                        .last_chat
                        .clone()
                        .or_else(|| self.chats.first().map(|c| c.id.clone()))
                        .map(Selection::Chat),
                    _ => None,
                };
                if let Some(target) = target {
                    return Task::done(Msg::Select(target));
                }
                Task::none()
            }
            Msg::SetFilter(filter) => {
                if *self.nav == Nav::Activity {
                    self.activity.unread_only = filter == Filter::Unread;
                    return Task::none();
                }
                *self.filter = filter;
                Task::none()
            }
            Msg::SetTab(tab) => {
                *self.tab = tab;
                if tab == Tab::Members {
                    return Task::done(Msg::ChannelMembers);
                }
                if tab == Tab::Files {
                    return match *self.selected {
                        Some(Selection::Channel { .. }) => self.open_channel_browser(),
                        _ => self.load_shared(false),
                    };
                }
                Task::none()
            }
            Msg::Search(query) => {
                *self.search = query;
                Task::none()
            }
            Msg::DismissError => {
                *self.error = None;
                Task::none()
            }
            Msg::DismissNotice(text) => {
                if self.notice.as_deref() == Some(text.as_str()) {
                    *self.notice = None;
                }
                if self.participants_notice.as_deref() == Some(text.as_str()) {
                    *self.participants_notice = None;
                }
                Task::none()
            }
            Msg::WorkspaceLoaded(_, result) => {
                *self.loading_workspace = false;
                match result {
                    Ok(workspace) => {
                        if let Some(preferences) = &workspace.preferences {
                            self.preferences_loaded(preferences);
                            self.receipts_preferences(preferences);
                        }
                        if workspace.favorites.is_some() {
                            *self.favorites = workspace.favorites;
                        }
                        self.favorites_folder = workspace.favorites_folder;
                        if !workspace.favorite_order.is_empty() {
                            self.pinned_order = workspace.favorite_order;
                        }
                        *self.notice = workspace.warning;
                        *self.teams_problem = workspace.teams_problem;
                        Arc::make_mut(&mut self.profiles).extend(
                            workspace
                                .users
                                .into_iter()
                                .filter(|p| !p.mri.is_empty())
                                .map(|p| (p.mri.clone(), p)),
                        );
                        let mut chats = workspace.chats;
                        // Indexed once: hundreds of chats with many members each on large tenants.
                        let known: HashMap<&str, &Conversation> = self
                            .chats
                            .iter()
                            .rev()
                            .map(|c| (c.id.as_str(), c))
                            .collect();
                        let mut by_case = None;
                        for chat in &mut chats {
                            if let Some(old) = known.get(chat.id.as_str())
                                && let Some(message) = &old.last_message
                                && conversations::message_time(message)
                                    > chat
                                        .last_message
                                        .as_ref()
                                        .and_then(conversations::message_time)
                            {
                                conversations::update_last_message(chat, message);
                            }
                            let changed = self.details.get(&chat.id).is_some_and(|d| {
                                chat.roster_version
                                    .as_ref()
                                    .is_some_and(|v| d.roster_version.as_ref() != Some(v))
                            });
                            if changed {
                                self.details.remove(&chat.id);
                                self.resolved_ids.remove(&chat.id);
                            }
                            if let Some(details) = self.details.get(&chat.id) {
                                conversations::apply_details(chat, details);
                            }
                            conversations::resolve_names_in(chat, &self.profiles, &mut by_case);
                        }
                        if workspace.more_chats {
                            let fresh: HashSet<&str> =
                                chats.iter().map(|c| c.id.as_str()).collect();
                            let older: Vec<Conversation> = self
                                .chats
                                .iter()
                                .filter(|old| !fresh.contains(old.id.as_str()))
                                .cloned()
                                .collect();
                            chats.extend(older);
                        }
                        *self.chats = chats;
                        let mut teams = workspace.teams;
                        if !workspace.teams_complete {
                            for old in &*self.teams {
                                if !teams.iter().any(|t| t.id == old.id) {
                                    teams.push(old.clone());
                                }
                            }
                        }
                        for team in &mut teams {
                            if self.channels_loaded.contains(&team.id)
                                && let Some(old) = self.teams.iter().find(|t| t.id == team.id)
                            {
                                for channel in &old.channels {
                                    if !team.channels.iter().any(|c| c.id == channel.id) {
                                        team.channels.push(channel.clone());
                                    }
                                }
                            }
                            normalize_team(team);
                        }
                        *self.teams = teams;
                        *self.more_chats = workspace.more_chats;
                        self.initialize_team_list(workspace.team_order);
                        if self.selected.is_none()
                            && let Some(chat) = self.chats.first()
                        {
                            let select = Task::done(Msg::Select(Selection::Chat(chat.id.clone())));
                            let resolve = self.resolve_chats();
                            return Task::batch([select, resolve]);
                        }
                    }
                    Err(e) => {
                        let e = String::from(e);
                        *self.teams_problem = Some(e.clone());
                        *self.error = Some(e);
                    }
                }
                self.resolve_chats()
            }
            Msg::ChatsResolved(_, result) => {
                self.resolving_chats = false;
                self.resolved_ids.extend(result.attempted);
                Arc::make_mut(&mut self.profiles).extend(
                    result
                        .profiles
                        .into_iter()
                        .filter(|p| !p.mri.is_empty())
                        .map(|p| (p.mri.clone(), p)),
                );
                for chat in result.chats {
                    self.details
                        .insert(chat.id.clone(), conversations::ChatDetails::of(chat));
                }
                for chat in &mut *self.chats {
                    if let Some(details) = self.details.get(&chat.id) {
                        conversations::apply_details(chat, details);
                    }
                    conversations::resolve_names(chat, &self.profiles);
                }
                if let Some(warning) = result.warning {
                    *self.participants_notice = Some(warning);
                }
                self.resolve_chats()
            }
            Msg::Select(selection) => {
                self.kept_unread.remove(selection.id());
                match &selection {
                    Selection::Chat(id) => self.last_chat = Some(id.clone()),
                    Selection::Channel { team, channel } => {
                        self.last_channel = Some((team.clone(), channel.clone()))
                    }
                }
                if let Selection::Channel { team, channel } = &selection {
                    self.reveal_channel(team, channel);
                }
                let read_horizon = match &selection {
                    Selection::Chat(id) => self
                        .chats
                        .iter()
                        .find(|c| &c.id == id)
                        .and_then(view::read_horizon),
                    Selection::Channel { .. } => None,
                };
                self.conv.switch(
                    self.selected.as_ref(),
                    &selection,
                    read_horizon,
                    &self.handoff,
                );
                *self.selected = Some(selection.clone());
                *self.tab = Tab::Messages;
                *self.new_chat = None;
                *self.nav = if selection.is_channel() {
                    Nav::Teams
                } else if *self.nav == Nav::Activity {
                    Nav::Activity
                } else {
                    Nav::Chat
                };
                self.load_feed(false)
            }
            Msg::ToggleTeam(id) => {
                if !self.expanded.remove(&id) {
                    self.expanded.insert(id);
                }
                Task::none()
            }
            Msg::LoadChannels(id) => {
                self.channel_errors.remove(&id);
                self.channels_loaded.remove(&id);
                self.expanded.insert(id);
                Task::none()
            }
            Msg::ChannelsLoaded {
                team: id, result, ..
            } => {
                self.channels_loading.remove(&id);
                self.channels_loaded.insert(id.clone());
                match result {
                    Ok((channels, report)) => {
                        self.channel_errors.remove(&id);
                        if let Some(team) = self.teams.iter_mut().find(|t| t.id == id) {
                            // Merged, not replaced: the startup data also lists channels this
                            // endpoint leaves out, private ones the user belongs to among
                            // them, as the web client keeps every channel it synced.
                            let mut merged = channels;
                            for old in std::mem::take(&mut team.channels) {
                                if !merged.iter().any(|c| c.id == old.id) {
                                    merged.push(old);
                                }
                            }
                            team.channels = merged;
                            normalize_team(team);
                            // Said rather than lost: these channels would silently be absent.
                            if let Some(report) = report {
                                *self.notice = Some(tf!(
                                    "core.app.some_channels_unreadable",
                                    team = team.display_name.clone(),
                                    error = report
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        self.channel_errors.insert(id, e);
                    }
                }
                Task::none()
            }
            Msg::ChannelMembers => {
                let (Some(client), Some(selection)) = (self.client.clone(), self.selected.clone())
                else {
                    return Task::none();
                };
                let key = selection.key();
                if self.members_loading.contains(&key) {
                    return Task::none();
                }
                self.members_loading.insert(key.clone());
                self.members_errors.remove(&key);
                let generation = self.generation;
                let fallback = if let Selection::Channel { team, channel } = &selection {
                    self.teams
                        .iter()
                        .find(|t| &t.id == team)
                        .and_then(|t| t.channels.iter().find(|c| &c.id == channel))
                        .filter(|c| c.channel_type.eq_ignore_ascii_case("standard"))
                        .map(|_| team.clone())
                } else {
                    None
                };
                Task::perform(
                    async move {
                        let roster = match client.all_thread_members(selection.id()).await {
                            Ok(members) if !members.is_empty() => Ok(members),
                            first => {
                                if let Some(team) = fallback {
                                    client.all_thread_members(&team).await
                                } else {
                                    first
                                }
                            }
                        };
                        let result = match roster {
                            Ok(members) => {
                                let ids: Vec<_> = members
                                    .iter()
                                    .filter(|m| m.display_name().is_none())
                                    .map(|m| m.id.clone())
                                    .collect();
                                let mut profiles = Vec::new();
                                for batch in ids.chunks(100) {
                                    if let Ok(p) = client.people_profiles(batch).await {
                                        profiles.extend(p);
                                    }
                                }
                                Ok((members, profiles))
                            }
                            Err(e) => Err(e.to_string()),
                        };
                        (key, result)
                    },
                    move |(key, r)| Msg::MembersLoaded {
                        generation,
                        key,
                        result: r,
                    },
                )
            }
            Msg::MembersLoaded { key, result, .. } => {
                self.members_loading.remove(&key);
                match result {
                    Ok((members, profiles)) => {
                        self.channel_members.insert(key.clone(), members);
                        self.members_errors.remove(&key);
                        Arc::make_mut(&mut self.profiles)
                            .extend(profiles.into_iter().map(|p| (p.mri.clone(), p)));
                    }
                    Err(e) => {
                        self.members_errors.insert(key, e);
                    }
                }
                Task::none()
            }
            Msg::FeedMerged(selection) => {
                if let Selection::Chat(id) = &selection
                    && let Some(i) = self.chats.iter().position(|c| &c.id == id)
                {
                    // Worked on a copy: writing back an unchanged chat would rebuild every part.
                    let mut chat = self.chats[i].clone();
                    for message in self.conv.messages() {
                        conversations::update_last_message(&mut chat, message);
                    }
                    conversations::name_members_from_messages(&mut chat, self.conv.messages());
                    if chat != self.chats[i] {
                        self.chats[i] = chat;
                    }
                }
                let presence = self.load_presence();
                let last = if selection.is_channel() {
                    // Posts and their loaded replies; ids are arrival times.
                    self.conv
                        .posts()
                        .iter()
                        .flat_map(|p| &p.messages)
                        .max_by_key(|m| m.id.parse::<i64>().unwrap_or(0))
                } else {
                    self.conv.messages().last()
                };
                if let Some(last) = last
                    && !self.kept_unread.contains(selection.id())
                {
                    return Task::batch([
                        presence,
                        Task::done(Msg::MarkRead(selection, last.id.clone())),
                    ]);
                }
                presence
            }
            Msg::MoreChats => {
                if *self.loading_workspace {
                    return Task::none();
                }
                let Some(client) = self.client.clone() else {
                    return Task::none();
                };
                *self.loading_workspace = true;
                let cursor = self.chat_cursor.clone();
                let generation = self.generation;
                Task::perform(
                    async move {
                        client
                            .chats_page(cursor.as_deref())
                            .await
                            .map_err(|e| e.to_string())
                    },
                    move |r| Msg::MoreChatsLoaded(generation, r),
                )
            }
            Msg::MoreChatsLoaded(_, result) => {
                *self.loading_workspace = false;
                match result {
                    Ok(page) => {
                        self.chat_cursor = page.continuation_token;
                        *self.more_chats = page.has_more_data && self.chat_cursor.is_some();
                        for chat in page
                            .chats
                            .into_iter()
                            .filter(|c| {
                                c.visible_in_chat_list(
                                    self.favorites
                                        .as_ref()
                                        .is_some_and(|ids| ids.contains(&c.id)),
                                )
                            })
                            .map(|c| c.conversation())
                            .filter(|c| conversations::is_private_chat(c, &self.teams))
                        {
                            if !self.chats.iter().any(|c| c.id == chat.id) {
                                self.chats.push(chat);
                            }
                        }
                    }
                    Err(e) => *self.error = Some(e),
                }
                self.resolve_chats()
            }
            Msg::DraftChanged(value) => {
                self.track_draft_edit(&value);
                self.conv.draft_typed(value);
                self.typed()
            }
            Msg::RichDraftChanged(value, formats) => {
                self.rich_draft_changed(value, formats);
                self.typed()
            }
            Msg::ReplaceDraft {
                start,
                length,
                text,
                mention,
                emoji,
                style,
            } => {
                let revision = self.conv.draft_rev();
                let inserted = text.encode_utf16().count() as u32;
                self.replace_draft(start, length, text, mention, emoji);
                if self.conv.draft_rev() != revision
                    && let Some(style) = style
                {
                    self.format_inserted(start, inserted, style);
                }
                Task::none()
            }
            Msg::Send => {
                if self.read_only() {
                    return Task::none();
                }
                if self.conv.sending()
                    || (self.conv.draft().trim().is_empty()
                        && !self.has_attachments()
                        && !self.has_extension_cards())
                {
                    return Task::none();
                }
                let attachments = match self.ready_attachments() {
                    Ok(ready) => ready,
                    Err(notice) => {
                        *self.notice = Some(notice);
                        return Task::none();
                    }
                };
                let (Some(client), Some(selection)) = (self.client.clone(), self.selected.clone())
                else {
                    return Task::none();
                };
                let (ids, uploaded): (Vec<_>, Vec<_>) = attachments.into_iter().unzip();
                let share = self.share_task(&selection, uploaded);
                self.conv.begin_send();
                *self.error = None;
                let content = self.conv.draft().to_owned();
                let reply = self.conv.reply().cloned();
                let generation = self.generation;
                let mut html = if content.trim().is_empty() {
                    String::new()
                } else {
                    self.composer_html()
                };
                let cards = self.take_extension_cards();
                let cards_json = cards.as_ref().map(|(_, list)| {
                    let (markup, json) = extensions::cards_markup(list);
                    html.push_str(&markup);
                    json
                });
                // Kept until the answer: a failed send gives them back to the draft.
                self.extensions.sending = cards;
                let mut message =
                    OutgoingMessage::html(client_message_id(), html, &*self.display_name);
                if let Some(json) = cards_json {
                    message.properties.cards = json;
                }
                if let Some(links) = self.draft_links() {
                    message.properties.links = links;
                }
                message.properties.mentions = self.composer_mentions();
                message.properties.subject = self.conv.subject().to_owned();
                message.properties.importance = self.conv.importance().wire().to_owned();
                Task::perform(
                    async move {
                        let result = async {
                            if let Some(share) = share {
                                message.properties.files = share.await?;
                            }
                            client
                                .send_message(selection.id(), reply.as_deref(), &message)
                                .await
                                .map(drop)
                                .map_err(|e| e.to_string())
                        }
                        .await;
                        (selection, reply, content, ids, result)
                    },
                    move |(selection, reply, content, ids, result)| Msg::Sent {
                        generation,
                        selection,
                        reply,
                        content,
                        ids,
                        result,
                    },
                )
            }
            Msg::Sent {
                selection,
                reply,
                content,
                ids,
                result,
                ..
            } => {
                self.conv.send_finished();
                let cards = self.extensions.sending.take();
                if result.is_err()
                    && let Some((key, cards)) = cards
                {
                    self.restore_extension_cards(key, cards);
                }
                match result {
                    Ok(()) => {
                        // Answering means the new messages were read.
                        if self.selected.as_ref() == Some(&selection) {
                            self.conv.forget_read_horizon();
                        }
                        self.sent_attachments(&ids);
                        self.typing_sent_message(selection.id());
                        let key =
                            format!("{}:{}", selection.key(), reply.as_deref().unwrap_or("main"));
                        let current = self.selected.as_ref() == Some(&selection)
                            && self.conv.reply() == reply.as_ref();
                        if self
                            .conv
                            .sent(self.selected.as_ref(), &key, current, &content)
                        {
                            self.composer.tokens.remove(&key);
                            self.composer.formats.remove(&key);
                        }
                        if self.selected.as_ref() == Some(&selection) {
                            return Task::batch([self.load_feed(false), self.load_workspace()]);
                        }
                        return self.load_workspace();
                    }
                    Err(e) => {
                        *self.error = Some(tf!("core.app.could_not_send_your_draft_was", error = e))
                    }
                }
                Task::none()
            }
            Msg::NewChat => {
                *self.new_chat = Some(NewChat::default());
                *self.nav = Nav::Chat;
                Task::none()
            }
            Msg::CancelNewChat => {
                *self.new_chat = None;
                Task::none()
            }
            Msg::PeopleQuery(q) => {
                if let Some(dialog) = &mut *self.new_chat {
                    dialog.query = q;
                    dialog.results.clear();
                    dialog.searched = false;
                }
                Task::none()
            }
            Msg::PeopleSearch => {
                let (Some(dialog), Some(client)) = (&mut *self.new_chat, self.client.clone())
                else {
                    return Task::none();
                };
                if dialog.query.trim().len() < 2 || dialog.searching {
                    return Task::none();
                }
                dialog.searching = true;
                let query = dialog.query.trim().to_owned();
                let generation = self.generation;
                Task::perform(
                    async move {
                        let r = client
                            .search_people(&query)
                            .await
                            .map_err(|e| e.to_string());
                        (query, r)
                    },
                    move |(q, r)| Msg::PeopleLoaded {
                        generation,
                        query: q,
                        result: r,
                    },
                )
            }
            Msg::PeopleLoaded { query, result, .. } => {
                if let Some(dialog) = &mut *self.new_chat {
                    dialog.searching = false;
                    if dialog.query.trim() == query {
                        dialog.searched = true;
                        match result {
                            Ok(people) => dialog.results = people,
                            Err(e) => *self.error = Some(e),
                        }
                    }
                }
                Task::none()
            }
            Msg::ChoosePerson(person) => {
                if let Some(d) = &mut *self.new_chat
                    && !d.people.iter().any(|p| p.mri == person.mri)
                {
                    d.people.push(person);
                    d.query.clear();
                    d.results.clear();
                }
                Task::none()
            }
            Msg::RemovePerson(id) => {
                if let Some(d) = &mut *self.new_chat {
                    d.people.retain(|p| p.mri != id);
                }
                Task::none()
            }
            Msg::NewTitle(title) => {
                if let Some(d) = &mut *self.new_chat {
                    d.title = title;
                }
                Task::none()
            }
            Msg::CreateChat => {
                let (Some(dialog), Some(client)) = (&mut *self.new_chat, self.client.clone())
                else {
                    return Task::none();
                };
                if dialog.people.is_empty() || dialog.creating {
                    return Task::none();
                }
                // Teams rejects a roster naming you twice or only you: a chat with yourself is your notes, which always exist.
                let me = self.me.clone();
                let people: Vec<_> = dialog
                    .people
                    .iter()
                    .filter(|p| !p.mri.eq_ignore_ascii_case(&me))
                    .cloned()
                    .collect();
                if people.is_empty() {
                    *self.new_chat = None;
                    return Task::done(Msg::Select(Selection::Chat(NOTES.into())));
                }
                if dialog.title.trim().is_empty()
                    && let Some(id) = existing_chat(&self.chats, &self.details, &me, &people)
                {
                    *self.new_chat = None;
                    return Task::done(Msg::Select(Selection::Chat(id)));
                }
                dialog.creating = true;
                let title = dialog.title.clone();
                let generation = self.generation;
                Task::perform(
                    async move {
                        let r = client.create_chat(&me, &people, &title).await.map_err(|e| {
                            if matches!(e, teams_chat::Error::Status { status, .. } if status.as_u16() == 400) {
                                t("core.app.teams_refuses_one_of_the_recipients").to_owned()
                            } else {
                                tf!("core.app.could_not_create_the_chat", error = e)
                            }
                        });
                        (people, title, r)
                    },
                    move |(p, t, r)| Msg::ChatCreated {
                        generation,
                        people: p,
                        title: t,
                        result: r,
                    },
                )
            }
            Msg::ChatCreated {
                people,
                title,
                result,
                ..
            } => match result {
                Ok(id) => {
                    let c = teams_chat::CsaChat {
                        id: id.clone(),
                        title,
                        members: people,
                        ..Default::default()
                    }
                    .conversation();
                    if !self.chats.iter().any(|c| c.id == id) {
                        self.chats.insert(0, c);
                    }
                    *self.new_chat = None;
                    Task::done(Msg::Select(Selection::Chat(id)))
                }
                Err(e) => {
                    *self.error = Some(e);
                    if let Some(d) = &mut *self.new_chat {
                        d.creating = false;
                    }
                    Task::none()
                }
            },
            Msg::OpenUrl(url) => {
                if html::safe_link(&url).is_some() {
                    Task::perform(blocking(move || open_browser(&url)), Msg::UrlOpened)
                } else {
                    *self.error = Some(t("core.app.this_link_is_not_a_valid").into());
                    Task::none()
                }
            }
            Msg::UrlOpened(r) => {
                if let Err(e) = r {
                    *self.error = Some(e);
                }
                Task::none()
            }
            Msg::MarkRead(selection, id) => {
                // Banners of a conversation the user has now read are stale.
                let withdraw = Task::effect(Effect::WithdrawNotifications {
                    conversation_id: selection.id().to_owned(),
                });
                let horizon = format!("{id};{};{id}", chrono::Utc::now().timestamp_millis());
                let mut clear_bookmark = false;
                if let Selection::Channel { team, channel } = &selection {
                    let me = self.me.clone();
                    // Looked up read-only first: a mutable borrow alone marks every part changed.
                    let found = self
                        .teams
                        .iter()
                        .filter(|t| &t.id == team)
                        .flat_map(|t| &t.channels)
                        .find(|c| &c.id == channel)
                        .is_some_and(|c| c.is_unread(&me));
                    let unread = found
                        .then(|| {
                            self.teams
                                .iter_mut()
                                .filter(|t| &t.id == team)
                                .flat_map(|t| t.channels.iter_mut())
                                .find(|c| &c.id == channel)
                        })
                        .flatten();
                    match unread {
                        Some(channel) => channel.set_read(horizon.clone()),
                        None => return withdraw,
                    }
                } else {
                    let unread = self
                        .chats
                        .iter()
                        .position(|c| c.id == selection.id() && c.is_unread());
                    let Some(chat) = unread.map(|i| &mut self.chats[i]) else {
                        return withdraw;
                    };
                    chat.is_read = Some(true);
                    clear_bookmark = chat_admin::has_unread_bookmark(chat);
                    chat.properties.remove("consumptionHorizonBookmark");
                    chat.extra.remove("userConsumptionHorizon");
                }
                let Some(client) = self.client.clone() else {
                    return withdraw;
                };
                let generation = self.generation;
                let now = chrono::Utc::now().timestamp_millis();
                let mark = Task::perform(
                    async move {
                        // "Mark as unread" left a bookmark: reading clears it first.
                        if clear_bookmark {
                            client
                                .set_conversation_property(
                                    selection.id(),
                                    "consumptionHorizonBookmark",
                                    serde_json::json!(format!("0;{now};0")),
                                )
                                .await
                                .map_err(|e| e.to_string())?;
                        }
                        client
                            .set_conversation_property(
                                selection.id(),
                                "consumptionhorizon",
                                serde_json::json!(horizon),
                            )
                            .await
                            .map(drop)
                            .map_err(|e| e.to_string())
                    },
                    move |r| Msg::ReadMarked(generation, r),
                );
                Task::batch([mark, withdraw])
            }
            Msg::ReadMarked(_, r) => {
                if let Err(e) = r {
                    *self.error = Some(tf!("core.app.read_receipt", error = e));
                }
                Task::none()
            }
            Msg::LoadAvatar(id, name) => {
                // Names the views give when they know no one: no photo to look for.
                const PLACEHOLDERS: [&str; 4] = [
                    "core.app.chat",
                    "core.app.group_chat",
                    "core.view.unknown_sender",
                    "core.view.your_account",
                ];
                if name == "Participant" || relocalized(&name, &PLACEHOLDERS).is_some() {
                    return Task::none();
                }
                if id.starts_with("28:")
                    && let Some(task) = self.load_bot_avatar(&id, &name)
                {
                    return task;
                }
                if let Some(mri) = self.avatar_mri(&id) {
                    self.avatars.lock().unwrap().request(id, mri, name);
                    self.mark(Parts::AVATARS);
                }
                Task::none()
            }
            Msg::BotAppLoaded { id, result, .. } => self.bot_app_loaded(id, result),
            Msg::AvatarLoaded { id, result, .. } => {
                self.avatars.lock().unwrap().finish(id, result);
                self.mark(Parts::AVATARS);
                Task::none()
            }
            Msg::MyProfileLoaded(_, result) => {
                if let Ok(mut profile) = result {
                    if let Some(name) = profile.known_name() {
                        *self.display_name = name.to_owned();
                    }
                    *self.account_email = profile.email.clone();
                    // /me may omit the MRI; routing uses the authenticated identity, not a profile field.
                    profile.mri = self.me.clone();
                    Arc::make_mut(&mut self.profiles).insert(self.me.clone(), profile);
                }
                Task::none()
            }
            Msg::PresenceLoaded(_, result) => {
                // Without presence the chats only show no badge: no banner either.
                if let Ok(found) = result {
                    let me = self.me.to_ascii_lowercase();
                    if let Some(mine) = found.iter().find(|p| p.mri.eq_ignore_ascii_case(&me)) {
                        let mine = mine.clone();
                        let _ = self.update_me(MeMsg::Loaded(self.generation, Ok(Some(mine))));
                    }
                    let found: HashMap<_, _> = found
                        .into_iter()
                        .map(|p| (p.mri.to_ascii_lowercase(), p))
                        .collect();
                    if *self.presence != found {
                        *self.presence = found;
                    }
                }
                Task::none()
            }
            Msg::ProfilesLoaded(_, result) => {
                // A failed lookup only leaves the generic name; it is not worth a banner.
                if let Ok(people) = result {
                    Arc::make_mut(&mut self.profiles).extend(
                        people
                            .into_iter()
                            .filter(|p| !p.mri.is_empty())
                            .map(|p| (p.mri.clone(), p)),
                    );
                    for chat in &mut *self.chats {
                        conversations::resolve_names(chat, &self.profiles);
                    }
                }
                Task::none()
            }
        }
    }
    fn load_visible_channels(&mut self) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        if *self.nav != Nav::Teams {
            return Task::none();
        }
        let visible = self.visible_team_ids();
        let ids: Vec<_> = self
            .teams
            .iter()
            .filter(|t| {
                visible.contains(&t.id)
                    && !self.channels_loaded.contains(&t.id)
                    && !self.channels_loading.contains(&t.id)
            })
            .take(4usize.saturating_sub(self.channels_loading.len()))
            .map(|t| t.id.clone())
            .collect();
        let generation = self.generation;
        Task::batch(
            ids.into_iter()
                .map(|id| {
                    self.channels_loading.insert(id.clone());
                    let client = client.clone();
                    Task::perform(
                        async move {
                            let r = client
                                .channels_with_report(&id)
                                .await
                                .map_err(|e| e.to_string());
                            (id, r)
                        },
                        move |(id, r)| Msg::ChannelsLoaded {
                            generation,
                            team: id,
                            result: r,
                        },
                    )
                })
                .collect::<Vec<_>>(),
        )
    }
    /// Looks up authors and reactors that the loaded messages do not name, once each.
    fn resolve_people(&mut self) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        // Runs after every message (each keystroke too): filter before allocating, parse reactions once per change.
        let wanted_mri = |mri: &str| {
            mri.starts_with("8:")
                && !mri.eq_ignore_ascii_case(&self.me)
                && !self.profiles_requested.contains(mri)
        };
        // Known participant MRIs resolve as one batch, without waiting for every chat's roster request.
        let participants = self
            .chats
            .iter()
            .flat_map(|c| &c.members)
            .filter(|member| member.display_name().is_none())
            .map(|member| member.id.as_str())
            .filter(|mri| wanted_mri(mri))
            .map(str::to_owned);
        let mut scanned = Vec::new();
        let mut candidates: Vec<String> = participants.collect();
        for message in self.conv.all_messages() {
            if message
                .imdisplayname
                .as_deref()
                .is_none_or(|n| n.trim().is_empty())
                && wanted_mri(message.sender_mri())
            {
                candidates.push(message.sender_mri().to_owned());
            }
            let Some(raw) = message.properties.get("emotions") else {
                continue;
            };
            let fingerprint = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::hash::DefaultHasher::new();
                message.id.hash(&mut hasher);
                match raw.as_str() {
                    Some(text) => text.hash(&mut hasher),
                    None => raw.to_string().hash(&mut hasher),
                }
                hasher.finish()
            };
            if self.reactions_scanned.contains(&fingerprint) {
                continue;
            }
            scanned.push(fingerprint);
            candidates.extend(
                message
                    .reactions(&self.me)
                    .into_iter()
                    .flat_map(|r| r.users)
                    .filter(|mri| wanted_mri(mri)),
            );
        }
        self.reactions_scanned.extend(scanned);
        let mut wanted = Vec::new();
        for mri in candidates {
            if !self.profiles_requested.contains(&mri) && self.known_name(&mri).is_none() {
                self.profiles_requested.insert(mri.clone());
                wanted.push(mri);
            }
        }
        if wanted.is_empty() {
            return Task::none();
        }
        let generation = self.generation;
        Task::perform(
            async move {
                let mut people = Vec::new();
                for batch in wanted.chunks(100) {
                    people.extend(
                        client
                            .people_profiles(batch)
                            .await
                            .map_err(|e| e.to_string())?,
                    );
                }
                Ok(people)
            },
            move |r| Msg::ProfilesLoaded(generation, r),
        )
    }
    fn resolve_chats(&mut self) -> Task<Msg> {
        if self.resolving_chats {
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let mut chats: Vec<_> = self
            .chats
            .iter()
            .filter(|c| {
                !self.resolved_ids.contains(&c.id) && conversations::needs_details(c, &self.me)
            })
            .collect();
        chats.sort_by_key(|chat| {
            std::cmp::Reverse((
                chat.is_one_on_one() && chat.members.len() < 2,
                chat.title().is_none(),
                conversations::last_message_time(chat),
            ))
        });
        let incomplete_direct = chats
            .iter()
            .filter(|chat| chat.is_one_on_one() && chat.members.len() < 2)
            .count();
        // Publish recovered direct-chat names before waiting for unrelated rosters.
        let batch_size = if incomplete_direct > 0 {
            incomplete_direct.min(4)
        } else {
            32
        };
        let chats: Vec<_> = chats.into_iter().take(batch_size).cloned().collect();
        if chats.is_empty() {
            return Task::none();
        }
        self.resolving_chats = true;
        let generation = self.generation;
        let profiles = Arc::clone(&self.profiles);
        Task::perform(conversations::resolve(client, chats, profiles), move |r| {
            Msg::ChatsResolved(generation, r)
        })
    }
    fn load_my_profile(&self) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let me = self.me.clone();
        let generation = self.generation;
        Task::perform(
            async move {
                match client.my_profile().await {
                    Ok(profile) => Ok(profile),
                    Err(_) => client
                        .people_profiles(std::slice::from_ref(&me))
                        .await
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .find(|p| p.mri.eq_ignore_ascii_case(&me))
                        .ok_or_else(|| t("core.app.account_profile_unavailable").to_owned()),
                }
            },
            move |result| Msg::MyProfileLoaded(generation, result),
        )
    }
    fn load_workspace(&mut self) -> Task<Msg> {
        if *self.loading_workspace {
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        *self.loading_workspace = true;
        let generation = self.generation;
        Task::perform(load_workspace_data(client), move |r| {
            Msg::WorkspaceLoaded(generation, r)
        })
    }
    fn read_only(&self) -> bool {
        read_only(self.selected.as_ref(), &self.chats, &self.teams)
    }
    fn title(&self) -> String {
        match &*self.selected {
            Some(Selection::Chat(id)) => self
                .chats
                .iter()
                .find(|c| &c.id == id)
                .map(|c| chat_title(c, &self.me))
                .unwrap_or_else(|| {
                    if id == NOTES {
                        notes_title()
                    } else {
                        t("core.app.chat")
                    }
                    .into()
                }),
            Some(Selection::Channel { team, channel }) => self
                .teams
                .iter()
                .find(|t| &t.id == team)
                .and_then(|t| t.channels.iter().find(|c| &c.id == channel))
                .map(|c| c.display_name.clone())
                .unwrap_or_else(|| t("core.app.channel").into()),
            None => t("core.app.welcome").into(),
        }
    }
}
/// Fetch chats and teams independently: one malformed startup item must not hide teams.
async fn load_workspace_data(client: Client) -> Result<Workspace, CoreError> {
    let (startup, teams, preferences) =
        tokio::join!(client.startup(), client.teams(), client.user_properties());
    let mut warnings = Vec::new();
    let (mut startup, startup_ok) = match startup {
        Ok(data) => (data, true),
        Err(error) => {
            warnings.push(tf!("core.app.csa_startup_unavailable", error = error));
            (Startup::default(), false)
        }
    };
    let teams_ok = match teams {
        Ok(teams) => {
            for mut team in teams {
                if let Some(initial) = startup.teams.iter_mut().find(|t| t.id == team.id) {
                    for channel in &initial.channels {
                        if !team.channels.iter().any(|c| c.id == channel.id) {
                            team.channels.push(channel.clone());
                        }
                    }
                    for (key, value) in &initial.extra {
                        team.extra
                            .entry(key.clone())
                            .or_insert_with(|| value.clone());
                    }
                    *initial = team;
                } else {
                    startup.teams.push(team);
                }
            }
            true
        }
        Err(error) => {
            warnings.push(tf!("core.app.team_list_unavailable", error = error));
            false
        }
    };
    let mut result = workspace(startup);
    let preferences = preferences.ok();
    result.team_order = preferences.as_ref().and_then(team_list::parse_order);
    if result.favorites_folder.is_none()
        && let Some(order) = preferences.as_ref().and_then(chat_admin::favorite_property)
    {
        result.favorites = Some(order.iter().cloned().collect());
        result.favorite_order = order;
    }
    result.preferences = preferences;
    if result.teams.is_empty() && (!startup_ok || !teams_ok) {
        result.teams_problem = Some(warnings.join(" · "));
    }
    result.teams_complete =
        teams_ok || (startup_ok && result.teams_complete && result.teams_problem.is_none());
    if !startup_ok {
        match client.conversations(100).await {
            Ok(page) => {
                result.chats = page
                    .conversations
                    .into_iter()
                    .filter(|c| conversations::is_private_chat(c, &result.teams))
                    .collect()
            }
            Err(error) if !teams_ok => {
                let message = tf!(
                    "core.app.chats_unavailable",
                    names = warnings.join(" · "),
                    error = error
                );
                return Err(CoreError::chat(&error, message));
            }
            Err(error) => warnings.push(tf!("core.app.chats_unavailable_2", error = error)),
        }
    }
    result
        .chats
        .retain(|c| conversations::is_private_chat(c, &result.teams));
    result.warning = (!warnings.is_empty()).then(|| warnings.join(" · "));
    Ok(result)
}
/// Redeems the authorization code of a sign-in that came back.
fn complete_login(login: LoginAttempt, callback: String) -> Task<Msg> {
    Task::perform(
        async move {
            login
                .complete(&callback)
                .await
                .map(Arc::new)
                .map_err(CoreError::from)
        },
        Msg::SignedIn,
    )
}
/// Runs a call that waits on the system (a process, the Keychain) away from the network workers.
async fn blocking(
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|_| Err(t("core.app.operation_interrupted").into()))
}
fn normalize_team(team: &mut Team) {
    team.channels.retain(|c| !c.is_deleted);
    if !team
        .channels
        .iter()
        .any(|c| c.is_general || c.id == team.id)
    {
        team.channels.insert(
            0,
            Channel {
                id: team.id.clone(),
                parent_team_id: team.id.clone(),
                display_name: general_channel().into(),
                description: team.description.clone(),
                channel_type: "Standard".into(),
                is_general: true,
                is_archived: team.is_archived,
                ..Default::default()
            },
        );
    }
    for channel in &mut team.channels {
        if channel.id == team.id {
            channel.is_general = true;
        }
        if channel.parent_team_id.is_empty() {
            channel.parent_team_id = team.id.clone();
        }
    }
    team.channels.sort_by(|a, b| {
        b.is_general
            .cmp(&a.is_general)
            .then_with(|| {
                let position = |c: &Channel| {
                    c.extra
                        .get("order")
                        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                        .unwrap_or(i64::MAX)
                };
                position(a).cmp(&position(b))
            })
            .then_with(|| {
                a.display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase())
            })
    });
}
fn workspace(mut startup: Startup) -> Workspace {
    for channel in startup.channels.drain(..) {
        if let Some(team) = startup
            .teams
            .iter_mut()
            .find(|t| t.id == channel.parent_team_id)
            && !team.channels.iter().any(|c| c.id == channel.id)
        {
            team.channels.push(channel);
        }
    }
    startup.teams.retain(|t| !t.is_deleted);
    for team in &mut startup.teams {
        normalize_team(team);
    }
    let favorites_folder = startup.conversation_folders.as_ref().and_then(|folders| {
        let folder = folders
            .conversation_folders
            .iter()
            .find(|f| !f.is_deleted && f.folder_type == "Favorites" && !f.id.is_empty())?;
        Some((folder.id.clone(), folders.folder_hierarchy_version?))
    });
    let favorites: Option<HashSet<String>> = startup.conversation_folders.map(|folders| {
        folders
            .conversation_folders
            .into_iter()
            .filter(|folder| !folder.is_deleted && folder.folder_type == "Favorites")
            .flat_map(|folder| {
                folder
                    .conversation_folder_items
                    .into_iter()
                    .map(|item| item.conversation_id)
            })
            .collect()
    });
    Workspace {
        chats: startup
            .chats
            .into_iter()
            .filter(|c| {
                c.visible_in_chat_list(favorites.as_ref().is_some_and(|ids| ids.contains(&c.id)))
            })
            .map(|c| c.conversation())
            .filter(|c| conversations::is_private_chat(c, &startup.teams))
            .collect(),
        favorites,
        favorites_folder,
        favorite_order: vec![],
        teams: startup.teams,
        more_chats: startup.metadata.is_partial_data,
        teams_complete: !startup.metadata.is_partial_data,
        teams_problem: None,
        team_order: None,
        preferences: None,
        users: startup.users,
        warning: None,
    }
}
/// The chat already gathering exactly these people, which Teams reopens; group rosters must be complete to avoid chance matches.
fn existing_chat(
    chats: &[Conversation],
    details: &HashMap<String, conversations::ChatDetails>,
    me: &str,
    people: &[Person],
) -> Option<String> {
    let wanted: HashSet<String> = people.iter().map(|p| p.mri.to_lowercase()).collect();
    chats
        .iter()
        .filter(|c| c.id != NOTES && !c.is_disabled())
        .filter(|c| {
            if wanted.len() == 1 {
                !c.is_group()
            } else {
                c.is_group() && details.contains_key(&c.id)
            }
        })
        .find(|c| {
            let members: HashSet<String> = c
                .members
                .iter()
                .map(|m| m.id.to_lowercase())
                .filter(|id| !id.is_empty() && !id.eq_ignore_ascii_case(me))
                .collect();
            members == wanted
        })
        .map(|c| c.id.clone())
}
/// The other person of a direct chat, when they have a Teams MRI.
pub(crate) fn chat_peer<'a>(c: &'a Conversation, me: &str) -> Option<&'a str> {
    if c.is_group() || c.id == NOTES {
        return None;
    }
    c.members
        .iter()
        .find(|m| !m.id.eq_ignore_ascii_case(me))
        .map(|m| m.id.as_str())
        .filter(|id| id.starts_with("8:"))
}
pub(crate) fn chat_title(c: &Conversation, me: &str) -> String {
    if c.id == NOTES {
        return notes_title().into();
    }
    if let Some(title) = c.title() {
        return title.to_owned();
    }
    let mut seen = HashSet::new();
    let others: Vec<_> = c
        .members
        .iter()
        .filter(|m| {
            !m.id.is_empty() && !m.id.eq_ignore_ascii_case(me) && seen.insert(m.id.as_str())
        })
        .collect();
    let names: Vec<_> = others.iter().filter_map(|m| m.display_name()).collect();
    let missing = others.len() - names.len();
    if names.is_empty() {
        return if c.is_group() || others.len() > 1 {
            t("core.app.group_chat").into()
        } else {
            t("core.app.chat").into()
        };
    }
    let mut title = names.join(", ");
    if missing > 0 {
        title.push_str(&tp!("core.app.and_others", missing));
    }
    if c.is_group() && others.len() == 1 {
        title = tf!("core.app.group_chat_2", title = title);
    }
    title
}

fn short_time(iso: &str) -> String {
    DateTime::parse_from_rfc3339(iso)
        .map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_default()
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "<br>")
}
/// [`escape`] for a double-quoted attribute value.
fn escape_attr(s: &str) -> String {
    escape(s).replace('"', "&quot;")
}
fn client_message_id() -> String {
    chrono::Utc::now().timestamp_micros().to_string()
}

#[cfg(test)]
#[path = "app/tests.rs"]
mod tests;
