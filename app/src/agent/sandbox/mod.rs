//! The sandbox for the agent's shell commands: macOS Seatbelt
//! (`sandbox-exec`) or Linux bubblewrap (`bwrap`). Inside it a command can
//! read the disk except where credentials live and everything private to
//! Pathfinder ([`crate::paths::Private`]: its config, database, logs,
//! caches and the staging copies of other reviews), write only in the
//! workspace (not `.git` or `.pathfinder`) and the temp folders, and has no
//! network unless asked for. On Linux it also runs in its own process
//! namespace, so it can't see or signal Pathfinder or your other programs.
//! Secret-looking environment variables are removed.
//!
//! When the workspace is itself inside a private folder (a staged handoff's
//! copy), only the workspace is shown there.
//!
//! Both sandboxes check a path, not the file behind it, so the folders
//! above a hidden path that a command could otherwise rename (inside the
//! workspace or a temp folder) can't be renamed or removed: moving one
//! would carry Pathfinder's files to a name nothing hides.
//!
//! Windows has no such sandbox (AppContainer and Windows Sandbox don't fit a
//! shell command in the user's workspace), so there, as anywhere the probe
//! finds none, [`available`] is None and every command is shown to the user
//! and approved one by one: it fails closed, never silently open. The probe
//! runs the full policy, private paths hidden, so a machine that can't apply
//! it (no unprivileged user namespaces, a profile that doesn't compile) has
//! no sandbox rather than a weaker one.
//!
//! The Seatbelt base policy is adapted from OpenAI Codex (see NOTICE).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::paths::Private;

const SEATBELT: &str = "/usr/bin/sandbox-exec";
const BASE_POLICY: &str = include_str!("seatbelt_base.sbpl");
const NETWORK_POLICY: &str = include_str!("seatbelt_network.sbpl");
const PREFERENCES_POLICY: &str = include_str!("seatbelt_preferences.sbpl");

/// Folders under the home folder that hold credentials: hidden from
/// sandboxed commands, which could otherwise read keys into the
/// conversation. Pathfinder's own folders are added wherever they resolve
/// to: [`crate::paths::private_paths`].
const HIDDEN: [&str; 10] = [
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".password-store",
    ".netrc",
    ".config/gh",
    "Library/Keychains",
];

/// Which sandbox this machine has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Seatbelt,
    Bwrap(PathBuf),
}

/// What a sandboxed command may do beyond reading, and what it must not
/// see.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Readable and writable: the user's folder, or a staged handoff's copy.
    pub workspace: PathBuf,
    pub network: bool,
    /// Hidden, apart from the workspace when it lies inside.
    pub private: Private,
}

impl Policy {
    /// The policy for `workspace` with Pathfinder's private paths as the
    /// config resolves them now.
    pub fn new(workspace: PathBuf, network: bool) -> Self {
        Self {
            workspace,
            network,
            private: crate::paths::private_paths(),
        }
    }
}

/// The sandbox on this machine, probed once; None where there's none, and
/// then every command asks the user first.
pub fn available() -> Option<Kind> {
    static FOUND: OnceLock<Option<Kind>> = OnceLock::new();
    FOUND.get_or_init(probe).clone()
}

fn probe() -> Option<Kind> {
    if crate::envvar::get_os("PATHFINDER_NO_SANDBOX").is_some() {
        return None;
    }
    let kind = if cfg!(target_os = "macos") {
        Path::new(SEATBELT).is_file().then_some(Kind::Seatbelt)?
    } else if cfg!(target_os = "linux") {
        let bwrap = std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|p| p.join("bwrap"))
                .find(|p| p.is_file())
        })?;
        Kind::Bwrap(bwrap)
    } else {
        return None;
    };
    applies(&kind).then_some(kind)
}

/// Whether the whole policy, private paths hidden and (on Linux) its own
/// process and network namespaces, applies here. AppArmor and container
/// setups can refuse user namespaces; then there is no sandbox at all
/// rather than a weaker one.
fn applies(kind: &Kind) -> bool {
    let dir = std::env::temp_dir().join(format!(
        "pathfinder-probe-{}",
        uuid::Uuid::new_v4().simple()
    ));
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    // The masks as a command gets them, without making Pathfinder's folders
    // yet (each command does that, and fails if it can't).
    let mut policy = Policy::new(dir.clone(), false);
    policy.private.made.clear();
    let Ok(argv) = wrap(kind, &policy, "true") else {
        std::fs::remove_dir_all(&dir).ok();
        return false;
    };
    let ok = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    std::fs::remove_dir_all(&dir).ok();
    ok
}

