//! The open conversation (messages, posts, thread, drafts, edits, reactions, previews), changed only through these methods.
use super::changes::{Parts, Tracked, feeds};
use super::*;
use teams_i18n::{t, tf};

/// Delivery option of a message: `properties.importance` is `""`, `"high"` or `"urgent"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Importance {
    #[default]
    Standard,
    /// "Important".
    High,
    /// "Urgent": Teams notifies the recipients again every two minutes for twenty minutes.
    Urgent,
}
impl Importance {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Standard => "",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
    pub(crate) fn of(message: &Message) -> Self {
        match message
            .properties
            .get("importance")
            .and_then(serde_json::Value::as_str)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("high") => Self::High,
            Some("urgent") => Self::Urgent,
            _ => Self::Standard,
        }
    }
}

/// Messages, or channel posts, kept while the reader is at the bottom of the feed.
pub(crate) const MAX_MESSAGES: usize = 300;

impl Ctx<'_> {
    fn is_open(&self, selection: &Selection) -> bool {
        self.selected == Some(selection)
    }
    fn handoff(&self) -> std::sync::MutexGuard<'_, media::Handoff> {
        self.handoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ConversationMsg {
    FeedLoaded {
        generation: u64,
        selection: Selection,
        append: bool,
        result: Result<MessagesPage, CoreError>,
    },
    PostsLoaded {
        generation: u64,
        selection: Selection,
        append: bool,
        result: Result<PostsPage, CoreError>,
    },
    MoreFeed,
    OpenReply(String),
    CloseReply,
    MoreReplies,
    RepliesLoaded {
        generation: u64,
        selection: Selection,
        post: String,
        append: bool,
        result: Result<RepliesPage, String>,
    },
    SubjectChanged(String),
    SetImportance(Importance),
    InsertEmoji(String),
    InsertMention,
    React {
        id: String,
        key: String,
    },
    Reacted {
        generation: u64,
        selection: Selection,
        id: String,
        key: String,
        remove: bool,
        result: Result<(), String>,
    },
    /// The pause between two reactions is over.
    ReactionReady,
    Delete(String),
    Mutated {
        generation: u64,
        selection: Selection,
        result: Result<(), String>,
    },
    ImageLoaded {
        generation: u64,
        selection: Selection,
        src: String,
        result: Result<media::LoadedImage, String>,
    },
    RetryImage(String),
    /// Ids of the messages and posts on screen and about a screen around it.
    Shown(Vec<String>),
}
impl From<ConversationMsg> for Msg {
    fn from(msg: ConversationMsg) -> Self {
        Msg::Conversation(msg)
    }
}

/// Each field says which parts of the screen it feeds: see [`Tracked`].
#[derive(Default)]
pub(crate) struct ConversationState {
    messages: Tracked<Vec<Message>, { feeds::ALL }>,
    posts: Tracked<Vec<ReplyChain>, { feeds::ALL }>,
    posts_error: Tracked<Option<String>, { feeds::CONVERSATION }>,
    feed_cursor: Tracked<Option<String>, { feeds::CONVERSATION }>,
    loading: Tracked<bool, { feeds::CONVERSATION }>,
    /// Older pages were asked for since the conversation opened, so the feed is not trimmed under the reader.
    paged: bool,
    /// Root post of the open thread.
    reply: Tracked<Option<String>, { feeds::ALL }>,
    replies: Tracked<Vec<Message>, { feeds::ALL }>,
    reply_cursor: Tracked<Option<String>, { feeds::CONVERSATION }>,
    loading_replies: Tracked<bool, { feeds::CONVERSATION }>,
    draft: Tracked<String, { feeds::COMPOSER }>,
    /// Bumped whenever the core itself replaces the draft, so the editor only resets then.
    draft_rev: Tracked<u64, { feeds::COMPOSER }>,
    /// Where the editor puts its cursor after the core replaced the draft.
    cursor: Tracked<u32, { feeds::COMPOSER }>,
    drafts: Tracked<HashMap<String, String>, { feeds::COMPOSER }>,
    subjects: Tracked<HashMap<String, String>, { feeds::COMPOSER }>,
    subject: Tracked<String, { feeds::COMPOSER }>,
    /// Delivery option of each saved draft, and of the open one.
    importances: Tracked<HashMap<String, Importance>, { feeds::COMPOSER }>,
    importance: Tracked<Importance, { feeds::COMPOSER }>,
    sending: Tracked<bool, { feeds::COMPOSER }>,
    editing: Tracked<Option<String>, { feeds::EDITING }>,
    reacting: Tracked<HashSet<(String, String)>, { feeds::MESSAGES }>,
    next_reaction: Tracked<Option<std::time::Instant>, { feeds::MESSAGES }>,
    images: Tracked<HashMap<String, media::ImageState>, { feeds::IMAGES }>,
    /// Previews of earlier conversations still held by Swift.
    retained_images: Tracked<media::Retained, { feeds::IMAGES }>,
    /// Messages and posts on screen: their previews load first and are never dropped for others.
    shown: HashSet<String>,
    /// Derived from the messages, so never a change of its own.
    rendered: attachments::Cache,
    /// Read horizon (epoch ms) when the chat opened: later messages sit under « Nouveaux messages ».
    read_horizon: Tracked<Option<i64>, { feeds::CONVERSATION }>,
}

