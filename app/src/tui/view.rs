//! Drawing. Borderless, the way Codex and Claude Code draw: the
//! conversation on top (with the side panel beside it when it has something
//! to show), a rule that carries the status, the input, and a footer. The
//! rows never change with what runs. Permission prompts and the sign-in
//! panel take the input's place; other dialogs float over the conversation.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::commands::{self, AttachCandidate, CommandInfo, Group};
use super::markdown;
use super::{App, Entry, Modal, SETTINGS, display_rows, filter_matches, is_header};
use crate::domain::{PartStatus, SessionMode};

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
/// From this width the side panel sits beside the conversation.
pub const DOCK_FROM: u16 = 110;
/// Output lines a tool shows before "… +N lines".
const TOOL_LINES: usize = 3;
/// Most rows the palette under the input takes, its counter included.
const PALETTE_ROWS: usize = 9;

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

enum PaletteKind {
    /// Each command with the section it is listed under (bare `/` only).
    Commands(Vec<(Option<Group>, CommandInfo)>),
    /// The path menu of `/open`, `/check` or `/attach`.
    Path(&'static str, Vec<AttachCandidate>),
}

fn active_palette(app: &App) -> Option<PaletteKind> {
    if app.modal.is_some() || app.selected.is_some() || !app.input.starts_with('/') {
        return None;
    }
    if app.palette_open() {
        let found = commands::palette(&app.input, Some(&app.session));
        return Some(PaletteKind::Commands(found));
    }
    let (cmd, _) = commands::path_arg_slice(&app.input)?;
    let found = app.path_menu();
    (!found.is_empty()).then_some(PaletteKind::Path(cmd, found))
}

/// The side panel's width when docked: a fifth of the window, 34 to 48
/// columns.
pub fn side_width(width: u16) -> u16 {
    (width / 5).clamp(34, 48)
}

/// The panel has something to show: a goal, parts, something waiting on
/// you, or more than two next steps. Otherwise it doesn't dock.
pub fn panel_earns(app: &App) -> bool {
    let s = &app.session;
    s.goal.is_some()
        || !s.map.parts.is_empty()
        || has_waiting_state(app)
        || next_actions(app).len() > 2
}

/// The panel sits beside the conversation in a window this wide.
/// Never in terminal-selection mode (F2): a drag then copies only the
/// conversation.
pub fn docks(app: &App, width: u16) -> bool {
    app.mouse_capture && app.show_side && width >= DOCK_FROM && panel_earns(app)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    app.screen_width = area.width;
    let docked = docks(app, area.width);
    if docked {
        app.side_overlay = false;
    }
    let permission = matches!(app.modal, Some(Modal::Permission { .. }));
    let text_width = area.width.saturating_sub(3).max(1);
    app.input_width = text_width;

    let (wrapped_input, cursor_row, cursor_col) =
        markdown::wrap_input(&app.input, app.cursor, text_width);
    let attach_rows = u16::from(!app.attachments.is_empty());
    // The sign-in panel: a blank row, then its lines, a gutter each side.
    // A list in it takes about half the screen at most, so the
    // conversation stays in view above it.
    let panel_width = area.width.saturating_sub(2).max(1);
    let panel_rows = (area.height / 2 + 2)
        .max(10)
        .min(area.height.saturating_sub(7).max(6));
    let login = login_panel(app, panel_width, panel_rows);
    let bottom = if let Some((lines, _)) = &login {
        (lines.len() as u16 + 1).min(area.height.saturating_sub(3).max(1))
    } else if permission {
        let max_perm = area.height.saturating_sub(4).max(5);
        (permission_lines(app, area.width).len() as u16).clamp(5, max_perm)
    } else {
        (wrapped_input.len() as u16).clamp(1, 10) + attach_rows
    };

    // The palette or path menu, under the input as the sign-in panel's
    // lists are; the conversation gives up the rows while it is open.
    let palette = if permission || login.is_some() {
        None
    } else {
        active_palette(app)
    };
    let menu_room = (area.height as usize)
        .saturating_sub(bottom as usize + 3 + 4)
        .min(PALETTE_ROWS);
    let menu = match &palette {
        Some(kind) if menu_room >= 3 => {
            palette_lines(app, kind, area.width.saturating_sub(1) as usize, menu_room)
        }
        _ => Vec::new(),
    };

    // The conversation, one rule (which carries the status), the input
    // between it and a hairline, the menu when open and the footer: the
    // same rows whatever runs or waits.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(bottom),
            Constraint::Length(1),
            Constraint::Length(menu.len() as u16),
            Constraint::Length(1),
        ])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(if docked {
            vec![
                Constraint::Min(40),
                Constraint::Length(side_width(area.width)),
            ]
        } else {
            vec![Constraint::Min(10)]
        })
        .split(rows[0]);

    app.conv_area = cols[0];
    app.menu_rows = menu.len() as u16;
    conversation(f, app, cols[0]);

    if docked {
        app.side_area = cols[1];
        side_panel(f, app, cols[1]);
    } else if app.side_overlay && app.mouse_capture {
        // Framed over the conversation's right side, a blank column to its
        // left so the text under it doesn't run into the border.
        let w = 36.min(rows[0].width.saturating_sub(4));
        let rect = Rect {
            x: rows[0].x + rows[0].width.saturating_sub(w),
            y: rows[0].y,
            width: w,
            height: rows[0].height,
        };
        let gap = Rect {
            x: rect.x.saturating_sub(1),
            width: rect.width + 1,
            ..rect
        };
        f.render_widget(Clear, gap);
        app.side_area = rect;
        side_panel(f, app, rect);
    } else {
        app.side_area = Rect::default();
        app.side_max_scroll = 0;
    }

    f.render_widget(
        Paragraph::new(rule_line(app, rows[1].width, docked)),
        rows[1],
    );
    if let Some((lines, cursor)) = login {
        let panel = Rect {
            x: rows[2].x + 1,
            y: rows[2].y + 1,
            width: panel_width,
            height: rows[2].height.saturating_sub(1),
        };
        f.render_widget(Paragraph::new(lines), panel);
        if let Some((row, col)) = cursor
            && (row as u16) < panel.height
        {
            f.set_cursor_position((
                panel.x + (col as u16).min(panel.width.saturating_sub(1)),
                panel.y + row as u16,
            ));
        }
    } else if permission {
        f.render_widget(Paragraph::new(permission_lines(app, area.width)), rows[2]);
    } else {
        input(f, app, rows[2], &wrapped_input, cursor_row, cursor_col);
    }

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(rows[3].width as usize),
            dim(),
        ))),
        rows[3],
    );
    if !menu.is_empty() {
        f.render_widget(Paragraph::new(menu), rows[4]);
    }
    f.render_widget(
        Paragraph::new(footer_line(app, rows[5].width, palette.as_ref())),
        rows[5],
    );

    // Dialogs float over the conversation only: the rule, the input, the
    // footer and a docked panel stay in view.
    if app.modal.is_some() && !permission {
        modal(f, app, cols[0]);
    }
}

/// The `/` palette or the path menu of `/open`, `/check` and `/attach`,
/// as the sign-in panel draws its lists (one look, one ranking): an arrow
/// and the accent on the selected row only, names then muted notes (a
/// command's arguments lead its note), a dim `◆` after commands that call
/// a model, section headers on bare `/`, and `(n/N)` under it. At most
/// `avail` rows, `width` columns.
fn palette_lines(app: &App, kind: &PaletteKind, width: usize, avail: usize) -> Vec<Line<'static>> {
    use crate::prompt::{Menu, MenuRow, menu_lines};
    const MARK: &str = " ◆";
    let lines = match kind {
        PaletteKind::Commands(cmds) => {
            if cmds.is_empty() {
                return Vec::new();
            }
            let notes: Vec<String> = cmds
                .iter()
                .map(|(_, c)| {
                    if c.args.is_empty() {
                        c.description.to_string()
                    } else {
                        format!("{} — {}", c.args, c.description)
                    }
                })
                .collect();
            let sel = app.cmd_sel.min(cmds.len() - 1);
            let mut rows = Vec::new();
            let mut selected = None;
            let mut last = None;
            for (i, ((group, c), note)) in cmds.iter().zip(&notes).enumerate() {
                if group.is_some() && *group != last {
                    rows.push(MenuRow {
                        header: true,
                        name: group.map_or("", Group::label),
                        ..MenuRow::default()
                    });
                    last = *group;
                }
                if i == sel {
                    selected = Some(rows.len());
                }
                rows.push(MenuRow {
                    header: false,
                    name: c.name.trim_start_matches('/'),
                    mark: if c.model { MARK } else { "" },
                    note,
                });
            }
            // The name column is the same for every query, so it holds
            // still while you type.
            let col = commands::all()
                .iter()
                .map(|c| c.name.width() - 1 + if c.model { MARK.width() } else { 0 })
                .max()
                .unwrap_or(0)
                + 2;
            let legend = if cmds.iter().any(|(_, c)| c.model) {
                MODEL_LEGEND
            } else {
                ""
            };
            let menu = Menu {
                rows,
                selected,
                place: (sel + 1, cmds.len()),
                col,
                legend: Some(legend),
            };
            menu_lines(&menu, width, avail)
        }
        PaletteKind::Path(_, files) => {
            if files.is_empty() {
                return Vec::new();
            }
            let sel = app.cmd_sel.min(files.len() - 1);
            let rows: Vec<MenuRow> = files
                .iter()
                .map(|c| MenuRow {
                    header: false,
                    name: &c.label,
                    mark: "",
                    note: if c.arg == "clear" {
                        "remove the attachments"
                    } else {
                        ""
                    },
                })
                .collect();
            let menu = Menu {
                col: Menu::column(&rows),
                rows,
                selected: Some(sel),
                place: (sel + 1, files.len()),
                legend: None,
            };
            menu_lines(&menu, width, avail)
        }
    };
    tone_lines(lines)
}

/// Lines laid out by [`crate::prompt`] in the TUI's colours: the accent is
/// cyan.
fn tone_lines(lines: Vec<crate::prompt::Line>) -> Vec<Line<'static>> {
    use crate::prompt::Tone;
    let accent = Style::default().fg(Color::Cyan);
    lines
        .into_iter()
        .map(|l| {
            Line::from(
                l.0.into_iter()
                    .map(|(tone, text)| {
                        let style = match tone {
                            Tone::Plain => Style::default(),
                            Tone::Title => accent.add_modifier(Modifier::BOLD),
                            Tone::Dim => dim(),
                            Tone::Accent => accent,
                            Tone::Good => Style::default().fg(Color::Green),
                            Tone::Error => Style::default().fg(Color::Red),
                        };
                        Span::styled(text, style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
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
                let plain = !app.mouse_capture;
                for (i, mut l) in markdown::render_as(text, body, Style::default(), plain)
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
            Entry::Note(text) => lines.extend(note_lines(text, width)),
            Entry::Source { title, range, body } => {
                lines.extend(source_block_as(
                    title,
                    range,
                    body,
                    width,
                    !app.mouse_capture,
                ));
            }
            Entry::Search { query, hits, total } => {
                lines.extend(search_block(query, hits, *total, width));
            }
            Entry::Info(rows) => lines.extend(info_block(rows, width)),
        }
        if !hidden_thought {
            prev = Some(entry);
            items.push((index, item_start..lines.len()));
        }
    }
    (lines, items)
}

/// A local note: a dim `·` lead, the text in the default colour (inline
/// markdown, so `` `x` `` reads as code).
pub fn note_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, l) in text.trim_matches('\n').lines().enumerate() {
        let mut spans = vec![Span::styled(if i == 0 { "· " } else { "  " }, dim())];
        spans.extend(markdown::inline(l, Style::default()));
        out.extend(markdown::wrap_hanging(&spans, width, 2));
    }
    out
}

/// A source `/open` showed: `▤ path · lines 20–24 of 391`, then its
/// numbered block under a two-column margin.
#[cfg(test)]
pub fn source_block(title: &str, range: &str, body: &str, width: u16) -> Vec<Line<'static>> {
    source_block_as(title, range, body, width, false)
}

/// As [`source_block`], or `plain` for terminal selection (F2): the code
/// without its gutter, so a drag copies only the text.
pub fn source_block_as(
    title: &str,
    range: &str,
    body: &str,
    width: u16,
    plain: bool,
) -> Vec<Line<'static>> {
    let mut out = markdown::wrap_hanging(
        &[
            Span::styled("▤ ", Style::default().fg(Color::Cyan)),
            Span::styled(
                title.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · {range}"), dim()),
        ],
        width,
        2,
    );
    for mut l in markdown::render_as(body, width.saturating_sub(2), Style::default(), plain) {
        l.spans.insert(0, Span::raw("  "));
        out.push(l);
    }
    out
}

