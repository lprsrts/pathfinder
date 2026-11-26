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

use super::commands::{self, AttachCandidate, CommandInfo};
use super::markdown;
use super::{App, Entry, Modal, SETTINGS, filter_matches};
use crate::domain::{PartStatus, SessionMode};

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
const SIDE_WIDTH: u16 = 34;
/// From this width the side panel sits beside the conversation.
pub const DOCK_FROM: u16 = 110;
/// Output lines a tool shows before "… +N lines".
const TOOL_LINES: usize = 3;
/// Maximum visible rows in the floating command palette.
const PALETTE_MAX_ITEMS: usize = 8;

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

enum PaletteKind {
    Commands(Vec<CommandInfo>),
    Attach(Vec<AttachCandidate>),
}

fn active_palette(app: &App) -> Option<PaletteKind> {
    if app.modal.is_some() || app.selected.is_some() || !app.input.starts_with('/') {
        return None;
    }
    if !app.input.contains(char::is_whitespace) {
        let found = commands::search_for(&app.input, Some(&app.session));
        return (!found.is_empty()).then_some(PaletteKind::Commands(found));
    }
    if !app.input.contains('\n')
        && app
            .input
            .split_whitespace()
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case("/attach"))
    {
        let found = commands::attach_matches(&app.input, &app.cwd);
        return (!found.is_empty()).then_some(PaletteKind::Attach(found));
    }
    None
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let docked = app.show_side && area.width >= DOCK_FROM;
    let permission = matches!(app.modal, Some(Modal::Permission { .. }));
    let text_width = area.width.saturating_sub(3).max(1);
    app.input_width = text_width;

    let (wrapped_input, cursor_row, cursor_col) =
        markdown::wrap_input(&app.input, app.cursor, text_width);
    let attach_rows = u16::from(!app.attachments.is_empty());
    let bottom = if permission {
        let max_perm = area.height.saturating_sub(4).max(5);
        (permission_lines(app, area.width).len() as u16)
            .clamp(5, max_perm)
    } else {
        (wrapped_input.len() as u16).clamp(1, 10) + attach_rows
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

    app.conv_area = cols[0];
    conversation(f, app, cols[0]);

    if docked {
        app.side_area = cols[1];
        side_panel(f, app, cols[1]);
    } else if app.side_overlay {
        let w = SIDE_WIDTH.min(area.width.saturating_sub(6));
        let rect = Rect {
            x: rows[0].x + rows[0].width.saturating_sub(w),
            y: rows[0].y,
            width: w,
            height: rows[0].height,
        };
        app.side_area = rect;
        f.render_widget(Clear, rect);
        side_panel(f, app, rect);
    } else {
        app.side_area = Rect::default();
        app.side_max_scroll = 0;
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
        input(f, app, rows[4], &wrapped_input, cursor_row, cursor_col);
    }

    let palette = if permission { None } else { active_palette(app) };
    footer(f, app, rows[5], palette.is_some());

    if let Some(kind) = palette {
        command_palette(f, app, area, rows[3].y, kind);
    }
    if app.modal.is_some() && !permission {
        modal(f, app, area);
    }
}

