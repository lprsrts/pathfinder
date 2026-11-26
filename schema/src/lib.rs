//! The usage events Pathfinder may send, shared by the app (which writes
//! them) and the collector (which accepts them). Everything here is a
//! number, a flag or a closed set of names: no field can hold message
//! text, file names or paths. The few strings (app version, model id,
//! locale) are checked by [`Envelope::validate`], and both sides reject
//! fields they don't know.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The schema version. Bump it when a field changes meaning; the collector
/// keeps accepting older versions it knows.
pub const VERSION: u32 = 1;

/// The privacy notice version the consent was given for. Changing what is
/// collected means a new notice, and asking again.
pub const NOTICE_VERSION: u32 = 3;

/// One event with what every event carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub v: u32,
    /// Random per event, so a retried upload isn't counted twice.
    pub id: String,
    /// Random per app run (a thread opened in the TUI or Zed).
    pub session: String,
    /// Unix seconds, rounded down to the minute.
    pub at: i64,
    /// App version, e.g. `0.1.0`.
    pub app: String,
    pub os: Os,
    pub arch: Arch,
    /// Language tag of the conversation, e.g. `de` or `en`. Empty if unknown.
    #[serde(default)]
    pub locale: String,
    pub event: Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Macos,
    Linux,
    Windows,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    Aarch64,
    X86_64,
    Other,
}

impl Os {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "macos" => Self::Macos,
            "linux" => Self::Linux,
            "windows" => Self::Windows,
            _ => Self::Other,
        }
    }
}

