//! Executes the HTTP commands of a [`CallSession`](crate::session::CallSession) against the conversation service.
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderMap, HeaderValue, Method};
use serde_json::Value;
use teams_auth::{Authenticator, Service};
use url::Url;

use crate::session::Op;

/// `TsCallingVersion` of the `X-Microsoft-Skype-Client` header.
const TS_CALLING_VERSION: &str = "2026.42.01.1";
/// Signalling requests fail faster than other service calls.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Status used in [`crate::session::Input::Response`] for failures without an HTTP status.
pub const NETWORK_ERROR: u16 = 490;
pub const FORBIDDEN_HOST: u16 = 403;
/// Credentials could not be obtained (shown as an expired session).
const AUTH_ERROR: u16 = 495;

/// Links come from the server: a token only goes to a Microsoft calling host (or the configured base, for tests).
const TRUSTED_SUFFIXES: [&str; 5] = [
    ".skype.com",
    ".teams.microsoft.com",
    ".teams.cloud.microsoft",
    ".teams.microsoft.us",
    ".teams.microsoftonline.cn",
];

pub struct ConversationClient<A> {
    auth: Arc<A>,
    http: reqwest::Client,
}

impl<A> Clone for ConversationClient<A> {
    fn clone(&self) -> Self {
        Self {
            auth: self.auth.clone(),
            http: self.http.clone(),
        }
    }
}

impl<A: Authenticator> ConversationClient<A> {
    pub fn new(auth: Arc<A>) -> Self {
        Self {
            auth,
            http: teams_auth::net::client(),
        }
    }

    pub fn authenticator(&self) -> Arc<A> {
        self.auth.clone()
    }

    /// Base URL where calls are started and meetings joined.
    pub fn conversation_service(&self) -> Option<String> {
        self.auth
            .base_url(Service::CallingConversation)
            .ok()
            .map(|mut url| {
                if url.path().ends_with("/epconv") {
                    url.set_path(&format!("{}/", url.path()));
                }
                String::from(url)
            })
    }

    fn trusted(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        let configured = [
            Service::CallingConversation,
            Service::CallingConversationBase,
        ]
        .into_iter()
        .filter_map(|s| self.auth.base_url(s).ok())
        .any(|base| base.host_str() == Some(host) && base.scheme() == url.scheme());
        configured
            || url.scheme() == "https"
                && (host == "teams.microsoft.com"
                    || TRUSTED_SUFFIXES.iter().any(|suffix| host.ends_with(suffix)))
    }

    /// Sends a session's command with the credentials its `op` needs.
    pub async fn send_op(
        &self,
        call_id: &str,
        op: Op,
        method: Method,
        url: &str,
        body: &Value,
    ) -> Result<Value, u16> {
        match op {
            Op::RecorderCommand(_) => self.send_to_recorder(call_id, url, body).await,
            _ => self.send(call_id, method, url, body).await,
        }
    }

