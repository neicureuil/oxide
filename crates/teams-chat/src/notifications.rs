//! Notification settings outside user/conversation properties: per-channel NSS settings and reply-chain follow (RCM).

use std::time::Duration;

use http::{HeaderMap, HeaderValue, Method, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use teams_auth::{Authenticator, Service};

use crate::client::ChatClient;
use crate::error::Error;
use crate::path::{enc, query};

/// Banner choice of the NSS channel model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NssChoice {
    BannerAndFeed,
    Feed,
    Off,
}

/// NSS settings of one channel, as posted by `setChannelNotificationSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelNotificationSettings {
    #[serde(rename = "allNewPosts")]
    pub all_new_posts: NssChoice,
    #[serde(rename = "includeReplies")]
    pub include_replies: bool,
    #[serde(rename = "channelMentions")]
    pub channel_mentions: NssChoice,
}

impl ChannelNotificationSettings {
    /// Defaults the web client derives from the global preset.
    pub fn for_preset(all_activity: bool) -> Self {
        if all_activity {
            Self {
                all_new_posts: NssChoice::BannerAndFeed,
                include_replies: true,
                channel_mentions: NssChoice::BannerAndFeed,
            }
        } else {
            Self::OFF
        }
    }

    /// Settings the service accepts: it rejects `includeReplies: true` with `allNewPosts: Off` (400 Bad Request).
    pub fn normalized(self) -> Self {
        Self {
            include_replies: self.include_replies && self.all_new_posts != NssChoice::Off,
            ..self
        }
    }

    /// "Turn off" of the activity context menu.
    pub const OFF: Self = Self {
        all_new_posts: NssChoice::Off,
        include_replies: false,
        channel_mentions: NssChoice::Off,
    };

    /// Fields the service returned, at the top level or under `settings` (unverified); missing ones are `None`.
    pub fn partial(value: &Value) -> PartialChannelSettings {
        let source = value
            .get("settings")
            .filter(|s| s.is_object())
            .unwrap_or(value);
        let choice = |key: &str| {
            source
                .get(key)
                .and_then(|v| serde_json::from_value::<NssChoice>(v.clone()).ok())
        };
        PartialChannelSettings {
            all_new_posts: choice("allNewPosts"),
            include_replies: source.get("includeReplies").and_then(|v| match v {
                Value::Bool(b) => Some(*b),
                Value::String(s) => s.parse().ok(),
                _ => None,
            }),
            channel_mentions: choice("channelMentions"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PartialChannelSettings {
    pub all_new_posts: Option<NssChoice>,
    pub include_replies: Option<bool>,
    pub channel_mentions: Option<NssChoice>,
}

impl PartialChannelSettings {
    pub fn or(self, defaults: ChannelNotificationSettings) -> ChannelNotificationSettings {
        ChannelNotificationSettings {
            all_new_posts: self.all_new_posts.unwrap_or(defaults.all_new_posts),
            include_replies: self.include_replies.unwrap_or(defaults.include_replies),
            channel_mentions: self.channel_mentions.unwrap_or(defaults.channel_mentions),
        }
    }
}

/// One entry of the batch read.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSettingsResult {
    pub channel_id: String,
    pub settings: Option<PartialChannelSettings>,
}

/// `followState` of reply-chain metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowState {
    None,
    ExplicitFollow,
    ImplicitFollow,
    ExplicitUnfollow,
}

impl FollowState {
    pub fn from_code(code: i64) -> Self {
        match code {
            1 => Self::ExplicitFollow,
            2 => Self::ImplicitFollow,
            3 => Self::ExplicitUnfollow,
            _ => Self::None,
        }
    }
}

/// `channelNotificationSettingsApiConfigs`: two GET retries one second apart, no POST retry.
const NSS_GET_RETRIES: u32 = 2;
const NSS_GET_RETRY_DELAY: Duration = Duration::from_secs(1);

fn channel_settings_path(team_id: &str, channel_id: &str, thread_tenant_id: Option<&str>) -> String {
    format!(
        "v1/me/notificationSettings/team/{}/channel/{}{}",
        enc(team_id),
        enc(channel_id),
        query(&[("threadTenantId", thread_tenant_id.map(str::to_owned))])
    )
}

fn nss_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    headers
}

impl<A: Authenticator> ChatClient<A> {
    async fn nss(
        &self,
        endpoint: &'static str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Error> {
        let response = self
            .service_request(Service::Uis, endpoint, method, path, body, nss_headers())
            .await?;
        let bytes = response.bytes().await?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|source| Error::Decode { endpoint, source })
    }

