//! Authentication seam: regional base URLs come from the session, [`TeamsAuthenticator`] does PKCE, authz discovery and refresh.

pub mod client_id;
pub mod diagnostics;
mod login;
pub mod net;
pub mod web_config;
pub use login::{LoginAttempt, RefreshTokenStore, TeamsAuthenticator};

use std::collections::HashMap;
use std::future::Future;

use http::HeaderMap;
use teams_i18n::{t, tf};
use url::Url;

macro_rules! build {
    () => {
        "26261.303.5165.1465"
    };
}
/// Build of the Teams web client whose requests are mirrored (`x-ms-client-version` of most services).
pub const CLIENT_BUILD: &str = build!();
/// `<platformId>/<build>` of that client, as calendar, calling and Trouter send it.
pub const CLIENT_VERSION: &str = concat!("1415/", build!());

/// Backend services the client talks to, keyed like the session's service table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Service {
    /// Messaging: `v1/users/ME/...`, `v1/threads/...`, `v2/users/ME/endpoints`.
    ChatService,
    /// Teams, channels and chat lists (CSA / chatsvcagg).
    TeamsAndChannels,
    /// CSA startup, chats, teams and channel posts.
    Csa,
    /// Profiles, avatars, settings (`api/mt/{region}`).
    MiddleTier,
    /// Calendar MT with STM bearer authentication, without a Skype token.
    Calendar,
    /// Authenticated profile pictures on the regional MT image service.
    ProfileImages,
    /// Presence.
    UnifiedPresence,
    /// Media storage: images, attachments, URL preview images.
    Ams,
    /// Link previews (`regionGtms.urlpV2`, else `urlp`), with AMS credentials.
    UrlPreview,
    /// Calling conversation service (NGC): start, join, then hypermedia links.
    CallingConversation,
    /// Regional `api.conv.skype.com` base (meeting role, captcha).
    CallingConversationBase,
    /// Trouter push channel used by the calling stack (`/v4/a` allocation).
    Trouter,
    /// Push registrar binding the Trouter session to this endpoint.
    Registrar,
    /// TURN/STUN relay credentials (TRAP).
    Trap,
    /// Outlook REST, where voicemail lives.
    Outlook,
    /// Substrate (`substrate.office.com`): files and links shared in a chat, with the Outlook audience.
    Substrate,
    /// Substrate search (`searchservice/api/v2/query`), with the `substrateTokenUrl` audience.
    SubstrateSearch,
    /// Per-channel notification settings (NSS, `regionGtms.userIntelligenceService`).
    Uis,
    /// The meeting recorder bot's `commandUrl` (from the roster): AAD bearer for `callRecording.resource`.
    CallRecorder,
}

impl Service {
    /// Services of the calling stack, which share STM negotiation.
    pub fn is_calling(self) -> bool {
        matches!(
            self,
            Service::CallingConversation
                | Service::CallingConversationBase
                | Service::Trouter
                | Service::Registrar
                | Service::Trap
        )
    }

    /// Key of this service in the session's service table.
    pub fn key(self) -> &'static str {
        match self {
            Service::ChatService => "chatService",
            Service::TeamsAndChannels => "teamsAndChannelsService",
            Service::Csa => "chatSvcAggAfd",
            Service::MiddleTier | Service::Calendar => "middleTier",
            Service::ProfileImages => "mtImageService",
            Service::UnifiedPresence => "unifiedPresence",
            Service::Ams => "ams",
            Service::UrlPreview => "urlpV2",
            Service::CallingConversation => "calling_conversationServiceUrl",
            Service::CallingConversationBase => "calling_conversationServiceBaseUrl",
            Service::Trouter => "calling_trouterUrl",
            Service::Registrar => "calling_registrarUrl",
            Service::Trap => "trap",
            Service::Outlook => "outlook",
            Service::Substrate => "substrate",
            Service::SubstrateSearch => "substrateSearch",
            Service::Uis => "userIntelligenceService",
            Service::CallRecorder => "callRecorder",
        }
    }
}

