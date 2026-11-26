//! Tests of a turn, end to end, with scripted models and decision services.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;
use crate::engine::{BoxFuture, noop_sink};

struct FakeEngine {
    replies: Mutex<VecDeque<(ToolAccess, EngineTier, String)>>,
    prompts: Mutex<Vec<String>>,
}

impl FakeEngine {
    fn new(items: Vec<(ToolAccess, EngineTier, &str)>) -> Self {
        Self {
            replies: Mutex::new(
                items
                    .into_iter()
                    .map(|(t, tier, s)| (t, tier, s.to_string()))
                    .collect(),
            ),
            prompts: Mutex::new(Vec::new()),
        }
    }
}

impl Engine for FakeEngine {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        _sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let (tools, tier, reply) = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected engine call");
            assert_eq!(req.tools, tools, "tool access mismatch");
            assert_eq!(req.tier, tier, "engine tier mismatch");
            self.prompts.lock().unwrap().push(req.prompt.clone());
            // Stream raw text in small chunks, as the CLIs do.
            if req.stream_text {
                let chars: Vec<char> = reply.chars().collect();
                for chunk in chars.chunks(3) {
                    _sink(crate::engine::StreamEvent::TextDelta(
                        chunk.iter().collect(),
                    ));
                }
            }
            Ok(reply)
        })
    }
}

use EngineTier::{System1 as S1, System2 as S2};

/// Two models; the fake engine ignores their routes.
fn test_config() -> Config {
    let entry = |id: &str| crate::config::ModelEntry {
        id: id.into(),
        label: id.into(),
        route: crate::config::Route::Anthropic,
        model: id.into(),
        effort: None,
        endpoint: None,
    };
    Config {
        models: vec![entry("fast"), entry("main")],
        discover: false,
        db_path: std::env::temp_dir()
            .join("harness-test")
            .join("harness.sqlite"),
        ..Config::default()
    }
}

fn test_models() -> TierModels {
    let config = test_config();
    TierModels {
        system1: config.pick(None, false).unwrap(),
        system2: config.pick(None, true).unwrap(),
    }
}

#[test]
fn set_model_stores_pick_and_rejects_unknown_ids() {
    let mut rt = Runtime::new(
        Arc::new(Store::open_in_memory().unwrap()),
        Arc::new(FakeEngine::new(vec![])),
    );
    rt.config = test_config;
    rt.create_session("s".into(), PathBuf::from(".")).unwrap();
    assert!(rt.set_model("s", true, "nope").is_err());
    let s = rt.set_model("s", false, "main").unwrap();
    assert_eq!(s.system1.as_deref(), Some("main"));
    let choices = rt.model_choices(&s);
    assert_eq!(choices.system1.as_deref(), Some("main"));
    assert_eq!(choices.system2.as_deref(), Some("main"));
    assert_eq!(choices.catalog.len(), 2);
}
use ToolAccess::{Full, ReadOnly};

const ONBOARD_READY: &str =
    r#"{"reply":"Got it.","ready_to_map":true,"goal":"Add a /status command"}"#;
const MAP: &str = r#"{"brief":"Two parts.","parts":[
    {"id":"route","title":"Command routing","note":"Where commands dispatch","depends_on":[]},
    {"id":"report","title":"What status reports","depends_on":["route"]}]}"#;

