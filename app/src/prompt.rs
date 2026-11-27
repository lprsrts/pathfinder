//! The sign-in panel: [`lines`] lays a [`Flow`]'s step out (title, rows
//! with an arrow on the selected one, a filter or field line, the hint),
//! and the TUI draws it in place of its input while `pathfinder auth`
//! draws it here, in place under the cursor (no full screen), with arrow
//! keys, type-to-filter, line editing and Esc to go back. Keys are typed
//! into masked fields. [`drive`] only reads crossterm events, no runtime
//! needed.

use std::io::{self, IsTerminal, Write};

use anyhow::Result;
use crossterm::cursor::{Hide, MoveToColumn, MoveUp, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{execute, queue};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use futures_util::StreamExt;
use pathfinder::auth::flow::{Action, Flow, Job, Screen};
use pathfinder::picker::{display_rows, is_header, split_note};

/// Whether there is a terminal to ask in: stdin and stdout both.
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Raw mode for as long as it lives; puts the terminal back when dropped
/// (also on a panic's unwind).
struct Raw;

impl Raw {
    fn on() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let _ = execute!(io::stdout(), EnableBracketedPaste);
        Ok(Self)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, Show);
        let _ = terminal::disable_raw_mode();
    }
}

/// How a piece of a line is drawn. Both front ends map these onto their
/// palette: the accent is the TUI's cyan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    /// The step's title: accent, bold.
    Title,
    Dim,
    /// The selected row's marker and name.
    Accent,
    /// A state that is there to use (✓ …).
    Good,
    Error,
}

/// One line to draw: pieces, each in its tone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line(pub Vec<(Tone, String)>);

impl Line {
    fn one(tone: Tone, text: impl Into<String>) -> Self {
        Self(vec![(tone, text.into())])
    }

    fn blank() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        self.0.iter().map(|(_, t)| t.as_str()).collect()
    }
}

/// Cuts `s` to `width` columns, with `…` when it was longer.
pub fn fit(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// Breaks `s` into lines of at most `width` columns, at spaces when it can.
fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in s.split(' ') {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if candidate.width() <= width || line.is_empty() {
            line = candidate;
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    out.push(line);
    out.into_iter().map(|l| fit(&l, width)).collect()
}

/// `s` in pieces of at most `width` columns, unbroken otherwise (an address).
fn chunks(s: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for c in s.chars() {
        let last = out.last_mut().expect("one line at least");
        if last.width() + c.width().unwrap_or(0) > width && !last.is_empty() {
            out.push(String::new());
        }
        out.last_mut().expect("one line at least").push(c);
    }
    out
}

/// Lists with more rows than this show where the selection is, `(n/N)`.
const FEW: usize = 5;

/// The most list rows a panel shows at once (the counter under them says
/// where the rest are), so the conversation above stays in view.
const LIST_MAX: usize = 8;

/// One row of a list panel: a section header (drawn dim, never
/// selected), or an item: a name, a dim mark after it (the palette's `◆`)
/// and a note in a column of its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuRow<'a> {
    pub header: bool,
    pub name: &'a str,
    pub mark: &'a str,
    pub note: &'a str,
}

/// A list as the sign-in panel and the command palette draw it: an arrow
/// and the accent on the selected row only, names in one column, notes
/// muted, and `(n/N)` under it when it is long.
#[derive(Debug, Clone, Default)]
pub struct Menu<'a> {
    pub rows: Vec<MenuRow<'a>>,
    /// The selected row, an index into `rows`.
    pub selected: Option<usize>,
    /// The selection's place among the items (1-based) and how many match.
    pub place: (usize, usize),
    /// Where notes start: the widest name (and mark), plus a gap.
    pub col: usize,
    /// Always count under the list, with this after the count.
    pub legend: Option<&'a str>,
}

