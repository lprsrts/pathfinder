//! The work behind signing in and out, shared by `pathfinder auth` and
//! `/login`: each check, save and model-list refresh, returning what to say
//! (never a key) instead of printing it.

use anyhow::{Context, Result, anyhow, bail};

use super::accounts::{Row, key_hint, process_env};
use super::flow::{Job, KeyTarget, Secret};
use super::{Credentials, check_key_shape};
use crate::catalog::{self, ModelCache};
use crate::config::{Config, Endpoint, Route};
use crate::decide::{self, Provider, Question};

/// For checking a key or a server: one that accepts the connection but never
/// answers fails instead of hanging.
pub fn checking_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()?)
}

/// Names a server can't take: they belong to Pathfinder's own routes and
/// keys.
const RESERVED_SERVERS: &[&str] = &[
    "anthropic",
    "openai",
    "chatgpt",
    "claude",
    "codex",
    "endpoint",
    "gemini",
    "google",
    "typesafe",
    "brave",
    "openalex",
];

/// The auth file entry for a server's key. It has its own prefix, so a
/// server's key can never replace a provider's (`openai-api-key`) or a search
/// key.
pub fn server_key_name(id: &str) -> Result<String> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("the name is letters, digits, - and _ only");
    }
    if RESERVED_SERVERS.contains(&id.to_ascii_lowercase().as_str()) {
        bail!("`{id}` is one of Pathfinder's own names; pick another for this server");
    }
    Ok(format!("endpoint-{id}-api-key"))
}

/// A short name for a server from its URL: `openrouter` for
/// `https://openrouter.ai/api/v1`, `localhost` for a local one.
pub fn server_name(base_url: &str) -> String {
    let host = base_url
        .split("://")
        .nth(1)
        .unwrap_or("")
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    if host.is_empty() || host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return "server".into();
    }
    let parts: Vec<&str> = host.split('.').collect();
    let name = match parts.as_slice() {
        [one] => one,
        [.., name, _tld] if name.len() > 2 => name,
        [.., name, _, _] => name,
        _ => host,
    };
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_lowercase()
}

/// A server that couldn't be reached: its address is what to fix, not the
/// key typed last.
#[derive(Debug)]
pub struct Unreachable(pub String);

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unreachable {}

/// A connection or timeout error from asking `url`, in a sentence (not
/// reqwest's own text) ending in `then`; other errors as they are.
pub fn reach_error(url: &str, then: &str, e: anyhow::Error) -> anyhow::Error {
    let Some(re) = e.chain().find_map(|c| c.downcast_ref::<reqwest::Error>()) else {
        return e;
    };
    let why = if re.is_timeout() {
        "it didn't answer in time"
    } else if re.is_connect() {
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(re);
        let mut kind = None;
        while let Some(err) = source {
            if let Some(io) = err.downcast_ref::<std::io::Error>() {
                kind = Some(io.kind());
            }
            source = err.source();
        }
        match kind {
            Some(std::io::ErrorKind::ConnectionRefused) => "connection refused",
            Some(std::io::ErrorKind::TimedOut) => "the connection timed out",
            _ => "no connection (check the host name and port)",
        }
    } else {
        return e;
    };
    anyhow::Error::new(Unreachable(format!("Could not reach {url}: {why}. {then}")))
}

const SERVER_DOWN: &str = "Is the server running?";

fn models_word(n: usize) -> String {
    format!("{n} model{} available", if n == 1 { "" } else { "s" })
}

/// Runs a step's job. `on_url` gets the browser sign-in's address, to show
/// in case the browser doesn't open.
pub async fn run(creds: &Credentials, job: Job, on_url: impl Fn(&str)) -> Result<String> {
    match job {
        Job::SignInChatgpt => sign_in_chatgpt(creds, on_url).await,
        Job::SaveKey { target, key } => save_key(creds, &target, &key).await,
        Job::UseEnv { target, var, save } => {
            let key = process_env(var).with_context(|| format!("${var} is empty"))?;
            let n = check(&target, &key).await?;
            let what = describe(&target);
            if save {
                store(creds, &target, &key)?;
                refresh_target(creds, &target).await;
                Ok(format!(
                    "{what}: the key in ${var} works, {}. Saved it to Pathfinder's auth file.",
                    models_word(n)
                ))
            } else {
                refresh_target(creds, &target).await;
                Ok(format!("{what}: using ${var}, {}.", models_word(n)))
            }
        }
        Job::AddServer { url, name, key } => add_server(creds, &url, &name, key.as_ref()).await,
        Job::SignOut(row) => sign_out(creds, &row, false).await,
        Job::RemoveServer(id) => remove_server(creds, &id).await,
    }
}

/// What the target is called in messages.
fn describe(target: &KeyTarget) -> String {
    target.row().name()
}

fn endpoint(id: &str) -> Result<Endpoint> {
    Config::load()
        .endpoints
        .into_iter()
        .find(|e| e.id == id)
        .with_context(|| format!("no endpoint `{id}` in config.json"))
}