fn runtime(engine: FakeEngine, flawed: bool) -> (Runtime, Arc<FakeEngine>) {
    let engine = Arc::new(engine);
    let mut rt = Runtime::new(Arc::new(Store::open_in_memory().unwrap()), engine.clone());
    rt.flaw_coin = if flawed { || true } else { || false };
    rt.config = test_config;
    // Outside any git repository, so handoffs run in place.
    let dir = std::env::temp_dir().join(format!("harness-rt-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    rt.create_session("s".into(), dir).unwrap();
    (rt, engine)
}

async fn turn(rt: &Runtime, text: &str) -> TurnOutput {
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
}

fn status(rt: &Runtime, id: &str) -> PartStatus {
    rt.load_session("s")
        .unwrap()
        .unwrap()
        .map
        .get(id)
        .unwrap()
        .status
}

#[tokio::test]
async fn understood_part_is_handed_off_reviewed_objected_and_approved() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"reply":"Right, acp.rs lists them.","map_change":null}"#,
            ),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Add it only in runtime.rs.","key_reason":"Zed never sees it unless acp.rs advertises it."}"#,
            ),
            (
                ReadOnly,
                S2,
                r#"{"pass":true,"feedback":"Yes: it must be advertised in acp.rs."}"#,
            ),
            (Full, S2, "Added /status."),
            (Full, S2, "Advertised it too."),
        ]),
        true,
    );

    let out = turn(&rt, "add a /status command").await;
    assert!(out.reply.contains("Command routing"));
    // The map lives in Zed's plan panel; replies only introduce the part.
    assert!(!out.reply.contains("Map:"));
    assert!(!out.reply.contains("What status reports"));
    assert_eq!(
        rt.load_session("s").unwrap().unwrap().active.as_deref(),
        Some("route")
    );

    // §5: summary before any log is refused.
    assert!(turn(&rt, "/summary it routes").await.reply.contains("log"));
    // §7: handoff before understanding is refused.
    assert!(turn(&rt, "/handoff").await.reply.contains("/delegate"));

    turn(
        &rt,
        "commands are advertised in acp.rs and dispatched in runtime.rs",
    )
    .await;
    let cand = turn(&rt, "/summary two places: advertise and dispatch").await;
    assert!(cand.reply.contains("Is it right?"));
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(!s.pending_candidate.as_ref().unwrap().key_correct);

    turn(&rt, "Wrong: acp.rs must advertise it or Zed won't show it").await;
    assert_eq!(status(&rt, "route"), PartStatus::Understood);

    turn(&rt, "/handoff").await;
    assert_eq!(status(&rt, "route"), PartStatus::InReview);
    turn(&rt, "/object it isn't advertised").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().objections.len(), 1);
    assert_eq!(
        s.map.get("route").unwrap().result.as_deref(),
        Some("Advertised it too.")
    );

    let out = turn(&rt, "/approve").await;
    assert_eq!(status(&rt, "route"), PartStatus::Done);
    assert!(out.reply.contains("What status reports"));
    assert!(engine.replies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn non_json_answer_is_reformatted_on_same_tier_before_escalating() {
    let store = Store::open_in_memory().unwrap();
    let engine = FakeEngine::new(vec![
        // S1 reads files, then answers in prose instead of JSON.
        (
            ReadOnly,
            S1,
            "Commands live in `acp.rs`. What should `/status` show?",
        ),
        // Same tier, no tools: re-wraps the same content as JSON.
        (
            ToolAccess::None,
            S1,
            r#"{"reply":"Commands live in `acp.rs`. What should `/status` show?","ready_to_map":false}"#,
        ),
    ]);
    let req = EngineRequest {
        tier: S1,
        tools: ReadOnly,
        cwd: PathBuf::from("."),
        system_prompt: String::new(),
        prompt: "onboard".into(),
        files: vec![],
        stream_text: false,
        models: test_models(),
        ctx: crate::engine::turn_ctx(),
    };
    let d: OnboardingDecision = run_gated(
        &engine,
        &store,
        "s",
        "onboarding",
        &req,
        noop_sink(),
        check_onboarding,
    )
    .await
    .unwrap();
    assert!(d.reply.starts_with("Commands live in"));
    assert!(engine.replies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn streamed_reply_is_shown_once_and_decoded() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![(
            ReadOnly,
            S1,
            r#"{"reply":"What do you want \"status\" to show?","ready_to_map":false}"#,
        )]),
        false,
    );
    let seen = Arc::new(Mutex::new(String::new()));
    let record = seen.clone();
    let sink: StreamSink = Arc::new(move |ev| {
        if let crate::engine::StreamEvent::TextDelta(t) = ev {
            record.lock().unwrap().push_str(&t);
        }
    });
    let out = rt
        .handle_turn(
            "s",
            TurnInput {
                text: "add /status".into(),
                files: vec![],
            },
            sink,
        )
        .await
        .unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        r#"What do you want "status" to show?"#
    );
    assert_eq!(out.reply, "");
}

/// Runs one onboarding turn whose first answer fails the JSON check and is
/// reformatted; returns everything the user saw.
async fn shown_after_repair(repaired_reply: &str) -> String {
    let repaired = format!(r#"{{"reply":"{repaired_reply}","ready_to_map":false}}"#);
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (
                ReadOnly,
                S1,
                r#"{"reply":"What should it show?"} {"ready_to_map":false}"#,
            ),
            (ToolAccess::None, S1, &repaired),
        ]),
        false,
    );
    let seen = Arc::new(Mutex::new(String::new()));
    let record = seen.clone();
    let sink: StreamSink = Arc::new(move |ev| {
        if let crate::engine::StreamEvent::TextDelta(t) = ev {
            record.lock().unwrap().push_str(&t);
        }
    });
    let input = TurnInput {
        text: "add /status".into(),
        files: vec![],
    };
    let out = rt.handle_turn("s", input, sink).await.unwrap();
    assert!(engine.replies.lock().unwrap().is_empty());
    let shown = seen.lock().unwrap().clone();
    shown + &out.reply
}

#[tokio::test]
async fn repeated_reply_after_retry_is_shown_once() {
    assert_eq!(
        shown_after_repair("What should it show?").await,
        "What should it show?"
    );
    // Onboarding replies are held until the decision is final, so only
    // the repaired reply is shown.
    assert_eq!(
        shown_after_repair("Which status fields?").await,
        "Which status fields?"
    );
}

#[tokio::test]
async fn failed_judgment_keeps_part_open() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, r#"{"reply":"Ok.","map_change":null}"#),
            (ReadOnly, S2, r#"{"candidate":"c","key_reason":"r"}"#),
            (
                ReadOnly,
                S2,
                r#"{"pass":false,"feedback":"It was correct."}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary s").await;
    let out = turn(&rt, "looks wrong").await;
    assert!(out.reply.contains("explain the part again"));
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Open);
    assert!(s.pending_candidate.is_none());
}

