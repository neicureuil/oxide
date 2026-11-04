//! Files: attachments (open, download, preview, upload), a chat's "Partagé" tab, and the OneDrive/channel folder browser.
use super::*;
use crate::attachments::Attachment;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use teams_files::{
    Destination, DriveEntry, FileReference, FilesClient, PersonalSite, ServiceName, Shared,
    SharedFile, SharedLink, UploadedFile,
};
use teams_i18n::{t, tf, tp};

pub(crate) type Files = FilesClient<AppAuthenticator>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, uniffi::Enum)]
pub enum SharedFilter {
    #[default]
    All,
    Files,
    Links,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, uniffi::Enum)]
pub enum BrowserPlace {
    #[default]
    Recent,
    MyFiles,
    SharedWithMe,
    /// The open channel's folder, in its team site.
    Channel,
}
#[derive(Debug, Clone, uniffi::Enum)]
pub enum FileAction {
    /// Open in the browser.
    Open {
        key: String,
    },
    CopyLink {
        key: String,
    },
    /// Download to `path`, chosen by the user.
    Download {
        key: String,
        path: String,
    },
    /// Download to a temporary folder and show it in Quick Look.
    Preview {
        key: String,
    },
    Cancel {
        key: String,
    },
    /// Upload local files as attachments of the current draft.
    Attach {
        paths: Vec<String>,
    },
    /// An image pasted in the composer, as PNG: it goes inline in the message, as in Teams, not to OneDrive.
    /// `id` is the one its placeholder in the draft carries (`ComposerFormat::image`); `width` and `height` are
    /// its size on screen, in points.
    PasteImage {
        id: String,
        png: Vec<u8>,
        width: u32,
        height: u32,
    },
    RemoveAttachment {
        id: String,
    },
    SharedFilter {
        filter: SharedFilter,
    },
    /// Filters the 'Partagé' list by name, sender or link; empty shows all.
    SharedSearch {
        query: String,
    },
    RefreshShared,
    Browse {
        place: BrowserPlace,
    },
    OpenFolder {
        key: String,
    },
    /// Go back to the crumb at `index` (0 = root).
    Crumb {
        index: u32,
    },
    NewFolder {
        name: String,
    },
    Rename {
        key: String,
        name: String,
    },
    Delete {
        key: String,
    },
    /// Folder colour, 0 (default yellow) to 15.
    SetColor {
        key: String,
        color: u8,
    },
    /// Upload local files into the folder shown by the browser.
    UploadHere {
        paths: Vec<String>,
    },
    RefreshBrowser,
    /// Search the place shown (the whole OneDrive, or the channel's folder); empty ends it.
    SearchFiles {
        query: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct PendingFileView {
    pub id: String,
    pub name: String,
    pub ext: String,
    /// 0–100.
    pub progress: u8,
    pub ready: bool,
    pub error: Option<String>,
    /// A pasted image: the draft shows it in the text, not as an attachment.
    pub inline: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct EntryView {
    pub key: String,
    pub name: String,
    pub ext: String,
    pub folder: bool,
    pub size: String,
    pub modified: String,
    pub modified_by: String,
    pub progress: Option<u8>,
    pub can_open_folder: bool,
    /// Folder holding a search result.
    pub location: String,
    /// Folder colour, 0 for the default.
    pub color: u8,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record)]
pub struct BrowserView {
    pub place: BrowserPlace,
    pub title: String,
    /// Search shown, empty when browsing.
    pub query: String,
    pub crumbs: Vec<String>,
    pub entries: Vec<EntryView>,
    pub loading: bool,
    pub busy: bool,
    /// New folder, rename, delete and upload are offered.
    pub can_write: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum FilesMsg {
    Action(FileAction),
    Site(u64, Result<(PersonalSite, String), String>),
    /// Upload progress of a draft attachment.
    Progress(String, u8),
    Uploaded(u64, String, Result<Uploaded, String>),
    /// Download progress, by file key.
    Transfer(String, u8),
    Downloaded(u64, String, bool, Result<PathBuf, String>),
    Shared(u64, u64, bool, Result<Vec<SharedFile>, String>),
    Links(u64, u64, Result<Vec<SharedLink>, String>),
    Listed(u64, u64, Result<Vec<DriveEntry>, String>),
    Changed(u64, u64, Result<(), String>),
}

/// Where a draft attachment went.
#[derive(Debug, Clone)]
pub(crate) enum Uploaded {
    /// To OneDrive or the channel's folder, sent as a file reference.
    File(UploadedFile),
    /// To AMS, shown in the message body.
    Image(InlineImage),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineImage {
    pub url: String,
    pub width: u32,
    pub height: u32,
}
impl InlineImage {
    /// The `<img>` the web client writes for a pasted image.
    pub(crate) fn tag(&self) -> String {
        format!(
            "<img src=\"{}\" width=\"{}\" height=\"{}\" alt=\"image\" itemscope=\"\" itemtype=\"http://schema.skype.com/AMSImage\">",
            escape_attr(&self.url),
            self.width,
            self.height
        )
    }
}
enum PendingState {
    /// Waiting for the OneDrive site to be known.
    Waiting,
    Uploading(u8),
    Ready(Uploaded),
    Failed(String),
}
enum Upload {
    /// A local file, uploaded to OneDrive or the channel's folder.
    File(PathBuf),
    /// A pasted image and its size in points, put on AMS to show inline.
    Image {
        png: Arc<Vec<u8>>,
        width: u32,
        height: u32,
    },
}
struct Pending {
    id: String,
    source: Upload,
    name: String,
    state: PendingState,
    cancel: Arc<AtomicBool>,
    selection: Selection,
}
/// Attachment uploads of one draft, kept with it when switching conversations.
type Drafts = HashMap<String, Vec<Pending>>;

#[derive(Default)]
struct Browser {
    view_place: BrowserPlace,
    site: Option<Url>,
    channel: Option<(String, String)>,
    /// (name, server-relative path) from the root down to the folder shown.
    path: Vec<(String, String)>,
    entries: Vec<DriveEntry>,
    query: String,
    loading: bool,
    busy: u32,
    error: Option<String>,
    request: u64,
}

#[derive(Default)]
pub(crate) struct FilesState {
    client: Option<Files>,
    personal: Option<(PersonalSite, String)>,
    site_loading: bool,
    site_error: Option<String>,
    attachments: Drafts,
    next_id: u64,
    transfers: HashMap<String, (u8, Arc<AtomicBool>)>,
    pub(crate) filter: SharedFilter,
    pub(crate) shared_query: String,
    shared_thread: Option<String>,
    shared_files: Option<Vec<SharedFile>>,
    shared_full: bool,
    shared_links: Vec<SharedLink>,
    shared_pending: u8,
    shared_error: Option<String>,
    shared_request: u64,
    browser: Browser,
}

impl FilesState {
    pub(crate) fn browser_place(&self) -> BrowserPlace {
        self.browser.view_place
    }
}

/// What a file key resolves to.
pub(crate) struct Source {
    name: String,
    web: Option<String>,
    /// Sharing or object link to redeem.
    link: Option<String>,
    /// Fallback for the redeem: the item on its site.
    item: Option<(Url, String)>,
    /// A folder-listing entry: its content is read directly.
    content: Option<(Url, String)>,
}

fn ext(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_uppercase())
        .filter(|e| (1..=4).contains(&e.chars().count()))
        .unwrap_or_else(|| t("core.files.file").into())
}
fn my_files() -> &'static str {
    t("core.files.my_files")
}
/// A folder's child count.
fn items(n: u64) -> String {
    tp!("core.files.item_count", n)
}
fn size_label(size: Option<u64>) -> String {
    match size {
        None => String::new(),
        Some(b) if b < 1024 => tf!("core.files.b", bytes = b),
        Some(b) if b < 1024 * 1024 => {
            tf!("core.files.kb", size = format!("{:.0}", b as f64 / 1024.0))
        }
        Some(b) if b < 1024 * 1024 * 1024 => {
            tf!(
                "core.files.mb",
                size = format!("{:.1}", b as f64 / 1048576.0)
            )
        }
        Some(b) => tf!(
            "core.files.gb",
            size = format!("{:.1}", b as f64 / 1073741824.0)
        ),
    }
}
pub(crate) fn date_short(iso: &str) -> String {
    DateTime::parse_from_rfc3339(iso)
        .map(|t| {
            let d = t.with_timezone(&Local);
            if d.date_naive() == Local::now().date_naive() {
                d.format("%H:%M").to_string()
            } else {
                d.format("%d/%m/%Y").to_string()
            }
        })
        .unwrap_or_default()
}
/// Keeps progress messages to whole-percent changes.
fn percent(done: u64, total: u64) -> u8 {
    (done.min(total) * 100).checked_div(total).unwrap_or(0) as u8
}
fn shared_key(f: &SharedFile) -> String {
    format!(
        "shr:{}",
        if f.object_url.is_empty() {
            &f.id
        } else {
            &f.object_url
        }
    )
}
fn shared_link(f: &SharedFile) -> Option<String> {
    [&f.share_url, &f.object_url]
        .into_iter()
        .find(|l| !l.is_empty())
        .cloned()
}
fn entry_key(e: &DriveEntry) -> String {
    format!("drv:{}", e.server_relative_url.as_deref().unwrap_or(&e.id))
}
fn local_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| t("core.files.file_2").into())
}
fn preview_path(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join("Oxide")
        .join("Aperçus")
        .join(uuid::Uuid::new_v4().simple().to_string())
        .join(name.replace(['/', '\\', ':'], "_"))
}

