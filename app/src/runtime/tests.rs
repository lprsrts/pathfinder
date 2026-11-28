//! Tests of a turn, end to end, with scripted models and decision services.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;
use crate::engine::{BoxFuture, noop_sink};

struct FakeEngine {
    replies: Mutex<VecDeque<(ToolAccess, EngineTier, String)>>,
    prompts: Mutex<Vec<String>>,
    systems: Mutex<Vec<String>>,
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
            systems: Mutex::new(Vec::new()),
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
            self.systems.lock().unwrap().push(req.system_prompt.clone());
            if reply == ENGINE_DOWN {
                return Err(anyhow!("the model is unreachable"));
            }
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

/// A scripted reply that makes the engine call fail.
const ENGINE_DOWN: &str = "!engine down";

/// The source search finds nothing.
const NO_HITS: &str = r#"{"hits":[],"search_terms":[]}"#;

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
                ToolAccess::None,
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
    // The map lives in the side panel; replies only introduce the part and
    // its horizon (§16): what it leads to, not the map.
    assert!(!out.reply.contains("Map:"));
    assert!(!out.reply.contains("`report`"));
    assert!(
        out.reply
            .contains("Horizon: leads to **What status reports**."),
        "{}",
        out.reply
    );
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

    // A verdict in plain words is shown back and judged only on your OK.
    let shown = turn(&rt, "Wrong: acp.rs must advertise it or Zed won't show it").await;
    assert!(
        shown.reply.starts_with("Judge this as your verdict"),
        "{}",
        shown.reply
    );
    assert_eq!(shown.shown, Shown::Notice);
    assert_eq!(status(&rt, "route"), PartStatus::Open);
    turn(&rt, "/confirm").await;
    assert_eq!(status(&rt, "route"), PartStatus::Understood);

    turn(&rt, "/handoff").await;
    assert_eq!(status(&rt, "route"), PartStatus::InReview);
    // The handed-off part is in the record with its §12 label.
    let why = turn(&rt, "/why").await.reply;
    assert!(
        why.contains("**Handed to the agent**\n- **Command routing**: reviewable,")
            && why.contains("waiting for your review."),
        "{why}"
    );
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
    // On the next part, the approved one stays in the record.
    let why = turn(&rt, "/why").await.reply;
    assert!(why.contains("- **Command routing**: reviewable,"), "{why}");
    assert!(why.contains("you approved it."), "{why}");
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
                ToolAccess::None,
                S2,
                r#"{"pass":false,"feedback":"It was correct."}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary s").await;
    turn(&rt, "looks wrong").await;
    let out = turn(&rt, "/confirm").await;
    assert!(out.reply.contains("explain it again"));
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
                ToolAccess::None,
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
                ToolAccess::None,
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
    // The lead goes with the first question, never alone as a heading.
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("Prüfungssimulation") && t.contains("Prüfungsfrage zu **")),
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
                ToolAccess::None,
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
    // Shown back as a possible verdict; judged only on your Enter, and the
    // judge finds it isn't one.
    assert!(session(&rt).pending_commitment.is_some());
    turn(&rt, "/confirm").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.mode, SessionMode::Onboarding);
    assert!(s.pending_candidate.is_none() && !s.quick_check);
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .starts_with("**Not found**")
    );
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
                ToolAccess::None,
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
    // The lead goes with the candidate, never alone as a heading.
    assert!(!shown.contains(&"Kurze Prüfung:"), "{shown:?}");
    assert!(
        shown
            .iter()
            .any(|t| t.starts_with("Kurze Prüfung:\n\n") && t.contains("Stimmt das?")),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|t| t.contains("Denk hier laut")),
        "no part card or hints in a quick check"
    );

    // Your verdict, read from plain words, is shown back first (§15).
    let shown = turn(&rt, "Falsch, es wird nachgeschlagen.").await;
    assert_eq!(shown.shown, Shown::Notice, "{}", shown.reply);
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.pending_commitment.as_ref().map(|p| p.kind),
        Some(CommitKind::Verdict)
    );
    assert!(
        s.quick_check && s.pending_candidate.is_some(),
        "not judged yet"
    );
    turn(&rt, "/confirm").await;
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
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .starts_with("**Nicht gefunden**"),
        "a question gets sources in the conversation's language, never the model's answer"
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
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
            ),
            (ReadOnly, S2, r#"{"candidate":"c2","key_reason":"r"}"#),
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":false,"feedback":"No.","misconception":"The registry is a global singleton."}"#,
            ),
            (ReadOnly, S2, r#"{"candidate":"c3","key_reason":"r"}"#),
            (
                ToolAccess::None,
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
    turn(&rt, "right, the name picks the handler").await;
    let out = turn(&rt, "/confirm").await;
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
    turn(&rt, "/confirm").await;
    turn(&rt, "/summary the registry maps names to handlers").await;
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(prompt.contains("The registry is a global singleton."));
    assert!(prompt.contains("FLAWED"));
    let s = rt.load_session("s").unwrap().unwrap();
    let pending = s.pending_candidate.as_ref().unwrap();
    assert!(!pending.key_correct);
    assert!(pending.misconception.is_some());

    turn(&rt, "wrong: it assumes one global registry").await;
    turn(&rt, "/confirm").await;
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
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            // `/tell` skips onboarding: the one reasoned answer.
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
        .upsert_workspace_concept(
            &ws,
            &crate::understanding::Concept {
                id: "phasors".into(),
                label: "Phasors".into(),
                aliases: vec![],
                domain: String::new(),
                created_by: "map".into(),
            },
        )
        .unwrap();

    turn(&rt, "/tell Can you explain phasors?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.mode, SessionMode::Onboarding, "no task was started");
    assert!(s.goal.is_none());
    let last = &s.history.last().unwrap().content;
    assert!(last.starts_with("**Told**"), "{last}");
    assert!(last.contains("A phasor is a complex number"));
    assert!(last.contains("Recorded as told: Phasors"), "{last}");
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
    // A plain question gets sources only: the model's prose ("Lima.") is
    // thrown away and nothing in the workspace says it.
    let last = &s.history.last().unwrap().content;
    assert!(last.starts_with("**Not found**"), "{last}");
    assert!(!last.contains("Lima"));
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(
        prompt.contains("capital of Peru"),
        "the question is searched"
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
    let cite = |quote: &str| {
        format!(r#"{{"hits":[{{"file":"router.md","page":null,"quote":"{quote}"}}]}}"#)
    };
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Unknown commands fall back to help.","key_reason":"The router has no fallback for unknown names, they are rejected"}"#,
            ),
            // A source line that settles the candidate: held by the pre-check.
            (ReadOnly, S1, &log_reply("ok")),
            (
                ReadOnly,
                S2,
                &cite("The router has no fallback for unknown names, they are rejected."),
            ),
            // A general source line passes.
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, &cite("Dispatch maps a name to a handler.")),
            // The check errors: withheld.
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, &cite("Dispatch maps a name to a handler.")),
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right: there is no fallback."}"#,
            ),
            // No secret any more: no check.
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, &cite("Dispatch maps a name to a handler.")),
        ]),
        false,
    );
    let rt = rt.with_decider(Arc::new(GateDecider(
        Mutex::new(vec![Some(0.05), None].into()),
        Mutex::new(VecDeque::new()),
    )));
    let cwd = rt.load_session("s").unwrap().unwrap().cwd;
    std::fs::write(
        cwd.join("router.md"),
        "Dispatch maps a name to a handler.\nThe router has no fallback for unknown names, they are rejected.\n",
    )
    .unwrap();
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

    // With no step decided, a message while a candidate waits is shown
    // back as a possible verdict; Esc keeps it as a note.
    turn(&rt, "does it fall back?").await;
    turn(&rt, "/cancel").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let last = &s.history.last().unwrap().content;
    assert!(last.starts_with("Not judged as a verdict"), "{last}");
    assert!(last.ends_with(crate::secrets::FIXED_REPLY), "{last}");

    turn(&rt, "what is dispatch?").await;
    turn(&rt, "/cancel").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.history.last().unwrap().content.ends_with(
        "**Where to look:**\n\n- `router.md:1`\n  > Dispatch maps a name to a handler."
    ));

    // The check fails: the fixed reply, and why it was held back.
    turn(&rt, "what is a handler?").await;
    turn(&rt, "/cancel").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let held = &s.history.last().unwrap().content;
    assert!(held.contains(crate::secrets::FIXED_REPLY), "{held}");
    assert!(held.contains("Check unreachable"), "{held}");

    turn(&rt, "it's wrong, unknown names are rejected").await;
    turn(&rt, "/confirm").await;
    assert!(!rt.store.secret(&id).unwrap().unwrap().active);
    turn(&rt, "/next").await;
    turn(&rt, "what is dispatch?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .ends_with("> Dispatch maps a name to a handler.")
    );
    assert!(engine.replies.lock().unwrap().is_empty());
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
            // A question while the candidate waits, kept as a note (Esc).
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"It means x.","map_change":null}"#,
            ),
            (ReadOnly, S2, NO_HITS),
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"intent":"move","part":null,"reply":"","map_change":null}"#,
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

    let shown = turn(&rt, "what does dispatch mean here?").await;
    assert!(
        shown.reply.contains("“what does dispatch mean here?”"),
        "{}",
        shown.reply
    );
    let out = turn(&rt, "/cancel").await;
    assert!(!out.reply.contains("understood"));
    // The chat reply never sees the waiting candidate.
    let log_prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(log_prompt.contains("what does dispatch mean here?"));
    assert!(!log_prompt.contains("z = 99"));
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_candidate.is_some());

    turn(&rt, "it's correct because names map to handlers").await;
    turn(&rt, "/confirm").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Understood);

    turn(&rt, "ok, next part").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let next = s.active.clone().unwrap();
    assert_ne!(next, "route");

    // A bare claim the Tutor would read as a log entry is asked about
    // first (§15), then recorded as you pick it.
    let ask = turn(&rt, "I already know this one").await;
    assert!(
        ask.reply.starts_with("Which did you mean:"),
        "{}",
        ask.reply
    );
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get(&next).unwrap().status, PartStatus::Open);
    turn(&rt, "1").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(s.map.get(&next).unwrap().status, PartStatus::Known);
}

/// One scripted Router answer: the step and its probability, the part
/// it names, every option's probability (`all`, else the rest is split
/// evenly) and a steer (§16).
#[derive(Clone, Default)]
struct Scripted {
    step: &'static str,
    p: f64,
    part: Option<&'static str>,
    all: Vec<(&'static str, f64)>,
    steer: Option<&'static str>,
}

fn says(step: &'static str, p: f64) -> Scripted {
    Scripted {
        step,
        p,
        ..Default::default()
    }
}

/// Scripted decision answers, and the step options offered on each call.
struct FakeDecider(Mutex<VecDeque<Scripted>>, Mutex<Vec<Vec<String>>>);

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
        assert!(
            questions.iter().any(|q| q.name == "steer"),
            "the router always asks for a steer"
        );
        let next = self.0.lock().unwrap().pop_front();
        let offered: Vec<String> = questions[0].options.iter().map(|o| o.0.clone()).collect();
        self.1.lock().unwrap().push(offered.clone());
        Box::pin(async move {
            let script = next.expect("unexpected decision call");
            assert!(
                offered.iter().any(|o| o == script.step),
                "{} not offered: {offered:?}",
                script.step
            );
            let mut answers = std::collections::HashMap::new();
            answers.insert(
                "step".to_string(),
                crate::decide::Answer {
                    choice: script.step.into(),
                    probability: script.p,
                    all: script
                        .all
                        .iter()
                        .map(|(k, v)| (k.to_string(), *v))
                        .collect(),
                },
            );
            if let Some(part) = script.part {
                answers.insert(
                    "part".to_string(),
                    crate::decide::Answer {
                        choice: part.into(),
                        probability: 0.9,
                        ..Default::default()
                    },
                );
            }
            if let Some(steer) = script.steer {
                answers.insert(
                    "steer".to_string(),
                    crate::decide::Answer {
                        choice: steer.into(),
                        probability: 0.8,
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
    with_script(
        rt,
        steps
            .into_iter()
            .map(|(step, p, part)| Scripted {
                step,
                p,
                part,
                ..Default::default()
            })
            .collect(),
    )
    .0
}

fn with_script(rt: Runtime, script: Vec<Scripted>) -> (Runtime, Arc<FakeDecider>) {
    let decider = Arc::new(FakeDecider(
        Mutex::new(script.into()),
        Mutex::new(Vec::new()),
    ));
    (rt.with_decider(decider.clone()), decider)
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
            // An unsure move is shown back as a verdict; confirmed, the judge
            // sees it isn't one and hands it back to chat.
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":false,"pass":false,"feedback":""}"#,
            ),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"That's what you're judging.","map_change":null}"#,
            ),
            (ReadOnly, S2, NO_HITS),
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
    let shown = turn(&rt, "is it right? I just want to move on").await;
    assert!(shown.reply.starts_with("Judge this as your verdict"));
    turn(&rt, "/confirm").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_candidate.is_some(), "the candidate keeps waiting");
    assert_eq!(s.active.as_deref(), Some("route"));
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .starts_with("**Not found**"),
        "a question gets sources, never the model's reply"
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
            (ReadOnly, S2, NO_HITS),
            (ToolAccess::None, S2, r#"{"pass":true,"feedback":"Right."}"#),
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
    let prompts = engine.prompts.lock().unwrap().clone();
    let log_prompt = &prompts[prompts.len() - 2];
    assert!(log_prompt.contains("Fake classified this message as a log entry"));
    assert!(!log_prompt.contains("z = 99"));
    assert!(
        !prompts.last().unwrap().contains("z = 99"),
        "nor does the source search"
    );
    assert!(
        rt.load_session("s")
            .unwrap()
            .unwrap()
            .pending_candidate
            .is_some()
    );
    turn(&rt, "right, names map to handlers").await;
    turn(&rt, "/confirm").await;
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

const CANDIDATE_JSON: &str = r#"{"candidate":"Unknown commands fall back to help.","key_reason":"The router has no fallback for unknown names, they are rejected"}"#;

/// A log reply with a map change whose brief names the flaw.
const LEAKY_CHANGE: &str = r#"{"intent":"log","reply":"ok","map_change":{"brief":"Unknown names are rejected, not sent to help","parts":[{"id":"reject","title":"Rejecting unknown names","depends_on":[]}],"remove":[]}}"#;

#[tokio::test]
async fn a_tutor_map_change_passes_the_gate_with_the_reply() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, CANDIDATE_JSON),
            // Blocked: neither the reply nor the change is shown.
            (ReadOnly, S1, LEAKY_CHANGE),
            // Passed: both are.
            (ReadOnly, S1, LEAKY_CHANGE),
        ]),
        false,
    );
    let rt = rt.with_decider(Arc::new(GateDecider(
        Mutex::new(vec![Some(0.9), Some(0.05)].into()),
        Mutex::new(VecDeque::new()),
    )));
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    assert!(
        rt.load_session("s")
            .unwrap()
            .unwrap()
            .pending_candidate
            .is_some()
    );

    turn(&rt, "names map to handlers").await;
    let out = turn(&rt, "/cancel").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_change.is_none(), "a blocked change is dropped");
    let last = &s.history.last().unwrap().content;
    assert!(last.contains(crate::secrets::FIXED_REPLY), "{last}");
    for shown in [last.as_str(), out.reply.as_str()] {
        assert!(!shown.contains("rejected"), "{shown}");
        assert!(!shown.contains("Rejecting"), "{shown}");
    }

    turn(&rt, "handlers are looked up by name").await;
    turn(&rt, "/cancel").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let change = s.pending_change.as_ref().expect("a passed change waits");
    assert_eq!(change.parts[0].id, "reject");
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .contains("Rejecting unknown names")
    );
}

