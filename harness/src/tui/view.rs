//! Drawing. Borderless, the way Codex and Claude Code draw: the
//! conversation on top, a status row while a reply runs, a one-row plan
//! tray when the side panel doesn't fit, a rule, the input, and a footer.
//! Permission prompts take the input's place; dialogs float on top.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::markdown;
use super::{App, Entry, Modal, SETTINGS};
use crate::domain::{PartStatus, SessionMode};

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
const SIDE_WIDTH: u16 = 34;
/// From this width the side panel sits beside the conversation.
pub const DOCK_FROM: u16 = 110;
/// Output lines a tool shows before "… +N lines".
const TOOL_LINES: usize = 3;

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let docked = app.show_side && area.width >= DOCK_FROM;
    let permission = matches!(app.modal, Some(Modal::Permission { .. }));
    let bottom = if permission {
        permission_lines(app, area.width).len() as u16
    } else {
        input_height(&app.input, area.width.saturating_sub(2).max(10)).clamp(1, 10)
            + u16::from(!app.attachments.is_empty())
    };
    let status = app.running.is_some() || app.scroll > 0;
    let tray = !docked && !tray_text(app).is_empty();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(u16::from(status)),
            Constraint::Length(u16::from(tray)),
            Constraint::Length(1),
            Constraint::Length(bottom),
            Constraint::Length(1),
        ])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(if docked {
            vec![Constraint::Min(40), Constraint::Length(SIDE_WIDTH)]
        } else {
            vec![Constraint::Min(10)]
        })
        .split(rows[0]);
    conversation(f, app, cols[0]);
    if docked {
        side_panel(f, app, cols[1]);
    } else if app.side_overlay {
        let w = SIDE_WIDTH.min(area.width.saturating_sub(6));
        let rect = Rect {
            x: rows[0].x + rows[0].width - w,
            y: rows[0].y,
            width: w,
            height: rows[0].height,
        };
        f.render_widget(Clear, rect);
        side_panel(f, app, rect);
    }
    if status {
        status_row(f, app, rows[1]);
    }
    if tray {
        tray_row(f, app, rows[2]);
    }
    rule(f, app, rows[3]);
    if permission {
        f.render_widget(Paragraph::new(permission_lines(app, area.width)), rows[4]);
    } else {
        input(f, app, rows[4]);
    }
    footer(f, app, rows[5]);
    if app.modal.is_some() && !permission {
        modal(f, app, area);
    }
}

/// The conversation's lines at `width`, oldest first.
pub fn transcript(app: &App, width: u16) -> Vec<Line<'static>> {
    let body = width.saturating_sub(2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut prev: Option<&Entry> = None;
    for entry in &app.entries {
        // One blank line between items; tools and thoughts in a run stay together.
        let quiet = |e: &Entry| matches!(e, Entry::Tool { .. } | Entry::Thought(_));
        let together = prev.is_some_and(|p| quiet(p) && quiet(entry));
        let hidden_thought = match entry {
            Entry::Thought(_) => !app.show_thoughts,
            Entry::Text(t) => t.trim().is_empty(),
            _ => false,
        };
        if prev.is_some() && !together && !hidden_thought {
            lines.push(Line::default());
        }
        match entry {
            Entry::User(text) => {
                let you = Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD);
                for (i, l) in text.lines().enumerate() {
                    let spans = vec![
                        Span::styled(if i == 0 { "› " } else { "  " }, you),
                        Span::styled(l.to_string(), Style::default().add_modifier(Modifier::BOLD)),
                    ];
                    lines.extend(markdown::wrap_hanging(&spans, width, 2));
                }
            }
            Entry::Text(text) => {
                // Blank lines at the edges come from streaming, not the reply.
                let text = text.trim_matches('\n');
                for (i, mut l) in markdown::render(text, body, Style::default())
                    .into_iter()
                    .enumerate()
                {
                    let lead = if i == 0 {
                        Span::styled("• ", Style::default().fg(Color::Magenta))
                    } else {
                        Span::raw("  ")
                    };
                    l.spans.insert(0, lead);
                    lines.push(l);
                }
            }
            Entry::Thought(text) => {
                if app.show_thoughts {
                    let style = dim().add_modifier(Modifier::ITALIC);
                    for l in text.lines().filter(|l| !l.trim().is_empty()) {
                        lines.extend(markdown::wrap_hanging(
                            &[Span::styled(format!("∴ {l}"), style)],
                            width,
                            2,
                        ));
                    }
                }
            }
            Entry::Tool {
                title, ok, detail, ..
            } => {
                let (mark, color) = match ok {
                    Some(true) => ("✓", Color::Green),
                    Some(false) => ("✗", Color::Red),
                    None => ("●", Color::Cyan),
                };
                lines.extend(markdown::wrap_hanging(
                    &[
                        Span::styled(format!("{mark} "), Style::default().fg(color)),
                        Span::styled(title.clone(), dim()),
                    ],
                    width,
                    2,
                ));
                let out: Vec<&str> = detail.lines().collect();
                // Never hide a single line behind "+1 line".
                let shown = if out.len() <= TOOL_LINES + 1 {
                    out.len()
                } else {
                    TOOL_LINES
                };
                for (i, d) in out.iter().take(shown).enumerate() {
                    let lead = if i == 0 { "  └ " } else { "    " };
                    lines.extend(markdown::wrap_hanging(
                        &[Span::styled(format!("{lead}{d}"), dim())],
                        width,
                        4,
                    ));
                }
                if out.len() > shown {
                    lines.push(Line::from(Span::styled(
                        format!("    … +{} lines", out.len() - shown),
                        dim(),
                    )));
                }
            }
            Entry::Note(text) => lines.extend(markdown::wrap_hanging(
                &[Span::styled(
                    text.clone(),
                    Style::default().fg(Color::Yellow),
                )],
                width,
                2,
            )),
        }
        if !hidden_thought {
            prev = Some(entry);
        }
    }
    lines
}

