//! Deterministic workspace snapshot handed to onboarding and planning, so the
//! model sees what the folder already answers (layout, stack, docs) instead of
//! asking the user.

use std::fs;
use std::path::Path;

const MAX_DEPTH: usize = 3;
const MAX_TREE_ENTRIES: usize = 150;
const MAX_EXCERPT_LINES: usize = 40;
const MAX_EXCERPT_BYTES: usize = 4_000;
const MAX_SECTION_LINES: usize = 14;
const MAX_KEY_FILES: usize = 6;

const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "out",
    "__pycache__",
    "venv",
    "vendor",
];

/// Manifest file name → stack label.
const MANIFESTS: &[(&str, &str)] = &[
    ("Cargo.toml", "Rust (Cargo)"),
    ("package.json", "JavaScript/TypeScript (npm)"),
    ("pyproject.toml", "Python"),
    ("requirements.txt", "Python"),
    ("go.mod", "Go"),
    ("Gemfile", "Ruby"),
    ("pom.xml", "Java (Maven)"),
    ("build.gradle", "JVM (Gradle)"),
    ("build.gradle.kts", "JVM (Gradle)"),
    ("Package.swift", "Swift (SwiftPM)"),
    ("mix.exs", "Elixir"),
    ("CMakeLists.txt", "C/C++ (CMake)"),
    ("flake.nix", "Nix"),
];

/// Docs worth excerpting, in priority order.
const DOC_FILES: &[&str] = &["README.md", "README", "AGENTS.md", "CLAUDE.md"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub tree: Vec<String>,
    pub truncated: bool,
    /// Detected stacks, e.g. `Rust (Cargo) in harness/`.
    pub stack: Vec<String>,
    /// (relative path, excerpt)
    pub key_files: Vec<(String, String)>,
}

impl WorkspaceSnapshot {
    pub fn scan(root: &Path) -> Self {
        let mut snap = Self::default();
        let mut manifests = Vec::new();
        let mut docs = Vec::new();
        walk(root, root, 0, &mut snap, &mut manifests, &mut docs);

        manifests.sort_by_key(|(rel, _)| depth_of(rel));
        docs.sort_by_key(|rel| {
            let name = rel.rsplit('/').next().unwrap_or(rel);
            let rank = DOC_FILES.iter().position(|d| *d == name).unwrap_or(99);
            (depth_of(rel), rank)
        });

        for (rel, label) in &manifests {
            let dir = rel.rsplit_once('/').map(|(d, _)| d);
            let entry = match dir {
                Some(d) => format!("{label} in {d}/"),
                None => format!("{label} at root"),
            };
            if !snap.stack.contains(&entry) {
                snap.stack.push(entry);
            }
        }

        let candidates = docs.iter().chain(manifests.iter().map(|(rel, _)| rel));
        for rel in candidates {
            if snap.key_files.len() >= MAX_KEY_FILES {
                break;
            }
            if snap.key_files.iter().any(|(p, _)| p == rel) {
                continue;
            }
            if let Some(excerpt) = excerpt(&root.join(rel)) {
                snap.key_files.push((rel.clone(), excerpt));
            }
        }

        snap
    }

    pub fn render(&self) -> String {
        if self.tree.is_empty() {
            return "(empty or unreadable folder)".to_string();
        }
        let mut out = String::new();
        if !self.stack.is_empty() {
            out.push_str("Detected stack: ");
            out.push_str(&self.stack.join("; "));
            out.push_str("\n\n");
        }
        out.push_str("Files (hidden and build dirs omitted):\n");
        out.push_str(&self.tree.join("\n"));
        if self.truncated {
            out.push_str("\n...(truncated)");
        }
        for (rel, text) in &self.key_files {
            out.push_str(&format!("\n\n--- {rel} (excerpt) ---\n{text}"));
        }
        out
    }
}

fn depth_of(rel: &str) -> usize {
    rel.matches('/').count()
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    snap: &mut WorkspaceSnapshot,
    manifests: &mut Vec<(String, &'static str)>,
    docs: &mut Vec<String>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| name.clone());

        if !is_dir {
            if let Some((_, label)) = MANIFESTS.iter().find(|(m, _)| *m == name) {
                manifests.push((rel.clone(), label));
            }
            if DOC_FILES.contains(&name.as_str()) {
                docs.push(rel.clone());
            }
        }

        if snap.tree.len() < MAX_TREE_ENTRIES {
            let indent = "  ".repeat(depth);
            let suffix = if is_dir { "/" } else { "" };
            snap.tree.push(format!("{indent}{name}{suffix}"));
        } else {
            snap.truncated = true;
        }

        if is_dir && depth + 1 < MAX_DEPTH {
            walk(root, &path, depth + 1, snap, manifests, docs);
        }
    }
}

