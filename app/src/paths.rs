//! Where Pathfinder keeps its files, whatever the setup: XDG variables or
//! none, macOS, Linux or Windows conventions, an unset `HOME`, or a
//! launcher that doesn't pass the shell's environment.
//!
//! For each kind of file, the first of these that applies:
//!
//! 1. `PATHFINDER_HOME`: everything in that one folder (the cache in `cache/`,
//!    state in `state/`).
//! 2. The kind's XDG variable (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`,
//!    `XDG_STATE_HOME`, `XDG_CACHE_HOME`), on any system, when it is an
//!    absolute path; the XDG spec says to ignore relative ones.
//! 3. A folder an earlier version or the other convention already made, so
//!    nothing is lost: the platform's own place, the place an earlier
//!    version used on this platform (`%APPDATA%\Pathfinder` for the
//!    database on Windows), then the XDG default.
//! 4. The platform's own place: `~/Library/Application Support/com.lprsrts.pathfinder`
//!    (`~/Library/Caches/…` for the cache) on macOS, `%APPDATA%\Pathfinder`
//!    for the config and `%LOCALAPPDATA%\Pathfinder` for the database,
//!    state and cache on Windows (the database stays out of the roaming
//!    profile), and the XDG defaults (`~/.config/harness`,
//!    `~/.local/share/harness`, …) elsewhere.
//!
//! `pathfinder paths` prints what this resolves to.

use std::env;
use std::path::{Path, PathBuf};

/// The XDG-style folder name.
const NAME: &str = "pathfinder";
/// The folder name before 0.3; an existing one keeps being used.
const OLD_NAME: &str = "harness";
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

    /// Inside `PATHFINDER_HOME`.
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
    crate::envvar::get_os(name)
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
            // Settings are small and follow the user between machines.
            Kind::Config => roaming.join(WINDOWS_NAME),
            // The database (and the staging copies near it) would be synced
            // with a roaming profile: it stays on this machine.
            Kind::Data | Kind::State => local.join(WINDOWS_NAME),
            Kind::Cache => local.join(WINDOWS_NAME).join("cache"),
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        xdg_default(kind, home)
    }
}

/// Where an earlier version kept files of this kind on this platform, when
/// that differs from `native`, and the file whose presence shows it is in
/// use (the folder alone can be another kind's).
fn earlier_native(kind: Kind, home: &Path) -> Option<(PathBuf, PathBuf)> {
    #[cfg(windows)]
    {
        // Earlier versions kept the database in the roaming profile, in the
        // folder the config still uses: only the database itself counts.
        if kind == Kind::Data {
            let roaming =
                abs_var("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"));
            let dir = roaming.join(WINDOWS_NAME);
            // `config::default_db_path`'s file name.
            let db = dir.join("harness.sqlite");
            return Some((dir, db));
        }
        None
    }
    #[cfg(not(windows))]
    {
        let _ = (kind, home);
        None
    }
}

fn xdg_base(kind: Kind, home: &Path) -> PathBuf {
    kind.xdg_default()
        .iter()
        .fold(home.to_path_buf(), |p, part| p.join(part))
}

#[cfg(any(test, not(any(target_os = "macos", windows))))]
fn xdg_default(kind: Kind, home: &Path) -> PathBuf {
    xdg_base(kind, home).join(NAME)
}

/// `base/pathfinder`, unless only an older `base/harness` exists.
fn named(base: &Path, exists: &impl Fn(&Path) -> bool) -> PathBuf {
    let new = base.join(NAME);
    let old = base.join(OLD_NAME);
    if !exists(&new) && exists(&old) {
        old
    } else {
        new
    }
}

/// The folder for files of `kind` (it may not exist yet).
pub fn dir(kind: Kind) -> PathBuf {
    resolve(kind, |p| p.is_dir())
}

fn resolve(kind: Kind, exists: impl Fn(&Path) -> bool) -> PathBuf {
    if let Some(root) = abs_var("PATHFINDER_HOME") {
        return match kind.in_harness_home() {
            Some(sub) => root.join(sub),
            None => root,
        };
    }
    if let Some(base) = abs_var(kind.xdg_var()) {
        return named(&base, &exists);
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
    // Checked before `native` exists: on Windows the database's new folder
    // is also the state folder, which earlier versions already made, so
    // the folder alone doesn't show where the database is.
    if let Some((earlier, marker)) = earlier_native(kind, &home)
        && marker.is_file()
        && marker
            .file_name()
            .is_none_or(|name| !native.join(name).is_file())
    {
        return earlier;
    }
    if exists(&native) {
        return native;
    }
    let xdg = named(&xdg_base(kind, &home), &exists);
    if exists(&xdg) { xdg } else { native }
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
        dirs.push(xdg_base(Kind::Config, &home).join(NAME));
        dirs.push(xdg_base(Kind::Config, &home).join(OLD_NAME));
    }
    if let Some(root) = abs_var("PATHFINDER_HOME") {
        dirs.push(root);
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

/// `pathfinder paths`.
pub fn print() {
    let show = |label: &str, p: PathBuf| {
        let note = if p.exists() { "" } else { "  (not made yet)" };
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
    show("log", crate::bugreport::tui_log());
    show("crashes", crate::crash::log_path());
    if let Ok(exe) = env::current_exe() {
        println!("{:<9}{}", "program", exe.display());
    }
    println!(
        "\nSet PATHFINDER_HOME to keep everything in one folder, or the XDG_*_HOME variables.\n\
         Apps started outside your shell don't see your shell's variables: set them\n\
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
        if abs_var("PATHFINDER_HOME").is_some() || abs_var("XDG_CONFIG_HOME").is_some() {
            return;
        }
        assert_eq!(resolve(Kind::Config, |p| p == xdg), xdg);
        // A folder from before the rename keeps being used.
        let old = xdg_base(Kind::Config, &home).join(OLD_NAME);
        assert_eq!(resolve(Kind::Config, |p| p == old), old);
        assert_eq!(resolve(Kind::Config, |p| p == native), native);
        assert_eq!(resolve(Kind::Config, |_| false), native);
        assert_eq!(resolve(Kind::Config, |_| true), native);
    }

    #[test]
    fn xdg_defaults_follow_the_spec() {
        let home = PathBuf::from("/home/someone");
        assert_eq!(
            xdg_default(Kind::Data, &home),
            PathBuf::from("/home/someone/.local/share/pathfinder")
        );
        assert_eq!(
            xdg_default(Kind::Cache, &home),
            PathBuf::from("/home/someone/.cache/pathfinder")
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
