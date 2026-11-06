//! Banners, sounds and notification settings (global, chats, NSS channels, RCM threads) and the chat push channel.
use super::*;
use chrono::{DateTime, Local, Utc};
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use teams_chat::{
    ChannelNotificationSettings, ChannelSettingsResult, FollowState, NssChoice, ResourceType,
};
use teams_i18n::{t, tf};
use teams_realtime::{PushedRequest, RealtimeConfig, RealtimeEvent};

use super::notify_rules::{self as rules, Candidate, ConversationRules, Kind, Prefs};
pub use super::notify_rules::{
    EmailDelay, MeetingChats, NotificationChoice, NotificationPreset, NotificationRow,
    NotificationSection, NotificationSound, NotificationToggle,
};
use super::push::{Incoming, PushChannel, PushTexts, Transition};

const PUSH_TEXTS: PushTexts = PushTexts {
    connecting: "core.notifications.connecting_to_the_notifications_channel",
    registering: "core.notifications.connected_registering",
    ready: "core.notifications.real_time_notifications_active",
    refused: "core.notifications.registration_refused",
    retrying: "core.notifications.retrying_in_s",
    stopped: "core.notifications.real_time_stopped_refreshing_periodically",
};
/// Conversations whose `alerts` are read ahead, so the first banner does not wait.
const PREFETCHED_CHATS: usize = 25;
/// `notificationHistoryCacheSize`.
const SEEN_CHATS: usize = 50;
const SEEN_PER_CHAT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, uniffi::Enum)]
pub enum NotificationPermission {
    #[default]
    Unknown,
    Granted,
    Denied,
}

/// Banners of a chat: the global settings, always, or never (`dskNotif`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ChatDesktop {
    FollowGlobal,
    On,
    Off,
}