impl App {
    fn files_client(&mut self) -> Option<Files> {
        if self.files.client.is_none() {
            self.files.client = self
                .client
                .as_ref()
                .map(|c| FilesClient::new(c.authenticator()));
        }
        self.files.client.clone()
    }

    pub(crate) fn update_files(&mut self, msg: FilesMsg) -> Task<Msg> {
        let g = self.generation;
        match msg {
            FilesMsg::Site(stamp, _)
            | FilesMsg::Uploaded(stamp, _, _)
            | FilesMsg::Downloaded(stamp, _, _, _)
            | FilesMsg::Shared(stamp, _, _, _)
            | FilesMsg::Links(stamp, _, _)
            | FilesMsg::Listed(stamp, _, _)
            | FilesMsg::Changed(stamp, _, _)
                if stamp != g =>
            {
                Task::none()
            }
            FilesMsg::Action(action) => self.file_action(action),
            FilesMsg::Site(_, result) => {
                self.files.site_loading = false;
                match result {
                    Ok(site) => {
                        self.files.personal = Some(site);
                        self.files.site_error = None;
                    }
                    Err(e) => {
                        self.files.site_error = Some(e.clone());
                        for p in self.files.attachments.values_mut().flatten() {
                            if matches!(p.state, PendingState::Waiting) {
                                p.state = PendingState::Failed(e.clone());
                            }
                        }
                        if matches!(
                            self.files.browser.view_place,
                            BrowserPlace::Recent
                                | BrowserPlace::MyFiles
                                | BrowserPlace::SharedWithMe
                        ) {
                            self.files.browser.loading = false;
                            self.files.browser.error = Some(e);
                        }
                        return Task::none();
                    }
                }
                let waiting: Vec<String> = self
                    .files
                    .attachments
                    .values()
                    .flatten()
                    .filter(|p| matches!(p.state, PendingState::Waiting))
                    .map(|p| p.id.clone())
                    .collect();
                let mut tasks: Vec<_> = waiting
                    .into_iter()
                    .map(|id| self.start_upload(&id))
                    .collect();
                if *self.nav == Nav::Files && self.files.browser.view_place != BrowserPlace::Channel
                {
                    tasks.push(self.browse(self.files.browser.view_place));
                }
                Task::batch(tasks)
            }
            FilesMsg::Progress(id, progress) => {
                if let Some(p) = self.pending_mut(&id)
                    && let PendingState::Uploading(old) = &mut p.state
                {
                    *old = progress;
                }
                Task::none()
            }
            FilesMsg::Uploaded(_, id, result) => {
                if let Some(p) = self.pending_mut(&id) {
                    p.state = match result {
                        Ok(file) => PendingState::Ready(file),
                        Err(e) => PendingState::Failed(e),
                    };
                    return Task::none();
                }
                // Removed meanwhile: do not leave the upload behind in OneDrive.
                match (result, self.files_client()) {
                    (Ok(Uploaded::File(file)), Some(client)) => recycle(client, file),
                    _ => Task::none(),
                }
            }
            FilesMsg::Transfer(key, progress) => {
                if let Some((p, _)) = self.files.transfers.get_mut(&key) {
                    *p = progress;
                }
                Task::none()
            }
            FilesMsg::Downloaded(_, key, preview, result) => {
                self.files.transfers.remove(&key);
                match result {
                    Ok(path) if preview => Task::effect(Effect::QuickLook {
                        path: path.to_string_lossy().into_owned(),
                    }),
                    Ok(path) => {
                        *self.notice = Some(tf!("core.files.downloaded", path = local_name(&path)));
                        Task::effect(Effect::Downloaded {
                            path: path.to_string_lossy().into_owned(),
                        })
                    }
                    Err(e) if e == teams_files::Error::Cancelled.to_string() => Task::none(),
                    Err(e) => {
                        *self.error = Some(tf!("core.files.download_failed", error = e));
                        Task::none()
                    }
                }
            }
            FilesMsg::Shared(_, request, fast, result) => {
                if request != self.files.shared_request {
                    return Task::none();
                }
                self.files.shared_pending = self.files.shared_pending.saturating_sub(1);
                match result {
                    // The complete index replaces the fast one, never the reverse.
                    Ok(files) if !fast || !self.files.shared_full => {
                        self.files.shared_full |= !fast;
                        self.files.shared_files = Some(files);
                        self.files.shared_error = None;
                    }
                    Ok(_) => {}
                    Err(e) if !fast || self.files.shared_files.is_none() => {
                        if self.files.shared_files.is_none() {
                            self.files.shared_error = Some(e);
                        }
                    }
                    Err(_) => {}
                }
                Task::none()
            }
            FilesMsg::Links(_, request, result) => {
                if request == self.files.shared_request {
                    self.files.shared_pending = self.files.shared_pending.saturating_sub(1);
                    if let Ok(links) = result {
                        self.files.shared_links = links;
                    }
                }
                Task::none()
            }
            FilesMsg::Listed(_, request, result) => {
                if request != self.files.browser.request {
                    return Task::none();
                }
                self.files.browser.loading = false;
                match result {
                    Ok(entries) => {
                        self.files.browser.entries = entries;
                        self.files.browser.error = None;
                    }
                    Err(e) => self.files.browser.error = Some(e),
                }
                Task::none()
            }
            FilesMsg::Changed(_, request, result) => {
                self.files.browser.busy = self.files.browser.busy.saturating_sub(1);
                if let Err(e) = result {
                    self.files.browser.error = Some(e);
                }
                if request == self.files.browser.request && self.files.browser.busy == 0 {
                    return self.list_browser();
                }
                Task::none()
            }
        }
    }

    fn pending_mut(&mut self, id: &str) -> Option<&mut Pending> {
        self.files
            .attachments
            .values_mut()
            .flatten()
            .find(|p| p.id == id)
    }

