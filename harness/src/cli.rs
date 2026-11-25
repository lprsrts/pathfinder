//! Terminal commands: `harness auth …` and `harness models`. Zed's sign-in
//! buttons run these too (as `harness acp auth …`).

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{Context, Result, bail};

use crate::auth::{Credentials, check_key_shape, mask};
use crate::catalog::{self, ModelCache};
use crate::config::Route;
use crate::decide::{self, Provider, Question};

const AUTH_USAGE: &str = "Usage:
  harness auth login chatgpt                      Sign in with ChatGPT (uses your plan)
  harness auth logout chatgpt [--forget]          Sign out; --forget also drops the registration
  harness auth set [anthropic|openai] [--stdin]   Store an API key in Pathfinder's auth file
  harness auth set [gemini|typesafe] [--stdin]    Store a decision-model key (Gemini Decision 1, TypeSafe Jev)
  harness auth remove <provider>                  Remove Pathfinder's stored key
  harness auth import <provider>                  Copy a <provider>-api-key macOS keychain item
  harness auth status                             Show which keys are available";

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
            Some("chatgpt") | None => login_chatgpt(&creds).await,
            Some(other) => {
                bail!("`login` is for ChatGPT; for {other} use `harness auth set {other}`")
            }
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
            Some(other) => bail!("to remove a {other} key use `harness auth remove {other}`"),
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
        let http = reqwest::Client::new();
        catalog::refresh(&creds, &http, &Route::DISCOVERED).await;
        catalog::refresh_endpoints(&crate::config::Config::load(), &creds, &http).await;
    }
    let entries = crate::config::Config::load().catalog();
    if entries.is_empty() {
        println!(
            "No models discovered yet. Add a key with `harness auth set`, or run `harness models --refresh`."
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
    match decide::call(&reqwest::Client::new(), p, key, "hello", &probe).await {
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
    match catalog::list_models(&reqwest::Client::new(), route, key).await {
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

/// Opens a URL in the default browser, unless HARNESS_NO_BROWSER is set
/// (e.g. over SSH), in which case the printed URL is used instead.
pub fn open_browser(url: &str) {
    if std::env::var_os("HARNESS_NO_BROWSER").is_some() {
        return;
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Refreshes the model list of the one provider a command changed.
async fn refresh(creds: &Credentials, route: Route) {
    catalog::refresh(creds, &reqwest::Client::new(), &[route]).await;
}

fn status(creds: &Credentials) -> Result<()> {
    let session = creds.chatgpt();
    match (session.signed_in(), session.registration()) {
        (true, Some(reg)) => println!(
            "{:<10} signed in{}",
            "ChatGPT",
            reg.email.map(|e| format!(" as {e}")).unwrap_or_default()
        ),
        _ => println!(
            "{:<10} not signed in (`harness auth login chatgpt`)",
            "ChatGPT"
        ),
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
                "{:<10} no key (`harness auth set {}`)",
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
                None => format!("missing key `{name}` (`harness auth import {name}`)"),
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
    match (&cfg.decisions, decision) {
        (Some(d), _) => println!(
            "{:<10} {} on endpoint `{}` (log probabilities)",
            "Decisions", d.model, d.endpoint
        ),
        (None, _) if cfg.local_only => println!(
            "{:<10} none: set `decisions` to an endpoint model",
            "Decisions"
        ),
        (None, decision) => match decision {
            Some((p, source)) => println!(
                "{:<10} {} (key from {})",
                "Decisions",
                p.label(),
                source.describe()
            ),
            None => println!(
                "{:<10} none (`harness auth set gemini` or `typesafe`, or an OpenAI key)",
                "Decisions"
            ),
        },
    }
    let cache = ModelCache::load();
    println!("\n{} models discovered.", cache.entries().len());
    Ok(())
}

/// `harness coverage [--export] [path]`: understanding coverage of the
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

/// `harness data`: what Pathfinder may send, and changing it.
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
            if std::env::var("HARNESS_TELEMETRY").is_ok_and(|v| v.trim() == "off") {
                println!("HARNESS_TELEMETRY=off: nothing is collected in this shell.");
            }
            println!("Waiting to be sent:             {} events", outbox.count()?);
            println!(
                "\n`harness data show` prints them. Notice: {}",
                consent::NOTICE_URL
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
            let invite = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .context("usage: harness data join <invite code> [--events] [--threads]")?;
            consent.events = has("--events");
            consent.threads = has("--threads");
            telemetry::enroll(invite, &mut consent).await?;
            if consent.events || consent.threads {
                println!(
                    "Joined. Thank you! `harness data` shows what is collected; `harness data off` stops it."
                );
            } else {
                println!(
                    "Joined: the shared gate is on, and no usage data is sent. To share some: \
                     harness data on --events and/or --threads (notice: {}).",
                    consent::NOTICE_URL
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
                println!(
                    "Saved. Nothing is sent until you join with an invite code: harness data join <code>"
                );
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
            println!("To erase what was already sent: harness data delete");
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
            bail!("unknown `harness data {other}`: use status, show, join, on, off or delete")
        }
    }
}