const MAP_CONCEPTS: &str = r#"{"brief":"Two parts.","concepts":[
    {"id":"dispatch","label":"Dispatch by name","needs":["closures"]},
    {"id":"handlers","label":"Handler functions","needs":[]},
    {"id":"registry","label":"Command registry","needs":[]},
    {"id":"closures","label":"Closures","needs":[]},
    {"id":"status-fields","label":"Status fields","needs":["dispatch"]}],
    "parts":[
    {"id":"route","title":"Command routing","depends_on":[],"concepts":["dispatch","handlers","registry"]},
    {"id":"report","title":"What status reports","depends_on":["route"],"concepts":["status-fields"]}]}"#;

#[tokio::test]
async fn an_exam_asks_grades_and_goes_on_until_stopped() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":true,"goal":"Abfragen: Routing","exam":true,"language":"de"}"#,
            ),
            (ReadOnly, S2, MAP_CONCEPTS),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Warum schlägt der Router Namen nach, statt sie zu probieren?","key_reason":"Nachschlagen ist O(1); Probieren hängt von der Reihenfolge ab."}"#,
            ),
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":true,"pass":false,"feedback":"Nicht ganz: es geht um Nachschlagen in O(1)."}"#,
            ),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Was zeigt der Statusbericht an?","key_reason":"Die Statusfelder."}"#,
            ),
            // "genug" isn't an answer: the exam ends and chat goes on.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"reply":"Gern, bis später.","ready_to_map":false,"question":true,"language":"de"}"#,
            ),
            (ReadOnly, S2, "Gern, bis später."),
        ]),
        false,
    );
    let out = turn(&rt, "frag mich wie ein Prüfer zum Routing ab").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.exam && s.pending_candidate.is_some());
    assert!(out.reply.contains("Deine Antwort?"), "{}", out.reply);
    let shown: Vec<&str> = s.history.iter().map(|t| t.content.as_str()).collect();
    assert!(
        shown.iter().any(|t| t.starts_with("Prüfungssimulation")),
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|t| t.starts_with("Prüfungsfrage zu **")),
        "{shown:?}"
    );

    turn(&rt, "Weil es schneller ist.").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.exam, "the exam goes on");
    assert_eq!(
        s.pending_candidate.as_ref().unwrap().candidate,
        "Was zeigt der Statusbericht an?"
    );
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(
        prompt.contains("Already asked") && prompt.contains("Warum schlägt der Router"),
        "the next question knows the last one"
    );
    let ws = crate::understanding::workspace_key(&s.cwd);
    let ids: Vec<String> = ["dispatch", "handlers", "registry"]
        .map(String::from)
        .to_vec();
    let events = rt.store.events(&ws, &ids).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == crate::understanding::EventKind::Fail),
        "a wrong answer is on the record"
    );

    turn(&rt, "genug").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(!s.exam && s.pending_candidate.is_none());
    assert_eq!(s.mode, SessionMode::Onboarding);
}

#[tokio::test]
async fn an_ignored_quick_check_is_dropped_without_a_record() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":true,"goal":"Check routing","check":true,"language":"en"}"#,
            ),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S2, r#"{"candidate":"c","key_reason":"r"}"#),
            // Not a verdict: the check is dropped, the question answered.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":false,"question":true,"language":"en"}"#,
            ),
            (ReadOnly, S2, "A DAG has no cycles."),
        ]),
        false,
    );
    turn(&rt, "got it").await;
    turn(&rt, "unrelated: what is a DAG?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.mode, SessionMode::Onboarding);
    assert!(s.pending_candidate.is_none() && !s.quick_check);
    assert_eq!(s.history.last().unwrap().content, "A DAG has no cycles.");
    let ws = crate::understanding::workspace_key(&s.cwd);
    let ids: Vec<String> = ["dispatch", "handlers", "registry"]
        .map(String::from)
        .to_vec();
    let events = rt.store.events(&ws, &ids).unwrap();
    assert!(
        !events.iter().any(|e| matches!(
            e.kind,
            crate::understanding::EventKind::Pass | crate::understanding::EventKind::Fail
        )),
        "no verdict, no evidence"
    );
}