/// Environment variables that look like credentials.
pub fn secret_env_names() -> Vec<String> {
    const MARKS: [&str; 7] = [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "COOKIE",
    ];
    let mut names: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter(|k| {
            let up = k.to_uppercase();
            MARKS.iter().any(|m| up.contains(m))
        })
        .collect();
    names.sort();
    names
}

fn home() -> Option<PathBuf> {
    crate::paths::home()
}

/// What a command must not see, as real paths: the credential folders and
/// everything private to Pathfinder.
struct Hidden {
    /// Those holding the workspace: hidden but for the workspace.
    outer: Vec<PathBuf>,
    /// All others, inside the workspace or elsewhere.
    inner: Vec<PathBuf>,
}

fn hidden(policy: &Policy) -> Hidden {
    let ws = real(&policy.workspace);
    let mut all: Vec<PathBuf> = home()
        .into_iter()
        .flat_map(|h| HIDDEN.iter().map(move |d| h.join(d)))
        .chain(policy.private.all().cloned())
        .map(|p| real(&p))
        // The root or a relative path would hide everything or nothing.
        .filter(|p| p.is_absolute() && p.parent().is_some())
        .collect();
    all.sort();
    all.dedup();
    let (outer, inner) = all.into_iter().partition(|p| ws.starts_with(p));
    Hidden { outer, inner }
}

/// The folders above a hidden path that a command could rename or remove,
/// carrying the hidden path to a name nothing hides: those inside one of
/// `writable` (the workspace itself included; another writable root, a
/// temp folder, isn't one: a command can't rename it), and not already
/// hidden. Seatbelt and bubblewrap both go by path, so these must stay put.
fn movable_ancestors(ws: &Path, hidden: &Hidden, writable: &[PathBuf]) -> Vec<PathBuf> {
    let concealed = |a: &Path| {
        hidden.inner.iter().any(|q| a.starts_with(q))
            || hidden
                .outer
                .iter()
                .any(|o| a.starts_with(o) && !a.starts_with(ws))
    };
    let mut found: Vec<PathBuf> = hidden
        .inner
        .iter()
        .chain(&hidden.outer)
        .flat_map(|p| p.ancestors().skip(1))
        .filter(|a| {
            writable
                .iter()
                .any(|r| a.starts_with(r) && (a != r || r == ws))
        })
        .filter(|a| !concealed(a))
        .map(Path::to_path_buf)
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The real path, as the kernel sees it (`/var` is `/private/var`): the
/// longest existing part resolved, the rest as given.
fn real(p: &Path) -> PathBuf {
    if let Ok(r) = p.canonicalize() {
        return r;
    }
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => real(parent).join(name),
        _ => p.to_path_buf(),
    }
}

/// A string literal for the Seatbelt profile language.
fn sb_string(p: &Path) -> String {
    let s = p.to_string_lossy();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `(require-not (subpath …))` for each of `paths`.
fn sb_not(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| format!(" (require-not (subpath {}))", sb_string(p)))
        .collect()
}