/// The probe a decision key is checked with.
fn probe() -> [Question; 1] {
    [Question {
        name: "probe".into(),
        instructions: "Is this a greeting?".into(),
        options: vec![("yes".into(), String::new()), ("no".into(), String::new())],
    }]
}

/// Checks a key where it is used; the number of models it lists (none for
/// the gate).
async fn check(target: &KeyTarget, key: &str) -> Result<usize> {
    check_key_shape(key).map_err(anyhow::Error::msg)?;
    let http = checking_client()?;
    let n = match target {
        KeyTarget::Route(route) => catalog::list_models(&http, *route, key)
            .await
            .map_err(|e| reach_error(route.provider_name(), "Check your connection.", e))?
            .len(),
        KeyTarget::Gate => {
            decide::call(&http, Provider::Typesafe, key, "hello", &probe()).await?;
            0
        }
        KeyTarget::Endpoint(id) => {
            let ep = endpoint(id)?;
            crate::llm::chat::list_models(&http, &ep.base_url, Some(key))
                .await
                .map_err(|e| reach_error(&ep.base_url, SERVER_DOWN, e))?
                .len()
        }
    };
    Ok(n)
}

fn store(creds: &Credentials, target: &KeyTarget, key: &str) -> Result<()> {
    match target {
        KeyTarget::Route(route) => creds.set_key(*route, key),
        KeyTarget::Gate => creds.set_named(Provider::Typesafe.key_name(), key),
        KeyTarget::Endpoint(id) => {
            let name = endpoint(id)?
                .key
                .with_context(|| format!("`{id}` takes no key"))?;
            creds.set_named(&name, key)
        }
    }
}

/// Brings the model list in line with a changed credential.
async fn refresh_target(creds: &Credentials, target: &KeyTarget) {
    match target {
        KeyTarget::Route(route) => refresh(creds, *route).await,
        KeyTarget::Gate => {}
        KeyTarget::Endpoint(_) => {
            catalog::refresh_endpoints(
                &Config::load(),
                creds,
                &checking_client().unwrap_or_default(),
            )
            .await;
        }
    }
}

/// Refreshes the model list of the one provider that changed.
pub async fn refresh(creds: &Credentials, route: Route) {
    catalog::refresh(creds, &checking_client().unwrap_or_default(), &[route]).await;
}

/// A note when the environment overrides what was just saved or removed.
fn env_note(target: &KeyTarget) -> String {
    let var = match target {
        KeyTarget::Route(r) => r.key_env(),
        KeyTarget::Gate => Some(Provider::Typesafe.key_env()),
        KeyTarget::Endpoint(_) => None,
    };
    match var.filter(|v| process_env(v).is_some()) {
        Some(v) => format!(" ${v} is set too, and is used instead while it is."),
        None => String::new(),
    }
}

/// Checks `key` where it is used, then saves it.
pub async fn save_key(creds: &Credentials, target: &KeyTarget, key: &Secret) -> Result<String> {
    let key = key.expose();
    let n = check(target, key)
        .await
        .map_err(|e| anyhow!("{e:#}. The key was not saved."))?;
    store(creds, target, key)?;
    refresh_target(creds, target).await;
    let what = describe(target);
    let done = match target {
        KeyTarget::Gate => format!(
            "{what}: key {} checked and saved. Chat messages now go through {} first.",
            key_hint(key),
            Provider::Typesafe.label()
        ),
        _ => format!(
            "{what}: key {} checked, {}. Saved to Pathfinder's auth file.",
            key_hint(key),
            models_word(n)
        ),
    };
    Ok(format!("{done}{}", env_note(target)))
}

/// Browser sign-in with ChatGPT, then its model list.
pub async fn sign_in_chatgpt(creds: &Credentials, on_url: impl Fn(&str)) -> Result<String> {
    let registration = creds
        .chatgpt()
        .login(|url| {
            on_url(url);
            crate::open_browser(url);
        })
        .await?;
    refresh(creds, Route::Chatgpt).await;
    let count = ModelCache::load()
        .providers
        .get(Route::Chatgpt.provider_id())
        .map_or(0, Vec::len);
    Ok(format!(
        "Signed in with ChatGPT{}: {}. Model usage counts against your ChatGPT plan.",
        registration
            .email
            .map(|e| format!(" as {e}"))
            .unwrap_or_default(),
        models_word(count)
    ))
}

