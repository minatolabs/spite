//! Microsoft identity sign-in (OAuth 2.0 device authorization grant,
//! RFC 8628) and token lifecycle. Runs entirely in the Rust core so tokens
//! never touch the webview. Refresh tokens live in the OS keychain; only
//! non-secret account metadata (UPN, display name) is written to disk.

pub mod token_store;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oauth2::basic::{BasicClient, BasicErrorResponseType, BasicTokenResponse};
use oauth2::{
    AuthUrl, ClientId, DeviceAuthorizationUrl, EndpointNotSet, EndpointSet, RefreshToken,
    RequestTokenError, Scope, StandardDeviceAuthorizationResponse, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::graph;
use token_store::TokenStore;

/// Delegated Graph scopes. Strict v0.1 set — deliberately no `Mail.ReadWrite`
/// (no server-side mark-read/drafts/delete in v0.1).
pub const SCOPES: [&str; 4] = [
    "https://graph.microsoft.com/Mail.Read",
    "https://graph.microsoft.com/Mail.Send",
    "https://graph.microsoft.com/User.Read",
    "offline_access",
];

#[derive(Debug, Clone, Serialize)]
pub struct DeviceCodePrompt {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub upn: String,
    pub display_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no client_id configured — set it in config.json (see README)")]
    NotConfigured,
    #[error("not signed in")]
    NeedsSignIn,
    #[error("identity provider returned no refresh token (offline_access missing?)")]
    NoRefreshToken,
    #[error("keychain error: {0}")]
    Store(String),
    #[error("account state error: {0}")]
    State(String),
    #[error("auth request failed: {0}")]
    OAuth(String),
    #[error(transparent)]
    Graph(#[from] graph::GraphError),
}

struct Session {
    access_token: String,
    expires_at: Instant,
}

type OAuthClient =
    BasicClient<EndpointSet, EndpointSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

pub struct Authenticator {
    config: AppConfig,
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    state_path: PathBuf,
    session: tokio::sync::Mutex<Option<Session>>,
}

impl Authenticator {
    pub fn new(
        config: AppConfig,
        store: Arc<dyn TokenStore>,
        data_dir: impl Into<PathBuf>,
    ) -> Self {
        let http = reqwest::Client::builder()
            // SSRF hardening: token endpoints must not redirect.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client construction cannot fail with these options");
        Self {
            config,
            http,
            store,
            state_path: data_dir.into().join("state.json"),
            session: tokio::sync::Mutex::new(None),
        }
    }

    /// Interactive device-code sign-in. `on_prompt` receives the user code +
    /// verification URI as soon as they are issued (the shell forwards them
    /// to the UI); the future then resolves when the user completes sign-in.
    pub async fn sign_in(
        &self,
        on_prompt: impl FnOnce(DeviceCodePrompt),
    ) -> Result<Account, AuthError> {
        let client = self.oauth_client()?;

        let details: StandardDeviceAuthorizationResponse = client
            .exchange_device_code()
            .add_scopes(SCOPES.iter().map(|s| Scope::new((*s).to_string())))
            .request_async(&self.http)
            .await
            .map_err(|e| AuthError::OAuth(e.to_string()))?;

        on_prompt(DeviceCodePrompt {
            user_code: details.user_code().secret().clone(),
            verification_uri: details.verification_uri().to_string(),
            expires_in_secs: details.expires_in().as_secs(),
        });

        let token = client
            .exchange_device_access_token(&details)
            .request_async(&self.http, tokio::time::sleep, None)
            .await
            .map_err(|e| AuthError::OAuth(e.to_string()))?;

        let me = graph::get_me(&self.http, token.access_token().secret()).await?;
        let account = Account {
            display_name: me
                .display_name
                .unwrap_or_else(|| me.user_principal_name.clone()),
            upn: me.user_principal_name,
        };

        let refresh = token
            .refresh_token()
            .ok_or(AuthError::NoRefreshToken)?
            .secret()
            .clone();
        self.save_refresh_token(&account.upn, refresh).await?;
        self.write_state(&account)?;
        self.cache_session(&token).await;
        Ok(account)
    }

    /// Relaunch path: exchange the stored refresh token for a fresh access
    /// token without prompting. Returns `NeedsSignIn` when there is no stored
    /// account/token or the IdP rejects the grant (revoked/expired).
    pub async fn silent_sign_in(&self) -> Result<Account, AuthError> {
        let account = self.read_state()?.ok_or(AuthError::NeedsSignIn)?;
        self.refresh_session(&account.upn).await?;
        Ok(account)
    }

    /// A valid access token for Graph calls, refreshing silently when the
    /// cached one is stale.
    pub async fn access_token(&self) -> Result<String, AuthError> {
        {
            let session = self.session.lock().await;
            if let Some(s) = session.as_ref() {
                if s.expires_at > Instant::now() + Duration::from_secs(60) {
                    return Ok(s.access_token.clone());
                }
            }
        }
        let account = self.read_state()?.ok_or(AuthError::NeedsSignIn)?;
        self.refresh_session(&account.upn).await?;
        let session = self.session.lock().await;
        session
            .as_ref()
            .map(|s| s.access_token.clone())
            .ok_or(AuthError::NeedsSignIn)
    }

    /// Drop the cached access token so the next `access_token()` call
    /// refreshes. Used when Graph rejects a token (401) that our local
    /// expiry bookkeeping still considered valid.
    pub async fn invalidate_session(&self) {
        *self.session.lock().await = None;
    }

    pub async fn sign_out(&self) -> Result<(), AuthError> {
        if let Some(account) = self.read_state()? {
            self.delete_refresh_token(&account.upn).await?;
        }
        match std::fs::remove_file(&self.state_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AuthError::State(e.to_string())),
        }
        *self.session.lock().await = None;
        Ok(())
    }

    async fn refresh_session(&self, upn: &str) -> Result<(), AuthError> {
        let refresh = self
            .load_refresh_token(upn)
            .await?
            .ok_or(AuthError::NeedsSignIn)?;

        let token = self
            .oauth_client()?
            .exchange_refresh_token(&RefreshToken::new(refresh))
            .add_scopes(SCOPES.iter().map(|s| Scope::new((*s).to_string())))
            .request_async(&self.http)
            .await
            .map_err(|e| match &e {
                // Revoked/expired refresh token → interactive sign-in again.
                RequestTokenError::ServerResponse(r)
                    if matches!(r.error(), BasicErrorResponseType::InvalidGrant) =>
                {
                    AuthError::NeedsSignIn
                }
                _ => AuthError::OAuth(e.to_string()),
            })?;

        // The IdP may rotate the refresh token; always keep the newest.
        if let Some(rotated) = token.refresh_token() {
            self.save_refresh_token(upn, rotated.secret().clone())
                .await?;
        }
        self.cache_session(&token).await;
        Ok(())
    }

    fn oauth_client(&self) -> Result<OAuthClient, AuthError> {
        if !self.config.is_configured() {
            return Err(AuthError::NotConfigured);
        }
        let authority = self.config.authority.trim_end_matches('/');
        let url_err = |e: oauth2::url::ParseError| AuthError::OAuth(e.to_string());
        Ok(
            BasicClient::new(ClientId::new(self.config.client_id.clone()))
                .set_auth_uri(
                    AuthUrl::new(format!("{authority}/oauth2/v2.0/authorize")).map_err(url_err)?,
                )
                .set_token_uri(
                    TokenUrl::new(format!("{authority}/oauth2/v2.0/token")).map_err(url_err)?,
                )
                .set_device_authorization_url(
                    DeviceAuthorizationUrl::new(format!("{authority}/oauth2/v2.0/devicecode"))
                        .map_err(url_err)?,
                ),
        )
    }

    async fn cache_session(&self, token: &BasicTokenResponse) {
        let expires_in = token.expires_in().unwrap_or(Duration::from_secs(300));
        *self.session.lock().await = Some(Session {
            access_token: token.access_token().secret().clone(),
            expires_at: Instant::now() + expires_in,
        });
    }

    // Keychain access goes through the Secret Service D-Bus socket, which is
    // blocking I/O — keep it off the async runtime's worker threads.
    async fn save_refresh_token(&self, upn: &str, token: String) -> Result<(), AuthError> {
        let store = Arc::clone(&self.store);
        let upn = upn.to_string();
        tokio::task::spawn_blocking(move || store.save_refresh_token(&upn, &token))
            .await
            .map_err(|e| AuthError::Store(e.to_string()))?
            .map_err(|e| AuthError::Store(e.to_string()))
    }

    async fn load_refresh_token(&self, upn: &str) -> Result<Option<String>, AuthError> {
        let store = Arc::clone(&self.store);
        let upn = upn.to_string();
        tokio::task::spawn_blocking(move || store.load_refresh_token(&upn))
            .await
            .map_err(|e| AuthError::Store(e.to_string()))?
            .map_err(|e| AuthError::Store(e.to_string()))
    }

    async fn delete_refresh_token(&self, upn: &str) -> Result<(), AuthError> {
        let store = Arc::clone(&self.store);
        let upn = upn.to_string();
        tokio::task::spawn_blocking(move || store.delete_refresh_token(&upn))
            .await
            .map_err(|e| AuthError::Store(e.to_string()))?
            .map_err(|e| AuthError::Store(e.to_string()))
    }

    fn read_state(&self) -> Result<Option<Account>, AuthError> {
        match std::fs::read_to_string(&self.state_path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| AuthError::State(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AuthError::State(e.to_string())),
        }
    }

    fn write_state(&self, account: &Account) -> Result<(), AuthError> {
        if let Some(dir) = self.state_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| AuthError::State(e.to_string()))?;
        }
        let raw =
            serde_json::to_string_pretty(account).map_err(|e| AuthError::State(e.to_string()))?;
        std::fs::write(&self.state_path, raw).map_err(|e| AuthError::State(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_scopes_exclude_mail_readwrite() {
        assert!(SCOPES.iter().all(|s| !s.contains("Mail.ReadWrite")));
        assert!(SCOPES.contains(&"offline_access"));
        assert_eq!(SCOPES.len(), 4);
    }

    #[test]
    fn unconfigured_client_id_is_rejected() {
        let auth = Authenticator::new(
            AppConfig::default(),
            Arc::new(token_store::MemoryTokenStore::default()),
            std::env::temp_dir(),
        );
        assert!(matches!(auth.oauth_client(), Err(AuthError::NotConfigured)));
    }
}