/// `/find` hits: `⌕ terms · first 12 of 31 matches`, then each file once
/// (bold) with its lines under a dim gutter that lines up with `/open`'s.
pub fn search_block(
    query: &str,
    hits: &[crate::sources::Hit],
    total: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let count = if total > hits.len() {
        format!(
            " · first {} of {total} matches · add a word to narrow",
            hits.len()
        )
    } else if hits.len() == 1 {
        " · 1 match".to_string()
    } else {
        format!(" · {} matches", hits.len())
    };
    let mut out = markdown::wrap_hanging(
        &[
            Span::styled("⌕ ", Style::default().fg(Color::Cyan)),
            Span::styled(
                query.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(count, dim()),
        ],
        width,
        2,
    );
    // Files in the order of their best hit; lines in order within a file.
    // `(page, line)` sorts a PDF's hits; the mark is what the gutter shows.
    let mut files: Vec<(&str, Vec<&crate::sources::Hit>)> = Vec::new();
    for h in hits {
        let Some((file, _, _)) = hit_place(h) else {
            continue;
        };
        match files.iter_mut().find(|(f, _)| *f == file) {
            Some((_, rows)) => rows.push(h),
            None => files.push((file, vec![h])),
        }
    }
    let gutter = hits
        .iter()
        .filter_map(|h| hit_place(h).map(|p| p.2.width()))
        .max()
        .unwrap_or(0)
        .max(5);
    for (file, mut rows) in files {
        rows.sort_by_key(|h| hit_place(h).map(|p| p.1));
        out.extend(markdown::wrap_hanging(
            &[
                Span::raw("  "),
                Span::styled(
                    file.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ],
            width,
            4,
        ));
        for h in rows {
            let mark = hit_place(h).map(|p| p.2).unwrap_or_default();
            let text = h.quote.trim();
            out.extend(markdown::wrap_hanging(
                &[
                    Span::styled(format!("  {mark:>gutter$}  "), dim()),
                    Span::raw(text.to_string()),
                ],
                width,
                gutter + 4,
            ));
        }
    }
    out
}

/// Where a hit is: its file, a key that sorts it (page, line), and what
/// the gutter shows (`42`, or `p3:12` in a PDF). None for a web hit.
fn hit_place(h: &crate::sources::Hit) -> Option<(&str, (usize, usize), String)> {
    match &h.at {
        crate::sources::Locator::Local { file, page, line } => Some((
            file.as_str(),
            (page.unwrap_or(0), *line),
            match page {
                Some(p) => format!("p{p}:{line}"),
                None => line.to_string(),
            },
        )),
        crate::sources::Locator::Web { .. } => None,
    }
}

/// Facts as rows: keys aligned in a dim column, values beside them; a value
/// with several lines continues under itself.
pub fn info_block(rows: &[(String, String)], width: u16) -> Vec<Line<'static>> {
    let key_w = rows.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
    let mut out = Vec::new();
    for (i, (key, value)) in rows.iter().enumerate() {
        for (j, v) in value.lines().enumerate() {
            let first = i == 0 && j == 0;
            let label = if j == 0 { key.as_str() } else { "" };
            let mut spans = vec![
                Span::styled(if first { "· " } else { "  " }, dim()),
                Span::styled(format!("{label:<key_w$}  "), dim()),
            ];
            spans.extend(markdown::inline(v, Style::default()));
            out.extend(markdown::wrap_hanging(&spans, width, key_w + 4));
        }
    }
    out
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
        // Placed as if the menu's rows were the conversation's, so opening
        // the menu covers the card's lower part instead of moving it.
        lines = empty_card(app, area.width, area.height + app.menu_rows);
        lines.truncate(area.height as usize);
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

/// A fresh thread's card, centred in the conversation: what Pathfinder is
/// for and the loop (ask, read, plan, be checked), the commands cyan, the
/// rest dim, and where earlier threads are.
pub fn empty_card(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let cyan = Style::default().fg(Color::Cyan);
    const COL: usize = 13;
    let headline = format!(
        "Understand {} by working through it",
        commands::tilde_path(&app.cwd)
    );
    let pair = |key: &str, what: &str| {
        vec![
            Span::styled(format!("{key:<COL$}"), cyan),
            Span::styled(what.to_string(), dim()),
        ]
    };
    let mut open_find = pair("/open f", "read a source");
    let find = pair("/find terms", "search files");
    let one_row = COL + 21 + COL + "search files".width();
    let mut rows: Vec<Vec<Span<'static>>> =
        vec![pair("ask", "type a question; you get sources, not answers")];
    if one_row <= width as usize {
        open_find[1] = Span::styled(format!("{:<21}", "read a source"), dim());
        open_find.extend(find);
        rows.push(open_find);
    } else {
        rows.push(open_find);
        rows.push(find);
    }
    rows.push(pair("/attach f", "add your notes or a photo of your work"));
    rows.push(pair("/plan goal", "map the parts to understand"));
    rows.push(vec![Span::styled(
        "say when it clicks: you'll judge a short candidate",
        dim(),
    )]);
    let mut last = Vec::new();
    if app.earlier_threads > 0 {
        last.push(Span::styled("^O", cyan));
        last.push(Span::styled(
            format!(" earlier threads ({}) · ", app.earlier_threads),
            dim(),
        ));
    }
    last.extend([
        Span::styled("F1", cyan),
        Span::styled(" keys · ", dim()),
        Span::styled("/", cyan),
        Span::styled(" commands", dim()),
    ]);

    let span_w = |spans: &[Span]| spans.iter().map(|s| s.content.width()).sum::<usize>();
    let block = rows
        .iter()
        .map(|r| span_w(r))
        .chain([headline.width(), span_w(&last)])
        .max()
        .unwrap_or(0);
    let w = width as usize;
    let left = if block < w { (w - block) / 2 } else { 0 };
    let room = (w - left).max(1) as u16;
    let mut card = markdown::wrap(&[Span::raw(headline)], room);
    card.push(Line::default());
    for r in rows {
        let hang = if r.len() > 1 { COL } else { 0 };
        card.extend(markdown::wrap_hanging(&r, room, hang));
    }
    card.push(Line::default());
    card.extend(markdown::wrap(&last, room));

    let top = (height as usize).saturating_sub(card.len()) / 2;
    let mut out = vec![Line::default(); top];
    let pad = " ".repeat(left);
    out.extend(card.into_iter().map(|mut l| {
        if !l.spans.is_empty() {
            l.spans.insert(0, Span::raw(pad.clone()));
        }
        l
    }));
    out
}

/// A file the last reply points to, as `/open` takes it: the first
/// backticked path that is a file here, with its lines.
fn reply_source(app: &App) -> Option<String> {
    let Some(Entry::Text(reply)) = app.entries.last() else {
        return None;
    };
    reply
        .split('`')
        .skip(1)
        .step_by(2)
        .take(20)
        .map(str::trim)
        .find(|span| {
            let path = span
                .split_once(':')
                .filter(|(_, at)| at.chars().all(|c| c.is_ascii_digit() || c == '-'))
                .map_or(*span, |(p, _)| p);
            !path.is_empty()
                && !path.contains(char::is_whitespace)
                && commands::resolve_attach_path(path, &app.cwd).is_some_and(|p| p.is_file())
        })
        .map(str::to_string)
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

fn has_review_ready(app: &App) -> bool {
    app.session
        .map
        .parts
        .iter()
        .any(|p| p.status == PartStatus::InReview)
}

fn has_waiting_state(app: &App) -> bool {
    let s = &app.session;
    s.pending_commitment.is_some()
        || s.pending_change.is_some()
        || s.pending_candidate.is_some()
        || has_review_ready(app)
}

/// What Enter does to what waits for your OK (§15), from the part's title
/// only: never the candidate or anything the gate holds (A6).
fn ok_what(kind: crate::domain::CommitKind, title: &str) -> String {
    use crate::domain::CommitKind;
    match kind {
        CommitKind::Verdict => "judge your message as your verdict".into(),
        CommitKind::Handoff => format!("hand {title} to the agent"),
        CommitKind::Delegate => format!("delegate {title}"),
    }
}

/// What waits on you, in a few words, for the rule when the panel isn't
/// docked.
fn waiting_text(app: &App) -> Option<&'static str> {
    let s = &app.session;
    if s.pending_commitment.is_some() {
        Some("⧗ your OK (Enter runs · Esc cancels)")
    } else if s.pending_change.is_some() {
        Some("⧗ map change (/accept · /reject)")
    } else if s.pending_candidate.is_some() && s.exam {
        Some("⧗ your answer")
    } else if s.pending_candidate.is_some() {
        Some("⧗ your verdict (right/flawed + why)")
    } else if has_review_ready(app) {
        Some("⧗ review ready (/approve · /object)")
    } else {
        None
    }
}

/// Parts done of all, and the active one: `Parts 2/5 » Verdict prompt`.
fn parts_text(app: &App) -> Option<String> {
    let s = &app.session;
    if s.map.parts.is_empty() {
        return None;
    }
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
    Some(format!("{what} {done}/{}{active}", s.map.parts.len()))
}

/// The thread's mode, for the rule and the side panel.
fn mode_name(app: &App) -> &'static str {
    mode_of(&app.session)
}

/// A thread's mode in a word or two: chat, task, exam, quick check.
pub fn mode_of(s: &crate::domain::Session) -> &'static str {
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

/// The thread's name for the rule (see [`super::thread_name`]).
fn rule_title(app: &App) -> Option<String> {
    super::thread_name(&app.session)
}

/// The rule over the input, the one status line: where you are on the
/// left (folder, mode, thread), and on the right what runs, what waits on
/// you and where the view is. When it doesn't all fit, the least needed
/// goes first: the thought, the parts count, the scroll chip; the thread
/// name shortens before any of them.
pub fn rule_line(app: &App, width: u16, docked: bool) -> Line<'static> {
    let w = width as usize;
    // Too narrow for any label or chip: the bare rule.
    if w < 12 {
        return Line::from(Span::styled("─".repeat(w), dim()));
    }
    let cyan = Style::default().fg(Color::Cyan);
    let yellow = Style::default().fg(Color::Yellow);
    // (spans, how much it's needed: higher stays longer)
    let mut pieces: Vec<(Vec<Span<'static>>, u8)> = Vec::new();
    if app.running.is_some()
        && let Some(h) = thought_header(app)
    {
        pieces.push((
            vec![Span::styled(
                fit(&h, 40),
                dim().add_modifier(Modifier::ITALIC),
            )],
            1,
        ));
    }
    if !docked {
        let parts = parts_text(app);
        if let Some(wait) = waiting_text(app) {
            let mut spans = vec![Span::styled(wait.to_string(), yellow)];
            if parts.is_none() {
                spans.push(Span::styled(" · ^B", dim()));
            }
            pieces.push((spans, 4));
        }
        if let Some(parts) = parts {
            pieces.push((
                vec![Span::styled(parts, cyan), Span::styled(" · ^B", dim())],
                2,
            ));
        }
    }
    if app.scroll > 0 {
        let chip = if app.new_below {
            Span::styled("↓ new below · End", yellow)
        } else {
            let n = app.scroll;
            let unit = if n == 1 { "line" } else { "lines" };
            Span::styled(format!("↑ {n} {unit} up · End ↓"), dim())
        };
        pieces.push((vec![chip], 3));
    }
    if app.running.is_some() {
        let secs = app.started.map_or(0, |t| t.elapsed().as_secs());
        pieces.push((
            vec![
                Span::styled(SPINNER[app.tick % SPINNER.len()].to_string(), cyan),
                Span::styled(format!(" Working {} · Esc stops", elapsed(secs)), dim()),
            ],
            5,
        ));
    }

    let span_w = |spans: &[Span]| spans.iter().map(|s| s.content.width()).sum::<usize>();
    // "──" + label + "─"… + " piece " "─" " piece " + "──"
    let cost = |pieces: &[(Vec<Span<'static>>, u8)]| -> usize {
        if pieces.is_empty() {
            0
        } else {
            pieces.iter().map(|(s, _)| span_w(s) + 2).sum::<usize>() + pieces.len() - 1
        }
    };
    let folder = folder_name(app);
    let core = format!(" {folder} · {} ", mode_name(app));
    let drop_lowest = |pieces: &mut Vec<(Vec<Span<'static>>, u8)>| {
        if let Some(i) = (0..pieces.len()).min_by_key(|&i| pieces[i].1) {
            pieces.remove(i);
        }
    };
    // The low ones go before the folder and mode are cut.
    while !pieces.is_empty()
        && 5 + core.width() + cost(&pieces) > w
        && pieces.iter().any(|(_, keep)| *keep <= 2)
    {
        drop_lowest(&mut pieces);
    }
    while pieces.len() > 1 && 5 + cost(&pieces) > w {
        drop_lowest(&mut pieces);
    }
    if let [(spans, _)] = pieces.as_mut_slice()
        && 5 + span_w(spans) + 2 > w
    {
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        let style = spans.first().map(|s| s.style).unwrap_or_default();
        *spans = vec![Span::styled(fit(&text, w.saturating_sub(7)), style)];
    }

    let label = match rule_title(app) {
        Some(title) => format!(" {folder} · {} · {title} ", mode_name(app)),
        None => core,
    };
    let room = w.saturating_sub(5 + cost(&pieces));
    let label = if room >= 6 {
        let cut = fit(label.trim_end(), room.saturating_sub(1));
        format!("{cut} ")
    } else {
        String::new()
    };
    let lead = if w >= 2 { "──" } else { "" };
    let tail = if pieces.is_empty() { 0 } else { 2 };
    let fill = w.saturating_sub(lead.width() + label.width() + cost(&pieces) + tail);
    let mut out = vec![
        Span::styled(lead.to_string(), dim()),
        Span::styled(label, dim()),
        Span::styled("─".repeat(fill), dim()),
    ];
    for (i, (spans, _)) in pieces.into_iter().enumerate() {
        if i > 0 {
            out.push(Span::styled("─", dim()));
        }
        out.push(Span::raw(" "));
        out.extend(spans);
        out.push(Span::raw(" "));
    }
    if tail > 0 {
        out.push(Span::styled("──", dim()));
    }
    Line::from(out)
}

fn folder_name(app: &App) -> String {
    app.cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.cwd.display().to_string())
}

/// The next steps the panel lists: the waiting blocks carry their own.
fn next_actions(app: &App) -> Vec<commands::QuickAction> {
    commands::contextual_actions(&app.session)
        .into_iter()
        .filter(|a| {
            !matches!(
                a.command,
                "/accept"
                    | "/reject"
                    | "/approve"
                    | "/object "
                    | "/confirm"
                    | "/cancel"
                    | "/undo"
                    | ""
            )
        })
        .collect()
}

/// The side panel's lines at `width`: the goal, what waits on you, the
/// parts and the next steps, one row each.
pub fn side_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let s = &app.session;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let yellow_bold = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let cyan = Style::default().fg(Color::Cyan);
    let w = width as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();
    let gap = |lines: &mut Vec<Line<'static>>| {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
    };
    if s.goal.is_none() && s.map.parts.is_empty() && !has_waiting_state(app) {
        lines.extend(markdown::wrap(
            &[Span::styled(
                "Your Understanding Map appears here after /plan",
                dim(),
            )],
            width,
        ));
    }
    if let Some(goal) = &s.goal {
        lines.push(Line::from(Span::styled("Goal", bold)));
        lines.extend(markdown::wrap(&[Span::raw(goal.clone())], width));
    }

    // What waits on you, with the commands and keys that answer it.
    if let Some((kind, title)) = s.waiting_for_ok() {
        gap(&mut lines);
        lines.extend(markdown::wrap(
            &[Span::styled("⧗ Waiting for your OK", yellow_bold)],
            width,
        ));
        let what = ok_what(kind, title);
        let mut what = what[..1].to_uppercase() + &what[1..];
        what.push('?');
        lines.extend(markdown::wrap(&[Span::raw(what)], width));
        let cancel = if kind == crate::domain::CommitKind::Verdict {
            " keeps it as a note"
        } else {
            " cancels"
        };
        lines.extend(markdown::wrap(
            &[
                Span::styled("Enter", cyan),
                Span::styled(" runs · ", dim()),
                Span::styled("Esc", cyan),
                Span::styled(cancel, dim()),
            ],
            width,
        ));
    } else if s.undoable()
        && let Some(title) = s
            .last_commitment
            .as_ref()
            .and_then(|u| s.map.get(&u.part))
            .map(|p| p.title.clone())
    {
        gap(&mut lines);
        lines.extend(markdown::wrap(
            &[
                Span::styled("↶ ", cyan),
                Span::raw(format!("You claimed {title}")),
            ],
            width,
        ));
        lines.extend(markdown::wrap(
            &[
                Span::styled("Esc", cyan),
                Span::styled(" or ", dim()),
                Span::styled("/undo", cyan),
                Span::styled(" takes it back, now only", dim()),
            ],
            width,
        ));
    }
    if let Some(change) = &s.pending_change {
        gap(&mut lines);
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
        gap(&mut lines);
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
        gap(&mut lines);
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
        gap(&mut lines);
        let count = format!("{done}/{}", s.map.parts.len());
        let head = if s.exam { "Exam" } else { "Parts" };
        let pad = w.saturating_sub(head.width() + count.width()).max(1);
        lines.push(Line::from(vec![
            Span::styled(head, bold),
            Span::raw(" ".repeat(pad)),
            Span::styled(count, dim()),
        ]));
        for (n, part) in s.map.parts.iter().enumerate() {
            let active = s.active.as_deref() == Some(part.id.as_str());
            let (mark, color) = match part.status {
                _ if active => ("▶", Color::Cyan),
                PartStatus::Understood | PartStatus::Done => ("✓", Color::Green),
                PartStatus::Known => ("~", Color::Yellow),
                PartStatus::Delegated => ("⇢", Color::Magenta),
                PartStatus::InReview => ("⧗", Color::Yellow),
                PartStatus::Open => ("○", Color::DarkGray),
            };
            let num = format!("{} ", n + 1);
            let mut spans = vec![
                Span::styled(format!("{mark} "), Style::default().fg(color)),
                Span::styled(num.clone(), dim()),
                Span::styled(
                    part.title.clone(),
                    if active { bold } else { Style::default() },
                ),
            ];
            if part.status == PartStatus::InReview {
                spans.push(Span::styled(
                    " [review]",
                    Style::default().fg(Color::Yellow),
                ));
            }
            // §16's horizon: the parts this one needs, by their numbers.
            let needs: Vec<String> = part
                .depends_on
                .iter()
                .filter_map(|id| s.map.parts.iter().position(|p| &p.id == id))
                .map(|i| (i + 1).to_string())
                .collect();
            if !needs.is_empty() {
                spans.push(Span::styled(format!(" ← {}", needs.join(", ")), dim()));
            }
            lines.extend(markdown::wrap_hanging(&spans, width, 2 + num.width()));
        }
    }

    // One row each: the command (its name alone), then what it does in the
    // palette's words, cut to fit.
    let actions: Vec<(&str, &str)> = next_actions(app)
        .into_iter()
        .map(|a| {
            match commands::all()
                .into_iter()
                .find(|c| c.name == a.command.trim())
            {
                Some(c) => (c.name, c.description),
                None => (a.label, a.description),
            }
        })
        .collect();
    if !actions.is_empty() {
        gap(&mut lines);
        lines.push(Line::from(Span::styled("Next", bold)));
        let col = actions
            .iter()
            .map(|(label, _)| label.width())
            .max()
            .unwrap_or(0)
            .min(w / 2);
        for (label, description) in actions {
            let label = fit(label, col.max(4));
            let pad = col.saturating_sub(label.width()) + 2;
            let room = w.saturating_sub(label.width() + pad);
            let mut spans = vec![Span::styled(label, cyan)];
            if room >= 4 {
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(fit(description, room), dim()));
            }
            lines.push(Line::from(spans));
        }
    }

    lines
}