/// Floating `/` command or `/attach` path palette anchored above the rule.
fn command_palette(
    f: &mut Frame,
    app: &App,
    area: Rect,
    bottom_y: u16,
    kind: PaletteKind,
) {
    let avail_h = bottom_y.saturating_sub(area.y);
    if avail_h < 3 || area.width < 20 {
        return;
    }
    let max_items = (avail_h.saturating_sub(2) as usize).clamp(1, PALETTE_MAX_ITEMS);
    let w = area.width.saturating_sub(2).min(92);
    let inner_w = w.saturating_sub(4) as usize; // borders + "› "

    let (title, hint, list_items, total_len): (String, &str, Vec<ListItem<'static>>, usize) =
        match kind {
            PaletteKind::Commands(cmds) => {
                let total = cmds.len();
                let cmd_col = cmds
                    .iter()
                    .map(|c| c.usage().width())
                    .max()
                    .unwrap_or(10)
                    .clamp(10, (inner_w / 2).max(10));
                let items = cmds
                    .into_iter()
                    .map(|c| {
                        let usage = fit(&c.usage(), cmd_col);
                        let pad = " ".repeat(cmd_col.saturating_sub(usage.width()) + 2);
                        let mut desc = c.description.to_string();
                        if !c.aliases.is_empty() {
                            desc.push_str(&format!(" ({})", c.aliases.join(", ")));
                        }
                        let cat = format!(" [{}]", c.category.label());
                        let desc_room = inner_w.saturating_sub(cmd_col + 2 + cat.width());
                        let shown_desc = fit(&desc, desc_room);
                        let cat_gap = inner_w.saturating_sub(
                            usage.width() + pad.width() + shown_desc.width() + cat.width(),
                        );
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                usage,
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(pad),
                            Span::raw(shown_desc),
                            Span::raw(" ".repeat(cat_gap)),
                            Span::styled(cat, dim()),
                        ]))
                    })
                    .collect();
                (
                    format!(" Commands ({total}) "),
                    " ↑↓ select · Tab complete · Enter run ",
                    items,
                    total,
                )
            }
            PaletteKind::Attach(files) => {
                let total = files.len();
                let items = files
                    .into_iter()
                    .map(|c| {
                        let style = if c.arg == "clear" {
                            Style::default().fg(Color::Yellow)
                        } else if c.is_dir {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        let tag = if c.is_dir { "  dir" } else { "" };
                        let label = fit(&c.label, inner_w.saturating_sub(tag.width()));
                        ListItem::new(Line::from(vec![
                            Span::styled(label, style),
                            Span::styled(tag, dim()),
                        ]))
                    })
                    .collect();
                (
                    format!(" Attach file ({total}) "),
                    " ↑↓ select · Tab complete · Enter attach ",
                    items,
                    total,
                )
            }
        };

    if total_len == 0 {
        return;
    }
    let h = (total_len.min(max_items) as u16) + 2;
    let rect = Rect {
        x: area.x + 1,
        y: bottom_y.saturating_sub(h),
        width: w,
        height: h,
    };
    let sel = app.cmd_sel.min(total_len - 1);
    let mut state = ListState::default();
    state.select(Some(sel));

    let shown_hint = fit(hint, w.saturating_sub(2) as usize);
    let shown_title = fit(&title, w.saturating_sub(2) as usize);

    f.render_widget(Clear, rect);
    f.render_stateful_widget(
        List::new(list_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(Span::styled(
                        shown_title,
                        Style::default().add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(Span::styled(shown_hint, dim())),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("› "),
        rect,
        &mut state,
    );
}

/// Reading and searching, as opposed to changing files or running commands.
fn exploring(title: &str) -> bool {
    ["Read ", "View ", "List", "Grep", "Glob", "Find"]
        .iter()
        .any(|p| title.starts_with(p))
}

/// Runs of two or more reads and searches with nothing shown between them:
/// (first entry, all entries in the run).
fn tool_groups(app: &App) -> Vec<(usize, Vec<usize>)> {
    let is_exploring = |e: &Entry| matches!(e, Entry::Tool { title, .. } if exploring(title));
    let mut groups = Vec::new();
    let mut i = 0;
    while i < app.entries.len() {
        if !is_exploring(&app.entries[i]) {
            i += 1;
            continue;
        }
        let mut members = vec![i];
        let mut k = i + 1;
        while k < app.entries.len() {
            match &app.entries[k] {
                e if is_exploring(e) => members.push(k),
                Entry::Thought(_) if !app.show_thoughts => {}
                Entry::Text(t) if t.trim().is_empty() => {}
                _ => break,
            }
            k += 1;
        }
        if members.len() >= 2 {
            groups.push((i, members));
        }
        i = k;
    }
    groups
}

/// One line for a run of reads and searches: "Read 3 files, 2 searches".
fn group_line(app: &App, members: &[usize], width: u16) -> Vec<Line<'static>> {
    let mut files = std::collections::BTreeSet::new();
    let (mut searches, mut lists, mut failed) = (0, 0, 0);
    let mut running = None;
    for &i in members {
        let Entry::Tool { title, ok, .. } = &app.entries[i] else {
            continue;
        };
        if let Some(rest) = title
            .strip_prefix("Read ")
            .or_else(|| title.strip_prefix("View "))
        {
            files.insert(rest.split(" (").next().unwrap_or(rest).to_string());
        } else if title.starts_with("List") {
            lists += 1;
        } else {
            searches += 1;
        }
        match ok {
            None => running = Some(title.clone()),
            Some(false) => failed += 1,
            Some(true) => {}
        }
    }
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if !files.is_empty() {
        parts.push(format!("read {}", plural(files.len(), "file", "files")));
    }
    if searches > 0 {
        parts.push(plural(searches, "search", "searches"));
    }
    if lists > 0 {
        parts.push(format!("listed {}", plural(lists, "folder", "folders")));
    }
    let mut text = parts.join(", ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    let (mark, color) = if running.is_some() {
        ("●", Color::Cyan)
    } else {
        ("✓", Color::Green)
    };
    let mut spans = vec![
        Span::styled(format!("{mark} "), Style::default().fg(color)),
        Span::styled(text, dim()),
    ];
    if failed > 0 {
        spans.push(Span::styled(format!(" ({failed} failed)"), dim()));
    }
    if let Some(now) = running {
        spans.push(Span::styled(format!(" · {now}"), dim()));
    } else {
        spans.push(Span::styled(" · Tab→Enter expands · ^G all", dim()));
    }
    markdown::wrap_hanging(&spans, width, 2)
}

/// Lines of one item in the conversation: (entry index, line range).
pub type ItemSpan = (usize, std::ops::Range<usize>);

/// The conversation's lines at `width`, oldest first, and which lines each
/// selectable item takes.
pub fn transcript(app: &App, width: u16) -> (Vec<Line<'static>>, Vec<ItemSpan>) {
    let body = width.saturating_sub(2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut prev: Option<&Entry> = None;
    let mut items: Vec<ItemSpan> = Vec::new();
    // Reads and searches in a row collapse to one line; Ctrl+G shows each.
    let mut group_at = std::collections::HashMap::new();
    let mut folded = std::collections::HashSet::new();
    if !app.show_tools {
        for (first, members) in tool_groups(app) {
            if app.expanded.contains(&first) {
                continue;
            }
            folded.extend(members.iter().skip(1).copied());
            group_at.insert(first, members);
        }
    }
    for (index, entry) in app.entries.iter().enumerate() {
        if folded.contains(&index) {
            continue;
        }
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
        let item_start = lines.len();
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
            Entry::Tool { .. } if group_at.contains_key(&index) => {
                lines.extend(group_line(app, &group_at[&index], width));
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
                let shown = if out.len() <= TOOL_LINES + 1 || app.expanded.contains(&index) {
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
                    lines.extend(markdown::wrap_hanging(
                        &[Span::styled(
                            format!(
                                "    … +{} lines (Tab→Enter expands · y copies)",
                                out.len() - shown
                            ),
                            dim(),
                        )],
                        width,
                        4,
                    ));
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
            items.push((index, item_start..lines.len()));
        }
    }
    (lines, items)
}

fn conversation(f: &mut Frame, app: &mut App, area: Rect) {
    // A one-column margin on each side.
    let area = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    let (mut lines, items) = transcript(app, area.width);
    if app.entries.is_empty() {
        let hint = format!("Ask about what's in {}.", app.cwd.display());
        lines.extend(markdown::wrap(&[Span::styled(hint, dim())], area.width));
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
    let mut start = max_scroll - app.scroll;
    // The selected item, brought into view after a move.
    let selected = app.selected.and_then(|sel| {
        items
            .iter()
            .find(|(i, _)| *i == sel)
            .map(|(_, r)| r.clone())
    });
    if let Some(r) = &selected
        && app.reveal_selected
    {
        if r.end > start + height {
            start = r.end.saturating_sub(height);
        }
        if r.start < start {
            start = r.start;
        }
        start = start.min(max_scroll);
        app.scroll = max_scroll - start;
        app.reveal_selected = false;
    }
    app.items = items.iter().map(|(i, _)| *i).collect();
    let visible: Vec<Line> = lines.into_iter().skip(start).take(height).collect();
    f.render_widget(Paragraph::new(visible), area);
    // A bar in the margin beside the selected item.
    if let Some(r) = selected {
        let bar = Style::default().fg(Color::Cyan);
        for line in r.start.max(start)..r.end.min(start + height) {
            let y = area.y + (line - start) as u16;
            f.buffer_mut()[(area.x - 1, y)]
                .set_symbol("▌")
                .set_style(bar);
        }
    }
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

fn has_review_ready(app: &App) -> bool {
    app.session
        .map
        .parts
        .iter()
        .any(|p| p.status == PartStatus::InReview)
}

fn has_waiting_state(app: &App) -> bool {
    let s = &app.session;
    s.pending_change.is_some() || s.pending_candidate.is_some() || has_review_ready(app)
}

/// Where the thread stands, in one line: what waits, the part, the count.
fn tray_text(app: &App) -> String {
    let s = &app.session;
    let mut bits = Vec::new();
    // What waits on you comes first, so a narrow window still shows it.
    if s.pending_change.is_some() {
        bits.push("⧗ map change (/accept · /reject)".to_string());
    }
    if s.pending_candidate.is_some() {
        bits.push(
            if s.exam {
                "⧗ your answer"
            } else {
                "⧗ your verdict (right/flawed + why)"
            }
            .to_string(),
        );
    } else if has_review_ready(app) {
        bits.push("⧗ review ready (/approve · /object)".to_string());
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
    let waiting = has_waiting_state(app);
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

fn side_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let s = &app.session;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let yellow_bold = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let cyan = Style::default().fg(Color::Cyan);
    let width = area.width.saturating_sub(3).max(8);

    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled("Thread  ", dim()),
        Span::styled(mode_name(app).to_string(), bold),
    ])];
    if let Some(goal) = &s.goal {
        lines.extend(markdown::wrap(&[Span::styled(goal.clone(), dim())], width));
    }

    // Contextual waiting-state blocks with clear next-step commands & shortcuts.
    if let Some(change) = &s.pending_change {
        lines.push(Line::default());
        lines.extend(markdown::wrap(
            &[Span::styled("⧗ Plan change proposed", yellow_bold)],
            width,
        ));
        if !change.brief.is_empty() {
            lines.extend(markdown::wrap(
                &[Span::styled(change.brief.clone(), dim())],
                width,
            ));
        }
        lines.extend(markdown::wrap(
            &[
                Span::styled("/accept", cyan),
                Span::styled(" (Alt+A) · ", dim()),
                Span::styled("/reject", cyan),
                Span::styled(" (Alt+R)", dim()),
            ],
            width,
        ));
    }

    if s.pending_candidate.is_some() {
        lines.push(Line::default());
        if s.exam {
            lines.extend(markdown::wrap(
                &[Span::styled("⧗ Exam question waiting", yellow_bold)],
                width,
            ));
            lines.extend(markdown::wrap(
                &[Span::styled("Answer in your own words", dim())],
                width,
            ));
        } else {
            lines.extend(markdown::wrap(
                &[Span::styled("⧗ Candidate waiting", yellow_bold)],
                width,
            ));
            lines.extend(markdown::wrap(
                &[
                    Span::styled("Verdict + why ", dim()),
                    Span::styled("(Alt+Y / Alt+N)", cyan),
                ],
                width,
            ));
        }
    } else if has_review_ready(app) {
        lines.push(Line::default());
        lines.extend(markdown::wrap(
            &[Span::styled("⧗ Result ready for review", yellow_bold)],
            width,
        ));
        lines.extend(markdown::wrap(
            &[
                Span::styled("/approve", cyan),
                Span::styled(" (Alt+A) · ", dim()),
                Span::styled("/object <why>", cyan),
                Span::styled(" (Alt+O)", dim()),
            ],
            width,
        ));
    }

    if !s.map.parts.is_empty() {
        let done = s
            .map
            .parts
            .iter()
            .filter(|p| matches!(p.status, PartStatus::Understood | PartStatus::Done))
            .count();
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("Parts ", bold),
            Span::styled(format!("({done}/{})", s.map.parts.len()), dim()),
        ]));
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
            let mut spans = vec![
                Span::styled(format!("{mark} "), Style::default().fg(color)),
                Span::styled(
                    part.title.clone(),
                    if active { bold } else { Style::default() },
                ),
            ];
            if part.status == PartStatus::InReview {
                spans.push(Span::styled(" [review]", Style::default().fg(Color::Yellow)));
            }
            lines.extend(markdown::wrap_hanging(&spans, width, 2));
        }
    }

    let extra_actions: Vec<_> = commands::contextual_actions(s)
        .into_iter()
        .filter(|a| {
            !matches!(
                a.command,
                "/accept" | "/reject" | "/approve" | "/object " | ""
            )
        })
        .collect();
    if !extra_actions.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Next", bold)));
        for a in extra_actions {
            let spans = vec![
                Span::styled(a.label, cyan),
                Span::styled(format!("  {}", a.description), dim()),
            ];
            lines.extend(markdown::wrap_hanging(&spans, width, 2));
        }
    }

    if app.side_overlay {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Esc closes", dim())));
    }

    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(dim())
        .padding(ratatui::widgets::Padding::left(1));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let total = lines.len();
    let show_scroll_bar = total > inner.height as usize && inner.height >= 2;
    let content_area = if show_scroll_bar {
        Rect {
            height: inner.height - 1,
            ..inner
        }
    } else {
        inner
    };
    let max_scroll = total.saturating_sub(content_area.height as usize);
    app.side_max_scroll = max_scroll;
    app.side_scroll = app.side_scroll.min(max_scroll);

    f.render_widget(
        Paragraph::new(lines).scroll((app.side_scroll as u16, 0)),
        content_area,
    );

    if show_scroll_bar {
        let hint_area = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        let end_row = (app.side_scroll + content_area.height as usize).min(total);
        let hint = fit(
            &format!("↑↓ {}-{end_row}/{total}", app.side_scroll + 1),
            hint_area.width as usize,
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(hint, dim()))),
            hint_area,
        );
    }
}

