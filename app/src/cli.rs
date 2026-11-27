//! Terminal commands: `pathfinder auth …` and `pathfinder models`.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{Context, Result, bail};

use crate::auth::Credentials;
use crate::auth::accounts::{self, NAMES, Row, parse_account, process_env};
use crate::auth::connect::{self, checking_client};
use crate::auth::flow::{Action, Failure, Flow, KeyTarget, Mode, Secret};
use crate::catalog;
use crate::config::{CliAccounts, Config, Route};
use crate::decide::{self, Provider, Question};
use crate::prompt;

const AUTH_USAGE: &str = "Usage:
  pathfinder auth login                         Pick how to connect (arrow keys, Esc goes back)
  pathfinder auth login chatgpt                 Sign in with your ChatGPT plan in the browser
  pathfinder auth login anthropic|openai        Add an API key (pay per use); masked, then checked
  pathfinder auth login endpoint                Add your own OpenAI-compatible server
  pathfinder auth login typesafe                The gate's key (TypeSafe Jev; in the TUI: Ctrl+P)
      --with-api-key                            Read the key from stdin instead (also: --stdin)
  pathfinder auth logout [account] [--forget]   Remove one sign-in or key (pick when not named);
                                                --forget also drops the ChatGPT registration
  pathfinder auth status                        Your accounts and what each uses
  pathfinder auth set <account> [--stdin]       Same as `auth login <account>`; also gemini,
                                                brave (`/web`) and openalex keys
  pathfinder auth remove <provider>             Remove Pathfinder's stored key
  pathfinder auth import <provider>             Copy a <provider>-api-key macOS keychain item";

pub async fn auth(args: &[String]) -> Result<()> {
    let creds = Credentials::system();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let from_stdin = flag("--stdin") || flag("--with-api-key");
    let positional: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with("--"))
        .collect();
    match positional.first().copied() {
        Some("login") => login(&creds, positional.get(1).copied(), from_stdin).await,
        Some("logout") => logout(&creds, positional.get(1).copied(), flag("--forget")).await,
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
                    let key =
                        read_key(&format!("Paste the key for {name} (hidden): "), from_stdin)?;
                    creds.set_named(&name, key.trim())?;
                    println!("Saved {name}.");
                }
            }
            Ok(())
        }
        Some("set") => match positional.get(1).copied() {
            Some("gemini") => set_decision(&creds, Provider::Gemini, from_stdin).await,
            name => login(&creds, name, from_stdin).await,
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
            connect::refresh(&creds, route).await;
            Ok(())
        }
        Some("import") => {
            let route = provider(positional.get(1).copied())?;
            if creds.import_keychain(route)? {
                println!(
                    "Copied the {}-api-key keychain item into Pathfinder's auth file.",
                    route.provider_id()
                );
                connect::refresh(&creds, route).await;
            } else {
                println!("No {}-api-key keychain item found.", route.provider_id());
            }
            Ok(())
        }
        Some("status") | None => status(&creds),
        Some("help") => {
            println!("{AUTH_USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown auth command `{other}`\n\n{AUTH_USAGE}"),
    }
}

pub async fn models(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--refresh") {
        let creds = Credentials::system();
        let http = checking_client()?;
        catalog::refresh(&creds, &http, &Route::DISCOVERED).await;
        catalog::refresh_endpoints(&Config::load(), &creds, &http).await;
    }
    let entries = Config::load().catalog();
    if entries.is_empty() {
        println!(
            "No models discovered yet. Connect an account with `pathfinder auth login`, or run `pathfinder models --refresh`."
        );
    }
    print!("{}", models_by_account(&entries));
    Ok(())
}

/// The catalog as `pathfinder models` prints it: a section per account (as
/// in the TUI's pickers), each model's id and label under it.
fn models_by_account(entries: &[crate::config::ModelEntry]) -> String {
    let mut out = String::new();
    for (i, (account, models)) in crate::config::by_account(entries).into_iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&account.header());
        out.push('\n');
        for m in models {
            out.push_str(&format!("  {:<44} {}\n", m.id, m.label));
        }
    }
    out
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
    Config::load()
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