#[tokio::test]
async fn a_claim_of_understanding_gets_one_candidate_then_chat_goes_on() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":true,"goal":"Prüfen, wie das Routing funktioniert","check":true,"language":"de"}"#,
            ),
            (ReadOnly, S2, MAP_CONCEPTS),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Namen werden der Reihe nach probiert.","key_reason":"Es wird nachgeschlagen, nicht probiert."}"#,
            ),
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Richtig: nachgeschlagen."}"#,
            ),
            // Back in chat: the next question is answered.
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":false,"goal":null,"question":true,"offer_check":true,"language":"de"}"#,
            ),
            (ReadOnly, S2, "Ein Handler ist eine Funktion."),
        ]),
        false,
    );
    let out = turn(&rt, "ok, macht Sinn").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.quick_check);
    assert!(s.pending_candidate.is_some(), "a candidate at once");
    assert!(s.pending_change.is_none(), "no gap parts in a quick check");
    assert!(out.reply.contains("Stimmt das?"), "{}", out.reply);
    let shown: Vec<&str> = s.history.iter().map(|t| t.content.as_str()).collect();
    assert!(shown.contains(&"Dann kurz zum Festigen:"), "{shown:?}");
    assert!(
        !shown.iter().any(|t| t.contains("Denk hier laut")),
        "no part card or hints in a quick check"
    );

    turn(&rt, "Falsch, es wird nachgeschlagen.").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.mode, SessionMode::Onboarding, "back to chat");
    assert!(s.map.parts.is_empty() && s.goal.is_none() && !s.quick_check);
    assert_eq!(
        s.history.last().unwrap().content,
        "Richtig: nachgeschlagen."
    );
    let ws = crate::understanding::workspace_key(&s.cwd);
    let events = rt
        .store
        .events(&ws, &["dispatch".to_string(), "handlers".to_string()])
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == crate::understanding::EventKind::Pass),
        "the pass is on the record"
    );

    turn(&rt, "Was ist ein Handler?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        "Ein Handler ist eine Funktion."
    );
}

#[tokio::test]
async fn understanding_comes_from_evidence_per_concept() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"Ok.","map_change":null,"concepts":["dispatch","Dispatch by name"],"misconception":null}"#,
            ),
            (ReadOnly, S2, r#"{"candidate":"c1","key_reason":"r"}"#),
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
            ),
            (ReadOnly, S2, r#"{"candidate":"c2","key_reason":"r"}"#),
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":true,"pass":false,"feedback":"No.","misconception":"The registry is a global singleton."}"#,
            ),
            (ReadOnly, S2, r#"{"candidate":"c3","key_reason":"r"}"#),
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Caught it."}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let route = s.map.get("route").unwrap();
    assert_eq!(route.concepts, vec!["dispatch", "handlers", "registry"]);
    // Closures sit under dispatch and no part covers them: a gap.
    let gap = s
        .pending_change
        .clone()
        .expect("a gap is proposed as a part");
    assert_eq!(gap.parts[0].concepts, vec!["closures".to_string()]);
    assert!(s.gaps_raised.contains(&"closures".to_string()));
    turn(&rt, "/reject").await;

    turn(&rt, "dispatch looks the name up").await;
    assert!(
        rt.why(&rt.load_session("s").unwrap().unwrap())
            .contains("unknown")
    );

    // Two concepts with no evidence go first; a pass covers only those.
    turn(&rt, "/summary it routes by name to a handler").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.pending_candidate.as_ref().unwrap().targets,
        vec!["dispatch", "handlers"]
    );
    let out = turn(&rt, "right, the name picks the handler").await;
    assert!(!out.reply.contains("understood"));
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Open);
    assert_eq!(
        s.pending_candidate.as_ref().unwrap().targets,
        vec!["registry"]
    );

    // A failed verdict that shows a wrong belief opens a misconception;
    // the next candidate is flawed on purpose and embodies it.
    turn(&rt, "it's right").await;
    turn(&rt, "/summary the registry maps names to handlers").await;
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(prompt.contains("The registry is a global singleton."));
    assert!(prompt.contains("FLAWED"));
    let s = rt.load_session("s").unwrap().unwrap();
    let pending = s.pending_candidate.as_ref().unwrap();
    assert!(!pending.key_correct);
    assert!(pending.misconception.is_some());

    turn(&rt, "wrong: it assumes one global registry").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Understood);
    let why = rt.why(&s);
    assert!(!why.contains("Open misconception"), "{why}");
    assert!(why.contains("Command registry<br/><small>shown"), "{why}");
    assert!(
        why.contains("n_closures --> n_dispatch"),
        "prerequisites one level down"
    );
}

#[tokio::test]
async fn a_claim_is_recorded_but_never_counts_as_evidence() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    turn(&rt, "/reject").await;
    turn(&rt, "/known").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Known);
    assert_eq!(s.active.as_deref(), Some("report"));
    let views = rt.concept_views(&s, &["dispatch".to_string()]).unwrap();
    assert_eq!(
        crate::understanding::state_of(&views, "dispatch"),
        crate::understanding::ConceptState::Claimed
    );
}

/// Answers the gate's questions with scripted p(yes); an `Err` entry
/// makes the check fail. Every other question goes unanswered.
/// The gate's scores in order, and the reader's (right, flawed,
/// neither) answers in order; an empty reader queue says "neither".
struct GateDecider(Mutex<VecDeque<Option<f64>>>, Mutex<VecDeque<[f64; 3]>>);

