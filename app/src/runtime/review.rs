//! Handoff in a staging copy and checked review (theory §13, A8).
//!
//! A handoff runs in an isolated copy of the workspace. Its diffs and report
//! are held back, and nothing reaches your files until your verdict. Now
//! and then the review you're shown has one hunk replaced by a plausible
//! flawed version. Approving it means the review didn't happen. Catching it
//! counts as evidence. Objecting to a real problem elsewhere goes back to
//! the agent as usual and is never scored as a miss. Only the agent's real
//! result is ever written.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

use super::provenance::RecordedHunk;
use super::{PartStatus, Runtime, TurnOutput, gate};
use crate::decide::Question;
use crate::domain::{Part, Session, Staging};
use crate::engine::{EngineRequest, EngineTier, StreamEvent, StreamSink, ToolAccess, run_gated};
use crate::ledger::{self, Hunk};
use crate::secrets::Secret;
use crate::understanding::{self, ConceptState, EventKind, NewEvent};
use crate::validate::Violation;

/// A planted flaw is kept when a decision model says it changes the
/// behaviour the part is about at this probability (A8).
const CHANGES_BEHAVIOUR: f64 = 0.7;

/// The chance a review is checked, by the thread's Checks setting and
/// whether the part's concepts are `shown` (any) or all `solid` (A8).
pub fn check_rate(checks: &str, any_only_shown: bool) -> f64 {
    match (checks, any_only_shown) {
        ("off", _) => 0.0,
        ("normal", true) => 0.25,
        ("normal", false) => 0.15,
        (_, true) => 0.15,
        (_, false) => 0.05,
    }
}

/// What planting a check produced, attempt by attempt (for the evals).
#[derive(Debug, Default)]
pub(super) struct PlantOutcome {
    pub attempts: Vec<Attempt>,
    /// The kept plant: added lines and the flaw.
    pub kept: Option<(Vec<String>, String)>,
}

#[derive(Debug)]
pub(super) struct Attempt {
    pub parses: bool,
    pub changes_behaviour: Option<f64>,
}

/// A uniform roll in [0, 1). `PATHFINDER_CHECK_ROLL` fixes it, for trying
/// checks out (0 checks every review that can be checked).
pub fn roll() -> f64 {
    if let Some(v) = crate::envvar::get("PATHFINDER_CHECK_ROLL")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    {
        return v;
    }
    let b = uuid::Uuid::new_v4();
    let n = u32::from_le_bytes(b.as_bytes()[..4].try_into().unwrap());
    n as f64 / (u32::MAX as f64 + 1.0)
}

#[derive(Debug, Deserialize)]
struct Planted {
    #[serde(default)]
    added: String,
    #[serde(default)]
    flaw: String,
}

#[derive(Debug, Deserialize)]
struct Caught {
    #[serde(default)]
    caught: bool,
}

pub(super) const PLANT_SYSTEM: &str = r#"You make a review check for Pathfinder. The user is about to review an agent's change. You get one hunk of it (the lines it adds) and the concepts the change relies on. Rewrite the hunk's added lines so they contain exactly one plausible conceptual flaw: a mistake someone who understands those concepts would catch when reading the diff, and that changes the behaviour the part is about.

- Keep it the same size and style, so it reads like the agent wrote it. It must still parse and look intentional.
- The flaw is about the concepts, not a typo, a syntax error, a renamed variable or a removed line.
- No comments that hint at it.

`added`: the replacement added lines, exactly as they'd appear in the file (keep indentation), separated by newlines. `flaw`: what is wrong and why it matters, in one or two sentences. In `flaw`, write math as plain text or Unicode (x², √, ≤, μᵢ), never LaTeX.

Return ONLY a JSON object:
{"added": "...", "flaw": "..."}"#;

const CAUGHT_SYSTEM: &str = r#"A review contained one deliberately planted flaw. Decide whether the user's objection names that flaw: the same mistake in the same place, in their own words. An objection about something else in the change, or a vague "something's off", doesn't count.

Return ONLY a JSON object:
{"caught": true}"#;

/// Shows which tools the agent calls (their titles and whether they
/// worked) but holds back everything else it streams: its prose, its
/// thinking, diffs and tool output, any of which could show the real
/// result, and so the planted hunk, before the review (A6).
fn staged(sink: &StreamSink) -> StreamSink {
    let inner = sink.clone();
    Arc::new(move |ev| match ev {
        StreamEvent::TextDelta(_) | StreamEvent::Thought(_) | StreamEvent::ToolUpdate { .. } => {}
        other => inner(other),
    })
}

/// A hunk as a fenced diff, with `added` in place of the hunk's own.
fn hunk_diff(h: &Hunk, added: &[String]) -> String {
    let mut out = format!(
        "@@ -{},{} +{},{} @@\n",
        h.old_start,
        h.old_len,
        h.new_start,
        added.len()
    );
    for l in &h.removed {
        out.push_str(&format!("-{l}\n"));
    }
    for l in added {
        out.push_str(&format!("+{l}\n"));
    }
    out
}

