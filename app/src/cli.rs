//! Terminal commands: `pathfinder auth …` and `pathfinder models`. Zed's sign-in
//! buttons run these too (as `pathfinder acp auth …`).

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{Context, Result, bail};

use crate::auth::{Credentials, check_key_shape, mask};
use crate::catalog::{self, ModelCache};
use crate::config::Route;
use crate::decide::{self, Provider, Question};

const AUTH_USAGE: &str = "Usage:
  pathfinder auth login                              Connect models: pick how
  pathfinder auth login chatgpt                      Sign in with ChatGPT (uses your plan)
  pathfinder auth login [anthropic|openai]           Add an API key
  pathfinder auth login server                       Add an OpenAI-compatible service or your own server
  pathfinder auth logout chatgpt [--forget]          Sign out; --forget also drops the registration
  pathfinder auth set [anthropic|openai] [--stdin]   Store an API key in Pathfinder's auth file
  pathfinder auth set [gemini|typesafe] [--stdin]    Store a decision-model key (Gemini Decision 1, TypeSafe Jev)
  pathfinder auth set brave [--stdin]                Store a Brave Search key for `/web` (or `auth import brave`)
  pathfinder auth set openalex [--stdin]             Store an OpenAlex key, past its free daily allowance
  pathfinder auth remove <provider>                  Remove Pathfinder's stored key
  pathfinder auth import <provider>                  Copy a <provider>-api-key macOS keychain item
  pathfinder auth status                             Show which keys are available";

pub async fn auth(args: &[String]) -> Result<()> {
    let creds = Credentials::system();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let positional: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with("--"))
        .collect();
    match positional.first().copied() {
        Some("login") => match positional.get(1).copied() {
            Some("chatgpt") => login_chatgpt(&creds).await,
            Some("server") => add_server(&creds).await,
            Some(p) if Route::from_provider_id(p).is_some() => {
                set(&creds, Some(p), flag("--stdin")).await
            }
            Some(other) => bail!("unknown login `{other}`\n\n{AUTH_USAGE}"),
            None => match choose_login()? {
                Login::Chatgpt => login_chatgpt(&creds).await,
                Login::Key(route) => set(&creds, Some(route.provider_id()), false).await,
                Login::Server => add_server(&creds).await,
            },
        },
        Some("logout") => match positional.get(1).copied() {
            Some("chatgpt") | None => {
                let revoked = creds.chatgpt().logout(flag("--forget")).await?;
                println!(
                    "Signed out of ChatGPT.{}",
                    if revoked {
                        ""
                    } else {
                        " The token couldn't be revoked with OpenAI, but it was deleted here."
                    }
                );
                refresh(&creds, Route::Chatgpt).await;
                Ok(())
            }
            Some(other) => bail!("to remove a {other} key use `pathfinder auth remove {other}`"),
        },
        // An endpoint's key, by the name its config entry gives.
        Some(cmd @ ("set" | "import" | "remove"))
            if endpoint_key(positional.get(1).copied()).is_some() =>
        {
            let name = endpoint_key(positional.get(1).copied()).unwrap();
            match cmd {
                "import" if creds.import_named(&name)? => {
                    println!("Copied the {name} keychain item into Pathfinder's auth file.")
                }
                "import" => println!("No {name} keychain item found."),
                "remove" if creds.remove_named(&name)? => println!("Removed {name}."),
                "remove" => println!("Pathfinder has no stored {name}."),
                _ => {
                    let key = if flag("--stdin") || !io::stdin().is_terminal() {
                        let mut line = String::new();
                        io::stdin().lock().read_line(&mut line)?;
                        line
                    } else {
                        rpassword::prompt_password(format!("Paste the key for {name} (hidden): "))?
                    };
                    creds.set_named(&name, key.trim())?;
                    println!("Saved {name}.");
                }
            }
            Ok(())
        }
        Some("set") => match decision_only(positional.get(1).copied()) {
            Some(p) => set_decision(&creds, p, flag("--stdin")).await,
            None => set(&creds, positional.get(1).copied(), flag("--stdin")).await,
        },
        Some("remove" | "import") if decision_only(positional.get(1).copied()).is_some() => {
            let p = decision_only(positional.get(1).copied()).unwrap();
            if positional[0] == "remove" {
                if creds.remove_named(p.key_name())? {
                    println!("Removed Pathfinder's {} key.", p.label());
                } else {
                    println!("Pathfinder has no stored {} key.", p.label());
                }
            } else if creds.import_named(p.key_name())? {
                println!(
                    "Copied the {} keychain item into Pathfinder's auth file.",
                    p.key_name()
                );
            } else {
                println!("No {} keychain item found.", p.key_name());
            }
            Ok(())
        }
        Some("remove") => {
            let route = provider(positional.get(1).copied())?;
            if creds.remove_key(route)? {
                println!("Removed Pathfinder's {} key.", route.provider_name());
            } else {
                println!("Pathfinder has no stored {} key.", route.provider_name());
            }
            refresh(&creds, route).await;
            Ok(())
        }
        Some("import") => {
            let route = provider(positional.get(1).copied())?;
            if creds.import_keychain(route)? {
                println!(
                    "Copied the {}-api-key keychain item into Pathfinder's auth file.",
                    route.provider_id()
                );
                refresh(&creds, route).await;
            } else {
                println!("No {}-api-key keychain item found.", route.provider_id());
            }
            Ok(())
        }
        Some("status") | None => status(&creds),
        Some(other) => bail!("unknown auth command `{other}`\n\n{AUTH_USAGE}"),
    }
}