/// A key from stdin (one line) when asked or when there is no terminal;
/// else a hidden prompt. Never echoed.
fn read_key(prompt: &str, from_stdin: bool) -> Result<String> {
    if from_stdin || !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        Ok(line.trim().to_string())
    } else {
        Ok(rpassword::prompt_password(prompt)?.trim().to_string())
    }
}

/// `auth set gemini`: Gemini Decision 1's key, checked with one request.
async fn set_decision(creds: &Credentials, p: Provider, from_stdin: bool) -> Result<()> {
    let key = read_key(
        &format!("Paste your {} API key (hidden): ", p.label()),
        from_stdin,
    )?;
    crate::auth::check_key_shape(&key).map_err(anyhow::Error::msg)?;
    print!("Checking the key with {}... ", p.label());
    io::stdout().flush()?;
    let probe = [Question {
        name: "probe".into(),
        instructions: "Is this a greeting?".into(),
        options: vec![("yes".into(), String::new()), ("no".into(), String::new())],
    }];
    match decide::call(&checking_client()?, p, &key, "hello", &probe).await {
        Ok(_) => println!("ok."),
        Err(e) => bail!("{e:#}. The key was not saved."),
    }
    creds.set_named(p.key_name(), &key)?;
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

fn account_rows(creds: &Credentials, cfg: &Config) -> Vec<accounts::RowView> {
    accounts::with_models(
        accounts::rows(creds, cfg, CliAccounts::detect(cfg), &process_env),
        &cfg.catalog(),
    )
}

/// The account `name` means; a name that isn't one is never echoed whole
/// (it may be a key pasted in the wrong place).
fn named_row(name: &str, cfg: &Config) -> Result<Row> {
    parse_account(name, cfg).with_context(|| accounts::unknown_account(name))
}

/// What happens when there is no terminal to pick in, or the key comes on
/// stdin: only named accounts, never a prompt that would wait.
fn no_terminal(what: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "There is no terminal here to pick in. Name the account: pathfinder auth {what} <{NAMES}>"
    )
}

/// `auth login [account]`: the account list, or straight to one account.
async fn login(creds: &Credentials, name: Option<&str>, from_stdin: bool) -> Result<()> {
    let cfg = Config::load();
    let row = name.map(|n| named_row(n, &cfg)).transpose()?;
    if from_stdin || !prompt::interactive() {
        let Some(row) = row else {
            return Err(no_terminal("login"));
        };
        return login_unattended(creds, &row, from_stdin).await;
    }
    let views = account_rows(creds, &cfg);
    let (flow, action) = match &row {
        None => (Flow::login(views), Action::None),
        Some(r) => Flow::at(Mode::Login, views, r),
    };
    drive(creds, flow, action).await
}

/// `auth login <account>` without a terminal, or with `--with-api-key`.
async fn login_unattended(creds: &Credentials, row: &Row, from_stdin: bool) -> Result<()> {
    match row {
        Row::Chatgpt => {
            if crate::browser_opens() {
                println!("Opening your browser to sign in with ChatGPT…");
            }
            let msg = connect::sign_in_chatgpt(creds, show_url).await?;
            println!("{msg}");
        }
        Row::ClaudeCode | Row::CodexCli => {
            let installed = CliAccounts::detect(&Config::load());
            let on = if *row == Row::ClaudeCode {
                installed.claude
            } else {
                installed.codex
            };
            println!("{}", row.sign_in_hint(on).unwrap_or_default());
        }
        Row::Advanced => bail!(
            "Adding a server asks a few questions: run `pathfinder auth login endpoint` in a terminal, or add it to config.json's `endpoints`."
        ),
        _ => {
            let target = KeyTarget::of(row).context("this account takes no key")?;
            // A terminal on stdin but not stdout (`… > log`): ask there,
            // masked, as the account is named.
            let key = if !from_stdin && io::stdin().is_terminal() {
                read_key(&format!("Paste your {} key (hidden): ", row.name()), false)?
            } else {
                read_key("", true)?
            };
            if key.is_empty() {
                bail!(
                    "No key on stdin. Pipe it in: … | pathfinder auth login {} --with-api-key",
                    row.id()
                );
            }
            crate::auth::check_key_shape(&key).map_err(anyhow::Error::msg)?;
            eprint!("Checking the key... ");
            let msg = connect::save_key(creds, &target, &Secret::new(&key)).await?;
            eprintln!("ok.");
            println!("{msg}");
        }
    }
    Ok(())
}

