//! Teams sign-in and per-service tokens. Secrets stay in memory.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::{HeaderMap, HeaderValue};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

use teams_i18n::{t, tf};

use crate::{AuthError, Authenticator, CallingContext, Service};

const REDIRECT: &str = "https://teams.microsoft.com/v2";
const ORIGIN: &str = "https://teams.microsoft.com";
const SPACES: &str = "https://api.spaces.skype.com";
const CSA: &str = "https://chatsvcagg.teams.microsoft.com";
const IC3_NON_CAE: &str = "https://ic3-non-cae.teams.office.com";
const IC3: &str = "https://ic3.teams.office.com";
const OUTLOOK: &str = "https://outlook.office.com";
/// `substrateTokenUrl`: the audience the web client asks for its searches.
const OUTLOOK_SEARCH: &str = "https://outlook.office.com/search";
/// Audience of the notification settings service (`activity.uisResourceURL`).
const UIS: &str = "https://uis.teams.microsoft.com";
/// `callRecording.resource` of the web client's config: the recorder bot's audience.
const CALL_RECORDER: &str = "4580fd1d-e5a3-4f56-9ad1-aab0e3bf8f76";
/// Token and authz requests are short; fail them sooner than service calls.
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

fn failed(message: impl Into<String>) -> AuthError {
    AuthError::Failed(message.into())
}
fn scope(resource: &str) -> String {
    format!("{}/.default offline_access", resource.trim_end_matches('/'))
}
fn random() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[derive(Clone)]
struct Protocol {
    authority: Url,
    authz: Url,
    client_id: String,
}
impl Protocol {
    fn new(client_id: String) -> Self {
        Self {
            authority: Url::parse("https://login.microsoftonline.com/common/").unwrap(),
            authz: Url::parse("https://teams.microsoft.com/api/authsvc/v1.0/authz").unwrap(),
            client_id,
        }
    }
}

/// A single-use login transaction (PKCE verifier, CSRF state); no Debug so they never reach logs.
pub struct LoginAttempt {
    protocol: Protocol,
    verifier: String,
    state: String,
    started: Instant,
    /// `select_account` in the sign-in window, `none` for a cookie-based renewal like MSAL's hidden iframe.
    prompt: &'static str,
    login_hint: Option<String>,
}
impl LoginAttempt {
    /// Fails while no application ID is known: [`crate::client_id::resolve`] first.
    pub fn new() -> Result<Self, AuthError> {
        crate::client_id::known()
            .map(Self::with_client_id)
            .ok_or_else(crate::client_id::not_found)
    }
    fn with_client_id(client_id: String) -> Self {
        Self {
            protocol: Protocol::new(client_id),
            verifier: random(),
            state: random(),
            started: Instant::now(),
            prompt: "select_account",
            login_hint: None,
        }
    }
    /// Sign-in without any page: Microsoft answers from its cookies or returns `login_required` / `interaction_required`.
    pub fn silent(login_hint: Option<&str>) -> Result<Self, AuthError> {
        Ok(Self {
            prompt: "none",
            login_hint: login_hint.map(str::to_owned),
            ..Self::new()?
        })
    }

    pub fn authorization_url(&self) -> Url {
        let mut url = self
            .protocol
            .authority
            .join("oauth2/v2.0/authorize")
            .unwrap();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes()));
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.protocol.client_id.as_str()),
            ("redirect_uri", REDIRECT),
            ("response_type", "code"),
            ("response_mode", "fragment"),
            ("scope", &scope(SPACES)),
            ("state", &self.state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("prompt", self.prompt),
        ]);
        if let Some(hint) = &self.login_hint {
            url.query_pairs_mut().append_pair("login_hint", hint);
        }
        url
    }

    fn callback_code(&self, callback: &str) -> Result<String, AuthError> {
        if self.started.elapsed() > Duration::from_secs(900) {
            return Err(failed(t("auth.login.sign_in_expired_please_start_over")));
        }
        let url = Url::parse(callback.trim())
            .map_err(|_| failed(t("auth.login.invalid_redirect_url")))?;
        let expected = Url::parse(REDIRECT).unwrap();
        if url.origin() != expected.origin()
            || url.path() != expected.path()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(failed(t("auth.login.the_url_must_be_the_microsoft")));
        }
        let pairs: Vec<_> = if let Some(fragment) = url.fragment() {
            url::form_urlencoded::parse(fragment.as_bytes())
                .into_owned()
                .collect()
        } else {
            url.query_pairs().into_owned().collect()
        };
        let get = |key: &str| -> Result<Option<&str>, AuthError> {
            let mut values = pairs
                .iter()
                .filter(|(k, _)| k == key)
                .map(|(_, v)| v.as_str());
            let value = values.next();
            if values.next().is_some() {
                return Err(failed(t("auth.login.duplicate_redirect_parameter")));
            }
            Ok(value)
        };
        if get("state")? != Some(self.state.as_str()) {
            return Err(failed(t("auth.login.the_redirect_doesn_t_match_this")));
        }
        if get("error")?.is_some() {
            return Err(failed(t("auth.login.microsoft_declined_or_canceled_the_sign")));
        }
        get("code")?
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                failed(t("auth.login.the_url_has_no_code_please"))
            })
    }

    /// Consumes the callback and exchanges its code; cannot be reused.
    pub async fn complete(self, callback: &str) -> Result<TeamsAuthenticator, AuthError> {
        let code = self.callback_code(callback)?;
        let http = crate::net::client();
        let token = token_request(
            &http,
            &self.protocol,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", REDIRECT),
                ("code_verifier", &self.verifier),
                ("scope", &scope(SPACES)),
            ],
        )
        .await?;
        TeamsAuthenticator::from_token(http, self.protocol, token, None).await
    }
}

/// OS-backed refresh token storage; must serialize save/clear and reject saves after clear (in-flight requests on logout).
pub trait RefreshTokenStore: Send + Sync {
    fn save(&self, token: &str) -> Result<(), AuthError>;
    fn clear(&self) -> Result<(), AuthError>;
}
impl TeamsAuthenticator {
    /// Restore via refresh grant and fresh authz discovery; no cached access token.
    pub async fn restore(refresh_token: &str) -> Result<Self, AuthError> {
        let client_id = crate::client_id::resolve().await?;
        Self::restore_with_protocol(refresh_token, Protocol::new(client_id)).await
    }
    async fn restore_with_protocol(
        refresh_token: &str,
        protocol: Protocol,
    ) -> Result<Self, AuthError> {
        let http = crate::net::client();
        let token = token_request(
            &http,
            &protocol,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("scope", &scope(SPACES)),
            ],
        )
        .await?;
        Self::from_token(http, protocol, token, Some(refresh_token)).await
    }
    async fn from_token(
        http: reqwest::Client,
        protocol: Protocol,
        mut token: TokenResponse,
        previous_refresh: Option<&str>,
    ) -> Result<Self, AuthError> {
        if token.refresh_token.is_none() {
            token.refresh_token = previous_refresh.map(str::to_owned);
        }
        let mut tokens = HashMap::new();
        tokens.insert(SPACES.to_owned(), AccessToken::from_response(&token)?);
        let discovery = discover(
            &http,
            &protocol,
            &token.access_token,
            previous_refresh.is_some(),
        )
        .await?;
        let mut identity = identity(&discovery.tokens.skype_token)?;
        identity.tenant = tenant(&token.access_token);
        identity.login_hint = login_hint(&token.access_token);
        let region_headers = discovery.region_headers();
        let routing = discovery.calling_routing();
        let session = Session {
            tokens,
            refresh_expires: refresh_expiry(&token),
            refresh_token: token.refresh_token,
            skype_expires: expiry(discovery.tokens.expires_in)?,
            skype_token: discovery.tokens.skype_token,
            services: service_urls(discovery.region_gtms)?,
            registration: None,
            chat_mode: (ChatMode::Cae, Instant::now()),
            calling_modes: HashMap::new(),
            region_headers,
            routing,
        };
        Ok(TeamsAuthenticator {
            http,
            protocol,
            session: RwLock::new(session),
            refresh: Mutex::new(()),
            identity,
            store: RwLock::new(None),
            persistence_disabled: AtomicBool::new(false),
            persistence_error: RwLock::new(None),
            refused: AtomicBool::new(false),
        })
    }
}