/// The Seatbelt profile for `policy`. Every allow leaves the hidden paths
/// out, and the final denies repeat that, so no earlier rule (Seatbelt
/// lets a later rule win) can open them.
pub fn seatbelt_profile(policy: &Policy) -> String {
    let ws = real(&policy.workspace);
    let Hidden { outer, inner } = hidden(policy);
    let all: Vec<PathBuf> = outer.iter().chain(&inner).cloned().collect();
    let mut profile = String::new();
    profile.push_str(BASE_POLICY);
    profile.push('\n');
    profile.push_str(PREFERENCES_POLICY);
    profile.push('\n');

    // Reads: everything but credentials and Pathfinder's own files, and
    // the workspace even inside one of those (a staging copy).
    profile.push_str(&format!(
        "\n; Reads anywhere except credentials and Pathfinder's private files.\n(allow file-read* (require-all (subpath \"/\"){}))\n",
        sb_not(&all)
    ));
    profile.push_str(&format!(
        "(allow file-read* (require-all (subpath {}){}))\n",
        sb_string(&ws),
        sb_not(&inner)
    ));

    // Writes: the workspace but its history, Pathfinder's record and
    // anything hidden, and the temp folders. Every write rule leaves these
    // out, even when the workspace sits in a temp folder.
    let protect = format!(
        "(require-not (subpath {})) (require-not (subpath {}))",
        sb_string(&ws.join(".git")),
        sb_string(&ws.join(".pathfinder")),
    );
    profile.push_str(&format!(
        "\n; Writes in the workspace, not in .git, .pathfinder or anything hidden.\n(allow file-write* (require-all (subpath {}) {protect}{}))\n",
        sb_string(&ws),
        sb_not(&inner),
    ));
    let mut temps = vec![PathBuf::from("/private/tmp")];
    let tmpdir = real(&std::env::temp_dir());
    // The per-user folder holding T/ (temp) and C/ (caches compilers use).
    temps.push(
        tmpdir
            .parent()
            .filter(|p| p.starts_with("/private/var/folders"))
            .map(Path::to_path_buf)
            .unwrap_or(tmpdir),
    );
    let mut writable = vec![ws.clone()];
    writable.extend(temps.iter().cloned());
    let pinned = movable_ancestors(
        &ws,
        &Hidden {
            outer: outer.clone(),
            inner: inner.clone(),
        },
        &writable,
    );
    let temps: Vec<String> = temps
        .iter()
        .map(|p| {
            format!(
                "(require-all (subpath {}) {protect}{})",
                sb_string(p),
                sb_not(&all)
            )
        })
        .collect();
    profile.push_str(&format!(
        "(allow file-write* {})\n(allow file-write-data (regex #\"^/dev/(null|zero|tty|stdout|stderr|fd/[0-9]+)$\"))\n",
        temps.join(" ")
    ));

    if policy.network {
        profile.push_str("\n; Network, asked for and allowed by the user.\n(allow network-outbound)\n(allow network-inbound)\n(allow system-socket)\n");
        profile.push_str(NETWORK_POLICY);
    }

    // Last, so they win over every allow above.
    profile.push_str("\n; Hidden, whatever allowed it above.\n");
    for p in &inner {
        profile.push_str(&format!(
            "(deny file-read* file-write* (subpath {}))\n",
            sb_string(p)
        ));
    }
    for p in &outer {
        profile.push_str(&format!(
            "(deny file-read* file-write* (require-all (subpath {}) (require-not (subpath {}))))\n",
            sb_string(p),
            sb_string(&ws)
        ));
    }
    // A rename or removal checks `file-write-unlink` on the folder itself;
    // files can still be made and changed inside it.
    profile.push_str("; Folders above a hidden path: not renamed or removed.\n");
    for a in &pinned {
        profile.push_str(&format!(
            "(deny file-write-unlink (literal {}))\n",
            sb_string(a)
        ));
    }
    profile
}

