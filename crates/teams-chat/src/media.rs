//! Authenticated AMS downloads. No bearer/Skype headers are sent to external images.
use crate::{ChatClient, Error};
use http::HeaderMap;
use teams_auth::{Authenticator, Service};
use teams_i18n::t;
use url::Url;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
fn object_path(url: &Url) -> Option<&str> {
    let start = url.path().find("/v1/")?;
    let tail = &url.path()[start + 1..];
    let parts: Vec<_> = tail.split('/').collect();
    ((parts.get(1) == Some(&"objects") && parts.len() >= 4)
        || (parts.get(2) == Some(&"objects") && parts.len() >= 5))
        .then_some(tail)
}
/// The URL preview service's images: card images it relays (`…/urlp/v1/url/content?url=…`), and link preview
/// thumbnails and favicons (`…/urlp/v1/url/image/Thumbnail`, `…/image/Favicon`); they want AMS credentials too.
fn url_preview_path(url: &Url) -> bool {
    let path = url.path();
    path.ends_with("/v1/url/content") || path.contains("/v1/url/image/")
}
/// Thumbnails the preview service names on its legacy host: the web client fetches them from `urlpV2` (`279775.SH`).
const LEGACY_URLP: &str = "urlp.asm.skype.com";
/// Other preview image hosts the web client fetches with the preview service's credentials.
fn urlp_host(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some("urlp.sfbassets.com" | "urlp.ams.gcc.teams.microsoft.com")
    )
}
fn ams_host(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("");
    host.ends_with(".asm.skype.com")
        || host.ends_with(".asyncgw.teams.microsoft.com")
        || [
            "api.ams.gcc.teams.microsoft.com",
            "api.ams.gov.teams.microsoft.us",
            "api.ams.dod.teams.microsoft.us",
            "api.asyncgw.teams.microsoft.scloud",
            "api.asyncgw.teams.microsoftonline.cn",
            "api.asyncgw.teams.eaglex.ic.gov",
            "api.asyncgw.communications.svc.sovcloud.fr",
            "api.asyncgw.communications.svc.sovcloud.de",
        ]
        .contains(&host)
        || (host == "teams.microsoft.com" && url.path().starts_with("/asm/"))
}
fn is_sharepoint(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        [
            ".sharepoint.com",
            ".sharepoint.us",
            ".sharepoint.cn",
            ".sharepoint.de",
        ]
        .iter()
        .any(|s| host.ends_with(s))
    })
}
fn safe_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}
/// A cookie AMS set for `<img>` loads, kept as `name=value` with the hosts it goes to.
#[derive(Debug, Clone)]
pub(crate) struct MediaCookie {
    /// Lower case, without a leading dot.
    domain: String,
    /// No `Domain` attribute: only the host that set it.
    host_only: bool,
    pair: String,
}
impl MediaCookie {
    fn matches(&self, host: &str) -> bool {
        host == self.domain || (!self.host_only && host.ends_with(&format!(".{}", self.domain)))
    }
}
fn parse_cookies(headers: &HeaderMap, host: &str) -> Vec<MediaCookie> {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|line| {
            let mut parts = line.split(';').map(str::trim);
            let pair = parts.next().filter(|p| p.contains('='))?.to_owned();
            let domain = parts
                .filter_map(|a| a.split_once('='))
                .find(|(k, _)| k.trim().eq_ignore_ascii_case("domain"))
                .map(|(_, v)| v.trim().trim_start_matches('.').to_ascii_lowercase());
            Some(MediaCookie {
                host_only: domain.is_none(),
                domain: domain.unwrap_or_else(|| host.to_ascii_lowercase()),
                pair,
            })
        })
        .collect()
}
impl<A: Authenticator> ChatClient<A> {
    /// The cookie the web client's `<img>` tags carry to AMS and the preview proxy: `amsImageAuthCookie`,
    /// `POST {amsV2}v1/{userId}/aadtokenauth` with the AMS credentials. Asked once, again on `renew`.
    async fn media_cookies(&self, renew: bool) -> Result<Vec<MediaCookie>, Error> {
        let mut cookies = self.media_cookies.lock().await;
        if let Some(known) = cookies.as_ref().filter(|_| !renew) {
            return Ok(known.clone());
        }
        let base = self.auth.base_url(Service::Ams)?;
        let oid = self
            .auth
            .calling_context()
            .and_then(|c| c.user_mri.strip_prefix("8:orgid:").map(str::to_owned));
        let path = match &oid {
            Some(oid) => format!("v1/{oid}/aadtokenauth"),
            None => "v1/aadtokenauth".to_owned(),
        };
        let url = Url::parse(&format!("{}/{path}", base.as_str().trim_end_matches('/')))?;
        let mut headers = HeaderMap::new();
        self.auth.authorize(Service::Ams, &mut headers).await?;
        let response = self
            .http
            .post(url)
            .headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::Status {
                endpoint: "aadtokenauth",
                status: response.status(),
                body: String::new(),
            });
        }
        let found = parse_cookies(response.headers(), base.host_str().unwrap_or_default());
        *cookies = Some(found.clone());
        Ok(found)
    }
    /// Puts an image on AMS, readable by the members of `conversation`, as the web client does for a pasted image:
    /// `POST v1/objects/` creates the object, `PUT v1/objects/{id}/content/imgpsh` sends the bytes.
    /// Returns the address the message's `<img>` points to.
    pub async fn upload_image(
        &self,
        conversation: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<String, Error> {
        let base = self.auth.base_url(Service::Ams)?;
        let base = base.as_str().trim_end_matches('/');
        let body = serde_json::json!({
            "type": "pish/image",
            "permissions": { conversation: ["read"] },
            "filename": name,
            "sharingMode": "Inline",
        });
        let mut headers = HeaderMap::new();
        self.auth.authorize(Service::Ams, &mut headers).await?;
        // AMS answers 400 to object creation without it.
        headers.insert(
            "x-ms-client-version",
            http::HeaderValue::from_static(teams_auth::CLIENT_VERSION),
        );
        let response = self
            .http
            .post(Url::parse(&format!("{base}/v1/objects/"))?)
            .headers(headers)
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(Error::Http)?;
        let status = response.status();
        let text = response.text().await.map_err(Error::Http)?;
        if !status.is_success() {
            return Err(Error::Status {
                endpoint: "AMS objects",
                status,
                body: text,
            });
        }
        let created: serde_json::Value =
            serde_json::from_str(&text).map_err(|source| Error::Decode {
                endpoint: "AMS objects",
                source,
            })?;
        let id = created["id"]
            .as_str()
            .filter(|id| !id.is_empty() && !id.contains(['/', '?', '#']))
            .ok_or(Error::Media(t("chat.media.the_image_service_returned_no_object")))?;
        let mut headers = HeaderMap::new();
        self.auth.authorize(Service::Ams, &mut headers).await?;
        let response = self
            .http
            .put(Url::parse(&format!("{base}/v1/objects/{id}/content/imgpsh"))?)
            .headers(headers)
            .header(http::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .map_err(Error::Http)?;
        if !response.status().is_success() {
            return Err(Error::Status {
                endpoint: "AMS content",
                status: response.status(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        Ok(format!("{base}/v1/objects/{id}/views/imgo"))
    }
    pub async fn download_image(&self, source: &str) -> Result<Vec<u8>, Error> {
        let source = Url::parse(source)
            .map_err(|_| Error::Media(t("chat.media.invalid_image_address")))?;
        if !safe_url(&source) {
            return Err(Error::Media(t("chat.media.image_address_isn_t_https_or")));
        }
        let ams = self.auth.base_url(Service::Ams).ok();
        let urlp = self.auth.base_url(Service::UrlPreview).ok();
        let mut source = source;
        if source.host_str() == Some(LEGACY_URLP)
            && let Some(base) = &urlp
        {
            // Only the origin changes, as in the web client.
            let _ = source.set_host(base.host_str());
            let _ = source.set_port(base.port());
            let _ = source.set_scheme(base.scheme());
        }
        // `getUrlPreviewContent` asks for `<thumbnail>&v=1`.
        if url_preview_path(&source) && !source.query_pairs().any(|(k, _)| k == "v") {
            source.query_pairs_mut().append_pair("v", "1");
        }
        let is_ams =
            ams_host(&source) || ams.as_ref().is_some_and(|b| b.origin() == source.origin());
        let mut target = if is_ams && let Some(tail) = object_path(&source) {
            if let Some(base) = &ams {
                let mut url =
                    Url::parse(&format!("{}/{}", base.as_str().trim_end_matches('/'), tail))?;
                url.set_query(source.query());
                if !url.query_pairs().any(|(k, _)| k == "v") {
                    url.query_pairs_mut().append_pair("v", "1");
                }
                url
            } else {
                return Err(Error::Media(t("chat.media.the_session_s_media_service_is")));
            }
        } else {
            source
        };
        let mut auth_retries = 0;
        // Preview images load as the web client's `<img>`: with the AMS cookie, and if refused, with it alone.
        let mut cookie_only = false;
        let mut cookies_renewed = false;
        for _ in 0..6 {
            let configured = [&ams, &urlp]
                .into_iter()
                .flatten()
                .any(|b| b.origin() == target.origin());
            // Only object and URL preview URLs of the discovered AMS or urlp, or a Microsoft AMS host, receive IC3 credentials.
            let authenticated_ams = (object_path(&target).is_some() || url_preview_path(&target))
                && (configured
                    || (safe_url(&target)
                        && (ams_host(&target) || urlp_host(&target) || target.host_str() == Some(LEGACY_URLP))));
            if !safe_url(&target) && !configured {
                return Err(Error::Media(t("chat.media.non_https_image_redirect_refused")));
            }
            let preview = authenticated_ams && url_preview_path(&target);
            let mut headers = HeaderMap::new();
            let mut sent_cookies = 0;
            if preview {
                let host = target.host_str().unwrap_or_default().to_ascii_lowercase();
                let pairs: Vec<String> = self
                    .media_cookies(false)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| c.matches(&host))
                    .map(|c| c.pair)
                    .collect();
                sent_cookies = pairs.len();
                if let Ok(mut value) = http::HeaderValue::from_str(&pairs.join("; "))
                    && !pairs.is_empty()
                {
                    value.set_sensitive(true);
                    headers.insert("cookie", value);
                }
            }
            if authenticated_ams && !cookie_only {
                self.auth.authorize(Service::Ams, &mut headers).await?;
            } else if is_sharepoint(&target) {
                self.auth.authorize_resource(&target, &mut headers).await?;
            }
            let response = self
                .http
                .get(target.clone())
                .headers(headers)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
                .map_err(|_| {
                    Error::Media(t("chat.media.couldn_t_download_the_image"))
                })?;
            if response.status() == http::StatusCode::UNAUTHORIZED && preview {
                if !cookie_only {
                    cookie_only = true;
                    continue;
                }
                if !cookies_renewed {
                    cookies_renewed = true;
                    cookie_only = false;
                    let _ = self.media_cookies(true).await;
                    continue;
                }
            }
            if response.status() == http::StatusCode::UNAUTHORIZED
                && authenticated_ams
                && !preview
                && auth_retries < 2
                && self
                    .auth
                    .on_challenge(Service::Ams, response.headers())
                    .await
            {
                auth_retries += 1;
                continue;
            }
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or(Error::Media(t("chat.media.invalid_image_redirect")))?;
                target = target.join(location).map_err(|_| {
                    Error::Media(t("chat.media.invalid_image_redirect"))
                })?;
                // Rebuild auth headers from scratch for the next origin, including CDN hops.
                continue;
            }
            if !response.status().is_success() {
                // Where it went and with what, to tell a refused cookie from a wrong address (no secrets).
                let mut body = format!(
                    "{}{}",
                    target.host_str().unwrap_or_default(),
                    target.path()
                );
                if preview {
                    let cookies = self.media_cookies(false).await;
                    body.push_str(&match cookies {
                        Ok(c) => format!(
                            " · cookies {sent_cookies}/{} ({})",
                            c.len(),
                            c.iter().map(|c| c.domain.as_str()).collect::<Vec<_>>().join(", ")
                        ),
                        Err(e) => format!(" · aadtokenauth: {e}"),
                    });
                }
                if let Some(challenge) = response
                    .headers()
                    .get("www-authenticate")
                    .and_then(|v| v.to_str().ok())
                {
                    body.push_str(&format!(" · {challenge}"));
                }
                return Err(Error::Status {
                    endpoint: "image",
                    status: response.status(),
                    body,
                });
            }
            if response
                .content_length()
                .is_some_and(|n| n > MAX_IMAGE_BYTES as u64)
            {
                return Err(Error::Media(t("chat.media.image_is_too_large_to_preview")));
            }
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| {
                Error::Media(t("chat.media.image_read_interrupted"))
            })? {
                if bytes.len() + chunk.len() > MAX_IMAGE_BYTES {
                    return Err(Error::Media(t("chat.media.image_is_too_large_to_preview")));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(bytes);
        }
        Err(Error::Media(t("chat.media.too_many_redirects_for_this_image")))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_only_microsoft_ams_objects() {
        let a = Url::parse("https://eu-api.asm.skype.com/v1/objects/abc/views/imgo").unwrap();
        assert!(ams_host(&a));
        assert_eq!(object_path(&a), Some("v1/objects/abc/views/imgo"));
        assert!(!ams_host(
            &Url::parse("https://eu-api.asm.skype.com.attacker.test/v1/objects/abc/views/imgo")
                .unwrap()
        ));
        let preview = Url::parse(
            "https://eu-prod.asyncgw.teams.microsoft.com/urlp/v1/url/content?url=https%3a%2f%2fteams.github.com%2fpr.png",
        )
        .unwrap();
        assert!(ams_host(&preview) && url_preview_path(&preview));
        assert!(!url_preview_path(&a));
        let thumbnail = Url::parse(
            "https://eu-prod.asyncgw.teams.microsoft.com/urlp/v1/url/image/Thumbnail?url=x",
        )
        .unwrap();
        assert!(ams_host(&thumbnail) && url_preview_path(&thumbnail));
        assert!(!safe_url(
            &Url::parse("https://user:pass@example.test/image.png").unwrap()
        ));
    }
}