pub async fn models(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--refresh") {
        let creds = Credentials::system();
        let http = checking_client()?;
        catalog::refresh(&creds, &http, &Route::DISCOVERED).await;
        catalog::refresh_endpoints(&crate::config::Config::load(), &creds, &http).await;
    }
    let entries = crate::config::Config::load().catalog();
    if entries.is_empty() {
        println!(
            "No models discovered yet. Add a key with `pathfinder auth set`, or run `pathfinder models --refresh`."
        );
    }
    for entry in entries {
        println!("{:<44} {}", entry.id, entry.label);
    }
    Ok(())
}

/// The key name of a configured endpoint, when `name` is one.
fn endpoint_key(name: Option<&str>) -> Option<String> {
    let name = name?;
    // The web search key (`/web`).
    if matches!(name, "brave" | crate::web::BRAVE_KEY) {
        return Some(crate::web::BRAVE_KEY.to_string());
    }
    if matches!(name, "openalex" | crate::web::OPENALEX_KEY) {
        return Some(crate::web::OPENALEX_KEY.to_string());
    }
    crate::config::Config::load()
        .endpoints
        .iter()
        .filter_map(|e| e.key.clone())
        .find(|k| k == name)
}

/// Decision services whose key isn't also a model route's (OpenAI's is).
fn decision_only(name: Option<&str>) -> Option<Provider> {
    name.and_then(Provider::from_id)
        .filter(|p| !matches!(p, Provider::Openai | Provider::Local))
}

async fn set_decision(creds: &Credentials, p: Provider, from_stdin: bool) -> Result<()> {
    let key = if from_stdin || !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        line
    } else {
        rpassword::prompt_password(format!("Paste your {} API key (hidden): ", p.label()))?
    };
    let key = key.trim();
    check_key_shape(key).map_err(anyhow::Error::msg)?;
    print!("Checking the key with {}... ", p.label());
    io::stdout().flush()?;
    let probe = [Question {
        name: "probe".into(),
        instructions: "Is this a greeting?".into(),
        options: vec![("yes".into(), String::new()), ("no".into(), String::new())],
    }];
    match decide::call(&checking_client()?, p, key, "hello", &probe).await {
        Ok(_) => println!("ok."),
        Err(e) => bail!("{e:#}. The key was not saved."),
    }
    creds.set_named(p.key_name(), key)?;
    println!(
        "Saved to Pathfinder's auth file. Chat messages now go through {} first.",
        p.label()
    );
    Ok(())
}

fn provider(name: Option<&str>) -> Result<Route> {
    let name = name.context(format!(
        "name a provider: anthropic or openai\n\n{AUTH_USAGE}"
    ))?;
    Route::from_provider_id(name)
        .with_context(|| format!("unknown provider `{name}`; use anthropic or openai"))
}

enum Login {
    Chatgpt,
    Key(Route),
    Server,
}