/// Who is calling and where their calls are routed, for calling payloads and headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallingContext {
    /// `8:orgid:<oid>`.
    pub user_mri: String,
    pub display_name: String,
    /// AAD tenant (`tid` claim), needed by meeting joins.
    pub tenant_id: Option<String>,
    pub region: Option<String>,
    pub partition: Option<String>,
    pub ring: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("{}", tf!("auth.lib.no_url_known_for_service", detail = format!("{:?}", .0)))]
    UnknownService(Service),
    #[error(
        "{}",
        t("auth.lib.your_microsoft_session_has_expired_please")
    )]
    SessionExpired,
    #[error("{}", t("auth.lib.not_signed_in"))]
    NotSignedIn,
    #[error("{}", tf!("auth.lib.authentication_failed", detail = .0))]
    Failed(String),
}

/// Base URLs and request credentials per service; implementations own token acquisition and refresh.
pub trait Authenticator: Send + Sync {
    /// Base URL of `service` for the signed-in user's region.
    fn base_url(&self, service: Service) -> Result<Url, AuthError>;

    /// Adds whatever credentials `service` expects to `headers`.
    fn authorize(
        &self,
        service: Service,
        headers: &mut HeaderMap,
    ) -> impl Future<Output = Result<(), AuthError>> + Send;

    /// Resource-scoped bearer token for a supported SharePoint or Graph origin.
    fn authorize_resource(
        &self,
        _resource: &Url,
        _headers: &mut HeaderMap,
    ) -> impl Future<Output = Result<(), AuthError>> + Send {
        async {
            Err(AuthError::Failed(
                t("auth.lib.unsupported_resource").into(),
            ))
        }
    }

    /// After a 401 from [`Self::authorize_resource`]: `true` if the token was renewed and a retry is worth it.
    fn on_resource_unauthorized(&self, _resource: &Url) -> impl Future<Output = bool> + Send {
        async { false }
    }

    /// After a 401: `true` if credentials were refreshed and the request is worth retrying once.
    fn on_unauthorized(&self, _service: Service) -> impl Future<Output = bool> + Send {
        async { false }
    }

    /// Handles server-requested auth protocol negotiation before retrying.
    fn on_challenge(
        &self,
        service: Service,
        _headers: &HeaderMap,
    ) -> impl Future<Output = bool> + Send {
        self.on_unauthorized(service)
    }

    /// Captures service credentials returned by a response (endpoint registration).
    fn observe_response(&self, _service: Service, _headers: &HeaderMap) -> Result<(), AuthError> {
        Ok(())
    }

    /// Identity and routing the calling stack puts in payloads; `None` before sign-in.
    fn calling_context(&self) -> Option<CallingContext> {
        None
    }
}

/// Fixed service table and headers, for tests and local development.
#[derive(Debug, Clone, Default)]
pub struct StaticAuthenticator {
    services: HashMap<Service, Url>,
    headers: HeaderMap,
    calling: Option<Box<CallingContext>>,
    /// Origins accepted by `authorize_resource`; they receive the fixed headers.
    resources: Vec<String>,
}

impl StaticAuthenticator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_service(mut self, service: Service, base: Url) -> Self {
        self.services.insert(service, base);
        self
    }

    pub fn with_header(mut self, name: http::HeaderName, value: http::HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    pub fn with_calling_context(mut self, context: CallingContext) -> Self {
        self.calling = Some(Box::new(context));
        self
    }

    /// Lets `authorize_resource` accept `origin` (a SharePoint site served by a test server).
    pub fn with_resource(mut self, origin: &Url) -> Self {
        self.resources.push(origin.origin().ascii_serialization());
        self
    }
}

impl Authenticator for StaticAuthenticator {
    fn base_url(&self, service: Service) -> Result<Url, AuthError> {
        self.services
            .get(&service)
            .cloned()
            .ok_or(AuthError::UnknownService(service))
    }

    async fn authorize(&self, _service: Service, headers: &mut HeaderMap) -> Result<(), AuthError> {
        headers.extend(self.headers.clone());
        Ok(())
    }

    async fn authorize_resource(
        &self,
        resource: &Url,
        headers: &mut HeaderMap,
    ) -> Result<(), AuthError> {
        if !self
            .resources
            .contains(&resource.origin().ascii_serialization())
        {
            return Err(AuthError::Failed(
                t("auth.lib.unsupported_resource").into(),
            ));
        }
        headers.extend(self.headers.clone());
        Ok(())
    }

    fn calling_context(&self) -> Option<CallingContext> {
        self.calling.as_deref().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_service_is_an_error() {
        let auth = StaticAuthenticator::new();
        assert!(matches!(
            auth.base_url(Service::ChatService),
            Err(AuthError::UnknownService(Service::ChatService))
        ));
    }
}
