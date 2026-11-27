use http::HeaderMap;
use std::sync::Arc;
#[cfg(feature = "demo")]
use teams_auth::StaticAuthenticator;
use teams_auth::{AuthError, Authenticator, CallingContext, Service, TeamsAuthenticator};
use teams_i18n::{t, tf};
use url::Url;

pub enum AppAuthenticator {
    Microsoft(Arc<TeamsAuthenticator>),
    #[cfg(feature = "demo")]
    Demo(StaticAuthenticator),
}
impl Authenticator for AppAuthenticator {
    fn base_url(&self, service: Service) -> Result<Url, AuthError> {
        match self {
            Self::Microsoft(a) => a.base_url(service),
            #[cfg(feature = "demo")]
            Self::Demo(a) => a.base_url(service),
        }
    }
    async fn authorize(&self, service: Service, headers: &mut HeaderMap) -> Result<(), AuthError> {
        match self {
            Self::Microsoft(a) => a.authorize(service, headers).await,
            #[cfg(feature = "demo")]
            Self::Demo(a) => a.authorize(service, headers).await,
        }
    }
    async fn authorize_resource(
        &self,
        resource: &Url,
        headers: &mut HeaderMap,
    ) -> Result<(), AuthError> {
        match self {
            Self::Microsoft(a) => a.authorize_resource(resource, headers).await,
            #[cfg(feature = "demo")]
            Self::Demo(a) => a.authorize_resource(resource, headers).await,
        }
    }
    async fn on_resource_unauthorized(&self, resource: &Url) -> bool {
        match self {
            Self::Microsoft(a) => a.on_resource_unauthorized(resource).await,
            #[cfg(feature = "demo")]
            Self::Demo(a) => a.on_resource_unauthorized(resource).await,
        }
    }
    async fn on_unauthorized(&self, service: Service) -> bool {
        match self {
            Self::Microsoft(a) => a.on_unauthorized(service).await,
            #[cfg(feature = "demo")]
            Self::Demo(a) => a.on_unauthorized(service).await,
        }
    }
    async fn on_challenge(&self, service: Service, headers: &HeaderMap) -> bool {
        match self {
            Self::Microsoft(a) => a.on_challenge(service, headers).await,
            #[cfg(feature = "demo")]
            Self::Demo(a) => a.on_challenge(service, headers).await,
        }
    }
    fn observe_response(&self, service: Service, headers: &HeaderMap) -> Result<(), AuthError> {
        match self {
            Self::Microsoft(a) => a.observe_response(service, headers),
            #[cfg(feature = "demo")]
            Self::Demo(a) => a.observe_response(service, headers),
        }
    }
    fn calling_context(&self) -> Option<CallingContext> {
        match self {
            Self::Microsoft(a) => a.calling_context(),
            #[cfg(feature = "demo")]
            Self::Demo(a) => a.calling_context(),
        }
    }
}

/// Opens an app-validated URL without invoking a shell.
pub fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let status = std::process::Command::new("xdg-open").arg(url).status();
    match status {
        Ok(status) if status.success() => Ok(()),
        _ => Err(t("core.session.could_not_open_the_browser_copy").into()),
    }
}

/// Where the refresh token is kept: the macOS Keychain, or memory in tests.
trait Vault: Send + Sync + 'static {
    fn write(&self, token: &str) -> Result<(), AuthError>;
    fn delete(&self) -> Result<(), AuthError>;
}

/// Microsoft rotates the refresh token ~10 times at startup and each Keychain write may prompt, so writes are coalesced.
struct CoalescingStore<V: Vault>(Arc<Coalescing<V>>);
struct Coalescing<V> {
    vault: V,
    delay: std::time::Duration,
    state: std::sync::Mutex<Pending>,
}
#[derive(Default)]
struct Pending {
    cleared: bool,
    scheduled: bool,
    next: Option<String>,
    saved: Option<String>,
    /// A failed write, reported on the next save.
    error: Option<String>,
}
impl<V: Vault> CoalescingStore<V> {
    fn new(vault: V, delay: std::time::Duration) -> Self {
        Self(Arc::new(Coalescing {
            vault,
            delay,
            state: Default::default(),
        }))
    }
}
impl<V: Vault> Coalescing<V> {
    fn flush(&self) {
        let token = {
            let mut state = self.state.lock().unwrap();
            state.scheduled = false;
            match state.next.take() {
                Some(token) if !state.cleared => token,
                _ => return,
            }
        };
        // The Keychain may wait on a permission dialog: never hold the lock meanwhile.
        let result = self.vault.write(&token);
        let mut state = self.state.lock().unwrap();
        if state.cleared {
            // Signed out while writing: do not leave the token behind.
            drop(state);
            let _ = self.vault.delete();
            return;
        }
        match result {
            Ok(()) => state.saved = Some(token),
            Err(e) => state.error = Some(e.to_string()),
        }
    }
}
impl<V: Vault> teams_auth::RefreshTokenStore for CoalescingStore<V> {
    fn save(&self, token: &str) -> Result<(), AuthError> {
        let mut state = self.0.state.lock().unwrap();
        if state.cleared {
            return Ok(());
        }
        let error = state.error.take();
        if state.saved.as_deref() != Some(token) || state.next.is_some() {
            state.next = Some(token.to_owned());
            if !state.scheduled {
                state.scheduled = true;
                let inner = self.0.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(inner.delay);
                    inner.flush();
                });
            }
        }
        error.map_or(Ok(()), |e| Err(AuthError::Failed(e)))
    }
    fn clear(&self) -> Result<(), AuthError> {
        {
            let mut state = self.0.state.lock().unwrap();
            state.cleared = true;
            state.next = None;
        }
        self.0.vault.delete()
    }
}