/// What the empty input says: what Pathfinder waits for.
fn placeholder(app: &App) -> String {
    if app.running.is_some() {
        "Esc stops the reply".into()
    } else if app.session.pending_change.is_some() {
        "/accept or /reject the proposed plan change".into()
    } else if app.session.pending_candidate.is_some() && app.session.exam {
        "Your answer".into()
    } else if app.session.pending_candidate.is_some() {
        "Right or flawed, and why?".into()
    } else if has_review_ready(app) {
        "/approve or /object <reason> on the staged result".into()
    } else {
        "Ask, show your work, or type / for commands".into()
    }
}

fn input(
    f: &mut Frame,
    app: &App,
    area: Rect,
    wrapped: &[String],
    cursor_row: u16,
    cursor_col: u16,
) {
    let prompt = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    let attach_rows = u16::from(!app.attachments.is_empty());
    let text_width = area.width.saturating_sub(3).max(1);

    if !app.attachments.is_empty() {
        let names: Vec<String> = app
            .attachments
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        let attach_area = Rect {
            x: area.x + 2,
            y: area.y,
            width: text_width,
            height: 1,
        };
        let suffix = if app.input.is_empty() {
            " (Backspace removes)"
        } else {
            ""
        };
        let room = (text_width as usize).saturating_sub(suffix.width());
        let chip = fit(&format!("📎 {}", names.join(", ")), room);
        let mut spans = vec![Span::styled(chip, Style::default().fg(Color::Cyan))];
        if !suffix.is_empty() && room > 6 {
            spans.push(Span::styled(suffix, dim()));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), attach_area);
    }

    let text_area = Rect {
        x: area.x + 2,
        y: area.y + attach_rows,
        width: text_width,
        height: area.height.saturating_sub(attach_rows).max(1),
    };

    f.render_widget(
        Paragraph::new(Line::from(Span::styled("›", prompt))),
        Rect {
            x: area.x,
            y: text_area.y,
            width: 1,
            height: 1,
        },
    );

    if app.input.is_empty() {
        let ph = fit(&placeholder(app), text_width as usize);
        f.render_widget(Paragraph::new(Line::from(Span::styled(ph, dim()))), text_area);
        if app.modal.is_none() && app.selected.is_none() {
            f.set_cursor_position((text_area.x, text_area.y));
        }
        return;
    }

    let lines: Vec<Line<'static>> = wrapped.iter().map(|l| Line::from(l.clone())).collect();
    let skip = cursor_row.saturating_sub(text_area.height.saturating_sub(1));
    f.render_widget(Paragraph::new(lines).scroll((skip, 0)), text_area);

    if app.modal.is_none() && app.selected.is_none() {
        let cx = (text_area.x + cursor_col).min(area.x + area.width.saturating_sub(1));
        let cy = (text_area.y + cursor_row - skip).min(area.y + area.height.saturating_sub(1));
        f.set_cursor_position((cx, cy));
    }
}