impl Decide for GateDecider {
    fn decide<'a>(
        &'a self,
        input: &'a str,
        questions: &'a [Question],
        _prefer: Option<&'a str>,
    ) -> futures_util::future::BoxFuture<'a, Result<Option<Decision>>> {
        if questions[0].name == "verdict" {
            let [right, flawed, neither] = self
                .1
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or([0.0, 0.0, 1.0]);
            return Box::pin(async move {
                let mut all = std::collections::HashMap::new();
                all.insert("right".to_string(), right);
                all.insert("flawed".to_string(), flawed);
                all.insert("neither".to_string(), neither);
                let mut answers = std::collections::HashMap::new();
                answers.insert(
                    "verdict".to_string(),
                    crate::decide::Answer {
                        choice: "neither".into(),
                        probability: neither,
                        all,
                    },
                );
                Ok(Some(Decision {
                    by: "Fake".into(),
                    answers,
                }))
            });
        }
        if !questions[0].name.starts_with('s') || questions[0].name == "step" {
            return Box::pin(async { Ok(None) });
        }
        assert!(input.contains("Draft reply:"));
        let next = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected gate call");
        Box::pin(async move {
            let p = next.ok_or_else(|| anyhow!("service down"))?;
            let mut answers = std::collections::HashMap::new();
            answers.insert(
                "s0".to_string(),
                crate::decide::Answer {
                    choice: "yes".into(),
                    probability: p,
                    ..Default::default()
                },
            );
            Ok(Some(Decision {
                by: "Fake".into(),
                answers,
            }))
        })
    }
}

fn log_reply(reply: &str) -> String {
    format!(r#"{{"intent":"log","reply":"{reply}","map_change":null}}"#)
}

#[tokio::test]
async fn a_question_is_answered_without_a_task() {
    const QUESTION: &str =
        r#"{"reply":"","ready_to_map":false,"goal":null,"question":true,"offer_check":true}"#;
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, QUESTION),
            (ReadOnly, S2, "A phasor is a complex number for a sinusoid."),
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":false,"goal":null,"question":true,"offer_check":false}"#,
            ),
            (ReadOnly, S2, "Lima."),
            // "check me" is a task: onboarding sets the goal and maps it.
            (
                ReadOnly,
                S1,
                r#"{"reply":"Understand phasors.","ready_to_map":true,"goal":"Understand phasors"}"#,
            ),
            (ReadOnly, S2, MAP),
        ]),
        false,
    );
    let session = rt.load_session("s").unwrap().unwrap();
    let ws = crate::understanding::workspace_key(&session.cwd);
    rt.store
        .upsert_concept(
            &ws,
            &crate::understanding::Concept {
                id: "phasors".into(),
                label: "Phasors".into(),
                aliases: vec![],
                created_by: "map".into(),
            },
        )
        .unwrap();

    turn(&rt, "Can you explain phasors?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.mode, SessionMode::Onboarding, "no task was started");
    assert!(s.goal.is_none());
    let last = &s.history.last().unwrap().content;
    assert!(last.starts_with("A phasor is a complex number"));
    assert!(
        last.contains("Told, not shown: Phasors is on your record"),
        "{last}"
    );
    let events = rt.store.events(&ws, &["phasors".to_string()]).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, crate::understanding::EventKind::Told);
    let views = rt.concept_views(&s, &["phasors".to_string()]).unwrap();
    assert_eq!(
        crate::understanding::state_of(&views, "phasors"),
        crate::understanding::ConceptState::Unknown,
        "being told is not understanding"
    );

    let record = turn(&rt, "/why").await.reply;
    assert!(record.contains("```mermaid"), "{record}");
    assert!(
        record.contains("Phasors<br/><small>told only</small>"),
        "{record}"
    );

    turn(&rt, "What's the capital of Peru?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        "Lima.",
        "no offer on a lookup"
    );
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(
        prompt.contains("Can you explain phasors?"),
        "answers see the conversation"
    );

    turn(&rt, "check me on phasors").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.goal.as_deref(), Some("Understand phasors"));
    assert_eq!(s.mode, SessionMode::Working);
}

#[tokio::test]
async fn a_review_check_allows_fewer_bits_than_a_candidate() {
    let (rt, _) = runtime(FakeEngine::new(vec![]), false);
    // 0.17 bits toward the truth: q = 0.5 · 2^0.17 ≈ 0.5626.
    let lean = 0.1252;
    let rt = rt.with_decider(Arc::new(GateDecider(
        Mutex::new(vec![Some(0.3), Some(0.3)].into()),
        Mutex::new(vec![[lean, 0.0, 1.0 - lean], [0.0, lean, 1.0 - lean]].into()),
    )));
    let session = rt.load_session("s").unwrap().unwrap();
    let candidate = crate::secrets::Secret::candidate_key("c", true, "why", 0);
    let mut planted = crate::secrets::Secret::candidate_key("c", false, "the flaw", 0);
    planted.kind = "planted_flaw".into();

    let c = rt
        .check(&session, &[candidate], "a reply")
        .await
        .unwrap()
        .unwrap();
    let bits = c.bits[0].unwrap();
    assert!((bits - 0.17).abs() < 0.01, "{bits}");
    assert!(!c.blocked, "a learning candidate allows 0.2 bits");

    let c = rt
        .check(&session, &[planted], "a reply")
        .await
        .unwrap()
        .unwrap();
    assert!(c.blocked, "a review check allows 0.14 bits");
}