impl Arch {
    pub fn current() -> Self {
        match std::env::consts::ARCH {
            "aarch64" => Self::Aarch64,
            "x86_64" => Self::X86_64,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    /// A thread opened.
    SessionStart { surface: Surface },
    /// A thread closed (or the app quit).
    SessionEnd { secs: u64, turns: u32 },
    /// One message answered.
    Turn {
        /// The route, e.g. `chatgpt`, `anthropic`, `endpoint`.
        route: String,
        /// The model id, e.g. `gpt-6-luna`.
        model: String,
        ms: u64,
        /// What the turn was (a log entry, a candidate, a verdict, …).
        step: Step,
        /// Agent steps (model calls) in the turn.
        steps: u32,
        /// Tool calls by kind.
        #[serde(default)]
        tools: BTreeMap<Tool, u32>,
        retries: u32,
        failed: bool,
    },
    /// The gate looked at a reply while a candidate was being judged.
    Gate {
        /// The highest leak score of the first draft (none: the pre-check
        /// or no decision service).
        score: Option<f64>,
        outcome: GateOutcome,
        /// Which service checked it (absent from apps before 0.3).
        #[serde(default)]
        via: Option<GateVia>,
        /// How long the check took.
        #[serde(default)]
        ms: Option<u64>,
        /// Why the check failed, when `outcome` is `failed`.
        #[serde(default)]
        cause: Option<ErrorCode>,
    },
    /// A candidate was judged.
    Check {
        outcome: CheckOutcome,
        /// Concepts the candidate checks (a count, never their names).
        concepts: u32,
        /// A quick check or exam question, not a part's own check.
        quick: bool,
    },
    /// A permission prompt was answered.
    Permission {
        scope: PermissionScope,
        answer: PermissionAnswer,
    },
    /// Something failed. Only a code: messages can hold paths.
    Error { code: ErrorCode },
    /// A feature was used.
    Ui { feature: Feature },
    /// The app crashed. No message or backtrace: those can hold paths.
    Crash { surface: Surface },
    /// `pathfinder update` finished (the envelope carries the version it
    /// updated from).
    Update { outcome: UpdateOutcome },
    /// A first time: how far new users get.
    Milestone { step: Milestone },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateVia {
    /// The user's own decision key.
    Own,
    /// The shared gate service.
    Shared,
    /// A model on the user's own endpoint.
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateOutcome {
    Installed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Milestone {
    FirstTurn,
    FirstMap,
    FirstCandidate,
    FirstCheck,
    FirstShare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Tui,
    Zed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Onboarding,
    Map,
    Log,
    Candidate,
    Verdict,
    Summary,
    Chat,
    Delegate,
    Handoff,
    Other,
}

/// What a tool call did, by kind (the same kinds every route reports).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Read,
    Edit,
    Search,
    Execute,
    Fetch,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    /// The draft went out as written.
    Passed,
    /// The draft was held and a rewrite went out.
    Rewritten,
    /// Neither passed: the fixed reply went out.
    Fixed,
    /// The check failed, so the fixed reply went out.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    /// The user's verdict was right.
    Right,
    /// The user's verdict was wrong.
    Wrong,
    /// A quick check left unanswered and dropped.
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionScope {
    Sandboxed,
    Network,
    OutsideSandbox,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAnswer {
    Once,
    Always,
    /// Allowed without asking, from an earlier "always".
    Remembered,
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Auth,
    UsageLimit,
    RateLimited,
    Network,
    Timeout,
    BadOutput,
    Tool,
    /// The gate's decision service failed or was capped.
    Gate,
    /// The app couldn't reach Pathfinder's server.
    Collector,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    ToolGrouping,
    Navigate,
    Copy,
    Threads,
    Settings,
    Help,
    SidePanel,
    Share,
    Exam,
    QuickCheck,
    Bug,
}

/// A batch as uploaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub events: Vec<Envelope>,
}

/// Enrolment: an invite code for an install token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enroll {
    pub invite: String,
    /// Random, made on the device; the collector never learns more.
    pub install: String,
    pub notice: u32,
    /// Which data the user agreed to.
    pub events: bool,
    pub threads: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Enrolled {
    pub token: String,
}

/// Signing in with the browser: the app asks for a link code, the person
/// opens `/join?code=…`, signs in with their email and approves it, and
/// the app collects its token with the secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkStart {
    pub install: String,
    pub notice: u32,
    pub events: bool,
    pub threads: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkStarted {
    /// Shown in the terminal and on the web page, to match them up.
    pub code: String,
    /// Only the app knows it; it collects the token with it.
    pub secret: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkPoll {
    pub secret: String,
}

/// A consent change after enrolment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consent {
    pub notice: u32,
    pub events: bool,
    pub threads: bool,
}

/// A bug report the user chose to send, after seeing exactly this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BugReport {
    /// Random per report.
    pub id: String,
    pub app: String,
    pub os: Os,
    pub arch: Arch,
    /// What happened, in their words.
    pub note: String,
    /// Settings, recent log lines and the last crash, paths shortened.
    pub details: String,
}

impl BugReport {
    pub const MAX_NOTE: usize = 4_000;
    pub const MAX_DETAILS: usize = 128 << 10;
}

/// A thread the user chose to share, after seeing exactly this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedThread {
    pub id: String,
    pub app: String,
    /// Why they shared it, in their words (optional).
    #[serde(default)]
    pub note: String,
    /// The thread as the user saw it, paths made relative.
    pub transcript: serde_json::Value,
}

/// The newest release, signed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    /// Per target (`aarch64-apple-darwin`, `universal-apple-darwin`, …).
    pub files: BTreeMap<String, ReleaseFile>,
    #[serde(default)]
    pub notes: String,
    /// The source commit this release was built from.
    #[serde(default)]
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseFile {
    /// Relative to the manifest, e.g. `0.3.0/pathfinder-0.3.0-universal-apple-darwin.tar.gz`.
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Why an event is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub String);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Invalid {}

/// A short token of letters, digits and `.:_-/+`: a version, a model id,
/// a route or a language tag. Never free text.
pub fn is_token(s: &str, max: usize) -> bool {
    s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '_' | '-' | '/' | '+'))
}

fn is_id(s: &str) -> bool {
    (8..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

impl Envelope {
    pub fn validate(&self) -> Result<(), Invalid> {
        let bad = |what: &str| Err(Invalid(format!("bad {what}")));
        if self.v == 0 || self.v > VERSION {
            return bad("version");
        }
        if !is_id(&self.id) {
            return bad("id");
        }
        if !is_id(&self.session) {
            return bad("session");
        }
        if self.at % 60 != 0 || self.at < 1_700_000_000 {
            return bad("time");
        }
        if self.app.is_empty() || !is_token(&self.app, 32) {
            return bad("app version");
        }
        if !is_token(&self.locale, 16) {
            return bad("locale");
        }
        match &self.event {
            Event::Turn { route, model, .. } => {
                if !is_token(route, 32) {
                    return bad("route");
                }
                if !is_token(model, 96) {
                    return bad("model");
                }
            }
            Event::Gate { score: Some(s), .. } if !(0.0..=1.0).contains(s) => {
                return bad("score");
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> Envelope {
        Envelope {
            v: VERSION,
            id: "0b5d0f7e-6c2a-4b8e-9f00-1a2b3c4d5e6f".into(),
            session: "7c1e2d3f-0000-4000-8000-123456789abc".into(),
            at: 1_790_000_040,
            app: "0.1.0".into(),
            os: Os::Macos,
            arch: Arch::Aarch64,
            locale: "de".into(),
            event: Event::Turn {
                route: "chatgpt".into(),
                model: "gpt-6-luna".into(),
                ms: 1200,
                step: Step::Log,
                steps: 2,
                tools: [(Tool::Read, 3)].into(),
                retries: 0,
                failed: false,
            },
        }
    }

    #[test]
    fn round_trips_and_validates() {
        let e = turn();
        let json = serde_json::to_string(&e).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
        back.validate().unwrap();
    }

    #[test]
    fn unknown_fields_are_refused() {
        let mut v = serde_json::to_value(turn()).unwrap();
        v["event"]["message"] = "what I typed".into();
        assert!(serde_json::from_value::<Envelope>(v).is_err());
        let mut v = serde_json::to_value(turn()).unwrap();
        v["path"] = "/Users/me".into();
        assert!(serde_json::from_value::<Envelope>(v).is_err());
    }

    #[test]
    fn free_text_in_strings_is_refused() {
        let mut e = turn();
        e.app = "0.1.0 my notes about entropy".into();
        assert!(e.validate().is_err());
        let mut e = turn();
        if let Event::Turn { model, .. } = &mut e.event {
            *model = "a model, with commas".into();
        }
        assert!(e.validate().is_err());
        let mut e = turn();
        e.at += 7;
        assert!(e.validate().is_err(), "time is to the minute");
    }

    #[test]
    fn unknown_tools_and_kinds_are_refused() {
        let mut v = serde_json::to_value(turn()).unwrap();
        v["event"]["tools"]["secret_tool"] = 1.into();
        assert!(serde_json::from_value::<Envelope>(v).is_err());
        let mut v = serde_json::to_value(turn()).unwrap();
        v["event"]["kind"] = "keystrokes".into();
        assert!(serde_json::from_value::<Envelope>(v).is_err());
    }
}