/// The permission question, drawn where the input was.
fn permission_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(Modal::Permission {
        title,
        always_key,
        sel,
        ..
    }) = &app.modal
    else {
        return Vec::new();
    };
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let rev = Style::default().add_modifier(Modifier::REVERSED);
    let prog = always_key.rsplit(':').next().unwrap_or(always_key);

    let mut lines = vec![Line::from(Span::styled(
        "Allow the agent to run this?",
        bold.fg(Color::Yellow),
    ))];
    lines.extend(markdown::wrap_hanging(
        &[Span::styled(format!("  {title}"), bold)],
        width.saturating_sub(1),
        4,
    ));

    let choices = [
        ("1 / y", "Yes, once".to_string()),
        (
            "2 / a",
            format!("Yes, always in this thread for `{prog}`"),
        ),
        ("3 / n / Esc", "No".to_string()),
    ];
    for (idx, (k, label)) in choices.iter().enumerate() {
        let chosen = *sel == Some(idx);
        let prefix = if chosen { "▶ " } else { "  " };
        let line_spans = if chosen {
            vec![
                Span::styled(format!("{prefix}{k}  {label}"), rev),
            ]
        } else {
            vec![
                Span::raw(prefix),
                Span::styled(format!("{k}  "), key),
                Span::raw(label.clone()),
            ]
        };
        lines.extend(markdown::wrap_hanging(
            &line_spans,
            width.saturating_sub(1),
            4,
        ));
    }

    let hint = if sel.is_some() {
        "  Enter confirms · ↑↓ choose · 1/y 2/a 3/n/Esc · PgUp/PgDn scroll"
    } else {
        "  ↑↓ choose · 1/y once · 2/a always · 3/n/Esc no · PgUp/PgDn scroll"
    };
    lines.extend(markdown::wrap_hanging(
        &[Span::styled(hint, dim())],
        width.saturating_sub(1),
        2,
    ));
    lines
}

/// Key hints on the left, mouse mode + models on the right; both shrink to fit.
fn footer(f: &mut Frame, app: &App, area: Rect, palette_open: bool) {
    let width = (area.width as usize).saturating_sub(1);
    let decisions = if app.current(crate::options::DECISIONS) == "on" {
        app.current(crate::options::DECISION_MODEL)
    } else {
        "off".into()
    };
    let (s1, s2, checks) = (
        app.current(crate::options::FAST_MODEL),
        app.current(crate::options::MAIN_MODEL),
        app.current(crate::options::CHECKS),
    );
    let mouse_badge = if app.mouse_capture {
        "F2 select"
    } else {
        "SELECT MODE · F2 mouse"
    };

    // Longest first: the model summary shrinks before the mouse badge / hints go.
    let rights: Vec<(String, Style)> = vec![
        (
            format!("{s1} · {s2} · decisions {decisions} · checks {checks} · {mouse_badge}"),
            dim(),
        ),
        (format!("{s1} · {s2} · checks {checks} · {mouse_badge}"), dim()),
        (format!("{s1} · {s2} · {mouse_badge}"), dim()),
        (format!("{s2} · {mouse_badge}"), dim()),
        (
            mouse_badge.to_string(),
            if app.mouse_capture {
                dim()
            } else {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            },
        ),
        (String::new(), dim()),
    ];

    let (left, left_style) = if !app.status.is_empty() {
        (app.status.clone(), Style::default().fg(Color::Yellow))
    } else if palette_open {
        (
            "↑↓ palette · Tab complete · Enter run".to_string(),
            dim(),
        )
    } else {
        let hints: &[&str] = if app.selected.is_some() && app.modal.is_none() {
            &[
                "↑↓ move · Enter expand · y/^C copy · g/G first/last · Esc back",
                "↑↓ · Enter · y copy · Esc",
            ]
        } else if matches!(app.modal, Some(Modal::Permission { .. })) {
            &[
                "1/y once · 2/a always · 3/n/Esc no · PgUp/PgDn scroll",
                "y · a · n",
            ]
        } else if matches!(app.modal, Some(Modal::Share { .. })) {
            &["Paths are shortened to ./ and ~; nothing else is changed"]
        } else if matches!(app.modal, Some(Modal::Consent { .. })) {
            &["Your answer is saved; `pathfinder data` changes it later"]
        } else if app.modal.is_some() {
            &["Enter picks · Esc closes"]
        } else {
            &[
                "Enter send · Shift/Alt+Enter line · ^P settings · ^O threads · F1 keys",
                "^P settings · ^O threads · F1 keys",
                "F1 keys",
            ]
        };
        let pick = hints
            .iter()
            .find(|h| h.width() + 2 + rights[2].0.width() <= width)
            .or(hints.last())
            .unwrap();
        (pick.to_string(), dim())
    };

    let reserve_min = if !app.mouse_capture {
        mouse_badge.width() + 2
    } else {
        0
    };
    let left = fit(&left, width.saturating_sub(reserve_min));
    let room = width.saturating_sub(left.width() + 2);
    let (right, right_style) = rights
        .iter()
        .find(|(m, _)| m.width() <= room)
        .cloned()
        .unwrap_or_else(|| (String::new(), dim()));
    let gap = width.saturating_sub(left.width() + right.width());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(left, left_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, right_style),
        ])),
        area,
    );
}

