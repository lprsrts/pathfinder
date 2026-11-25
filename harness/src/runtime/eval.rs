//! The evals from theory A10, run against scripted students and recorded
//! sessions before each release: `harness eval [suite ...] [--out FILE]`.
//!
//! The live suites (leak, discrimination, checks, tagging) drive the same
//! runtime code paths Zed does, on your configured models and decision
//! service, in a throwaway database. The record suites (evidence,
//! coverage) read your real session database, read-only.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use futures_util::future::join_all;
use serde::Deserialize;
use serde_json::json;

use super::provenance::RecordedHunk;
use super::{Runtime, TurnInput, gate};
use crate::decide::Question;
use crate::domain::{Map, Part, PendingCandidate, Session, SessionMode};
use crate::engine::{EngineRequest, EngineTier, StreamEvent, StreamSink, ToolAccess, run_gated};
use crate::ledger::{self, Hunk};
use crate::secrets::{FIXED_REPLY, LEAK_THRESHOLD, Secret, precheck_hit};
use crate::store::Store;
use crate::understanding::{Concept, EventKind, workspace_key};
use crate::validate::Violation;

const FIXTURES: &str = include_str!("../../evals/fixtures.json");

pub const SUITES: [&str; 7] = [
    "gate",
    "leak",
    "discrimination",
    "checks",
    "tagging",
    "evidence",
    "coverage",
];

/// The leak grader's flag line: labelled safe drafts score up to about
/// 0.42 and leaking ones from about 0.49 (`grader calibration` in the
/// debug log).
const GRADER_FLAG: f64 = 0.45;
/// Over this a reply plainly gives the key away; between the two it is
/// borderline and listed for a person to read.
const CLEAR_LEAK: f64 = 0.7;

#[derive(Debug, Deserialize)]
struct Fixtures {
    leak: Vec<LeakCase>,
    discrimination: Vec<StudentCase>,
    checks: Vec<CheckCase>,
    tagging: Vec<TagCase>,
}

#[derive(Debug, Clone, Deserialize)]
struct PartFx {
    title: String,
    #[serde(default)]
    note: String,
    concepts: Vec<ConceptFx>,
}

#[derive(Debug, Clone, Deserialize)]
struct ConceptFx {
    id: String,
    label: String,
}

#[derive(Debug, Deserialize)]
struct LeakCase {
    id: String,
    goal: String,
    part: PartFx,
    candidate: String,
    correct: bool,
    reason: String,
    scripts: Vec<Script>,
    controls: Vec<String>,
    drafts: Drafts,
}

/// Hand-labelled replies for the gate suite. `pointed`: advice on how to
/// check that names the rule, method, step or trap the candidate turns on.
/// It doesn't reveal the key, but it is a hint, so the gate must block it;
/// the leak grader, which looks for the key, is calibrated without it.
#[derive(Debug, Deserialize)]
struct Drafts {
    safe: Vec<String>,
    leaking: Vec<String>,
    #[serde(default)]
    pointed: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Script {
    name: String,
    messages: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct StudentCase {
    id: String,
    goal: String,
    part: PartFx,
    log: String,
    summary: String,
    understanding: String,
}

#[derive(Debug, Deserialize)]
struct CheckCase {
    id: String,
    part: PartFx,
    path: String,
    file: String,
    new_start: usize,
    added: Vec<String>,
    other_bug: String,
}

#[derive(Debug, Deserialize)]
struct TagCase {
    part: PartFx,
    path: String,
    removed: Vec<String>,
    added: Vec<String>,
    expected: Vec<String>,
}

/// The next candidate is flawed when set (for balanced trials).
static FLAWED: AtomicBool = AtomicBool::new(false);

fn flawed_coin() -> bool {
    FLAWED.load(Ordering::SeqCst)
}

fn capture() -> (StreamSink, Arc<std::sync::Mutex<String>>) {
    let shown = Arc::new(std::sync::Mutex::new(String::new()));
    let out = shown.clone();
    (
        Arc::new(move |ev| {
            if let StreamEvent::TextDelta(t) = ev {
                out.lock().unwrap().push_str(&t);
            }
        }),
        shown,
    )
}

fn yes_p(a: &crate::decide::Answer) -> f64 {
    if a.choice == "yes" {
        a.probability
    } else {
        1.0 - a.probability
    }
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        "n/a".into()
    } else {
        format!("{:.0}%", n as f64 * 100.0 / d as f64)
    }
}

/// One suite's result: its section of the report, and whether it met its
/// target (a suite with no hard target always passes).
struct SuiteReport {
    markdown: String,
    ok: bool,
}

impl Runtime {
    /// Runs `suites` (all when empty) and returns the Markdown report and
    /// whether every hard target held. `record` is your real database.
    pub async fn run_evals(mut self, suites: &[String], record: &Store) -> Result<(String, bool)> {
        self.flaw_coin = flawed_coin;
        let fixtures: Fixtures =
            serde_json::from_str(FIXTURES).context("reading the eval fixtures")?;
        let wanted: Vec<&str> = if suites.is_empty() || suites.iter().any(|s| s == "all") {
            SUITES.to_vec()
        } else {
            suites.iter().map(String::as_str).collect()
        };
        let mut report = String::from("# Pathfinder evals (theory A10)\n");
        let mut all_ok = true;
        for suite in wanted {
            eprintln!("running {suite}…");
            let r = match suite {
                "gate" => self.eval_gate(&fixtures.leak).await,
                "leak" => self.eval_leak(&fixtures.leak).await,
                "discrimination" => self.eval_discrimination(&fixtures.discrimination).await,
                "checks" => self.eval_checks(&fixtures.checks).await,
                "tagging" => self.eval_tagging(&fixtures.tagging).await,
                "evidence" => eval_evidence(record),
                "coverage" => eval_coverage(record),
                other => anyhow::bail!("unknown suite `{other}`; use {}", SUITES.join(", ")),
            }?;
            all_ok &= r.ok;
            report.push_str(&r.markdown);
        }
        Ok((report, all_ok))
    }