/// Identity extracted for display/routing only; JWT decoding is not verification.
#[derive(Debug)]
struct Identity {
    mri: String,
    name: String,
    tenant: Option<String>,
    /// Account name, so a silent renewal picks this account among the signed-in ones.
    login_hint: Option<String>,
}
impl Identity {
    /// `Oid:{oid}@{tenantId}`, the Substrate routing key.
    fn anchor_mailbox(&self) -> Option<String> {
        let oid = self.mri.strip_prefix("8:orgid:")?;
        Some(format!("Oid:{oid}@{}", self.tenant.as_deref()?))
    }
}
fn claims(token: &str) -> Option<serde_json::Value> {
    let bytes = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    serde_json::from_slice(&bytes).ok()
}
/// AAD tenant of the signed-in user, from the `tid` claim of an AAD access token.
fn tenant(aad: &str) -> Option<String> {
    claims(aad)?
        .get("tid")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
/// `preferred_username` (v2 tokens) or `upn` (v1) of an AAD access token.
fn login_hint(aad: &str) -> Option<String> {
    let claims = claims(aad)?;
    ["preferred_username", "upn"]
        .iter()
        .find_map(|key| claims.get(*key)?.as_str().filter(|s| s.contains('@')))
        .map(str::to_owned)
}
fn refresh_expiry(token: &TokenResponse) -> Option<SystemTime> {
    token.refresh_token.as_ref()?;
    SystemTime::now().checked_add(Duration::from_secs(token.refresh_token_expires_in?))
}
fn identity(token: &str) -> Result<Identity, AuthError> {
    let claims = claims(token)
        .ok_or_else(|| failed(t("auth.login.invalid_skypetoken_jwt")))?;
    let id = claims
        .get("userId")
        .or_else(|| claims.get("skypeid"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            failed(t("auth.login.userid_missing_from_the_skypetoken"))
        })?;
    let mri = if id.starts_with("orgid:") {
        format!("8:{id}")
    } else if id.contains(':') {
        id.to_owned()
    } else {
        format!("8:orgid:{id}")
    };
    let name = claims
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    Ok(Identity {
        mri,
        name,
        tenant: None,
        login_hint: None,
    })
}

struct AccessToken {
    value: String,
    expires: SystemTime,
}
impl AccessToken {
    fn from_response(token: &TokenResponse) -> Result<Self, AuthError> {
        if token.access_token.is_empty() {
            return Err(failed(t("auth.login.empty_aad_token")));
        }
        Ok(Self {
            value: token.access_token.clone(),
            expires: expiry(token.expires_in)?,
        })
    }
    fn valid(&self) -> bool {
        self.expires > SystemTime::now() + Duration::from_secs(60)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChatMode {
    Cae,
    NonCae,
    Skype,
}
impl ChatMode {
    /// AAD resource of the IC3 bearer; `None` for the Skype token.
    fn resource(self) -> Option<&'static str> {
        match self {
            ChatMode::Cae => Some(IC3),
            ChatMode::NonCae => Some(IC3_NON_CAE),
            ChatMode::Skype => None,
        }
    }
}
struct Session {
    tokens: HashMap<String, AccessToken>,
    refresh_token: Option<String>,
    /// `refresh_token_expires_in`: a SPA refresh token dies 24 h after sign-in, however often it rotates.
    refresh_expires: Option<SystemTime>,
    skype_token: String,
    skype_expires: SystemTime,
    services: HashMap<String, Url>,
    registration: Option<HeaderValue>,
    chat_mode: (ChatMode, Instant),
    /// Token type negotiated per calling service through `WWW-Authenticate`.
    calling_modes: HashMap<Service, (ChatMode, Instant)>,
    region_headers: HeaderMap,
    routing: CallingRouting,
}
#[derive(Clone, Default)]
struct CallingRouting {
    region: Option<String>,
    partition: Option<String>,
    ring: Option<String>,
}

/// In-memory Teams session. Serializes refresh so concurrent API calls share it.
pub struct TeamsAuthenticator {
    http: reqwest::Client,
    protocol: Protocol,
    session: RwLock<Session>,
    refresh: Mutex<()>,
    identity: Identity,
    store: RwLock<Option<Arc<dyn RefreshTokenStore>>>,
    persistence_disabled: AtomicBool,
    persistence_error: RwLock<Option<String>>,
    /// Microsoft refused the refresh token: only a new sign-in can continue.
    refused: AtomicBool,
}
impl std::fmt::Debug for TeamsAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TeamsAuthenticator { credentials: [REDACTED] }")
    }
}
impl TeamsAuthenticator {
    pub fn user_id(&self) -> &str {
        &self.identity.mri
    }
    pub fn display_name(&self) -> &str {
        &self.identity.name
    }
    pub fn login_hint(&self) -> Option<&str> {
        self.identity.login_hint.as_deref()
    }
    /// Whether a new sign-in is needed: the refresh token was refused or expires within `margin`.
    pub fn needs_renewal(&self, margin: Duration) -> bool {
        self.refused.load(Ordering::SeqCst)
            || self
                .session
                .read()
                .unwrap()
                .refresh_expires
                .is_some_and(|expires| expires <= SystemTime::now() + margin)
    }
    /// Takes over a new sign-in of the same account, so clients built on `self` carry on, and saves its refresh token.
    pub async fn adopt(&self, renewed: TeamsAuthenticator) -> Result<(), AuthError> {
        if renewed.identity.mri != self.identity.mri {
            return Err(failed(t("auth.login.signing_in_again_opened_a_different")));
        }
        let _refresh = self.refresh.lock().await;
        let session = renewed
            .session
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *self.session.write().unwrap() = session;
        self.refused.store(false, Ordering::SeqCst);
        // A storage failure must not discard the renewed session.
        let _ = self.persist_refresh();
        Ok(())
    }

