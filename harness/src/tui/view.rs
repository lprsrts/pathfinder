//! Drawing: the conversation, the side panel, the input box, the status
//! line, and dialogs on top.

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

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let input_inner = area.width.saturating_sub(2).max(10);
    let input_rows = input_height(&app.input, input_inner) + u16::from(!app.attachments.is_empty());
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(input_rows.clamp(1, 8) + 2),
            Constraint::Length(1),
        ])
        .split(area);
    let side = app.show_side && area.width >= 90;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(if side {
            vec![Constraint::Min(40), Constraint::Length(SIDE_WIDTH)]
        } else {
            vec![Constraint::Min(10)]
        })
        .split(rows[0]);
    conversation(f, app, cols[0]);
    if side {
        side_panel(f, app, cols[1]);
    }
    input(f, app, rows[1]);
    status_line(f, app, rows[2]);
    if app.modal.is_some() {
        modal(f, app, area);
    }
}

fn conversation(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(2);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines: Vec<Line<'static>> = Vec::new();
    if app.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "Ask anything about what's in {}. F1 shows the keys.",
                app.cwd.display()
            ),
            dim,
        )));
    }
    let mut prev_thought = false;
    for entry in &app.entries {
        let thought = matches!(entry, Entry::Thought(_));
        match entry {
            Entry::User(text) => {
                lines.push(Line::default());
                let you = Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD);
                for (i, l) in text.lines().enumerate() {
                    let mut spans = vec![Span::styled(if i == 0 { "› " } else { "  " }, you)];
                    spans.push(Span::styled(
                        l.to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    lines.extend(markdown::wrap_hanging(&spans, width, 2));
                }
                lines.push(Line::default());
            }
            Entry::Text(text) => lines.extend(markdown::render(text, width, Style::default())),
            Entry::Thought(text) if app.show_thoughts => {
                let style = dim.add_modifier(Modifier::ITALIC);
                for l in text.lines().filter(|l| !l.trim().is_empty()) {
                    lines.extend(markdown::wrap(
                        &[Span::styled(format!("∴ {l}"), style)],
                        width,
                    ));
                }
            }
            Entry::Thought(_) => {
                if !prev_thought {
                    lines.push(Line::from(Span::styled(
                        "∴ thinking (Ctrl+T shows it)",
                        dim,
                    )));
                }
            }
            Entry::Tool {
                title, ok, detail, ..
            } => {
                let (mark, color) = match ok {
                    Some(true) => ("✓", Color::Green),
                    Some(false) => ("✗", Color::Red),
                    None => ("…", Color::DarkGray),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{mark} "), Style::default().fg(color)),
                    Span::styled(title.clone(), dim),
                ]));
                for d in detail.lines() {
                    lines.extend(markdown::wrap(
                        &[Span::styled(format!("    {d}"), dim)],
                        width,
                    ));
                }
            }
            Entry::Note(text) => lines.push(Line::from(Span::styled(
                text.clone(),
                Style::default().fg(Color::Yellow),
            ))),
        }
        prev_thought = thought;
    }
    if app.running.is_some() {
        lines.push(Line::from(Span::styled(
            format!("{} working · Esc stops", SPINNER[app.tick % SPINNER.len()]),
            Style::default().fg(Color::Cyan),
        )));
    }
    let height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(height);
    app.scroll = app.scroll.min(max_scroll);
    let start = max_scroll - app.scroll;
    let visible: Vec<Line> = lines.into_iter().skip(start).take(height).collect();
    let title = if app.scroll > 0 {
        format!(" {} · scrolled up (PageDown) ", folder_name(app))
    } else {
        format!(" {} ", folder_name(app))
    };
    f.render_widget(
        Paragraph::new(visible).block(Block::default().borders(Borders::ALL).title(title)),
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
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let width = area.width.saturating_sub(2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mode = if s.exam {
        "exam"
    } else if s.quick_check {
        "quick check"
    } else {
        match s.mode {
            SessionMode::Onboarding => "chat",
            SessionMode::Working => "task",
            SessionMode::Done => "task done",
        }
    };
    lines.push(Line::from(vec![
        Span::styled("Thread  ", dim),
        Span::styled(mode.to_string(), bold),
    ]));
    if let Some(goal) = &s.goal {
        lines.extend(markdown::wrap(&[Span::styled(goal.clone(), dim)], width));
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
    lines.push(Line::from(Span::styled("/why  your record", dim)));
    lines.push(Line::from(Span::styled("/map  the plan", dim)));
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Pathfinder ")),
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

fn input(f: &mut Frame, app: &App, area: Rect) {
    let inner_w = area.width.saturating_sub(2).max(1);
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
    for l in app.input.split('\n') {
        lines.push(Line::from(l.to_string()));
    }
    let title = if app.running.is_some() {
        " Message · Esc stops the reply "
    } else {
        " Message · Enter sends · Alt+Enter new line "
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
    if app.modal.is_none() {
        // The cursor: rows of the lines before it, then its column.
        let before = &app.input[..app.cursor];
        let mut row = u16::from(!app.attachments.is_empty());
        let mut segments = before.split('\n').peekable();
        let mut col = 0u16;
        while let Some(seg) = segments.next() {
            let w = seg.width() as u16;
            if segments.peek().is_some() {
                row += (w.max(1)).div_ceil(inner_w);
            } else {
                row += w / inner_w;
                col = w % inner_w;
            }
        }
        let max_row = area.height.saturating_sub(3);
        f.set_cursor_position((area.x + 1 + col, area.y + 1 + row.min(max_row)));
    }
}

fn status_line(f: &mut Frame, app: &App, area: Rect) {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let models = format!(
        "{} · {} · decisions {} · checks {}",
        app.current("system1"),
        app.current("system2"),
        if app.current("decisions") == "on" {
            app.current("decision_model")
        } else {
            "off".into()
        },
        app.current("checks"),
    );
    let left = if app.status.is_empty() {
        "Ctrl+P settings · Ctrl+O threads · Ctrl+N new · F1 keys".to_string()
    } else {
        app.status.clone()
    };
    let gap = (area.width as usize).saturating_sub(left.width() + models.width() + 1);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                left,
                if app.status.is_empty() {
                    dim
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ),
            Span::raw(" ".repeat(gap)),
            Span::styled(models, dim),
        ])),
        area,
    );
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
                    .title(format!(" {title} "))
                    .title_bottom(format!(" {hint} ")),
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
        Modal::Permission { title, .. } => {
            let rect = centered(area, 70, 7);
            f.render_widget(Clear, rect);
            f.render_widget(
                Paragraph::new(vec![
                    Line::from("The agent wants to run:"),
                    Line::from(Span::styled(
                        title.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    )),
                    Line::default(),
                    Line::from("[y] once   [a] always for this program   [n] no"),
                ])
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Allow this command? "),
                ),
                rect,
            );
        }
        Modal::Help => {
            let keys = [
                ("Enter", "send"),
                ("Alt+Enter, Ctrl+J", "new line"),
                ("Esc, Ctrl+C", "stop the reply"),
                ("PageUp/PageDown, Shift+↑↓", "scroll"),
                ("↑ (empty input)", "your last message again"),
                ("Ctrl+N", "new thread"),
                ("Ctrl+O", "open an earlier thread here"),
                ("Ctrl+P", "models, decisions, checks"),
                ("Ctrl+T", "show or hide thinking"),
                ("Ctrl+B", "show or hide the side panel"),
                ("/attach <file>", "attach a PDF or image (or drop it in)"),
                ("/why, /map", "your record, the plan"),
                ("Ctrl+C, Ctrl+D (idle)", "quit"),
            ];
            let rect = centered(area, 64, keys.len() as u16 + 4);
            f.render_widget(Clear, rect);
            let mut lines: Vec<Line> = keys
                .iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(
                            format!("{k:<28}"),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(v.to_string()),
                    ])
                })
                .collect();
            lines.push(Line::default());
            lines.push(Line::from("Any key closes this."));
            f.render_widget(
                Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Keys ")),
                rect,
            );
        }
    }
}