impl<'a> Menu<'a> {
    /// The name column for these rows: as wide as the widest name and mark,
    /// plus two.
    pub fn column(rows: &[MenuRow]) -> usize {
        rows.iter()
            .filter(|r| !r.header)
            .map(|r| r.name.width() + r.mark.width())
            .max()
            .unwrap_or(0)
            + 2
    }
}

/// The lines of `menu` at `width` columns, in at most `avail` rows (the
/// counter, when shown, takes one): the rows scroll to keep the selection
/// in view.
pub fn menu_lines(menu: &Menu, width: usize, avail: usize) -> Vec<Line> {
    let (n, total) = menu.place;
    let counter = menu.legend.is_some() || total > FEW || menu.rows.len() > avail;
    let room = if counter {
        avail.saturating_sub(1).max(2)
    } else {
        avail
    };
    let at = menu.selected.unwrap_or(0);
    let len = menu.rows.len();
    let mut start = at
        .saturating_sub(room.saturating_sub(1))
        .min(len.saturating_sub(room));
    let sectioned = menu.rows.iter().any(|r| r.header);
    let indent = if sectioned { "  " } else { "" };
    let mut out = Vec::new();
    // A header with none of its rows in view is never last: the window
    // moves a row (up, or down past it while the selection keeps its own
    // header), or a blank row takes its place, so the menu keeps its height
    // while the selection moves.
    let mut end = (start + room).min(len);
    let mut blank = false;
    if end > start + 1 && menu.rows[end - 1].header {
        if start > 0 {
            start -= 1;
            end -= 1;
        } else if end < len && (at > start + 1 || !menu.rows[start].header) {
            start += 1;
            end += 1;
        } else {
            end -= 1;
            blank = true;
        }
    }
    let room = end - start;
    for (i, row) in menu.rows.iter().enumerate().skip(start).take(room) {
        if row.header {
            out.push(Line::one(Tone::Dim, fit(&format!("  {}", row.name), width)));
        } else {
            out.push(row_line(
                row,
                menu.col,
                width,
                menu.selected == Some(i),
                indent,
            ));
        }
    }
    if blank {
        out.push(Line::default());
    }
    if counter && total > 0 {
        let mut count = vec![(Tone::Dim, format!("  ({n}/{total})"))];
        if let Some(legend) = menu.legend.filter(|l| !l.is_empty()) {
            let room = width.saturating_sub(count[0].1.width() + 3);
            if room > 4 {
                count.push((Tone::Dim, format!(" · {}", fit(legend, room))));
            }
        }
        out.push(Line(count));
    }
    out
}

/// A list row: the marker, the name (and its mark) padded to `col`, the
/// note after it. Fitted to `width`: the padding goes first, then the note
/// is cut with `…` (only when even the name and note alone don't fit).
fn row_line(row: &MenuRow, col: usize, width: usize, selected: bool, indent: &str) -> Line {
    let marker = if selected { "→ " } else { "  " };
    let lead = format!("{marker}{indent}");
    let room = width.saturating_sub(lead.width());
    let name = fit(row.name, room);
    let mark = if row.mark.is_empty() || name.width() + row.mark.width() > room {
        String::new()
    } else {
        row.mark.to_string()
    };
    let mut pieces = vec![
        (
            if selected { Tone::Accent } else { Tone::Dim },
            marker.to_string(),
        ),
        (Tone::Plain, indent.to_string()),
    ];
    // A qualifier after ` · ` in a name ("· pay per use") is dimmed.
    let (main, qualifier) = match name.split_once(" · ") {
        Some((main, rest)) => (main.to_string(), format!(" · {rest}")),
        None => (name.clone(), String::new()),
    };
    pieces.push((if selected { Tone::Accent } else { Tone::Plain }, main));
    if !qualifier.is_empty() {
        pieces.push((Tone::Dim, qualifier));
    }
    if !mark.is_empty() {
        pieces.push((Tone::Dim, mark.clone()));
    }
    let named = name.width() + mark.width();
    let note = row.note;
    if !note.is_empty() {
        let after = room.saturating_sub(named);
        let pad = col.saturating_sub(named).max(2);
        let (pad, note) = if pad + note.width() <= after {
            (pad, note.to_string())
        } else if 2 + note.width() <= after {
            (2, note.to_string())
        } else if after > 3 {
            (2, fit(note, after - 2))
        } else {
            (0, String::new())
        };
        if !note.is_empty() {
            let tone = if note.starts_with('✓') {
                Tone::Good
            } else if selected {
                Tone::Accent
            } else {
                Tone::Dim
            };
            pieces.push((Tone::Plain, " ".repeat(pad)));
            pieces.push((tone, note));
        }
    }
    Line(pieces)
}

