//! The agent's sandbox, run for real: a command must not reach anything
//! private to Pathfinder (theory A2), while the workspace, a staged
//! handoff's own copy included, stays readable and writable.
//!
//! These skip where the machine has no sandbox, unless
//! `PATHFINDER_REQUIRE_SANDBOX_TESTS=1` (CI on Linux and macOS), when a
//! missing sandbox fails them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use pathfinder::agent::sandbox::{self, Kind, Policy};
use pathfinder::paths;

const SECRET: &str = "PLANTED-FLAW-MARKER";

/// A fake Pathfinder home under target/ (not the temp folder, which
/// sandboxed commands may use), set as `PATHFINDER_HOME` before anything
/// reads the environment.
fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pf-private-{}", uuid::Uuid::new_v4().simple()));
        let home = root.join("pfhome");
        fs::create_dir_all(&home).unwrap();
        let root = root.canonicalize().unwrap();
        // SAFETY: set once, before any test in this binary reads the
        // environment (each goes through `root()` first).
        unsafe {
            std::env::set_var("PATHFINDER_HOME", root.join("pfhome"));
            for var in [
                "PATHFINDER_DB_PATH",
                "HARNESS_DB_PATH",
                "PATHFINDER_NO_SANDBOX",
            ] {
                std::env::remove_var(var);
            }
        }
        let home = root.join("pfhome");
        let put = |rel: &str, text: &str| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        };
        put("auth.json", &format!("{{\"key\":\"{SECRET}\"}}"));
        for f in [
            "harness.sqlite",
            "harness.sqlite-wal",
            "harness.sqlite-shm",
            "tui.log",
            "latest-version",
            "state/crash.log",
            "cache/models.json",
            "staging/sessA/p1-aaaaaaaa/answer.rs",
        ] {
            put(f, SECRET);
        }
        put("staging/sessB/p2-bbbbbbbb/mine.txt", "mine");
        root
    })
}

/// The sandbox, or None to skip.
fn sandbox() -> Option<Kind> {
    root();
    let kind = sandbox::available();
    if kind.is_none() {
        assert!(
            std::env::var_os("PATHFINDER_REQUIRE_SANDBOX_TESTS").is_none_or(|v| v != "1"),
            "PATHFINDER_REQUIRE_SANDBOX_TESTS=1 but there's no sandbox here"
        );
        eprintln!("no sandbox here; skipping");
    }
    kind
}

struct Run {
    ok: bool,
    out: String,
}

fn run(kind: &Kind, policy: &Policy, cmd: &str) -> Run {
    let argv = sandbox::wrap(kind, policy, cmd).unwrap();
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&policy.workspace)
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Run {
        ok: out.status.success(),
        out: text,
    }
}

/// Neither reading nor writing `path` works in the sandbox, and the file on
/// disk is unchanged.
fn unreachable(kind: &Kind, policy: &Policy, path: &Path) {
    let p = path.display();
    let r = run(kind, policy, &format!("cat '{p}'"));
    assert!(!r.ok && !r.out.contains(SECRET), "read {p}: {}", r.out);
    let before = fs::read(path).ok();
    let r = run(kind, policy, &format!("echo TAMPER >> '{p}'"));
    assert!(!r.ok, "wrote {p}: {}", r.out);
    assert_eq!(fs::read(path).ok(), before, "{p} changed");
}

#[test]
fn a_staged_handoff_sees_only_its_own_copy() {
    let Some(kind) = sandbox() else { return };
    let home = root().join("pfhome");
    let copy = home.join("staging/sessB/p2-bbbbbbbb");
    let policy = Policy::new(copy.clone(), false);
    for f in [
        "harness.sqlite",
        "harness.sqlite-wal",
        "harness.sqlite-shm",
        "auth.json",
        "tui.log",
        "latest-version",
        "state/crash.log",
        "cache/models.json",
        "staging/sessA/p1-aaaaaaaa/answer.rs",
    ] {
        unreachable(&kind, &policy, &home.join(f));
    }
    let r = run(
        &kind,
        &policy,
        &format!("ls '{}'", home.join("staging").display()),
    );
    assert!(!r.out.contains("sessA"), "listed other reviews: {}", r.out);
    // Its own copy, read and written by relative and absolute path.
    let r = run(
        &kind,
        &policy,
        &format!(
            "cat mine.txt && echo new > new.txt && cat '{}/mine.txt' && pwd",
            copy.display()
        ),
    );
    assert!(r.ok && r.out.contains("mine"), "{}", r.out);
    assert_eq!(fs::read_to_string(copy.join("new.txt")).unwrap(), "new\n");
}