/// While a candidate waits, no prompt but the Judge's carries it or its
/// key: the Tutor's, the Mapper's on a replan, and `/tell`'s (A2).
#[tokio::test]
async fn no_prompt_but_the_judges_sees_a_waiting_candidate() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, CANDIDATE_JSON),
            // `/replan`: the Mapper.
            (
                ReadOnly,
                S2,
                r#"{"brief":"Add help","parts":[{"id":"help","title":"Help text","depends_on":[]}],"remove":[]}"#,
            ),
            // A note kept from a possible verdict: the Tutor.
            (ReadOnly, S1, &log_reply("ok")),
            // `/tell`.
            (ReadOnly, S2, "Dispatch maps a name to a handler."),
        ]),
        false,
    );
    let rt = rt.with_decider(Arc::new(GateDecider(
        Mutex::new(vec![Some(0.05), Some(0.05), Some(0.05)].into()),
        Mutex::new(VecDeque::new()),
    )));
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let candidate = s.pending_candidate.as_ref().unwrap().candidate.clone();
    assert!(
        s.history.iter().any(|t| t.content.contains(&candidate)),
        "the candidate is in the conversation"
    );
    let from = engine.prompts.lock().unwrap().len();

    turn(&rt, "/replan the router changed").await;
    turn(&rt, "names map to handlers").await;
    turn(&rt, "/cancel").await;
    turn(&rt, "/tell what is dispatch?").await;
    assert!(engine.replies.lock().unwrap().is_empty());
    let prompts = engine.prompts.lock().unwrap();
    assert_eq!(prompts.len() - from, 3);
    for p in &prompts[from..] {
        assert!(!p.contains(&candidate), "candidate in a prompt:\n{p}");
        assert!(!p.contains("no fallback"), "key in a prompt:\n{p}");
    }
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
    assert!(out.reply.contains("Say yes to add it, or no."));
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

#[tokio::test]
async fn check_reads_a_photo_of_handwritten_work() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (
                ToolAccess::None,
                S2,
                r#"{"lines": ["x² - 4 = 0", "(x - 2)(x + 2) = 0", "x = 2 or x = 4"]}"#,
            ),
            (ToolAccess::None, S2, r#"{"first_wrong": "x = 2 or x = 4"}"#),
        ]),
        false,
    );
    let cwd = rt.load_session("s").unwrap().unwrap().cwd;
    std::fs::write(cwd.join("work.jpg"), b"not really a jpeg").unwrap();
    let out = turn(&rt, "/check work.jpg").await;
    assert!(
        out.reply.starts_with("Read from `work.jpg`"),
        "{}",
        out.reply
    );
    assert!(out.reply.contains("  3  x = 2 or x = 4"));
    assert!(
        out.reply
            .contains("**First step that doesn't hold** (line 3)")
    );
    assert!(engine.replies.lock().unwrap().is_empty());
    // The check saw the copied text, not the image.
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(prompt.contains("(x - 2)(x + 2) = 0"));
}

#[tokio::test]
async fn tell_and_check_go_through_the_gate_while_you_judge() {
    const CANDIDATE: &str = "Unknown commands fall back to help.";
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Unknown commands fall back to help.","key_reason":"The router has no fallback for unknown names, they are rejected"}"#,
            ),
            // /tell settles it: held.
            (
                ReadOnly,
                S2,
                "It's wrong: names it doesn't know get an error.",
            ),
            // /check on the candidate's text: which step fails is the key.
            (
                ToolAccess::None,
                S2,
                r#"{"first_wrong": "Unknown commands fall back to help."}"#,
            ),
            // A /tell about something else passes.
            (ReadOnly, S2, "A handler is the function a name maps to."),
        ]),
        false,
    );
    let rt = rt.with_decider(Arc::new(GateDecider(
        Mutex::new(vec![Some(0.9), Some(0.9), Some(0.05)].into()),
        Mutex::new(VecDeque::new()),
    )));
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    assert!(
        rt.load_session("s")
            .unwrap()
            .unwrap()
            .pending_candidate
            .is_some()
    );

    turn(&rt, "/tell is the candidate right?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        crate::secrets::FIXED_REPLY
    );
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(
        !prompt.contains(CANDIDATE),
        "the candidate is hidden: {prompt}"
    );

    let out = turn(&rt, &format!("/check {CANDIDATE}")).await;
    assert_eq!(out.reply, crate::secrets::FIXED_REPLY);

    turn(&rt, "/tell what is a handler?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let last = &s.history.last().unwrap().content;
    assert!(
        last.contains("A handler is the function a name maps to."),
        "{last}"
    );
    assert!(engine.replies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn tell_is_held_while_you_judge_without_a_decision_service() {
    let (rt, _engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Unknown commands fall back to help.","key_reason":"The router has no fallback for unknown names, they are rejected"}"#,
            ),
            (
                ReadOnly,
                S2,
                "It's wrong: names it doesn't know get an error.",
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    let mut s = rt.load_session("s").unwrap().unwrap();
    assert!(s.pending_candidate.is_some());
    s.decisions_off = true;
    rt.store.save_session(&s).unwrap();
    turn(&rt, "/tell is the candidate right?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    assert_eq!(
        s.history.last().unwrap().content,
        crate::secrets::FIXED_REPLY
    );
}

static LATE_LISTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// [`test_config`] asking for a main model `late` that is listed only once
/// [`LATE_LISTED`] is set, as after a background refresh.
fn late_config() -> Config {
    let mut cfg = test_config();
    cfg.system2 = Some("late".into());
    if LATE_LISTED.load(std::sync::atomic::Ordering::SeqCst) {
        let mut late = cfg.models[1].clone();
        late.id = "late".into();
        cfg.models.push(late);
    }
    cfg
}

#[tokio::test]
async fn a_list_refreshed_mid_turn_applies_from_the_next_turn() {
    let (rt, _) = runtime(FakeEngine::new(vec![]), false);
    let rt = rt.with_config(late_config);
    let s = rt.load_session("s").unwrap().unwrap();
    let start = rt.config().catalog();
    let (first, second) = TURN_CATALOG
        .scope(start, async {
            let first = rt.models(&s).unwrap().system2.id;
            // The refresh lands between two calls of the same turn.
            LATE_LISTED.store(true, std::sync::atomic::Ordering::SeqCst);
            tokio::task::yield_now().await;
            (first, rt.models(&s).unwrap().system2.id)
        })
        .await;
    assert_eq!((first.as_str(), second.as_str()), ("main", "main"));
    // The next turn reads the list again.
    assert_eq!(rt.models(&s).unwrap().system2.id, "late");
}

/// Thinks, reads a file, then answers: what a CLI agent streams.
struct SteppingEngine;

impl Engine for SteppingEngine {
    fn run_text<'a>(
        &'a self,
        _req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            use crate::engine::StreamEvent as E;
            sink(E::Thought("Look at ".into()));
            sink(E::Thought("the gate.".into()));
            sink(E::ToolStart(crate::stream::ToolStart {
                id: "t1".into(),
                title: "Read gate.rs".into(),
                kind: crate::stream::ToolKind::Read,
                path: None,
            }));
            sink(E::ToolUpdate {
                id: "t1".into(),
                content: crate::stream::ToolContent::Text("a\nb".into()),
            });
            sink(E::ToolEnd {
                id: "t1".into(),
                ok: true,
            });
            Ok("The gate checks the draft.".into())
        })
    }
}

#[tokio::test]
async fn thoughts_and_tool_calls_are_kept_with_the_reply_they_came_before() {
    let engine = Arc::new(SteppingEngine);
    let mut rt = Runtime::new(Arc::new(Store::open_in_memory().unwrap()), engine);
    rt.config = test_config;
    let dir = std::env::temp_dir().join(format!("harness-rt-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    rt.create_session("s".into(), dir).unwrap();

    turn(&rt, "/tell What does the gate check?").await;
    let s = rt.load_session("s").unwrap().unwrap();
    let user = &s.history[0];
    assert_eq!(user.role, "user");
    assert!(user.steps.is_empty());
    let reply = s.history.iter().find(|t| t.role == "assistant").unwrap();
    assert_eq!(
        reply.steps,
        vec![
            crate::domain::Step::Thought {
                text: "Look at the gate.".into()
            },
            crate::domain::Step::Tool {
                id: "t1".into(),
                title: "Read gate.rs".into(),
                ok: Some(true),
                detail: "a\nb".into(),
            },
        ]
    );
}

// §15 and §16: commitments from plain words, and the next step.

use crate::domain::{CommitKind, Staging};
use crate::engine::StreamEvent;
use crate::understanding::{ConceptState, Event};

/// A thread on `MAP_CONCEPTS` with the gap proposal turned down: `route`
/// (dispatch, handlers, registry) is active and open.
async fn on_concepts(rt: &Runtime) {
    turn(rt, "goal").await;
    turn(rt, "/reject").await;
}

fn session(rt: &Runtime) -> Session {
    rt.load_session("s").unwrap().unwrap()
}

fn events_of(rt: &Runtime, concept: &str) -> Vec<Event> {
    let s = session(rt);
    let ws = crate::understanding::workspace_key(&s.cwd);
    rt.store.events(&ws, &[concept.to_string()]).unwrap()
}

fn state_of(rt: &Runtime, concept: &str) -> ConceptState {
    let s = session(rt);
    let views = rt.concept_views(&s, &[concept.to_string()]).unwrap();
    crate::understanding::state_of(&views, concept)
}

/// Opens misconception `m` on `concept` and returns a candidate on it:
/// built to embody it (`flawed`) or not (a correct candidate, an exam).
fn open_misconception(
    rt: &Runtime,
    concept: &str,
    m: &str,
    flawed: bool,
) -> crate::domain::PendingCandidate {
    let s = session(rt);
    rt.store
        .add_misconception(
            &Runtime::workspace(&s),
            &crate::understanding::Misconception {
                id: m.into(),
                concept: concept.into(),
                statement: "The registry is a global singleton.".into(),
                source: "log".into(),
            },
        )
        .unwrap();
    rt.append_events(
        &s,
        vec![
            crate::understanding::NewEvent::new(concept, EventKind::MisconceptionShown)
                .reference(m.to_string()),
        ],
    );
    crate::domain::PendingCandidate {
        part_id: "route".into(),
        candidate: "c".into(),
        key_correct: !flawed,
        key_reason: "r".into(),
        secret: None,
        targets: vec![concept.into()],
        misconception: Some(m.into()),
        id: "cand-1".into(),
    }
}

fn open_misconceptions(rt: &Runtime, concept: &str) -> Vec<String> {
    let s = session(rt);
    let views = rt.concept_views(&s, &[concept.to_string()]).unwrap();
    views
        .get(concept)
        .map(|v| v.open_misconceptions.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_correct_answer_passes_but_leaves_the_misconception_open() {
    // An exam answer (or a correct candidate) that names the belief is a
    // pass on the concept, never a resolution (§10, A5).
    let (rt, _) = runtime(FakeEngine::new(vec![]), false);
    let pending = open_misconception(&rt, "registry", "m-1", false);
    rt.record_verdict(&session(&rt), &pending, true, None);
    assert_eq!(open_misconceptions(&rt, "registry"), vec!["m-1"]);
    assert!(
        !events_of(&rt, "registry")
            .iter()
            .any(|e| e.kind == EventKind::MisconceptionResolved)
    );
    assert!(
        events_of(&rt, "registry")
            .iter()
            .any(|e| e.kind == EventKind::Pass)
    );
}

#[tokio::test]
async fn passing_the_flawed_candidate_resolves_the_misconception() {
    let (rt, _) = runtime(FakeEngine::new(vec![]), false);
    let pending = open_misconception(&rt, "registry", "m-1", true);
    // Failing it leaves the belief open.
    rt.record_verdict(&session(&rt), &pending, false, None);
    assert_eq!(open_misconceptions(&rt, "registry"), vec!["m-1"]);
    rt.record_verdict(&session(&rt), &pending, true, None);
    assert!(open_misconceptions(&rt, "registry").is_empty());
}

#[tokio::test]
async fn an_exam_question_never_aims_at_a_misconception() {
    // The exam's key is always "correct", so the question carries no flaw
    // to embody the belief, and its pass can't close it.
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"Ok.","map_change":null,"concepts":["registry"],"misconception":null}"#,
            ),
            (ReadOnly, S2, r#"{"candidate":"q1","key_reason":"r"}"#),
        ]),
        false,
    );
    on_concepts(&rt).await;
    turn(&rt, "the registry maps names").await;
    open_misconception(&rt, "registry", "m-1", true);
    let mut s = session(&rt);
    s.exam = true;
    rt.store.save_session(&s).unwrap();
    rt.write_candidate(&mut s, "route", &[], noop_sink())
        .await
        .unwrap();
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(!prompt.contains("global singleton"), "{prompt}");
    let pending = s.pending_candidate.clone().unwrap();
    assert!(pending.misconception.is_none());
    rt.record_verdict(&s, &pending, true, None);
    assert_eq!(open_misconceptions(&rt, "registry"), vec!["m-1"]);
}

fn set_status(rt: &Runtime, id: &str, status: PartStatus) {
    let mut s = session(rt);
    s.map.get_mut(id).unwrap().status = status;
    rt.store.save_session(&s).unwrap();
}

#[tokio::test]
async fn a_plain_claim_is_shown_back_and_undone() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("known", 0.8, None)]);
    on_concepts(&rt).await;
    let out = turn(&rt, "I already know routing").await;
    assert!(
        out.reply.starts_with(
            "Recorded: you already know **Command routing**. A claim, not evidence. Say undo, or press Esc, to take it back."
        ),
        "{}",
        out.reply
    );
    let s = session(&rt);
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Known);
    assert_eq!(s.active.as_deref(), Some("report"));
    assert!(s.undoable(), "undoable from the frontend, between turns");
    assert_eq!(state_of(&rt, "dispatch"), ConceptState::Claimed);

    let out = turn(&rt, "/undo").await;
    assert_eq!(out.shown, Shown::Notice);
    assert!(
        out.reply
            .starts_with("Undone: your claim on **Command routing** is withdrawn."),
        "{}",
        out.reply
    );
    let s = session(&rt);
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Open);
    assert_eq!(s.active.as_deref(), Some("route"));
    assert!(
        s.pending_change.is_none(),
        "a gap the claim raised goes with it"
    );
    assert_eq!(s.gaps_raised, vec!["closures".to_string()]);
    assert!(s.last_commitment.is_none());
    assert_eq!(state_of(&rt, "dispatch"), ConceptState::Unknown);
    // Compensating events: the claim stays on the log, withdrawn.
    let events = events_of(&rt, "dispatch");
    let claim = events.iter().find(|e| e.kind == EventKind::Claim).unwrap();
    assert!(events.iter().any(|e| e.kind == EventKind::Retracted
        && e.reference.as_deref() == Some(claim.id.to_string().as_str())));
    assert!(
        turn(&rt, "/undo")
            .await
            .reply
            .starts_with("Nothing to undo.")
    );

    // /why keeps it visible as withdrawn, and shows the step history.
    let why = turn(&rt, "/why").await.reply;
    assert!(
        why.contains("Claim on **Dispatch by name** withdrawn ("),
        "{why}"
    );
    assert!(
        why.contains("Of the ideas above, 0 passed a check and 0 rest on your word alone."),
        "{why}"
    );
    // A step no verdict followed says nothing yet: no history line.
    assert!(!why.contains("led to a pass so far"), "{why}");
    assert!(!why.contains("0 of 0"), "{why}");
    assert!(engine.replies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn undo_reaches_only_the_last_step() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("known", 0.8, None), ("log", 0.9, None)]);
    on_concepts(&rt).await;
    turn(&rt, "I already know routing").await;
    turn(&rt, "the report lists the fields").await;
    assert!(
        turn(&rt, "/undo")
            .await
            .reply
            .starts_with("Nothing to undo.")
    );
    assert_eq!(status(&rt, "route"), PartStatus::Known);
    assert_eq!(state_of(&rt, "dispatch"), ConceptState::Claimed);
}

