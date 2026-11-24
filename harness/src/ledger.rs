//! Provenance (theory §12, A7): what the agent changed, tied to the
//! concepts it relies on and the state of your understanding at that
//! moment, in a hash-chained, append-only ledger, and the understanding
//! coverage computed from it.
//!
//! Changes are found by snapshotting the workspace as a git tree before and
//! after the agent runs (through a scratch index, so your own index is never
//! touched), which works the same for every route.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const GENESIS: &str = "genesis";

fn git(root: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(args);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd.output().context("running git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The git work tree containing `cwd`, if any.
pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    git(cwd, &["rev-parse", "--show-toplevel"], None)
        .ok()
        .map(|s| PathBuf::from(s.trim()))
}

/// The workspace's files (tracked and untracked, not ignored) as a tree id.
pub fn snapshot(root: &Path) -> Result<String> {
    let real = git(root, &["rev-parse", "--git-path", "index"], None)?;
    let real = root.join(real.trim());
    let scratch =
        std::env::temp_dir().join(format!("harness-index-{}", uuid::Uuid::new_v4().simple()));
    // Starting from a copy of your index reuses its file stat cache.
    if real.exists() {
        std::fs::copy(&real, &scratch).ok();
    }
    let tree = git(root, &["add", "-A"], Some(&scratch))
        .and_then(|_| git(root, &["write-tree"], Some(&scratch)));
    std::fs::remove_file(&scratch).ok();
    Ok(tree?.trim().to_string())
}

fn scratch_index(root: &Path) -> Result<PathBuf> {
    let real = git(root, &["rev-parse", "--git-path", "index"], None)?;
    let real = root.join(real.trim());
    let scratch =
        std::env::temp_dir().join(format!("harness-index-{}", uuid::Uuid::new_v4().simple()));
    if real.exists() {
        std::fs::copy(&real, &scratch).ok();
    }
    Ok(scratch)
}

/// Like `snapshot`, of another directory using `root`'s object store and
/// ignore rules; `drop` names entries to leave out (the staging copy's
/// links to ignored folders).
pub fn snapshot_worktree(root: &Path, worktree: &Path, drop: &[String]) -> Result<String> {
    let scratch = scratch_index(root)?;
    let wt = format!("--work-tree={}", worktree.display());
    let mut result = git(root, &[&wt, "add", "-A"], Some(&scratch));
    for name in drop {
        if result.is_ok() {
            result = git(
                root,
                &["rm", "-r", "-q", "--cached", "--ignore-unmatch", "--", name],
                Some(&scratch),
            );
        }
    }
    let tree = result.and_then(|_| git(root, &["write-tree"], Some(&scratch)));
    std::fs::remove_file(&scratch).ok();
    Ok(tree?.trim().to_string())
}

/// Writes `tree` out into `dir` (a staging copy), then links the
/// workspace's ignored top-level entries (dependencies, virtualenvs, build
/// folders) so builds and tests work there. Returns the linked names.
pub fn materialize(root: &Path, tree: &str, dir: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let scratch = scratch_index(root)?;
    let prefix = format!("--prefix={}/", dir.display());
    let result = git(root, &["read-tree", tree], Some(&scratch)).and_then(|_| {
        git(
            root,
            &["checkout-index", "-a", "-f", &prefix],
            Some(&scratch),
        )
    });
    std::fs::remove_file(&scratch).ok();
    result?;
    let ignored = git(
        root,
        &[
            "status",
            "--ignored",
            "--porcelain=v1",
            "--untracked-files=normal",
        ],
        None,
    )?;
    let mut linked = Vec::new();
    for line in ignored.lines() {
        let Some(path) = line.strip_prefix("!! ") else {
            continue;
        };
        let name = path.trim_end_matches('/');
        if name.contains('/') || name == ".pathfinder" {
            continue;
        }
        #[cfg(unix)]
        if std::os::unix::fs::symlink(root.join(name), dir.join(name)).is_ok() {
            linked.push(name.to_string());
        }
    }
    Ok(linked)
}

/// A file's content in a snapshot; `None` if it isn't there.
pub fn file_at(root: &Path, tree: &str, path: &str) -> Option<String> {
    git(root, &["show", &format!("{tree}:{path}")], None).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

impl Hunk {
    pub fn ranges(&self) -> String {
        format!(
            "-{},{} +{},{}",
            self.old_start, self.old_len, self.new_start, self.new_len
        )
    }

    /// The hunk as a diff, for the Tagger.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for l in &self.removed {
            out.push_str(&format!("-{l}\n"));
        }
        for l in &self.added {
            out.push_str(&format!("+{l}\n"));
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<Hunk>,
}

/// Hunks between two snapshots, without context lines.
pub fn diff(root: &Path, before: &str, after: &str) -> Result<Vec<FileDiff>> {
    let text = git(
        root,
        &[
            "diff-tree",
            "-r",
            "-p",
            "-U0",
            "--no-renames",
            "--no-color",
            "--no-ext-diff",
            before,
            after,
        ],
        None,
    )?;
    Ok(parse_diff(&text))
}

fn parse_range(s: &str) -> (usize, usize) {
    let mut it = s.splitn(2, ',');
    let start = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let len = it.next().map_or(1, |x| x.parse().unwrap_or(0));
    (start, len)
}

pub fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            let path = rest.split(" b/").next().unwrap_or(rest).to_string();
            files.push(FileDiff {
                path,
                hunks: vec![],
            });
        } else if let Some(rest) = line.strip_prefix("@@ -") {
            let Some(file) = files.last_mut() else {
                continue;
            };
            let mut parts = rest.split_whitespace();
            let old = parts.next().map(parse_range).unwrap_or((0, 0));
            let new = parts
                .next()
                .and_then(|p| p.strip_prefix('+'))
                .map(parse_range)
                .unwrap_or((0, 0));
            file.hunks.push(Hunk {
                old_start: old.0,
                old_len: old.1,
                new_start: new.0,
                new_len: new.1,
                removed: vec![],
                added: vec![],
            });
        } else if let Some(h) = files.last_mut().and_then(|f| f.hunks.last_mut()) {
            if let Some(l) = line.strip_prefix('+') {
                if !line.starts_with("+++") {
                    h.added.push(l.to_string());
                }
            } else if let Some(l) = line.strip_prefix('-')
                && !line.starts_with("---")
            {
                h.removed.push(l.to_string());
            }
        }
    }
    files.retain(|f| !f.hunks.is_empty());
    files
}