fn conversation(f: &mut Frame, app: &mut App, area: Rect) {
    // A one-column margin on each side.
    let area = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    let mut lines = transcript(app, area.width);
    if app.entries.is_empty() {
        for hint in [
            format!("Ask anything about what's in {}.", app.cwd.display()),
            "Drop a PDF or a screenshot here to attach it. F1 shows the keys.".to_string(),
        ] {
            lines.extend(markdown::wrap(&[Span::styled(hint, dim())], area.width));
        }
    }
    let total = lines.len();
    // Scrolled up: keep the view where it is while new lines arrive.
    if app.scroll > 0 && total > app.last_total {
        app.scroll += total - app.last_total;
        app.new_below = true;
    }
    if app.scroll == 0 {
        app.new_below = false;
    }
    app.last_total = total;
    let height = area.height as usize;
    let max_scroll = total.saturating_sub(height);
    app.scroll = app.scroll.min(max_scroll);
    let start = max_scroll - app.scroll;
    let visible: Vec<Line> = lines.into_iter().skip(start).take(height).collect();
    f.render_widget(Paragraph::new(visible), area);
}

/// The latest thinking's headline: its last `**bold**` line, else its last line.
fn thought_header(app: &App) -> Option<String> {
    let thought = app
        .entries
        .iter()
        .rev()
        .take_while(|e| !matches!(e, Entry::User(_)))
        .find_map(|e| match e {
            Entry::Thought(t) => Some(t),
            _ => None,
        })?;
    let lines: Vec<&str> = thought
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let bold = lines.iter().rev().find_map(|l| {
        let inner = l.strip_prefix("**")?.strip_suffix("**")?;
        (!inner.is_empty()).then(|| inner.to_string())
    });
    bold.or_else(|| lines.last().map(|l| l.replace("**", "")))
}