/// The panel for a screen, at `width` columns and at most `max_rows` rows
/// (the list scrolls to fit), with the cursor's (row, column) when typing
/// goes somewhere: the filter or a field. The TUI draws it where the input
/// was; `pathfinder auth login` under the prompt. With `show_url`, a
/// sign-in address is drawn too, in pieces (the terminal prints it whole
/// above instead).
pub fn lines(
    screen: &Screen,
    width: usize,
    max_rows: usize,
    show_url: bool,
) -> (Vec<Line>, Option<(usize, usize)>) {
    let width = width.max(20);
    let mut head = vec![
        Line::one(Tone::Title, fit(&screen.title, width)),
        Line::blank(),
    ];
    for b in &screen.body {
        head.extend(
            wrap(b, width)
                .into_iter()
                .map(|l| Line::one(Tone::Plain, l)),
        );
    }
    if let (true, Some((note, url))) = (show_url, screen.url) {
        head.extend(
            wrap(note, width)
                .into_iter()
                .map(|l| Line::one(Tone::Dim, l)),
        );
        head.extend(
            chunks(url, width)
                .into_iter()
                .map(|l| Line::one(Tone::Accent, l)),
        );
    }
    if head.len() > 2 && (screen.list.is_some() || screen.field.is_some()) {
        head.push(Line::blank());
    }
    let mut cursor = None;
    let filter = screen.list.filter(|p| p.filterable());
    if let Some(p) = filter {
        // The filter is a line of input, the cursor after what is typed. A
        // key typed into it by mistake shows as dots, as in a key field.
        let query = if pathfinder::auth::accounts::looks_like_key(&p.query) {
            "•".repeat(p.query.chars().count())
        } else {
            p.query.clone()
        };
        let shown = fit(&query, width.saturating_sub(3));
        cursor = Some((head.len(), 2 + shown.width()));
        head.push(Line(vec![
            (Tone::Accent, "> ".into()),
            (Tone::Plain, shown),
        ]));
        head.push(Line::blank());
    }

    let mut tail = Vec::new();
    let mut field_row = None;
    if let Some((shown, at)) = &screen.field {
        // Long text scrolls so the cursor stays in view.
        let room = width.saturating_sub(3).max(1);
        let chars: Vec<char> = shown.chars().collect();
        let start = (*at + 1).saturating_sub(room);
        let visible: String = chars.iter().skip(start).take(room).collect();
        field_row = Some((tail.len(), 2 + at - start));
        tail.push(Line(vec![
            (Tone::Accent, "> ".into()),
            (Tone::Plain, visible),
        ]));
    }
    if let Some(e) = screen.error {
        if !tail.is_empty() || screen.list.is_some() {
            tail.push(Line::blank());
        }
        tail.extend(
            wrap(e, width)
                .into_iter()
                .map(|l| Line::one(Tone::Error, l)),
        );
    }
    tail.push(Line::blank());
    tail.push(Line::one(Tone::Dim, fit(&screen.hint, width)));

    let mut list = Vec::new();
    if let Some(p) = screen.list {
        let matched = p.matches();
        let shown = display_rows(&p.items, &p.query);
        // What the list may take; a counter, when shown, takes a row of it.
        let avail = max_rows
            .saturating_sub(head.len() + tail.len())
            .clamp(3, LIST_MAX + 1);
        if matched.is_empty() && !p.items.is_empty() {
            list.push(Line::one(Tone::Dim, "  No matches."));
        }
        let sel = p.sel.min(matched.len().saturating_sub(1));
        let selected = matched.get(sel).copied();
        let rows: Vec<MenuRow> = shown
            .iter()
            .map(|&i| {
                let (_, label) = &p.items[i];
                if is_header(&p.items[i]) {
                    MenuRow {
                        header: true,
                        name: label,
                        ..MenuRow::default()
                    }
                } else {
                    let (name, note) = split_note(label);
                    MenuRow {
                        name,
                        note,
                        ..MenuRow::default()
                    }
                }
            })
            .collect();
        // Names in one column, as wide as the widest (of all, so the column
        // stays put while the list scrolls or filters).
        let col = p
            .items
            .iter()
            .filter(|i| !is_header(i))
            .map(|(_, l)| split_note(l).0.width())
            .max()
            .unwrap_or(0)
            + 2;
        let menu = Menu {
            rows,
            selected: shown.iter().position(|&i| Some(i) == selected),
            place: (sel + 1, matched.len()),
            col,
            legend: None,
        };
        list.extend(menu_lines(&menu, width, avail));
    }

    let mut out = head;
    out.extend(list);
    if let Some((row, col)) = field_row {
        cursor = Some((out.len() + row, col));
    }
    out.extend(tail);
    (out, cursor)
}