#[test]
fn a_database_in_the_workspace_is_hidden_there() {
    let Some(kind) = sandbox() else { return };
    let ws = root().join("proj");
    let data = ws.join("data");
    fs::create_dir_all(data.join("staging/s/p1-cccccccc")).unwrap();
    for f in [
        "harness.sqlite",
        "harness.sqlite-wal",
        "tui.log",
        "staging/s/p1-cccccccc/answer.rs",
    ] {
        fs::write(data.join(f), SECRET).unwrap();
    }
    fs::write(data.join("notes.txt"), "user notes").unwrap();
    let mut policy = Policy::new(ws.clone(), false);
    policy.private = paths::private_paths_for(&data.join("harness.sqlite"));
    for f in [
        "harness.sqlite",
        "harness.sqlite-wal",
        "tui.log",
        "staging/s/p1-cccccccc/answer.rs",
    ] {
        unreachable(&kind, &policy, &data.join(f));
    }
    // The rest of the workspace, that folder included, is the user's.
    let r = run(
        &kind,
        &policy,
        "cat data/notes.txt && echo ok > data/out.txt",
    );
    assert!(r.ok && r.out.contains("user notes"), "{}", r.out);
    assert!(data.join("out.txt").is_file());
}

#[test]
fn a_workspace_above_pathfinders_folders_still_hides_them() {
    let Some(kind) = sandbox() else { return };
    let ws = root().to_path_buf();
    let policy = Policy::new(ws.clone(), false);
    let home = ws.join("pfhome");
    for f in [
        "auth.json",
        "harness.sqlite-wal",
        "staging/sessA/p1-aaaaaaaa/answer.rs",
    ] {
        unreachable(&kind, &policy, &home.join(f));
    }
    let r = run(&kind, &policy, "echo ok > above.txt && cat above.txt");
    assert!(r.ok && r.out.contains("ok"), "{}", r.out);
}

#[test]
fn pathfinder_itself_is_out_of_reach() {
    let Some(kind) = sandbox() else { return };
    let ws = root().join("pid-ws");
    fs::create_dir_all(&ws).unwrap();
    let policy = Policy::new(ws, false);
    let me = std::process::id();
    // Its open files (the database), its view of the disk, its arguments.
    for cmd in [
        format!("ls /proc/{me}/fd"),
        format!("cat /proc/{me}/fd/*"),
        format!("ls /proc/{me}/root/"),
        format!("cat /proc/{me}/cmdline"),
        format!("cat /proc/{me}/environ"),
        format!("kill -0 {me}"),
    ] {
        let r = run(&kind, &policy, &cmd);
        assert!(!r.ok, "`{cmd}` worked: {}", r.out);
    }
}

/// Seatbelt and bubblewrap check a path, not the file behind it: renaming a
/// folder above a private path (the workspace's own `data/`, or `Library/`
/// when the workspace is the home folder) moves the private files to a path
/// nothing hides. The rename itself must fail, or Pathfinder's files are
/// readable at the new name (macOS at once; Linux from the next command on,
/// when the masks are built for the old name).
#[test]
fn renaming_a_folder_above_a_private_path_doesnt_expose_it() {
    let Some(kind) = sandbox() else { return };
    // The database in the workspace (`PATHFINDER_DB_PATH`).
    let ws = root().join("proj-rename");
    let data = ws.join("data");
    fs::create_dir_all(data.join("staging/s/p1-dddddddd")).unwrap();
    fs::write(data.join("harness.sqlite"), SECRET).unwrap();
    fs::write(data.join("staging/s/p1-dddddddd/answer.rs"), SECRET).unwrap();
    let mut policy = Policy::new(ws.clone(), false);
    policy.private = paths::private_paths_for(&data.join("harness.sqlite"));
    let r = run(
        &kind,
        &policy,
        "mv data moved; cat moved/harness.sqlite moved/staging/s/*/answer.rs",
    );
    let moved = ws.join("moved");
    let renamed = moved.exists();
    if renamed {
        fs::rename(&moved, &data).unwrap();
    }
    assert!(!r.out.contains(SECRET), "read after a rename: {}", r.out);
    assert!(!renamed, "renamed the folder holding the database");

    // The workspace is the home folder: Pathfinder's folder two levels down.
    let home = root().join("home-rename");
    let app = home.join("Library/Application Support/pf");
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("auth.json"), SECRET).unwrap();
    let policy = Policy {
        workspace: home.clone(),
        network: false,
        private: paths::Private {
            dirs: vec![app.clone()],
            files: vec![],
            made: vec![],
        },
    };
    let r = run(
        &kind,
        &policy,
        "mv Library L2; cat 'L2/Application Support/pf/auth.json'",
    );
    let renamed = home.join("L2").exists();
    if renamed {
        fs::rename(home.join("L2"), home.join("Library")).unwrap();
    }
    assert!(!r.out.contains(SECRET), "read after a rename: {}", r.out);
    assert!(!renamed, "renamed a folder above Pathfinder's");
}