/// Blank lines and lone delimiters carry no authorship (A7).
pub fn trivial(line: &str) -> bool {
    let t = line.trim();
    t.len() < 2
        || t.chars()
            .all(|c| "{}[]()<>;,.:'\"`|\\/-_=+*#".contains(c) || c.is_whitespace())
}

/// The path-scoped hash of line `i` with its nearest non-blank neighbours.
pub fn window_hash(path: &str, lines: &[&str], i: usize) -> String {
    let prev = lines[..i]
        .iter()
        .rev()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let next = lines[i + 1..]
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut h = Sha256::new();
    for part in [path, prev, lines[i].trim(), next] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hashes for a hunk's added lines: non-trivial ones whose text wasn't
/// already in the file before (A7).
pub fn hunk_hashes(path: &str, after: &str, before: Option<&str>, hunk: &Hunk) -> Vec<String> {
    let lines: Vec<&str> = after.lines().collect();
    let existed: HashSet<&str> = before
        .map(|b| b.lines().map(str::trim).collect())
        .unwrap_or_default();
    let start = hunk.new_start.saturating_sub(1);
    (start..(start + hunk.new_len).min(lines.len()))
        .filter(|&i| !trivial(lines[i]) && !existed.contains(lines[i].trim()))
        .map(|i| window_hash(path, &lines, i))
        .collect()
}

/// One ledger entry (A4). `hash` = sha256(prev_hash + the entry's JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub session_id: String,
    pub part_id: String,
    /// `change` or `review`.
    pub kind: String,
    /// A review: the change entry it is about.
    pub refers_to: Option<String>,
    pub path: String,
    pub hunk: String,
    pub line_hashes: Vec<String>,
    pub concepts: Vec<String>,
    pub tag_confidence: f64,
    /// `reviewable`, `claimed` or `delegated`; empty on a review.
    pub label: String,
    pub rationale: Value,
    /// The events the label rests on, and the `now` it was computed at.
    pub evidence: Value,
    /// On a review: `approved`, `objected`, `checked_caught`, `checked_missed`.
    pub review: String,
    pub at: i64,
    pub prev_hash: String,
    pub hash: String,
}