fn side_panel(f: &mut Frame, app: &mut App, area: Rect) {
    // Docked: a hairline on the left. Over the conversation: a frame that
    // says how to close it.
    let block = if app.side_overlay {
        Block::default()
            .borders(Borders::ALL)
            .border_style(dim())
            .title(Span::styled(
                " Map ",
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(" Esc closes ", dim()))
            .padding(ratatui::widgets::Padding::horizontal(1))
    } else {
        Block::default()
            .borders(Borders::LEFT)
            .border_style(dim())
            .padding(ratatui::widgets::Padding::left(1))
    };
    let inner = block.inner(area);
    // A column to spare on the right when docked; the frame has its own.
    let spare = u16::from(!app.side_overlay);
    let lines = side_lines(app, inner.width.saturating_sub(spare).max(8));
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
    } else if let Some((kind, title)) = app.session.waiting_for_ok() {
        let esc = if kind == crate::domain::CommitKind::Verdict {
            "keeps it as a note"
        } else {
            "cancels"
        };
        format!("Enter: {} · Esc {esc}", ok_what(kind, title))
    } else if app.session.undoable() {
        "Esc takes back your claim, or go on".into()
    } else if app.session.pending_change.is_some() {
        "/accept or /reject the proposed plan change".into()
    } else if app.session.pending_candidate.is_some() && app.session.exam {
        "Your answer".into()
    } else if app.session.pending_candidate.is_some() {
        "Right or flawed, and why?".into()
    } else if has_review_ready(app) {
        "/approve or /object <reason> on the staged result".into()
    } else if let Some(path) = reply_source(app) {
        format!("Open a source: /open {path}")
    } else if app.session.mode == SessionMode::Working && app.session.active_part().is_some() {
        "Explain the current part, or /next".into()
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
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(ph, dim()))),
            text_area,
        );
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
/// What approving a command outside any sandbox grants, and why there's
/// no sandbox.
fn no_sandbox_warning() -> &'static str {
    if crate::agent::sandbox::available().is_some() {
        "Outside the sandbox: this runs with your full access to your files, credentials and network."
    } else if cfg!(windows) {
        "No sandbox: this runs with your full access to your files, credentials and network (Windows has no OS sandbox for Pathfinder)."
    } else if cfg!(target_os = "linux") {
        "No sandbox: this runs with your full access to your files, credentials and network (install bubblewrap, `bwrap`, to sandbox commands)."
    } else {
        "No sandbox: this runs with your full access to your files, credentials and network (no OS sandbox is available here)."
    }
}

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
    let prog = super::always_label(always_key);

    let mut lines = vec![Line::from(Span::styled(
        "Allow the agent to run this?",
        bold.fg(Color::Yellow),
    ))];
    lines.extend(markdown::wrap_hanging(
        &[Span::styled(format!("  {title}"), bold)],
        width.saturating_sub(1),
        4,
    ));
    let unsandboxed = always_key.starts_with("unsandboxed:");
    if unsandboxed {
        lines.extend(markdown::wrap_hanging(
            &[Span::styled(
                format!("  {}", no_sandbox_warning()),
                Style::default().fg(Color::Red),
            )],
            width.saturating_sub(1),
            4,
        ));
    }

    let choices = [
        ("1 / y", "Yes, once".to_string()),
        (
            "2 / a",
            if unsandboxed {
                format!("Yes, always in this thread for exactly `{prog}`, without a sandbox")
            } else {
                format!("Yes, always in this thread for `{prog}`")
            },
        ),
        ("3 / n / Esc", "No".to_string()),
    ];
    for (idx, (k, label)) in choices.iter().enumerate() {
        let chosen = *sel == Some(idx);
        let prefix = if chosen { "▶ " } else { "  " };
        let line_spans = if chosen {
            vec![Span::styled(format!("{prefix}{k}  {label}"), rev)]
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

/// The footer: key hints (or a status message, or the selected command in
/// full) on the left; the models and settings on the right, and SELECT in
/// terminal-selection mode. The right side gives way first, from the full
/// `fast … · main … · decisions … · checks …` down to the main model; the
/// hints then shorten down to `Enter send · F1 keys`. Nothing runs past the
/// edge. While a dialog is open the left stays empty: its border holds its
/// keys.
const IDLE_HINTS: &[&str] = &[
    "Enter send · ⇧↵ line · ^P settings · ^O threads · F1 keys",
    "Enter send · ^P settings · ^O threads · F1 keys",
    "Enter send · F1 keys",
];

fn strings(hints: &[&str]) -> Vec<String> {
    hints.iter().map(|h| h.to_string()).collect()
}

fn footer_line(app: &App, width: u16, palette: Option<&PaletteKind>) -> Line<'static> {
    use crate::options::{CHECKS, DECISION_MODEL, DECISIONS, FAST_MODEL, MAIN_MODEL};
    // A column of margin at each end.
    let w = (width as usize).saturating_sub(2);
    let decisions = if app.current(DECISIONS) == "on" {
        app.footer_model(DECISION_MODEL, false)
    } else {
        "off".into()
    };
    let checks = app.current(CHECKS);
    let (fast_a, main_a) = (
        app.footer_model(FAST_MODEL, true),
        app.footer_model(MAIN_MODEL, true),
    );
    let (fast, main) = (
        app.footer_model(FAST_MODEL, false),
        app.footer_model(MAIN_MODEL, false),
    );
    let models = [
        format!("fast {fast_a} · main {main_a} · decisions {decisions} · checks {checks}"),
        format!("fast {fast} · main {main} · decisions {decisions} · checks {checks}"),
        format!("fast {fast} · main {main} · checks {checks}"),
        format!("fast {fast} · main {main}"),
        format!("main {main}"),
        main.clone(),
        String::new(),
    ];
    // Down to here the hints are worth more than the models.
    const KEEP_MODEL: usize = 4;
    let badge = (!app.mouse_capture).then_some("SELECT · F2");
    let right_w = |m: &str| match badge {
        Some(b) if m.is_empty() => b.width(),
        Some(b) => b.width() + 1 + m.width(),
        None => m.width(),
    };

    enum Left {
        /// Hints, longest first.
        Hints(Vec<String>),
        /// The palette's line: it fits beside the steady right side, cut
        /// short rather than moving the models while you type.
        Menu(Vec<String>),
        Text(String, Style),
    }
    let source_selected = app
        .selected
        .is_some_and(|i| matches!(app.entries.get(i), Some(Entry::Source { .. })));
    let left = if app.modal.is_some() {
        match app.modal {
            Some(Modal::Share { .. }) => Left::Text(
                "Paths are shortened to ./ and ~; nothing else is changed".into(),
                dim(),
            ),
            Some(Modal::Consent { .. }) => Left::Text(
                "Your answer is saved; `pathfinder data` changes it later".into(),
                dim(),
            ),
            _ => Left::Text(String::new(), dim()),
        }
    } else if !app.status.is_empty() {
        Left::Text(app.status.clone(), Style::default())
    } else if let Some(kind) = palette {
        Left::Menu(palette_hints(app, kind))
    } else if let Some(fix) = app
        .unknown_fix()
        .filter(|_| app.selected.is_none() && !app.input.contains('\n'))
    {
        // A /word no command has: what Tab makes of it, before Enter.
        let word = fix.split(char::is_whitespace).next().unwrap_or_default();
        Left::Text(format!("No such command · Tab → {word}"), dim())
    } else if source_selected {
        Left::Hints(strings(&[
            "↑↓ move · y copy code · Y with path:lines · g/G first/last · Esc back",
            "↑↓ · y code · Y with path · Esc",
        ]))
    } else if app.selected.is_some() {
        Left::Hints(strings(&[
            "↑↓ move · Enter expand · y/^C copy · g/G first/last · Esc back",
            "↑↓ · Enter · y copy · Esc",
        ]))
    } else {
        Left::Hints(strings(IDLE_HINTS))
    };

    let fits = |l: usize, m: &str| {
        let r = right_w(m);
        l + r + usize::from(l > 0 && r > 0) * 2 <= w
    };
    let last = models.len() - 1;
    // The richest right side, from `from` down to `upto`, that fits beside
    // `l` columns of left.
    let best_right =
        |l: usize, from: usize, upto: usize| (from..=upto).find(|&i| fits(l, &models[i]));
    let pick = |hints: &[String], from: usize| -> (String, usize) {
        hints
            .iter()
            .find_map(|h| {
                best_right(h.width(), from, KEEP_MODEL.max(from)).map(|i| (h.to_string(), i))
            })
            .unwrap_or_else(|| {
                let h = hints.last().map(String::as_str).unwrap_or_default();
                let i = best_right(h.width(), from, last).unwrap_or(last);
                let r = right_w(&models[i]);
                (fit(h, w.saturating_sub(r + usize::from(r > 0) * 2)), i)
            })
    };
    // What the right side shows with the usual hints: other left sides start
    // from there, so the models don't jump about as messages come and go.
    let (_, steady) = pick(&strings(IDLE_HINTS), 0);
    let (left, left_style, at) = match left {
        Left::Hints(hints) => {
            let (h, i) = pick(&hints, steady);
            (h, dim(), i)
        }
        Left::Menu(hints) => {
            let r = right_w(&models[steady]);
            let room = w.saturating_sub(r + usize::from(r > 0) * 2);
            let h = hints
                .iter()
                .find(|h| h.width() <= room)
                .cloned()
                .unwrap_or_else(|| fit(hints.last().map(String::as_str).unwrap_or_default(), room));
            (h, dim(), steady)
        }
        Left::Text(text, style) => {
            // The message stays whole when it can: the models give way.
            let i = best_right(text.width(), steady, last).unwrap_or(last);
            let r = right_w(&models[i]);
            let room = w.saturating_sub(r + usize::from(r > 0) * 2);
            (fit(&text, room), style, i)
        }
    };
    let right = models[at].clone();

    let mut right_spans = Vec::new();
    if let Some(b) = badge {
        right_spans.push(Span::styled(
            b,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        if !right.is_empty() {
            right_spans.push(Span::raw(" "));
        }
    }
    let rw = right_w(&right).min(w);
    if !right.is_empty() {
        right_spans.push(Span::styled(right, dim()));
    }
    let gap = w.saturating_sub(left.width() + rw);
    let mut spans = vec![Span::raw(" "), Span::styled(left, left_style)];
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right_spans);
    Line::from(spans)
}

/// The footer while a menu is open, longest first: the selected command
/// with its other names, then the keys; a path menu's keys say what Enter
/// does to the selected row.
fn palette_hints(app: &App, kind: &PaletteKind) -> Vec<String> {
    match kind {
        PaletteKind::Commands(cmds) => {
            let Some((_, c)) = cmds.get(app.cmd_sel.min(cmds.len().saturating_sub(1))) else {
                return Vec::new();
            };
            let enter = if app.enter_runs(c) {
                "Enter run"
            } else {
                "Enter fill in"
            };
            let mut named = c.usage();
            if !c.aliases.is_empty() {
                named.push_str(&format!(" · also {}", c.aliases.join(", ")));
            }
            vec![
                format!("{named} · {enter} · Tab complete · Esc close"),
                format!("{} · {enter} · Tab complete", c.name),
                format!("{enter} · Tab complete"),
            ]
        }
        PaletteKind::Path(cmd, files) => {
            let Some(c) = files.get(app.cmd_sel.min(files.len().saturating_sub(1))) else {
                return Vec::new();
            };
            let enter = match *cmd {
                _ if c.is_dir => "Enter go in",
                "/open" => "Enter open",
                "/attach" => "Enter attach",
                _ => "Enter pick",
            };
            vec![
                format!("{enter} · Tab complete · Esc close"),
                format!("{enter} · Tab complete"),
            ]
        }
    }
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

/// A boxed list drawn as the palette draws its rows: a column of padding
/// inside the border, then `→ ` and the accent on the selected row only.
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
                    .title_bottom(Span::styled(fit(&format!(" {hint} "), inner_w), dim()))
                    .padding(ratatui::widgets::Padding::left(1)),
            )
            .highlight_style(Style::default().fg(Color::Cyan))
            .highlight_symbol("→ "),
        area,
        &mut state,
    );
}