    /// `CallingBotRestClient.sendCommand`: POST to the bot's `commandUrl` with the recorder's AAD token only.
    async fn send_to_recorder(&self, call_id: &str, url: &str, body: &Value) -> Result<Value, u16> {
        let url = Url::parse(url).map_err(|_| FORBIDDEN_HOST)?;
        // `isTrustedCommandUrl` with the web client's empty domain list: any https host.
        if url.scheme() != "https" && !self.trusted(&url) {
            return Err(FORBIDDEN_HOST);
        }
        crate::diagnostics::record(&format!(
            "recorder command host={}",
            url.host_str().unwrap_or_default()
        ));
        for attempt in 0..2 {
            let mut headers = HeaderMap::new();
            self.auth
                .authorize(Service::CallRecorder, &mut headers)
                .await
                .map_err(|_| AUTH_ERROR)?;
            for (name, value) in [
                ("requestid", uuid::Uuid::new_v4().to_string()),
                ("x-microsoft-skype-chain-id", call_id.to_owned()),
            ] {
                if let Ok(value) = HeaderValue::from_str(&value) {
                    headers.insert(name, value);
                }
            }
            let response = self
                .http
                .post(url.clone())
                .headers(headers)
                .json(body)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
                .map_err(|_| NETWORK_ERROR)?;
            let status = response.status().as_u16();
            crate::diagnostics::record(&format!("recorder status={status}"));
            if status == 401 && attempt == 0 && self.auth.on_unauthorized(Service::CallRecorder).await {
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(status);
            }
            let bytes = response.bytes().await.unwrap_or_default();
            return Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null));
        }
        Err(401)
    }

    /// Sends one command. `Err` carries the HTTP status, [`NETWORK_ERROR`] or [`FORBIDDEN_HOST`].
    pub async fn send(
        &self,
        call_id: &str,
        method: Method,
        url: &str,
        body: &Value,
    ) -> Result<Value, u16> {
        let url = Url::parse(url).map_err(|_| FORBIDDEN_HOST)?;
        if !self.trusted(&url) {
            return Err(FORBIDDEN_HOST);
        }
        let mut previous = "";
        let path_shape: Vec<_> = url
            .path()
            .split('/')
            .map(|part| {
                let redact = previous == "conv" || part.len() > 24 || part.contains('@');
                previous = part;
                if redact { ":id" } else { part }
            })
            .collect();
        crate::diagnostics::record(&format!(
            "request host={} path={} query-keys={:?}",
            url.host_str().unwrap_or_default(),
            path_shape.join("/"),
            url.query_pairs()
                .map(|(key, _)| key.into_owned())
                .collect::<Vec<_>>()
        ));
        let client = format!(
            "TeamsRust/{}/TsCallingVersion={TS_CALLING_VERSION}",
            teams_auth::CLIENT_VERSION
        );
        for attempt in 0..2 {
            let mut headers = HeaderMap::new();
            self.auth
                .authorize(Service::CallingConversation, &mut headers)
                .await
                .map_err(|_| AUTH_ERROR)?;
            let mut put = |name: &'static str, value: &str| {
                if let Ok(value) = HeaderValue::from_str(value) {
                    headers.insert(name, value);
                }
            };
            put("x-microsoft-skype-chain-id", call_id);
            put(
                "x-microsoft-skype-message-id",
                &uuid::Uuid::new_v4().to_string(),
            );
            put("x-microsoft-skype-client", &client);
            if let Some(context) = self.auth.calling_context() {
                for (name, value) in [
                    ("ms-teams-region", context.region),
                    ("ms-teams-partition", context.partition),
                    ("ms-teams-ring", context.ring),
                ] {
                    if let Some(value) = value {
                        put(name, &value);
                    }
                }
            }
            let response = self
                .http
                .request(method.clone(), url.clone())
                .headers(headers)
                .json(body)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
                .map_err(|_| NETWORK_ERROR)?;
            let status = response.status().as_u16();
            crate::diagnostics::record(&format!(
                "response chain-header={} differs={}",
                response
                    .headers()
                    .contains_key("x-microsoft-skype-chain-id"),
                response
                    .headers()
                    .get("x-microsoft-skype-chain-id")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v != call_id)
            ));
            crate::diagnostics::record(&format!("http {method} status={status}"));
            if status == 401
                && attempt == 0
                && self
                    .auth
                    .on_challenge(Service::CallingConversation, response.headers())
                    .await
            {
                continue;
            }
            if !(200..300).contains(&status) {
                let bytes = response.bytes().await.unwrap_or_default();
                let error_body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let error = error_body.get("operationFailure").unwrap_or(&error_body);
                crate::diagnostics::record(&format!(
                    "http error code={} subCode={} phrase={} keys={:?}",
                    error.get("code").unwrap_or(&Value::Null),
                    error.get("subCode").unwrap_or(&Value::Null),
                    error.get("phrase").unwrap_or(&Value::Null),
                    error.as_object().map(|o| o.keys().collect::<Vec<_>>())
                ));
                return Err(status);
            }
            let bytes = response.bytes().await.map_err(|_| NETWORK_ERROR)?;
            if bytes.is_empty() {
                return Ok(Value::Null);
            }
            // A 2xx with a body we cannot read still went through.
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            crate::diagnostics::record(&format!(
                "setup-debug-present={} conversation-type={} modalities={:?}",
                body.get("streamingSetupFailureDebugInfo")
                    .is_some_and(|v| !v.is_null()),
                body.pointer("/state/conversationType")
                    .unwrap_or(&Value::Null),
                body.get("activeModalities")
                    .and_then(Value::as_object)
                    .map(|o| o.keys().collect::<Vec<_>>())
            ));
            crate::diagnostics::record(&format!(
                "response keys={:?} links={:?} controller={}",
                body.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                body.get("links")
                    .and_then(Value::as_object)
                    .map(|o| o.keys().collect::<Vec<_>>()),
                body.get("conversationController").is_some()
            ));
            return Ok(body);
        }
        Err(401)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teams_auth::StaticAuthenticator;

    #[tokio::test]
    async fn credentials_only_go_to_microsoft_calling_hosts_or_the_configured_service() {
        let auth = StaticAuthenticator::new().with_service(
            Service::CallingConversation,
            Url::parse("http://127.0.0.1:9/epconv/").unwrap(),
        );
        let client = ConversationClient::new(Arc::new(auth));
        for url in [
            "https://evil.example/leave",
            "https://teams.microsoft.com.evil.example/x",
            "http://api.flightproxy.teams.microsoft.com/x",
            "not a url",
        ] {
            assert_eq!(
                client.send("c", Method::POST, url, &Value::Null).await,
                Err(FORBIDDEN_HOST),
                "{url}"
            );
        }
        for url in [
            "https://api.flightproxy.teams.microsoft.com/api/v2/epconv/x",
            "https://api.conv.skype.com/conv/x",
            "http://127.0.0.1:9/c/1/leave",
        ] {
            assert!(client.trusted(&Url::parse(url).unwrap()), "{url}");
        }
    }
}
