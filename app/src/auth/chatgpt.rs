//! Sign in with ChatGPT, using OpenAI's open-source flow: model usage counts
//! against the user's ChatGPT plan. Authorization code + PKCE with a loopback
//! callback; the first sign-in registers "Pathfinder" as a dynamic client.
//! See https://developers.openai.com/siwc/token-sharing-open-source/sign-in

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use super::secrets::SecretStore;

pub const RESOURCE: &str = crate::providers::OPENAI;
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const APP_NAME: &str = "Pathfinder";
/// Refresh when less than this remains on the access token.
const REFRESH_MARGIN: u64 = 5 * 60;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Entries in the auth file.
const REGISTRATION: &str = "chatgpt";
const ID_TOKEN: &str = "chatgpt-id-token";
const REFRESH: &str = "chatgpt-refresh";
const ACCESS: &str = "chatgpt-access";

/// Refresh errors that mean the session is over.
const TERMINAL_REFRESH: [&str; 6] = [
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];

#[derive(Debug, Error)]
pub enum ChatGptError {
    #[error(
        "Not signed in to ChatGPT. Run `pathfinder auth login chatgpt` in a terminal, then try again."
    )]
    NotSignedIn,
    #[error(
        "The ChatGPT sign-in has expired. Run `pathfinder auth login chatgpt` to sign in again."
    )]
    Expired,
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub issuer: String,
    pub authorize: String,
    pub token: String,
    pub jwks: String,
    pub discovery: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        let issuer = "https://auth.openai.com".to_string();
        Self {
            authorize: format!("{issuer}/api/accounts/authorize"),
            token: format!("{issuer}/api/accounts/oauth/token"),
            jwks: format!("{issuer}/.well-known/jwks.json"),
            discovery: format!("{issuer}/.well-known/openid-configuration"),
            issuer,
        }
    }
}

/// The registered client and the account it is bound to. Kept after logout so
/// the next sign-in reuses the registration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Registration {
    pub client_id: String,
    pub subject: String,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Access {
    access_token: String,
    /// Unix seconds.
    expires_at: u64,
    scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdClaims {
    sub: String,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 64 random alphanumeric characters (PKCE verifier, state, nonce).
fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) fn config_dir() -> PathBuf {
    crate::paths::config_dir()
}

/// Stable per-install id sent as `ext_agent_host_id`. Not a secret.
fn host_id(config_dir: &std::path::Path) -> Result<String> {
    let path = config_dir.join("install.json");
    if let Some(id) = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v["host_id"].as_str().map(str::to_string))
    {
        return Ok(id);
    }
    let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    fs::create_dir_all(config_dir)?;
    fs::write(&path, serde_json::json!({ "host_id": id }).to_string())?;
    Ok(id)
}

fn default_lock_path() -> PathBuf {
    crate::paths::state_dir().join("chatgpt.lock")
}

pub struct ChatGpt {
    store: Arc<dyn SecretStore>,
    http: reqwest::Client,
    endpoints: Endpoints,
    /// Serializes refreshes within this process; a file lock does it across
    /// processes, since refresh tokens rotate.
    access: Mutex<Option<Access>>,
    verify_signatures: bool,
    config_dir: PathBuf,
    lock_path: PathBuf,
}