/// Checks a server, then adds it to `config.json` and its key (if any) to
/// the auth file.
pub async fn add_server(
    creds: &Credentials,
    base_url: &str,
    id: &str,
    key: Option<&Secret>,
) -> Result<String> {
    let key_name = server_key_name(id)?;
    let http = checking_client()?;
    let models = crate::llm::chat::list_models(&http, base_url, key.map(Secret::expose))
        .await
        .map_err(|e| match reach_error(base_url, SERVER_DOWN, e) {
            e if e.is::<Unreachable>() => {
                anyhow::Error::new(Unreachable(format!("{e} Nothing was saved.")))
            }
            e => anyhow!("{e:#}. Nothing was saved."),
        })?;
    let key_name = key.map(|_| key_name);
    if let (Some(name), Some(key)) = (&key_name, key) {
        creds.set_named(name, key.expose())?;
    }
    Config::add_endpoint(&Endpoint {
        id: id.to_string(),
        label: None,
        base_url: base_url.to_string(),
        key: key_name,
        models: Vec::new(),
        extra: None,
    })?;
    catalog::refresh_endpoints(&Config::load(), creds, &http).await;
    let next = match models.first() {
        Some(m) => format!(" /model (or `pathfinder models`) shows them, e.g. {id}:{m}."),
        None => " Its models show up once it lists some.".into(),
    };
    Ok(format!(
        "Added `{id} · your server` to config.json: {}.{next}",
        models_word(models.len())
    ))
}

/// Removes a server from `config.json`, and its key from the auth file.
pub async fn remove_server(creds: &Credentials, id: &str) -> Result<String> {
    let ep = endpoint(id)?;
    let key_removed = match &ep.key {
        Some(name) => creds.remove_named(name)?,
        None => false,
    };
    Config::remove_endpoint(id)?;
    let mut cache = ModelCache::load();
    if cache.providers.remove(&format!("endpoint:{id}")).is_some() {
        cache.save()?;
    }
    Ok(format!(
        "Removed `{id}` from config.json{}.",
        if key_removed {
            ", and its key from Pathfinder's auth file"
        } else {
            ""
        }
    ))
}

/// Removes only `row`'s credential, then refreshes its model list. With
/// `forget`, signing out of ChatGPT also drops the client registration.
pub async fn sign_out(creds: &Credentials, row: &Row, forget: bool) -> Result<String> {
    let removed = |what: &str, done: bool, target: Option<&KeyTarget>| {
        let note = target.map(env_note).unwrap_or_default();
        if done {
            format!("Removed the {what} from Pathfinder's auth file.{note}")
        } else {
            format!("Pathfinder had no stored {what}.{note}")
        }
    };
    match row {
        Row::Chatgpt => {
            let revoked = creds.chatgpt().logout(forget).await?;
            refresh(creds, Route::Chatgpt).await;
            Ok(format!(
                "Signed out of ChatGPT.{}",
                if revoked {
                    ""
                } else {
                    " The token couldn't be revoked with OpenAI, but it was deleted here."
                }
            ))
        }
        Row::AnthropicApi | Row::OpenaiApi => {
            let route = row.route().expect("an API row");
            let done = creds.remove_key(route)?;
            refresh(creds, route).await;
            Ok(removed(
                &format!("{} key", row.name()),
                done,
                Some(&KeyTarget::Route(route)),
            ))
        }
        Row::Gate => {
            let done = creds.remove_named(Provider::Typesafe.key_name())?;
            Ok(removed(
                &format!("{} key", Provider::Typesafe.label()),
                done,
                Some(&KeyTarget::Gate),
            ))
        }
        Row::Endpoint(id) => {
            let name = endpoint(id)?
                .key
                .with_context(|| format!("`{id}` takes no key"))?;
            let done = creds.remove_named(&name)?;
            Ok(removed(&format!("key for `{id}`"), done, None))
        }
        Row::ClaudeCode | Row::CodexCli => Ok(row.sign_in_hint(true).unwrap_or_default()),
        Row::Advanced => bail!("name the server to sign out of"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_key_never_takes_a_providers_name() {
        assert_eq!(server_key_name("lab").unwrap(), "endpoint-lab-api-key");
        for taken in [
            "openai",
            "Anthropic",
            "brave",
            "openalex",
            "typesafe",
            "gemini",
        ] {
            assert!(server_key_name(taken).is_err(), "{taken}");
        }
        assert!(server_key_name("my lab").is_err());
        assert!(server_key_name("").is_err());
    }

    #[test]
    fn a_server_is_named_after_its_host() {
        assert_eq!(server_name("https://openrouter.ai/api/v1"), "openrouter");
        assert_eq!(server_name("https://api.together.xyz/v1"), "together");
        assert_eq!(server_name("http://localhost:11434/v1"), "localhost");
        assert_eq!(server_name("http://gpu-box:8000/v1"), "gpu-box");
        assert_eq!(server_name("https://llm.lab.example.co.uk/v1"), "example");
        assert_eq!(server_name("http://192.168.1.5:8000/v1"), "server");
    }

    #[test]
    fn a_secret_never_shows_in_debug_output() {
        let job = Job::SaveKey {
            target: KeyTarget::Gate,
            key: Secret::new("ts-live-abcdefghijklmnopqrstuvwxyz"),
        };
        assert!(!format!("{job:?}").contains("abcdef"));
    }
}
