//! Keeps tool paths inside the workspace.

use std::path::{Component, Path, PathBuf};

/// Resolves `requested` against `root` and refuses anything outside it.
/// Symlinks and `..` are resolved through the longest existing ancestor, so a
/// link pointing outside the workspace is caught. Paths in `extra_readable`
/// (files the user attached) are allowed for reading.
/// Pathfinder's private files ([`crate::paths::Private`]) are refused too,
/// even inside the workspace (a database set there with
/// `PATHFINDER_DB_PATH`), unless the workspace itself lies inside one (a
/// staging copy).
pub fn confine(
    root: &Path,
    requested: &str,
    write: bool,
    extra_readable: &[PathBuf],
) -> Result<PathBuf, String> {
    confine_with(
        root,
        requested,
        write,
        extra_readable,
        &crate::paths::private_paths(),
    )
}

fn confine_with(
    root: &Path,
    requested: &str,
    write: bool,
    extra_readable: &[PathBuf],
    private: &crate::paths::Private,
) -> Result<PathBuf, String> {
    // Every comparison below is between paths from std's canonicalize, so
    // both sides share one form (on Windows the verbatim `\\?\C:\...`). Only
    // the result and the messages are simplified to the familiar `C:\...`.
    let root = root
        .canonicalize()
        .map_err(|e| format!("workspace unavailable: {e}"))?;
    let requested = requested.trim();
    let joined = if Path::new(requested).is_absolute() {
        PathBuf::from(requested)
    } else {
        root.join(requested)
    };
    let resolved = resolve(&joined)?;

    if !write
        && extra_readable
            .iter()
            .filter_map(|p| p.canonicalize().ok())
            .any(|p| p == resolved)
    {
        return Ok(plain(resolved));
    }
    if !resolved.starts_with(&root) {
        return Err(format!(
            "`{requested}` is outside the workspace; only paths under {} are allowed",
            dunce::simplified(&root).display()
        ));
    }
    if hidden_from(&root, private)
        .iter()
        .any(|p| under(&resolved, p))
    {
        return Err(format!(
            "`{requested}` is one of Pathfinder's own files (its record, settings or another review's copy), which the agent's tools can't open"
        ));
    }
    if write {
        let first = resolved
            .strip_prefix(&root)
            .ok()
            .and_then(|rel| rel.components().next());
        if let Some(Component::Normal(first)) = first {
            if is_protected(first, ".git") {
                return Err("writing inside .git is not allowed".to_string());
            }
            if is_protected(first, ".pathfinder") {
                return Err(
                    "writing inside .pathfinder is not allowed: it holds the record of your understanding"
                        .to_string(),
                );
            }
        }
    }
    Ok(plain(resolved))
}

/// Whether `path` is `prefix` or inside it. On macOS and Windows, whose
/// disks ignore case by default, case is ignored too: a private file that
/// doesn't exist yet (a database's `-journal`) keeps the case it was
/// spelled in, and `HARNESS.SQLITE-JOURNAL` is the same file there. On a
/// disk there that does tell case apart this refuses a little more than
/// it must, never less.
fn under(path: &Path, prefix: &Path) -> bool {
    if path.starts_with(prefix) {
        return true;
    }
    if !cfg!(any(target_os = "macos", windows)) {
        return false;
    }
    let fold = |c: Component| c.as_os_str().to_string_lossy().to_lowercase();
    let mut parts = path.components();
    prefix
        .components()
        .all(|p| parts.next().is_some_and(|c| fold(c) == fold(p)))
}

/// Pathfinder's private paths as real paths, less those holding `root` (a
/// staging copy's workspace lies inside one): what no tool may show.
fn hidden_from(root: &Path, private: &crate::paths::Private) -> Vec<PathBuf> {
    private
        .all()
        .filter_map(|p| resolve(p).ok())
        .filter(|p| !root.starts_with(p))
        .collect()
}

/// What a walk from the workspace (`ls`, `glob`, `grep`) may show: an entry
/// whose real path, links followed, lies inside the workspace and outside
/// Pathfinder's private paths, the same rule [`confine`] applies to the
/// path a tool starts from. A database set inside the workspace, a staging
/// copy beside it, or a link into a private folder is neither named nor
/// read (theory §11, A2).
#[derive(Debug, Clone)]
pub struct Visible {
    root: PathBuf,
    hidden: Vec<PathBuf>,
}

impl Visible {
    /// For `root` with the current setup's private paths.
    pub fn new(root: &Path) -> Self {
        Self::with(root, &crate::paths::private_paths())
    }

    /// For `root` with `private` as the private paths.
    pub fn with(root: &Path, private: &crate::paths::Private) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let hidden = hidden_from(&root, private);
        Self { root, hidden }
    }

    /// Whether a walk may name `path` and read it. A link that doesn't
    /// resolve is left out too: its target can't be checked.
    pub fn allows(&self, path: &Path) -> bool {
        let Ok(real) = path.canonicalize() else {
            return false;
        };
        real.starts_with(&self.root) && !self.hidden.iter().any(|h| under(&real, h))
    }

    /// A walk from `base` that never enters or yields a path this hides.
    pub fn walk(&self, base: &Path) -> ignore::WalkBuilder {
        let mut walk = ignore::WalkBuilder::new(base);
        let me = self.clone();
        walk.filter_entry(move |e| me.allows(e.path()));
        walk
    }
}

/// The plain form of a canonical path (`C:\...` rather than `\\?\C:\...` on
/// Windows, whenever that names the same file); unchanged elsewhere.
fn plain(path: PathBuf) -> PathBuf {
    dunce::simplified(&path).to_path_buf()
}