/// Draws a block of lines in place, replacing the one drawn before.
struct Painter {
    /// Rows between the block's first line and the cursor.
    cursor_row: usize,
    drawn: bool,
}

impl Painter {
    fn new() -> Self {
        Self {
            cursor_row: 0,
            drawn: false,
        }
    }

    fn erase(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.drawn {
            queue!(out, MoveToColumn(0))?;
            if self.cursor_row > 0 {
                queue!(out, MoveUp(self.cursor_row as u16))?;
            }
            queue!(out, Clear(ClearType::FromCursorDown))?;
        }
        self.drawn = false;
        self.cursor_row = 0;
        Ok(())
    }

    fn draw(&mut self, out: &mut impl Write, screen: &Screen) -> io::Result<()> {
        let (w, h) = terminal::size().unwrap_or((80, 24));
        let (mut lines, mut cursor) = lines(
            screen,
            w.saturating_sub(1) as usize,
            h.saturating_sub(3) as usize,
            false,
        );
        // A blank line between the command and the panel, as in the TUI.
        lines.insert(0, Line::blank());
        cursor = cursor.map(|(row, col)| (row + 1, col));
        self.erase(out)?;
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                queue!(out, Print("\r\n"))?;
            }
            for (tone, text) in &line.0 {
                match tone {
                    Tone::Plain => {}
                    // The TUI's palette: ratatui's Cyan, Green and Red are
                    // crossterm's Dark* (ANSI 6, 2, 1).
                    Tone::Title => queue!(
                        out,
                        SetForegroundColor(Color::DarkCyan),
                        SetAttribute(Attribute::Bold)
                    )?,
                    Tone::Dim => queue!(out, SetAttribute(Attribute::Dim))?,
                    Tone::Accent => queue!(out, SetForegroundColor(Color::DarkCyan))?,
                    Tone::Good => queue!(out, SetForegroundColor(Color::DarkGreen))?,
                    Tone::Error => queue!(out, SetForegroundColor(Color::DarkRed))?,
                }
                queue!(out, Print(text), SetAttribute(Attribute::Reset), ResetColor)?;
            }
        }
        let last = lines.len().saturating_sub(1);
        match cursor {
            Some((row, col)) => {
                if last > row {
                    queue!(out, MoveUp((last - row) as u16))?;
                }
                queue!(out, MoveToColumn(col as u16), Show)?;
                self.cursor_row = row;
            }
            None => {
                queue!(out, Hide)?;
                self.cursor_row = last;
            }
        }
        self.drawn = true;
        out.flush()
    }
}