impl ConversationState {
    /// The parts changed since the previous call.
    pub(super) fn take_changes(&mut self) -> Parts {
        // Exhaustive on purpose: a new field must say which parts it feeds.
        let Self {
            messages,
            posts,
            posts_error,
            feed_cursor,
            loading,
            paged: _,
            reply,
            replies,
            reply_cursor,
            loading_replies,
            draft,
            draft_rev,
            cursor,
            drafts,
            subjects,
            subject,
            importances,
            importance,
            sending,
            editing,
            reacting,
            next_reaction,
            images,
            retained_images,
            shown: _,
            rendered: _,
            read_horizon,
        } = self;
        messages.take_changes()
            | posts.take_changes()
            | posts_error.take_changes()
            | feed_cursor.take_changes()
            | loading.take_changes()
            | reply.take_changes()
            | replies.take_changes()
            | reply_cursor.take_changes()
            | loading_replies.take_changes()
            | draft.take_changes()
            | draft_rev.take_changes()
            | cursor.take_changes()
            | drafts.take_changes()
            | subjects.take_changes()
            | subject.take_changes()
            | importances.take_changes()
            | importance.take_changes()
            | sending.take_changes()
            | editing.take_changes()
            | reacting.take_changes()
            | next_reaction.take_changes()
            | images.take_changes()
            | retained_images.take_changes()
            | read_horizon.take_changes()
    }
    /// After a sign-out: nothing of the previous account, but a new draft revision so Swift's editor resets.
    pub(super) fn after(previous: &Self) -> Self {
        Self {
            draft_rev: Tracked::new(*previous.draft_rev + 1),
            ..Self::default()
        }
    }
    /// Rendered bodies hold localized card fallbacks and quote times.
    pub(crate) fn relocalize(&mut self) {
        self.rendered.clear();
    }
    pub(super) fn messages(&self) -> &[Message] {
        &self.messages
    }
    pub(super) fn posts(&self) -> &[ReplyChain] {
        &self.posts
    }
    pub(super) fn replies(&self) -> &[Message] {
        &self.replies
    }
    /// Every loaded message: chat messages, then channel posts, then thread replies.
    pub(super) fn all_messages(&self) -> impl Iterator<Item = &Message> {
        self.messages
            .iter()
            .chain(self.posts.iter().flat_map(|p| p.messages.iter()))
            .chain(self.replies.iter())
    }
    pub(super) fn reply(&self) -> Option<&String> {
        self.reply.as_ref()
    }
    pub(super) fn posts_error(&self) -> Option<&String> {
        self.posts_error.as_ref()
    }
    pub(super) fn has_more(&self) -> bool {
        self.feed_cursor.is_some()
    }
    pub(super) fn more_replies(&self) -> bool {
        self.reply_cursor.is_some()
    }
    pub(super) fn loading(&self) -> bool {
        *self.loading
    }
    pub(super) fn loading_replies(&self) -> bool {
        *self.loading_replies
    }
    pub(super) fn draft(&self) -> &str {
        &self.draft
    }
    pub(super) fn draft_rev(&self) -> u64 {
        *self.draft_rev
    }
    pub(super) fn cursor(&self) -> u32 {
        *self.cursor
    }
    pub(super) fn subject(&self) -> &str {
        &self.subject
    }
    pub(super) fn importance(&self) -> Importance {
        *self.importance
    }
    pub(super) fn sending(&self) -> bool {
        *self.sending
    }
    pub(super) fn editing(&self) -> Option<&String> {
        self.editing.as_ref()
    }
    pub(super) fn reacting(&self, selection: &Selection, id: &str) -> bool {
        self.reacting.contains(&(selection.key(), id.to_owned()))
    }
    /// The pause between two reactions is over.
    pub(super) fn reactions_ready(&self) -> bool {
        self.next_reaction
            .is_none_or(|t| std::time::Instant::now() >= t)
    }
    pub(super) fn images(&self) -> &HashMap<String, media::ImageState> {
        &self.images
    }
    pub(super) fn retained_images(&self) -> &media::Retained {
        &self.retained_images
    }
    pub(super) fn rendered(&self) -> &attachments::Cache {
        &self.rendered
    }
    pub(super) fn read_horizon(&self) -> Option<i64> {
        *self.read_horizon
    }

