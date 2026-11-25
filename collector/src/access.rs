//! Who is signed in on `/join`: Cloudflare Access sits in front of it,
//! has the person prove their email with a one-time code, and passes a
//! signed token (`Cf-Access-Jwt-Assertion`). The collector checks that
//! token itself, so a request that went around Access gets nowhere.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use axum::http::HeaderMap;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::sync::RwLock;

/// Cloudflare rotates its signing keys every few weeks; look again after
/// this long, or when a token names a key we don't have.
const KEYS_FOR: Duration = Duration::from_secs(3600);
const REFETCH_AFTER: Duration = Duration::from_secs(60);

pub const HEADER: &str = "cf-access-jwt-assertion";

pub enum Access {
    Cloudflare {
        /// `https://<team>.cloudflareaccess.com`
        team: String,
        /// The Access application's audience tag.
        aud: String,
        http: reqwest::Client,
        keys: RwLock<Option<(Instant, JwkSet)>>,
    },
    /// Tests: the email in `x-test-email`.
    #[cfg(test)]
    Fake,
}

#[derive(Deserialize)]
struct Claims {
    email: String,
}

impl Access {
    pub fn cloudflare(team: &str, aud: &str, http: reqwest::Client) -> Self {
        let team = team.trim().trim_end_matches('/');
        let team = if team.starts_with("https://") {
            team.to_string()
        } else {
            format!("https://{team}")
        };
        Access::Cloudflare {
            team,
            aud: aud.trim().to_string(),
            http,
            keys: RwLock::new(None),
        }
    }

    /// The signed-in email, checked.
    pub async fn email(&self, headers: &HeaderMap) -> Result<String> {
        match self {
            #[cfg(test)]
            Access::Fake => headers
                .get("x-test-email")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .context("not signed in"),
            Access::Cloudflare {
                team,
                aud,
                http,
                keys,
            } => {
                let token = headers
                    .get(HEADER)
                    .and_then(|v| v.to_str().ok())
                    .context("no Access token")?;
                let kid = decode_header(token)?
                    .kid
                    .context("the token names no key")?;
                let key = key_for(team, http, keys, &kid).await?;
                let mut validation = Validation::new(Algorithm::RS256);
                validation.set_audience(&[aud]);
                validation.set_issuer(&[team]);
                let data = decode::<Claims>(token, &key, &validation)?;
                if data.claims.email.trim().is_empty() {
                    bail!("the token has no email");
                }
                Ok(data.claims.email)
            }
        }
    }
}

async fn key_for(
    team: &str,
    http: &reqwest::Client,
    keys: &RwLock<Option<(Instant, JwkSet)>>,
    kid: &str,
) -> Result<DecodingKey> {
    {
        let cached = keys.read().await;
        if let Some((at, set)) = cached.as_ref()
            && let Some(jwk) = set.find(kid)
            && at.elapsed() < KEYS_FOR
        {
            return Ok(DecodingKey::from_jwk(jwk)?);
        }
    }
    let mut cached = keys.write().await;
    // Don't let unknown key ids make us hammer Cloudflare.
    if let Some((at, set)) = cached.as_ref()
        && at.elapsed() < REFETCH_AFTER
    {
        let jwk = set
            .find(kid)
            .ok_or_else(|| anyhow!("unknown signing key"))?;
        return Ok(DecodingKey::from_jwk(jwk)?);
    }
    let set: JwkSet = http
        .get(format!("{team}/cdn-cgi/access/certs"))
        .send()
        .await
        .context("fetching Access keys")?
        .error_for_status()?
        .json()
        .await
        .context("reading Access keys")?;
    let key = set
        .find(kid)
        .map(DecodingKey::from_jwk)
        .transpose()?
        .ok_or_else(|| anyhow!("unknown signing key"));
    *cached = Some((Instant::now(), set));
    key
}
