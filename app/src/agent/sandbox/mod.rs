//! The sandbox for the agent's shell commands: macOS Seatbelt
//! (`sandbox-exec`) or Linux bubblewrap (`bwrap`). Inside it a command can
//! read the disk except where credentials live, write only in the workspace
//! (not `.git` or `.pathfinder`) and the temp folders, and has no network
//! unless asked for. Secret-looking environment variables are removed.
//!
//! The Seatbelt base policy is adapted from OpenAI Codex (see NOTICE).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const SEATBELT: &str = "/usr/bin/sandbox-exec";
const BASE_POLICY: &str = include_str!("seatbelt_base.sbpl");
const NETWORK_POLICY: &str = include_str!("seatbelt_network.sbpl");
const PREFERENCES_POLICY: &str = include_str!("seatbelt_preferences.sbpl");

/// Folders under the home folder that hold credentials: hidden from
/// sandboxed commands, which could otherwise read keys into the
/// conversation.
/// Pathfinder's own config folder (with its secrets) is added wherever it
/// resolves to: [`crate::paths::secret_dirs`].
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

/// What a sandboxed command may do beyond reading.
#[derive(Debug, Clone)]
pub struct Policy {
    pub workspace: PathBuf,
    pub network: bool,
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
    if cfg!(target_os = "macos") {
        let ok = std::process::Command::new(SEATBELT)
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .status()
            .is_ok_and(|s| s.success());
        return ok.then_some(Kind::Seatbelt);
    }
    if cfg!(target_os = "linux") {
        let bwrap = std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|p| p.join("bwrap"))
                .find(|p| p.is_file())
        })?;
        // AppArmor and container setups can refuse user namespaces.
        let ok = std::process::Command::new(&bwrap)
            .args(["--ro-bind", "/", "/", "--unshare-net", "true"])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        return ok.then_some(Kind::Bwrap(bwrap));
    }
    None
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

/// Every credential folder, as absolute paths.
fn hidden() -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = home()
        .into_iter()
        .flat_map(|h| HIDDEN.iter().map(move |d| h.join(d)))
        .collect();
    all.extend(crate::paths::secret_dirs());
    all
}

/// The real path, as the kernel sees it (`/var` is `/private/var`); the
/// path itself when it doesn't exist yet.
fn real(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// A string literal for the Seatbelt profile language.
fn sb_string(p: &Path) -> String {
    let s = p.to_string_lossy();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The Seatbelt profile for `policy`.
pub fn seatbelt_profile(policy: &Policy) -> String {
    let ws = real(&policy.workspace);
    let mut profile = String::new();
    profile.push_str(BASE_POLICY);
    profile.push('\n');
    profile.push_str(PREFERENCES_POLICY);
    profile.push('\n');

    // Reads: everything but the credential folders.
    let hidden: Vec<String> = hidden()
        .iter()
        .map(|p| format!("(require-not (subpath {}))", sb_string(&real(p))))
        .collect();
    profile.push_str(&format!(
        "\n; Reads anywhere except where credentials live.\n(allow file-read* (require-all (subpath \"/\") {}))\n",
        hidden.join(" ")
    ));

    // Writes: the workspace but its history and Pathfinder's record, and
    // the temp folders.
    // Every write rule leaves these out, even when the workspace sits in a
    // temp folder.
    let protect = format!(
        "(require-not (subpath {})) (require-not (subpath {}))",
        sb_string(&ws.join(".git")),
        sb_string(&ws.join(".pathfinder")),
    );
    profile.push_str(&format!(
        "\n; Writes in the workspace, not in .git or .pathfinder.\n(allow file-write* (require-all (subpath {}) {protect}))\n",
        sb_string(&ws),
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
    let temps: Vec<String> = temps
        .iter()
        .map(|p| format!("(require-all (subpath {}) {protect})", sb_string(p)))
        .collect();
    profile.push_str(&format!(
        "(allow file-write* {})\n(allow file-write-data (regex #\"^/dev/(null|zero|tty|stdout|stderr|fd/[0-9]+)$\"))\n",
        temps.join(" ")
    ));

    if policy.network {
        profile.push_str("\n; Network, asked for and allowed by the user.\n(allow network-outbound)\n(allow network-inbound)\n(allow system-socket)\n");
        profile.push_str(NETWORK_POLICY);
    }
    profile
}

/// The program and arguments that run `command` (a shell command line) in
/// the sandbox, with secret environment variables removed.
pub fn wrap(kind: &Kind, policy: &Policy, command: &str) -> Vec<String> {
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
            let ws = real(&policy.workspace);
            let s = |p: &Path| p.to_string_lossy().into_owned();
            argv.push(s(bwrap));
            argv.extend(
                [
                    "--ro-bind",
                    "/",
                    "/",
                    "--dev",
                    "/dev",
                    "--proc",
                    "/proc",
                    "--tmpfs",
                    "/tmp",
                ]
                .map(String::from),
            );
            argv.extend(["--bind".into(), s(&ws), s(&ws)]);
            for sub in [".git", ".pathfinder"] {
                let p = ws.join(sub);
                if p.exists() {
                    argv.extend(["--ro-bind".into(), s(&p), s(&p)]);
                }
            }
            for p in hidden() {
                if p.is_dir() {
                    argv.extend(["--tmpfs".into(), s(&p)]);
                } else if p.is_file() {
                    argv.extend(["--ro-bind".into(), "/dev/null".into(), s(&p)]);
                }
            }
            if !policy.network {
                argv.push("--unshare-net".into());
            }
            argv.extend(["--die-with-parent".into(), "--new-session".into()]);
        }
    }
    argv.extend(["/bin/sh".into(), "-c".into(), command.into()]);
    argv
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
        let argv = wrap(&kind, policy, cmd);
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&policy.workspace)
            .output()
            .unwrap();
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    #[test]
    fn pathfinders_secrets_are_hidden_wherever_they_live() {
        let profile = seatbelt_profile(&Policy {
            workspace: PathBuf::from("/tmp/ws"),
            network: false,
        });
        for dir in crate::paths::secret_dirs() {
            assert!(
                profile.contains(&sb_string(&real(&dir))),
                "{} isn't hidden",
                dir.display()
            );
        }
        // Both conventions, so another environment can't expose them.
        assert!(
            profile.contains("Application Support/com.lprsrts.pathfinder")
                || !cfg!(target_os = "macos")
        );
        assert!(profile.contains(".config/harness"));
    }

    #[test]
    fn commands_write_only_in_the_workspace_and_temp() {
        if available().is_none() {
            eprintln!("no sandbox here; skipping");
            return;
        }
        let ws = workspace();
        let policy = Policy {
            workspace: ws.clone(),
            network: false,
        };
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
        if available().is_none() {
            return;
        }
        let ws = workspace();
        let policy = Policy {
            workspace: ws.clone(),
            network: false,
        };
        let home = home().unwrap();
        if home.join(".ssh").is_dir() {
            let (ok, out) = run(&policy, &format!("ls {}/.ssh", home.display()));
            assert!(!ok, "listed ~/.ssh: {out}");
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
