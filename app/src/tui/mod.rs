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
use crate::stream::ToolContent;

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
}

/// A dialog over the conversation.
pub enum Modal {
    Threads {
        items: Vec<(String, String)>,
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
    /// Keys and commands; `scroll` lines down.
    Help {
        scroll: usize,
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
}

/// Indices into `items` matching `query` (case-insensitive substring on id or label).
pub fn filter_matches(items: &[(String, String)], query: &str) -> Vec<usize> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return (0..items.len()).collect();
    }
    items
        .iter()
        .enumerate()
        .filter(|(_, (id, label))| {
            label.to_ascii_lowercase().contains(&q) || id.to_ascii_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
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

/// The thread settings Ctrl+P offers: (option id, name).
pub const SETTINGS: [(&str, &str); 7] = [
    (crate::options::FAST_MODEL, "Fast model"),
    (crate::options::FAST_EFFORT, "Fast model effort"),
    (crate::options::MAIN_MODEL, "Main model"),
    (crate::options::MAIN_EFFORT, "Main model effort"),
    (crate::options::DECISIONS, "Decisions"),
    (crate::options::DECISION_MODEL, "Decision service"),
    (crate::options::CHECKS, "Review checks"),
];

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
    /// Kinds of command the user allowed for the rest of this thread.
    pub always: std::collections::HashSet<String>,
    /// When this thread was opened here, and its turns since (usage events).
    thread_started: std::time::Instant,
    thread_turns: u32,
    tx: mpsc::UnboundedSender<AppEvent>,
    /// A browser sign-in waiting for its approval.
    sign_in: Option<tokio::task::AbortHandle>,
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
            always: Default::default(),
            thread_started: std::time::Instant::now(),
            thread_turns: 0,
            tx,
            sign_in: None,
            quit: false,
        }
    }

    fn check_models_on_startup(&mut self) {
        let empty = self.rt.model_choices(&self.session).catalog.is_empty();
        self.apply_startup_model_note(empty);
    }

    fn apply_startup_model_note(&mut self, catalog_empty: bool) {
        if catalog_empty {
            self.entries.push(Entry::Note(
                "No models are configured yet. Run `pathfinder auth login` (or `pathfinder auth set <provider>`) in a terminal to connect an account or API key, then open /model (Ctrl+P)."
                    .into(),
            ));
            if self.status.is_empty() {
                self.status = "No models configured — run `pathfinder auth login` first.".into();
            }
        }
    }
}