#[tokio::test]
async fn a_failed_message_still_closes_the_undo_window() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S1, ENGINE_DOWN),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("known", 0.8, None), ("log", 0.9, None)]);
    on_concepts(&rt).await;
    turn(&rt, "I already know routing").await;
    let failed = rt
        .handle_turn(
            "s",
            TurnInput {
                text: "the report lists the fields".into(),
                files: vec![],
            },
            noop_sink(),
        )
        .await;
    assert!(failed.is_err());
    assert!(!session(&rt).undoable());
    assert!(
        turn(&rt, "/undo")
            .await
            .reply
            .starts_with("Nothing to undo.")
    );
    assert_eq!(status(&rt, "route"), PartStatus::Known);
}

#[tokio::test]
async fn a_plain_undo_is_read_by_the_router() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
        ]),
        false,
    );
    let (rt, decider) = with_script(rt, vec![says("known", 0.8), says("undo", 0.8)]);
    on_concepts(&rt).await;
    turn(&rt, "I already know routing").await;
    let out = turn(&rt, "wait, no, take that back").await;
    assert!(out.reply.starts_with("Undone:"), "{}", out.reply);
    assert_eq!(status(&rt, "route"), PartStatus::Open);
    let offered = decider.1.lock().unwrap();
    let has = |i: usize, o: &str| offered[i].iter().any(|x| x == o);
    // Agent and tell are always offered; undo only right after a claim,
    // confirm only while something waits.
    assert!(has(0, "agent") && has(0, "tell"));
    assert!(!has(0, "undo") && !has(0, "confirm"));
    assert!(has(1, "undo") && !has(1, "confirm"));
}

#[tokio::test]
async fn a_typed_known_is_not_shown_back() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
        ]),
        false,
    );
    on_concepts(&rt).await;
    let out = turn(&rt, "/known").await;
    assert!(
        out.reply
            .starts_with("**Command routing** recorded as already known."),
        "{}",
        out.reply
    );
    assert!(session(&rt).last_commitment.is_none());
    assert!(
        turn(&rt, "/undo")
            .await
            .reply
            .starts_with("Nothing to undo.")
    );
    assert_eq!(status(&rt, "route"), PartStatus::Known);
}

#[tokio::test]
async fn a_plain_verdict_waits_for_confirm() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, r#"{"candidate":"c","key_reason":"r"}"#),
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
            ),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("log", 0.9, None), ("verdict", 0.8, None)]);
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    let shown = turn(&rt, "it's right, names map to handlers").await;
    assert_eq!(
        shown.reply,
        "Judge this as your verdict on the candidate? “it's right, names map to handlers” Enter judges · Esc keeps it as a note."
    );
    assert_eq!(shown.shown, Shown::Notice);
    assert_eq!(engine.replies.lock().unwrap().len(), 1, "no judge yet");
    let s = session(&rt);
    assert!(s.pending_candidate.is_some());
    assert_eq!(
        s.pending_commitment.as_ref().map(|p| p.kind),
        Some(CommitKind::Verdict)
    );

    turn(&rt, "/confirm").await;
    assert!(engine.replies.lock().unwrap().is_empty());
    let s = session(&rt);
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Understood);
    assert!(s.pending_commitment.is_none() && s.pending_candidate.is_none());
    let said = s
        .history
        .iter()
        .filter(|t| t.content == "it's right, names map to handlers")
        .count();
    assert_eq!(said, 1, "the verdict is in the thread once");
    assert!(
        turn(&rt, "/confirm")
            .await
            .reply
            .starts_with("Nothing is waiting")
    );
}

#[tokio::test]
async fn cancel_keeps_a_verdict_as_a_note() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, r#"{"candidate":"c","key_reason":"r"}"#),
            // Esc: one Tutor call, as a log entry; no judge.
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("log", 0.9, None), ("verdict", 0.8, None)]);
    on_concepts(&rt).await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    turn(&rt, "I'd say the lookup part holds").await;
    turn(&rt, "/cancel").await;
    assert!(engine.replies.lock().unwrap().is_empty());
    let s = session(&rt);
    assert!(s.pending_candidate.is_some(), "the candidate still waits");
    assert!(s.pending_commitment.is_none());
    assert!(
        s.map
            .get("route")
            .unwrap()
            .log
            .contains(&"I'd say the lookup part holds".to_string())
    );
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .starts_with("Not judged as a verdict: kept as a note.")
    );
    let events = events_of(&rt, "dispatch");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::Pass | EventKind::Fail))
    );
}

#[tokio::test]
async fn undo_never_touches_a_judged_verdict() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, r#"{"candidate":"c","key_reason":"r"}"#),
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":false,"feedback":"It was correct."}"#,
            ),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("log", 0.9, None), ("verdict", 0.8, None)]);
    on_concepts(&rt).await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    turn(&rt, "it's wrong").await;
    turn(&rt, "/confirm").await;
    assert_eq!(state_of(&rt, "dispatch"), ConceptState::Shaky);
    assert!(
        turn(&rt, "/undo")
            .await
            .reply
            .starts_with("Nothing to undo.")
    );
    assert_eq!(state_of(&rt, "dispatch"), ConceptState::Shaky);
    let fails = events_of(&rt, "dispatch")
        .iter()
        .filter(|e| e.kind == EventKind::Fail)
        .count();
    assert_eq!(fails, 1);
    assert!(
        !events_of(&rt, "dispatch")
            .iter()
            .any(|e| e.kind == EventKind::Retracted)
    );
}

#[tokio::test]
async fn a_plain_handoff_runs_nothing_before_confirm() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (Full, S2, "Added /status."),
        ]),
        false,
    );
    let (rt, decider) = with_script(
        rt,
        vec![
            says("agent", 0.8),
            says("agent", 0.8),
            says("confirm", 0.95),
        ],
    );
    turn(&rt, "goal").await;
    set_status(&rt, "route", PartStatus::Understood);
    let out = turn(&rt, "let the agent do this part").await;
    assert_eq!(
        session(&rt).waiting_for_ok(),
        Some((CommitKind::Handoff, "Command routing"))
    );
    assert!(
        out.reply
            .starts_with("Hand **Command routing** to the agent? This isn't a git repository"),
        "{}",
        out.reply
    );
    assert_eq!(engine.replies.lock().unwrap().len(), 1, "nothing ran");
    let out = turn(&rt, "/cancel").await;
    assert_eq!(
        out.reply,
        "Not run: **Command routing** wasn't handed to the agent."
    );
    assert_eq!(status(&rt, "route"), PartStatus::Understood);
    assert!(session(&rt).pending_commitment.is_none());

    turn(&rt, "ok, have the agent do it").await;
    turn(&rt, "yes, go ahead").await;
    assert!(engine.replies.lock().unwrap().is_empty());
    assert_eq!(status(&rt, "route"), PartStatus::InReview);
    let offered = decider.1.lock().unwrap();
    assert!(offered[2].iter().any(|o| o == "confirm"));
}

#[tokio::test]
async fn a_plain_delegate_on_an_open_part() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (Full, S2, "Wrote it."),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("agent", 0.8, None)]);
    on_concepts(&rt).await;
    let out = turn(&rt, "just have the agent do routing").await;
    assert!(
        out.reply
            .starts_with("Delegate **Command routing** without understanding it?"),
        "{}",
        out.reply
    );
    assert_eq!(status(&rt, "route"), PartStatus::Open);
    turn(&rt, "/confirm").await;
    assert_eq!(status(&rt, "route"), PartStatus::Delegated);
    assert!(
        events_of(&rt, "dispatch")
            .iter()
            .any(|e| e.kind == EventKind::Delegated)
    );
    // On the next part, /why still shows the delegated work, labelled as
    // §12 labels it.
    assert_eq!(session(&rt).active.as_deref(), Some("report"));
    let why = turn(&rt, "/why").await.reply;
    assert!(
        why.contains(
            "**Handed to the agent**\n- **Command routing**: delegated, done without your understanding; you don't vouch for it."
        ),
        "{why}"
    );
}

#[tokio::test]
async fn agent_on_a_known_part_is_refused_at_once() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP)]),
        false,
    );
    let rt = with_decisions(rt, vec![("agent", 0.8, None)]);
    turn(&rt, "goal").await;
    set_status(&rt, "route", PartStatus::Known);
    let out = turn(&rt, "let the agent do it").await;
    assert_eq!(
        out.reply,
        "**Command routing** is already claimed as known."
    );
    assert!(session(&rt).pending_commitment.is_none());
}

#[tokio::test]
async fn a_plain_message_drops_a_pending_commitment() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("agent", 0.8, None), ("log", 0.95, None)]);
    turn(&rt, "goal").await;
    set_status(&rt, "route", PartStatus::Understood);
    turn(&rt, "let the agent do this").await;
    turn(&rt, "actually, let me think about the edge cases first").await;
    let s = session(&rt);
    assert!(s.pending_commitment.is_none());
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Understood);
    let last = &s.history.last().unwrap().content;
    assert!(
        last.starts_with("Not run: **Command routing** wasn't handed to the agent."),
        "{last}"
    );
    assert!(engine.replies.lock().unwrap().is_empty(), "no agent call");
}

#[tokio::test]
async fn a_plain_tell_is_held_and_recorded_as_told() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S2, "Names are looked up in a table."),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("tell", 0.8, None)]);
    on_concepts(&rt).await;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let record = seen.clone();
    let sink: StreamSink = Arc::new(move |ev| {
        if let StreamEvent::TextDelta(t) = ev {
            record.lock().unwrap().push(t);
        }
    });
    rt.handle_turn(
        "s",
        TurnInput {
            text: "just tell me how dispatch by name works".into(),
            files: vec![],
        },
        sink,
    )
    .await
    .unwrap();
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen[0].starts_with("Telling you, as you asked."),
        "{seen:?}"
    );
    // Held: the answer arrives whole, after the show-back.
    assert_eq!(seen[1], "**Told**\n\nNames are looked up in a table.");
    assert!(engine.replies.lock().unwrap().is_empty());
    assert!(
        events_of(&rt, "dispatch")
            .iter()
            .any(|e| e.kind == EventKind::Told)
    );
    assert_eq!(state_of(&rt, "dispatch"), ConceptState::Unknown);
    assert!(
        turn(&rt, "/undo")
            .await
            .reply
            .starts_with("Nothing to undo.")
    );
}

#[tokio::test]
async fn tell_is_gated_while_a_review_secret_is_active() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S2, "It falls back to help."),
            (
                ReadOnly,
                S1,
                r#"{"intent":"tell","reply":"","map_change":null}"#,
            ),
            (ReadOnly, S2, "It falls back to help."),
        ]),
        false,
    );
    let rt = rt.with_decider(Arc::new(GateDecider(
        Mutex::new(vec![Some(0.9), Some(0.9)].into()),
        Mutex::new(VecDeque::new()),
    )));
    turn(&rt, "goal").await;
    // A review in staging with a planted hunk: no candidate waits.
    let mut s = session(&rt);
    let secret = crate::secrets::Secret {
        id: "sec-plant".into(),
        kind: "planted_flaw".into(),
        payload: serde_json::json!({"candidate": "+ return HANDLERS.get(name, help)", "correct": false, "reason": "unknown names fall back"}),
        probe: "whether a hunk is planted, or where".into(),
        opened_at_turn: s.history.len(),
        active: true,
    };
    rt.store.add_secret("s", &secret).unwrap();
    let route = s.map.get_mut("route").unwrap();
    route.status = PartStatus::InReview;
    route.staging = Some(Staging {
        dir: s.cwd.join("staging"),
        base: String::new(),
        result: None,
        secret: Some("sec-plant".into()),
        report: String::new(),
        linked: vec![],
    });
    rt.store.save_session(&s).unwrap();
    assert!(s.pending_candidate.is_none());

    turn(&rt, "/tell does it fall back to help?").await;
    let s = session(&rt);
    assert_eq!(
        s.history.last().unwrap().content,
        crate::secrets::FIXED_REPLY
    );
    turn(&rt, "just tell me whether unknown names fall back").await;
    let s = session(&rt);
    let last = &s.history.last().unwrap().content;
    assert!(last.ends_with(crate::secrets::FIXED_REPLY), "{last}");
    assert!(!last.contains("falls back to help"), "{last}");
    assert!(engine.replies.lock().unwrap().is_empty());
    let ws = crate::understanding::workspace_key(&s.cwd);
    assert!(
        rt.store.concepts(&ws).unwrap().is_empty(),
        "nothing recorded as told"
    );
    assert!(rt.store.secret("sec-plant").unwrap().unwrap().active);
}

#[tokio::test]
async fn an_unsure_commitment_asks() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP)]),
        false,
    );
    let (rt, _) = with_script(
        rt,
        vec![Scripted {
            step: "known",
            p: 0.45,
            all: vec![("known", 0.45), ("log", 0.3), ("tell", 0.25)],
            ..Default::default()
        }],
    );
    turn(&rt, "goal").await;
    let out = turn(&rt, "routing, I sort of know that, or do I").await;
    assert_eq!(
        out.reply,
        "Which did you mean: (1) record that I already know **Command routing** (no check), (2) check me on **Command routing**, (3) just tell me or (4) just a note? Nothing is recorded until you say; a number or a few words is enough."
    );
    assert_eq!(out.shown, Shown::Notice);
    let s = session(&rt);
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Open);
    assert!(s.map.get("route").unwrap().log.is_empty());
    assert!(s.pending_commitment.is_none() && s.last_commitment.is_none());
    assert!(engine.replies.lock().unwrap().is_empty());
    let n = s.history.len();
    assert_eq!(
        s.history[n - 2].content,
        "routing, I sort of know that, or do I"
    );
}

#[tokio::test]
async fn an_unsure_non_commitment_falls_back_to_the_llm() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    let (rt, _) = with_script(
        rt,
        vec![Scripted {
            step: "log",
            p: 0.5,
            all: vec![
                ("log", 0.5),
                ("move", 0.3),
                ("known", 0.2),
                ("agent", 0.0),
                ("tell", 0.0),
            ],
            ..Default::default()
        }],
    );
    turn(&rt, "goal").await;
    let out = turn(&rt, "the router keeps a table").await;
    assert!(!out.reply.contains("did you mean"));
    assert!(engine.replies.lock().unwrap().is_empty());
    assert_eq!(
        session(&rt).map.get("route").unwrap().log,
        vec!["the router keeps a table".to_string()]
    );
}

#[tokio::test]
async fn the_llm_intent_known_is_shown_back() {
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
    rt.set_option("s", "decisions", OptionValue::Bool(false))
        .unwrap();
    turn(&rt, "goal").await;
    let out = turn(&rt, "I know routing already").await;
    assert!(
        out.reply
            .starts_with("Recorded: you already know **Command routing**."),
        "{}",
        out.reply
    );
    assert!(turn(&rt, "/undo").await.reply.starts_with("Undone:"));
    assert_eq!(status(&rt, "route"), PartStatus::Open);
}

#[tokio::test]
async fn fixed_lines_never_carry_the_key() {
    const CAND: &str = "CANDTEXT-7f";
    const KEY: &str = "KEYREASON-9c";
    let candidate = format!(r#"{{"candidate":"{CAND}","key_reason":"{KEY}"}}"#);
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, &candidate),
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    let rt = with_decisions(
        rt,
        vec![
            ("log", 0.9, None),
            ("verdict", 0.8, None),
            ("log", 0.95, None),
            ("agent", 0.95, None),
        ],
    );
    on_concepts(&rt).await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    let from = session(&rt).history.len();
    let mut shown = vec![
        turn(&rt, "I think it is wrong").await.reply,
        turn(&rt, "hmm, let me think about it").await.reply,
        turn(&rt, "let the agent do it").await.reply,
        turn(&rt, "/cancel").await.reply,
        turn(&rt, "/undo").await.reply,
        turn(&rt, "/confirm").await.reply,
        turn(&rt, "/why").await.reply,
    ];
    let mut s = session(&rt);
    assert!(
        rt.next_step(&mut s, None, None, false).is_none(),
        "no next step while a secret is active"
    );
    assert!(
        s.map
            .get("route")
            .unwrap()
            .log
            .contains(&"I think it is wrong".to_string()),
        "a verdict you went past stays a note, as the line says"
    );
    shown.extend(s.history[from..].iter().map(|t| t.content.clone()));
    for line in &shown {
        assert!(!line.contains(CAND) && !line.contains(KEY), "{line}");
    }
    assert!(
        shown
            .iter()
            .any(|l| l.starts_with("Judge this as your verdict"))
    );
    assert!(
        shown
            .iter()
            .any(|l| l.starts_with("Not judged as a verdict"))
    );
    assert!(
        shown
            .iter()
            .any(|l| l.starts_with("Delegate **Command routing**"))
    );
    assert!(!shown.iter().any(|l| l.contains("Next:")));
}