    fn file_action(&mut self, action: FileAction) -> Task<Msg> {
        match action {
            FileAction::Open { key } => match self.source(&key).and_then(|s| s.web) {
                Some(url) => Task::done(Msg::OpenUrl(url)),
                None => {
                    *self.error = Some(t("core.files.this_file_has_no_link_to").into());
                    Task::none()
                }
            },
            FileAction::CopyLink { key } => {
                match self.source(&key).and_then(|s| s.link.or(s.web)) {
                    Some(value) => Task::effect(Effect::Copy { value }),
                    None => Task::none(),
                }
            }
            FileAction::Download { key, path } => {
                if path.trim().is_empty() {
                    return Task::none();
                }
                self.download(key, PathBuf::from(path), false)
            }
            FileAction::Preview { key } => {
                let Some(name) = self.source(&key).map(|s| s.name) else {
                    return Task::none();
                };
                self.download(key, preview_path(&name), true)
            }
            FileAction::Cancel { key } => {
                if let Some((_, cancel)) = self.files.transfers.get(&key) {
                    cancel.store(true, Ordering::Relaxed);
                }
                Task::none()
            }
            FileAction::Attach { paths } => {
                let Some(selection) = self.selected.clone() else {
                    return Task::none();
                };
                if self.read_only() {
                    return Task::none();
                }
                let key = self.draft_key();
                let mut tasks = vec![];
                for path in paths {
                    let path = PathBuf::from(path);
                    if !path.is_file() {
                        *self.error = Some(tf!(
                            "core.files.is_not_a_file_folders_cannot",
                            path = local_name(&path)
                        ));
                        continue;
                    }
                    self.files.next_id += 1;
                    let id = format!("up{}", self.files.next_id);
                    self.files
                        .attachments
                        .entry(key.clone())
                        .or_default()
                        .push(Pending {
                            id: id.clone(),
                            name: local_name(&path),
                            source: Upload::File(path),
                            state: PendingState::Waiting,
                            cancel: Arc::default(),
                            selection: selection.clone(),
                        });
                    tasks.push(self.start_upload(&id));
                }
                Task::batch(tasks)
            }
            FileAction::PasteImage {
                id,
                png,
                width,
                height,
            } => {
                let Some(selection) = self.selected.clone() else {
                    return Task::none();
                };
                if self.read_only() {
                    return Task::none();
                }
                if self.pending_mut(&id).is_some() {
                    return Task::none();
                }
                let key = self.draft_key();
                self.files
                    .attachments
                    .entry(key)
                    .or_default()
                    .push(Pending {
                        id: id.clone(),
                        name: "image.png".into(),
                        source: Upload::Image {
                            png: Arc::new(png),
                            width,
                            height,
                        },
                        state: PendingState::Waiting,
                        cancel: Arc::default(),
                        selection,
                    });
                self.start_upload(&id)
            }
            FileAction::RemoveAttachment { id } => {
                let mut removed = None;
                for list in self.files.attachments.values_mut() {
                    if let Some(i) = list.iter().position(|p| p.id == id) {
                        removed = Some(list.remove(i));
                    }
                }
                let Some(pending) = removed else {
                    return Task::none();
                };
                pending.cancel.store(true, Ordering::Relaxed);
                match (pending.state, self.files_client()) {
                    (PendingState::Ready(Uploaded::File(file)), Some(client)) => {
                        recycle(client, file)
                    }
                    _ => Task::none(),
                }
            }
            FileAction::SharedFilter { filter } => {
                self.files.filter = filter;
                Task::none()
            }
            FileAction::SharedSearch { query } => {
                self.files.shared_query = query.trim().to_owned();
                Task::none()
            }
            FileAction::RefreshShared => self.load_shared(true),
            FileAction::Browse { place } => {
                if place == BrowserPlace::Channel {
                    return self.open_channel_browser();
                }
                self.browse(place)
            }
            FileAction::OpenFolder { key } => {
                let Some(entry) = self
                    .files
                    .browser
                    .entries
                    .iter()
                    .find(|e| entry_key(e) == key && e.folder)
                    .cloned()
                else {
                    return Task::none();
                };
                match entry.server_relative_url {
                    Some(path) if self.files.browser.site.is_some() => {
                        if !self.files.browser.query.is_empty() {
                            // From a result: the folder under the place's root.
                            self.files.browser.query.clear();
                            let root = match self.files.browser.view_place {
                                BrowserPlace::Channel => self.files.browser.path.first().cloned(),
                                _ => self
                                    .files
                                    .personal
                                    .as_ref()
                                    .map(|(s, _)| (my_files().to_owned(), s.library_path())),
                            };
                            if self.files.browser.view_place != BrowserPlace::Channel {
                                self.files.browser.view_place = BrowserPlace::MyFiles;
                            }
                            self.files.browser.path = root.into_iter().collect();
                        }
                        self.files.browser.path.push((entry.name, path));
                        self.files.browser.entries.clear();
                        self.list_browser()
                    }
                    // Folders of Recent / Shared with me live in other drives.
                    _ => Task::done(Msg::OpenUrl(entry.web_url)),
                }
            }
            FileAction::SearchFiles { query } => {
                let query = query.trim().to_owned();
                if query == self.files.browser.query {
                    return Task::none();
                }
                self.files.browser.query = query;
                self.files.browser.entries.clear();
                self.list_browser()
            }
            FileAction::Crumb { index } => {
                let keep = index as usize + 1;
                if keep < self.files.browser.path.len() {
                    self.files.browser.path.truncate(keep);
                    self.files.browser.entries.clear();
                    return self.list_browser();
                }
                Task::none()
            }
            FileAction::NewFolder { name } => {
                let (Some(client), Some(site), Some((_, parent))) = (
                    self.files_client(),
                    self.files.browser.site.clone(),
                    self.files.browser.path.last().cloned(),
                ) else {
                    return Task::none();
                };
                self.browser_change(
                    async move { client.create_folder(&site, &parent, &name).await },
                )
            }
            FileAction::Rename { key, name } => {
                let (Some(client), Some(site), Some(entry)) = (
                    self.files_client(),
                    self.files.browser.site.clone(),
                    self.browser_entry(&key).cloned(),
                ) else {
                    return Task::none();
                };
                self.browser_change(async move { client.rename(&site, &entry, &name).await })
            }
            FileAction::Delete { key } => {
                let (Some(client), Some(site), Some(entry)) = (
                    self.files_client(),
                    self.files.browser.site.clone(),
                    self.browser_entry(&key).cloned(),
                ) else {
                    return Task::none();
                };
                self.browser_change(async move { client.delete(&site, &entry).await })
            }
            FileAction::SetColor { key, color } => {
                let (Some(client), Some(site), Some(entry)) = (
                    self.files_client(),
                    self.files.browser.site.clone(),
                    self.browser_entry(&key).cloned(),
                ) else {
                    return Task::none();
                };
                self.browser_change(async move { client.set_color(&site, &entry, color).await })
            }
            FileAction::UploadHere { paths } => {
                let (Some(client), Some(site), Some((_, folder))) = (
                    self.files_client(),
                    self.files.browser.site.clone(),
                    self.files.browser.path.last().cloned(),
                ) else {
                    return Task::none();
                };
                let tasks: Vec<_> = paths
                    .into_iter()
                    .map(PathBuf::from)
                    .filter(|p| p.is_file())
                    .map(|path| {
                        let client = client.clone();
                        let destination = Destination {
                            site: site.clone(),
                            folder: folder.clone(),
                            library_relative: None,
                        };
                        self.browser_change(async move {
                            let name = local_name(&path);
                            client
                                .upload(
                                    &destination,
                                    &path,
                                    &name,
                                    &|_, _| {},
                                    &AtomicBool::new(false),
                                )
                                .await
                                .map(drop)
                        })
                    })
                    .collect();
                Task::batch(tasks)
            }
            FileAction::RefreshBrowser => self.list_browser(),
        }
    }

    fn browser_entry(&self, key: &str) -> Option<&DriveEntry> {
        self.files.browser.entries.iter().find(|e| entry_key(e) == key)
    }

    fn browser_change(
        &mut self,
        work: impl std::future::Future<Output = Result<(), teams_files::Error>> + Send + 'static,
    ) -> Task<Msg> {
        self.files.browser.busy += 1;
        self.files.browser.error = None;
        let g = self.generation;
        let request = self.files.browser.request;
        Task::perform(
            async move { work.await.map_err(|e| e.to_string()) },
            move |r| Msg::Files(FilesMsg::Changed(g, request, r)),
        )
    }