fn ask(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn choose_login() -> Result<Login> {
    if !io::stdin().is_terminal() {
        bail!("name what to connect\n\n{AUTH_USAGE}");
    }
    let mut options = vec![(
        "Sign in with ChatGPT (uses your plan)".to_string(),
        Login::Chatgpt,
    )];
    for route in Route::API {
        options.push((
            format!("{} API key", route.provider_name()),
            Login::Key(route),
        ));
    }
    options.push((
        "Another OpenAI-compatible service or your own server (OpenRouter, Ollama, vLLM, ...)"
            .to_string(),
        Login::Server,
    ));
    println!("Connect models:");
    for (i, (label, _)) in options.iter().enumerate() {
        println!("  {}) {label}", i + 1);
    }
    let n: usize = ask("Number: ")?.parse().context("enter a number")?;
    options
        .into_iter()
        .nth(n.wrapping_sub(1))
        .map(|(_, l)| l)
        .context("no such option")
}

/// `pathfinder auth login server`: an OpenAI-compatible endpoint, checked
/// and added to `config.json`.
async fn add_server(creds: &Credentials) -> Result<()> {
    let base_url = ask("Base URL, up to and including /v1 (e.g. http://localhost:11434/v1): ")?;
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        bail!("the base URL starts with http:// or https://");
    }
    let base_url = base_url.trim_end_matches('/').to_string();
    let suggested = server_name(&base_url);
    let id = ask(&format!("Short name [{suggested}]: "))?;
    let id = if id.is_empty() { suggested } else { id };
    let key_name = server_key_name(&id)?;
    let key = rpassword::prompt_password("API key (hidden; Enter if it needs none): ")?;
    let key = key.trim();
    let key = (!key.is_empty()).then_some(key);

    print!("Checking {base_url}... ");
    io::stdout().flush()?;
    let http = checking_client()?;
    let models = crate::llm::chat::list_models(&http, &base_url, key)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}. Nothing was saved."))?;
    println!("ok, {} models available.", models.len());

    let key_name = key.map(|_| key_name);
    if let (Some(name), Some(key)) = (&key_name, key) {
        creds.set_named(name, key)?;
    }
    crate::config::Config::add_endpoint(&crate::config::Endpoint {
        id: id.clone(),
        label: None,
        base_url,
        key: key_name,
        models: Vec::new(),
        extra: None,
    })?;
    catalog::refresh_endpoints(&crate::config::Config::load(), creds, &http).await;
    println!(
        "Added `{id}` to {}. Start a new Pathfinder thread to use its models.",
        crate::config::default_config_path().display()
    );
    Ok(())
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
fn server_key_name(id: &str) -> Result<String> {
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

/// For checking a server: one that accepts the connection but never answers
/// fails instead of hanging.
fn checking_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()?)
}