    /// `GET v1/me/notificationSettings/team/{teamId}/channel/{channelId}`.
    pub async fn channel_notification_settings(
        &self,
        team_id: &str,
        channel_id: &str,
        thread_tenant_id: Option<&str>,
    ) -> Result<PartialChannelSettings, Error> {
        let path = channel_settings_path(team_id, channel_id, thread_tenant_id);
        let mut attempt = 0;
        loop {
            match self
                .nss("getChannelNotificationSettings", Method::GET, &path, None)
                .await
            {
                Ok(value) => return Ok(ChannelNotificationSettings::partial(&value)),
                Err(error) if attempt < NSS_GET_RETRIES && retryable(&error) => {
                    attempt += 1;
                    tokio::time::sleep(NSS_GET_RETRY_DELAY).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// `POST` of the same URL with the whole settings object (not a nested string).
    pub async fn set_channel_notification_settings(
        &self,
        team_id: &str,
        channel_id: &str,
        thread_tenant_id: Option<&str>,
        settings: &ChannelNotificationSettings,
    ) -> Result<(), Error> {
        let path = channel_settings_path(team_id, channel_id, thread_tenant_id);
        self.nss(
            "setChannelNotificationSettings",
            Method::POST,
            &path,
            Some(serde_json::to_value(settings).expect("settings serialize")),
        )
        .await
        .map(drop)
    }

    /// `POST v1/me/notificationSettings/channels/batch`.
    pub async fn batch_channel_notification_settings(
        &self,
        channel_ids: &[String],
        thread_tenant_id: Option<&str>,
    ) -> Result<Vec<ChannelSettingsResult>, Error> {
        let mut body = json!({ "channelIds": channel_ids });
        if let Some(tid) = thread_tenant_id {
            body["threadTenantId"] = json!(tid);
        }
        let value = self
            .nss(
                "getBatchChannelNotificationSettings",
                Method::POST,
                "v1/me/notificationSettings/channels/batch",
                Some(body),
            )
            .await?;
        Ok(batch_results(&value))
    }

    /// `PUT …/rcmetadata/{root}` with `{"f":1}` (follow) or `{"f":3}` (explicit unfollow).
    pub async fn set_reply_chain_follow(
        &self,
        user_mri: &str,
        conv: &str,
        root_message_id: &str,
        follow: bool,
    ) -> Result<(), Error> {
        let path = format!(
            "v1/users/{}/conversations/{}/rcmetadata/{}",
            enc(user_mri),
            enc(conv),
            enc(root_message_id)
        );
        self.send_json(
            "rcmetadata",
            Method::PUT,
            path,
            &json!({ "f": if follow { 1 } else { 3 } }),
        )
        .await
        .map(drop)
    }

    /// `POST …/getrcmetadata` for the given root messages; returns their follow states.
    pub async fn reply_chain_follow_states(
        &self,
        user_mri: &str,
        conv: &str,
        root_message_ids: &[String],
    ) -> Result<Vec<(String, FollowState)>, Error> {
        let ids: Vec<Value> = root_message_ids
            .iter()
            .map(|id| id.parse::<u64>().map(Value::from).unwrap_or(json!(id)))
            .collect();
        let path = format!(
            "v1/users/{}/conversations/{}/getrcmetadata",
            enc(user_mri),
            enc(conv)
        );
        let value = self
            .send_json("getrcmetadata", Method::POST, path, &json!({ "rootMessageIds": ids }))
            .await?;
        Ok(follow_states(&value))
    }
}

fn retryable(error: &Error) -> bool {
    match error {
        Error::Http(_) => true,
        Error::Status { status, .. } => status.is_server_error(),
        _ => false,
    }
}

fn batch_results(value: &Value) -> Vec<ChannelSettingsResult> {
    let items = value
        .get("results")
        .or_else(|| value.get("value"))
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned())
        .unwrap_or_default();
    items
        .iter()
        .filter_map(|item| {
            let channel_id = item.get("channelId")?.as_str()?.to_owned();
            let ok = item.get("success").and_then(Value::as_bool) != Some(false);
            let settings = item
                .get("settings")
                .filter(|_| ok)
                .map(ChannelNotificationSettings::partial);
            Some(ChannelSettingsResult {
                channel_id,
                settings,
            })
        })
        .collect()
}

/// Unverified shape: a list of `{rootMessageId, f|followState}` or a map by root id, maybe wrapped.
fn follow_states(value: &Value) -> Vec<(String, FollowState)> {
    let value = ["rcMetadata", "metadata", "results", "value"]
        .iter()
        .find_map(|k| value.get(*k))
        .unwrap_or(value);
    let code = |m: &Value| {
        m.get("f")
            .or_else(|| m.get("followState"))
            .and_then(|f| f.as_i64().or_else(|| f.as_str()?.parse().ok()))
    };
    let id_text = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let id = item
                    .get("rootMessageId")
                    .or_else(|| item.get("id"))
                    .and_then(id_text)?;
                let meta = item.get("metadata").unwrap_or(item);
                Some((id, FollowState::from_code(code(meta)?)))
            })
            .collect(),
        Value::Object(map) => map
            .iter()
            .filter_map(|(id, meta)| Some((id.clone(), FollowState::from_code(code(meta)?))))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_settings_fall_back_to_defaults() {
        let partial = ChannelNotificationSettings::partial(
            &json!({"settings":{"allNewPosts":"Feed","includeReplies":"false"}}),
        );
        assert_eq!(
            partial.or(ChannelNotificationSettings::for_preset(true)),
            ChannelNotificationSettings {
                all_new_posts: NssChoice::Feed,
                include_replies: false,
                channel_mentions: NssChoice::BannerAndFeed,
            }
        );
    }

    #[test]
    fn replies_drop_when_new_posts_are_off() {
        let settings = ChannelNotificationSettings {
            all_new_posts: NssChoice::Off,
            include_replies: true,
            channel_mentions: NssChoice::BannerAndFeed,
        };
        assert!(!settings.normalized().include_replies);
        let feed = ChannelNotificationSettings {
            all_new_posts: NssChoice::Feed,
            ..settings
        };
        assert!(feed.normalized().include_replies);
    }

    #[test]
    fn batch_skips_failed_entries() {
        let results = batch_results(&json!({"results":[
            {"channelId":"19:a","success":true,"settings":{"allNewPosts":"Off"}},
            {"channelId":"19:b","success":false,"errorMessage":"nope"}
        ]}));
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].settings.unwrap().all_new_posts,
            Some(NssChoice::Off)
        );
        assert_eq!(results[1].settings, None);
    }

    #[test]
    fn follow_states_accept_lists_and_maps() {
        assert_eq!(
            follow_states(&json!([{"rootMessageId": 12, "f": 1}])),
            vec![("12".into(), FollowState::ExplicitFollow)]
        );
        assert_eq!(
            follow_states(&json!({"rcMetadata": {"34": {"f": 3}}})),
            vec![("34".into(), FollowState::ExplicitUnfollow)]
        );
    }
}
