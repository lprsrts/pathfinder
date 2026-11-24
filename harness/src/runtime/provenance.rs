//! The runtime's side of provenance (theory §12, A7): snapshot around agent
//! work, tag each hunk with the concepts it relies on, label it by your
//! understanding at that moment, chain it into the ledger, and record your
//! review of it.

use std::path::PathBuf;

use serde_json::json;

use super::Runtime;
use crate::decide::Question;
use crate::domain::{Part, Session};
use crate::ledger::{self, Entry, FileDiff, Hunk};
use crate::understanding::{self, ConceptState};

/// A hunk is tagged with a concept at this probability (A7).
const TAG: f64 = 0.6;
/// "Something else" at this probability leaves the hunk untagged.
const SOMETHING_ELSE: f64 = 0.5;
/// Hunks past this many in one run are left untagged (counted delegated).
const MAX_TAGGED_HUNKS: usize = 40;
/// The Tagger sees at most this much of a hunk.
const HUNK_CHARS: usize = 4000;

/// The workspace as it was before the agent ran.
pub(super) struct Before {
    root: PathBuf,
    tree: String,
}

fn yes_p(a: &crate::decide::Answer) -> f64 {
    if a.choice == "yes" {
        a.probability
    } else {
        1.0 - a.probability
    }
}

impl Runtime {
    /// `None` outside a git repository: there's nothing to tie lines to.
    pub(super) fn snapshot_before(&self, session: &Session) -> Option<Before> {
        let root = ledger::repo_root(&session.cwd)?;
        match ledger::snapshot(&root) {
            Ok(tree) => Some(Before { root, tree }),
            Err(e) => {
                tracing::warn!("snapshotting the workspace: {e:#}");
                None
            }
        }
    }