/// The bubblewrap arguments for `policy`, in mount order: later mounts
/// cover earlier ones. Folders holding the workspace are hidden first, the
/// workspace is mounted back over them, the folders in it above a hidden
/// path are mounted over themselves (a mount point can't be renamed or
/// removed), then everything else hidden is covered, inside the workspace
/// too: an empty read-only tmpfs over a folder, `/dev/null` over a file.
/// Only paths that exist are covered; [`prepare`] makes Pathfinder's own
/// folders first, so those always are.
fn bwrap_args(policy: &Policy) -> Vec<String> {
    let ws = real(&policy.workspace);
    let Hidden { outer, inner } = hidden(policy);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mut args: Vec<String> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        // Its own process namespace and /proc: Pathfinder's open database
        // (`/proc/<pid>/fd`), its view of the disk (`/proc/<pid>/root`) and
        // your other programs are out of sight and out of reach.
        "--unshare-pid",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
    ]
    .map(String::from)
    .to_vec();
    // Each tmpfs is made read-only at the end, once everything mounted
    // inside it (the workspace) is in place: writes there fail rather than
    // vanish.
    let mut tmpfs: Vec<&Path> = Vec::new();
    for p in &outer {
        args.extend(["--tmpfs".into(), s(p)]);
        tmpfs.push(p);
    }
    args.extend(["--bind".into(), s(&ws), s(&ws)]);
    let protected = [ws.join(".git"), ws.join(".pathfinder")];
    for p in &protected {
        if p.exists() {
            args.extend(["--ro-bind".into(), s(p), s(p)]);
        }
    }
    // Only the workspace is writable here (/tmp is the sandbox's own).
    let pinned = movable_ancestors(
        &ws,
        &Hidden {
            outer: outer.clone(),
            inner: inner.clone(),
        },
        std::slice::from_ref(&ws),
    );
    for a in pinned.iter().filter(|a| **a != ws && a.is_dir()) {
        let how = if protected.iter().any(|p| a.starts_with(p)) {
            "--ro-bind"
        } else {
            "--bind"
        };
        args.extend([how.into(), s(a), s(a)]);
    }
    let mut covered: Vec<&Path> = Vec::new();
    for p in &inner {
        // Already out of sight under a tmpfs, outside the workspace.
        let gone = covered.iter().any(|d| p.starts_with(d))
            || (!p.starts_with(&ws) && outer.iter().any(|o| p.starts_with(o)));
        if gone {
            continue;
        }
        if p.is_dir() {
            args.extend(["--tmpfs".into(), s(p)]);
            covered.push(p);
            tmpfs.push(p);
        } else if p.exists() {
            args.extend(["--ro-bind".into(), "/dev/null".into(), s(p)]);
        }
    }
    for p in tmpfs {
        args.extend(["--remount-ro".into(), s(p)]);
    }
    if !policy.network {
        args.push("--unshare-net".into());
    }
    args.extend(["--die-with-parent".into(), "--new-session".into()]);
    args
}

/// Before a bubblewrap command: makes Pathfinder's own folders (this user's
/// only) so the masks cover them from the start. A tmpfs needs a folder to
/// cover, and one made while the command runs (the staging root at the
/// first review) would otherwise be in plain sight, or a command could
/// put a link where it was going to be.
fn prepare(policy: &Policy) -> Result<(), String> {
    for d in &policy.private.made {
        crate::paths::ensure_private_dir(d)
            .map_err(|e| format!("couldn't make {} for the sandbox: {e}", d.display()))?;
    }
    Ok(())
}

/// The program and arguments that run `command` (a shell command line) in
/// the sandbox, with secret environment variables removed. An error means
/// the policy can't be applied, and the command must not run.
pub fn wrap(kind: &Kind, policy: &Policy, command: &str) -> Result<Vec<String>, String> {
    let mut argv: Vec<String> = vec!["/usr/bin/env".into()];
    for name in secret_env_names() {
        argv.push("-u".into());
        argv.push(name);
    }
    match kind {
        Kind::Seatbelt => {
            argv.extend([SEATBELT.into(), "-p".into(), seatbelt_profile(policy)]);
        }
        Kind::Bwrap(bwrap) => {
            prepare(policy)?;
            argv.push(bwrap.to_string_lossy().into_owned());
            argv.extend(bwrap_args(policy));
        }
    }
    argv.extend(["/bin/sh".into(), "-c".into(), command.into()]);
    Ok(argv)
}