#[test]
fn confirm_is_counted_as_its_command() {
    use pathfinder_schema::Step;
    let s = Session::new("s".into(), PathBuf::from("."));
    assert_eq!(
        turn_step("/confirm", &s, false, Some(CommitKind::Handoff)),
        Step::Handoff
    );
    assert_eq!(
        turn_step("/confirm", &s, true, Some(CommitKind::Verdict)),
        Step::Verdict
    );
    assert_eq!(
        turn_step("/confirm", &s, false, Some(CommitKind::Delegate)),
        Step::Delegate
    );
    assert_eq!(turn_step("/confirm", &s, false, None), Step::Other);
    // Only a confirmation that ran sets what ran; these never do.
    assert_eq!(turn_step("/undo", &s, false, None), Step::Other);
    assert_eq!(turn_step("/cancel", &s, false, None), Step::Other);
}

#[tokio::test]
async fn judgment_is_saved_before_the_next_candidate() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S1, &log_reply("ok")),
            (ReadOnly, S2, r#"{"candidate":"c1","key_reason":"r"}"#),
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
            ),
            // Writing the next candidate fails.
            (ReadOnly, S2, ENGINE_DOWN),
        ]),
        false,
    );
    on_concepts(&rt).await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary it routes by name to a handler").await;
    turn(&rt, "right, the name picks the handler").await;
    let failed = rt
        .handle_turn(
            "s",
            TurnInput {
                text: "/confirm".into(),
                files: vec![],
            },
            noop_sink(),
        )
        .await;
    assert!(failed.is_err());
    let s = session(&rt);
    assert!(
        s.pending_candidate.is_none(),
        "the judged candidate isn't left waiting"
    );
    assert!(s.pending_commitment.is_none());
    let passes = events_of(&rt, "dispatch")
        .iter()
        .filter(|e| e.kind == EventKind::Pass)
        .count();
    assert_eq!(passes, 1);
    // Nothing can be judged (and passed) a second time.
    assert!(
        turn(&rt, "/confirm")
            .await
            .reply
            .starts_with("Nothing is waiting")
    );
}

#[tokio::test]
async fn entering_a_part_shows_the_next_step_and_horizon() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
        ]),
        false,
    );
    let cwd = session(&rt).cwd;
    let mut notes: Vec<String> = (1..=35).map(|i| format!("filler {i}")).collect();
    notes[29] = "Dispatch by name: the router looks the name up.".into();
    std::fs::write(cwd.join("notes.md"), notes.join("\n")).unwrap();
    let out = turn(&rt, "goal").await;
    assert!(
        out.reply.contains(
            "Next: `notes.md:20-35` · What role does “Dispatch by name” play in this part? Put it in your own words."
        ),
        "{}",
        out.reply
    );
    assert!(
        out.reply
            .contains("Horizon: leads to **What status reports**."),
        "{}",
        out.reply
    );
    let steps: Vec<Event> = events_of(&rt, "dispatch")
        .into_iter()
        .filter(|e| e.kind == EventKind::Step)
        .collect();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].detail["kind"], "wide-purpose");
    assert_eq!(steps[0].detail["steered"], false);
    assert_eq!(steps[0].reference.as_deref(), Some("route"));
    let last = session(&rt).last_step.unwrap();
    assert_eq!(
        (last.concept.as_str(), last.kind.as_str()),
        ("dispatch", "wide-purpose")
    );
    // A step is activity, never evidence.
    assert_eq!(state_of(&rt, "dispatch"), ConceptState::Unknown);
}

#[tokio::test]
async fn a_steer_changes_the_next_step_and_keeps_the_message() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    let (rt, _) = with_script(
        rt,
        vec![Scripted {
            step: "log",
            p: 0.8,
            steer: Some("example"),
            ..Default::default()
        }],
    );
    on_concepts(&rt).await;
    let text = "I think dispatch looks the name up; give me an example first";
    turn(&rt, text).await;
    let s = session(&rt);
    // Your words stay on the log, and the Tutor answered them.
    assert_eq!(s.map.get("route").unwrap().log, vec![text.to_string()]);
    assert!(engine.replies.lock().unwrap().is_empty(), "the Tutor ran");
    let reply = &s.history.last().unwrap().content;
    assert!(
        reply.ends_with(
            "Next: nothing in your sources names **Dispatch by name** yet; ask me where to look. · Find one place where “Dispatch by name” comes up. What happens there?"
        ),
        "{reply}"
    );
    let steered: Vec<Event> = events_of(&rt, "dispatch")
        .into_iter()
        .filter(|e| e.kind == EventKind::Step && e.detail["steered"] == true)
        .collect();
    assert_eq!(steered.len(), 1);
    assert_eq!(steered[0].detail["kind"], "wide-example");
    let why = turn(&rt, "/why").await.reply;
    assert!(why.contains("steered by you 1 times"), "{why}");
}

// Adversarial review (§15): a verdict held for your OK must be judged
// against the candidate it was shown back for, never a newer one.
#[tokio::test]
async fn a_pending_verdict_is_never_judged_against_a_newer_candidate() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"first candidate","key_reason":"r1"}"#,
            ),
            // A second `/summary` replaces the waiting candidate.
            (
                ReadOnly,
                S2,
                r#"{"candidate":"second candidate","key_reason":"r2"}"#,
            ),
            // Only reached if `/confirm` judges the old words against the
            // new candidate.
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
            ),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("log", 0.9, None), ("verdict", 0.8, None)]);
    turn(&rt, "goal").await;
    turn(&rt, "thinking").await;
    turn(&rt, "/summary names map to handlers").await;
    let shown = turn(&rt, "the first one is right").await;
    assert!(shown.reply.starts_with("Judge this as your verdict"));
    // A typed command replaces the candidate; the verdict still waits.
    turn(&rt, "/summary a second take").await;
    let s = session(&rt);
    assert_eq!(
        s.pending_candidate.as_ref().map(|c| c.candidate.as_str()),
        Some("second candidate")
    );
    // The typed command dropped the verdict, said so, and kept your words
    // as a note.
    assert!(s.pending_commitment.is_none());
    assert!(
        s.history.iter().any(|t| t.role == "assistant"
            && t.content
                .starts_with("Not judged as a verdict: kept as a note.")),
        "{:#?}",
        s.history
    );
    assert!(
        s.map
            .get("route")
            .unwrap()
            .log
            .contains(&"the first one is right".to_string())
    );
    // Even if it still waited, it would never be judged against the
    // newer candidate: confirming checks the candidate's id.
    let mut held = s.clone();
    held.pending_commitment = Some(crate::domain::PendingCommitment {
        kind: CommitKind::Verdict,
        part: "route".into(),
        text: "the first one is right".into(),
        files: vec![],
        candidate: Some("cand-first".into()),
        then: Vec::new(),
    });
    let stale = rt.confirm(&mut held, noop_sink()).await.unwrap();
    assert!(stale.reply.starts_with("Not run"), "{}", stale.reply);
    assert!(held.pending_candidate.is_some());
    let out = turn(&rt, "/confirm").await;
    let s = session(&rt);
    assert_eq!(
        s.map.get("route").unwrap().status,
        PartStatus::Open,
        "words about the first candidate were judged against the second: {}",
        out.reply
    );
    assert_eq!(
        engine.replies.lock().unwrap().len(),
        1,
        "the Judge must not be called"
    );
}

/// Streams the whole answer, then the turn ends without finishing (a
/// dropped connection, or Esc pressed after reading it).
struct StreamsThenStops;

impl Engine for StreamsThenStops {
    fn run_text<'a>(
        &'a self,
        req: &'a EngineRequest,
        sink: StreamSink,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            if req.stream_text {
                sink(crate::engine::StreamEvent::TextDelta(
                    "A closure captures its environment by reference or by move.".into(),
                ));
            }
            Err(anyhow!("stopped"))
        })
    }
}

// Adversarial review (§11, §15): an answer you have seen is recorded as
// told. A typed `/tell` with no secret active streams the answer, so a turn
// stopped after the text arrived records nothing.
#[tokio::test]
async fn a_streamed_tell_you_read_is_recorded_as_told() {
    let mut rt = Runtime::new(
        Arc::new(Store::open_in_memory().unwrap()),
        Arc::new(StreamsThenStops),
    );
    rt.config = test_config;
    let dir = std::env::temp_dir().join(format!("harness-rt-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    rt.create_session("s".into(), dir).unwrap();
    let seen = Arc::new(Mutex::new(String::new()));
    let sink: StreamSink = {
        let seen = seen.clone();
        Arc::new(move |e| {
            if let crate::engine::StreamEvent::TextDelta(t) = e {
                seen.lock().unwrap().push_str(&t);
            }
        })
    };
    let out = rt
        .handle_turn(
            "s",
            TurnInput {
                text: "/tell what does a closure capture".into(),
                files: vec![],
            },
            sink,
        )
        .await;
    assert!(out.is_err());
    assert!(
        seen.lock().unwrap().contains("captures its environment"),
        "the answer was shown"
    );
    let told = rt
        .store
        .event_counts("s")
        .unwrap()
        .into_iter()
        .filter(|((_, kind), _)| kind == "told")
        .count();
    assert!(told > 0, "an answer you read left no told mark");
}

// Adversarial review (telemetry): a `/confirm` that ran nothing is not a
// verdict, and a handoff confirmed by a plain "yes" is a handoff, not a log.
#[tokio::test]
async fn confirm_turns_are_counted_by_what_ran() {
    let (rt, _engine) = runtime(
        FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP)]),
        false,
    );
    turn(&rt, "goal").await;
    // Stale: the verdict waited, but no candidate does any more, so the
    // confirmation runs nothing and is counted as no step of the loop.
    let mut s = session(&rt);
    s.pending_commitment = Some(crate::domain::PendingCommitment {
        kind: CommitKind::Verdict,
        part: "route".into(),
        text: "it's right".into(),
        files: vec![],
        candidate: Some("cand-gone".into()),
        then: Vec::new(),
    });
    let out = rt.confirm(&mut s, noop_sink()).await.unwrap();
    assert!(out.reply.starts_with("Not run"), "{}", out.reply);
    assert_eq!(s.ran_commitment, None);
    assert_eq!(
        turn_step("/confirm", &s, false, s.ran_commitment),
        pathfinder_schema::Step::Other,
        "a /confirm that judged nothing is counted as a verdict"
    );
    // A plain "yes, do it" the Router read as confirm, running a handoff.
    assert_eq!(
        turn_step("yes, do it", &s, false, Some(CommitKind::Handoff)),
        pathfinder_schema::Step::Handoff,
        "a handoff confirmed in plain words is counted as a log turn"
    );
}

// A real session's deviations (§11, §15, §16, A3): a bare claim below the
// Router's threshold, a part named by its source, an explanation logged
// with no Tutor reply, commands taught in replies, and two pointers.

/// Two parts whose notes point into the workspace (`b.md`, `a.md`).
const MAP_NOTES: &str = r#"{"brief":"Two parts.","concepts":[
    {"id":"dispatch","label":"Dispatch by name","needs":[]},
    {"id":"status-fields","label":"Status fields","needs":[]}],
    "parts":[
    {"id":"route","title":"Command routing","note":"See b.md:38","depends_on":[],"concepts":["dispatch"]},
    {"id":"report","title":"What status reports","note":"a.md:1","depends_on":["route"],"concepts":["status-fields"]}]}"#;

/// `a.md` names the concept first; `b.md`, where the routing part points,
/// names it near line 38.
fn write_notes(rt: &Runtime) {
    let cwd = session(rt).cwd;
    std::fs::write(cwd.join("a.md"), "Dispatch by name, in passing.\nmore\n").unwrap();
    let mut b: Vec<String> = (1..=60).map(|i| format!("filler {i}")).collect();
    b[39] = "Dispatch by name: the router looks the name up.".into();
    std::fs::write(cwd.join("b.md"), b.join("\n")).unwrap();
}

#[tokio::test]
async fn a_bare_claim_below_the_threshold_is_asked_about_for_the_part_it_names() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP)]),
        false,
    );
    // The decision service read "log (47%)": below 0.6, no commitment weighed.
    let (rt, _) = with_script(rt, vec![says("log", 0.47)]);
    turn(&rt, "goal").await;
    let out = turn(&rt, "I understand the part on status reports").await;
    assert_eq!(out.shown, Shown::Notice);
    assert!(
        out.reply.starts_with(
            "Which did you mean: (1) record that I already know **What status reports** (no check), (2) check me on **What status reports** or (3) just a note?"
        ),
        "{}",
        out.reply
    );
    // No command taught, nothing recorded, no model called.
    assert!(!out.reply.contains("`/"), "{}", out.reply);
    let s = session(&rt);
    assert!(s.map.parts.iter().all(|p| p.status == PartStatus::Open));
    assert!(s.map.parts.iter().all(|p| p.log.is_empty()));
    assert!(s.last_commitment.is_none());
    assert!(engine.replies.lock().unwrap().is_empty());

    // "No check" picks the claim on the part the words named.
    let out = turn(&rt, "record it, no check").await;
    assert!(
        out.reply
            .starts_with("Recorded: you already know **What status reports**."),
        "{}",
        out.reply
    );
    assert_eq!(status(&rt, "report"), PartStatus::Known);
    assert_eq!(status(&rt, "route"), PartStatus::Open);
    assert!(session(&rt).pending_ask.is_none());
    // No reply in the whole flow teaches a command (§15).
    for t in session(&rt)
        .history
        .iter()
        .filter(|t| t.role == "assistant")
    {
        assert!(!t.content.contains("`/"), "{}", t.content);
    }
}

#[tokio::test]
async fn undoing_a_claim_on_a_named_part_takes_you_back() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP)]),
        false,
    );
    let (rt, _) = with_script(rt, vec![says("log", 0.47), says("undo", 0.95)]);
    turn(&rt, "goal").await;
    assert_eq!(session(&rt).active.as_deref(), Some("route"));
    turn(&rt, "I understand the part on status reports").await;
    turn(&rt, "1").await;
    assert_eq!(status(&rt, "report"), PartStatus::Known);
    let out = turn(&rt, "undo that").await;
    assert!(out.reply.contains("What status reports"), "{}", out.reply);
    let s = session(&rt);
    assert_eq!(s.map.get("report").unwrap().status, PartStatus::Open);
    // Back on the part you were working on, not the one you named.
    assert_eq!(s.active.as_deref(), Some("route"));
}