/// A filterable list in a box: a filter row, then the matches (section
/// headers among them, never selected). `meta`, when given, holds a dim
/// note per item, right-aligned beside it.
#[allow(clippy::too_many_arguments)]
fn filterable_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: &[(String, String)],
    meta: &[String],
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
    let filter_area = Rect { height: 1, ..inner };
    // A column of padding inside the border, as the palette's rows have.
    let list_area = Rect {
        x: inner.x + 1,
        y: inner.y + 1,
        width: inner.width.saturating_sub(1),
        height: inner.height - 1,
    };

    let total = items.iter().filter(|i| !is_header(i)).count();
    let count = if query.is_empty() {
        format!(" ({total})")
    } else {
        format!(" ({} of {total})", matched.len())
    };
    let filter_spans = if query.is_empty() {
        vec![
            Span::styled(" Filter: ", dim()),
            Span::styled(
                fit(
                    &format!("type to search…{count}"),
                    inner_w.saturating_sub(9),
                ),
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

    // Section headers are drawn among the rows but never selected: `sel`
    // counts matches only.
    let shown = display_rows(items, query);
    let sectioned = items.iter().any(is_header);
    let sel = sel.min(matched.len() - 1);
    let at = shown
        .iter()
        .position(|&idx| idx == matched[sel])
        .unwrap_or(0);
    // Headers fit inside the box with a column to spare (after the
    // selection marker's two columns).
    let header_w = (list_area.width as usize).saturating_sub(3);
    // A row's room after the selection marker, a column short of the
    // border.
    let row_w = (list_area.width as usize).saturating_sub(3);
    let rows: Vec<ListItem> = shown
        .iter()
        .map(|&idx| {
            if is_header(&items[idx]) {
                ListItem::new(Line::from(Span::styled(
                    fit(&items[idx].1, header_w),
                    dim().add_modifier(Modifier::BOLD),
                )))
            } else if sectioned {
                ListItem::new(format!("  {}", items[idx].1))
            } else if let Some(note) = meta.get(idx) {
                ListItem::new(with_note(&items[idx].1, note, row_w))
            } else {
                ListItem::new(items[idx].1.clone())
            }
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(at));
    f.render_stateful_widget(
        List::new(rows)
            .highlight_style(Style::default().fg(Color::Cyan))
            .highlight_symbol("→ "),
        list_area,
        &mut state,
    );
}

/// A list row: the title, cut to leave room, then `note` dim and
/// right-aligned, `width` columns in all. A note that leaves the title
/// under 12 columns is dropped.
pub fn with_note(title: &str, note: &str, width: usize) -> Line<'static> {
    let note_w = note.width();
    if width < note_w + 2 + 12 {
        return Line::from(fit(title, width));
    }
    let title = fit(title, width - note_w - 2);
    let pad = width - note_w - title.width();
    Line::from(vec![
        Span::raw(title),
        Span::raw(" ".repeat(pad)),
        Span::styled(note.to_string(), dim()),
    ])
}

/// A settings row: the name, then a setting's few values with the
/// current one bright and the rest dim (they change in place), or its
/// value (a chooser opens).
pub fn settings_row(app: &App, option: &str, name: &str) -> Line<'static> {
    let mut spans = vec![Span::raw(format!("{name:<20} "))];
    if app.cycles(option) {
        let (values, at) = app.cycle_values(option);
        for (i, v) in values.into_iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" · ", dim()));
            }
            spans.push(if Some(i) == at {
                Span::styled(v, Style::default().add_modifier(Modifier::BOLD))
            } else {
                Span::styled(v, dim())
            });
        }
    } else {
        spans.push(Span::raw(app.current_setting(option)));
    }
    Line::from(spans)
}

fn modal(f: &mut Frame, app: &mut App, area: Rect) {
    let mut next_help_max = app.help_max_scroll;
    let mut next_consent_max = app.consent_max_scroll;

    match app.modal.as_ref().unwrap() {
        Modal::Threads {
            items,
            meta,
            query,
            sel,
        } => {
            // As tall as the whole list, so filtering doesn't move it.
            let rect = centered(area, 96, (items.len() as u16 + 4).max(6));
            filterable_list(
                f,
                rect,
                "Threads in this folder",
                items,
                meta,
                query,
                *sel,
                "↑↓ pick · type filters · Enter open · Esc close",
            );
        }
        Modal::Settings { sel } => {
            let rect = centered(area, 64, SETTINGS.len() as u16 + 2);
            let rows = SETTINGS
                .iter()
                .map(|(option, name)| ListItem::new(settings_row(app, option, name)))
                .collect();
            list(
                f,
                rect,
                "Settings for this thread",
                rows,
                *sel,
                "Enter ←→ change · s saves as default · Esc closes",
            );
        }
        Modal::Choose {
            title,
            items,
            query,
            sel,
            ..
        } => {
            // As tall as the whole list, so filtering doesn't move it.
            let rows = items.len().max(1);
            let rect = centered(area, 72, (rows as u16 + 4).max(6));
            filterable_list(
                f,
                rect,
                title,
                items,
                &[],
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
        Modal::Help { scroll, query } => {
            let (max, visible) = help_modal(f, area, *scroll, query);
            next_help_max = max;
            app.help_page = visible.saturating_sub(1).max(1);
        }
        // Drawn in place of the input.
        Modal::Login(_) => {}
    }

    app.help_max_scroll = next_help_max;
    app.consent_max_scroll = next_consent_max;
}

/// The sign-in panel's lines and its cursor (row, column).
type LoginPanel = (Vec<Line<'static>>, Option<(usize, usize)>);

/// `/login` and `/logout` (and the gate key from Settings): the sign-in
/// panel, laid out as `pathfinder auth login` draws it, in place of the
/// input so the conversation stays in view. Its lines and the cursor (row,
/// column), at `width` columns and at most `max_rows` rows.
fn login_panel(app: &App, width: u16, max_rows: u16) -> Option<LoginPanel> {
    let Some(Modal::Login(flow)) = &app.modal else {
        return None;
    };
    let (lines, cursor) =
        crate::prompt::lines(&flow.screen(), width as usize, max_rows as usize, true);
    Some((tone_lines(lines), cursor))
}

/// Help's keys, by section: (keys, what they do).
const HELP_KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Compose",
        &[
            ("Enter", "send"),
            ("⇧↵ ⌥↵ ^J", "new line"),
            ("↑ ↓", "lines of the input, then earlier inputs"),
            ("Tab", "complete a /command or a path"),
            (
                "Esc",
                "close a menu; stop a running reply (the draft stays)",
            ),
            ("^A ^E  Home End", "start / end of the line"),
            ("^U ^K", "delete to the start / end of the line"),
            ("^W ⌥⌫  ⌥D", "delete the word before / after"),
            ("^← ^→  ⌥B ⌥F", "move by a word"),
        ],
    ),
    (
        "Scroll",
        &[
            ("PgUp PgDn", "a page"),
            ("^U ^D", "half a page (input empty)"),
            ("⇧↑ ⇧↓ / wheel", "a line"),
            ("^Home  End", "top / bottom (input empty)"),
            (
                "Tab ^↑",
                "select messages: ↑↓ move, Enter expands, y copies, Y with its place",
            ),
        ],
    ),
    (
        "Threads & panels",
        &[
            ("^N ^O", "new thread / earlier threads"),
            ("^P", "settings: models, decisions, checks"),
            ("^B", "plan panel"),
            ("^T ^G", "thinking / every tool call"),
            ("F1", "these keys"),
            ("F2", "terminal text selection (no panel or bars)"),
            ("^C", "stop the reply; quit when idle"),
            ("^D ^D", "quit (input empty, at the bottom)"),
        ],
    ),
    (
        "When waiting",
        &[
            ("⌥A ⌥R", "accept or approve / reject"),
            ("⌥O", "object to the agent's result"),
            ("⌥Y ⌥N", "start a verdict: right / flawed, because …"),
            (
                "Enter  Esc",
                "input empty: run / cancel what waits for your OK",
            ),
            ("Esc", "input empty: take back a claim you just made"),
        ],
    ),
];

/// Columns between a help row's key and what it does.
const HELP_GAP: usize = 2;

/// What the dim `◆` after a command means, in Help and the palette.
pub const MODEL_LEGEND: &str = "◆ calls the model";

/// One help row: `key` in its own column of `key_w` (wrapping inside it),
/// a column of space, then `what` wrapped in the rest of `width`.
fn help_row(
    key: Vec<Span<'static>>,
    what: Vec<Span<'static>>,
    key_w: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let what_w = width.saturating_sub(key_w + HELP_GAP).max(8);
    let keys = markdown::wrap(&key, key_w.max(1) as u16);
    let whats = markdown::wrap(&what, what_w as u16);
    (0..keys.len().max(whats.len()))
        .map(|i| {
            let mut spans = Vec::new();
            let k = keys.get(i).cloned().unwrap_or_default();
            let used: usize = k.spans.iter().map(|s| s.content.width()).sum();
            spans.extend(k.spans);
            spans.push(Span::raw(" ".repeat(key_w.saturating_sub(used) + HELP_GAP)));
            if let Some(w) = whats.get(i) {
                spans.extend(w.spans.clone());
            }
            Line::from(spans)
        })
        .collect()
}

/// Help's lines at `width`: How it works, the commands by palette section
/// (from the same list the palette uses), then the keys. A `query` keeps
/// the rows that mention it, under their headings.
pub fn help_lines(width: usize, query: &str) -> Vec<Line<'static>> {
    let cyan = Style::default().fg(Color::Cyan);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let heading = cyan.add_modifier(Modifier::BOLD);
    let sub = dim().add_modifier(Modifier::BOLD);
    let q = query.trim().to_lowercase();
    let keep = |k: &str, v: &str| {
        q.is_empty() || k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q)
    };

    // (section, subsection, key, what, key style, calls a model)
    let mut rows: Vec<(&str, String, String, String, Style, bool)> = Vec::new();
    for (k, v) in [
        ("ask", "type a question; you get sources, not answers"),
        ("/open f", "read a source"),
        ("/find terms", "search files"),
        ("/attach f", "add your notes or a photo of your work"),
        ("/plan goal", "map the parts to understand"),
        ("say when it clicks", "you'll judge a short candidate"),
    ] {
        rows.push((
            "How it works",
            String::new(),
            k.into(),
            v.into(),
            cyan,
            false,
        ));
    }
    let mut cmds = commands::all();
    cmds.sort_by_key(|c| (c.group, commands::order_of(c.name)));
    for c in cmds {
        let mut what = if c.args.is_empty() {
            c.description.to_string()
        } else {
            format!("{} — {}", c.args, c.description)
        };
        if !c.aliases.is_empty() {
            what.push_str(&format!(" (also {})", c.aliases.join(", ")));
        }
        rows.push((
            "Commands",
            c.group.label().to_string(),
            c.name.into(),
            what,
            cyan,
            c.model,
        ));
    }
    for (section, keys) in HELP_KEYS {
        for (k, v) in keys.iter() {
            rows.push((
                "Keys",
                section.to_string(),
                k.to_string(),
                v.to_string(),
                bold,
                false,
            ));
        }
    }
    // The key column: as wide as the widest key of all rows (not only the
    // matching ones, so it holds still while filtering), at most a third.
    let key_w = rows
        .iter()
        .map(|r| r.2.width() + if r.5 { 2 } else { 0 })
        .max()
        .unwrap_or(4)
        .min(width / 3)
        .max(4);
    rows.retain(|(_, sub, k, v, _, _)| {
        keep(k, v) || (!q.is_empty() && sub.to_lowercase().contains(&q))
    });
    if rows.is_empty() {
        return vec![Line::from(Span::styled(
            format!("Nothing mentions “{query}”."),
            dim(),
        ))];
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    let (mut section, mut subsection) = ("", String::new());
    for (sec, subsec, k, v, style, model) in rows {
        if sec != section {
            if !out.is_empty() {
                out.push(Line::default());
            }
            out.push(Line::from(Span::styled(sec.to_string(), heading)));
            section = sec;
            subsection.clear();
        }
        if !subsec.is_empty() && subsec != subsection {
            out.push(Line::from(Span::styled(subsec.clone(), sub)));
            subsection = subsec;
        }
        let mut key = vec![Span::styled(k, style)];
        if model {
            key.push(Span::styled(" ◆", dim()));
        }
        out.extend(help_row(key, vec![Span::raw(v)], key_w, width));
    }
    if section == "Commands"
        || out
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content == " ◆"))
    {
        out.push(Line::default());
        out.push(Line::from(Span::styled(MODEL_LEGEND, dim())));
    }
    out
}