fn excerpt(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let is_markdown = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("md"))
        .unwrap_or(false);
    let out = if is_markdown {
        markdown_excerpt(&raw)
    } else {
        head_excerpt(&raw)
    };
    let out = out.trim_end().to_string();
    (!out.is_empty()).then_some(out)
}

fn head_excerpt(raw: &str) -> String {
    let mut out = String::new();
    for (i, line) in raw.lines().enumerate() {
        if i >= MAX_EXCERPT_LINES || out.len() + line.len() > MAX_EXCERPT_BYTES {
            out.push_str("...");
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Keeps every heading and the first lines under each, so a late section
/// (e.g. a command list) is never cut off by a long early one.
fn markdown_excerpt(raw: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    let mut printed_open_fence = false;
    let mut section_lines = 0usize;
    let mut elided = false;

    for line in raw.lines() {
        let is_fence = line.trim_start().starts_with("```");

        if !in_fence && line.starts_with('#') {
            section_lines = 0;
            elided = false;
            if out.len() + line.len() > MAX_EXCERPT_BYTES {
                out.push_str("...\n");
                break;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if is_fence {
            in_fence = !in_fence;
            // Always close a fence whose opening line was printed.
            if !in_fence && printed_open_fence {
                if elided {
                    out.push_str("...\n");
                }
                out.push_str(line);
                out.push('\n');
                printed_open_fence = false;
                elided = true;
                continue;
            }
        }

        if section_lines >= MAX_SECTION_LINES || out.len() + line.len() > MAX_EXCERPT_BYTES {
            if !elided && !printed_open_fence {
                out.push_str("...\n");
            }
            elided = true;
            continue;
        }

        out.push_str(line);
        out.push('\n');
        section_lines += 1;
        if is_fence && in_fence {
            printed_open_fence = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-ws-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_nested_manifest_and_excerpts_docs() {
        let root = temp_root("nested");
        fs::create_dir_all(root.join("harness/src")).unwrap();
        fs::create_dir_all(root.join("harness/target/release")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(
            root.join("harness/Cargo.toml"),
            "[package]\nname = \"harness\"\n",
        )
        .unwrap();
        fs::write(
            root.join("harness/README.md"),
            "# harness\nPathfinder over ACP.\n",
        )
        .unwrap();
        fs::write(root.join("harness/src/main.rs"), "fn main() {}\n").unwrap();

        let snap = WorkspaceSnapshot::scan(&root);
        assert_eq!(snap.stack, vec!["Rust (Cargo) in harness/".to_string()]);
        assert!(snap.tree.iter().any(|l| l.trim() == "main.rs"));
        assert!(
            !snap
                .tree
                .iter()
                .any(|l| l.contains("target") || l.contains(".git"))
        );
        let paths: Vec<&str> = snap.key_files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["harness/README.md", "harness/Cargo.toml"]);

        let rendered = snap.render();
        assert!(rendered.starts_with("Detected stack: Rust (Cargo) in harness/"));
        assert!(rendered.contains("Pathfinder over ACP."));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn markdown_excerpt_keeps_late_sections() {
        let mut readme = String::from("# tool\n\n## Long section\n");
        for i in 0..60 {
            readme.push_str(&format!("filler line {i}\n"));
        }
        readme.push_str("\n## Slash commands\n\n- `/check` verify\n- `/map` show map\n");
        let out = markdown_excerpt(&readme);
        assert!(out.contains("## Slash commands"));
        assert!(out.contains("- `/map` show map"));
        assert!(!out.contains("filler line 30"));
    }

    #[test]
    fn markdown_excerpt_closes_cut_code_fences() {
        let mut readme = String::from("## Build\n```bash\n");
        for i in 0..30 {
            readme.push_str(&format!("step {i}\n"));
        }
        readme.push_str("```\n## Next\ntext\n");
        let out = markdown_excerpt(&readme);
        assert_eq!(out.matches("```").count(), 2, "unbalanced fences:\n{out}");
        assert!(out.contains("## Next\ntext"));
    }

    #[test]
    fn empty_folder_renders_placeholder() {
        let root = temp_root("empty");
        let snap = WorkspaceSnapshot::scan(&root);
        assert!(snap.stack.is_empty());
        assert_eq!(snap.render(), "(empty or unreadable folder)");
        fs::remove_dir_all(root).unwrap();
    }
}