/// The review you're shown: every hunk, one of them planted if checked.
fn review_text(hunks: &[RecordedHunk], planted: Option<(usize, &[String])>) -> String {
    let mut out = String::new();
    let mut current: Option<&str> = None;
    for (i, r) in hunks.iter().enumerate() {
        if current != Some(r.path.as_str()) {
            if current.is_some() {
                out.push_str("```\n");
            }
            out.push_str(&format!("\n`{}`\n```diff\n", r.path));
            current = Some(&r.path);
        }
        let added: &[String] = match planted {
            Some((p, a)) if p == i => a,
            _ => &r.hunk.added,
        };
        out.push_str(&hunk_diff(&r.hunk, added));
    }
    if current.is_some() {
        out.push_str("```\n");
    }
    out
}

/// Checks the planted file still parses, where a checker is at hand. The
/// file to check is written under `private` (the staging root, hidden from
/// sandboxed commands and this user's only), never the shared temp folder:
/// the planted flaw is what the agent must not see.
fn still_parses(private: &Path, path: &str, content: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if ext == "json" {
        return serde_json::from_str::<serde_json::Value>(content).is_ok();
    }
    // Checkers to try in turn; the first one installed decides. Windows has
    // the `py` launcher, and its `python3` is often only a store alias.
    let checkers: &[&[&str]] = match ext {
        "py" if cfg!(windows) => &[
            &["py", "-3", "-m", "py_compile"],
            &["python3", "-m", "py_compile"],
            &["python", "-m", "py_compile"],
        ],
        "py" => &[&["python3", "-m", "py_compile"]],
        "js" | "mjs" | "cjs" => &[&["node", "--check"]],
        _ => return true,
    };
    let dir = private.join(format!("check-{}", uuid::Uuid::new_v4().simple()));
    if crate::paths::ensure_private_dir(private).is_err()
        || crate::paths::ensure_private_dir(&dir).is_err()
    {
        return true;
    }
    let file = dir.join(format!("check.{ext}"));
    let ok = std::fs::write(&file, content).is_ok()
        && checkers
            .iter()
            .find_map(|checker| {
                let out = std::process::Command::new(checker[0])
                    .args(&checker[1..])
                    .arg(&file)
                    .output()
                    .ok()?;
                (!no_interpreter(&out)).then(|| out.status.success())
            })
            // No checker installed: nothing to check with.
            .unwrap_or(true);
    std::fs::remove_dir_all(&dir).ok();
    ok
}

/// A checker that started but has no interpreter behind it: the Windows
/// store alias for `python3` prints "Python was not found" and exits 9009,
/// as cmd does for a missing command.
fn no_interpreter(out: &std::process::Output) -> bool {
    cfg!(windows)
        && (out.status.code() == Some(9009)
            || String::from_utf8_lossy(&out.stderr).contains("Python was not found"))
}

/// `content` with lines `start..start+len` (1-based) replaced by `added`.
fn splice(content: &str, start: usize, len: usize, added: &[String]) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let from = start.saturating_sub(1).min(lines.len());
    let to = (from + len).min(lines.len());
    let mut out: Vec<String> = lines[..from].iter().map(|s| s.to_string()).collect();
    out.extend(added.iter().cloned());
    out.extend(lines[to..].iter().map(|s| s.to_string()));
    let mut text = out.join("\n");
    if content.ends_with('\n') {
        text.push('\n');
    }
    text
}

impl Runtime {
    fn staging_dir(&self, session: &Session, part_id: &str) -> PathBuf {
        let run = uuid::Uuid::new_v4().simple().to_string();
        crate::paths::staging_root(&(self.config)().db_path)
            .join(&session.id)
            .join(format!("{part_id}-{}", &run[..8]))
    }