#[tokio::test]
async fn i_get_it_is_asked_about_even_when_the_router_says_log() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"It routes by a table of names.","key_reason":"r"}"#,
            ),
        ]),
        false,
    );
    let (rt, _) = with_script(rt, vec![says("log", 0.9)]);
    turn(&rt, "goal").await;
    let out = turn(&rt, "ok I get it").await;
    assert!(
        out.reply.starts_with("Which did you mean:"),
        "{}",
        out.reply
    );
    assert!(out.reply.contains("check me on **Command routing**"));
    // "Check me on it": your words start the log, and a candidate is written.
    turn(&rt, "check me").await;
    let s = session(&rt);
    assert!(s.pending_candidate.is_some());
    assert_eq!(
        s.map.get("route").unwrap().log,
        vec!["ok I get it".to_string()]
    );
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Open);
    assert!(engine.replies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn just_a_note_logs_the_message_once() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"ok","ack":"You say you understand routing.","question":"","map_change":null}"#,
            ),
        ]),
        false,
    );
    rt.set_option("s", "decisions", OptionValue::Bool(false))
        .unwrap();
    turn(&rt, "goal").await;
    let out = turn(&rt, "I understand this part").await;
    assert!(
        out.reply.starts_with("Which did you mean:"),
        "{}",
        out.reply
    );
    turn(&rt, "3").await;
    let s = session(&rt);
    let route = s.map.get("route").unwrap();
    assert_eq!(route.log, vec!["I understand this part".to_string()]);
    assert_eq!(route.status, PartStatus::Open);
    let users: Vec<&str> = s
        .history
        .iter()
        .filter(|t| t.role == "user")
        .map(|t| t.content.as_str())
        .collect();
    assert_eq!(
        users
            .iter()
            .filter(|u| **u == "I understand this part")
            .count(),
        1,
        "{users:?}"
    );
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .starts_with("You say you understand routing."),
        "{}",
        s.history.last().unwrap().content
    );
    assert!(engine.replies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn any_other_message_drops_the_question() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    let (rt, _) = with_script(rt, vec![says("log", 0.47), says("log", 0.9)]);
    turn(&rt, "goal").await;
    turn(&rt, "I know this").await;
    assert!(session(&rt).pending_ask.is_some());
    turn(&rt, "the router keeps a table of handlers").await;
    let s = session(&rt);
    assert!(s.pending_ask.is_none());
    assert_eq!(
        s.map.get("route").unwrap().log,
        vec!["the router keeps a table of handlers".to_string()]
    );
    assert_eq!(s.map.get("route").unwrap().status, PartStatus::Open);
}

#[tokio::test]
async fn a_doubt_or_a_no_never_records_a_claim() {
    for answer in [
        "I don't know",
        "don't record anything",
        "not sure, I don't know which",
        "weiß nicht",
        "skip",
    ] {
        let (rt, _) = runtime(
            FakeEngine::new(vec![
                (ReadOnly, S1, ONBOARD_READY),
                (ReadOnly, S2, MAP),
                (ReadOnly, S1, &log_reply("ok")),
            ]),
            false,
        );
        let (rt, _) = with_script(rt, vec![says("log", 0.47), says("log", 0.9)]);
        turn(&rt, "goal").await;
        let out = turn(&rt, "I understand this part").await;
        assert!(
            out.reply.starts_with("Which did you mean:"),
            "{}",
            out.reply
        );
        let out = turn(&rt, answer).await;
        assert!(!out.reply.contains("Recorded"), "{answer}: {}", out.reply);
        let s = session(&rt);
        assert!(s.pending_ask.is_none(), "{answer}");
        assert!(s.last_commitment.is_none(), "{answer}");
        assert!(
            s.map.parts.iter().all(|p| p.status == PartStatus::Open),
            "{answer}"
        );
    }
}

#[tokio::test]
async fn an_ack_that_adds_to_your_words_is_not_shown() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_NOTES),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"ok","ack":"You're right, and an unknown name falls through to the default handler.","question":"Good. Now where does it fail?","map_change":null}"#,
            ),
        ]),
        false,
    );
    write_notes(&rt);
    let (rt, _) = with_script(rt, vec![says("log", 0.9)]);
    turn(&rt, "goal").await;
    turn(&rt, "Explain how the router picks the handler").await;
    let reply = session(&rt).history.last().unwrap().content.clone();
    // New facts in the ack: the fixed line instead (§11). Two sentences in
    // the question: the fixed question instead.
    assert!(reply.starts_with("Logged.\n\nNext: `b.md:"), "{reply}");
    assert!(!reply.contains("default handler"), "{reply}");
    assert!(!reply.contains("Good. Now"), "{reply}");
    assert!(reply.ends_with("Put it in your own words."), "{reply}");
}

#[tokio::test]
async fn a_claim_on_a_source_with_two_parts_asks_which_part() {
    const MAP_ONE_FILE: &str = r#"{"brief":"Two parts.","parts":[
        {"id":"route","title":"Command routing","note":"notes.md:1","depends_on":[]},
        {"id":"report","title":"What status reports","note":"notes.md:3","depends_on":["route"]}]}"#;
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_ONE_FILE),
        ]),
        false,
    );
    std::fs::write(session(&rt).cwd.join("notes.md"), "a\nb\nc\nd\n").unwrap();
    let rt = with_decisions(rt, vec![("known", 0.9, None)]);
    turn(&rt, "goal").await;
    let out = turn(&rt, "I know what's in notes.md").await;
    assert_eq!(
        out.reply,
        "Which part do you mean: (1) **Command routing** or (2) **What status reports**? Nothing is recorded until you say."
    );
    assert!(
        session(&rt)
            .map
            .parts
            .iter()
            .all(|p| p.status == PartStatus::Open)
    );
    // One thing to do on the part you pick: it is done, and shown back.
    let out = turn(&rt, "2").await;
    assert!(
        out.reply
            .starts_with("Recorded: you already know **What status reports**."),
        "{}",
        out.reply
    );
    assert_eq!(status(&rt, "report"), PartStatus::Known);
    assert_eq!(status(&rt, "route"), PartStatus::Open);
}

#[tokio::test]
async fn an_explanation_gets_a_tutor_reply_and_one_next_step() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_NOTES),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"ok","ack":"You traced how the router finds the handler for a name.","question":"Where in b.md does the lookup fail for an unknown name, and what happens then?","map_change":null,"concepts":["dispatch"],"misconception":null}"#,
            ),
        ]),
        false,
    );
    write_notes(&rt);
    let (rt, _) = with_script(rt, vec![says("log", 0.7)]);
    let entered = turn(&rt, "goal").await;
    // One pointer: the concept's lines in the part's own file, not the
    // first file that names it, and no separate "Read:".
    assert!(!entered.reply.contains("Read:"), "{}", entered.reply);
    assert_eq!(
        entered.reply.matches("Next:").count(),
        1,
        "{}",
        entered.reply
    );
    assert!(
        entered.reply.contains("Next: `b.md:30-50` · "),
        "{}",
        entered.reply
    );
    assert!(!entered.reply.contains("a.md"), "{}", entered.reply);
    // Plain words, no commands taught (§15).
    assert!(!entered.reply.contains("`/"), "{}", entered.reply);
    assert!(entered.reply.contains("just say so"), "{}", entered.reply);

    let text = "The router reads the command name, looks it up in a table and calls the handler it finds there.";
    turn(&rt, text).await;
    let s = session(&rt);
    assert_eq!(s.map.get("route").unwrap().log, vec![text.to_string()]);
    let reply = &s.history.last().unwrap().content;
    assert!(!reply.contains("Logged."), "{reply}");
    assert!(
        reply
            .starts_with("You traced how the router finds the handler for a name.\n\nNext: `b.md:"),
        "{reply}"
    );
    assert!(
        reply.ends_with(
            " · Where in b.md does the lookup fail for an unknown name, and what happens then?"
        ),
        "{reply}"
    );
    assert_eq!(reply.matches("Next:").count(), 1, "{reply}");
    // The Extractor still ran on the log entry.
    assert!(
        events_of(&rt, "dispatch")
            .iter()
            .any(|e| e.kind == EventKind::Touched)
    );
    assert!(engine.replies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_log_entry_without_model_text_falls_back_to_fixed_lines() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_NOTES),
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    write_notes(&rt);
    let (rt, _) = with_script(rt, vec![says("log", 0.9)]);
    turn(&rt, "goal").await;
    turn(&rt, "the router keeps a table").await;
    let reply = session(&rt).history.last().unwrap().content.clone();
    // Every reply still ends with one exact next step, even one shown
    // before, with a grammatical question around the concept's name.
    assert!(reply.starts_with("Logged.\n\nNext: `b.md:"), "{reply}");
    assert!(
        reply.ends_with(
            "· What role does “Dispatch by name” play in this part? Put it in your own words."
        ),
        "{reply}"
    );
    // Shown again, recorded once.
    let steps = events_of(&rt, "dispatch")
        .into_iter()
        .filter(|e| e.kind == EventKind::Step)
        .count();
    assert_eq!(steps, 1);
}

/// An electricity bill in three parts, as in the demo: read the bill,
/// check the charges, write the letter. The charges' tariff needs the
/// energy used and VAT, which no part covers.
const MAP_BILL: &str = r#"{"brief":"Three parts.","concepts":[
    {"id":"kwh","label":"Energy used","needs":[]},
    {"id":"vat","label":"VAT","needs":[]},
    {"id":"tariff","label":"Tariff","needs":["kwh","vat"]}],
    "parts":[
    {"id":"bill","title":"Read the bill","depends_on":[],"concepts":["kwh"]},
    {"id":"check","title":"Check the charges","depends_on":["bill"],"concepts":["tariff"]},
    {"id":"letter","title":"Write the complaint letter","depends_on":["check"],"concepts":["tariff"]}]}"#;

/// A thread on `MAP_BILL`, on the first part, with the Router's steps
/// scripted.
async fn on_bill(
    script: Vec<Scripted>,
    engine: Vec<(ToolAccess, EngineTier, &'static str)>,
) -> (Runtime, Arc<FakeEngine>, Arc<FakeDecider>) {
    let mut items = vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, MAP_BILL)];
    items.extend(engine);
    let (rt, engine) = runtime(FakeEngine::new(items), false);
    let (rt, decider) = with_script(rt, script);
    turn(&rt, "goal").await;
    let s = session(&rt);
    assert_eq!(s.active.as_deref(), Some("bill"));
    assert!(s.pending_change.is_none(), "no gap under the first part");
    (rt, engine, decider)
}

/// Passes on `concepts`: understanding shown.
fn shown(rt: &Runtime, concepts: &[&str]) {
    let s = session(rt);
    rt.append_events(
        &s,
        concepts
            .iter()
            .map(|c| crate::understanding::NewEvent::new(c, EventKind::Pass))
            .collect(),
    );
}

#[tokio::test]
async fn the_agent_gets_the_part_named_and_a_handoff_once_its_concepts_are_shown() {
    let (rt, engine, _) = on_bill(
        vec![says("agent", 0.8), says("confirm", 0.95)],
        vec![(Full, S2, "Wrote letter.md.")],
    )
    .await;
    shown(&rt, &["kwh", "tariff"]);
    let out = turn(&rt, "I get the bill now. let the agent write the letter").await;
    assert!(
        out.reply
            .starts_with("Hand **Write the complaint letter** to the agent?"),
        "{}",
        out.reply
    );
    let s = session(&rt);
    let p = s.pending_commitment.clone().unwrap();
    assert_eq!((p.kind, p.part.as_str()), (CommitKind::Handoff, "letter"));
    assert_eq!(s.active.as_deref(), Some("bill"), "nothing moves before OK");
    assert_eq!(engine.replies.lock().unwrap().len(), 1, "nothing ran");

    turn(&rt, "yes").await;
    let s = session(&rt);
    assert_eq!(s.active.as_deref(), Some("letter"));
    assert_eq!(status(&rt, "letter"), PartStatus::InReview);
    assert_eq!(status(&rt, "bill"), PartStatus::Open);
    assert!(engine.replies.lock().unwrap().is_empty());
    let prompt = engine.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(prompt.contains("will review your result"), "{prompt}");
}

#[tokio::test]
async fn the_agent_on_a_part_not_shown_is_a_delegation_that_says_why() {
    let (rt, engine, _) = on_bill(vec![says("agent", 0.8), says("agent", 0.8)], vec![]).await;
    shown(&rt, &["kwh"]);
    let out = turn(
        &rt,
        "I already understand parts 1 and 2. hand the letter to the agent",
    )
    .await;
    assert!(
        out.reply
            .starts_with("Delegate **Write the complaint letter** without understanding it?"),
        "{}",
        out.reply
    );
    assert!(
        out.reply
            .ends_with("\nNot a handoff with your review: you haven't shown Tariff yet."),
        "{}",
        out.reply
    );
    let s = session(&rt);
    assert_eq!(s.pending_commitment.as_ref().unwrap().part, "letter");
    assert_eq!(s.history.last().unwrap().content, out.reply);
    // One step per message: the claim in it isn't recorded on its own.
    assert_eq!(status(&rt, "bill"), PartStatus::Open);
    assert!(engine.replies.lock().unwrap().is_empty());

    // Reviewable, but you say you won't check it: a delegation, no why.
    shown(&rt, &["tariff"]);
    let out = turn(&rt, "let the agent write the letter without me checking").await;
    assert!(
        out.reply
            .starts_with("Delegate **Write the complaint letter** without understanding it?")
            && !out.reply.contains("haven't shown"),
        "{}",
        out.reply
    );
}

#[tokio::test]
async fn a_request_to_write_is_offered_to_the_agent_not_told() {
    let (rt, engine, _) = on_bill(vec![says("tell", 0.8)], vec![]).await;
    let out = turn(&rt, "can you write the letter for me?").await;
    assert!(
        out.reply
            .starts_with("Delegate **Write the complaint letter** without understanding it?"),
        "{}",
        out.reply
    );
    assert!(engine.replies.lock().unwrap().is_empty(), "no told answer");
    assert!(
        events_of(&rt, "kwh")
            .iter()
            .all(|e| e.kind != EventKind::Told)
    );
}

#[tokio::test]
async fn an_agent_request_naming_several_parts_runs_them_in_order_on_one_enter() {
    let (rt, engine, _) = on_bill(
        vec![says("agent", 0.8)],
        vec![
            (Full, S2, "Checked the charges."),
            (Full, S2, "Wrote letter.md."),
        ],
    )
    .await;
    let out = turn(&rt, "let the agent do parts 2 and 3").await;
    assert_eq!(
        out.reply,
        "Let the agent do these parts, one after another? **Check the charges** (delegated: you haven't shown Tariff yet); **Write the complaint letter** (delegated: you haven't shown Tariff yet). Enter runs them all · Esc cancels."
    );
    let s = session(&rt);
    let p = s.pending_commitment.clone().unwrap();
    assert_eq!((p.kind, p.part.as_str()), (CommitKind::Delegate, "check"));
    assert_eq!(p.then.len(), 1);
    assert_eq!(engine.replies.lock().unwrap().len(), 2, "nothing ran");

    let deltas = Arc::new(Mutex::new(String::new()));
    let sink: StreamSink = {
        let deltas = deltas.clone();
        Arc::new(move |ev| {
            if let crate::engine::StreamEvent::TextDelta(t) = ev {
                deltas.lock().unwrap().push_str(&t);
            }
        })
    };
    let out = rt
        .handle_turn(
            "s",
            TurnInput {
                text: "/confirm".into(),
                files: vec![],
            },
            sink,
        )
        .await
        .unwrap();
    assert_eq!(status(&rt, "check"), PartStatus::Delegated);
    assert_eq!(status(&rt, "letter"), PartStatus::Delegated);
    assert_eq!(status(&rt, "bill"), PartStatus::Open);
    assert!(engine.replies.lock().unwrap().is_empty(), "both ran");
    // Each run is labelled as what it was, the first above the second.
    let shown = format!("{}{}", deltas.lock().unwrap(), out.reply);
    let first = shown.find("**Check the charges**").unwrap();
    let second = shown.rfind("**Write the complaint letter**").unwrap();
    assert!(first < second, "{shown}");
    assert!(
        out.reply.contains("Write the complaint letter"),
        "{}",
        out.reply
    );
    let prompts = engine.prompts.lock().unwrap().clone();
    let agent: Vec<&String> = prompts
        .iter()
        .filter(|p| p.contains("handed this part off"))
        .collect();
    assert_eq!(agent.len(), 2, "{prompts:?}");
    assert!(agent[0].contains("Check the charges"), "{}", agent[0]);
    assert!(
        agent[1].contains("Write the complaint letter"),
        "{}",
        agent[1]
    );
    assert!(agent[1].contains("Do only this part now"), "{}", agent[1]);
    let s = session(&rt);
    assert!(s.agent_queue.is_empty() && s.pending_commitment.is_none());
}