impl ChatGpt {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            http: reqwest::Client::new(),
            endpoints: Endpoints::default(),
            access: Mutex::new(None),
            verify_signatures: true,
            config_dir: config_dir(),
            lock_path: default_lock_path(),
        }
    }

    #[cfg(test)]
    pub fn for_tests(store: Arc<dyn SecretStore>, endpoints: Endpoints, dir: PathBuf) -> Self {
        Self {
            store,
            http: reqwest::Client::new(),
            endpoints,
            access: Mutex::new(None),
            verify_signatures: false,
            config_dir: dir.join("config"),
            lock_path: dir.join("state").join("chatgpt.lock"),
        }
    }

    fn get(&self, account: &str) -> Option<String> {
        self.store.get(account).ok().flatten()
    }

    fn put(&self, account: &str, value: &str) -> Result<()> {
        self.store.set(account, value)
    }

    pub fn registration(&self) -> Option<Registration> {
        self.get(REGISTRATION)
            .and_then(|raw| serde_json::from_str(&raw).ok())
    }

    pub fn signed_in(&self) -> bool {
        self.get(REFRESH).is_some()
    }

    /// A valid access token, refreshed first when it's about to expire.
    pub async fn access_token(&self) -> Result<String, ChatGptError> {
        self.token(false).await
    }

    /// Refreshes even if the current token looks valid (after a 401).
    pub async fn force_refresh(&self) -> Result<String, ChatGptError> {
        self.token(true).await
    }

    async fn token(&self, force: bool) -> Result<String, ChatGptError> {
        let mut cached = self.access.lock().await;
        let fresh = |a: &Access| a.expires_at > now() + REFRESH_MARGIN;
        if !force && let Some(a) = cached.as_ref().filter(|a| fresh(a)) {
            return Ok(a.access_token.clone());
        }
        let stale = cached.as_ref().map(|a| a.access_token.clone());

        let _file_lock = lock_file(self.lock_path.clone())
            .await
            .map_err(|e| ChatGptError::Other(e.to_string()))?;
        // Another process may have refreshed while we waited.
        if let Some(stored) = self
            .get(ACCESS)
            .and_then(|raw| serde_json::from_str::<Access>(&raw).ok())
            && fresh(&stored)
            && (!force || Some(&stored.access_token) != stale.as_ref())
        {
            let token = stored.access_token.clone();
            *cached = Some(stored);
            return Ok(token);
        }
        let refresh_token = self.get(REFRESH).ok_or(ChatGptError::NotSignedIn)?;
        let registration = self.registration().ok_or(ChatGptError::NotSignedIn)?;
        let resp = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", registration.client_id.as_str()),
                ("refresh_token", refresh_token.as_str()),
                ("resource", RESOURCE),
            ])
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| ChatGptError::Other(format!("refreshing the ChatGPT sign-in: {e}")))?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let code = body["error"]
                .as_str()
                .or_else(|| body.pointer("/error/code").and_then(Value::as_str))
                .unwrap_or_default();
            if TERMINAL_REFRESH.contains(&code) {
                let _ = self.store.delete(REFRESH);
                let _ = self.store.delete(ACCESS);
                *cached = None;
                return Err(ChatGptError::Expired);
            }
            return Err(ChatGptError::Other(format!(
                "refreshing the ChatGPT sign-in failed ({status}): {code}"
            )));
        }
        let tokens: TokenResponse = serde_json::from_value(body)
            .map_err(|e| ChatGptError::Other(format!("bad token response: {e}")))?;
        let previous_scopes = cached
            .as_ref()
            .map(|a| a.scopes.clone())
            .unwrap_or_default();
        let access = self
            .store_tokens(&tokens, previous_scopes)
            .map_err(|e| ChatGptError::Other(e.to_string()))?;
        let token = access.access_token.clone();
        *cached = Some(access);
        Ok(token)
    }

    /// Saves a token response: rotated refresh token first, then access.
    fn store_tokens(&self, tokens: &TokenResponse, previous_scopes: Vec<String>) -> Result<Access> {
        if tokens.access_token.starts_with("sk-") {
            bail!("received an API key instead of a ChatGPT access token");
        }
        if let Some(refresh) = &tokens.refresh_token {
            self.put(REFRESH, refresh)?;
        }
        let scopes = tokens
            .scope
            .as_deref()
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or(previous_scopes);
        let access = Access {
            access_token: tokens.access_token.clone(),
            expires_at: now() + tokens.expires_in.unwrap_or(3600),
            scopes,
        };
        self.put(ACCESS, &serde_json::to_string(&access)?)?;
        Ok(access)
    }

    /// Runs the browser sign-in. `open` receives the URL to show.
    pub async fn login(&self, open: impl Fn(&str)) -> Result<Registration> {
        let previous = self.registration();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("opening a local port for the sign-in callback")?;
        let redirect = format!(
            "http://127.0.0.1:{}/auth/callback",
            listener.local_addr()?.port()
        );
        let verifier = random_token();
        let state = random_token();
        let nonce = random_token();
        let host = host_id(&self.config_dir)?;

        let client_id = previous
            .as_ref()
            .map(|r| r.client_id.clone())
            .unwrap_or_else(|| DYNAMIC_CLIENT.to_string());
        let challenge = pkce_challenge(&verifier);
        let mut params: Vec<(&str, String)> = vec![
            ("response_type", "code".into()),
            ("client_id", client_id),
            ("redirect_uri", redirect.clone()),
            ("scope", SCOPES.into()),
            ("resource", RESOURCE.into()),
            ("state", state.clone()),
            ("nonce", nonce.clone()),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256".into()),
            ("ext_agent_host_id", host),
        ];
        match &previous {
            None => params.push(("agent_name_hint", APP_NAME.into())),
            Some(reg) => {
                if let Some(hint) = self.get(ID_TOKEN) {
                    params.push(("id_token_hint", hint));
                }
                if let Some(email) = &reg.email {
                    params.push(("login_hint", email.clone()));
                }
            }
        }
        let url = reqwest::Url::parse_with_params(&self.endpoints.authorize, &params)?;
        open(url.as_str());

        let callback = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_callback(&listener, &state))
            .await
            .map_err(|_| anyhow!("timed out waiting for the browser sign-in"))??;

        let issued = match (&previous, callback.client_id.as_deref()) {
            (None, Some(id)) if id != DYNAMIC_CLIENT => id.to_string(),
            (None, _) => bail!("ChatGPT didn't return a client registration; sign-in incomplete"),
            (Some(reg), Some(id)) if id != reg.client_id => {
                bail!("ChatGPT returned a different client registration; sign-in rejected")
            }
            (Some(reg), _) => reg.client_id.clone(),
        };

        let resp = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", issued.as_str()),
                ("code", callback.code.as_str()),
                ("code_verifier", verifier.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("resource", RESOURCE),
            ])
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("exchanging the sign-in code")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!(
                "ChatGPT sign-in failed ({status}): {}",
                body["error"].as_str().unwrap_or("unknown error")
            );
        }
        let tokens: TokenResponse = serde_json::from_value(body).context("bad token response")?;

        let granted: Vec<&str> = tokens
            .scope
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect();
        if !granted.contains(&PLAN_SCOPE) {
            bail!(
                "This ChatGPT account didn't grant plan access to Pathfinder (it needs an eligible Plus or Pro plan)."
            );
        }
        if tokens.refresh_token.is_none() {
            bail!("ChatGPT didn't return a refresh token; sign-in incomplete");
        }
        let id_token = tokens
            .id_token
            .as_deref()
            .context("ChatGPT didn't return an ID token")?;
        let claims = self.verify_id_token(id_token, &issued, &nonce).await?;
        if let Some(reg) = &previous
            && reg.subject != claims.sub
        {
            bail!(
                "You signed in as a different account than the one registered here. Run `pathfinder auth logout chatgpt --forget` first to switch accounts."
            );
        }

        let registration = Registration {
            client_id: issued,
            subject: claims.sub,
            email: claims.email,
        };
        self.put(REGISTRATION, &serde_json::to_string(&registration)?)?;
        self.put(ID_TOKEN, id_token)?;
        let access = self.store_tokens(&tokens, Vec::new())?;
        *self.access.lock().await = Some(access);
        Ok(registration)
    }

    async fn verify_id_token(&self, token: &str, client_id: &str, nonce: &str) -> Result<IdClaims> {
        let claims: IdClaims = if self.verify_signatures {
            let header = jsonwebtoken::decode_header(token).context("bad ID token")?;
            let kid = header.kid.context("ID token has no key id")?;
            let jwks: jsonwebtoken::jwk::JwkSet = self
                .http
                .get(&self.endpoints.jwks)
                .timeout(Duration::from_secs(15))
                .send()
                .await
                .context("fetching OpenAI's signing keys")?
                .json()
                .await
                .context("reading OpenAI's signing keys")?;
            let jwk = jwks
                .find(&kid)
                .context("ID token signed with an unknown key")?;
            let key = jsonwebtoken::DecodingKey::from_jwk(jwk)?;
            let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
            validation.set_issuer(&[&self.endpoints.issuer]);
            validation.set_audience(&[client_id]);
            jsonwebtoken::decode::<IdClaims>(token, &key, &validation)
                .context("the ID token failed verification")?
                .claims
        } else {
            // Tests only: claims are still checked below.
            let payload = token.split('.').nth(1).context("bad ID token")?;
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload)?)?
        };
        if claims.nonce.as_deref() != Some(nonce) {
            bail!("the ID token's nonce doesn't match this sign-in");
        }
        Ok(claims)
    }

    /// Revokes the refresh token and deletes the tokens. With `forget`, also
    /// drops the client registration so a different account can sign in.
    /// Returns whether revocation was confirmed.
    pub async fn logout(&self, forget: bool) -> Result<bool> {
        let refresh = self.get(REFRESH);
        let registration = self.registration();
        let mut revoked = refresh.is_none();
        if let (Some(token), Some(reg)) = (&refresh, &registration) {
            revoked = self
                .revoke(token, &reg.client_id)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("revoking the ChatGPT token: {e:#}");
                    false
                });
        }
        for account in [REFRESH, ACCESS] {
            let _ = self.store.delete(account);
        }
        if forget {
            for account in [REGISTRATION, ID_TOKEN] {
                let _ = self.store.delete(account);
            }
        }
        *self.access.lock().await = None;
        Ok(revoked)
    }

    async fn revoke(&self, token: &str, client_id: &str) -> Result<bool> {
        let discovery: Value = self
            .http
            .get(&self.endpoints.discovery)
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .json()
            .await?;
        let endpoint = discovery["revocation_endpoint"]
            .as_str()
            .context("no revocation endpoint")?;
        if !endpoint.starts_with(&format!("{}/", self.endpoints.issuer)) {
            bail!("revocation endpoint is not on the issuer's origin");
        }
        let resp = self
            .http
            .post(endpoint)
            .form(&[
                ("token", token),
                ("token_type_hint", "refresh_token"),
                ("client_id", client_id),
            ])
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        Ok(resp.status().is_success())
    }
}