    /// `/handoff` in a git repository: run the agent in a staging copy,
    /// record its hunks, and show the review (maybe checked).
    pub(super) async fn handoff_staged(
        &self,
        session: &mut Session,
        part: Part,
        root: &Path,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let base = ledger::snapshot(root)?;
        // A real folder, this user's only: a link planted where the staging
        // root goes would put the copies (and their planted flaws) in sight.
        let staging_root = crate::paths::staging_root(&(self.config)().db_path);
        crate::paths::ensure_private_dir(&staging_root)
            .with_context(|| format!("making {}", staging_root.display()))?;
        let dir = self.staging_dir(session, &part.id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        let linked = ledger::materialize(root, &base, &dir)?;
        let staging = Staging {
            dir,
            base,
            result: None,
            secret: None,
            report: String::new(),
            linked,
        };
        self.run_staged(session, part, root, staging, instructions, files, sink)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_staged(
        &self,
        session: &mut Session,
        part: Part,
        root: &Path,
        mut staging: Staging,
        instructions: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        sink(StreamEvent::Thought(
            "The agent works in a staging copy; its diffs and report come with the review, and your files change only after your verdict.\n".into(),
        ));
        let report = self
            .run_agent(
                session,
                &part,
                true,
                instructions,
                files,
                staged(&sink),
                Some(&staging.dir),
            )
            .await?;
        let after = ledger::snapshot_worktree(root, &staging.dir, &staging.linked)?;
        let recorded = self
            .record_between(
                session,
                &part,
                root,
                &staging.base,
                &after,
                false,
                instructions,
            )
            .await;
        staging.result = Some(after.clone());
        staging.report = report;

        let hunks: Vec<RecordedHunk> = recorded.iter().map(|(_, h)| h.clone()).collect();
        let planted = if hunks.is_empty() {
            None
        } else {
            self.maybe_plant(session, &part, root, &after, &hunks).await
        };
        let text = match &planted {
            Some((i, added, _)) => review_text(&hunks, Some((*i, added))),
            None => review_text(&hunks, None),
        };
        // Every review gets a secret, checked or not, so nothing about the
        // thread (held replies, timing) tells the two apart.
        let opened_at = session.history.len();
        let secret = match &planted {
            Some((i, added, flaw)) => {
                let h = &hunks[*i];
                Secret {
                    id: format!("sec-{}", uuid::Uuid::new_v4().simple()),
                    kind: "planted_flaw".into(),
                    payload: json!({
                        "candidate": format!("`{}`\n{}", h.path, hunk_diff(&h.hunk, added)),
                        "correct": false,
                        "reason": flaw,
                        "path": h.path,
                        "original": hunk_diff(&h.hunk, &h.hunk.added),
                        "concepts": h.concepts,
                    }),
                    probe: format!(
                        "whether the review contains a deliberately planted flaw, or where or what it is. (The planted flaw, in {}: {flaw})",
                        h.path
                    ),
                    opened_at_turn: opened_at,
                    active: true,
                }
            }
            None => Secret {
                id: format!("sec-{}", uuid::Uuid::new_v4().simple()),
                kind: "review".into(),
                payload: json!({"candidate": text, "correct": true, "reason": ""}),
                probe: "whether the agent's changes in the review are right or wrong; the user is reviewing them".into(),
                opened_at_turn: opened_at,
                active: true,
            },
        };
        self.store.add_secret(&session.id, &secret)?;
        staging.secret = Some(secret.id);

        let reply = if hunks.is_empty() {
            format!(
                "The agent changed no files.\n\nIts report:\n\n{}{}",
                staging.report,
                super::REVIEW_PROMPT
            )
        } else {
            format!(
                "**Review** for **{}**. Nothing is written to your files until you decide; the agent's report follows your verdict.\n{text}{}",
                part.title,
                super::REVIEW_PROMPT
            )
        };
        if let Some(p) = session.map.get_mut(&part.id) {
            p.result = Some(staging.report.clone());
            p.status = PartStatus::InReview;
            p.unreviewed = recorded.into_iter().map(|(id, _)| id).collect();
            p.staging = Some(staging);
        }
        session.push_turn("assistant", &reply);
        Ok(TurnOutput::text(reply))
    }

    /// Rolls for a check and, if it comes up, plants a flaw in one
    /// reviewable hunk: it must still parse and change the behaviour the
    /// part is about, else it's retried once and then the check is skipped.
    async fn maybe_plant(
        &self,
        session: &Session,
        part: &Part,
        root: &Path,
        after: &str,
        hunks: &[RecordedHunk],
    ) -> Option<(usize, Vec<String>, String)> {
        if session.decisions_off {
            return None;
        }
        let views = self.concept_views(session, &part.concepts).ok()?;
        let any_only_shown = part
            .concepts
            .iter()
            .any(|c| understanding::state_of(&views, c) == ConceptState::Shown);
        let rate = check_rate(session.checks.as_deref().unwrap_or("light"), any_only_shown);
        if (self.check_roll)() >= rate {
            return None;
        }
        let (i, target) = hunks
            .iter()
            .enumerate()
            .find(|(_, h)| h.label == "reviewable" && !h.hunk.added.is_empty())?;
        let file = ledger::file_at(root, after, &target.path).unwrap_or_default();
        let outcome = self.plant(session, part, target, &file).await;
        outcome.kept.map(|(added, flaw)| (i, added, flaw))
    }

    /// Plants a flaw in `target`, retrying once; records each attempt.
    pub(super) async fn plant(
        &self,
        session: &Session,
        part: &Part,
        target: &RecordedHunk,
        file: &str,
    ) -> PlantOutcome {
        let mut outcome = PlantOutcome::default();
        let file = file.to_string();
        let labels: Vec<String> = target.concepts.clone();
        for _ in 0..2 {
            let req = EngineRequest {
                tier: EngineTier::System2,
                tools: ToolAccess::None,
                cwd: session.cwd.clone(),
                system_prompt: PLANT_SYSTEM.to_string(),
                prompt: format!(
                    "Part: {}\nConcepts the change relies on: {}\nFile: {}\n\nThe file after the change:\n{}\n\nThe hunk's added lines (starting at line {}):\n{}\n\nRespond with JSON only.",
                    part.title,
                    labels.join(", "),
                    target.path,
                    file.chars().take(8000).collect::<String>(),
                    target.hunk.new_start,
                    target.hunk.added.join("\n"),
                ),
                files: vec![],
                stream_text: false,
                models: match self.models(session) {
                    Ok(m) => m,
                    Err(_) => return outcome,
                },
                ctx: crate::engine::turn_ctx(),
            };
            let original = target.hunk.added.clone();
            let planted: Planted = match run_gated(
                self.engine.as_ref(),
                self.store.as_ref(),
                &session.id,
                "plant",
                &req,
                gate::quiet(),
                |p: &Planted| {
                    let mut v = Vec::new();
                    if p.flaw.trim().is_empty() {
                        v.push(Violation::new("empty_field", "`flaw` must not be empty"));
                    }
                    if p.added.lines().map(str::to_string).collect::<Vec<_>>() == original {
                        v.push(Violation::new(
                            "unchanged",
                            "`added` must differ from the hunk",
                        ));
                    }
                    v
                },
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("planting a review check: {e:#}");
                    return outcome;
                }
            };
            let added: Vec<String> = planted.added.lines().map(str::to_string).collect();
            let content = splice(&file, target.hunk.new_start, target.hunk.new_len, &added);
            let parses = still_parses(
                &crate::paths::staging_root(&(self.config)().db_path),
                &target.path,
                &content,
            );
            outcome.attempts.push(Attempt {
                parses,
                changes_behaviour: None,
            });
            if !parses {
                continue;
            }
            let q = [Question {
                name: "changes".into(),
                instructions: format!(
                    "Does the modified version change the behaviour that \"{}\" is about, in a way someone who understands it should catch?",
                    part.title
                ),
                options: vec![
                    ("yes".into(), "It changes that behaviour".into()),
                    ("no".into(), "It doesn't, or only cosmetically".into()),
                ],
            }];
            let input = format!(
                "Original:\n{}\n\nModified:\n{}",
                target.hunk.added.join("\n"),
                added.join("\n")
            );
            match self
                .decider
                .decide(&input, &q, session.decision_model.as_deref())
                .await
            {
                Ok(Some(d)) => {
                    let p = d.answers.get("changes").map(|a| {
                        if a.choice == "yes" {
                            a.probability
                        } else {
                            1.0 - a.probability
                        }
                    });
                    if let Some(last) = outcome.attempts.last_mut() {
                        last.changes_behaviour = p;
                    }
                    if p.is_some_and(|p| p >= CHANGES_BEHAVIOUR) {
                        outcome.kept = Some((added, planted.flaw));
                        return outcome;
                    }
                }
                // Without a decision model there's no gate either: no check.
                _ => return outcome,
            }
        }
        outcome
    }

    /// `/approve` on a staged review.
    pub(super) async fn approve_staged(
        &self,
        session: &mut Session,
        part: Part,
    ) -> Result<TurnOutput> {
        let staging = part.staging.clone().unwrap();
        let secret = match &staging.secret {
            Some(id) => self.store.secret(id)?,
            None => None,
        };
        let mut lead = String::new();
        match secret.as_ref().filter(|s| s.kind == "planted_flaw") {
            Some(planted) => {
                // The review didn't happen: missed on the planted concepts.
                let events = concepts_of(planted)
                    .iter()
                    .map(|c| {
                        NewEvent::new(c, EventKind::ReviewMissed).reference(planted.id.clone())
                    })
                    .collect();
                self.append_events(session, events);
                self.record_review(session, &part.id, &part.unreviewed, "checked_missed", None);
                if let Some(p) = session.map.get_mut(&part.id) {
                    p.review_missed = true;
                }
                lead.push_str(&format!(
                    "This review was checked: one hunk had a planted flaw.\n\n{}\nThe flaw: {}\n\nYou approved it, so this review isn't recorded as one, and what it relied on counts as shaky again. Your files get the agent's real result.\n\n",
                    planted.candidate(),
                    planted.reason()
                ));
            }
            None => {
                self.record_review(session, &part.id, &part.unreviewed, "approved", None);
                if let Some(p) = session.map.get_mut(&part.id) {
                    p.review_missed = false;
                }
            }
        }
        let applied = self.apply_staged(session, &staging).await;
        lead.push_str(&applied);
        if !staging.report.trim().is_empty() {
            lead.push_str(&format!(
                "\n\nThe agent's report:\n\n{}",
                staging.report.trim()
            ));
        }
        if let Some(p) = session.map.get_mut(&part.id) {
            p.unreviewed.clear();
            p.staging = None;
        }
        std::fs::remove_dir_all(&staging.dir).ok();
        self.set_status(session, &part.id, PartStatus::Done, "result approved")?;
        Ok(self.after_agent_run(
            session,
            format!("{lead}\n\n**{}** done.", part.title),
            &part.concepts,
        ))
    }

    /// `/object` on a staged review: catching the planted flaw ends the
    /// review; any other objection goes back to the agent in staging.
    pub(super) async fn object_staged(
        &self,
        session: &mut Session,
        part: Part,
        reason: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        let staging = part.staging.clone().unwrap();
        let secret = match &staging.secret {
            Some(id) => self.store.secret(id)?,
            None => None,
        };
        let mut reveal = String::new();
        if let Some(planted) = secret.as_ref().filter(|s| s.kind == "planted_flaw") {
            if self.caught(session, planted, reason).await? {
                let events = concepts_of(planted)
                    .iter()
                    .map(|c| NewEvent::new(c, EventKind::ReviewOk).reference(planted.id.clone()))
                    .collect();
                self.append_events(session, events);
                self.record_review(
                    session,
                    &part.id,
                    &part.unreviewed,
                    "checked_caught",
                    Some(reason),
                );
                // What you did comes first: a long report and the next
                // part's card would otherwise bury it.
                let mut lead = format!(
                    "You caught it: that hunk was a planted check, never part of the agent's work. ({})\n\nYour files get the agent's real result.\n\n",
                    planted.reason()
                );
                lead.push_str(&self.apply_staged(session, &staging).await);
                if !staging.report.trim().is_empty() {
                    lead.push_str(&format!(
                        "\n\nThe agent's report:\n\n{}",
                        staging.report.trim()
                    ));
                }
                if let Some(p) = session.map.get_mut(&part.id) {
                    p.unreviewed.clear();
                    p.staging = None;
                    p.review_caught = true;
                    p.review_missed = false;
                }
                std::fs::remove_dir_all(&staging.dir).ok();
                self.set_status(
                    session,
                    &part.id,
                    PartStatus::Done,
                    "planted flaw caught, objected",
                )?;
                return Ok(self.after_agent_run(
                    session,
                    format!("{lead}\n\n**{}** done.", part.title),
                    &part.concepts,
                ));
            }
            // A real objection on a checked review: never a miss. The plant
            // is retired now, so it's revealed: you shouldn't go on
            // believing the agent wrote it.
            sink(StreamEvent::Thought(
                "That objection is about the agent's own work; sending it back.\n".into(),
            ));
            reveal = format!(
                "One hunk you saw was a planted check, not the agent's work.\n\n{}\nThe flaw: {}\n\nYour objection goes back to the agent.\n\n",
                planted.candidate(),
                planted.reason()
            );
        }
        if let Some(id) = &staging.secret {
            self.store.retire_secret(id).ok();
        }
        self.record_review(
            session,
            &part.id,
            &part.unreviewed,
            "objected",
            Some(reason),
        );
        let Some(root) = ledger::repo_root(&session.cwd) else {
            anyhow::bail!("the workspace is no longer a git repository");
        };
        let mut part = part;
        part.objections.push(reason.to_string());
        part.result = Some(staging.report.clone());
        if let Some(p) = session.map.get_mut(&part.id) {
            p.objections = part.objections.clone();
            p.unreviewed.clear();
        }
        let mut next = staging;
        next.secret = None;
        next.result = None;
        let turns = session.history.len();
        let mut out = self
            .run_staged(session, part, &root, next, "", files, sink)
            .await?;
        if !reveal.is_empty() {
            out.reply = format!("{reveal}{}", out.reply);
            if let Some(reply) = session
                .history
                .iter_mut()
                .skip(turns)
                .find(|t| t.role == "assistant")
            {
                reply.content = format!("{reveal}{}", reply.content);
            }
        }
        Ok(out)
    }

    pub(super) async fn caught(
        &self,
        session: &Session,
        planted: &Secret,
        objection: &str,
    ) -> Result<bool> {
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::None,
            cwd: session.cwd.clone(),
            system_prompt: CAUGHT_SYSTEM.to_string(),
            prompt: format!(
                "Planted hunk:\n{}\n\nWhat the agent really wrote there:\n{}\n\nThe planted flaw: {}\n\nThe user's objection: {objection}\n\nRespond with JSON only.",
                planted.candidate(),
                planted.payload["original"].as_str().unwrap_or_default(),
                planted.reason()
            ),
            files: vec![],
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let c: Caught = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "caught",
            &req,
            gate::quiet(),
            |_: &Caught| vec![],
        )
        .await?;
        Ok(c.caught)
    }