    pub(super) fn draft_key(&self, selected: Option<&Selection>) -> String {
        let key = selected.map(Selection::key).unwrap_or_default();
        match self.editing.as_deref() {
            Some(id) => format!("{key}:edit:{id}"),
            None => format!("{key}:{}", self.reply.as_deref().unwrap_or("main")),
        }
    }
    /// Puts the draft aside to edit message `id` in its place; [`Self::end_edit`] brings it back.
    pub(super) fn begin_edit(&mut self, selected: Option<&Selection>, id: String) {
        self.save_draft(selected);
        *self.editing = Some(id);
    }
    pub(super) fn end_edit(&mut self, selected: Option<&Selection>) {
        if self.editing.is_none() {
            return;
        }
        let key = self.draft_key(selected);
        self.drafts.remove(&key);
        self.subjects.remove(&key);
        self.importances.remove(&key);
        *self.editing = None;
        self.restore_draft(selected);
    }
    pub(super) fn save_draft(&mut self, selected: Option<&Selection>) {
        if selected.is_some() {
            let key = self.draft_key(selected);
            self.subjects.insert(key.clone(), self.subject.clone());
            if *self.importance == Importance::Standard {
                self.importances.remove(&key);
            } else {
                self.importances.insert(key.clone(), *self.importance);
            }
            self.drafts.insert(key, self.draft.clone());
        }
    }
    fn restore_draft(&mut self, selected: Option<&Selection>) {
        let draft = self
            .drafts
            .get(&self.draft_key(selected))
            .cloned()
            .unwrap_or_default();
        self.set_draft(draft);
    }
    /// Replace the draft from the core side; the editor resets only for these changes.
    pub(super) fn set_draft(&mut self, value: String) {
        if *self.draft != value {
            *self.draft = value;
        }
        *self.draft_rev += 1;
        *self.cursor = self.draft.encode_utf16().count() as u32;
    }
    /// The editor changed the draft: it already shows this text.
    pub(super) fn draft_typed(&mut self, value: String) {
        *self.draft = value;
    }
    pub(super) fn set_cursor(&mut self, cursor: u32) {
        *self.cursor = cursor;
    }
    /// Opens `to`, saving `from`'s draft; previews Swift holds are retained, in-flight and failed ones dropped.
    pub(super) fn switch(
        &mut self,
        from: Option<&Selection>,
        to: &Selection,
        read_horizon: Option<i64>,
        handoff: &std::sync::Mutex<media::Handoff>,
    ) {
        self.save_draft(from);
        *self.read_horizon = read_horizon;
        *self.reply = None;
        self.replies.clear();
        *self.reply_cursor = None;
        self.restore_draft(Some(to));
        *self.subject = self
            .subjects
            .get(&self.draft_key(Some(to)))
            .cloned()
            .unwrap_or_default();
        *self.importance = self
            .importances
            .get(&self.draft_key(Some(to)))
            .copied()
            .unwrap_or_default();
        let mut handoff = handoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (src, state) in self.images.drain() {
            if let media::ImageState::Ready(image) = state {
                if handoff.waiting(&src) {
                    handoff.withdraw(&src);
                } else {
                    self.retained_images.keep(src, image);
                }
            }
        }
        drop(handoff);
        self.retained_images.evict_to(media::RETAINED_BYTES);
        self.shown.clear();
        self.paged = false;
        self.rendered.clear();
        self.messages.clear();
        self.posts.clear();
        *self.posts_error = None;
        *self.feed_cursor = None;
        *self.loading = false;
        *self.loading_replies = false;
    }
    pub(super) fn begin_send(&mut self) {
        *self.sending = true;
    }
    /// A message went out from draft `key`: forgets it unless edited meanwhile; returns whether it was forgotten.
    pub(super) fn sent(
        &mut self,
        selected: Option<&Selection>,
        key: &str,
        current: bool,
        content: &str,
    ) -> bool {
        // While a message is edited, the draft it went from is put aside.
        let current = current && self.editing.is_none();
        let unchanged = if current {
            *self.draft == content
        } else {
            self.drafts.get(key).map(String::as_str) == Some(content)
        };
        if unchanged {
            self.drafts.remove(key);
        }
        if current && unchanged {
            self.set_draft(String::new());
            self.subject.clear();
            *self.importance = Importance::Standard;
        }
        self.save_draft(selected);
        unchanged
    }
    pub(super) fn send_finished(&mut self) {
        *self.sending = false;
    }
    /// Answering means the new messages were read.
    pub(super) fn forget_read_horizon(&mut self) {
        *self.read_horizon = None;
    }
    #[cfg(test)]
    pub(super) fn image_ready(&mut self, src: String, image: media::LoadedImage) {
        self.images.insert(src, media::ImageState::Ready(image));
    }
    #[cfg(test)]
    pub(super) fn test_messages(&mut self) -> &mut Vec<Message> {
        &mut self.messages
    }
    #[cfg(test)]
    pub(super) fn test_replies(&mut self) -> &mut Vec<Message> {
        &mut self.replies
    }
    #[cfg(test)]
    pub(super) fn test_reply(&mut self) -> &mut Option<String> {
        &mut self.reply
    }
    #[cfg(test)]
    pub(super) fn test_loading(&mut self) -> &mut bool {
        &mut self.loading
    }
    #[cfg(test)]
    pub(super) fn test_images(&mut self) -> &mut HashMap<String, media::ImageState> {
        &mut self.images
    }
    #[cfg(test)]
    pub(super) fn test_drafts(&self) -> &HashMap<String, String> {
        &self.drafts
    }