impl Entry {
    pub fn compute_hash(&self) -> String {
        let mut body = self.clone();
        body.hash = String::new();
        let json = serde_json::to_string(&body).unwrap_or_default();
        let mut h = Sha256::new();
        h.update(self.prev_hash.as_bytes());
        h.update(json.as_bytes());
        hex(&h.finalize())
    }
}

/// Whether the chain is intact; the first broken entry's id otherwise.
pub fn verify(entries: &[Entry]) -> Result<(), String> {
    let mut prev = GENESIS.to_string();
    for e in entries {
        if e.prev_hash != prev || e.compute_hash() != e.hash {
            return Err(e.id.clone());
        }
        prev = e.hash.clone();
    }
    Ok(())
}

/// What a line of the current workspace counts as (A7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    Vouched,
    ReviewableUnreviewed,
    Claimed,
    Delegated,
    NotFromPathfinder,
}

impl Bucket {
    pub const ALL: [Bucket; 5] = [
        Bucket::Vouched,
        Bucket::ReviewableUnreviewed,
        Bucket::Claimed,
        Bucket::Delegated,
        Bucket::NotFromPathfinder,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vouched => "vouched",
            Self::ReviewableUnreviewed => "reviewable, unreviewed",
            Self::Claimed => "claimed, unverified",
            Self::Delegated => "delegated",
            Self::NotFromPathfinder => "not from Pathfinder",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Coverage {
    pub chain_ok: bool,
    pub chain_head: String,
    pub files: Vec<FileCoverage>,
    pub total: HashMap<Bucket, usize>,
    pub deletions_recorded: usize,
    /// `git-ai` authorship notes (`refs/notes/ai`) exist in the repository.
    pub git_ai_notes: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileCoverage {
    pub path: String,
    pub lines: HashMap<Bucket, usize>,
}

/// Coverage of the current files the ledger touched. A broken chain
/// vouches for nothing (A10).
pub fn coverage(root: &Path, entries: &[Entry]) -> Coverage {
    let chain_ok = verify(entries).is_ok();
    let mut reviews: HashMap<&str, &str> = HashMap::new();
    for e in entries.iter().filter(|e| e.kind == "review") {
        if let Some(r) = &e.refers_to {
            reviews.insert(r, &e.review);
        }
    }
    // Newest change entry wins for a hash.
    let mut by_hash: HashMap<&str, Bucket> = HashMap::new();
    let mut paths: Vec<String> = Vec::new();
    let mut deletions = 0;
    for e in entries.iter().filter(|e| e.kind == "change") {
        if !paths.contains(&e.path) {
            paths.push(e.path.clone());
        }
        if e.line_hashes.is_empty() {
            deletions += 1;
        }
        let reviewed = matches!(
            reviews.get(e.id.as_str()).copied(),
            Some("approved") | Some("checked_caught")
        );
        let bucket = match e.label.as_str() {
            "reviewable" if reviewed && chain_ok => Bucket::Vouched,
            "reviewable" => Bucket::ReviewableUnreviewed,
            "claimed" => Bucket::Claimed,
            _ => Bucket::Delegated,
        };
        for h in &e.line_hashes {
            by_hash.insert(h, bucket);
        }
    }
    let mut files = Vec::new();
    let mut total: HashMap<Bucket, usize> = HashMap::new();
    for path in paths {
        let Ok(content) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        let mut counts: HashMap<Bucket, usize> = HashMap::new();
        for i in 0..lines.len() {
            if trivial(lines[i]) {
                continue;
            }
            let h = window_hash(&path, &lines, i);
            let b = by_hash
                .get(h.as_str())
                .copied()
                .unwrap_or(Bucket::NotFromPathfinder);
            *counts.entry(b).or_default() += 1;
            *total.entry(b).or_default() += 1;
        }
        files.push(FileCoverage {
            path,
            lines: counts,
        });
    }
    Coverage {
        chain_ok,
        chain_head: entries
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| GENESIS.into()),
        files,
        total,
        deletions_recorded: deletions,
        git_ai_notes: git(
            root,
            &["rev-parse", "--verify", "--quiet", "refs/notes/ai"],
            None,
        )
        .is_ok(),
    }
}

pub fn render(c: &Coverage) -> String {
    let count = |m: &HashMap<Bucket, usize>, b: Bucket| m.get(&b).copied().unwrap_or(0);
    let all: usize = c.total.values().sum();
    if c.files.is_empty() {
        return "No agent changes are recorded in this workspace yet.".into();
    }
    let mut out = String::from("Understanding coverage of the files the agent changed:\n");
    for b in Bucket::ALL {
        let n = count(&c.total, b);
        let pct = if all == 0 {
            0.0
        } else {
            n as f64 * 100.0 / all as f64
        };
        out.push_str(&format!("- {}: {n} lines ({pct:.0}%)\n", b.label()));
    }
    out.push_str(
        "\nPer file (vouched / unreviewed / claimed / delegated / not from Pathfinder):\n",
    );
    for f in &c.files {
        out.push_str(&format!(
            "- `{}`: {} / {} / {} / {} / {}\n",
            f.path,
            count(&f.lines, Bucket::Vouched),
            count(&f.lines, Bucket::ReviewableUnreviewed),
            count(&f.lines, Bucket::Claimed),
            count(&f.lines, Bucket::Delegated),
            count(&f.lines, Bucket::NotFromPathfinder),
        ));
    }
    if c.deletions_recorded > 0 {
        out.push_str(&format!(
            "\n{} deletion-only changes are recorded in the ledger.\n",
            c.deletions_recorded
        ));
    }
    if c.chain_ok {
        out.push_str(&format!(
            "\nLedger chain verified, head `{}`.",
            &c.chain_head[..c.chain_head.len().min(16)]
        ));
    } else {
        out.push_str("\nThe ledger chain does NOT verify, so nothing counts as vouched.");
    }
    out.push_str(" Coverage is understanding, not test results: run your tests separately.");
    if c.git_ai_notes {
        out.push_str("\n\nThis repository has git-ai notes. Pairing with them isn't built yet, so lines other AI tools wrote count as not from Pathfinder.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zero_context_hunks() {
        let text = "diff --git a/src/x.py b/src/x.py\nindex 1..2 100644\n--- a/src/x.py\n+++ b/src/x.py\n@@ -3 +3,2 @@ def f():\n-    return 1\n+    y = 2\n+    return y\n@@ -10,2 +11,0 @@\n-a\n-b\n";
        let d = parse_diff(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "src/x.py");
        assert_eq!(d[0].hunks[0].new_start, 3);
        assert_eq!(d[0].hunks[0].new_len, 2);
        assert_eq!(d[0].hunks[0].added, vec!["    y = 2", "    return y"]);
        assert_eq!(d[0].hunks[1].new_len, 0);
        assert_eq!(d[0].hunks[1].removed, vec!["a", "b"]);
    }

    #[test]
    fn hashes_skip_trivial_and_preexisting_lines() {
        let after = "def f():\n    y = 2\n    }\n    return y\n";
        let hunk = Hunk {
            old_start: 1,
            old_len: 1,
            new_start: 2,
            new_len: 3,
            removed: vec![],
            added: vec![],
        };
        let hs = hunk_hashes("x.py", after, Some("def f():\n    return y\n"), &hunk);
        assert_eq!(hs.len(), 1, "only `y = 2` is new and non-trivial");
        let lines: Vec<&str> = after.lines().collect();
        assert_eq!(hs[0], window_hash("x.py", &lines, 1));
        assert_ne!(hs[0], window_hash("y.py", &lines, 1), "path-scoped");
    }

    fn entry(id: &str, prev: &str) -> Entry {
        let mut e = Entry {
            id: id.into(),
            session_id: "s".into(),
            part_id: "p".into(),
            kind: "change".into(),
            refers_to: None,
            path: "x".into(),
            hunk: "-1,0 +1,1".into(),
            line_hashes: vec![],
            concepts: vec![],
            tag_confidence: 0.9,
            label: "reviewable".into(),
            rationale: Value::Null,
            evidence: Value::Null,
            review: String::new(),
            at: 1,
            prev_hash: prev.into(),
            hash: String::new(),
        };
        e.hash = e.compute_hash();
        e
    }

    #[test]
    fn the_chain_catches_any_edit() {
        let a = entry("a", GENESIS);
        let b = entry("b", &a.hash);
        assert!(verify(&[a.clone(), b.clone()]).is_ok());
        let mut forged = b.clone();
        forged.label = "reviewable".into();
        forged.review = "approved".into();
        assert_eq!(verify(&[a, forged]), Err("b".into()));
    }
}