    // ---- OneDrive site ---------------------------------------------------

    /// Looks the OneDrive site up once; the chat folder is the localized one that exists, French otherwise.
    fn ensure_site(&mut self) -> Task<Msg> {
        if self.files.personal.is_some() || self.files.site_loading {
            return Task::none();
        }
        let (Some(files), Some(chat)) = (self.files_client(), self.client.clone()) else {
            return Task::none();
        };
        self.files.site_loading = true;
        let g = self.generation;
        Task::perform(
            async move {
                let site = match files.personal_site().await {
                    Ok(site) => site,
                    Err(e) => chat
                        .my_properties()
                        .await
                        .ok()
                        .and_then(|v| v.get("personalFileSite").and_then(PersonalSite::parse))
                        .ok_or_else(|| tf!("core.files.onedrive_not_found", error = e))?,
                };
                let french = teams_files::chat_folder_name("fr");
                let english = teams_files::chat_folder_name("en");
                let folder = match files
                    .list_folder(&site.site_url, &site.library_path())
                    .await
                {
                    Ok(list)
                        if list.entries.iter().any(|e| e.folder && e.name == english)
                            && !list.entries.iter().any(|e| e.folder && e.name == french) =>
                    {
                        english
                    }
                    _ => french,
                };
                Ok((site, folder.to_owned()))
            },
            move |r| Msg::Files(FilesMsg::Site(g, r)),
        )
    }

    // ---- Attachments -----------------------------------------------------

    fn start_upload(&mut self, id: &str) -> Task<Msg> {
        if self
            .pending_mut(id)
            .is_some_and(|p| matches!(p.source, Upload::Image { .. }))
        {
            return self.start_image_upload(id);
        }
        let Some(client) = self.files_client() else {
            return Task::none();
        };
        let personal = self.files.personal.clone();
        let channel_target = {
            let Some(p) = self
                .files
                .attachments
                .values()
                .flatten()
                .find(|p| p.id == id)
            else {
                return Task::none();
            };
            match &p.selection {
                Selection::Channel { team, channel } => Some(self.channel_folder(team, channel)),
                Selection::Chat(_) => None,
            }
        };
        let destination = match channel_target {
            Some(Ok((site, folder))) => Destination {
                site,
                folder,
                library_relative: None,
            },
            Some(Err(e)) => {
                if let Some(p) = self.pending_mut(id) {
                    p.state = PendingState::Failed(e);
                }
                return Task::none();
            }
            None => match personal {
                Some((site, folder)) => Destination {
                    folder: site.chat_folder(&folder),
                    library_relative: Some(folder),
                    site: site.site_url,
                },
                None => {
                    if let Some(error) = self.files.site_error.clone() {
                        if let Some(p) = self.pending_mut(id) {
                            p.state = PendingState::Failed(error);
                        }
                        // A later attempt may succeed: look the site up again.
                        self.files.site_error = None;
                        return Task::none();
                    }
                    return self.ensure_site();
                }
            },
        };
        let Some(p) = self.pending_mut(id) else {
            return Task::none();
        };
        let Upload::File(path) = &p.source else {
            return Task::none();
        };
        let path = path.clone();
        p.state = PendingState::Uploading(0);
        let (name, cancel, id) = (
            p.name.clone(),
            p.cancel.clone(),
            p.id.clone(),
        );
        let g = self.generation;
        Task::subscribe(move |sink| async move {
            let last = std::sync::atomic::AtomicU8::new(0);
            let progress_sink = sink.clone();
            let progress_id = id.clone();
            let progress = move |done: u64, total: u64| {
                let p = percent(done, total);
                if last.swap(p, Ordering::Relaxed) != p {
                    progress_sink.send(Msg::Files(FilesMsg::Progress(progress_id.clone(), p)));
                }
            };
            let result = client
                .upload(&destination, &path, &name, &progress, &cancel)
                .await
                .map(Uploaded::File)
                .map_err(|e| e.to_string());
            sink.send(Msg::Files(FilesMsg::Uploaded(g, id, result)));
        })
    }

