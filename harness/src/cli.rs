//! Terminal commands: `harness auth …` and `harness models`. Zed's sign-in
//! buttons run these too (as `harness acp auth …`).

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{Context, Result, bail};

use crate::auth::{Credentials, check_key_shape, mask};
use crate::catalog::{self, ModelCache};
use crate::config::Route;

const AUTH_USAGE: &str = "Usage:
  harness auth login chatgpt                      Sign in with ChatGPT (uses your plan)
  harness auth logout chatgpt [--forget]          Sign out; --forget also drops the registration
  harness auth set [anthropic|openai] [--stdin]   Store an API key in Pathfinder's auth file
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
        Some("set") => set(&creds, positional.get(1).copied(), flag("--stdin")).await,
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
    let cache = if args.iter().any(|a| a == "--refresh") {
        catalog::refresh(
            &Credentials::system(),
            &reqwest::Client::new(),
            &Route::DISCOVERED,
        )
        .await
    } else {
        ModelCache::load()
    };
    let entries = cache.entries();
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
    let cache = ModelCache::load();
    println!("\n{} models discovered.", cache.entries().len());
    Ok(())
}