pub async fn run(rt: Arc<Runtime>, cwd: PathBuf) -> Result<()> {
    let cwd = dunce::canonicalize(&cwd).unwrap_or(cwd);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = rt.create_session(uuid::Uuid::new_v4().to_string(), cwd.clone())?;
    let mut app = App::new(rt, cwd, session, tx);
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
    if app
        .rt
        .sessions_in(&app.cwd)
        .iter()
        .any(|s| !s.history.is_empty())
    {
        app.status = "Ctrl+O opens an earlier thread in this folder.".into();
    }
    app.check_models_on_startup();

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
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(120));
    let mut paste_until: Option<std::time::Instant> = None;
    let result: Result<()> = async {
        loop {
            terminal.draw(|f| view::draw(f, &mut app))?;
            tokio::select! {
                ev = events.next() => match ev {
                    // A Windows console never reports a paste: take the keys
                    // already waiting with this one and see if they're one.
                    Some(Ok(first @ Event::Key(_))) if cfg!(windows) => {
                        let mut batch = vec![first];
                        loop {
                            // Polled with this task's waker, so the stream
                            // still wakes the loop when it runs dry.
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
                        let now = std::time::Instant::now();
                        let continuing = paste_until.is_some_and(|t| now < t);
                        if let Some(text) = paste_burst(&batch, continuing) {
                            app.on_paste(&text);
                            paste_until = Some(now + PASTE_GAP);
                        } else {
                            paste_until = None;
                            for ev in batch {
                                match ev {
                                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                                        app.on_key(key)
                                    }
                                    Event::Paste(text) => app.on_paste(&text),
                                    Event::Mouse(mouse) => app.on_mouse(mouse),
                                    _ => {}
                                }
                            }
                        }
                    }
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => app.on_key(key),
                    Some(Ok(Event::Paste(text))) => app.on_paste(&text),
                    Some(Ok(Event::Mouse(mouse))) => app.on_mouse(mouse),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                },
                Some(ev) = rx.recv() => app.on_event(ev),
                _ = ticker.tick(), if app.running.is_some() => app.tick += 1,
            }
            if app.quit {
                break;
            }
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
fn copy_to_clipboard(text: &str) -> std::result::Result<(), String> {
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

/// The conversation a stored thread shows.
fn entries_of(session: &Session) -> Vec<Entry> {
    session
        .history
        .iter()
        .map(|t| match t.role.as_str() {
            "user" => Entry::User(t.content.clone()),
            _ => Entry::Text(t.content.clone()),
        })
        .collect()
}

/// A thread's name in the list: its goal, else its first message.
pub fn thread_title(s: &Session) -> String {
    let raw = s.goal.clone().unwrap_or_else(|| {
        s.history
            .iter()
            .find(|t| t.role == "user")
            .map(|t| t.content.clone())
            .unwrap_or_else(|| "(empty thread)".into())
    });
    let one_line = raw.replace('\n', " ");
    let mut title: String = one_line.chars().take(70).collect();
    if one_line.chars().count() > 70 {
        title.push('…');
    }
    title
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

    fn is_attach_arg(&self) -> bool {
        commands::path_arg_slice(&self.input).is_some_and(|(cmd, _)| cmd == "/attach")
    }

    fn is_path_arg(&self) -> bool {
        commands::path_arg_slice(&self.input).is_some()
    }

    fn palette_open(&self) -> bool {
        self.input.starts_with('/')
            && !self.input.contains(char::is_whitespace)
            && !commands::search_for(&self.input, Some(&self.session)).is_empty()
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
            "Terminal text selection on (F2 restores mouse scroll).".into()
        };
    }

    fn on_paste(&mut self, text: &str) {
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
                Modal::Threads { query, sel, .. } | Modal::Choose { query, sel, .. } => {
                    for c in clean.chars() {
                        if !c.is_control() {
                            query.push(c);
                        }
                    }
                    *sel = 0;
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
                        Modal::Help { scroll } => {
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
                        Modal::Threads { items, query, sel } => {
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
            MouseEventKind::Down(MouseButton::Left)
                if self.modal.is_none()
                    && self.selected.is_some()
                    && !rect_contains(self.conv_area, mouse.column, mouse.row) =>
            {
                self.selected = None;
            }
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Modifier(_)) {
            return;
        }
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
                self.selected = self.items.last().copied();
                self.reveal_selected = true;
            }
            KeyCode::Tab if self.is_path_arg() => {
                let matches = commands::path_matches(&self.input, &self.cwd);
                if !matches.is_empty() {
                    let completed = if self.cmd_sel == 0 && matches.len() > 1 {
                        commands::complete_path(&self.input, &self.cwd)
                            .or_else(|| commands::complete_path_at(&self.input, 0, &self.cwd))
                    } else {
                        commands::complete_path_at(&self.input, self.cmd_sel, &self.cwd)
                    };
                    if let Some(done) = completed {
                        self.input = done;
                        self.cursor = self.input.len();
                        self.cmd_sel = 0;
                        self.history_idx = None;
                        self.history_draft.clear();
                    }
                }
            }
            KeyCode::Tab
                if self.input.starts_with('/') && !self.input.contains(char::is_whitespace) =>
            {
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
                self.selected = self.items.last().copied();
                self.reveal_selected = true;
            }
            KeyCode::Tab => {}
            KeyCode::Char('c') if ctrl => match &self.running {
                Some(cancel) => cancel.cancel(),
                None => self.quit = true,
            },
            KeyCode::Char('d') if ctrl => {
                if self.input.is_empty() {
                    self.quit = true;
                } else {
                    self.delete_char_forward();
                }
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
            KeyCode::Char('t') if ctrl => self.show_thoughts = !self.show_thoughts,
            KeyCode::Char('g') if ctrl => {
                used(Feature::ToolGrouping);
                self.show_tools = !self.show_tools;
            }
            KeyCode::Char('b') if ctrl => {
                used(Feature::SidePanel);
                let wide = crossterm::terminal::size().is_ok_and(|(w, _)| w >= view::DOCK_FROM);
                if wide {
                    self.show_side = !self.show_side;
                } else {
                    self.side_overlay = !self.side_overlay;
                }
                self.side_scroll = 0;
            }
            KeyCode::Char('j') if ctrl => self.insert_char('\n'),
            KeyCode::F(1) => {
                used(Feature::Help);
                self.modal = Some(Modal::Help { scroll: 0 });
            }
            KeyCode::Esc if self.side_overlay => self.side_overlay = false,
            KeyCode::Esc => {
                if let Some(cancel) = &self.running {
                    cancel.cancel();
                }
            }
            KeyCode::Enter if alt || shift => self.insert_char('\n'),
            KeyCode::Enter => {
                if self.is_attach_arg() {
                    let arg = self
                        .input
                        .split_once(char::is_whitespace)
                        .map(|(_, a)| a.trim())
                        .unwrap_or("");
                    let ready = arg.eq_ignore_ascii_case("clear")
                        || commands::resolve_attach_path(arg, &self.cwd)
                            .is_some_and(|p| p.is_file());
                    if ready {
                        self.submit();
                    } else if let Some(cand) = commands::attach_matches(&self.input, &self.cwd)
                        .get(self.cmd_sel)
                        .cloned()
                    {
                        self.input = format!("/attach {}", cand.arg);
                        self.cursor = self.input.len();
                        self.cmd_sel = 0;
                        if !cand.is_dir {
                            self.submit();
                        }
                    } else {
                        self.submit();
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
                        } else {
                            self.input = cmd.name.to_string();
                            self.cursor = self.input.len();
                            self.cmd_sel = 0;
                            self.submit();
                        }
                    } else {
                        self.submit();
                    }
                } else {
                    self.submit();
                }
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(10);
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
                if self.is_attach_arg() {
                    let len = commands::attach_matches(&self.input, &self.cwd).len();
                    if len > 0 {
                        self.cmd_sel = commands::step_selection(self.cmd_sel, len, -1);
                        return;
                    }
                } else if self.palette_open() {
                    let len = commands::search_for(&self.input, Some(&self.session)).len();
                    if len > 0 {
                        self.cmd_sel = commands::step_selection(self.cmd_sel, len, -1);
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
                if self.is_attach_arg() {
                    let len = commands::attach_matches(&self.input, &self.cwd).len();
                    if len > 0 {
                        self.cmd_sel = commands::step_selection(self.cmd_sel, len, 1);
                        return;
                    }
                } else if self.palette_open() {
                    let len = commands::search_for(&self.input, Some(&self.session)).len();
                    if len > 0 {
                        self.cmd_sel = commands::step_selection(self.cmd_sel, len, 1);
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
        let one = |e: &Entry| match e {
            Entry::User(t) | Entry::Text(t) | Entry::Thought(t) | Entry::Note(t) => t.clone(),
            Entry::Tool { title, detail, .. } => {
                if detail.is_empty() {
                    title.clone()
                } else {
                    format!("{title}\n{detail}")
                }
            }
        };
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
        self.submit();
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
                self.status = "New thread.".into();
            }
            Err(e) => self.status = format!("Couldn't start a thread: {e}"),
        }
    }

    fn open_threads(&mut self) {
        let items: Vec<(String, String)> = self
            .rt
            .sessions_in(&self.cwd)
            .iter()
            .filter(|s| !s.history.is_empty())
            .map(|s| (s.id.clone(), thread_title(s)))
            .collect();
        if items.is_empty() {
            self.status = "No earlier threads in this folder.".into();
            return;
        }
        self.modal = Some(Modal::Threads {
            items,
            query: String::new(),
            sel: 0,
        });
    }

    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
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
                commands::Parsed::Unknown(word) => {
                    self.status = format!("Unknown command {word}. /help lists the commands.");
                    return;
                }
                commands::Parsed::Task => {}
            }
        }
        if self.running.is_some() {
            self.status = "Wait for the reply, or Esc to stop it.".into();
            return;
        }
        let mut files = std::mem::take(&mut self.attachments);
        for f in dropped_files(&text, &self.cwd) {
            if !files.contains(&f) {
                files.push(f);
            }
        }
        let shown = if files.is_empty() {
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
                    *detail = match content {
                        ToolContent::Diff { path, old, new } => {
                            let before = old.as_deref().map_or(0, |o| o.lines().count());
                            format!(
                                "{}: {} lines, was {before}",
                                path.display(),
                                new.lines().count()
                            )
                        }
                        ToolContent::Text(t) => {
                            let tail: Vec<&str> = t.lines().rev().take(6).collect();
                            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
                        }
                    };
                }
            }
            AppEvent::Stream(StreamEvent::ToolEnd { id, ok: done }) => {
                if let Some(Entry::Tool { ok, .. }) = self.tool_mut(&id) {
                    *ok = Some(done);
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
                self.modal = Some(Modal::Permission {
                    title,
                    always_key,
                    sel: None,
                    reply: Some(reply),
                });
            }
            AppEvent::Status(line) => self.status = line,
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
                self.thread_turns += 1;
                match result {
                    Ok(out) if !out.reply.is_empty() => match self.entries.last_mut() {
                        Some(Entry::Text(s)) => s.push_str(&out.reply),
                        _ => self.entries.push(Entry::Text(out.reply)),
                    },
                    Ok(_) => {}
                    Err(_) if stopped => self.entries.push(Entry::Note("Stopped.".into())),
                    Err(e) => self
                        .entries
                        .push(Entry::Note(format!("Something went wrong: {e}"))),
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

    /// The values a setting can take: (value, label).
    fn choices(&self, option: &str) -> Vec<(String, String)> {
        let choices = self.rt.model_choices(&self.session);
        match option {
            crate::options::FAST_MODEL | crate::options::MAIN_MODEL => choices
                .catalog
                .iter()
                .map(|m| (m.id.clone(), format!("{}  ({})", m.label, m.route.label())))
                .collect(),
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
                .map(|c| (c.to_string(), c.to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A setting's current value, for the settings list.
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
                Modal::Help { .. }
                | Modal::Threads { .. }
                | Modal::Settings { .. }
                | Modal::Choose { .. } => {}
            }
            return;
        }
        self.modal = match modal {
            Modal::Help { scroll } => {
                let max = self.help_max_scroll;
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => Some(Modal::Help {
                        scroll: (scroll + 1).min(max),
                    }),
                    KeyCode::Up | KeyCode::Char('k') => Some(Modal::Help {
                        scroll: scroll.saturating_sub(1),
                    }),
                    KeyCode::PageDown | KeyCode::Char(' ') => Some(Modal::Help {
                        scroll: (scroll + 10).min(max),
                    }),
                    KeyCode::PageUp => Some(Modal::Help {
                        scroll: scroll.saturating_sub(10),
                    }),
                    KeyCode::Home | KeyCode::Char('g') => Some(Modal::Help { scroll: 0 }),
                    KeyCode::End | KeyCode::Char('G') => Some(Modal::Help { scroll: max }),
                    _ => None,
                }
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
                mut query,
                mut sel,
            } => {
                let filtered = filter_matches(&items, &query);
                match key.code {
                    KeyCode::Esc => None,
                    KeyCode::Enter => {
                        if let Some(&orig_idx) = filtered.get(sel)
                            && let Some((id, _)) = items.get(orig_idx)
                            && let Ok(Some(s)) = self.rt.load_session(id)
                        {
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
                            self.session = s;
                            self.scroll = 0;
                            self.side_scroll = 0;
                            self.new_below = false;
                            self.status = "Thread opened.".into();
                        }
                        None
                    }
                    KeyCode::Up => {
                        sel = sel.saturating_sub(1);
                        Some(Modal::Threads { items, query, sel })
                    }
                    KeyCode::Down => {
                        sel = (sel + 1).min(filtered.len().saturating_sub(1));
                        Some(Modal::Threads { items, query, sel })
                    }
                    KeyCode::PageUp => {
                        sel = sel.saturating_sub(10);
                        Some(Modal::Threads { items, query, sel })
                    }
                    KeyCode::PageDown => {
                        sel = (sel + 10).min(filtered.len().saturating_sub(1));
                        Some(Modal::Threads { items, query, sel })
                    }
                    KeyCode::Home => Some(Modal::Threads {
                        items,
                        query,
                        sel: 0,
                    }),
                    KeyCode::End => Some(Modal::Threads {
                        items,
                        query,
                        sel: filtered.len().saturating_sub(1),
                    }),
                    KeyCode::Char('u') if ctrl => {
                        query.clear();
                        Some(Modal::Threads {
                            items,
                            query,
                            sel: 0,
                        })
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        Some(Modal::Threads {
                            items,
                            query,
                            sel: 0,
                        })
                    }
                    KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => {
                        query.push(c);
                        Some(Modal::Threads {
                            items,
                            query,
                            sel: 0,
                        })
                    }
                    _ => Some(Modal::Threads { items, query, sel }),
                }
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
                KeyCode::Enter => {
                    let (option, name) = SETTINGS[sel];
                    let items = self.choices(option);
                    if items.is_empty() {
                        self.status = format!("No choices for {name}.");
                        Some(Modal::Settings { sel })
                    } else {
                        let current = self.current(option);
                        let at = items
                            .iter()
                            .position(|(v, l)| *v == current || l.starts_with(&current))
                            .unwrap_or(0);
                        Some(Modal::Choose {
                            option,
                            title: name.to_string(),
                            items,
                            query: String::new(),
                            sel: at,
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
                                    self.status = format!("{title} set.");
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
                                            self.status =
                                                format!("{title} set (saved as default).");
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
                    KeyCode::Up => {
                        sel = sel.saturating_sub(1);
                        Some(Modal::Choose {
                            option,
                            title,
                            items,
                            query,
                            sel,
                        })
                    }
                    KeyCode::Down => {
                        sel = (sel + 1).min(filtered.len().saturating_sub(1));
                        Some(Modal::Choose {
                            option,
                            title,
                            items,
                            query,
                            sel,
                        })
                    }
                    KeyCode::PageUp => {
                        sel = sel.saturating_sub(10);
                        Some(Modal::Choose {
                            option,
                            title,
                            items,
                            query,
                            sel,
                        })
                    }
                    KeyCode::PageDown => {
                        sel = (sel + 10).min(filtered.len().saturating_sub(1));
                        Some(Modal::Choose {
                            option,
                            title,
                            items,
                            query,
                            sel,
                        })
                    }
                    KeyCode::Home => Some(Modal::Choose {
                        option,
                        title,
                        items,
                        query,
                        sel: 0,
                    }),
                    KeyCode::End => Some(Modal::Choose {
                        option,
                        title,
                        items,
                        query,
                        sel: filtered.len().saturating_sub(1),
                    }),
                    KeyCode::Char('u') if ctrl => {
                        query.clear();
                        Some(Modal::Choose {
                            option,
                            title,
                            items,
                            query,
                            sel: 0,
                        })
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        Some(Modal::Choose {
                            option,
                            title,
                            items,
                            query,
                            sel: 0,
                        })
                    }
                    KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => {
                        query.push(c);
                        Some(Modal::Choose {
                            option,
                            title,
                            items,
                            query,
                            sel: 0,
                        })
                    }
                    _ => Some(Modal::Choose {
                        option,
                        title,
                        items,
                        query,
                        sel,
                    }),
                }
            }
        };
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
                self.modal = Some(Modal::Help { scroll: 0 });
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
                for line in self.status_lines() {
                    self.entries.push(Entry::Note(line));
                }
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

    /// `/status`: what this Pathfinder is running with.
    fn status_lines(&self) -> Vec<String> {
        use crate::options;
        let c = crate::telemetry::Consent::load();
        let yes = |b: bool| if b { "yes" } else { "no" };
        let gate = if self.current(options::DECISIONS) != "on" {
            "off for this thread".to_string()
        } else if c.enrolled {
            "on (your own decision key if you have one, else the shared gate)".to_string()
        } else {
            "on if you have a decision key; join the beta (/join) for the shared gate".to_string()
        };
        vec![
            format!(
                "Pathfinder {} on {} {}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
            format!(
                "Models: fast {}, main {} · checks {}",
                self.current(options::FAST_MODEL),
                self.current(options::MAIN_MODEL),
                self.current(options::CHECKS)
            ),
            format!("Gate: {gate}"),
            format!(
                "Joined: {} · usage events: {} · thread sharing: {}",
                yes(c.enrolled),
                yes(c.events),
                yes(c.threads)
            ),
            format!(
                "Settings: {} · log: {}",
                crate::paths::config_dir().display(),
                crate::bugreport::tui_log().display()
            ),
        ]
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
    use crate::domain::{MapChange, Part, PartStatus, PendingCandidate, ProposedPart, TurnRecord};
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
        app.modal = Some(Modal::Help { scroll: 12 });
        app.on_key(key(KeyCode::PageDown));
        match app.modal {
            Some(Modal::Help { scroll }) => assert_eq!(scroll, 15),
            _ => panic!("expected Help"),
        }

        // Settings sel preserved on Esc from Choose, plus type-to-filter.
        app.modal = Some(Modal::Settings { sel: 3 });
        app.on_key(key(KeyCode::Enter));
        match &app.modal {
            Some(Modal::Choose { title, .. }) => assert_eq!(title, "Main model effort"),
            _ => panic!("expected Choose"),
        }
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
        s2.history.push(TurnRecord {
            role: "user".into(),
            content: "hello".into(),
        });
        store.save_session(&s2).unwrap();
        app.attach("\"my doc.pdf\"");
        app.modal = Some(Modal::Threads {
            items: vec![("t2".into(), "hello".into())],
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
}