/// The browser sign-in's address, for when the browser doesn't open (or
/// none is opened here), on a line of its own.
fn show_url(url: &str) {
    if crate::browser_opens() {
        println!("If the browser doesn't open, open this address:\n{url}\n");
    } else {
        println!(
            "Open this address in a browser to sign in with ChatGPT. It sends the browser back to 127.0.0.1 on this machine{}:\n{url}\n",
            if crate::over_ssh() {
                " (over SSH, forward that port first with ssh -L)"
            } else {
                ""
            }
        );
    }
    println!("Waiting for the browser… (Ctrl+C stops)");
}

/// Shows the flow until it closes or a job finishes. A job runs under its
/// busy step (Esc cancels it); a failed one goes back to the step that
/// started it, with the reason.
async fn drive(creds: &Credentials, mut flow: Flow, mut action: Action) -> Result<()> {
    loop {
        let job = match action {
            Action::Run(job) => job,
            Action::Close | Action::Cancel => {
                println!("Nothing changed.");
                return Ok(());
            }
            Action::None | Action::Copy(_) => match prompt::drive(&mut flow)? {
                Some(job) => job,
                None => {
                    println!("Nothing changed.");
                    return Ok(());
                }
            },
        };
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let run = connect::run(creds, job, move |url| {
            let _ = tx.send(url.to_string());
        });
        match prompt::wait(&mut flow, run, rx).await? {
            prompt::JobEnd::Done(Ok(msg)) => {
                println!("{msg}");
                return Ok(());
            }
            prompt::JobEnd::Done(Err(e)) => {
                if !flow.fail(Failure::of(&e)) {
                    return Err(e);
                }
            }
            prompt::JobEnd::Cancelled => {
                if flow.is_closed() {
                    println!("Cancelled. Nothing changed.");
                    return Ok(());
                }
                flow.error = Some("Cancelled.".into());
            }
        }
        action = Action::None;
    }
}

/// `auth logout [account]`: removes one sign-in or key; picks when not
/// named.
async fn logout(creds: &Credentials, name: Option<&str>, forget: bool) -> Result<()> {
    let cfg = Config::load();
    match name {
        Some(n) => {
            let row = named_row(n, &cfg)?;
            let msg = match &row {
                // A server without a key: removing it is all there is to do.
                Row::Endpoint(id) if cfg.endpoint(id).is_some_and(|e| e.key.is_none()) => {
                    connect::remove_server(creds, id).await?
                }
                _ => connect::sign_out(creds, &row, forget).await?,
            };
            println!("{msg}");
            Ok(())
        }
        None if !prompt::interactive() => Err(no_terminal("logout")),
        None => {
            let flow = Flow::logout(account_rows(creds, &cfg));
            if flow.is_empty() {
                println!("{}", flow.empty_text());
                return Ok(());
            }
            drive(creds, flow, Action::None).await
        }
    }
}

/// Opens a URL in the default browser, unless PATHFINDER_NO_BROWSER is set
/// (e.g. over SSH), in which case the printed URL is used instead.
pub fn open_browser(url: &str) {
    crate::open_browser(url);
}

fn status(creds: &Credentials) -> Result<()> {
    let cfg = Config::load();
    let catalog = cfg.catalog();
    // A status report: nothing is cut; a long line wraps in the terminal.
    for line in status_text(&account_rows(creds, &cfg), cfg.local_only, catalog.len()) {
        println!("{line}");
    }
    Ok(())
}