#[tokio::test]
async fn the_rest_for_the_agent_waits_for_your_review_between_parts() {
    let (rt, engine, _) = on_bill(
        vec![says("agent", 0.8)],
        vec![
            (Full, S2, "Checked the charges."),
            (Full, S2, "Wrote letter.md."),
        ],
    )
    .await;
    shown(&rt, &["kwh", "tariff"]);
    turn(&rt, "/known").await;
    let out = turn(
        &rt,
        "Let the agent do the rest in charges.py: the check and the letter, and run it",
    )
    .await;
    assert_eq!(
        out.reply,
        "Let the agent do these parts, one after another? **Check the charges** (a handoff: you review the result); **Write the complaint letter** (a handoff: you review the result). Enter runs them all · Esc cancels."
    );
    turn(&rt, "/confirm").await;
    // Outside git, a handoff edits in place and waits for your review: the
    // next part runs only after your verdict.
    assert_eq!(status(&rt, "check"), PartStatus::InReview);
    assert_eq!(status(&rt, "letter"), PartStatus::Open);
    assert_eq!(engine.replies.lock().unwrap().len(), 1);
    assert_eq!(session(&rt).agent_queue.len(), 1);

    let out = turn(&rt, "/approve").await;
    assert_eq!(status(&rt, "check"), PartStatus::Done);
    assert_eq!(status(&rt, "letter"), PartStatus::InReview);
    assert!(engine.replies.lock().unwrap().is_empty());
    assert!(session(&rt).agent_queue.is_empty());
    assert!(!out.reply.is_empty());
}

#[tokio::test]
async fn the_rest_means_every_part_still_open() {
    let (rt, _, _) = on_bill(vec![says("agent", 0.8)], vec![]).await;
    let out = turn(
        &rt,
        "do all the remaining parts in one go, let the agent do the rest",
    )
    .await;
    assert!(
        out.reply
            .starts_with("Let the agent do these parts, one after another? **Read the bill**"),
        "{}",
        out.reply
    );
    let p = session(&rt).pending_commitment.unwrap();
    assert_eq!(p.part, "bill");
    let rest: Vec<&str> = p.then.iter().map(|q| q.part.as_str()).collect();
    assert_eq!(rest, ["check", "letter"]);
}

#[tokio::test]
async fn your_own_share_of_a_request_stays_yours() {
    let (rt, _, _) = on_bill(vec![says("agent", 0.8)], vec![]).await;
    let out = turn(
        &rt,
        "Let the agent write the letter, I'll check the charges myself",
    )
    .await;
    assert!(
        out.reply
            .starts_with("Delegate **Write the complaint letter** without understanding it?"),
        "{}",
        out.reply
    );
    assert!(session(&rt).pending_commitment.unwrap().then.is_empty());
}

#[tokio::test]
async fn a_claim_on_several_parts_is_shown_back_once_and_undone_together() {
    let (rt, _, _) = on_bill(vec![says("known", 0.8)], vec![]).await;
    let out = turn(&rt, "I already know the first two parts").await;
    assert!(
        out.reply.starts_with(
            "Recorded: you already know **Read the bill** and **Check the charges**. A claim, not evidence."
        ),
        "{}",
        out.reply
    );
    assert_eq!(status(&rt, "bill"), PartStatus::Known);
    assert_eq!(status(&rt, "check"), PartStatus::Known);
    assert_eq!(status(&rt, "letter"), PartStatus::Open);
    assert_eq!(state_of(&rt, "kwh"), ConceptState::Claimed);
    assert_eq!(state_of(&rt, "tariff"), ConceptState::Claimed);
    let s = session(&rt);
    assert_eq!(s.active.as_deref(), Some("letter"));
    assert!(s.undoable());

    let out = turn(&rt, "/undo").await;
    assert!(
        out.reply.starts_with(
            "Undone: your claim on **Read the bill** and **Check the charges** is withdrawn."
        ),
        "{}",
        out.reply
    );
    let s = session(&rt);
    assert_eq!(status(&rt, "bill"), PartStatus::Open);
    assert_eq!(status(&rt, "check"), PartStatus::Open);
    assert_eq!(s.active.as_deref(), Some("bill"));
    assert!(s.pending_change.is_none());
    assert_eq!(state_of(&rt, "kwh"), ConceptState::Unknown);
    assert_eq!(state_of(&rt, "tariff"), ConceptState::Unknown);
}

#[tokio::test]
async fn a_claim_naming_another_part_by_number_claims_that_part() {
    let (rt, _, _) = on_bill(vec![says("known", 0.8)], vec![]).await;
    let out = turn(&rt, "I already know part 2").await;
    assert!(
        out.reply
            .starts_with("Recorded: you already know **Check the charges**."),
        "{}",
        out.reply
    );
    assert_eq!(status(&rt, "check"), PartStatus::Known);
    assert_eq!(status(&rt, "bill"), PartStatus::Open);
}

#[tokio::test]
async fn a_record_question_in_plain_words_gets_the_record() {
    // No Router call and no model call: the script and engine are empty.
    let (rt, engine, decider) = on_bill(vec![], vec![]).await;
    // Shown on this part, and on an idea only a later part rests on.
    shown(&rt, &["kwh", "tariff"]);
    let out = turn(&rt, "so what do I actually understand now?").await;
    assert_eq!(out.reply, rt.folder_record(&session(&rt)));
    assert!(out.reply.contains("Energy used"), "{}", out.reply);
    assert!(
        out.reply.contains("Tariff"),
        "the whole folder: {}",
        out.reply
    );
    assert!(
        out.reply
            .contains("Of the ideas above, 2 passed a check and 0 rest on your word alone."),
        "{}",
        out.reply
    );
    assert!(!out.reply.contains("Where to look"), "{}", out.reply);
    assert!(engine.replies.lock().unwrap().is_empty());
    assert!(decider.1.lock().unwrap().is_empty(), "no Router call");
    let s = session(&rt);
    assert_eq!(s.history.last().unwrap().content, out.reply);
}

#[tokio::test]
async fn an_agent_run_proposes_only_the_gaps_it_opened() {
    let (rt, _, _) = on_bill(vec![], vec![(Full, S2, "Read it.")]).await;
    // Delegating the bill leaves Energy used delegated; the charges need
    // it, and VAT, which no part covers.
    let out = turn(&rt, "/delegate").await;
    let s = session(&rt);
    assert_eq!(s.active.as_deref(), Some("check"));
    let change = s.pending_change.clone().expect("the run's own gap");
    let concepts: Vec<&str> = change
        .parts
        .iter()
        .flat_map(|p| p.concepts.iter().map(String::as_str))
        .collect();
    assert_eq!(concepts, vec!["kwh"], "{}", out.reply);
    assert_eq!(s.gaps_raised, vec!["kwh".to_string()]);
    assert!(!out.reply.contains("**VAT**"), "{}", out.reply);
}

#[tokio::test]
async fn an_agent_run_that_opened_no_gap_proposes_no_map_change() {
    let (rt, _, _) = on_bill(vec![], vec![(Full, S2, "Read it.")]).await;
    shown(&rt, &["kwh"]);
    let out = turn(&rt, "/delegate").await;
    let s = session(&rt);
    assert_eq!(s.active.as_deref(), Some("check"));
    assert!(s.pending_change.is_none(), "{}", out.reply);
    assert!(!out.reply.contains("Proposed map change"), "{}", out.reply);
    assert!(
        s.gaps_raised.is_empty(),
        "VAT waits for the next part entered"
    );
    // Entered as usual, the charges raise it.
    let out = turn(&rt, "/focus check").await;
    let s = session(&rt);
    assert!(s.gaps_raised.contains(&"vat".to_string()), "{}", out.reply);
    assert!(s.pending_change.is_some(), "{}", out.reply);
}

#[tokio::test]
async fn a_claim_read_in_words_that_ask_for_the_agent_is_the_agents_step() {
    let (rt, _, _) = on_bill(vec![says("known", 0.9)], vec![]).await;
    let out = turn(&rt, "I get the bill now. let the agent write the letter").await;
    assert!(
        out.reply
            .starts_with("Delegate **Write the complaint letter** without understanding it?"),
        "{}",
        out.reply
    );
    assert_eq!(status(&rt, "bill"), PartStatus::Open, "no claim recorded");
}

#[tokio::test]
async fn an_agent_request_read_as_a_note_asks_about_the_part_it_names() {
    let (rt, _, _) = on_bill(vec![says("log", 0.9)], vec![]).await;
    let out = turn(&rt, "I get the bill now. let the agent write the letter").await;
    assert!(
        out.reply.starts_with("Which did you mean:"),
        "{}",
        out.reply
    );
    let ask = session(&rt).pending_ask.clone().unwrap();
    assert_eq!(ask.part, "letter");
    let out = turn(&rt, "1").await;
    assert!(
        out.reply
            .starts_with("Delegate **Write the complaint letter** without understanding it?"),
        "{}",
        out.reply
    );
}

/// The system prompts the engine got since the last call, cleared.
fn systems_since(engine: &FakeEngine) -> Vec<String> {
    std::mem::take(&mut *engine.systems.lock().unwrap())
}

#[tokio::test]
async fn every_model_is_told_the_language_you_write_in() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            // The model reads the message as German (as a German locale or
            // time zone can tip it): your own words win.
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":true,"goal":"Check how routing works","check":true,"language":"de"}"#,
            ),
            (ReadOnly, S2, MAP_CONCEPTS),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Names are tried in order.","key_reason":"They are looked up, not tried."}"#,
            ),
            (
                ToolAccess::None,
                S2,
                r#"{"is_judgment":true,"pass":true,"feedback":"Right: looked up."}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "I think I get it now: the router looks the names up").await;
    let s = session(&rt);
    assert_eq!(
        s.language.as_deref(),
        Some("en"),
        "your words, not the model's guess"
    );
    assert!(s.pending_candidate.is_some());
    let systems = systems_since(&engine);
    assert_eq!(systems.len(), 3, "onboarding, map, candidate");
    for system in &systems {
        assert!(system.contains("in English (`en`)"), "{system}");
        assert!(system.contains("system's locale"), "{system}");
        assert!(!system.contains("German"), "{system}");
    }

    turn(&rt, "That's wrong, it does not try them, it looks them up.").await;
    turn(&rt, "/confirm").await;
    let judge = systems_since(&engine);
    assert_eq!(judge.len(), 1);
    assert!(judge[0].contains("in English (`en`)"), "{}", judge[0]);
}

#[tokio::test]
async fn the_language_stays_until_your_own_words_change_it() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":false,"goal":null,"question":true,"language":"en"}"#,
            ),
            (ReadOnly, S2, NO_HITS),
            // "ok" says nothing about a language; the model guesses German.
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":false,"goal":null,"question":true,"language":"de"}"#,
            ),
            (ReadOnly, S2, NO_HITS),
            // You write German: now it is German.
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":false,"goal":null,"question":true,"language":"de"}"#,
            ),
            (ReadOnly, S2, NO_HITS),
        ]),
        false,
    );
    turn(&rt, "What is the base price on my bill?").await;
    assert_eq!(session(&rt).language.as_deref(), Some("en"));
    systems_since(&engine);

    turn(&rt, "ok").await;
    let s = session(&rt);
    assert_eq!(s.language.as_deref(), Some("en"), "a guess never flips it");
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .starts_with("**Not found**"),
        "{}",
        s.history.last().unwrap().content
    );
    let onboarding = &systems_since(&engine)[0];
    assert!(onboarding.contains("in English (`en`)"), "{onboarding}");

    turn(&rt, "Was ist der Unterschied zwischen den beiden Preisen?").await;
    let s = session(&rt);
    assert_eq!(
        s.language.as_deref(),
        Some("de"),
        "your own words change it"
    );
    assert!(
        s.history
            .last()
            .unwrap()
            .content
            .starts_with("**Nicht gefunden**")
    );
    let onboarding = &systems_since(&engine)[0];
    assert!(onboarding.contains("in German (`de`)"), "{onboarding}");
}

#[test]
fn without_a_language_yet_the_rule_still_rules_out_the_locale() {
    let rule = crate::i18n::rule(None);
    assert!(rule.contains("the user's own messages") && rule.contains("system's locale"));
    assert!(crate::i18n::rule(Some("pt-BR")).contains("Portuguese (`pt`)"));
    assert!(crate::i18n::rule(Some("xx")).contains("in `xx`"));
}

#[tokio::test]
async fn why_counts_the_ideas_it_draws() {
    // Two claims on one idea, and a claim later passed, count as the
    // drawing shows them: one passed, one resting on a claim alone.
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
        ]),
        false,
    );
    on_concepts(&rt).await;
    let s = session(&rt);
    let ev = crate::understanding::NewEvent::new;
    rt.append_events(
        &s,
        vec![
            ev("dispatch", EventKind::Claim),
            ev("dispatch", EventKind::Claim),
            ev("handlers", EventKind::Claim),
            ev("handlers", EventKind::Step).detail(serde_json::json!({"kind":"wide-example"})),
            // You took the step up: you wrote about handlers.
            ev("handlers", EventKind::Touched),
            ev("handlers", EventKind::Pass),
            // A step no verdict followed yet.
            ev("registry", EventKind::Step).detail(serde_json::json!({"kind":"wide-purpose"})),
        ],
    );
    let why = turn(&rt, "/why").await.reply;
    assert_eq!(state_of(&rt, "dispatch"), ConceptState::Claimed);
    assert_eq!(state_of(&rt, "handlers"), ConceptState::Shown);
    assert!(
        why.contains("Of the ideas above, 1 passed a check and 1 rest on your word alone."),
        "{why}"
    );
    // Plain words, and only what a verdict followed.
    assert!(
        why.contains("What has led to a pass so far: starting from an example, 1 of 1 times."),
        "{why}"
    );
    assert!(!why.contains("asking what it's for"), "{why}");
    assert!(!why.contains("wide·"), "{why}");
}

#[tokio::test]
async fn a_claim_is_worded_by_what_the_part_already_rests_on() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_CONCEPTS),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("known", 0.8, None), ("known", 0.8, None)]);
    on_concepts(&rt).await;
    let s = session(&rt);
    let ev = crate::understanding::NewEvent::new;
    // Two of routing's three ideas already shown.
    rt.append_events(
        &s,
        vec![
            ev("dispatch", EventKind::Pass),
            ev("handlers", EventKind::Pass),
        ],
    );
    let out = turn(&rt, "I already know routing").await;
    assert!(
        out.reply.starts_with(
            "Recorded: you already know **Command routing**. Only your word for **Command registry**; the rest you've already shown."
        ),
        "{}",
        out.reply
    );
    turn(&rt, "/undo").await;
    // All three shown: no "a claim, not evidence".
    let s = session(&rt);
    rt.append_events(&s, vec![ev("registry", EventKind::Pass)]);
    let out = turn(&rt, "I already know routing").await;
    assert!(
        out.reply.starts_with(
            "Recorded: you already know **Command routing**. You've already shown every idea it rests on"
        ),
        "{}",
        out.reply
    );
    assert!(
        !out.reply.contains("A claim, not evidence"),
        "{}",
        out.reply
    );
}

#[tokio::test]
async fn a_quick_check_lead_comes_with_the_candidate() {
    // The lead is streamed right before the candidate's header, after the
    // files the candidate writer reads: never alone above the tool lines.
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (
                ReadOnly,
                S1,
                r#"{"reply":"","ready_to_map":true,"goal":"Check how routing works","check":true,"language":"en"}"#,
            ),
            (ReadOnly, S2, MAP_CONCEPTS),
            (
                ReadOnly,
                S2,
                r#"{"candidate":"Names are tried in order.","key_reason":"They are looked up."}"#,
            ),
        ]),
        false,
    );
    let deltas = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink: StreamSink = {
        let deltas = deltas.clone();
        Arc::new(move |ev| {
            if let crate::engine::StreamEvent::TextDelta(t) = ev {
                deltas.lock().unwrap().push(t);
            }
        })
    };
    let out = rt
        .handle_turn(
            "s",
            TurnInput {
                text: "check me on routing".into(),
                files: vec![],
            },
            sink,
        )
        .await
        .unwrap();
    let s = session(&rt);
    assert!(
        s.quick_check && s.pending_candidate.is_some(),
        "{}",
        out.reply
    );
    let deltas = deltas.lock().unwrap();
    assert!(
        !deltas.iter().any(|d| d.trim() == "Quick check:"),
        "{deltas:?}"
    );
    assert!(
        deltas
            .iter()
            .any(|d| d.starts_with("Quick check:\n\nCandidate for **")),
        "{deltas:?}"
    );
}