/// `argv` as one shell command line, for a terminal that runs `sh -c`.
pub fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A note for the model when a sandboxed command's output looks like the
/// sandbox stopped it.
pub fn denial_hint(output: &str, network: bool) -> Option<String> {
    let blocked_file = ["Operation not permitted", "Read-only file system"]
        .iter()
        .any(|m| output.contains(m));
    let blocked_net = !network
        && [
            "Could not resolve host",
            "nodename nor servname",
            "Name or service not known",
            "Temporary failure in name resolution",
            "Network is unreachable",
            "Failed to establish a new connection",
        ]
        .iter()
        .any(|m| output.contains(m));
    if !(blocked_file || blocked_net) {
        return None;
    }
    let mut hint = String::from(
        "\n\n(This looks blocked by the sandbox: commands can write only inside the workspace (not .git or .pathfinder) and the temp folders, and have no network.",
    );
    if blocked_net {
        hint.push_str(
            " If it needs the internet, run it again with `network: true` and a `justification`.",
        );
    }
    if blocked_file {
        hint.push_str(" If it really must write elsewhere or into .git, run it again with `outside_sandbox: true` and a `justification`.");
    }
    hint.push_str(" The user is asked first.)");
    Some(hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// The sandbox, or None to skip; with `PATHFINDER_REQUIRE_SANDBOX_TESTS=1`
    /// (CI on Linux and macOS) a missing one fails the test instead.
    fn sandbox() -> Option<Kind> {
        let kind = available();
        if kind.is_none() {
            assert!(
                std::env::var_os("PATHFINDER_REQUIRE_SANDBOX_TESTS").is_none_or(|v| v != "1"),
                "PATHFINDER_REQUIRE_SANDBOX_TESTS=1 but there's no sandbox here"
            );
            eprintln!("no sandbox here; skipping");
        }
        kind
    }

    fn workspace() -> PathBuf {
        // Under target/, not the temp folder, which the sandbox lets commands
        // write to.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("pf-sandbox-{}", uuid::Uuid::new_v4().simple()))
            .join("ws");
        fs::create_dir_all(dir.join(".git")).unwrap();
        dir.canonicalize().unwrap()
    }

    fn run(policy: &Policy, cmd: &str) -> (bool, String) {
        let Some(kind) = available() else {
            return (true, "no sandbox".into());
        };
        let argv = wrap(&kind, policy, cmd).unwrap();
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&policy.workspace)
            .output()
            .unwrap();
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    /// The real private paths, without making the folders on this machine.
    fn real_policy(ws: &Path) -> Policy {
        let mut policy = Policy::new(ws.to_path_buf(), false);
        policy.private.made.clear();
        policy
    }

    // Only the Unix profile tests below build policies by hand.
    #[cfg(unix)]
    fn policy_at(workspace: &Path, private: Private) -> Policy {
        Policy {
            workspace: workspace.to_path_buf(),
            network: false,
            private,
        }
    }

    // The profile is Seatbelt's and its paths are Unix ones; Windows has no
    // sandbox to check.
    #[cfg(unix)]
    #[test]
    fn pathfinders_private_paths_are_hidden_wherever_they_live() {
        let policy = Policy::new(PathBuf::from("/tmp/ws"), false);
        let profile = seatbelt_profile(&policy);
        for p in policy.private.all() {
            let deny = format!(
                "(deny file-read* file-write* (subpath {}))",
                sb_string(&real(p))
            );
            assert!(profile.contains(&deny), "{} isn't hidden", p.display());
        }
        // Both conventions, so another environment can't expose them.
        assert!(
            profile.contains("Application Support/com.lprsrts.pathfinder")
                || !cfg!(target_os = "macos")
        );
        assert!(profile.contains(".config/harness"));
        assert!(profile.contains(".local/share/harness"));
    }

    #[cfg(unix)]
    #[test]
    fn the_workspace_shows_through_a_private_folder_holding_it() {
        let data = PathBuf::from("/srv/pf-data");
        let ws = data.join("staging").join("s").join("p1-abcd1234");
        let db = data.join("harness.sqlite");
        let private = Private {
            dirs: vec![data.clone(), data.join("staging")],
            files: vec![db.clone()],
            made: vec![],
        };
        let policy = policy_at(&ws, private);
        let Hidden { outer, inner } = hidden(&policy);
        assert_eq!(outer, vec![data.clone(), data.join("staging")]);
        assert!(inner.contains(&db));
        // Seatbelt: the workspace is read and written; the rest of the
        // folders are denied after every allow.
        let profile = seatbelt_profile(&policy);
        let last_allow = profile.rfind("(allow ").unwrap();
        let deny = profile
            .find(&format!(
                "(deny file-read* file-write* (require-all (subpath {}) (require-not (subpath {}))))",
                sb_string(&data),
                sb_string(&ws)
            ))
            .unwrap();
        assert!(deny > last_allow);
        // bubblewrap: the folders go first, the workspace over them.
        let args = bwrap_args(&policy);
        let at = |a: &[&str]| {
            args.windows(a.len())
                .position(|w| w.iter().zip(a).all(|(x, y)| x == y))
                .unwrap_or_else(|| panic!("{a:?} not in {args:?}"))
        };
        let ws_s = ws.to_string_lossy();
        let data_s = data.to_string_lossy();
        assert!(at(&["--tmpfs", &data_s]) < at(&["--bind", &ws_s, &ws_s]));
        assert!(at(&["--bind", &ws_s, &ws_s]) < at(&["--remount-ro", &data_s]));
        assert!(at(&["--unshare-pid", "--proc", "/proc"]) > 0);
    }

    #[cfg(unix)]
    #[test]
    fn folders_above_a_hidden_path_in_the_workspace_stay_put() {
        let ws = PathBuf::from("/w");
        let hidden = Hidden {
            outer: vec![],
            inner: vec![
                PathBuf::from("/w/data/harness.sqlite"),
                PathBuf::from("/w/data/staging"),
                PathBuf::from("/w/a/b/.ssh"),
                PathBuf::from("/elsewhere/x"),
            ],
        };
        let pinned = movable_ancestors(&ws, &hidden, std::slice::from_ref(&ws));
        let want: Vec<PathBuf> = ["/w", "/w/a", "/w/a/b", "/w/data"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(pinned, want);
        // A staged copy inside a hidden folder: only the folders inside the
        // copy (none here), not the hidden ones around it.
        let ws = PathBuf::from("/d/staging/s/p");
        let hidden = Hidden {
            outer: vec![PathBuf::from("/d"), PathBuf::from("/d/staging")],
            inner: vec![PathBuf::from("/d/harness.sqlite")],
        };
        assert!(movable_ancestors(&ws, &hidden, std::slice::from_ref(&ws)).is_empty());
    }

    #[test]
    fn commands_write_only_in_the_workspace_and_temp() {
        if sandbox().is_none() {
            return;
        }
        let ws = workspace();
        let policy = real_policy(&ws);
        let (ok, out) = run(
            &policy,
            "echo hi > notes.txt && cat notes.txt && ls / >/dev/null",
        );
        assert!(ok && out.contains("hi"), "{out}");
        let (ok, out) = run(&policy, "t=$(mktemp) && echo x > \"$t\" && rm \"$t\"");
        assert!(ok, "temp files: {out}");
        let (ok, _) = run(&policy, "echo x > .git/hooks-evil");
        assert!(!ok, "wrote into .git");
        let outside = ws.parent().unwrap().join("pf-escape.txt");
        let (ok, _) = run(&policy, &format!("echo x > {}", outside.display()));
        assert!(!ok && !outside.exists(), "wrote outside the workspace");
        let home = home().unwrap();
        let (ok, _) = run(
            &policy,
            &format!("touch {}/pf-sandbox-probe", home.display()),
        );
        assert!(!ok, "wrote into the home folder");
        fs::remove_dir_all(ws.parent().unwrap()).unwrap();
    }

    #[test]
    fn credentials_and_network_stay_out_of_reach() {
        if sandbox().is_none() {
            return;
        }
        let ws = workspace();
        let policy = real_policy(&ws);
        let home = home().unwrap();
        if home.join(".ssh").is_dir() {
            // Seatbelt refuses; bubblewrap shows an empty folder.
            let (ok, out) = run(&policy, &format!("ls -A {}/.ssh", home.display()));
            assert!(!ok || out.trim().is_empty(), "listed ~/.ssh: {out}");
        }
        // SAFETY: tests in this module don't read this variable concurrently.
        unsafe { std::env::set_var("PF_TEST_API_KEY", "sekrit") };
        let (_, out) = run(&policy, "echo \"[$PF_TEST_API_KEY]\"");
        assert!(out.contains("[]"), "secret env leaked: {out}");
        let (ok, _) = run(
            &policy,
            "curl -sS --max-time 5 https://example.com >/dev/null",
        );
        assert!(!ok, "reached the network");
        fs::remove_dir_all(ws.parent().unwrap()).unwrap();
    }

    #[test]
    fn hints_name_the_way_out() {
        let h = denial_hint("touch: /etc/x: Operation not permitted", false).unwrap();
        assert!(h.contains("outside_sandbox"));
        let h = denial_hint("curl: (6) Could not resolve host: x", false).unwrap();
        assert!(h.contains("network: true"));
        assert!(denial_hint("all good", false).is_none());
        assert_eq!(
            shell_join(&["/usr/bin/env".into(), "it's".into(), "a b".into()]),
            "/usr/bin/env 'it'\\''s' 'a b'"
        );
    }
}