#[tokio::test]
async fn the_gate_holds_replies_while_you_judge() {
    let leak = log_reply("It is wrong: dispatch never falls back.");
    let general = log_reply("Dispatch maps a name to a handler.");
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("Ok.")),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Unknown commands fall back to help.","key_reason":"The router has no fallback for unknown names, they are rejected"}"#,
            ),
            // Blocked, rewritten, passes.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (ReadOnly, S1, &leak),
            (
                ToolAccess::None,
                S1,
                r#"{"reply":"Think about what happens to a name with no handler."}"#,
            ),
            // Blocked twice: the fixed reply.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (ReadOnly, S1, &leak),
            (ToolAccess::None, S1, r#"{"reply":"Still: it is wrong."}"#),
            // The check errors: withheld.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (ReadOnly, S1, &general),
            // The pre-check catches an echo of the key with no decision.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (
                ReadOnly,
                S1,
                &log_reply("Note the router has no fallback for unknown names, so..."),
            ),
            (
                ToolAccess::None,
                S1,
                r#"{"reply":"Look at how unknown names are handled."}"#,
            ),
            // In the band, the reader learns nothing: shown as drafted.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (ReadOnly, S1, &general),
            // In the band, the reader learns it's right: rewritten.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (ReadOnly, S1, &general),
            (
                ToolAccess::None,
                S1,
                r#"{"reply":"A handler is the function a name maps to."}"#,
            ),
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right: there is no fallback."}"#,
            ),
            // No secret any more: replies stream unchecked.
            (ReadOnly, S1, &general),
        ]),
        false,
    );
    let rt = rt.with_decider(Arc::new(GateDecider(
        Mutex::new(
            vec![
                Some(0.9),
                Some(0.1),
                Some(0.9),
                Some(0.8),
                None,
                Some(0.05),
                Some(0.25),
                Some(0.3),
                Some(0.1),
            ]
            .into(),
        ),
        // In the band, the reader decides: it sees nothing, then a reply
        // that tells it the (correct) candidate is right.
        Mutex::new(vec![[0.0, 0.0, 1.0], [0.9, 0.0, 0.1]].into()),
    )));
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let raw = serde_json::to_string(&s).unwrap();
    assert!(
        !raw.contains("no fallback"),
        "the key stays out of the thread's state"
    );
    let id = s
        .pending_candidate
        .as_ref()
        .unwrap()
        .secret
        .clone()
        .unwrap();
    assert!(rt.store.secret(&id).unwrap().unwrap().active);

    let out = turn(&rt, "does it fall back?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        "Think about what happens to a name with no handler."
    );
    assert!(out.reply.is_empty());

    turn(&rt, "so is it wrong?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        crate::secrets::FIXED_REPLY
    );

    // The check fails: the fixed reply, and why it was held back.
    turn(&rt, "what is dispatch?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let held = &s.history.last().unwrap().content;
    assert!(held.starts_with(crate::secrets::FIXED_REPLY), "{held}");
    assert!(held.contains("couldn't be reached"), "{held}");

    turn(&rt, "hmm").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        "Look at how unknown names are handled."
    );

    turn(&rt, "what is a handler?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        "Dispatch maps a name to a handler."
    );
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(prompt.contains(gate::NARROW), "asked to answer narrowly");

    turn(&rt, "and what is a route?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        "A handler is the function a name maps to."
    );

    turn(&rt, "it's wrong, unknown names are rejected").await;
    assert!(!rt.store.secret(&id).unwrap().unwrap().active);
    turn(&rt, "/next").await;
    turn(&rt, "what is dispatch?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        "Dispatch maps a name to a handler."
    );
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(!prompt.contains(gate::NARROW));
}

#[tokio::test]
async fn chat_drives_the_loop() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"Ok.","map_change":null}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"intent":"summary","reply":"","map_change":null}"#,
            ),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"w = 0.5 so z = 99","key_reason":"r"}"#,
            ),
            // A question while the candidate waits: not a verdict.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"It means x.","map_change":null}"#,
            ),
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"intent":"move","part":null,"reply":"","map_change":null}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"intent":"known","reply":"","map_change":null}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "I get it: it routes by name").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_candidate.is_some());
    let part = s.map.get("route").unwrap();
    assert_eq!(part.log, vec!["thinking".to_string()]);
    assert_eq!(part.summary.as_deref(), Some("I get it: it routes by name"));

    let out = turn(&rt, "what does dispatch mean here?").await;
    assert!(!out.reply.contains("understood"));
    // The chat reply never sees the waiting candidate.
    let log_prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(log_prompt.contains("what does dispatch mean here?"));
    assert!(!log_prompt.contains("z = 99"));
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_candidate.is_some());

    turn(&rt, "it's correct because names map to handlers").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Understood);

    turn(&rt, "ok, next part").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let next = s.active.clone().unwrap();
    assert_ne!(next, "route");

    turn(&rt, "I already know this one").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get(&next).unwrap().status, PartStatus::Known);
}