/// Adversarial: approving a checked review without catching the planted
/// flaw means "the review didn't happen and is not recorded as one" (§13),
/// so `/why` must not list the part as reviewable and approved.
#[tokio::test]
async fn a_missed_planted_check_is_not_shown_as_an_approved_review() {
    let (rt, _engine) = runtime(
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
                ToolAccess::None,
                S2,
                r#"{"pass":true,"feedback":"Yes: it must be advertised in acp.rs."}"#,
            ),
            (Full, S2, "Added /status."),
        ]),
        true,
    );
    turn(&rt, "add a /status command").await;
    turn(
        &rt,
        "commands are advertised in acp.rs and dispatched in runtime.rs",
    )
    .await;
    turn(&rt, "/summary two places: advertise and dispatch").await;
    turn(&rt, "Wrong: acp.rs must advertise it or Zed won't show it").await;
    turn(&rt, "/confirm").await;
    turn(&rt, "/handoff").await;
    assert_eq!(status(&rt, "route"), PartStatus::InReview);
    // The review is a checked one: a planted hunk sits in its staging.
    let mut s = session(&rt);
    let secret = crate::secrets::Secret {
        id: "sec-plant-missed".into(),
        kind: "planted_flaw".into(),
        payload: serde_json::json!({"candidate": "+ return HANDLERS.get(name, help)", "correct": false, "reason": "unknown names fall back"}),
        probe: "whether a hunk is planted, or where".into(),
        opened_at_turn: s.history.len(),
        active: true,
    };
    rt.store.add_secret("s", &secret).unwrap();
    let route = s.map.get_mut("route").unwrap();
    route.staging = Some(Staging {
        dir: s.cwd.join("staging-missed"),
        base: String::new(),
        result: None,
        secret: Some("sec-plant-missed".into()),
        report: String::new(),
        linked: vec![],
    });
    rt.store.save_session(&s).unwrap();
    let out = turn(&rt, "/approve").await;
    assert!(out.reply.contains("isn't recorded as one"), "{}", out.reply);
    let why = turn(&rt, "/why").await.reply;
    assert!(
        !why.contains("you approved it."),
        "a missed check is shown as an approved, reviewable review: {why}"
    );
}

/// Adversarial: while a checked review waits, the planted hunk is a secret
/// only the Judge, the Gate's probe and the Planter may see (A2). The review
/// text that holds it is in the conversation, and `/replan`'s Mapper reads
/// the conversation with no Gate on its map change: it must not see it.
#[tokio::test]
async fn the_mapper_never_sees_a_planted_hunk_while_you_review() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP),
            (
                ReadOnly,
                S2,
                r#"{"brief":"Add help","parts":[{"id":"help","title":"Help text","depends_on":[]}],"remove":[]}"#,
            ),
        ]),
        false,
    );
    turn(&rt, "goal").await;
    let mut s = session(&rt);
    let planted = "+ return HANDLERS.get(name, help)";
    let secret = crate::secrets::Secret {
        id: "sec-plant-map".into(),
        kind: "planted_flaw".into(),
        payload: serde_json::json!({"candidate": planted, "correct": false, "reason": "unknown names fall back"}),
        probe: "whether a hunk is planted, or where".into(),
        opened_at_turn: s.history.len(),
        active: true,
    };
    rt.store.add_secret("s", &secret).unwrap();
    // The review as it is shown, with the planted hunk among the agent's.
    s.push_turn(
        "assistant",
        format!("**Review** for **Command routing**.\n`src/router.rs`\n{planted}\n+ log(name)"),
    );
    let route = s.map.get_mut("route").unwrap();
    route.status = PartStatus::InReview;
    route.staging = Some(Staging {
        dir: s.cwd.join("staging-map"),
        base: String::new(),
        result: None,
        secret: Some("sec-plant-map".into()),
        report: String::new(),
        linked: vec![],
    });
    rt.store.save_session(&s).unwrap();
    let from = engine.prompts.lock().unwrap().len();
    turn(&rt, "/replan the router changed").await;
    let prompts = engine.prompts.lock().unwrap();
    assert_eq!(prompts.len() - from, 1);
    assert!(
        !prompts[from].contains("HANDLERS.get(name, help)"),
        "planted hunk in the Mapper's prompt:\n{}",
        prompts[from]
    );
}

/// Adversarial: `/handoff` on an open part whose concepts are all shown
/// marks it understood and runs the agent, but a candidate waiting on it
/// stays. Judging that candidate later (a pass) sets the part back to
/// understood: the review waiting on the agent's work is dropped and never
/// recorded (§7, §12).
#[tokio::test]
async fn a_handoff_leaves_no_candidate_to_undo_its_review() {
    let (rt, _engine, _) = on_bill(
        vec![says("log", 0.95), says("log", 0.95)],
        vec![
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"ok","map_change":null}"#,
            ),
            (ReadOnly, S2, CANDIDATE_JSON),
            (Full, S2, "Read it."),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"ok","map_change":null}"#,
            ),
        ],
    )
    .await;
    shown(&rt, &["kwh"]);
    turn(&rt, "the kWh line is the usage").await;
    turn(&rt, "/summary usage times price").await;
    assert!(session(&rt).pending_candidate.is_some());
    turn(&rt, "/handoff").await;
    assert_eq!(status(&rt, "bill"), PartStatus::InReview);
    assert!(
        session(&rt).pending_candidate.is_none(),
        "the candidate outlived the handoff"
    );
    turn(
        &rt,
        "Wrong: there is no fallback, unknown names are rejected",
    )
    .await;
    assert_eq!(
        status(&rt, "bill"),
        PartStatus::InReview,
        "a verdict on a stale candidate ended the review"
    );
}

/// The demo's review: the agent wrote the letter, and you accept it in
/// plain words. Whatever the Router read, "looks good, approve it" is your
/// approval, shown back and run only on Enter, as `/approve` (§7, §15).
#[tokio::test]
async fn plain_words_approve_the_agents_result() {
    let (rt, engine, decider) = on_bill(
        vec![
            says("agent", 0.8),
            says("confirm", 0.95),
            says("log", 0.7),
            says("confirm", 0.95),
        ],
        vec![(Full, S2, "Wrote letter.md.")],
    )
    .await;
    shown(&rt, &["kwh", "tariff"]);
    turn(&rt, "ok let the agent write the letter").await;
    turn(&rt, "yes").await;
    assert_eq!(status(&rt, "letter"), PartStatus::InReview);
    let out = turn(&rt, "looks good, approve it").await;
    assert!(
        out.reply
            .starts_with("Approve the agent's result for **Write the complaint letter**?"),
        "{}",
        out.reply
    );
    let offered = decider.1.lock().unwrap().last().cloned().unwrap();
    assert!(
        offered.iter().any(|o| o == "approve") && offered.iter().any(|o| o == "object"),
        "{offered:?}"
    );
    assert_eq!(
        status(&rt, "letter"),
        PartStatus::InReview,
        "nothing runs before OK"
    );
    assert_eq!(
        session(&rt).pending_commitment.as_ref().map(|p| p.kind),
        Some(CommitKind::Approve)
    );
    turn(&rt, "yes").await;
    assert_eq!(status(&rt, "letter"), PartStatus::Done);
    let why = turn(&rt, "/why").await.reply;
    assert!(why.contains("you approved it."), "{why}");
    assert!(engine.replies.lock().unwrap().is_empty());
}

/// Saying what's wrong with the agent's result, in plain words, sends it
/// back with your words as the objection, once you confirm it.
#[tokio::test]
async fn plain_words_object_to_the_agents_result() {
    let (rt, engine, _) = on_bill(
        vec![
            says("agent", 0.8),
            says("confirm", 0.95),
            says("object", 0.9),
            says("confirm", 0.95),
        ],
        vec![
            (Full, S2, "Wrote letter.md."),
            (Full, S2, "Fixed the reading."),
        ],
    )
    .await;
    shown(&rt, &["kwh", "tariff"]);
    turn(&rt, "ok let the agent write the letter").await;
    turn(&rt, "yes").await;
    let objection = "the letter quotes the estimated reading, it must ask for a real one";
    let out = turn(&rt, objection).await;
    assert!(
        out.reply.starts_with(
            "Send **Write the complaint letter** back to the agent with your objection?"
        ) && out.reply.contains(objection),
        "{}",
        out.reply
    );
    assert_eq!(engine.replies.lock().unwrap().len(), 1, "nothing ran");
    turn(&rt, "yes").await;
    assert!(engine.replies.lock().unwrap().is_empty());
    let s = session(&rt);
    let letter = s.map.get("letter").unwrap();
    assert_eq!(letter.status, PartStatus::InReview);
    assert_eq!(letter.objections, vec![objection.to_string()]);
}

/// An approval or objection in plain words needs a result in review:
/// on a part with none, the Router isn't offered them and code doesn't
/// read them.
#[tokio::test]
async fn approval_words_mean_nothing_without_a_result_in_review() {
    let (rt, _engine, decider) = on_bill(
        vec![says("log", 0.9)],
        vec![(
            ReadOnly,
            S1,
            r#"{"intent":"log","reply":"ok","map_change":null}"#,
        )],
    )
    .await;
    turn(&rt, "looks good, approve it").await;
    let offered = decider.1.lock().unwrap().last().cloned().unwrap();
    assert!(!offered.iter().any(|o| o == "approve"), "{offered:?}");
    assert!(session(&rt).pending_commitment.is_none());
}

/// The demo's step 5: the message that starts the map also asks for the
/// agent. The map is drawn and the agent proposed for the part the words
/// name, waiting for Enter, never dropped or run.
#[tokio::test]
async fn an_agent_request_that_starts_the_map_is_proposed_with_it() {
    let (rt, engine) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_BILL),
        ]),
        false,
    );
    let out = turn(
        &rt,
        "great. let the agent write them a letter asking for a real meter reading and a corrected bill, and save it here",
    )
    .await;
    assert!(out.reply.contains("**Read the bill**"), "{}", out.reply);
    assert!(
        out.reply
            .contains("Delegate **Write the complaint letter** without understanding it?"),
        "{}",
        out.reply
    );
    let s = session(&rt);
    let p = s.pending_commitment.clone().expect("the agent is proposed");
    assert_eq!((p.kind, p.part.as_str()), (CommitKind::Delegate, "letter"));
    assert_eq!(s.active.as_deref(), Some("bill"), "nothing moves before OK");
    assert!(engine.replies.lock().unwrap().is_empty(), "nothing ran");
    let last = &s.history.last().unwrap();
    assert!(
        last.role == "assistant"
            && last
                .content
                .contains("Delegate **Write the complaint letter**"),
        "{}",
        last.content
    );
}

/// Asking which you meant, about the agent doing a part other than the one
/// you're on, names that part.
#[tokio::test]
async fn the_ask_names_the_part_the_agent_would_do() {
    let (rt, _engine, _) = on_bill(vec![says("log", 0.9)], vec![]).await;
    let out = turn(&rt, "please write the letter now").await;
    assert_eq!(
        session(&rt).pending_ask.as_ref().map(|a| a.part.as_str()),
        Some("letter")
    );
    assert!(
        out.reply
            .contains("(1) let the agent do **Write the complaint letter**"),
        "{}",
        out.reply
    );
}

/// A move read from plain words keeps its reply in the thread, so the
/// next model never sees your message left unanswered.
#[tokio::test]
async fn a_plain_words_move_keeps_its_reply() {
    let (rt, _engine, _) = on_bill(vec![says("move", 0.95)], vec![]).await;
    let out = turn(&rt, "let's go on to the next part").await;
    let s = session(&rt);
    let last = s.history.last().unwrap();
    assert_eq!(last.role, "assistant");
    assert_eq!(last.content, out.reply.trim_start());
}

/// The masked history hides a waiting review's turn, and only that one.
#[test]
fn a_waiting_review_is_hidden_from_the_history() {
    let mut s = Session::new("s".into(), std::env::temp_dir());
    s.push_turn("user", "/handoff");
    s.push_turn("assistant", "**Review**\n+ return HANDLERS.get(name, help)");
    s.push_turn("user", "hmm");
    let shown = mask_history(&s, &[1]);
    assert!(!shown.contains("HANDLERS"), "{shown}");
    assert!(shown.contains(REVIEW_HIDDEN) && shown.contains("user: hmm"));
    assert!(mask_history(&s, &[]).contains("HANDLERS"));
}

#[tokio::test]
async fn a_check_passed_in_another_thread_of_the_folder_counts_here() {
    let map = r#"{"brief":"One part.","concepts":[
        {"id":"l2-gmm","label":"L2 distance of two GMMs","needs":[]}],
        "parts":[{"id":"l2","title":"Score a reduction","depends_on":[],"concepts":["l2-gmm"]}]}"#;
    let (rt, _) = runtime(
        FakeEngine::new(vec![(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, map)]),
        false,
    );
    let (rt, _) = with_script(rt, vec![says("agent", 0.8)]);
    // A quick check in this folder's chat thread passed on the idea, as
    // that thread worded it.
    let chat = session(&rt);
    let ws = crate::understanding::workspace_key(&chat.cwd);
    rt.store
        .upsert_workspace_concept(
            &ws,
            &crate::understanding::Concept {
                id: "l2-distance".into(),
                label: "L2 distance between Gaussian mixtures".into(),
                aliases: vec![],
                domain: String::new(),
                created_by: "map".into(),
            },
        )
        .unwrap();
    rt.append_events(
        &chat,
        vec![crate::understanding::NewEvent::new(
            "l2-distance",
            EventKind::Pass,
        )],
    );
    // A task thread in the same folder.
    rt.create_session("t".into(), chat.cwd.clone()).unwrap();
    let say = |text: &'static str| {
        rt.handle_turn(
            "t",
            TurnInput {
                text: text.into(),
                files: vec![],
            },
            noop_sink(),
        )
    };
    say("goal").await.unwrap();
    let task = rt.load_session("t").unwrap().unwrap();
    assert_eq!(
        task.map.get("l2").unwrap().concepts,
        vec!["l2-distance".to_string()],
        "one idea, however each thread words it"
    );
    let out = say("let the agent do it").await.unwrap();
    assert!(
        out.reply
            .starts_with("Hand **Score a reduction** to the agent?"),
        "{}",
        out.reply
    );
    assert!(!out.reply.contains("haven't shown"), "{}", out.reply);
}

#[tokio::test]
async fn an_explanation_ending_in_check_me_is_checked() {
    let (rt, engine, _) = on_bill(
        // The Router reads the long explanation as a question.
        vec![says("log", 0.95)],
        vec![(
            ReadOnly,
            S2,
            r#"{"candidate":"kWh times the unit price, plus VAT.","key_reason":"r"}"#,
        )],
    )
    .await;
    turn(
        &rt,
        "The bill multiplies the kWh used by the unit price, then adds the standing charge per day. So the total grows with use. I think I understand it now. Check me",
    )
    .await;
    let s = session(&rt);
    assert!(s.pending_candidate.is_some(), "a candidate, not sources");
    assert!(engine.replies.lock().unwrap().is_empty());
    let log = &s.map.get("bill").unwrap().log;
    assert_eq!(log.len(), 1);
    assert!(log[0].starts_with("The bill multiplies"), "{log:?}");
    assert!(
        !s.history
            .iter()
            .any(|t| t.content.contains("Where to look")),
        "{:?}",
        s.history
    );
}