/// Help over the conversation, filtered by what was typed. Returns the
/// most it can scroll.
fn help_modal(f: &mut Frame, area: Rect, scroll: usize, query: &str) -> (usize, usize) {
    let modal_w = 86u16.min(area.width.saturating_sub(4)).max(20);
    // A column of padding inside each border.
    let text_w = modal_w.saturating_sub(4).max(12) as usize;
    let lines = help_lines(text_w, query);
    let all = help_lines(text_w, "").len();
    // The box keeps its height while filtering.
    let height = (all as u16 + 3).min(area.height.saturating_sub(2)).max(5);
    let visible = height.saturating_sub(3) as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = scroll.min(max_scroll);
    let rect = centered(area, modal_w, height);
    let title_w = rect.width.saturating_sub(2) as usize;
    let counter = if max_scroll > 0 {
        format!("line {} of {} · ", scroll + 1, lines.len())
    } else {
        String::new()
    };
    // q closes only while nothing is typed (then it filters).
    let close = if query.is_empty() {
        "Esc F1 q close"
    } else {
        "Esc F1 close"
    };
    let hint = format!(" {counter}↑↓ PgUp PgDn scroll · {close} ");
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(dim())
        .title(Span::styled(
            fit(" Help ", title_w),
            Style::default().add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(fit(&hint, title_w), dim()));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let pad = |r: Rect| Rect {
        x: r.x + 1,
        width: r.width.saturating_sub(2),
        ..r
    };
    let filter = Rect {
        height: 1,
        ..pad(inner)
    };
    let body = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..pad(inner)
    };
    let filter_line = if query.is_empty() {
        Line::from(Span::styled("type to filter…", dim()))
    } else {
        Line::from(vec![
            Span::styled("Filter: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(query.to_string(), Style::default().fg(Color::Cyan)),
            Span::styled("▏", dim()),
        ])
    };
    f.render_widget(Paragraph::new(filter_line), filter);
    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), body);
    (max_scroll, visible)
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
            "Joining links this computer to {name}'s server with your email. That turns on the shared gate: the check that keeps a candidate's answer from slipping out while you judge it. Without joining, Pathfinder still runs on your own model account, and the gate needs a decision key of your own (/login, then the Gate row)."
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
            push_wrapped(
                &mut lines,
                &[Span::styled("Starting the sign-in…", dim())],
                0,
            );
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
            push_wrapped(
                &mut lines,
                &[Span::styled("Waiting for you there…", dim())],
                0,
            );
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
            &[Span::styled(e.to_string(), Style::default().fg(Color::Red))],
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

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn no_yellow(lines: &[Line]) {
        for l in lines {
            for s in &l.spans {
                assert_ne!(s.style.fg, Some(Color::Yellow), "{:?} is yellow", s.content);
            }
        }
    }

    #[test]
    fn in_select_mode_a_source_draws_without_its_gutter() {
        let long = "x".repeat(100);
        let body = format!(
            "```rust numbered\n   20  fn a() {{}}\n   21  {long}\n```\n22–391 not shown · `/open a.rs:22-81`"
        );
        let lines = source_block_as("a.rs", "lines 20–21 of 391", &body, 60, true);
        let text = texts(&lines);
        assert_eq!(text[1], "  fn a() {}");
        assert!(
            text.iter().all(|l| !l.contains('┆') && !l.contains("└─")),
            "{text:#?}"
        );
        assert!(
            text.iter()
                .all(|l| !l.contains(" 20 ") && !l.contains(" 21 ")),
            "{text:#?}"
        );
        assert!(
            text.last().unwrap().contains("22–391 not shown"),
            "{text:#?}"
        );
    }

    #[test]
    fn a_source_reads_as_a_source() {
        let body = "```rust numbered\n   20      name: \"probe\",\n   21  },\n```\n22–391 not shown · `/open app/src/runtime/gate.rs:22-81`";
        let lines = source_block("app/src/runtime/gate.rs", "lines 20–21 of 391", body, 80);
        assert_eq!(
            texts(&lines),
            vec![
                "▤ app/src/runtime/gate.rs · lines 20–21 of 391",
                "     20      name: \"probe\",",
                "     21  },",
                "  └─ 22–391 not shown · /open app/src/runtime/gate.rs:22-81",
            ]
        );
        let span = |text: &str| {
            lines[0]
                .spans
                .iter()
                .find(|s| s.content.contains(text))
                .unwrap()
                .style
        };
        assert_eq!(span("▤").fg, Some(Color::Cyan));
        assert!(span("gate.rs").add_modifier.contains(Modifier::BOLD));
        assert!(span("391").add_modifier.contains(Modifier::DIM));
        no_yellow(&lines);
    }

    #[test]
    fn search_hits_group_by_file_under_a_count() {
        use crate::sources::{Hit, Locator};
        let hit = |file: &str, line: usize, quote: &str| Hit {
            at: Locator::Local {
                file: file.into(),
                page: None,
                line,
            },
            quote: quote.into(),
        };
        let hits = vec![
            hit("view.rs", 861, "    for l in wrap_hanging(&row, w, 2) {"),
            hit(
                "markdown.rs",
                106,
                "out.extend(wrap_hanging(&spans, width, 2));",
            ),
            hit("view.rs", 368, "markdown::wrap_hanging(&spans, width, 2)"),
        ];
        let lines = search_block("wrap_hanging", &hits, 31, 100);
        assert_eq!(
            texts(&lines),
            vec![
                "⌕ wrap_hanging · first 3 of 31 matches · add a word to narrow",
                "  view.rs",
                "    368  markdown::wrap_hanging(&spans, width, 2)",
                "    861  for l in wrap_hanging(&row, w, 2) {",
                "  markdown.rs",
                "    106  out.extend(wrap_hanging(&spans, width, 2));",
            ]
        );
        let style_of = |l: &Line, text: &str| {
            l.spans
                .iter()
                .find(|s| s.content.contains(text))
                .unwrap()
                .style
        };
        assert!(
            style_of(&lines[1], "view.rs")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            style_of(&lines[2], "368")
                .add_modifier
                .contains(Modifier::DIM)
        );
        no_yellow(&lines);
        // All of them shown: no "first".
        let all = texts(&search_block("x", &hits[..1], 1, 100));
        assert_eq!(all[0], "⌕ x · 1 match");
    }

    #[test]
    fn info_rows_align_their_keys_and_notes_are_not_yellow() {
        let rows = vec![
            ("Version".to_string(), "Pathfinder 1 · macos".to_string()),
            (
                "Accounts".to_string(),
                "Models\n  ChatGPT  signed in".to_string(),
            ),
        ];
        let lines = info_block(&rows, 80);
        assert_eq!(
            texts(&lines),
            vec![
                "· Version   Pathfinder 1 · macos",
                "  Accounts  Models",
                "              ChatGPT  signed in",
            ]
        );
        no_yellow(&lines);
        let note = note_lines("No file `x.rs` in this workspace.", 80);
        assert_eq!(texts(&note), vec!["· No file x.rs in this workspace."]);
        assert!(note[0].spans[0].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(note[0].spans[1].style.fg, None);
        no_yellow(&note);
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
    fn the_palette_sits_under_the_composer_between_two_rules() {
        let mut app = test_app();
        app.input = "/".into();
        app.cursor = 1;
        for (w, h) in [(120u16, 36u16), (100, 30), (80, 24)] {
            let (screen, cursor) = render_to_string(&mut app, w, h);
            let rows: Vec<&str> = screen.lines().collect();
            let input = rows.iter().position(|r| r.starts_with("› /")).unwrap();
            assert!(rows[input - 1].starts_with("── "), "{screen}");
            assert!(
                rows[input + 1].trim_end().chars().all(|c| c == '─'),
                "{screen}"
            );
            // Bare `/`: sections, what the thread asks for first.
            assert_eq!(rows[input + 2].trim_end(), "  Next", "{screen}");
            assert!(rows[input + 3].starts_with("→   find "), "{screen}");
            assert!(
                rows[input + 3].contains("<terms> — search files"),
                "{screen}"
            );
            assert!(rows[input + 4].starts_with("    plan ◆ "), "{screen}");
            // At most eight rows and the counter, then the footer.
            let counter = rows.iter().position(|r| r.contains("(1/")).unwrap();
            assert!(counter - (input + 1) <= 9, "{screen}");
            assert!(rows[counter].contains("◆ calls the model"), "{screen}");
            assert_eq!(counter + 1, h as usize - 1, "{screen}");
            assert!(rows[h as usize - 1].contains("Enter fill in"), "{screen}");
            assert!(
                !screen.contains("[local]") && !screen.contains("[task]"),
                "{screen}"
            );
            assert_eq!(cursor.1 as usize, input, "{screen}");
        }

        // Only the selected row is coloured; no reverse bar.
        let menu = palette_lines(&app, &active_palette(&app).unwrap(), 99, 9);
        for (i, l) in menu.iter().enumerate() {
            let cyan = l.spans.iter().any(|s| s.style.fg == Some(Color::Cyan));
            assert_eq!(cyan, i == 1, "{:?}", line_text(l));
            assert!(
                l.spans
                    .iter()
                    .all(|s| !s.style.add_modifier.contains(Modifier::REVERSED))
            );
        }

        // Typed: ranked, no headers; the counter follows the selection.
        app.input = "/sear".into();
        app.cursor = app.input.len();
        app.cmd_sel = 1;
        let lines: Vec<String> = palette_lines(&app, &active_palette(&app).unwrap(), 99, 9)
            .iter()
            .map(line_text)
            .collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].starts_with("  find "), "{lines:?}");
        assert!(lines[1].starts_with("→ web ◆ "), "{lines:?}");
        assert_eq!(lines[2], "  (2/2) · ◆ calls the model");
    }

    #[test]
    fn a_fresh_thread_shows_the_loop_in_the_middle() {
        let mut app = test_app();
        let card = empty_card(&app, 118, 30);
        let text: Vec<String> = card.iter().map(line_text).collect();
        let first = text.iter().position(|l| !l.is_empty()).unwrap();
        // Centred: as many blank rows above as below, give or take one.
        let below = 30 - text.len();
        assert!(first.abs_diff(below) <= 1, "{first} {below}");
        let headline = &text[first];
        assert!(
            headline
                .trim_start()
                .starts_with("Understand /tmp/pathfinder-view-test by working through it"),
            "{headline}"
        );
        let left = headline.len() - headline.trim_start().len();
        assert!(left > 20, "{headline}");
        let all = text.join("\n");
        for want in [
            "ask          type a question; you get sources, not answers",
            "/open f      read a source        /find terms  search files",
            "/attach f    add your notes or a photo of your work",
            "/plan goal   map the parts to understand",
            "say when it clicks: you'll judge a short candidate",
            "F1 keys · / commands",
        ] {
            assert!(all.contains(want), "{want}\n{all}");
        }
        assert!(!all.contains("earlier threads"), "{all}");
        app.earlier_threads = 3;
        let all: Vec<String> = empty_card(&app, 118, 30).iter().map(line_text).collect();
        assert!(all.join("\n").contains("^O earlier threads (3) · F1 keys"));
        // The commands are cyan, the rest dim.
        let card = empty_card(&app, 118, 30);
        let row = card
            .iter()
            .find(|l| line_text(l).contains("/attach f"))
            .unwrap();
        let attach = row
            .spans
            .iter()
            .find(|s| s.content.contains("/attach"))
            .unwrap();
        assert_eq!(attach.style.fg, Some(Color::Cyan));
        let what = row
            .spans
            .iter()
            .find(|s| s.content.contains("notes"))
            .unwrap();
        assert!(what.style.add_modifier.contains(Modifier::DIM));

        // Narrow: /open and /find take a row each, and nothing runs over.
        for w in [40u16, 58] {
            let card = empty_card(&app, w, 20);
            assert!(card.iter().all(|l| width_of(l) <= w as usize), "{w}");
            let text: Vec<String> = card.iter().map(line_text).collect();
            assert!(
                text.iter()
                    .any(|l| l.trim_start().starts_with("/find terms")),
                "{text:?}"
            );
        }
        // The card goes with the first entry.
        app.entries.push(Entry::Note("hi".into()));
        let (screen, _) = render_to_string(&mut app, 100, 30);
        assert!(!screen.contains("by working through it"), "{screen}");
    }

    #[test]
    fn the_placeholder_says_what_comes_next() {
        let dir = std::env::temp_dir().join(format!("pf-ph-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/gate.rs"), "fn gate() {}\n").unwrap();
        let mut app = test_app();
        app.cwd = dir.clone();
        assert_eq!(
            placeholder(&app),
            "Ask, show your work, or type / for commands"
        );
        app.entries.push(Entry::Text(
            "Start with `probe` in `src/gate.rs:20-60`, then `docs/missing.md`.".into(),
        ));
        assert_eq!(placeholder(&app), "Open a source: /open src/gate.rs:20-60");
        // A reply naming no file here: the usual words.
        app.entries
            .push(Entry::Text("Try `cargo test` first.".into()));
        assert_eq!(
            placeholder(&app),
            "Ask, show your work, or type / for commands"
        );
        with_parts(&mut app, true);
        assert_eq!(placeholder(&app), "Explain the current part, or /next");
        // What waits on you comes first.
        app.session.pending_change = Some(MapChange::default());
        assert_eq!(
            placeholder(&app),
            "/accept or /reject the proposed plan change"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn command_palette_renders_even_when_status_is_set() {
        let mut app = test_app();
        app.status = "Ctrl+O opens an earlier thread in this folder.".into();
        app.input = "/re".into();
        app.cursor = app.input.len();
        app.cmd_sel = 1;

        let (screen, _) = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("(2/3)"), "{screen}");
        assert!(screen.contains("  resume"), "{screen}");
        assert!(screen.contains("→ reject"), "{screen}");
        assert!(screen.contains("  replan"), "{screen}");
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
        // Footer is row 19 and the hairline 18; input spans rows
        // (18 - lines.len())..18.
        let expected_y = 18 - (lines.len() as u16) + row;
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
                .map(|i| {
                    Part::new(
                        format!("p{i}"),
                        format!("Part {i} title"),
                        String::new(),
                        vec![],
                    )
                })
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

        app.modal = Some(Modal::help());
        let (help_narrow, _) = render_to_string(&mut app, 60, 24);
        assert!(help_narrow.contains(" Help "), "{help_narrow}");
        assert!(help_narrow.contains("How it works"), "{help_narrow}");
        assert!(app.help_max_scroll > 0);

        if let Some(Modal::Help { scroll, .. }) = &mut app.modal {
            *scroll = app.help_max_scroll;
        }
        let (help_scrolled, _) = render_to_string(&mut app, 60, 24);
        assert!(help_scrolled.contains("^D ^D"), "{help_scrolled}");
        assert!(help_scrolled.contains("⌥Y ⌥N"), "{help_scrolled}");
        assert!(
            help_scrolled.contains("take back a claim"),
            "{help_scrolled}"
        );
    }

    #[test]
    fn login_is_a_bottom_panel_that_fits_and_never_draws_a_key() {
        use crate::auth::KeySource;
        use crate::auth::accounts::{Row, RowView, State};
        use crate::auth::flow::{Flow, Mode};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let views = vec![
            RowView::new(
                Row::Chatgpt,
                State::SignedIn {
                    email: Some("me@example.com".into()),
                },
            ),
            RowView::new(Row::ClaudeCode, State::Installed),
            RowView::new(Row::AnthropicApi, State::NotSet),
            RowView::new(
                Row::OpenaiApi,
                State::Key {
                    hint: "…zQAA".into(),
                    source: KeySource::File,
                },
            ),
            RowView::new(Row::Advanced, State::Action),
            RowView::new(Row::Gate, State::NotSet),
        ];
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        for width in [80u16, 100, 120] {
            let mut app = test_app();
            app.modal = Some(Modal::Login(Box::new(Flow::login(views.clone()))));
            let (screen, cursor) = render_to_string(&mut app, width, 30);
            let rows: Vec<&str> = screen.lines().collect();
            // The conversation stays in view above; the panel sits where the
            // input was, above the footer.
            assert!(screen.contains("by working through it"), "{screen}");
            let title = rows
                .iter()
                .position(|l| l.contains("Select how to connect:"))
                .unwrap();
            assert!(title > 15, "at the bottom: {title}\n{screen}");
            let selected = rows.iter().find(|l| l.contains("→ ")).unwrap();
            assert!(
                selected.contains("Sign in with your ChatGPT plan"),
                "{selected}"
            );
            assert!(
                selected.contains("✓ signed in as me@example.com"),
                "{selected}"
            );
            for want in [
                "Use an API key · pay per use",
                "Add your own server · advanced",
                "↑↓ navigate · Enter select · Esc cancel",
            ] {
                assert!(screen.contains(want), "{want}\n{screen}");
            }
            for gone in ["Claude Code", "TypeSafe", "Gate", "auth file", "› Ask"] {
                assert!(!screen.contains(gone), "{gone}\n{screen}");
            }
            let _ = cursor;

            // Step two: a filter line, short states, nothing cut.
            app.on_key(key(KeyCode::Down));
            app.on_key(key(KeyCode::Enter));
            for c in "ope".chars() {
                app.on_key(key(KeyCode::Char(c)));
            }
            let (screen, cursor) = render_to_string(&mut app, width, 30);
            let rows: Vec<&str> = screen.lines().collect();
            let filter = rows.iter().position(|l| l.starts_with(" > ope")).unwrap();
            assert_eq!(cursor, (6, filter as u16), "{screen}");
            assert!(screen.contains("Select a provider:"), "{screen}");
            assert!(screen.contains("→ OpenAI"), "{screen}");
            assert!(screen.contains("✓ stored"), "{screen}");
            assert!(!screen.contains('…'), "{screen}");
            assert!(!screen.contains("Anthropic"), "filtered out\n{screen}");
            assert!(screen.contains("type to filter"), "{screen}");
            assert!(screen.contains("Esc clears"), "{screen}");
            // Esc clears the filter, walks back, then cancels.
            app.on_key(key(KeyCode::Esc));
            app.on_key(key(KeyCode::Esc));
            app.on_key(key(KeyCode::Esc));
            assert!(app.modal.is_none());
            assert_eq!(app.status, "Sign-in cancelled");
        }

        let mut app = test_app();
        let (flow, _) = Flow::at(Mode::Login, views, &Row::AnthropicApi);
        app.modal = Some(Modal::Login(Box::new(flow)));
        app.on_paste("sk-ant-api03-TOPSECRET-abcdefgh");
        let (screen, _) = render_to_string(&mut app, 110, 30);
        assert!(!screen.contains("TOPSECRET"), "{screen}");
        assert!(!screen.contains("sk-ant"), "{screen}");
        assert!(screen.contains("> ••••"), "{screen}");
        assert!(screen.contains("Anthropic API key:"), "{screen}");
        // Esc at the first step closes, and nothing is saved.
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.modal.is_none());
        assert_eq!(app.status, "Sign-in cancelled");
    }

    #[test]
    fn settings_show_the_gate_key_next_to_the_decision_service() {
        let mut app = test_app();
        app.modal = Some(Modal::Settings { sel: 0 });
        let (screen, _) = render_to_string(&mut app, 100, 24);
        let rows: Vec<&str> = screen.lines().collect();
        let service = rows
            .iter()
            .position(|l| l.contains("Decision service"))
            .unwrap();
        assert!(rows[service + 1].contains("Gate key"), "{screen}");
        let state = rows[service + 1];
        assert!(
            state.contains("not set") || state.contains("✓ stored") || state.contains("✓ from $"),
            "{state}"
        );
    }

    #[test]
    fn model_picker_shows_account_headers_that_are_never_selected() {
        let mut app = test_app();
        let items: Vec<(String, String)> = [
            ("", "ChatGPT sign-in · ChatGPT plan"),
            ("chatgpt:gpt-6-luna", "GPT-6 Luna"),
            ("", "Claude Code · Claude subscription"),
            ("claude-haiku", "Haiku"),
            ("claude-sonnet", "Sonnet  ✓"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        app.modal = Some(Modal::Choose {
            option: crate::options::MAIN_MODEL,
            title: "Main model".into(),
            items: items.clone(),
            query: String::new(),
            sel: 2,
        });
        let (screen, _) = render_to_string(&mut app, 100, 24);
        assert!(screen.contains("ChatGPT plan"), "{screen}");
        assert!(
            screen.contains("Claude Code · Claude subscription"),
            "{screen}"
        );
        assert!(
            screen.contains("→ ") && screen.contains("Sonnet  ✓"),
            "{screen}"
        );
        assert!(
            !screen.contains("› Haiku") && !screen.contains("│›"),
            "{screen}"
        );
        let selected = screen.lines().find(|l| l.contains("→ ")).unwrap();
        assert!(selected.contains("Sonnet"), "{selected}");
        assert!(screen.contains("(3)"), "{screen}");

        // Filtering keeps only the sections with a match.
        app.modal = Some(Modal::Choose {
            option: crate::options::MAIN_MODEL,
            title: "Main model".into(),
            items,
            query: "luna".into(),
            sel: 0,
        });
        let (filtered, _) = render_to_string(&mut app, 100, 24);
        assert!(filtered.contains("ChatGPT plan"), "{filtered}");
        assert!(!filtered.contains("Claude Code"), "{filtered}");
        assert!(filtered.contains("(1 of 3)"), "{filtered}");
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
        assert!(choose_screen.contains("(1 of 2)"), "{choose_screen}");
        assert!(choose_screen.contains("SELECT · F2"), "{choose_screen}");

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
        assert!(attach_screen.contains("→ paper.pdf"), "{attach_screen}");
        assert!(attach_screen.contains("Enter attach"), "{attach_screen}");
        // /open and /check share the menu, titled by the command.
        std::fs::write(dir.join("parts.md"), "# P\n").unwrap();
        app.input = "/open pa".into();
        app.cursor = app.input.len();
        let (open_screen, _) = render_to_string(&mut app, 90, 24);
        assert!(open_screen.contains("Enter open"), "{open_screen}");
        assert!(open_screen.contains("parts.md"), "{open_screen}");
        app.input = "/check pa".into();
        app.cursor = app.input.len();
        let (check_screen, _) = render_to_string(&mut app, 90, 24);
        assert!(check_screen.contains("Enter pick"), "{check_screen}");
        // Closed with Esc, it stays closed until the input changes.
        app.palette_dismissed = Some(app.input.clone());
        let (closed, _) = render_to_string(&mut app, 90, 24);
        assert!(!closed.contains("parts.md"), "{closed}");

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

    fn width_of(line: &Line) -> usize {
        line.spans.iter().map(|s| s.content.width()).sum()
    }

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn with_parts(app: &mut App, goal: bool) {
        app.session.mode = SessionMode::Working;
        if goal {
            app.session.goal = Some("How the gate decides what to withhold".into());
        }
        app.session.map = Map {
            parts: vec![
                Part {
                    status: PartStatus::Understood,
                    ..Part::new(
                        "p1".into(),
                        "Candidate checks".into(),
                        String::new(),
                        vec![],
                    )
                },
                Part::new(
                    "p2".into(),
                    "The verdict prompt".into(),
                    String::new(),
                    vec![],
                ),
            ],
        };
        app.session.active = Some("p2".into());
    }

    #[test]
    fn the_rule_carries_status_scroll_and_tray_and_always_fits() {
        let mut app = test_app();
        let idle = rule_line(&app, 80, false);
        assert_eq!(width_of(&idle), 80);
        let text = line_text(&idle);
        assert!(
            text.starts_with("── pathfinder-view-test · chat "),
            "{text}"
        );
        assert!(text.ends_with('─'), "{text}");

        // Running: spinner, time and how to stop it, at the right.
        app.running = Some(tokio_util::sync::CancellationToken::new());
        app.started = Some(std::time::Instant::now());
        app.scroll = 40;
        let busy = line_text(&rule_line(&app, 120, false));
        assert!(busy.contains("↑ 40 lines up · End ↓"), "{busy}");
        assert!(busy.ends_with("⠋ Working 0s · Esc stops ──"), "{busy}");

        // Not docked: the parts and what waits ride on the rule.
        with_parts(&mut app, false);
        app.session.pending_candidate = Some(PendingCandidate {
            part_id: "p2".into(),
            candidate: "x".into(),
            key_correct: false,
            key_reason: String::new(),
            secret: None,
            targets: vec![],
            misconception: None,
            id: "c1".into(),
        });
        let line = rule_line(&app, 160, false);
        let text = line_text(&line);
        assert!(
            text.contains("Parts 1/2 » The verdict prompt · ^B"),
            "{text}"
        );
        let wait = line
            .spans
            .iter()
            .find(|s| s.content.contains("your verdict"))
            .unwrap();
        assert_eq!(wait.style.fg, Some(Color::Yellow));
        // Docked, the panel shows them instead.
        let docked = line_text(&rule_line(&app, 160, true));
        assert!(!docked.contains("Parts"), "{docked}");
        assert!(!docked.contains("verdict"), "{docked}");

        // Narrow: the parts and the scroll chip go first; what runs and what
        // waits stay; nothing runs past the edge.
        for w in [30u16, 50, 60, 80, 100] {
            let line = rule_line(&app, w, false);
            assert!(width_of(&line) <= w as usize, "{w}: {}", line_text(&line));
            let text = line_text(&line);
            assert!(text.contains("Working") || w < 50, "{w}: {text}");
        }
        // Every width, the narrowest too, is filled exactly.
        for w in 1u16..=40 {
            let line = rule_line(&app, w, false);
            assert_eq!(width_of(&line), w as usize, "{w}: {}", line_text(&line));
        }
        let text = line_text(&rule_line(&app, 80, false));
        assert!(text.contains("⧗ your verdict"), "{text}");
        assert!(text.contains("Working"), "{text}");
        assert!(!text.contains("Parts"), "{text}");
    }

    #[test]
    fn opening_the_palette_covers_the_empty_card_instead_of_moving_it() {
        let mut app = test_app();
        for (w, h) in [(120u16, 36u16), (80, 24)] {
            let headline = |app: &mut App| {
                let (screen, _) = render_to_string(app, w, h);
                screen
                    .lines()
                    .position(|l| l.contains("Understand "))
                    .unwrap_or_else(|| panic!("{screen}"))
            };
            let models = |app: &mut App| {
                let (screen, _) = render_to_string(app, w, h);
                let footer = screen.lines().last().unwrap().to_string();
                footer[footer.find("fast ").or(footer.find("main ")).unwrap_or(0)..]
                    .trim_end()
                    .to_string()
            };
            app.input.clear();
            app.cursor = 0;
            let idle = headline(&mut app);
            let idle_models = models(&mut app);
            for typed in ["/", "/o", "/a", "/he"] {
                app.input = typed.into();
                app.cursor = typed.len();
                app.cmd_sel = 0;
                assert_eq!(headline(&mut app), idle, "{w}x{h} {typed}");
                // The footer's right side holds still while you type.
                assert_eq!(models(&mut app), idle_models, "{w}x{h} {typed}");
            }
            // Moving in the bare-`/` palette keeps the menu's height.
            app.input = "/".into();
            app.cursor = 1;
            let rule_at = |app: &mut App| {
                let (screen, _) = render_to_string(app, w, h);
                screen.lines().position(|l| l.starts_with("› /")).unwrap()
            };
            app.cmd_sel = 0;
            let first = rule_at(&mut app);
            for sel in 1..12 {
                app.cmd_sel = sel;
                assert_eq!(rule_at(&mut app), first, "{w}x{h} row {sel}");
            }
        }
    }

    #[test]
    fn an_unknown_command_says_so_while_you_type() {
        let mut app = test_app();
        app.input = "/stauts".into();
        app.cursor = app.input.len();
        let (screen, _) = render_to_string(&mut app, 100, 24);
        let footer = screen.lines().last().unwrap();
        assert!(
            footer.contains("No such command · Tab → /status"),
            "{footer}"
        );

        // A loose palette match says Enter only fills it in; a prefix runs.
        for (typed, enter) in [("/ask", "Enter fill in"), ("/stat", "Enter run")] {
            app.input = typed.into();
            app.cursor = typed.len();
            app.cmd_sel = 0;
            let (screen, _) = render_to_string(&mut app, 100, 24);
            let footer = screen.lines().last().unwrap();
            assert!(footer.contains(enter), "{typed}: {footer}");
        }
    }

    #[test]
    fn help_keeps_its_key_column_while_filtering() {
        let all = texts(&help_lines(80, ""));
        let filtered = texts(&help_lines(80, "scroll"));
        let row = |rows: &[String]| {
            rows.iter()
                .find(|l| l.starts_with("⇧↑ ⇧↓ / wheel"))
                .unwrap_or_else(|| panic!("{rows:#?}"))
                .clone()
        };
        let (a, f) = (row(&all), row(&filtered));
        assert_eq!(a, f);
        let col = |l: &str| l.find("a line").unwrap();
        assert!(a["⇧↑ ⇧↓ / wheel".len()..col(&a)].len() >= 2, "{a}");
        let page = all.iter().find(|l| l.starts_with("PgUp PgDn")).unwrap();
        assert_eq!(
            page.find("a page").map(|i| page[..i].chars().count()),
            Some(a[..col(&a)].chars().count())
        );
        assert!(all.iter().any(|l| l.starts_with("say when it clicks")));
    }

    #[test]
    fn the_layout_holds_still_while_a_reply_runs_or_the_view_scrolls() {
        let mut app = test_app();
        for i in 0..60 {
            app.entries.push(Entry::Note(format!("note {i}")));
        }
        app.scroll = 5;
        let (before, _) = render_to_string(&mut app, 100, 30);
        app.running = Some(tokio_util::sync::CancellationToken::new());
        let (after, _) = render_to_string(&mut app, 100, 30);
        let (b, a): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
        // The same rows: the first transcript row and the rule's row.
        assert_eq!(b[0], a[0]);
        assert!(
            b[26].starts_with("──") && a[26].starts_with("──"),
            "{after}"
        );
        assert!(a[26].contains("Working"), "{after}");
        assert!(b[27].starts_with('›') && a[27].starts_with('›'), "{after}");
        assert!(a[28].chars().all(|c| c == '─'), "{after}");
    }

    #[test]
    fn dialogs_float_over_the_conversation_only() {
        let mut app = test_app();
        with_parts(&mut app, true);
        app.modal = Some(Modal::Settings { sel: 0 });
        let (screen, _) = render_to_string(&mut app, 120, 30);
        let rows: Vec<&str> = screen.lines().collect();
        let panel_x = 120 - side_width(120) as usize;
        // The docked panel's border runs the full height, untouched.
        for row in &rows[..26] {
            let c = row.chars().nth(panel_x).unwrap();
            assert_eq!(c, '│', "{screen}");
        }
        assert!(rows[0].contains("Goal"), "{screen}");
        assert!(rows[26].starts_with("──"), "{screen}");
        assert!(rows[27].starts_with('›'), "{screen}");
        assert!(screen.contains("Settings for this thread"), "{screen}");
        // While a dialog is open the footer's left holds no key hints.
        assert!(!rows[29].contains("Enter send"), "{screen}");
    }

    #[test]
    fn the_footer_fits_hints_first_then_models() {
        let mut app = test_app();
        for w in [40u16, 60, 90, 100, 120, 200] {
            let line = footer_line(&app, w, None);
            let text = line_text(&line);
            assert!(width_of(&line) <= w as usize, "{w}: {text}");
            assert!(text.contains("F1 keys"), "{w}: {text}");
        }
        let wide = line_text(&footer_line(&app, 200, None));
        assert!(
            wide.contains("Enter send · ⇧↵ line · ^P settings · ^O threads · F1 keys"),
            "{wide}"
        );
        assert!(wide.contains("fast ") && wide.contains("checks "), "{wide}");
        let narrow = line_text(&footer_line(&app, 40, None));
        assert!(narrow.contains("Enter send · F1 keys"), "{narrow}");

        // Terminal selection: SELECT stays, at every width.
        app.mouse_capture = false;
        for w in [30u16, 60, 120] {
            let line = footer_line(&app, w, None);
            assert!(width_of(&line) <= w as usize);
            assert!(line_text(&line).contains("SELECT · F2"), "{w}");
        }
        app.mouse_capture = true;

        // A short message leaves the right side as it was.
        let right_of = |l: &Line| l.spans.last().unwrap().content.to_string();
        let idle = right_of(&footer_line(&app, 120, None));
        app.status = "Showing thinking.".into();
        assert_eq!(right_of(&footer_line(&app, 120, None)), idle);

        // A status message takes the left whole; the models give way.
        app.status = "Copied the reply (412 chars)".into();
        let line = line_text(&footer_line(&app, 60, None));
        assert!(line.starts_with(" Copied the reply (412 chars)"), "{line}");
        app.status.clear();

        // The palette: the selected command in full.
        app.input = "/op".into();
        app.cursor = 3;
        let kind = active_palette(&app).unwrap();
        let line = line_text(&footer_line(&app, 120, Some(&kind)));
        assert!(line.contains("/open"), "{line}");
        assert!(line.contains(" · "), "{line}");
        assert!(!line.contains("Enter send"), "{line}");
    }

    #[test]
    fn a_status_message_goes_after_four_seconds() {
        let mut app = test_app();
        let t0 = std::time::Instant::now();
        app.status = "Copied the reply.".into();
        app.on_tick(t0);
        app.on_tick(t0 + std::time::Duration::from_secs(3));
        assert_eq!(app.status, "Copied the reply.");
        // A new message starts its own four seconds.
        app.status = "Thread opened.".into();
        app.on_tick(t0 + std::time::Duration::from_secs(3));
        app.on_tick(t0 + std::time::Duration::from_secs(6));
        assert_eq!(app.status, "Thread opened.");
        app.on_tick(t0 + std::time::Duration::from_secs(7));
        assert!(app.status.is_empty());
        // No spinner while idle.
        assert_eq!(app.tick, 0);
    }

    #[test]
    fn the_panel_docks_only_when_it_has_something_to_show() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = test_app();
        assert!(!panel_earns(&app));
        let (screen, _) = render_to_string(&mut app, 200, 30);
        assert!(!screen.contains('│'), "{screen}");
        assert_eq!(app.side_area, Rect::default());

        // ^B in a wide window with nothing to dock: the framed overlay.
        let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        app.on_key(ctrl_b);
        assert!(app.side_overlay);
        let (screen, _) = render_to_string(&mut app, 200, 30);
        assert!(screen.contains("┌ Map "), "{screen}");
        assert!(screen.contains("/plan  build the plan now"), "{screen}");
        assert!(screen.contains("Esc closes"), "{screen}");
        assert!(
            screen.contains("Your Understanding Map appears"),
            "{screen}"
        );
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.side_overlay);

        // A goal and parts: it docks, a fifth of the width, and ^B hides it.
        with_parts(&mut app, true);
        assert!(panel_earns(&app));
        let (screen, _) = render_to_string(&mut app, 200, 30);
        assert_eq!(app.side_area.width, side_width(200));
        assert_eq!(side_width(200), 40);
        assert_eq!(side_width(120), 34);
        assert_eq!(side_width(300), 48);
        assert!(!screen.contains("Thread "), "{screen}");
        app.on_key(ctrl_b);
        assert!(!app.show_side && !app.side_overlay);
        let (screen, _) = render_to_string(&mut app, 200, 30);
        assert!(!screen.contains("Goal"), "{screen}");
        app.on_key(ctrl_b);
        assert!(app.show_side);

        // Narrow, ^B frames it over the conversation, with a blank column.
        render_to_string(&mut app, 100, 30);
        app.on_key(ctrl_b);
        assert!(app.side_overlay);
        let (screen, _) = render_to_string(&mut app, 100, 30);
        let rows: Vec<&str> = screen.lines().collect();
        let top = rows.iter().find(|l| l.contains("┌ Map ")).unwrap();
        let x = top.chars().position(|c| c == '┌').unwrap();
        assert_eq!(top.chars().nth(x - 1), Some(' '), "{screen}");
        assert!(screen.contains("Esc closes"), "{screen}");
    }

    #[test]
    fn panel_lines_name_the_goal_count_parts_and_keep_next_steps_to_a_row() {
        let mut app = test_app();
        with_parts(&mut app, true);
        let lines = side_lines(&app, 31);
        let t = texts(&lines);
        assert_eq!(t[0], "Goal");
        assert!(t.iter().all(|l| !l.starts_with("Thread")), "{t:?}");
        let parts = t.iter().position(|l| l.starts_with("Parts")).unwrap();
        assert_eq!(t[parts], format!("Parts{}1/2", " ".repeat(31 - 8)));
        assert_eq!(t[parts + 1], "✓ 1 Candidate checks");
        assert_eq!(t[parts + 2], "▶ 2 The verdict prompt");
        let next = t.iter().position(|l| l == "Next").unwrap();
        let actions = next_actions(&app);
        assert!(actions.len() > 2);
        // One row per step, each within the width.
        assert_eq!(t.len(), next + 1 + actions.len(), "{t:?}");
        for l in &lines[next + 1..] {
            assert!(width_of(l) <= 31, "{:?}", line_text(l));
            assert_eq!(l.spans[0].style.fg, Some(Color::Cyan));
        }
    }

    #[test]
    fn what_waits_for_your_ok_is_shown_with_its_keys_and_never_the_candidate() {
        use crate::domain::{CommitKind, PendingCommitment, Undoable};
        let mut app = test_app();
        with_parts(&mut app, true);
        app.session.map.parts[1].depends_on = vec!["p1".into()];
        app.session.pending_candidate = Some(PendingCandidate {
            part_id: "p2".into(),
            candidate: "CANDTEXT-7f".into(),
            key_correct: false,
            key_reason: "KEYREASON-9c".into(),
            secret: None,
            targets: vec![],
            misconception: None,
            id: "c1".into(),
        });
        app.session.pending_commitment = Some(PendingCommitment {
            kind: CommitKind::Handoff,
            part: "p2".into(),
            text: "please do it".into(),
            files: vec![],
            candidate: None,
        });
        let t = texts(&side_lines(&app, 31));
        let at = t.iter().position(|l| l == "⧗ Waiting for your OK").unwrap();
        assert_eq!(t[at + 1], "Hand The verdict prompt to the");
        assert_eq!(t[at + 3], "Enter runs · Esc cancels");
        // The horizon: part 2 needs part 1.
        assert!(
            t.contains(&"▶ 2 The verdict prompt ← 1".to_string()),
            "{t:?}"
        );
        // Its keys are in the block, not again under Next.
        assert!(!t.iter().any(|l| l.starts_with("/confirm")), "{t:?}");
        assert!(
            t.iter()
                .all(|l| !l.contains("CANDTEXT") && !l.contains("KEYREASON")),
            "{t:?}"
        );
        assert_eq!(
            placeholder(&app),
            "Enter: hand The verdict prompt to the agent · Esc cancels"
        );
        assert_eq!(
            waiting_text(&app),
            Some("⧗ your OK (Enter runs · Esc cancels)")
        );

        // A verdict read from plain words: Esc keeps it as a note.
        app.session.pending_commitment = Some(PendingCommitment {
            kind: CommitKind::Verdict,
            part: "p2".into(),
            text: "it is right".into(),
            files: vec![],
            candidate: None,
        });
        assert_eq!(
            placeholder(&app),
            "Enter: judge your message as your verdict · Esc keeps it as a note"
        );
        let t = texts(&side_lines(&app, 40));
        assert!(
            t.contains(&"Enter runs · Esc keeps it as a note".to_string()),
            "{t:?}"
        );
        assert!(
            t.iter()
                .all(|l| !l.contains("CANDTEXT") && !l.contains("KEYREASON")),
            "{t:?}"
        );

        // A claim from plain words, undoable for one turn only.
        app.session.pending_commitment = None;
        app.session.pending_candidate = None;
        app.session.turn = 4;
        app.session.last_commitment = Some(Undoable {
            part: "p1".into(),
            events: vec![(1, "c".into())],
            turn: 3,
            prev_active: None,
            prev_mode: SessionMode::Working,
            prev_change: None,
            prev_gaps: vec![],
        });
        assert_eq!(placeholder(&app), "Esc takes back your claim, or go on");
        let t = texts(&side_lines(&app, 40));
        assert!(
            t.contains(&"↶ You claimed Candidate checks".to_string()),
            "{t:?}"
        );
        assert!(!t.iter().any(|l| l.starts_with("/undo ")), "{t:?}");
        app.session.turn = 5;
        assert_ne!(placeholder(&app), "Esc takes back your claim, or go on");
        let t = texts(&side_lines(&app, 40));
        assert!(t.iter().all(|l| !l.starts_with('↶')), "{t:?}");
    }

    #[test]
    fn a_waiting_handoff_renders_docked_and_in_the_rule_when_narrow() {
        use crate::domain::{CommitKind, PendingCommitment};
        let mut app = test_app();
        with_parts(&mut app, true);
        app.session.map.parts.push(Part {
            depends_on: vec!["p1".into(), "p2".into()],
            ..Part::new("p3".into(), "The report".into(), String::new(), vec![])
        });
        app.session.pending_commitment = Some(PendingCommitment {
            kind: CommitKind::Delegate,
            part: "p2".into(),
            text: "let the agent do it".into(),
            files: vec![],
            candidate: None,
        });
        app.entries.push(Entry::User("let the agent do it".into()));
        app.entries.push(Entry::Note(
            "Delegate **The verdict prompt** without understanding it? Enter runs · Esc cancels."
                .into(),
        ));
        let (screen, _) = render_to_string(&mut app, 140, 24);
        for want in [
            "⧗ Waiting for your OK",
            "Delegate The verdict prompt?",
            "Enter runs · Esc cancels",
            "○ 3 The report ← 1, 2",
            "Enter: delegate The verdict prompt · Esc cancels",
        ] {
            assert!(screen.contains(want), "{want:?} in\n{screen}");
        }
        let (screen, _) = render_to_string(&mut app, 90, 24);
        assert!(
            screen.contains("⧗ your OK (Enter runs · Esc cancels)"),
            "{screen}"
        );
    }

    #[test]
    fn the_empty_conversation_counts_earlier_threads() {
        let mut app = test_app();
        let (screen, _) = render_to_string(&mut app, 100, 20);
        assert!(!screen.contains("earlier threads"), "{screen}");
        app.earlier_threads = 3;
        let (screen, _) = render_to_string(&mut app, 100, 20);
        assert!(screen.contains("^O earlier threads (3)"), "{screen}");
    }

    #[test]
    fn help_reads_how_it_works_then_commands_by_section_then_keys() {
        let lines = help_lines(60, "");
        let text = texts(&lines);
        let at = |needle: &str| {
            text.iter()
                .position(|l| l == needle)
                .unwrap_or_else(|| panic!("no {needle:?} in {text:#?}"))
        };
        let (how, cmds, keys) = (at("How it works"), at("Commands"), at("Keys"));
        assert!(how < cmds && cmds < keys);
        // Commands under the palette's sections, in its order.
        let read = at("Read & search");
        let settings = at("Settings & help");
        assert!(cmds < read && read < settings && settings < keys);
        let wide = texts(&help_lines(120, ""));
        let row = |name: &str| {
            // The last: `/open f` also leads How it works.
            wide.iter()
                .rev()
                .find(|l| l.starts_with(&format!("{name} ")))
                .unwrap_or_else(|| panic!("no {name} row"))
                .clone()
        };
        assert!(row("/open").contains("<file> [page or line] — show a file"));
        assert!(row("/web").starts_with("/web ◆ "), "{}", row("/web"));
        assert!(!row("/find").contains('◆'));
        assert!(row("/new").contains("(also /clear)"));
        assert!(text.iter().any(|l| l == "◆ calls the model"));
        // Short key labels, under the keys' own headings.
        assert!(at("Compose") > keys && at("When waiting") > at("Scroll"));
        assert!(text.iter().any(|l| l.starts_with("⇧↵ ⌥↵ ^J")));
        // Every row fits, and keys keep to a third of the width.
        for l in &lines {
            assert!(l.width() <= 60, "{l:?}");
        }
        let what_col = text
            .iter()
            .find(|l| l.starts_with("Enter"))
            .unwrap()
            .find("send")
            .unwrap();
        assert!(what_col <= 60 / 3 + 1, "{what_col}");

        // A filter keeps matching rows under their headings.
        let found = texts(&help_lines(60, "thinking"));
        assert!(
            found.iter().any(|l| l.starts_with("/thinking")),
            "{found:#?}"
        );
        assert!(found.iter().any(|l| l.starts_with("^T ^G")), "{found:#?}");
        assert!(!found.iter().any(|l| l.starts_with("/open")));
        assert_eq!(
            texts(&help_lines(60, "zzqx")),
            vec!["Nothing mentions “zzqx”."]
        );
    }

    #[test]
    fn help_is_a_box_that_keeps_its_height_and_counts_lines() {
        let mut app = test_app();
        app.modal = Some(Modal::help());
        let (full, _) = render_to_string(&mut app, 100, 30);
        assert!(full.contains("type to filter…"), "{full}");
        assert!(full.contains("line 1 of"), "{full}");
        assert!(full.contains("Esc F1 q close"), "{full}");
        let top = |screen: &str| screen.lines().position(|l| l.contains(" Help ")).unwrap();
        if let Some(Modal::Help { query, .. }) = &mut app.modal {
            *query = "copy".into();
        }
        let (filtered, _) = render_to_string(&mut app, 100, 30);
        assert_eq!(top(&full), top(&filtered));
        assert!(filtered.contains("Filter: copy"), "{filtered}");
        assert!(filtered.contains("/copy"), "{filtered}");
    }

    #[test]
    fn a_row_note_sits_dim_on_the_right_and_goes_when_tight() {
        let line = with_note("Understand the gate", "task · 12 · 2h", 40);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.width(), 40);
        assert!(text.starts_with("Understand the gate"));
        assert!(text.ends_with("task · 12 · 2h"));
        assert!(line.spans[2].style.add_modifier.contains(Modifier::DIM));
        let cut: String = with_note(&"x".repeat(60), "chat · 1 · now", 40)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(cut.contains('…') && cut.ends_with("now") && cut.width() == 40);
        let tight: String = with_note("Understand the gate", "chat · 1 · now", 20)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!tight.contains("now"), "{tight}");
    }

    #[test]
    fn the_thread_list_shows_titles_with_mode_turns_and_age() {
        let mut app = test_app();
        app.modal = Some(Modal::Threads {
            items: vec![
                ("a".into(), "How the gate decides".into()),
                ("b".into(), "Sources: README.md".into()),
            ],
            meta: vec!["(open) task · 12 · 2h".into(), "chat · 1 · 3d".into()],
            query: String::new(),
            sel: 0,
        });
        let (screen, _) = render_to_string(&mut app, 120, 30);
        let row = |needle: &str| {
            screen
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no {needle} in {screen}"))
                .to_string()
        };
        assert!(row("How the gate decides").contains("(open) task · 12 · 2h"));
        assert!(row("Sources: README.md").contains("chat · 1 · 3d"));
        // Filtering keeps the box where it was.
        let top = |s: &str| {
            s.lines()
                .position(|l| l.contains("Threads in this folder"))
                .unwrap()
        };
        let before = top(&screen);
        if let Some(Modal::Threads { query, .. }) = &mut app.modal {
            *query = "gate".into();
        }
        let (filtered, _) = render_to_string(&mut app, 120, 30);
        assert_eq!(top(&filtered), before);
        assert!(!filtered.contains("README"), "{filtered}");
    }

    #[test]
    fn the_chooser_keeps_its_height_while_filtering() {
        let mut app = test_app();
        let items: Vec<(String, String)> = (0..8)
            .map(|i| (format!("m{i}"), format!("Model {i}")))
            .collect();
        app.modal = Some(Modal::Choose {
            option: crate::options::MAIN_MODEL,
            title: "Main model".into(),
            items,
            query: String::new(),
            sel: 0,
        });
        let rows = |s: &str| {
            let l: Vec<&str> = s.lines().collect();
            (
                l.iter().position(|l| l.contains("Main model")).unwrap(),
                l.iter().rposition(|l| l.contains("Esc back")).unwrap(),
            )
        };
        let (all, _) = render_to_string(&mut app, 100, 30);
        if let Some(Modal::Choose { query, .. }) = &mut app.modal {
            *query = "7".into();
        }
        let (one, _) = render_to_string(&mut app, 100, 30);
        assert_eq!(rows(&all), rows(&one), "{one}");
    }

    #[test]
    fn small_settings_show_their_values_in_place() {
        let app = test_app();
        let line = settings_row(&app, crate::options::CHECKS, "Review checks");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("off · light · normal"), "{text}");
        let current = app.current(crate::options::CHECKS);
        for s in &line.spans {
            if ["off", "light", "normal"].contains(&s.content.as_ref()) {
                assert_eq!(
                    s.style.add_modifier.contains(Modifier::DIM),
                    s.content != current.as_str(),
                    "{s:?}"
                );
            }
        }
    }

    #[test]
    fn selecting_text_draws_code_without_bars_or_frames() {
        let mut app = test_app();
        app.entries.push(Entry::Text(
            "See:\n\n```rust\nfn a() {}\n```\n\n> quoted".into(),
        ));
        let (framed, _) = transcript(&app, 60);
        let framed = texts(&framed);
        assert!(
            framed.iter().any(|l| l.contains("│ fn a() {}")),
            "{framed:?}"
        );
        app.mouse_capture = false;
        let (plain, _) = transcript(&app, 60);
        let plain = texts(&plain);
        assert!(plain.iter().any(|l| l == "  fn a() {}"), "{plain:?}");
        assert!(
            plain
                .iter()
                .all(|l| !l.contains('│') && !l.contains("┌─") && !l.contains("└─")),
            "{plain:?}"
        );
        assert!(plain.iter().any(|l| l.ends_with("quoted")));
    }
}