/// Whether a workspace-relative first component names the protected folder
/// `name`. Case is ignored: on NTFS and on default (case-insensitive) APFS,
/// `.GIT` is `.git`, and a folder that doesn't exist yet isn't canonicalized
/// to its on-disk case. On Windows, Win32 also drops trailing dots and spaces
/// (`.git. `) and reads `name:stream` / `name::$INDEX_ALLOCATION` as `name`.
fn is_protected(component: &std::ffi::OsStr, name: &str) -> bool {
    let text = component.to_string_lossy();
    #[cfg(windows)]
    let text = {
        let base = text.split(':').next().unwrap_or("");
        base.trim_end_matches(['.', ' ']).to_string()
    };
    text.eq_ignore_ascii_case(name)
}

/// Canonicalizes the longest existing prefix and appends the rest normally.
fn resolve(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut tail = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                existing.pop();
            }
            None => break,
        }
    }
    let mut out = existing
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
    for part in tail.into_iter().rev() {
        let p = Path::new(&part);
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::Normal(n) => out.push(n),
                _ => {}
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-confine-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "x").unwrap();
        // The plain form `confine` returns (`C:\...` on Windows).
        dunce::canonicalize(dir).unwrap()
    }

    #[test]
    fn allows_inside_and_refuses_escapes() {
        let ws = workspace();
        assert_eq!(
            confine(&ws, "src/a.rs", false, &[]).unwrap(),
            ws.join("src/a.rs")
        );
        assert_eq!(confine(&ws, ".", false, &[]).unwrap(), ws);
        assert!(confine(&ws, "../outside", false, &[]).is_err());
        assert!(confine(&ws, "src/../../outside", false, &[]).is_err());
        assert!(confine(&ws, "/etc/passwd", false, &[]).is_err());
        // A new file in a new directory is fine for writing.
        assert!(confine(&ws, "new/dir/b.rs", true, &[]).is_ok());
        assert!(confine(&ws, ".git/hooks/pre-commit", true, &[]).is_err());
        assert!(confine(&ws, ".git/config", false, &[]).is_ok());
        // Protected folders are matched without regard to case, even before
        // they exist.
        assert!(confine(&ws, ".GIT/hooks/pre-commit", true, &[]).is_err());
        assert!(confine(&ws, ".Pathfinder/x.json", true, &[]).is_err());
        assert!(confine(&ws, "src/../.pathfinder/x.json", true, &[]).is_err());
        assert!(confine(&ws, ".gitignore", true, &[]).is_ok());
        fs::remove_dir_all(ws).unwrap();
    }

    #[test]
    fn refuses_pathfinders_files_inside_the_workspace() {
        let ws = workspace();
        let db = ws.join("data").join("harness.sqlite");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        fs::write(&db, "db").unwrap();
        fs::write(ws.join("data").join("notes.txt"), "mine").unwrap();
        let private = crate::paths::private_paths_for(&db);
        let open = |p: &str, write| confine_with(&ws, p, write, &[], &private);
        assert!(open("data/harness.sqlite", false).is_err());
        assert!(open("data/harness.sqlite-wal", true).is_err());
        assert!(open("data/tui.log", false).is_err());
        assert!(open("data/staging/s/p1/answer.rs", false).is_err());
        assert!(open("data/notes.txt", false).is_ok());
        // A workspace inside a private folder (a staging copy) still works.
        let copy = ws.join("data").join("staging").join("s").join("p1");
        fs::create_dir_all(&copy).unwrap();
        let copy = dunce::canonicalize(copy).unwrap();
        assert!(confine_with(&copy, "a.rs", true, &[], &private).is_ok());
        assert!(confine_with(&copy, "../../../harness.sqlite", false, &[], &private).is_err());
        fs::remove_dir_all(ws).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_that_leave_the_workspace() {
        let ws = workspace();
        let outside = std::env::temp_dir().join(format!("harness-out-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("link")).unwrap();
        assert!(confine(&ws, "link/secret", false, &[]).is_err());
        // Attached files may be read even outside the workspace.
        let attached = outside.join("secret");
        assert!(
            confine(
                &ws,
                attached.to_str().unwrap(),
                false,
                std::slice::from_ref(&attached)
            )
            .is_ok()
        );
        assert!(
            confine(
                &ws,
                attached.to_str().unwrap(),
                true,
                std::slice::from_ref(&attached)
            )
            .is_err()
        );
        fs::remove_dir_all(ws).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    /// Adversarial: on a case-insensitive disk (macOS, Windows) a private
    /// file that doesn't exist yet (a database's `-journal` or `-wal`) is
    /// the same file in any case. Writing `HARNESS.SQLITE-JOURNAL` plants a
    /// hot journal SQLite rolls the record back from on its next open.
    #[test]
    fn refuses_pathfinders_files_in_another_case() {
        let ws = workspace();
        let db = ws.join("data").join("harness.sqlite");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        fs::write(&db, "db").unwrap();
        // Only meaningful where the disk ignores case.
        if !ws.join("DATA").join("HARNESS.SQLITE").exists() {
            fs::remove_dir_all(ws).unwrap();
            return;
        }
        let private = crate::paths::private_paths_for(&db);
        let open = |p: &str, write| confine_with(&ws, p, write, &[], &private);
        assert!(open("data/harness.sqlite-journal", true).is_err());
        assert!(open("data/HARNESS.SQLITE-JOURNAL", true).is_err());
        assert!(open("data/harness.sqlite-WAL", true).is_err());
        assert!(open("data/TUI.LOG", true).is_err());
        fs::remove_dir_all(ws).unwrap();
    }
}
