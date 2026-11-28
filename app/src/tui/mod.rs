//! `pathfinder tui [folder]`: Pathfinder in the terminal, the front end to
//! the runtime (threads, the record, checks and exams).

mod commands;
mod markdown;
mod view;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use futures_util::StreamExt;
use ratatui::layout::Rect;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

use crate::domain::Session;
use crate::engine::{BoxFuture, Host, Permission, RunContext, StreamEvent, StreamSink, with_turn};
use crate::runtime::{CHECKS, EFFORTS, OptionValue, Runtime, TurnInput, TurnOutput};

/// One thing in the conversation as shown.
#[derive(Debug, Clone)]
pub enum Entry {
    User(String),
    Text(String),
    Thought(String),
    Tool {
        id: String,
        title: String,
        ok: Option<bool>,
        detail: String,
    },
    Note(String),
    /// A source `/open` showed: its path, the part shown (`lines 20–24 of
    /// 391`) and its numbered block in markdown, with the footer.
    Source {
        title: String,
        range: String,
        body: String,
    },
    /// `/find` hits, the best first, and how many lines matched in all.
    Search {
        query: String,
        hits: Vec<crate::sources::Hit>,
        total: usize,
    },
    /// Facts as rows with aligned keys (`/status`). A value may span lines.
    Info(Vec<(String, String)>),
}

impl Entry {
    /// What a turn's reply becomes on screen.
    fn of_reply(reply: String, shown: crate::runtime::Shown) -> Self {
        use crate::runtime::Shown;
        match shown {
            Shown::Reply => Self::Text(reply),
            Shown::Source { title, range, body } => Self::Source { title, range, body },
            Shown::Search { query, hits, total } => Self::Search { query, hits, total },
            Shown::Notice => Self::Note(reply),
        }
    }

    /// The text `y` copies: a reply as written, a source's code without
    /// its gutter or fence, search hits as `path:line  text`.
    fn copy_text(&self) -> String {
        match self {
            Entry::User(t) | Entry::Text(t) | Entry::Thought(t) | Entry::Note(t) => t.clone(),
            Entry::Tool { title, detail, .. } => {
                if detail.is_empty() {
                    title.clone()
                } else {
                    format!("{title}\n{detail}")
                }
            }
            Entry::Source { body, .. } => source_code(body),
            Entry::Search { hits, .. } => hits
                .iter()
                .map(|h| format!("{}  {}", h.at, h.quote.trim()))
                .collect::<Vec<_>>()
                .join("\n"),
            Entry::Info(rows) => rows
                .iter()
                .map(|(k, v)| format!("{k}: {}", v.replace('\n', "; ")))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// What `Y` copies: a source's code under its `path:lines`; else as `y`.
    fn copy_with_place(&self) -> String {
        match self {
            Entry::Source { title, range, body } => {
                let place = match source_lines(range) {
                    Some((a, b)) if a == b => format!("{title}:{a}"),
                    Some((a, b)) => format!("{title}:{a}-{b}"),
                    None => title.clone(),
                };
                format!("{place}\n{}", source_code(body))
            }
            _ => self.copy_text(),
        }
    }
}

/// A numbered block's code: the lines inside the fence without their
/// number gutter. Anything else (a folder's list) as it is.
fn source_code(body: &str) -> String {
    let mut lines = body.lines();
    let Some(open) = lines.next() else {
        return String::new();
    };
    let fence: String = open
        .chars()
        .take_while(|c| *c == '`' || *c == '~')
        .collect();
    if fence.len() < 3 {
        return body.to_string();
    }
    lines
        .take_while(|l| l.trim() != fence)
        .map(|l| markdown::split_gutter(l).1)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `lines 20–24 of 391` → (20, 24).
fn source_lines(range: &str) -> Option<(usize, usize)> {
    let span = range.strip_prefix("lines ")?.split(" of ").next()?;
    let (a, b) = span.split_once('–')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// What the background turn tells the screen.
pub enum AppEvent {
    Stream(StreamEvent),
    Done {
        result: std::result::Result<TurnOutput, String>,
        stopped: bool,
    },
    Ask {
        call_id: String,
        always_key: String,
        reply: oneshot::Sender<Permission>,
    },
    /// Joining finished (an invite code, or signing in with the browser).
    Enrolled(std::result::Result<(), String>),
    /// The collector gave a sign-in code (or couldn't).
    SignInStarted(std::result::Result<crate::telemetry::SignIn, String>),
    /// A newer release is out.
    UpdateAvailable(String),
    /// A shared thread was sent (or not).
    Shared(std::result::Result<(), String>),
    /// A line for the status bar, from a background check.
    Status(String),
    /// The startup refresh of the model list finished.
    ModelsRefreshed(crate::catalog::RefreshOutcome),
    /// The `/login` or `/logout` job has news.
    Login(LoginEvent),
}

/// What a running `/login` or `/logout` job says, tagged with the job's
/// number: news from a job cancelled meanwhile is dropped.
#[derive(Debug, Clone)]
pub enum LoginEvent {
    /// The browser sign-in's address, shown in case the browser doesn't open.
    Url(u64, String),
    /// Finished: what to say, or why it failed.
    Done(u64, std::result::Result<String, crate::auth::flow::Failure>),
}

/// A dialog over the conversation.
pub enum Modal {
    /// Earlier threads: (id, title), and beside each its dim
    /// `mode · turns · age` (and whether it is the open one).
    Threads {
        items: Vec<(String, String)>,
        meta: Vec<String>,
        query: String,
        sel: usize,
    },
    Settings {
        sel: usize,
    },
    Choose {
        option: &'static str,
        title: String,
        items: Vec<(String, String)>,
        query: String,
        sel: usize,
    },
    Permission {
        title: String,
        always_key: String,
        sel: Option<usize>,
        reply: Option<oneshot::Sender<Permission>>,
    },
    /// How it works, the commands and the keys, filtered by `query`;
    /// `scroll` lines down.
    Help {
        scroll: usize,
        query: String,
    },
    /// Asks once whether to share usage data, then signs in.
    /// Exactly what `/share` or `/bug` would send, before it is sent.
    Share {
        outgoing: Outgoing,
        lines: Vec<String>,
        scroll: usize,
        busy: bool,
    },
    Consent {
        events: bool,
        threads: bool,
        step: ConsentStep,
        error: Option<String>,
        busy: bool,
        scroll: usize,
    },
    /// `/login` and `/logout`: the account list and its steps.
    Login(Box<crate::auth::flow::Flow>),
}

pub use crate::picker::{display_rows, filter_matches, is_header};

impl Modal {
    /// Help, at its top with nothing typed.
    pub fn help() -> Self {
        Modal::Help {
            scroll: 0,
            query: String::new(),
        }
    }
}

/// Something you may send, shown in full first.
#[derive(Debug, Clone)]
pub enum Outgoing {
    Thread(pathfinder_schema::SharedThread),
    Bug(pathfinder_schema::BugReport),
}

impl Outgoing {
    pub fn title(&self) -> String {
        match self {
            Outgoing::Thread(_) => format!(" Share this thread with {}? ", crate::operator::NAME),
            Outgoing::Bug(_) => format!(" Send this bug report to {}? ", crate::operator::NAME),
        }
    }
}

/// Where the consent screen is.
#[derive(Debug, Clone)]
pub enum ConsentStep {
    /// Choosing what to share.
    Choose,
    /// Signing in with the browser; None until the collector gave a code.
    Browser(Option<crate::telemetry::SignIn>),
    /// Typing an invite code instead.
    Invite(String),
}

/// The gate key's row in [`SETTINGS`].
fn gate_row() -> usize {
    SETTINGS
        .iter()
        .position(|(o, _)| *o == GATE_KEY)
        .unwrap_or(0)
}

/// A turn's error, pointing at `/login` where it says to sign in or add a
/// key from a terminal.
fn login_hint(e: &str) -> String {
    if e.contains("`pathfinder auth") {
        format!("{e} Here, /login does it.")
    } else {
        e.to_string()
    }
}

/// Said when a background refresh asked for model lists and got none.
const UNLISTED: &str =
    "Couldn't list the models your accounts offer; `pathfinder models --refresh` shows why.";

/// A model id without its `provider:` prefix, as in a short note.
fn bare_model(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, model)| model)
}

/// Not a thread option: the gate's own key (TypeSafe Jev), set, replaced
/// or removed from its Settings row.
pub const GATE_KEY: &str = "gate_key";

/// The thread settings Ctrl+P offers: (option id, name).
pub const SETTINGS: [(&str, &str); 8] = [
    (crate::options::FAST_MODEL, "Fast model"),
    (crate::options::FAST_EFFORT, "Fast model effort"),
    (crate::options::MAIN_MODEL, "Main model"),
    (crate::options::MAIN_EFFORT, "Main model effort"),
    (crate::options::DECISIONS, "Decisions"),
    (crate::options::DECISION_MODEL, "Decision service"),
    (GATE_KEY, "Gate key"),
    (crate::options::CHECKS, "Review checks"),
];

/// A position in the rendered conversation transcript: (line index, content column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPos {
    pub line: usize,
    pub col: u16,
}

/// Active mouse drag text selection in the conversation pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSelection {
    pub anchor: TextPos,
    pub head: TextPos,
    pub dragging: bool,
}

impl TextSelection {
    pub fn ordered(self) -> (TextPos, TextPos) {
        if (self.anchor.line, self.anchor.col) <= (self.head.line, self.head.col) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }
}

/// Per-line layout metadata for selection highlighting and clean copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineMeta {
    pub soft_wrap: bool,
    pub soft_space: bool,
    pub prefix_cols: u16,
}

/// Cached rendered transcript so scrolling and ticks don't re-wrap every frame.
pub struct CachedTranscript {
    pub width: u16,
    pub entries_len: usize,
    pub last_fp: u64,
    pub show_thoughts: bool,
    pub show_tools: bool,
    pub mouse_capture: bool,
    pub expanded_fp: u64,
    pub lines: Vec<ratatui::text::Line<'static>>,
    pub items: Vec<view::ItemSpan>,
    pub meta: Vec<LineMeta>,
}

/// Lines scrolled up from the bottom; 0 follows new output.
pub struct App {
    pub rt: Arc<Runtime>,
    pub cwd: PathBuf,
    pub session: Session,
    pub entries: Vec<Entry>,
    pub input: String,
    /// Cursor position in `input`, in bytes.
    pub cursor: usize,
    pub attachments: Vec<PathBuf>,
    pub running: Option<CancellationToken>,
    /// Lines scrolled up from the bottom; 0 follows new output.
    pub scroll: usize,
    /// Smooth visual scroll offset (in lines from bottom) used during animation.
    pub visual_scroll: f64,
    /// When true, `visual_scroll` eases toward `scroll` across frames.
    pub smooth_scroll: bool,
    /// First visible transcript line index from the last draw.
    pub last_start: usize,
    /// Mouse drag text selection in the conversation pane.
    pub text_sel: Option<TextSelection>,
    /// Cached transcript lines + metadata.
    pub transcript_cache: Option<CachedTranscript>,
    pub show_thoughts: bool,
    /// Every tool line, instead of runs of reads and searches as one line.
    pub show_tools: bool,
    /// The message selected with the keyboard (an entry index); None while
    /// typing.
    pub selected: Option<usize>,
    /// Scroll the selection into view at the next draw.
    pub reveal_selected: bool,
    /// Selectable entries in order, from the last draw.
    pub items: Vec<usize>,
    /// Entries shown in full: a tool's whole output, a group's every step.
    pub expanded: std::collections::HashSet<usize>,
    pub show_side: bool,
    pub modal: Option<Modal>,
    pub status: String,
    pub last_sent: String,
    pub cmd_sel: usize,
    pub input_history: Vec<String>,
    pub history_idx: Option<usize>,
    pub history_draft: String,
    /// The input when Esc closed its menu: the menu stays closed until the
    /// input changes.
    pub palette_dismissed: Option<String>,
    /// The input when ↑↓ last moved in its path menu: Tab then completes the
    /// row the user chose, never a row nobody picked.
    pub menu_moved: Option<String>,
    /// The last key was ^D at the bottom with nothing typed: another quits.
    pub quit_armed: bool,
    /// Rows the inline menu takes under the input (0 while it is closed):
    /// the empty-state card is laid out as if they were the conversation's,
    /// so opening the menu covers it instead of moving it.
    pub menu_rows: u16,
    /// Rows of Help's body in view, for PgUp/PgDn.
    pub help_page: usize,
    pub side_scroll: usize,
    pub side_max_scroll: usize,
    pub mouse_capture: bool,
    pub input_width: u16,
    pub conv_area: Rect,
    pub side_area: Rect,
    pub help_max_scroll: usize,
    pub consent_max_scroll: usize,
    pub tick: usize,
    /// When the running reply started.
    pub started: Option<std::time::Instant>,
    /// Conversation lines at the last draw, to hold a scrolled-up view still.
    pub last_total: usize,
    /// Scrolled up, and new lines arrived below.
    pub new_below: bool,
    /// The plan panel over the conversation, in a window too narrow to dock it.
    pub side_overlay: bool,
    /// The window's width at the last draw.
    pub screen_width: u16,
    /// Earlier threads in this folder, for the empty conversation.
    pub earlier_threads: usize,
    /// The status message on screen and when it came: it goes after
    /// `STATUS_FOR`.
    status_seen: Option<(String, std::time::Instant)>,
    /// Kinds of command the user allowed for the rest of this thread.
    pub always: std::collections::HashSet<String>,
    /// When this thread was opened here, and its turns since (usage events).
    thread_started: std::time::Instant,
    thread_turns: u32,
    tx: mpsc::UnboundedSender<AppEvent>,
    /// A browser sign-in waiting for its approval.
    sign_in: Option<tokio::task::AbortHandle>,
    /// The credentials the engine uses: signing out here reaches it too.
    pub creds: Arc<crate::auth::Credentials>,
    /// The running `/login` or `/logout` job, by number.
    login_task: Option<(u64, tokio::task::AbortHandle)>,
    /// The last `/login` job's number.
    login_jobs: u64,
    /// Open `/login` once the dialog in front closes (first run).
    pub login_pending: bool,
    /// Permission questions that came while the sign-in dialog was open:
    /// asked once it closes, so keys typed for a key never answer them.
    pending_asks: std::collections::VecDeque<Modal>,
    /// The model list is being refreshed in the background.
    pub refreshing_models: bool,
    /// The last background refresh asked for model lists and got none.
    pub models_unlisted: bool,
    /// The last note that a tier runs another model than the one asked for.
    pub model_note: Option<String>,
    pub quit: bool,
}

/// Runs the TUI in `cwd` until the user quits.
/// Puts the terminal back as it was: on quit, and from the panic hook.
pub fn restore_terminal() {
    print!("\x1b[?1007l");
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags).ok();
    }
    crossterm::execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    )
    .ok();
    ratatui::restore();
}

impl App {
    pub fn new(
        rt: Arc<Runtime>,
        cwd: PathBuf,
        session: Session,
        tx: mpsc::UnboundedSender<AppEvent>,
    ) -> Self {
        Self {
            rt,
            cwd,
            session,
            entries: Vec::new(),
            input: String::new(),
            cursor: 0,
            attachments: Vec::new(),
            running: None,
            scroll: 0,
            visual_scroll: 0.0,
            smooth_scroll: false,
            last_start: 0,
            text_sel: None,
            transcript_cache: None,
            show_thoughts: false,
            show_tools: false,
            selected: None,
            reveal_selected: false,
            items: Vec::new(),
            expanded: Default::default(),
            show_side: true,
            modal: None,
            status: String::new(),
            last_sent: String::new(),
            cmd_sel: 0,
            input_history: Vec::new(),
            history_idx: None,
            history_draft: String::new(),
            palette_dismissed: None,
            menu_moved: None,
            quit_armed: false,
            menu_rows: 0,
            help_page: 10,
            side_scroll: 0,
            side_max_scroll: 0,
            mouse_capture: true,
            input_width: 76,
            conv_area: Rect::default(),
            side_area: Rect::default(),
            help_max_scroll: 0,
            consent_max_scroll: 0,
            tick: 0,
            started: None,
            last_total: 0,
            new_below: false,
            side_overlay: false,
            screen_width: 0,
            earlier_threads: 0,
            status_seen: None,
            always: Default::default(),
            thread_started: std::time::Instant::now(),
            thread_turns: 0,
            tx,
            sign_in: None,
            creds: Arc::new(crate::auth::Credentials::system()),
            login_task: None,
            login_jobs: 0,
            login_pending: false,
            pending_asks: Default::default(),
            refreshing_models: false,
            models_unlisted: false,
            model_note: None,
            quit: false,
        }
    }

    /// Refreshes the model list in the background when the cache is empty
    /// or a day old: at most once per launch, never waiting for it.
    fn start_model_refresh(&mut self) {
        let cfg = self.rt.config();
        let creds = self.creds.clone();
        let plan =
            crate::catalog::plan_refresh(&cfg, &crate::catalog::ModelCache::load(), |route| {
                crate::catalog::signed_in(&creds, route)
            });
        if plan.is_empty() {
            return;
        }
        self.refreshing_models = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let outcome = crate::catalog::run_plan(&cfg, &creds, &plan).await;
            let _ = tx.send(AppEvent::ModelsRefreshed(outcome));
        });
    }

    fn on_models_refreshed(&mut self, outcome: crate::catalog::RefreshOutcome) {
        self.refreshing_models = false;
        self.models_unlisted = outcome.asked > 0 && outcome.listed == 0;
        self.note_model_fallback();
        if self.models_unlisted && self.model_note.is_none() {
            self.status = UNLISTED.into();
        }
    }

    /// Why a tier's model isn't listed, in a few words.
    fn missing_reason(&self) -> &'static str {
        if self.refreshing_models {
            " not listed yet"
        } else if self.models_unlisted {
            ": list refresh failed · /status"
        } else {
            " not offered · /model"
        }
    }

    /// Says once when a tier runs another model than the thread or config
    /// asks for, and clears that note once it no longer applies. Short, as
    /// it shares the footer with the model summary; /status says more.
    fn note_model_fallback(&mut self) {
        let falls: Vec<(&str, String, crate::config::ModelEntry)> = self
            .rt
            .tier_picks(&self.session)
            .into_iter()
            .zip(["Fast", "Main"])
            .filter(|(t, _)| t.fallback)
            .filter_map(|(t, name)| Some((name, t.wanted?, t.entry?)))
            .collect();
        let reason = self.missing_reason();
        let note = match falls.as_slice() {
            [] => None,
            [(name, wanted, using)] => Some(format!(
                "{name}: {} ({}) — {}{reason}",
                using.label,
                crate::config::Account::of(using).short(),
                bare_model(wanted),
            )),
            [(_, _, fast), (_, _, main)] => Some(format!(
                "Fast: {}, Main: {} — chosen models{reason}{}",
                fast.label,
                main.label,
                if reason.contains('·') {
                    ""
                } else {
                    " · /status"
                }
            )),
            _ => None,
        };
        if note == self.model_note {
            return;
        }
        if self.model_note.as_ref() == Some(&self.status) {
            self.status.clear();
        }
        if let Some(n) = &note {
            self.status = n.clone();
        }
        self.model_note = note;
    }

    fn check_models_on_startup(&mut self) {
        let empty = self.rt.model_choices(&self.session).catalog.is_empty();
        self.apply_startup_model_note(empty);
    }

    fn apply_startup_model_note(&mut self, catalog_empty: bool) {
        if catalog_empty {
            self.entries.push(Entry::Note(
                "No models are configured yet. /login connects an account or API key (as does `pathfinder auth login` in a terminal); then /model (Ctrl+P) picks one."
                    .into(),
            ));
            if self.status.is_empty() {
                self.status = "No models configured — /login connects one.".into();
            }
            // Only when nothing at all can run: a key the refresh is still
            // listing models for doesn't need it.
            if !crate::auth::accounts::any_model_account(&self.account_rows()) {
                self.login_pending = true;
            }
        }
    }

    /// The account list with each account's state and model count (as the
    /// pickers list them), for `/login`.
    fn account_rows(&self) -> Vec<crate::auth::accounts::RowView> {
        let cfg = self.rt.config();
        crate::auth::accounts::with_models(
            crate::auth::accounts::rows(
                &self.creds,
                &cfg,
                crate::config::CliAccounts::detect(&cfg),
                &crate::auth::accounts::process_env,
            ),
            &self.rt.model_choices(&self.session).catalog,
        )
    }

    /// What comes up once nothing is in front: a permission question that
    /// waited for the sign-in dialog, else the first-run `/login`.
    fn next_modal(&mut self) {
        if self.modal.is_some() {
            return;
        }
        if let Some(ask) = self.pending_asks.pop_front() {
            self.modal = Some(ask);
        } else if self.login_pending && self.running.is_none() {
            self.open_login(false, "");
        }
    }

    /// Opens `/login` (or `/logout`, with `logout`), or goes straight to a
    /// named account's step.
    fn open_login(&mut self, logout: bool, name: &str) {
        use crate::auth::flow::{Action, Flow, Mode};
        self.login_pending = false;
        if self.running.is_some() {
            // A tool may ask for permission meanwhile: its keys aren't for
            // a key field.
            self.status = format!(
                "Wait for the reply, or Esc to stop it, then /{}.",
                if logout { "logout" } else { "login" }
            );
            return;
        }
        let views = self.account_rows();
        // Only the first word is an account name; the rest is never shown.
        let name = name.split_whitespace().next().unwrap_or("");
        if !name.is_empty() {
            let cfg = self.rt.config();
            let Some(row) = crate::auth::accounts::parse_account(name, &cfg) else {
                self.status = format!(
                    "{} /{} alone picks.",
                    crate::auth::accounts::unknown_account(name),
                    if logout { "logout" } else { "login" }
                );
                return;
            };
            let mode = if logout { Mode::Logout } else { Mode::Login };
            let (flow, action) = Flow::at(mode, views, &row);
            self.modal = Some(Modal::Login(Box::new(flow)));
            if let Action::Run(job) = action {
                self.start_login_job(job);
            }
            return;
        }
        let flow = if logout {
            Flow::logout(views)
        } else {
            Flow::login(views)
        };
        if flow.is_empty() {
            self.status = flow.empty_text();
            return;
        }
        self.modal = Some(Modal::Login(Box::new(flow)));
    }

    /// The gate key's Settings row: its own steps in the sign-in panel. Not
    /// while a reply runs (a tool may ask for permission meanwhile).
    fn open_gate_key(&mut self) -> Option<Modal> {
        if self.running.is_some() {
            self.status = "Wait for the reply, or Esc to stop it, then set the gate key.".into();
            return Some(Modal::Settings { sel: gate_row() });
        }
        let flow = crate::auth::flow::Flow::gate(self.account_rows());
        Some(Modal::Login(Box::new(flow)))
    }

    /// What comes back once a sign-in flow closes: Settings for the gate
    /// key, which was opened from there; else nothing.
    fn after_login(mode: crate::auth::flow::Mode) -> Option<Modal> {
        (mode == crate::auth::flow::Mode::Gate).then(|| Modal::Settings { sel: gate_row() })
    }

    /// Runs a `/login` job in the background; it reports through
    /// [`AppEvent::Login`].
    fn start_login_job(&mut self, job: crate::auth::flow::Job) {
        self.stop_login_job();
        self.login_jobs += 1;
        let id = self.login_jobs;
        let tx = self.tx.clone();
        let creds = self.creds.clone();
        let task = tokio::spawn(async move {
            let url_tx = tx.clone();
            let result = crate::auth::connect::run(&creds, job, move |url| {
                let _ = url_tx.send(AppEvent::Login(LoginEvent::Url(id, url.to_string())));
            })
            .await
            .map_err(|e| crate::auth::flow::Failure::of(&e));
            let _ = tx.send(AppEvent::Login(LoginEvent::Done(id, result)));
        });
        self.login_task = Some((id, task.abort_handle()));
    }

    /// Stops the running `/login` job; what it says later is dropped.
    fn stop_login_job(&mut self) {
        if let Some((_, task)) = self.login_task.take() {
            task.abort();
        }
    }

    fn on_login_event(&mut self, ev: LoginEvent) {
        let current = self.login_task.as_ref().map(|(id, _)| *id);
        let id = match &ev {
            LoginEvent::Url(id, _) | LoginEvent::Done(id, _) => *id,
        };
        // From a job cancelled meanwhile, or replaced by a newer one.
        if current != Some(id) {
            return;
        }
        let Some(Modal::Login(flow)) = self.modal.as_mut() else {
            self.login_task = None;
            return;
        };
        match ev {
            LoginEvent::Url(_, url) => flow.set_url(&url),
            LoginEvent::Done(_, result) => {
                self.login_task = None;
                if !flow.busy() {
                    return;
                }
                let mode = flow.mode;
                match result {
                    Ok(msg) => {
                        self.modal = Self::after_login(mode);
                        self.entries.push(Entry::Note(msg));
                        self.scroll = 0;
                        // The model list changed: the footer and pickers
                        // read it again.
                        self.models_unlisted = false;
                        if self.model_note.is_none() && self.status.starts_with("No models") {
                            self.status.clear();
                        }
                        self.note_model_fallback();
                        if self.rt.model_choices(&self.session).catalog.is_empty() {
                            self.status =
                                "Connected; no models listed yet. /status shows the accounts."
                                    .into();
                        }
                    }
                    Err(e) => {
                        let text = e.text.clone();
                        if !flow.fail(e) {
                            self.modal = Self::after_login(mode);
                            self.status = text;
                        }
                    }
                }
            }
        }
    }
}

