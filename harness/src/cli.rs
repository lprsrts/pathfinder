//! Terminal commands: `harness auth …` and `harness models`. Zed's sign-in
//! buttons run these too (as `harness acp auth …`).

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{Context, Result, bail};

use crate::auth::{Credentials, KeySource, check_key_shape, mask};
use crate::catalog::{self, ModelCache};
use crate::config::Route;

const AUTH_USAGE: &str = "Usage:
  harness auth set [anthropic|openai] [--stdin]   Store an API key in your keychain
  harness auth remove <provider>                  Remove Pathfinder's stored key
  harness auth import <provider>                  Copy an existing <provider>-api-key keychain item
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
        Some("set") => set(&creds, positional.get(1).copied(), flag("--stdin")).await,
        Some("remove") => {
            let route = provider(positional.get(1).copied())?;
            if creds.remove_key(route)? {
                println!("Removed Pathfinder's {} key.", route.provider_name());
            } else {
                println!("Pathfinder has no stored {} key.", route.provider_name());
            }
            refresh(&creds).await;
            Ok(())
        }
        Some("import") => {
            let route = provider(positional.get(1).copied())?;
            if creds.import_shared(route)? {
                println!(
                    "Copied {}-api-key into Pathfinder's keychain entry.",
                    route.provider_id()
                );
                refresh(&creds).await;
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
        catalog::refresh(&Credentials::system(), &reqwest::Client::new()).await
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
    println!("Saved to your keychain. Start a new Pathfinder thread to use it.");
    refresh(creds).await;
    Ok(())
}

async fn refresh(creds: &Credentials) {
    catalog::refresh(creds, &reqwest::Client::new()).await;
}

fn status(creds: &Credentials) -> Result<()> {
    for route in Route::API {
        match creds.lookup(route) {
            Some((key, source)) => {
                let note = match source {
                    KeySource::Shared => format!(
                        " (run `harness auth import {}` to give Pathfinder its own copy)",
                        route.provider_id()
                    ),
                    _ => String::new(),
                };
                println!(
                    "{:<10} {} from {}{note}",
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