/// Scripted decision answers: (step, probability, part).
struct FakeDecider(Mutex<VecDeque<(&'static str, f64, Option<&'static str>)>>);

impl Decide for FakeDecider {
    fn decide<'a>(
        &'a self,
        _input: &'a str,
        questions: &'a [Question],
        _prefer: Option<&'a str>,
    ) -> futures_util::future::BoxFuture<'a, Result<Option<Decision>>> {
        // Only the router's step question is scripted here.
        if questions[0].name != "step" {
            return Box::pin(async { Ok(None) });
        }
        let next = self.0.lock().unwrap().pop_front();
        let offered: Vec<String> = questions[0].options.iter().map(|o| o.0.clone()).collect();
        Box::pin(async move {
            let (step, probability, part) = next.expect("unexpected decision call");
            assert!(
                offered.iter().any(|o| o == step),
                "{step} not offered: {offered:?}"
            );
            let mut answers = std::collections::HashMap::new();
            answers.insert(
                "step".to_string(),
                crate::decide::Answer {
                    choice: step.into(),
                    probability,
                    ..Default::default()
                },
            );
            if let Some(part) = part {
                answers.insert(
                    "part".to_string(),
                    crate::decide::Answer {
                        choice: part.into(),
                        probability: 0.9,
                        ..Default::default()
                    },
                );
            }
            Ok(Some(Decision {
                by: "Fake".into(),
                answers,
            }))
        })
    }
}

fn with_decisions(rt: Runtime, steps: Vec<(&'static str, f64, Option<&'static str>)>) -> Runtime {
    rt.with_decider(Arc::new(FakeDecider(Mutex::new(steps.into()))))
}

#[test]
fn toolbar_options_persist_per_thread() {
    let (rt, _) = runtime(FakeEngine::new(vec![]), false);
    let s = rt
        .set_option("s", "effort", OptionValue::Id("high".into()))
        .unwrap();
    assert_eq!(
        rt.models(&s).unwrap().system2.effort.as_deref(),
        Some("high")
    );
    assert_eq!(rt.model_choices(&s).effort, "high");
    assert!(
        rt.set_option("s", "effort", OptionValue::Id("max".into()))
            .is_err()
    );
    let s = rt
        .set_option("s", "effort", OptionValue::Id("default".into()))
        .unwrap();
    assert_eq!(s.effort, None);
    let s = rt
        .set_option("s", "fast_effort", OptionValue::Id("medium".into()))
        .unwrap();
    assert_eq!(
        rt.models(&s).unwrap().system1.effort.as_deref(),
        Some("medium")
    );
    assert_eq!(rt.models(&s).unwrap().system2.effort, None);

    let s = rt
        .set_option("s", "decisions", OptionValue::Bool(false))
        .unwrap();
    assert!(s.decisions_off);
    let s = rt
        .set_option("s", "decisions", OptionValue::Id("on".into()))
        .unwrap();
    assert!(!s.decisions_off);
    let s = rt
        .set_option("s", "decision_model", OptionValue::Id("openai".into()))
        .unwrap();
    assert_eq!(s.decision_model.as_deref(), Some("openai"));
    assert!(
        rt.set_option("s", "decision_model", OptionValue::Id("x".into()))
            .is_err()
    );
}

#[tokio::test]
async fn decisions_off_skips_the_decision_model() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"Ok.","map_change":null}"#,
            ),
        ]),
        false,
    );
    // An empty script: any decision call would panic.
    let rt = with_decisions(rt, vec![]);
    rt.set_option("s", "decisions", OptionValue::Bool(false))
        .unwrap();
    turn(&rt, "goal").await;
    assert_eq!(turn(&rt, "thinking").await.reply, "");
}

#[tokio::test]
async fn a_skip_can_go_to_a_named_part() {
    const MAP3: &str = r#"{"brief":"Three parts.","parts":[
        {"id":"a","title":"A","depends_on":[]},
        {"id":"b","title":"B","depends_on":[]},
        {"id":"c","title":"C","depends_on":[]}]}"#;
    let (rt, _) = runtime(
        FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP3)]),
        false,
    );
    // The second skip names no part: the next unresolved one follows.
    let rt = with_decisions(rt, vec![("known", 0.95, Some("c")), ("known", 0.9, None)]);
    turn(&rt, "goal").await;
    let out = turn(&rt, "I know this one already, get to c").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("a").unwrap().status, PartStatus::Known);
    assert_eq!(s.active.as_deref(), Some("c"));
    assert!(out.reply.contains("**C**"));
    turn(&rt, "know this too").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("c").unwrap().status, PartStatus::Known);
    assert_eq!(s.active.as_deref(), Some("b"));
}