/// NSS choice of a channel line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ChannelChoice {
    BannerAndFeed,
    Feed,
    Off,
}
impl From<NssChoice> for ChannelChoice {
    fn from(choice: NssChoice) -> Self {
        match choice {
            NssChoice::BannerAndFeed => Self::BannerAndFeed,
            NssChoice::Feed => Self::Feed,
            NssChoice::Off => Self::Off,
        }
    }
}
impl From<ChannelChoice> for NssChoice {
    fn from(choice: ChannelChoice) -> Self {
        match choice {
            ChannelChoice::BannerAndFeed => Self::BannerAndFeed,
            ChannelChoice::Feed => Self::Feed,
            ChannelChoice::Off => Self::Off,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct NotificationRowView {
    pub row: NotificationRow,
    pub section: NotificationSection,
    pub title: String,
    pub choice: NotificationChoice,
}

/// The settings sheet of one chat or channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct NotificationEditorView {
    pub title: String,
    pub channel: bool,
    pub loading: bool,
    pub saving: bool,
    pub error: Option<String>,
    /// Chat: `alerts == false`.
    pub muted: bool,
    pub everyone_mentions: bool,
    pub desktop: ChatDesktop,
    /// Channel (NSS).
    pub all_new_posts: ChannelChoice,
    pub include_replies: bool,
    pub channel_mentions: ChannelChoice,
    /// `followAllRc` has an open segment.
    pub follow_all_threads: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct NotificationsView {
    pub loaded: bool,
    pub saving: bool,
    pub error: Option<String>,
    pub permission: NotificationPermission,
    /// State of the chat push channel.
    pub realtime: String,
    /// The chat push channel is connected and registered.
    pub realtime_live: bool,
    pub preset: NotificationPreset,
    pub rows: Vec<NotificationRowView>,
    pub show_preview: bool,
    pub mute_during_meetings: bool,
    pub sounds: bool,
    pub call_sounds: bool,
    pub sounds_when_busy: bool,
    pub sounds_for_urgent: bool,
    pub only_urgent: bool,
    pub sound: NotificationSound,
    pub urgent_sound: NotificationSound,
    pub email_delay: EmailDelay,
    pub meeting_chats: MeetingChats,
    /// Local time banners resume, e.g. "16:30" or "demain 08:00"; `None` when not paused.
    pub paused_until: Option<String>,
    /// Banners muted for the current call only.
    pub muted_in_call: bool,
    /// Read receipts in chats (`readReceiptsEnabled`).
    pub read_receipts: bool,
    pub editor: Option<NotificationEditorView>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum NotificationsAction {
    SetChoice {
        row: NotificationRow,
        choice: NotificationChoice,
    },
    SetPreset {
        preset: NotificationPreset,
    },
    SetToggle {
        toggle: NotificationToggle,
        on: bool,
    },
    SetSound {
        urgent: bool,
        sound: NotificationSound,
    },
    SetEmailDelay {
        delay: EmailDelay,
    },
    SetMeetingChats {
        choice: MeetingChats,
    },
    Pause {
        minutes: u32,
    },
    /// Until 8:00 tomorrow, local time.
    PauseUntilTomorrow,
    PauseUntil {
        epoch_ms: i64,
    },
    Resume,
    PermissionChanged {
        permission: NotificationPermission,
    },
    /// A banner was clicked.
    Open {
        conversation_id: String,
        reply_chain_id: Option<String>,
    },
    /// Quick reply typed in a banner.
    Reply {
        conversation_id: String,
        reply_chain_id: Option<String>,
        text: String,
    },
    MarkRead {
        conversation_id: String,
    },
    SetChatMuted {
        conversation_id: String,
        muted: bool,
    },
    EditChat {
        conversation_id: String,
    },
    EditChannel {
        team: String,
        channel: String,
    },
    CloseEditor,
    /// Applies to the chat of the open editor.
    SetChatSettings {
        muted: bool,
        everyone_mentions: bool,
        desktop: ChatDesktop,
    },
    /// Applies to the channel of the open editor.
    SetChannelSettings {
        all_new_posts: ChannelChoice,
        include_replies: bool,
        channel_mentions: ChannelChoice,
        follow_all_threads: bool,
    },
    /// "Turn off" from a channel menu.
    MuteChannel {
        team: String,
        channel: String,
    },
    /// Follow or unfollow a post of the open channel.
    SetPostFollow {
        post: String,
        follow: bool,
    },
    MarkAllActivityRead,
    MuteDuringThisCall {
        muted: bool,
    },
    /// Send and see read receipts (`readReceiptsEnabled`).
    SetReadReceipts {
        on: bool,
    },
    /// Shows a sample banner, whatever the settings.
    SendTest,
}

/// Personal properties of one conversation, or why they could not be read.
pub(crate) type PropertiesResult = Result<Map<String, Value>, String>;

#[derive(Debug, Clone)]
pub(crate) enum NotificationsMsg {
    Action(NotificationsAction),
    Active(bool),
    Realtime(u64, RealtimeEvent),
    PrefsLoaded(u64, Result<Value, String>),
    Saved(u64, u64, Result<(), String>),
    ConversationsLoaded(u64, Vec<(String, PropertiesResult)>),
    ConversationSaved(u64, String, ConversationSettings, Result<(), String>),
    ChannelsLoaded(u64, Vec<String>, Result<Vec<ChannelSettingsResult>, String>),
    ChannelSaved(
        u64,
        String,
        Option<ChannelNotificationSettings>,
        Result<(), String>,
    ),
    FollowsLoaded(u64, Vec<String>, Result<Vec<(String, FollowState)>, String>),
    FollowSaved(u64, String, Option<FollowState>, Result<(), String>),
    Replied(u64, String, Result<(), String>),
    ActivityRead(u64, Result<(), String>),
}

/// Personal properties of one conversation that drive its notifications.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ConversationSettings {
    /// Explicit `alerts`; `None` when never set.
    alerts: Option<bool>,
    /// `conversationNotificationSettings` (chats), kept whole so writes preserve keys.
    granular: Map<String, Value>,
    follow_all_rc: String,
}

impl ConversationSettings {
    fn from_properties(properties: &Map<String, Value>) -> Self {
        let text = |key: &str| match properties.get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Bool(b)) => Some(b.to_string()),
            _ => None,
        };
        let granular = [
            "conversationNotificationSettings",
            "channelNotificationSettings",
        ]
        .into_iter()
        .find_map(|key| match properties.get(key)? {
            Value::String(s) => serde_json::from_str::<Value>(s).ok()?.as_object().cloned(),
            Value::Object(map) => Some(map.clone()),
            _ => None,
        })
        .unwrap_or_default();
        Self {
            alerts: text("alerts").and_then(|s| match s.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }),
            granular,
            follow_all_rc: text("followAllRc").unwrap_or_default(),
        }
    }
    fn desktop(&self) -> ChatDesktop {
        match self.granular.get("dskNotif").and_then(Value::as_str) {
            Some("On") => ChatDesktop::On,
            Some("Off") => ChatDesktop::Off,
            _ => ChatDesktop::FollowGlobal,
        }
    }
    fn everyone(&self, default: bool) -> bool {
        match self.granular.get("everyone").and_then(Value::as_str) {
            Some("On") => true,
            Some("Off") => false,
            _ => default,
        }
    }
    /// `setFollowNotificationSettingsForChats` mapper.
    fn apply_chat(&mut self, muted: bool, everyone: bool, desktop: ChatDesktop) {
        self.alerts = Some(!muted);
        let on_off = |on: bool| json!(if on { "On" } else { "Off" });
        self.granular.insert("allNewPosts".into(), on_off(!muted));
        self.granular
            .insert("everyone".into(), on_off(everyone && !muted));
        self.granular.insert(
            "dskNotif".into(),
            match desktop {
                ChatDesktop::On => json!("On"),
                ChatDesktop::Off => json!("Off"),
                ChatDesktop::FollowGlobal => Value::Null,
            },
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorTarget {
    Chat(String),
    Channel { team: String, channel: String },
}

#[derive(Debug, Clone)]
struct Editor {
    target: EditorTarget,
    loading: bool,
    saving: bool,
    error: Option<String>,
}

pub(crate) struct NotificationsState {
    prefs: Prefs,
    /// Last document the server has, restored when a write fails.
    confirmed: Prefs,
    loaded: bool,
    /// Id of the latest preferences write; earlier answers are ignored.
    write: u64,
    saving: bool,
    error: Option<String>,
    pub(crate) active: bool,
    permission: NotificationPermission,
    conversations: HashMap<String, ConversationSettings>,
    requested: HashSet<String>,
    /// Banners waiting for their conversation's settings.
    waiting: HashMap<String, Vec<Candidate>>,
    /// Meeting chats joined before their `alerts` were known.
    joined: HashSet<String>,
    channels: HashMap<String, ChannelNotificationSettings>,
    channels_requested: HashSet<String>,
    /// Why NSS cannot be used (no service URL…); stops further requests.
    nss_problem: Option<String>,
    /// `conv|root` → follow state.
    follows: HashMap<String, FollowState>,
    follows_requested: HashSet<String>,
    seen: VecDeque<(String, VecDeque<String>)>,
    push: PushChannel,
    status: String,
    muted_in_call: bool,
    editor: Option<Editor>,
}

impl Default for NotificationsState {
    fn default() -> Self {
        Self {
            prefs: Prefs::default(),
            confirmed: Prefs::default(),
            loaded: false,
            write: 0,
            saving: false,
            error: None,
            active: true,
            permission: NotificationPermission::Unknown,
            conversations: HashMap::new(),
            requested: HashSet::new(),
            waiting: HashMap::new(),
            joined: HashSet::new(),
            channels: HashMap::new(),
            channels_requested: HashSet::new(),
            nss_problem: None,
            follows: HashMap::new(),
            follows_requested: HashSet::new(),
            seen: VecDeque::new(),
            push: PushChannel::default(),
            status: String::new(),
            muted_in_call: false,
            editor: None,
        }
    }
}

impl NotificationsState {
    pub(crate) fn push_live(&self) -> bool {
        self.push.live()
    }
    pub(crate) fn muted_in_call(&self) -> bool {
        self.muted_in_call
    }
    pub(crate) fn end_call(&mut self) {
        self.muted_in_call = false;
    }
}

fn follow_key(conv: &str, root: &str) -> String {
    format!("{conv}|{root}")
}
pub(crate) fn is_channel_id(id: &str) -> bool {
    id.contains("@thread.tacv2") || id.contains("@thread.skype")
}
/// `{conv};messageid={root}` in a conversation link names the reply chain.
fn reply_chain(message: &Message) -> Option<String> {
    let link = message.conversation_link.as_deref()?;
    let (_, rest) = link.rsplit('/').next()?.split_once(";messageid=")?;
    Some(rest.to_owned()).filter(|s| !s.is_empty())
}
fn epoch_ms(time: Option<&str>) -> Option<i64> {
    DateTime::parse_from_rfc3339(time?)
        .ok()
        .map(|t| t.timestamp_millis())
}
pub(super) fn mentions(message: &Message) -> Vec<Value> {
    match message.properties.get("mentions") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_default(),
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}
fn shorten(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        text
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}
fn describe(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl App {
    pub(crate) fn update_notifications(&mut self, msg: NotificationsMsg) -> Task<Msg> {
        match msg {
            NotificationsMsg::Action(action) => self.notification_action(action),
            NotificationsMsg::Active(active) => {
                // Whether the app is in front only decides banners.
                self.notifications.narrowly(Parts::NOTIFICATIONS).active = active;
                Task::none()
            }
            NotificationsMsg::Realtime(g, _)
            | NotificationsMsg::PrefsLoaded(g, _)
            | NotificationsMsg::Saved(g, _, _)
            | NotificationsMsg::ConversationsLoaded(g, _)
            | NotificationsMsg::ConversationSaved(g, _, _, _)
            | NotificationsMsg::ChannelsLoaded(g, _, _)
            | NotificationsMsg::ChannelSaved(g, _, _, _)
            | NotificationsMsg::FollowsLoaded(g, _, _)
            | NotificationsMsg::FollowSaved(g, _, _, _)
            | NotificationsMsg::Replied(g, _, _)
            | NotificationsMsg::ActivityRead(g, _)
                if g != self.generation =>
            {
                Task::none()
            }
            NotificationsMsg::Realtime(_, event) => self.messaging_event(event),
            NotificationsMsg::PrefsLoaded(_, result) => {
                if let Ok(properties) = result {
                    self.preferences_loaded(&properties);
                }
                Task::none()
            }
            NotificationsMsg::Saved(_, write, result) => {
                if write != self.notifications.write {
                    return Task::none();
                }
                let n = &mut *self.notifications;
                n.saving = false;
                match result {
                    Ok(()) => n.confirmed = n.prefs.clone(),
                    Err(error) => {
                        n.prefs = n.confirmed.clone();
                        n.error = Some(tf!("core.notifications.setting_not_saved", error = error));
                    }
                }
                Task::none()
            }
            NotificationsMsg::ConversationsLoaded(_, results) => {
                let mut tasks = Vec::new();
                for (id, result) in results {
                    let settings = match result {
                        Ok(properties) => ConversationSettings::from_properties(&properties),
                        Err(error) => {
                            if let Some(editor) = self
                                .notifications
                                .editor
                                .as_mut()
                                .filter(|e| editor_conversation(&e.target) == id)
                            {
                                editor.error = Some(error);
                            }
                            ConversationSettings::default()
                        }
                    };
                    self.notifications
                        .conversations
                        .entry(id.clone())
                        .or_insert(settings);
                    let notifications = &mut *self.notifications;
                    if let Some(editor) = notifications
                        .editor
                        .as_mut()
                        .filter(|e| editor_conversation(&e.target) == id)
                    {
                        editor.loading = editor_waits_for_channel(
                            &editor.target,
                            &notifications.channels,
                            &notifications.nss_problem,
                        );
                    }
                    for candidate in self.notifications.waiting.remove(&id).unwrap_or_default() {
                        tasks.push(self.deliver(candidate));
                    }
                    if self.notifications.joined.remove(&id) {
                        tasks.push(self.joined_meeting_chat(&id));
                    }
                }
                Task::batch(tasks)
            }
            NotificationsMsg::ConversationSaved(_, id, previous, result) => {
                if let Some(editor) = self.notifications.editor.as_mut() {
                    editor.saving = false;
                }
                if let Err(error) = result {
                    self.notifications.conversations.insert(id, previous);
                    let message = tf!(
                        "core.notifications.notification_setting_not_saved",
                        error = error
                    );
                    match self.notifications.editor.as_mut() {
                        Some(editor) => editor.error = Some(message),
                        None => *self.error = Some(message),
                    }
                }
                Task::none()
            }
            NotificationsMsg::ChannelsLoaded(_, requested, result) => {
                match result {
                    Ok(results) => {
                        let defaults = self.channel_defaults();
                        for item in results {
                            let settings = item.settings.unwrap_or_default().or(defaults);
                            self.notifications
                                .channels
                                .insert(item.channel_id, settings);
                        }
                    }
                    Err(error) => {
                        // Without NSS, channels follow the global settings.
                        self.notifications.nss_problem = Some(error);
                    }
                }
                let mut tasks = Vec::new();
                for id in requested {
                    let notifications = &mut *self.notifications;
                    if let Some(editor) = notifications.editor.as_mut()
                        && editor_conversation(&editor.target) == id
                    {
                        editor.loading = !notifications.conversations.contains_key(&id);
                    }
                    for candidate in self.notifications.waiting.remove(&id).unwrap_or_default() {
                        tasks.push(self.deliver(candidate));
                    }
                }
                Task::batch(tasks)
            }
            NotificationsMsg::ChannelSaved(_, id, previous, result) => {
                if let Some(editor) = self.notifications.editor.as_mut() {
                    editor.saving = false;
                }
                if let Err(error) = result {
                    match previous {
                        Some(previous) => {
                            self.notifications.channels.insert(id, previous);
                        }
                        None => {
                            self.notifications.channels.remove(&id);
                        }
                    }
                    let message = tf!(
                        "core.notifications.channel_setting_not_saved",
                        error = error
                    );
                    match self.notifications.editor.as_mut() {
                        Some(editor) => editor.error = Some(message),
                        None => *self.error = Some(message),
                    }
                }
                Task::none()
            }
            NotificationsMsg::FollowsLoaded(_, keys, result) => {
                if let Ok(states) = result {
                    let conv = keys
                        .first()
                        .and_then(|k| k.split_once('|'))
                        .map(|(c, _)| c.to_owned())
                        .unwrap_or_default();
                    for (root, state) in states {
                        self.notifications
                            .follows
                            .insert(follow_key(&conv, &root), state);
                    }
                }
                Task::none()
            }
            NotificationsMsg::FollowSaved(_, key, previous, result) => {
                if let Err(error) = result {
                    match previous {
                        Some(state) => {
                            self.notifications.follows.insert(key, state);
                        }
                        None => {
                            self.notifications.follows.remove(&key);
                        }
                    }
                    *self.error = Some(tf!(
                        "core.notifications.could_not_change_following_the_post",
                        error = error
                    ));
                }
                Task::none()
            }
            NotificationsMsg::Replied(_, conv, result) => match result {
                Ok(()) if self.selected.as_ref().is_some_and(|s| s.id() == conv) => {
                    self.load_feed(false)
                }
                Ok(()) => Task::none(),
                Err(error) => {
                    *self.error = Some(tf!("core.notifications.reply_not_sent", error = error));
                    Task::none()
                }
            },
            NotificationsMsg::ActivityRead(_, result) => {
                if let Err(error) = result {
                    self.activity.error = Some(tf!(
                        "core.notifications.could_not_mark_the_activity_feed",
                        error = error
                    ));
                }
                Task::none()
            }
        }
    }

    pub(crate) fn notifications_permission(&self) -> NotificationPermission {
        self.notifications.permission
    }
    /// Sign-out resets the state; the system permission does not change with the account.
    pub(crate) fn restore_notifications_permission(&mut self, permission: NotificationPermission) {
        self.notifications.permission = permission;
    }

    // ---- Global preferences ------------------------------------------------

    /// `GET v1/users/ME/properties`, from the workspace load or a push.
    pub(crate) fn preferences_loaded(&mut self, properties: &Value) {
        let n = &mut *self.notifications;
        n.loaded = true;
        // A pending write must not be replaced by the value it is about to overwrite.
        if n.saving {
            return;
        }
        if let Some(prefs) = Prefs::from_properties(properties) {
            n.confirmed = prefs.clone();
            n.prefs = prefs;
        }
    }

    /// Applies `change` at once and PUTs the whole category, as `setUserPreference` does.
    fn write_prefs(&mut self, change: impl FnOnce(&mut Prefs)) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let n = &mut *self.notifications;
        change(&mut n.prefs);
        n.write += 1;
        n.saving = true;
        n.error = None;
        let body = n.prefs.serialized();
        let (generation, write) = (self.generation, n.write);
        Task::perform(
            async move {
                client
                    .set_user_property(rules::PROPERTY, body)
                    .await
                    .map_err(describe)
            },
            move |r| Msg::Notifications(NotificationsMsg::Saved(generation, write, r)),
        )
    }

    fn reload_preferences(&self) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let generation = self.generation;
        Task::perform(
            async move { client.user_properties().await.map_err(describe) },
            move |r| Msg::Notifications(NotificationsMsg::PrefsLoaded(generation, r)),
        )
    }

    fn notification_action(&mut self, action: NotificationsAction) -> Task<Msg> {
        match action {
            NotificationsAction::SetChoice { row, choice } => {
                self.write_prefs(|p| p.set_choice(row, choice))
            }
            NotificationsAction::SetPreset { preset } => {
                self.write_prefs(|p| p.apply_preset(preset))
            }
            NotificationsAction::SetToggle { toggle, on } => {
                self.write_prefs(|p| p.set_toggle(toggle, on))
            }
            NotificationsAction::SetSound { urgent, sound } => {
                self.write_prefs(|p| p.set_sound(urgent, sound))
            }
            NotificationsAction::SetEmailDelay { delay } => {
                self.write_prefs(|p| p.set_email_delay(delay))
            }
            NotificationsAction::SetMeetingChats { choice } => {
                self.write_prefs(|p| p.set_meeting_chats(choice))
            }
            NotificationsAction::Pause { minutes } => {
                let until = Utc::now() + chrono::Duration::minutes(i64::from(minutes.max(1)));
                self.write_prefs(|p| p.set_paused_until(Some(until)))
            }
            NotificationsAction::PauseUntilTomorrow => {
                let tomorrow = Local::now().date_naive().succ_opt().unwrap_or_default();
                let until = tomorrow
                    .and_hms_opt(8, 0, 0)
                    .and_then(|t| t.and_local_timezone(Local).earliest())
                    .map(|t| t.with_timezone(&Utc))
                    .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(12));
                self.write_prefs(|p| p.set_paused_until(Some(until)))
            }
            NotificationsAction::PauseUntil { epoch_ms } => {
                match DateTime::from_timestamp_millis(epoch_ms).filter(|t| *t > Utc::now()) {
                    Some(until) => self.write_prefs(|p| p.set_paused_until(Some(until))),
                    None => {
                        self.notifications.error =
                            Some(t("core.notifications.choose_an_end_date_in_the").into());
                        Task::none()
                    }
                }
            }
            NotificationsAction::Resume => self.write_prefs(|p| p.set_paused_until(None)),
            NotificationsAction::PermissionChanged { permission } => {
                self.notifications.permission = permission;
                Task::none()
            }
            NotificationsAction::Open {
                conversation_id,
                reply_chain_id,
            } => self.open_notification(conversation_id, reply_chain_id),
            NotificationsAction::Reply {
                conversation_id,
                reply_chain_id,
                text,
            } => self.quick_reply(conversation_id, reply_chain_id, text),
            NotificationsAction::MarkRead { conversation_id } => {
                let last = self
                    .chats
                    .iter()
                    .find(|c| c.id == conversation_id)
                    .and_then(|c| c.last_message.as_ref())
                    .map(|m| m.id.clone());
                match last {
                    Some(id) => Task::done(Msg::MarkRead(Selection::Chat(conversation_id), id)),
                    None => Task::effect(Effect::WithdrawNotifications { conversation_id }),
                }
            }
            NotificationsAction::SetChatMuted {
                conversation_id,
                muted,
            } => self.set_chat_muted(conversation_id, muted),
            NotificationsAction::EditChat { conversation_id } => {
                self.open_editor(EditorTarget::Chat(conversation_id))
            }
            NotificationsAction::EditChannel { team, channel } => {
                self.open_editor(EditorTarget::Channel { team, channel })
            }
            NotificationsAction::CloseEditor => {
                self.notifications.editor = None;
                Task::none()
            }
            NotificationsAction::SetChatSettings {
                muted,
                everyone_mentions,
                desktop,
            } => self.save_chat_settings(muted, everyone_mentions, desktop),
            NotificationsAction::SetChannelSettings {
                all_new_posts,
                include_replies,
                channel_mentions,
                follow_all_threads,
            } => {
                let Some(EditorTarget::Channel { team, channel }) =
                    self.notifications.editor.as_ref().map(|e| e.target.clone())
                else {
                    return Task::none();
                };
                let settings = ChannelNotificationSettings {
                    all_new_posts: all_new_posts.into(),
                    include_replies,
                    channel_mentions: channel_mentions.into(),
                };
                Task::batch([
                    self.save_channel_settings(&team, &channel, settings),
                    self.save_follow_all(&channel, follow_all_threads),
                ])
            }
            NotificationsAction::MuteChannel { team, channel } => {
                self.save_channel_settings(&team, &channel, ChannelNotificationSettings::OFF)
            }
            NotificationsAction::SetPostFollow { post, follow } => {
                self.set_post_follow(post, follow)
            }
            NotificationsAction::MarkAllActivityRead => self.mark_all_activity_read(),
            NotificationsAction::SetReadReceipts { on } => self.set_read_receipts(on),
            NotificationsAction::MuteDuringThisCall { muted } => {
                self.notifications.muted_in_call = muted;
                Task::none()
            }
            NotificationsAction::SendTest => {
                let sound = rules::sound(&self.notifications.prefs, false, false);
                Task::effect(Effect::Notify {
                    id: uuid::Uuid::new_v4().to_string(),
                    conversation_id: String::new(),
                    reply_chain_id: None,
                    title: "Oxide".into(),
                    subtitle: None,
                    body: t("core.notifications.notifications_are_working").into(),
                    sound,
                    urgent: false,
                    can_reply: false,
                })
            }
        }
    }

    // ---- Chat push channel --------------------------------------------------

    /// Translates the push channel state again when it carries no details.
    pub(crate) fn relocalize_notifications(&mut self) {
        if let Some(text) = PUSH_TEXTS.relocalized(&self.notifications.status) {
            self.notifications.status = text.into();
        }
    }

    /// Connects the CDL push channel once signed in (independent of native calls).
    pub(crate) fn sync_messaging(&mut self) -> Task<Msg> {
        let Some(client) = self.client.as_ref() else {
            self.notifications.push.halt();
            return Task::none();
        };
        if self.notifications.push.running() {
            return Task::none();
        }
        let auth = client.authenticator();
        let g = self.generation;
        let task = self.notifications.push.start(
            auth,
            |endpoint| RealtimeConfig::messaging(endpoint, uuid::Uuid::new_v4().to_string()),
            move |event| Msg::Notifications(NotificationsMsg::Realtime(g, event)),
        );
        self.notifications.status = t(PUSH_TEXTS.connecting).into();
        task
    }

    fn messaging_event(&mut self, event: RealtimeEvent) -> Task<Msg> {
        let mut catch_up = false;
        let change = match Incoming::from(event) {
            Incoming::Change(change) => change,
            Incoming::Request(request) => return self.pushed_message(&request),
            // Pushes were dropped: reload what they would have changed.
            Incoming::MessageLoss => return self.catch_up(),
            Incoming::Diagnostic(_) => return Task::none(),
        };
        self.notifications.status = match self.notifications.push.apply(&PUSH_TEXTS, change) {
            Transition::Registered { status, resumed } => {
                // Pushes sent while the channel was down are lost: catch up once, then ticks stop polling.
                catch_up = resumed;
                status
            }
            Transition::Connected { status, .. }
            | Transition::Down(status)
            | Transition::Stopped(status) => status,
        };
        if catch_up {
            self.catch_up()
        } else {
            Task::none()
        }
    }

    /// One `EventMessage` POSTed to `/messaging` (or a long-poll style batch).
    fn pushed_message(&mut self, request: &PushedRequest) -> Task<Msg> {
        let Ok(mut value) = serde_json::from_str::<Value>(&request.body) else {
            return Task::none();
        };
        let events: Vec<Value> = match value.get_mut("eventMessages").map(Value::take) {
            Some(Value::Array(events)) => events,
            _ => vec![value],
        };
        let tasks: Vec<Task<Msg>> = events
            .into_iter()
            .filter_map(|v| serde_json::from_value::<teams_chat::Event>(v).ok())
            .map(|event| self.chat_event(event))
            .collect();
        Task::batch(tasks)
    }

    pub(crate) fn chat_event(&mut self, event: teams_chat::Event) -> Task<Msg> {
        // Decoded once: each `message()` clones and deserializes the resource.
        let message = match event.resource_type {
            ResourceType::NewMessage | ResourceType::MessageUpdate => event.message(),
            _ => None,
        };
        // A call log written or updated: the web client matches the thread type, not the conversation id.
        if message.as_ref().is_some_and(|m| {
            m.threadtype.as_deref() == Some("streamofcalllogs")
                || m.conversation_id() == Some(teams_calling::history::CALL_LOGS)
        }) {
            return self.load_calls(true);
        }
        match event.resource_type {
            ResourceType::NewMessage => match message {
                Some(message) => self.new_message(message),
                None => Task::none(),
            },
            ResourceType::MessageUpdate => {
                let conv = message.and_then(|m| m.conversation_id().map(str::to_owned));
                if conv.is_some_and(|c| self.selected.as_ref().is_some_and(|s| s.id() == c)) {
                    self.load_feed(false)
                } else {
                    Task::none()
                }
            }
            ResourceType::CustomUserProperties => self.reload_preferences(),
            ResourceType::ConversationUpdate | ResourceType::ThreadUpdate => {
                // Personal properties (alerts…) may have changed on another client.
                let id = event
                    .resource
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| event.resource_link.rsplit('/').next().map(str::to_owned));
                if let Some(id) = id {
                    self.notifications.requested.remove(&id);
                    self.notifications.conversations.remove(&id);
                }
                Task::none()
            }
            ResourceType::Other => Task::none(),
        }
    }