    fn start_image_upload(&mut self, id: &str) -> Task<Msg> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let Some(p) = self.pending_mut(id) else {
            return Task::none();
        };
        let Upload::Image {
            png, width, height, ..
        } = &p.source
        else {
            return Task::none();
        };
        let (png, width, height) = (png.clone(), *width, *height);
        p.state = PendingState::Uploading(0);
        let (name, id) = (p.name.clone(), p.id.clone());
        let conversation = p.selection.id().to_owned();
        let g = self.generation;
        Task::perform(
            async move {
                let url = client
                    .upload_image(&conversation, &name, png.to_vec())
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Uploaded::Image(InlineImage { url, width, height }))
            },
            move |result| Msg::Files(FilesMsg::Uploaded(g, id, result)),
        )
    }

    /// Site and folder of a channel's files, from the team and channel the CSA returned.
    fn channel_folder(&self, team: &str, channel: &str) -> Result<(Url, String), String> {
        let team = self.teams.iter().find(|t| t.id == team);
        let ch = team.and_then(|t| t.channels.iter().find(|c| c.id == channel));
        let site = ch
            .and_then(|c| c.sharepoint_site_url())
            .or_else(|| team.and_then(|t| t.sharepoint_site_url()));
        let folder = ch.and_then(|c| c.files_relative_path());
        match (site, folder) {
            (Some(site), Some(folder)) => teams_files::site_root(site)
                .map(|s| (s, folder.to_owned()))
                .map_err(|e| e.to_string()),
            _ => Err(t("core.files.this_channel_s_sharepoint_folder_is").into()),
        }
    }

    /// Pasted images deleted from the text are left out, but kept until sending in case of an undo.
    pub(crate) fn pending_views(&self) -> Vec<PendingFileView> {
        let placed = self.draft_image_ids();
        self.files
            .attachments
            .get(&self.draft_key())
            .into_iter()
            .flatten()
            .filter(|p| !matches!(p.source, Upload::Image { .. }) || placed.contains(&p.id))
            .map(|p| {
                let (progress, ready, error) = match &p.state {
                    PendingState::Waiting => (0, false, None),
                    PendingState::Uploading(n) => (*n, false, None),
                    PendingState::Ready(_) => (100, true, None),
                    PendingState::Failed(e) => (0, false, Some(e.clone())),
                };
                PendingFileView {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    ext: ext(&p.name),
                    progress,
                    ready,
                    error,
                    inline: matches!(p.source, Upload::Image { .. }),
                }
            })
            .collect()
    }

    /// Attachments of the current draft that are ready, or why sending must wait.
    pub(crate) fn ready_attachments(&self) -> Result<Vec<(String, Uploaded)>, String> {
        let mut ready = vec![];
        for p in self
            .files
            .attachments
            .get(&self.draft_key())
            .into_iter()
            .flatten()
        {
            match &p.state {
                PendingState::Ready(file) => ready.push((p.id.clone(), file.clone())),
                PendingState::Failed(_) => {
                    return Err(t("core.files.remove_the_failed_files_before_sending").into());
                }
                _ => {
                    return Err(t("core.files.please_wait_files_are_still_uploading").into());
                }
            }
        }
        Ok(ready)
    }
    /// Forgets the pasted images no longer in the draft's text: they do not go out with it.
    pub(crate) fn drop_unplaced_images(&mut self) {
        let placed = self.draft_image_ids();
        let key = self.draft_key();
        if let Some(list) = self.files.attachments.get_mut(&key) {
            list.retain(|p| !matches!(p.source, Upload::Image { .. }) || placed.contains(&p.id));
        }
    }
    /// A pasted image of the draft, once on AMS.
    pub(crate) fn inline_image(&self, id: &str) -> Option<&InlineImage> {
        self.files
            .attachments
            .get(&self.draft_key())?
            .iter()
            .find_map(|p| match &p.state {
                PendingState::Ready(Uploaded::Image(image)) if p.id == id => Some(image),
                _ => None,
            })
    }
    pub(crate) fn has_attachments(&self) -> bool {
        self.files
            .attachments
            .get(&self.draft_key())
            .is_some_and(|l| !l.is_empty())
    }
    /// Forget attachments that went out with a message.
    pub(crate) fn sent_attachments(&mut self, ids: &[String]) {
        for list in self.files.attachments.values_mut() {
            list.retain(|p| !ids.contains(&p.id));
        }
        self.files.attachments.retain(|_, l| !l.is_empty());
    }

    /// Turns uploads into `properties.files`, inviting chat members first (`shareFiles`); channel files keep its permissions.
    pub(crate) fn share_task(
        &mut self,
        selection: &Selection,
        files: Vec<UploadedFile>,
    ) -> Option<impl std::future::Future<Output = Result<String, String>> + Send + 'static> {
        if files.is_empty() {
            return None;
        }
        let client = self.files_client()?;
        let recipients: Vec<String> = match selection {
            Selection::Chat(id) => self
                .chat_members(id)
                .map(|members| {
                    members
                        .iter()
                        .filter(|m| !m.id.eq_ignore_ascii_case(&self.me))
                        .filter_map(|m| m.id.strip_prefix("8:orgid:").map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            Selection::Channel { .. } => vec![],
        };
        let service = if selection.is_channel() {
            ServiceName::Teams
        } else {
            ServiceName::P2p
        };
        Some(async move {
            let mut references = vec![];
            for file in files {
                let shared = if service == ServiceName::P2p && !recipients.is_empty() {
                    let site = teams_files::site_root(&file.site_url).map_err(|e| e.to_string())?;
                    client
                        .invite(&site, Some(&file.file_id), &file.object_url, &recipients)
                        .await
                        .map_err(|e| {
                            tf!("core.files.could_not_share", name = file.name, error = e)
                        })?
                } else {
                    Shared::default()
                };
                references.push(FileReference {
                    file,
                    shared,
                    service,
                });
            }
            Ok(teams_files::files_property(&references))
        })
    }

    // ---- Downloads -------------------------------------------------------

    pub(crate) fn transfer_progress(&self, key: &str) -> Option<u8> {
        self.files.transfers.get(key).map(|(p, _)| *p)
    }

    fn download(&mut self, key: String, dest: PathBuf, preview: bool) -> Task<Msg> {
        if self.files.transfers.contains_key(&key) {
            return Task::none();
        }
        let (Some(client), Some(source)) = (self.files_client(), self.source(&key)) else {
            return Task::none();
        };
        if source.content.is_none() && source.link.is_none() {
            *self.error = Some(t("core.files.this_file_cannot_be_downloaded").into());
            return Task::none();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.files
            .transfers
            .insert(key.clone(), (0, cancel.clone()));
        let g = self.generation;
        Task::subscribe(move |sink| async move {
            let last = std::sync::atomic::AtomicU8::new(0);
            let progress_sink = sink.clone();
            let progress_key = key.clone();
            let progress = move |done: u64, total: Option<u64>| {
                let p = total.map(|t| percent(done, t)).unwrap_or(0);
                if last.swap(p, Ordering::Relaxed) != p {
                    progress_sink.send(Msg::Files(FilesMsg::Transfer(progress_key.clone(), p)));
                }
            };
            let result = async {
                if let Some(parent) = dest.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| teams_files::Error::Io(e.to_string()))?;
                }
                if let Some((site, path)) = &source.content {
                    client
                        .download_content(site, path, &dest, &progress, &cancel)
                        .await?;
                } else if let Some(link) = &source.link {
                    let fallback = source.item.as_ref().map(|(s, id)| (s, id.as_str()));
                    let info = client.redeem(link, fallback).await?;
                    let url = info.download_url.ok_or_else(|| {
                        teams_files::Error::Invalid(t("core.files.no_download_link").into())
                    })?;
                    client.download(&url, &dest, &progress, &cancel).await?;
                }
                Ok::<_, teams_files::Error>(dest)
            }
            .await
            .map_err(|e| e.to_string());
            sink.send(Msg::Files(FilesMsg::Downloaded(g, key, preview, result)));
        })
    }

    fn attachments_loaded(&self) -> impl Iterator<Item = (Attachment, Option<&Message>)> + '_ {
        self.conv
            .messages()
            .iter()
            .chain(self.conv.posts().iter().flat_map(|p| p.messages.iter()))
            .chain(self.conv.replies().iter())
            .flat_map(move |m| {
                self.conv
                    .rendered()
                    .get(m)
                    .files
                    .clone()
                    .into_iter()
                    .map(move |f| (f, Some(m)))
            })
    }

    /// Resolves a key shown in any view.
    pub(crate) fn source(&self, key: &str) -> Option<Source> {
        if key.starts_with("att:") {
            let (file, _) = self.attachments_loaded().find(|(f, _)| f.key() == key)?;
            if file.deleted {
                return None;
            }
            let site = file
                .site_url
                .as_deref()
                .and_then(|s| teams_files::site_root(s).ok());
            let item = site.zip((!file.id.is_empty()).then(|| file.id.clone()));
            return Some(Source {
                web: file.url.clone().or_else(|| file.link().map(str::to_owned)),
                link: file.link().map(str::to_owned),
                item,
                content: None,
                name: file.name,
            });
        }
        if key.starts_with("shr:") {
            let f = self
                .files
                .shared_files
                .iter()
                .flatten()
                .find(|f| shared_key(f) == key)?;
            let link = shared_link(f);
            let site = teams_files::site_root(&f.site_url).ok();
            return Some(Source {
                name: f.name.clone(),
                web: link.clone(),
                link,
                item: site.zip((!f.id.is_empty()).then(|| f.id.clone())),
                content: None,
            });
        }
        if let Some(url) = key.strip_prefix("lnk:") {
            return Some(Source {
                name: url.to_owned(),
                web: Some(url.to_owned()),
                link: Some(url.to_owned()),
                item: None,
                content: None,
            });
        }
        if key.starts_with("drv:") {
            let e = self.browser_entry(key)?;
            let content = e
                .server_relative_url
                .clone()
                .filter(|_| !e.folder)
                .and(self.files.browser.site.clone())
                .zip(e.server_relative_url.clone());
            return Some(Source {
                name: e.name.clone(),
                web: (!e.web_url.is_empty()).then(|| e.web_url.clone()),
                link: (!e.web_url.is_empty() && content.is_none()).then(|| e.web_url.clone()),
                item: None,
                content,
            });
        }
        None
    }

    // ---- "Partagé" tab of a chat ------------------------------------------

    pub(crate) fn load_shared(&mut self, force: bool) -> Task<Msg> {
        let Some(Selection::Chat(thread)) = self.selected.clone() else {
            return Task::none();
        };
        if !force
            && self.files.shared_thread.as_deref() == Some(&thread)
            && (self.files.shared_pending > 0 || self.files.shared_full)
        {
            return Task::none();
        }
        let Some(client) = self.files_client() else {
            return Task::none();
        };
        if self.files.shared_thread.as_deref() != Some(&thread) {
            self.files.shared_files = None;
            self.files.shared_links.clear();
            self.files.shared_query.clear();
        }
        self.files.shared_thread = Some(thread.clone());
        self.files.shared_full = false;
        self.files.shared_error = None;
        self.files.shared_request += 1;
        self.files.shared_pending = 3;
        let (g, request) = (self.generation, self.files.shared_request);
        let task = |fast: bool| {
            let (client, thread) = (client.clone(), thread.clone());
            Task::perform(
                async move {
                    client
                        .shared_in_chat(&thread, fast)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |r| Msg::Files(FilesMsg::Shared(g, request, fast, r)),
            )
        };
        let links = {
            let (client, thread) = (client.clone(), thread.clone());
            Task::perform(
                async move {
                    client
                        .shared_links(&thread)
                        .await
                        .map_err(|e| e.to_string())
                },
                move |r| Msg::Files(FilesMsg::Links(g, request, r)),
            )
        };
        Task::batch([task(true), task(false), links])
    }

    /// The open chat's files and links: the Substrate index merged with loaded attachments it may lack yet.
    pub(crate) fn shared_items(&self) -> (Vec<super::view::FileView>, bool, Option<String>) {
        use super::view::FileView;
        let thread = match &*self.selected {
            Some(Selection::Chat(id)) => Some(id.as_str()),
            _ => None,
        };
        let indexed = self.files.shared_thread.as_deref() == thread && thread.is_some();
        let mut items: Vec<(i64, FileView)> = vec![];
        let mut seen = HashSet::new();
        let filter = self.files.filter;
        let time = |iso: &str| {
            DateTime::parse_from_rfc3339(iso)
                .map(|t| t.timestamp_millis())
                .unwrap_or(0)
        };
        if filter != SharedFilter::Links {
            if indexed {
                for f in self.files.shared_files.iter().flatten() {
                    let dedup = f.object_url.to_lowercase();
                    if !dedup.is_empty() && !seen.insert(dedup) {
                        continue;
                    }
                    let key = shared_key(f);
                    items.push((
                        time(&f.shared_at),
                        FileView {
                            ext: ext(&f.name),
                            name: f.name.clone(),
                            url: shared_link(f),
                            deleted: false,
                            description: if f.sender_name.is_empty() {
                                t("core.files.shared_file").into()
                            } else {
                                tf!("core.files.shared_by", sender_name = f.sender_name)
                            },
                            progress: self.transfer_progress(&key),
                            can_download: true,
                            thumbnail: None,
                            date: date_short(&f.shared_at),
                            is_link: false,
                            key,
                        },
                    ));
                }
            }
            for (file, message) in self.attachments_loaded() {
                if file.deleted {
                    continue;
                }
                let dedup = file
                    .object_url
                    .clone()
                    .or(file.url.clone())
                    .unwrap_or(file.name.clone())
                    .to_lowercase();
                if !seen.insert(dedup) {
                    continue;
                }
                let at = message.and_then(conversations::message_time).unwrap_or(0);
                let mut view = self.file_view(&file);
                if let Some(m) = message {
                    view.description = tf!("core.files.shared_by_2", name = self.sender_name(m));
                    view.date = DateTime::from_timestamp_millis(at)
                        .map(|t| date_short(&t.to_rfc3339()))
                        .unwrap_or_default();
                }
                items.push((at, view));
            }
        }
        if filter != SharedFilter::Files && indexed {
            for l in &self.files.shared_links {
                if !seen.insert(l.url.to_lowercase()) {
                    continue;
                }
                items.push((
                    time(&l.modified),
                    FileView {
                        key: format!("lnk:{}", l.url),
                        name: l.title.clone(),
                        url: Some(l.url.clone()),
                        deleted: false,
                        ext: "URL".into(),
                        description: if l.sender_name.is_empty() {
                            l.url.clone()
                        } else {
                            tf!("core.files.shared_by", sender_name = l.sender_name)
                        },
                        progress: None,
                        can_download: false,
                        thumbnail: None,
                        date: date_short(&l.modified),
                        is_link: true,
                    },
                ));
            }
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.0));
        let loading = indexed && self.files.shared_pending > 0;
        let error = if indexed {
            self.files.shared_error.clone()
        } else {
            None
        };
        (
            items
                .into_iter()
                .map(|(_, v)| v)
                .filter(|v| super::view::matches_search(&v.name, &self.search))
                .filter(|v| {
                    let q = &self.files.shared_query;
                    [Some(&v.name), Some(&v.description), v.url.as_ref()]
                        .into_iter()
                        .flatten()
                        .any(|s| super::view::matches_search(s, q))
                })
                .collect(),
            loading,
            error,
        )
    }

    // ---- Folder browser --------------------------------------------------

    pub(crate) fn browse(&mut self, place: BrowserPlace) -> Task<Msg> {
        let b = &mut self.files.browser;
        if b.view_place != place || b.channel.is_some() {
            b.entries.clear();
            b.path.clear();
        }
        b.view_place = place;
        b.channel = None;
        b.error = None;
        b.query.clear();
        let Some((site, _)) = self.files.personal.clone() else {
            self.files.browser.loading = true;
            self.files.browser.request += 1;
            return self.ensure_site();
        };
        self.files.browser.site = Some(site.site_url.clone());
        if place == BrowserPlace::MyFiles && self.files.browser.path.is_empty() {
            self.files.browser.path = vec![(my_files().into(), site.library_path())];
        }
        self.list_browser()
    }

    pub(crate) fn open_channel_browser(&mut self) -> Task<Msg> {
        let Some(Selection::Channel { team, channel }) = self.selected.clone() else {
            return Task::none();
        };
        let same = self.files.browser.channel.as_ref() == Some(&(team.clone(), channel.clone()));
        let b = &mut self.files.browser;
        b.view_place = BrowserPlace::Channel;
        if same && !b.entries.is_empty() {
            return Task::none();
        }
        b.channel = Some((team.clone(), channel.clone()));
        b.entries.clear();
        b.path.clear();
        b.error = None;
        b.query.clear();
        match self.channel_folder(&team, &channel) {
            Ok((site, folder)) => {
                let name = self
                    .teams
                    .iter()
                    .flat_map(|t| &t.channels)
                    .find(|c| c.id == channel)
                    .map(|c| c.display_name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| t("core.files.channel").into());
                self.files.browser.site = Some(site);
                self.files.browser.path = vec![(name, folder)];
                self.list_browser()
            }
            Err(e) => {
                self.files.browser.site = None;
                self.files.browser.error = Some(e);
                Task::none()
            }
        }
    }

    fn list_browser(&mut self) -> Task<Msg> {
        let (Some(client), Some(site)) = (self.files_client(), self.files.browser.site.clone())
        else {
            return Task::none();
        };
        let b = &mut self.files.browser;
        b.request += 1;
        b.loading = true;
        let (g, request) = (self.generation, b.request);
        let place = b.view_place;
        let folder = b.path.last().map(|(_, p)| p.clone());
        let query = b.query.clone();
        // A channel search stays inside its folder; the site's library holds every channel.
        let within = (place == BrowserPlace::Channel)
            .then(|| {
                b.path
                    .first()
                    .map(|(_, p)| format!("{}/", p.trim_end_matches('/')))
            })
            .flatten();
        Task::perform(
            async move {
                if !query.is_empty() {
                    return client
                        .search(&site, &query)
                        .await
                        .map(|l| {
                            l.entries
                                .into_iter()
                                .filter(|e| match &within {
                                    Some(root) => e
                                        .server_relative_url
                                        .as_deref()
                                        .is_some_and(|p| p.starts_with(root.as_str())),
                                    None => true,
                                })
                                .collect()
                        })
                        .map_err(|e| e.to_string());
                }
                match (place, folder) {
                    (BrowserPlace::Recent, _) => client.recent(&site).await,
                    (BrowserPlace::SharedWithMe, _) => client.shared_with_me(&site).await,
                    (_, Some(folder)) => client.list_folder(&site, &folder).await,
                    (_, None) => Ok(Default::default()),
                }
                .map(|l| l.entries)
                .map_err(|e| e.to_string())
            },
            move |r| Msg::Files(FilesMsg::Listed(g, request, r)),
        )
    }

    /// Renames the OneDrive root crumb in the current language.
    pub(crate) fn relocalize_files(&mut self) {
        if let Some((name, _)) = self.files.browser.path.first_mut()
            && ["Mes fichiers", "My files"].contains(&name.as_str())
        {
            *name = my_files().into();
        }
    }

    pub(crate) fn browser_view(&self) -> BrowserView {
        let b = &self.files.browser;
        let title = match b.view_place {
            BrowserPlace::Recent => t("core.files.recent").to_owned(),
            BrowserPlace::MyFiles => my_files().to_owned(),
            BrowserPlace::SharedWithMe => t("core.files.shared_with_me").to_owned(),
            BrowserPlace::Channel => b.path.first().map(|(n, _)| n.clone()).unwrap_or_default(),
        };
        let searching = !b.query.is_empty();
        let listing =
            !searching && matches!(b.view_place, BrowserPlace::MyFiles | BrowserPlace::Channel);
        let title = if searching {
            tf!("core.files.results_for", query = b.query)
        } else {
            title
        };
        BrowserView {
            place: b.view_place,
            query: b.query.clone(),
            title,
            crumbs: if listing {
                b.path.iter().map(|(n, _)| n.clone()).collect()
            } else {
                vec![]
            },
            entries: b
                .entries
                .iter()
                // Search results are shown whole: the top bar's filter does not apply.
                .filter(|e| searching || super::view::matches_search(&e.name, &self.search))
                .map(|e| {
                    let key = entry_key(e);
                    EntryView {
                        progress: self.transfer_progress(&key),
                        key,
                        name: e.name.clone(),
                        ext: if e.folder {
                            t("core.files.folder").into()
                        } else {
                            ext(&e.name)
                        },
                        folder: e.folder,
                        size: if e.folder {
                            e.child_count.map(items).unwrap_or_default()
                        } else {
                            size_label(e.size)
                        },
                        modified: date_short(&e.modified),
                        modified_by: e.modified_by.clone(),
                        can_open_folder: e.folder,
                        color: e.color,
                        location: if searching {
                            e.server_relative_url
                                .as_deref()
                                .and_then(|p| p.rsplit_once('/'))
                                .and_then(|(parent, _)| parent.rsplit('/').next())
                                .unwrap_or_default()
                                .to_owned()
                        } else {
                            String::new()
                        },
                    }
                })
                .collect(),
            loading: b.loading,
            busy: b.busy > 0,
            can_write: listing && b.site.is_some() && !b.path.is_empty(),
            error: b.error.clone(),
        }
    }
}