/// The masks are built from what exists when a command starts (on Linux a
/// tmpfs needs a folder to cover), but a command can run for minutes: a
/// private folder made while it runs (another review's `staging/` next to a
/// database set with `PATHFINDER_DB_PATH`, the state or cache folder on a
/// fresh install) must not be readable from it.
#[test]
fn a_private_folder_made_while_a_command_runs_stays_hidden() {
    let Some(kind) = sandbox() else { return };
    let ws = root().join("late-ws");
    fs::create_dir_all(&ws).unwrap();
    let work = root().join("late-db");
    fs::create_dir_all(&work).unwrap();
    let db = work.join("pf.sqlite");
    fs::write(&db, "").unwrap();
    let mut policy = Policy::new(ws, false);
    policy.private = paths::private_paths_for(&db);
    let staging = paths::staging_root(&db);
    assert!(!staging.exists());
    let other = staging.join("sessA/p1-eeeeeeee/answer.rs");
    let maker = {
        let other = other.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(700));
            fs::create_dir_all(other.parent().unwrap()).unwrap();
            fs::write(&other, SECRET).unwrap();
        })
    };
    let r = run(
        &kind,
        &policy,
        &format!("sleep 3; cat '{}'", other.display()),
    );
    maker.join().unwrap();
    assert!(
        !r.out.contains(SECRET),
        "read another review's copy made during the command: {}",
        r.out
    );
}

/// SQLite follows a symlinked database to its target and keeps `-wal` and
/// `-shm` beside the target, not beside the link: those must be hidden too.
#[test]
fn a_symlinked_database_hides_the_journal_beside_its_target() {
    let Some(kind) = sandbox() else { return };
    let ws = root().join("link-ws");
    let vault = root().join("link-vault");
    let work = root().join("link-work");
    for d in [&ws, &vault, &work] {
        fs::create_dir_all(d).unwrap();
    }
    let target = vault.join("pf.sqlite");
    fs::write(&target, SECRET).unwrap();
    let link = work.join("pf.sqlite");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    // Where SQLite puts them for a database opened through `link`.
    for f in ["pf.sqlite-wal", "pf.sqlite-shm"] {
        fs::write(vault.join(f), SECRET).unwrap();
    }
    let mut policy = Policy::new(ws, false);
    policy.private = paths::private_paths_for(&link);
    for f in ["pf.sqlite", "pf.sqlite-wal", "pf.sqlite-shm"] {
        unreachable(&kind, &policy, &vault.join(f));
    }
}

/// A command can't put a link where Pathfinder will later make one of its
/// files or folders (the staging root or the TUI's log beside a database in
/// the workspace), which would send them somewhere in sight.
#[test]
fn a_link_planted_where_pathfinder_writes_is_refused() {
    let Some(kind) = sandbox() else { return };
    let ws = root().join("plant-ws");
    let data = ws.join("data");
    fs::create_dir_all(ws.join("leak")).unwrap();
    fs::create_dir_all(&data).unwrap();
    let db = data.join("harness.sqlite");
    fs::write(&db, SECRET).unwrap();
    let mut policy = Policy::new(ws.clone(), false);
    policy.private = paths::private_paths_for(&db);
    let staging = paths::staging_root(&db);
    assert!(!staging.exists());
    run(
        &kind,
        &policy,
        "rm -rf data/staging; ln -s ../leak data/staging; ln -s ../leak/log data/tui.log; ln -s ../leak/v data/latest-version",
    );
    // No link at the staging root (on Linux the sandbox made it a plain
    // folder before the command; on macOS nothing can be made there).
    assert!(
        fs::symlink_metadata(&staging).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
            || !staging.exists() && fs::symlink_metadata(&staging).is_err(),
        "a link at the staging root"
    );
    paths::ensure_private_dir(&staging).unwrap();
    // Where a link could be made (Linux: the log isn't there yet), Pathfinder
    // won't write through it.
    for f in ["tui.log", "latest-version"] {
        let p = data.join(f);
        if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            let opened =
                paths::no_follow(fs::OpenOptions::new().create(true).append(true)).open(&p);
            assert!(opened.is_err(), "wrote through a link at {f}");
        }
    }
    assert!(fs::read_dir(ws.join("leak")).unwrap().next().is_none());
}