/// `text` cut to `width` columns, with an ellipsis when it was cut.
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
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
    let w = width.min(area.width.saturating_sub(4)).max(1);
    let h = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

fn list(f: &mut Frame, area: Rect, title: &str, items: Vec<ListItem>, sel: usize, hint: &str) {
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(sel.min(items.len() - 1)));
    }
    let inner_w = area.width.saturating_sub(2) as usize;
    f.render_widget(Clear, area);
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(dim())
                    .title(Span::styled(
                        fit(&format!(" {title} "), inner_w),
                        Style::default().add_modifier(Modifier::BOLD),
                    ))
                    .title_bottom(Span::styled(fit(&format!(" {hint} "), inner_w), dim())),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("› "),
        area,
        &mut state,
    );
}

fn filterable_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: &[(String, String)],
    query: &str,
    sel: usize,
    hint: &str,
) {
    let matched = filter_matches(items, query);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let inner_w = area.width.saturating_sub(2) as usize;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(dim())
        .title(Span::styled(fit(&format!(" {title} "), inner_w), bold))
        .title_bottom(Span::styled(fit(&format!(" {hint} "), inner_w), dim()));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    if inner.height < 2 {
        return;
    }
    let filter_area = Rect {
        height: 1,
        ..inner
    };
    let list_area = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };

    let count = if query.is_empty() {
        format!(" ({})", items.len())
    } else {
        format!(" ({}/{})", matched.len(), items.len())
    };
    let filter_spans = if query.is_empty() {
        vec![
            Span::styled(" Filter: ", dim()),
            Span::styled(
                fit(&format!("type to search…{count}"), inner_w.saturating_sub(9)),
                dim(),
            ),
        ]
    } else {
        let room = inner_w.saturating_sub(10 + count.width());
        vec![
            Span::styled(" Filter: ", bold),
            Span::styled(fit(query, room), Style::default().fg(Color::Cyan)),
            Span::styled("▏", dim()),
            Span::styled(count, dim()),
        ]
    };
    f.render_widget(Paragraph::new(Line::from(filter_spans)), filter_area);

    if matched.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled("  No matches.", dim()))),
            list_area,
        );
        return;
    }

    let rows: Vec<ListItem> = matched
        .iter()
        .map(|&idx| ListItem::new(items[idx].1.clone()))
        .collect();
    let mut state = ListState::default();
    state.select(Some(sel.min(rows.len() - 1)));
    f.render_stateful_widget(
        List::new(rows)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("› "),
        list_area,
        &mut state,
    );
}

fn modal(f: &mut Frame, app: &mut App, area: Rect) {
    let mut next_help_max = app.help_max_scroll;
    let mut next_consent_max = app.consent_max_scroll;

    match app.modal.as_ref().unwrap() {
        Modal::Threads { items, query, sel } => {
            let matched_len = filter_matches(items, query).len().max(1);
            let rect = centered(area, 80, (matched_len as u16 + 4).max(6));
            filterable_list(
                f,
                rect,
                "Threads in this folder",
                items,
                query,
                *sel,
                "↑↓ pick · type filters · Enter open · Esc close",
            );
        }
        Modal::Settings { sel } => {
            let rect = centered(area, 64, SETTINGS.len() as u16 + 2);
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
                "Enter changes · s/^S save defaults · Esc closes",
            );
        }
        Modal::Choose {
            title,
            items,
            query,
            sel,
            ..
        } => {
            let matched_len = filter_matches(items, query).len().max(1);
            let rect = centered(area, 72, (matched_len as u16 + 4).max(6));
            filterable_list(
                f,
                rect,
                title,
                items,
                query,
                *sel,
                "↑↓ pick · type filters · Enter thread · ^S default · Esc back",
            );
        }
        Modal::Permission { .. } => {}
        Modal::Share {
            outgoing,
            lines,
            scroll,
            busy,
        } => {
            let rect = centered(area, 100, area.height.saturating_sub(4));
            f.render_widget(Clear, rect);
            let body: Vec<Line> = lines
                .iter()
                .skip(*scroll)
                .map(|l| Line::from(l.clone()))
                .collect();
            let hint = if *busy {
                " Sending… (Esc/^C cancels) ".to_string()
            } else {
                format!(
                    " ↑↓/Space scroll ({}/{}) · Enter sends exactly this · Esc cancels ",
                    (*scroll + 1).min(lines.len().max(1)),
                    lines.len().max(1)
                )
            };
            let inner_w = rect.width.saturating_sub(2) as usize;
            f.render_widget(
                Paragraph::new(body).wrap(Wrap { trim: false }).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(dim())
                        .title(Span::styled(
                            fit(&outgoing.title(), inner_w),
                            Style::default().add_modifier(Modifier::BOLD),
                        ))
                        .title_bottom(Span::styled(fit(&hint, inner_w), dim())),
                ),
                rect,
            );
        }
        Modal::Consent {
            events,
            threads,
            step,
            error,
            busy,
            scroll,
        } => {
            next_consent_max = consent(
                f,
                area,
                *events,
                *threads,
                step,
                error.as_deref(),
                *busy,
                *scroll,
            );
        }
        Modal::Help { scroll } => {
            next_help_max = help_modal(f, area, *scroll);
        }
    }

    app.help_max_scroll = next_help_max;
    app.consent_max_scroll = next_consent_max;
}