    /// Records what the agent changed since `before` and returns the new
    /// change entries' ids. `handed_off_blind`: `/delegate`, so every hunk
    /// is delegated whatever it relies on (§6).
    pub(super) async fn record_changes(
        &self,
        session: &Session,
        part: &Part,
        before: Option<Before>,
        handed_off_blind: bool,
        instructions: &str,
    ) -> Vec<String> {
        let Some(before) = before else {
            return vec![];
        };
        let diffs = match ledger::snapshot(&before.root)
            .and_then(|after| Ok((ledger::diff(&before.root, &before.tree, &after)?, after)))
        {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("diffing the agent's work: {e:#}");
                return vec![];
            }
        };
        let (files, after) = diffs;
        let workspace = before.root.to_string_lossy().into_owned();
        let views = self
            .concept_views(session, &part.concepts)
            .unwrap_or_default();
        let labels: std::collections::HashMap<String, String> = self
            .store
            .concepts(&understanding::workspace_key(&session.cwd))
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.id, c.label))
            .collect();
        let now = understanding::now();
        let mut ids = Vec::new();
        let mut tagged = 0;
        for FileDiff { path, hunks } in files {
            if path.starts_with(".pathfinder/") {
                continue;
            }
            let new_text = ledger::file_at(&before.root, &after, &path).unwrap_or_default();
            let old_text = ledger::file_at(&before.root, &before.tree, &path);
            for hunk in hunks {
                let hashes = ledger::hunk_hashes(&path, &new_text, old_text.as_deref(), &hunk);
                let (concepts, confidence) = if handed_off_blind || tagged >= MAX_TAGGED_HUNKS {
                    (vec![], 0.0)
                } else {
                    tagged += 1;
                    self.tag(session, part, &labels, &path, &hunk).await
                };
                let states: Vec<(String, ConceptState)> = concepts
                    .iter()
                    .map(|c| (c.clone(), understanding::state_of(&views, c)))
                    .collect();
                let label = if handed_off_blind || states.is_empty() {
                    "delegated"
                } else if states.iter().all(|(_, s)| s.has_evidence()) {
                    "reviewable"
                } else if states
                    .iter()
                    .all(|(_, s)| s.has_evidence() || *s == ConceptState::Claimed)
                {
                    "claimed"
                } else {
                    "delegated"
                };
                let event_ids: Vec<i64> = concepts
                    .iter()
                    .filter_map(|c| views.get(c))
                    .flat_map(|v| v.event_ids.iter().copied())
                    .collect();
                let entry = Entry {
                    id: format!("chg-{}", uuid::Uuid::new_v4().simple()),
                    session_id: session.id.clone(),
                    part_id: part.id.clone(),
                    kind: "change".into(),
                    refers_to: None,
                    path: path.clone(),
                    hunk: hunk.ranges(),
                    line_hashes: hashes,
                    concepts: concepts.clone(),
                    tag_confidence: confidence,
                    label: label.into(),
                    rationale: json!({
                        "summary": part.summary,
                        "instructions": instructions,
                        "objections": part.objections,
                    }),
                    evidence: json!({
                        "events": event_ids,
                        "states": states.iter().map(|(c, s)| (c.clone(), s.label())).collect::<std::collections::HashMap<_, _>>(),
                        "now": now,
                    }),
                    review: String::new(),
                    at: now,
                    prev_hash: String::new(),
                    hash: String::new(),
                };
                match self.store.append_ledger(&workspace, entry) {
                    Ok(e) => ids.push(e.id),
                    Err(e) => tracing::warn!("appending to the ledger: {e:#}"),
                }
            }
        }
        ids
    }

    /// The Tagger (A7): a predicate per core concept of the part plus
    /// "something else", in one decision request. Untagged when no
    /// decision service answers: it then counts as delegated.
    async fn tag(
        &self,
        session: &Session,
        part: &Part,
        labels: &std::collections::HashMap<String, String>,
        path: &str,
        hunk: &Hunk,
    ) -> (Vec<String>, f64) {
        if part.concepts.is_empty() || session.decisions_off {
            return (vec![], 0.0);
        }
        let label = |c: &String| labels.get(c).cloned().unwrap_or_else(|| c.clone());
        let mut questions: Vec<Question> = part
            .concepts
            .iter()
            .enumerate()
            .map(|(i, c)| Question {
                name: format!("t{i}"),
                instructions: format!(
                    "To judge whether this change is right, would someone need to understand \"{}\"?",
                    label(c)
                ),
                options: vec![
                    ("yes".into(), "Judging the change relies on it".into()),
                    ("no".into(), "It doesn't".into()),
                ],
            })
            .collect();
        let names: Vec<String> = part.concepts.iter().map(label).collect();
        questions.push(Question {
            name: "else".into(),
            instructions: format!(
                "Does judging this change rely mainly on something other than these: {}?",
                names.join("; ")
            ),
            options: vec![
                ("yes".into(), "Mainly on something else".into()),
                ("no".into(), "On those".into()),
            ],
        });
        let text: String = hunk.text().chars().take(HUNK_CHARS).collect();
        let input = format!(
            "Part of the task: {}\nFile: {path}\nChange (lines removed with -, added with +):\n{text}",
            part.title
        );
        let decision = match self
            .decider
            .decide(&input, &questions, session.decision_model.as_deref())
            .await
        {
            Ok(Some(d)) => d,
            Ok(None) => return (vec![], 0.0),
            Err(e) => {
                tracing::warn!("tagging a hunk: {e:#}");
                return (vec![], 0.0);
            }
        };
        if decision
            .answers
            .get("else")
            .is_some_and(|a| yes_p(a) >= SOMETHING_ELSE)
        {
            return (vec![], 0.0);
        }
        let mut tagged = Vec::new();
        let mut confidence: f64 = 1.0;
        for (i, c) in part.concepts.iter().enumerate() {
            if let Some(a) = decision.answers.get(&format!("t{i}")) {
                let p = yes_p(a);
                if p >= TAG {
                    tagged.push(c.clone());
                    confidence = confidence.min(p);
                }
            }
        }
        if tagged.is_empty() {
            confidence = 0.0;
        }
        (tagged, confidence)
    }

    /// A review entry per change entry: the verdict comes later than the
    /// change, as its own entry (append-only, A4).
    pub(super) fn record_review(
        &self,
        session: &Session,
        part_id: &str,
        changes: &[String],
        review: &str,
        objection: Option<&str>,
    ) {
        let Some(root) = ledger::repo_root(&session.cwd) else {
            return;
        };
        let workspace = root.to_string_lossy().into_owned();
        let now = understanding::now();
        for id in changes {
            let entry = Entry {
                id: format!("rev-{}", uuid::Uuid::new_v4().simple()),
                session_id: session.id.clone(),
                part_id: part_id.to_string(),
                kind: "review".into(),
                refers_to: Some(id.clone()),
                path: String::new(),
                hunk: String::new(),
                line_hashes: vec![],
                concepts: vec![],
                tag_confidence: 0.0,
                label: String::new(),
                rationale: json!({ "objection": objection }),
                evidence: serde_json::Value::Null,
                review: review.into(),
                at: now,
                prev_hash: String::new(),
                hash: String::new(),
            };
            if let Err(e) = self.store.append_ledger(&workspace, entry) {
                tracing::warn!("appending a review to the ledger: {e:#}");
            }
        }
    }

    /// `/coverage`.
    pub(super) fn coverage_report(&self, session: &Session) -> String {
        let Some(root) = ledger::repo_root(&session.cwd) else {
            return "Coverage needs a git repository: this workspace isn't one.".into();
        };
        match self.store.ledger(&root.to_string_lossy()) {
            Ok(entries) => ledger::render(&ledger::coverage(&root, &entries)),
            Err(e) => format!("Couldn't read the ledger: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::process::Command;
    use std::sync::Arc;

    use anyhow::Result;
    use futures_util::future::BoxFuture;

    use super::*;
    use crate::decide::{Answer, Decide, Decision};
    use crate::domain::{Map, Part};
    use crate::ledger::Bucket;
    use crate::store::Store;
    use crate::understanding::{EventKind, NewEvent};

    struct NoEngine;

    impl crate::engine::Engine for NoEngine {
        fn run_text<'a>(
            &'a self,
            _req: &'a crate::engine::EngineRequest,
            _sink: crate::engine::StreamSink,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { anyhow::bail!("no engine in this test") })
        }
    }

    /// Tags every hunk with every concept.
    struct TagAll;

    impl Decide for TagAll {
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

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[tokio::test]
    async fn agent_changes_are_labelled_chained_and_covered() {
        let dir =
            std::env::temp_dir().join(format!("harness-prov-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("app.py"), "def main():\n    print('hi')\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "init"]);

        let engine = Arc::new(NoEngine);
        let rt = Runtime::new(Arc::new(Store::open_in_memory().unwrap()), engine)
            .with_decider(Arc::new(TagAll));
        let mut session = rt.create_session("s".into(), dir.clone()).unwrap();
        let mut a = Part::new("route".into(), "Routing".into(), String::new(), vec![]);
        a.concepts = vec!["dispatch".into()];
        a.summary = Some("names map to handlers".into());
        let mut b = Part::new("report".into(), "Report".into(), String::new(), vec![]);
        b.concepts = vec!["fields".into()];
        session.map = Map {
            parts: vec![a.clone(), b.clone()],
        };
        rt.append_events(
            &session,
            vec![
                NewEvent::new("dispatch", EventKind::Pass)
                    .detail(serde_json::json!({"flawed": false})),
                NewEvent::new("fields", EventKind::Claim),
            ],
        );

        // A handoff on an understood part: reviewable, then approved.
        let before = rt.snapshot_before(&session);
        std::fs::write(
            dir.join("app.py"),
            "def main():\n    print('hi')\n\ndef route(name):\n    return HANDLERS[name]\n",
        )
        .unwrap();
        let changes = rt.record_changes(&session, &a, before, false, "").await;
        assert_eq!(changes.len(), 1);
        rt.record_review(&session, "route", &changes, "approved", None);

        // Work resting on a claim, and blind delegation.
        let before = rt.snapshot_before(&session);
        std::fs::write(
            dir.join("report.py"),
            "def status():\n    return {'uptime': 1}\n",
        )
        .unwrap();
        rt.record_changes(&session, &b, before, false, "").await;
        let before = rt.snapshot_before(&session);
        std::fs::write(dir.join("extra.py"), "VALUE = compute_value()\n").unwrap();
        rt.record_changes(&session, &a, before, true, "").await;

        let root = crate::ledger::repo_root(&dir).unwrap();
        let entries = rt.store.ledger(&root.to_string_lossy()).unwrap();
        assert!(crate::ledger::verify(&entries).is_ok());
        let labels: Vec<&str> = entries
            .iter()
            .filter(|e| e.kind == "change")
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(labels, vec!["reviewable", "claimed", "delegated"]);

        let c = crate::ledger::coverage(&root, &entries);
        let n = |b: Bucket| c.total.get(&b).copied().unwrap_or(0);
        assert_eq!(n(Bucket::Vouched), 2, "the two new lines in app.py");
        assert_eq!(n(Bucket::NotFromPathfinder), 2, "the original lines");
        assert_eq!(n(Bucket::Claimed), 2);
        assert_eq!(n(Bucket::Delegated), 1);
        // Your own index is untouched by snapshots.
        let staged = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert!(staged.stdout.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