fn elapsed(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// While a reply runs: spinner, time, how to stop it, what it's thinking
/// about. While scrolled up: how to get back.
fn status_row(f: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(1) as usize;
    let mut spans = vec![Span::raw(" ")];
    if app.running.is_some() {
        let secs = app.started.map_or(0, |t| t.elapsed().as_secs());
        let head = format!("{} Working ", SPINNER[app.tick % SPINNER.len()]);
        let meta = format!("({} · Esc stops)", elapsed(secs));
        let used = head.width() + meta.width();
        spans.push(Span::styled(head, Style::default().fg(Color::Cyan)));
        spans.push(Span::styled(meta, dim()));
        if let Some(h) = thought_header(app)
            && width > used + 6
        {
            spans.push(Span::styled(
                format!(" · {}", fit(&h, width - used - 3)),
                dim().add_modifier(Modifier::ITALIC),
            ));
        }
    }
    if app.scroll > 0 {
        let chip = if app.new_below {
            "↓ new below · End jumps back"
        } else {
            "↑ scrolled up · End jumps back"
        };
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        let gap = width.saturating_sub(used + chip.width());
        spans.push(Span::raw(" ".repeat(gap.max(1))));
        spans.push(Span::styled(chip, Style::default().fg(Color::Yellow)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Where the thread stands, in one line: the part, the count, what waits.
fn tray_text(app: &App) -> String {
    let s = &app.session;
    let mut bits = Vec::new();
    // What waits on you comes first, so a narrow window still shows it.
    if s.pending_candidate.is_some() {
        bits.push(
            if s.exam {
                "⧗ your answer"
            } else {
                "⧗ your verdict"
            }
            .to_string(),
        );
    }
    if !s.map.parts.is_empty() {
        let done = s
            .map
            .parts
            .iter()
            .filter(|p| matches!(p.status, PartStatus::Understood | PartStatus::Done))
            .count();
        let active = s
            .active
            .as_deref()
            .and_then(|id| s.map.parts.iter().find(|p| p.id == id))
            .map(|p| format!(" » {}", p.title))
            .unwrap_or_default();
        let what = if s.exam { "Exam" } else { "Parts" };
        bits.push(format!("{what} {done}/{}{active}", s.map.parts.len()));
    }
    bits.join(" · ")
}

fn tray_row(f: &mut Frame, app: &App, area: Rect) {
    let text = tray_text(app);
    let hint = " Ctrl+B plan";
    let room = (area.width as usize).saturating_sub(hint.width() + 2);
    let waiting = app.session.pending_candidate.is_some();
    let shown = fit(&text, room);
    let gap = (area.width as usize).saturating_sub(shown.width() + 1 + hint.width());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                shown,
                if waiting {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::Cyan)
                },
            ),
            Span::raw(" ".repeat(gap)),
            Span::styled(hint, dim()),
        ])),
        area,
    );
}

/// The thread's mode, for the rule and the side panel.
fn mode_name(app: &App) -> &'static str {
    let s = &app.session;
    if s.exam {
        "exam"
    } else if s.quick_check {
        "quick check"
    } else {
        match s.mode {
            SessionMode::Onboarding => "chat",
            SessionMode::Working => "task",
            SessionMode::Done => "task done",
        }
    }
}

/// A dim rule over the input, naming the folder and the thread's mode.
fn rule(f: &mut Frame, app: &App, area: Rect) {
    let label = format!(" {} · {} ", folder_name(app), mode_name(app));
    let label = fit(&label, (area.width as usize).saturating_sub(4));
    let rest = (area.width as usize).saturating_sub(2 + label.width());
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("──{label}{}", "─".repeat(rest)),
            dim(),
        ))),
        area,
    );
}

fn folder_name(app: &App) -> String {
    app.cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.cwd.display().to_string())
}

fn side_panel(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.session;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let width = area.width.saturating_sub(3);
    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled("Thread  ", dim()),
        Span::styled(mode_name(app).to_string(), bold),
    ])];
    if let Some(goal) = &s.goal {
        lines.extend(markdown::wrap(&[Span::styled(goal.clone(), dim())], width));
    }
    if s.pending_candidate.is_some() {
        lines.push(Line::default());
        let what = if s.exam {
            "An exam question waits for your answer."
        } else {
            "A candidate waits for your verdict: is it right, and why?"
        };
        lines.extend(markdown::wrap(
            &[Span::styled(
                what.to_string(),
                Style::default().fg(Color::Yellow),
            )],
            width,
        ));
    }
    if !s.map.parts.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Parts", bold)));
        for part in &s.map.parts {
            let active = s.active.as_deref() == Some(part.id.as_str());
            let (mark, color) = match part.status {
                _ if active => ("▶", Color::Cyan),
                PartStatus::Understood | PartStatus::Done => ("✓", Color::Green),
                PartStatus::Known => ("~", Color::Yellow),
                PartStatus::Delegated => ("⇢", Color::Magenta),
                PartStatus::InReview => ("⧗", Color::Yellow),
                PartStatus::Open => ("○", Color::DarkGray),
            };
            let spans = vec![
                Span::styled(format!("{mark} "), Style::default().fg(color)),
                Span::styled(
                    part.title.clone(),
                    if active { bold } else { Style::default() },
                ),
            ];
            lines.extend(markdown::wrap_hanging(&spans, width, 2));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("/why  your record", dim())));
    lines.push(Line::from(Span::styled("/map  the plan", dim())));
    if app.side_overlay {
        lines.push(Line::from(Span::styled("Esc closes", dim())));
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(dim())
                .padding(ratatui::widgets::Padding::left(1)),
        ),
        area,
    );
}