    pub(super) fn update(&mut self, ctx: &Ctx, msg: ConversationMsg) -> Task<Msg> {
        use ConversationMsg as M;
        match msg {
            M::FeedLoaded { generation, .. }
            | M::PostsLoaded { generation, .. }
            | M::RepliesLoaded { generation, .. }
            | M::Mutated { generation, .. }
                if generation != ctx.generation =>
            {
                Task::none()
            }
            M::FeedLoaded {
                selection,
                append,
                result,
                ..
            } => {
                if !ctx.is_open(&selection) {
                    return Task::none();
                }
                *self.loading = false;
                match result {
                    Ok(page) => {
                        let cursor = page.metadata.older_page_cursor();
                        if append || self.messages.is_empty() {
                            *self.feed_cursor = cursor.clone();
                        }
                        self.paged |= append;
                        // An unchanged poll must not mark every part for a rebuild.
                        if merge_changes(&self.messages, &page.messages) {
                            merge_messages(&mut self.messages, page.messages);
                        }
                        if !append {
                            self.trim_feed(cursor);
                        }
                        Task::done(Msg::FeedMerged(selection))
                    }
                    Err(e) => Task::done(Msg::ShowError(e.into())),
                }
            }
            M::PostsLoaded {
                selection,
                append,
                result,
                ..
            } => {
                if !ctx.is_open(&selection) {
                    return Task::none();
                }
                *self.loading = false;
                let error = match result {
                    Ok(page) => {
                        *self.posts_error = None;
                        let cursor = page.continuation_token;
                        if append || self.posts.is_empty() {
                            *self.feed_cursor = cursor.clone();
                        }
                        self.paged |= append;
                        for post in page.reply_chains {
                            if let Some(existing) = self.posts.iter_mut().find(|p| p.id == post.id)
                            {
                                *existing = post;
                            } else {
                                self.posts.push(post);
                            }
                        }
                        self.posts.sort_by(|a, b| {
                            post_time(a)
                                .cmp(&post_time(b))
                                .then_with(|| a.id.cmp(&b.id))
                        });
                        if append {
                            Task::none()
                        } else {
                            self.trim_posts(cursor);
                            Task::done(Msg::FeedMerged(selection.clone()))
                        }
                    }
                    Err(e) => {
                        let e = String::from(e);
                        *self.posts_error = Some(e.clone());
                        Task::done(Msg::ShowError(e))
                    }
                };
                let replies = if let Some(post) = self.reply.clone() {
                    self.load_replies(ctx, post, false)
                } else {
                    Task::none()
                };
                Task::batch([error, replies])
            }
            M::MoreFeed => self.load_feed(ctx, true),
            M::OpenReply(post) => {
                self.save_draft(ctx.selected);
                *self.reply = Some(post.clone());
                self.replies.clear();
                *self.reply_cursor = None;
                *self.loading_replies = false;
                self.restore_draft(ctx.selected);
                self.load_replies(ctx, post, false)
            }
            M::CloseReply => {
                self.save_draft(ctx.selected);
                *self.reply = None;
                self.replies.clear();
                *self.loading_replies = false;
                self.restore_draft(ctx.selected);
                Task::none()
            }
            M::MoreReplies => {
                if let Some(post) = self.reply.clone() {
                    self.load_replies(ctx, post, true)
                } else {
                    Task::none()
                }
            }
            M::RepliesLoaded {
                selection,
                post,
                append,
                result,
                ..
            } => {
                if !ctx.is_open(&selection) || self.reply.as_ref() != Some(&post) {
                    return Task::none();
                }
                *self.loading_replies = false;
                match result {
                    Ok(page) => {
                        *self.reply_cursor = page.continuation_token;
                        if append {
                            merge_messages(&mut self.replies, page.messages);
                        } else {
                            *self.replies = page.messages;
                            sort_messages(&mut self.replies);
                        }
                        Task::none()
                    }
                    Err(e) => Task::done(Msg::ShowError(e)),
                }
            }
            M::SubjectChanged(value) => {
                *self.subject = value;
                Task::none()
            }
            M::SetImportance(importance) => {
                // Urgent repeats notifications to the people of a chat: posts have no such thing.
                let allowed = importance != Importance::Urgent
                    || !ctx.selected.is_some_and(Selection::is_channel);
                if allowed && *self.importance != importance {
                    *self.importance = importance;
                }
                Task::none()
            }
            M::InsertEmoji(emoji) => {
                self.append_word(&emoji);
                Task::none()
            }
            M::InsertMention => {
                self.append_word("@");
                Task::none()
            }
            M::React { id, key } => self.react(ctx, id, key),
            M::Reacted {
                generation,
                selection,
                id,
                key,
                remove,
                result,
            } => {
                if generation != ctx.generation {
                    return Task::none();
                }
                self.reacting.remove(&(selection.key(), id.clone()));
                let error = match result {
                    Ok(()) if ctx.is_open(&selection) => {
                        for message in self
                            .messages
                            .iter_mut()
                            .chain(self.replies.iter_mut())
                            .chain(self.posts.iter_mut().flat_map(|p| p.messages.iter_mut()))
                            .filter(|m| m.id == id)
                        {
                            message.apply_reaction(ctx.me, &key, remove);
                        }
                        Task::none()
                    }
                    Err(e) => Task::done(Msg::ShowError(tf!(
                        "core.conversation.the_reaction_could_not_be_saved",
                        error = e
                    ))),
                    _ => Task::none(),
                };
                let ready = Task::perform(
                    async {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    },
                    move |_| ConversationMsg::ReactionReady.into(),
                );
                Task::batch([error, ready])
            }
            // `next_reaction` is now in the past: the reaction buttons come back.
            M::ReactionReady => {
                self.next_reaction.touch();
                Task::none()
            }
            M::Delete(id) => {
                let (Some(selection), Some(client)) = (ctx.selected.cloned(), ctx.client.cloned())
                else {
                    return Task::none();
                };
                if self.find_message(&id).is_none_or(|m| m.sender_mri() != ctx.me) {
                    return Task::none();
                }
                let generation = ctx.generation;
                Task::perform(
                    async move {
                        let result = client
                            .delete_message(selection.id(), &id)
                            .await
                            .map_err(|e| e.to_string());
                        (selection, result)
                    },
                    move |(selection, result)| {
                        ConversationMsg::Mutated {
                            generation,
                            selection,
                            result,
                        }
                        .into()
                    },
                )
            }
            M::Mutated {
                selection, result, ..
            } => match result {
                Ok(()) => {
                    if ctx.is_open(&selection) {
                        self.load_feed(ctx, false)
                    } else {
                        Task::none()
                    }
                }
                Err(e) => Task::done(Msg::ShowError(e)),
            },
            M::ImageLoaded {
                generation,
                selection,
                src,
                result,
            } => {
                // Dropped meanwhile to make room for previews on screen.
                let wanted = matches!(self.images.get(&src), Some(media::ImageState::Loading));
                if generation == ctx.generation && ctx.is_open(&selection) && wanted {
                    let state = match result {
                        Ok(image) => self.admit_image(ctx, &src, image),
                        Err(e) => media::ImageState::Failed(e),
                    };
                    self.images.insert(src, state);
                }
                Task::none()
            }
            M::RetryImage(src) => {
                if matches!(self.images.get(&src), Some(media::ImageState::Failed(_))) {
                    self.images.remove(&src);
                }
                Task::none()
            }
            M::Shown(ids) => {
                self.shown = ids.into_iter().collect();
                Task::none()
            }
        }
    }
    /// Keeps the latest messages only; `cursor` (the latest page's) makes "load more" fetch the dropped ones again.
    fn trim_feed(&mut self, cursor: Option<String>) {
        if self.paged || self.messages.len() <= MAX_MESSAGES {
            return;
        }
        let excess = self.messages.len() - MAX_MESSAGES;
        self.messages.drain(..excess);
        *self.feed_cursor = cursor;
        self.forget_dropped();
    }
    /// The same for channel posts; the open thread's post stays.
    fn trim_posts(&mut self, cursor: Option<String>) {
        if self.paged || self.posts.len() <= MAX_MESSAGES {
            return;
        }
        let excess = self.posts.len() - MAX_MESSAGES;
        let reply = self.reply.clone();
        let mut index = 0;
        self.posts.retain(|post| {
            index += 1;
            index > excess || reply.as_ref() == Some(&post.id)
        });
        *self.feed_cursor = cursor;
        self.forget_dropped();
    }
    /// Messages, then thread replies, then channel posts.
    pub(super) fn find_message(&self, id: &str) -> Option<&Message> {
        self.messages
            .iter()
            .chain(self.replies.iter())
            .chain(self.posts.iter().flat_map(|p| p.messages.iter()))
            .find(|m| m.id == id)
    }
    fn append_word(&mut self, text: &str) {
        if !self.draft.is_empty() && !self.draft.ends_with(char::is_whitespace) {
            self.draft.push(' ');
        }
        self.draft.push_str(text);
        *self.draft_rev += 1;
    }
    fn forget_dropped(&self) {
        let loaded: HashSet<&str> = self.all_messages().map(|m| m.id.as_str()).collect();
        self.rendered.retain(|id| loaded.contains(id));
    }
    fn react(&mut self, ctx: &Ctx, id: String, key: String) -> Task<Msg> {
        let (Some(client), Some(selection)) = (ctx.client.cloned(), ctx.selected.cloned()) else {
            return Task::none();
        };
        if ctx.read_only()
            || self.reacting.contains(&(selection.key(), id.clone()))
            || !self.reactions_ready()
        {
            return Task::none();
        }
        let Some(message) = self.find_message(&id).filter(|m| !m.is_deleted()) else {
            return Task::none();
        };
        let remove = message
            .reactions(ctx.me)
            .iter()
            .any(|r| r.key == key && r.mine);
        *self.next_reaction = Some(std::time::Instant::now() + Duration::from_secs(1));
        self.reacting.insert((selection.key(), id.clone()));
        let generation = ctx.generation;
        Task::perform(
            async move {
                let result = if remove {
                    client.remove_reaction(selection.id(), &id, &key).await
                } else {
                    client
                        .add_reaction(
                            selection.id(),
                            &id,
                            &key,
                            chrono::Utc::now().timestamp_millis(),
                        )
                        .await
                }
                .map_err(|e| e.to_string());
                (selection, id, key, remove, result)
            },
            move |(selection, id, key, remove, result)| {
                ConversationMsg::Reacted {
                    generation,
                    selection,
                    id,
                    key,
                    remove,
                    result,
                }
                .into()
            },
        )
    }
    /// A downloaded preview, within the memory budget: earlier conversations' previews make room first, then those off screen.
    fn admit_image(
        &mut self,
        ctx: &Ctx,
        src: &str,
        image: media::LoadedImage,
    ) -> media::ImageState {
        let used = |images: &HashMap<String, media::ImageState>| -> usize {
            images
                .values()
                .map(|i| match i {
                    media::ImageState::Ready(image) => image.bytes,
                    _ => 0,
                })
                .sum()
        };
        if used(&self.images) + image.bytes > media::MAX_IMAGE_BYTES {
            let (order, keep) = self.image_sources(None);
            while used(&self.images) + image.bytes > media::MAX_IMAGE_BYTES
                && self.drop_image(ctx, &order, &keep, true)
            {}
        }
        let used = used(&self.images);
        self.retained_images
            .evict_to(media::MAX_IMAGE_BYTES.saturating_sub(used + image.bytes));
        if used + self.retained_images.bytes() + image.bytes > media::MAX_IMAGE_BYTES {
            media::ImageState::Failed(
                t("core.conversation.preview_memory_full_open_another_chat").into(),
            )
        } else {
            let mut image = image;
            ctx.handoff().offer(src.to_owned(), &mut image);
            media::ImageState::Ready(image)
        }
    }