    pub fn remember_session(&self, store: Arc<dyn RefreshTokenStore>) -> Result<(), AuthError> {
        let mut slot = self.store.write().unwrap();
        if self.persistence_disabled.load(Ordering::SeqCst) {
            return Err(AuthError::NotSignedIn);
        }
        *slot = Some(store);
        drop(slot);
        self.persist_refresh()
    }
    pub fn forget_session(&self) -> Result<(), AuthError> {
        let mut slot = self.store.write().unwrap();
        self.persistence_disabled.store(true, Ordering::SeqCst);
        let store = slot.take();
        drop(slot);
        if let Some(store) = store {
            store.clear()?;
        }
        Ok(())
    }
    pub fn persistence_error(&self) -> Option<String> {
        self.persistence_error.read().unwrap().clone()
    }
    fn persist_refresh(&self) -> Result<(), AuthError> {
        let store = self.store.read().unwrap().clone();
        let token = self.session.read().unwrap().refresh_token.clone();
        if let Some(store) = store {
            let token = token.ok_or_else(|| {
                failed(t("auth.login.microsoft_didn_t_provide_a_refresh"))
            })?;
            let result = store.save(&token);
            *self.persistence_error.write().unwrap() =
                result.as_ref().err().map(ToString::to_string);
            result?;
        }
        Ok(())
    }
    /// AAD resource of presence requests: the origin of `regionGtms.unifiedPresence`.
    fn presence_resource(&self) -> Result<String, AuthError> {
        self.session
            .read()
            .unwrap()
            .services
            .get(Service::UnifiedPresence.key())
            .map(|url| url.origin().ascii_serialization())
            .ok_or(AuthError::UnknownService(Service::UnifiedPresence))
    }
    /// Cached token without the refresh lock, so requests don't queue behind another audience's renewal.
    async fn access_token(&self, resource: &str, force: bool) -> Result<String, AuthError> {
        if !force && let Some(token) = self.cached_token(resource) {
            return Ok(token);
        }
        let _refresh = self.refresh.lock().await;
        self.access_token_locked(resource, force).await
    }
    fn cached_token(&self, resource: &str) -> Option<String> {
        let session = self.session.read().unwrap();
        session
            .tokens
            .get(resource)
            .filter(|t| t.valid())
            .map(|t| t.value.clone())
    }
    /// Caller holds `refresh`; re-checks the cache another holder may have filled.
    async fn access_token_locked(&self, resource: &str, force: bool) -> Result<String, AuthError> {
        {
            let session = self.session.read().unwrap();
            if !force && let Some(token) = session.tokens.get(resource).filter(|t| t.valid()) {
                return Ok(token.value.clone());
            }
        }
        let refresh = self
            .session
            .read()
            .unwrap()
            .refresh_token
            .clone()
            .ok_or_else(|| {
                failed(t("auth.login.your_microsoft_session_has_expired_please"))
            })?;
        let token = token_request(
            &self.http,
            &self.protocol,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("scope", &scope(resource)),
            ],
        )
        .await
        .inspect_err(|e| {
            if matches!(e, AuthError::SessionExpired) {
                self.refused.store(true, Ordering::SeqCst);
            }
        })?;
        let access = AccessToken::from_response(&token)?;
        let value = access.value.clone();
        let mut session = self.session.write().unwrap();
        if token.refresh_token.is_some() {
            if let Some(expires) = refresh_expiry(&token) {
                session.refresh_expires = Some(expires);
            }
            session.refresh_token = token.refresh_token;
        }
        session.tokens.insert(resource.to_owned(), access);
        drop(session);
        // A storage failure must not discard a valid token or break the live session.
        let _ = self.persist_refresh();
        Ok(value)
    }

    fn skype_fresh(&self) -> bool {
        self.session.read().unwrap().skype_expires > SystemTime::now() + Duration::from_secs(60)
    }
    async fn ensure_skype(&self, force: bool) -> Result<(), AuthError> {
        if !force && self.skype_fresh() {
            return Ok(());
        }
        let _refresh = self.refresh.lock().await;
        self.ensure_skype_locked(force).await
    }
    async fn ensure_skype_locked(&self, force: bool) -> Result<(), AuthError> {
        if !force && self.skype_fresh() {
            return Ok(());
        }
        let aad = self.access_token_locked(SPACES, force).await?;
        let mut discovery = discover(&self.http, &self.protocol, &aad, true).await?;
        let previous = self.session.read().unwrap().skype_token.clone();
        if discovery.tokens.skype_token == previous {
            // forceRenew is an AAD acquisition option, not an authz parameter: bypass the AAD cache.
            let aad = self.access_token_locked(SPACES, true).await?;
            discovery = discover(&self.http, &self.protocol, &aad, true).await?;
            if discovery.tokens.skype_token == previous {
                return Err(failed(t("auth.login.authz_didn_t_renew_the_skypetoken")));
            }
        }
        let expires = expiry(discovery.tokens.expires_in)?;
        if identity(&discovery.tokens.skype_token)?.mri != self.identity.mri {
            return Err(failed(t("auth.login.your_teams_identity_has_changed_please")));
        }
        let region_headers = discovery.region_headers();
        let routing = discovery.calling_routing();
        let services = service_urls(discovery.region_gtms)?;
        let mut session = self.session.write().unwrap();
        session.skype_token = discovery.tokens.skype_token;
        session.skype_expires = expires;
        session.services = services;
        session.region_headers = region_headers;
        session.routing = routing;
        session.registration = None;
        Ok(())
    }

    /// STM for the calling stack: CAE ic3 bearer first, then what `WWW-Authenticate: token_types=` asks for.
    fn calling_mode(&self, service: Service) -> ChatMode {
        match self.session.read().unwrap().calling_modes.get(&service) {
            Some((mode, until)) if *until > Instant::now() => *mode,
            _ => ChatMode::Cae,
        }
    }
    fn set_calling_mode(&self, service: Service, mode: ChatMode) {
        self.session
            .write()
            .unwrap()
            .calling_modes
            .insert(service, (mode, Instant::now() + Duration::from_secs(86400)));
    }
    async fn authorize_calling(
        &self,
        service: Service,
        headers: &mut HeaderMap,
    ) -> Result<(), AuthError> {
        let mode = self.calling_mode(service);
        if self.insert_ic3(mode, headers).await? {
            return Ok(());
        }
        if mode != ChatMode::Skype {
            self.set_calling_mode(service, ChatMode::Skype);
        }
        self.ensure_skype(false).await?;
        insert_secret(
            headers,
            "x-skypetoken",
            &self.session.read().unwrap().skype_token,
        )
    }
    /// Renews the token `mode` sends.
    async fn refresh_mode_locked(&self, mode: ChatMode) -> Result<(), AuthError> {
        match mode.resource() {
            Some(resource) => self.access_token_locked(resource, true).await.map(drop),
            None => self.ensure_skype_locked(true).await,
        }
    }
    /// Chat service token type, back to CAE once the negotiated one expired.
    fn chat_mode(&self) -> ChatMode {
        let (mode, until) = self.session.read().unwrap().chat_mode;
        if until < Instant::now() {
            ChatMode::Cae
        } else {
            mode
        }
    }
    /// Inserts the IC3 bearer of `mode`; `false` for Skype or when AAD refuses it.
    async fn insert_ic3(&self, mode: ChatMode, headers: &mut HeaderMap) -> Result<bool, AuthError> {
        let Some(resource) = mode.resource() else {
            return Ok(false);
        };
        let Ok(aad) = self.access_token(resource, false).await else {
            return Ok(false);
        };
        insert_bearer(headers, &aad)?;
        headers.insert("x-ms-migration", HeaderValue::from_static("True"));
        headers.insert("x-ms-test-user", HeaderValue::from_static("False"));
        Ok(true)
    }
}