/// Shows `flow` until it closes (None) or hands over a job to run. The
/// block is erased either way; the caller says what happened.
pub fn drive(flow: &mut Flow) -> Result<Option<Job>> {
    let _raw = Raw::on()?;
    let mut out = io::stdout();
    let mut painter = Painter::new();
    let result = loop {
        painter.draw(&mut out, &flow.screen())?;
        let action = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Release => continue,
            Event::Key(key)
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c' | 'd')) =>
            {
                Action::Close
            }
            Event::Key(key) => flow.on_key(key),
            Event::Paste(text) => {
                flow.on_paste(&text);
                Action::None
            }
            _ => Action::None,
        };
        match action {
            // The address is printed whole above the block already.
            Action::None | Action::Copy(_) => {}
            Action::Close | Action::Cancel => break None,
            Action::Run(job) => break Some(job),
        }
    };
    painter.erase(&mut out)?;
    out.flush()?;
    Ok(result)
}

/// How a job shown by [`wait`] ended.
pub enum JobEnd {
    Done(Result<String>),
    /// Esc or Ctrl+C: the job was dropped (a sign-in's callback listener
    /// with it) and the flow is back a step, or closed.
    Cancelled,
}

fn is_ctrl_c(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'd'))
}

/// Shows the flow's busy step in place while `job` runs, so Esc and Ctrl+C
/// cancel it instead of printing escape codes. An address the job gives
/// (`urls`) is printed once in full above the block, where it can be copied
/// whole and stays after the block is gone.
pub async fn wait(
    flow: &mut Flow,
    job: impl std::future::Future<Output = Result<String>>,
    mut urls: tokio::sync::mpsc::UnboundedReceiver<String>,
) -> Result<JobEnd> {
    let _raw = Raw::on()?;
    let mut out = io::stdout();
    let mut painter = Painter::new();
    let mut events = EventStream::new();
    tokio::pin!(job);
    let mut urls_open = true;
    let end = loop {
        painter.draw(&mut out, &flow.screen())?;
        tokio::select! {
            result = &mut job => break JobEnd::Done(result),
            url = urls.recv(), if urls_open => match url {
                Some(url) => {
                    flow.set_url(&url);
                    let note = flow.screen().url.map(|(n, _)| n.to_string()).unwrap_or_default();
                    painter.erase(&mut out)?;
                    queue!(out, Print(format!("{note}\r\n{url}\r\n\r\n")))?;
                }
                None => urls_open = false,
            },
            ev = events.next() => match ev {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    let key = if is_ctrl_c(&key) {
                        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
                    } else {
                        key
                    };
                    match flow.on_key(key) {
                        Action::Cancel | Action::Close => break JobEnd::Cancelled,
                        Action::Copy(url) => {
                            flow.error = Some(match crate::tui::copy_to_clipboard(&url) {
                                Ok(()) => "Copied the address.".into(),
                                Err(e) => format!("Couldn't copy: {e}"),
                            });
                        }
                        _ => {}
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => {}
            },
        }
    };
    painter.erase(&mut out)?;
    out.flush()?;
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use pathfinder::auth::KeySource;
    use pathfinder::auth::accounts::{METHOD_API_KEY, Row, RowView, State};
    use pathfinder::auth::flow::Mode;

    fn text(lines: &[Line]) -> String {
        lines.iter().map(Line::text).collect::<Vec<_>>().join("\n")
    }

    fn views() -> Vec<RowView> {
        vec![
            RowView::new(
                Row::Chatgpt,
                State::SignedIn {
                    email: Some("someone@example.com".into()),
                },
            ),
            RowView::new(
                Row::AnthropicApi,
                State::Key {
                    hint: "…zQAA".into(),
                    source: KeySource::File,
                },
            ),
            RowView::new(
                Row::OpenaiApi,
                State::Key {
                    hint: "…vxoA".into(),
                    source: KeySource::Env("OPENAI_API_KEY"),
                },
            ),
            RowView::new(Row::Endpoint("lab".into()), State::NoKeyNeeded),
            RowView::new(Row::Gate, State::NotSet),
        ]
    }

    fn press(flow: &mut Flow, code: KeyCode) {
        flow.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn the_first_step_draws_like_a_panel_with_an_arrow_on_the_selection() {
        let flow = Flow::login(views());
        let (lines, cursor) = lines(&flow.screen(), 100, 20, false);
        assert!(cursor.is_none());
        assert_eq!(lines[0], Line::one(Tone::Title, "Select how to connect:"));
        assert_eq!(lines[1], Line::blank());
        let first = &lines[2];
        assert_eq!(first.0[0], (Tone::Accent, "→ ".into()));
        assert!(first.text().contains("Sign in with your ChatGPT plan"));
        assert!(
            first
                .0
                .iter()
                .any(|(t, s)| *t == Tone::Good && s == "✓ signed in as someone@example.com")
        );
        assert!(
            lines[3]
                .text()
                .starts_with("  Use an API key · pay per use")
        );
        assert!(
            lines[4]
                .text()
                .starts_with("  Add your own server · advanced")
        );
        assert_eq!(
            lines.last().unwrap().text(),
            "↑↓ navigate · Enter select · Esc cancel"
        );
        let all = text(&lines);
        for gone in ["Claude Code", "Codex", "TypeSafe", "Gate", "(1/"] {
            assert!(!all.contains(gone), "{gone}: {all}");
        }
    }

    #[test]
    fn the_providers_have_a_filter_line_short_states_and_a_counter_when_long() {
        let mut flow = Flow::login(views());
        flow.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(flow.picker().unwrap().selected(), Some(METHOD_API_KEY));
        press(&mut flow, KeyCode::Enter);
        let (lines, cursor) = lines(&flow.screen(), 80, 20, false);
        assert_eq!(lines[0].text(), "Select a provider:");
        assert_eq!(lines[2].text(), "> ");
        assert_eq!(cursor, Some((2, 2)));
        let all = text(&lines);
        assert!(all.contains("→ Anthropic  ✓ stored"), "{all}");
        assert!(all.contains("  OpenAI     ✓ from $OPENAI_API_KEY"), "{all}");
        assert!(all.contains("  lab        no key needed"), "{all}");
        for never in ["…", "pay per use", "auth file", "model"] {
            assert!(!all.contains(never), "{never}: {all}");
        }
        assert!(!all.contains("(1/3)"), "three rows: no counter");
        // Typing edits the filter line; the cursor follows.
        for c in "lax".chars() {
            press(&mut flow, KeyCode::Char(c));
        }
        press(&mut flow, KeyCode::Backspace);
        let (lines, cursor) = super::lines(&flow.screen(), 80, 20, false);
        assert_eq!(lines[2].text(), "> la");
        assert_eq!(cursor, Some((2, 4)));
        assert!(text(&lines).contains("→ lab"));

        // Many rows, or too few to show them all: (n/N) under the list.
        let many: Vec<RowView> = (0..9)
            .map(|i| RowView::new(Row::Endpoint(format!("server{i}")), State::NoKeyNeeded))
            .collect();
        let mut flow = Flow::login(many);
        press(&mut flow, KeyCode::Down);
        press(&mut flow, KeyCode::Enter);
        press(&mut flow, KeyCode::Down);
        let (lines, _) = super::lines(&flow.screen(), 80, 40, false);
        assert!(text(&lines).contains("  (2/9)"), "{}", text(&lines));
        let (short, _) = super::lines(&flow.screen(), 80, 12, false);
        assert!(short.len() <= 12, "{}", text(&short));
        assert!(text(&short).contains("→ server1"), "{}", text(&short));
        assert!(text(&short).contains("(2/9)"));
    }

    #[test]
    fn every_step_fits_at_80_100_and_120_columns_without_cutting_at_80() {
        let mut flows = vec![Flow::login(views()), Flow::logout(views())];
        let mut providers = Flow::login(views());
        press(&mut providers, KeyCode::Down);
        press(&mut providers, KeyCode::Enter);
        flows.push(providers);
        flows.push(Flow::at(Mode::Login, views(), &Row::OpenaiApi).0);
        flows.push(Flow::at(Mode::Login, views(), &Row::Advanced).0);
        flows.push(Flow::gate(views()));
        for width in [80, 100, 120] {
            for flow in &flows {
                // As the TUI and the terminal give it: a column each side.
                let (lines, _) = super::lines(&flow.screen(), width - 2, 30, true);
                for l in &lines {
                    assert!(l.text().width() <= width - 2, "{width}: {:?}", l.text());
                    if width == 80 {
                        assert!(!l.text().contains('…'), "cut at 80: {:?}", l.text());
                    }
                }
            }
        }
        // Narrower, a long state is cut last, with …, after the padding.
        let (narrow, _) = super::lines(&Flow::login(views()).screen(), 40, 20, false);
        assert!(narrow.iter().all(|l| l.text().width() <= 40), "{narrow:?}");
        assert!(
            narrow[2]
                .text()
                .starts_with("→ Sign in with your ChatGPT plan  ✓ "),
            "{:?}",
            narrow[2].text()
        );
        assert!(narrow[2].text().ends_with('…'));
    }

    #[test]
    fn a_typed_key_draws_as_dots_with_the_cursor_after_them() {
        let (mut flow, _) = Flow::at(Mode::Login, views(), &Row::OpenaiApi);
        // $OPENAI_API_KEY is set: paste another.
        press(&mut flow, KeyCode::End);
        press(&mut flow, KeyCode::Enter);
        flow.on_paste("sk-proj-THE-REAL-SECRET-0123456789");
        let (lines, cursor) = lines(&flow.screen(), 60, 20, false);
        let all = text(&lines);
        assert!(!all.contains("SECRET"), "{all}");
        assert!(!all.contains("sk-proj"), "{all}");
        assert_eq!(lines[0].text(), "OpenAI API key:");
        let (row, col) = cursor.unwrap();
        let field = lines[row].text();
        assert!(field.starts_with("> •"));
        assert_eq!(col, field.width());
    }

    #[test]
    fn a_long_list_shows_at_most_eight_rows_and_a_key_typed_into_its_filter_is_dots() {
        let many: Vec<RowView> = (0..11)
            .map(|i| RowView::new(Row::Endpoint(format!("server{i}")), State::NoKeyNeeded))
            .collect();
        let mut flow = Flow::login(many);
        press(&mut flow, KeyCode::Down);
        press(&mut flow, KeyCode::Enter);
        let (lines, _) = super::lines(&flow.screen(), 78, 60, false);
        let rows = lines.iter().filter(|l| l.text().contains("server")).count();
        assert_eq!(rows, LIST_MAX, "{}", text(&lines));
        assert!(text(&lines).contains("(1/11)"));
        press(&mut flow, KeyCode::End);
        let (lines, _) = super::lines(&flow.screen(), 78, 60, false);
        assert!(text(&lines).contains("→ server10"), "{}", text(&lines));
        assert!(text(&lines).contains("(11/11)"));
        // Typed key by key (no bracketed paste): drawn as dots once it
        // looks like a key.
        for c in "sk-ant-TYPEDKEY".chars() {
            press(&mut flow, KeyCode::Char(c));
        }
        let (lines, cursor) = super::lines(&flow.screen(), 78, 60, false);
        let all = text(&lines);
        assert!(!all.contains("TYPEDKEY") && !all.contains("sk-"), "{all}");
        let (row, col) = cursor.unwrap();
        assert!(lines[row].text().starts_with("> •••"));
        assert_eq!(col, lines[row].text().width());
    }

    #[test]
    fn a_method_qualifier_is_dimmed() {
        let flow = Flow::login(views());
        let (lines, _) = lines(&flow.screen(), 100, 20, false);
        assert!(lines[3].0.contains(&(Tone::Plain, "Use an API key".into())));
        assert!(lines[3].0.contains(&(Tone::Dim, " · pay per use".into())));
    }

    #[test]
    fn a_menu_marks_only_the_selected_row_and_counts_with_a_legend() {
        let rows = vec![
            MenuRow {
                header: true,
                name: "Read & search",
                ..MenuRow::default()
            },
            MenuRow {
                name: "open",
                note: "<file> — show a file",
                ..MenuRow::default()
            },
            MenuRow {
                name: "web",
                mark: " ◆",
                note: "<terms> — search online",
                ..MenuRow::default()
            },
            MenuRow {
                header: true,
                name: "Ask",
                ..MenuRow::default()
            },
            MenuRow {
                name: "tell",
                mark: " ◆",
                note: "<question> — get an answer",
                ..MenuRow::default()
            },
        ];
        let menu = Menu {
            col: Menu::column(&rows),
            rows,
            selected: Some(2),
            place: (2, 3),
            legend: Some("◆ calls the model"),
        };
        assert_eq!(menu.col, 8);
        let lines = menu_lines(&menu, 60, 9);
        let texts: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(
            texts,
            [
                "  Read & search",
                "    open    <file> — show a file",
                "→   web ◆   <terms> — search online",
                "  Ask",
                "    tell ◆  <question> — get an answer",
                "  (2/3) · ◆ calls the model",
            ]
        );
        // Only the selected row is in the accent, its note too; the mark
        // and the other notes are dim.
        assert!(lines[2].0.contains(&(Tone::Accent, "web".into())));
        assert!(lines[2].0.contains(&(Tone::Dim, " ◆".into())));
        assert!(
            lines[2]
                .0
                .contains(&(Tone::Accent, "<terms> — search online".into()))
        );
        assert!(lines[1].0.contains(&(Tone::Plain, "open".into())));
        assert!(
            lines[1]
                .0
                .contains(&(Tone::Dim, "<file> — show a file".into()))
        );
        // Scrolled: a header is never the last row in view; the window
        // moves past it instead, so the menu keeps its height.
        let lines = menu_lines(&menu, 60, 5);
        let texts: Vec<String> = lines.iter().map(Line::text).collect();
        assert_eq!(texts.len(), 5, "{texts:?}");
        assert_ne!(texts[3], "  Ask", "{texts:?}");
        assert!(texts.iter().any(|t| t.starts_with("→")), "{texts:?}");
        // Every selection keeps the same height.
        for sel in [1, 2, 4] {
            let moved = Menu {
                selected: Some(sel),
                ..menu.clone()
            };
            assert_eq!(menu_lines(&moved, 60, 5).len(), 5, "selected {sel}");
        }
    }

    #[test]
    fn the_provider_filter_ranks_prefixes_first() {
        let mut flow = Flow::login(views());
        press(&mut flow, KeyCode::Down);
        press(&mut flow, KeyCode::Enter);
        // "o" is in Anthropic too, but OpenAI starts with it.
        press(&mut flow, KeyCode::Char('o'));
        let (lines, _) = super::lines(&flow.screen(), 80, 20, false);
        let all = text(&lines);
        let openai = all.find("OpenAI").unwrap();
        let anthropic = all.find("Anthropic").unwrap();
        assert!(openai < anthropic, "{all}");
        assert!(all.contains("→ OpenAI"), "{all}");
    }

    #[test]
    fn long_text_wraps_inside_the_width() {
        let w = wrap("one two three four five six seven", 10);
        assert!(w.iter().all(|l| l.width() <= 10), "{w:?}");
        assert_eq!(w.join(" "), "one two three four five six seven");
        assert_eq!(chunks("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }
}
