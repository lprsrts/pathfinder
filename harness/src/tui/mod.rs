//! `harness tui [folder]`: Pathfinder in the terminal. The same runtime as
//! the Zed connection, so threads, the record, checks and exams behave the
//! same; this is only another front door.

mod markdown;
mod view;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::domain::Session;
use crate::engine::{
    BoxFuture, CommandOutput, Host, Permission, RunContext, StreamEvent, StreamSink, with_turn,
};
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
}

/// A dialog over the conversation.
pub enum Modal {
    Threads {
        items: Vec<(String, String)>,
        sel: usize,
    },
    Settings {
        sel: usize,
    },
    Choose {
        option: &'static str,
        title: String,
        items: Vec<(String, String)>,
        sel: usize,
    },
    Permission {
        title: String,
        always_key: String,
        reply: Option<oneshot::Sender<Permission>>,
    },
    Help,
}

/// The settings the toolbar offers in Zed: (option id, name).
pub const SETTINGS: [(&str, &str); 7] = [
    ("system1", "Fast model"),
    ("fast_effort", "Fast model effort"),
    ("system2", "Main model"),
    ("effort", "Main model effort"),
    ("decisions", "Decisions"),
    ("decision_model", "Decision service"),
    ("checks", "Review checks"),
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
    pub show_side: bool,
    pub modal: Option<Modal>,
    pub status: String,
    pub last_sent: String,
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
    tx: mpsc::UnboundedSender<AppEvent>,
    pub quit: bool,
}