fn help_modal(f: &mut Frame, area: Rect, scroll: usize) -> usize {
    let keys = [
        ("Enter", "send"),
        ("Shift+Enter, Alt+Enter, Ctrl+J", "new line"),
        ("Esc, Ctrl+C", "stop the reply"),
        ("PageUp / PageDown, Shift+↑↓, wheel", "scroll conversation"),
        (
            "↑ / ↓",
            "command palette / multiline cursor / input history",
        ),
        (
            "Ctrl+A / Ctrl+E, Home / End",
            "start / end of line (End empty or Ctrl+End: bottom)",
        ),
        ("Ctrl+Home", "start of input"),
        ("Ctrl+U / Ctrl+K", "delete to start / end of line"),
        (
            "Ctrl+W, Alt+Backspace / Alt+D",
            "delete previous / next word",
        ),
        ("Ctrl+← →, Alt+← →, Alt+B/F", "move by a word"),
        (
            "Tab",
            "complete /command or /attach; when empty, select messages",
        ),
        ("Ctrl+↑", "select messages anytime"),
        ("Ctrl+N / Ctrl+O / Ctrl+P", "new thread / earlier threads / settings"),
        (
            "Ctrl+T / Ctrl+G / Ctrl+B",
            "thinking / every tool call / plan panel",
        ),
        ("F2", "toggle mouse scroll vs terminal text selection"),
        (
            "Alt+A / Alt+R / Alt+O",
            "accept or approve / reject / object (when waiting)",
        ),
        ("Alt+Y / Alt+N", "prefill candidate verdict (when waiting)"),
        (
            "Ctrl+D (empty), Ctrl+C (idle)",
            "quit; the conversation stays in your terminal",
        ),
    ];

    let modal_w = 82u16.min(area.width.saturating_sub(4)).max(20);
    let inner_w = modal_w.saturating_sub(2).max(12);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let cyan_bold = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let push_row = |lines: &mut Vec<Line<'static>>, k: String, v: &str| {
        if inner_w >= 54 {
            let key_col = 28usize.min(inner_w as usize / 2);
            if k.width() < key_col {
                let pad = " ".repeat(key_col - k.width());
                lines.extend(markdown::wrap_hanging(
                    &[
                        Span::styled(k, bold),
                        Span::raw(pad),
                        Span::raw(v.to_string()),
                    ],
                    inner_w,
                    key_col,
                ));
            } else {
                lines.extend(markdown::wrap(&[Span::styled(k, bold)], inner_w));
                lines.extend(markdown::wrap_hanging(
                    &[Span::raw(format!("  {v}"))],
                    inner_w,
                    2,
                ));
            }
        } else {
            lines.extend(markdown::wrap(&[Span::styled(k, bold)], inner_w));
            lines.extend(markdown::wrap_hanging(
                &[Span::raw(format!("  {v}"))],
                inner_w,
                2,
            ));
        }
    };

    for (k, v) in keys {
        push_row(&mut lines, k.to_string(), v);
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Commands", cyan_bold)));
    for (_, name, aliases, what) in commands::LOCAL {
        let shown = std::iter::once(name)
            .chain(aliases.iter().copied())
            .collect::<Vec<_>>()
            .join(", ");
        push_row(&mut lines, shown, what);
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("In a task", cyan_bold)));
    for (name, what) in commands::TASK {
        push_row(&mut lines, name.to_string(), what);
    }

    let height = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(4);
    let visible = height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = scroll.min(max_scroll);
    let hint = if max_scroll > 0 {
        format!(
            " ↑↓/PgUp/PgDn scroll ({}/{}) · Esc/any key closes ",
            scroll + 1,
            max_scroll + 1
        )
    } else {
        " Any key closes this ".to_string()
    };
    let rect = centered(area, modal_w, height);
    let title_w = rect.width.saturating_sub(2) as usize;
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).scroll((scroll as u16, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(dim())
                .title(Span::styled(fit(" Keys and commands ", title_w), bold))
                .title_bottom(Span::styled(fit(&hint, title_w), dim())),
        ),
        rect,
    );
    max_scroll
}