pub async fn run(
    rt: Arc<Runtime>,
    cwd: PathBuf,
    creds: Arc<crate::auth::Credentials>,
) -> Result<()> {
    let cwd = dunce::canonicalize(&cwd).unwrap_or(cwd);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = rt.create_session(uuid::Uuid::new_v4().to_string(), cwd.clone())?;
    let mut app = App::new(rt, cwd, session, tx);
    app.creds = creds;
    emit(UsageEvent::SessionStart {
        surface: pathfinder_schema::Surface::Tui,
    });
    {
        let tx = app.tx.clone();
        tokio::spawn(async move {
            if let Some(v) = crate::update::available().await {
                let _ = tx.send(AppEvent::UpdateAvailable(v));
            }
        });
    }
    if crate::telemetry::Consent::load().needs_answer() && !env_says_off() {
        let saved = crate::telemetry::Consent::load();
        app.modal = Some(Modal::Consent {
            // Asked again for a new notice: start from the old answers.
            events: saved.events,
            threads: saved.threads,
            step: ConsentStep::Choose,
            error: None,
            busy: false,
            scroll: 0,
        });
    }
    app.count_earlier_threads();
    app.check_models_on_startup();
    app.start_model_refresh();
    app.note_model_fallback();

    app.next_modal();
    let mut terminal = ratatui::init();
    // Each may be missing (a legacy Windows console has no bracketed
    // paste); the TUI works without either, and an early return here would
    // leave the terminal raw.
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);
    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
        && crossterm::execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
    }
    app.smooth_scroll = true;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(120));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut scroll_ticker = tokio::time::interval(std::time::Duration::from_millis(16));
    scroll_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut paste_until: Option<std::time::Instant> = None;
    let result: Result<()> = async {
        loop {
            terminal.draw(|f| view::draw(f, &mut app))?;
            tokio::select! {
                ev = events.next() => {
                    let mut batch = match ev {
                        Some(Ok(ev)) => vec![ev],
                        Some(Err(e)) => return Err(e.into()),
                        None => break,
                    };
                    // Drain queued terminal events (trackpad/wheel bursts, key
                    // repeats, Windows paste bursts) before redrawing so scrolling
                    // and typing never lag behind the event stream.
                    while batch.len() < 64 {
                        let next = std::future::poll_fn(|cx| {
                            std::task::Poll::Ready(events.poll_next_unpin(cx))
                        })
                        .await;
                        match next {
                            std::task::Poll::Ready(Some(Ok(ev))) => batch.push(ev),
                            std::task::Poll::Ready(Some(Err(e))) => return Err(e.into()),
                            _ => break,
                        }
                    }
                    let handled_paste = if cfg!(windows)
                        && batch.iter().any(|e| matches!(e, Event::Key(_)))
                    {
                        let now = std::time::Instant::now();
                        let continuing = paste_until.is_some_and(|t| now < t);
                        if let Some(text) = paste_burst(&batch, continuing) {
                            app.on_paste(&text);
                            paste_until = Some(now + PASTE_GAP);
                            true
                        } else {
                            paste_until = None;
                            false
                        }
                    } else {
                        false
                    };
                    if !handled_paste {
                        for ev in batch {
                            match ev {
                                Event::Key(key) if key.kind != KeyEventKind::Release => {
                                    app.on_key(key)
                                }
                                Event::Paste(text) => app.on_paste(&text),
                                Event::Mouse(mouse) => app.on_mouse(mouse),
                                _ => {}
                            }
                            if app.quit {
                                break;
                            }
                        }
                    }
                }
                Some(ev) = rx.recv() => app.on_event(ev),
                _ = scroll_ticker.tick(), if app.is_scroll_animating() => {
                    app.step_scroll_anim();
                }
                _ = ticker.tick(), if app.running.is_some() || !app.status.is_empty() => {
                    app.on_tick(std::time::Instant::now());
                }
            }
            if app.quit {
                break;
            }
            // A question that waited, or on a first run with no account
            // /login, once nothing is in front.
            app.next_modal();
        }
        Ok(())
    }
    .await;
    if let Some(cancel) = &app.running {
        cancel.cancel();
    }
    restore_terminal();
    print_transcript(&app);
    app.end_thread();
    result
}

use crate::telemetry::emit;
use pathfinder_schema::{Event as UsageEvent, Feature};

fn used(feature: Feature) {
    emit(UsageEvent::Ui { feature });
}

/// After quitting, the conversation goes into the terminal's own
/// scrollback, where it can be searched, selected and copied.
fn print_transcript(app: &App) {
    use crossterm::style::{Attribute, Color as C, Print, SetAttribute, SetForegroundColor};
    use ratatui::style::{Color, Modifier};
    if app.entries.is_empty() {
        return;
    }
    let width = crossterm::terminal::size().map_or(100, |(w, _)| w).min(120);
    let mut out = std::io::stdout();
    for line in view::transcript(app, width).0 {
        for span in &line.spans {
            let color = match span.style.fg {
                Some(Color::Green) => Some(C::Green),
                Some(Color::Red) => Some(C::Red),
                Some(Color::Cyan) => Some(C::Cyan),
                Some(Color::Yellow) => Some(C::Yellow),
                Some(Color::Magenta) => Some(C::Magenta),
                Some(Color::DarkGray) => Some(C::DarkGrey),
                _ => None,
            };
            if let Some(c) = color {
                let _ = crossterm::queue!(out, SetForegroundColor(c));
            }
            let m = span.style.add_modifier;
            for (flag, attr) in [
                (Modifier::BOLD, Attribute::Bold),
                (Modifier::DIM, Attribute::Dim),
                (Modifier::ITALIC, Attribute::Italic),
            ] {
                if m.contains(flag) {
                    let _ = crossterm::queue!(out, SetAttribute(attr));
                }
            }
            let _ = crossterm::queue!(
                out,
                Print(span.content.as_ref()),
                SetAttribute(Attribute::Reset)
            );
        }
        let _ = crossterm::queue!(out, Print("\r\n"));
    }
    let _ = std::io::Write::flush(&mut out);
}

/// How long after a paste further keys still belong to it.
/// How long a status message stays before the key hints come back.
pub const STATUS_FOR: std::time::Duration = std::time::Duration::from_secs(4);

const PASTE_GAP: std::time::Duration = std::time::Duration::from_millis(50);

/// A paste that arrived as key presses, as on Windows, where each line
/// break would otherwise be an Enter that sends half a message. Keys that
/// were waiting together are a paste when all are plain text and a line
/// break is among them, or when they follow a paste straight on.
fn paste_burst(batch: &[Event], continuing: bool) -> Option<String> {
    let mut text = String::new();
    let mut newline = false;
    for ev in batch {
        let Event::Key(key) = ev else {
            return None;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // AltGr arrives as Ctrl+Alt.
            KeyCode::Char(c) if ctrl == alt => text.push(c),
            KeyCode::Enter if !ctrl && !alt => {
                text.push('\n');
                newline = true;
            }
            KeyCode::Tab if !ctrl && !alt => text.push('\t'),
            _ => return None,
        }
    }
    let keys = text.chars().count();
    (keys > 0 && (continuing || (newline && keys >= 2))).then_some(text)
}

/// Puts `text` on the system clipboard: the platform's clipboard (the
/// Windows clipboard API, else pbcopy, wl-copy, xclip or xsel), else OSC 52,
/// which most terminals pass to the clipboard (over SSH too).
pub(crate) fn copy_to_clipboard(text: &str) -> std::result::Result<(), String> {
    use std::io::Write;
    // Not clip.exe: it reads stdin in the console code page and mangles
    // anything outside ASCII.
    #[cfg(windows)]
    if clipboard_win::set_clipboard_string(text).is_ok() {
        return Ok(());
    }
    for (cmd, args) in [
        ("pbcopy", &[][..]),
        ("wl-copy", &[][..]),
        ("xclip", &["-selection", "clipboard"][..]),
        ("xsel", &["--clipboard", "--input"][..]),
    ] {
        if let Ok(mut child) = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(text.as_bytes())
                    .map_err(|e| e.to_string())?;
            }
            if child.wait().is_ok_and(|s| s.success()) {
                return Ok(());
            }
        }
    }
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{encoded}\x07").map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

/// Asks in the terminal before the agent runs a command.
struct TuiHost {
    tx: mpsc::UnboundedSender<AppEvent>,
}

impl Host for TuiHost {
    fn ask<'a>(&'a self, call_id: &'a str, always_key: &'a str) -> BoxFuture<'a, Permission> {
        Box::pin(async move {
            let (reply, answer) = oneshot::channel();
            let sent = self.tx.send(AppEvent::Ask {
                call_id: call_id.to_string(),
                always_key: always_key.to_string(),
                reply,
            });
            if sent.is_err() {
                return Permission::Denied;
            }
            answer.await.unwrap_or(Permission::Denied)
        })
    }
}

/// The conversation a stored thread shows: each reply after the thoughts
/// and tool calls kept with it.
fn entries_of(session: &Session) -> Vec<Entry> {
    use crate::domain::Step;
    let mut asked = "";
    let mut out = Vec::new();
    for t in &session.history {
        if t.role == "user" {
            asked = &t.content;
            out.push(Entry::User(t.content.clone()));
            continue;
        }
        out.extend(t.steps.iter().map(|step| match step {
            Step::Thought { text } => Entry::Thought(text.clone()),
            Step::Tool {
                id,
                title,
                ok,
                detail,
            } => Entry::Tool {
                id: id.clone(),
                title: title.clone(),
                // Stopped before it ended: it didn't finish.
                ok: Some(ok.unwrap_or(false)),
                detail: detail.clone(),
            },
        }));
        out.push(stored_reply(asked, &t.content));
    }
    out
}

/// A stored reply as it showed live: `/open` and `/find` replies read back
/// into a source or hits (else a notice), the rest as text.
fn stored_reply(asked: &str, reply: &str) -> Entry {
    let command = asked.split_whitespace().next().unwrap_or("");
    match command {
        "/open" => match crate::sources::split_source(reply) {
            Some((title, range, body)) => Entry::Source { title, range, body },
            None => Entry::Note(reply.to_string()),
        },
        "/find" => {
            let hits: Vec<_> = reply
                .lines()
                .filter_map(crate::sources::parse_hit)
                .collect();
            if hits.is_empty() {
                Entry::Note(reply.to_string())
            } else {
                Entry::Search {
                    query: asked["/find".len()..].trim().to_string(),
                    total: crate::sources::find_total(reply)
                        .unwrap_or(0)
                        .max(hits.len()),
                    hits,
                }
            }
        }
        _ => Entry::Text(reply.to_string()),
    }
}

/// A thread's name: its goal, else its first message that isn't a
/// command, else the sources it opened (`Sources: README.md, gate.rs`),
/// else its first command.
pub fn thread_name(s: &Session) -> Option<String> {
    let said = s.goal.clone().or_else(|| {
        s.history
            .iter()
            .find(|t| t.role == "user" && !t.content.trim_start().starts_with('/'))
            .map(|t| t.content.clone())
    });
    if let Some(raw) = said {
        let one = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        return (!one.is_empty()).then_some(one);
    }
    let mut names: Vec<String> = Vec::new();
    for t in s.history.iter().filter(|t| t.role == "user") {
        let Some(arg) = ["/open ", "/attach ", "/check "]
            .iter()
            .find_map(|c| t.content.trim_start().strip_prefix(c))
        else {
            continue;
        };
        let path = commands::parse_path_arg(arg.trim());
        // `/open f.rs:20-24` and `/open f.pdf 3` name the file.
        let path = match path.rsplit_once(' ') {
            Some((p, page)) if !page.is_empty() && page.chars().all(|c| c.is_ascii_digit()) => p,
            _ => path.as_str(),
        };
        let path = match path.rsplit_once(':') {
            Some((p, lines)) if lines.chars().all(|c| c.is_ascii_digit() || c == '-') => p,
            _ => path,
        };
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.is_empty() && name != "clear" && !names.contains(&name) {
            names.push(name);
        }
    }
    if !names.is_empty() {
        return Some(format!("Sources: {}", names.join(", ")));
    }
    // Only other commands: the first, as typed (`/find wrap_hanging`).
    s.history
        .iter()
        .find(|t| t.role == "user")
        .map(|t| t.content.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
}

/// A thread's name in the list, on one line of at most 70 characters.
pub fn thread_title(s: &Session) -> String {
    let Some(one_line) = thread_name(s) else {
        return "(empty thread)".into();
    };
    let mut title: String = one_line.chars().take(70).collect();
    if one_line.chars().count() > 70 {
        title.push('…');
    }
    title
}

/// What the thread list shows beside a title: `chat · 12 · 2h`, the
/// mode, the messages you sent and how long ago it last changed.
pub fn thread_meta(s: &Session, age_secs: u64) -> String {
    let turns = s.history.iter().filter(|t| t.role == "user").count();
    format!("{} · {turns} · {}", view::mode_of(s), age(age_secs))
}

/// `now`, `5m`, `2h`, `3d`, `4mo`.
pub fn age(secs: u64) -> String {
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        86_400..5_184_000 => format!("{}d", secs / 86_400),
        _ => format!("{}mo", secs / 2_592_000),
    }
}

/// Settings with at most this many values change in place.
pub const CYCLE_MAX: usize = 6;

/// The current value's mark in a chooser.
const CURRENT_MARK: &str = "  ✓";

/// Where a chooser's cursor goes when its filter changes: it stays on the
/// current value (marked ✓) while that matches, else the best match.
/// Where the chooser's cursor goes for `query`: on the current value while
/// it matches as well as anything does (by its own name, never only by its
/// section), else on the best match.
fn chooser_sel(items: &[(String, String)], query: &str) -> usize {
    let q = query.trim().to_lowercase();
    let matched = filter_matches(items, query);
    let own = |i: usize| crate::picker::own_rank(items, i, &q);
    let best = matched.iter().filter_map(|&i| own(i)).min();
    let current = matched
        .iter()
        .position(|&i| !is_header(&items[i]) && items[i].1.ends_with(CURRENT_MARK));
    if let Some(at) = current
        && (q.is_empty() || own(matched[at]).is_some_and(|r| Some(r) <= best && r < 3))
    {
        return at;
    }
    best.and_then(|b| matched.iter().position(|&i| own(i) == Some(b)))
        .unwrap_or(0)
}

/// Files a message names that exist and can go to the model: a path the
/// terminal pasted when a file was dropped on it.
fn dropped_files(text: &str, cwd: &Path) -> Vec<PathBuf> {
    // Words, keeping backslash-escaped spaces (`my\ sheet.pdf`) inside.
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                word.push(' ');
                chars.next();
            }
            c if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
        .into_iter()
        .map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_string())
        .filter_map(|w| {
            let p = Path::new(&w);
            let p = if p.is_absolute() {
                p.to_path_buf()
            } else {
                cwd.join(p)
            };
            (p.is_file() && crate::llm::attachable_type(&p).is_some()).then_some(p)
        })
        .collect()
}

impl App {
    fn reload(&mut self) {
        if let Ok(Some(s)) = self.rt.load_session(&self.session.id) {
            self.session = s;
        }
    }

    fn is_path_arg(&self) -> bool {
        commands::path_arg_slice(&self.input).is_some()
    }

    /// Menus stay closed while ↑↓ walk the input history, and after Esc
    /// until the input changes.
    fn menu_hidden(&self) -> bool {
        self.history_idx.is_some() || self.palette_dismissed.as_deref() == Some(self.input.as_str())
    }

    /// Whether Enter runs `cmd`, the palette's selected row, as it is:
    /// typed in full, or a command without required arguments whose name
    /// starts with what was typed or that was picked with ↑↓. A looser
    /// match only fills in.
    fn enter_runs(&self, cmd: &commands::CommandInfo) -> bool {
        let typed = self.input.trim().to_ascii_lowercase();
        let moved = self.menu_moved.as_deref() == Some(self.input.as_str());
        !cmd.args.contains('<')
            && (cmd.name == typed
                || cmd.aliases.contains(&typed.as_str())
                || moved
                || commands::starts(cmd, &typed))
    }

    fn palette_open(&self) -> bool {
        !self.menu_hidden()
            && self.input.starts_with('/')
            && !self.input.contains(char::is_whitespace)
            && !commands::search_for(&self.input, Some(&self.session)).is_empty()
    }

    /// The files and folders the path menu of `/open`, `/check` or `/attach`
    /// shows; empty while it is closed.
    fn path_menu(&self) -> Vec<commands::AttachCandidate> {
        if self.menu_hidden() || self.input.contains('\n') {
            return Vec::new();
        }
        commands::path_matches(&self.input, &self.cwd)
    }

    /// The input with its unknown command word replaced by the closest one.
    fn unknown_fix(&self) -> Option<String> {
        let word = self.input.split(char::is_whitespace).next()?;
        if !word.starts_with('/') {
            return None;
        }
        let commands::Parsed::Unknown(_) = commands::parse(word) else {
            return None;
        };
        let closest = commands::closest(word)?;
        Some(format!("{closest}{}", &self.input[word.len()..]))
    }

    fn has_part_in_review(&self) -> bool {
        self.session
            .active_part()
            .is_some_and(|p| p.status == crate::domain::PartStatus::InReview)
            || self
                .session
                .map
                .parts
                .iter()
                .any(|p| p.status == crate::domain::PartStatus::InReview)
    }

    fn toggle_mouse_capture(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        self.text_sel = None;
        self.transcript_cache = None;
        if std::io::stdout().is_terminal() {
            if self.mouse_capture {
                let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
            } else {
                let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
            }
        }
        self.status = if self.mouse_capture {
            "Mouse scroll/click on (F2 for terminal text selection).".into()
        } else {
            "Terminal text selection on (F2 restores mouse scroll) · Tab then Y copies a source with its path.".into()
        };
    }

    fn on_paste(&mut self, text: &str) {
        let before = self.input.clone();
        self.paste(text);
        self.after_edit(&before);
    }

    /// An Esc dismissal and a row picked with ↑↓ hold only for the input
    /// they were made on: any edit forgets both.
    fn after_edit(&mut self, before: &str) {
        if self.input != before {
            self.palette_dismissed = None;
            self.menu_moved = None;
        }
    }

