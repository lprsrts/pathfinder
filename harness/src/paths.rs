//! Where Pathfinder keeps its files, whatever the setup: XDG variables or
//! none, macOS, Linux or Windows conventions, an unset `HOME`, or an app
//! (Zed) started without the shell's environment.
//!
//! For each kind of file, the first of these that applies:
//!
//! 1. `HARNESS_HOME`: everything in that one folder (the cache in `cache/`,
//!    state in `state/`).
//! 2. The kind's XDG variable (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
//!    `XDG_STATE_HOME`, `XDG_CACHE_HOME`), on any system, when it is an
//!    absolute path; the XDG spec says to ignore relative ones.
//! 3. A folder an earlier version or the other convention already made, so
//!    nothing is lost: the platform's own place, then the XDG default.
//! 4. The platform's own place: `~/Library/Application Support/com.lprsrts.pathfinder`
//!    (`~/Library/Caches/…` for the cache) on macOS, `%APPDATA%\Pathfinder`
//!    (`%LOCALAPPDATA%\Pathfinder` for the cache) on Windows, and the XDG
//!    defaults (`~/.config/harness`, `~/.local/share/harness`, …) elsewhere.
//!
//! `harness paths` prints what this resolves to.

use std::env;
use std::path::{Path, PathBuf};

/// The XDG-style folder name (the binary's name).
const NAME: &str = "harness";
/// The folder name in macOS's Library (the release's bundle id).
#[cfg(target_os = "macos")]
const MAC_NAME: &str = "com.lprsrts.pathfinder";
#[cfg(windows)]
const WINDOWS_NAME: &str = "Pathfinder";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Settings and secrets: `config.json`, `auth.json`, `telemetry.json`.
    Config,
    /// The database.
    Data,
    /// Locks and small run-to-run state.
    State,
    /// Things that can be fetched again: the model list.
    Cache,
}

impl Kind {
    fn xdg_var(self) -> &'static str {
        match self {
            Kind::Config => "XDG_CONFIG_HOME",
            Kind::Data => "XDG_DATA_HOME",
            Kind::State => "XDG_STATE_HOME",
            Kind::Cache => "XDG_CACHE_HOME",
        }
    }

    /// The XDG default under the home folder.
    fn xdg_default(self) -> &'static [&'static str] {
        match self {
            Kind::Config => &[".config"],
            Kind::Data => &[".local", "share"],
            Kind::State => &[".local", "state"],
            Kind::Cache => &[".cache"],
        }
    }

    /// Inside `HARNESS_HOME`.
    fn in_harness_home(self) -> Option<&'static str> {
        match self {
            Kind::Config | Kind::Data => None,
            Kind::State => Some("state"),
            Kind::Cache => Some("cache"),
        }
    }
}

/// An environment variable holding an absolute path.
fn abs_var(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The home folder: `HOME` when it is an absolute path, else the account's
/// home from the system (the password database, or the Windows profile).
pub fn home() -> Option<PathBuf> {
    abs_var("HOME").or_else(|| {
        #[allow(deprecated)]
        env::home_dir().filter(|p| p.is_absolute() && !p.as_os_str().is_empty())
    })
}

/// Where the platform itself keeps an app's files of this kind.
fn native(kind: Kind, home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let library = home.join("Library");
        match kind {
            Kind::Cache => library.join("Caches").join(MAC_NAME),
            _ => library.join("Application Support").join(MAC_NAME),
        }
    }
    #[cfg(windows)]
    {
        let roaming = abs_var("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"));
        let local = abs_var("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"));
        match kind {
            Kind::Config | Kind::Data => roaming.join(WINDOWS_NAME),
            Kind::State => local.join(WINDOWS_NAME),
            Kind::Cache => local.join(WINDOWS_NAME).join("cache"),
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        xdg_default(kind, home)
    }
}

fn xdg_default(kind: Kind, home: &Path) -> PathBuf {
    kind.xdg_default()
        .iter()
        .fold(home.to_path_buf(), |p, part| p.join(part))
        .join(NAME)
}