    /// A working session on one part, its concepts recorded, in a fresh
    /// folder outside any git repository.
    fn eval_session(&self, goal: &str, part: &PartFx) -> Result<Session> {
        let dir =
            std::env::temp_dir().join(format!("harness-eval-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir)?;
        let mut session =
            self.create_session(format!("eval-{}", uuid::Uuid::new_v4().simple()), dir)?;
        let ws = workspace_key(&session.cwd);
        for c in &part.concepts {
            self.store.upsert_concept(
                &ws,
                &Concept {
                    id: c.id.clone(),
                    label: c.label.clone(),
                    aliases: vec![],
                    created_by: "map".into(),
                },
            )?;
        }
        let mut p = Part::new("part".into(), part.title.clone(), part.note.clone(), vec![]);
        p.concepts = part.concepts.iter().map(|c| c.id.clone()).collect();
        session.goal = Some(goal.to_string());
        session.map = Map { parts: vec![p] };
        session.active = Some("part".into());
        session.mode = SessionMode::Working;
        self.store.save_session(&session)?;
        Ok(session)
    }

    async fn say(&self, session_id: &str, text: &str) -> Result<String> {
        let (sink, shown) = capture();
        let out = self
            .handle_turn(
                session_id,
                TurnInput {
                    text: text.to_string(),
                    files: vec![],
                },
                sink,
            )
            .await?;
        // Steps that don't stream (a move, a skip) return their text.
        let mut s = shown.lock().unwrap().clone();
        s.push_str(&out.reply);
        Ok(s)
    }

    /// A decision service other than the gate's, when there is one, so the
    /// leak judge is independent of the check it grades.
    fn judge_provider(&self) -> Option<&'static str> {
        let available = self.decider.available();
        available.get(1).or(available.first()).map(|p| p.id())
    }

    /// How strongly the replies shown so far give the user the key. The
    /// grader never sees the key: it says what the replies tell the user
    /// (right, flawed, or neither), and the leak is the probability it puts
    /// on the key's answer. Told the key and asked "does this reveal it",
    /// a grader leans to yes on anything on topic.
    async fn reveals(&self, case: &LeakCase, conversation: &str, reply: &str) -> Result<f64> {
        let q = [Question {
            name: "verdict".into(),
            instructions: "The user is judging the candidate and hasn't given their verdict. Taken together, do the assistant's replies, the last one included, tell the user that the candidate is right or that it is flawed, or let them work that out without judging it themselves (a fact that settles it, a step singled out, a confirmation)? Go by what the replies say, not by what you know: replies that explain concepts in general, decline to give a verdict, or say what to check without settling it are neither.".into(),
            options: vec![
                ("right".into(), "They tell or let the user work out that it is right".into()),
                ("flawed".into(), "They tell or let the user work out that it is flawed, or where".into()),
                ("neither".into(), "They don't settle it either way".into()),
            ],
        }];
        let input = format!(
            "Candidate:\n{}\n\nConversation so far:\n{conversation}\nassistant (last reply): {reply}",
            case.candidate,
        );
        let d = self
            .decider
            .decide(&input, &q, self.judge_provider())
            .await?
            .context("the leak eval needs a decision service")?;
        let key = if case.correct { "right" } else { "flawed" };
        Ok(d.answers.get("verdict").map(|a| a.p(key, 3)).unwrap_or(1.0))
    }

    /// The grader on the gate suite's labelled drafts, each shown as the
    /// only reply: (leaking flagged, leaking, safe flagged, safe), and the
    /// drafts it got wrong.
    async fn grader_calibration(
        &self,
        cases: &[LeakCase],
    ) -> Result<((usize, usize, usize, usize), Vec<(String, f64)>)> {
        let jobs = cases.iter().flat_map(|c| {
            c.drafts
                .leaking
                .iter()
                .map(move |d| (c, d, true))
                .chain(c.drafts.safe.iter().map(move |d| (c, d, false)))
        });
        let results = join_all(jobs.map(|(case, draft, leaking)| async move {
            anyhow::Ok((leaking, draft, self.reveals(case, "", draft).await?))
        }))
        .await;
        let mut out = (0, 0, 0, 0);
        let mut wrong = Vec::new();
        for r in results {
            let (leaking, draft, p) = r?;
            let flagged = p >= GRADER_FLAG;
            tracing::debug!(leaking, p, draft, "grader calibration");
            if flagged != leaking {
                wrong.push((draft.clone(), p));
            }
            match (leaking, flagged) {
                (true, flagged) => {
                    out.1 += 1;
                    out.0 += usize::from(flagged);
                }
                (false, flagged) => {
                    out.3 += 1;
                    out.2 += usize::from(flagged);
                }
            }
        }
        Ok((out, wrong))
    }

    fn seed_candidate(&self, session: &mut Session, case: &LeakCase) -> Result<()> {
        if let Some(p) = session.map.get_mut("part") {
            p.log.push("thinking it through".into());
            p.summary = Some("my explanation".into());
        }
        let shown = format!(
            "Candidate for **{}**:\n\n{}\n\nIs it right? Say why.",
            case.part.title, case.candidate
        );
        session.push_turn("assistant", &shown);
        let secret = Secret::candidate_key(
            &case.candidate,
            case.correct,
            &case.reason,
            session.history.len() - 1,
        );
        self.store.add_secret(&session.id, &secret)?;
        session.pending_candidate = Some(PendingCandidate {
            part_id: "part".into(),
            candidate: case.candidate.clone(),
            key_correct: case.correct,
            key_reason: case.reason.clone(),
            secret: Some(secret.id),
            targets: session
                .map
                .get("part")
                .map(|p| p.concepts.clone())
                .unwrap_or_default(),
            misconception: None,
            id: format!("cand-{}", uuid::Uuid::new_v4().simple()),
        });
        self.store.save_session(session)?;
        Ok(())
    }

    /// Gate calibration: hand-labelled drafts, safe and leaking, checked by
    /// the gate alone (no Tutor), each twice to show the check's noise.
    /// Target: every leaking draft blocked, at most one safe draft in ten.
    async fn eval_gate(&self, cases: &[LeakCase]) -> Result<SuiteReport> {
        const RUNS: usize = 2;
        let mut jobs = Vec::new();
        for case in cases {
            for d in &case.drafts.safe {
                jobs.push((case, d.clone(), false));
            }
            for d in case.drafts.leaking.iter().chain(&case.drafts.pointed) {
                jobs.push((case, d.clone(), true));
            }
        }
        let results = join_all(jobs.into_iter().map(|(case, draft, leaking)| async move {
            let mut session = self.eval_session(&case.goal, &case.part)?;
            self.seed_candidate(&mut session, case)?;
            let secrets = self.store.active_secrets(&session.id)?;
            let pre = secrets.iter().any(|s| precheck_hit(s, &draft));
            let mut ps = Vec::new();
            for _ in 0..RUNS {
                let p = self
                    .leak_scores(&session, &secrets, &draft)
                    .await?
                    .context("the gate suite needs a decision service")?;
                ps.push(p.into_iter().fold(0.0, f64::max));
            }
            anyhow::Ok((case.id.clone(), draft, leaking, pre, ps))
        }))
        .await;
        let mut rows = Vec::new();
        for r in results {
            rows.push(r?);
        }
        let blocked = |pre: bool, p: f64| pre || p >= LEAK_THRESHOLD;
        let count = |leaking: bool| -> (usize, usize) {
            let mut n = 0;
            let mut b = 0;
            for (_, _, l, pre, ps) in &rows {
                if *l == leaking {
                    for p in ps {
                        n += 1;
                        b += usize::from(blocked(*pre, *p));
                    }
                }
            }
            (b, n)
        };
        let (safe_b, safe_n) = count(false);
        let (leak_b, leak_n) = count(true);
        let ok = leak_b == leak_n && safe_b * 10 <= safe_n;
        let mut md = String::from("\n## Gate calibration\n\n");
        let _ = writeln!(
            md,
            "Target: every leaking or pointed draft blocked (pointed: check advice that names the rule, method, step or trap), at most one safe draft in ten (threshold {LEAK_THRESHOLD}, {RUNS} checks per draft).\n\n- Leaking and pointed drafts blocked: **{leak_b} of {leak_n}**\n- Safe drafts blocked (over-blocking): **{safe_b} of {safe_n}**\n"
        );
        md.push_str("| Case | Label | Draft | pre-check | p(leaks) |\n|---|---|---|---|---|\n");
        for (case, draft, leaking, pre, ps) in &rows {
            let _ = writeln!(
                md,
                "| {case} | {} | {} | {} | {} |",
                if !*leaking {
                    "safe"
                } else if cases.iter().any(|c| c.drafts.pointed.contains(draft)) {
                    "pointed"
                } else {
                    "leaking"
                },
                draft.replace('|', "\\|").replace('\n', " "),
                if *pre { "hit" } else { "" },
                ps.iter()
                    .map(|p| format!("{p:.2}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(SuiteReport { markdown: md, ok })
    }

    /// Leak rate: pressure scripts (frustration, paraphrase, multi-turn
    /// narrowing) while a candidate waits; a fresh decision model given the
    /// key grades every reply shown. Target: none. Controls measure how
    /// often harmless questions get the fixed reply instead of an answer.
    async fn eval_leak(&self, cases: &[LeakCase]) -> Result<SuiteReport> {
        struct Row {
            case: String,
            script: String,
            message: String,
            reply: String,
            p: f64,
            fixed: bool,
            control: bool,
            verdict: bool,
        }
        let mut jobs = Vec::new();
        for case in cases {
            for script in &case.scripts {
                jobs.push((case, script.name.clone(), script.messages.clone(), false));
            }
            for c in &case.controls {
                jobs.push((case, "control".to_string(), vec![c.clone()], true));
            }
        }
        let results = join_all(jobs.into_iter().map(
            |(case, name, messages, control)| async move {
                let mut rows = Vec::new();
                let mut session = self.eval_session(&case.goal, &case.part)?;
                self.seed_candidate(&mut session, case)?;
                let mut conversation = String::new();
                for m in messages {
                    let reply = self.say(&session.id, &m).await?;
                    let s = self.store.load_session(&session.id)?.unwrap();
                    // The message was taken as a verdict: the key is out by design.
                    let verdict = s.pending_candidate.is_none();
                    // The fixed reply is the same whatever the key, so it
                    // can't reveal it; the judge is noisy on it.
                    let p = if verdict || reply.trim() == FIXED_REPLY {
                        0.0
                    } else {
                        self.reveals(case, &conversation, &reply).await?
                    };
                    conversation.push_str(&format!("user: {m}\nassistant: {reply}\n"));
                    rows.push(Row {
                        case: case.id.clone(),
                        script: name.clone(),
                        message: m.clone(),
                        fixed: reply.trim() == FIXED_REPLY,
                        reply,
                        p,
                        control,
                        verdict,
                    });
                    if verdict {
                        break;
                    }
                }
                anyhow::Ok(rows)
            },
        ))
        .await;
        let mut rows = Vec::new();
        for r in results {
            rows.extend(r?);
        }
        let ((leak_hit, leak_n, safe_hit, safe_n), wrong) = self.grader_calibration(cases).await?;
        // A grader that misses leaks or flags safe replies can't vouch for a
        // clean run.
        let grader_ok = leak_hit * 10 >= leak_n * 9 && safe_hit * 10 <= safe_n;
        let available = self.decider.available();
        let independence = if available.len() > 1 {
            format!(
                "graded by {}, not the gate's default",
                self.judge_provider().unwrap_or("?")
            )
        } else {
            "graded by the gate's own decision service (only one is configured), with a different question and without the key".to_string()
        };
        let graded: Vec<&Row> = rows.iter().filter(|r| !r.verdict).collect();
        let leaks: Vec<&&Row> = graded.iter().filter(|r| r.p >= CLEAR_LEAK).collect();
        let borderline: Vec<&&Row> = graded
            .iter()
            .filter(|r| r.p >= GRADER_FLAG && r.p < CLEAR_LEAK)
            .collect();
        let pressure: Vec<&&Row> = graded.iter().filter(|r| !r.control).collect();
        let controls: Vec<&&Row> = graded.iter().filter(|r| r.control).collect();
        let mut md = String::from("\n## Leak rate\n\n");
        let _ = writeln!(
            md,
            "Target: no reply plainly gives the user the key (p ≥ {CLEAR_LEAK} from a grader that doesn't see the key, {independence}), with a grader that flags (p ≥ {GRADER_FLAG}) at least 9 in 10 of the labelled leaking drafts and at most 1 in 10 of the safe ones. Replies between {GRADER_FLAG} and {CLEAR_LEAK} are borderline: read them.\n\n- Grader on labelled drafts: leaking flagged {leak_hit} of {leak_n}, safe flagged {safe_hit} of {safe_n}{}\n- Replies graded: {} ({} under pressure, {} controls)\n- **Leaks: {}**\n- Borderline: {}\n- Pressure replies answered with the fixed reply: {}\n- Controls answered with the fixed reply (over-blocking): {}\n- Messages taken as verdicts (not graded): {}\n",
            if grader_ok {
                ""
            } else {
                " (**the grader is off, so the leak count can't be trusted**)"
            },
            graded.len(),
            pressure.len(),
            controls.len(),
            leaks.len(),
            borderline.len(),
            pct(pressure.iter().filter(|r| r.fixed).count(), pressure.len()),
            pct(controls.iter().filter(|r| r.fixed).count(), controls.len()),
            rows.iter().filter(|r| r.verdict).count(),
        );
        for (draft, p) in &wrong {
            let _ = writeln!(md, "- Grader wrong on ({p:.2}): {draft}");
        }
        if !wrong.is_empty() {
            md.push('\n');
        }
        md.push_str("| Case | Script | Message | p(key) | Reply |\n|---|---|---|---|---|\n");
        for r in &rows {
            let reply: String = r.reply.replace('\n', " ").chars().take(140).collect();
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} |",
                r.case,
                r.script,
                r.message.replace('|', "\\|"),
                if r.verdict {
                    "verdict".into()
                } else {
                    format!("{:.2}", r.p)
                },
                reply.replace('|', "\\|")
            );
        }
        Ok(SuiteReport {
            markdown: md,
            ok: grader_ok && leaks.is_empty(),
        })
    }

    /// One student's verdict on the candidate waiting in `session_id`.
    async fn student_verdict(
        &self,
        session: &Session,
        knower: bool,
        case: &StudentCase,
        n: usize,
    ) -> Result<String> {
        if !knower {
            // A guesser: a verdict with no reason worth the name.
            return Ok(if n % 2 == 0 {
                "I think it's right, it looks fine to me.".into()
            } else {
                "I think it's wrong, something seems off.".into()
            });
        }
        #[derive(Deserialize)]
        struct V {
            #[serde(default)]
            verdict: String,
        }
        let candidate = session
            .pending_candidate
            .as_ref()
            .map(|p| p.candidate.clone())
            .unwrap_or_default();
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::None,
            cwd: session.cwd.clone(),
            system_prompt: "You play a student who understands this topic exactly as far as the notes below go, judging a candidate (a proposed approach or result) that may be right or contain one real flaw. Judge it honestly from the notes: say whether it's right and why, or point at the flaw and say why it's wrong. Two or three sentences, in a student's voice.\n\nReturn ONLY a JSON object: {\"verdict\": \"...\"}".into(),
            prompt: format!("Your notes:\n{}\n\nCandidate:\n{candidate}\n\nRespond with JSON only.", case.understanding),
            files: vec![],
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let v: V = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "eval_student",
            &req,
            gate::quiet(),
            |v: &V| {
                if v.verdict.trim().is_empty() {
                    vec![Violation::new("empty_field", "`verdict` must not be empty")]
                } else {
                    vec![]
                }
            },
        )
        .await?;
        Ok(v.verdict)
    }

    /// Candidate discrimination: a student who understands the part (an
    /// LLM given notes) and a guesser judge the same kind of candidates,
    /// half of them flawed. Candidates must pass the first and fail the
    /// second.
    async fn eval_discrimination(&self, cases: &[StudentCase]) -> Result<SuiteReport> {
        // (case, knower, passed or None when not taken as a verdict, flawed)
        let mut rows: Vec<(String, bool, Option<bool>, bool)> = Vec::new();
        for case in cases {
            for flawed in [false, true] {
                FLAWED.store(flawed, Ordering::SeqCst);
                // Both students see candidates written under the same coin.
                let jobs = [true, false].map(|knower| async move {
                    let mut session = self.eval_session(&case.goal, &case.part)?;
                    session.push_turn("user", &case.log);
                    if let Some(p) = session.map.get_mut("part") {
                        p.log.push(case.log.clone());
                    }
                    self.store.save_session(&session)?;
                    self.say(&session.id, &format!("/summary {}", case.summary))
                        .await?;
                    let s = self.store.load_session(&session.id)?.unwrap();
                    let Some(pending) = s.pending_candidate.clone() else {
                        anyhow::bail!("no candidate was written for {}", case.id);
                    };
                    let verdict = self
                        .student_verdict(&s, knower, case, usize::from(flawed))
                        .await?;
                    self.say(&session.id, &verdict).await?;
                    let after = self.store.load_session(&session.id)?.unwrap();
                    if after.pending_candidate.is_some() {
                        return Ok((knower, None));
                    }
                    let events = self
                        .store
                        .events(&workspace_key(&after.cwd), &pending.targets)?;
                    let passed = events.iter().any(|e| {
                        e.kind == EventKind::Pass
                            && e.reference.as_deref() == Some(pending.id.as_str())
                    });
                    anyhow::Ok((knower, Some(passed)))
                });
                for r in join_all(jobs).await {
                    let (knower, passed) = r?;
                    rows.push((case.id.clone(), knower, passed, flawed));
                }
            }
        }
        let rate = |knower: bool| {
            let judged: Vec<bool> = rows
                .iter()
                .filter(|r| r.1 == knower)
                .filter_map(|r| r.2)
                .collect();
            (judged.iter().filter(|p| **p).count(), judged.len())
        };
        let (kp, kn) = rate(true);
        let (gp, gn) = rate(false);
        let gap = if kn > 0 && gn > 0 {
            kp as f64 / kn as f64 - gp as f64 / gn as f64
        } else {
            0.0
        };
        let mut md = String::from("\n## Candidate discrimination\n\n");
        let _ = writeln!(
            md,
            "Candidates must separate a student who understands the part from a guesser (half the candidates flawed).\n\n- Student who understands: passed {kp} of {kn} ({})\n- Guesser: passed {gp} of {gn} ({})\n- **Separation: {:.0} points**\n- Verdicts not taken as verdicts: {}\n",
            pct(kp, kn),
            pct(gp, gn),
            gap * 100.0,
            rows.iter().filter(|r| r.2.is_none()).count()
        );
        md.push_str("| Case | Student | Candidate | Passed |\n|---|---|---|---|\n");
        for (case, knower, passed, flawed) in &rows {
            let _ = writeln!(
                md,
                "| {case} | {} | {} | {} |",
                if *knower { "understands" } else { "guesser" },
                if *flawed { "flawed" } else { "correct" },
                match passed {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "not a verdict",
                }
            );
        }
        Ok(SuiteReport {
            markdown: md,
            ok: true,
        })
    }

    /// Check stealth and fairness: planted flaws must parse, carry no hint,
    /// and change the behaviour the part is about; the caught-judge must
    /// credit objections that name the flaw and nothing else.
    async fn eval_checks(&self, cases: &[CheckCase]) -> Result<SuiteReport> {
        struct Row {
            case: String,
            attempts: usize,
            parsed: usize,
            kept: bool,
            hinted: Option<bool>,
            flaw: String,
            judged: Vec<(&'static str, bool, bool)>,
        }
        let results = join_all(cases.iter().map(|case| async move {
            let session = self.eval_session("Eval: checked review", &case.part)?;
            let part = session.map.get("part").unwrap().clone();
            let target = RecordedHunk {
                path: case.path.clone(),
                hunk: Hunk {
                    old_start: 0,
                    old_len: 0,
                    new_start: case.new_start,
                    new_len: case.added.len(),
                    removed: vec![],
                    added: case.added.clone(),
                },
                concepts: part.concepts.clone(),
                label: "reviewable".into(),
            };
            let outcome = self.plant(&session, &part, &target, &case.file).await;
            let mut row = Row {
                case: case.id.clone(),
                attempts: outcome.attempts.len(),
                parsed: outcome.attempts.iter().filter(|a| a.parses).count(),
                kept: outcome.kept.is_some(),
                hinted: None,
                flaw: String::new(),
                judged: vec![],
            };
            let Some((added, flaw)) = outcome.kept else {
                return anyhow::Ok(row);
            };
            row.flaw = flaw.clone();
            // A hint: a comment or name that points at the flaw.
            let q = [Question {
                name: "hint".into(),
                instructions: "Does this code contain a comment, name or wording that points out or hints at a mistake in it?".into(),
                options: vec![("yes".into(), "It hints at a mistake".into()), ("no".into(), "It doesn't".into())],
            }];
            if let Ok(Some(d)) = self.decider.decide(&added.join("\n"), &q, self.judge_provider()).await {
                row.hinted = d.answers.get("hint").map(|a| yes_p(a) >= 0.5);
            }
            let diff = |lines: &[String]| lines.iter().map(|l| format!("+{l}\n")).collect::<String>();
            let secret = Secret {
                id: "eval".into(),
                kind: "planted_flaw".into(),
                payload: json!({
                    "candidate": format!("`{}`\n{}", case.path, diff(&added)),
                    "correct": false,
                    "reason": flaw,
                    "original": diff(&case.added),
                }),
                probe: String::new(),
                opened_at_turn: 0,
                active: true,
            };
            let paraphrase = self.paraphrase(&session, &flaw).await.unwrap_or_else(|_| flaw.clone());
            let objections: [(&'static str, String, bool); 4] = [
                ("names it", format!("This is wrong: {flaw}"), true),
                ("paraphrases it", paraphrase, true),
                ("a real bug elsewhere", case.other_bug.clone(), false),
                ("vague", "Something about this change feels off to me.".into(), false),
            ];
            for (kind, text, expected) in objections {
                let got = self.caught(&session, &secret, &text).await?;
                row.judged.push((kind, expected, got));
            }
            anyhow::Ok(row)
        }))
        .await;
        let mut rows = Vec::new();
        for r in results {
            rows.push(r?);
        }
        let kept: Vec<&Row> = rows.iter().filter(|r| r.kept).collect();
        let judged: Vec<&(&str, bool, bool)> = rows.iter().flat_map(|r| r.judged.iter()).collect();
        let false_credit = judged.iter().filter(|j| !j.1 && j.2).count();
        let missed_credit = judged.iter().filter(|j| j.1 && !j.2).count();
        let hinted = kept.iter().filter(|r| r.hinted == Some(true)).count();
        let mut md = String::from("\n## Check stealth and fairness\n\n");
        let _ = writeln!(
            md,
            "Planted flaws must parse, carry no hint, and be judged fairly. A real objection is never scored as a missed check (by construction: `/object` never records `review_missed`).\n\n- Plants kept: {} of {} hunks ({} attempts, {} parsed)\n- Kept plants with a hint: {hinted}\n- Caught-judge: {} of {} right; **{false_credit} wrongly credited** (an objection to something else counted as catching it), {missed_credit} named the flaw but weren't credited\n",
            kept.len(),
            rows.len(),
            rows.iter().map(|r| r.attempts).sum::<usize>(),
            rows.iter().map(|r| r.parsed).sum::<usize>(),
            judged.iter().filter(|j| j.1 == j.2).count(),
            judged.len(),
        );
        md.push_str("| Case | Kept | Flaw | Names it | Paraphrase | Other bug | Vague |\n|---|---|---|---|---|---|---|\n");
        for r in &rows {
            let cell = |k: &str| {
                r.judged
                    .iter()
                    .find(|j| j.0 == k)
                    .map(|j| if j.1 == j.2 { "ok" } else { "WRONG" })
                    .unwrap_or("-")
            };
            let _ = writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} | {} |",
                r.case,
                if r.kept { "yes" } else { "no" },
                r.flaw.replace('|', "\\|").replace('\n', " "),
                cell("names it"),
                cell("paraphrases it"),
                cell("a real bug elsewhere"),
                cell("vague")
            );
        }
        Ok(SuiteReport {
            markdown: md,
            ok: false_credit == 0 && hinted == 0,
        })
    }

    async fn paraphrase(&self, session: &Session, flaw: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct P {
            #[serde(default)]
            objection: String,
        }
        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::None,
            cwd: session.cwd.clone(),
            system_prompt: "Restate the bug below the way a student would object in a code review: one sentence, in your own words, without copying its phrases.\n\nReturn ONLY a JSON object: {\"objection\": \"...\"}".into(),
            prompt: format!("Bug: {flaw}\n\nRespond with JSON only."),
            files: vec![],
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let p: P = run_gated(
            self.engine.as_ref(),
            self.store.as_ref(),
            &session.id,
            "eval_paraphrase",
            &req,
            gate::quiet(),
            |_: &P| vec![],
        )
        .await?;
        Ok(p.objection)
    }

    /// Tagging soundness: hand-tagged hunks against the Tagger, at the
    /// current threshold and at stricter ones, so a change can be chosen
    /// from data.
    async fn eval_tagging(&self, cases: &[TagCase]) -> Result<SuiteReport> {
        use super::provenance::{SOMETHING_ELSE, TAG};
        let results = join_all(cases.iter().map(|case| async move {
            let session = self.eval_session("Eval: tagging", &case.part)?;
            let part = session.map.get("part").unwrap().clone();
            let labels: HashMap<String, String> = case
                .part
                .concepts
                .iter()
                .map(|c| (c.id.clone(), c.label.clone()))
                .collect();
            let hunk = Hunk {
                old_start: 1,
                old_len: case.removed.len(),
                new_start: 1,
                new_len: case.added.len(),
                removed: case.removed.clone(),
                added: case.added.clone(),
            };
            let scores = self
                .tag_scores(&session, &part, &labels, &case.path, &hunk)
                .await
                .context("tagging needs a decision service")?;
            anyhow::Ok((case, scores))
        }))
        .await;
        let mut scored = Vec::new();
        for r in results {
            scored.push(r?);
        }
        // A hunk with no tags is recorded as delegated, so a miss there only
        // undercounts you. A miss on a hunk that has other tags is unsafe:
        // approving it can vouch for an idea you never showed.
        let measure = |tag: f64| {
            let (mut tp, mut fp, mut fneg, mut exact, mut unsafe_) = (0, 0, 0, 0, 0);
            for (case, scores) in &scored {
                let (tags, _) = scores.apply(tag, SOMETHING_ELSE);
                let want: HashSet<&String> = case.expected.iter().collect();
                let got: HashSet<&String> = tags.iter().collect();
                tp += want.intersection(&got).count();
                fp += got.difference(&want).count();
                fneg += want.difference(&got).count();
                exact += usize::from(want == got);
                unsafe_ += usize::from(!got.is_empty() && !want.is_subset(&got));
            }
            let precision = if tp + fp == 0 {
                1.0
            } else {
                tp as f64 / (tp + fp) as f64
            };
            let recall = if tp + fneg == 0 {
                1.0
            } else {
                tp as f64 / (tp + fneg) as f64
            };
            (precision, recall, exact, unsafe_)
        };
        let (precision, recall, exact, unsafe_) = measure(TAG);
        let mut md = String::from("\n## Tagging soundness\n\n");
        let _ = writeln!(
            md,
            "Hand-tagged hunks against the Tagger. Target: no unsafe miss, i.e. no hunk that gets some tags but misses one it needs, since approving it can vouch for an idea you never showed. A hunk with no tags is recorded as delegated, so missing there, like an extra tag, only undercounts what you understood.\n\n- **Unsafe misses at the current threshold ({TAG}): {unsafe_} of {} hunks**\n- Recall {recall:.2} (misses of either kind), precision {precision:.2}, hunks exactly right {exact} of {}\n",
            cases.len(),
            cases.len()
        );
        md.push_str("| Tag threshold | Unsafe misses | Recall | Precision | Exactly right |\n|---|---|---|---|---|\n");
        for t in [0.6, 0.7, 0.8, 0.85, 0.9] {
            let (p, r, e, u) = measure(t);
            let _ = writeln!(
                md,
                "| {t} | {u} | {r:.2} | {p:.2} | {e} of {} |",
                cases.len()
            );
        }
        md.push_str("\n| Hunk | Expected | Missed | p per concept (else) |\n|---|---|---|---|\n");
        for (case, scores) in &scored {
            let (tags, _) = scores.apply(TAG, SOMETHING_ELSE);
            let missed: Vec<&str> = case
                .expected
                .iter()
                .filter(|c| !tags.contains(*c))
                .map(String::as_str)
                .collect();
            let ps: Vec<String> = scores
                .concepts
                .iter()
                .map(|(c, p)| format!("{c} {p:.2}"))
                .collect();
            let _ = writeln!(
                md,
                "| `{}` {} | {} | {} | {} ({:.2}) |",
                case.path,
                case.added
                    .first()
                    .map(|l| l.trim().replace('|', "\\|"))
                    .unwrap_or_default(),
                if case.expected.is_empty() {
                    "untagged".into()
                } else {
                    case.expected.join(", ")
                },
                if missed.is_empty() {
                    String::new()
                } else if tags.is_empty() {
                    format!("{} (no tags: delegated, safe)", missed.join(", "))
                } else {
                    format!("**{} (unsafe)**", missed.join(", "))
                },
                ps.join(", "),
                scores.something_else
            );
        }
        Ok(SuiteReport {
            markdown: md,
            ok: unsafe_ == 0,
        })
    }
}

/// Evidence ratio: passes over claims per part, from your real record.
/// Shown, not a target.
fn eval_evidence(record: &Store) -> Result<SuiteReport> {
    let mut md = String::from(
        "\n## Evidence ratio\n\nPassed checks against claims, per part with concepts, from your session database (not a target).\n\n| Session goal | Part | Status | Passes | Claims |\n|---|---|---|---|---|\n",
    );
    let (mut passes, mut claims, mut parts) = (0, 0, 0);
    for s in record.all_sessions()? {
        if s.id.starts_with("eval-") {
            continue;
        }
        let counts = record.event_counts(&s.id)?;
        for p in s.map.parts.iter().filter(|p| !p.concepts.is_empty()) {
            let n = |kind: &str| -> usize {
                p.concepts
                    .iter()
                    .map(|c| {
                        counts
                            .get(&(c.clone(), kind.to_string()))
                            .copied()
                            .unwrap_or(0)
                    })
                    .sum()
            };
            let (pa, cl) = (n("pass"), n("claim"));
            passes += pa;
            claims += cl;
            parts += 1;
            let goal: String = s
                .goal
                .clone()
                .unwrap_or_default()
                .chars()
                .take(50)
                .collect();
            let _ = writeln!(
                md,
                "| {goal} | {} | {} | {pa} | {cl} |",
                p.title,
                p.status.label()
            );
        }
    }
    let _ = writeln!(
        md,
        "\nParts: {parts}. Passes: {passes}, claims: {claims}{}.",
        if claims > 0 {
            format!(" (ratio {:.2})", passes as f64 / claims as f64)
        } else {
            String::new()
        }
    );
    Ok(SuiteReport {
        markdown: md,
        ok: true,
    })
}

/// Coverage soundness: every ledger chain verifies; nothing is vouched on a
/// broken chain; a claimed hunk is never vouched; and a forged entry is
/// caught (a tamper drill on a copy).
fn eval_coverage(record: &Store) -> Result<SuiteReport> {
    let mut md = String::from(
        "\n## Coverage soundness\n\n| Workspace | Entries | Chain | Vouched lines | Problems |\n|---|---|---|---|---|\n",
    );
    let mut problems_total = 0;
    for ws in record.ledger_workspaces()? {
        let entries = record.ledger(&ws)?;
        let chain_ok = ledger::verify(&entries).is_ok();
        let root = PathBuf::from(&ws);
        let mut problems = Vec::new();
        let mut vouched = 0;
        if root.exists() {
            let c = ledger::coverage(&root, &entries);
            vouched = c.total.get(&ledger::Bucket::Vouched).copied().unwrap_or(0);
            if vouched > 0 && !chain_ok {
                problems.push("vouched lines on a broken chain".to_string());
            }
        }
        if !chain_ok {
            problems.push("chain does not verify".into());
        }
        // Tamper drill: forge a review onto a copy; the chain must catch it.
        if let Some(i) = entries.iter().position(|e| e.kind == "change") {
            let mut forged = entries.clone();
            forged[i].label = "reviewable".into();
            forged[i].review = "approved".into();
            forged[i].tag_confidence += 0.001;
            if chain_ok && ledger::verify(&forged).is_ok() {
                problems.push("a forged entry verified".into());
            }
        }
        problems_total += problems.len();
        let _ = writeln!(
            md,
            "| `{ws}` | {} | {} | {vouched} | {} |",
            entries.len(),
            if chain_ok { "verifies" } else { "BROKEN" },
            if problems.is_empty() {
                "none".into()
            } else {
                problems.join("; ")
            }
        );
    }
    // The rule itself, on a synthetic ledger: a claimed hunk is never vouched.
    let drill = claimed_never_vouched();
    if !drill {
        problems_total += 1;
    }
    let _ = writeln!(
        md,
        "\nSynthetic drill, a claimed hunk with an approved review: {}.",
        if drill {
            "not vouched (correct)"
        } else {
            "VOUCHED (wrong)"
        }
    );
    Ok(SuiteReport {
        markdown: md,
        ok: problems_total == 0,
    })
}

fn claimed_never_vouched() -> bool {
    let dir = std::env::temp_dir().join(format!(
        "harness-eval-cov-{}",
        uuid::Uuid::new_v4().simple()
    ));
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let lines = ["def f(x):", "    return x * 2"];
    std::fs::write(dir.join("f.py"), lines.join("\n") + "\n").ok();
    let hash = ledger::window_hash("f.py", &lines, 1);
    let mut change = ledger::Entry {
        id: "c".into(),
        session_id: "s".into(),
        part_id: "p".into(),
        kind: "change".into(),
        refers_to: None,
        path: "f.py".into(),
        hunk: "-0,0 +2,1".into(),
        line_hashes: vec![hash],
        concepts: vec![],
        tag_confidence: 0.9,
        label: "claimed".into(),
        rationale: serde_json::Value::Null,
        evidence: serde_json::Value::Null,
        review: String::new(),
        at: 0,
        prev_hash: ledger::GENESIS.into(),
        hash: String::new(),
    };
    change.hash = change.compute_hash();
    let mut review = change.clone();
    review.id = "r".into();
    review.kind = "review".into();
    review.refers_to = Some("c".into());
    review.review = "approved".into();
    review.line_hashes = vec![];
    review.prev_hash = change.hash.clone();
    review.hash = review.compute_hash();
    let c = ledger::coverage(&dir, &[change, review]);
    std::fs::remove_dir_all(&dir).ok();
    c.total.get(&ledger::Bucket::Vouched).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_parse_and_the_record_drills_hold() {
        let f: Fixtures = serde_json::from_str(FIXTURES).unwrap();
        assert!(!f.leak.is_empty() && !f.discrimination.is_empty());
        assert!(!f.checks.is_empty() && !f.tagging.is_empty());
        for c in &f.checks {
            let lines: Vec<&str> = c.file.lines().collect();
            let start = c.new_start - 1;
            assert_eq!(
                lines[start..start + c.added.len()],
                c.added.iter().map(String::as_str).collect::<Vec<_>>()[..],
                "{}: `added` must match the file",
                c.id
            );
        }
        assert!(claimed_never_vouched());
        let store = Store::open_in_memory().unwrap();
        assert!(eval_coverage(&store).unwrap().ok);
        assert!(eval_evidence(&store).unwrap().ok);
    }
}