    fn new_message(&mut self, message: Message) -> Task<Msg> {
        let Some(conv) = message.conversation_id().map(str::to_owned) else {
            return Task::none();
        };
        // Typing and read positions only change the open chat's indicators.
        if let Some(task) = self.control_message(&conv, &message) {
            return task;
        }
        self.stop_typing(&conv, message.sender_mri());
        if is_channel_id(&conv) {
            self.channel_message(&conv, &message);
        }
        // Keep the screen current: the push replaces waiting for the next poll.
        let refresh = if conv == activity::FEED {
            self.load_activity(false)
        } else if self.selected.as_ref().is_some_and(|s| s.id() == conv) {
            self.load_feed(false)
        } else if !is_channel_id(&conv) {
            self.load_workspace()
        } else {
            Task::none()
        };
        let candidate = if conv == activity::FEED {
            self.activity_candidate(&message)
        } else {
            self.message_candidate(&conv, &message)
        };
        let notify = candidate
            .map(|c| self.consider(c))
            .unwrap_or_else(Task::none);
        Task::batch([refresh, notify])
    }

    /// A post or reply makes the channel unread until it is opened.
    fn channel_message(&mut self, conv: &str, message: &Message) {
        let kind = message.messagetype.as_str();
        if !(kind == "Text" || kind.starts_with("RichText")) {
            return;
        }
        let channel = self
            .teams
            .iter_mut()
            .flat_map(|t| t.channels.iter_mut())
            .find(|c| c.id == conv);
        if let Some(channel) = channel {
            let newer = channel.last_message().is_none_or(|last| {
                conversations::message_time(message) > conversations::message_time(&last)
            });
            if newer {
                channel.set_last_message(message);
            }
        }
    }