/// Refresh credentials live in the OS keychain, never in a workspace/config file.
#[cfg(any(target_os = "macos", windows))]
struct Keychain;
#[cfg(any(target_os = "macos", windows))]
fn keychain_entry() -> Result<keyring::Entry, AuthError> {
    keyring::Entry::new("oxide.microsoft", "current-account")
        .map_err(|_| AuthError::Failed(t("core.session.the_macos_keychain_is_unavailable").into()))
}
#[cfg(any(target_os = "macos", windows))]
impl Vault for Keychain {
    fn write(&self, token: &str) -> Result<(), AuthError> {
        // Windows keeps a password as UTF-16, twice the size: as bytes, a refresh token fits its 2560-byte limit.
        #[cfg(windows)]
        let saved = keychain_entry()?.set_secret(token.as_bytes());
        #[cfg(not(windows))]
        let saved = keychain_entry()?.set_password(token);
        saved.map_err(|_| AuthError::Failed(t("core.session.could_not_save_the_session_to").into()))
    }
    fn delete(&self) -> Result<(), AuthError> {
        match keychain_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AuthError::Failed(
                t("core.session.could_not_remove_the_session_from").into(),
            )),
        }
    }
}
#[cfg(any(target_os = "macos", windows))]
const KEYCHAIN_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
pub fn remember(auth: &TeamsAuthenticator) -> Result<(), String> {
    #[cfg(any(target_os = "macos", windows))]
    {
        auth.remember_session(Arc::new(CoalescingStore::new(Keychain, KEYCHAIN_DELAY)))
            .map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = auth;
        Err(t("core.session.saving_the_session_is_currently_only").into())
    }
}
pub async fn restore() -> Result<Option<Arc<TeamsAuthenticator>>, crate::error::CoreError> {
    use crate::error::CoreError;
    #[cfg(any(target_os = "macos", windows))]
    {
        // The Keychain may wait on an access prompt: keep it off the network workers.
        let entry = keychain_entry().map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let read = tokio::task::spawn_blocking(move || {
            entry
                .get_secret()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        })
        .await
        .map_err(|_| t("core.session.could_not_read_the_session_from").to_owned())?;
        #[cfg(not(windows))]
        let read = tokio::task::spawn_blocking(move || entry.get_password())
            .await
            .map_err(|_| t("core.session.could_not_read_the_session_from").to_owned())?;
        let token = match read {
            Ok(token) => token,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(_) => {
                return Err(t("core.session.could_not_read_the_session_from").into());
            }
        };
        match TeamsAuthenticator::restore(&token).await {
            Ok(auth) => Ok(Some(Arc::new(auth))),
            Err(AuthError::SessionExpired) => {
                tokio::task::spawn_blocking(|| Keychain.delete())
                    .await
                    .map_err(|_| t("core.session.could_not_remove_the_session_from").to_owned())?
                    .map_err(|e| e.to_string())?;
                Err(CoreError::SessionExpired(
                    t("core.session.your_microsoft_session_has_expired_please").into(),
                ))
            }
            Err(e) => Err(tf!("core.session.could_not_restore_the_session", error = e).into()),
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use teams_auth::RefreshTokenStore;

    #[derive(Clone, Default)]
    struct Memory(Arc<Mutex<(Vec<String>, usize)>>);
    impl Vault for Memory {
        fn write(&self, token: &str) -> Result<(), AuthError> {
            self.0.lock().unwrap().0.push(token.to_owned());
            Ok(())
        }
        fn delete(&self) -> Result<(), AuthError> {
            self.0.lock().unwrap().1 += 1;
            Ok(())
        }
    }

    #[test]
    fn a_burst_of_rotated_tokens_writes_the_keychain_once() {
        let memory = Memory::default();
        let store = CoalescingStore::new(memory.clone(), Duration::from_millis(50));
        for i in 0..10 {
            store.save(&format!("token-{i}")).unwrap();
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(memory.0.lock().unwrap().0, ["token-9"]);
        store.save("token-9").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            memory.0.lock().unwrap().0.len(),
            1,
            "an unchanged token is not rewritten"
        );
        store.save("token-10").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(memory.0.lock().unwrap().0, ["token-9", "token-10"]);
    }

    #[test]
    fn signing_out_cancels_a_pending_write() {
        let memory = Memory::default();
        let store = CoalescingStore::new(memory.clone(), Duration::from_millis(50));
        store.save("secret").unwrap();
        store.clear().unwrap();
        store.save("late").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let (writes, deletes) = memory.0.lock().unwrap().clone();
        assert!(writes.is_empty());
        assert_eq!(deletes, 1);
    }
}