/// Rows the input text takes at `width`.
fn input_height(text: &str, width: u16) -> u16 {
    let w = width.max(1) as usize;
    text.split('\n')
        .map(|l| (l.width().max(1)).div_ceil(w))
        .sum::<usize>()
        .max(1) as u16
}

/// What the empty input says: what Pathfinder waits for.
fn placeholder(app: &App) -> String {
    if app.running.is_some() {
        "Esc stops the reply".into()
    } else if app.session.pending_candidate.is_some() && app.session.exam {
        "Your answer, as you'd give it in the exam".into()
    } else if app.session.pending_candidate.is_some() {
        "Right or wrong, and why? Or just ask your next question".into()
    } else {
        "Ask, or show your work".into()
    }
}

fn input(f: &mut Frame, app: &App, area: Rect) {
    let prompt = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let text_area = Rect {
        x: area.x + 2,
        width: area.width.saturating_sub(3).max(1),
        ..area
    };
    let inner_w = text_area.width;
    let mut lines: Vec<Line> = Vec::new();
    if !app.attachments.is_empty() {
        let names: Vec<String> = app
            .attachments
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        lines.push(Line::from(Span::styled(
            format!("📎 {}", names.join(", ")),
            Style::default().fg(Color::Cyan),
        )));
    }
    if app.input.is_empty() {
        lines.push(Line::from(Span::styled(placeholder(app), dim())));
    } else {
        for l in app.input.split('\n') {
            lines.push(Line::from(l.to_string()));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled("›", prompt))),
        Rect {
            width: 1,
            height: 1,
            ..area
        },
    );
    // Long input scrolls so the cursor's row stays in view.
    let before = &app.input[..app.cursor];
    let mut row = u16::from(!app.attachments.is_empty());
    let mut col = 0u16;
    let mut segments = before.split('\n').peekable();
    while let Some(seg) = segments.next() {
        let w = seg.width() as u16;
        if segments.peek().is_some() {
            row += (w.max(1)).div_ceil(inner_w);
        } else {
            row += w / inner_w;
            col = w % inner_w;
        }
    }
    let skip = row.saturating_sub(area.height.saturating_sub(1));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((skip, 0)),
        text_area,
    );
    if app.modal.is_none() {
        f.set_cursor_position((text_area.x + col, text_area.y + row - skip));
    }
}

/// The permission question, drawn where the input was.
fn permission_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(Modal::Permission { title, .. }) = &app.modal else {
        return Vec::new();
    };
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(Span::styled(
        "Allow the agent to run this?",
        bold.fg(Color::Yellow),
    ))];
    lines.extend(markdown::wrap_hanging(
        &[Span::styled(format!("  {title}"), bold)],
        width.saturating_sub(1),
        4,
    ));
    for (k, label) in [
        ("1", "Yes, once (y)"),
        (
            "2",
            "Yes, and don't ask again in this thread for this program (a)",
        ),
        ("3", "No (n, Esc)"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k}. "), key),
            Span::raw(label),
        ]));
    }
    lines
}