    fn message_candidate(&self, conv: &str, message: &Message) -> Option<Candidate> {
        let kind_text = message.messagetype.as_str();
        if kind_text == "Event/Call" {
            return self.meeting_started_candidate(conv, message);
        }
        if !(kind_text == "Text" || kind_text.starts_with("RichText")) {
            return None;
        }
        let me = self.me.as_str();
        let channel = is_channel_id(conv);
        let reply_chain_id = reply_chain(message).filter(|rc| *rc != message.id);
        let mut kind = if channel {
            match &reply_chain_id {
                None => Kind::ChannelPost,
                Some(root) if self.follows_thread(conv, root) => Kind::FollowedThreadReply,
                Some(_) => Kind::ChannelReply,
            }
        } else {
            Kind::Chat
        };
        for mention in mentions(message) {
            let mri = activity::field(&mention, "mri");
            let kind_of = activity::field(&mention, "mentionType");
            let mention_kind = if !me.is_empty() && mri.eq_ignore_ascii_case(me) {
                Some(Kind::PersonalMention)
            } else {
                match kind_of.as_str() {
                    "everyone" if !channel => Some(Kind::EveryoneMention),
                    "team" | "channel" if channel => Some(Kind::TeamMention),
                    "tag" => Some(Kind::TagMention),
                    _ => None,
                }
            };
            if let Some(found) = mention_kind
                && kind != Kind::PersonalMention
            {
                kind = found;
            }
        }
        let sender = message
            .imdisplayname
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| {
                self.profiles
                    .get(message.sender_mri())
                    .map(|p| p.display_name.clone())
            })
            .unwrap_or_else(|| t("core.notifications.someone").into());
        let text = html::to_text(&message.content);
        let (title, subtitle) = if channel {
            let place = self
                .channel_place(conv)
                .unwrap_or_else(|| t("core.notifications.channel").into());
            (sender, Some(place))
        } else {
            let chat = self.chats.iter().find(|c| c.id == conv);
            match chat {
                Some(c) if c.is_group() || c.title().is_some() => (chat_title(c, me), Some(sender)),
                _ => (sender, None),
            }
        };
        Some(Candidate {
            kind,
            conversation_id: conv.to_owned(),
            message_id: message.id.clone(),
            reply_chain_id,
            from_me: !me.is_empty() && message.sender_mri().eq_ignore_ascii_case(me),
            title,
            subtitle,
            body: shorten(&text, 240),
            time_ms: epoch_ms(
                message
                    .originalarrivaltime
                    .as_deref()
                    .or(message.composetime.as_deref()),
            )
            .unwrap_or_else(|| Utc::now().timestamp_millis()),
            urgent: conversation::Importance::of(message) == conversation::Importance::Urgent,
            deleted: message.is_deleted(),
            display_banner: display_banner(&message.properties),
            from_activity: false,
        })
    }

    /// `Event/Call` with `<partlist type="started">` in a meeting chat: the meeting is live.
    fn meeting_started_candidate(&self, conv: &str, message: &Message) -> Option<Candidate> {
        if !message.content.contains("type=\"started\"") || is_channel_id(conv) {
            return None;
        }
        let chat = self
            .chats
            .iter()
            .find(|c| c.id == conv)
            .filter(|c| c.is_meeting())?;
        Some(Candidate {
            kind: Kind::MeetingStarted,
            conversation_id: conv.to_owned(),
            message_id: message.id.clone(),
            reply_chain_id: None,
            from_me: !self.me.is_empty() && message.sender_mri().eq_ignore_ascii_case(&self.me),
            title: chat_title(chat, &self.me),
            subtitle: None,
            body: t("core.notifications.the_meeting_has_started").into(),
            time_ms: epoch_ms(message.originalarrivaltime.as_deref())
                .unwrap_or_else(|| Utc::now().timestamp_millis()),
            urgent: false,
            deleted: false,
            display_banner: None,
            from_activity: false,
        })
    }

    /// An item of `48:notifications`: mentions in channels, replies, reactions…
    fn activity_candidate(&self, message: &Message) -> Option<Candidate> {
        let data = activity::metadata(message);
        let kind_text = activity::field(&data, "activityType");
        let subtype = activity::field(&data, "activitySubtype");
        let kind = match (kind_text.as_str(), subtype.as_str()) {
            ("mentionInChat", "everyone") => Kind::EveryoneMention,
            ("mentionInChat" | "mention" | "mentions", "person" | "mention" | "") => {
                Kind::PersonalMention
            }
            ("mention", "team" | "channel") => Kind::TeamMention,
            ("mention", "tag") => Kind::TagMention,
            ("reply" | "replyToReply" | "replyInChat", _) => Kind::Reply,
            ("like" | "likeInChat" | "reaction" | "reactionInChat", _) => Kind::Reaction,
            // Chat messages come straight from the conversation; calls have their own window.
            ("chat" | "call" | "missedCall" | "voicemail" | "callOrMeetup", _) => return None,
            ("", _) => return None,
            _ => Kind::Other,
        };
        let source = activity::field(&data, "sourceThreadId");
        let actor = activity::field(&data, "sourceUserImDisplayName");
        let preview = activity::field(&data, "messagePreview");
        let label = activity::label(&kind_text);
        let topic = activity::field(&data, "sourceThreadTopic");
        let source_message = activity::field(&data, "sourceMessageId");
        let reply_chain = activity::field(&data, "sourceReplyChainId");
        Some(Candidate {
            kind,
            message_id: if kind == Kind::Reaction || source_message.is_empty() {
                // Each reaction is its own event on the same message.
                format!("activity:{}", message.id)
            } else {
                source_message.clone()
            },
            reply_chain_id: Some(reply_chain).filter(|rc| !rc.is_empty() && *rc != source_message),
            conversation_id: source,
            from_me: !self.me.is_empty()
                && activity::field(&data, "sourceUserId").eq_ignore_ascii_case(&self.me),
            title: if actor.is_empty() {
                t("core.notifications.activity").into()
            } else {
                actor
            },
            subtitle: Some(topic).filter(|t| !t.is_empty()),
            body: if preview.is_empty() {
                label.to_owned()
            } else {
                tf!(
                    "core.notifications.text",
                    label = label,
                    preview = shorten(&preview, 220)
                )
            },
            time_ms: epoch_ms(Some(&activity::field(&data, "activityTimestamp")))
                .or_else(|| epoch_ms(message.originalarrivaltime.as_deref()))
                .unwrap_or_else(|| Utc::now().timestamp_millis()),
            urgent: kind_text == "priority",
            deleted: false,
            display_banner: display_banner(&message.properties)
                .or_else(|| data.as_object().and_then(display_banner)),
            from_activity: true,
        })
    }

    fn follows_thread(&self, conv: &str, root: &str) -> bool {
        match self.notifications.follows.get(&follow_key(conv, root)) {
            Some(FollowState::ExplicitFollow | FollowState::ImplicitFollow) => true,
            Some(FollowState::ExplicitUnfollow) => false,
            _ => {
                rules::follows_all(
                    &self
                        .notifications
                        .conversations
                        .get(conv)
                        .map(|c| c.follow_all_rc.clone())
                        .unwrap_or_default(),
                ) || self.participates_in(conv, root)
            }
        }
    }
    /// Author or replier: implicit follow.
    fn participates_in(&self, conv: &str, root: &str) -> bool {
        self.selected.as_ref().is_some_and(|s| s.id() == conv)
            && self
                .conv
                .posts()
                .iter()
                .any(|p| p.id == root && p.messages.iter().any(|m| m.sender_mri() == *self.me))
    }

    /// Fetches the conversation's settings first when they are unknown.
    fn consider(&mut self, candidate: Candidate) -> Task<Msg> {
        let conv = candidate.conversation_id.clone();
        if conv.is_empty() || candidate.from_me {
            return Task::none();
        }
        let channel = is_channel_id(&conv);
        let missing_conv = !self.notifications.conversations.contains_key(&conv);
        let missing_channel = channel
            && self.notifications.nss_problem.is_none()
            && !self.notifications.channels.contains_key(&conv);
        if !(missing_conv || missing_channel) || self.client.is_none() {
            return self.deliver(candidate);
        }
        let already = self.notifications.waiting.contains_key(&conv);
        self.notifications
            .waiting
            .entry(conv.clone())
            .or_default()
            .push(candidate);
        if already {
            return Task::none();
        }
        let mut tasks = Vec::new();
        if missing_conv {
            self.notifications.requested.insert(conv.clone());
            tasks.push(self.fetch_conversations(vec![conv.clone()]));
        }
        if missing_channel {
            tasks.push(self.fetch_channels(vec![conv]));
        }
        Task::batch(tasks)
    }

    fn rules_for(&self, candidate: &Candidate) -> ConversationRules {
        let conv = &candidate.conversation_id;
        let muted = self.chat_muted(conv);
        let desktop = match self
            .notifications
            .conversations
            .get(conv)
            .map(ConversationSettings::desktop)
        {
            Some(ChatDesktop::On) => Some(true),
            Some(ChatDesktop::Off) => Some(false),
            _ => None,
        };
        let channel_banner = self.notifications.channels.get(conv).map(|s| {
            let banner = |c: NssChoice| c == NssChoice::BannerAndFeed;
            match candidate.kind {
                Kind::ChannelPost => banner(s.all_new_posts),
                Kind::ChannelReply => s.include_replies && banner(s.all_new_posts),
                Kind::TeamMention => banner(s.channel_mentions),
                _ => true,
            }
        });
        ConversationRules {
            muted,
            desktop: if is_channel_id(conv) { None } else { desktop },
            channel_banner,
        }
    }

    /// Meeting chats without an explicit `alerts` follow `meetingChatNotification`.
    fn meeting_chat_muted(&self, conv: &str) -> bool {
        let Some(chat) = self
            .chats
            .iter()
            .find(|c| c.id == conv && c.is_meeting())
        else {
            return false;
        };
        let rsvp = self
            .calendar
            .meeting_response(conv)
            .map(|(response, organizer)| rules::Rsvp::from_response(&response, organizer));
        let participated = chat
            .last_message
            .as_ref()
            .is_some_and(|m| m.sender_mri() == *self.me);
        self.notifications
            .prefs
            .meeting_chat_muted(rsvp, participated)
    }

    /// Records the message and returns whether it was already seen.
    fn seen(&mut self, conv: &str, message: &str) -> bool {
        let seen = &mut self.notifications.seen;
        if let Some(index) = seen.iter().position(|(c, _)| c == conv) {
            let mut entry = seen.remove(index).expect("index is valid");
            let duplicate = entry.1.iter().any(|m| m == message);
            if !duplicate {
                entry.1.push_back(message.to_owned());
                if entry.1.len() > SEEN_PER_CHAT {
                    entry.1.pop_front();
                }
            }
            seen.push_back(entry);
            return duplicate;
        }
        seen.push_back((conv.to_owned(), VecDeque::from([message.to_owned()])));
        if seen.len() > SEEN_CHATS {
            seen.pop_front();
        }
        false
    }

    fn deliver(&mut self, candidate: Candidate) -> Task<Msg> {
        let duplicate = self.seen(&candidate.conversation_id, &candidate.message_id);
        let visible = self.notifications.active
            && matches!(*self.nav, Nav::Chat | Nav::Teams)
            && self
                .selected
                .as_ref()
                .is_some_and(|s| s.id() == candidate.conversation_id)
            && (candidate.reply_chain_id.is_none()
                || (self.conv.reply().is_some()
                    && self.conv.reply() == candidate.reply_chain_id.as_ref())
                || self.selected.as_ref().is_some_and(Selection::is_channel));
        let in_call = self.call_in_progress();
        let context = rules::Context {
            prefs: &self.notifications.prefs,
            now: Utc::now(),
            conversation: self.rules_for(&candidate),
            visible,
            in_call,
            muted_in_call: self.notifications.muted_in_call,
            duplicate,
        };
        if rules::decide(&candidate, &context).is_err() {
            return Task::none();
        }
        let prefs = &self.notifications.prefs;
        let body = if prefs.toggle(NotificationToggle::ShowPreview) {
            candidate.body.clone()
        } else if candidate.kind.is_mention() {
            t("core.notifications.mentioned_you").into()
        } else {
            t("core.notifications.new_message").into()
        };
        Task::effect(Effect::Notify {
            id: format!("{}|{}", candidate.conversation_id, candidate.message_id),
            conversation_id: candidate.conversation_id.clone(),
            reply_chain_id: candidate.reply_chain_id.clone(),
            title: candidate.title.clone(),
            subtitle: candidate.subtitle.clone(),
            body,
            sound: rules::sound(prefs, candidate.urgent, in_call),
            urgent: candidate.urgent,
            can_reply: !candidate.conversation_id.starts_with("48:"),
        })
    }

    // ---- Banner actions -----------------------------------------------------

    fn open_notification(&mut self, conv: String, reply_chain: Option<String>) -> Task<Msg> {
        if conv.is_empty() {
            return Task::none();
        }
        let selection = if is_channel_id(&conv) {
            match self.thread_team(&conv) {
                Some(team) => Selection::Channel {
                    team: team.id.clone(),
                    channel: conv,
                },
                None => return Task::none(),
            }
        } else {
            Selection::Chat(conv)
        };
        let nav = if selection.is_channel() {
            Nav::Teams
        } else {
            Nav::Chat
        };
        let navigate = Task::done(Msg::Navigate(nav));
        let select = Task::done(Msg::Select(selection));
        let reply = reply_chain
            .map(|rc| Task::done(ConversationMsg::OpenReply(rc).into()))
            .unwrap_or_else(Task::none);
        Task::batch([navigate, select, reply])
    }

    fn quick_reply(
        &mut self,
        conv: String,
        reply_chain: Option<String>,
        text: String,
    ) -> Task<Msg> {
        let text = text.trim().to_owned();
        let Some(client) = self.client.clone().filter(|_| !text.is_empty()) else {
            return Task::none();
        };
        let html = format!(
            "<p>{}</p>",
            text.lines().map(escape).collect::<Vec<_>>().join("<br>")
        );
        let message = teams_chat::OutgoingMessage::html(
            uuid::Uuid::new_v4().as_u128().to_string(),
            html,
            self.display_name.clone(),
        );
        let generation = self.generation;
        let id = conv.clone();
        Task::perform(
            async move {
                client
                    .send_message(&id, reply_chain.as_deref(), &message)
                    .await
                    .map(drop)
                    .map_err(describe)
            },
            move |r| Msg::Notifications(NotificationsMsg::Replied(generation, conv, r)),
        )
    }

    // ---- Chats --------------------------------------------------------------

    pub(crate) fn chat_muted(&self, conv: &str) -> bool {
        match self
            .notifications
            .conversations
            .get(conv)
            .and_then(|s| s.alerts)
        {
            Some(alerts) => !alerts,
            None => self.meeting_chat_muted(conv),
        }
    }

    fn set_chat_muted(&mut self, conv: String, muted: bool) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let entry = self
            .notifications
            .conversations
            .entry(conv.clone())
            .or_default();
        let previous = entry.clone();
        entry.alerts = Some(!muted);
        let generation = self.generation;
        let id = conv.clone();
        Task::perform(
            async move {
                client
                    .set_conversation_property(&id, "alerts", json!((!muted).to_string()))
                    .await
                    .map(drop)
                    .map_err(describe)
            },
            move |r| {
                Msg::Notifications(NotificationsMsg::ConversationSaved(
                    generation, conv, previous, r,
                ))
            },
        )
    }

    /// With `meetingChatNotification = participated`, joining sets an explicit `alerts = true` unless already set.
    pub(crate) fn joined_meeting_chat(&mut self, thread: &str) -> Task<Msg> {
        if is_channel_id(thread)
            || self.notifications.prefs.meeting_chats() != MeetingChats::Participated
        {
            return Task::none();
        }
        match self.notifications.conversations.get(thread) {
            // An explicit choice, muted or not, is the user's.
            Some(settings) if settings.alerts.is_some() => Task::none(),
            Some(_) => self.set_chat_muted(thread.to_owned(), false),
            None => {
                self.notifications.joined.insert(thread.to_owned());
                if self.notifications.requested.insert(thread.to_owned()) {
                    self.fetch_conversations(vec![thread.to_owned()])
                } else {
                    Task::none()
                }
            }
        }
    }

    fn save_chat_settings(
        &mut self,
        muted: bool,
        everyone: bool,
        desktop: ChatDesktop,
    ) -> Task<Msg> {
        let Some(EditorTarget::Chat(conv)) =
            self.notifications.editor.as_ref().map(|e| e.target.clone())
        else {
            return Task::none();
        };
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let entry = self
            .notifications
            .conversations
            .entry(conv.clone())
            .or_default();
        let previous = entry.clone();
        entry.apply_chat(muted, everyone, desktop);
        let granular = Value::Object(entry.granular.clone()).to_string();
        if let Some(editor) = self.notifications.editor.as_mut() {
            editor.saving = true;
            editor.error = None;
        }
        let generation = self.generation;
        let id = conv.clone();
        Task::perform(
            async move {
                // Both properties are written; a partial success reports what failed.
                let (settings, alerts) = tokio::join!(
                    client.set_conversation_property(
                        &id,
                        "conversationNotificationSettings",
                        json!(granular)
                    ),
                    client.set_conversation_property(&id, "alerts", json!((!muted).to_string())),
                );
                match (settings, alerts) {
                    (Ok(_), Ok(_)) => Ok(()),
                    (Err(e), _) => Err(format!("conversationNotificationSettings : {e}")),
                    (_, Err(e)) => Err(format!("alerts : {e}")),
                }
            },
            move |r| {
                Msg::Notifications(NotificationsMsg::ConversationSaved(
                    generation, conv, previous, r,
                ))
            },
        )
    }

    fn open_editor(&mut self, target: EditorTarget) -> Task<Msg> {
        let conv = editor_conversation(&target).to_owned();
        let channel = matches!(target, EditorTarget::Channel { .. });
        let need_conv = !self.notifications.conversations.contains_key(&conv);
        let need_channel = channel
            && self.notifications.nss_problem.is_none()
            && !self.notifications.channels.contains_key(&conv);
        self.notifications.editor = Some(Editor {
            target,
            loading: need_conv || need_channel,
            saving: false,
            error: None,
        });
        let mut tasks = Vec::new();
        if need_conv {
            self.notifications.requested.insert(conv.clone());
            tasks.push(self.fetch_conversations(vec![conv.clone()]));
        }
        if need_channel {
            tasks.push(self.fetch_channels(vec![conv]));
        }
        Task::batch(tasks)
    }

    fn fetch_conversations(&self, ids: Vec<String>) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let generation = self.generation;
        Task::perform(
            async move {
                let mut results = Vec::new();
                for id in ids {
                    let result = client
                        .conversation(&id)
                        .await
                        .map(|c| c.properties)
                        .map_err(describe);
                    results.push((id, result));
                }
                results
            },
            move |r| Msg::Notifications(NotificationsMsg::ConversationsLoaded(generation, r)),
        )
    }

    // ---- Channels -----------------------------------------------------------

    fn channel_defaults(&self) -> ChannelNotificationSettings {
        ChannelNotificationSettings::for_preset(
            self.notifications.prefs.preset() != NotificationPreset::MyAtMentions,
        )
    }

    fn channel_tenant(&self, channel: &str) -> Option<String> {
        self.teams
            .iter()
            .flat_map(|t| t.channels.iter())
            .find(|c| c.id == channel)
            .and_then(|c| {
                ["tenantId", "threadTenantId"]
                    .into_iter()
                    .find_map(|k| c.extra.get(k).and_then(Value::as_str))
                    .map(str::to_owned)
            })
    }

    fn fetch_channels(&mut self, ids: Vec<String>) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        self.notifications
            .channels_requested
            .extend(ids.iter().cloned());
        let tenant = ids.first().and_then(|id| self.channel_tenant(id));
        let generation = self.generation;
        Task::perform(
            async move {
                let result = client
                    .batch_channel_notification_settings(&ids, tenant.as_deref())
                    .await
                    .map_err(describe);
                (ids, result)
            },
            move |(ids, r)| {
                Msg::Notifications(NotificationsMsg::ChannelsLoaded(generation, ids, r))
            },
        )
    }

    fn save_channel_settings(
        &mut self,
        team: &str,
        channel: &str,
        settings: ChannelNotificationSettings,
    ) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        if self.notifications.channels.get(channel) == Some(&settings) {
            return Task::none();
        }
        let previous = self
            .notifications
            .channels
            .insert(channel.to_owned(), settings);
        if let Some(editor) = self.notifications.editor.as_mut() {
            editor.saving = true;
            editor.error = None;
        }
        let tenant = self.channel_tenant(channel);
        let generation = self.generation;
        let (team, id) = (team.to_owned(), channel.to_owned());
        let key = id.clone();
        Task::perform(
            async move {
                client
                    .set_channel_notification_settings(&team, &id, tenant.as_deref(), &settings)
                    .await
                    .map_err(describe)
            },
            move |r| {
                Msg::Notifications(NotificationsMsg::ChannelSaved(generation, key, previous, r))
            },
        )
    }

    fn save_follow_all(&mut self, channel: &str, on: bool) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let entry = self
            .notifications
            .conversations
            .entry(channel.to_owned())
            .or_default();
        if rules::follows_all(&entry.follow_all_rc) == on {
            return Task::none();
        }
        let previous = entry.clone();
        let at = self
            .conv
            .posts()
            .iter()
            .flat_map(|p| p.messages.iter())
            .filter_map(conversations::message_time)
            .max()
            .filter(|_| self.selected.as_ref().is_some_and(|s| s.id() == channel))
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        entry.follow_all_rc = rules::follow_all_rc(&entry.follow_all_rc, on, at);
        let value = entry.follow_all_rc.clone();
        let generation = self.generation;
        let id = channel.to_owned();
        let conv = id.clone();
        Task::perform(
            async move {
                client
                    .set_conversation_property(&conv, "followAllRc", json!(value))
                    .await
                    .map(drop)
                    .map_err(describe)
            },
            move |r| {
                Msg::Notifications(NotificationsMsg::ConversationSaved(
                    generation, id, previous, r,
                ))
            },
        )
    }

    pub(crate) fn channel_muted(&self, channel: &str) -> bool {
        self.notifications.channels.get(channel) == Some(&ChannelNotificationSettings::OFF)
    }

    // ---- Followed posts -----------------------------------------------------

    pub(crate) fn post_followed(&self, conv: &str, root: &str) -> bool {
        self.follows_thread(conv, root)
    }

    fn set_post_follow(&mut self, post: String, follow: bool) -> Task<Msg> {
        let (Some(client), Some(conv)) = (
            self.client.clone(),
            self.selected
                .as_ref()
                .filter(|s| s.is_channel())
                .map(|s| s.id().to_owned()),
        ) else {
            return Task::none();
        };
        let key = follow_key(&conv, &post);
        let state = if follow {
            FollowState::ExplicitFollow
        } else {
            FollowState::ExplicitUnfollow
        };
        let previous = self.notifications.follows.insert(key.clone(), state);
        let me = self.me.clone();
        let generation = self.generation;
        Task::perform(
            async move {
                client
                    .set_reply_chain_follow(&me, &conv, &post, follow)
                    .await
                    .map_err(describe)
            },
            move |r| {
                Msg::Notifications(NotificationsMsg::FollowSaved(generation, key, previous, r))
            },
        )
    }

    // ---- Activity -----------------------------------------------------------

    fn mark_all_activity_read(&mut self) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let now = Utc::now().timestamp_millis();
        self.activity.mark_all_locally_read(now);
        let generation = self.generation;
        let bookmark = format!("0;{now};0");
        Task::batch([
            Task::perform(
                async move {
                    client
                        .set_conversation_property(
                            activity::FEED,
                            "consumptionHorizonBookmark",
                            json!(bookmark),
                        )
                        .await
                        .map(drop)
                        .map_err(describe)
                },
                move |r| Msg::Notifications(NotificationsMsg::ActivityRead(generation, r)),
            ),
            Task::effect(Effect::WithdrawNotifications {
                conversation_id: activity::FEED.into(),
            }),
        ])
    }

    // ---- Background reads -----------------------------------------------------

    /// Idempotent read-ahead: first chats' `alerts`, shown channels' NSS, open channel's post follow states.
    pub(crate) fn load_notification_settings(&mut self) -> Task<Msg> {
        if self.client.is_none() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        let chats: Vec<String> = self
            .chats
            .iter()
            .take(PREFETCHED_CHATS)
            .map(|c| c.id.clone())
            .filter(|id| !self.notifications.requested.contains(id))
            .collect();
        if !chats.is_empty() {
            self.notifications.requested.extend(chats.iter().cloned());
            tasks.push(self.fetch_conversations(chats));
        }
        if self.notifications.nss_problem.is_none() && *self.nav == Nav::Teams {
            let channels: Vec<String> = self
                .teams
                .iter()
                .filter(|t| self.expanded.contains(&t.id))
                .flat_map(|t| t.channels.iter().map(|c| c.id.clone()))
                .filter(|id| !self.notifications.channels_requested.contains(id))
                .take(50)
                .collect();
            if !channels.is_empty() {
                tasks.push(self.fetch_channels(channels));
            }
        }
        if let Some(Selection::Channel { channel, .. }) = self.selected.clone() {
            let roots: Vec<String> = self
                .conv
                .posts()
                .iter()
                .map(|p| p.id.clone())
                .filter(|root| {
                    !self
                        .notifications
                        .follows_requested
                        .contains(&follow_key(&channel, root))
                })
                .take(50)
                .collect();
            if !roots.is_empty()
                && let Some(client) = self.client.clone()
            {
                let keys: Vec<String> = roots.iter().map(|r| follow_key(&channel, r)).collect();
                self.notifications
                    .follows_requested
                    .extend(keys.iter().cloned());
                let (me, generation) = (self.me.clone(), self.generation);
                tasks.push(Task::perform(
                    async move {
                        client
                            .reply_chain_follow_states(&me, &channel, &roots)
                            .await
                            .map_err(describe)
                    },
                    move |r| {
                        Msg::Notifications(NotificationsMsg::FollowsLoaded(generation, keys, r))
                    },
                ));
            }
        }
        Task::batch(tasks)
    }

    // ---- View -----------------------------------------------------------------

    pub(crate) fn notifications_view(&self) -> NotificationsView {
        let n = &*self.notifications;
        let p = &n.prefs;
        let now = Utc::now();
        NotificationsView {
            loaded: n.loaded,
            saving: n.saving,
            error: n.error.clone(),
            permission: n.permission,
            realtime: n.status.clone(),
            realtime_live: n.push.live(),
            preset: p.preset(),
            rows: NotificationRow::ALL
                .into_iter()
                .map(|row| NotificationRowView {
                    row,
                    section: row.section(),
                    title: row.title().into(),
                    choice: p.choice(row),
                })
                .collect(),
            show_preview: p.toggle(NotificationToggle::ShowPreview),
            mute_during_meetings: p.toggle(NotificationToggle::MuteDuringMeetings),
            sounds: p.toggle(NotificationToggle::Sounds),
            call_sounds: p.toggle(NotificationToggle::CallSounds),
            sounds_when_busy: p.toggle(NotificationToggle::SoundsWhenBusy),
            sounds_for_urgent: p.toggle(NotificationToggle::SoundsForUrgent),
            only_urgent: p.toggle(NotificationToggle::OnlyUrgent),
            sound: p.sound(false),
            urgent_sound: p.sound(true),
            email_delay: p.email_delay(),
            meeting_chats: p.meeting_chats(),
            paused_until: p.paused_until(now).map(|until| {
                let local = until.with_timezone(&Local);
                if local.date_naive() == Local::now().date_naive() {
                    local.format("%H:%M").to_string()
                } else {
                    local.format("%d/%m %H:%M").to_string()
                }
            }),
            muted_in_call: n.muted_in_call,
            read_receipts: self.read_receipts_on(),
            editor: n.editor.as_ref().map(|e| self.editor_view(e)),
        }
    }

    fn editor_view(&self, editor: &Editor) -> NotificationEditorView {
        let conv = editor_conversation(&editor.target);
        let settings = self
            .notifications
            .conversations
            .get(conv)
            .cloned()
            .unwrap_or_default();
        let channel = self
            .notifications
            .channels
            .get(conv)
            .copied()
            .unwrap_or_else(|| self.channel_defaults());
        let (title, is_channel) = match &editor.target {
            EditorTarget::Chat(id) => (
                self.chats
                    .iter()
                    .find(|c| &c.id == id)
                    .map(|c| chat_title(c, &self.me))
                    .unwrap_or_else(|| t("core.notifications.chat").into()),
                false,
            ),
            EditorTarget::Channel { channel, .. } => (
                self.teams
                    .iter()
                    .flat_map(|t| t.channels.iter())
                    .find(|c| &c.id == channel)
                    .map(|c| c.display_name.clone())
                    .unwrap_or_else(|| t("core.notifications.channel").into()),
                true,
            ),
        };
        NotificationEditorView {
            title,
            channel: is_channel,
            loading: editor.loading,
            saving: editor.saving,
            error: editor.error.clone().or_else(|| {
                is_channel
                    .then(|| self.notifications.nss_problem.clone())
                    .flatten()
                    .map(|e| tf!("core.notifications.channel_settings_unavailable", error = e))
            }),
            muted: self.chat_muted(conv),
            everyone_mentions: settings.everyone(
                self.notifications
                    .prefs
                    .banner(NotificationRow::EveryoneMentions),
            ),
            desktop: settings.desktop(),
            all_new_posts: channel.all_new_posts.into(),
            include_replies: channel.include_replies,
            channel_mentions: channel.channel_mentions.into(),
            follow_all_threads: rules::follows_all(&settings.follow_all_rc),
        }
    }
}