fn recycle(client: Files, file: UploadedFile) -> Task<Msg> {
    Task::perform(
        async move {
            if let Ok(site) = teams_files::site_root(&file.site_url) {
                let _ = client.recycle(&site, &file.file_id).await;
            }
        },
        // No real generation matches: the result is dropped.
        |_| Msg::Files(FilesMsg::Changed(u64::MAX, 0, Ok(()))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_sizes_and_percentages() {
        assert_eq!(size_label(Some(512)), "512 o");
        assert_eq!(size_label(Some(2048)), "2 Ko");
        assert_eq!(size_label(Some(5 * 1048576)), "5.0 Mo");
        assert_eq!(percent(5, 10), 50);
        assert_eq!(percent(1, 0), 0);
        assert_eq!(ext("rapport.final.PDF"), "PDF");
        assert_eq!(ext("archive"), "FIC");
    }

    use crate::task::{Job, Sink};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    /// Drives the app like the core thread does, against the demo services.
    struct Harness {
        app: App,
        tx: UnboundedSender<Msg>,
        rx: UnboundedReceiver<Msg>,
        effects: Vec<Effect>,
    }
    impl Harness {
        fn run(&mut self, task: Task<Msg>) {
            for job in task.0 {
                match job {
                    Job::Effect(effect) => self.effects.push(effect),
                    Job::Future(future) => {
                        let tx = self.tx.clone();
                        tokio::spawn(async move {
                            let _ = tx.send(future.await);
                        });
                    }
                    Job::Subscribe(start) => {
                        let tx = self.tx.clone();
                        tokio::spawn(start(Sink::new(move |msg| {
                            let _ = tx.send(msg);
                        })));
                    }
                    Job::Done(msg) => {
                        let _ = self.tx.send(msg);
                    }
                }
            }
        }
        fn send(&mut self, msg: Msg) {
            let task = self.app.update(msg);
            self.run(task);
        }
        fn file(&mut self, action: FileAction) {
            self.send(Msg::Files(FilesMsg::Action(action)));
        }
        async fn until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while !done(&self.app) {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                let msg = tokio::time::timeout(left, self.rx.recv())
                    .await
                    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
                    .expect("channel open");
                let task = self.app.update(msg);
                self.run(task);
            }
        }
    }
    fn local_file(name: &str, text: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "teams-core-files-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn channel_files_tab_comes_back_after_the_files_screen() {
        let (app, task) = App::boot(true);
        let (tx, rx) = unbounded_channel();
        let mut h = Harness {
            app,
            tx,
            rx,
            effects: vec![],
        };
        h.run(task);
        h.until("workspace", |a| !a.teams.is_empty() && !a.chats.is_empty())
            .await;
        let channel = || Msg::Select(Selection::Channel {
            team: "19:product-team@thread.tacv2".into(),
            channel: "19:general@thread.tacv2".into(),
        });
        h.send(Msg::Navigate(Nav::Teams));
        h.send(channel());
        h.send(Msg::SetTab(Tab::Files));
        h.until("channel", |a| !a.files.browser.loading).await;
        h.send(Msg::Navigate(Nav::Files));
        h.file(FileAction::Browse {
            place: BrowserPlace::MyFiles,
        });
        h.until("my files", |a| !a.files.browser.loading).await;
        // Back to Teams: the channel and its files tab are still selected.
        h.send(Msg::Navigate(Nav::Teams));
        h.until("channel again", |a| !a.files.browser.loading).await;
        let view = h.app.browser_view();
        assert_eq!(view.place, BrowserPlace::Channel);
        assert!(view.entries.iter().any(|e| e.name == "Plan produit.txt"));
    }

    #[tokio::test]
    async fn a_pasted_image_goes_where_it_is_in_the_text_and_a_deleted_one_stays_out() {
        let (app, task) = App::boot(true);
        let (tx, rx) = unbounded_channel();
        let mut h = Harness {
            app,
            tx,
            rx,
            effects: vec![],
        };
        h.run(task);
        h.until("workspace", |a| !a.chats.is_empty()).await;
        h.send(Msg::Select(Selection::Chat("19:projet-rust@thread.v2".into())));
        h.until("feed", |a| !a.conv.messages().is_empty()).await;
        for id in ["kept", "deleted"] {
            h.file(FileAction::PasteImage {
                id: id.into(),
                png: b"png bytes".to_vec(),
                width: 320,
                height: 200,
            });
        }
        let image = |start, id: &str| crate::ComposerFormat {
            start,
            length: 1,
            image: Some(id.into()),
            ..Default::default()
        };
        // An image format on anything but its placeholder is dropped.
        h.send(Msg::RichDraftChanged(
            "voici \u{FFFC} fin".into(),
            vec![image(6, "kept"), image(8, "deleted")],
        ));
        h.until("upload", |a| {
            a.pending_views()
                .iter()
                .all(|p| p.ready || p.error.is_some())
        })
        .await;
        let pending = h.app.pending_views();
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert!(pending[0].ready && pending[0].inline, "{pending:?}");
        h.send(Msg::Send);
        h.until("sent", |a| !a.conv.sending() && a.pending_views().is_empty())
            .await;
        assert_eq!(h.app.error, None);
        assert!(h.app.files.attachments.is_empty());
        h.until("feed with the image", |a| {
            a.conv.messages().iter().any(|m| {
                m.content.matches("http://schema.skype.com/AMSImage").count() == 1
                    && m.content.contains("<p>voici <img src=")
                    && m.content.contains("/v1/objects/demo-pasted/views/imgo\" width=\"320\"")
                    && m.content.contains("> fin</p>")
            })
        })
        .await;
    }
    #[tokio::test]
    async fn demo_attach_send_download_share_and_browse() {
        let (app, task) = App::boot(true);
        let (tx, rx) = unbounded_channel();
        let mut h = Harness {
            app,
            tx,
            rx,
            effects: vec![],
        };
        h.run(task);
        h.until("workspace", |a| !a.teams.is_empty() && !a.chats.is_empty())
            .await;

        // An attachment of a loaded message downloads through redeem + tempauth URL.
        let chat = Selection::Chat("19:projet-rust@thread.v2".into());
        h.send(Msg::Select(chat.clone()));
        h.until("feed", |a| !a.conv.messages().is_empty()).await;
        let attachment = h
            .app
            .attachments_loaded()
            .map(|(f, _)| f)
            .find(|f| f.name == "Feuille de route.txt")
            .expect("demo attachment");
        let dest = std::path::Path::new(&local_file("x", "")).with_file_name("copie.txt");
        h.file(FileAction::Download {
            key: attachment.key(),
            path: dest.to_string_lossy().into_owned(),
        });
        h.until("download", |a| a.files.transfers.is_empty()).await;
        assert_eq!(h.app.error, None);
        assert!(
            std::fs::read_to_string(&dest)
                .unwrap()
                .starts_with("Feuille de route")
        );
        assert!(matches!(h.effects.last(), Some(Effect::Downloaded { .. })));

        // Attach: site found, file uploaded, members invited, and the message carries the v2 reference.
        h.file(FileAction::Attach {
            paths: vec![local_file("compte-rendu.txt", "Compte rendu")],
        });
        h.until("upload", |a| {
            a.pending_views()
                .iter()
                .all(|p| p.ready || p.error.is_some())
        })
        .await;
        assert!(
            h.app.pending_views()[0].ready,
            "{:?}",
            h.app.pending_views()
        );
        h.send(Msg::Send);
        h.until("sent", |a| {
            !a.conv.sending() && a.pending_views().is_empty()
        })
        .await;
        assert_eq!(h.app.error, None);
        h.until("feed with the file", |a| {
            a.conv.messages().iter().any(|m| {
                m.properties
                    .get("files")
                    .and_then(|f| f.as_str())
                    .is_some_and(|f| {
                        f.contains("compte-rendu.txt") && f.contains("_layouts/15/share/")
                    })
            })
        })
        .await;

        // "Partagé" tab: the Substrate index and the link list.
        h.send(Msg::SetTab(Tab::Files));
        h.until("shared", |a| a.files.shared_pending == 0).await;
        let (items, _, error) = h.app.shared_items();
        assert_eq!(error, None);
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"compte-rendu.txt"), "{names:?}");
        assert!(names.contains(&"Apprendre Rust"), "{names:?}");
        assert_eq!(
            names
                .iter()
                .filter(|n| **n == "Feuille de route.txt")
                .count(),
            1
        );
        // The tab's own search matches names, senders and link addresses.
        h.file(FileAction::SharedSearch {
            query: " rust-lang.org/learn ".into(),
        });
        let (items, _, _) = h.app.shared_items();
        assert_eq!(
            items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            ["Apprendre Rust"]
        );
        assert_eq!(h.app.files_view().query, "rust-lang.org/learn");
        h.file(FileAction::SharedSearch {
            query: String::new(),
        });
        assert_eq!(h.app.shared_items().0.len(), items.len() + names.len() - 1);

        // Global Files: recent items, then my files with folder operations.
        h.send(Msg::Navigate(Nav::Files));
        h.until("recent", |a| !a.files.browser.loading).await;
        assert!(
            h.app
                .browser_view()
                .entries
                .iter()
                .any(|e| e.name == "compte-rendu.txt")
        );
        h.file(FileAction::Browse {
            place: BrowserPlace::MyFiles,
        });
        h.until("my files", |a| !a.files.browser.loading).await;
        let projets = h
            .app
            .browser_view()
            .entries
            .into_iter()
            .find(|e| e.name == "Projets")
            .unwrap();
        assert_eq!(projets.color, 5);
        h.file(FileAction::OpenFolder { key: projets.key });
        h.until("folder", |a| !a.files.browser.loading).await;
        assert_eq!(h.app.browser_view().crumbs, ["Mes fichiers", "Projets"]);
        h.file(FileAction::NewFolder {
            name: "Brouillons".into(),
        });
        h.until("new folder", |a| {
            a.files.browser.busy == 0
                && !a.files.browser.loading
                && a.files
                    .browser
                    .entries
                    .iter()
                    .any(|e| e.name == "Brouillons")
        })
        .await;
        let key = |a: &App, name: &str| {
            a.browser_view()
                .entries
                .into_iter()
                .find(|e| e.name == name)
                .map(|e| e.key)
        };
        let draft = key(&h.app, "Brouillons").unwrap();
        h.file(FileAction::Rename {
            key: draft,
            name: "Archives".into(),
        });
        h.until("rename", |a| {
            key(a, "Archives").is_some() && !a.files.browser.loading
        })
        .await;
        let archives = key(&h.app, "Archives").unwrap();
        h.file(FileAction::SetColor {
            key: archives.clone(),
            color: 9,
        });
        h.until("colour", |a| {
            a.browser_view().entries.iter().any(|e| e.name == "Archives" && e.color == 9)
                && !a.files.browser.loading
        })
        .await;
        h.file(FileAction::Delete { key: archives });
        h.until("delete", |a| {
            key(a, "Archives").is_none() && !a.files.browser.loading && a.files.browser.busy == 0
        })
        .await;
        assert_eq!(h.app.files.browser.error, None);

        // Search the whole OneDrive, then open a folder found.
        h.file(FileAction::SearchFiles {
            query: "spéc".into(),
        });
        h.until("search", |a| !a.files.browser.loading).await;
        let view = h.app.browser_view();
        assert!(view.crumbs.is_empty() && !view.can_write);
        let hit = view
            .entries
            .iter()
            .find(|e| e.name == "Spécifications.md")
            .expect("found");
        assert_eq!(hit.location, "Projets");
        h.file(FileAction::SearchFiles {
            query: "projets".into(),
        });
        h.until("search folder", |a| !a.files.browser.loading).await;
        let folder = key(&h.app, "Projets").unwrap();
        h.file(FileAction::OpenFolder { key: folder });
        h.until("folder from search", |a| !a.files.browser.loading)
            .await;
        assert_eq!(h.app.browser_view().crumbs, ["Mes fichiers", "Projets"]);
        assert_eq!(h.app.browser_view().query, "");

        // A channel's files tab lists its SharePoint folder, and uploads go there.
        h.send(Msg::Select(Selection::Channel {
            team: "19:product-team@thread.tacv2".into(),
            channel: "19:general@thread.tacv2".into(),
        }));
        h.send(Msg::SetTab(Tab::Files));
        h.until("channel folder", |a| !a.files.browser.loading)
            .await;
        assert_eq!(h.app.browser_view().place, BrowserPlace::Channel);
        assert!(
            h.app
                .browser_view()
                .entries
                .iter()
                .any(|e| e.name == "Plan produit.txt")
        );
        h.file(FileAction::UploadHere {
            paths: vec![local_file("budget.txt", "42")],
        });
        h.until("channel upload", |a| {
            a.files.browser.busy == 0
                && !a.files.browser.loading
                && a.files
                    .browser
                    .entries
                    .iter()
                    .any(|e| e.name == "budget.txt")
        })
        .await;
        // A channel search stays in its folder: the other channel's README is left out.
        h.file(FileAction::SearchFiles { query: "e".into() });
        h.until("channel search", |a| !a.files.browser.loading)
            .await;
        let names: Vec<_> = h
            .app
            .browser_view()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(
            names.contains(&"budget.txt".to_owned()) && !names.contains(&"README.md".to_owned()),
            "{names:?}"
        );
        h.file(FileAction::SearchFiles {
            query: String::new(),
        });
        h.until("channel listing", |a| !a.files.browser.loading)
            .await;
        let budget = key(&h.app, "budget.txt").unwrap();
        h.file(FileAction::Preview { key: budget });
        h.until("preview", |a| a.files.transfers.is_empty()).await;
        let Some(Effect::QuickLook { path }) = h.effects.last() else {
            panic!("no Quick Look: {:?} / {:?}", h.effects.last(), h.app.error);
        };
        assert_eq!(std::fs::read_to_string(path).unwrap(), "42");
    }
}