    pub(super) fn load_feed(&mut self, ctx: &Ctx, append: bool) -> Task<Msg> {
        if *self.loading {
            return Task::none();
        }
        let (Some(client), Some(selection)) = (ctx.client.cloned(), ctx.selected.cloned()) else {
            return Task::none();
        };
        *self.loading = true;
        let generation = ctx.generation;
        let cursor = if append {
            self.feed_cursor.clone()
        } else {
            None
        };
        match selection.clone() {
            Selection::Chat(id) => Task::perform(
                async move {
                    client
                        .messages(
                            &id,
                            &MessagesQuery {
                                page_size: Some(PAGE_SIZE),
                                sync_state: cursor,
                                start_time: Some(1),
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(CoreError::from)
                },
                move |result| {
                    ConversationMsg::FeedLoaded {
                        generation,
                        selection,
                        append,
                        result,
                    }
                    .into()
                },
            ),
            Selection::Channel { team, channel } => {
                let metadata = ctx
                    .teams
                    .iter()
                    .find(|t| t.id == team)
                    .and_then(|t| t.channels.iter().find(|c| c.id == channel))
                    .cloned()
                    .unwrap_or(Channel {
                        id: channel.clone(),
                        ..Default::default()
                    });
                Task::perform(
                    async move {
                        client
                            .channel_posts_for(&team, &metadata, cursor.as_deref())
                            .await
                            .map_err(CoreError::from)
                    },
                    move |result| {
                        ConversationMsg::PostsLoaded {
                            generation,
                            selection,
                            append,
                            result,
                        }
                        .into()
                    },
                )
            }
        }
    }
    fn load_replies(&mut self, ctx: &Ctx, post: String, append: bool) -> Task<Msg> {
        if *self.loading_replies {
            return Task::none();
        }
        let (Some(client), Some(selection @ Selection::Channel { team, channel })) =
            (ctx.client.cloned(), ctx.selected)
        else {
            return Task::none();
        };
        let (selection, team, channel) = (selection.clone(), team.clone(), channel.clone());
        *self.loading_replies = true;
        let generation = ctx.generation;
        let cursor = if append {
            self.reply_cursor.clone()
        } else {
            None
        };
        Task::perform(
            async move {
                let result = client
                    .channel_replies(&team, &channel, &post, cursor.as_deref())
                    .await
                    .map_err(|e| e.to_string());
                (post, result)
            },
            move |(post, result)| {
                ConversationMsg::RepliesLoaded {
                    generation,
                    selection,
                    post,
                    append,
                    result,
                }
                .into()
            },
        )
    }
    /// Every preview of the loaded messages and the draft, in the order they load: the draft's, those on screen,
    /// then the others newest first, open thread before the feed (feeds open at the bottom). Also returns those that stay.
    fn image_sources(&self, draft_image: Option<String>) -> (Vec<String>, HashSet<String>) {
        let messages = || {
            self.replies
                .iter()
                .rev()
                .chain(self.messages.iter().rev())
                .chain(self.posts.iter().rev().flat_map(|p| p.messages.iter()))
        };
        let post_shown: HashSet<&str> = self
            .posts
            .iter()
            .filter(|p| self.shown.contains(&p.id))
            .flat_map(|p| p.messages.iter().map(|m| m.id.as_str()))
            .collect();
        let mut order: Vec<String> = draft_image.into_iter().collect();
        let mut seen: HashSet<String> = order.iter().cloned().collect();
        let mut keep = seen.clone();
        for on_screen in [true, false] {
            for message in messages() {
                let shown =
                    self.shown.contains(&message.id) || post_shown.contains(message.id.as_str());
                if shown != on_screen || message.is_deleted() {
                    continue;
                }
                let rendered = self.rendered.get(message);
                let thumbnails = rendered.files.iter().filter_map(|f| f.thumbnail.as_ref());
                let link = rendered.link.iter().filter_map(|l| l.thumbnail.as_ref());
                for src in rendered
                    .images
                    .iter()
                    .chain(&rendered.card_images)
                    .chain(link)
                    .map(|i| &i.src)
                    .chain(thumbnails)
                {
                    if seen.insert(src.clone()) {
                        order.push(src.clone());
                        if on_screen {
                            keep.insert(src.clone());
                        }
                    }
                }
            }
        }
        (order, keep)
    }
    /// Drops the preview furthest down `order` that is not kept (nor downloading, unless `loading`), so another fits.
    fn drop_image(
        &mut self,
        ctx: &Ctx,
        order: &[String],
        keep: &HashSet<String>,
        loading: bool,
    ) -> bool {
        let rank: HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(i, src)| (src.as_str(), i))
            .collect();
        let Some(src) = self
            .images
            .iter()
            .filter(|(src, state)| {
                !keep.contains(*src) && (loading || !matches!(state, media::ImageState::Loading))
            })
            .max_by_key(|(src, _)| rank.get(src.as_str()).copied().unwrap_or(usize::MAX))
            .map(|(src, _)| src.clone())
        else {
            return false;
        };
        self.images.remove(&src);
        ctx.handoff().withdraw(&src);
        true
    }
    /// Starts the downloads of the previews the loaded messages and the draft's link preview show, up to the budget.
    /// Previews on screen always get a place, taken from those furthest from it.
    pub(super) fn load_images(&mut self, ctx: &Ctx, draft_image: Option<String>) -> Task<Msg> {
        let (Some(client), Some(selection)) = (ctx.client, ctx.selected) else {
            return Task::none();
        };
        let (order, keep) = self.image_sources(draft_image);
        let mut in_flight = self
            .images
            .values()
            .filter(|v| matches!(v, media::ImageState::Loading))
            .count();
        let mut downloads = Vec::new();
        for src in &order {
            if self.images.contains_key(src) {
                continue;
            }
            // Previews kept from an earlier visit come back without a download.
            let retained = self.retained_images.srcs().any(|kept| kept == src);
            if !retained && in_flight >= 4 {
                continue;
            }
            if self.images.len() >= media::MAX_IMAGES
                && !(keep.contains(src) && self.drop_image(ctx, &order, &keep, true))
            {
                break;
            }
            // Looked up first: taking from the kept previews counts as a change.
            if retained && let Some(image) = self.retained_images.take(src) {
                self.images
                    .insert(src.clone(), media::ImageState::Ready(image));
            } else {
                self.images.insert(src.clone(), media::ImageState::Loading);
                in_flight += 1;
                downloads.push(src.clone());
            }
        }
        let generation = ctx.generation;
        Task::batch(
            downloads
                .into_iter()
                .map(|src| {
                    let selection = selection.clone();
                    let loaded = src.clone();
                    Task::perform(media::load(client.clone(), src), move |result| {
                        ConversationMsg::ImageLoaded {
                            generation,
                            selection,
                            src: loaded,
                            result,
                        }
                        .into()
                    })
                })
                .collect::<Vec<_>>(),
        )
    }
}

fn post_time(post: &ReplyChain) -> Option<&String> {
    post.root()
        .and_then(|m| m.originalarrivaltime.as_ref().or(m.composetime.as_ref()))
}
pub(super) fn sort_messages(messages: &mut Vec<Message>) {
    messages.sort_by(|a, b| {
        a.composetime
            .as_ref()
            .or(a.originalarrivaltime.as_ref())
            .cmp(&b.composetime.as_ref().or(b.originalarrivaltime.as_ref()))
            .then(a.id.cmp(&b.id))
    });
    messages.dedup_by(|a, b| a.id == b.id);
}
/// Whether merging `incoming` would add or replace anything.
pub(super) fn merge_changes(target: &[Message], incoming: &[Message]) -> bool {
    let mut index: HashMap<&str, &Message> = HashMap::with_capacity(target.len());
    for m in target {
        index.entry(m.id.as_str()).or_insert(m);
    }
    incoming
        .iter()
        .any(|m| index.get(m.id.as_str()).is_none_or(|current| *current != m))
}

pub(super) fn merge_messages(target: &mut Vec<Message>, incoming: Vec<Message>) {
    // Indexed once: each 5 s refresh merges a page into a history that keeps growing.
    let mut index: HashMap<String, usize> = HashMap::with_capacity(target.len());
    for (i, m) in target.iter().enumerate() {
        index.entry(m.id.clone()).or_insert(i);
    }
    for message in incoming {
        if let Some(&i) = index.get(&message.id) {
            target[i] = message;
        } else {
            index.insert(message.id.clone(), target.len());
            target.push(message);
        }
    }
    sort_messages(target);
}