#[tokio::test]
async fn decided_steps_skip_the_llm() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP)]),
        false,
    );
    let rt = with_decisions(
        rt,
        vec![("known", 0.95, None), ("move", 0.9, Some("route"))],
    );
    turn(&rt, "goal").await;
    turn(&rt, "I already know routing").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Known);
    assert_eq!(s.active.as_deref(), Some("report"));
    turn(&rt, "go back to routing").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.active.as_deref(), Some("route"));
}

#[tokio::test]
async fn only_a_clear_move_drops_a_waiting_candidate() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"Ok.","map_change":null}"#,
            ),
            (ReadOnly, S2, r#"{"candidate":"z = 99","key_reason":"r"}"#),
            // An unsure move goes to the judge, which hands it back to chat.
            (
                ReadOnly,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"That's what you're judging.","map_change":null}"#,
            ),
        ]),
        false,
    );
    let rt = with_decisions(
        rt,
        vec![
            ("log", 0.9, None),
            ("move", 0.8, None),
            ("move", 0.95, None),
        ],
    );
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    turn(&rt, "is it right? I just want to move on").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_candidate.is_some(), "the candidate keeps waiting");
    assert_eq!(s.active.as_deref(), Some("route"));
    assert_eq!(
        s.history.last().unwrap().content,
        "That's what you're judging."
    );

    turn(&rt, "skip it, next part").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_candidate.is_none());
    assert_eq!(s.active.as_deref(), Some("report"));
}

#[tokio::test]
async fn a_question_about_a_waiting_candidate_skips_the_judge() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"Ok.","map_change":null}"#,
            ),
            (ReadOnly, S2, r#"{"candidate":"z = 99","key_reason":"r"}"#),
            // Decided as a log entry: only the reply LLM runs, no judge.
            (
                ReadOnly,
                S1,
                r#"{"intent":"summary","reply":"It means x.","map_change":null}"#,
            ),
            (ReadOnly, S2, r#"{"pass":true,"feedback":"Right."}"#),
        ]),
        false,
    );
    let rt = with_decisions(
        rt,
        vec![
            ("log", 0.9, None),
            ("summary", 0.8, None),
            ("log", 0.95, None),
            ("verdict", 0.9, None),
        ],
    );
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "I get it: names map to handlers").await;
    let out = turn(&rt, "what does dispatch mean?").await;
    // The LLM's own intent is ignored once the decision model has spoken.
    assert!(!out.reply.contains("Candidate"));
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(prompt.contains("Fake classified this message as a log entry"));
    assert!(!prompt.contains("z = 99"));
    assert!(
        rt.load_session("s")
            .unwrap()
            .unwrap()
            .pending_candidate
            .is_some()
    );
    turn(&rt, "right, names map to handlers").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Understood);
}

#[tokio::test]
async fn an_unsure_decision_leaves_it_to_the_llm() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"intent":"known","reply":"","map_change":null}"#,
            ),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("summary", 0.4, None)]);
    turn(&rt, "goal").await;
    turn(&rt, "hmm, I sort of know this").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Known);
}

#[tokio::test]
async fn a_summary_needs_a_log_first() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"intent":"summary","reply":"","map_change":null}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    let out = turn(&rt, "I get it: it routes by name").await;
    assert!(out.reply.contains("Noted in your log"));
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_candidate.is_none());
    assert_eq!(s.map.get("route").unwrap().log.len(), 1);
}

#[tokio::test]
async fn two_kinds_of_skip() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (Full, S2, "Wrote the report."),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    turn(&rt, "/known").await;
    assert_eq!(status(&rt, "route"), PartStatus::Known);
    let out = turn(&rt, "/delegate").await;
    assert!(out.reply.contains("without understanding"));
    assert_eq!(status(&rt, "report"), PartStatus::Delegated);
    // No review for delegated parts.
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.mode, SessionMode::Done);
    assert!(engine.replies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn map_change_waits_for_your_ok() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"reply":"That needs the store.","map_change":{"brief":"Status reads the store","parts":[{"id":"store","title":"Session store","depends_on":[]}],"remove":[]}}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    let out = turn(&rt, "status should show saved sessions").await;
    assert!(out.reply.contains("/accept"));
    assert!(
        rt.load_session("s")
            .unwrap()
            .unwrap()
            .map
            .get("store")
            .is_none()
    );

    turn(&rt, "/accept").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.map.get("store").is_some());
    assert!(s.pending_change.is_none());
}

#[tokio::test]
async fn accepted_change_can_reset_the_goal() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"reply":"That changes the task.","map_change":{"brief":"You need conv layers","goal":"Understand convolutional networks for an image classifier","parts":[{"id":"conv","title":"Conv layers","depends_on":[]}],"remove":[]}}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    let before = rt.load_session("s").unwrap().unwrap().goal;
    let out = turn(&rt, "I really need this for a CNN").await;
    assert!(out.reply.contains("Goal → Understand convolutional"));
    assert_eq!(rt.load_session("s").unwrap().unwrap().goal, before);

    turn(&rt, "/accept").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.goal.as_deref(),
        Some("Understand convolutional networks for an image classifier")
    );
}