/// Runs the TUI in `cwd` until the user quits.
pub async fn run(rt: Arc<Runtime>, cwd: PathBuf) -> Result<()> {
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let session = rt.create_session(uuid::Uuid::new_v4().to_string(), cwd.clone())?;
    let mut app = App {
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
        show_side: true,
        modal: None,
        status: String::new(),
        last_sent: String::new(),
        tick: 0,
        started: None,
        last_total: 0,
        new_below: false,
        side_overlay: false,
        always: Default::default(),
        tx,
        quit: false,
    };
    if app
        .rt
        .sessions_in(&app.cwd)
        .iter()
        .any(|s| !s.history.is_empty())
    {
        app.status = "Ctrl+O opens an earlier thread in this folder.".into();
    }

    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), EnableBracketedPaste)?;
    // Alternate scroll: the mouse wheel arrives as ↑/↓, and text selection
    // stays the terminal's own because the mouse isn't captured.
    print!("\x1b[?1007h");
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(120));
    let result: Result<()> = async {
        loop {
            terminal.draw(|f| view::draw(f, &mut app))?;
            tokio::select! {
                ev = events.next() => match ev {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => app.on_key(key),
                    Some(Ok(Event::Paste(text))) => app.on_paste(&text),
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
    print!("\x1b[?1007l");
    crossterm::execute!(std::io::stdout(), DisableBracketedPaste).ok();
    ratatui::restore();
    print_transcript(&app);
    result
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
    for line in view::transcript(app, width) {
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

    fn run_in_terminal<'a>(
        &'a self,
        _call_id: &'a str,
        _command: &'a str,
        _cwd: &'a Path,
        _timeout: std::time::Duration,
        _cancel: &'a CancellationToken,
    ) -> Option<BoxFuture<'a, Result<CommandOutput>>> {
        None
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

    fn on_paste(&mut self, text: &str) {
        if self.modal.is_some() {
            return;
        }
        self.input.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if self.modal.is_some() {
            self.on_modal_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('c') if ctrl => match &self.running {
                Some(cancel) => cancel.cancel(),
                None => self.quit = true,
            },
            KeyCode::Char('d') if ctrl && self.input.is_empty() => self.quit = true,
            KeyCode::Char('n') if ctrl => self.new_thread(),
            KeyCode::Char('o') if ctrl => self.open_threads(),
            KeyCode::Char('p') if ctrl => self.modal = Some(Modal::Settings { sel: 0 }),
            KeyCode::Char('t') if ctrl => self.show_thoughts = !self.show_thoughts,
            KeyCode::Char('e') if ctrl => self.show_tools = !self.show_tools,
            KeyCode::Char('b') if ctrl => {
                let wide = crossterm::terminal::size().is_ok_and(|(w, _)| w >= view::DOCK_FROM);
                if wide {
                    self.show_side = !self.show_side;
                } else {
                    self.side_overlay = !self.side_overlay;
                }
            }
            KeyCode::Char('j') if ctrl => self.insert_char('\n'),
            KeyCode::F(1) => self.modal = Some(Modal::Help),
            KeyCode::Esc if self.side_overlay => self.side_overlay = false,
            KeyCode::Esc => {
                if let Some(cancel) = &self.running {
                    cancel.cancel();
                }
            }
            KeyCode::Enter if alt || key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.insert_char('\n')
            }
            KeyCode::Enter => self.submit(),
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => self.scroll += 1,
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.scroll = self.scroll.saturating_sub(1)
            }
            KeyCode::Up if alt => {
                self.input = self.last_sent.clone();
                self.cursor = self.input.len();
            }
            // The wheel arrives as ↑/↓; with nothing typed they scroll.
            KeyCode::Up if self.input.is_empty() => self.scroll += 1,
            KeyCode::Down if self.input.is_empty() => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::End if self.input.is_empty() || ctrl => self.scroll = 0,
            KeyCode::Char(c) => self.insert_char(c),
            KeyCode::Backspace => {
                if let Some((i, _)) = self.input[..self.cursor].char_indices().next_back() {
                    self.input.replace_range(i..self.cursor, "");
                    self.cursor = i;
                }
            }
            KeyCode::Delete => {
                if let Some(ch) = self.input[self.cursor..].chars().next() {
                    self.input
                        .replace_range(self.cursor..self.cursor + ch.len_utf8(), "");
                }
            }
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
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            _ => {}
        }
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
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
                self.session = s;
                self.entries.clear();
                self.always.clear();
                self.scroll = 0;
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
        self.modal = Some(Modal::Threads { items, sel: 0 });
    }

    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.running.is_some() {
            self.status = "Wait for the reply, or Esc to stop it.".into();
            return;
        }
        if let Some(path) = text.strip_prefix("/attach ") {
            let p = Path::new(path.trim());
            let p = if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.cwd.join(p)
            };
            if p.is_file() {
                self.status = format!("Attached {}.", p.display());
                self.attachments.push(p);
            } else {
                self.status = format!("No file at {}.", p.display());
            }
            self.input.clear();
            self.cursor = 0;
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
            let result = with_turn(ctx, rt.handle_turn(&id, TurnInput { text, files }, sink))
                .await
                .map_err(|e| format!("{e:#}"));
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
                    reply: Some(reply),
                });
            }
            AppEvent::Done { result, stopped } => {
                self.running = None;
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
            "system1" | "system2" => choices
                .catalog
                .iter()
                .map(|m| (m.id.clone(), format!("{}  ({})", m.label, m.route.label())))
                .collect(),
            "effort" | "fast_effort" => EFFORTS
                .iter()
                .map(|e| (e.to_string(), e.to_string()))
                .collect(),
            "decisions" => vec![("on".into(), "on".into()), ("off".into(), "off".into())],
            "decision_model" => choices
                .decision_available
                .iter()
                .map(|p| (p.id().to_string(), p.label().to_string()))
                .collect(),
            "checks" => CHECKS
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
            "system1" => label(&c.system1),
            "system2" => label(&c.system2),
            "effort" => c.effort.clone(),
            "fast_effort" => c.fast_effort.clone(),
            "decisions" => if c.decisions_on { "on" } else { "off" }.into(),
            "decision_model" => c
                .decision_model
                .map(|p| p.short().to_string())
                .unwrap_or_else(|| "none".into()),
            "checks" => c.checks.clone(),
            _ => String::new(),
        }
    }

    fn on_modal_key(&mut self, key: KeyEvent) {
        let Some(modal) = self.modal.take() else {
            return;
        };
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
        self.modal = match modal {
            Modal::Help => None,
            Modal::Permission {
                title,
                always_key,
                reply,
            } => {
                let answer = match key.code {
                    KeyCode::Char('y' | '1') | KeyCode::Enter => Some(Permission::Once),
                    KeyCode::Char('a' | '2') => Some(Permission::Always),
                    KeyCode::Char('n' | '3') | KeyCode::Esc => Some(Permission::Denied),
                    _ => None,
                };
                match (answer, reply) {
                    (Some(a), Some(tx)) => {
                        // The tool line above already names the command.
                        let said = match a {
                            Permission::Denied => "✗ You declined this.".to_string(),
                            Permission::Always => {
                                self.always.insert(always_key.clone());
                                format!(
                                    "✓ Allowed, and from now on in this thread for `{}`.",
                                    always_key.rsplit(':').next().unwrap_or(&always_key)
                                )
                            }
                            _ => "✓ Allowed once.".to_string(),
                        };
                        self.entries.push(Entry::Note(said));
                        let _ = tx.send(a);
                        None
                    }
                    (None, reply) => Some(Modal::Permission {
                        title,
                        always_key,
                        reply,
                    }),
                    (Some(_), None) => None,
                }
            }
            Modal::Threads { items, sel } => match key.code {
                KeyCode::Esc => None,
                KeyCode::Enter => {
                    if let Some((id, _)) = items.get(sel)
                        && let Ok(Some(s)) = self.rt.load_session(id)
                    {
                        self.entries = entries_of(&s);
                        self.always.clear();
                        self.session = s;
                        self.scroll = 0;
                        self.status = "Thread opened.".into();
                    }
                    None
                }
                _ => {
                    let sel = step(sel, items.len());
                    Some(Modal::Threads { items, sel })
                }
            },
            Modal::Settings { sel } => match key.code {
                KeyCode::Esc => None,
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
                            sel: at,
                        })
                    }
                }
                _ => Some(Modal::Settings {
                    sel: step(sel, SETTINGS.len()),
                }),
            },
            Modal::Choose {
                option,
                title,
                items,
                sel,
            } => match key.code {
                KeyCode::Esc => Some(Modal::Settings { sel: 0 }),
                KeyCode::Enter => {
                    let (value, _) = items[sel].clone();
                    let value = if option == "decisions" {
                        OptionValue::Bool(value == "on")
                    } else {
                        OptionValue::Id(value)
                    };
                    match self.rt.set_option(&self.session.id, option, value) {
                        Ok(s) => {
                            self.session = s;
                            self.status = format!("{title} set.");
                        }
                        Err(e) => self.status = format!("Couldn't set {title}: {e}"),
                    }
                    let back = SETTINGS.iter().position(|(o, _)| *o == option).unwrap_or(0);
                    Some(Modal::Settings { sel: back })
                }
                _ => {
                    let sel = step(sel, items.len());
                    Some(Modal::Choose {
                        option,
                        title,
                        items,
                        sel,
                    })
                }
            },
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_are_attached_when_they_exist() {
        let dir = std::env::temp_dir().join(format!("pf-tui-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("my sheet.pdf"), b"%PDF").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let text = format!("look at {}/my\\ sheet.pdf please notes.txt", dir.display());
        assert_eq!(dropped_files(&text, &dir), vec![dir.join("my sheet.pdf")]);
    }
}