async fn lock_file(path: PathBuf) -> Result<fs::File> {
    tokio::task::spawn_blocking(move || -> Result<fs::File> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        file.lock()?;
        Ok(file)
    })
    .await?
}

struct Callback {
    code: String,
    client_id: Option<String>,
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Pathfinder</title><body style=\"font-family:system-ui;margin:3em\"><h2>Signed in to Pathfinder</h2><p>You can close this tab and return to your editor.</p>";
const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Pathfinder</title><body style=\"font-family:system-ui;margin:3em\"><h2>Sign-in did not complete</h2><p>Return to your editor for details.</p>";

/// Serves the loopback redirect until the matching callback arrives.
async fn wait_for_callback(listener: &TcpListener, state: &str) -> Result<Callback> {
    loop {
        let (mut sock, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))?;
        if url.path() != "/auth/callback" {
            let _ = sock
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
            continue;
        }
        let param = |k: &str| {
            url.query_pairs()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.into_owned())
        };
        let outcome = if param("state").as_deref() != Some(state) {
            Err(anyhow!("the sign-in callback had the wrong state; ignored"))
        } else if let Some(err) = param("error") {
            Err(anyhow!(if err == "access_denied" {
                "Sign-in was cancelled.".to_string()
            } else {
                format!("Sign-in failed: {err}")
            }))
        } else if let Some(code) = param("code") {
            Ok(Callback {
                code,
                client_id: param("client_id"),
            })
        } else {
            Err(anyhow!("the sign-in callback had no code"))
        };
        let page = if outcome.is_ok() {
            DONE_PAGE
        } else {
            FAILED_PAGE
        };
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
            page.len()
        );
        let _ = sock.write_all(reply.as_bytes()).await;
        let _ = sock.shutdown().await;
        // A stray request with the wrong state doesn't end the sign-in.
        match outcome {
            Err(e) if e.to_string().contains("wrong state") => continue,
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use super::super::secrets::MemoryStore;
    use super::*;

    /// A fake OpenAI auth server: token endpoint, discovery, revocation.
    async fn fake_server(
        token_reply: impl Fn(&HashMap<String, String>) -> (u16, Value) + Send + Sync + 'static,
    ) -> (
        Endpoints,
        Arc<StdMutex<Vec<(String, HashMap<String, String>)>>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let log = seen.clone();
        let base2 = base.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16384];
                let mut len = 0;
                loop {
                    let n = sock.read(&mut buf[len..]).await.unwrap();
                    len += n;
                    let text = String::from_utf8_lossy(&buf[..len]).to_string();
                    if let Some(pos) = text.find("\r\n\r\n") {
                        let cl = text[..pos]
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if len >= pos + 4 + cl || n == 0 {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&buf[..len]).to_string();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
                let form: HashMap<String, String> = url_form(body);
                log.lock().unwrap().push((path.clone(), form.clone()));
                let (status, json) = if path.ends_with("openid-configuration") {
                    (
                        200,
                        serde_json::json!({"revocation_endpoint": format!("{base2}/api/accounts/oauth/revoke")}),
                    )
                } else if path.ends_with("/revoke") {
                    (200, Value::Null)
                } else {
                    token_reply(&form)
                };
                let body = if json.is_null() {
                    String::new()
                } else {
                    json.to_string()
                };
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let endpoints = Endpoints {
            issuer: base.clone(),
            authorize: format!("{base}/api/accounts/authorize"),
            token: format!("{base}/api/accounts/oauth/token"),
            jwks: format!("{base}/.well-known/jwks.json"),
            discovery: format!("{base}/.well-known/openid-configuration"),
        };
        (endpoints, seen)
    }

    fn url_form(body: &str) -> HashMap<String, String> {
        reqwest::Url::parse(&format!("http://x/?{body}"))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    /// An unsigned ID token; tests skip signature checks, not claim checks.
    fn id_token(sub: &str, nonce: &str) -> String {
        let enc = |v: Value| URL_SAFE_NO_PAD.encode(v.to_string());
        format!(
            "{}.{}.sig",
            enc(serde_json::json!({"alg": "RS256", "typ": "JWT"})),
            enc(serde_json::json!({"sub": sub, "nonce": nonce, "email": "me@example.com"}))
        )
    }

    /// Plays the browser: parses the authorize URL and calls back.
    fn browser(
        client_id: Option<&'static str>,
        seen_nonce: Arc<StdMutex<String>>,
    ) -> impl Fn(&str) {
        move |url: &str| {
            let url = reqwest::Url::parse(url).unwrap();
            let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["resource"], RESOURCE);
            assert!(q["scope"].contains(PLAN_SCOPE));
            let mut cb = format!("{}?code=c0de&state={}", q["redirect_uri"], q["state"]);
            if let Some(id) = client_id {
                cb.push_str(&format!("&client_id={id}"));
            }
            let nonce = q["nonce"].clone();
            *seen_nonce.lock().unwrap() = nonce;
            tokio::spawn(async move {
                reqwest::get(cb).await.unwrap();
            });
        }
    }

    #[tokio::test]
    async fn first_sign_in_registers_then_refreshes_and_logs_out() {
        let tmp = std::env::temp_dir().join(format!("harness-siwc-{}", uuid::Uuid::new_v4()));
        let nonce = Arc::new(StdMutex::new(String::new()));
        let server_nonce = nonce.clone();
        let (endpoints, seen) = fake_server(move |form| match form["grant_type"].as_str() {
            "authorization_code" => (
                200,
                serde_json::json!({
                    "access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600,
                    "id_token": id_token("user-1", &server_nonce.lock().unwrap()),
                    "scope": SCOPES
                }),
            ),
            "refresh_token" if form["refresh_token"] == "rt-1" => (
                200,
                serde_json::json!({"access_token": "at-2", "refresh_token": "rt-2", "expires_in": 3600}),
            ),
            _ => (400, serde_json::json!({"error": "invalid_grant"})),
        })
        .await;
        let store = Arc::new(MemoryStore::default());
        let gpt = ChatGpt::for_tests(store.clone(), endpoints, tmp.clone());

        let reg = gpt.login(browser(Some("oaiapp_123"), nonce)).await.unwrap();
        assert_eq!(reg.client_id, "oaiapp_123");
        assert_eq!(reg.email.as_deref(), Some("me@example.com"));
        assert_eq!(gpt.access_token().await.unwrap(), "at-1");

        // Token exchange used the issued client id, not the dynamic one.
        let exchange = seen.lock().unwrap()[0].1.clone();
        assert_eq!(exchange["client_id"], "oaiapp_123");
        assert_eq!(exchange["resource"], RESOURCE);
        assert!(exchange["code_verifier"].len() >= 43);

        // Forced refresh rotates the refresh token.
        assert_eq!(gpt.force_refresh().await.unwrap(), "at-2");
        assert_eq!(store.get(REFRESH).unwrap().as_deref(), Some("rt-2"));

        // A rejected refresh ends the session.
        assert!(matches!(
            gpt.force_refresh().await,
            Err(ChatGptError::Expired)
        ));
        assert!(!gpt.signed_in());
        assert!(gpt.registration().is_some());

        store.set(REFRESH, "rt-x").unwrap();
        assert!(gpt.logout(false).await.unwrap());
        assert!(!gpt.signed_in());
        assert!(gpt.registration().is_some(), "registration kept for reuse");
        let paths: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(p, _)| p.clone())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("/revoke")));
        fs::remove_dir_all(tmp).ok();
    }

    #[tokio::test]
    async fn rejects_a_first_sign_in_without_an_issued_client() {
        let (endpoints, _) = fake_server(|_| (500, Value::Null)).await;
        let tmp = std::env::temp_dir().join(format!("harness-siwc-{}", uuid::Uuid::new_v4()));
        let gpt = ChatGpt::for_tests(Arc::new(MemoryStore::default()), endpoints, tmp.clone());
        let err = gpt
            .login(browser(None, Arc::new(StdMutex::new(String::new()))))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("client registration"), "{err}");
        fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn pkce_challenge_is_base64url_sha256() {
        // RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