/// Token type a calling service asks for in `WWW-Authenticate`; `None` means "same type, just refresh it".
fn calling_challenge(challenge: &str, current: ChatMode) -> Option<ChatMode> {
    let challenge = challenge.to_ascii_lowercase();
    let types: Vec<ChatMode> = challenge
        .split_once("token_types")
        .map(|(_, rest)| {
            let list = rest
                .trim_start_matches(['=', ' ', '"'])
                .split('"')
                .next()
                .unwrap_or("");
            list.split(',')
                .filter_map(|t| match t.trim() {
                    "cae" => Some(ChatMode::Cae),
                    "aad" => Some(ChatMode::NonCae),
                    "skype" => Some(ChatMode::Skype),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let mismatch =
        challenge.contains("protocol_mismatch") || challenge.contains("insufficient_claims");
    if types.is_empty() {
        // A claims problem with no hint: walk down CAE → AAD → Skype.
        return mismatch.then_some(match current {
            ChatMode::Cae => ChatMode::NonCae,
            ChatMode::NonCae | ChatMode::Skype => ChatMode::Skype,
        });
    }
    if types.contains(&current) && !mismatch {
        return None;
    }
    [ChatMode::Cae, ChatMode::NonCae, ChatMode::Skype]
        .into_iter()
        .find(|m| types.contains(m) && *m != current)
        .or(Some(current))
}
impl Authenticator for TeamsAuthenticator {
    fn base_url(&self, service: Service) -> Result<Url, AuthError> {
        let session = self.session.read().unwrap();
        if service == Service::ProfileImages {
            return session
                .services
                .get("mtImageService")
                .or_else(|| session.services.get("middleTier"))
                .cloned()
                .ok_or(AuthError::UnknownService(service));
        }
        if service == Service::ChatService
            && let Some(url) = session.services.get("chatServiceAfd")
        {
            return Ok(url.clone());
        }
        if service == Service::Ams
            && let Some(url) = session.services.get("amsV2")
        {
            return Ok(url.clone());
        }
        // `enableUrlPreviewV2Endpoint`: `urlpV2`, else the legacy `urlp`.
        if service == Service::UrlPreview
            && let Some(url) = session.services.get("urlp")
            && !session.services.contains_key("urlpV2")
        {
            return Ok(url.clone());
        }
        // `enableAfdPresenceEndpoint`: requests go through Front Door when it is known.
        if service == Service::UnifiedPresence
            && let Some(url) = session.services.get("presenceUPS")
        {
            return Ok(url.clone());
        }
        if service == Service::Csa {
            return Ok(session
                .services
                .get("chatSvcAggAfd")
                .cloned()
                .unwrap_or_else(|| Url::parse("https://teams.microsoft.com/api/csa/").unwrap()));
        }
        // Defaults when regionGtms omits a calling key.
        let fallback = match service {
            Service::CallingConversation => {
                Some("https://api.flightproxy.teams.microsoft.com/api/v2/epconv/")
            }
            Service::CallingConversationBase => Some("https://api.conv.skype.com/"),
            Service::Trouter => Some("https://go.trouter.teams.microsoft.com/v4/a"),
            Service::Registrar => {
                Some("https://teams.microsoft.com/registrar/prod/v2/registrations")
            }
            Service::Trap => Some("https://teams.microsoft.com/trap/tokens"),
            Service::Outlook => Some("https://outlook.office.com/"),
            Service::Substrate | Service::SubstrateSearch => Some("https://substrate.office.com/"),
            _ => None,
        };
        if let Some(fallback) = fallback {
            return Ok(session
                .services
                .get(service.key())
                .cloned()
                .unwrap_or_else(|| Url::parse(fallback).unwrap()));
        }
        session
            .services
            .get(service.key())
            .cloned()
            .ok_or(AuthError::UnknownService(service))
    }

    async fn authorize(&self, service: Service, headers: &mut HeaderMap) -> Result<(), AuthError> {
        match service {
            Service::ChatService | Service::MiddleTier | Service::Uis => {
                self.ensure_skype(false).await?
            }
            _ => {}
        }
        match service {
            Service::ChatService => {
                let mode = self.chat_mode();
                if self.insert_ic3(mode, headers).await? {
                    return Ok(());
                }
                if mode != ChatMode::Skype {
                    self.session.write().unwrap().chat_mode =
                        (ChatMode::Skype, Instant::now() + Duration::from_secs(86400));
                }
                let session = self.session.read().unwrap();
                if let Some(registration) = &session.registration {
                    headers.insert("registrationtoken", registration.clone());
                } else {
                    insert_secret(
                        headers,
                        "authentication",
                        &format!("skypetoken={}", session.skype_token),
                    )?;
                }
            }
            Service::Calendar | Service::ProfileImages => {
                let aad = self.access_token(SPACES, false).await?;
                headers.remove("x-skypetoken");
                insert_bearer(headers, &aad)?;
                headers.insert("x-ms-migration", HeaderValue::from_static("True"));
            }
            Service::MiddleTier => {
                let aad = self.access_token(SPACES, false).await?;
                insert_bearer(headers, &aad)?;
                insert_secret(
                    headers,
                    "x-skypetoken",
                    &self.session.read().unwrap().skype_token,
                )?;
            }
            // `enableSendPresenceTokenToUPS`: bearer for the regional host, even through Front Door.
            Service::UnifiedPresence => {
                let aad = self.access_token(&self.presence_resource()?, false).await?;
                insert_bearer(headers, &aad)?;
            }
            Service::TeamsAndChannels | Service::Csa => {
                let aad = self.access_token(CSA, false).await?;
                insert_bearer(headers, &aad)?;
                headers.extend(self.session.read().unwrap().region_headers.clone());
                headers.insert("x-ms-migration", HeaderValue::from_static("True"));
            }
            Service::Ams | Service::UrlPreview => {
                if !self.insert_ic3(ChatMode::Cae, headers).await? {
                    self.ensure_skype(false).await?;
                    insert_secret(
                        headers,
                        "authorization",
                        &format!("skype_token {}", self.session.read().unwrap().skype_token),
                    )?;
                }
            }
            Service::Outlook => {
                let aad = self.access_token(OUTLOOK, false).await?;
                insert_bearer(headers, &aad)?;
            }
            // `disableSkypeTokenOnNss = false`: AAD bearer plus the Skype token.
            Service::Uis => {
                let aad = self.access_token(UIS, false).await?;
                insert_bearer(headers, &aad)?;
                insert_secret(
                    headers,
                    "x-skypetoken",
                    &self.session.read().unwrap().skype_token,
                )?;
            }
            Service::CallRecorder => {
                let aad = self.access_token(CALL_RECORDER, false).await?;
                insert_bearer(headers, &aad)?;
            }
            Service::Substrate | Service::SubstrateSearch => {
                let audience = if service == Service::SubstrateSearch {
                    OUTLOOK_SEARCH
                } else {
                    OUTLOOK
                };
                let aad = self.access_token(audience, false).await?;
                insert_bearer(headers, &aad)?;
                if let Some(anchor) = self.identity.anchor_mailbox() {
                    insert_secret(headers, "x-anchormailbox", &anchor)?;
                }
            }
            calling => self.authorize_calling(calling, headers).await?,
        }
        Ok(())
    }
    async fn authorize_resource(
        &self,
        resource: &Url,
        headers: &mut HeaderMap,
    ) -> Result<(), AuthError> {
        let audience = resource_audience(resource)?;
        let aad = self.access_token(&audience, false).await?;
        insert_bearer(headers, &aad)
    }
    async fn on_resource_unauthorized(&self, resource: &Url) -> bool {
        let Ok(audience) = resource_audience(resource) else {
            return false;
        };
        let _refresh = self.refresh.lock().await;
        self.access_token_locked(&audience, true).await.is_ok()
    }
    async fn on_unauthorized(&self, service: Service) -> bool {
        let _refresh = self.refresh.lock().await;
        match service {
            Service::TeamsAndChannels | Service::Csa => {
                self.access_token_locked(CSA, true).await.map(drop)
            }
            Service::Ams | Service::UrlPreview => {
                self.access_token_locked(IC3, true).await.map(drop)
            }
            Service::UnifiedPresence => match self.presence_resource() {
                Ok(resource) => self.access_token_locked(&resource, true).await.map(drop),
                Err(error) => Err(error),
            },
            Service::ChatService => self.refresh_mode_locked(self.chat_mode()).await,
            Service::Calendar | Service::ProfileImages => {
                self.access_token_locked(SPACES, true).await.map(drop)
            }
            Service::MiddleTier => self.ensure_skype_locked(true).await,
            Service::Outlook | Service::Substrate => {
                self.access_token_locked(OUTLOOK, true).await.map(drop)
            }
            Service::SubstrateSearch => self
                .access_token_locked(OUTLOOK_SEARCH, true)
                .await
                .map(drop),
            Service::Uis => self.access_token_locked(UIS, true).await.map(drop),
            Service::CallRecorder => self
                .access_token_locked(CALL_RECORDER, true)
                .await
                .map(drop),
            calling => self.refresh_mode_locked(self.calling_mode(calling)).await,
        }
        .is_ok()
    }
    async fn on_challenge(&self, service: Service, headers: &HeaderMap) -> bool {
        let challenge = headers
            .get("www-authenticate")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if service.is_calling() {
            let current = self.calling_mode(service);
            match calling_challenge(challenge, current) {
                Some(mode) if mode != current => {
                    self.set_calling_mode(service, mode);
                    return true;
                }
                _ => return self.on_unauthorized(service).await,
            }
        }
        if service == Service::ChatService {
            let challenge = challenge.to_ascii_lowercase();
            if challenge.contains("token_types") {
                let mode = if challenge.contains("aad") {
                    ChatMode::NonCae
                } else if challenge.contains("skype") {
                    ChatMode::Skype
                } else {
                    return false;
                };
                self.session.write().unwrap().chat_mode =
                    (mode, Instant::now() + Duration::from_secs(86400));
                return true;
            }
            if self.session.write().unwrap().registration.take().is_some() {
                return true;
            }
        }
        self.on_unauthorized(service).await
    }
    fn observe_response(&self, service: Service, headers: &HeaderMap) -> Result<(), AuthError> {
        if service == Service::ChatService
            && let Some(value) = headers.get("set-registrationtoken")
        {
            let value = value.to_str().map_err(|_| {
                failed(t("auth.login.invalid_registrationtoken"))
            })?;
            let parts: Vec<_> = value
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter(|s| {
                    !s.split_once('=')
                        .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("expires"))
                })
                .collect();
            if !parts.iter().any(|s| s.starts_with("registrationToken="))
                || !parts.iter().any(|s| s.starts_with("endpointId="))
            {
                return Ok(());
            }
            let mut value = HeaderValue::from_str(&parts.join("; ")).map_err(|_| {
                failed(t("auth.login.invalid_registrationtoken"))
            })?;
            value.set_sensitive(true);
            self.session.write().unwrap().registration = Some(value);
        }
        Ok(())
    }
    fn calling_context(&self) -> Option<CallingContext> {
        let routing = self.session.read().unwrap().routing.clone();
        Some(CallingContext {
            user_mri: self.identity.mri.clone(),
            display_name: self.identity.name.clone(),
            tenant_id: self.identity.tenant.clone(),
            region: routing.region,
            partition: routing.partition,
            ring: routing.ring,
        })
    }
}

/// AAD resource for a SharePoint/OneDrive or Graph URL: its origin, one token per host.
fn resource_audience(resource: &Url) -> Result<String, AuthError> {
    let host = resource.host_str().unwrap_or("");
    let sharepoint = [
        ".sharepoint.com",
        ".sharepoint.us",
        ".sharepoint.cn",
        ".sharepoint.de",
    ]
    .iter()
    .any(|suffix| host.ends_with(suffix));
    if resource.scheme() != "https"
        || resource.port_or_known_default() != Some(443)
        || !resource.username().is_empty()
        || resource.password().is_some()
        || !(sharepoint || host == "graph.microsoft.com")
    {
        return Err(failed(t("auth.login.invalid_sharepoint_resource")));
    }
    Ok(resource.origin().ascii_serialization())
}

fn insert_bearer(headers: &mut HeaderMap, aad: &str) -> Result<(), AuthError> {
    insert_secret(headers, "authorization", &format!("Bearer {aad}"))
}
fn insert_secret(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), AuthError> {
    let mut value = HeaderValue::from_str(value).map_err(|_| {
        failed(t("auth.login.invalid_authentication_header"))
    })?;
    value.set_sensitive(true);
    headers.insert(name, value);
    Ok(())
}
/// Wall clock, as the server means it: `Instant` stops while the Mac sleeps, so a token would still look fresh after a night.
fn expiry(seconds: u64) -> Result<SystemTime, AuthError> {
    if seconds == 0 {
        return Err(failed(t("auth.login.token_already_expired")));
    }
    SystemTime::now()
        .checked_add(Duration::from_secs(seconds))
        .ok_or_else(|| {
            failed(t("auth.login.invalid_token_expiration"))
        })
}
fn service_urls(table: HashMap<String, String>) -> Result<HashMap<String, Url>, AuthError> {
    let mut services = HashMap::new();
    for (key, value) in table {
        // regionGtms also holds UDP transports and opaque settings: keep only consumed services.
        if !matches!(
            key.as_str(),
            "chatService"
                | "chatServiceAfd"
                | "chatSvcAggAfd"
                | "teamsAndChannelsService"
                | "middleTier"
                | "mtImageService"
                | "unifiedPresence"
                | "presenceUPS"
                | "ams"
                | "amsV2"
                | "urlp"
                | "urlpV2"
                | "calling_conversationServiceUrl"
                | "calling_conversationServiceBaseUrl"
                | "calling_callControllerServiceUrl"
                | "calling_trouterUrl"
                | "calling_registrarUrl"
                | "userIntelligenceService"
        ) {
            continue;
        }
        let mut url = Url::parse(&value).map_err(|_| {
            failed(tf!("auth.login.invalid_regional_url_for", key = key))
        })?;
        if key == "calling_trouterUrl" {
            url = trouter_allocation_url(url);
        }
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(failed(tf!("auth.login.regional_url_for_isn_t_https", key = key)));
        }
        services.insert(key, url);
    }
    if !services.contains_key("chatServiceAfd")
        && !services.contains_key(Service::ChatService.key())
    {
        return Err(AuthError::UnknownService(Service::ChatService));
    }
    Ok(services)
}
/// The calling stack allocates Trouter sessions over HTTPS on `/v4/a`, whatever the advertised `wss://…/v3/c`.
fn trouter_allocation_url(mut url: Url) -> Url {
    if url.scheme() == "wss" {
        let _ = url.set_scheme("https");
    }
    if url.path().trim_end_matches('/').ends_with("/v3/c") {
        let path = url.path().trim_end_matches('/');
        let path = format!("{}/v4/a", &path[..path.len() - "/v3/c".len()]);
        url.set_path(&path);
    }
    url
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
    /// Seconds, as a number or a string (MSAL accepts both).
    #[serde(default, deserialize_with = "optional_seconds")]
    refresh_token_expires_in: Option<u64>,
}
fn optional_seconds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => s.parse().ok(),
        _ => None,
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    tokens: SkypeToken,
    region_gtms: HashMap<String, String>,
    #[serde(flatten)]
    routing: HashMap<String, serde_json::Value>,
}
impl Discovery {
    fn region_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (key, name) in [
            ("region", "x-ms-region"),
            ("userRegion", "x-ms-user-region"),
            ("partition", "x-ms-partition"),
            ("userPartition", "x-ms-user-partition"),
        ] {
            if let Some(value) = self.routing.get(key).and_then(|v| v.as_str())
                && let Ok(value) = HeaderValue::from_str(value)
            {
                headers.insert(name, value);
            }
        }
        headers
    }
    fn calling_routing(&self) -> CallingRouting {
        let get = |key: &str| {
            self.routing
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        CallingRouting {
            region: get("region"),
            partition: get("partition"),
            ring: get("ring").or_else(|| {
                self.routing
                    .get("regionSettings")
                    .and_then(|s| s.get("ring"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            }),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkypeToken {
    #[serde(alias = "skypetoken")]
    skype_token: String,
    expires_in: u64,
}

async fn token_request(
    http: &reqwest::Client,
    protocol: &Protocol,
    fields: &[(&str, &str)],
) -> Result<TokenResponse, AuthError> {
    let mut fields = fields.to_vec();
    fields.push(("client_id", protocol.client_id.as_str()));
    let response = http
        .post(protocol.authority.join("oauth2/v2.0/token").unwrap())
        .header("origin", ORIGIN)
        .form(&fields)
        .timeout(AUTH_TIMEOUT)
        .send()
        .await
        .map_err(|_| {
            failed(t("auth.login.couldn_t_reach_microsoft_for_the"))
        })?;
    let status = response.status();
    if !status.is_success() {
        // Only inspect stable OAuth error codes; never include raw response data.
        if let Ok(error) = response.json::<serde_json::Value>().await {
            if matches!(
                error.get("error").and_then(|v| v.as_str()),
                Some(
                    "invalid_grant"
                        | "interaction_required"
                        | "login_required"
                        | "consent_required"
                )
            ) {
                return Err(AuthError::SessionExpired);
            }
        }
        return Err(failed(tf!("auth.login.microsoft_token_http_please_sign_in", status = status)));
    }
    response
        .json()
        .await
        .map_err(|_| failed(t("auth.login.invalid_aad_response")))
}
async fn discover(
    http: &reqwest::Client,
    protocol: &Protocol,
    aad: &str,
    refresh: bool,
) -> Result<Discovery, AuthError> {
    let url = protocol.authz.clone();
    let id = Uuid::new_v4().to_string();
    let mut headers = HeaderMap::new();
    insert_bearer(&mut headers, aad)?;
    let response = http
        .post(url)
        .headers(headers)
        .header(
            "ms-teams-authz-type",
            if refresh {
                "TokenRefresh"
            } else {
                "ExplicitLogin"
            },
        )
        .header("claimschallengecapable", "true")
        .header("x-ms-migration", "True")
        .header("clientrequestid", &id)
        .header("x-ms-request-id", &id)
        .header("x-ms-client-type", "web")
        .header("x-ms-client-version", crate::CLIENT_BUILD)
        .header("x-client-ui-language", "fr-fr")
        .timeout(AUTH_TIMEOUT)
        .send()
        .await
        .map_err(|_| {
            failed(t("auth.login.couldn_t_reach_teams_authz"))
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(failed(tf!("auth.login.teams_authz_http_please_sign_in", status = status)));
    }
    response
        .json()
        .await
        .map_err(|_| failed(t("auth.login.invalid_authz_response")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn file_resources_get_their_own_origin_as_audience() {
        let audience = |u: &str| resource_audience(&Url::parse(u).unwrap()).ok();
        assert_eq!(
            audience("https://contoso-my.sharepoint.com/personal/a/_api/v2.0/drive").as_deref(),
            Some("https://contoso-my.sharepoint.com")
        );
        assert_eq!(
            audience("https://graph.microsoft.com/v1.0/me/drive").as_deref(),
            Some("https://graph.microsoft.com")
        );
        assert!(audience("http://contoso.sharepoint.com/").is_none());
        assert!(audience("https://contoso.sharepoint.com.evil.example/").is_none());
        assert!(audience("https://contoso.sharepoint.com:8443/").is_none());
    }

    #[test]
    fn substrate_is_anchored_on_the_user_mailbox() {
        let id = Identity {
            mri: "8:orgid:oid-1".into(),
            name: String::new(),
            tenant: Some("tid-2".into()),
            login_hint: None,
        };
        assert_eq!(id.anchor_mailbox().as_deref(), Some("Oid:oid-1@tid-2"));
        assert_eq!(
            Identity {
                tenant: None,
                ..id
            }
            .anchor_mailbox(),
            None
        );
    }

    #[test]
    fn identity_normalizes_orgid_to_the_member_and_message_mri() {
        for claim in ["userId", "skypeid"] {
            for id in ["alice", "orgid:alice", "8:orgid:alice"] {
                let token = format!(
                    "e30.{}.signature",
                    URL_SAFE_NO_PAD
                        .encode(serde_json::to_vec(&json!({claim: id, "name": "Alice"})).unwrap())
                );
                let identity = identity(&token).unwrap();
                assert_eq!(identity.mri, "8:orgid:alice");
                assert_eq!(identity.name, "Alice");
            }
        }
    }

    #[test]
    fn a_token_without_a_name_never_uses_the_routing_id_as_a_display_name() {
        let token = format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(br#"{"userId":"orgid:alice"}"#)
        );
        let user = identity(&token).unwrap();
        assert_eq!(user.mri, "8:orgid:alice");
        assert!(user.name.is_empty());
    }
    fn jwt(marker: &str) -> String {
        format!(
            "e30.{}.{}",
            URL_SAFE_NO_PAD.encode(br#"{"userId":"alice","name":"Alice"}"#),
            marker
        )
    }
    fn callback(login: &LoginAttempt) -> String {
        format!("{REDIRECT}#code=test-code&state={}", login.state)
    }
    const CLIENT_ID: &str = "0c6e1f4a-3b1d-4c8e-9f2a-7d5b6a4c3e21";
    fn new_login() -> LoginAttempt {
        LoginAttempt::with_client_id(CLIENT_ID.into())
    }
    fn attempt(server: &MockServer) -> LoginAttempt {
        let mut login = new_login();
        login.protocol = Protocol {
            authority: Url::parse(&format!("{}/", server.uri())).unwrap(),
            authz: Url::parse(&format!("{}/authz", server.uri())).unwrap(),
            client_id: CLIENT_ID.into(),
        };
        login
    }
    fn authz_body(token: &str) -> serde_json::Value {
        json!({"tokens": {"skypetoken": token, "expiresIn": 3600}, "region": "emea", "regionGtms": {
            "chatService": "https://chat.example/base/", "middleTier": "https://mt.example/",
            "teamsAndChannelsService": "https://csa.example/", "unifiedPresence": "https://presence.example/", "ams": "https://ams.example/"
        }})
    }
    async fn signed_in(server: &MockServer) -> TeamsAuthenticator {
        Mock::given(method("POST"))
            .and(path("/oauth2/v2.0/token"))
            .and(header("origin", ORIGIN))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("code=test-code"))
            .and(body_string_contains("code_verifier="))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token":"aad", "expires_in":3600, "refresh_token":"refresh"}),
            ))
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/authz"))
            .and(header("authorization", "Bearer aad"))
            .and(header("ms-teams-authz-type", "ExplicitLogin"))
            .and(header("claimschallengecapable", "true"))
            .and(header("x-ms-migration", "True"))
            .respond_with(ResponseTemplate::new(200).set_body_json(authz_body(&jwt("initial"))))
            .expect(1)
            .mount(server)
            .await;
        let login = attempt(server);
        let callback = callback(&login);
        let auth = login.complete(&callback).await.unwrap();
        auth.session.write().unwrap().chat_mode =
            (ChatMode::Skype, Instant::now() + Duration::from_secs(86400));
        auth
    }

    #[tokio::test]
    async fn calendar_unauthorized_refreshes_only_the_spaces_aad_token() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        Mock::given(method("POST"))
            .and(path("/oauth2/v2.0/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("api.spaces.skype.com"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"new-calendar-aad", "expires_in":3600, "refresh_token":"rotated"})))
            .expect(1).mount(&server).await;
        assert!(auth.on_unauthorized(Service::Calendar).await);
        let mut headers = HeaderMap::new();
        auth.authorize(Service::Calendar, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer new-calendar-aad");
        assert!(!headers.contains_key("x-skypetoken"));
        assert_eq!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.url.path() == "/authz")
                .count(),
            1
        );
    }

    #[test]
    fn pkce_and_callback_validation() {
        let login = new_login();
        let url = login.authorization_url();
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["client_id"], CLIENT_ID);
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(
            params["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(login.verifier.as_bytes()))
        );
        assert!(!params.contains_key("code_verifier"));
        assert!(params["scope"].contains("offline_access"));
        assert_ne!(params["state"], new_login().state);
        assert_eq!(login.callback_code(&callback(&login)).unwrap(), "test-code");
        assert!(
            login
                .callback_code(&callback(&login).replace("teams.microsoft.com", "attacker.example"))
                .is_err()
        );
        assert!(
            login
                .callback_code(&format!("{REDIRECT}#code=x&state=wrong"))
                .is_err()
        );
        assert!(
            login
                .callback_code(&format!("{}&state=duplicate", callback(&login)))
                .is_err()
        );
        assert!(
            login
                .callback_code(&format!(
                    "{REDIRECT}#error=access_denied&state={}",
                    login.state
                ))
                .is_err()
        );
        assert!(
            login
                .callback_code(&format!("{REDIRECT}#state={}", login.state))
                .is_err()
        );
    }

    #[tokio::test]
    async fn exchange_discovery_and_service_headers() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        assert_eq!(auth.user_id(), "8:orgid:alice");
        assert_eq!(auth.display_name(), "Alice");
        assert_eq!(
            auth.base_url(Service::ChatService).unwrap().as_str(),
            "https://chat.example/base/"
        );
        let mut headers = HeaderMap::new();
        auth.authorize(Service::ChatService, &mut headers)
            .await
            .unwrap();
        assert_eq!(
            headers["authentication"],
            format!("skypetoken={}", jwt("initial"))
        );
        assert!(headers["authentication"].is_sensitive());
        assert!(!headers.contains_key("authorization"));
        let mut headers = HeaderMap::new();
        auth.authorize(Service::MiddleTier, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer aad");
        assert_eq!(headers["x-skypetoken"], jwt("initial"));
        let mut calendar = HeaderMap::new();
        auth.authorize(Service::Calendar, &mut calendar)
            .await
            .unwrap();
        assert_eq!(calendar["authorization"], "Bearer aad");
        assert_eq!(calendar["x-ms-migration"], "True");
        assert!(!calendar.contains_key("x-skypetoken"));
        let mut image_headers = HeaderMap::new();
        auth.authorize(Service::ProfileImages, &mut image_headers)
            .await
            .unwrap();
        assert_eq!(image_headers["authorization"], "Bearer aad");
        assert_eq!(image_headers["x-ms-migration"], "True");
        assert!(!image_headers.contains_key("x-skypetoken"));
        assert_eq!(
            auth.base_url(Service::ProfileImages).unwrap(),
            auth.base_url(Service::MiddleTier).unwrap()
        );

        assert_eq!(
            auth.base_url(Service::Calendar).unwrap(),
            auth.base_url(Service::MiddleTier).unwrap()
        );
        let requests = server.received_requests().await.unwrap();
        let authz = requests.iter().find(|r| r.url.path() == "/authz").unwrap();
        assert!(authz.body.is_empty());
        assert_eq!(
            authz.headers["clientrequestid"],
            authz.headers["x-ms-request-id"]
        );
        assert!(Uuid::parse_str(authz.headers["clientrequestid"].to_str().unwrap()).is_ok());
        assert!(!format!("{auth:?}").contains("initial"));
    }

    #[tokio::test]
    async fn registration_token_is_captured_and_expiry_removed() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        let mut response = HeaderMap::new();
        response.insert(
            "set-registrationtoken",
            HeaderValue::from_static("registrationToken=abc; expires=1234; endpointId=endpoint"),
        );
        auth.observe_response(Service::ChatService, &response)
            .unwrap();
        let mut headers = HeaderMap::new();
        auth.authorize(Service::ChatService, &mut headers)
            .await
            .unwrap();
        assert_eq!(
            headers["registrationtoken"],
            "registrationToken=abc; endpointId=endpoint"
        );
        assert!(headers["registrationtoken"].is_sensitive());
    }

    async fn mock_refresh(server: &MockServer, resource: &str, access: &str) {
        let encoded_scope: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("scope", &scope(resource))
            .finish();
        Mock::given(method("POST"))
            .and(path("/oauth2/v2.0/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=refresh"))
            .and(body_string_contains(encoded_scope))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token":access,"expires_in":3600,"refresh_token":"rotated"}),
            ))
            .expect(1)
            .mount(server)
            .await;
    }

    #[test]
    fn silent_attempt_asks_for_no_page_and_names_the_account() {
        let url = LoginAttempt {
            prompt: "none",
            login_hint: Some("alice@contoso.com".into()),
            ..new_login()
        }
        .authorization_url();
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["prompt"], "none");
        assert_eq!(params["login_hint"], "alice@contoso.com");
        let params: HashMap<_, _> = new_login()
            .authorization_url()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(params["prompt"], "select_account");
        assert!(!params.contains_key("login_hint"));
    }

    #[test]
    fn refresh_token_lifetime_is_read_as_a_number_or_a_string() {
        for body in [
            json!({"access_token":"a","expires_in":3600,"refresh_token":"r","refresh_token_expires_in":86399}),
            json!({"access_token":"a","expires_in":3600,"refresh_token":"r","refresh_token_expires_in":"86399"}),
        ] {
            let token: TokenResponse = serde_json::from_value(body).unwrap();
            assert_eq!(token.refresh_token_expires_in, Some(86399));
            assert!(refresh_expiry(&token).is_some());
        }
        let token: TokenResponse =
            serde_json::from_value(json!({"access_token":"a","expires_in":3600})).unwrap();
        assert_eq!(refresh_expiry(&token), None);
    }

    #[tokio::test]
    async fn renewal_is_due_near_the_refresh_token_end_or_once_it_is_refused() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        // The mock gives no lifetime: nothing to anticipate.
        assert!(!auth.needs_renewal(Duration::from_secs(3600)));
        auth.session.write().unwrap().refresh_expires =
            Some(SystemTime::now() + Duration::from_secs(1800));
        assert!(auth.needs_renewal(Duration::from_secs(3600)));
        assert!(!auth.needs_renewal(Duration::from_secs(600)));
        auth.session.write().unwrap().refresh_expires = None;
        auth.session.write().unwrap().tokens.clear();
        Mock::given(method("POST"))
            .and(path("/oauth2/v2.0/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error":"invalid_grant"})))
            .mount(&server)
            .await;
        assert!(matches!(
            auth.access_token(SPACES, false).await,
            Err(AuthError::SessionExpired)
        ));
        assert!(auth.needs_renewal(Duration::ZERO));
    }

    #[tokio::test]
    async fn adopting_a_renewed_sign_in_keeps_the_authenticator_and_its_store() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        auth.refused.store(true, Ordering::SeqCst);
        let renewed = signed_in(&MockServer::start().await).await;
        renewed.session.write().unwrap().refresh_token = Some("renewed".into());
        auth.adopt(renewed).await.unwrap();
        assert_eq!(
            auth.session.read().unwrap().refresh_token.as_deref(),
            Some("renewed")
        );
        assert!(!auth.needs_renewal(Duration::ZERO));
        let other = signed_in(&MockServer::start().await).await;
        let other = TeamsAuthenticator {
            identity: Identity {
                mri: "8:orgid:bob".into(),
                name: String::new(),
                tenant: None,
                login_hint: None,
            },
            ..other
        };
        assert!(auth.adopt(other).await.is_err());
    }

    #[tokio::test]
    async fn refresh_on_expiration_is_shared_and_rotates_refresh_token() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        {
            let mut s = auth.session.write().unwrap();
            s.skype_expires = SystemTime::now();
            s.tokens.get_mut(SPACES).unwrap().expires = SystemTime::now();
        }
        mock_refresh(&server, SPACES, "new-aad").await;
        Mock::given(method("POST"))
            .and(path("/authz"))
            .and(header("ms-teams-authz-type", "TokenRefresh"))
            .and(header("authorization", "Bearer new-aad"))
            .respond_with(ResponseTemplate::new(200).set_body_json(authz_body(&jwt("renewed"))))
            .expect(1)
            .mount(&server)
            .await;
        let mut first = HeaderMap::new();
        let mut second = HeaderMap::new();
        let (a, b) = tokio::join!(
            auth.authorize(Service::ChatService, &mut first),
            auth.authorize(Service::ChatService, &mut second)
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(
            first["authentication"],
            format!("skypetoken={}", jwt("renewed"))
        );
        assert_eq!(first, second);
        assert_eq!(
            auth.session.read().unwrap().refresh_token.as_deref(),
            Some("rotated")
        );
    }

    #[tokio::test]
    async fn unauthorized_forces_aad_and_skype_refresh_and_clears_registration() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        auth.session.write().unwrap().registration =
            Some(HeaderValue::from_static("registrationToken=old"));
        mock_refresh(&server, SPACES, "new-aad").await;
        Mock::given(method("POST"))
            .and(path("/authz"))
            .and(header("ms-teams-authz-type", "TokenRefresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(authz_body(&jwt("renewed"))))
            .expect(1)
            .mount(&server)
            .await;
        assert!(auth.on_unauthorized(Service::ChatService).await);
        assert!(auth.session.read().unwrap().registration.is_none());
    }

    #[tokio::test]
    async fn duplicate_skype_token_triggers_force_renew() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        auth.session.write().unwrap().skype_expires = SystemTime::now();
        Mock::given(method("POST"))
            .and(path("/authz"))
            .and(header("ms-teams-authz-type", "TokenRefresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(authz_body(&jwt("initial"))))
            .with_priority(10)
            .expect(1)
            .mount(&server)
            .await;
        mock_refresh(&server, SPACES, "forced-aad").await;
        Mock::given(method("POST"))
            .and(path("/authz"))
            .and(header("authorization", "Bearer forced-aad"))
            .respond_with(ResponseTemplate::new(200).set_body_json(authz_body(&jwt("renewed"))))
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        let mut headers = HeaderMap::new();
        auth.authorize(Service::ChatService, &mut headers)
            .await
            .unwrap();
        assert_eq!(
            headers["authentication"],
            format!("skypetoken={}", jwt("renewed"))
        );
    }

    #[tokio::test]
    async fn csa_gets_its_own_resource_without_skype_token() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        mock_refresh(&server, CSA, "csa-aad").await;
        let mut headers = HeaderMap::new();
        auth.authorize(Service::TeamsAndChannels, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer csa-aad");
        assert!(!headers.contains_key("x-skypetoken"));
        assert!(!headers.contains_key("authentication"));
    }

    #[tokio::test]
    async fn ams_migrates_to_ic3() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        mock_refresh(&server, IC3, "ic3-aad").await;
        let mut headers = HeaderMap::new();
        auth.authorize(Service::Ams, &mut headers).await.unwrap();
        assert_eq!(headers["authorization"], "Bearer ic3-aad");
        assert_eq!(headers["x-ms-migration"], "True");
        assert_eq!(headers["x-ms-test-user"], "False");
    }

    #[tokio::test]
    async fn ams_falls_back_to_skype_if_ic3_fails() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        Mock::given(method("POST"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;
        let mut headers = HeaderMap::new();
        auth.authorize(Service::Ams, &mut headers).await.unwrap();
        assert_eq!(
            headers["authorization"],
            format!("skype_token {}", jwt("initial"))
        );
        assert!(!headers.contains_key("x-ms-migration"));
    }

    #[tokio::test]
    async fn errors_never_expose_raw_token_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("secret-access-token"))
            .mount(&server)
            .await;
        let login = attempt(&server);
        let callback = callback(&login);
        let error = login.complete(&callback).await.unwrap_err().to_string();
        assert!(error.contains("400"));
        assert!(!error.contains("secret-access-token"));
        assert!(!error.contains("test-code"));
    }

    #[test]
    fn discovery_requires_https_chat_and_valid_expiration() {
        assert!(service_urls(HashMap::new()).is_err());
        assert!(
            service_urls(HashMap::from([(
                "chatService".into(),
                "http://chat.example/".into()
            )]))
            .is_err()
        );
        assert!(expiry(0).is_err());
        assert!(identity("not-a-jwt").is_err());
    }

    #[test]
    fn discovery_keeps_calling_services_and_ignores_other_transports() {
        let services = service_urls(HashMap::from([
            ("chatService".into(), "https://chat.example/".into()),
            ("chatSvcAggAfd".into(), "https://csa.example/".into()),
            (
                "calling_udpTransportUrl".into(),
                "udp://media.example:3478".into(),
            ),
            (
                "calling_trouterUrl".into(),
                "wss://trouter.example/v3/c".into(),
            ),
            (
                "calling_conversationServiceUrl".into(),
                "https://conv.example/api/v2/epconv/".into(),
            ),
            ("futureTransport".into(), "not an HTTP URL".into()),
        ]))
        .unwrap();
        assert_eq!(services.len(), 4);
        assert_eq!(services["chatService"].as_str(), "https://chat.example/");
        assert_eq!(services["chatSvcAggAfd"].as_str(), "https://csa.example/");
        assert_eq!(
            services["calling_trouterUrl"].as_str(),
            "https://trouter.example/v4/a"
        );
        assert_eq!(
            services["calling_conversationServiceUrl"].as_str(),
            "https://conv.example/api/v2/epconv/"
        );
    }

    #[test]
    fn only_trouter_may_be_advertised_as_a_websocket() {
        let table = |key: &str, value: &str| {
            HashMap::from([
                ("chatService".into(), "https://chat.example/".into()),
                (key.to_owned(), value.to_owned()),
            ])
        };
        assert!(service_urls(table("calling_registrarUrl", "wss://reg.example/")).is_err());
        assert!(service_urls(table("calling_trouterUrl", "ws://trouter.example/v3/c")).is_err());
        assert_eq!(
            service_urls(table("calling_trouterUrl", "https://t.example/v4/a"))
                .unwrap()["calling_trouterUrl"]
                .as_str(),
            "https://t.example/v4/a"
        );
    }

    #[tokio::test]
    async fn calling_services_fall_back_to_web_client_defaults() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        assert_eq!(
            auth.base_url(Service::CallingConversation)
                .unwrap()
                .as_str(),
            "https://api.flightproxy.teams.microsoft.com/api/v2/epconv/"
        );
        assert_eq!(
            auth.base_url(Service::Trouter).unwrap().as_str(),
            "https://go.trouter.teams.microsoft.com/v4/a"
        );
        assert_eq!(
            auth.base_url(Service::Registrar).unwrap().as_str(),
            "https://teams.microsoft.com/registrar/prod/v2/registrations"
        );
    }

    #[tokio::test]
    async fn calling_uses_cae_then_negotiates_with_token_types() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        mock_refresh(&server, IC3, "ic3-calling").await;
        let mut headers = HeaderMap::new();
        auth.authorize(Service::CallingConversation, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer ic3-calling");
        assert_eq!(headers["x-ms-migration"], "True");
        assert!(!headers.contains_key("x-skypetoken"));

        let mut challenge = HeaderMap::new();
        challenge.insert(
            "www-authenticate",
            HeaderValue::from_static("Bearer error=\"protocol_mismatch\", token_types=\"skype\""),
        );
        assert!(auth.on_challenge(Service::Registrar, &challenge).await);
        let mut headers = HeaderMap::new();
        auth.authorize(Service::Registrar, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["x-skypetoken"], jwt("initial"));
        assert!(headers["x-skypetoken"].is_sensitive());
        assert!(!headers.contains_key("authorization"));
        // Negotiation is per service: the conversation service keeps its bearer.
        let mut headers = HeaderMap::new();
        auth.authorize(Service::CallingConversation, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer ic3-calling");
    }

    #[test]
    fn calling_challenges_pick_the_next_acceptable_token() {
        use ChatMode::*;
        let pick = |c: &str, m| calling_challenge(c, m);
        assert!(pick("Bearer token_types=\"aad,cae,skype\"", Cae).is_none());
        assert!(matches!(
            pick("Bearer token_types=\"aad,skype\"", Cae),
            Some(NonCae)
        ));
        assert!(matches!(
            pick("Bearer token_types=\"Skype\"", Cae),
            Some(Skype)
        ));
        assert!(matches!(
            pick("Bearer error=\"insufficient_claims\"", Cae),
            Some(NonCae)
        ));
        assert!(pick("Bearer realm=\"x\"", Cae).is_none());
    }

    #[tokio::test]
    async fn outlook_and_calling_context_come_from_aad_claims() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        mock_refresh(&server, OUTLOOK, "outlook-aad").await;
        let mut headers = HeaderMap::new();
        auth.authorize(Service::Outlook, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer outlook-aad");
        assert!(!headers.contains_key("x-skypetoken"));
        let context = auth.calling_context().unwrap();
        assert_eq!(context.user_mri, "8:orgid:alice");
        assert_eq!(context.display_name, "Alice");
        assert_eq!(context.region.as_deref(), Some("emea"));
        assert_eq!(
            tenant(&format!(
                "e30.{}.sig",
                URL_SAFE_NO_PAD.encode(br#"{"tid":"tenant-1"}"#)
            ))
            .as_deref(),
            Some("tenant-1")
        );
    }

    #[test]
    fn discovery_still_rejects_invalid_urls_for_consumed_http_services() {
        for key in [
            "chatService",
            "chatServiceAfd",
            "chatSvcAggAfd",
            "teamsAndChannelsService",
            "middleTier",
            "unifiedPresence",
            "ams",
        ] {
            for value in [
                "http://service.example/",
                "udp://service.example:3478",
                "https://user:password@service.example/",
                "invalid",
            ] {
                let mut table =
                    HashMap::from([("chatService".into(), "https://chat.example/".into())]);
                table.insert(key.into(), value.into());
                assert!(
                    service_urls(table).is_err(),
                    "accepted invalid URL for {key}"
                );
            }
        }
    }
    #[tokio::test]
    async fn chat_service_uses_ic3_by_default_and_negotiates_skype() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        auth.session.write().unwrap().chat_mode =
            (ChatMode::Cae, Instant::now() + Duration::from_secs(86400));
        mock_refresh(&server, IC3, "ic3-chat").await;
        let mut headers = HeaderMap::new();
        auth.authorize(Service::ChatService, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer ic3-chat");
        assert_eq!(headers["x-ms-migration"], "True");
        assert!(!headers.contains_key("authentication"));
        let mut challenge = HeaderMap::new();
        challenge.insert(
            "www-authenticate",
            HeaderValue::from_static("Bearer token_types=\"Skype\""),
        );
        assert!(auth.on_challenge(Service::ChatService, &challenge).await);
        let mut headers = HeaderMap::new();
        auth.authorize(Service::ChatService, &mut headers)
            .await
            .unwrap();
        assert!(headers.contains_key("authentication"));
        assert!(!headers.contains_key("authorization"));
    }
    #[tokio::test]
    async fn aad_challenge_uses_non_cae_and_discovery_resolves_correct_hosts() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        let mut challenge = HeaderMap::new();
        challenge.insert(
            "www-authenticate",
            HeaderValue::from_static("Bearer token_types=\"AAD,Skype\""),
        );
        assert!(auth.on_challenge(Service::ChatService, &challenge).await);
        mock_refresh(&server, IC3_NON_CAE, "non-cae").await;
        let mut headers = HeaderMap::new();
        auth.authorize(Service::ChatService, &mut headers)
            .await
            .unwrap();
        assert_eq!(headers["authorization"], "Bearer non-cae");
        let mut session = auth.session.write().unwrap();
        session.services.insert(
            "chatSvcAggAfd".into(),
            Url::parse("https://csa.region.example/").unwrap(),
        );
        session.services.insert(
            "chatServiceAfd".into(),
            Url::parse("https://chat.afd.example/").unwrap(),
        );
        drop(session);
        assert_eq!(
            auth.base_url(Service::Csa).unwrap().as_str(),
            "https://csa.region.example/"
        );
        assert_eq!(
            auth.base_url(Service::ChatService).unwrap().as_str(),
            "https://chat.afd.example/"
        );
    }
    #[derive(Default)]
    struct MemoryStore(std::sync::Mutex<(bool, Option<String>)>);
    impl RefreshTokenStore for MemoryStore {
        fn save(&self, token: &str) -> Result<(), AuthError> {
            let mut s = self.0.lock().unwrap();
            if !s.0 {
                s.1 = Some(token.into());
            }
            Ok(())
        }
        fn clear(&self) -> Result<(), AuthError> {
            *self.0.lock().unwrap() = (true, None);
            Ok(())
        }
    }
    #[tokio::test]
    async fn rotated_credentials_are_persisted_and_logout_cannot_resurrect_them() {
        let server = MockServer::start().await;
        let auth = signed_in(&server).await;
        let store = Arc::new(MemoryStore::default());
        auth.remember_session(store.clone()).unwrap();
        assert_eq!(store.0.lock().unwrap().1.as_deref(), Some("refresh"));
        Mock::given(method("POST"))
            .and(path("/oauth2/v2.0/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token":"next","expires_in":3600,"refresh_token":"rotated"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        auth.access_token(IC3, true).await.unwrap();
        assert_eq!(store.0.lock().unwrap().1.as_deref(), Some("rotated"));
        auth.forget_session().unwrap();
        assert!(auth.remember_session(store.clone()).is_err());
        store.save("late in-flight token").unwrap();
        auth.persist_refresh().unwrap();
        assert!(store.0.lock().unwrap().1.is_none());
    }
    #[tokio::test]
    async fn restore_refreshes_credentials_and_rebuilds_service_discovery() {
        for rotated in [true, false] {
            let server = MockServer::start().await;
            let mut response = json!({"access_token":"restored-aad","expires_in":3600});
            if rotated {
                response["refresh_token"] = json!("rotated");
            }
            Mock::given(method("POST"))
                .and(path("/oauth2/v2.0/token"))
                .and(body_string_contains("grant_type=refresh_token"))
                .and(body_string_contains("refresh_token=saved"))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/authz"))
                .and(header("authorization", "Bearer restored-aad"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(authz_body(&jwt("restored"))),
                )
                .expect(1)
                .mount(&server)
                .await;
            let auth =
                TeamsAuthenticator::restore_with_protocol("saved", attempt(&server).protocol)
                    .await
                    .unwrap();
            let store = Arc::new(MemoryStore::default());
            auth.remember_session(store.clone()).unwrap();
            assert_eq!(auth.user_id(), "8:orgid:alice");
            assert_eq!(
                auth.base_url(Service::ChatService).unwrap().as_str(),
                "https://chat.example/base/"
            );
            assert_eq!(
                store.0.lock().unwrap().1.as_deref(),
                Some(if rotated { "rotated" } else { "saved" })
            );
        }
    }
    #[tokio::test]
    async fn only_definitively_expired_refresh_tokens_invalidate_the_saved_session() {
        for (status, body, expired) in [
            (
                400,
                json!({"error":"invalid_grant","error_description":"secret"}),
                true,
            ),
            (503, json!({"error":"temporarily_unavailable"}), false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/oauth2/v2.0/token"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .mount(&server)
                .await;
            let e = TeamsAuthenticator::restore_with_protocol(
                "saved-secret",
                attempt(&server).protocol,
            )
            .await
            .unwrap_err();
            assert_eq!(matches!(e, AuthError::SessionExpired), expired);
            assert!(!e.to_string().contains("secret"));
        }
    }
}