    /// Writes the agent's real result to your files. A file you changed
    /// since the handoff began is left alone and named.
    async fn apply_staged(&self, session: &Session, staging: &Staging) -> String {
        let Some(root) = ledger::repo_root(&session.cwd) else {
            return "The workspace is no longer a git repository; nothing was written.".into();
        };
        let Some(result) = &staging.result else {
            return String::new();
        };
        let files = match ledger::diff(&root, &staging.base, result) {
            Ok(f) => f,
            Err(e) => {
                return format!("Couldn't read the staged result ({e:#}); nothing was written.");
            }
        };
        let (mut written, mut conflicts) = (Vec::new(), Vec::new());
        for f in files {
            let path = root.join(&f.path);
            let base = ledger::file_at(&root, &staging.base, &f.path);
            let now = std::fs::read_to_string(&path).ok();
            if now != base {
                conflicts.push(f.path.clone());
                continue;
            }
            let ok = match ledger::file_at(&root, result, &f.path) {
                Some(content) => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    std::fs::write(&path, &content).is_ok()
                }
                None => std::fs::remove_file(&path).is_ok(),
            };
            if ok {
                written.push(f.path);
            } else {
                conflicts.push(f.path);
            }
        }
        let mut out = String::new();
        if !written.is_empty() {
            out.push_str(&format!(
                "Written: {}.",
                written
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !conflicts.is_empty() {
            out.push_str(&format!(
                " Not written, because they changed since the handoff began: {}. The agent's version is in {}.",
                conflicts.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", "),
                staging.dir.display()
            ));
        }
        out
    }
}

fn concepts_of(secret: &Secret) -> Vec<String> {
    secret.payload["concepts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_staged_sink_drops_thoughts_and_output() {
        use crate::stream::{ToolContent, ToolKind, ToolStart};
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        let sink: StreamSink = Arc::new(move |ev| record.lock().unwrap().push(ev));
        let staged = staged(&sink);
        staged(StreamEvent::TextDelta("the real result".into()));
        staged(StreamEvent::Thought("I'll return HANDLERS[name]".into()));
        staged(StreamEvent::ToolStart(ToolStart {
            id: "t1".into(),
            title: "Edit app.py".into(),
            kind: ToolKind::Edit,
            path: None,
        }));
        staged(StreamEvent::ToolUpdate {
            id: "t1".into(),
            content: ToolContent::Text("return HANDLERS[name]".into()),
        });
        staged(StreamEvent::ToolUpdate {
            id: "t1".into(),
            content: ToolContent::Diff {
                path: "app.py".into(),
                old: None,
                new: "return HANDLERS[name]".into(),
            },
        });
        staged(StreamEvent::ToolEnd {
            id: "t1".into(),
            ok: true,
        });
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(matches!(&seen[0], StreamEvent::ToolStart(t) if t.title == "Edit app.py"));
        assert!(matches!(&seen[1], StreamEvent::ToolEnd { ok: true, .. }));
    }

    #[test]
    fn rates_and_splicing() {
        assert_eq!(check_rate("off", true), 0.0);
        assert_eq!(check_rate("light", true), 0.15);
        assert_eq!(check_rate("light", false), 0.05);
        assert_eq!(check_rate("normal", true), 0.25);
        assert_eq!(check_rate("normal", false), 0.15);
        let r = roll();
        assert!((0.0..1.0).contains(&r));
        assert_eq!(
            splice("a\nb\nc\n", 2, 1, &["x".into(), "y".into()]),
            "a\nx\ny\nc\n"
        );
        let tmp = std::env::temp_dir();
        assert!(still_parses(&tmp, "x.json", "{}"));
        assert!(!still_parses(&tmp, "x.json", "{"));
    }

    use std::collections::{HashMap, VecDeque};
    use std::process::Command;
    use std::sync::Mutex;

    use crate::decide::{Answer, Decide, Decision};
    use crate::domain::{Map, SessionMode};
    use crate::engine::{BoxFuture, Engine, noop_sink};
    use crate::runtime::TurnInput;
    use crate::store::Store;

    enum Step {
        /// The agent writes `content` to `file` in its working directory.
        Agent(&'static str, &'static str),
        Json(&'static str),
    }

    struct Script(Mutex<VecDeque<Step>>);

    impl Engine for Script {
        fn run_text<'a>(
            &'a self,
            req: &'a EngineRequest,
            _sink: StreamSink,
        ) -> BoxFuture<'a, Result<String>> {
            let step = self.0.lock().unwrap().pop_front().unwrap_or_else(|| {
                panic!(
                    "unexpected engine call: {}",
                    req.prompt.chars().take(300).collect::<String>()
                )
            });
            Box::pin(async move {
                match step {
                    Step::Agent(file, content) => {
                        assert_eq!(req.tools, ToolAccess::Full);
                        std::fs::write(req.cwd.join(file), content)?;
                        Ok("Added the route function.".into())
                    }
                    Step::Json(j) => Ok(j.to_string()),
                }
            })
        }
    }

