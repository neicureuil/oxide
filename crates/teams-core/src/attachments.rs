//! Normalize the legacy and v2 file schemas used by Teams' toFiles mapper (524454).
use crate::cards;
use crate::html::{self, Block, InlineImage};
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use teams_chat::Message;
use teams_i18n::{t, tf};
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    /// Where the file opens in the browser.
    pub url: Option<String>,
    pub preview: Option<String>,
    pub deleted: bool,
    /// `id` (v2) / `itemid`: the list item GUID, or the driveItem id.
    pub id: String,
    /// `fileInfo.itemId`.
    pub item_id: Option<String>,
    /// `baseUrl` / `fileInfo.siteUrl`.
    pub site_url: Option<String>,
    /// `objectUrl` / `fileInfo.fileUrl`.
    pub object_url: Option<String>,
    /// `fileInfo.shareUrl`: the sharing link, the one to redeem first.
    pub share_url: Option<String>,
    /// Lower-case extension (`type` / `fileType`).
    pub file_type: String,
    /// SharePoint thumbnail, for files without an AMS preview.
    pub thumbnail: Option<String>,
}
impl Attachment {
    /// Stable identity in views and actions.
    pub fn key(&self) -> String {
        let id = if self.id.is_empty() {
            self.object_url
                .as_deref()
                .or(self.url.as_deref())
                .unwrap_or(&self.name)
        } else {
            &self.id
        };
        format!("att:{id}")
    }
    /// The link redeemed into a download URL: the sharing link, else the object URL.
    pub fn link(&self) -> Option<&str> {
        self.share_url.as_deref().or(self.object_url.as_deref())
    }
}
/// File types SharePoint renders a thumbnail for.
const THUMBNAILS: [&str; 20] = [
    "png", "jpg", "jpeg", "gif", "bmp", "heic", "webp", "tif", "tiff", "pdf", "doc", "docx", "xls",
    "xlsx", "ppt", "pptx", "mp4", "mov", "m4v", "vsdx",
];
pub(crate) fn sharepoint(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some_and(|h| {
                [
                    ".sharepoint.com",
                    ".sharepoint.us",
                    ".sharepoint.cn",
                    ".sharepoint.de",
                ]
                .iter()
                .any(|s| h.ends_with(s))
            })
    })
}
fn string(value: &Value, paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .filter_map(|path| value.pointer(path).and_then(Value::as_str))
        .find(|s| !s.trim().is_empty())
        .map(str::to_owned)
}
fn json(value: &Value) -> Value {
    let mut v = value.clone();
    for _ in 0..2 {
        if let Some(s) = v.as_str() {
            v = serde_json::from_str(s).unwrap_or(Value::Null);
        } else {
            break;
        }
    }
    v
}
pub fn files(message: &Message) -> Vec<Attachment> {
    let Some(raw) = raw_files(message) else {
        return vec![];
    };
    let value = json(raw);
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            let state = string(file, &["/state", "/fileChicletState/state"]).unwrap_or_default();
            if state == "hidden"
                || string(file, &["/fileType", "/type"]).as_deref() == Some("deeplink")
            {
                return None;
            }
            let name = string(file, &["/title", "/fileName", "/name"])
                .unwrap_or_else(|| t("core.attachments.shared_file").into());
            let url = string(
                file,
                &[
                    "/openUrl",
                    "/fileInfo/shareUrl",
                    "/objectUrl",
                    "/fileInfo/fileUrl",
                    "/botFileProperties/url",
                ],
            )
            .and_then(|u| html::safe_link(&u));
            let preview = string(file, &["/filePreview/previewUrl", "/previewUrl"]);
            let web = |paths: &[&str]| string(file, paths).and_then(|u| html::safe_link(&u));
            let share_url = web(&["/fileInfo/shareUrl"]);
            let object_url = web(&["/objectUrl", "/fileInfo/fileUrl"]);
            let file_type = string(file, &["/type", "/fileType"])
                .map(|t| t.to_ascii_lowercase())
                .unwrap_or_else(|| teams_files::extension(&name));
            let link = share_url.as_deref().or(object_url.as_deref());
            let thumbnail = link
                .filter(|l| {
                    preview.is_none()
                        && state != "deleted"
                        && THUMBNAILS.contains(&file_type.as_str())
                        && sharepoint(l)
                })
                .and_then(|l| teams_files::thumbnail_url(l, "c400x225"));
            Some(Attachment {
                url,
                preview,
                deleted: state == "deleted",
                id: string(file, &["/id", "/itemid"]).unwrap_or_default(),
                item_id: string(file, &["/fileInfo/itemId"]),
                site_url: web(&["/baseUrl", "/fileInfo/siteUrl"]),
                share_url,
                object_url,
                file_type,
                thumbnail,
                name,
            })
        })
        .collect()
}
/// A message's parsed body and attachments, built once instead of on every frame.
pub struct Rendered {
    /// Hash of what it was rendered from, rather than a second copy of the message.
    source: u64,
    pub blocks: Vec<Block>,
    pub runs: Vec<html::Run>,
    pub images: Vec<InlineImage>,
    /// Images drawn inside cards: loaded like previews, not listed after the body.
    pub card_images: Vec<InlineImage>,
    pub files: Vec<Attachment>,
    /// The preview card of the message's first link.
    pub link: Option<crate::app::link_preview::LinkPreview>,
}
fn raw_files(message: &Message) -> Option<&Value> {
    message
        .properties
        .get("files")
        .or_else(|| message.extra.get("files"))
}
/// What a rendering depends on: the body, its files and its cards.
fn source(message: &Message) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    message.content.hash(&mut hasher);
    hash_value(raw_files(message), &mut hasher);
    hash_value(message.properties.get("cards"), &mut hasher);
    hash_value(message.properties.get("links"), &mut hasher);
    hash_value(message.properties.get("urlpreviews"), &mut hasher);
    hasher.finish()
}
fn hash_value(value: Option<&Value>, hasher: &mut impl Hasher) {
    let Some(value) = value else {
        return 0u8.hash(hasher);
    };
    match value {
        Value::Null => 1u8.hash(hasher),
        Value::Bool(b) => (2u8, b).hash(hasher),
        Value::Number(n) => (3u8, n.to_string()).hash(hasher),
        Value::String(s) => (4u8, s).hash(hasher),
        Value::Array(items) => {
            (5u8, items.len()).hash(hasher);
            for item in items {
                hash_value(Some(item), hasher);
            }
        }
        Value::Object(map) => {
            (6u8, map.len()).hash(hasher);
            for (key, item) in map {
                key.hash(hasher);
                hash_value(Some(item), hasher);
            }
        }
    }
}
/// Keyed by message id and checked against the source, so edits re-render.
#[derive(Default)]
pub struct Cache(Mutex<HashMap<String, Arc<Rendered>>>);
impl Cache {
    pub fn get(&self, message: &Message) -> Arc<Rendered> {
        let mut map = self.0.lock().unwrap();
        let source = source(message);
        if let Some(r) = map.get(&message.id)
            && r.source == source
        {
            return r.clone();
        }
        let doc = body(message);
        let files = files(message);
        let card_images = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Card(card) => Some(card.images().cloned()),
                _ => None,
            })
            .flatten()
            .collect();
        let rendered = Arc::new(Rendered {
            source,
            blocks: doc.blocks,
            runs: doc.runs,
            images: with_previews(doc.images, &files),
            card_images,
            files,
            link: crate::app::link_preview::from_message(message),
        });
        map.insert(message.id.clone(), rendered.clone());
        rendered
    }
    pub fn clear(&self) {
        *self.0.lock().unwrap() = HashMap::new();
    }
    /// Forgets the renderings of messages no longer loaded.
    pub fn retain(&self, mut keep: impl FnMut(&str) -> bool) {
        self.0.lock().unwrap().retain(|id, _| keep(id));
    }
}
/// The body as blocks, by message type: HTML, plain text or a system event, plus cards.
fn body(message: &Message) -> html::Document {
    let mut doc = if message.messagetype == "Text" {
        html::plain(&message.content)
    } else if let Some(summary) = system_summary(message) {
        html::Document::new(
            vec![Block::Text {
                runs: vec![html::Run {
                    text: summary,
                    italic: true,
                    ..Default::default()
                }],
            }],
            vec![],
        )
    } else {
        html::parse(&message.content)
    };
    let cards = cards::from_message(message);
    if !cards.is_empty() {
        let mut blocks = std::mem::take(&mut doc.blocks);
        blocks.extend(cards.into_iter().map(Block::Card));
        doc = html::Document::new(blocks, std::mem::take(&mut doc.images));
    }
    doc
}
/// Conversation events carry XML, not a body: a short sentence instead.
pub(crate) fn system_summary(message: &Message) -> Option<String> {
    let kind = &message.messagetype;
    let event = kind
        .strip_prefix("ThreadActivity/")
        .or_else(|| kind.strip_prefix("Event/"))?;
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let start = message.content.find(&open)? + open.len();
        let end = message.content[start..].find(&format!("</{name}>"))?;
        Some(html::to_text(&message.content[start..start + end]))
    };
    Some(match event {
        "AddMember" | "MemberJoined" => t("core.attachments.participants_were_added").into(),
        "DeleteMember" | "MemberLeft" => {
            t("core.attachments.a_participant_left_the_conversation").into()
        }
        "TopicUpdate" => match tag("value").filter(|v| !v.is_empty()) {
            Some(topic) => tf!("core.attachments.conversation_renamed", topic = topic),
            None => t("core.attachments.conversation_renamed_2").into(),
        },
        "PictureUpdate" => t("core.attachments.conversation_picture_changed").into(),
        "HistoryDisclosedUpdate" => t("core.attachments.history_visibility_changed").into(),
        "Call" if message.content.contains("type=\"ended\"") => {
            t("core.attachments.call_ended").into()
        }
        "Call" => t("core.attachments.call_started").into(),
        _ => t("core.attachments.conversation_activity").into(),
    })
}
fn with_previews(mut images: Vec<InlineImage>, files: &[Attachment]) -> Vec<InlineImage> {
    for file in files.iter().filter(|f| !f.deleted).cloned() {
        if let Some(src) = file.preview
            && !images.iter().any(|i| i.src == src)
        {
            images.push(InlineImage {
                src,
                alt: file.name,
                ..Default::default()
            });
        }
    }
    images
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn renderings_are_reused_until_the_body_files_or_cards_change() {
        let cache = Cache::default();
        let mut m = Message {
            id: "m".into(),
            content: "<p>Bonjour</p>".into(),
            ..Default::default()
        };
        let first = cache.get(&m);
        assert!(Arc::ptr_eq(&first, &cache.get(&m.clone())));
        m.properties
            .insert("files".into(), json!([{"fileName": "a.pdf"}]));
        let with_file = cache.get(&m);
        assert!(!Arc::ptr_eq(&first, &with_file));
        assert_eq!(with_file.files.len(), 1);
        m.content = "<p>Modifié</p>".into();
        assert_eq!(cache.get(&m).runs[0].text, "Modifié");
        let kept = cache.get(&m);
        cache.retain(|id| id != "m");
        assert!(
            !Arc::ptr_eq(&kept, &cache.get(&m)),
            "forgotten, so rendered again"
        );
    }
    #[test]
    fn reads_legacy_and_v2_with_null_fields_and_retains_deleted_files() {
        let mut m = Message::default();
        m.properties.insert("files".into(),json!(json!([
        {"version":2,"id":"new","title":"Budget.xlsx","openUrl":null,"objectUrl":"https://tenant.sharepoint.com/budget","fileInfo":{"shareUrl":null},"state":"active"},
        {"itemid":"old","fileName":"Notes.docx","fileInfo":{"fileUrl":"https://tenant.sharepoint.com/notes"}},
        {"id":"deleted","title":"Ancien.pdf","state":"deleted"},
        {"id":"hidden","title":"Masqué","state":"hidden"}
    ]).to_string()));
        let f = files(&m);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].name, "Budget.xlsx");
        assert_eq!(
            f[0].url.as_deref(),
            Some("https://tenant.sharepoint.com/budget")
        );
        assert_eq!(f[1].name, "Notes.docx");
        assert!(f[2].deleted);
        // Identity and redeem link survive for downloads.
        assert_eq!(f[0].key(), "att:new");
        assert_eq!(f[0].link(), Some("https://tenant.sharepoint.com/budget"));
        assert_eq!(f[0].file_type, "xlsx");
        assert!(f[0].thumbnail.as_deref().is_some_and(|t| t.starts_with(
            "https://tenant.sharepoint.com/_api/v2.1/shares/u!"
        )));
        assert_eq!(f[1].key(), "att:old");
        assert!(f[2].thumbnail.is_none());
    }
    #[test]
    fn cache_reuses_parse_until_message_changes() {
        let cache = Cache::default();
        let mut m = Message {
            id: "1".into(),
            content: "<p>avant</p>".into(),
            ..Default::default()
        };
        let first = cache.get(&m);
        assert!(Arc::ptr_eq(&first, &cache.get(&m)));
        m.content = "<p>après</p><img src=\"https://x.test/a.png\">".into();
        let edited = cache.get(&m);
        assert_eq!(edited.runs[0].text, "après");
        assert_eq!(edited.images.len(), 1);
    }
}
