//! Flat view-models for the macOS app, so Swift only lays them out; each part is sent only when it changed.
pub use super::activity::ActivityRow;
use super::calendar::CalendarView;
use super::call::CallPart;
use super::calls::CallsView;
use super::files::{BrowserView, PendingFileView, SharedFilter};
use super::*;
use crate::attachments::Attachment;
use serde_json::Value;
use std::hash::{Hash, Hasher};
use teams_i18n::{t, tf};

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct Target {
    /// Set for channels: the team the channel belongs to.
    pub team: Option<String>,
    pub id: String,
}
impl From<&Selection> for Target {
    fn from(s: &Selection) -> Self {
        match s {
            Selection::Chat(id) => Self {
                team: None,
                id: id.clone(),
            },
            Selection::Channel { team, channel } => Self {
                team: Some(team.clone()),
                id: channel.clone(),
            },
        }
    }
}
impl From<Target> for Selection {
    fn from(t: Target) -> Self {
        match t.team {
            Some(team) => Selection::Channel {
                team,
                channel: t.id,
            },
            None => Selection::Chat(t.id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct Shell {
    pub signed_in: bool,
    pub signing_in: bool,
    pub me: String,
    pub display_name: String,
    pub account_email: String,
    /// External-browser sign-in is waiting for the pasted return URL.
    pub browser_login: bool,
    pub callback: String,
    pub nav: Nav,
    pub filter: Filter,
    pub tab: Tab,
    pub search: String,
    pub error: Option<String>,
    pub notices: Vec<String>,
    pub unread: u32,
    pub activity_unread: u32,
    pub teams_unread: bool,
    pub loading_workspace: bool,
    pub selected: Option<Target>,
    pub reply_open: bool,
    pub read_only: bool,
    pub editing: Option<String>,
}

/// What the message editors show; apart from [`Shell`] because it changes on every keystroke.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ComposerView {
    pub sending: bool,
    pub draft: String,
    pub draft_rev: u64,
    pub draft_cursor: u32,
    pub mention_ranges: Vec<super::composer::MentionRange>,
    pub draft_formats: Vec<super::formatting::ComposerFormat>,
    pub mention_loading: bool,
    pub mention_error: Option<String>,
    pub subject: String,
    pub importance: super::conversation::Importance,
    /// Files attached to the draft, uploading or ready.
    pub attachments: Vec<PendingFileView>,
    /// Cards from message extensions, sent with the draft.
    pub cards: Vec<super::extensions::ComposerCardView>,
    /// The preview of the draft's first link, sent with it.
    pub link_preview: Option<super::link_preview::LinkPreviewView>,
}

/// Availability shown on a person's avatar, as the web client's presence badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PresenceStatus {
    Available,
    Busy,
    DoNotDisturb,
    BeRightBack,
    Away,
    Offline,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct PresenceView {
    pub status: PresenceStatus,
    /// Automatic replies are on (`calendarData.isOutOfOffice`).
    pub out_of_office: bool,
}
impl From<&teams_chat::Presence> for PresenceView {
    fn from(p: &teams_chat::Presence) -> Self {
        let status = match p.availability.as_str() {
            "Available" => PresenceStatus::Available,
            "Busy" | "BusyIdle" => PresenceStatus::Busy,
            "DoNotDisturb" => PresenceStatus::DoNotDisturb,
            "BeRightBack" => PresenceStatus::BeRightBack,
            "Away" | "AvailableIdle" => PresenceStatus::Away,
            "Offline" => PresenceStatus::Offline,
            _ => PresenceStatus::Unknown,
        };
        Self {
            status,
            out_of_office: p.out_of_office,
        }
    }
}

/// How an avatar is framed, as in Teams: people and chats in a circle, apps and teams in a rounded square.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AvatarShape {
    #[default]
    Round,
    Square,
}
impl AvatarShape {
    /// Apps and bots have `28:` MRIs.
    pub(crate) fn of(mri: &str) -> Self {
        if mri.starts_with("28:") {
            Self::Square
        } else {
            Self::Round
        }
    }
    /// A direct chat with an app takes the app's shape; any other chat is round.
    fn of_chat(c: &Conversation, me: &str) -> Self {
        if c.is_group() {
            return Self::Round;
        }
        c.members
            .iter()
            .find(|m| !m.id.eq_ignore_ascii_case(me))
            .map_or(Self::Round, |m| Self::of(&m.id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ChatRow {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub time: String,
    pub unread: bool,
    pub pinned: bool,
    /// Hidden by hand; listed only while searching.
    pub hidden: bool,
    pub can_leave: bool,
    pub avatar_shape: AvatarShape,
    pub selected: bool,
    /// Notifications turned off (`alerts`), or a meeting chat muted by default.
    pub muted: bool,
    /// Direct chats: the other person's presence, once known.
    pub presence: Option<PresenceView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ChannelRow {
    pub team: String,
    pub id: String,
    pub name: String,
    pub selected: bool,
    pub hidden: bool,
    pub kind: String,
    pub archived: bool,
    pub pending: bool,
    /// All notifications of the channel are off (NSS).
    pub muted: bool,
    pub unread: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct TeamRow {
    pub hidden: bool,
    pub pending: bool,
    pub hidden_channel_count: u32,
    pub show_hidden_channels: bool,
    pub id: String,
    pub name: String,
    pub description: String,
    pub archived: bool,
    pub expanded: bool,
    /// Channels are listed under the team in the sidebar (open, or a search is active).
    pub show_channels: bool,
    pub channels: Vec<ChannelRow>,
    /// A shown channel has unread messages, listed or not.
    pub unread: bool,
    pub loading: bool,
    pub failed: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct Sidebar {
    pub activities: Vec<ActivityRow>,
    pub activity_loading: bool,
    pub more_activity: bool,
    pub activity_error: Option<String>,
    pub chats: Vec<ChatRow>,
    pub teams: Vec<TeamRow>,
    pub team_controls: super::team_list::TeamListControls,
    pub team_count: u32,
    pub more_chats: bool,
    pub teams_problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct RunView {
    pub text: String,
    pub link: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<String>,
    pub highlight: Option<String>,
    pub size: u32,
    pub mention: bool,
}
/// A message body part, drawn by its own view: paragraphs, quoted replies, code…
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum BlockView {
    Text {
        runs: Vec<RunView>,
    },
    List {
        ordered: bool,
        start: u32,
        items: Vec<ListItemView>,
    },
    /// A reply to another message (`message_id`), or a plain quote when it has no author.
    Quote {
        author: Option<String>,
        time: Option<String>,
        message_id: Option<String>,
        body: Vec<BlockView>,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Table {
        header: bool,
        rows: Vec<TableRowView>,
    },
    Rule,
    Image {
        image: ImageView,
    },
    Card {
        card: CardView,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ListItemView {
    pub blocks: Vec<BlockView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct TableRowView {
    pub cells: Vec<TableCellView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct TableCellView {
    pub runs: Vec<RunView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CardView {
    /// Which card of its message, for `Action::CardAction`.
    pub index: u32,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub items: Vec<CardItemView>,
    pub actions: Vec<CardActionView>,
    /// The bot's answer to the last button, or why it failed.
    pub footer: Option<String>,
    /// A button's request is out: the card waits for the bot.
    pub busy: bool,
    /// `0xRRGGBB` for the bar along the card's top edge (a connector card's `themeColor`).
    pub accent: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CardItemView {
    Text { runs: Vec<RunView>, muted: bool },
    Blocks { blocks: Vec<BlockView> },
    Gap { points: u32, separator: bool },
    Facts { facts: Vec<FactView> },
    Image { image: ImageView },
    Input { input: CardInputView },
    Columns { columns: Vec<CardColumnView> },
    Emphasis { items: Vec<CardItemView> },
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CardColumnView {
    pub width: CardColumnWidth,
    pub items: Vec<CardItemView>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CardColumnWidth {
    Auto,
    Stretch,
    Weight { weight: u32 },
    Points { points: u32 },
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CardInputKind {
    Text { placeholder: String, multiline: bool },
    Number { placeholder: String },
    Date,
    Time,
    Toggle { title: String, on: String, off: String },
    Choice { choices: Vec<CardChoiceView>, multi: bool },
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CardChoiceView {
    pub title: String,
    pub value: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CardInputView {
    pub id: String,
    pub label: Option<String>,
    pub kind: CardInputKind,
    pub value: String,
    pub required: bool,
}
/// A value typed into a card input, by input id; multi-choice values joined by commas.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CardInputValue {
    pub id: String,
    pub value: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CardActionKind {
    OpenUrl,
    /// Sent to the bot with the card's inputs.
    Submit,
    Unsupported,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct FactView {
    pub title: String,
    pub value: String,
}
/// A card button: a link, a request to the bot, or one only Teams can run (disabled).
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CardActionView {
    pub title: String,
    pub url: Option<String>,
    pub kind: CardActionKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ImageStatus {
    Loading,
    Ready {
        width: u32,
        height: u32,
    },
    Failed {
        reason: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ImageView {
    pub src: String,
    pub alt: String,
    pub status: ImageStatus,
    /// Size declared by the message, before the image loads.
    pub width: Option<u32>,
    pub height: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct FileView {
    /// Identity for `FileAction`s.
    pub key: String,
    pub name: String,
    pub url: Option<String>,
    pub deleted: bool,
    pub ext: String,
    pub description: String,
    /// Download progress (0–100) while one runs.
    pub progress: Option<u8>,
    pub can_download: bool,
    pub thumbnail: Option<ImageView>,
    /// Short date, in the shared lists.
    pub date: String,
    pub is_link: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ReactionView {
    pub key: String,
    pub emoji: String,
    /// Catalog emoticon whose Teams artwork draws it.
    pub emoji_id: Option<String>,
    pub count: u64,
    pub mine: bool,
    pub hint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct MessageView {
    pub id: String,
    pub sender: String,
    pub sender_id: String,
    pub sender_avatar_shape: AvatarShape,
    /// The author's presence, once known.
    pub sender_presence: Option<PresenceView>,
    pub mine: bool,
    pub time: String,
    pub day: String,
    pub subject: Option<String>,
    /// Marked Important or Urgent by its sender.
    pub importance: super::conversation::Importance,
    pub deleted: bool,
    pub editing: bool,
    /// The body as flat text runs: for copy and height estimates.
    pub runs: Vec<RunView>,
    pub blocks: Vec<BlockView>,
    pub images: Vec<ImageView>,
    pub files: Vec<FileView>,
    pub link_preview: Option<super::link_preview::LinkPreviewView>,
    pub reactions: Vec<ReactionView>,
    /// A reaction request for this message is in flight.
    pub pending: bool,
    pub can_react: bool,
    pub editable: bool,
    /// Read receipt of one of my chat messages (only the latest seen and latest sent).
    pub receipt: Option<super::receipts::ReceiptView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct PostView {
    pub id: String,
    pub root: MessageView,
    pub replies: u32,
    /// Followed explicitly, by taking part, or through "follow all threads".
    pub following: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ThreadView {
    pub root_preview: String,
    /// Who started the thread and when, shown above the replies.
    pub root_sender: String,
    pub root_sender_id: String,
    pub root_sender_avatar_shape: AvatarShape,
    pub root_time: String,
    pub loading: bool,
    pub has_more: bool,
    pub empty_text: Option<String>,
    /// Id of the post, for its follow toggle.
    pub post: String,
    /// Channel posts only: whether replies to this post are notified.
    pub following: Option<bool>,
}
/// The open conversation without its messages, which come in [`FeedPart`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ConversationView {
    pub target: Target,
    pub title: String,
    pub subtitle: String,
    pub is_channel: bool,
    pub member_count: Option<u32>,
    pub loading: bool,
    pub has_more: bool,
    pub empty_text: Option<String>,
    pub posts_error: Option<String>,
    pub thread: Option<ThreadView>,
    /// A meeting chat whose meeting can be joined natively.
    pub is_meeting: bool,
    /// A chat whose members can be called (1:1 or group).
    pub can_call: bool,
    /// Notifications of this chat or channel are off.
    pub muted: bool,
    /// The open chat as the chat list shows it, for the same actions; `None` for channels.
    pub chat: Option<ChatRow>,
    /// First message from someone else that was unread when the chat was opened.
    pub first_unread: Option<String>,
    /// Direct chats: the other person's presence, once known.
    pub presence: Option<PresenceView>,
    pub avatar_shape: AvatarShape,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ConversationPart {
    pub view: Option<ConversationView>,
}
/// "Alice est en train d’écrire…" (chats only); apart from the conversation so typing does not resend it whole.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct TypingPart {
    pub text: Option<String>,
}
/// One list of the open conversation as a difference: ids in order when they change, plus only new or changed views.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct MessageList {
    pub ids: Option<Vec<String>>,
    pub changed: Vec<MessageView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct PostList {
    pub ids: Option<Vec<String>>,
    pub changed: Vec<PostView>,
}
/// Chat messages, channel posts and the open thread's replies.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct FeedPart {
    pub messages: MessageList,
    pub posts: PostList,
    pub replies: MessageList,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct MemberView {
    pub id: String,
    pub name: String,
    pub role: String,
    /// Known for the members of a group chat and the people who wrote in the open conversation.
    pub presence: Option<PresenceView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct MembersView {
    pub is_channel: bool,
    /// The open chat is a group chat: people can be added and removed, and it can be renamed.
    pub can_manage: bool,
    /// Its name, empty when it has none.
    pub chat_name: String,
    pub members: Option<Vec<MemberView>>,
    pub loading: bool,
    pub error: Option<String>,
}
/// The "Partagé" tab of a chat.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct FilesView {
    pub files: Vec<FileView>,
    pub filter: SharedFilter,
    /// The tab's own search, empty when every item is shown.
    pub query: String,
    pub loading: bool,
    pub error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct PersonView {
    pub mri: String,
    pub name: String,
    pub email: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct NewChatView {
    pub query: String,
    pub title: String,
    pub chosen: Vec<PersonView>,
    pub results: Vec<PersonView>,
    pub searching: bool,
    pub searched: bool,
    pub creating: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct NewChatPart {
    pub dialog: Option<NewChatView>,
}

/// Only the parts that changed since the previous update are set.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct Update {
    pub avatar_ids: Option<Vec<String>>,
    /// Sources of the previews Swift may keep: it drops the pixels of any other one.
    pub image_srcs: Option<Vec<String>>,
    pub calendar: Option<CalendarView>,
    pub calls: Option<CallsView>,
    pub call: Option<CallPart>,
    pub shell: Option<Shell>,
    pub composer: Option<ComposerView>,
    /// Who can be mentioned in the open conversation: apart from the composer, which changes on every keystroke.
    pub mention_people: Option<Vec<PersonView>>,
    pub sidebar: Option<Sidebar>,
    pub conversation: Option<ConversationPart>,
    /// Messages of the open conversation that changed, see [`MessageList`].
    pub feed: Option<FeedPart>,
    pub typing: Option<TypingPart>,
    pub members: Option<MembersView>,
    pub files: Option<FilesView>,
    /// The folder browser: global Files view, or a channel's files.
    pub browser: Option<BrowserView>,
    pub new_chat: Option<NewChatPart>,
    pub notifications: Option<super::notifications::NotificationsView>,
    pub me: Option<super::me::MeView>,
    pub chat_admin: Option<super::chat_admin::ChatAdminView>,
    pub teams_admin: Option<super::teams_admin::TeamsAdminView>,
    pub gifs: Option<super::gifs::GifsView>,
    pub extensions: Option<super::extensions::ExtensionsView>,
    pub search: Option<super::search::SearchView>,
}
impl Update {
    pub fn is_empty(&self) -> bool {
        self.browser.is_none()
            && self.calendar.is_none()
            && self.calls.is_none()
            && self.call.is_none()
            && self.avatar_ids.is_none()
            && self.image_srcs.is_none()
            && self.shell.is_none()
            && self.composer.is_none()
            && self.mention_people.is_none()
            && self.sidebar.is_none()
            && self.conversation.is_none()
            && self.feed.is_none()
            && self.typing.is_none()
            && self.members.is_none()
            && self.files.is_none()
            && self.new_chat.is_none()
            && self.notifications.is_none()
            && self.me.is_none()
            && self.chat_admin.is_none()
            && self.teams_admin.is_none()
            && self.gifs.is_none()
            && self.extensions.is_none()
            && self.search.is_none()
    }
}

/// What Swift already has, as one hash per part (one per message for the open conversation's lists).
#[derive(Default)]
pub(crate) struct Snapshot {
    avatar_ids: Option<u64>,
    image_srcs: Option<u64>,
    calendar: Option<u64>,
    calls: Option<u64>,
    call: Option<u64>,
    shell: Option<u64>,
    composer: Option<u64>,
    mention_people: Option<u64>,
    sidebar: Option<u64>,
    conversation: Option<u64>,
    messages: ListSnapshot,
    posts: ListSnapshot,
    replies: ListSnapshot,
    typing: Option<u64>,
    members: Option<u64>,
    files: Option<u64>,
    browser: Option<u64>,
    new_chat: Option<u64>,
    notifications: Option<u64>,
    me: Option<u64>,
    chat_admin: Option<u64>,
    teams_admin: Option<u64>,
    gifs: Option<u64>,
    extensions: Option<u64>,
    search: Option<u64>,
}
fn hash_of<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
fn changed<T: Hash>(previous: &mut Option<u64>, next: T) -> Option<T> {
    let hash = hash_of(&next);
    if *previous == Some(hash) {
        None
    } else {
        *previous = Some(hash);
        Some(next)
    }
}
/// The order of one list and a hash of each of its views, by id.
#[derive(Default)]
struct ListSnapshot {
    order: Option<u64>,
    views: HashMap<String, u64>,
}
impl ListSnapshot {
    /// The ids if their order changed, and the views Swift does not have yet.
    fn diff<T: Hash>(
        &mut self,
        views: Vec<T>,
        id: fn(&T) -> &str,
    ) -> (Option<Vec<String>>, Vec<T>) {
        let mut hashes = HashMap::with_capacity(views.len());
        let ids: Vec<String> = views.iter().map(|v| id(v).to_owned()).collect();
        let changed = views
            .into_iter()
            .filter(|view| {
                let hash = hash_of(view);
                let known = self.views.get(id(view)) == Some(&hash);
                hashes.insert(id(view).to_owned(), hash);
                !known
            })
            .collect();
        self.views = hashes;
        (self::changed(&mut self.order, ids), changed)
    }
}
pub(crate) use super::changes::Parts;
fn changed_in<T: Hash>(
    parts: Parts,
    part: Parts,
    previous: &mut Option<u64>,
    next: impl FnOnce() -> T,
) -> Option<T> {
    if parts.has(part) {
        changed(previous, next())
    } else {
        None
    }
}
/// [`changed_in`] for a view the app already holds: hashed in place, cloned only when Swift lacks it.
fn changed_ref<T: Hash + Clone>(
    parts: Parts,
    part: Parts,
    previous: &mut Option<u64>,
    view: &T,
) -> Option<T> {
    if !parts.has(part) {
        return None;
    }
    let hash = hash_of(view);
    if *previous == Some(hash) {
        None
    } else {
        *previous = Some(hash);
        Some(view.clone())
    }
}
impl Snapshot {
    /// Rebuilds the given parts and keeps those differing from Swift's; other parts must be unchanged since the last diff.
    pub(crate) fn diff(&mut self, app: &App, parts: Parts) -> Update {
        Update {
            avatar_ids: changed_in(parts, Parts::AVATARS, &mut self.avatar_ids, || {
                app.avatars.lock().unwrap().ready_ids()
            }),
            image_srcs: changed_in(parts, Parts::IMAGES, &mut self.image_srcs, || {
                app.kept_image_srcs()
            }),
            calendar: changed_ref(
                parts,
                Parts::CALENDAR,
                &mut self.calendar,
                &app.calendar.view,
            ),
            calls: changed_ref(parts, Parts::CALLS, &mut self.calls, &app.calls.view),
            call: changed_in(parts, Parts::CALL, &mut self.call, || app.call_part()),
            shell: changed_in(parts, Parts::SHELL, &mut self.shell, || app.shell()),
            composer: changed_in(parts, Parts::COMPOSER, &mut self.composer, || {
                app.composer_view()
            }),
            mention_people: changed_in(parts, Parts::COMPOSER, &mut self.mention_people, || {
                app.mention_people()
            }),
            sidebar: changed_in(parts, Parts::SIDEBAR, &mut self.sidebar, || app.sidebar()),
            conversation: changed_in(parts, Parts::CONVERSATION, &mut self.conversation, || {
                app.conversation_part()
            }),
            feed: if parts.has(Parts::MESSAGES) {
                self.feed(app)
            } else {
                None
            },
            typing: changed_in(parts, Parts::TYPING, &mut self.typing, || TypingPart {
                text: app.typing_text(),
            }),
            members: changed_in(parts, Parts::MEMBERS, &mut self.members, || {
                app.members_view()
            }),
            files: changed_in(parts, Parts::FILES, &mut self.files, || app.files_view()),
            browser: changed_in(parts, Parts::BROWSER, &mut self.browser, || {
                app.browser_view()
            }),
            new_chat: changed_in(parts, Parts::NEW_CHAT, &mut self.new_chat, || {
                app.new_chat_part()
            }),
            notifications: changed_in(parts, Parts::NOTIFICATIONS, &mut self.notifications, || {
                app.notifications_view()
            }),
            me: changed_ref(parts, Parts::ME, &mut self.me, &app.me_state.view),
            chat_admin: changed_ref(
                parts,
                Parts::MANAGE,
                &mut self.chat_admin,
                &app.chat_admin.view,
            ),
            teams_admin: changed_ref(
                parts,
                Parts::MANAGE,
                &mut self.teams_admin,
                &app.teams_admin.view,
            ),
            gifs: changed_ref(parts, Parts::GIFS, &mut self.gifs, &app.gifs.view),
            extensions: changed_ref(
                parts,
                Parts::EXTENSIONS,
                &mut self.extensions,
                &app.extensions.view,
            ),
            search: changed_ref(
                parts,
                Parts::RESULTS,
                &mut self.search,
                &app.search_state.view,
            ),
        }
    }
    fn feed(&mut self, app: &App) -> Option<FeedPart> {
        let (messages, posts, replies) = app.feed_views();
        let (ids, changed) = self.messages.diff(messages, |m| &m.id);
        let messages = MessageList { ids, changed };
        let (ids, changed) = self.posts.diff(posts, |p| &p.id);
        let posts = PostList { ids, changed };
        let (ids, changed) = self.replies.diff(replies, |m| &m.id);
        let replies = MessageList { ids, changed };
        let unchanged = |ids: &Option<Vec<String>>, changed: usize| ids.is_none() && changed == 0;
        if unchanged(&messages.ids, messages.changed.len())
            && unchanged(&posts.ids, posts.changed.len())
            && unchanged(&replies.ids, replies.changed.len())
        {
            None
        } else {
            Some(FeedPart {
                messages,
                posts,
                replies,
            })
        }
    }
}
/// The app a bot message comes from, as `fromAppMetadata` names it (English for catalog apps).
pub(crate) fn app_metadata_name(m: &Message) -> Option<String> {
    super::bot_apps::metadata(m, "name")
}
/// When a message was written, else when it reached the server (RFC 3339).
fn sent_at(m: &Message) -> Option<&str> {
    m.composetime
        .as_deref()
        .or(m.originalarrivaltime.as_deref())
}
/// When a message reached the server (epoch ms).
pub(crate) fn arrival_ms(m: &Message) -> Option<i64> {
    m.originalarrivaltime
        .as_deref()
        .or(m.composetime.as_deref())
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp_millis())
}
/// How far an unread chat was read: its consumption horizon if the chat list has one, else just before its last message.
pub(crate) fn read_horizon(c: &Conversation) -> Option<i64> {
    if !c.is_unread() {
        return None;
    }
    let number = |v: &serde_json::Value| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(teams_chat::receipts::horizon_time))
    };
    let csa = c.extra.get("consumptionHorizon").and_then(|v| match v {
        serde_json::Value::Object(o) => o.get("originalArrivalTime").and_then(number),
        other => number(other),
    });
    let chat_service = c
        .properties
        .get("consumptionhorizon")
        .and_then(|v| v.as_str())
        .and_then(teams_chat::receipts::horizon_time);
    csa.or(chat_service)
        .or_else(|| Some(arrival_ms(c.last_message.as_ref()?)? - 1))
}
fn is_pinned(c: &Conversation) -> bool {
    c.is_sticky == Some(true)
        || [&c.extra, &c.properties].into_iter().any(|properties| {
            ["isSticky", "favorite", "isFavorite"]
                .into_iter()
                .any(|key| {
                    properties.get(key).is_some_and(|value| {
                        value.as_bool() == Some(true) || value.as_str() == Some("true")
                    })
                })
        })
}
pub(super) fn matches_search(value: &str, q: &str) -> bool {
    q.trim().is_empty() || value.to_lowercase().contains(&q.trim().to_lowercase())
}
fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() > max {
        format!("{}…", value.chars().take(max).collect::<String>())
    } else {
        value.to_owned()
    }
}
fn date_label(iso: &str) -> String {
    DateTime::parse_from_rfc3339(iso)
        .map(|t| {
            let d = t.with_timezone(&Local);
            if d.date_naive() == Local::now().date_naive() {
                teams_i18n::t("core.view.today").into()
            } else {
                d.format(teams_i18n::t("core.view.m_d_y")).to_string()
            }
        })
        .unwrap_or_default()
}
impl App {
    pub(crate) fn file_view(&self, file: &Attachment) -> FileView {
        let description = if file.deleted {
            t("core.view.file_deleted")
        } else if file.url.is_some() {
            t("core.view.shared_file")
        } else {
            t("core.view.file_link_unavailable")
        };
        let ext = file
            .name
            .rsplit_once('.')
            .map(|(_, e)| e.to_uppercase())
            .filter(|e| (1..=4).contains(&e.chars().count()))
            .unwrap_or_else(|| t("core.view.file").into());
        let key = file.key();
        FileView {
            progress: self.transfer_progress(&key),
            can_download: !file.deleted
                && file.link().is_some_and(|link| !self.is_foreign_link(link)),
            thumbnail: file
                .thumbnail
                .as_ref()
                .filter(|_| !file.deleted)
                .map(|src| {
                    self.image_view(&html::InlineImage {
                        src: src.clone(),
                        alt: file.name.clone(),
                        ..Default::default()
                    })
                })
                // A file without a thumbnail keeps its badge instead of an error.
                .filter(|v| !matches!(v.status, ImageStatus::Failed { .. })),
            date: String::new(),
            is_link: false,
            key,
            name: file.name.clone(),
            url: (!file.deleted).then(|| file.url.clone()).flatten(),
            deleted: file.deleted,
            ext,
            description: description.into(),
        }
    }
}
fn card_input_view(input: &crate::cards::CardInput) -> CardInputView {
    use crate::cards::InputKind as K;
    CardInputView {
        id: input.id.clone(),
        label: input.label.clone(),
        value: input.value.clone(),
        required: input.required,
        kind: match &input.kind {
            K::Text {
                placeholder,
                multiline,
            } => CardInputKind::Text {
                placeholder: placeholder.clone(),
                multiline: *multiline,
            },
            K::Number { placeholder } => CardInputKind::Number {
                placeholder: placeholder.clone(),
            },
            K::Date => CardInputKind::Date,
            K::Time => CardInputKind::Time,
            K::Toggle { title, on, off } => CardInputKind::Toggle {
                title: title.clone(),
                on: on.clone(),
                off: off.clone(),
            },
            K::Choice { choices, multi } => CardInputKind::Choice {
                choices: choices
                    .iter()
                    .map(|(title, value)| CardChoiceView {
                        title: title.clone(),
                        value: value.clone(),
                    })
                    .collect(),
                multi: *multi,
            },
        },
    }
}
/// `membershipSummary.totalMemberCount` of a team or channel, as the lists carry it.
fn summary_count(extra: &serde_json::Map<String, Value>) -> Option<u32> {
    let count = extra.get("membershipSummary")?.get("totalMemberCount")?;
    count
        .as_u64()
        .or_else(|| count.as_str()?.parse().ok())
        .filter(|n| *n > 0)
        .map(|n| n as u32)
}
impl App {
    /// The header's count before the roster loads: a private/shared channel's own summary, else the team's, as the web client shows.
    fn channel_member_count(&self, team: &str, channel: &str) -> Option<u32> {
        let team = self.teams.iter().find(|t| t.id == team)?;
        let channel = team.channels.iter().find(|c| c.id == channel);
        let standard = channel.is_none_or(|c| c.channel_type.eq_ignore_ascii_case("standard"));
        if standard {
            summary_count(&team.extra)
        } else {
            channel.and_then(|c| summary_count(&c.extra))
        }
    }
}
fn person_view(p: &Person) -> PersonView {
    PersonView {
        mri: p.mri.clone(),
        name: p.name().to_owned(),
        email: p.email.clone(),
    }
}
fn run_views(runs: &[html::Run]) -> Vec<RunView> {
    runs.iter()
        .map(|r| RunView {
            text: r.text.clone(),
            link: r.link.clone(),
            bold: r.bold,
            italic: r.italic,
            code: r.code,
            underline: r.underline,
            strike: r.strike,
            color: r.color.clone(),
            highlight: r.highlight.clone(),
            size: r.size,
            mention: r.mention,
        })
        .collect()
}
fn message_text(runs: &[html::Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

impl App {
    fn shell(&self) -> Shell {
        Shell {
            signed_in: self.client.is_some(),
            signing_in: *self.signing_in,
            me: self.me.clone(),
            display_name: if self.display_name.trim().is_empty() {
                t("core.view.your_account").into()
            } else {
                self.display_name.clone()
            },
            account_email: self.account_email.clone(),
            browser_login: self.login.is_some(),
            callback: self.callback.clone(),
            nav: *self.nav,
            filter: if *self.nav == Nav::Activity {
                if self.activity.unread_only {
                    Filter::Unread
                } else {
                    Filter::All
                }
            } else {
                *self.filter
            },
            tab: *self.tab,
            search: self.search.clone(),
            error: self.error.clone(),
            notices: self
                .notice
                .iter()
                .chain(self.participants_notice.iter())
                .cloned()
                .collect(),
            // A hidden chat is out of the list, so it cannot be read: counting it left a ghost in the badge.
            unread: self
                .chats
                .iter()
                .filter(|c| c.is_unread() && !self.is_hidden(c))
                .count() as u32,
            activity_unread: self.activity_unread_count(),
            teams_unread: self.teams.iter().any(|t| self.team_unread(t)),
            // Toggled every 30 s by background refreshes; only matters while a list is empty or can grow, so skip redrawing the window.
            loading_workspace: *self.loading_workspace
                && (self.chats.is_empty() || self.teams.is_empty() || *self.more_chats),
            selected: self.selected.as_ref().map(Target::from),
            reply_open: self.conv.reply().is_some(),
            read_only: self.read_only(),
            editing: self.conv.editing().cloned(),
        }
    }
    pub(crate) fn composer_view(&self) -> ComposerView {
        ComposerView {
            sending: self.conv.sending(),
            draft: self.conv.draft().to_owned(),
            draft_rev: self.conv.draft_rev(),
            draft_cursor: self.conv.cursor(),
            mention_ranges: self.mention_ranges(),
            draft_formats: self.draft_formats(),
            mention_loading: self
                .selected
                .as_ref()
                .is_some_and(|s| self.members_loading.contains(&s.key())),
            mention_error: self
                .selected
                .as_ref()
                .and_then(|s| self.members_errors.get(&s.key()).cloned()),
            subject: self.conv.subject().to_owned(),
            importance: self.conv.importance(),
            attachments: self.pending_views(),
            cards: self.composer_cards(),
            link_preview: self.composer_link_preview(),
        }
    }
    pub(crate) fn sidebar(&self) -> Sidebar {
        let mut chats: Vec<_> = self
            .chats
            .iter()
            .filter(|c| {
                let filter = match *self.filter {
                    Filter::Unread => c.is_unread(),
                    Filter::Meetings => c.is_meeting(),
                    Filter::All => true,
                };
                // A hidden chat comes back with a search, so it can be shown again.
                let shown = !self.is_hidden(c) || !self.search.trim().is_empty();
                shown && filter && matches_search(&chat_title(c, &self.me), &self.search)
            })
            .collect();
        // The key parses dates: compute it once per chat, not once per comparison.
        chats.sort_by_cached_key(|c| {
            (
                std::cmp::Reverse(self.is_favorite(c)),
                std::cmp::Reverse(conversations::last_message_time(c)),
            )
        });
        let chats = chats.into_iter().map(|c| self.chat_row(c)).collect();
        let (teams, team_controls) = self.team_list_rows();
        Sidebar {
            // Only the Activity screen lists them; elsewhere they would be rebuilt for nothing.
            activities: if *self.nav == Nav::Activity {
                self.activity_rows()
            } else {
                vec![]
            },
            activity_loading: self.activity.loading,
            more_activity: self.activity.cursor.is_some(),
            activity_error: self.activity.error.clone(),
            chats,
            teams,
            team_controls,
            team_count: self.teams.len() as u32,
            more_chats: *self.more_chats,
            teams_problem: self.teams_problem.clone(),
        }
    }
    fn is_favorite(&self, c: &Conversation) -> bool {
        self.favorites
            .as_ref()
            .map_or_else(|| is_pinned(c), |ids| ids.contains(&c.id))
    }
    fn chat_row(&self, c: &Conversation) -> ChatRow {
        let preview = c
            .last_message
            .as_ref()
            .map(|m| {
                // Calls and roster changes carry XML: their sentence, not the ids in it.
                if let Some(summary) = crate::attachments::system_summary(m) {
                    return summary;
                }
                format!(
                    "{}{}",
                    if m.sender_mri() == *self.me {
                        t("core.view.you")
                    } else {
                        ""
                    },
                    html::to_text_cached(&m.content).replace('\n', " ")
                )
            })
            .unwrap_or_else(|| t("core.view.start_the_conversation").into());
        let time = c
            .last_message
            .as_ref()
            .and_then(sent_at)
            .map(short_time)
            .unwrap_or_default();
        ChatRow {
            id: c.id.clone(),
            title: chat_title(c, &self.me),
            preview: truncate(&preview, 120),
            time,
            unread: c.is_unread(),
            pinned: self.is_favorite(c),
            hidden: self.is_hidden(c),
            can_leave: c.id != super::NOTES && c.is_group() && !c.is_meeting(),
            avatar_shape: AvatarShape::of_chat(c, &self.me),
            selected: self.selected.as_ref() == Some(&Selection::Chat(c.id.clone())),
            muted: self.chat_muted(&c.id),
            presence: self.peer_presence(c),
        }
    }
    fn peer_presence(&self, c: &Conversation) -> Option<PresenceView> {
        self.presence_of(chat_peer(c, &self.me)?)
    }
    fn presence_of(&self, mri: &str) -> Option<PresenceView> {
        self.presence
            .get(&mri.to_ascii_lowercase())
            .map(PresenceView::from)
    }
    fn reacting_to(&self, id: &str) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|s| self.conv.reacting(s, id))
    }
    /// A name from the directory, or from chat and channel rosters.
    pub(crate) fn known_name(&self, mri: &str) -> Option<String> {
        self.profiles
            .get(mri)
            .and_then(|p| p.known_name())
            .map(str::to_owned)
            .or_else(|| {
                self.roster_names()
                    .get(&mri.to_ascii_lowercase())
                    .cloned()
                    .flatten()
            })
    }
    /// The first roster entry per lowercase member id, as a linear search would find; built on first use after each update.
    fn roster_names(&self) -> &HashMap<String, Option<String>> {
        self.roster_names.get_or_init(|| {
            let mut names = HashMap::new();
            for m in self
                .chats
                .iter()
                .flat_map(|c| &c.members)
                .chain(self.channel_members.values().flatten())
            {
                names
                    .entry(m.id.to_ascii_lowercase())
                    .or_insert_with(|| m.display_name().map(str::to_owned));
            }
            names
        })
    }
    fn person_name(&self, mri: &str) -> String {
        if mri.eq_ignore_ascii_case(&self.me) {
            return t("core.view.you_2").into();
        }
        self.known_name(mri).unwrap_or_else(|| "Participant".into())
    }
    /// An incoming webhook or Office 365 connector (`28:integration:<id>`), named from its channel's `connectorProfiles`.
    fn connector_name(&self, mri: &str) -> Option<String> {
        let id = mri.strip_prefix("28:")?;
        let bare = id.strip_prefix("integration:").unwrap_or(id);
        self.teams
            .iter()
            .flat_map(|t| &t.channels)
            .filter_map(|c| c.extra.get("connectorProfiles")?.as_array())
            .flatten()
            .find(|p| {
                p.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|p| p.eq_ignore_ascii_case(bare) || p.eq_ignore_ascii_case(id))
            })
            .and_then(|p| p.get("displayName")?.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }
    /// Never a raw id: the directory name is fetched when the message has none.
    pub(crate) fn sender_name(&self, m: &Message) -> String {
        self.bot_app(m)
            .map(|app| app.name.clone())
            .or_else(|| app_metadata_name(m))
            .or_else(|| {
                m.imdisplayname
                    .as_deref()
                    .filter(|s| {
                        !s.trim().is_empty() && !teams_chat::is_placeholder_name(m.sender_mri(), s)
                    })
                    .map(str::to_owned)
            })
            .or_else(|| self.known_name(m.sender_mri()))
            .or_else(|| self.connector_name(m.sender_mri()))
            .unwrap_or_else(|| {
                if m.sender_mri().starts_with("28:") {
                    t("core.view.app").into()
                } else {
                    t("core.view.unknown_sender").into()
                }
            })
    }
    pub(crate) fn message_view(&self, m: &Message) -> MessageView {
        let mine = m.sender_mri() == *self.me;
        let sender = self.sender_name(m);
        let stamp = sent_at(m);
        let deleted = m.is_deleted();
        let rendered = self.conv.rendered().get(m);
        let pending = self.reacting_to(&m.id);
        let read_only = self.read_only();
        let can_react = !pending && !read_only && self.conv.reactions_ready();
        let reactions = if deleted {
            vec![]
        } else {
            m.reactions(&self.me)
                .into_iter()
                .map(|r| {
                    let (emoji, name) = crate::emoji::reaction(&r.key);
                    let mut names: Vec<_> = r.users.iter().map(|u| self.person_name(u)).collect();
                    let missing = r.count.saturating_sub(r.users.len() as u64);
                    if missing > 0 {
                        names.push(if missing == 1 {
                            t("core.view.1_other").into()
                        } else {
                            tf!("core.view.others", missing = missing)
                        });
                    }
                    ReactionView {
                        key: r.key.clone(),
                        emoji,
                        emoji_id: crate::emoji::artwork(&r.key),
                        count: r.count,
                        mine: r.mine,
                        hint: format!(
                            "{name} · {}{}",
                            names.join(", "),
                            if r.mine {
                                t("core.view.click_to_remove")
                            } else {
                                ""
                            }
                        ),
                    }
                })
                .collect()
        };
        let images = if deleted {
            vec![]
        } else {
            rendered
                .images
                .iter()
                .map(|image| self.image_view(image))
                .collect()
        };
        MessageView {
            id: m.id.clone(),
            sender,
            sender_id: m.sender_mri().to_owned(),
            sender_avatar_shape: AvatarShape::of(m.sender_mri()),
            sender_presence: self.presence_of(m.sender_mri()),
            mine,
            time: stamp.map(short_time).unwrap_or_default(),
            day: stamp.map(date_label).unwrap_or_default(),
            subject: m
                .properties
                .get("subject")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            importance: if deleted {
                super::conversation::Importance::Standard
            } else {
                super::conversation::Importance::of(m)
            },
            deleted,
            editing: self.conv.editing() == Some(&m.id),
            runs: if deleted {
                vec![]
            } else {
                run_views(&rendered.runs)
            },
            blocks: if deleted {
                vec![]
            } else {
                self.message_blocks(m, &rendered.blocks)
            },
            images,
            files: if deleted {
                vec![]
            } else {
                rendered.files.iter().map(|f| self.file_view(f)).collect()
            },
            link_preview: rendered
                .link
                .as_ref()
                .filter(|_| !deleted)
                .map(|l| self.link_preview_view(l)),
            reactions,
            pending,
            can_react,
            editable: mine && !deleted && !read_only,
            receipt: None,
        }
    }
    pub(crate) fn image_view(&self, image: &html::InlineImage) -> ImageView {
        ImageView {
            src: image.src.clone(),
            alt: image.alt.clone(),
            status: match self.conv.images().get(&image.src) {
                Some(media::ImageState::Ready(i)) => ImageStatus::Ready {
                    width: i.width,
                    height: i.height,
                },
                Some(media::ImageState::Failed(e)) => ImageStatus::Failed { reason: e.clone() },
                _ => ImageStatus::Loading,
            },
            width: image.width,
            height: image.height,
        }
    }
    fn card_item_views(&self, items: &[crate::cards::CardItem]) -> Vec<CardItemView> {
        use crate::cards::{CardItem, ColumnWidth};
        items
            .iter()
            .map(|item| match item {
                CardItem::Text { runs, muted } => CardItemView::Text {
                    runs: run_views(runs),
                    muted: *muted,
                },
                CardItem::Facts(facts) => CardItemView::Facts {
                    facts: facts
                        .iter()
                        .map(|f| FactView {
                            title: f.title.clone(),
                            value: f.value.clone(),
                        })
                        .collect(),
                },
                CardItem::Image(image) => CardItemView::Image {
                    image: self.image_view(image),
                },
                CardItem::Blocks(blocks) => CardItemView::Blocks {
                    blocks: self.block_views(blocks),
                },
                CardItem::Gap { points, separator } => CardItemView::Gap {
                    points: *points,
                    separator: *separator,
                },
                CardItem::Input(input) => CardItemView::Input {
                    input: card_input_view(input),
                },
                CardItem::Columns(columns) => CardItemView::Columns {
                    columns: columns
                        .iter()
                        .map(|column| CardColumnView {
                            width: match column.width {
                                ColumnWidth::Auto => CardColumnWidth::Auto,
                                ColumnWidth::Stretch => CardColumnWidth::Stretch,
                                ColumnWidth::Weight(weight) => CardColumnWidth::Weight { weight },
                                ColumnWidth::Points(points) => CardColumnWidth::Points { points },
                            },
                            items: self.card_item_views(&column.items),
                        })
                        .collect(),
                },
                CardItem::Emphasis(items) => CardItemView::Emphasis {
                    items: self.card_item_views(items),
                },
            })
            .collect()
    }
    pub(crate) fn block_views(&self, blocks: &[html::Block]) -> Vec<BlockView> {
        use html::Block;
        blocks
            .iter()
            .map(|block| match block {
                Block::Text { runs } => BlockView::Text {
                    runs: run_views(runs),
                },
                Block::List {
                    ordered,
                    start,
                    items,
                } => BlockView::List {
                    ordered: *ordered,
                    start: *start,
                    items: items
                        .iter()
                        .map(|item| ListItemView {
                            blocks: self.block_views(item),
                        })
                        .collect(),
                },
                Block::Quote {
                    author,
                    time,
                    message_id,
                    body,
                } => BlockView::Quote {
                    author: author.clone(),
                    time: time.clone(),
                    message_id: message_id.clone(),
                    body: self.block_views(body),
                },
                Block::Code { language, text } => BlockView::Code {
                    language: language.clone(),
                    text: text.clone(),
                },
                Block::Table { header, rows } => BlockView::Table {
                    header: *header,
                    rows: rows
                        .iter()
                        .map(|row| TableRowView {
                            cells: row
                                .iter()
                                .map(|cell| TableCellView {
                                    runs: run_views(cell),
                                })
                                .collect(),
                        })
                        .collect(),
                },
                Block::Rule => BlockView::Rule,
                Block::Image(image) => BlockView::Image {
                    image: self.image_view(image),
                },
                Block::Card(card) => BlockView::Card {
                    card: CardView {
                        index: 0,
                        footer: None,
                        busy: false,
                        title: card.title.clone(),
                        subtitle: card.subtitle.clone(),
                        accent: card.accent,
                        items: self.card_item_views(&card.items),
                        actions: card
                            .actions
                            .iter()
                            .map(|a| CardActionView {
                                title: a.title.clone(),
                                url: a.url.clone(),
                                kind: match a.kind {
                                    crate::cards::ActionKind::OpenUrl => CardActionKind::OpenUrl,
                                    crate::cards::ActionKind::Submit
                                    | crate::cards::ActionKind::Execute => CardActionKind::Submit,
                                    crate::cards::ActionKind::Unsupported => {
                                        CardActionKind::Unsupported
                                    }
                                },
                            })
                            .collect(),
                    },
                },
            })
            .collect()
    }
    /// Messages the search keeps, oldest first.
    fn visible<'a>(&'a self, messages: &'a [Message]) -> impl Iterator<Item = &'a Message> {
        messages.iter().filter(|m| {
            self.search.trim().is_empty()
                || matches_search(
                    &message_text(&self.conv.rendered().get(m).runs),
                    &self.search,
                )
        })
    }
    /// Posts the search keeps, with their root message.
    fn visible_posts(&self) -> impl Iterator<Item = (&ReplyChain, &Message)> {
        self.conv.posts().iter().filter_map(|post| {
            let root = post.root()?;
            matches_search(
                &message_text(&self.conv.rendered().get(root).runs),
                &self.search,
            )
            .then_some((post, root))
        })
    }
    /// The open conversation's chat messages, channel posts and thread replies.
    pub(crate) fn feed_views(&self) -> (Vec<MessageView>, Vec<PostView>, Vec<MessageView>) {
        let Some(selection) = &*self.selected else {
            return (vec![], vec![], vec![]);
        };
        let channel = selection.is_channel();
        let messages = if channel {
            vec![]
        } else {
            let receipts = self.receipts_for(self.conv.messages());
            self.visible(self.conv.messages())
                .map(|m| {
                    let mut view = self.message_view(m);
                    view.receipt = receipts.get(&view.id).copied();
                    view
                })
                .collect()
        };
        let posts = if channel {
            self.visible_posts()
                .map(|(post, root)| PostView {
                    id: post.id.clone(),
                    root: self.message_view(root),
                    replies: post.messages.len().saturating_sub(1) as u32,
                    following: self.post_followed(selection.id(), &post.id),
                })
                .collect()
        } else {
            vec![]
        };
        let replies = if self.conv.reply().is_some() {
            self.visible(self.conv.replies())
                .map(|m| self.message_view(m))
                .collect()
        } else {
            vec![]
        };
        (messages, posts, replies)
    }
    pub(crate) fn conversation_part(&self) -> ConversationPart {
        let Some(selection) = &*self.selected else {
            return ConversationPart { view: None };
        };
        let channel = selection.is_channel();
        let subtitle = match selection {
            Selection::Channel { team, .. } => self
                .teams
                .iter()
                .find(|t| &t.id == team)
                .map(|t| t.display_name.clone())
                .unwrap_or_default(),
            Selection::Chat(id) => self
                .chats
                .iter()
                .find(|c| &c.id == id)
                .map(|c| {
                    c.members
                        .iter()
                        .filter(|m| m.id != *self.me)
                        .map(|m| m.display_name().unwrap_or("Participant"))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
        };
        let member_count = match selection {
            Selection::Chat(id) => self
                .chats
                .iter()
                .find(|c| &c.id == id)
                .map(|c| c.members.len() as u32),
            s @ Selection::Channel { team, channel } => self
                .channel_members
                .get(&s.key())
                .map(|m| m.len() as u32)
                .or_else(|| self.channel_member_count(team, channel)),
        };
        // Counted without building the views: the header changes far less than they do.
        let shown = if channel {
            self.visible_posts().count()
        } else {
            self.visible(self.conv.messages()).count()
        };
        let empty_text = (shown == 0).then(|| {
            if channel {
                if self.conv.loading() {
                    t("core.view.loading_posts")
                } else if self.conv.posts_error().is_some() {
                    t("core.view.the_posts_could_not_be_loaded")
                } else if !self.search.is_empty() {
                    t("core.view.no_loaded_post_matches_the_search")
                } else {
                    t("core.view.no_posts_write_the_first_message")
                }
            } else if self.conv.loading() {
                t("core.view.loading_messages")
            } else if self.search.is_empty() {
                t("core.view.no_messages_start_the_conversation")
            } else {
                t("core.view.no_loaded_message_matches_the_search")
            }
            .to_owned()
        });
        let thread = self.conv.reply().map(|reply| {
            let no_replies = self.visible(self.conv.replies()).next().is_none();
            let root = self
                .conv
                .posts()
                .iter()
                .find(|p| &p.id == reply)
                .and_then(ReplyChain::root);
            let stamp = root.and_then(sent_at);
            ThreadView {
                post: reply.clone(),
                following: self
                    .selected
                    .as_ref()
                    .filter(|s| s.is_channel())
                    .map(|s| self.post_followed(s.id(), reply)),
                root_preview: root
                    .map(|root| truncate(&message_text(&self.conv.rendered().get(root).runs), 160))
                    .unwrap_or_default(),
                root_sender: root.map(|root| self.sender_name(root)).unwrap_or_default(),
                root_sender_id: root
                    .map(|root| root.sender_mri().to_owned())
                    .unwrap_or_default(),
                root_sender_avatar_shape: root
                    .map(|root| AvatarShape::of(root.sender_mri()))
                    .unwrap_or_default(),
                root_time: root
                    .map(|_| {
                        let day = stamp.map(date_label).unwrap_or_default();
                        let time = stamp.map(short_time).unwrap_or_default();
                        format!("{day} {time}").trim().to_owned()
                    })
                    .unwrap_or_default(),
                empty_text: no_replies.then(|| {
                    if self.conv.loading_replies() {
                        t("core.view.loading_messages")
                    } else if self.search.is_empty() {
                        t("core.view.no_replies_start_the_discussion")
                    } else {
                        t("core.view.no_loaded_message_matches_the_search")
                    }
                    .to_owned()
                }),
                loading: self.conv.loading_replies() && (no_replies || self.conv.more_replies()),
                has_more: self.conv.more_replies(),
            }
        });
        let is_meeting =
            matches!(&selection, Selection::Chat(id) if self.chat_meeting(id).is_some());
        let can_call = matches!(&selection, Selection::Chat(id) if self.chat_call(id).is_ok());
        ConversationPart {
            view: Some(ConversationView {
                first_unread: self.conv.read_horizon().and_then(|horizon| {
                    self.conv
                        .messages()
                        .iter()
                        .find(|m| {
                            m.sender_mri() != *self.me
                                && !m.is_deleted()
                                && arrival_ms(m).is_some_and(|t| t > horizon)
                        })
                        .map(|m| m.id.clone())
                }),
                presence: match &selection {
                    Selection::Chat(id) => self
                        .chats
                        .iter()
                        .find(|c| &c.id == id)
                        .and_then(|c| self.peer_presence(c)),
                    Selection::Channel { .. } => None,
                },
                avatar_shape: match &selection {
                    Selection::Chat(id) => self
                        .chats
                        .iter()
                        .find(|c| &c.id == id)
                        .map_or(AvatarShape::Round, |c| AvatarShape::of_chat(c, &self.me)),
                    Selection::Channel { .. } => AvatarShape::Square,
                },
                is_meeting,
                can_call,
                muted: match selection {
                    Selection::Chat(id) => self.chat_muted(id),
                    Selection::Channel { channel, .. } => self.channel_muted(channel),
                },
                chat: match &selection {
                    Selection::Chat(id) => self
                        .chats
                        .iter()
                        .find(|c| &c.id == id)
                        .map(|c| self.chat_row(c)),
                    Selection::Channel { .. } => None,
                },
                target: selection.into(),
                title: self.title(),
                subtitle,
                is_channel: channel,
                member_count,
                // Same for the 5 s feed refresh: only shown with an empty feed or a "more" button.
                loading: self.conv.loading() && (shown == 0 || self.conv.has_more()),
                has_more: self.conv.has_more(),
                empty_text,
                posts_error: self.conv.posts_error().cloned(),
                thread,
            }),
        }
    }
    pub(crate) fn members_view(&self) -> MembersView {
        let key = self
            .selected
            .as_ref()
            .map(Selection::key)
            .unwrap_or_default();
        let members = match &*self.selected {
            Some(Selection::Chat(id)) => self
                .chats
                .iter()
                .find(|c| &c.id == id)
                .map(|c| c.members.as_slice()),
            Some(Selection::Channel { .. }) => self.channel_members.get(&key).map(Vec::as_slice),
            None => None,
        };
        let managed = self.managed_chat();
        MembersView {
            can_manage: managed.is_some(),
            chat_name: managed
                .and_then(|c| c.title())
                .unwrap_or_default()
                .to_owned(),
            is_channel: self.selected.as_ref().is_some_and(Selection::is_channel),
            members: members.map(|members| {
                members
                    .iter()
                    .map(|member| MemberView {
                        id: member.id.clone(),
                        name: if member.id.eq_ignore_ascii_case(&self.me) {
                            t("core.view.you_2").to_owned()
                        } else {
                            self.profiles
                                .get(&member.id)
                                .and_then(|p| p.known_name())
                                .or_else(|| member.display_name())
                                .unwrap_or(&member.id)
                                .to_owned()
                        },
                        role: match member.role.as_deref().unwrap_or("User") {
                            "Admin" => t("core.view.owner"),
                            "Guest" => t("core.view.guest"),
                            _ => t("core.view.member"),
                        }
                        .into(),
                        presence: self.presence_of(&member.id),
                    })
                    .collect()
            }),
            loading: self.members_loading.contains(&key),
            error: self.members_errors.get(&key).cloned(),
        }
    }
    pub(crate) fn files_view(&self) -> FilesView {
        let (files, loading, error) = self.shared_items();
        FilesView {
            files,
            filter: self.files.filter,
            query: self.files.shared_query.clone(),
            loading,
            error,
        }
    }
    /// People offered by the new-chat dialog: search results, or recent contacts.
    pub(crate) fn candidates(&self) -> Vec<Person> {
        let Some(d) = &*self.new_chat else {
            return vec![];
        };
        let mut people = d.results.clone();
        if d.query.is_empty() {
            let mut seen = HashSet::new();
            for c in &*self.chats {
                for m in &c.members {
                    if !m.id.eq_ignore_ascii_case(&self.me) && seen.insert(m.id.to_lowercase()) {
                        people.push(Person {
                            mri: m.id.clone(),
                            friendly_name: m
                                .extra
                                .get("friendlyName")
                                .and_then(|v| v.as_str())
                                .unwrap_or(&m.id)
                                .into(),
                            ..Default::default()
                        });
                    }
                }
            }
        }
        people
            .into_iter()
            .filter(|p| !p.mri.is_empty() && !d.people.iter().any(|c| c.mri == p.mri))
            .take(20)
            .collect()
    }
    fn new_chat_part(&self) -> NewChatPart {
        NewChatPart {
            dialog: self.new_chat.as_ref().map(|d| NewChatView {
                query: d.query.clone(),
                title: d.title.clone(),
                chosen: d.people.iter().map(person_view).collect(),
                results: self.candidates().iter().map(person_view).collect(),
                searching: d.searching,
                searched: d.searched,
                creating: d.creating,
            }),
        }
    }
    /// Every preview within the budget, open conversation or not, sorted for diffing.
    pub(crate) fn kept_image_srcs(&self) -> Vec<String> {
        let mut srcs: Vec<_> = self
            .conv
            .images()
            .iter()
            .filter(|(_, state)| matches!(state, media::ImageState::Ready(_)))
            .map(|(src, _)| src)
            .chain(self.conv.retained_images().srcs())
            .cloned()
            .collect();
        srcs.sort();
        srcs
    }
    #[cfg(test)]
    pub(crate) fn take_image(&self, src: &str) -> Option<(u32, u32, Vec<u8>)> {
        self.handoff().take_image(src)
    }
}

#[cfg(test)]
mod sidebar_tests {
    use super::*;
    use serde_json::json;

    fn chat(id: &str, time: &str) -> Conversation {
        serde_json::from_value(json!({
            "id": id, "lastMessage": {"id": id, "originalarrivaltime": time}
        }))
        .unwrap()
    }

    /// Name of a message's variant; exhaustive so a new variant must also be sampled in `narrowed_diffs_match_full_rebuilds`.
    fn kind(msg: &Msg) -> &'static str {
        use ConversationMsg as C;
        match msg {
            Msg::TeamList(_) => "TeamList",
            Msg::Conversation(msg) => match msg {
                C::FeedLoaded { .. } => "FeedLoaded",
                C::PostsLoaded { .. } => "PostsLoaded",
                C::MoreFeed => "MoreFeed",
                C::OpenReply(_) => "OpenReply",
                C::CloseReply => "CloseReply",
                C::MoreReplies => "MoreReplies",
                C::RepliesLoaded { .. } => "RepliesLoaded",
                C::SubjectChanged(_) => "SubjectChanged",
                C::SetImportance(_) => "SetImportance",
                C::InsertEmoji(_) => "InsertEmoji",
                C::InsertMention => "InsertMention",
                C::React { .. } => "React",
                C::Reacted { .. } => "Reacted",
                C::ReactionReady => "ReactionReady",
                C::Delete(_) => "Delete",
                C::Mutated { .. } => "Mutated",
                C::ImageLoaded { .. } => "ImageLoaded",
                C::RetryImage(_) => "RetryImage",
                C::Shown(_) => "Shown",
            },
            Msg::FeedMerged(_) => "FeedMerged",
            Msg::ShowError(_) => "ShowError",
            Msg::Activity(_) => "Activity",
            Msg::LoadAvatar(..) => "LoadAvatar",
            Msg::BotAppLoaded { .. } => "BotAppLoaded",
            Msg::AvatarLoaded { .. } => "AvatarLoaded",
            Msg::Calendar(_) => "Calendar",
            Msg::Calls(_) => "Calls",
            Msg::Call(_) => "Call",
            Msg::Files(_) => "Files",
            Msg::Notifications(_) => "Notifications",
            Msg::Receipts(_) => "Receipts",
            Msg::SignIn => "SignIn",
            Msg::LoginReturned(_) => "LoginReturned",
            Msg::LoginCancelled => "LoginCancelled",
            Msg::SilentLoginFailed => "SilentLoginFailed",
            Msg::SilentRenewed(..) => "SilentRenewed",
            Msg::SignInBrowser => "SignInBrowser",
            Msg::CopyLoginUrl => "CopyLoginUrl",
            Msg::BrowserOpened(_) => "BrowserOpened",
            Msg::CallbackChanged(_) => "CallbackChanged",
            Msg::CompleteSignIn => "CompleteSignIn",
            Msg::SignedIn(_) => "SignedIn",
            Msg::SignOut => "SignOut",
            Msg::SessionRestored(..) => "SessionRestored",
            Msg::SessionRemembered(..) => "SessionRemembered",
            Msg::MyProfileLoaded(..) => "MyProfileLoaded",
            Msg::Tick => "Tick",
            Msg::Foreground => "Foreground",
            Msg::SetRefreshMode(_) => "SetRefreshMode",
            Msg::Refresh => "Refresh",
            Msg::Navigate(_) => "Navigate",
            Msg::SetFilter(_) => "SetFilter",
            Msg::SetTab(_) => "SetTab",
            Msg::Search(_) => "Search",
            Msg::DismissError => "DismissError",
            Msg::DismissNotice(_) => "DismissNotice",
            Msg::WorkspaceLoaded(..) => "WorkspaceLoaded",
            Msg::ChatsResolved(..) => "ChatsResolved",
            Msg::Select(_) => "Select",
            Msg::ToggleTeam(_) => "ToggleTeam",
            Msg::LoadChannels(_) => "LoadChannels",
            Msg::ChannelMembers => "ChannelMembers",
            Msg::MembersLoaded { .. } => "MembersLoaded",
            Msg::ChannelsLoaded { .. } => "ChannelsLoaded",
            Msg::MoreChats => "MoreChats",
            Msg::MoreChatsLoaded(..) => "MoreChatsLoaded",
            Msg::DraftChanged(_) => "DraftChanged",
            Msg::RichDraftChanged(..) => "RichDraftChanged",
            Msg::ReplaceDraft { .. } => "ReplaceDraft",
            Msg::Edit(_) => "Edit",
            Msg::CancelEdit => "CancelEdit",
            Msg::SaveEdit => "SaveEdit",
            Msg::EditSaved { .. } => "EditSaved",
            Msg::Send => "Send",
            Msg::Sent { .. } => "Sent",
            Msg::NewChat => "NewChat",
            Msg::CancelNewChat => "CancelNewChat",
            Msg::PeopleQuery(_) => "PeopleQuery",
            Msg::PeopleSearch => "PeopleSearch",
            Msg::PeopleLoaded { .. } => "PeopleLoaded",
            Msg::ChoosePerson(_) => "ChoosePerson",
            Msg::RemovePerson(_) => "RemovePerson",
            Msg::NewTitle(_) => "NewTitle",
            Msg::CreateChat => "CreateChat",
            Msg::ChatCreated { .. } => "ChatCreated",
            Msg::OpenUrl(_) => "OpenUrl",
            Msg::UrlOpened(_) => "UrlOpened",
            Msg::MarkRead(..) => "MarkRead",
            Msg::ReadMarked(..) => "ReadMarked",
            Msg::ProfilesLoaded(..) => "ProfilesLoaded",
            Msg::PresenceLoaded(..) => "PresenceLoaded",
            Msg::Me(_) => "Me",
            Msg::ChatAdmin(_) => "ChatAdmin",
            Msg::TeamsAdmin(_) => "TeamsAdmin",
            Msg::Gifs(_) => "Gifs",
            Msg::Extensions(_) => "Extensions",
            Msg::LinkPreview(_) => "LinkPreview",
            Msg::Card(_) => "Card",
            Msg::FullSearch(_) => "FullSearch",
        }
    }

    /// Every `Msg` and `ConversationMsg` variant in a plausible session: rebuilding only the marked parts must match a full rebuild.
    #[tokio::test]
    async fn narrowed_diffs_match_full_rebuilds() {
        use super::super::{
            activity::ActivityMsg, calendar::CalendarAction, calendar::CalendarMsg,
            call::CallAction, call::CallMsg, calls::CallsAction, calls::CallsMsg,
            files::FileAction, files::FilesMsg, notifications::NotificationsAction,
            notifications::NotificationsMsg, receipts::ReceiptsMsg, team_list::TeamListAction,
            team_list::TeamListMsg, team_list::TeamSort,
        };
        use super::super::{
            chat_admin::{ChatAdminAction, ChatAdminMsg},
            gifs::{GifsAction, GifsMsg},
            me::{MeAction, MeMsg, MyStatus, StatusDuration},
            teams_admin::{TeamsAdminAction, TeamsAdminMsg},
        };
        use ConversationMsg as C;
        let (mut app, _) = App::boot(true);
        let me = app.me.to_string();
        let g = app.generation;
        *app.chats = vec![
            chat("a", "2026-10-15T10:00:00Z"),
            chat("b", "2026-10-15T09:00:00Z"),
        ];
        app.chats[0].members = serde_json::from_value(json!([
            {"id": me, "role": "Admin"}, {"id": "8:orgid:peer", "friendlyName": "Peer"}
        ]))
        .unwrap();
        app.chats[0].is_read = Some(false);
        *app.teams = vec![
            serde_json::from_value(json!({
                "id": "team", "displayName": "Équipe",
                "channels": [{"id": "19:c@thread.tacv2", "displayName": "Canal"}]
            }))
            .unwrap(),
        ];
        let chat_a = Selection::Chat("a".into());
        let channel = Selection::Channel {
            team: "team".into(),
            channel: "19:c@thread.tacv2".into(),
        };
        let image = "https://example.com/i.png";
        let message = |id: &str, from: &str, minute: u32, body: &str| -> Message {
            serde_json::from_value(json!({
                "id": id, "from": from, "content": body,
                "originalarrivaltime": format!("2026-10-15T11:{minute:02}:00Z"),
                "composetime": format!("2026-10-15T11:{minute:02}:00Z"),
            }))
            .unwrap()
        };
        let page = MessagesPage {
            messages: vec![
                message(
                    "m1",
                    "8:orgid:peer",
                    1,
                    &format!("<p>Bonjour <img src=\"{image}\" alt=\"x\"></p>"),
                ),
                message("m2", &me, 2, "<p>Salut</p>"),
            ],
            ..Default::default()
        };
        let posts: PostsPage = serde_json::from_value(json!({"replyChains": [{"id": "p1",
            "messages": [{"id": "p1", "from": "8:orgid:peer", "content": "<p>Publication</p>",
                "originalarrivaltime": "2026-10-15T08:00:00Z"}]}]}))
        .unwrap();
        let replies: RepliesPage = serde_json::from_value(json!({"messages": [{"id": "r1",
            "from": me, "content": "<p>Réponse</p>", "originalarrivaltime": "2026-10-15T08:05:00Z"}]}))
        .unwrap();
        let peer = Person {
            mri: "8:orgid:alice".into(),
            display_name: "Alice".into(),
            ..Default::default()
        };
        let loaded = |bytes: usize| {
            Ok(media::LoadedImage {
                width: 2,
                height: 2,
                data: Arc::new(vec![0; 16]),
                bytes,
            })
        };
        let startup: teams_chat::Startup = serde_json::from_value(json!({
            "chats": [{"id": "a", "isOneOnOne": true, "lastMessage": {"id": "m2",
                "originalArrivalTime": "2026-10-15T11:02:00Z"}}, {"id": "w"}],
            "teams": [{"id": "team", "displayName": "Équipe"}]
        }))
        .unwrap();
        let chats_page: ChatsPage =
            serde_json::from_value(json!({"chats": [{"id": "z"}]})).unwrap();
        let activity_page: MessagesPage = serde_json::from_value(json!({"messages": [{"id": "n1",
            "originalarrivaltime": "2026-10-15T08:00:00Z",
            "properties": {"activity": {"sourceThreadId": "a", "activityType": "mention"}}}]}))
        .unwrap();
        let msgs: Vec<Msg> = vec![
            Msg::Select(chat_a.clone()),
            C::FeedLoaded {
                generation: g,
                selection: chat_a.clone(),
                append: false,
                result: Ok(page),
            }
            .into(),
            Msg::FeedMerged(chat_a.clone()),
            Msg::MarkRead(chat_a.clone(), "m1".into()),
            Msg::ReadMarked(g, Err("lecture".into())),
            Msg::DraftChanged("Bon".into()),
            Msg::RichDraftChanged("Bonjour".into(), vec![]),
            Msg::ReplaceDraft {
                start: 0,
                length: 3,
                text: "Re".into(),
                mention: None,
                emoji: None,
                style: None,
            },
            C::SubjectChanged("Objet".into()).into(),
            C::SetImportance(super::super::conversation::Importance::Urgent).into(),
            C::InsertEmoji("🎉".into()).into(),
            C::InsertMention.into(),
            Msg::CallbackChanged("https://teams.microsoft.com/v2#code=x".into()),
            Msg::Search("bon".into()),
            Msg::Search(String::new()),
            Msg::LoadAvatar("8:orgid:peer".into(), "Peer".into()),
            Msg::BotAppLoaded {
                generation: g,
                id: "app-1".into(),
                result: Err("réseau".into()),
            },
            Msg::AvatarLoaded {
                generation: g,
                id: "8:orgid:peer".into(),
                result: Err(()),
            },
            C::ImageLoaded {
                generation: g,
                selection: chat_a.clone(),
                src: image.into(),
                result: Err("réseau".into()),
            }
            .into(),
            C::RetryImage(image.into()).into(),
            C::Shown(vec!["m2".into()]).into(),
            C::ImageLoaded {
                generation: g,
                selection: chat_a.clone(),
                src: image.into(),
                result: loaded(16),
            }
            .into(),
            C::React {
                id: "m1".into(),
                key: "like".into(),
            }
            .into(),
            C::Reacted {
                generation: g,
                selection: chat_a.clone(),
                id: "m1".into(),
                key: "like".into(),
                remove: false,
                result: Ok(()),
            }
            .into(),
            C::ReactionReady.into(),
            Msg::Edit("m2".into()),
            Msg::DraftChanged("modifié".into()),
            Msg::CancelEdit,
            Msg::Edit("m2".into()),
            Msg::SaveEdit,
            Msg::EditSaved {
                generation: g,
                selection: chat_a.clone(),
                id: "m2".into(),
                html: String::new(),
                result: Ok(()),
            },
            C::Mutated {
                generation: g,
                selection: chat_a.clone(),
                result: Ok(()),
            }
            .into(),
            C::Delete("m2".into()).into(),
            C::Mutated {
                generation: g,
                selection: chat_a.clone(),
                result: Err("refusé".into()),
            }
            .into(),
            Msg::DraftChanged("À envoyer".into()),
            Msg::Send,
            Msg::Sent {
                generation: g,
                selection: chat_a.clone(),
                reply: None,
                content: "À envoyer".into(),
                ids: vec![],
                result: Ok(()),
            },
            Msg::Sent {
                generation: g,
                selection: chat_a.clone(),
                reply: None,
                content: "perdu".into(),
                ids: vec![],
                result: Err("hors ligne".into()),
            },
            C::MoreFeed.into(),
            Msg::Receipts(ReceiptsMsg::TypingSent),
            Msg::Receipts(ReceiptsMsg::HorizonsLoaded(g, "a".into(), Ok(vec![]))),
            Msg::Receipts(ReceiptsMsg::Expire("a".into())),
            Msg::Receipts(ReceiptsMsg::Saved(g, false, Err("refusé".into()))),
            Msg::NewChat,
            Msg::PeopleQuery("al".into()),
            Msg::PeopleSearch,
            Msg::PeopleLoaded {
                generation: g,
                query: "al".into(),
                result: Ok(vec![peer.clone()]),
            },
            Msg::ChoosePerson(peer.clone()),
            Msg::NewTitle("Projet".into()),
            Msg::CreateChat,
            Msg::ChatCreated {
                generation: g,
                people: vec![peer.clone()],
                title: "Projet".into(),
                result: Err("400".into()),
            },
            Msg::RemovePerson(peer.mri.clone()),
            Msg::CancelNewChat,
            Msg::NewChat,
            Msg::ChatCreated {
                generation: g,
                people: vec![peer.clone()],
                title: "Projet".into(),
                result: Ok("c".into()),
            },
            Msg::Tick,
            Msg::ShowError("x".into()),
            Msg::DismissError,
            Msg::DismissNotice("x".into()),
            Msg::Navigate(Nav::Teams),
            Msg::ToggleTeam("team".into()),
            Msg::LoadChannels("team".into()),
            Msg::ChannelsLoaded {
                generation: g,
                team: "team".into(),
                result: Ok((
                    vec![Channel {
                        id: "19:c@thread.tacv2".into(),
                        display_name: "Canal".into(),
                        ..Default::default()
                    }],
                    Some("unjoined: 403".into()),
                )),
            },
            Msg::Select(channel.clone()),
            C::PostsLoaded {
                generation: g,
                selection: channel.clone(),
                append: false,
                result: Ok(posts),
            }
            .into(),
            C::OpenReply("p1".into()).into(),
            C::RepliesLoaded {
                generation: g,
                selection: channel.clone(),
                post: "p1".into(),
                append: false,
                result: Ok(replies),
            }
            .into(),
            C::MoreReplies.into(),
            C::CloseReply.into(),
            Msg::ChannelMembers,
            Msg::MembersLoaded {
                generation: g,
                key: channel.key(),
                result: Ok((
                    serde_json::from_value(json!([{"id": "8:orgid:alice"}])).unwrap(),
                    vec![peer.clone()],
                )),
            },
            Msg::SetTab(Tab::Members),
            Msg::SetTab(Tab::Files),
            Msg::SetTab(Tab::Messages),
            Msg::TeamList(TeamListMsg::Action(TeamListAction::Sort {
                sort: TeamSort::Alphabetical,
            })),
            Msg::TeamList(TeamListMsg::ShownSaved(
                g,
                "team".into(),
                None,
                Err("x".into()),
            )),
            Msg::TeamList(TeamListMsg::OrderSaved(g, vec![], Err("x".into()))),
            Msg::Navigate(Nav::Chat),
            Msg::SetFilter(Filter::Unread),
            Msg::SetFilter(Filter::All),
            Msg::Navigate(Nav::Activity),
            Msg::Activity(ActivityMsg::Loaded(g, false, Ok(activity_page))),
            Msg::Activity(ActivityMsg::Open("n1".into())),
            Msg::Activity(ActivityMsg::Shown),
            Msg::Activity(ActivityMsg::More),
            Msg::Activity(ActivityMsg::ReadMarked(g, "n1".into(), Err("x".into()))),
            Msg::Activity(ActivityMsg::BookmarkLoaded(g, Some(0))),
            Msg::Activity(ActivityMsg::MeetingFound(g, "n1".into(), Ok(None))),
            Msg::SetFilter(Filter::Unread),
            Msg::Navigate(Nav::Calendar),
            Msg::Calendar(CalendarMsg::Action(CalendarAction::Refresh)),
            Msg::Calendar(CalendarMsg::Listed(g, 0, Ok(vec![]))),
            Msg::Calendar(CalendarMsg::Action(CalendarAction::CloseDetail)),
            Msg::Tick,
            Msg::Navigate(Nav::Calls),
            Msg::Calls(CallsMsg::Action(CallsAction::Refresh)),
            Msg::Calls(CallsMsg::Mutated(g, Err("x".into()))),
            Msg::Tick,
            Msg::Call(CallMsg::Action(CallAction::CancelJoin)),
            Msg::Call(CallMsg::Action(CallAction::Dismiss)),
            Msg::Navigate(Nav::Files),
            Msg::Files(FilesMsg::Action(FileAction::Browse {
                place: files::BrowserPlace::MyFiles,
            })),
            Msg::Files(FilesMsg::Action(FileAction::SharedSearch {
                query: "doc".into(),
            })),
            Msg::Files(FilesMsg::Progress("upload".into(), 10)),
            Msg::Files(FilesMsg::Transfer("download".into(), 20)),
            Msg::Navigate(Nav::Settings),
            Msg::Notifications(NotificationsMsg::Active(false)),
            Msg::Notifications(NotificationsMsg::Action(NotificationsAction::Pause {
                minutes: 30,
            })),
            Msg::Notifications(NotificationsMsg::Action(NotificationsAction::Resume)),
            Msg::Notifications(NotificationsMsg::PrefsLoaded(g, Err("x".into()))),
            Msg::Navigate(Nav::Chat),
            Msg::Select(Selection::Chat("b".into())),
            Msg::WorkspaceLoaded(g, Ok(workspace(startup))),
            Msg::ChatsResolved(
                g,
                conversations::Resolution {
                    profiles: vec![peer.clone()],
                    attempted: vec!["w".into()],
                    warning: Some("Participants incomplets".into()),
                    ..Default::default()
                },
            ),
            Msg::DismissNotice("Participants incomplets".into()),
            Msg::MoreChats,
            Msg::MoreChatsLoaded(g, Ok(chats_page)),
            Msg::ProfilesLoaded(g, Ok(vec![peer.clone()])),
            Msg::PresenceLoaded(
                g,
                Ok(vec![Presence {
                    mri: peer.mri.clone(),
                    availability: "Busy".into(),
                    activity: Some("InAMeeting".into()),
                    out_of_office: false,
                    ..Default::default()
                }]),
            ),
            Msg::MyProfileLoaded(g, Ok(peer.clone())),
            Msg::Me(MeMsg::Action(MeAction::SetStatus {
                status: MyStatus::Busy,
                duration: StatusDuration::OneHour,
            })),
            Msg::Me(MeMsg::Saved(g, Err("refusé".into()))),
            Msg::Me(MeMsg::Loaded(
                g,
                Ok(Some(Presence {
                    mri: me.clone(),
                    availability: "Away".into(),
                    note: Some("<p>En congés</p>".into()),
                    ..Default::default()
                })),
            )),
            Msg::Me(MeMsg::RepliesLoaded(g, Err("x".into()))),
            Msg::ChatAdmin(ChatAdminMsg::Action(ChatAdminAction::SetPinned {
                chat_id: "a".into(),
                pinned: true,
            })),
            Msg::ChatAdmin(ChatAdminMsg::Action(ChatAdminAction::MarkUnread {
                chat_id: "a".into(),
            })),
            Msg::ChatAdmin(ChatAdminMsg::Action(ChatAdminAction::Hide {
                chat_id: "b".into(),
            })),
            Msg::ChatAdmin(ChatAdminMsg::Action(ChatAdminAction::StartAdding)),
            Msg::ChatAdmin(ChatAdminMsg::PeopleFound(g, String::new(), Ok(vec![peer.clone()]))),
            Msg::ChatAdmin(ChatAdminMsg::Action(ChatAdminAction::CancelAdding)),
            Msg::TeamsAdmin(TeamsAdminMsg::Action(TeamsAdminAction::OpenGallery)),
            Msg::TeamsAdmin(TeamsAdminMsg::Action(TeamsAdminAction::CloseGallery)),
            Msg::TeamsAdmin(TeamsAdminMsg::TeamCreated(g, Err("x".into()))),
            Msg::Gifs(GifsMsg::Action(GifsAction::Search { query: "chat".into() })),
            Msg::Gifs(GifsMsg::Found(
                g,
                "chat".into(),
                Ok(vec![teams_chat::Gif {
                    url: "https://media.giphy.com/x.gif".into(),
                    preview: "https://media.giphy.com/x_s.gif".into(),
                    width: 200,
                    height: 100,
                    title: "Chat".into(),
                }]),
            )),
            Msg::Extensions(super::super::extensions::ExtensionsMsg::Action(
                super::super::extensions::ExtensionsAction::Back,
            )),
            Msg::LinkPreview(super::super::link_preview::LinkPreviewMsg::Action(
                super::super::link_preview::LinkPreviewAction::Dismiss,
            )),
            Msg::Card(super::super::card_actions::CardMsg::Done(
                g,
                ("m1".into(), 0),
                true,
                Err("x".into()),
            )),
            Msg::FullSearch(super::super::search::SearchMsg::Found(
                g,
                0,
                false,
                Err("x".into()),
            )),
            Msg::OpenUrl("https://example.com".into()),
            Msg::OpenUrl("javascript:alert(1)".into()),
            Msg::UrlOpened(Err("navigateur".into())),
            Msg::Refresh,
            Msg::SignIn,
            Msg::LoginReturned("https://login/callback".into()),
            Msg::LoginCancelled,
            Msg::SilentLoginFailed,
            Msg::SilentRenewed(g, Err("refusé".into())),
            Msg::SignInBrowser,
            Msg::CopyLoginUrl,
            Msg::BrowserOpened(Err("navigateur".into())),
            Msg::CompleteSignIn,
            Msg::SessionRemembered(g, Err("trousseau".into())),
            Msg::SessionRestored(g, Ok(None)),
            Msg::SignedIn(Err("refusé".into())),
            Msg::Tick,
            Msg::SignOut,
            Msg::Tick,
        ];
        let mut narrow = Snapshot::default();
        let mut full = Snapshot::default();
        let _ = app.take_changes();
        let _ = narrow.diff(&app, Parts::ALL);
        let _ = full.diff(&app, Parts::ALL);
        let mut covered = HashSet::new();
        for msg in msgs {
            covered.insert(kind(&msg));
            let name: String = format!("{msg:?}").chars().take(120).collect();
            let _ = app.update(msg);
            let parts = app.take_changes();
            assert_eq!(
                narrow.diff(&app, parts),
                full.diff(&app, Parts::ALL),
                "{name}"
            );
        }
        let all = [
            "TeamList",
            "FeedLoaded",
            "PostsLoaded",
            "MoreFeed",
            "OpenReply",
            "CloseReply",
            "MoreReplies",
            "RepliesLoaded",
            "SubjectChanged",
            "InsertEmoji",
            "InsertMention",
            "React",
            "Reacted",
            "ReactionReady",
            "Edit",
            "CancelEdit",
            "SaveEdit",
            "EditSaved",
            "Delete",
            "Mutated",
            "ImageLoaded",
            "RetryImage",
            "Shown",
            "FeedMerged",
            "ShowError",
            "Activity",
            "LoadAvatar",
            "BotAppLoaded",
            "AvatarLoaded",
            "Calendar",
            "Calls",
            "Call",
            "Files",
            "Notifications",
            "Receipts",
            "SignIn",
            "LoginReturned",
            "LoginCancelled",
            "SilentLoginFailed",
            "SilentRenewed",
            "SignInBrowser",
            "CopyLoginUrl",
            "BrowserOpened",
            "CallbackChanged",
            "CompleteSignIn",
            "SignedIn",
            "SignOut",
            "SessionRestored",
            "SessionRemembered",
            "MyProfileLoaded",
            "Tick",
            "Refresh",
            "Navigate",
            "SetFilter",
            "SetTab",
            "Search",
            "DismissError",
            "DismissNotice",
            "WorkspaceLoaded",
            "ChatsResolved",
            "Select",
            "ToggleTeam",
            "LoadChannels",
            "ChannelMembers",
            "MembersLoaded",
            "ChannelsLoaded",
            "MoreChats",
            "MoreChatsLoaded",
            "DraftChanged",
            "RichDraftChanged",
            "ReplaceDraft",
            "Send",
            "Sent",
            "NewChat",
            "CancelNewChat",
            "PeopleQuery",
            "PeopleSearch",
            "PeopleLoaded",
            "ChoosePerson",
            "RemovePerson",
            "NewTitle",
            "CreateChat",
            "ChatCreated",
            "OpenUrl",
            "UrlOpened",
            "MarkRead",
            "ReadMarked",
            "ProfilesLoaded",
            "PresenceLoaded",
            "SetImportance",
            "Me",
            "ChatAdmin",
            "TeamsAdmin",
            "Gifs",
            "Extensions",
            "LinkPreview",
            "Card",
            "FullSearch",
        ];
        let missing: Vec<_> = all.iter().filter(|k| !covered.contains(*k)).collect();
        assert!(missing.is_empty(), "variants without a sample: {missing:?}");
        assert_eq!(covered.len(), all.len(), "a variant is missing from `all`");
    }

    #[test]
    fn channel_headers_count_members_before_the_roster_loads() {
        let mut app = App::empty();
        *app.teams = vec![
            serde_json::from_value(json!({
                "id": "team", "displayName": "Équipe", "membershipSummary": {"totalMemberCount": 12},
                "channels": [
                    {"id": "19:std@thread.tacv2", "displayName": "Canal", "channelType": 0},
                    {"id": "19:priv@thread.tacv2", "displayName": "Privé", "channelType": 1,
                     "membershipSummary": {"totalMemberCount": 3}}
                ]
            }))
            .unwrap(),
        ];
        let count = |app: &mut App, channel: &str| {
            *app.selected = Some(Selection::Channel {
                team: "team".into(),
                channel: channel.into(),
            });
            app.conversation_part().view.unwrap().member_count
        };
        assert_eq!(count(&mut app, "19:std@thread.tacv2"), Some(12));
        assert_eq!(count(&mut app, "19:priv@thread.tacv2"), Some(3));
        // The loaded roster wins once it is there.
        app.channel_members.insert(
            "channel:team:19:std@thread.tacv2".into(),
            serde_json::from_value(json!([{"id": "8:orgid:a"}, {"id": "8:orgid:b"}])).unwrap(),
        );
        assert_eq!(count(&mut app, "19:std@thread.tacv2"), Some(2));
    }

    /// Typing resends the draft, not the list of people who can be mentioned.
    #[test]
    fn keystrokes_do_not_resend_mention_people() {
        let mut app = App::empty();
        *app.me = "8:orgid:me".into();
        let mut chat: Conversation = serde_json::from_value(json!({
            "id": "a", "members": [{"id": "8:orgid:b", "friendlyName": "Bea"}]
        }))
        .unwrap();
        chat.is_read = Some(true);
        *app.chats = vec![chat];
        let mut snapshot = Snapshot::default();
        let mut step = |app: &mut App, msg: Msg| {
            let _ = app.update(msg);
            let parts = app.take_changes();
            snapshot.diff(app, parts)
        };
        let opened = step(&mut app, Msg::Select(Selection::Chat("a".into())));
        assert!(opened.mention_people.is_some());
        let typed = step(&mut app, Msg::DraftChanged("Bon".into()));
        assert_eq!(typed.composer.unwrap().draft, "Bon");
        assert!(typed.mention_people.is_none());
    }

    /// A poll that brings back the same page leaves the screen alone.
    #[test]
    fn unchanged_feed_page_marks_nothing_shown() {
        let mut app = App::empty();
        *app.me = "8:orgid:me".into();
        let mut chat = chat("a", "2026-10-15T11:01:00Z");
        chat.is_read = Some(true);
        *app.chats = vec![chat];
        let selection = Selection::Chat("a".into());
        let _ = app.update(Msg::Select(selection.clone()));
        let page = || {
            Ok(MessagesPage {
                messages: vec![
                    serde_json::from_value(json!({
                        "id": "m1", "from": "8:orgid:peer", "content": "<p>Bonjour</p>",
                        "originalarrivaltime": "2026-10-15T11:01:00Z",
                    }))
                    .unwrap(),
                ],
                ..Default::default()
            })
        };
        let load = |app: &mut App| {
            let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
                generation: app.generation,
                selection: selection.clone(),
                append: false,
                result: page(),
            }));
            app.take_changes()
        };
        assert!(load(&mut app).has(Parts::MESSAGES));
        let again = load(&mut app);
        assert!(!again.has(Parts::MESSAGES), "{again:?}");
        assert!(!again.has(Parts::SIDEBAR), "{again:?}");
    }

    /// The common, frequent messages rebuild only what they show.
    #[test]
    fn frequent_messages_mark_only_their_parts() {
        let mut app = App::empty();
        *app.me = "8:orgid:me".into();
        *app.chats = vec![chat("a", "2026-10-15T10:00:00Z")];
        let _ = app.update(Msg::Select(Selection::Chat("a".into())));
        let _ = app.take_changes();
        let cases: Vec<(Msg, Parts)> = vec![
            (Msg::DraftChanged("Bon".into()), Parts::COMPOSER),
            (
                Msg::RichDraftChanged("Bonjour".into(), vec![]),
                Parts::COMPOSER,
            ),
            (
                ConversationMsg::SubjectChanged("Objet".into()).into(),
                Parts::COMPOSER,
            ),
            (Msg::CallbackChanged("x".into()), Parts::SHELL),
            (Msg::NewChat, Parts::ALL),
            (Msg::PeopleQuery("al".into()), Parts::NEW_CHAT),
            (
                Msg::Notifications(notifications::NotificationsMsg::Active(false)),
                Parts::NOTIFICATIONS,
            ),
            (
                Msg::Receipts(receipts::ReceiptsMsg::Expire("a".into())),
                Parts::TYPING,
            ),
        ];
        for (msg, expected) in cases {
            let name = format!("{msg:?}");
            let _ = app.update(msg);
            let parts = app.take_changes();
            assert!(
                parts.0 & !expected.0 == 0,
                "{name} marked {parts:?}, beyond {expected:?}"
            );
        }
        let _ = app.update(Msg::Search("bon".into()));
        assert!(!app.take_changes().has(Parts::COMPOSER));
        let _ = app.update(Msg::Tick);
        assert!(!app.take_changes().has(Parts::MESSAGES), "the 5 s refresh");
    }

    /// Only the messages that changed cross to Swift; the order comes when it changes.
    #[test]
    fn feed_diffs_carry_only_changed_messages() {
        let mut app = App::empty();
        *app.me = "8:orgid:me".into();
        *app.chats = vec![
            chat("a", "2026-10-15T10:00:00Z"),
            chat("b", "2026-10-15T09:00:00Z"),
        ];
        let selection = Selection::Chat("a".into());
        let image = "https://example.com/i.png";
        let message = |id: &str, minute: u32, body: &str| -> Message {
            serde_json::from_value(json!({
                "id": id, "from": "8:orgid:peer",
                "originalarrivaltime": format!("2026-10-15T11:{minute:02}:00Z"),
                "content": body,
            }))
            .unwrap()
        };
        let mut snapshot = Snapshot::default();
        let mut step = |app: &mut App, msg: Msg| {
            let _ = app.update(msg);
            let parts = app.take_changes();
            snapshot.diff(app, parts)
        };
        let _ = step(&mut app, Msg::Select(selection.clone()));
        let generation = app.generation;
        let page = Ok(MessagesPage {
            messages: vec![
                message("m1", 1, "<p>Bonjour</p>"),
                message("m2", 2, &format!("<p><img src=\"{image}\" alt=\"x\"></p>")),
                message("m3", 3, "<p>Au revoir</p>"),
            ],
            ..Default::default()
        });
        let feed = step(
            &mut app,
            Msg::from(ConversationMsg::FeedLoaded {
                generation,
                selection: selection.clone(),
                append: false,
                result: page,
            }),
        )
        .feed
        .unwrap();
        assert_eq!(feed.messages.ids.unwrap(), ["m1", "m2", "m3"]);
        assert_eq!(feed.messages.changed.len(), 3);

        let update = step(&mut app, Msg::DraftChanged("Bon".into()));
        assert!(update.feed.is_none() && update.conversation.is_none());

        let loaded = Ok(media::LoadedImage {
            width: 2,
            height: 2,
            data: Arc::new(vec![0; 16]),
            bytes: 16,
        });
        // As `load_images` leaves it when the download starts.
        app.conv
            .test_images()
            .insert(image.into(), media::ImageState::Loading);
        let update = step(
            &mut app,
            Msg::from(ConversationMsg::ImageLoaded {
                generation,
                selection,
                src: image.into(),
                result: loaded,
            }),
        );
        let feed = update.feed.unwrap();
        assert!(
            update.conversation.is_none(),
            "the header does not show images"
        );
        assert!(feed.messages.ids.is_none(), "same order");
        assert_eq!(
            feed.messages
                .changed
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["m2"]
        );

        let feed = step(&mut app, Msg::Search("revoir".into())).feed.unwrap();
        assert_eq!(feed.messages.ids.unwrap(), ["m3"]);
        assert!(feed.messages.changed.is_empty(), "Swift still has m3");
        let feed = step(&mut app, Msg::Search(String::new())).feed.unwrap();
        assert_eq!(feed.messages.ids.unwrap(), ["m1", "m2", "m3"]);
        assert_eq!(
            feed.messages.changed.len(),
            2,
            "m1 and m2 were dropped by Swift"
        );

        let feed = step(&mut app, Msg::Select(Selection::Chat("b".into())))
            .feed
            .unwrap();
        assert_eq!(feed.messages.ids.unwrap(), Vec::<String>::new());
    }

    #[test]
    fn favorites_precede_chats_sorted_by_message_instant_with_missing_dates_last() {
        let mut app = App::empty();
        let mut favorite = chat("favorite", "2026-10-01T09:00:00Z");
        favorite.is_sticky = Some(true);
        *app.chats = vec![
            chat("missing", "invalid"),
            chat("older", "2026-10-15T12:00:00+02:00"),
            favorite,
            chat("newer", "2026-10-15T10:30:00Z"),
        ];
        let rows = app.sidebar().chats;
        assert_eq!(
            rows.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["favorite", "newer", "older", "missing"]
        );
        assert!(rows[0].pinned);
        *app.filter = Filter::Unread;
        app.chats[3].is_read = Some(false);
        assert_eq!(app.sidebar().chats[0].id, "newer");
        assert_eq!(app.sidebar().chats.len(), 1);
    }

    #[test]
    fn the_open_chat_carries_its_list_row_even_when_filtered_out() {
        let mut app = App::empty();
        let mut favorite = chat("favorite", "2026-10-01T09:00:00Z");
        favorite.is_sticky = Some(true);
        *app.chats = vec![favorite, chat("other", "2026-10-01T09:00:00Z")];
        *app.filter = Filter::Unread;
        let mut pinned = |id: &str| {
            *app.selected = Some(Selection::Chat(id.into()));
            app.conversation_part().view.unwrap().chat.unwrap().pinned
        };
        assert!(pinned("favorite"));
        assert!(!pinned("other"));
    }

    #[test]
    fn hidden_unread_chats_stay_out_of_the_unread_count() {
        let mut app = App::empty();
        let mut hidden = chat("hidden", "2026-10-15T10:00:00Z");
        hidden.is_read = Some(false);
        let mut shown = chat("shown", "2026-10-15T10:00:00Z");
        shown.is_read = Some(false);
        *app.chats = vec![hidden, shown];
        assert_eq!(app.shell().unread, 2);
        let after = conversations::last_message_time(&app.chats[0]).unwrap();
        app.hidden_chats.insert("hidden".into(), after);
        assert_eq!(app.shell().unread, 1);
    }

    #[test]
    fn call_events_preview_as_a_sentence_not_their_xml() {
        let mut app = App::empty();
        *app.me = "8:orgid:me".into();
        *app.chats = vec![
            serde_json::from_value(json!({"id": "c", "lastMessage": {
                "id": "1", "messagetype": "Event/Call", "from": "8:orgid:me",
                "content": "<partlist type=\"ended\" alt=\"\"><part identity=\"8:orgid:me\"><name>8:orgid:me</name><duration>42</duration></part></partlist>"
            }}))
            .unwrap(),
        ];
        assert_eq!(app.sidebar().chats[0].preview, "Appel terminé.");
    }

    #[test]
    fn csa_favorites_folder_is_authoritative_and_applies_to_later_pages() {
        let startup: Startup = serde_json::from_value(json!({
            "chats": [{"id": "legacy", "isSticky": true}, {"id": "favorite"}],
            "conversationFolders": {"conversationFolders": [
                {"folderType": "Favorites", "conversationFolderItems": [
                    {"conversationId": "favorite"}, {"conversationId": "later"}]},
                {"folderType": "Favorites", "isDeleted": true,
                    "conversationFolderItems": [{"conversationId": "legacy"}]},
                {"folderType": "UserCreated", "conversationFolderItems": [{"conversationId": "legacy"}]}
            ]}
        })).unwrap();
        let mut app = App::empty();
        let _ = app.update(Msg::WorkspaceLoaded(0, Ok(workspace(startup))));
        app.chats.push(chat("later", "2026-10-15T10:00:00Z"));
        let rows = app.sidebar().chats;
        assert!(rows.iter().find(|c| c.id == "favorite").unwrap().pinned);
        assert!(rows.iter().find(|c| c.id == "later").unwrap().pinned);
        assert!(!rows.iter().find(|c| c.id == "legacy").unwrap().pinned);
    }

    #[test]
    fn sent_and_received_feed_messages_reorder_chats_and_survive_stale_refresh() {
        for sender in ["me", "peer"] {
            let mut app = App::empty();
            *app.me = "me".into();
            *app.chats = vec![
                chat("a", "2026-10-15T10:00:00Z"),
                chat("b", "2026-10-15T09:00:00Z"),
            ];
            let selection = Selection::Chat("b".into());
            *app.selected = Some(selection.clone());
            let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
                generation: 0,
                selection: selection.clone(),
                append: false,
                result: Ok(MessagesPage {
                    messages: vec![
                        serde_json::from_value(json!({"id": "latest", "from": sender,
                    "content": "Bonjour", "originalarrivaltime": "2026-10-15T11:00:00Z"}))
                        .unwrap(),
                    ],
                    ..Default::default()
                }),
            }));
            assert_eq!(app.sidebar().chats[0].id, "b");
            assert!(app.sidebar().chats[0].preview.ends_with("Bonjour"));
            let stale: Startup = serde_json::from_value(json!({"chats": [
                {"id": "a", "lastMessage": {"id": "a", "originalarrivaltime": "2026-10-15T10:00:00Z"}},
                {"id": "b", "lastMessage": {"id": "b", "originalarrivaltime": "2026-10-15T09:00:00Z"}}
            ]})).unwrap();
            let _ = app.update(Msg::WorkspaceLoaded(0, Ok(workspace(stale))));
            assert_eq!(app.sidebar().chats[0].id, "b");
            assert!(app.sidebar().chats[0].preview.ends_with("Bonjour"));
            // An older history page must never push the chat back down.
            let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
                generation: 0,
                selection,
                append: true,
                result: Ok(MessagesPage {
                    messages: vec![
                        chat("history", "2026-10-01T10:00:00Z")
                            .last_message
                            .unwrap(),
                    ],
                    ..Default::default()
                }),
            }));
            assert_eq!(app.sidebar().chats[0].id, "b");
        }
    }
}