/// Key hints on the left, the models on the right; both shrink to fit.
fn footer(f: &mut Frame, app: &App, area: Rect) {
    let width = (area.width as usize).saturating_sub(1);
    let decisions = if app.current("decisions") == "on" {
        app.current("decision_model")
    } else {
        "off".into()
    };
    let (s1, s2, checks) = (
        app.current("system1"),
        app.current("system2"),
        app.current("checks"),
    );
    // Longest first: the model summary shrinks before the hints go.
    let models = [
        format!("{s1} · {s2} · decisions {decisions} · checks {checks}"),
        format!("{s1} · {s2} · checks {checks}"),
        format!("{s1} · {s2}"),
        s2.clone(),
        String::new(),
    ];
    let (left, left_style) = if app.status.is_empty() {
        let hints: &[&str] = if matches!(app.modal, Some(Modal::Permission { .. })) {
            &["1/y once · 2/a always · 3/n/Esc no", "y · a · n"]
        } else if app.modal.is_some() {
            &["Enter picks · Esc closes"]
        } else {
            &[
                "Enter send · Alt+Enter line · ^P settings · ^O threads · F1 keys",
                "^P settings · ^O threads · F1 keys",
                "F1 keys",
            ]
        };
        let pick = hints
            .iter()
            .find(|h| h.width() + 2 + models[2].width() <= width)
            .or(hints.last())
            .unwrap();
        (pick.to_string(), dim())
    } else {
        (app.status.clone(), Style::default().fg(Color::Yellow))
    };
    let left = fit(&left, width);
    let room = width.saturating_sub(left.width() + 2);
    let right = models
        .iter()
        .find(|m| m.width() <= room)
        .cloned()
        .unwrap_or_default();
    let gap = width.saturating_sub(left.width() + right.width());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(left, left_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, dim()),
        ])),
        area,
    );
}

/// `text` cut to `width` columns, with an ellipsis when it was cut.
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars() {
        if out.width() + ch.to_string().width() + 1 > width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn list(f: &mut Frame, area: Rect, title: &str, items: Vec<ListItem>, sel: usize, hint: &str) {
    let mut state = ListState::default();
    state.select(Some(sel));
    f.render_widget(Clear, area);
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(dim())
                    .title(Span::styled(
                        format!(" {title} "),
                        Style::default().add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(Span::styled(format!(" {hint} "), dim())),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("› "),
        area,
        &mut state,
    );
}

fn modal(f: &mut Frame, app: &App, area: Rect) {
    match app.modal.as_ref().unwrap() {
        Modal::Threads { items, sel } => {
            let rect = centered(area, 80, items.len() as u16 + 2);
            let rows = items
                .iter()
                .map(|(_, t)| ListItem::new(t.clone()))
                .collect();
            list(
                f,
                rect,
                "Threads in this folder",
                rows,
                *sel,
                "Enter opens · Esc closes",
            );
        }
        Modal::Settings { sel } => {
            let rect = centered(area, 60, SETTINGS.len() as u16 + 2);
            let rows = SETTINGS
                .iter()
                .map(|(option, name)| ListItem::new(format!("{name:<20} {}", app.current(option))))
                .collect();
            list(
                f,
                rect,
                "Settings for this thread",
                rows,
                *sel,
                "Enter changes · Esc closes",
            );
        }
        Modal::Choose {
            title, items, sel, ..
        } => {
            let rect = centered(area, 70, items.len() as u16 + 2);
            let rows = items
                .iter()
                .map(|(_, l)| ListItem::new(l.clone()))
                .collect();
            list(f, rect, title, rows, *sel, "Enter picks · Esc goes back");
        }
        Modal::Permission { .. } => {}
        Modal::Help => {
            let keys = [
                ("Enter", "send"),
                ("Alt+Enter, Ctrl+J", "new line"),
                ("Esc, Ctrl+C", "stop the reply"),
                ("PageUp/PageDown, wheel", "scroll"),
                ("↑↓ (empty input)", "scroll"),
                ("End (empty input)", "back to the latest"),
                ("Alt+↑", "your last message again"),
                ("Ctrl+N", "new thread"),
                ("Ctrl+O", "open an earlier thread here"),
                ("Ctrl+P", "models, decisions, checks"),
                ("Ctrl+T", "show or hide thinking"),
                ("Ctrl+B", "show or hide the plan panel"),
                ("/attach <file>", "attach a PDF or image (or drop it in)"),
                ("/why, /map", "your record, the plan"),
                (
                    "Ctrl+C, Ctrl+D (idle)",
                    "quit; the conversation stays in your terminal",
                ),
            ];
            let rect = centered(area, 72, keys.len() as u16 + 4);
            f.render_widget(Clear, rect);
            let mut lines: Vec<Line> = keys
                .iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(
                            format!("{k:<26}"),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(v.to_string()),
                    ])
                })
                .collect();
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("Any key closes this.", dim())));
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(dim())
                        .title(Span::styled(
                            " Keys ",
                            Style::default().add_modifier(Modifier::BOLD),
                        )),
                ),
                rect,
            );
        }
    }
}