/// Adversarial: a part you exclude from the request ("…, but not the
/// letter") must not be queued for the agent alongside the one you named.
#[tokio::test]
async fn adv_a_part_you_exclude_is_not_handed_to_the_agent() {
    let (rt, _, _) = on_bill(vec![says("agent", 0.8)], vec![]).await;
    turn(&rt, "let the agent check the charges, but not the letter").await;
    let p = session(&rt).pending_commitment.unwrap();
    let mut parts: Vec<String> = vec![p.part.clone()];
    parts.extend(p.then.iter().map(|q| q.part.clone()));
    assert!(!parts.contains(&"letter".to_string()), "queued: {parts:?}");
}

/// Adversarial: "the rest except the letter" is every open part but the
/// letter, never the letter too.
#[tokio::test]
async fn adv_the_rest_except_a_part_leaves_that_part_out() {
    let (rt, _, _) = on_bill(vec![says("agent", 0.8)], vec![]).await;
    turn(&rt, "let the agent do the rest except the letter").await;
    let p = session(&rt).pending_commitment.unwrap();
    let mut parts: Vec<String> = vec![p.part.clone()];
    parts.extend(p.then.iter().map(|q| q.part.clone()));
    assert!(!parts.contains(&"letter".to_string()), "queued: {parts:?}");
}

#[test]
fn every_prompt_whose_prose_the_user_reads_asks_for_unicode_math() {
    for (name, system) in [
        ("onboarding", ONBOARDING_SYSTEM),
        ("map", MAP_SYSTEM),
        ("change", CHANGE_SYSTEM),
        ("log", LOG_SYSTEM),
        ("candidate", CANDIDATE_SYSTEM),
        ("exam question", EXAM_QUESTION_SYSTEM),
        ("exam judge", EXAM_JUDGE_SYSTEM),
        ("judge", JUDGE_SYSTEM),
        ("agent", AGENT_SYSTEM),
        ("tell", cite::TELL_SYSTEM),
        ("plant", review::PLANT_SYSTEM),
    ] {
        assert!(system.contains("never LaTeX"), "{name}");
    }
}

/// A quick check on the Gaussian product in this folder.
const QUICK_PRODUCT: &str = r#"{"brief":"One part.","concepts":[
    {"id":"gaussian-product","label":"Product of two Gaussians","needs":[]}],
    "parts":[{"id":"product","title":"Gaussian product","depends_on":[],"concepts":["gaussian-product"]}]}"#;

/// Passes a quick check on `gaussian-product` in session "s".
async fn pass_quick_check(rt: &Runtime) {
    turn(rt, "check me on the product of two Gaussians").await;
    assert!(session(rt).quick_check && session(rt).pending_candidate.is_some());
    turn(rt, "it's right: the means are weighted by precision").await;
    turn(rt, "/confirm").await;
    assert_eq!(
        state_of(rt, "gaussian-product"),
        ConceptState::Shown,
        "the quick check passed"
    );
}

fn quick_check_replies() -> Vec<(ToolAccess, EngineTier, &'static str)> {
    vec![
        (
            ReadOnly,
            S1,
            r#"{"reply":"","ready_to_map":true,"goal":"Check the Gaussian product","check":true,"language":"en"}"#,
        ),
        (ReadOnly, S2, QUICK_PRODUCT),
        (
            ReadOnly,
            S2,
            r#"{"candidate":"The product's mean weights each mean by its precision.","key_reason":"r"}"#,
        ),
        (
            ToolAccess::None,
            S2,
            r#"{"is_judgment":true,"pass":true,"feedback":"Right."}"#,
        ),
    ]
}

/// A task map that reuses the quick check's concept by id.
const TASK_REUSES: &str = r#"{"brief":"One part.","concepts":[
    {"id":"mixture-weights","label":"Mixture weights","needs":[]}],
    "parts":[{"id":"fuse","title":"Fuse two components","depends_on":[],"concepts":["gaussian-product","mixture-weights"]}]}"#;

/// A quick check passed in this folder counts in a task here: the Mapper
/// is shown the concept with its state, reuses its id, and the part rests
/// on it as shown. Another folder neither lists it nor counts it.
#[tokio::test]
async fn a_quick_check_pass_shows_in_the_task_that_rests_on_it() {
    let mut replies = quick_check_replies();
    replies.extend([(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, TASK_REUSES)]);
    replies.extend([(ReadOnly, S1, ONBOARD_READY), (ReadOnly, S2, TASK_REUSES)]);
    let (rt, engine) = runtime(FakeEngine::new(replies), false);
    pass_quick_check(&rt).await;

    let cwd = session(&rt).cwd;
    rt.create_session("t".into(), cwd).unwrap();
    let out = rt
        .handle_turn(
            "t",
            TurnInput {
                text: "goal".into(),
                files: vec![],
            },
            noop_sink(),
        )
        .await
        .unwrap();
    let map_prompt = engine.prompts.lock().unwrap().last().unwrap().clone();
    assert!(
        map_prompt.contains("- `gaussian-product`: Product of two Gaussians (shown)"),
        "{map_prompt}"
    );
    let task = rt.load_session("t").unwrap().unwrap();
    assert_eq!(
        task.map.get("fuse").unwrap().concepts,
        vec!["gaussian-product", "mixture-weights"]
    );
    let shown = format!("{}{}", out.reply, task.history.last().unwrap().content);
    assert!(
        shown.contains("Rests on: Product of two Gaussians (shown), Mixture weights (unknown)."),
        "{shown}"
    );

    // Another folder: the same id is a different record.
    let other = std::env::temp_dir().join(format!("harness-rt-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&other).unwrap();
    rt.create_session("u".into(), other).unwrap();
    rt.handle_turn(
        "u",
        TurnInput {
            text: "goal".into(),
            files: vec![],
        },
        noop_sink(),
    )
    .await
    .unwrap();
    let map_prompt = engine.prompts.lock().unwrap().last().unwrap().clone();
    assert!(!map_prompt.contains("gaussian-product"), "{map_prompt}");
    let u = rt.load_session("u").unwrap().unwrap();
    let last = &u.history.last().unwrap().content;
    assert!(
        last.contains("Rests on: Product of two Gaussians (unknown)")
            || last.contains("Rests on: gaussian product (unknown)"),
        "{last}"
    );
    assert!(!last.contains("(shown)"), "{last}");
}

/// A Mapper that still words the idea anew in a map change, under a new
/// id and label, is linked to the recorded concept by the name in its id,
/// so the pass counts; and
/// the change's prompt lists the folder's concepts with their states.
#[tokio::test]
async fn a_change_that_renames_a_shown_concept_is_linked_to_it() {
    let mut replies = quick_check_replies();
    replies.extend([
        (ReadOnly, S1, ONBOARD_READY),
        (
            ReadOnly,
            S2,
            r#"{"brief":"One part.","concepts":[{"id":"rng","label":"Reproducible random inputs","needs":[]}],
            "parts":[{"id":"inputs","title":"Random inputs","depends_on":[],"concepts":["rng"]}]}"#,
        ),
        (
            ReadOnly,
            S2,
            r#"{"brief":"You add the fusion.","goal":null,"remove":[],
            "concepts":[{"id":"products-of-gaussians","label":"Gaussian fusion","needs":[]}],
            "parts":[{"id":"fuse","title":"Fuse two components","depends_on":["inputs"],"concepts":["products-of-gaussians"]}]}"#,
        ),
    ]);
    let (rt, engine) = runtime(FakeEngine::new(replies), false);
    let decider = Arc::new(ConceptDecider("gaussian-product", Mutex::new(Vec::new())));
    let rt = rt.with_decider(decider.clone());
    pass_quick_check(&rt).await;
    let cwd = session(&rt).cwd;
    rt.create_session("t".into(), cwd).unwrap();
    let say = |text: &'static str| {
        rt.handle_turn(
            "t",
            TurnInput {
                text: text.into(),
                files: vec![],
            },
            noop_sink(),
        )
    };
    say("goal").await.unwrap();
    say("/replan add the fusion").await.unwrap();
    let change_prompt = engine.prompts.lock().unwrap().last().unwrap().clone();
    assert!(
        change_prompt.contains("- `gaussian-product`: Product of two Gaussians (shown)"),
        "{change_prompt}"
    );
    say("/accept").await.unwrap();
    let task = rt.load_session("t").unwrap().unwrap();
    assert_eq!(
        task.map.get("fuse").unwrap().concepts,
        vec!["gaussian-product"],
        "one idea, however the Mapper words it, once the decision model says so"
    );
    let asked = decider.1.lock().unwrap().clone();
    assert!(
        asked
            .iter()
            .any(|q| q.contains("Gaussian fusion") && q.contains("gaussian-product")),
        "the id's words only make it a candidate: {asked:?}"
    );
}

/// Answers concept-matching questions (`c0`, `c1`, …) with one recorded
/// concept at p = 0.9 when it is offered; records each question with its
/// options. Every other question goes unanswered.
struct ConceptDecider(&'static str, Mutex<Vec<String>>);

impl Decide for ConceptDecider {
    fn decide<'a>(
        &'a self,
        _input: &'a str,
        questions: &'a [Question],
        _prefer: Option<&'a str>,
    ) -> futures_util::future::BoxFuture<'a, Result<Option<Decision>>> {
        let concept = |q: &Question| {
            q.name.len() > 1
                && q.name.starts_with('c')
                && q.name[1..].chars().all(|c| c.is_ascii_digit())
        };
        if !questions.iter().any(concept) {
            return Box::pin(async { Ok(None) });
        }
        let mut answers = std::collections::HashMap::new();
        for q in questions.iter().filter(|q| concept(q)) {
            let offered: Vec<&str> = q.options.iter().map(|o| o.0.as_str()).collect();
            self.1
                .lock()
                .unwrap()
                .push(format!("{} {}", q.instructions, offered.join(" ")));
            let choice = if offered.contains(&self.0) {
                self.0
            } else {
                "new"
            };
            answers.insert(
                q.name.clone(),
                crate::decide::Answer {
                    choice: choice.into(),
                    probability: 0.9,
                    ..Default::default()
                },
            );
        }
        Box::pin(async move {
            Ok(Some(Decision {
                by: "Fake".into(),
                answers,
            }))
        })
    }
}

/// A Mapper id that names a recorded concept, under a label for another
/// idea, is not merged on its words alone: without the decision model's
/// yes it stays a concept of its own, and its label never becomes an alias.
#[tokio::test]
async fn a_concept_id_alone_never_merges_a_differently_labelled_idea() {
    let mut replies = quick_check_replies();
    replies.extend([
        (ReadOnly, S1, ONBOARD_READY),
        (
            ReadOnly,
            S2,
            r#"{"brief":"One part.","concepts":[{"id":"product-of-gaussians","label":"Variance of a product of Gaussians","needs":[]}],
            "parts":[{"id":"var","title":"The fused variance","depends_on":[],"concepts":["product-of-gaussians"]}]}"#,
        ),
    ]);
    let (rt, _) = runtime(FakeEngine::new(replies), false);
    pass_quick_check(&rt).await;
    let cwd = session(&rt).cwd;
    rt.create_session("t".into(), cwd).unwrap();
    rt.handle_turn(
        "t",
        TurnInput {
            text: "goal".into(),
            files: vec![],
        },
        noop_sink(),
    )
    .await
    .unwrap();
    let task = rt.load_session("t").unwrap().unwrap();
    let ids = &task.map.get("var").unwrap().concepts;
    assert_eq!(ids.len(), 1);
    assert_ne!(ids[0], "gaussian-product", "a narrower idea isn't credited");
    let card = &task.history.last().unwrap().content;
    assert!(!card.contains("(shown)"), "{card}");
}

/// Delegating a part hands off only what no other open part rests on: a
/// concept you may still show on the next part keeps its own state.
#[tokio::test]
async fn delegating_a_part_does_not_mark_a_shared_concept() {
    let map = r#"{"brief":"Two parts.","concepts":[
        {"id":"densities","label":"Gaussian densities","needs":[]},
        {"id":"rng","label":"Reproducible random inputs","needs":[]},
        {"id":"product","label":"Product of two Gaussians","needs":[]}],
        "parts":[
        {"id":"inputs","title":"Random inputs","depends_on":[],"concepts":["rng","densities"]},
        {"id":"fuse","title":"Fuse two components","depends_on":[],"concepts":["product","densities"]}]}"#;
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, map),
            (Full, S2, "Wrote it."),
        ]),
        false,
    );
    let rt = with_decisions(rt, vec![("agent", 0.8, None)]);
    turn(&rt, "goal").await;
    turn(&rt, "/reject").await;
    let out = turn(&rt, "just have the agent do the inputs").await;
    assert!(
        out.reply.starts_with("Delegate **Random inputs**"),
        "{}",
        out.reply
    );
    turn(&rt, "/confirm").await;
    assert_eq!(status(&rt, "inputs"), PartStatus::Delegated);
    assert_eq!(state_of(&rt, "rng"), ConceptState::Delegated);
    assert_eq!(
        state_of(&rt, "densities"),
        ConceptState::Unknown,
        "the fusion part still rests on it"
    );
    let s = session(&rt);
    assert_eq!(s.active.as_deref(), Some("fuse"));
    let card = &s.history.last().unwrap().content;
    assert!(!card.contains("(delegated)"), "{card}");
}

#[tokio::test]
async fn answering_a_specific_next_question_never_loops_silently_to_logged_and_same_next() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_NOTES),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"ok","ack":"","missing":"","question":"On line 30, how does the table map a name to a handler?","map_change":null,"concepts":["dispatch"],"misconception":null}"#,
            ),
            (ReadOnly, S2, "{}"),
            (ReadOnly, S1, &log_reply("ok")),
        ]),
        false,
    );
    write_notes(&rt);
    let (rt, _) = with_script(rt, vec![says("log", 0.9), says("log", 0.9)]);
    let entered = turn(&rt, "goal").await;
    assert!(
        entered.reply.contains(
            "What role does “Dispatch by name” play in this part? Put it in your own words."
        ),
        "{}",
        entered.reply
    );

    // User asks a question and gets "Where to look:" plus a specific Next question.
    turn(&rt, "How does dispatch find the handler?").await;
    let q_reply = session(&rt).history.last().unwrap().content.clone();
    assert!(
        q_reply.contains("On line 30, how does the table map a name to a handler?"),
        "{q_reply}"
    );

    // User answers that specific question directly; even with empty model
    // ack/missing/question, the harness must explain why it didn't advance
    // and must not repeat the verbatim StepWidePurpose prompt.
    turn(
        &rt,
        "It looks up the name in the table and calls the handler",
    )
    .await;
    let reply = session(&rt).history.last().unwrap().content.clone();
    assert!(
        reply.contains("still open: **Dispatch by name**"),
        "{reply}"
    );
    assert!(!reply.contains("`/"), "{reply}");
    assert!(
        !reply.contains("What role does “Dispatch by name” play in this part?"),
        "must not repeat the verbatim Next question: {reply}"
    );
    assert!(
        reply.ends_with("· How does “Dispatch by name” work here, step by step?"),
        "{reply}"
    );
}

#[tokio::test]
async fn log_reply_missing_field_explains_what_is_still_unaddressed() {
    let (rt, _) = runtime(
        FakeEngine::new(vec![
            (ReadOnly, S1, ONBOARD_READY),
            (ReadOnly, S2, MAP_NOTES),
            (
                ReadOnly,
                S1,
                r#"{"intent":"log","reply":"ok","ack":"","missing":"You haven't explained what happens when a name is missing from the table yet.","question":"","map_change":null,"concepts":["dispatch"],"misconception":null}"#,
            ),
        ]),
        false,
    );
    write_notes(&rt);
    let (rt, _) = with_script(rt, vec![says("log", 0.9)]);
    turn(&rt, "goal").await;
    turn(&rt, "The router keeps a table").await;
    let reply = session(&rt).history.last().unwrap().content.clone();
    assert!(
        reply.contains(
            "You haven't explained what happens when a name is missing from the table yet."
        ),
        "{reply}"
    );
}