    /// Tags with every concept and says every plant changes behaviour.
    struct Yes;

    impl Decide for Yes {
        fn decide<'a>(
            &'a self,
            _input: &'a str,
            questions: &'a [Question],
            _prefer: Option<&'a str>,
        ) -> BoxFuture<'a, Result<Option<Decision>>> {
            let answers: HashMap<String, Answer> = questions
                .iter()
                .map(|q| {
                    let choice = if q.name == "else" { "no" } else { "yes" };
                    (
                        q.name.clone(),
                        Answer {
                            choice: choice.into(),
                            probability: 0.9,
                            ..Default::default()
                        },
                    )
                })
                .collect();
            Box::pin(async move {
                Ok(Some(Decision {
                    by: "Fake".into(),
                    answers,
                }))
            })
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }

    const ORIGINAL: &str = "def main():\n    print('hi')\n";
    const REAL: &str =
        "def main():\n    print('hi')\n\ndef route(name):\n    return HANDLERS[name]\n";

    fn setup(steps: Vec<Step>) -> (Runtime, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("harness-rev-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("app.py"), ORIGINAL).unwrap();
        std::fs::write(dir.join(".gitignore"), "venv/\n").unwrap();
        std::fs::create_dir_all(dir.join("venv")).unwrap();
        std::fs::write(dir.join("venv").join("lib.py"), "x = 1\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "init"]);
        let mut rt = Runtime::new(
            Arc::new(Store::open_in_memory().unwrap()),
            Arc::new(Script(Mutex::new(steps.into()))),
        )
        .with_decider(Arc::new(Yes));
        fn cfg() -> crate::config::Config {
            let entry = |id: &str| crate::config::ModelEntry {
                id: id.into(),
                label: id.into(),
                route: crate::config::Route::Anthropic,
                model: id.into(),
                effort: None,
                endpoint: None,
            };
            crate::config::Config {
                models: vec![entry("fast"), entry("main")],
                discover: false,
                db_path: std::env::temp_dir()
                    .join("harness-test")
                    .join("harness.sqlite"),
                ..crate::config::Config::default()
            }
        }
        rt.config = cfg;
        rt.check_roll = || 0.0;
        let mut session = rt.create_session("s".into(), dir.clone()).unwrap();
        let mut part = Part::new("route".into(), "Routing".into(), String::new(), vec![]);
        part.concepts = vec!["dispatch".into()];
        part.status = PartStatus::Understood;
        session.map = Map { parts: vec![part] };
        session.active = Some("route".into());
        session.mode = SessionMode::Working;
        rt.store.save_session(&session).unwrap();
        rt.append_events(
            &session,
            vec![NewEvent::new("dispatch", EventKind::Pass).detail(json!({"flawed": false}))],
        );
        (rt, dir)
    }

    async fn turn(rt: &Runtime, text: &str) -> String {
        rt.handle_turn(
            "s",
            TurnInput {
                text: text.into(),
                files: vec![],
            },
            noop_sink(),
        )
        .await
        .unwrap()
        .reply
    }

    fn state(rt: &Runtime) -> ConceptState {
        let s = rt.store.load_session("s").unwrap().unwrap();
        let v = rt.concept_views(&s, &["dispatch".to_string()]).unwrap();
        understanding::state_of(&v, "dispatch")
    }

    const PLANT: &str = r#"{"added":"def route(name):\n    return HANDLERS.get(name, HANDLERS['help'])","flaw":"Unknown names silently fall back to help instead of being rejected."}"#;

    #[tokio::test]
    async fn approving_a_planted_flaw_is_a_missed_review() {
        let (rt, dir) = setup(vec![Step::Agent("app.py", REAL), Step::Json(PLANT)]);
        let review = turn(&rt, "/handoff").await;
        // Nothing reaches your files before the verdict, and the review shows
        // the planted hunk in place of the real one.
        assert_eq!(
            std::fs::read_to_string(dir.join("app.py")).unwrap(),
            ORIGINAL
        );
        assert!(review.contains("HANDLERS.get(name"), "{review}");
        assert!(!review.contains("return HANDLERS[name]"));
        assert!(
            !review.contains("Added the route function"),
            "the report waits"
        );
        let s = rt.store.load_session("s").unwrap().unwrap();
        let staging = s.map.get("route").unwrap().staging.clone().unwrap();
        assert!(
            staging.dir.join("venv").exists(),
            "ignored folders are linked in"
        );
        let secret = rt
            .store
            .secret(staging.secret.as_ref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(secret.kind, "planted_flaw");
        assert!(secret.active);

        let out = turn(&rt, "/approve").await;
        assert!(out.contains("planted flaw"), "{out}");
        assert_eq!(std::fs::read_to_string(dir.join("app.py")).unwrap(), REAL);
        assert_eq!(state(&rt), ConceptState::Shaky);
        let root = ledger::repo_root(&dir).unwrap();
        let entries = rt.store.ledger(&root.to_string_lossy()).unwrap();
        assert!(entries.iter().any(|e| e.review == "checked_missed"));
        let c = ledger::coverage(&root, &entries);
        assert_eq!(c.total.get(&ledger::Bucket::Vouched), None);
        assert!(!staging.dir.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_real_objection_goes_back_and_catching_the_flaw_counts() {
        let (rt, dir) = setup(vec![
            Step::Agent("app.py", REAL),
            Step::Json(PLANT),
            // An objection about something else: not caught, back to the agent.
            Step::Json(r#"{"caught":false}"#),
            Step::Agent("app.py", REAL),
            Step::Json(PLANT),
            Step::Json(r#"{"caught":true}"#),
        ]);
        turn(&rt, "/handoff").await;
        let again = turn(&rt, "/object add a docstring").await;
        assert!(again.contains("Review"), "{again}");
        // The missed plant is revealed: you shouldn't think the agent wrote it.
        assert!(
            again.starts_with("One hunk you saw was a planted check, not the agent's work."),
            "{again}"
        );
        assert!(again.contains("HANDLERS.get(name"), "{again}");
        assert!(again.contains("Your objection goes back to the agent."));
        let s = rt.store.load_session("s").unwrap().unwrap();
        assert!(
            s.history.iter().any(|t| t
                .content
                .starts_with("One hunk you saw was a planted check")),
            "the reveal is kept in the thread"
        );
        assert_eq!(state(&rt), ConceptState::Shown, "never scored as a miss");
        assert_eq!(
            std::fs::read_to_string(dir.join("app.py")).unwrap(),
            ORIGINAL
        );

        let out = turn(
            &rt,
            "/object unknown names fall back to help instead of failing",
        )
        .await;
        assert!(out.contains("You caught it"), "{out}");
        // What you did comes first, before the agent's report and any next
        // part's card.
        assert!(
            out.starts_with("You caught it: that hunk was a planted check"),
            "{out}"
        );
        assert!(
            out.find("You caught it").unwrap()
                < out.find("The agent's report").unwrap_or(usize::MAX),
            "{out}"
        );
        let s = rt.store.load_session("s").unwrap().unwrap();
        let last = s
            .history
            .iter()
            .rev()
            .find(|t| t.role == "assistant")
            .unwrap();
        assert!(
            last.content.starts_with("You caught it"),
            "the thread keeps the order"
        );
        // The record says you caught it and objected, never that you
        // approved it.
        let why = turn(&rt, "/why").await;
        assert!(
            why.contains("you caught the planted check and objected"),
            "{why}"
        );
        assert!(!why.contains("you approved it"), "{why}");
        assert_eq!(std::fs::read_to_string(dir.join("app.py")).unwrap(), REAL);
        let root = ledger::repo_root(&dir).unwrap();
        let entries = rt.store.ledger(&root.to_string_lossy()).unwrap();
        let c = ledger::coverage(&root, &entries);
        assert_eq!(c.total.get(&ledger::Bucket::Vouched), Some(&2));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Adversarial: one Enter queued two parts for the agent; catching the
    /// planted check on the first finishes it (Done), so the queue goes on
    /// at once, never waiting for an unrelated later `/approve`.
    #[tokio::test]
    async fn adv_catching_a_plant_does_not_strand_the_agent_queue() {
        let (rt, dir) = setup(vec![
            Step::Agent("app.py", REAL),
            Step::Json(PLANT),
            Step::Json(r#"{"caught":true}"#),
            Step::Agent("docs.md", "# Routing\n"),
            Step::Json(r#"{"added":"","flaw":""}"#),
            Step::Json(
                r#"{"added":"Unknown names go to help.","flaw":"Unknown names are documented as falling back to help."}"#,
            ),
        ]);
        let mut s = rt.store.load_session("s").unwrap().unwrap();
        let mut docs = Part::new("docs".into(), "Docs".into(), String::new(), vec![]);
        docs.concepts = vec!["dispatch".into()];
        docs.status = PartStatus::Understood;
        s.map.parts.push(docs);
        s.pending_commitment = Some(crate::domain::PendingCommitment {
            kind: crate::domain::CommitKind::Handoff,
            part: "route".into(),
            text: "do both".into(),
            files: vec![],
            candidate: None,
            then: vec![crate::domain::QueuedAgent {
                part: "docs".into(),
                kind: crate::domain::CommitKind::Handoff,
            }],
        });
        rt.store.save_session(&s).unwrap();
        turn(&rt, "/confirm").await;
        let out = turn(
            &rt,
            "/object unknown names fall back to help instead of failing",
        )
        .await;
        assert!(
            out.contains("You caught it")
                || rt
                    .store
                    .load_session("s")
                    .unwrap()
                    .unwrap()
                    .history
                    .iter()
                    .any(|t| t.content.contains("You caught it")),
            "{out}"
        );
        let s = rt.store.load_session("s").unwrap().unwrap();
        assert_eq!(s.map.get("route").unwrap().status, PartStatus::Done);
        assert!(
            s.agent_queue.is_empty(),
            "queue stranded after a caught plant"
        );
        assert_ne!(
            s.map.get("docs").unwrap().status,
            PartStatus::Understood,
            "the queued part never ran"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A part you hand over yourself replaces what an older request left
    /// queued: a later `/approve` never starts it.
    #[tokio::test]
    async fn a_new_handoff_drops_an_older_queue() {
        let (rt, dir) = setup(vec![Step::Agent("app.py", REAL), Step::Json(PLANT)]);
        let mut s = rt.store.load_session("s").unwrap().unwrap();
        s.agent_queue = vec![crate::domain::QueuedAgent {
            part: "docs".into(),
            kind: crate::domain::CommitKind::Handoff,
        }];
        s.agent_text = "old request".into();
        rt.store.save_session(&s).unwrap();
        turn(&rt, "/handoff").await;
        let s = rt.store.load_session("s").unwrap().unwrap();
        assert!(s.agent_queue.is_empty());
        assert!(s.agent_text.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