/// `auth status`: the accounts, then how many models the pickers list (the
/// same catalog: discovered, CLI and server models).
fn status_text(views: &[accounts::RowView], local_only: bool, models: usize) -> Vec<String> {
    let mut out = accounts::status_lines(views);
    if local_only {
        out.push("Local only: endpoints only, no cloud models or decision services".into());
    }
    out.push(String::new());
    out.push(match models {
        0 => "No models listed yet. `pathfinder auth login` connects an account.".into(),
        1 => "1 model listed; `pathfinder models` shows it.".into(),
        n => format!("{n} models listed; `pathfinder models` shows them."),
    });
    out
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
    use super::models_by_account;

    #[test]
    fn models_print_in_sections_by_account() {
        use crate::config::{ModelEntry, Route};
        let m = |id: &str, label: &str, route| ModelEntry {
            id: id.into(),
            label: label.into(),
            route,
            model: id.into(),
            effort: None,
            endpoint: None,
        };
        let out = models_by_account(&[
            m("anthropic:claude-opus", "Claude Opus", Route::Anthropic),
            m("claude-haiku", "Haiku", Route::Claude),
        ]);
        let lines: Vec<&str> = out.lines().map(str::trim_end).collect();
        assert_eq!(lines[0], "Claude Code · Claude subscription");
        assert!(lines[1].starts_with("  claude-haiku") && lines[1].ends_with("Haiku"));
        assert_eq!(lines[2], "");
        assert_eq!(lines[3], "Anthropic API · pay per use");
        assert!(lines[4].ends_with("Claude Opus"));
    }

    #[test]
    fn status_counts_the_models_the_pickers_list() {
        use crate::auth::accounts::{Row, RowView, State, with_models};
        use crate::catalog::{CachedModel, ModelCache};
        use crate::config::{CliAccounts, Config, Endpoint};
        let mut cache = ModelCache::default();
        cache.providers.insert(
            "endpoint:local".into(),
            ["llama3", "qwen2.5"]
                .iter()
                .map(|m| CachedModel {
                    id: m.to_string(),
                    label: m.to_string(),
                })
                .collect(),
        );
        let cfg = Config {
            endpoints: vec![Endpoint {
                id: "local".into(),
                label: None,
                base_url: "http://127.0.0.1:8080/v1".into(),
                key: None,
                models: Vec::new(),
                extra: None,
            }],
            ..Config::default()
        };
        let catalog = cfg.catalog_from(&cache, CliAccounts::default());
        let views = with_models(
            vec![RowView::new(
                Row::Endpoint("local".into()),
                State::NoKeyNeeded,
            )],
            &catalog,
        );
        let text = super::status_text(&views, false, catalog.len()).join("\n");
        assert!(text.starts_with("Your servers\n"), "{text}");
        assert!(
            text.contains("  local  ✓ no key needed · 2 models"),
            "{text}"
        );
        assert!(
            text.ends_with("2 models listed; `pathfinder models` shows them."),
            "{text}"
        );
        assert!(!text.contains("connects"), "{text}");
        let none = super::status_text(&[], false, 0);
        assert!(none.last().unwrap().starts_with("No models listed yet"));
    }

    #[tokio::test]
    async fn without_a_terminal_a_picker_is_never_waited_for() {
        if crate::prompt::interactive() {
            return;
        }
        let creds = crate::auth::Credentials::system();
        let err = super::login(&creds, None, false).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("pathfinder auth login <chatgpt|anthropic|openai|endpoint|typesafe>"),
            "{err}"
        );
        let err = super::logout(&creds, None, false).await.unwrap_err();
        assert!(err.to_string().contains("no terminal"), "{err}");
        let err = super::login(&creds, Some("nope"), false).await.unwrap_err();
        assert!(err.to_string().contains("No account `nope`"), "{err}");
        // A key where the account goes is never echoed.
        let err = super::login(&creds, Some("sk-proj-PASTEDKEY-0123456789abcd"), false)
            .await
            .unwrap_err();
        assert!(!err.to_string().contains("PASTEDKEY"), "{err}");
        assert!(err.to_string().contains("looks like a key"), "{err}");
        // A server asks questions: it says so instead of waiting.
        let err = super::login(&creds, Some("endpoint"), true)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("in a terminal"), "{err}");
    }
}
