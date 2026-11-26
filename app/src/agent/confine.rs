//! Keeps tool paths inside the workspace.

use std::path::{Component, Path, PathBuf};

/// Resolves `requested` against `root` and refuses anything outside it.
/// Symlinks and `..` are resolved through the longest existing ancestor, so a
/// link pointing outside the workspace is caught. Paths in `extra_readable`
/// (files the user attached) are allowed for reading.
pub fn confine(
    root: &Path,
    requested: &str,
    write: bool,
    extra_readable: &[PathBuf],
) -> Result<PathBuf, String> {
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
        return Ok(resolved);
    }
    if !resolved.starts_with(&root) {
        return Err(format!(
            "`{requested}` is outside the workspace; only paths under {} are allowed",
            root.display()
        ));
    }
    if write
        && resolved
            .strip_prefix(&root)
            .is_ok_and(|rel| rel.starts_with(".git"))
    {
        return Err("writing inside .git is not allowed".to_string());
    }
    if write
        && resolved
            .strip_prefix(&root)
            .is_ok_and(|rel| rel.starts_with(".pathfinder"))
    {
        return Err(
            "writing inside .pathfinder is not allowed: it holds the record of your understanding"
                .to_string(),
        );
    }
    Ok(resolved)
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
        dir.canonicalize().unwrap()
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
}