/// The folder for files of `kind` (it may not exist yet).
pub fn dir(kind: Kind) -> PathBuf {
    resolve(kind, |p| p.is_dir())
}

fn resolve(kind: Kind, exists: impl Fn(&Path) -> bool) -> PathBuf {
    if let Some(root) = abs_var("HARNESS_HOME") {
        return match kind.in_harness_home() {
            Some(sub) => root.join(sub),
            None => root,
        };
    }
    if let Some(base) = abs_var(kind.xdg_var()) {
        return base.join(NAME);
    }
    let Some(home) = home() else {
        // No home folder at all (a bare service account): a private folder
        // under the temp folder beats writing into whatever folder we were
        // started in.
        let user = env::var("USER")
            .or_else(|_| env::var("USERNAME"))
            .unwrap_or_default();
        return env::temp_dir()
            .join(format!("{NAME}-{user}"))
            .join(match kind {
                Kind::Config => "config",
                Kind::Data => "data",
                Kind::State => "state",
                Kind::Cache => "cache",
            });
    };
    let native = native(kind, &home);
    let xdg = xdg_default(kind, &home);
    if !exists(&native) && exists(&xdg) {
        xdg
    } else {
        native
    }
}

pub fn config_dir() -> PathBuf {
    dir(Kind::Config)
}

pub fn data_dir() -> PathBuf {
    dir(Kind::Data)
}

pub fn state_dir() -> PathBuf {
    dir(Kind::State)
}

pub fn cache_dir() -> PathBuf {
    dir(Kind::Cache)
}

/// Folders a sandboxed command must not read: wherever the config (with
/// the secrets) resolves to, plus the XDG and platform places it could be
/// in under another environment.
pub fn secret_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![config_dir()];
    if let Some(home) = home() {
        dirs.push(native(Kind::Config, &home));
        dirs.push(xdg_default(Kind::Config, &home));
    }
    if let Some(root) = abs_var("HARNESS_HOME") {
        dirs.push(root);
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

/// `harness paths`.
pub fn print() {
    let show = |label: &str, p: PathBuf| {
        let note = if p.is_dir() { "" } else { "  (not made yet)" };
        println!("{label:<9}{}{note}", p.display());
    };
    show("config", config_dir());
    show(
        "data",
        crate::config::Config::load()
            .db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(data_dir),
    );
    show("state", state_dir());
    show("cache", cache_dir());
    if let Ok(exe) = env::current_exe() {
        println!("{:<9}{}", "program", exe.display());
    }
    println!(
        "\nSet HARNESS_HOME to keep everything in one folder, or the XDG_*_HOME variables.\n\
         Apps started outside your shell (Zed) don't see your shell's variables: set them\n\
         in that app's settings too, or rely on these defaults."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_folders_win_over_the_platform_default() {
        let home = home().unwrap();
        let xdg = xdg_default(Kind::Config, &home);
        let native = native(Kind::Config, &home);
        // Run only without overrides in the environment.
        if abs_var("HARNESS_HOME").is_some() || abs_var("XDG_CONFIG_HOME").is_some() {
            return;
        }
        assert_eq!(resolve(Kind::Config, |p| p == xdg), xdg);
        assert_eq!(resolve(Kind::Config, |p| p == native), native);
        assert_eq!(resolve(Kind::Config, |_| false), native);
        assert_eq!(resolve(Kind::Config, |_| true), native);
    }

    #[test]
    fn xdg_defaults_follow_the_spec() {
        let home = PathBuf::from("/home/someone");
        assert_eq!(
            xdg_default(Kind::Data, &home),
            PathBuf::from("/home/someone/.local/share/harness")
        );
        assert_eq!(
            xdg_default(Kind::Cache, &home),
            PathBuf::from("/home/someone/.cache/harness")
        );
    }

    #[test]
    fn secrets_are_hidden_wherever_they_could_be() {
        let dirs = secret_dirs();
        assert!(dirs.contains(&config_dir()));
        if let Some(home) = home() {
            assert!(dirs.contains(&xdg_default(Kind::Config, &home)));
            assert!(dirs.contains(&native(Kind::Config, &home)));
        }
    }
}