/// The one-time question about usage data. Returns `max_scroll` for `App`.
#[allow(clippy::too_many_arguments)]
fn consent(
    f: &mut Frame,
    area: Rect,
    events: bool,
    threads: bool,
    step: &super::ConsentStep,
    error: Option<&str>,
    busy: bool,
    scroll: usize,
) -> usize {
    use super::ConsentStep;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let tick = |on: bool| if on { "[x]" } else { "[ ]" };
    let name = crate::operator::NAME;

    let modal_w = 82u16.min(area.width.saturating_sub(4)).max(24);
    let inner_w = modal_w.saturating_sub(2).max(16);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let push_wrapped = |lines: &mut Vec<Line<'static>>, spans: &[Span<'static>], hang: usize| {
        lines.extend(markdown::wrap_hanging(spans, inner_w, hang));
    };

    push_wrapped(
        &mut lines,
        &[Span::styled("Join the beta (invite-only for now)", bold)],
        0,
    );
    push_wrapped(
        &mut lines,
        &[Span::raw(format!(
            "Joining links this computer to {name}'s server with your email. That turns on the shared gate: the check that keeps a candidate's answer from slipping out while you judge it. Without joining, Pathfinder still runs on your own model account, and the gate needs a decision key of your own (`pathfinder auth set typesafe`)."
        ))],
        0,
    );
    lines.push(Line::default());

    push_wrapped(
        &mut lines,
        &[Span::styled(
            "Share data to help improve it (each is optional, and separate from joining)",
            bold,
        )],
        0,
    );
    push_wrapped(
        &mut lines,
        &[Span::styled(
            format!("{} e  Usage events", tick(events)),
            bold,
        )],
        7,
    );
    push_wrapped(
        &mut lines,
        &[Span::raw(
            "       Counts and timings: which steps you use, how long replies take, whether a reply was held back, checks as right or wrong, crashes. Never your messages, the answers, file or folder names, or your work.",
        )],
        7,
    );
    push_wrapped(
        &mut lines,
        &[Span::styled(
            format!("{} t  Threads you choose to share", tick(threads)),
            bold,
        )],
        7,
    );
    push_wrapped(
        &mut lines,
        &[Span::raw(
            "       Only when you type /share in a thread, after seeing exactly what goes.",
        )],
        7,
    );
    lines.push(Line::default());

    push_wrapped(
        &mut lines,
        &[Span::raw(format!(
            "Kept on {name}'s server in Germany for up to 12 months, with your email. Bug reports (/bug) go only when you send one. `pathfinder data off` stops sharing; `pathfinder data delete` erases everything sent."
        ))],
        0,
    );
    push_wrapped(
        &mut lines,
        &[Span::styled(
            format!("Full notice: {}", crate::telemetry::consent::notice_url()),
            dim(),
        )],
        0,
    );
    lines.push(Line::default());

    let hint = match step {
        ConsentStep::Choose => {
            "e, t toggle · Enter: join with email · i: invite code · Esc: not now"
        }
        ConsentStep::Browser(None) => {
            push_wrapped(&mut lines, &[Span::styled("Starting the sign-in…", dim())], 0);
            "Esc: not now · i: invite code instead"
        }
        ConsentStep::Browser(Some(sign_in)) => {
            push_wrapped(
                &mut lines,
                &[Span::raw(
                    "Sign in with your email in the browser that just opened:",
                )],
                0,
            );
            push_wrapped(
                &mut lines,
                &[Span::styled(
                    format!("  {}", sign_in.url),
                    Style::default().fg(Color::Cyan),
                )],
                2,
            );
            lines.push(Line::default());
            push_wrapped(
                &mut lines,
                &[
                    Span::raw("Check that the page shows "),
                    Span::styled(sign_in.code.clone(), bold),
                    Span::raw(", then press Link this computer."),
                ],
                0,
            );
            push_wrapped(&mut lines, &[Span::styled("Waiting for you there…", dim())], 0);
            lines.push(Line::default());
            "o: open page again · i: invite code · Esc: not now"
        }
        ConsentStep::Invite(code) => {
            push_wrapped(
                &mut lines,
                &[
                    Span::styled("Invite code: ", bold),
                    Span::raw(code.to_string()),
                    Span::styled(if busy { "  checking…" } else { "▏" }, dim()),
                ],
                0,
            );
            "Enter joins · Esc skips (join later: pathfinder data join)"
        }
    };
    if let Some(e) = error {
        push_wrapped(
            &mut lines,
            &[Span::styled(
                e.to_string(),
                Style::default().fg(Color::Red),
            )],
            0,
        );
    }

    let height = (lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2))
        .max(4);
    let visible = height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = scroll.min(max_scroll);

    let full_hint = if max_scroll > 0 {
        format!(" ↑↓ scroll · {hint} ")
    } else {
        format!(" {hint} ")
    };

    let rect = centered(area, modal_w, height);
    let title_w = rect.width.saturating_sub(2) as usize;
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).scroll((scroll as u16, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(dim())
                .title(Span::styled(fit(" Welcome to Pathfinder ", title_w), bold))
                .title_bottom(Span::styled(fit(&full_hint, title_w), dim())),
        ),
        rect,
    );
    max_scroll
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tokio::sync::mpsc;

    use crate::domain::{Map, MapChange, Part, PendingCandidate};
    use crate::engine::{BoxFuture, Engine, EngineRequest, StreamSink};
    use crate::runtime::Runtime;
    use crate::store::Store;

    struct DummyEngine;

    impl Engine for DummyEngine {
        fn run_text<'a>(
            &'a self,
            _req: &'a EngineRequest,
            _sink: StreamSink,
        ) -> BoxFuture<'a, anyhow::Result<String>> {
            Box::pin(async { unreachable!() })
        }
    }

    fn test_app() -> App {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let rt = Arc::new(Runtime::new(store, Arc::new(DummyEngine)));
        let cwd = PathBuf::from("/tmp/pathfinder-view-test");
        let session = rt.create_session("test-sess".into(), cwd.clone()).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        App::new(rt, cwd, session, tx)
    }

    fn render_to_string(app: &mut App, w: u16, h: u16) -> (String, (u16, u16)) {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let pos = terminal.get_cursor_position().unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        (out, (pos.x, pos.y))
    }

    #[test]
    fn command_palette_renders_even_when_status_is_set() {
        let mut app = test_app();
        app.status = "Ctrl+O opens an earlier thread in this folder.".into();
        app.input = "/re".into();
        app.cursor = app.input.len();
        app.cmd_sel = 1;

        let (screen, _) = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("Commands ("), "{screen}");
        assert!(screen.contains("/resume"), "{screen}");
        assert!(screen.contains("/replan"), "{screen}");
        assert!(screen.contains("/reject"), "{screen}");
        assert!(screen.contains("Ctrl+O opens"), "{screen}");
    }

    #[test]
    fn wrapped_input_cursor_matches_wrap_input() {
        let mut app = test_app();
        app.input = "first line wraps onto second line cleanly".into();
        app.cursor = app.input.find("second").unwrap();

        let (screen, cursor) = render_to_string(&mut app, 30, 20);
        let (lines, row, col) = markdown::wrap_input(&app.input, app.cursor, app.input_width);
        assert!(lines.len() >= 2, "{lines:?}");
        // Footer is row 19; input spans rows (19 - lines.len())..19.
        let expected_y = 19 - (lines.len() as u16) + row;
        assert_eq!(cursor, (2 + col, expected_y));
        let cursor_line = screen.lines().nth(cursor.1 as usize).unwrap();
        assert!(cursor_line.contains("second"), "{cursor_line:?}");
    }

    #[test]
    fn attachment_chip_shows_backspace_hint_when_input_empty() {
        let mut app = test_app();
        app.attachments.push(PathBuf::from("/tmp/notes.pdf"));
        let (screen, _) = render_to_string(&mut app, 80, 20);
        assert!(screen.contains("📎"), "{screen}");
        assert!(screen.contains("notes.pdf"), "{screen}");
        assert!(screen.contains("(Backspace removes)"), "{screen}");
    }

    #[test]
    fn side_panel_scrolls_and_shows_contextual_waiting_actions() {
        let mut app = test_app();
        app.session.mode = SessionMode::Working;
        app.session.goal = Some("Understand every part of the solver".into());
        app.session.pending_change = Some(MapChange {
            brief: "Split step 2 into two parts".into(),
            ..Default::default()
        });
        app.session.pending_candidate = Some(PendingCandidate {
            part_id: "p1".into(),
            candidate: "x = 2".into(),
            key_correct: false,
            key_reason: String::new(),
            secret: None,
            targets: vec![],
            misconception: None,
            id: "c1".into(),
        });
        app.session.map = Map {
            parts: (1..=15)
                .map(|i| Part::new(format!("p{i}"), format!("Part {i} title"), String::new(), vec![]))
                .collect(),
        };
        app.session.active = Some("p1".into());
        app.side_scroll = 2;

        let (screen, _) = render_to_string(&mut app, 120, 20);
        assert!(app.side_max_scroll > 0);
        assert!(screen.contains("↑↓"), "{screen}");

        app.side_scroll = 0;
        let (top_screen, _) = render_to_string(&mut app, 120, 20);
        assert!(top_screen.contains("Plan change proposed"), "{top_screen}");
        assert!(top_screen.contains("/accept"), "{top_screen}");
        assert!(top_screen.contains("/reject"), "{top_screen}");
        assert!(top_screen.contains("Candidate waiting"), "{top_screen}");
    }

    #[test]
    fn consent_modal_scrolls_on_80x24_and_help_wraps_under_80_cols() {
        let mut app = test_app();
        app.modal = Some(Modal::Consent {
            events: true,
            threads: false,
            step: super::super::ConsentStep::Invite("BETA-42".into()),
            error: Some("Invalid invite code".into()),
            busy: false,
            scroll: 0,
        });
        let (_, _) = render_to_string(&mut app, 80, 24);
        assert!(app.consent_max_scroll > 0);

        if let Some(Modal::Consent { scroll, .. }) = &mut app.modal {
            *scroll = app.consent_max_scroll;
        }
        let (bottom_consent, _) = render_to_string(&mut app, 80, 24);
        assert!(bottom_consent.contains("BETA-42"), "{bottom_consent}");
        assert!(
            bottom_consent.contains("Invalid invite code"),
            "{bottom_consent}"
        );

        app.modal = Some(Modal::Help { scroll: 0 });
        let (help_narrow, _) = render_to_string(&mut app, 60, 24);
        assert!(help_narrow.contains("Keys and commands"), "{help_narrow}");
        assert!(help_narrow.contains("Shift+Enter"), "{help_narrow}");
        assert!(app.help_max_scroll > 0);

        if let Some(Modal::Help { scroll }) = &mut app.modal {
            *scroll = 12.min(app.help_max_scroll);
        }
        let (help_scrolled, _) = render_to_string(&mut app, 60, 24);
        assert!(help_scrolled.contains("Ctrl+G"), "{help_scrolled}");
        assert!(help_scrolled.contains("F2"), "{help_scrolled}");
    }

    #[test]
    fn filter_bars_permission_and_mouse_footer_render_cleanly() {
        let mut app = test_app();
        app.mouse_capture = false;
        app.modal = Some(Modal::Choose {
            option: crate::options::MAIN_MODEL,
            title: "Main model".into(),
            items: vec![
                ("sonnet".into(), "Claude Sonnet".into()),
                ("opus".into(), "Claude Opus".into()),
            ],
            query: "opus".into(),
            sel: 0,
        });
        let (choose_screen, _) = render_to_string(&mut app, 100, 24);
        assert!(choose_screen.contains("Filter: opus"), "{choose_screen}");
        assert!(choose_screen.contains("(1/2)"), "{choose_screen}");
        assert!(choose_screen.contains("SELECT MODE"), "{choose_screen}");

        app.modal = Some(Modal::Permission {
            title: "Bash rm -rf build".into(),
            always_key: "bash:rm".into(),
            sel: None,
            reply: None,
        });
        let (perm_screen, _) = render_to_string(&mut app, 90, 24);
        assert!(
            perm_screen.contains("Allow the agent to run this?"),
            "{perm_screen}"
        );
        assert!(perm_screen.contains("PgUp/PgDn scroll"), "{perm_screen}");
        assert!(
            !perm_screen.contains("Enter confirms"),
            "no Enter confirm hint until option selected: {perm_screen}"
        );
    }

    #[test]
    fn attach_palette_review_state_and_tool_hints_render() {
        let dir = std::env::temp_dir().join(format!("pf-view-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("paper.pdf"), b"%PDF").unwrap();

        let mut app = test_app();
        app.cwd = dir.clone();
        app.input = "/attach pa".into();
        app.cursor = app.input.len();
        let (attach_screen, _) = render_to_string(&mut app, 90, 24);
        assert!(attach_screen.contains("Attach file ("), "{attach_screen}");
        assert!(attach_screen.contains("paper.pdf"), "{attach_screen}");

        app.input.clear();
        app.cursor = 0;
        app.session.mode = SessionMode::Working;
        app.session.map = Map {
            parts: vec![Part {
                status: PartStatus::InReview,
                ..Part::new("p1".into(), "Verify proof".into(), String::new(), vec![])
            }],
        };
        app.session.active = Some("p1".into());
        app.entries.push(Entry::Tool {
            id: "t1".into(),
            title: "Read a.rs".into(),
            ok: Some(true),
            detail: "l1\nl2\nl3\nl4\nl5".into(),
        });
        app.entries.push(Entry::Tool {
            id: "t2".into(),
            title: "Read b.rs".into(),
            ok: Some(true),
            detail: String::new(),
        });
        let (review_screen, _) = render_to_string(&mut app, 120, 24);
        assert!(
            review_screen.contains("Result ready for review"),
            "{review_screen}"
        );
        assert!(review_screen.contains("/approve"), "{review_screen}");
        assert!(review_screen.contains("/object"), "{review_screen}");
        assert!(
            review_screen.contains("Tab→Enter expands"),
            "{review_screen}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
