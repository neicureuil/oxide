//! SharePoint / OneDrive / Substrate files; no fixed base: URLs are built on the file's site, one token per host.
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use teams_auth::diagnostics::Log;
use teams_auth::net::{self, Payload, Signer};
use teams_auth::{Authenticator, Service};
use teams_i18n::{t, tf};
use url::Url;

mod download;
mod drive;
mod reference;
mod share;
mod site;
mod substrate;
mod upload;

pub use download::{DownloadInfo, Progress, thumbnail_url};
pub use drive::{DriveEntry, DriveList, FOLDER_COLORS};
pub use reference::{FileReference, ServiceName, extension, files_property};
pub use share::Shared;
pub use site::{PersonalSite, chat_folder_name};
pub use substrate::{SharedFile, SharedLink};
pub use upload::{CHUNK_SIZE, Destination, UploadedFile};

/// Refused SharePoint requests: why a transfer failed, without tokens or file paths.
static FILES_LOG: Log = Log::new("files.log");

/// Client type sent in the `Application` header (`Teams_<ClientType>`).
const APPLICATION: &str = "Teams_Web";

/// SharePoint error codes the client recognises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpoError {
    AlreadyPresent,
    FileLocked,
    QuotaReached,
    NotPresent,
    AccessDenied,
    AlreadyInProgress,
    FolderMissing,
    InvalidAudience,
    /// [`Self::InvalidAudience`] from a site whose challenge names another tenant than the user's.
    ForeignTenant,
    Other(String),
}
impl SpoError {
    pub fn from_code(code: &str) -> Self {
        let code = code.split(',').next().unwrap_or("").trim();
        match code {
            "-2130575257" | "-2147024816" | "nameAlreadyExists" => Self::AlreadyPresent,
            "-2147018894" | "-2147018887" | "resourceLocked" => Self::FileLocked,
            "-2147023080" | "quotaLimitReached" => Self::QuotaReached,
            "-2130575338" | "itemNotFound" => Self::NotPresent,
            "-2147024891" | "FileAccessDenied" | "accessDenied" => Self::AccessDenied,
            "-2130575142" | "-1" => Self::AlreadyInProgress,
            "-2147024893" => Self::FolderMissing,
            "-2147024809"
            | "invalidAudienceUri"
            | "Microsoft.IdentityModel.Tokens.AudienceUriValidationFailedException" => {
                Self::InvalidAudience
            }
            other => Self::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Auth(#[from] teams_auth::AuthError),
    #[error("{}", tf!("files.lib.the_file_service_is_throttled_try", seconds = .0))]
    RateLimited(u64),
    #[error("{}", http_message(*status, code))]
    Http {
        status: u16,
        code: Option<SpoError>,
        /// Multi-geo: `error["@error.redirectLocation"]` of a 404.
        redirect: Option<String>,
    },
    #[error("{}", tf!("files.lib.couldn_t_connect_to_the_file", detail = .0))]
    Transport(String),
    #[error("{}", t("files.lib.invalid_response_from_the_file_service"))]
    Decode,
    #[error("{}", tf!("files.lib.couldn_t_read_the_local_file", detail = .0))]
    Io(String),
    #[error("{}", t("files.lib.transfer_canceled"))]
    Cancelled,
}
impl From<net::Failure> for Error {
    fn from(failure: net::Failure) -> Self {
        match failure {
            net::Failure::Auth(e) => Self::Auth(e),
            net::Failure::RateLimited(seconds) => Self::RateLimited(seconds),
            net::Failure::Transport(e) => Self::Transport(e.without_url().to_string()),
        }
    }
}
fn http_message(status: u16, code: &Option<SpoError>) -> String {
    t(match code {
        Some(SpoError::AlreadyPresent) => "files.lib.name_already_exists",
        Some(SpoError::FileLocked) => "files.lib.file_locked",
        Some(SpoError::QuotaReached) => "files.lib.storage_full",
        Some(SpoError::NotPresent) => "files.lib.not_found",
        Some(SpoError::AccessDenied) => "files.lib.access_denied",
        Some(SpoError::AlreadyInProgress) => "files.lib.upload_in_progress",
        Some(SpoError::InvalidAudience) => "files.lib.site_moved",
        Some(SpoError::ForeignTenant) => "files.lib.foreign_tenant",
        _ if status == 401 || status == 403 => "files.lib.access_denied",
        _ if status == 404 => "files.lib.not_found",
        _ => return tf!("files.lib.file_service_unavailable_http", status = status),
    })
    .into()
}
impl Error {
    pub fn spo(&self) -> Option<&SpoError> {
        match self {
            Self::Http { code, .. } => code.as_ref(),
            _ => None,
        }
    }
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

/// Who signs a request.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Auth {
    /// Bearer for the origin of the URL (SharePoint, OneDrive, Graph).
    Resource,
    Service(Service),
    /// Pre-authenticated URLs (`@microsoft.graph.downloadUrl`): no credentials at all.
    None,
}
impl Auth {
    /// SharePoint ignores challenges: a 401 only renews the token.
    fn signer(self) -> Signer {
        match self {
            Self::Resource => Signer::Resource,
            Self::Service(service) => Signer::Refresh(service),
            Self::None => Signer::Anonymous,
        }
    }
}
pub(crate) enum Body {
    Empty,
    Json(Value),
    Bytes(Vec<u8>),
}
pub(crate) struct Response {
    pub body: Vec<u8>,
}
impl Response {
    pub fn json(&self) -> Result<Value, Error> {
        if self.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&self.body).map_err(|_| Error::Decode)
    }
}

pub struct FilesClient<A> {
    auth: Arc<A>,
    http: reqwest::Client,
    /// 429 deadlines per host.
    backoff: net::Backoff<String>,
}
impl<A> Clone for FilesClient<A> {
    fn clone(&self) -> Self {
        Self {
            auth: self.auth.clone(),
            http: self.http.clone(),
            backoff: self.backoff.clone(),
        }
    }
}
/// Uploads take up to 300 s per request (`Y2`); everything else is short.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const LONG_TIMEOUT: Duration = Duration::from_secs(300);
impl<A: Authenticator> FilesClient<A> {
    pub fn new(auth: Arc<A>) -> Self {
        Self {
            auth,
            http: net::client(),
            backoff: net::Backoff::default(),
        }
    }
    pub fn authenticator(&self) -> Arc<A> {
        self.auth.clone()
    }

    /// Sends one request, retrying once after a 401 when credentials were renewed.
    pub(crate) async fn send(
        &self,
        auth: Auth,
        method: Method,
        url: Url,
        headers: &[(&str, &str)],
        body: Body,
    ) -> Result<Response, Error> {
        let host = url.host_str().unwrap_or("").to_owned();
        let long = matches!(body, Body::Bytes(_));
        let body = match body {
            // SharePoint answers 411 to a POST without Content-Length.
            Body::Empty if method != Method::GET && method != Method::DELETE => {
                Some((Vec::new(), None))
            }
            Body::Empty => None,
            Body::Json(v) => Some((
                serde_json::to_vec(&v).map_err(|_| Error::Decode)?,
                Some("application/json"),
            )),
            Body::Bytes(b) => Some((b, Some("application/octet-stream"))),
        };
        let base = || {
            let mut map = HeaderMap::new();
            map.insert("accept", HeaderValue::from_static("application/json"));
            if let Auth::Resource = auth {
                map.insert("application", HeaderValue::from_static(APPLICATION));
            }
            map
        };
        let prepare = |map: &mut HeaderMap| {
            for (name, value) in headers {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| Error::Invalid(t("files.lib.invalid_header").into()))?;
                let value = HeaderValue::from_str(value)
                    .map_err(|_| Error::Invalid(t("files.lib.invalid_header").into()))?;
                map.insert(name, value);
            }
            if let Some((bytes, kind)) = &body {
                if bytes.is_empty() {
                    // hyper omits a zero length; SharePoint then answers 411.
                    map.append("content-length", HeaderValue::from_static("0"));
                }
                if let Some(kind) = kind
                    && !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
                {
                    map.append("content-type", HeaderValue::from_static(kind));
                }
            }
            Ok(())
        };
        let target = || Ok::<_, Error>(url.clone());
        let request = net::Exchange::new(auth.signer(), method, &target)
            .headers(&base)
            .prepare(&prepare)
            .payload(body.as_ref().map_or(Payload::Empty, |(bytes, _)| Payload::Bytes(bytes)))
            .timeout(Some(if long { LONG_TIMEOUT } else { TIMEOUT }))
            .throttle(&self.backoff, host, |headers| {
                net::retry_after(headers).unwrap_or(30).min(3600)
            });
        let response = net::exchange(&self.http, self.auth.as_ref(), request).await?;
        let status = response.status().as_u16();
        let refused = (!(200..300).contains(&status)).then(|| response.headers().clone());
        let body = response
            .bytes()
            .await
            .map_err(|e| Error::Transport(e.without_url().to_string()))?
            .to_vec();
        if let Some(headers) = refused {
            return Err(self.refused(&url, status, &headers, &body));
        }
        Ok(Response { body })
    }

    /// Logs a refused request and reads its error, telling a site of another organization apart.
    pub(crate) fn refused(
        &self,
        url: &Url,
        status: u16,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Error {
        record_refusal(url, status, headers, body);
        match error_of(status, body) {
            Error::Http {
                status,
                code: Some(SpoError::InvalidAudience),
                redirect,
            } if self.is_foreign(headers) => Error::Http {
                status,
                code: Some(SpoError::ForeignTenant),
                redirect,
            },
            error => error,
        }
    }
    /// Whether a refusal's challenge names another tenant than the signed-in user's.
    fn is_foreign(&self, headers: &HeaderMap) -> bool {
        let home = self.auth.calling_context().and_then(|c| c.tenant_id);
        realm(headers)
            .zip(home)
            .is_some_and(|(site, home)| !site.eq_ignore_ascii_case(&home))
    }

    pub(crate) async fn json(
        &self,
        auth: Auth,
        method: Method,
        url: Url,
        headers: &[(&str, &str)],
        body: Body,
    ) -> Result<Value, Error> {
        self.send(auth, method, url, headers, body).await?.json()
    }
}

/// SharePoint error code: `odata.error.code`, else `error.innerError.code` / `error.code`.
fn raw_code(value: &Value) -> Option<&str> {
    [
        "/odata.error/code",
        "/error/innerError/code",
        "/error/innererror/code",
        "/error/code",
    ]
    .iter()
    .filter_map(|p| value.pointer(p).and_then(Value::as_str))
    .find(|c| !c.is_empty())
}

/// Logs a refused request to `files.log`.
fn record_refusal(url: &Url, status: u16, headers: &HeaderMap, body: &[u8]) {
    FILES_LOG.record(&refusal_line(url, status, headers, body));
}

/// Host and API route (no site, item or share id), SharePoint's code and message (which may name the file),
/// and its authentication diagnostics: `WWW-Authenticate` says which tenant it expects, `x-ms-diagnostics`
/// why a token was refused.
fn refusal_line(url: &Url, status: u16, headers: &HeaderMap, body: &[u8]) -> String {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let message = ["/odata.error/message/value", "/error/message"]
        .iter()
        .find_map(|p| value.pointer(p).and_then(Value::as_str))
        .map(|m| m.chars().take(300).collect::<String>());
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    format!(
        "refused host={} route={} status={status} code={:?} message={message:?} www-authenticate={:?} x-ms-diagnostics={:?}",
        url.host_str().unwrap_or("-"),
        api_route(url),
        raw_code(&value),
        header("www-authenticate"),
        header("x-ms-diagnostics"),
    )
}

/// The tenant id a SharePoint challenge names: `Bearer realm="{tenant}",…`.
fn realm(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("www-authenticate")?.to_str().ok()?;
    let (_, rest) = value.split_once("realm=\"")?;
    rest.split('"').next().filter(|realm| !realm.is_empty())
}

/// The two segments after `_api`, without arguments (`v2.0/shares`, `web/GetFileById`), else `-`.
fn api_route(url: &Url) -> String {
    let mut segments = url.path_segments().into_iter().flatten();
    if !segments.any(|s| s == "_api") {
        return "-".into();
    }
    segments
        .take(2)
        .map(|s| s.split('(').next().unwrap_or(s))
        .collect::<Vec<_>>()
        .join("/")
}

/// SharePoint error code and multi-geo redirect.
fn error_of(status: u16, body: &[u8]) -> Error {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let code = raw_code(&value).map(SpoError::from_code);
    let redirect = value
        .pointer("/error/@error.redirectLocation")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Error::Http {
        status,
        code,
        redirect,
    }
}

/// `encodeURIComponent`: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )`.
const COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');
pub(crate) fn component(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, COMPONENT).to_string()
}
/// Value of an `@v`/`@fn`/`@folder`/`@file` alias: `encodeURIComponent(s.replace(/'/g,"''"))`.
pub(crate) fn alias(value: &str) -> String {
    component(&value.replace('\'', "''"))
}
/// A relative path whose segments are each encoded.
pub(crate) fn path(value: &str) -> String {
    value
        .split('/')
        .filter(|s| !s.is_empty())
        .map(component)
        .collect::<Vec<_>>()
        .join("/")
}

/// `u!` + unpadded base64url(url with `#` → `%23`), as functions `877509.H` / `788514.W` build a `shareId`.
pub fn share_id(url: &str) -> String {
    use base64::Engine;
    let encoded =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(url.replace('#', "%23"));
    format!("u!{encoded}")
}

/// `{site}` with a trailing `/`, as the `_api` URLs are built on it.
pub fn site_root(site: &str) -> Result<Url, Error> {
    let mut url = Url::parse(site.trim())
        .ok()
        .filter(|u| matches!(u.scheme(), "https" | "http") && u.host_str().is_some())
        .ok_or_else(|| Error::Invalid(t("files.lib.invalid_site").into()))?;
    url.set_query(None);
    url.set_fragment(None);
    if !url.path().ends_with('/') {
        let p = format!("{}/", url.path());
        url.set_path(&p);
    }
    Ok(url)
}
/// `{site}` followed by an `_api` path whose query is already encoded.
pub(crate) fn api(site: &Url, rest: &str) -> Result<Url, Error> {
    let site = site_root(site.as_str())?;
    Url::parse(&format!("{}{}", site.as_str(), rest))
        .map_err(|_| Error::Invalid(t("files.lib.invalid_sharepoint_url").into()))
}
pub(crate) fn origin(url: &Url) -> Result<Url, Error> {
    Url::parse(&format!("{}/", url.origin().ascii_serialization()))
        .map_err(|_| Error::Invalid(t("files.lib.invalid_sharepoint_url").into()))
}
pub(crate) fn str_at<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths
        .iter()
        .filter_map(|p| value.pointer(p).and_then(Value::as_str))
        .find(|s| !s.is_empty())
}
pub(crate) fn u64_at(value: &Value, paths: &[&str]) -> Option<u64> {
    paths.iter().find_map(|p| {
        let v = value.pointer(p)?;
        v.as_u64().or_else(|| v.as_str()?.parse().ok())
    })
}
pub(crate) fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_id_matches_the_web_client() {
        // u! + base64url without padding, '#' escaped first.
        assert_eq!(
            share_id("https://contoso.sharepoint.com/a#b"),
            format!(
                "u!{}",
                base64::Engine::encode(
                    &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                    "https://contoso.sharepoint.com/a%23b"
                )
            )
        );
        assert!(!share_id("https://x/?a=>>>").contains(['=', '+', '/']));
    }

    #[test]
    fn aliases_double_quotes_and_encode_like_encode_uri_component() {
        assert_eq!(alias("l'été 1.docx"), "l''%C3%A9t%C3%A9%201.docx");
        assert_eq!(path("/Documents/Microsoft Teams Chat Files/"), "Documents/Microsoft%20Teams%20Chat%20Files");
    }

    #[test]
    fn reads_spo_error_codes_and_redirects() {
        let e = error_of(
            409,
            br#"{"odata.error":{"code":"-2130575257, Microsoft.SharePoint.SPException"}}"#,
        );
        assert_eq!(e.spo(), Some(&SpoError::AlreadyPresent));
        let e = error_of(
            404,
            br#"{"error":{"code":"itemNotFound","@error.redirectLocation":"https://eu.sharepoint.com/sites/x/_api/v2.0/a"}}"#,
        );
        assert_eq!(e.spo(), Some(&SpoError::NotPresent));
        assert!(matches!(e, Error::Http { redirect: Some(_), .. }));
        assert_eq!(
            error_of(500, br#"{"error":{"code":"generalException","innerError":{"code":"quotaLimitReached"}}}"#).spo(),
            Some(&SpoError::QuotaReached)
        );
    }

    #[test]
    fn logs_refusals_without_the_path_or_ids() {
        let url = Url::parse(
            "https://contoso-my.sharepoint.com/personal/someone_contoso_com/_api/v2.0/shares/u!aHR0cHM6Ly9/driveItem/?select=id",
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "www-authenticate",
            HeaderValue::from_static(r#"Bearer realm="tenant-guid",error="invalid_token""#),
        );
        headers.insert(
            "x-ms-diagnostics",
            HeaderValue::from_static(r#"3000003;reason="Invalid audience Uri""#),
        );
        let line = refusal_line(
            &url,
            401,
            &headers,
            br#"{"error":{"code":"invalidAudienceUri","message":"Invalid audience Uri"}}"#,
        );
        assert_eq!(
            line,
            r#"refused host=contoso-my.sharepoint.com route=v2.0/shares status=401 code=Some("invalidAudienceUri") message=Some("Invalid audience Uri") www-authenticate=Some("Bearer realm=\"tenant-guid\",error=\"invalid_token\"") x-ms-diagnostics=Some("3000003;reason=\"Invalid audience Uri\"")"#
        );
        let by_id =
            Url::parse("https://contoso.sharepoint.com/_api/web/GetFileById('0fb7e6b8')/$value")
                .unwrap();
        assert_eq!(api_route(&by_id), "web/GetFileById");
        let download = Url::parse("https://contoso-my.sharepoint.com/personal/someone/_layouts/15/download.aspx?tempauth=x").unwrap();
        assert!(
            refusal_line(&download, 403, &HeaderMap::new(), b"")
                .contains(" route=- status=403 code=None message=None ")
        );
    }

    #[test]
    fn site_root_keeps_one_trailing_slash() {
        assert_eq!(
            site_root("https://contoso-my.sharepoint.com/personal/jdoe_contoso_com").unwrap().as_str(),
            "https://contoso-my.sharepoint.com/personal/jdoe_contoso_com/"
        );
        assert!(site_root("ftp://x").is_err());
    }
}