fn editor_conversation(target: &EditorTarget) -> &str {
    match target {
        EditorTarget::Chat(id) => id,
        EditorTarget::Channel { channel, .. } => channel,
    }
}
fn editor_waits_for_channel(
    target: &EditorTarget,
    channels: &HashMap<String, ChannelNotificationSettings>,
    nss_problem: &Option<String>,
) -> bool {
    match target {
        EditorTarget::Channel { channel, .. } => {
            nss_problem.is_none() && !channels.contains_key(channel)
        }
        EditorTarget::Chat(_) => false,
    }
}
fn display_banner(properties: &Map<String, Value>) -> Option<String> {
    ["displayBannerV2", "displayBanner"]
        .into_iter()
        .find_map(|key| match properties.get(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Job;

    /// Runs `task` and its follow-ups like the core thread (no push subscriptions); returns the effects.
    async fn drive(app: &mut App, task: Task<Msg>) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut queue = vec![task];
        let mut steps = 0;
        while let Some(task) = queue.pop() {
            for job in task.0 {
                match job {
                    Job::Effect(effect) => effects.push(effect),
                    Job::Future(future) => {
                        steps += 1;
                        assert!(steps < 500, "tasks never settle");
                        let msg = future.await;
                        queue.push(app.update(msg));
                    }
                    Job::Done(msg) => queue.push(app.update(msg)),
                    Job::Subscribe(_) => {}
                }
            }
        }
        effects
    }
    /// A pushed typing indicator: its task is only the 22 s expiry timer, not awaited.
    fn typing_push(msg: &Msg) -> bool {
        matches!(msg, Msg::Notifications(NotificationsMsg::Realtime(_, RealtimeEvent::Request(r)))
            if r.body.contains(teams_chat::receipts::TYPING))
    }
    fn banners(effects: &[Effect]) -> Vec<(String, String)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Notify {
                    conversation_id,
                    body,
                    ..
                } => Some((conversation_id.clone(), body.clone())),
                _ => None,
            })
            .collect()
    }
    fn pushed(conv: &str, id: &str, text: &str, mentions: Value) -> teams_chat::Event {
        serde_json::from_value(json!({
            "resourceType": "NewMessage",
            "resourceLink": format!("https://mock/v1/users/ME/conversations/{conv}/messages/{id}"),
            "resource": {
                "id": id,
                "messagetype": "RichText/Html",
                "content": format!("<p>{text}</p>"),
                "from": "https://mock/v1/users/ME/contacts/8:orgid:alice",
                "imdisplayname": "Alice Martin",
                "originalarrivaltime": Utc::now().to_rfc3339(),
                "conversationLink": format!("https://mock/v1/users/ME/conversations/{conv}"),
                "properties": {"mentions": mentions.to_string()},
            }
        }))
        .unwrap()
    }
    async fn demo() -> App {
        let (mut app, task) = App::boot(true);
        drive(&mut app, task).await;
        assert!(app.chats.len() > 1, "the demo has chats");
        app
    }
    fn action(action: NotificationsAction) -> Msg {
        Msg::Notifications(NotificationsMsg::Action(action))
    }

    #[tokio::test]
    async fn pushed_messages_become_banners_unless_muted_visible_or_disabled() {
        let mut app = demo().await;
        let other = app
            .chats
            .iter()
            .map(|c| c.id.clone())
            .find(|id| app.selected.as_ref().is_none_or(|s| s.id() != id))
            .unwrap();
        let task = app.chat_event(pushed(&other, "9001", "Salut", json!([])));
        let effects = drive(&mut app, task).await;
        assert_eq!(banners(&effects), vec![(other.clone(), "Salut".into())]);

        // The same message again (e.g. through the activity feed) is not shown twice.
        let task = app.chat_event(pushed(&other, "9001", "Salut", json!([])));
        assert!(banners(&drive(&mut app, task).await).is_empty());

        let task = app.update(action(NotificationsAction::SetChatMuted {
            conversation_id: other.clone(),
            muted: true,
        }));
        drive(&mut app, task).await;
        assert!(app.chat_muted(&other));
        assert!(app.sidebar().chats.iter().any(|c| c.id == other && c.muted));
        let task = app.chat_event(pushed(&other, "9002", "Muet", json!([])));
        assert!(banners(&drive(&mut app, task).await).is_empty());
        let mention = json!([{"mri": mock::ME, "mentionType": "person", "itemid": "0"}]);
        let task = app.chat_event(pushed(&other, "9003", "Toi", mention));
        assert_eq!(
            banners(&drive(&mut app, task).await).len(),
            1,
            "mentions pass"
        );

        let task = app.update(action(NotificationsAction::SetChatMuted {
            conversation_id: other.clone(),
            muted: false,
        }));
        drive(&mut app, task).await;
        let task = app.update(Msg::Select(Selection::Chat(other.clone())));
        drive(&mut app, task).await;
        let task = app.chat_event(pushed(&other, "9004", "Vu", json!([])));
        assert!(
            banners(&drive(&mut app, task).await).is_empty(),
            "the conversation is on screen"
        );
        let _ = app.update(Msg::Notifications(NotificationsMsg::Active(false)));
        let task = app.chat_event(pushed(&other, "9005", "Absent", json!([])));
        assert_eq!(banners(&drive(&mut app, task).await).len(), 1);

        let task = app.update(action(NotificationsAction::SetToggle {
            toggle: NotificationToggle::ShowPreview,
            on: false,
        }));
        drive(&mut app, task).await;
        let task = app.chat_event(pushed(&other, "9006", "Secret", json!([])));
        assert_eq!(
            banners(&drive(&mut app, task).await),
            vec![(other.clone(), "Nouveau message".into())]
        );
    }

    #[tokio::test]
    async fn settings_are_written_to_the_user_property_and_read_back() {
        let mut app = demo().await;
        let task = app.update(action(NotificationsAction::SetChoice {
            row: NotificationRow::Chat,
            choice: NotificationChoice::Off,
        }));
        drive(&mut app, task).await;
        let task = app.update(action(NotificationsAction::Pause { minutes: 30 }));
        drive(&mut app, task).await;
        assert!(!app.notifications.saving);
        assert!(
            app.notifications.error.is_none(),
            "{:?}",
            app.notifications.error
        );

        // A fresh read from the service shows what was written.
        let stored = app.client.clone().unwrap().user_properties().await.unwrap();
        app.notifications.prefs = Prefs::default();
        app.preferences_loaded(&stored);
        let view = app.notifications_view();
        assert_eq!(view.preset, NotificationPreset::Custom);
        let chat = view
            .rows
            .iter()
            .find(|r| r.row == NotificationRow::Chat)
            .unwrap();
        assert_eq!(chat.choice, NotificationChoice::Off);
        assert!(view.paused_until.is_some());
        assert!(
            stored[rules::PROPERTY].is_string(),
            "stored as a JSON string"
        );

        let task = app.update(action(NotificationsAction::Resume));
        drive(&mut app, task).await;
        assert!(app.notifications_view().paused_until.is_none());
    }

    #[tokio::test]
    async fn chat_and_channel_editors_write_their_properties() {
        let mut app = demo().await;
        let chat = app.chats[0].id.clone();
        let task = app.update(action(NotificationsAction::EditChat {
            conversation_id: chat.clone(),
        }));
        drive(&mut app, task).await;
        let task = app.update(action(NotificationsAction::SetChatSettings {
            muted: false,
            everyone_mentions: false,
            desktop: ChatDesktop::Off,
        }));
        drive(&mut app, task).await;
        let editor = app.notifications_view().editor.unwrap();
        assert!(editor.error.is_none(), "{:?}", editor.error);
        assert_eq!(editor.desktop, ChatDesktop::Off);
        assert!(!editor.everyone_mentions);
        let client = app.client.clone().unwrap();
        let stored = client.conversation(&chat).await.unwrap().properties;
        assert_eq!(stored["alerts"], "true");
        let granular: Value =
            serde_json::from_str(stored["conversationNotificationSettings"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            granular,
            json!({"allNewPosts":"On","everyone":"Off","dskNotif":"Off"})
        );

        let (team, channel) = app
            .teams
            .iter()
            .find_map(|t| t.channels.first().map(|c| (t.id.clone(), c.id.clone())))
            .unwrap();
        let task = app.update(action(NotificationsAction::EditChannel {
            team: team.clone(),
            channel: channel.clone(),
        }));
        drive(&mut app, task).await;
        let task = app.update(action(NotificationsAction::SetChannelSettings {
            all_new_posts: ChannelChoice::BannerAndFeed,
            include_replies: true,
            channel_mentions: ChannelChoice::Feed,
            follow_all_threads: true,
        }));
        drive(&mut app, task).await;
        let editor = app.notifications_view().editor.unwrap();
        assert!(editor.error.is_none(), "{:?}", editor.error);
        assert_eq!(editor.all_new_posts, ChannelChoice::BannerAndFeed);
        assert!(editor.follow_all_threads);
        let stored = client
            .channel_notification_settings(&team, &channel, None)
            .await
            .unwrap();
        assert_eq!(stored.channel_mentions, Some(NssChoice::Feed));
        let props = client.conversation(&channel).await.unwrap().properties;
        assert!(rules::follows_all(props["followAllRc"].as_str().unwrap()));

        let task = app.update(action(NotificationsAction::MuteChannel {
            team,
            channel: channel.clone(),
        }));
        drive(&mut app, task).await;
        assert!(app.channel_muted(&channel));
    }

    /// Demo end to end: the peer's reply is pushed through the fake Trouter to `/messaging` and becomes a banner.
    #[tokio::test(flavor = "multi_thread")]
    async fn demo_replies_arrive_through_the_push_channel() {
        use std::time::Duration;
        let mut app = demo().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.notifications.push.stop = None;
        for job in app.sync_messaging().0 {
            if let Job::Subscribe(start) = job {
                let tx = tx.clone();
                tokio::spawn(start(crate::task::Sink::new(move |msg| {
                    let _ = tx.send(msg);
                })));
            }
        }
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(15), rx.recv())
                .await
                .expect("push channel event")
                .expect("channel open");
            let registered = matches!(
                &msg,
                Msg::Notifications(NotificationsMsg::Realtime(
                    _,
                    RealtimeEvent::Registered { .. }
                ))
            );
            let task = app.update(msg);
            drive(&mut app, task).await;
            if registered {
                break;
            }
        }
        assert_eq!(
            app.notifications.status,
            "Notifications en temps réel actives"
        );
        let _ = app.update(Msg::Notifications(NotificationsMsg::Active(false)));
        let conv = app.chats[0].id.clone();
        let message = teams_chat::OutgoingMessage::html("1", "<p>Bonjour</p>", "Vous");
        app.client
            .clone()
            .unwrap()
            .send_message(&conv, None, &message)
            .await
            .unwrap();
        // The demo Trouter is shared process-wide: another test's reply in this chat may arrive here.
        let banner = loop {
            let msg = tokio::time::timeout(Duration::from_secs(15), rx.recv())
                .await
                .expect("push channel event")
                .expect("channel open");
            let typing = typing_push(&msg);
            let task = app.update(msg);
            if typing {
                continue;
            }
            let effects = drive(&mut app, task).await;
            if let Some(found) = banners(&effects).into_iter().find(|(c, _)| *c == conv) {
                break found;
            }
        };
        assert_eq!(banner.0, conv);
        assert!(banner.1.starts_with("Bien reçu : "), "{banner:?}");
    }

    /// Demo: the peer's typing shows, then their reply clears it and their read position marks my message seen.
    #[tokio::test(flavor = "multi_thread")]
    async fn demo_peer_types_then_reads_my_message() {
        use std::time::Duration;
        let mut app = demo().await;
        let conv = app
            .chats
            .iter()
            .find(|c| !c.is_group() && !c.id.starts_with("48:"))
            .map(|c| c.id.clone())
            .unwrap();
        let task = app.update(Msg::Select(Selection::Chat(conv.clone())));
        drive(&mut app, task).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.notifications.push.stop = None;
        for job in app.sync_messaging().0 {
            if let Job::Subscribe(start) = job {
                let tx = tx.clone();
                tokio::spawn(start(crate::task::Sink::new(move |msg| {
                    let _ = tx.send(msg);
                })));
            }
        }
        let mut next = async || {
            tokio::time::timeout(Duration::from_secs(15), rx.recv())
                .await
                .expect("push channel event")
                .expect("channel open")
        };
        loop {
            let msg = next().await;
            let registered = matches!(
                &msg,
                Msg::Notifications(NotificationsMsg::Realtime(
                    _,
                    RealtimeEvent::Registered { .. }
                ))
            );
            let task = app.update(msg);
            drive(&mut app, task).await;
            if registered {
                break;
            }
        }
        let message = teams_chat::OutgoingMessage::html("1", "<p>Tu es là ?</p>", "Vous");
        app.client
            .clone()
            .unwrap()
            .send_message(&conv, None, &message)
            .await
            .unwrap();
        let task = app.load_feed(false);
        drive(&mut app, task).await;
        let mut saw_typing = false;
        loop {
            let msg = next().await;
            let typing = typing_push(&msg);
            let task = app.update(msg);
            saw_typing |= app.typing_text().is_some();
            if !typing {
                drive(&mut app, task).await;
            }
            let (messages, _, _) = app.feed_views();
            let mine = messages.iter().rev().find(|m| m.mine);
            if saw_typing
                && app.typing_text().is_none()
                && mine.is_some_and(|m| m.receipt == Some(crate::app::receipts::ReceiptView::Seen))
            {
                break;
            }
        }
    }

    #[tokio::test]
    async fn joining_unmutes_a_meeting_chat_unless_the_user_chose() {
        let mut app = demo().await;
        let (free, chosen) = (app.chats[0].id.clone(), app.chats[1].id.clone());
        let task = app.update(action(NotificationsAction::SetChatMuted {
            conversation_id: chosen.clone(),
            muted: true,
        }));
        drive(&mut app, task).await;
        // Not read yet: its properties are fetched before deciding.
        app.notifications.conversations.remove(&free);
        app.notifications.requested.remove(&free);
        for thread in [&free, &chosen] {
            let task = app.joined_meeting_chat(thread);
            drive(&mut app, task).await;
        }
        let client = app.client.clone().unwrap();
        let free_props = client.conversation(&free).await.unwrap().properties;
        assert_eq!(free_props["alerts"], "true");
        let chosen_props = client.conversation(&chosen).await.unwrap().properties;
        assert_eq!(chosen_props["alerts"], "false", "an explicit mute is kept");

        let other = app.chats[2].id.clone();
        let task = app.update(action(NotificationsAction::SetMeetingChats {
            choice: MeetingChats::All,
        }));
        drive(&mut app, task).await;
        let task = app.joined_meeting_chat(&other);
        drive(&mut app, task).await;
        let props = client.conversation(&other).await.unwrap().properties;
        assert!(
            props.get("alerts").is_none(),
            "only for \"mute until I join\""
        );
    }

    #[test]
    fn activity_items_name_the_source_conversation_and_skip_chat_copies() {
        let mut app = App::empty();
        *app.me = "8:orgid:me".into();
        let item = |kind: &str| -> Message {
            serde_json::from_value(json!({
                "id": "77",
                "originalarrivaltime": Utc::now().to_rfc3339(),
                "properties": {"activity": {
                    "activityType": kind, "activitySubtype": "person",
                    "sourceThreadId": "19:chan@thread.tacv2", "sourceMessageId": 5,
                    "sourceReplyChainId": 4, "sourceUserId": "8:orgid:bob",
                    "sourceUserImDisplayName": "Bob", "messagePreview": "regarde",
                    "activityTimestamp": Utc::now().to_rfc3339(),
                }}
            }))
            .unwrap()
        };
        let mention = app.activity_candidate(&item("mention")).unwrap();
        assert_eq!(mention.kind, Kind::PersonalMention);
        assert_eq!(mention.conversation_id, "19:chan@thread.tacv2");
        assert_eq!(mention.message_id, "5");
        assert_eq!(mention.reply_chain_id.as_deref(), Some("4"));
        assert!(mention.body.ends_with("regarde"));
        assert!(app.activity_candidate(&item("chat")).is_none());
        let reaction = app.activity_candidate(&item("reactionInChat")).unwrap();
        assert_eq!(reaction.message_id, "activity:77");
    }

    #[test]
    fn channel_replies_follow_threads_and_report_their_chain() {
        let mut app = App::empty();
        *app.me = "8:orgid:me".into();
        let message: Message = serde_json::from_value(json!({
            "id": "12", "messagetype": "RichText/Html", "content": "<p>re</p>",
            "from": "https://x/contacts/8:orgid:bob",
            "conversationLink": "https://x/conversations/19:c@thread.tacv2;messageid=10",
        }))
        .unwrap();
        let reply = app
            .message_candidate("19:c@thread.tacv2", &message)
            .unwrap();
        assert_eq!(reply.kind, Kind::ChannelReply);
        assert_eq!(reply.reply_chain_id.as_deref(), Some("10"));
        app.notifications.follows.insert(
            follow_key("19:c@thread.tacv2", "10"),
            FollowState::ExplicitFollow,
        );
        let reply = app
            .message_candidate("19:c@thread.tacv2", &message)
            .unwrap();
        assert_eq!(reply.kind, Kind::FollowedThreadReply);
    }

    // ---- Refresh modes ------------------------------------------------------

    /// Network requests a message starts.
    fn requests(task: &Task<Msg>) -> usize {
        task.0
            .iter()
            .filter(|j| matches!(j, Job::Future(_)))
            .count()
    }
    fn realtime(app: &mut App, event: RealtimeEvent) -> Task<Msg> {
        let g = app.generation;
        app.update(Msg::Notifications(NotificationsMsg::Realtime(g, event)))
    }
    fn down() -> RealtimeEvent {
        RealtimeEvent::Disconnected {
            reason: "Connexion perdue.".into(),
            retry_in: std::time::Duration::from_secs(1),
        }
    }
    /// A demo with a chat open whose push channel registered with the fake Trouter.
    async fn live_demo() -> App {
        use std::time::Duration;
        let mut app = demo().await;
        let conv = app.chats[0].id.clone();
        let task = app.update(Msg::Select(Selection::Chat(conv)));
        drive(&mut app, task).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.notifications.push.stop = None;
        for job in app.sync_messaging().0 {
            if let Job::Subscribe(start) = job {
                let tx = tx.clone();
                tokio::spawn(start(crate::task::Sink::new(move |msg| {
                    let _ = tx.send(msg);
                })));
            }
        }
        assert!(
            !app.notifications.push_live(),
            "registering is not live yet"
        );
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(15), rx.recv())
                .await
                .expect("push channel event")
                .expect("channel open");
            let registered = matches!(
                &msg,
                Msg::Notifications(NotificationsMsg::Realtime(
                    _,
                    RealtimeEvent::Registered { .. }
                ))
            );
            let task = app.update(msg);
            if registered {
                assert!(requests(&task) > 0, "the first registration catches up");
                drive(&mut app, task).await;
                break;
            }
            drive(&mut app, task).await;
        }
        assert!(app.notifications.push_live());
        assert!(app.notifications_view().realtime_live);
        // Presence has no push channel here and keeps its own slow poll.
        app.presence_asked = Some(std::time::Instant::now());
        app.presence_asked_for = app.presence_targets().into_iter().collect();
        app
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn push_mode_with_a_live_channel_never_polls() {
        let mut app = live_demo().await;
        app.clock_minute = 0;
        for tick in 1..=12 {
            let task = app.update(Msg::Tick);
            assert_eq!(requests(&task), 0, "tick {tick} reached the network");
            // What shows the time is still redrawn, once per minute.
            let changes = app.take_changes();
            if tick == 1 {
                assert!(changes.has(Parts::CONVERSATION));
            }
            assert!(changes.has(Parts::TYPING));
        }
        // Back to the front: the open conversation only.
        let task = app.update(Msg::Foreground);
        assert_eq!(requests(&task), 1, "one feed reload");
        drive(&mut app, task).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn push_mode_reloads_the_calendar_and_calls_only_when_opened_or_in_front() {
        let mut app = live_demo().await;
        // The range the calendar screen shows.
        app.calendar.view.start = "2026-10-01T00:00:00Z".into();
        app.calendar.view.end = "2026-11-01T00:00:00Z".into();
        let task = realtime(&mut app, RealtimeEvent::MessageLoss { tags: vec![] });
        let chats_only = requests(&task);
        drive(&mut app, task).await;
        for nav in [Nav::Calendar, Nav::Calls] {
            let task = app.update(Msg::Navigate(nav));
            assert!(requests(&task) > 0, "{nav:?} loads when opened");
            drive(&mut app, task).await;
            // Even long after the last load, ticks stay quiet.
            app.calendar.refreshed = None;
            app.calls.refreshed = None;
            for tick in 1..=12 {
                let task = app.update(Msg::Tick);
                assert_eq!(
                    requests(&task),
                    0,
                    "{nav:?}: tick {tick} reached the network"
                );
            }
            let task = app.update(Msg::Foreground);
            assert!(requests(&task) > 0, "{nav:?} reloads back in front");
            drive(&mut app, task).await;
            // A gap in the pushes reloads the open screen.
            let task = realtime(&mut app, RealtimeEvent::MessageLoss { tags: vec![] });
            assert!(requests(&task) > chats_only, "{nav:?} caught up too");
            drive(&mut app, task).await;
        }
    }

    #[tokio::test]
    async fn a_pushed_call_log_reloads_the_calls_screen_only() {
        let mut app = demo().await;
        let mut log = pushed("48:calllogs", "1", "", json!([]));
        log.resource["threadtype"] = json!("streamofcalllogs");
        let chat = pushed(&app.chats[0].id.clone(), "2", "Salut", json!([]));
        let task = app.chat_event(log.clone());
        assert_eq!(requests(&task), 0, "not on the calls screen");
        let task = app.update(Msg::Navigate(Nav::Calls));
        drive(&mut app, task).await;
        let task = app.chat_event(log);
        assert!(requests(&task) > 0, "history and voicemail reloaded");
        assert!(app.calls.view.loading_history);
        drive(&mut app, task).await;
        // Other messages leave the screen alone.
        let _ = app.chat_event(chat);
        assert!(!app.calls.view.loading_history);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn push_mode_polls_while_the_channel_is_down_and_catches_up_once_back() {
        let mut app = live_demo().await;
        let task = realtime(&mut app, down());
        assert_eq!(requests(&task), 0);
        assert!(!app.notifications.push_live());
        // Down: the feed every tick, the chat list and the activity every 6th.
        let mut counts = vec![];
        for _ in 0..6 {
            let task = app.update(Msg::Tick);
            counts.push(requests(&task));
            drive(&mut app, task).await;
        }
        assert!(counts[..5].iter().all(|&n| n == 1), "{counts:?}");
        assert!(counts[5] > 1, "workspace and activity: {counts:?}");
        // Reconnecting: still down until registered.
        let _ = realtime(&mut app, RealtimeEvent::Connected { surl: "s".into() });
        let task = app.update(Msg::Tick);
        assert_eq!(requests(&task), 1);
        drive(&mut app, task).await;
        // Registered again: one catch-up, then quiet ticks.
        let task = realtime(&mut app, RealtimeEvent::Registered { ttl: 3600 });
        assert!(requests(&task) > 1, "feed, chat list and activity");
        drive(&mut app, task).await;
        let task = realtime(&mut app, RealtimeEvent::Registered { ttl: 3600 });
        assert_eq!(requests(&task), 0, "a renewal is not a reconnection");
        let task = app.update(Msg::Tick);
        assert_eq!(requests(&task), 0);
        // Lost pushes reload the open conversation too.
        let task = realtime(&mut app, RealtimeEvent::MessageLoss { tags: vec![] });
        assert!(requests(&task) > 1);
        drive(&mut app, task).await;
        // A stopped channel falls back to polling for good.
        let _ = realtime(&mut app, RealtimeEvent::Fatal("jeton refusé".into()));
        let task = app.update(Msg::Tick);
        assert_eq!(requests(&task), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn polling_mode_polls_as_before_even_with_a_live_channel() {
        let mut app = live_demo().await;
        let _ = app.update(Msg::SetRefreshMode(crate::app::RefreshMode::Polling));
        let mut counts = vec![];
        for _ in 0..12 {
            let task = app.update(Msg::Tick);
            counts.push(requests(&task));
            drive(&mut app, task).await;
        }
        let reloads = |range: std::ops::Range<usize>| counts[range].iter().all(|&n| n == 1);
        assert!(reloads(0..5) && reloads(6..11), "{counts:?}");
        assert!(counts[5] > 1 && counts[11] > 1, "{counts:?}");
        let task = app.update(Msg::Foreground);
        assert_eq!(requests(&task), 1);
        drive(&mut app, task).await;
        // Back to push mode: quiet again at once.
        let _ = app.update(Msg::SetRefreshMode(crate::app::RefreshMode::Push));
        assert_eq!(requests(&app.update(Msg::Tick)), 0);
    }

    #[tokio::test]
    async fn the_refresh_mode_survives_signing_out() {
        let mut app = demo().await;
        let _ = app.update(Msg::SetRefreshMode(crate::app::RefreshMode::Polling));
        app.reset();
        assert_eq!(app.refresh_mode, crate::app::RefreshMode::Polling);
        assert!(!app.notifications.push_live());
    }
}