    fn paste(&mut self, text: &str) {
        let clean = text.replace("\r\n", "\n").replace('\r', "\n");
        self.selected = None;
        self.status.clear();
        if let Some(modal) = self.modal.as_mut() {
            match modal {
                Modal::Consent {
                    step: ConsentStep::Invite(code),
                    error,
                    busy: false,
                    scroll,
                    ..
                } => {
                    for c in clean.chars() {
                        if !c.is_control() && code.len() + c.len_utf8() <= 64 {
                            code.push(c);
                        }
                    }
                    *error = None;
                    *scroll = self.consent_max_scroll;
                }
                Modal::Login(flow) => flow.on_paste(text),
                Modal::Threads { query, sel, .. } => {
                    query.extend(clean.chars().filter(|c| !c.is_control()));
                    *sel = 0;
                }
                Modal::Choose {
                    items, query, sel, ..
                } => {
                    query.extend(clean.chars().filter(|c| !c.is_control()));
                    *sel = chooser_sel(items, query);
                }
                Modal::Help { query, scroll } => {
                    query.extend(clean.chars().filter(|c| !c.is_control()));
                    *scroll = 0;
                }
                _ => {}
            }
            return;
        }
        self.history_idx = None;
        self.history_draft.clear();
        self.cmd_sel = 0;
        self.input.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let rect_contains = |r: Rect, col: u16, row: u16| {
            r.width > 0
                && r.height > 0
                && col >= r.x
                && col < r.x + r.width
                && row >= r.y
                && row < r.y + r.height
        };
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = matches!(mouse.kind, MouseEventKind::ScrollUp);
                if let Some(modal) = self.modal.as_mut() {
                    match modal {
                        Modal::Permission { .. } => {
                            if up {
                                self.scroll = self.scroll.saturating_add(3);
                            } else {
                                self.scroll = self.scroll.saturating_sub(3);
                                if self.scroll == 0 {
                                    self.new_below = false;
                                }
                            }
                        }
                        Modal::Help { scroll, .. } => {
                            *scroll = if up {
                                scroll.saturating_sub(3)
                            } else {
                                (*scroll + 3).min(self.help_max_scroll)
                            };
                        }
                        Modal::Share { scroll, lines, .. } => {
                            let max = lines.len().saturating_sub(1);
                            *scroll = if up {
                                scroll.saturating_sub(3)
                            } else {
                                (*scroll + 3).min(max)
                            };
                        }
                        Modal::Consent { scroll, .. } => {
                            *scroll = if up {
                                scroll.saturating_sub(3)
                            } else {
                                (*scroll + 3).min(self.consent_max_scroll)
                            };
                        }
                        Modal::Threads {
                            items, query, sel, ..
                        } => {
                            let len = filter_matches(items, query).len();
                            *sel = if up {
                                sel.saturating_sub(1)
                            } else {
                                (*sel + 1).min(len.saturating_sub(1))
                            };
                        }
                        Modal::Choose {
                            items, query, sel, ..
                        } => {
                            let len = filter_matches(items, query).len();
                            *sel = if up {
                                sel.saturating_sub(1)
                            } else {
                                (*sel + 1).min(len.saturating_sub(1))
                            };
                        }
                        Modal::Settings { sel } => {
                            *sel = if up {
                                sel.saturating_sub(1)
                            } else {
                                (*sel + 1).min(SETTINGS.len().saturating_sub(1))
                            };
                        }
                        Modal::Login(flow) => {
                            let code = if up { KeyCode::Up } else { KeyCode::Down };
                            flow.on_key(KeyEvent::new(code, KeyModifiers::NONE));
                        }
                    }
                    return;
                }
                if (self.show_side || self.side_overlay)
                    && rect_contains(self.side_area, mouse.column, mouse.row)
                {
                    self.side_scroll = if up {
                        self.side_scroll.saturating_sub(3)
                    } else {
                        (self.side_scroll + 3).min(self.side_max_scroll)
                    };
                } else if up {
                    self.scroll = self.scroll.saturating_add(3);
                } else {
                    self.scroll = self.scroll.saturating_sub(3);
                    if self.scroll == 0 {
                        self.new_below = false;
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) if self.modal.is_none() => {
                if rect_contains(self.conv_area, mouse.column, mouse.row) {
                    self.selected = None;
                    if let Some(pos) = self.mouse_to_text_pos(mouse.column, mouse.row) {
                        self.text_sel = Some(TextSelection {
                            anchor: pos,
                            head: pos,
                            dragging: true,
                        });
                    }
                } else {
                    self.selected = None;
                    self.text_sel = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.modal.is_none() => {
                if let Some(mut sel) = self.text_sel
                    && sel.dragging
                    && self.conv_area.height > 0
                    && self.conv_area.width > 2
                {
                    if mouse.row < self.conv_area.y {
                        self.scroll = self.scroll.saturating_add(1);
                    } else if mouse.row >= self.conv_area.y + self.conv_area.height {
                        self.scroll = self.scroll.saturating_sub(1);
                        if self.scroll == 0 {
                            self.new_below = false;
                        }
                    }
                    let clamped_col = mouse
                        .column
                        .clamp(self.conv_area.x + 1, self.conv_area.x + self.conv_area.width - 2);
                    let clamped_row = mouse
                        .row
                        .clamp(self.conv_area.y, self.conv_area.y + self.conv_area.height - 1);
                    if let Some(pos) = self.mouse_to_text_pos(clamped_col, clamped_row) {
                        sel.head = pos;
                        self.text_sel = Some(sel);
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.modal.is_none() => {
                if let Some(mut sel) = self.text_sel
                    && sel.dragging
                {
                    sel.dragging = false;
                    if sel.is_empty() {
                        self.text_sel = None;
                    } else {
                        self.text_sel = Some(sel);
                    }
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Modifier(_)) {
            return;
        }
        let before = self.input.clone();
        let armed = std::mem::take(&mut self.quit_armed);
        self.key(key, armed);
        self.after_edit(&before);
    }

    fn key(&mut self, key: KeyEvent, quit_armed: bool) {
        self.status.clear();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if self.modal.is_some() {
            self.on_modal_key(key);
            return;
        }
        if self.selected.is_some() && self.on_nav_key(key) {
            return;
        }
        match key.code {
            KeyCode::F(2) => self.toggle_mouse_capture(),
            KeyCode::Up if ctrl && !self.items.is_empty() => {
                used(Feature::Navigate);
                self.text_sel = None;
                self.selected = self.items.last().copied();
                self.reveal_selected = true;
            }
            KeyCode::Tab if self.is_path_arg() && !self.input.contains('\n') => {
                // Tab opens a closed menu; it completes the one match, the
                // matches' common start, or the row picked with ↑↓, and
                // otherwise leaves the menu open to pick from.
                let hidden = self.menu_hidden();
                self.palette_dismissed = None;
                if hidden && self.history_idx.is_none() {
                    return;
                }
                self.history_idx = None;
                self.history_draft.clear();
                let matches = self.path_menu();
                let picked = self.menu_moved.as_deref() == Some(self.input.as_str());
                let completed = if matches.is_empty() {
                    None
                } else if picked {
                    commands::complete_path_at(&self.input, self.cmd_sel, &self.cwd)
                } else {
                    commands::complete_path(&self.input, &self.cwd)
                };
                match completed {
                    Some(done) => {
                        self.input = done;
                        self.cursor = self.input.len();
                        self.cmd_sel = 0;
                    }
                    None if matches.len() > 1 => {
                        self.status = format!("{} matches · ↑↓ picks one", matches.len());
                    }
                    None => {}
                }
            }
            KeyCode::Tab if !self.palette_open() && self.unknown_fix().is_some() => {
                if let Some(fixed) = self.unknown_fix() {
                    self.cursor = fixed.find(char::is_whitespace).unwrap_or(fixed.len());
                    self.input = fixed;
                    self.cmd_sel = 0;
                    self.history_idx = None;
                    self.history_draft.clear();
                }
            }
            KeyCode::Tab
                if self.input.starts_with('/') && !self.input.contains(char::is_whitespace) =>
            {
                self.palette_dismissed = None;
                let matches = commands::search_for(&self.input, Some(&self.session));
                if !matches.is_empty()
                    && let Some(done) =
                        commands::complete_at(&self.input, self.cmd_sel, Some(&self.session))
                {
                    self.input = done;
                    self.cursor = self.input.len();
                    self.cmd_sel = 0;
                    self.history_idx = None;
                    self.history_draft.clear();
                }
            }
            KeyCode::Tab if self.input.is_empty() && !self.items.is_empty() => {
                used(Feature::Navigate);
                self.text_sel = None;
                self.selected = self.items.last().copied();
                self.reveal_selected = true;
            }
            KeyCode::Tab => {}
            KeyCode::Char('y') if !ctrl && !alt && self.input.is_empty() && self.has_text_selection() => {
                self.copy_text_selection();
            }
            KeyCode::Char('c') if ctrl => {
                if self.has_text_selection() {
                    self.copy_text_selection();
                } else {
                    match &self.running {
                        Some(cancel) => cancel.cancel(),
                        None => self.quit = true,
                    }
                }
            }
            // With nothing typed: half a page down; at the bottom a second
            // ^D in a row quits, so paging down never ends the session.
            KeyCode::Char('d') if ctrl => {
                if !self.input.is_empty() {
                    self.delete_char_forward();
                } else if self.scroll > 0 {
                    self.scroll = self.scroll.saturating_sub(self.half_page());
                    if self.scroll == 0 {
                        self.new_below = false;
                    }
                } else if quit_armed {
                    self.quit = true;
                } else {
                    self.quit_armed = true;
                    self.status = "Press ^D again to quit".into();
                }
            }
            // With nothing typed: half a page up; else it clears the line.
            KeyCode::Char('u') if ctrl && self.input.is_empty() => {
                self.scroll = self.scroll.saturating_add(self.half_page());
            }
            KeyCode::Char('n') if ctrl => self.new_thread(),
            KeyCode::Char('o') if ctrl => {
                used(Feature::Threads);
                self.open_threads();
            }
            KeyCode::Char('p') if ctrl => {
                used(Feature::Settings);
                self.modal = Some(Modal::Settings { sel: 0 });
            }
            KeyCode::Char('t') if ctrl => self.run_local(commands::Local::Thinking, ""),
            KeyCode::Char('g') if ctrl => self.run_local(commands::Local::Tools, ""),
            KeyCode::Char('b') if ctrl && !self.mouse_capture => {
                self.status =
                    "The plan panel stays hidden while you select text · F2 ends it.".into();
            }
            KeyCode::Char('b') if ctrl => {
                used(Feature::SidePanel);
                // Docked, it hides and shows again; otherwise (too narrow,
                // or nothing to dock yet) it opens over the conversation.
                let wide = self.screen_width >= view::DOCK_FROM;
                if wide && view::panel_earns(self) {
                    self.show_side = !self.show_side;
                    self.side_overlay = false;
                } else {
                    self.side_overlay = !self.side_overlay;
                }
                self.side_scroll = 0;
            }
            KeyCode::Char('j') if ctrl => self.insert_char('\n'),
            KeyCode::F(1) => {
                used(Feature::Help);
                self.modal = Some(Modal::help());
            }
            // Esc closes what is open, then stops a running reply. It never
            // clears the draft (Ctrl+U does).
            KeyCode::Esc if self.text_sel.is_some() => self.text_sel = None,
            KeyCode::Esc if self.side_overlay => self.side_overlay = false,
            KeyCode::Esc if self.palette_open() || !self.path_menu().is_empty() => {
                self.palette_dismissed = Some(self.input.clone());
                self.cmd_sel = 0;
            }
            // Then, with nothing running and nothing typed, it answers what
            // you said in plain words (§15): it cancels what waits for your
            // OK, or takes back the claim you just made.
            KeyCode::Esc => {
                if let Some(cancel) = &self.running {
                    cancel.cancel();
                } else if self.input.trim().is_empty() {
                    if self.session.pending_commitment.is_some() {
                        self.run_task_shortcut("/cancel");
                    } else if self.session.undoable() {
                        self.run_task_shortcut("/undo");
                    }
                }
            }
            KeyCode::Enter if alt || shift => self.insert_char('\n'),
            // Enter on an empty input runs what waits for your OK (§15).
            KeyCode::Enter
                if self.input.trim().is_empty()
                    && self.running.is_none()
                    && self.session.pending_commitment.is_some() =>
            {
                self.run_task_shortcut("/confirm");
            }
            KeyCode::Enter => {
                let menu = self.path_menu();
                if let Some((cmd, arg)) = commands::path_arg_slice(&self.input)
                    && !menu.is_empty()
                {
                    // A path that is already whole runs; otherwise the picked
                    // row: a folder opens in the menu, a file fills in (and
                    // `/open` and `/attach` run it).
                    let arg = arg.trim();
                    let ready = (cmd == "/attach" && arg.eq_ignore_ascii_case("clear"))
                        || commands::resolve_attach_path(arg, &self.cwd)
                            .is_some_and(|p| p.is_file());
                    if ready {
                        self.submit();
                    } else if let Some(cand) = menu.get(self.cmd_sel.min(menu.len() - 1)) {
                        self.input = format!("{cmd} {}", cand.arg);
                        self.cursor = self.input.len();
                        self.cmd_sel = 0;
                        if !cand.is_dir && cmd != "/check" {
                            self.submit();
                        }
                    }
                } else if self.palette_open() {
                    let matches = commands::search_for(&self.input, Some(&self.session));
                    let typed = self.input.trim().to_ascii_lowercase();
                    if let Some(cmd) = matches.get(self.cmd_sel).copied() {
                        if cmd.name == typed || cmd.aliases.contains(&typed.as_str()) {
                            self.cmd_sel = 0;
                            self.submit();
                        } else if cmd.args.contains('<') {
                            self.input = format!("{} ", cmd.name);
                            self.cursor = self.input.len();
                            self.cmd_sel = 0;
                            self.history_idx = None;
                            self.history_draft.clear();
                        } else if self.enter_runs(&cmd) {
                            self.input = cmd.name.to_string();
                            self.cursor = self.input.len();
                            self.cmd_sel = 0;
                            self.submit();
                        } else {
                            // A loose match (a word, letters in order, the
                            // description) only fills in: Enter again runs
                            // it, so a typo never starts a model turn.
                            self.input = cmd.name.to_string();
                            self.cursor = self.input.len();
                            self.cmd_sel = 0;
                            self.history_idx = None;
                            self.history_draft.clear();
                        }
                    } else {
                        self.submit();
                    }
                } else {
                    self.submit();
                }
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(self.page_rows()),
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(self.page_rows());
                if self.scroll == 0 {
                    self.new_below = false;
                }
            }
            KeyCode::Up if shift => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Down if shift => {
                self.scroll = self.scroll.saturating_sub(1);
                if self.scroll == 0 {
                    self.new_below = false;
                }
            }
            KeyCode::Up if alt => {
                if !self.last_sent.is_empty() {
                    self.input = self.last_sent.clone();
                    self.cursor = self.input.len();
                    self.cmd_sel = 0;
                }
            }
            KeyCode::Up if !ctrl && !alt => {
                let menu = self.path_menu().len();
                if menu > 0 {
                    self.cmd_sel = commands::step_selection(self.cmd_sel, menu, -1);
                    self.menu_moved = Some(self.input.clone());
                    return;
                } else if self.palette_open() {
                    let len = commands::search_for(&self.input, Some(&self.session)).len();
                    if len > 0 {
                        self.cmd_sel = commands::step_selection(self.cmd_sel, len, -1);
                        self.menu_moved = Some(self.input.clone());
                        return;
                    }
                }
                let (_, cur_row, cur_col) =
                    markdown::wrap_input(&self.input, self.cursor, self.input_width.max(10));
                if cur_row > 0 {
                    self.move_cursor_visual_row(cur_row - 1, cur_col);
                } else {
                    self.history_prev();
                }
            }
            KeyCode::Down if !ctrl && !alt => {
                let menu = self.path_menu().len();
                if menu > 0 {
                    self.cmd_sel = commands::step_selection(self.cmd_sel, menu, 1);
                    self.menu_moved = Some(self.input.clone());
                    return;
                } else if self.palette_open() {
                    let len = commands::search_for(&self.input, Some(&self.session)).len();
                    if len > 0 {
                        self.cmd_sel = commands::step_selection(self.cmd_sel, len, 1);
                        self.menu_moved = Some(self.input.clone());
                        return;
                    }
                }
                let (lines, cur_row, cur_col) =
                    markdown::wrap_input(&self.input, self.cursor, self.input_width.max(10));
                if (cur_row as usize) + 1 < lines.len() {
                    self.move_cursor_visual_row(cur_row + 1, cur_col);
                } else {
                    self.history_next();
                }
            }
            KeyCode::Home if ctrl => {
                if self.input.is_empty() {
                    self.scroll = self.last_total;
                } else {
                    self.cursor = 0;
                }
            }
            KeyCode::End if ctrl => {
                self.scroll = 0;
                self.new_below = false;
                if !self.input.is_empty() {
                    self.cursor = self.input.len();
                }
            }
            KeyCode::End if self.input.is_empty() => {
                self.scroll = 0;
                self.new_below = false;
            }
            // Line editing, the way shells / readline do it.
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Char('u') if ctrl => {
                let start = self.line_start();
                self.input.replace_range(start..self.cursor, "");
                self.cursor = start;
                self.history_idx = None;
                self.history_draft.clear();
                self.cmd_sel = 0;
            }
            KeyCode::Char('k') if ctrl => {
                let end = self.line_end();
                self.input.replace_range(self.cursor..end, "");
                self.history_idx = None;
                self.history_draft.clear();
                self.cmd_sel = 0;
            }
            KeyCode::Char('w') if ctrl => self.delete_word_back(),
            KeyCode::Backspace if alt || ctrl => self.delete_word_back(),
            KeyCode::Delete if alt || ctrl => self.delete_word_forward(),
            KeyCode::Left if alt || ctrl => self.cursor = self.word_left(),
            KeyCode::Right if alt || ctrl => self.cursor = self.word_right(),
            KeyCode::Char('b') if alt && !ctrl => self.cursor = self.word_left(),
            KeyCode::Char('f') if alt && !ctrl => self.cursor = self.word_right(),
            KeyCode::Char('d') if alt && !ctrl => self.delete_word_forward(),
            // Contextual quick actions while waiting on a change, review, or candidate.
            KeyCode::Char('a')
                if alt
                    && !ctrl
                    && self.running.is_none()
                    && self.session.pending_change.is_some() =>
            {
                self.run_task_shortcut("/accept");
            }
            KeyCode::Char('r')
                if alt
                    && !ctrl
                    && self.running.is_none()
                    && self.session.pending_change.is_some() =>
            {
                self.run_task_shortcut("/reject");
            }
            KeyCode::Char('a')
                if alt && !ctrl && self.running.is_none() && self.has_part_in_review() =>
            {
                self.run_task_shortcut("/approve");
            }
            KeyCode::Char('o')
                if alt && !ctrl && self.running.is_none() && self.has_part_in_review() =>
            {
                self.input = "/object ".into();
                self.cursor = self.input.len();
                self.cmd_sel = 0;
                self.history_idx = None;
                self.history_draft.clear();
            }
            KeyCode::Char('y')
                if alt
                    && !ctrl
                    && self.running.is_none()
                    && self.session.pending_candidate.is_some()
                    && !self.session.exam =>
            {
                self.input = "Right, because ".into();
                self.cursor = self.input.len();
                self.cmd_sel = 0;
                self.history_idx = None;
                self.history_draft.clear();
            }
            KeyCode::Char('n')
                if alt
                    && !ctrl
                    && self.running.is_none()
                    && self.session.pending_candidate.is_some()
                    && !self.session.exam =>
            {
                self.input = "Flawed, because ".into();
                self.cursor = self.input.len();
                self.cmd_sel = 0;
                self.history_idx = None;
                self.history_draft.clear();
            }
            // Guard against stray characters from unhandled Ctrl or Alt combos.
            KeyCode::Char(_) if ctrl || alt => {}
            KeyCode::Char(c) => self.insert_char(c),
            KeyCode::Backspace => {
                if self.input.is_empty() && !self.attachments.is_empty() {
                    if let Some(removed) = self.attachments.pop() {
                        let name = removed
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| removed.display().to_string());
                        self.status = format!("Removed attachment {name}.");
                    }
                } else if let Some((i, _)) = self.input[..self.cursor].char_indices().next_back() {
                    self.input.replace_range(i..self.cursor, "");
                    self.cursor = i;
                    self.history_idx = None;
                    self.history_draft.clear();
                    self.cmd_sel = 0;
                }
            }
            KeyCode::Delete => self.delete_char_forward(),
            KeyCode::Left => {
                if let Some((i, _)) = self.input[..self.cursor].char_indices().next_back() {
                    self.cursor = i;
                }
            }
            KeyCode::Right => {
                if let Some(ch) = self.input[self.cursor..].chars().next() {
                    self.cursor += ch.len_utf8();
                }
            }
            _ => {}
        }
    }

    /// Keys while a message is selected. False hands the key on to the
    /// input, after leaving the selection.
    fn on_nav_key(&mut self, key: KeyEvent) -> bool {
        let Some(sel) = self.selected else {
            return false;
        };
        let pos = self.items.iter().position(|i| *i == sel).unwrap_or(0);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let go = |app: &mut Self, at: usize| {
            if let Some(i) = app.items.get(at) {
                app.selected = Some(*i);
                app.reveal_selected = true;
            }
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if !ctrl => go(self, pos.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') if !ctrl => go(self, pos + 1),
            KeyCode::Char('g') => go(self, 0),
            KeyCode::Char('G') => go(self, self.items.len().saturating_sub(1)),
            KeyCode::PageUp => go(self, pos.saturating_sub(5)),
            KeyCode::PageDown => go(self, pos + 5),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if !self.expanded.remove(&sel) {
                    self.expanded.insert(sel);
                }
                self.reveal_selected = true;
            }
            KeyCode::Char('y') if !ctrl => {
                let text = self.entry_text(sel);
                used(Feature::Copy);
                self.status = match copy_to_clipboard(&text) {
                    Ok(()) => format!("Copied {} characters.", text.chars().count()),
                    Err(e) => format!("Couldn't copy: {e}"),
                };
            }
            // A source with its `path:lines` above the code.
            KeyCode::Char('Y') if !ctrl => {
                let text = self.entry_text_placed(sel);
                used(Feature::Copy);
                self.status = match copy_to_clipboard(&text) {
                    Ok(()) => format!("Copied {} characters.", text.chars().count()),
                    Err(e) => format!("Couldn't copy: {e}"),
                };
            }
            KeyCode::Char('c') if ctrl => {
                let text = self.entry_text(sel);
                used(Feature::Copy);
                self.status = match copy_to_clipboard(&text) {
                    Ok(()) => format!("Copied {} characters.", text.chars().count()),
                    Err(e) => format!("Couldn't copy: {e}"),
                };
            }
            KeyCode::Esc | KeyCode::Tab => {
                self.selected = None;
            }
            // Ctrl keys (stop, settings, quit) work as everywhere.
            _ if ctrl => {
                self.selected = None;
                return false;
            }
            // Typing goes back to the input.
            KeyCode::Char(_) | KeyCode::Backspace => {
                self.selected = None;
                return false;
            }
            _ => {}
        }
        true
    }

    /// An entry as plain text, for the clipboard: a group copies each step.
    fn entry_text(&self, index: usize) -> String {
        let one = Entry::copy_text;
        let Some(first) = self.entries.get(index) else {
            return String::new();
        };
        // A folded group: this entry and the reads and searches after it.
        let next_shown = self.items.iter().find(|i| **i > index).copied();
        if matches!(first, Entry::Tool { .. })
            && !self.expanded.contains(&index)
            && !self.show_tools
        {
            let end = next_shown.unwrap_or(self.entries.len());
            return self.entries[index..end]
                .iter()
                .filter(|e| matches!(e, Entry::Tool { .. }))
                .map(one)
                .collect::<Vec<_>>()
                .join("\n");
        }
        one(first)
    }

    /// What `Y` copies: a source under its `path:lines`, else as `y`.
    fn entry_text_placed(&self, index: usize) -> String {
        match self.entries.get(index) {
            Some(e @ Entry::Source { .. }) => e.copy_with_place(),
            _ => self.entry_text(index),
        }
    }

    pub fn is_scroll_animating(&self) -> bool {
        self.smooth_scroll && (self.visual_scroll - self.scroll as f64).abs() >= 0.5
    }

    pub fn step_scroll_anim(&mut self) {
        let target = self.scroll as f64;
        let diff = target - self.visual_scroll;
        if diff.abs() < 0.5 {
            self.visual_scroll = target;
            return;
        }
        let mut step = diff * 0.45;
        if step.abs() < 1.0 {
            step = step.signum();
        }
        self.visual_scroll += step;
        if (target - self.visual_scroll).abs() < 0.5 {
            self.visual_scroll = target;
        }
    }

    pub fn has_text_selection(&self) -> bool {
        self.text_sel.is_some_and(|s| !s.is_empty())
    }

    fn copy_text_selection(&mut self) {
        let text = self.selected_text();
        if text.is_empty() {
            self.text_sel = None;
            return;
        }
        used(Feature::Copy);
        self.status = match copy_to_clipboard(&text) {
            Ok(()) => format!("Copied {} characters.", text.chars().count()),
            Err(e) => format!("Couldn't copy: {e}"),
        };
        self.text_sel = None;
    }

    fn mouse_to_text_pos(&self, col: u16, row: u16) -> Option<TextPos> {
        if self.conv_area.width <= 2 || self.conv_area.height == 0 || self.last_total == 0 {
            return None;
        }
        let rel_row = row.saturating_sub(self.conv_area.y) as usize;
        let line = (self.last_start + rel_row).min(self.last_total.saturating_sub(1));
        let max_col = self.conv_area.width.saturating_sub(3);
        let content_col = col.saturating_sub(self.conv_area.x + 1).min(max_col);
        Some(TextPos {
            line,
            col: content_col,
        })
    }

    pub fn selected_text(&self) -> String {
        let Some(sel) = self.text_sel.filter(|s| !s.is_empty()) else {
            return String::new();
        };
        let (start_pos, end_pos) = sel.ordered();
        let width = self.conv_area.width.saturating_sub(2).max(1);
        let owned;
        let (lines, meta): (&[ratatui::text::Line<'static>], &[LineMeta]) =
            match &self.transcript_cache {
                Some(c) if c.width == width => (&c.lines, &c.meta),
                _ => {
                    owned = view::transcript_with_meta(self, width);
                    (&owned.0, &owned.2)
                }
            };
        if lines.is_empty() {
            return String::new();
        }
        let last_line = end_pos.line.min(lines.len() - 1);
        let mut out = String::new();
        for idx in start_pos.line..=last_line {
            let prefix = meta.get(idx).map_or(0, |m| m.prefix_cols);
            let from_col = if idx == start_pos.line {
                start_pos.col.max(prefix)
            } else {
                prefix
            };
            let to_col = if idx == end_pos.line {
                Some(end_pos.col)
            } else {
                None
            };
            let piece = if to_col.is_some_and(|t| t < from_col) {
                String::new()
            } else {
                Self::slice_line_cols(&lines[idx], from_col, to_col)
            };
            if idx == start_pos.line {
                out.push_str(&piece);
            } else if meta.get(idx).is_some_and(|m| m.soft_wrap) {
                if meta[idx].soft_space && !out.ends_with(' ') && !piece.is_empty() {
                    out.push(' ');
                }
                out.push_str(&piece);
            } else {
                out.push('\n');
                out.push_str(&piece);
            }
        }
        out
    }

    pub fn cached_transcript(&mut self, width: u16) -> &CachedTranscript {
        let entries_len = self.entries.len();
        let last_fp = self.entries.last().map_or(0, Self::entry_fingerprint);
        let expanded_fp = {
            let mut h = self.expanded.len() as u64;
            for &i in &self.expanded {
                h = h.wrapping_mul(31).wrapping_add(i as u64 + 1);
            }
            h
        };
        let valid = self.transcript_cache.as_ref().is_some_and(|c| {
            c.width == width
                && c.entries_len == entries_len
                && c.last_fp == last_fp
                && c.show_thoughts == self.show_thoughts
                && c.show_tools == self.show_tools
                && c.mouse_capture == self.mouse_capture
                && c.expanded_fp == expanded_fp
        });
        if !valid {
            let (lines, items, meta) = view::transcript_with_meta(self, width);
            self.transcript_cache = Some(CachedTranscript {
                width,
                entries_len,
                last_fp,
                show_thoughts: self.show_thoughts,
                show_tools: self.show_tools,
                mouse_capture: self.mouse_capture,
                expanded_fp,
                lines,
                items,
                meta,
            });
        }
        self.transcript_cache.as_ref().unwrap()
    }

    fn entry_fingerprint(entry: &Entry) -> u64 {
    match entry {
        Entry::User(s) | Entry::Text(s) | Entry::Thought(s) | Entry::Note(s) => s.len() as u64,
        Entry::Tool {
            title, ok, detail, ..
        } => {
            let status = match ok {
                None => 1,
                Some(false) => 2,
                Some(true) => 3,
            };
            (title.len() as u64)
                .wrapping_mul(131)
                .wrapping_add(detail.len() as u64)
                .wrapping_mul(7)
                .wrapping_add(status)
        }
        Entry::Source { body, .. } => body.len() as u64,
        Entry::Search { hits, total, .. } => (hits.len() as u64).wrapping_mul(31).wrapping_add(*total as u64),
        Entry::Info(rows) => rows.len() as u64,
    }
}

    fn slice_line_cols(line: &ratatui::text::Line<'_>, from_col: u16, to_col: Option<u16>) -> String {
    use unicode_width::UnicodeWidthChar as _;
    let mut out = String::new();
    let mut col: u16 = 0;
    for span in &line.spans {
        for ch in span.content.chars() {
            let w = ch.width().unwrap_or(0) as u16;
            if let Some(end) = to_col
                && col > end
            {
                return out;
            }
            if col >= from_col {
                out.push(ch);
            }
            col = col.saturating_add(w);
        }
    }
    out
}

    fn line_start(&self) -> usize {
        self.input[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.input[self.cursor..]
            .find('\n')
            .map_or(self.input.len(), |i| self.cursor + i)
    }

    /// Where the word before the cursor starts (skipping spaces first).
    fn word_left(&self) -> usize {
        let before = &self.input[..self.cursor];
        let trimmed = before.trim_end();
        trimmed
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8())
    }

    /// Where the word after the cursor ends.
    fn word_right(&self) -> usize {
        let after = &self.input[self.cursor..];
        let skip = after.len() - after.trim_start().len();
        let rest = &after[skip..];
        let word = rest.find(char::is_whitespace).unwrap_or(rest.len());
        self.cursor + skip + word
    }

    fn delete_char_forward(&mut self) {
        if let Some(ch) = self.input[self.cursor..].chars().next() {
            self.input
                .replace_range(self.cursor..self.cursor + ch.len_utf8(), "");
            self.history_idx = None;
            self.history_draft.clear();
            self.cmd_sel = 0;
        }
    }

    fn delete_word_back(&mut self) {
        let start = self.word_left();
        self.input.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.history_idx = None;
        self.history_draft.clear();
        self.cmd_sel = 0;
    }

    fn delete_word_forward(&mut self) {
        let end = self.word_right();
        self.input.replace_range(self.cursor..end, "");
        self.history_idx = None;
        self.history_draft.clear();
        self.cmd_sel = 0;
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.history_idx = None;
        self.history_draft.clear();
        self.cmd_sel = 0;
    }

    fn move_cursor_visual_row(&mut self, target_row: u16, target_col: u16) {
        let w = self.input_width.max(1);
        let bounds: Vec<usize> = self
            .input
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(self.input.len()))
            .collect();
        let row_col = |pos: usize| {
            let (_, r, c) = markdown::wrap_input(&self.input, pos, w);
            (r, c)
        };
        let lo = bounds.partition_point(|&p| row_col(p).0 < target_row);
        let hi = bounds.partition_point(|&p| row_col(p).0 <= target_row);
        if lo >= hi {
            return;
        }
        let slice = &bounds[lo..hi];
        let idx = slice.partition_point(|&p| row_col(p).1 <= target_col);
        self.cursor = if idx > 0 { slice[idx - 1] } else { slice[0] };
    }

    /// PageUp/PageDown: the visible conversation less two rows of context.
    fn page_rows(&self) -> usize {
        (self.conv_area.height as usize).saturating_sub(2).max(1)
    }

    /// Ctrl+U/Ctrl+D with nothing typed: half the visible conversation.
    fn half_page(&self) -> usize {
        (self.conv_area.height as usize / 2).max(1)
    }

    fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let next_idx = match self.history_idx {
            None => {
                self.history_draft = std::mem::take(&mut self.input);
                self.input_history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.history_idx = Some(next_idx);
        self.input = self.input_history[next_idx].clone();
        self.cursor = self.input.len();
        self.cmd_sel = 0;
    }

    fn history_next(&mut self) {
        let Some(idx) = self.history_idx else {
            return;
        };
        if idx + 1 < self.input_history.len() {
            let next_idx = idx + 1;
            self.history_idx = Some(next_idx);
            self.input = self.input_history[next_idx].clone();
        } else {
            self.history_idx = None;
            self.input = std::mem::take(&mut self.history_draft);
        }
        self.cursor = self.input.len();
        self.cmd_sel = 0;
    }

    fn run_task_shortcut(&mut self, cmd: &str) {
        self.input = cmd.to_string();
        self.cursor = self.input.len();
        self.cmd_sel = 0;
        let shown = self.shortcut_shown(cmd);
        self.submit_shown(shown);
    }

    /// What the thread shows as your message for a key that answers what
    /// waits for your OK (§15): your intent ("yes, hand it off"), not the
    /// command the key sends.
    fn shortcut_shown(&self, cmd: &str) -> Option<String> {
        use crate::domain::CommitKind;
        use crate::i18n::{Line, line};
        let lang = self.session.language.as_deref();
        let kind = self.session.pending_commitment.as_ref()?.kind;
        let which = match cmd {
            "/confirm" => match kind {
                CommitKind::Handoff => Line::ConfirmHandoff,
                CommitKind::Delegate => Line::ConfirmDelegate,
                CommitKind::Verdict => Line::ConfirmVerdict,
                CommitKind::Approve => Line::ConfirmApprove,
                CommitKind::Object => Line::ConfirmObject,
            },
            "/cancel" => Line::CancelShown,
            _ => return None,
        };
        Some(line(lang, which).to_string())
    }

    /// Usage events: the thread on screen closes…
    fn end_thread(&mut self) {
        emit(UsageEvent::SessionEnd {
            secs: self.thread_started.elapsed().as_secs(),
            turns: self.thread_turns,
        });
    }

    /// …and another opens.
    fn begin_thread(&mut self) {
        crate::telemetry::new_session();
        self.thread_started = std::time::Instant::now();
        self.thread_turns = 0;
        emit(UsageEvent::SessionStart {
            surface: pathfinder_schema::Surface::Tui,
        });
    }

    /// A tick of the clock: the spinner turns while a reply runs, and a
    /// status message goes once it has been on screen for `STATUS_FOR`.
    pub fn on_tick(&mut self, now: std::time::Instant) {
        if self.running.is_some() {
            self.tick += 1;
        }
        if self.status.is_empty() {
            self.status_seen = None;
            return;
        }
        match &self.status_seen {
            Some((text, at)) if *text == self.status => {
                if now.saturating_duration_since(*at) >= STATUS_FOR {
                    self.status.clear();
                    self.status_seen = None;
                }
            }
            _ => self.status_seen = Some((self.status.clone(), now)),
        }
    }

    /// Counts the other threads in this folder that have messages.
    pub fn count_earlier_threads(&mut self) {
        self.earlier_threads = self
            .rt
            .sessions_in(&self.cwd)
            .iter()
            .filter(|s| s.id != self.session.id && !s.history.is_empty())
            .count();
    }

    fn new_thread(&mut self) {
        if self.running.is_some() {
            self.status = "A reply is still running; Esc stops it.".into();
            return;
        }
        match self
            .rt
            .create_session(uuid::Uuid::new_v4().to_string(), self.cwd.clone())
        {
            Ok(s) => {
                self.end_thread();
                self.begin_thread();
                self.session = s;
                self.entries.clear();
                self.attachments.clear();
                self.last_sent.clear();
                self.history_idx = None;
                self.history_draft.clear();
                self.cmd_sel = 0;
                self.always.clear();
                self.expanded.clear();
                self.selected = None;
                self.scroll = 0;
                self.side_scroll = 0;
                self.new_below = false;
                self.count_earlier_threads();
                self.status = "New thread.".into();
            }
            Err(e) => self.status = format!("Couldn't start a thread: {e}"),
        }
    }

    /// Ctrl+O: the threads in this folder, the most recent first, the
    /// open one marked.
    fn open_threads(&mut self) {
        let threads: Vec<(Session, u64)> = self
            .rt
            .threads_in(&self.cwd)
            .into_iter()
            .filter(|(s, _)| !s.history.is_empty())
            .collect();
        if threads.iter().all(|(s, _)| s.id == self.session.id) {
            self.status = "No earlier threads in this folder.".into();
            return;
        }
        let items = threads
            .iter()
            .map(|(s, _)| (s.id.clone(), thread_title(s)))
            .collect();
        let meta = threads
            .iter()
            .map(|(s, age)| {
                let meta = thread_meta(s, *age);
                if s.id == self.session.id {
                    format!("(open) {meta}")
                } else {
                    meta
                }
            })
            .collect();
        self.modal = Some(Modal::Threads {
            items,
            meta,
            query: String::new(),
            sel: 0,
        });
    }

    /// Shows a stored thread in place of this one.
    fn open_thread(&mut self, s: Session) {
        self.end_thread();
        self.begin_thread();
        self.entries = entries_of(&s);
        self.attachments.clear();
        self.last_sent.clear();
        self.history_idx = None;
        self.history_draft.clear();
        self.cmd_sel = 0;
        self.always.clear();
        self.expanded.clear();
        self.selected = None;
        self.status = format!("Opened \"{}\"", view::fit(&thread_title(&s), 60));
        self.session = s;
        self.scroll = 0;
        self.side_scroll = 0;
        self.new_below = false;
        self.count_earlier_threads();
        self.model_note = None;
        self.note_model_fallback();
    }

    fn submit(&mut self) {
        self.submit_shown(None);
    }

    /// Sends the input; the thread shows `shown` for it when given.
    fn submit_shown(&mut self, shown: Option<String>) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.starts_with('/')
            && let commands::Parsed::Unknown(_) = commands::parse(&text)
        {
            // Kept in the input to fix, and out of the history.
            let word = text.split(char::is_whitespace).next().unwrap_or(&text);
            self.status = match commands::closest(word) {
                Some(c) => format!("No such command · Tab → {c}"),
                None => "No such command · /help lists them".into(),
            };
            return;
        }
        if self.input_history.last() != Some(&text) {
            self.input_history.push(text.clone());
            if self.input_history.len() > 200 {
                let drop = self.input_history.len() - 200;
                self.input_history.drain(..drop);
            }
        }
        self.history_idx = None;
        self.history_draft.clear();
        self.cmd_sel = 0;
        if text.starts_with('/') {
            match commands::parse(&text) {
                commands::Parsed::Local(cmd, arg) => {
                    let arg = arg.to_string();
                    self.input.clear();
                    self.cursor = 0;
                    self.run_local(cmd, &arg);
                    return;
                }
                commands::Parsed::Unknown(_) | commands::Parsed::Task => {}
            }
        }
        if self.running.is_some() {
            self.status = "Wait for the reply, or Esc to stop it.".into();
            return;
        }
        let mut files = std::mem::take(&mut self.attachments);
        // A path in a command is the command's argument, not a dropped file.
        let dropped = if text.starts_with('/') {
            Vec::new()
        } else {
            dropped_files(&text, &self.cwd)
        };
        for f in dropped {
            if !files.contains(&f) {
                files.push(f);
            }
        }
        let shown = if let Some(shown) = shown {
            shown
        } else if files.is_empty() {
            text.clone()
        } else {
            let names: Vec<String> = files
                .iter()
                .map(|f| {
                    f.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .collect();
            format!("{text}\n📎 {}", names.join(", "))
        };
        self.entries.push(Entry::User(shown));
        // What this answers no longer waits: the rule and the panel drop
        // it now, not when the reply ends (the reload then has the truth).
        match text.trim() {
            "/confirm" | "/cancel" => self.session.pending_commitment = None,
            "/accept" | "/reject" => self.session.pending_change = None,
            _ => {}
        }
        self.last_sent = text.clone();
        self.input.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.status.clear();

        let cancel = CancellationToken::new();
        self.running = Some(cancel.clone());
        self.started = Some(std::time::Instant::now());
        self.tick = 0;
        let tx = self.tx.clone();
        let sink_tx = self.tx.clone();
        let sink: StreamSink = Arc::new(move |ev| {
            let _ = sink_tx.send(AppEvent::Stream(ev));
        });
        let ctx = RunContext {
            cancel: cancel.clone(),
            host: Some(Arc::new(TuiHost {
                tx: self.tx.clone(),
            })),
        };
        let rt = self.rt.clone();
        let id = self.session.id.clone();
        tokio::spawn(async move {
            use futures_util::FutureExt;
            let turn = with_turn(ctx, rt.handle_turn(&id, TurnInput { text, files }, sink));
            // A panic in the turn ends the turn, not the app.
            let result = match std::panic::AssertUnwindSafe(turn).catch_unwind().await {
                Ok(r) => r.map_err(|e| format!("{e:#}")),
                Err(_) => Err(format!(
                    "Pathfinder hit an internal error (details in {}). `/bug` sends a report.",
                    crate::crash::log_path().display()
                )),
            };
            let _ = tx.send(AppEvent::Done {
                result,
                stopped: cancel.is_cancelled(),
            });
        });
    }

    fn on_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Stream(StreamEvent::TextDelta(t)) => match self.entries.last_mut() {
                Some(Entry::Text(s)) => s.push_str(&t),
                _ => self.entries.push(Entry::Text(t)),
            },
            AppEvent::Stream(StreamEvent::Thought(t)) => match self.entries.last_mut() {
                Some(Entry::Thought(s)) => s.push_str(&t),
                _ => self.entries.push(Entry::Thought(t)),
            },
            AppEvent::Stream(StreamEvent::ToolStart(t)) => self.entries.push(Entry::Tool {
                id: t.id,
                title: t.title,
                ok: None,
                detail: String::new(),
            }),
            AppEvent::Stream(StreamEvent::ToolUpdate { id, content }) => {
                if let Some(Entry::Tool { detail, .. }) = self.tool_mut(&id) {
                    *detail = content.summary();
                    self.transcript_cache = None;
                }
            }
            AppEvent::Stream(StreamEvent::ToolEnd { id, ok: done }) => {
                if let Some(Entry::Tool { ok, .. }) = self.tool_mut(&id) {
                    *ok = Some(done);
                    self.transcript_cache = None;
                }
            }
            AppEvent::Ask {
                call_id,
                always_key,
                reply,
            } => {
                // Allowed "always" earlier in this thread: no question.
                if self.always.contains(&always_key) {
                    emit(UsageEvent::Permission {
                        scope: permission_scope(&always_key),
                        answer: pathfinder_schema::PermissionAnswer::Remembered,
                    });
                    let _ = reply.send(Permission::Always);
                    return;
                }
                let title = match self.tool_mut(&call_id) {
                    Some(Entry::Tool { title, .. }) => title.clone(),
                    _ => format!("Run `{always_key}`"),
                };
                let ask = Modal::Permission {
                    title,
                    always_key,
                    sel: None,
                    reply: Some(reply),
                };
                // Never over the sign-in dialog: keys typed there would
                // answer it, and the rest would land in the composer.
                if matches!(self.modal, Some(Modal::Login(_))) {
                    self.pending_asks.push_back(ask);
                    self.status =
                        "A tool asks for permission; it is asked once this dialog closes.".into();
                } else {
                    self.modal = Some(ask);
                }
            }
            AppEvent::Status(line) => self.status = line,
            AppEvent::ModelsRefreshed(outcome) => self.on_models_refreshed(outcome),
            AppEvent::Login(ev) => self.on_login_event(ev),
            AppEvent::Shared(result) => {
                self.modal = None;
                self.status = match result {
                    Ok(()) => "Sent. Thank you!".into(),
                    Err(e) => format!("Not sent: {e}"),
                };
            }
            AppEvent::UpdateAvailable(version) => {
                if self.status.is_empty() {
                    self.status =
                        format!("Pathfinder {version} is out: quit and run `pathfinder update`.");
                }
            }
            AppEvent::SignInStarted(result) => {
                if let Some(Modal::Consent {
                    events,
                    threads,
                    step: ConsentStep::Browser(None),
                    ..
                }) = self.modal.take()
                {
                    self.modal = Some(match result {
                        Ok(sign_in) => Modal::Consent {
                            events,
                            threads,
                            step: ConsentStep::Browser(Some(sign_in)),
                            error: None,
                            busy: false,
                            scroll: self.consent_max_scroll,
                        },
                        Err(e) => {
                            self.sign_in = None;
                            Modal::Consent {
                                events,
                                threads,
                                step: ConsentStep::Choose,
                                error: Some(e),
                                busy: false,
                                scroll: self.consent_max_scroll,
                            }
                        }
                    });
                }
            }
            AppEvent::Enrolled(result) => {
                self.sign_in = None;
                if let Some(Modal::Consent {
                    events,
                    threads,
                    step,
                    ..
                }) = self.modal.take()
                {
                    match result {
                        Ok(()) => {
                            emit(UsageEvent::SessionStart {
                                surface: pathfinder_schema::Surface::Tui,
                            });
                            self.status = if events || threads {
                                "Joined. Thank you! `pathfinder data` shows what is collected; `pathfinder data off` stops it.".into()
                            } else {
                                "Joined: the shared gate is on. No usage data is sent.".into()
                            };
                        }
                        Err(e) => {
                            self.modal = Some(Modal::Consent {
                                events,
                                threads,
                                step: match step {
                                    ConsentStep::Invite(code) => ConsentStep::Invite(code),
                                    _ => ConsentStep::Choose,
                                },
                                error: Some(e),
                                busy: false,
                                scroll: self.consent_max_scroll,
                            });
                        }
                    }
                }
            }
            AppEvent::Done { result, stopped } => {
                self.running = None;
                // The turn is over: questions it left waiting have no one
                // to answer.
                self.pending_asks.clear();
                self.thread_turns += 1;
                match result {
                    Ok(out) if !out.reply.is_empty() => {
                        match (
                            self.entries.last_mut(),
                            Entry::of_reply(out.reply, out.shown),
                        ) {
                            (Some(Entry::Text(s)), Entry::Text(more)) => s.push_str(&more),
                            (_, entry) => self.entries.push(entry),
                        }
                    }
                    Ok(_) => {}
                    Err(_) if stopped => self.entries.push(Entry::Note("Stopped.".into())),
                    Err(e) => self.entries.push(Entry::Note(format!(
                        "Something went wrong: {}",
                        login_hint(&e)
                    ))),
                }
                self.reload();
            }
        }
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .rev()
            .find(|e| matches!(e, Entry::Tool { id: i, .. } if i == id))
    }

    /// The values a setting can take: (value, label). Models come in
    /// sections by account, each after a header row (an empty value).
    fn choices(&self, option: &str) -> Vec<(String, String)> {
        let choices = self.rt.model_choices(&self.session);
        match option {
            crate::options::FAST_MODEL | crate::options::MAIN_MODEL => {
                crate::config::by_account(&choices.catalog)
                    .into_iter()
                    .flat_map(|(account, entries)| {
                        // A CLI that keeps its own login says where it is.
                        let header = match account.own_login() {
                            Some(note) => format!("{} · {note}", account.header()),
                            None => account.header(),
                        };
                        std::iter::once((String::new(), header))
                            .chain(entries.into_iter().map(|m| (m.id.clone(), m.label.clone())))
                    })
                    .collect()
            }
            crate::options::MAIN_EFFORT | crate::options::FAST_EFFORT => EFFORTS
                .iter()
                .map(|e| (e.to_string(), e.to_string()))
                .collect(),
            crate::options::DECISIONS => {
                vec![("on".into(), "on".into()), ("off".into(), "off".into())]
            }
            crate::options::DECISION_MODEL => choices
                .decision_available
                .iter()
                .map(|p| (p.id().to_string(), p.label().to_string()))
                .collect(),
            crate::options::CHECKS => CHECKS
                .iter()
                .map(|c| (c.to_string(), crate::runtime::checks_label(c).to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A setting (not a model) with at most `CYCLE_MAX` values: Enter,
    /// Space and ←→ step through them in place instead of a chooser.
    pub fn cycles(&self, option: &str) -> bool {
        // Models keep their chooser: its sections say which account runs
        // and pays for each.
        if matches!(
            option,
            GATE_KEY | crate::options::FAST_MODEL | crate::options::MAIN_MODEL
        ) {
            return false;
        }
        let n = self
            .choices(option)
            .iter()
            .filter(|i| !is_header(i))
            .count();
        (1..=CYCLE_MAX).contains(&n)
    }

    /// A setting's values and which is current, for the settings list.
    pub fn cycle_values(&self, option: &str) -> (Vec<String>, Option<usize>) {
        let items: Vec<(String, String)> = self
            .choices(option)
            .into_iter()
            .filter(|i| !is_header(i))
            .collect();
        let current = self.current_value(option);
        let at = items.iter().position(|(v, _)| *v == current);
        (items.into_iter().map(|(_, label)| label).collect(), at)
    }

    /// Sets the settings row `row` to its next value (`delta` 1) or the
    /// one before (-1), for this thread.
    fn cycle_setting(&mut self, row: usize, delta: isize) {
        let (option, title) = SETTINGS[row];
        let items: Vec<String> = self
            .choices(option)
            .into_iter()
            .filter(|i| !is_header(i))
            .map(|(v, _)| v)
            .collect();
        if items.is_empty() {
            self.status = format!("No choices for {title}.");
            return;
        }
        let current = self.current_value(option);
        let n = items.len() as isize;
        let next = match items.iter().position(|v| *v == current) {
            Some(i) => (i as isize + delta).rem_euclid(n) as usize,
            None => 0,
        };
        let value = if option == crate::options::DECISIONS {
            OptionValue::Bool(items[next] == "on")
        } else {
            OptionValue::Id(items[next].clone())
        };
        match self.rt.set_option(&self.session.id, option, value) {
            Ok(s) => {
                self.session = s;
                self.status = self.set_message(title, option, "");
                self.note_model_fallback();
            }
            Err(e) => self.status = format!("Couldn't set {title}: {e}"),
        }
    }

    /// A setting's current value as its picker's value: the model id for
    /// models.
    fn current_value(&self, option: &str) -> String {
        let c = self.rt.model_choices(&self.session);
        match option {
            crate::options::FAST_MODEL => c.system1.unwrap_or_default(),
            crate::options::MAIN_MODEL => c.system2.unwrap_or_default(),
            crate::options::DECISION_MODEL => c
                .decision_model
                .map(|p| p.id().to_string())
                .unwrap_or_default(),
            _ => self.current(option),
        }
    }

    /// A setting's current value, for the settings list.
    /// The model a tier option runs, if it is a model option.
    fn tier_model(&self, option: &str) -> Option<crate::config::ModelEntry> {
        let c = self.rt.model_choices(&self.session);
        let id = match option {
            crate::options::FAST_MODEL => c.system1,
            crate::options::MAIN_MODEL => c.system2,
            _ => None,
        }?;
        c.catalog.into_iter().find(|m| m.id == id)
    }

    /// A setting's value for the settings list: models with their account,
    /// as the same label can come from several.
    pub fn current_setting(&self, option: &str) -> String {
        if option == GATE_KEY {
            return crate::auth::accounts::gate_key_label(
                &self.creds,
                &crate::auth::accounts::process_env,
            );
        }
        match self.tier_model(option) {
            Some(m) => format!("{} · {}", m.label, crate::config::Account::of(&m).short()),
            None => self.current(option),
        }
    }

    /// A model for the footer: its label with the account's short name
    /// (`GPT-6 Luna·ChatGPT`), or without it, `$` when billed per use.
    pub fn footer_model(&self, option: &str, with_account: bool) -> String {
        match self.tier_model(option) {
            Some(m) if with_account => {
                format!("{}·{}", m.label, crate::config::Account::of(&m).short())
            }
            _ => self.current_short(option),
        }
    }

    /// A model's label for the footer, with `$` when it is billed per use.
    pub fn current_short(&self, option: &str) -> String {
        match self.tier_model(option) {
            Some(m) if crate::config::Account::of(&m).pay_per_use() => format!("{} $", m.label),
            _ => self.current(option),
        }
    }

    /// The status after setting an option: a model names its account.
    fn set_message(&self, title: &str, option: &str, saved: &str) -> String {
        match self.tier_model(option) {
            Some(m) => format!(
                "{title} set{saved}: {} ({}).",
                m.label,
                crate::config::Account::of(&m).header()
            ),
            None => format!("{title} set{saved}."),
        }
    }

    pub fn current(&self, option: &str) -> String {
        let c = self.rt.model_choices(&self.session);
        let label = |id: &Option<String>| {
            id.as_ref()
                .and_then(|id| c.catalog.iter().find(|m| &m.id == id))
                .map(|m| m.label.clone())
                .unwrap_or_else(|| "—".into())
        };
        match option {
            crate::options::FAST_MODEL => label(&c.system1),
            crate::options::MAIN_MODEL => label(&c.system2),
            crate::options::MAIN_EFFORT => c.effort.clone(),
            crate::options::FAST_EFFORT => c.fast_effort.clone(),
            crate::options::DECISIONS => if c.decisions_on { "on" } else { "off" }.into(),
            crate::options::DECISION_MODEL => c
                .decision_model
                .map(|p| p.short().to_string())
                .unwrap_or_else(|| "none".into()),
            crate::options::CHECKS => c.checks.clone(),
            _ => String::new(),
        }
    }

    /// A setting's current value as the settings list names it.
    pub fn current_label(&self, option: &str) -> String {
        match option {
            crate::options::CHECKS => {
                crate::runtime::checks_label(&self.current(option)).to_string()
            }
            _ => self.current_setting(option),
        }
    }

    fn on_modal_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let up = matches!(key.code, KeyCode::Up | KeyCode::Char('k'));
        let down = matches!(key.code, KeyCode::Down | KeyCode::Char('j'));
        let step = |sel: usize, len: usize| -> usize {
            if up {
                sel.saturating_sub(1)
            } else if down {
                (sel + 1).min(len.saturating_sub(1))
            } else {
                sel
            }
        };
        let resolve_permission = |app: &mut Self,
                                  always_key: String,
                                  reply: Option<oneshot::Sender<Permission>>,
                                  a: Permission| {
            if let Some(tx) = reply {
                let said = match a {
                    Permission::Denied => "✗ You declined this.".to_string(),
                    Permission::Always => {
                        app.always.insert(always_key.clone());
                        format!(
                            "✓ Allowed, and from now on in this thread for `{}`.",
                            always_label(&always_key)
                        )
                    }
                    _ => "✓ Allowed once.".to_string(),
                };
                app.entries.push(Entry::Note(said));
                emit(UsageEvent::Permission {
                    scope: permission_scope(&always_key),
                    answer: match a {
                        Permission::Denied => pathfinder_schema::PermissionAnswer::No,
                        Permission::Always => pathfinder_schema::PermissionAnswer::Always,
                        _ => pathfinder_schema::PermissionAnswer::Once,
                    },
                });
                let _ = tx.send(a);
            }
        };
        if ctrl && key.code == KeyCode::Char('c') {
            match modal {
                Modal::Permission {
                    always_key, reply, ..
                } => {
                    resolve_permission(self, always_key, reply, Permission::Denied);
                    if let Some(cancel) = &self.running {
                        cancel.cancel();
                    }
                }
                Modal::Share { busy, .. } => {
                    self.status = if busy {
                        "Cancelled.".into()
                    } else {
                        "Nothing was sent.".into()
                    };
                }
                Modal::Consent {
                    events,
                    threads,
                    step: cstep,
                    ..
                } => {
                    if let Some(task) = self.sign_in.take() {
                        task.abort();
                    }
                    let (ev, th) = match cstep {
                        ConsentStep::Choose => (false, false),
                        _ => (events, threads),
                    };
                    let mut c = crate::telemetry::Consent::load();
                    c.events = ev;
                    c.threads = th;
                    c.stamp();
                    let _ = c.save();
                    crate::telemetry::reload();
                    self.status = if ev || th {
                        "Saved; nothing is sent until you join: pathfinder data join".into()
                    } else {
                        "Nothing is shared. `pathfinder data join` adds the shared gate later."
                            .into()
                    };
                }
                Modal::Login(flow) => {
                    self.stop_login_job();
                    self.status = login_cancelled(flow.mode).into();
                    self.modal = Self::after_login(flow.mode);
                }
                Modal::Help { .. }
                | Modal::Threads { .. }
                | Modal::Settings { .. }
                | Modal::Choose { .. } => {}
            }
            return;
        }
        self.modal = match modal {
            Modal::Help {
                mut scroll,
                mut query,
            } => {
                // Typing filters; only Esc, F1 and q (with nothing typed)
                // close it.
                let max = self.help_max_scroll;
                match key.code {
                    KeyCode::Esc | KeyCode::F(1) => return,
                    KeyCode::Char('q') if query.is_empty() && !ctrl && !alt => return,
                    KeyCode::Down => scroll = (scroll + 1).min(max),
                    KeyCode::Up => scroll = scroll.saturating_sub(1),
                    KeyCode::PageDown => scroll = (scroll + self.help_page).min(max),
                    KeyCode::PageUp => scroll = scroll.saturating_sub(self.help_page),
                    KeyCode::Home => scroll = 0,
                    KeyCode::End => scroll = max,
                    KeyCode::Char('u') if ctrl => {
                        query.clear();
                        scroll = 0;
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        scroll = 0;
                    }
                    KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => {
                        query.push(c);
                        scroll = 0;
                    }
                    _ => {}
                }
                Some(Modal::Help { scroll, query })
            }
            Modal::Share {
                outgoing,
                lines,
                scroll,
                busy,
            } => {
                if key.code == KeyCode::Esc {
                    self.status = if busy {
                        "Cancelled.".into()
                    } else {
                        "Nothing was sent.".into()
                    };
                    return;
                }
                if busy {
                    return self.modal = Some(Modal::Share {
                        outgoing,
                        lines,
                        scroll,
                        busy,
                    });
                }
                let page = 10;
                let max = lines.len().saturating_sub(1);
                let scroll = match key.code {
                    KeyCode::Char('n') => {
                        self.status = "Nothing was sent.".into();
                        return;
                    }
                    KeyCode::Enter => {
                        let tx = self.tx.clone();
                        let out = outgoing.clone();
                        tokio::spawn(async move {
                            let consent = crate::telemetry::Consent::load();
                            let r = match &out {
                                Outgoing::Thread(t) => crate::telemetry::share(&consent, t).await,
                                Outgoing::Bug(b) => crate::telemetry::report_bug(&consent, b).await,
                            }
                            .map_err(|e| format!("{e:#}"));
                            let _ = tx.send(AppEvent::Shared(r));
                        });
                        used(match outgoing {
                            Outgoing::Thread(_) => Feature::Share,
                            Outgoing::Bug(_) => Feature::Bug,
                        });
                        return self.modal = Some(Modal::Share {
                            outgoing,
                            lines,
                            scroll,
                            busy: true,
                        });
                    }
                    KeyCode::Down | KeyCode::Char('j') => (scroll + 1).min(max),
                    KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
                    KeyCode::PageDown | KeyCode::Char(' ') => (scroll + page).min(max),
                    KeyCode::PageUp => scroll.saturating_sub(page),
                    KeyCode::Char('g') | KeyCode::Home => 0,
                    KeyCode::Char('G') | KeyCode::End => max,
                    _ => scroll,
                };
                Some(Modal::Share {
                    outgoing,
                    lines,
                    scroll,
                    busy: false,
                })
            }
            Modal::Consent {
                events,
                threads,
                step,
                error,
                busy,
                scroll,
            } => self.on_consent_key(key, events, threads, step, error, busy, scroll),
            Modal::Login(mut flow) => {
                use crate::auth::flow::Action;
                match flow.on_key(key) {
                    Action::None => Some(Modal::Login(flow)),
                    Action::Close => {
                        self.status = login_cancelled(flow.mode).into();
                        Self::after_login(flow.mode)
                    }
                    Action::Cancel => {
                        self.stop_login_job();
                        if flow.is_closed() {
                            // It was the first step: close now, so the next
                            // key reaches the composer.
                            self.status = login_cancelled(flow.mode).into();
                            Self::after_login(flow.mode)
                        } else {
                            flow.error = Some("Cancelled.".into());
                            Some(Modal::Login(flow))
                        }
                    }
                    Action::Run(job) => {
                        self.start_login_job(job);
                        Some(Modal::Login(flow))
                    }
                    Action::Copy(url) => {
                        self.status = match copy_to_clipboard(&url) {
                            Ok(()) => "Copied the sign-in address.".into(),
                            Err(e) => format!("Couldn't copy: {e}"),
                        };
                        Some(Modal::Login(flow))
                    }
                }
            }
            Modal::Permission {
                title,
                always_key,
                mut sel,
                reply,
            } => {
                let answer = match key.code {
                    KeyCode::PageUp => {
                        self.scroll = self.scroll.saturating_add(10);
                        None
                    }
                    KeyCode::PageDown => {
                        self.scroll = self.scroll.saturating_sub(10);
                        if self.scroll == 0 {
                            self.new_below = false;
                        }
                        None
                    }
                    KeyCode::Up | KeyCode::Left => {
                        sel = Some(sel.map_or(0, |i| i.saturating_sub(1)));
                        None
                    }
                    KeyCode::Down | KeyCode::Right => {
                        sel = Some(sel.map_or(0, |i| (i + 1).min(2)));
                        None
                    }
                    KeyCode::Char('y' | '1') => Some(Permission::Once),
                    KeyCode::Char('a' | '2') => Some(Permission::Always),
                    KeyCode::Char('n' | '3') | KeyCode::Esc => Some(Permission::Denied),
                    KeyCode::Enter => sel.map(|i| match i {
                        0 => Permission::Once,
                        1 => Permission::Always,
                        _ => Permission::Denied,
                    }),
                    _ => None,
                };
                match answer {
                    Some(a) => {
                        resolve_permission(self, always_key, reply, a);
                        None
                    }
                    None => Some(Modal::Permission {
                        title,
                        always_key,
                        sel,
                        reply,
                    }),
                }
            }
            Modal::Threads {
                items,
                meta,
                mut query,
                mut sel,
            } => {
                let filtered = filter_matches(&items, &query);
                let last = filtered.len().saturating_sub(1);
                match key.code {
                    KeyCode::Esc => return,
                    KeyCode::Enter => {
                        if let Some(&orig_idx) = filtered.get(sel)
                            && let Some((id, _)) = items.get(orig_idx)
                        {
                            if *id == self.session.id {
                                self.status = "That thread is already open.".into();
                            } else if let Ok(Some(s)) = self.rt.load_session(id) {
                                self.open_thread(s);
                            }
                        }
                        return;
                    }
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(last),
                    KeyCode::PageUp => sel = sel.saturating_sub(10),
                    KeyCode::PageDown => sel = (sel + 10).min(last),
                    KeyCode::Home => sel = 0,
                    KeyCode::End => sel = last,
                    KeyCode::Char('u') if ctrl => {
                        query.clear();
                        sel = 0;
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        sel = 0;
                    }
                    KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => {
                        query.push(c);
                        sel = 0;
                    }
                    _ => {}
                }
                Some(Modal::Threads {
                    items,
                    meta,
                    query,
                    sel,
                })
            }
            Modal::Settings { sel } => match key.code {
                KeyCode::Esc => None,
                KeyCode::Char('s') => {
                    match self.rt.save_session_defaults(&self.session) {
                        Ok(()) => {
                            self.status =
                                "Current settings saved as defaults for new threads.".into();
                        }
                        Err(e) => self.status = format!("Couldn't save defaults: {e}"),
                    }
                    Some(Modal::Settings { sel })
                }
                KeyCode::Enter if SETTINGS[sel].0 == GATE_KEY => self.open_gate_key(),
                // A few values: they change in place.
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Left
                    if self.cycles(SETTINGS[sel].0) =>
                {
                    let delta = if key.code == KeyCode::Left { -1 } else { 1 };
                    self.cycle_setting(sel, delta);
                    Some(Modal::Settings { sel })
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let (option, name) = SETTINGS[sel];
                    let mut items = self.choices(option);
                    if items.iter().all(is_header) {
                        self.status = format!("No choices for {name}.");
                        Some(Modal::Settings { sel })
                    } else {
                        let current = self.current_value(option);
                        let at = items
                            .iter()
                            .position(|item| !is_header(item) && item.0 == current);
                        if let Some(i) = at {
                            items[i].1.push_str(CURRENT_MARK);
                        }
                        let sel = at
                            .and_then(|i| filter_matches(&items, "").iter().position(|&m| m == i))
                            .unwrap_or(0);
                        Some(Modal::Choose {
                            option,
                            title: name.to_string(),
                            items,
                            query: String::new(),
                            sel,
                        })
                    }
                }
                KeyCode::PageUp | KeyCode::Home => Some(Modal::Settings { sel: 0 }),
                KeyCode::PageDown | KeyCode::End => Some(Modal::Settings {
                    sel: SETTINGS.len().saturating_sub(1),
                }),
                _ => Some(Modal::Settings {
                    sel: step(sel, SETTINGS.len()),
                }),
            },
            Modal::Choose {
                option,
                title,
                items,
                mut query,
                mut sel,
            } => {
                let back = SETTINGS.iter().position(|(o, _)| *o == option).unwrap_or(0);
                let filtered = filter_matches(&items, &query);
                let save_as_default = ctrl && key.code == KeyCode::Char('s');
                match key.code {
                    KeyCode::Esc => Some(Modal::Settings { sel: back }),
                    KeyCode::Enter => {
                        if let Some(&orig_idx) = filtered.get(sel) {
                            let (raw_val, _) = items[orig_idx].clone();
                            let value = if option == crate::options::DECISIONS {
                                OptionValue::Bool(raw_val == "on")
                            } else {
                                OptionValue::Id(raw_val)
                            };
                            match self.rt.set_option(&self.session.id, option, value) {
                                Ok(s) => {
                                    self.session = s;
                                    self.status = self.set_message(&title, option, "");
                                    self.note_model_fallback();
                                }
                                Err(e) => self.status = format!("Couldn't set {title}: {e}"),
                            }
                        }
                        Some(Modal::Settings { sel: back })
                    }
                    _ if save_as_default => {
                        if let Some(&orig_idx) = filtered.get(sel) {
                            let (raw_val, _) = items[orig_idx].clone();
                            let make_value = |v: &str| {
                                if option == crate::options::DECISIONS {
                                    OptionValue::Bool(v == "on")
                                } else {
                                    OptionValue::Id(v.to_string())
                                }
                            };
                            match self
                                .rt
                                .set_option(&self.session.id, option, make_value(&raw_val))
                            {
                                Ok(s) => {
                                    self.session = s;
                                    match self.rt.set_default_option(option, make_value(&raw_val)) {
                                        Ok(()) => {
                                            self.status = self.set_message(
                                                &title,
                                                option,
                                                " (saved as default)",
                                            );
                                        }
                                        Err(e) => {
                                            self.status = format!(
                                                "{title} set, but couldn't save default: {e}"
                                            );
                                        }
                                    }
                                }
                                Err(e) => self.status = format!("Couldn't set {title}: {e}"),
                            }
                        }
                        Some(Modal::Settings { sel: back })
                    }
                    _ => {
                        let last = filtered.len().saturating_sub(1);
                        match key.code {
                            KeyCode::Up => sel = sel.saturating_sub(1),
                            KeyCode::Down => sel = (sel + 1).min(last),
                            KeyCode::PageUp => sel = sel.saturating_sub(10),
                            KeyCode::PageDown => sel = (sel + 10).min(last),
                            KeyCode::Home => sel = 0,
                            KeyCode::End => sel = last,
                            KeyCode::Char('u') if ctrl => {
                                query.clear();
                                sel = chooser_sel(&items, &query);
                            }
                            KeyCode::Backspace => {
                                query.pop();
                                sel = chooser_sel(&items, &query);
                            }
                            KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => {
                                query.push(c);
                                sel = chooser_sel(&items, &query);
                            }
                            _ => {}
                        }
                        Some(Modal::Choose {
                            option,
                            title,
                            items,
                            query,
                            sel,
                        })
                    }
                }
            }
        };
    }
}

/// What the footer says when the sign-in panel closes without a change.
fn login_cancelled(mode: crate::auth::flow::Mode) -> &'static str {
    use crate::auth::flow::Mode;
    match mode {
        Mode::Login => "Sign-in cancelled",
        Mode::Logout => "Sign-out cancelled",
        Mode::Gate => "Nothing changed.",
    }
}

/// The kind of permission an always-key stands for.
/// The part of an "Always allow" key a person reads: the program, or for an
/// unsandboxed command the whole command line (which may itself contain `:`).
pub(crate) fn always_label(always_key: &str) -> &str {
    always_key
        .split_once(':')
        .filter(|(kind, _)| matches!(*kind, "unsandboxed" | "network"))
        .map_or(always_key, |(_, rest)| rest)
}

fn permission_scope(always_key: &str) -> pathfinder_schema::PermissionScope {
    use pathfinder_schema::PermissionScope as S;
    match always_key.split_once(':').map(|(k, _)| k) {
        Some("unsandboxed") => S::OutsideSandbox,
        Some("network") => S::Network,
        Some(_) => S::Other,
        None => S::Sandboxed,
    }
}

fn env_says_off() -> bool {
    matches!(
        crate::envvar::get("PATHFINDER_TELEMETRY")
            .as_deref()
            .map(str::trim),
        Ok("off" | "0" | "false" | "no")
    )
}

impl App {
    /// Keys on the consent screen. Returns the modal to keep showing.
    #[allow(clippy::too_many_arguments)]
    fn on_consent_key(
        &mut self,
        key: KeyEvent,
        mut events: bool,
        mut threads: bool,
        step: ConsentStep,
        error: Option<String>,
        busy: bool,
        mut scroll: usize,
    ) -> Option<Modal> {
        let max_scroll = self.consent_max_scroll;
        macro_rules! keep {
            ($step:expr, $error:expr, $busy:expr, $scroll:expr) => {
                Some(Modal::Consent {
                    events,
                    threads,
                    step: $step,
                    error: $error,
                    busy: $busy,
                    scroll: $scroll,
                })
            };
        }
        let save_answer = |events: bool, threads: bool| {
            let mut c = crate::telemetry::Consent::load();
            c.events = events;
            c.threads = threads;
            c.stamp();
            if let Err(e) = c.save() {
                tracing::warn!("saving the data answer: {e:#}");
            }
            crate::telemetry::reload();
        };
        let not_joined = |events: bool, threads: bool| -> String {
            if events || threads {
                "Saved; nothing is sent until you join: pathfinder data join".into()
            } else {
                "Nothing is shared. `pathfinder data join` adds the shared gate later.".into()
            }
        };
        if key.code == KeyCode::Esc {
            if let Some(task) = self.sign_in.take() {
                task.abort();
            }
            match step {
                ConsentStep::Choose => {
                    save_answer(false, false);
                    self.status =
                        "Nothing is shared. `pathfinder data` can change that later.".into();
                }
                _ => {
                    save_answer(events, threads);
                    self.status = not_joined(events, threads);
                }
            }
            return None;
        }
        match key.code {
            KeyCode::PageUp => {
                scroll = scroll.saturating_sub(10);
                return keep!(step, error, busy, scroll);
            }
            KeyCode::PageDown => {
                scroll = (scroll + 10).min(max_scroll);
                return keep!(step, error, busy, scroll);
            }
            _ => {}
        }
        if busy {
            return keep!(step, error, busy, scroll);
        }
        match step {
            ConsentStep::Choose => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    keep!(ConsentStep::Choose, error, false, scroll.saturating_sub(1))
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    keep!(
                        ConsentStep::Choose,
                        error,
                        false,
                        (scroll + 1).min(max_scroll)
                    )
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    keep!(ConsentStep::Choose, error, false, 0)
                }
                KeyCode::End | KeyCode::Char('G') => {
                    keep!(ConsentStep::Choose, error, false, max_scroll)
                }
                KeyCode::Char('e') => {
                    events = !events;
                    keep!(ConsentStep::Choose, None, false, scroll)
                }
                KeyCode::Char('t') => {
                    threads = !threads;
                    keep!(ConsentStep::Choose, None, false, scroll)
                }
                KeyCode::Char('i') => {
                    keep!(ConsentStep::Invite(String::new()), None, false, max_scroll)
                }
                KeyCode::Enter => {
                    let mut consent = crate::telemetry::Consent::load();
                    consent.events = events;
                    consent.threads = threads;
                    if consent.enrolled && consent.token().is_some() {
                        // Already joined, asked again for a new notice:
                        // just the new answer.
                        consent.stamp();
                        if let Err(e) = consent.save() {
                            return keep!(
                                ConsentStep::Choose,
                                Some(format!("{e:#}")),
                                false,
                                max_scroll
                            );
                        }
                        crate::telemetry::reload();
                        tokio::spawn(async move {
                            if let Err(e) = crate::telemetry::set_remote_consent(&consent).await {
                                tracing::warn!("telling the collector the new answer: {e:#}");
                            }
                        });
                        self.status = "Saved. `pathfinder data` changes it later.".into();
                        return None;
                    }
                    let tx = self.tx.clone();
                    let task = tokio::spawn(async move {
                        let sign_in = match crate::telemetry::start_sign_in(&consent).await {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = tx.send(AppEvent::SignInStarted(Err(format!("{e:#}"))));
                                return;
                            }
                        };
                        crate::cli::open_browser(&sign_in.url);
                        let _ = tx.send(AppEvent::SignInStarted(Ok(sign_in.clone())));
                        let r = crate::telemetry::finish_sign_in(&sign_in, &mut consent)
                            .await
                            .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(AppEvent::Enrolled(r));
                    });
                    self.sign_in = Some(task.abort_handle());
                    keep!(ConsentStep::Browser(None), None, false, max_scroll)
                }
                _ => keep!(ConsentStep::Choose, error, false, scroll),
            },
            ConsentStep::Browser(sign_in) => match key.code {
                KeyCode::Char('i') => {
                    if let Some(task) = self.sign_in.take() {
                        task.abort();
                    }
                    keep!(ConsentStep::Invite(String::new()), None, false, max_scroll)
                }
                KeyCode::Char('o') => {
                    if let Some(s) = &sign_in {
                        crate::cli::open_browser(&s.url);
                    }
                    keep!(ConsentStep::Browser(sign_in), error, false, scroll)
                }
                _ => keep!(ConsentStep::Browser(sign_in), error, false, scroll),
            },
            ConsentStep::Invite(mut code) => match key.code {
                KeyCode::Enter if !code.trim().is_empty() => {
                    let mut consent = crate::telemetry::Consent::load();
                    consent.events = events;
                    consent.threads = threads;
                    let tx = self.tx.clone();
                    let code2 = code.clone();
                    tokio::spawn(async move {
                        let r = crate::telemetry::enroll(&code2, &mut consent)
                            .await
                            .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(AppEvent::Enrolled(r));
                    });
                    keep!(ConsentStep::Invite(code), None, true, max_scroll)
                }
                KeyCode::Backspace => {
                    code.pop();
                    keep!(ConsentStep::Invite(code), None, false, max_scroll)
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT)
                        && !c.is_control()
                        && code.len() < 64 =>
                {
                    code.push(c);
                    keep!(ConsentStep::Invite(code), None, false, max_scroll)
                }
                _ => keep!(ConsentStep::Invite(code), error, false, scroll),
            },
        }
    }
}

impl App {
    /// A command the terminal app handles itself.
    fn run_local(&mut self, cmd: commands::Local, arg: &str) {
        use commands::Local;
        match cmd {
            Local::Help => {
                used(Feature::Help);
                self.modal = Some(Modal::help());
            }
            Local::Quit => {
                if let Some(cancel) = &self.running {
                    cancel.cancel();
                }
                self.quit = true;
            }
            Local::New => self.new_thread(),
            Local::Resume => {
                used(Feature::Threads);
                self.open_threads();
            }
            Local::Model => {
                used(Feature::Settings);
                self.modal = Some(Modal::Settings { sel: 0 });
            }
            Local::Status => {
                self.entries.push(Entry::User("/status".into()));
                self.entries.push(Entry::Info(self.status_rows()));
                self.scroll = 0;
            }
            Local::Copy => {
                let last = self.entries.iter().rev().find_map(|e| match e {
                    Entry::Text(t) if !t.trim().is_empty() => Some(t.clone()),
                    _ => None,
                });
                self.status = match last {
                    Some(text) => {
                        used(Feature::Copy);
                        match copy_to_clipboard(&text) {
                            Ok(()) => format!(
                                "Copied the last reply ({} characters).",
                                text.chars().count()
                            ),
                            Err(e) => format!("Couldn't copy: {e}"),
                        }
                    }
                    // Sources aren't replies: say how to copy one.
                    None if self
                        .entries
                        .iter()
                        .any(|e| matches!(e, Entry::Source { .. } | Entry::Search { .. })) =>
                    {
                        "There's no reply to copy yet · Tab selects a source, y copies it".into()
                    }
                    None => "There's no reply to copy yet.".into(),
                };
            }
            Local::Thinking => {
                self.show_thoughts = !self.show_thoughts;
                self.status = if self.show_thoughts {
                    "Showing thinking.".into()
                } else {
                    "Hiding thinking.".into()
                };
            }
            Local::Tools => {
                used(Feature::ToolGrouping);
                self.show_tools = !self.show_tools;
                self.status = if self.show_tools {
                    "Showing every tool call.".into()
                } else {
                    "Grouping consecutive reads and searches.".into()
                };
            }
            Local::Join => {
                if env_says_off() {
                    self.status =
                        "PATHFINDER_TELEMETRY=off is set, so nothing is shared from here.".into();
                    return;
                }
                let saved = crate::telemetry::Consent::load();
                self.modal = Some(Modal::Consent {
                    events: saved.events,
                    threads: saved.threads,
                    step: ConsentStep::Choose,
                    error: None,
                    busy: false,
                    scroll: 0,
                });
            }
            Local::Data => {
                let c = crate::telemetry::Consent::load();
                let yes = |b: bool| if b { "yes" } else { "no" };
                self.entries.push(Entry::Note(format!(
                    "Joined: {} · usage events: {} · thread sharing: {}. /join changes it; `pathfinder data show` prints what's queued; `pathfinder data delete` erases what was sent.",
                    yes(c.enrolled),
                    yes(c.events),
                    yes(c.threads)
                )));
                self.scroll = 0;
            }
            Local::Update => {
                self.status = "Checking for a new version…".into();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let line = match crate::update::latest().await {
                        Ok(r) if crate::update::newer(&r.version, env!("CARGO_PKG_VERSION")) => {
                            format!(
                                "Pathfinder {} is out: quit and run `pathfinder update`.",
                                r.version
                            )
                        }
                        Ok(_) => format!(
                            "Pathfinder {} is the newest version.",
                            env!("CARGO_PKG_VERSION")
                        ),
                        Err(e) => format!("Couldn't check for updates: {e}"),
                    };
                    let _ = tx.send(AppEvent::Status(line));
                });
            }
            Local::Attach => self.attach(arg),
            Local::Share => self.start_share(arg),
            Local::Bug => self.start_bug(arg),
            Local::Login => self.open_login(false, arg),
            Local::Logout => self.open_login(true, arg),
        }
    }

    fn attach(&mut self, path: &str) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            self.status =
                "Say which file: /attach <file>, /attach clear, or drop it onto the window.".into();
            return;
        }
        if trimmed.eq_ignore_ascii_case("clear") {
            let count = self.attachments.len();
            self.attachments.clear();
            self.status = if count == 0 {
                "No attachments to clear.".into()
            } else {
                format!("Cleared {count} attachment(s).")
            };
            return;
        }
        let Some(p) = commands::resolve_attach_path(trimmed, &self.cwd) else {
            self.status = "Say which file: /attach <file>, or /attach clear.".into();
            return;
        };
        if p.is_file() {
            self.status = format!("Attached {}.", p.display());
            if !self.attachments.contains(&p) {
                self.attachments.push(p);
            }
        } else if p.is_dir() {
            self.status = format!("{} is a folder; pick a file inside it.", p.display());
        } else {
            self.status = format!("No file at {}.", p.display());
        }
    }

    /// `/status`: what this Pathfinder is running with, as key/value rows.
    fn status_rows(&self) -> Vec<(String, String)> {
        use crate::options;
        let c = crate::telemetry::Consent::load();
        let yes = |b: bool| if b { "yes" } else { "no" };
        let gate = if self.current(options::DECISIONS) != "on" {
            "off for this thread".to_string()
        } else if c.enrolled {
            "on · your own decision key if you have one, else the shared gate".to_string()
        } else {
            "on if you have a decision key; join the beta (/join) for the shared gate".to_string()
        };
        let tiers = self.rt.tier_picks(&self.session);
        // One account a line, as `pathfinder auth status` prints them.
        let mut accounts = vec!["/login adds one · /logout removes one".to_string()];
        accounts.extend(crate::auth::accounts::status_lines(&self.account_rows()));
        let row = |k: &str, v: String| (k.to_string(), v);
        vec![
            row(
                "Version",
                format!(
                    "Pathfinder {} · {} {}",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            ),
            row("Fast model", self.tier_line(&tiers[0])),
            row("Main model", self.tier_line(&tiers[1])),
            row(
                "Checks",
                crate::runtime::checks_label(&self.current(options::CHECKS)).to_string(),
            ),
            row("Gate", gate),
            row("Accounts", accounts.join("\n")),
            row(
                "Display",
                format!(
                    "thinking {} · tool calls {}",
                    if self.show_thoughts {
                        "shown"
                    } else {
                        "hidden"
                    },
                    if self.show_tools {
                        "each shown"
                    } else {
                        "grouped"
                    }
                ),
            ),
            row(
                "Sharing",
                format!(
                    "{} · usage events {} · threads {}",
                    if c.enrolled { "joined" } else { "not joined" },
                    yes(c.events),
                    yes(c.threads)
                ),
            ),
            row(
                "Paths",
                format!(
                    "settings {}\nlog {}",
                    commands::tilde_path(&crate::paths::config_dir()),
                    commands::tilde_path(&crate::bugreport::tui_log())
                ),
            ),
        ]
    }

    /// `/status` as lines of `key: value`, as the clipboard gets it.
    #[cfg(test)]
    fn status_lines(&self) -> Vec<String> {
        Entry::Info(self.status_rows())
            .copy_text()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A `/status` line for one tier: the model that runs, and the one
    /// configured when that differs.
    fn tier_line(&self, tier: &crate::config::TierPick) -> String {
        let using = tier.entry.as_ref().map_or_else(
            || "none (/login connects an account)".to_string(),
            |m| format!("{} ({})", m.label, crate::config::Account::of(m).header()),
        );
        match (&tier.wanted, tier.fallback) {
            (Some(wanted), true) if self.refreshing_models => format!(
                "{using} · configured {wanted}, not available until the model list refreshes"
            ),
            (Some(wanted), true) if self.models_unlisted => format!(
                "{using} · configured {wanted}, not listed: the model list couldn't refresh (`pathfinder models --refresh` shows why)"
            ),
            (Some(wanted), true) => {
                format!("{using} · configured {wanted}, which your accounts don't offer")
            }
            (Some(wanted), false) => format!("{using} · configured {wanted}"),
            (None, _) => format!("{using} · picked automatically"),
        }
    }

    /// `/share [note]`: shows exactly what would be sent; Enter sends it.
    fn start_share(&mut self, note: &str) {
        let consent = crate::telemetry::Consent::load();
        if !consent.may_share() {
            self.status = if consent.enrolled {
                "Sharing threads is off. `pathfinder data on --threads` turns it on.".into()
            } else {
                "Sharing needs you to join first: pathfinder data join --threads".into()
            };
            return;
        }
        let session = match self.rt.load_session(&self.session.id) {
            Ok(Some(s)) => s,
            _ => self.session.clone(),
        };
        if session.history.is_empty() {
            self.status = "This thread is empty; there's nothing to share.".into();
            return;
        }
        let turns: Vec<(String, String)> = session
            .history
            .iter()
            .map(|t| (t.role.clone(), t.content.clone()))
            .collect();
        let thread = crate::telemetry::share::thread(&session.id, &turns, &self.cwd, note);
        let lines = crate::telemetry::share::preview(&thread);
        self.modal = Some(Modal::Share {
            outgoing: Outgoing::Thread(thread),
            lines,
            scroll: 0,
            busy: false,
        });
    }

    /// `/bug [what happened]`: shows the report in full; Enter sends it.
    /// Without a link to the collector, it is saved to a file instead.
    fn start_bug(&mut self, note: &str) {
        let report = crate::bugreport::build(note, &self.cwd);
        let consent = crate::telemetry::Consent::load();
        if !consent.enrolled {
            self.status = match crate::bugreport::save(&report) {
                Ok(path) => format!(
                    "Saved the report to {}: send it to {} or open an issue at {}",
                    path.display(),
                    crate::operator::CONTACT,
                    crate::operator::ISSUES
                ),
                Err(e) => format!("Couldn't save the report: {e}"),
            };
            return;
        }
        let lines = crate::bugreport::preview(&report);
        self.modal = Some(Modal::Share {
            outgoing: Outgoing::Bug(report),
            lines,
            scroll: 0,
            busy: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{MapChange, Part, PartStatus, PendingCandidate, ProposedPart};
    use crate::engine::{BoxFuture, Engine, EngineRequest};
    use crate::store::Store;

    struct UnusedEngine;

    impl Engine for UnusedEngine {
        fn run_text<'a>(
            &'a self,
            _req: &'a EngineRequest,
            _sink: StreamSink,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok(String::new()) })
        }
    }

    fn test_app() -> (App, mpsc::UnboundedReceiver<AppEvent>, Arc<Store>) {
        let dir = std::env::temp_dir().join(format!("pf-tui-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Store::open_in_memory().unwrap());
        let rt = Arc::new(Runtime::new(store.clone(), Arc::new(UnusedEngine)));
        let session = rt
            .create_session(uuid::Uuid::new_v4().to_string(), dir.clone())
            .unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        (App::new(rt, dir, session, tx), rx, store)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn burst(keys: &[KeyEvent]) -> Vec<Event> {
        keys.iter()
            .flat_map(|k| {
                let mut up = *k;
                up.kind = KeyEventKind::Release;
                [Event::Key(*k), Event::Key(up)]
            })
            .collect()
    }

    fn a_source() -> Entry {
        Entry::Source {
            title: "app/x.rs".into(),
            range: "lines 20–21 of 391".into(),
            body: "```rust numbered\n   20      name: \"probe\",\n   21  },\n```\n22–391 not shown · `/open app/x.rs:22-81`".into(),
        }
    }

    #[tokio::test]
    async fn status_is_one_info_block_after_the_command() {
        let (mut app, _rx, _store) = test_app();
        app.input = "/status".into();
        app.cursor = app.input.len();
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(&app.entries[0], Entry::User(t) if t == "/status"));
        let Some(Entry::Info(rows)) = app.entries.get(1) else {
            panic!("no info block");
        };
        let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        for k in [
            "Version",
            "Fast model",
            "Main model",
            "Gate",
            "Display",
            "Paths",
        ] {
            assert!(keys.contains(&k), "{keys:?}");
        }
        assert_eq!(app.entries.len(), 2);
    }

    #[tokio::test]
    async fn copy_takes_replies_only_and_says_how_to_copy_a_source() {
        let (mut app, _rx, _store) = test_app();
        app.entries = vec![
            Entry::User("/open app/x.rs:20-21".into()),
            a_source(),
            Entry::Note("No file `y` in this workspace.".into()),
        ];
        app.run_local(commands::Local::Copy, "");
        assert_eq!(
            app.status,
            "There's no reply to copy yet · Tab selects a source, y copies it"
        );
        app.entries.clear();
        app.run_local(commands::Local::Copy, "");
        assert_eq!(app.status, "There's no reply to copy yet.");
    }

    #[test]
    fn a_source_copies_its_code_and_hits_copy_as_places() {
        let source = a_source();
        assert_eq!(source.copy_text(), "    name: \"probe\",\n},");
        assert_eq!(
            source.copy_with_place(),
            "app/x.rs:20-21\n    name: \"probe\",\n},"
        );
        let hits = Entry::Search {
            query: "gate".into(),
            hits: vec![crate::sources::Hit {
                at: crate::sources::Locator::Local {
                    file: "a.rs".into(),
                    page: None,
                    line: 3,
                },
                quote: "    let gate = 1;".into(),
            }],
            total: 1,
        };
        assert_eq!(hits.copy_text(), "a.rs:3  let gate = 1;");
        assert_eq!(hits.copy_with_place(), hits.copy_text());
    }

    #[tokio::test]
    async fn a_resumed_thread_shows_sources_and_hits_again() {
        let (mut app, _rx, _store) = test_app();
        std::fs::write(app.cwd.join("x.rs"), "fn a() {}\n").unwrap();
        let opened = crate::sources::open(&app.cwd, "x.rs", None).unwrap();
        let s = &mut app.session;
        s.push_turn("user", "/open x.rs");
        s.push_turn("assistant", &opened);
        s.push_turn("user", "/open nope.rs");
        s.push_turn("assistant", "No file `nope.rs` in this workspace.");
        s.push_turn("user", "/find gate here");
        s.push_turn(
            "assistant",
            "**1/31** for `gate here`:\n\n- `x.rs:1`  gate here",
        );
        s.push_turn("user", "why?");
        s.push_turn("assistant", "Because.");
        let entries = entries_of(&app.session);
        assert!(matches!(&entries[1], Entry::Source { title, range, .. }
                if title == "x.rs" && range == "lines 1–1 of 1"));
        assert!(matches!(&entries[3], Entry::Note(_)));
        assert!(matches!(&entries[5], Entry::Search { query, hits, total }
                if query == "gate here" && hits.len() == 1 && *total == 31));
        assert!(matches!(&entries[7], Entry::Text(t) if t == "Because."));
    }

    #[test]
    fn keys_waiting_together_with_a_line_break_are_a_paste() {
        let keys = [
            key(KeyCode::Char('a')),
            shift(KeyCode::Char('B')),
            key(KeyCode::Enter),
            KeyEvent::new(
                KeyCode::Char('@'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        ];
        assert_eq!(paste_burst(&burst(&keys), false).as_deref(), Some("aB\n@"));
        // A lone Enter is a send, unless it follows a paste straight on.
        assert_eq!(paste_burst(&burst(&[key(KeyCode::Enter)]), false), None);
        assert_eq!(
            paste_burst(&burst(&[key(KeyCode::Enter)]), true).as_deref(),
            Some("\n")
        );
        // Typing without a line break stays typing.
        let typed = [key(KeyCode::Char('a')), key(KeyCode::Char('b'))];
        assert_eq!(paste_burst(&burst(&typed), false), None);
        // Shortcuts and other keys are never pasted.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            paste_burst(&burst(&[ctrl_c, key(KeyCode::Enter)]), true),
            None
        );
        assert_eq!(
            paste_burst(&burst(&[key(KeyCode::Esc), key(KeyCode::Enter)]), false),
            None
        );
    }

    #[test]
    fn dropped_paths_are_attached_when_they_exist() {
        let dir = std::env::temp_dir().join(format!("pf-tui-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("my sheet.pdf"), b"%PDF").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let text = format!("look at {}/my\\ sheet.pdf please notes.txt", dir.display());
        assert_eq!(dropped_files(&text, &dir), vec![dir.join("my sheet.pdf")]);
    }

    #[test]
    fn readline_alt_and_ctrl_keys_work_without_stray_chars() {
        let (mut app, _rx, _) = test_app();
        app.input = "hello world\nsecond line".into();
        app.cursor = 2; // he|llo world\nsecond line

        // Ctrl+E moves to end of current line (before '\n'), Ctrl+A to start.
        app.on_key(ctrl(KeyCode::Char('e')));
        assert_eq!(app.cursor, 11);
        app.on_key(ctrl(KeyCode::Char('a')));
        assert_eq!(app.cursor, 0);

        // End / Home also move to line end / line start.
        app.on_key(key(KeyCode::End));
        assert_eq!(app.cursor, 11);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.cursor, 0);

        // Alt+f, Alt+b, Alt+d word navigation and delete-word-forward.
        app.on_key(alt(KeyCode::Char('f')));
        assert_eq!(app.cursor, 5);
        app.on_key(alt(KeyCode::Char('d')));
        assert_eq!(app.input, "hello\nsecond line");
        app.on_key(alt(KeyCode::Char('b')));
        assert_eq!(app.cursor, 0);

        // Stray Alt+n / Alt+p must not insert characters.
        app.on_key(alt(KeyCode::Char('n')));
        app.on_key(alt(KeyCode::Char('p')));
        assert_eq!(app.input, "hello\nsecond line");

        // Ctrl+D forward-deletes when non-empty; quits only when empty.
        app.on_key(ctrl(KeyCode::Char('d')));
        assert_eq!(app.input, "ello\nsecond line");
        assert!(!app.quit);

        // Ctrl+G toggles tool grouping; Ctrl+E no longer toggles it.
        assert!(!app.show_tools);
        app.on_key(ctrl(KeyCode::Char('g')));
        assert!(app.show_tools);

        // Shift+Enter inserts newline.
        app.cursor = app.input.len();
        app.on_key(shift(KeyCode::Enter));
        assert!(app.input.ends_with('\n'));

        app.input.clear();
        app.cursor = 0;
        app.on_key(ctrl(KeyCode::Char('d')));
        assert!(!app.quit, "the first ^D at the bottom only asks");
        app.on_key(ctrl(KeyCode::Char('d')));
        assert!(app.quit);
    }

    #[test]
    fn multiline_up_down_moves_lines_before_history() {
        let (mut app, _rx, _) = test_app();
        app.input_width = 10;
        app.input_history = vec!["first".into(), "second".into()];
        app.input = "abcde\nfg".into();
        app.cursor = app.input.len(); // row 1, col 2

        // Up moves from row 1 col 2 to row 0 col 2.
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.cursor, 2);
        assert_eq!(app.input, "abcde\nfg");

        // Up on row 0 recalls previous history and saves draft.
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input, "second");
        assert_eq!(app.history_idx, Some(1));

        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input, "first");
        assert_eq!(app.history_idx, Some(0));

        // Down restores newer history and then original draft.
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input, "second");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input, "abcde\nfg");
        assert_eq!(app.history_idx, None);
    }

    #[test]
    fn history_walks_past_commands_without_the_palette_taking_the_arrows() {
        let (mut app, _rx, _) = test_app();
        app.input_history = vec!["/status".into(), "/help".into()];
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input, "/help");
        // A recalled command opens no menu, so the arrows keep walking.
        assert!(!app.palette_open());
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.input, "/status");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input, "/help");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.input, "");
        assert_eq!(app.history_idx, None);
        // Editing a recalled command brings the palette back.
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input, "/hel");
        assert!(app.palette_open());
    }

    #[tokio::test]
    async fn one_path_menu_for_open_check_and_attach_and_tab_never_guesses() {
        let (mut app, _rx, _) = test_app();
        std::fs::create_dir_all(app.cwd.join("apps")).unwrap();
        std::fs::write(app.cwd.join("apps/x.rs"), "fn x() {}\n").unwrap();
        std::fs::write(app.cwd.join("apple.rs"), "fn a() {}\n").unwrap();
        std::fs::write(app.cwd.join("apricot.md"), "# A\n").unwrap();
        let type_in = |app: &mut App, text: &str| {
            app.input = text.into();
            app.cursor = app.input.len();
            app.cmd_sel = 0;
        };

        // Three matches, no common start: Tab keeps the input and the menu.
        type_in(&mut app, "/open ap");
        assert_eq!(app.path_menu().len(), 3);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input, "/open ap");
        assert!(app.status.contains("3 matches"), "{}", app.status);
        // A row picked with ↑↓ is what Tab completes.
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input, "/open apple.rs");

        // /check has the same menu; a file fills in and waits.
        type_in(&mut app, "/check ap");
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input, "/check apricot.md");
        assert!(app.entries.is_empty());

        // Enter on a folder goes into it; on a file /open runs.
        type_in(&mut app, "/open ap");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input, "/open apps/");
        assert!(app.entries.is_empty());
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input, "");
        assert!(
            matches!(app.entries.first(), Some(Entry::User(t)) if t == "/open apps/x.rs"),
            "{:?}",
            app.entries.first().map(|_| ())
        );

        // Esc closes the menu and keeps the text; Tab opens it again.
        type_in(&mut app, "/attach ap");
        assert!(!app.path_menu().is_empty());
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.input, "/attach ap");
        assert!(app.path_menu().is_empty());
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input, "/attach ap");
        assert!(!app.path_menu().is_empty());
    }

    #[tokio::test]
    async fn enter_on_a_loose_palette_match_fills_in_and_never_runs_a_model_command() {
        for typed in ["/ask", "/explain", "/agent", "/smry"] {
            let (mut app, _rx, _) = test_app();
            for c in typed.chars() {
                app.on_key(key(KeyCode::Char(c)));
            }
            assert!(app.palette_open(), "{typed}");
            app.on_key(key(KeyCode::Enter));
            assert!(app.entries.is_empty(), "{typed} ran something");
            assert!(app.running.is_none(), "{typed} started a turn");
            assert!(app.input.starts_with('/') && app.input != typed, "{typed}");
            assert!(
                app.palette_open(),
                "{typed} -> {:?}: the filled command stays in view",
                app.input
            );
        }
        // A prefix of a local command still runs on Enter.
        let (mut app, _rx, _) = test_app();
        for c in "/stat".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert!(app.input.is_empty());
        assert!(!app.entries.is_empty());
    }

    #[tokio::test]
    async fn an_esc_dismissal_and_a_picked_row_end_with_the_next_edit() {
        let (mut app, _rx, _) = test_app();
        std::fs::write(app.cwd.join("apple.rs"), "fn a() {}\n").unwrap();
        std::fs::write(app.cwd.join("apricot.md"), "# A\n").unwrap();
        std::fs::write(app.cwd.join("apz.md"), "# Z\n").unwrap();
        let typed = |app: &mut App, text: &str| {
            for c in text.chars() {
                app.on_key(key(KeyCode::Char(c)));
            }
        };
        // `/`, Esc, Backspace, `/` again: the palette is back.
        typed(&mut app, "/");
        app.on_key(key(KeyCode::Esc));
        assert!(!app.palette_open());
        app.on_key(key(KeyCode::Backspace));
        typed(&mut app, "/");
        assert!(app.palette_open());
        // The same with the path menu and ^U.
        app.on_key(key(KeyCode::Backspace));
        typed(&mut app, "/open ap");
        app.on_key(key(KeyCode::Esc));
        assert!(app.path_menu().is_empty());
        app.on_key(ctrl(KeyCode::Char('u')));
        typed(&mut app, "/open ap");
        assert_eq!(app.path_menu().len(), 3);
        // A row picked with ↑↓, then an edit back to the same text: Tab
        // doesn't complete the old pick.
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Backspace));
        typed(&mut app, "p");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input, "/open ap");
        assert!(app.status.contains("3 matches"), "{}", app.status);
    }

    #[test]
    fn the_chooser_cursor_leaves_the_current_value_for_a_better_match() {
        let (mut app, _rx, _) = test_app();
        let items: Vec<(String, String)> = [
            ("", "Claude Code · Claude subscription · own login"),
            ("haiku", "Haiku  ✓"),
            ("sonnet", "Sonnet"),
            ("opus", "Opus"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        app.modal = Some(Modal::Choose {
            option: crate::options::FAST_MODEL,
            title: "Fast model".into(),
            sel: chooser_sel(&items, ""),
            items,
            query: String::new(),
        });
        for c in "son".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        let Some(Modal::Choose {
            items, query, sel, ..
        }) = &app.modal
        else {
            panic!("expected the chooser");
        };
        let matched = filter_matches(items, query);
        // The header's letters in order don't pull in every model.
        assert_eq!(matched.len(), 1);
        assert_eq!(items[matched[*sel]].0, "sonnet");
    }

    #[test]
    fn help_pages_by_its_height_and_q_filters_once_something_is_typed() {
        let (mut app, _rx, _) = test_app();
        app.modal = Some(Modal::help());
        app.help_max_scroll = 100;
        app.help_page = 20;
        app.on_key(key(KeyCode::PageDown));
        assert!(matches!(app.modal, Some(Modal::Help { scroll: 20, .. })));
        for c in "scroll".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Char('q')));
        assert!(
            matches!(&app.modal, Some(Modal::Help { query, .. }) if query == "scrollq"),
            "q filters while something is typed"
        );
        app.on_key(ctrl(KeyCode::Char('u')));
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.modal.is_none(), "q closes with nothing typed");
    }

    #[tokio::test]
    async fn a_file_named_in_a_command_is_not_attached_as_dropped() {
        let (mut app, _rx, _) = test_app();
        std::fs::write(app.cwd.join("x.pdf"), b"%PDF").unwrap();
        app.input = "/open x.pdf".into();
        app.cursor = app.input.len();
        app.palette_dismissed = Some(app.input.clone());
        app.on_key(key(KeyCode::Enter));
        let Some(Entry::User(shown)) = app.entries.first() else {
            panic!("the command is echoed");
        };
        assert_eq!(shown, "/open x.pdf");
    }

    #[test]
    fn the_palette_keys_walk_its_sections_and_rank_as_typed() {
        let (mut app, _rx, _) = test_app();
        app.on_key(key(KeyCode::Char('/')));
        assert!(app.palette_open());
        // Bare `/`: what the fresh thread asks for first (/find, /plan),
        // then the sections; ↑↓ skip the headers.
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.cmd_sel, 1);
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input, "/open ");
        // ↑ from the top wraps to the last row.
        app.on_key(ctrl(KeyCode::Char('u')));
        app.on_key(key(KeyCode::Char('/')));
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input, "/quit ");
        // Typed: a prefix leads; a word of what it does when no name fits.
        app.on_key(ctrl(KeyCode::Char('u')));
        for c in "/sear".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input, "/web ", "a command with arguments fills in");
        assert!(app.entries.is_empty());
    }

    #[test]
    fn esc_closes_menus_and_stops_but_never_clears_the_draft() {
        let (mut app, _rx, _) = test_app();
        app.input = "/he".into();
        app.cursor = 3;
        assert!(app.palette_open());
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.input, "/he");
        assert!(!app.palette_open());
        // Dismissed until the next edit.
        app.on_key(key(KeyCode::Left));
        assert!(!app.palette_open());
        app.on_key(key(KeyCode::End));
        app.on_key(key(KeyCode::Char('l')));
        assert!(app.palette_open());

        app.input = "half a thought".into();
        app.cursor = app.input.len();
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.input, "half a thought");
        let cancel = CancellationToken::new();
        app.running = Some(cancel.clone());
        app.on_key(key(KeyCode::Esc));
        assert!(cancel.is_cancelled());
        assert_eq!(app.input, "half a thought");
        // Ctrl+U still clears the line.
        app.on_key(ctrl(KeyCode::Char('u')));
        assert_eq!(app.input, "");
    }

    fn pending_handoff() -> crate::domain::PendingCommitment {
        crate::domain::PendingCommitment {
            kind: crate::domain::CommitKind::Handoff,
            part: "p".into(),
            text: "do it".into(),
            files: vec![],
            candidate: None,
            then: Vec::new(),
        }
    }

    fn last_sent(app: &App) -> Option<&str> {
        app.entries.iter().rev().find_map(|e| match e {
            Entry::User(t) => Some(t.as_str()),
            _ => None,
        })
    }

    #[tokio::test]
    async fn enter_on_empty_input_confirms_a_pending_commitment() {
        let (mut app, _rx, _) = test_app();
        // Nothing waits: Enter on nothing sends nothing.
        app.on_key(key(KeyCode::Enter));
        assert!(app.entries.is_empty() && app.running.is_none());
        app.session.pending_commitment = Some(pending_handoff());
        app.on_key(key(KeyCode::Enter));
        // Your intent shows in the thread, not the command the key sends.
        assert_eq!(last_sent(&app), Some("yes, hand it off"));
        assert_eq!(app.last_sent, "/confirm");
        assert!(app.running.is_some());
        // What it answered no longer waits on you while it runs: the rule
        // and the panel drop "your OK" now (the reload at the end has the
        // truth).
        assert!(app.session.pending_commitment.is_none());
        // While it runs, Enter on nothing sends nothing more.
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.entries.len(), 1);
    }

    #[tokio::test]
    async fn a_confirming_key_shows_your_intent_per_kind() {
        use crate::domain::CommitKind;
        for (kind, shown) in [
            (CommitKind::Delegate, "yes, delegate it"),
            (CommitKind::Verdict, "yes, judge it"),
        ] {
            let (mut app, _rx, _) = test_app();
            app.session.pending_commitment = Some(crate::domain::PendingCommitment {
                kind,
                ..pending_handoff()
            });
            app.on_key(key(KeyCode::Enter));
            assert_eq!(last_sent(&app), Some(shown));
            assert_eq!(app.last_sent, "/confirm");
            assert!(
                !app.entries
                    .iter()
                    .any(|e| matches!(e, Entry::User(t) if t.starts_with('/')))
            );
        }
    }

    #[tokio::test]
    async fn enter_with_text_still_submits_it() {
        let (mut app, _rx, _) = test_app();
        app.session.pending_commitment = Some(pending_handoff());
        app.input = "wait, which part?".into();
        app.cursor = app.input.len();
        app.on_key(key(KeyCode::Enter));
        assert_eq!(last_sent(&app), Some("wait, which part?"));
    }

    #[tokio::test]
    async fn esc_closes_a_menu_before_cancelling() {
        let (mut app, _rx, _) = test_app();
        app.session.pending_commitment = Some(pending_handoff());
        app.input = "/he".into();
        app.cursor = 3;
        assert!(app.palette_open());
        app.on_key(key(KeyCode::Esc));
        assert!(app.entries.is_empty());
        // A draft is never cancelled away: Esc leaves it and what waits.
        app.input = "half a thought".into();
        app.cursor = app.input.len();
        app.on_key(key(KeyCode::Esc));
        assert!(app.entries.is_empty());
        assert_eq!(app.input, "half a thought");
        // A running reply is stopped first; nothing is sent.
        app.input.clear();
        app.cursor = 0;
        let cancel = CancellationToken::new();
        app.running = Some(cancel.clone());
        app.on_key(key(KeyCode::Esc));
        assert!(cancel.is_cancelled());
        assert!(app.entries.is_empty());
        app.running = None;
        app.on_key(key(KeyCode::Esc));
        // Your intent shows in the thread; the command is what runs.
        assert_eq!(last_sent(&app), Some("no"));
        assert_eq!(app.last_sent, "/cancel");
    }

    #[tokio::test]
    async fn esc_takes_back_a_claim_only_in_the_next_turn() {
        let (mut app, _rx, _) = test_app();
        app.on_key(key(KeyCode::Esc));
        assert!(app.entries.is_empty());
        app.session.turn = 2;
        app.session.last_commitment = Some(crate::domain::Undoable {
            part: "p".into(),
            events: vec![(1, "c".into())],
            turn: 0,
            prev_active: None,
            prev_mode: crate::domain::SessionMode::Working,
            prev_change: None,
            prev_gaps: vec![],
            also: vec![],
        });
        // Too late: a step has passed since the claim.
        app.on_key(key(KeyCode::Esc));
        assert!(app.entries.is_empty());
        app.session.last_commitment.as_mut().unwrap().turn = 1;
        app.on_key(key(KeyCode::Esc));
        assert_eq!(last_sent(&app), Some("/undo"));
    }

    #[test]
    fn pages_follow_the_visible_height_and_toggles_say_what_they_did() {
        let (mut app, _rx, _) = test_app();
        app.conv_area = Rect::new(0, 0, 80, 20);
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.scroll, 18);
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.scroll, 36);
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.scroll, 18);

        app.on_key(ctrl(KeyCode::Char('t')));
        assert!(app.show_thoughts);
        assert_eq!(app.status, "Showing thinking.");
        app.on_key(ctrl(KeyCode::Char('t')));
        assert_eq!(app.status, "Hiding thinking.");
        app.on_key(ctrl(KeyCode::Char('g')));
        assert!(app.show_tools);
        assert_eq!(app.status, "Showing every tool call.");
    }

    #[test]
    fn unknown_commands_point_at_the_closest_and_stay_out_of_history() {
        let (mut app, _rx, _) = test_app();
        app.input = "/opne README.md".into();
        app.cursor = app.input.len();
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.status, "No such command · Tab → /open");
        assert_eq!(app.input, "/opne README.md");
        assert!(app.input_history.is_empty());
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input, "/open README.md");
        assert_eq!(app.cursor, "/open".len());

        app.input = "/frobnicate".into();
        app.cursor = app.input.len();
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.status, "No such command · /help lists them");
        assert!(app.input_history.is_empty());
    }

    #[test]
    fn mouse_wheel_scrolls_even_with_non_empty_input_and_f2_toggles() {
        let (mut app, _rx, _) = test_app();
        app.input = "typing something".into();
        app.conv_area = Rect::new(0, 0, 80, 20);
        app.side_area = Rect::new(80, 0, 30, 20);
        app.side_max_scroll = 10;

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll, 3);

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll, 0);

        // Hovering side panel scrolls side_scroll.
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 85,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.side_scroll, 3);

        assert!(app.mouse_capture);
        app.on_key(key(KeyCode::F(2)));
        assert!(!app.mouse_capture);
        assert!(app.status.contains("selection"));
    }

    #[test]
    fn tab_does_not_hijack_mid_typing_and_nav_preserves_scroll() {
        let (mut app, _rx, _) = test_app();
        app.items = vec![0, 1];
        app.entries = vec![Entry::User("hi".into()), Entry::Text("reply".into())];
        app.input = "hello".into();
        app.cursor = 5;

        // Tab mid-typing must not enter nav mode.
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.selected, None);

        // Ctrl+Up enters nav mode even with non-empty input.
        app.on_key(ctrl(KeyCode::Up));
        assert_eq!(app.selected, Some(1));

        // Ctrl+C in nav mode copies without quitting or resetting scroll.
        app.scroll = 7;
        app.on_key(ctrl(KeyCode::Char('c')));
        assert!(!app.quit);
        assert_eq!(app.scroll, 7);

        // Esc in nav mode exits without resetting scroll.
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.selected, None);
        assert_eq!(app.scroll, 7);

        // Tab enters nav mode when input is empty.
        app.input.clear();
        app.cursor = 0;
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.selected, Some(1));
    }

    #[tokio::test]
    async fn command_palette_navigation_completion_and_status_clearing() {
        let (mut app, _rx, _) = test_app();
        app.status = "Stale status".into();
        app.input = "/he".into();
        app.cursor = 3;

        // Keystroke clears transient status; Down/Tab navigates palette.
        app.on_key(key(KeyCode::Char('l')));
        assert!(app.status.is_empty());
        assert_eq!(app.input, "/hel");

        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.input, "/help ");

        // Partial prefix + Enter on non-arg command opens Help modal directly.
        app.input = "/hel".into();
        app.cursor = 4;
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.modal, Some(Modal::Help { .. })));

        // Partial prefix + Enter on arg-requiring command completes with trailing space.
        app.modal = None;
        app.input = "/focu".into();
        app.cursor = 5;
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.input, "/focus ");
    }

    #[test]
    fn permission_modal_does_not_auto_approve_on_enter_and_allows_scroll_and_ctrl_c() {
        let (mut app, _rx, _) = test_app();
        let (tx, mut rx) = oneshot::channel();
        app.running = Some(CancellationToken::new());
        app.modal = Some(Modal::Permission {
            title: "Run `rm -rf`".into(),
            always_key: "exec:rm".into(),
            sel: None,
            reply: Some(tx),
        });

        // Pressing Enter immediately when modal appears does nothing.
        app.on_key(key(KeyCode::Enter));
        assert!(app.modal.is_some());
        assert!(rx.try_recv().is_err());

        // PageUp scrolls conversation while Permission is open.
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.scroll, 10);
        assert!(app.modal.is_some());

        // Arrow down selects choice 0 (Once), then Enter confirms.
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Enter));
        assert!(app.modal.is_none());
        assert_eq!(rx.try_recv().unwrap(), Permission::Once);

        // Ctrl+C denies permission and cancels running token.
        let (tx2, mut rx2) = oneshot::channel();
        let token = CancellationToken::new();
        app.running = Some(token.clone());
        app.modal = Some(Modal::Permission {
            title: "Run `ls`".into(),
            always_key: "exec:ls".into(),
            sel: None,
            reply: Some(tx2),
        });
        app.on_key(ctrl(KeyCode::Char('c')));
        assert!(app.modal.is_none());
        assert_eq!(rx2.try_recv().unwrap(), Permission::Denied);
        assert!(token.is_cancelled());
    }

    #[test]
    fn modals_paste_filtering_scroll_clamping_and_settings_sel_preserved() {
        let (mut app, _rx, _) = test_app();

        // Paste normalizes CRLF -> LF and exits nav mode.
        app.selected = Some(0);
        app.on_paste("line1\r\nline2\rline3");
        assert_eq!(app.selected, None);
        assert_eq!(app.input, "line1\nline2\nline3");

        // Paste into Consent Invite step.
        app.modal = Some(Modal::Consent {
            events: false,
            threads: false,
            step: ConsentStep::Invite(String::new()),
            error: None,
            busy: false,
            scroll: 0,
        });
        app.on_paste("INVITE-123\r\n");
        match &app.modal {
            Some(Modal::Consent {
                step: ConsentStep::Invite(code),
                ..
            }) => assert_eq!(code, "INVITE-123"),
            _ => panic!("expected Consent Invite"),
        }

        // Esc closes Consent even when busy == true.
        app.modal = Some(Modal::Consent {
            events: true,
            threads: false,
            step: ConsentStep::Invite("ABC".into()),
            error: None,
            busy: true,
            scroll: 0,
        });
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());

        // Esc and Ctrl+C close Share even when busy == true.
        let dummy_bug = crate::bugreport::build("test", &app.cwd);
        app.modal = Some(Modal::Share {
            outgoing: Outgoing::Bug(dummy_bug.clone()),
            lines: vec!["line1".into(), "line2".into()],
            scroll: 0,
            busy: true,
        });
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());

        app.modal = Some(Modal::Share {
            outgoing: Outgoing::Bug(dummy_bug),
            lines: vec!["line1".into(), "line2".into()],
            scroll: 0,
            busy: true,
        });
        app.on_key(ctrl(KeyCode::Char('c')));
        assert!(app.modal.is_none());

        // Ctrl+End jumps scroll to 0 even when input is non-empty.
        app.input = "some text".into();
        app.cursor = 2;
        app.scroll = 12;
        app.on_key(ctrl(KeyCode::End));
        assert_eq!(app.scroll, 0);
        assert_eq!(app.cursor, 9);
        app.on_key(ctrl(KeyCode::Home));
        assert_eq!(app.cursor, 0);
        app.input.clear();

        // Help scroll clamping.
        app.help_max_scroll = 15;
        app.modal = Some(Modal::Help {
            scroll: 12,
            query: String::new(),
        });
        app.on_key(key(KeyCode::PageDown));
        match app.modal {
            Some(Modal::Help { scroll, .. }) => assert_eq!(scroll, 15),
            _ => panic!("expected Help"),
        }

        // A setting with a few values changes in place: Enter, Space and
        // → step forward, ← back, and the row stays selected.
        app.modal = Some(Modal::Settings { sel: 3 });
        assert_eq!(SETTINGS[3].0, crate::options::MAIN_EFFORT);
        let before = app.current(crate::options::MAIN_EFFORT);
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.modal, Some(Modal::Settings { sel: 3 })));
        let after = app.current(crate::options::MAIN_EFFORT);
        assert_ne!(before, after);
        assert!(
            app.status.starts_with("Main model effort set"),
            "{}",
            app.status
        );
        app.on_key(key(KeyCode::Left));
        assert_eq!(app.current(crate::options::MAIN_EFFORT), before);
        app.on_key(key(KeyCode::Char(' ')));
        app.on_key(key(KeyCode::Right));
        app.on_key(key(KeyCode::Left));
        app.on_key(key(KeyCode::Left));
        assert_eq!(app.current(crate::options::MAIN_EFFORT), before);

        // Settings sel preserved on Esc from Choose, plus type-to-filter.
        app.modal = Some(Modal::Choose {
            option: crate::options::MAIN_EFFORT,
            title: "Main model effort".into(),
            items: app.choices(crate::options::MAIN_EFFORT),
            query: String::new(),
            sel: 0,
        });
        app.on_key(key(KeyCode::Char('h')));
        match &app.modal {
            Some(Modal::Choose { query, items, .. }) => {
                assert_eq!(query, "h");
                assert_eq!(filter_matches(items, query).len(), 1);
            }
            _ => panic!("expected Choose"),
        }
        app.on_key(key(KeyCode::Esc));
        match app.modal {
            Some(Modal::Settings { sel }) => assert_eq!(sel, 3),
            _ => panic!("expected Settings with sel=3"),
        }
    }

    #[test]
    fn login_commands_open_the_dialog_and_errors_point_at_it() {
        use commands::{Local, Parsed};
        assert_eq!(commands::parse("/login"), Parsed::Local(Local::Login, ""));
        assert_eq!(
            commands::parse("/logout openai"),
            Parsed::Local(Local::Logout, "openai")
        );
        let (mut app, _rx, _) = test_app();
        app.run_local(Local::Login, "nope");
        assert!(app.modal.is_none());
        assert!(app.status.contains("No account `nope`"), "{}", app.status);
        app.run_local(Local::Login, "");
        assert!(matches!(app.modal, Some(Modal::Login(_))));
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());
        assert!(
            login_hint("OpenAI needs an API key. Run `pathfinder auth set openai` in a terminal.")
                .ends_with("Here, /login does it.")
        );
        assert_eq!(login_hint("timeout"), "timeout");
    }

    /// Stands in for a running `/login` job (nothing real runs), as job `id`.
    fn fake_login_job(app: &mut App, id: u64) {
        let handle = tokio::spawn(std::future::pending::<()>()).abort_handle();
        app.login_jobs = id;
        app.login_task = Some((id, handle));
    }

    #[tokio::test]
    async fn a_finished_login_job_closes_or_goes_back_with_why() {
        use crate::auth::accounts::{Row, RowView, State};
        use crate::auth::flow::{Action, Flow, Job, Mode};
        let views = vec![RowView {
            stored: Some("…ABCD".into()),
            ..RowView::new(Row::Gate, State::NotSet)
        }];
        let (mut app, _rx, _) = test_app();
        // Signing out asks first.
        let (mut flow, action) = Flow::at(Mode::Logout, views.clone(), &Row::Gate);
        assert_eq!(action, Action::None);
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::SignOut(Row::Gate))
        );
        app.modal = Some(Modal::Login(Box::new(flow)));
        fake_login_job(&mut app, 1);
        app.on_event(AppEvent::Login(LoginEvent::Done(
            1,
            Ok("Removed it.".into()),
        )));
        assert!(app.modal.is_none());
        assert!(matches!(app.entries.last(), Some(Entry::Note(n)) if n == "Removed it."));

        let (mut flow, _) = Flow::at(Mode::Login, views, &Row::Gate);
        flow.on_paste("ts-0123456789abcdefghij");
        assert!(matches!(flow.on_key(key(KeyCode::Enter)), Action::Run(_)));
        app.modal = Some(Modal::Login(Box::new(flow)));
        fake_login_job(&mut app, 2);
        app.on_event(AppEvent::Login(LoginEvent::Url(
            2,
            "https://x.example".into(),
        )));
        app.on_event(AppEvent::Login(LoginEvent::Done(
            2,
            Err("401: bad key".to_string().into()),
        )));
        let Some(Modal::Login(flow)) = &app.modal else {
            panic!("closed")
        };
        assert_eq!(flow.error.as_deref(), Some("401: bad key"));
        assert!(!flow.busy());
    }

    #[tokio::test]
    async fn cancelling_a_sign_in_closes_and_the_next_key_reaches_the_input() {
        use crate::auth::accounts::{Row, RowView, State};
        use crate::auth::flow::{Action, Flow, Job, Mode};
        let views = vec![RowView::new(Row::Chatgpt, State::NotSignedIn)];
        let (mut app, _rx, _) = test_app();
        let (flow, action) = Flow::at(Mode::Login, views.clone(), &Row::Chatgpt);
        assert_eq!(action, Action::Run(Job::SignInChatgpt));
        app.modal = Some(Modal::Login(Box::new(flow)));
        fake_login_job(&mut app, 1);
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());
        assert!(app.login_task.is_none());
        // The cancelled job's late answer changes nothing.
        app.on_event(AppEvent::Login(LoginEvent::Done(1, Ok("Signed in".into()))));
        app.on_key(key(KeyCode::Char('/')));
        assert_eq!(app.input, "/");

        // A cancelled job's answer never lands on a newer job.
        app.input.clear();
        app.cursor = 0;
        let mut flow = Flow::login(views);
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::SignInChatgpt)
        );
        app.modal = Some(Modal::Login(Box::new(flow)));
        fake_login_job(&mut app, 2);
        app.on_key(key(KeyCode::Esc));
        let Some(Modal::Login(flow)) = &mut app.modal else {
            panic!("back at the list")
        };
        assert_eq!(flow.error.as_deref(), Some("Cancelled."));
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::SignInChatgpt)
        );
        fake_login_job(&mut app, 3);
        app.on_event(AppEvent::Login(LoginEvent::Done(2, Ok("stale".into()))));
        assert!(matches!(&app.modal, Some(Modal::Login(f)) if f.busy()));
        assert!(app.login_task.is_some());
        app.on_event(AppEvent::Login(LoginEvent::Done(3, Ok("Signed in".into()))));
        assert!(app.modal.is_none());
        assert!(matches!(app.entries.last(), Some(Entry::Note(n)) if n == "Signed in"));
    }

    #[test]
    fn a_permission_question_waits_for_the_sign_in_dialog() {
        use crate::auth::accounts::{Row, RowView, State};
        use crate::auth::flow::{Flow, Mode};
        use crate::commands::Local;
        let (mut app, _rx, _) = test_app();
        let views = vec![RowView::new(Row::AnthropicApi, State::NotSet)];
        let (flow, _) = Flow::at(Mode::Login, views, &Row::AnthropicApi);
        app.modal = Some(Modal::Login(Box::new(flow)));
        app.on_paste("sk-ant-api03-");
        let (reply, mut answer) = oneshot::channel();
        app.on_event(AppEvent::Ask {
            call_id: "c1".into(),
            always_key: "bash:ls".into(),
            reply,
        });
        // 'a' would be "always", '1' "once": here they are the key's.
        for c in "a1y-rest".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        assert!(matches!(app.modal, Some(Modal::Login(_))));
        assert!(answer.try_recv().is_err(), "nothing answered");
        assert!(app.input.is_empty());
        assert!(app.always.is_empty());
        // Closed, the question comes up.
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());
        app.next_modal();
        assert!(matches!(app.modal, Some(Modal::Permission { .. })));

        // While a reply runs, /login waits for it.
        app.modal = None;
        app.running = Some(CancellationToken::new());
        app.run_local(Local::Login, "");
        assert!(app.modal.is_none());
        assert!(app.status.contains("Wait for the reply"), "{}", app.status);
    }

    #[tokio::test]
    async fn the_gate_key_is_set_from_settings_and_comes_back_there() {
        use crate::auth::flow::Mode;
        let (mut app, _rx, _) = test_app();
        let row = gate_row();
        assert_eq!(SETTINGS[row - 1].1, "Decision service");
        assert_eq!(SETTINGS[row], (GATE_KEY, "Gate key"));
        app.modal = Some(Modal::Settings { sel: row });
        app.on_key(key(KeyCode::Enter));
        let Some(Modal::Login(flow)) = &app.modal else {
            panic!("the gate key's steps")
        };
        assert_eq!(flow.mode, Mode::Gate);
        // Esc (back through a replace/remove choice if a key is stored)
        // returns to Settings on the same row.
        for _ in 0..2 {
            if matches!(app.modal, Some(Modal::Login(_))) {
                app.on_key(key(KeyCode::Esc));
            }
        }
        assert!(
            matches!(app.modal, Some(Modal::Settings { sel }) if sel == row),
            "back in Settings"
        );
        assert_eq!(app.status, "Nothing changed.");
        // A finished job comes back there too, with what it did.
        app.modal = Some(Modal::Login(Box::new(crate::auth::flow::Flow::gate(
            Vec::new(),
        ))));
        if let Some(Modal::Login(flow)) = &mut app.modal {
            flow.on_paste("ts-0123456789abcdefghij");
            assert!(matches!(
                flow.on_key(key(KeyCode::Enter)),
                crate::auth::flow::Action::Run(_)
            ));
        }
        fake_login_job(&mut app, 7);
        app.on_event(AppEvent::Login(LoginEvent::Done(7, Ok("Saved it.".into()))));
        assert!(matches!(app.modal, Some(Modal::Settings { sel }) if sel == row));
        assert!(matches!(app.entries.last(), Some(Entry::Note(n)) if n == "Saved it."));
        // Not while a reply runs.
        app.running = Some(CancellationToken::new());
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.modal, Some(Modal::Settings { .. })));
        assert!(app.status.contains("Wait for the reply"), "{}", app.status);
    }

    #[test]
    fn logout_lists_only_what_login_creates() {
        use crate::auth::accounts::{Row, RowView, State};
        use crate::auth::flow::Flow;
        let stored = |row| RowView {
            stored: Some("…ABCD".into()),
            ..RowView::new(row, State::NotSet)
        };
        let flow = Flow::logout(vec![
            stored(Row::Chatgpt),
            stored(Row::AnthropicApi),
            RowView::new(Row::ClaudeCode, State::Installed),
            RowView::new(Row::Endpoint("lab".into()), State::NoKeyNeeded),
            stored(Row::Gate),
        ]);
        let ids: Vec<&str> = flow
            .picker()
            .unwrap()
            .items
            .iter()
            .map(|i| i.0.as_str())
            .collect();
        assert_eq!(ids, ["chatgpt", "anthropic", "endpoint:lab"]);
    }

    #[test]
    fn a_key_given_as_the_account_name_is_never_shown() {
        use crate::commands::Local;
        let (mut app, _rx, _) = test_app();
        app.run_local(Local::Login, "openai sk-proj-SECRETVALUE-0123456789");
        assert!(!app.status.contains("SECRETVALUE"), "{}", app.status);
        app.modal = None;
        app.run_local(Local::Login, "sk-proj-SECRETVALUE-0123456789");
        assert!(app.modal.is_none());
        assert!(!app.status.contains("SECRETVALUE"), "{}", app.status);
        assert!(app.status.contains("looks like a key"), "{}", app.status);
    }

    #[test]
    fn section_headers_are_shown_but_skipped_and_filtered() {
        let items: Vec<(String, String)> = [
            ("", "ChatGPT sign-in · ChatGPT plan"),
            ("chatgpt:gpt-6-luna", "GPT-6 Luna"),
            ("", "Claude Code · Claude subscription"),
            ("claude-haiku", "Haiku"),
            ("claude-sonnet", "Sonnet"),
            ("", "Anthropic API · pay per use"),
            ("anthropic:claude-haiku-5-5", "Claude Haiku 5.5"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(filter_matches(&items, ""), [1, 3, 4, 6]);
        assert_eq!(display_rows(&items, ""), (0..7).collect::<Vec<_>>());
        // A section's header goes only when one of its models matches.
        assert_eq!(filter_matches(&items, "haiku"), [3, 6]);
        assert_eq!(display_rows(&items, "haiku"), [2, 3, 5, 6]);
        // Typing an account's name keeps its whole section.
        assert_eq!(filter_matches(&items, "pay per use"), [6]);
        assert_eq!(filter_matches(&items, "claude code"), [3, 4]);
        assert!(display_rows(&items, "nothing").is_empty());

        // Keys step over the headers: `sel` counts models only.
        let (mut app, _rx, _store) = test_app();
        app.modal = Some(Modal::Choose {
            option: crate::options::MAIN_MODEL,
            title: "Main model".into(),
            items: items.clone(),
            query: String::new(),
            sel: 0,
        });
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        match &app.modal {
            Some(Modal::Choose {
                sel, items, query, ..
            }) => {
                assert_eq!(*sel, 3);
                let idx = filter_matches(items, query)[*sel];
                assert_eq!(items[idx].0, "anthropic:claude-haiku-5-5");
            }
            _ => panic!("expected Choose"),
        }
        app.on_key(key(KeyCode::Home));
        match &app.modal {
            Some(Modal::Choose {
                sel, items, query, ..
            }) => {
                assert!(!is_header(&items[filter_matches(items, query)[*sel]]));
            }
            _ => panic!("expected Choose"),
        }
    }

    fn two_model_config() -> crate::config::Config {
        let entry = |id: &str, label: &str| crate::config::ModelEntry {
            id: id.into(),
            label: label.into(),
            route: crate::config::Route::Anthropic,
            model: id.into(),
            effort: None,
            endpoint: None,
        };
        crate::config::Config {
            models: vec![entry("fast", "Fast One"), entry("main", "Main One")],
            discover: false,
            ..crate::config::Config::default()
        }
    }

    #[test]
    fn the_model_picker_opens_on_the_current_model_in_its_section() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let rt =
            Arc::new(Runtime::new(store, Arc::new(UnusedEngine)).with_config(two_model_config));
        let session = rt.create_session("s".into(), PathBuf::from(".")).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(rt, PathBuf::from("."), session, tx);
        let main = SETTINGS
            .iter()
            .position(|(o, _)| *o == crate::options::MAIN_MODEL)
            .unwrap();
        app.modal = Some(Modal::Settings { sel: main });
        app.on_key(key(KeyCode::Enter));
        match &app.modal {
            Some(Modal::Choose {
                items, sel, query, ..
            }) => {
                assert_eq!(
                    items[0],
                    (String::new(), "Anthropic API · pay per use".into())
                );
                let idx = filter_matches(items, query)[*sel];
                assert_eq!(items[idx], ("main".into(), "Main One  ✓".into()));
                assert_eq!(items[1].1, "Fast One");
            }
            _ => panic!("expected Choose"),
        }
    }

    #[test]
    fn a_fallback_note_shows_once_and_clears() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let rt =
            Arc::new(Runtime::new(store, Arc::new(UnusedEngine)).with_config(two_model_config));
        let session = rt.create_session("s".into(), PathBuf::from(".")).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(rt, PathBuf::from("."), session, tx);

        // Nothing asked for: nothing to say.
        app.note_model_fallback();
        assert!(app.model_note.is_none() && app.status.is_empty());
        assert!(
            app.status_lines().iter().any(|l| l
                == "Main model: Main One (Anthropic API · pay per use) · picked automatically")
        );

        // A thread asking for a model no account lists: one note.
        app.session.system2 = Some("chatgpt:gpt-6.2-sol".into());
        app.refreshing_models = true;
        app.note_model_fallback();
        assert_eq!(
            app.status,
            "Main: Main One (Anthropic API) — gpt-6.2-sol not listed yet"
        );
        let status = app.status_lines().join("\n");
        assert!(status.contains("Main model: Main One (Anthropic API · pay per use) · configured chatgpt:gpt-6.2-sol, not available until the model list refreshes"), "{status}");
        app.status.clear();
        app.note_model_fallback();
        assert!(app.status.is_empty(), "said once");

        // Once the model is there (here: picked), the note goes away.
        app.status = app.model_note.clone().unwrap();
        app.session.system2 = Some("main".into());
        app.on_models_refreshed(crate::catalog::RefreshOutcome {
            asked: 1,
            listed: 1,
        });
        assert!(app.model_note.is_none());
        assert!(app.status.is_empty());
        assert!(!app.refreshing_models);

        // Nothing could be listed: a one-line note.
        app.on_models_refreshed(crate::catalog::RefreshOutcome {
            asked: 2,
            listed: 0,
        });
        assert!(
            app.status.starts_with("Couldn't list the models"),
            "{}",
            app.status
        );
    }

    #[test]
    fn a_failed_refresh_keeps_its_cause_in_the_fallback_note() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let rt =
            Arc::new(Runtime::new(store, Arc::new(UnusedEngine)).with_config(two_model_config));
        let session = rt.create_session("s".into(), PathBuf::from(".")).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(rt, PathBuf::from("."), session, tx);
        app.session.system2 = Some("chatgpt:gpt-6.2-sol".into());
        app.refreshing_models = true;
        app.note_model_fallback();

        // Offline, or a key was rejected: nothing listed.
        app.on_models_refreshed(crate::catalog::RefreshOutcome {
            asked: 1,
            listed: 0,
        });
        assert_eq!(
            app.status,
            "Main: Main One (Anthropic API) — gpt-6.2-sol: list refresh failed · /status"
        );
        assert!(!app.status.contains("not offered"));
        let status = app.status_lines().join("\n");
        assert!(
            status.contains(
                "the model list couldn't refresh (`pathfinder models --refresh` shows why)"
            ),
            "{status}"
        );
        // Fits an 80-column footer.
        assert!(app.status.chars().count() <= 79, "{}", app.status);

        // A refresh that listed something but not this model: not offered.
        app.on_models_refreshed(crate::catalog::RefreshOutcome {
            asked: 1,
            listed: 1,
        });
        assert_eq!(
            app.status,
            "Main: Main One (Anthropic API) — gpt-6.2-sol not offered · /model"
        );

        // Both tiers fall back: one short note.
        app.session.system1 = Some("chatgpt:gpt-6-luna".into());
        app.note_model_fallback();
        assert_eq!(
            app.status,
            "Fast: Fast One, Main: Main One — chosen models not offered · /model"
        );
    }

    #[test]
    fn settings_and_confirmations_name_the_account() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let rt =
            Arc::new(Runtime::new(store, Arc::new(UnusedEngine)).with_config(two_model_config));
        let session = rt.create_session("s".into(), PathBuf::from(".")).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let app = App::new(rt, PathBuf::from("."), session, tx);
        assert_eq!(
            app.current_setting(crate::options::MAIN_MODEL),
            "Main One · Anthropic API"
        );
        assert_eq!(app.current_short(crate::options::MAIN_MODEL), "Main One $");
        assert_eq!(app.current(crate::options::MAIN_MODEL), "Main One");
        assert_eq!(
            app.current_setting(crate::options::CHECKS),
            app.current(crate::options::CHECKS)
        );
        assert_eq!(
            app.set_message("Main model", crate::options::MAIN_MODEL, ""),
            "Main model set: Main One (Anthropic API · pay per use)."
        );
        assert_eq!(
            app.set_message(
                "Review checks",
                crate::options::CHECKS,
                " (saved as default)"
            ),
            "Review checks set (saved as default)."
        );
    }

    #[test]
    fn attachments_startup_note_and_contextual_shortcuts() {
        let (mut app, _rx, store) = test_app();
        app.apply_startup_model_note(true);
        assert!(
            app.entries
                .iter()
                .any(|e| matches!(e, Entry::Note(n) if n.contains("No models are configured")))
        );

        // Quoted /attach with spaces, Backspace removal, and /attach clear.
        let file = app.cwd.join("my doc.pdf");
        std::fs::write(&file, b"%PDF").unwrap();
        app.attach("\"my doc.pdf\"");
        assert_eq!(app.attachments, vec![file.clone()]);

        // Backspace on empty input pops last attachment.
        app.on_key(key(KeyCode::Backspace));
        assert!(app.attachments.is_empty());

        // /attach clear + new_thread clears attachments and history state.
        app.attach("\"my doc.pdf\"");
        app.last_sent = "prev".into();
        app.new_thread();
        assert!(app.attachments.is_empty());
        assert!(app.last_sent.is_empty());

        // Threads load clears attachments.
        let mut s2 = app.rt.create_session("t2".into(), app.cwd.clone()).unwrap();
        s2.push_turn("user", "hello");
        store.save_session(&s2).unwrap();
        app.attach("\"my doc.pdf\"");
        app.modal = Some(Modal::Threads {
            items: vec![("t2".into(), "hello".into())],
            meta: vec!["chat · 1 · now".into()],
            query: String::new(),
            sel: 0,
        });
        app.on_key(key(KeyCode::Enter));
        assert!(app.attachments.is_empty());
        assert_eq!(app.session.id, "t2");

        // Contextual shortcuts: candidate & InReview pre-fill, pending_change accepts.
        app.session.pending_candidate = Some(PendingCandidate {
            part_id: "p1".into(),
            candidate: "c".into(),
            key_correct: true,
            key_reason: String::new(),
            secret: None,
            targets: vec![],
            misconception: None,
            id: "cand1".into(),
        });
        app.on_key(alt(KeyCode::Char('y')));
        assert_eq!(app.input, "Right, because ");
        app.session.pending_candidate = None;

        let mut p1 = Part::new("p1".into(), "Part 1".into(), String::new(), vec![]);
        p1.status = PartStatus::InReview;
        app.session.map.parts.push(p1);
        app.session.active = Some("p1".into());
        app.on_key(alt(KeyCode::Char('o')));
        assert_eq!(app.input, "/object ");
        app.session.pending_change = Some(MapChange {
            brief: "add p2".into(),
            goal: None,
            parts: vec![ProposedPart {
                id: "p2".into(),
                title: "P2".into(),
                note: String::new(),
                depends_on: vec![],
                concepts: vec![],
            }],
            remove: vec![],
            concepts: vec![],
        });
        assert!(app.has_part_in_review());
    }

    #[test]
    fn a_resumed_thread_shows_its_tool_calls_and_thoughts_before_each_reply() {
        use crate::domain::Step;
        let (mut app, _rx, _store) = test_app();
        let s = &mut app.session;
        s.push_turn("user", "What does the gate check?");
        s.push_turn("assistant", "The draft.");
        s.history[1].steps = vec![
            Step::Thought {
                text: "Look first.".into(),
            },
            Step::Tool {
                id: "t1".into(),
                title: "Read gate.rs".into(),
                ok: Some(true),
                detail: "a".into(),
            },
            // Stopped before it ended.
            Step::Tool {
                id: "t2".into(),
                title: "Grep verdict".into(),
                ok: None,
                detail: String::new(),
            },
        ];
        // An older reply, stored without steps.
        s.push_turn("user", "and then?");
        s.push_turn("assistant", "It waits.");
        let entries = entries_of(&app.session);
        assert!(matches!(&entries[0], Entry::User(_)));
        assert!(matches!(&entries[1], Entry::Thought(t) if t == "Look first."));
        assert!(
            matches!(&entries[2], Entry::Tool { title, ok: Some(true), .. } if title == "Read gate.rs")
        );
        assert!(matches!(
            &entries[3],
            Entry::Tool {
                ok: Some(false),
                ..
            }
        ));
        assert!(matches!(&entries[4], Entry::Text(t) if t == "The draft."));
        assert!(matches!(&entries[5], Entry::User(_)));
        assert!(matches!(&entries[6], Entry::Text(t) if t == "It waits."));
        assert_eq!(entries.len(), 7);
    }

    #[test]
    fn threads_are_named_by_goal_message_or_sources_with_mode_turns_and_age() {
        let mut s = Session::new("a".into(), PathBuf::from("."));
        assert_eq!(thread_name(&s), None);
        assert_eq!(thread_title(&s), "(empty thread)");
        s.push_turn("user", "/find  wrap_hanging");
        assert_eq!(thread_name(&s).as_deref(), Some("/find wrap_hanging"));
        s.push_turn("user", "/open app/src/x.rs:20-24");
        s.push_turn("user", "/attach \"my notes.pdf\"");
        s.push_turn("user", "/open app/src/x.rs");
        s.push_turn("user", "/open book.pdf 3");
        s.push_turn("user", "/attach clear");
        assert_eq!(
            thread_name(&s).as_deref(),
            Some("Sources: x.rs, my notes.pdf, book.pdf")
        );
        s.push_turn("user", "What does\n the gate check?");
        assert_eq!(
            thread_name(&s).as_deref(),
            Some("What does the gate check?")
        );
        s.goal = Some("How the gate decides".into());
        assert_eq!(thread_title(&s), "How the gate decides");
        assert_eq!(thread_meta(&s, 7200), "chat · 7 · 2h");
        assert_eq!(
            [0, 59, 60, 3599, 86_399, 86_400, 5_184_000].map(age),
            ["now", "now", "1m", "59m", "23h", "1d", "2mo"].map(String::from)
        );
    }

    #[test]
    fn the_thread_list_marks_the_open_one_and_opening_names_the_thread() {
        let (mut app, _rx, store) = test_app();
        app.on_key(ctrl(KeyCode::Char('o')));
        assert!(app.modal.is_none());
        assert_eq!(app.status, "No earlier threads in this folder.");

        app.session.push_turn("user", "this one");
        store.save_session(&app.session).unwrap();
        app.on_key(ctrl(KeyCode::Char('o')));
        assert!(app.modal.is_none(), "only the open thread: nothing to list");

        let mut other = app
            .rt
            .create_session("other".into(), app.cwd.clone())
            .unwrap();
        other.goal = Some("Understand the gate".into());
        other.push_turn("user", "hi");
        store.save_session(&other).unwrap();
        app.on_key(ctrl(KeyCode::Char('o')));
        let (items, meta) = match &app.modal {
            Some(Modal::Threads { items, meta, .. }) => (items.clone(), meta.clone()),
            _ => panic!("expected the thread list"),
        };
        assert_eq!(items.len(), 2);
        let mine = items
            .iter()
            .position(|(id, _)| *id == app.session.id)
            .unwrap();
        assert!(meta[mine].starts_with("(open) chat · 1 · "), "{meta:?}");
        let theirs = 1 - mine;
        assert_eq!(items[theirs].1, "Understand the gate");
        assert!(meta[theirs].starts_with("chat · 1 · "), "{meta:?}");

        // Enter on the open one keeps it; on the other opens it by name.
        for _ in 0..mine {
            app.on_key(key(KeyCode::Down));
        }
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.status, "That thread is already open.");
        app.on_key(ctrl(KeyCode::Char('o')));
        app.on_key(key(KeyCode::Home));
        for _ in 0..theirs {
            app.on_key(key(KeyCode::Down));
        }
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.session.id, "other");
        assert_eq!(app.status, "Opened \"Understand the gate\"");
    }

    #[test]
    fn help_filters_as_you_type_and_only_esc_f1_or_q_close_it() {
        let (mut app, _rx, _store) = test_app();
        app.on_key(key(KeyCode::F(1)));
        assert!(matches!(app.modal, Some(Modal::Help { .. })));
        // q with nothing typed closes; F1 and Esc always do.
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.modal.is_none());
        app.on_key(key(KeyCode::F(1)));
        app.on_key(key(KeyCode::F(1)));
        assert!(app.modal.is_none());
        app.on_key(key(KeyCode::F(1)));
        for c in "jkgq".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Down));
        match &app.modal {
            Some(Modal::Help { query, .. }) => assert_eq!(query, "jkgq"),
            _ => panic!("typing must not close help"),
        }
        app.on_key(key(KeyCode::Backspace));
        app.on_key(ctrl(KeyCode::Char('u')));
        match &app.modal {
            Some(Modal::Help { query, scroll }) => {
                assert!(query.is_empty());
                assert_eq!(*scroll, 0);
            }
            _ => panic!("expected help"),
        }
        app.on_key(key(KeyCode::Esc));
        assert!(app.modal.is_none());
    }

    #[test]
    fn the_model_chooser_keeps_its_cursor_on_the_current_model_while_filtering() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let rt =
            Arc::new(Runtime::new(store, Arc::new(UnusedEngine)).with_config(two_model_config));
        let session = rt.create_session("s".into(), PathBuf::from(".")).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(rt, PathBuf::from("."), session, tx);
        let main = SETTINGS
            .iter()
            .position(|(o, _)| *o == crate::options::MAIN_MODEL)
            .unwrap();
        app.modal = Some(Modal::Settings { sel: main });
        app.on_key(key(KeyCode::Enter));
        let on = |app: &App| match &app.modal {
            Some(Modal::Choose {
                items, query, sel, ..
            }) => items[filter_matches(items, query)[*sel]].0.clone(),
            _ => panic!("expected the chooser"),
        };
        assert_eq!(on(&app), "main");
        // Both match "one": the cursor stays on the current model.
        for c in "one".chars() {
            app.on_key(key(KeyCode::Char(c)));
            assert_eq!(on(&app), "main", "after {c:?}");
        }
        // It doesn't match "fast": the best match.
        app.on_key(ctrl(KeyCode::Char('u')));
        assert_eq!(on(&app), "main");
        app.on_key(key(KeyCode::Char('f')));
        assert_eq!(on(&app), "fast");
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(on(&app), "main");
        // Model rows keep the chooser even with two models.
        assert!(!app.cycles(crate::options::MAIN_MODEL));
        assert!(app.cycles(crate::options::CHECKS));
        assert!(!app.cycles(GATE_KEY));
    }

    #[test]
    fn ctrl_u_and_ctrl_d_page_by_half_with_nothing_typed() {
        let (mut app, _rx, _store) = test_app();
        app.conv_area = Rect::new(0, 0, 80, 20);
        app.on_key(ctrl(KeyCode::Char('u')));
        app.on_key(ctrl(KeyCode::Char('u')));
        assert_eq!(app.scroll, 20);
        app.on_key(ctrl(KeyCode::Char('d')));
        assert_eq!(app.scroll, 10);
        assert!(!app.quit);
        app.on_key(ctrl(KeyCode::Char('d')));
        assert_eq!(app.scroll, 0);
        assert!(!app.quit, "the press that reaches the bottom doesn't quit");
        app.on_key(ctrl(KeyCode::Char('d')));
        assert!(!app.quit, "paging down past the bottom never quits");
        assert_eq!(app.status, "Press ^D again to quit");
        // Another key in between disarms it.
        app.on_key(key(KeyCode::Right));
        app.on_key(ctrl(KeyCode::Char('d')));
        assert!(!app.quit);
        app.on_key(ctrl(KeyCode::Char('d')));
        assert!(app.quit, "a second ^D in a row quits");

        // With text: Ctrl+U clears the line, Ctrl+D deletes forward.
        app.quit = false;
        app.scroll = 4;
        app.input = "abc".into();
        app.cursor = 1;
        app.on_key(ctrl(KeyCode::Char('d')));
        assert_eq!(app.input, "ac");
        app.cursor = 2;
        app.on_key(ctrl(KeyCode::Char('u')));
        assert_eq!(app.input, "");
        assert_eq!(app.scroll, 4);
    }

    #[test]
    fn selecting_text_hides_the_plan_panel_and_ctrl_b_says_why() {
        let (mut app, _rx, _store) = test_app();
        app.session.goal = Some("g".into());
        assert!(view::docks(&app, 140));
        app.on_key(key(KeyCode::F(2)));
        assert!(!view::docks(&app, 140));
        app.on_key(ctrl(KeyCode::Char('b')));
        assert!(app.status.contains("F2"), "{}", app.status);
        assert!(!app.side_overlay);
        app.on_key(key(KeyCode::F(2)));
        assert!(view::docks(&app, 140));
    }

    #[test]
    fn smooth_scroll_animation_converges_to_target() {
        let (mut app, _rx, _store) = test_app();
        app.smooth_scroll = true;
        app.conv_area = Rect::new(0, 0, 80, 20);
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.scroll, 18);
        assert!(app.is_scroll_animating());
        let mut steps = 0;
        while app.is_scroll_animating() && steps < 50 {
            app.step_scroll_anim();
            steps += 1;
        }
        assert!(!app.is_scroll_animating());
        assert!((app.visual_scroll - 18.0).abs() < 0.01);
        assert!((2..30).contains(&steps), "steps={steps}");
    }

    #[test]
    fn mouse_drag_selection_strips_gutters_rejoins_wraps_and_handles_esc_ctrl_c() {
        let (mut app, _rx, _store) = test_app();
        app.entries.push(Entry::Text(
            "The quick brown fox jumps over the lazy dog in the yard.".into(),
        ));
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 12)).unwrap();
        term.draw(|f| view::draw(f, &mut app)).unwrap();
        let start_row = app.conv_area.y;
        let x0 = app.conv_area.x + 1;
        // Drag from col 0 of line 0 across the soft-wrapped line 1.
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x0,
            row: start_row,
            modifiers: KeyModifiers::NONE,
        });
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: x0 + 20,
            row: start_row + 1,
            modifiers: KeyModifiers::NONE,
        });
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: x0 + 20,
            row: start_row + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.has_text_selection());
        let copied = app.selected_text();
        assert!(
            !copied.contains('•') && !copied.contains('\n'),
            "expected prefix stripped & soft-wrap rejoined: {copied:?}"
        );
        assert!(copied.starts_with("The quick brown fox"), "{copied:?}");

        // Esc clears selection without quitting.
        app.on_key(key(KeyCode::Esc));
        assert!(!app.has_text_selection());
        assert!(!app.quit);
    }

    #[test]
    fn mouse_drag_selection_on_source_strips_line_number_gutter() {
        let (mut app, _rx, _store) = test_app();
        app.entries.push(Entry::Source {
            title: "src/main.rs".into(),
            range: "lines 1–2 of 2".into(),
            body: "```rust numbered\n   1  fn hello() {\n   2      println!(\"hi\");\n```".into(),
        });
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 14)).unwrap();
        term.draw(|f| view::draw(f, &mut app)).unwrap();
        // Line 0 is header `▤ src/main.rs ...`, line 1 is `     1  fn hello() {`, line 2 is `     2      println!("hi");`.
        let x0 = app.conv_area.x + 1;
        let y1 = app.conv_area.y + 1;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x0,
            row: y1,
            modifiers: KeyModifiers::NONE,
        });
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: x0 + 30,
            row: y1 + 1,
            modifiers: KeyModifiers::NONE,
        });
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: x0 + 30,
            row: y1 + 1,
            modifiers: KeyModifiers::NONE,
        });
        let copied = app.selected_text();
        assert_eq!(copied, "fn hello() {\n    println!(\"hi\");");
    }
}