/// A short name for a server from its URL: `openrouter` for
/// `https://openrouter.ai/api/v1`, `localhost` for a local one.
fn server_name(base_url: &str) -> String {
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

fn choose_provider() -> Result<Route> {
    println!("Which provider's API key do you want to add?");
    for (i, route) in Route::API.iter().enumerate() {
        println!("  {}) {}", i + 1, route.provider_name());
    }
    print!("Number: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let n: usize = line.trim().parse().context("enter a number")?;
    Route::API
        .get(n.wrapping_sub(1))
        .copied()
        .context("no such provider")
}

async fn set(creds: &Credentials, name: Option<&str>, from_stdin: bool) -> Result<()> {
    let route = match name {
        Some(n) => provider(Some(n))?,
        None => choose_provider()?,
    };
    let key = if from_stdin || !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        line
    } else {
        rpassword::prompt_password(format!(
            "Paste your {} API key (hidden): ",
            route.provider_name()
        ))?
    };
    let key = key.trim();
    check_key_shape(key).map_err(anyhow::Error::msg)?;

    print!("Checking the key with {}... ", route.provider_name());
    io::stdout().flush()?;
    match catalog::list_models(&checking_client()?, route, key).await {
        Ok(models) => println!("ok, {} models available.", models.len()),
        Err(e) => bail!("{e:#}. The key was not saved."),
    }
    creds.set_key(route, key)?;
    println!("Saved to Pathfinder's auth file. Start a new Pathfinder thread to use it.");
    refresh(creds, route).await;
    Ok(())
}

async fn login_chatgpt(creds: &Credentials) -> Result<()> {
    let session = creds.chatgpt();
    println!("Opening your browser to sign in with ChatGPT...");
    let registration = session
        .login(|url| {
            println!("If the browser doesn't open, visit:\n{url}\n");
            open_browser(url);
        })
        .await?;
    println!(
        "Signed in with ChatGPT{}. Model usage counts against your ChatGPT plan.",
        registration
            .email
            .map(|e| format!(" as {e}"))
            .unwrap_or_default()
    );
    refresh(creds, Route::Chatgpt).await;
    let count = ModelCache::load()
        .providers
        .get("chatgpt")
        .map_or(0, Vec::len);
    println!("{count} ChatGPT models available. Start a new Pathfinder thread to use them.");
    Ok(())
}

/// Opens a URL in the default browser, unless PATHFINDER_NO_BROWSER is set
/// (e.g. over SSH), in which case the printed URL is used instead.
pub fn open_browser(url: &str) {
    crate::open_browser(url);
}

/// Refreshes the model list of the one provider a command changed.
async fn refresh(creds: &Credentials, route: Route) {
    catalog::refresh(creds, &checking_client().unwrap_or_default(), &[route]).await;
}

fn status(creds: &Credentials) -> Result<()> {
    let session = creds.chatgpt();
    match (session.signed_in(), session.registration()) {
        (true, Some(reg)) => println!(
            "{:<10} signed in{}",
            "ChatGPT",
            reg.email.map(|e| format!(" as {e}")).unwrap_or_default()
        ),
        _ => println!("{:<10} not signed in (`pathfinder auth login`)", "ChatGPT"),
    }
    for route in Route::API {
        match creds.lookup(route) {
            Some((key, source)) => {
                println!(
                    "{:<10} {} from {}",
                    route.provider_name(),
                    mask(&key),
                    source.describe()
                );
            }
            None => println!(
                "{:<10} no key (`pathfinder auth set {}`)",
                route.provider_name(),
                route.provider_id()
            ),
        }
    }
    let cfg = crate::config::Config::load();
    for ep in &cfg.endpoints {
        let key = match &ep.key {
            None => "no key".to_string(),
            Some(name) => match creds.stored(name) {
                Some(k) => format!("key {}", mask(&k)),
                None => format!("missing key `{name}` (`pathfinder auth import {name}`)"),
            },
        };
        println!("{:<10} {} ({key})", ep.id, ep.base_url);
    }
    if cfg.local_only {
        println!(
            "{:<10} only endpoints: no cloud models or decision services",
            "Local only"
        );
    }
    let decision = Provider::ALL.into_iter().find_map(|p| {
        if p == Provider::Local {
            return None;
        }
        creds
            .named_key(p.key_name(), p.key_env())
            .map(|(_, s)| (p, s))
    });
    // The gate: the check that keeps a candidate's answer back while you
    // judge it. It needs a decision model: your own, or the shared one.
    let label = "Gate";
    let joined = crate::telemetry::Consent::load().enrolled;
    match (&cfg.decisions, decision) {
        (Some(d), _) => println!(
            "{label:<10} your endpoint model {} on `{}`",
            d.model, d.endpoint
        ),
        (None, _) if cfg.local_only => {
            println!("{label:<10} off: on-premises mode needs `decisions` set to an endpoint model")
        }
        (None, Some((p, source))) => println!(
            "{label:<10} your own key: {} (from {})",
            p.label(),
            source.describe()
        ),
        (None, None) if joined => {
            println!("{label:<10} the shared gate (you've joined the beta; no key needed)")
        }
        (None, None) => println!(
            "{label:<10} off: join the beta (`pathfinder data join`) or add a key (`pathfinder auth set typesafe`)"
        ),
    }
    let cache = ModelCache::load();
    println!("\n{} models discovered.", cache.entries().len());
    Ok(())
}

/// `pathfinder coverage [--export] [path]`: understanding coverage of the
/// agent's changes in the repository at `path` (default: here).
pub fn coverage(args: &[String]) -> Result<()> {
    let export = args.iter().any(|a| a == "--export");
    let dir = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let Some(root) = crate::ledger::repo_root(&dir) else {
        bail!("{} is not in a git repository", dir.display());
    };
    let store = crate::store::Store::open(&crate::config::Config::load().db_path)?;
    let entries = store.ledger(&root.to_string_lossy())?;
    let report = crate::ledger::coverage(&root, &entries);
    println!("{}", crate::ledger::render(&report));
    if export {
        let out = root.join(".pathfinder").join("coverage.json");
        std::fs::create_dir_all(out.parent().unwrap())?;
        std::fs::write(&out, serde_json::to_string_pretty(&report)?)?;
        println!("\nWrote {}", out.display());
    }
    if !report.chain_ok {
        bail!("the ledger chain does not verify");
    }
    Ok(())
}

/// `pathfinder data`: what Pathfinder may send, and changing it.
/// `pathfinder bug [what happened]`: shows the report in full, then sends
/// it on a yes (or saves it to a file when this computer hasn't joined).
pub async fn bug(args: &[String]) -> Result<()> {
    let note = args.join(" ");
    let cwd = std::env::current_dir().unwrap_or_default();
    let report = crate::bugreport::build(&note, &cwd);
    let consent = crate::telemetry::Consent::load();
    if !consent.enrolled {
        let path = crate::bugreport::save(&report)?;
        println!(
            "This computer hasn't joined, so the report was saved instead:\n\n  {}\n\n\
             Send it to {} or attach it to an issue at {}",
            path.display(),
            crate::operator::CONTACT,
            crate::operator::ISSUES
        );
        return Ok(());
    }
    println!("This is exactly what would be sent:\n");
    for line in crate::bugreport::preview(&report) {
        println!("  {line}");
    }
    print!("\nSend it to {}? [y/N] ", crate::operator::NAME);
    use std::io::Write;
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !answer.trim().eq_ignore_ascii_case("y") {
        println!("Nothing was sent.");
        return Ok(());
    }
    crate::telemetry::report_bug(&consent, &report).await?;
    println!("Sent. Thank you!");
    Ok(())
}

pub async fn data(args: &[String]) -> Result<()> {
    use crate::telemetry::{self, Consent, Outbox, consent};
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let cfg = crate::config::Config::load();
    let mut consent = Consent::load();
    match args.first().map(String::as_str) {
        None | Some("status") => {
            let outbox = Outbox::open(&cfg.db_path)?;
            let yes = |b: bool| if b { "yes" } else { "no" };
            println!("Usage events (no message text): {}", yes(consent.events));
            println!("Sharing threads you choose:     {}", yes(consent.threads));
            println!(
                "Enrolled:                       {}",
                if consent.enrolled {
                    format!("yes, at {}", telemetry::collector_url())
                } else {
                    "no".into()
                }
            );
            if crate::envvar::get("PATHFINDER_TELEMETRY").is_ok_and(|v| v.trim() == "off") {
                println!("PATHFINDER_TELEMETRY=off: nothing is collected in this shell.");
            }
            println!("Waiting to be sent:             {} events", outbox.count()?);
            println!(
                "\n`pathfinder data show` prints them. Notice: {}",
                consent::notice_url()
            );
            Ok(())
        }
        Some("show") => {
            let outbox = Outbox::open(&cfg.db_path)?;
            let rows = outbox.peek(usize::MAX)?;
            if rows.is_empty() {
                println!("Nothing is waiting to be sent.");
            }
            for row in rows {
                println!("{row}");
            }
            Ok(())
        }
        Some("join") => {
            consent.events = has("--events");
            consent.threads = has("--threads");
            match args.get(1).filter(|a| !a.starts_with("--")) {
                Some(invite) => telemetry::enroll(invite, &mut consent).await?,
                None => {
                    let sign_in = telemetry::start_sign_in(&consent).await?;
                    println!(
                        "Sign in with your email in the browser:\n\n  {}\n",
                        sign_in.url
                    );
                    println!("Check that the page shows this code: {}\n", sign_in.code);
                    open_browser(&sign_in.url);
                    println!("Waiting for you to link this computer there… (Ctrl+C stops)");
                    telemetry::finish_sign_in(&sign_in, &mut consent).await?;
                }
            }
            if consent.events || consent.threads {
                println!(
                    "Joined. Thank you! `pathfinder data` shows what is collected; `pathfinder data off` stops it."
                );
            } else {
                println!(
                    "Joined: the shared gate is on, and no usage data is sent. To share some: \
                     pathfinder data on --events and/or --threads (notice: {}).",
                    consent::notice_url()
                );
            }
            Ok(())
        }
        Some("on") => {
            let (events, threads) = match (has("--events"), has("--threads")) {
                (false, false) => (true, true),
                pick => pick,
            };
            consent.events |= events;
            consent.threads |= threads;
            consent.stamp();
            // The collector first: it refuses events it hasn't heard a yes for.
            if consent.enrolled {
                telemetry::set_remote_consent(&consent).await?;
            }
            consent.save()?;
            if consent.enrolled {
                println!("Saved.");
            } else {
                println!("Saved. Nothing is sent until you join: pathfinder data join");
            }
            Ok(())
        }
        Some("off") => {
            consent.events = false;
            consent.threads = false;
            consent.stamp();
            consent.save()?;
            Outbox::open(&cfg.db_path)?.clear()?;
            if let Err(e) = telemetry::set_remote_consent(&consent).await {
                eprintln!(
                    "Stopped here, but couldn't tell the collector ({e:#}). Nothing more is sent."
                );
            } else {
                println!("Stopped. Nothing more is sent, and the queue is cleared.");
            }
            println!("To erase what was already sent: pathfinder data delete");
            Ok(())
        }
        Some("delete") => {
            telemetry::delete_remote(&consent).await?;
            Outbox::open(&cfg.db_path)?.clear()?;
            consent.forget()?;
            consent.stamp();
            consent.save()?;
            println!(
                "Erased: the collector deleted everything from this install, and this install forgot its token."
            );
            Ok(())
        }
        Some(other) => {
            bail!("unknown `pathfinder data {other}`: use status, show, join, on, off or delete")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{server_key_name, server_name};

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
}
