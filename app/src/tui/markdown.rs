//! Markdown for the terminal: the subset Pathfinder's replies use
//! (headings, blockquotes, bold, italic, links, inline code, ordered and
//! unordered lists, fenced code blocks, aligned tables), word-wrapped to a
//! width. Mermaid blocks become a list of nodes with what
//! feeds into each.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Styled lines for `text`, wrapped to `width` columns.
pub fn render(text: &str, width: u16, base: Style) -> Vec<Line<'static>> {
    let raw: Vec<&str> = text.lines().collect();
    render_lines(&raw, width, base)
}

thread_local! {
    /// Rendering for terminal text selection: no bars or frames to copy.
    static PLAIN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// As [`render`], or with `plain` without the `│ ` bars and frames of
/// code blocks and quotes, so a terminal drag copies only the text.
pub fn render_as(text: &str, width: u16, base: Style, plain: bool) -> Vec<Line<'static>> {
    let was = PLAIN.with(|p| p.replace(plain));
    let out = render(text, width, base);
    PLAIN.with(|p| p.set(was));
    out
}

fn plain() -> bool {
    PLAIN.with(|p| p.get())
}

thread_local! {
    /// Inside a blockquote: quotes from sources stay verbatim, so their
    /// LaTeX is not rendered as math.
    static VERBATIM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn verbatim() -> bool {
    VERBATIM.with(|v| v.get())
}

fn render_lines(raw: &[&str], width: u16, base: Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let line = raw[i];
        let trimmed = line.trim_start();

        // Fenced code blocks: ```lang or ~~~lang (with >= 3 fence chars).
        if let Some((fence_ch, fence_len, info)) = parse_fence_open(trimmed) {
            i += 1;
            let mut block = Vec::new();
            while i < raw.len() {
                if is_fence_close(raw[i].trim_start(), fence_ch, fence_len) {
                    i += 1;
                    break;
                }
                block.push(raw[i]);
                i += 1;
            }
            let lang = info.split_whitespace().next().unwrap_or("");
            let numbered = info.split_whitespace().any(|w| w == "numbered");
            if numbered {
                // `/open` puts what wasn't shown on the line after the block.
                let footer = raw.get(i).and_then(|l| source_footer(l));
                if footer.is_some() {
                    i += 1;
                }
                render_numbered_block(&block, lang, footer.as_deref(), width, base, &mut out);
            } else if lang == "mermaid" {
                render_mermaid(&block.join("\n"), width, base, &mut out);
            } else {
                render_code_block(&block, lang, width, base, &mut out);
            }
            continue;
        }

        // Display math: `$$` or `\[` opening a line, closed on a later one,
        // as one indented line of Unicode math.
        if !verbatim()
            && let Some(close) = display_math_open(trimmed)
            && let Some(end) = (i + 1..raw.len()).find(|&j| raw[j].contains(close))
        {
            let joined = raw[i..=end].join(" ");
            let body = joined.trim();
            let math = crate::mathtext::delimited(body)
                .filter(|(_, used)| body[*used..].trim().is_empty())
                .map(|(m, _)| m);
            if let Some(math) = math {
                let spans = vec![Span::styled(format!("  {math}"), base)];
                out.extend(wrap_hanging(&spans, width, 2));
                i = end + 1;
                continue;
            }
        }

        // Blockquotes: consecutive `> ...` lines rendered with a `│ ` bar.
        if trimmed.starts_with('>') {
            let lead_spaces = line.width() - trimmed.width();
            let mut quote_lines = Vec::new();
            while i < raw.len() {
                let t = raw[i].trim_start();
                if t.starts_with('>') {
                    quote_lines.push(strip_quote_prefix(t));
                    i += 1;
                } else {
                    break;
                }
            }
            render_blockquote(&quote_lines, lead_spaces, width, base, &mut out);
            continue;
        }

        // Markdown tables: header + separator (`| --- | ... |`) + body rows,
        // or >= 2 consecutive `| ... |` rows.
        if is_table_start(raw, i) {
            let start = i;
            i += 1;
            while i < raw.len() && is_table_row_candidate(raw[i].trim()) {
                i += 1;
            }
            render_table(&raw[start..i], width, base, &mut out);
            continue;
        }

        // Headings: `# ` through `###### `.
        if let Some(h) = heading(trimmed) {
            let style = base.add_modifier(Modifier::BOLD).fg(Color::Cyan);
            out.extend(wrap(&inline(h, style), width));
            i += 1;
            continue;
        }

        // Horizontal rules (`---`, `***`, `___`).
        if is_horizontal_rule(trimmed) {
            let rule_w = (width as usize).clamp(1, 40);
            out.push(Line::from(Span::styled(
                "─".repeat(rule_w),
                base.add_modifier(Modifier::DIM),
            )));
            i += 1;
            continue;
        }

        // Unordered lists (`- ` or `* `).
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let indent = " ".repeat(line.width() - trimmed.width());
            let mut spans = vec![Span::styled(format!("{indent}• "), base)];
            spans.extend(inline(item, base));
            out.extend(wrap_hanging(&spans, width, indent.width() + 2));
            i += 1;
            continue;
        }

        // Ordered lists (`1. `, `2. `, `1) `, etc.).
        if let Some((prefix, item)) = ordered_list_item(trimmed) {
            let indent = " ".repeat(line.width() - trimmed.width());
            let hang = indent.width() + prefix.width();
            let mut spans = vec![Span::styled(format!("{indent}{prefix}"), base)];
            spans.extend(inline(item, base));
            out.extend(wrap_hanging(&spans, width, hang));
            i += 1;
            continue;
        }

        // Orphan table separator row.
        if is_table_separator(trimmed) {
            out.push(Line::from(Span::styled(
                trimmed.to_string(),
                base.add_modifier(Modifier::DIM),
            )));
            i += 1;
            continue;
        }

        let lead = line.width() - trimmed.width();
        out.extend(wrap_hanging(&inline(line, base), width, lead));
        i += 1;
    }
    out
}

/// The closing delimiter of display math opened at the start of `trimmed`
/// and not closed on the same line.
fn display_math_open(trimmed: &str) -> Option<&'static str> {
    let (open, close) = if trimmed.starts_with("$$") {
        ("$$", "$$")
    } else if trimmed.starts_with("\\[") {
        ("\\[", "\\]")
    } else {
        return None;
    };
    (!trimmed[open.len()..].contains(close)).then_some(close)
}

fn parse_fence_open(trimmed: &str) -> Option<(char, usize, &str)> {
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = trimmed.chars().take_while(|&c| c == ch).count();
    if len < 3 {
        return None;
    }
    let info = trimmed[len..].trim();
    // Backtick fences cannot contain backticks in the info string (CommonMark).
    if ch == '`' && info.contains('`') {
        return None;
    }
    Some((ch, len, info))
}

fn is_fence_close(trimmed: &str, ch: char, min_len: usize) -> bool {
    let count = trimmed.chars().take_while(|&c| c == ch).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

fn render_code_block(
    block: &[&str],
    lang: &str,
    width: u16,
    base: Style,
    out: &mut Vec<Line<'static>>,
) {
    let total_w = (width as usize).max(4);
    if plain() {
        // The code alone, every character kept, wrapped as below.
        for raw_line in block {
            let expanded = raw_line.replace('\t', "    ");
            let mut rest = expanded.as_str();
            if rest.is_empty() {
                out.push(Line::default());
            }
            while !rest.is_empty() {
                let (head, tail) = split_row(rest, total_w, false);
                out.push(Line::from(Span::styled(head.to_string(), base)));
                rest = tail;
            }
        }
        return;
    }
    let border = base.fg(Color::DarkGray);
    let lang_style = base.fg(Color::Cyan).add_modifier(Modifier::DIM);
    if !lang.is_empty() {
        let max_lang = total_w.saturating_sub(3);
        let shown_lang = truncate_to_width(lang, max_lang);
        out.push(Line::from(vec![
            Span::styled("┌─ ", border),
            Span::styled(shown_lang, lang_style),
        ]));
    }
    let content_w = total_w.saturating_sub(2).max(1);
    for raw_line in block {
        let expanded = raw_line.replace('\t', "    ");
        if expanded.is_empty() {
            out.push(Line::from(Span::styled("│ ", border)));
            continue;
        }
        // Every character is kept (indent included): rows rejoin to the line.
        let mut rest = expanded.as_str();
        while !rest.is_empty() {
            let (head, tail) = split_row(rest, content_w, false);
            out.push(Line::from(vec![
                Span::styled("│ ", border),
                Span::styled(head.to_string(), base),
            ]));
            rest = tail;
        }
    }
    out.push(Line::from(Span::styled("└─", border)));
}

/// Where a row of `text` ends at `avail` columns: all of it when it fits;
/// else, for prose, after the last space; for code, after the last space or
/// punctuation within 20 columns of the edge; else a hard cut. Never empty.
fn split_row(text: &str, avail: usize, prose: bool) -> (&str, &str) {
    let avail = avail.max(1);
    let mut used = 0usize;
    let mut cut = text.len();
    let mut soft_space: Option<usize> = None;
    let mut soft_mark: Option<usize> = None;
    let mut seen_text = false;
    for (idx, ch) in text.char_indices() {
        let w = ch.width().unwrap_or(0);
        if used + w > avail {
            cut = idx;
            break;
        }
        used += w;
        let end = idx + ch.len_utf8();
        let near_edge = prose || used + 20 >= avail;
        if seen_text && near_edge && ch == ' ' {
            soft_space = Some(end);
        } else if seen_text && near_edge && !prose && is_break_mark(ch) {
            soft_mark = Some(end);
        }
        if !ch.is_whitespace() {
            seen_text = true;
        }
    }
    if cut == text.len() {
        return (text, "");
    }
    // A space beats a mark: `{probe}` and `what's` stay whole.
    let at = soft_space.or(soft_mark).unwrap_or(cut);
    // A character wider than the row still moves forward.
    let at = if at == 0 {
        text.chars().next().map_or(text.len(), char::len_utf8)
    } else {
        at
    };
    text.split_at(at)
}

/// Punctuation a code row may end after (not quotes, brackets that open,
/// or `_` inside names).
fn is_break_mark(ch: char) -> bool {
    matches!(
        ch,
        ',' | ';' | ')' | ']' | '}' | '.' | ':' | '=' | '>' | '|' | '&' | '+' | '-' | '/'
    )
}

/// The `└─` text `/open` leaves after a cut block (`11–40 not shown ·
/// \`/open f:11-70\``), without its backticks.
fn source_footer(line: &str) -> Option<String> {
    let plain = line.trim().replace('`', "");
    plain.contains(" not shown · /open ").then_some(plain)
}

/// A source with its line numbers (`/open`): the numbers in a dim gutter,
/// long lines wrapped under the code with a dim `┆` and the code's own
/// indent, and a `└─` row that says what wasn't shown.
fn render_numbered_block(
    block: &[&str],
    lang: &str,
    footer: Option<&str>,
    width: u16,
    base: Style,
    out: &mut Vec<Line<'static>>,
) {
    let total_w = (width as usize).max(4);
    let gutter_style = base.add_modifier(Modifier::DIM);
    let prose = matches!(lang, "markdown" | "text" | "");
    if plain() {
        // For terminal selection: the code alone, wrapped the same way, so
        // a drag copies no numbers or marks.
        for raw_line in block {
            let expanded = raw_line.replace('\t', "    ");
            let (_, code) = split_gutter(&expanded);
            let fitted = if prose {
                fit_indent(code, total_w)
            } else {
                code.into()
            };
            let mut rest = fitted.as_ref();
            if rest.is_empty() {
                out.push(Line::default());
            }
            while !rest.is_empty() {
                let (head, tail) = split_row(rest, total_w, prose);
                out.push(Line::from(Span::styled(head.to_string(), base)));
                rest = tail;
            }
        }
        if let Some(text) = footer {
            out.extend(wrap(&[Span::styled(text.to_string(), gutter_style)], width));
        }
        return;
    }
    for raw_line in block {
        let expanded = raw_line.replace('\t', "    ");
        let (gutter, code) = split_gutter(&expanded);
        let g = gutter.width();
        let code_w = total_w.saturating_sub(g);
        let fitted = if prose {
            fit_indent(code, code_w)
        } else {
            code.into()
        };
        let code = fitted.as_ref();
        if code_w < 8 {
            // Too narrow for a gutter: the line as it is, cut to fit.
            let mut rest = expanded.as_str();
            while !rest.is_empty() {
                let (head, tail) = split_row(rest, total_w, prose);
                out.push(Line::from(Span::styled(head.to_string(), base)));
                rest = tail;
            }
            continue;
        }
        let (head, mut rest) = split_row(code, code_w, prose);
        out.push(Line::from(vec![
            Span::styled(gutter.to_string(), gutter_style),
            Span::styled(head.to_string(), base),
        ]));
        if rest.is_empty() {
            continue;
        }
        let cont = format!("{}┆  ", " ".repeat(g.saturating_sub(3)));
        let lead = code.len() - code.trim_start().len();
        let indent = code[..lead].width().min(code_w / 2);
        while !rest.is_empty() {
            let trimmed = rest.trim_start();
            if trimmed.is_empty() {
                break;
            }
            let (head, tail) = split_row(trimmed, code_w - indent, prose);
            out.push(Line::from(vec![
                Span::styled(cont.clone(), gutter_style),
                Span::styled(format!("{}{head}", " ".repeat(indent)), base),
            ]));
            rest = tail;
        }
    }
    let mut spans = vec![Span::styled("└─", gutter_style)];
    if let Some(text) = footer {
        match text.split_once("/open ") {
            Some((what, cmd)) => {
                spans.push(Span::styled(format!(" {what}"), gutter_style));
                spans.push(Span::styled(format!("/open {cmd}"), base.fg(Color::Cyan)));
            }
            None => spans.push(Span::styled(format!(" {text}"), gutter_style)),
        }
    }
    out.extend(wrap_hanging(&spans, width, 3));
}

/// A prose line laid out far to the right (a PDF page's label, such as
/// `FuB 1.19`, under 70 columns of `pdftotext -layout` spaces) with less
/// indent, so its first word isn't cut in two at the row's edge: the text
/// keeps to the right as far as it fits.
fn fit_indent(code: &str, avail: usize) -> std::borrow::Cow<'_, str> {
    let text = code.trim_start_matches(' ');
    let lead = code.len() - text.len();
    let whole = text.width();
    let first = text.split_whitespace().next().unwrap_or("").width();
    let lead = if lead == 0 || lead + whole <= avail {
        return code.into();
    } else if whole <= avail {
        // All of it fits on one row: as far right as it can be.
        avail - whole
    } else if lead + first > avail {
        // At least the first word whole on the first row.
        avail.saturating_sub(first)
    } else {
        return code.into();
    };
    format!("{}{text}", " ".repeat(lead)).into()
}

/// `   12  code` → (`   12  `, `code`); a line without a number has no gutter.
pub(crate) fn split_gutter(line: &str) -> (&str, &str) {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    let digits = line[spaces..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    if digits == 0 {
        return ("", line);
    }
    let after = spaces + digits;
    let rest = &line[after..];
    if rest.is_empty() || rest.trim().is_empty() {
        // Up to two more columns, on a char boundary (the rest may be any
        // whitespace, such as U+3000).
        let end = line
            .char_indices()
            .map(|(i, _)| i)
            .find(|&i| i >= after + 2)
            .unwrap_or(line.len());
        return (&line[..end], &line[end..]);
    }
    match rest.strip_prefix("  ") {
        Some(code) => (&line[..after + 2], code),
        None => ("", line),
    }
}

fn strip_quote_prefix(trimmed: &str) -> &str {
    let after = trimmed.strip_prefix('>').unwrap_or(trimmed);
    after.strip_prefix(' ').unwrap_or(after)
}

fn render_blockquote(
    quote_lines: &[&str],
    lead_spaces: usize,
    width: u16,
    base: Style,
    out: &mut Vec<Line<'static>>,
) {
    let bar = if plain() { "  " } else { "│ " };
    let prefix = format!("{}{bar}", " ".repeat(lead_spaces));
    let prefix_w = prefix.width() as u16;
    let bar_style = base.fg(Color::Cyan);
    if width <= prefix_w + 1 {
        for q in quote_lines {
            let text = truncate_to_width(q, (width as usize).saturating_sub(prefix_w as usize));
            out.push(Line::from(vec![
                Span::styled(prefix.clone(), bar_style),
                Span::styled(text, base),
            ]));
        }
        return;
    }
    let inner_w = width.saturating_sub(prefix_w).max(1);
    let was = VERBATIM.with(|v| v.replace(true));
    let inner = render_lines(quote_lines, inner_w, base);
    VERBATIM.with(|v| v.set(was));
    if inner.is_empty() {
        out.push(Line::from(Span::styled(prefix, bar_style)));
    } else {
        for mut l in inner {
            l.spans.insert(0, Span::styled(prefix.clone(), bar_style));
            out.push(l);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

fn is_table_start(raw: &[&str], i: usize) -> bool {
    let trimmed = raw[i].trim();
    if !trimmed.contains('|') || is_table_separator(trimmed) {
        return false;
    }
    let Some(next) = raw.get(i + 1).map(|s| s.trim()) else {
        return false;
    };
    if is_table_separator(next) {
        return true;
    }
    trimmed.starts_with('|')
        && trimmed.ends_with('|')
        && next.starts_with('|')
        && next.ends_with('|')
        && split_table_cells(trimmed).len() >= 2
        && split_table_cells(next).len() >= 2
}

fn is_table_row_candidate(trimmed: &str) -> bool {
    !trimmed.is_empty() && trimmed.contains('|')
}

fn is_table_separator(trimmed: &str) -> bool {
    if !trimmed.contains('|') || !trimmed.contains('-') {
        return false;
    }
    if !trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
        return false;
    }
    let cells = split_table_cells(trimmed);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim();
            let core = c.strip_prefix(':').unwrap_or(c);
            let core = core.strip_suffix(':').unwrap_or(core);
            !core.is_empty() && core.chars().all(|ch| ch == '-')
        })
}

fn split_table_cells(row: &str) -> Vec<String> {
    let mut trimmed = row.trim();
    if let Some(rest) = trimmed.strip_prefix('|') {
        trimmed = rest;
    }
    if let Some(rest) = trimmed.strip_suffix('|')
        && !rest.ends_with('\\')
    {
        trimmed = rest;
    }
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = trimmed.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'|') {
            cur.push('|');
            chars.next();
        } else if ch == '|' {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

fn parse_align(cell: &str) -> Align {
    let c = cell.trim();
    match (c.starts_with(':'), c.ends_with(':')) {
        (true, true) => Align::Center,
        (false, true) => Align::Right,
        _ => Align::Left,
    }
}

fn render_table(rows_raw: &[&str], width: u16, base: Style, out: &mut Vec<Line<'static>>) {
    enum Row {
        Sep,
        Cells(Vec<String>),
    }

    let mut rows = Vec::with_capacity(rows_raw.len());
    let mut aligns: Vec<Align> = Vec::new();
    let mut ncols = 0usize;

    for r in rows_raw {
        let t = r.trim();
        if is_table_separator(t) {
            let sep_cells = split_table_cells(t);
            ncols = ncols.max(sep_cells.len());
            if aligns.is_empty() {
                aligns = sep_cells.iter().map(|c| parse_align(c)).collect();
            }
            rows.push(Row::Sep);
        } else {
            let cells = split_table_cells(t);
            ncols = ncols.max(cells.len());
            rows.push(Row::Cells(cells));
        }
    }
    if ncols == 0 {
        return;
    }
    aligns.resize(ncols, Align::Left);

    let has_sep = rows.iter().any(|r| matches!(r, Row::Sep));
    let mut parsed_rows: Vec<Option<Vec<Vec<Span<'static>>>>> = Vec::with_capacity(rows.len());
    let mut nat_w = vec![1usize; ncols];

    for (idx, row) in rows.iter().enumerate() {
        match row {
            Row::Sep => parsed_rows.push(None),
            Row::Cells(cells) => {
                let is_header = idx == 0 && has_sep;
                let style = if is_header {
                    base.add_modifier(Modifier::BOLD)
                } else {
                    base
                };
                let mut cell_spans = Vec::with_capacity(ncols);
                for (c, max_col) in nat_w.iter_mut().enumerate() {
                    let text = cells.get(c).map(String::as_str).unwrap_or("");
                    let spans = inline(text, style);
                    let w: usize = spans.iter().map(|s| s.content.width()).sum();
                    *max_col = (*max_col).max(w.max(1));
                    cell_spans.push(spans);
                }
                parsed_rows.push(Some(cell_spans));
            }
        }
    }

    let total_width = (width as usize).max(1);
    let sep_w = if total_width >= ncols * 4 { 3 } else { 1 };
    let total_sep = sep_w * ncols.saturating_sub(1);
    let avail = total_width.saturating_sub(total_sep).max(ncols);

    let mut col_w = nat_w;
    while col_w.iter().sum::<usize>() > avail {
        let Some((max_i, _)) = col_w
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > 1)
            .max_by_key(|(_, w)| *w)
        else {
            break;
        };
        col_w[max_i] -= 1;
    }

    let dim = base.add_modifier(Modifier::DIM);
    let col_sep = if sep_w == 3 { " │ " } else { "│" };
    let rule_sep = if sep_w == 3 { "─┼─" } else { "┼" };

    for row in parsed_rows {
        let Some(cells) = row else {
            let line: String = col_w
                .iter()
                .map(|&w| "─".repeat(w))
                .collect::<Vec<_>>()
                .join(rule_sep);
            out.push(Line::from(Span::styled(
                truncate_to_width(&line, total_width),
                dim,
            )));
            continue;
        };

        let wrapped: Vec<Vec<Line<'static>>> = cells
            .into_iter()
            .enumerate()
            .map(|(c, spans)| wrap(&spans, col_w[c] as u16))
            .collect();
        let row_h = wrapped.iter().map(Vec::len).max().unwrap_or(1);

        for sub in 0..row_h {
            let mut line_spans: Vec<Span<'static>> = Vec::new();
            for c in 0..ncols {
                if c > 0 {
                    line_spans.push(Span::styled(col_sep, dim));
                }
                let cell_line = wrapped[c].get(sub);
                let cw: usize = cell_line
                    .map(|l| l.spans.iter().map(|s| s.content.width()).sum())
                    .unwrap_or(0);
                let pad = col_w[c].saturating_sub(cw);
                let (left_pad, right_pad) = match aligns[c] {
                    Align::Left => (0, pad),
                    Align::Right => (pad, 0),
                    Align::Center => (pad / 2, pad - pad / 2),
                };
                if left_pad > 0 {
                    line_spans.push(Span::raw(" ".repeat(left_pad)));
                }
                if let Some(l) = cell_line {
                    line_spans.extend(l.spans.iter().cloned());
                }
                if right_pad > 0 {
                    line_spans.push(Span::raw(" ".repeat(right_pad)));
                }
            }
            out.push(Line::from(line_spans));
        }
    }
}

fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' '))
        .flatten()
}

fn is_horizontal_rule(trimmed: &str) -> bool {
    let mut chars = trimmed.chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let count = 1 + chars.take_while(|&c| c == first).count();
    count >= 3 && trimmed.chars().all(|c| c == first || c.is_whitespace())
}

fn ordered_list_item(trimmed: &str) -> Option<(&str, &str)> {
    let digits = trimmed.bytes().take_while(|b| b.is_ascii_digit()).count();
    if !(1..=9).contains(&digits) {
        return None;
    }
    let after_digits = &trimmed[digits..];
    if after_digits.starts_with(". ") || after_digits.starts_with(") ") {
        let prefix_len = digits + 2;
        Some((&trimmed[..prefix_len], &trimmed[prefix_len..]))
    } else {
        None
    }
}

fn truncate_to_width(s: &str, max_w: usize) -> String {
    if s.width() <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > max_w {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

/// Inline formatting: `` `code` ``, `***bold italic***`, `**bold**`,
/// `*italic*`, `_italic_` (preserving `snake_case`), `[label](url)`, and
/// LaTeX math (`\(…\)`, `$…$`) shown as Unicode outside blockquotes.
pub fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;
    let mut buf = String::new();
    let mut prev_char: Option<char> = None;

    let flush_buf = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), base));
        }
    };

    while !rest.is_empty() {
        // Inline code takes highest precedence.
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            flush_buf(&mut buf, &mut spans);
            let inner = &after[..end];
            spans.push(Span::styled(inner.to_string(), base.fg(Color::Cyan)));
            prev_char = Some('`');
            rest = &after[end + 1..];
            continue;
        }

        // Links: [label](url).
        if rest.starts_with('[')
            && let Some((label, url, consumed)) = parse_link(rest)
        {
            flush_buf(&mut buf, &mut spans);
            spans.push(Span::styled(
                label.to_string(),
                base.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED),
            ));
            if label != url && url != format!("mailto:{label}") && !link_repeats(label, url) {
                spans.push(Span::styled(
                    format!(" ({url})"),
                    base.add_modifier(Modifier::DIM),
                ));
            }
            prev_char = Some(')');
            rest = &rest[consumed..];
            continue;
        }

        // LaTeX math, shown as Unicode (never in a quote from a source).
        if rest.starts_with(['\\', '$'])
            && !verbatim()
            && let Some((math, used)) = crate::mathtext::delimited(rest)
        {
            buf.push_str(&math);
            prev_char = math.chars().last();
            rest = &rest[used..];
            continue;
        }

        // ***bold italic***
        if let Some(after) = rest.strip_prefix("***")
            && !after.starts_with('*')
            && let Some(end) = find_closing_stars(after, 3)
            && end > 0
        {
            flush_buf(&mut buf, &mut spans);
            spans.extend(inline(
                &after[..end],
                base.add_modifier(Modifier::BOLD | Modifier::ITALIC),
            ));
            prev_char = Some('*');
            rest = &after[end + 3..];
            continue;
        }

        // **bold**
        if let Some(after) = rest.strip_prefix("**")
            && !after.starts_with('*')
            && let Some(end) = find_closing_stars(after, 2)
            && end > 0
        {
            flush_buf(&mut buf, &mut spans);
            spans.extend(inline(&after[..end], base.add_modifier(Modifier::BOLD)));
            prev_char = Some('*');
            rest = &after[end + 2..];
            continue;
        }

        // *italic*
        if let Some(after) = rest.strip_prefix('*')
            && !after.starts_with(|c: char| c.is_whitespace() || c == '*')
            && let Some(end) = find_closing_star_single(after)
        {
            flush_buf(&mut buf, &mut spans);
            spans.extend(inline(&after[..end], base.add_modifier(Modifier::ITALIC)));
            prev_char = Some('*');
            rest = &after[end + 1..];
            continue;
        }

        // _italic_ (avoid breaking snake_case_identifiers).
        if let Some(after) = rest.strip_prefix('_')
            && !prev_char.is_some_and(|c| c.is_alphanumeric() || c == '_')
            && !after.starts_with(|c: char| c.is_whitespace() || c == '_')
            && let Some(end) = find_closing_underscore(after)
        {
            flush_buf(&mut buf, &mut spans);
            spans.extend(inline(&after[..end], base.add_modifier(Modifier::ITALIC)));
            prev_char = Some('_');
            rest = &after[end + 1..];
            continue;
        }

        let ch = rest.chars().next().unwrap();
        buf.push(ch);
        prev_char = Some(ch);
        rest = &rest[ch.len_utf8()..];
    }

    if !buf.is_empty() || spans.is_empty() {
        spans.push(Span::styled(buf, base));
    }
    spans
}

/// A relative link whose label already names its target
/// (`[gate.rs:20](app/src/runtime/gate.rs#L20)`): the url adds nothing.
fn link_repeats(label: &str, url: &str) -> bool {
    if url.contains("://") || url.starts_with("mailto:") {
        return false;
    }
    // The path alone, without `#L20`, `:20-24` or a query.
    let path = |s: &str| -> String {
        let s = s.trim().trim_matches('`');
        let s = s.split(['#', '?']).next().unwrap_or(s);
        let s = match s.split_once(':') {
            Some((p, rest)) if rest.chars().all(|c| c.is_ascii_digit() || c == '-') => p,
            _ => s,
        };
        s.trim_start_matches("./").to_string()
    };
    let (l, u) = (path(label), path(url));
    if l.is_empty() || u.is_empty() {
        return false;
    }
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    l == u || u.ends_with(&format!("/{l}")) || base(&l) == base(&u)
}

fn parse_link(text: &str) -> Option<(&str, &str, usize)> {
    let after_open = text.strip_prefix('[')?;
    let close_bracket = after_open.find(']')?;
    let label = &after_open[..close_bracket];
    if label.is_empty() || label.contains('[') || label.contains('\n') {
        return None;
    }
    let after_bracket = &after_open[close_bracket + 1..];
    let after_paren = after_bracket.strip_prefix('(')?;
    let mut depth = 1usize;
    let mut end = None;
    for (i, ch) in after_paren.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            '\n' | ' ' | '\t' => return None,
            _ => {}
        }
    }
    let url_end = end?;
    let url = &after_paren[..url_end];
    if url.is_empty() {
        return None;
    }
    let consumed = 1 + close_bracket + 1 + 1 + url_end + 1;
    Some((label, url, consumed))
}

fn find_closing_stars(after: &str, count: usize) -> Option<usize> {
    let needle = "*".repeat(count);
    let mut pos = 0;
    while let Some(rel) = after[pos..].find(&needle) {
        let idx = pos + rel;
        let prev = after[..idx].chars().next_back();
        if prev.is_some_and(char::is_whitespace) {
            pos = idx + 1;
            continue;
        }
        // If followed by extra `*`, skip past the run unless it's exact `count`.
        let run = after[idx..].chars().take_while(|&c| c == '*').count();
        if run == count {
            return Some(idx);
        }
        pos = idx + run;
    }
    None
}

fn find_closing_star_single(after: &str) -> Option<usize> {
    let mut in_code = false;
    let mut prev: Option<char> = None;
    let mut idx = 0;
    while idx < after.len() {
        let ch = after[idx..].chars().next().unwrap();
        if ch == '`' {
            in_code = !in_code;
        } else if !in_code && ch == '*' {
            let next = after[idx + 1..].chars().next();
            if next == Some('*') {
                // Skip ** run so `*a **b** c*` finds the real single `*`.
                let run = after[idx..].chars().take_while(|&c| c == '*').count();
                idx += run;
                prev = Some('*');
                continue;
            }
            if idx > 0 && !prev.is_some_and(char::is_whitespace) {
                return Some(idx);
            }
        }
        prev = Some(ch);
        idx += ch.len_utf8();
    }
    None
}

fn find_closing_underscore(after: &str) -> Option<usize> {
    let mut in_code = false;
    let mut prev: Option<char> = None;
    let mut idx = 0;
    while idx < after.len() {
        let ch = after[idx..].chars().next().unwrap();
        if ch == '`' {
            in_code = !in_code;
        } else if !in_code && ch == '_' {
            let next = after[idx + 1..].chars().next();
            if idx > 0
                && !prev.is_some_and(char::is_whitespace)
                && !next.is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                return Some(idx);
            }
        }
        prev = Some(ch);
        idx += ch.len_utf8();
    }
    None
}

/// Word-wraps styled spans to `width` columns.
pub fn wrap(spans: &[Span<'static>], width: u16) -> Vec<Line<'static>> {
    wrap_hanging(spans, width, 0)
}

/// Like `wrap`, with continuation lines indented by `hang` columns.
pub fn wrap_hanging(spans: &[Span<'static>], width: u16, hang: usize) -> Vec<Line<'static>> {
    let width = (width as usize).max(1);
    let hang = hang.min(width.saturating_sub(1));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;

    let push_wrap =
        |current: &mut Vec<Span<'static>>, used: &mut usize, lines: &mut Vec<Line<'static>>| {
            lines.push(Line::from(std::mem::take(current)));
            if hang > 0 {
                current.push(Span::raw(" ".repeat(hang)));
            }
            *used = hang;
        };

    for span in spans {
        // Words with the spaces before them, so spacing survives wrapping.
        let mut word = String::new();
        let flush = |word: &mut String,
                     current: &mut Vec<Span<'static>>,
                     used: &mut usize,
                     lines: &mut Vec<Line<'static>>| {
            if word.is_empty() {
                return;
            }
            let w = word.width();
            if *used + w > width && *used > hang {
                push_wrap(current, used, lines);
                let trimmed = word.trim_start().to_string();
                if !trimmed.is_empty() {
                    *used += trimmed.width();
                    current.push(Span::styled(trimmed, span.style));
                }
            } else {
                *used += w;
                current.push(Span::styled(std::mem::take(word), span.style));
            }
            word.clear();
        };

        for ch in span.content.chars() {
            if ch == ' ' && !word.trim().is_empty() {
                flush(&mut word, &mut current, &mut used, &mut lines);
            }
            let ch_w = ch.width().unwrap_or(0);
            if used + word.width() + ch_w > width {
                if used > hang {
                    // Try moving `word` to a fresh continuation line before cutting it.
                    push_wrap(&mut current, &mut used, &mut lines);
                    word = word.trim_start().to_string();
                    if ch == ' ' && word.is_empty() {
                        continue;
                    }
                    if used + word.width() + ch_w > width && !word.is_empty() {
                        current.push(Span::styled(std::mem::take(&mut word), span.style));
                        push_wrap(&mut current, &mut used, &mut lines);
                    }
                } else if !word.is_empty() {
                    // Already at `hang`; cut the overlong word right before `ch`.
                    current.push(Span::styled(std::mem::take(&mut word), span.style));
                    push_wrap(&mut current, &mut used, &mut lines);
                    if ch == ' ' {
                        continue;
                    }
                }
            }
            word.push(ch);
        }
        flush(&mut word, &mut current, &mut used, &mut lines);
    }
    lines.push(Line::from(current));
    lines
}

/// Deterministic input-box wrapping for `text` at `width` display columns.
/// Returns `(visual_lines, cursor_row, cursor_col)` where `visual_lines` is
/// always non-empty, preserves all spaces/newlines, wraps at word boundaries
/// (breaking overlong words cleanly by display width), and matches `cursor_byte`
/// with zero drift.
pub fn wrap_input(text: &str, cursor_byte: usize, width: u16) -> (Vec<String>, u16, u16) {
    let max_w = (width as usize).max(1);
    let mut cursor = cursor_byte.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }

    let mut visual_lines: Vec<String> = Vec::new();
    let mut cursor_pos: Option<(u16, u16)> = None;

    let mut seg_start = 0usize;
    for seg in text.split('\n') {
        let seg_end = seg_start + seg.len();
        let mut seg_lines: Vec<(usize, usize, String)> = Vec::new();

        if seg.is_empty() {
            seg_lines.push((seg_start, seg_end, String::new()));
        } else {
            let mut cur = String::new();
            let mut cur_start = seg_start;
            let mut cur_w = 0usize;

            // Partition `seg` into alternating runs of spaces vs non-spaces.
            let mut pos = 0usize;
            while pos < seg.len() {
                let first = seg[pos..].chars().next().unwrap();
                let is_space = first == ' ';
                let run_rel_start = pos;
                while pos < seg.len() {
                    let ch = seg[pos..].chars().next().unwrap();
                    if (ch == ' ') != is_space {
                        break;
                    }
                    pos += ch.len_utf8();
                }
                let run = &seg[run_rel_start..pos];
                let run_abs_start = seg_start + run_rel_start;
                let run_w = run.width();

                if !is_space && !cur.is_empty() && cur_w + run_w > max_w {
                    seg_lines.push((cur_start, run_abs_start, std::mem::take(&mut cur)));
                    cur_start = run_abs_start;
                    cur_w = 0;
                }

                for (rel_i, ch) in run.char_indices() {
                    let cw = ch.width().unwrap_or(0);
                    if !cur.is_empty() && cur_w + cw > max_w {
                        let abs_i = run_abs_start + rel_i;
                        seg_lines.push((cur_start, abs_i, std::mem::take(&mut cur)));
                        cur_start = abs_i;
                        cur_w = 0;
                    }
                    cur.push(ch);
                    cur_w += cw;
                }
            }
            seg_lines.push((cur_start, seg_end, cur));
        }

        let base_row = visual_lines.len();
        let last_idx = seg_lines.len() - 1;
        for (idx, (v_start, v_end, line_str)) in seg_lines.into_iter().enumerate() {
            if cursor_pos.is_none()
                && cursor >= v_start
                && (cursor < v_end || (idx == last_idx && cursor <= v_end))
            {
                let col = text[v_start..cursor].width();
                cursor_pos = Some(((base_row + idx) as u16, col as u16));
            }
            visual_lines.push(line_str);
        }

        seg_start = seg_end + 1; // skip '\n'
    }

    let (row, col) = cursor_pos.unwrap_or((0, 0));
    (visual_lines, row, col)
}

/// A node of a Mermaid flowchart, as a row: its status mark (`✓`, `▶`,
/// `○`, … when the label starts with one), its title, the rest of the
/// label, and the numbers of the nodes feeding into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowRow {
    pub mark: Option<String>,
    pub title: String,
    pub meta: String,
    pub feeds: Vec<usize>,
}

/// Marks a node label may start with: the part statuses and the active part.
const FLOW_MARKS: [&str; 6] = ["▶", "✓", "○", "⧗", "~", "⇢"];

/// A Mermaid flowchart as rows: each node, numbered in order, then the
/// numbers of the nodes feeding into it.
pub fn mermaid_rows(source: &str) -> Vec<FlowRow> {
    let mut nodes: Vec<(String, String)> = Vec::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    for raw in source.lines() {
        let line = raw.trim();
        if let Some((from, to)) = line.split_once("-->") {
            edges.push((node_id(from), node_id(to)));
            for part in [from, to] {
                if let Some(node) = node_def(part) {
                    upsert(&mut nodes, node);
                }
            }
        } else if let Some(node) = node_def(line) {
            upsert(&mut nodes, node);
        }
    }
    nodes
        .iter()
        .map(|(id, label)| {
            let (mark, label) = match FLOW_MARKS
                .iter()
                .find_map(|m| label.strip_prefix(m).and_then(|r| r.strip_prefix(' ')))
            {
                Some(rest) => (
                    Some(label[..label.len() - rest.len() - 1].to_string()),
                    rest,
                ),
                None => (None, label.as_str()),
            };
            let (title, meta) = label.split_once(" · ").unwrap_or((label, ""));
            let feeds = edges
                .iter()
                .filter(|(_, to)| to == id)
                .filter_map(|(from, _)| nodes.iter().position(|(n, _)| n == from))
                .map(|i| i + 1)
                .collect();
            FlowRow {
                mark,
                title: title.to_string(),
                meta: meta.to_string(),
                feeds,
            }
        })
        .collect()
}

/// A Mermaid flowchart as text: `✓ 1 Title · meta`, then `← 2, 3` for
/// the nodes that feed it.
#[cfg(test)]
pub fn mermaid_to_text(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, r) in mermaid_rows(source).iter().enumerate() {
        let mark = r.mark.as_deref().unwrap_or("·");
        let meta = if r.meta.is_empty() {
            String::new()
        } else {
            format!(" · {}", r.meta)
        };
        out.push(format!("{mark} {} {}{meta}", i + 1, r.title));
        if !r.feeds.is_empty() {
            out.push(format!("    ← {}", join_numbers(&r.feeds)));
        }
    }
    out
}

fn join_numbers(ns: &[usize]) -> String {
    ns.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The flowchart's rows: the mark in its status colour, the number and the
/// meta dim, and continuations under the title.
fn render_mermaid(source: &str, width: u16, base: Style, out: &mut Vec<Line<'static>>) {
    let rows = mermaid_rows(source);
    let num_w = rows.len().to_string().len();
    for (i, r) in rows.iter().enumerate() {
        let mark = r.mark.as_deref().unwrap_or("·");
        let color = match mark {
            "▶" => Some(Color::Cyan),
            "✓" => Some(Color::Green),
            "⧗" => Some(Color::Yellow),
            "⇢" => Some(Color::Magenta),
            _ => None,
        };
        let mark_style = match color {
            Some(c) => base.fg(c),
            None => base.add_modifier(Modifier::DIM),
        };
        let dim = base.add_modifier(Modifier::DIM);
        let mut spans = vec![
            Span::styled(format!("{mark} "), mark_style),
            Span::styled(format!("{:>num_w$} ", i + 1), dim),
        ];
        let title_style = if mark == "▶" {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        };
        spans.extend(inline(&r.title, title_style));
        if !r.meta.is_empty() {
            spans.push(Span::styled(format!(" · {}", r.meta), dim));
        }
        let hang = 2 + num_w + 1;
        out.extend(wrap_hanging(&spans, width, hang));
        if !r.feeds.is_empty() {
            out.extend(wrap_hanging(
                &[Span::styled(
                    format!("{}← {}", " ".repeat(hang), join_numbers(&r.feeds)),
                    dim,
                )],
                width,
                hang + 2,
            ));
        }
    }
}

fn upsert(nodes: &mut Vec<(String, String)>, node: (String, String)) {
    if !nodes.iter().any(|(id, _)| *id == node.0) {
        nodes.push(node);
    }
}

fn node_id(part: &str) -> String {
    part.trim()
        .split(['[', '(', '{'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// `id["Title<br/><small>meta</small>"]` → (id, "Title · meta").
fn node_def(part: &str) -> Option<(String, String)> {
    let part = part.trim();
    let open = part.find('[')?;
    let close = part.rfind(']')?;
    let id = part[..open].trim().to_string();
    let label = part[open + 1..close].trim().trim_matches('"');
    let label = label
        .replace("<br/>", " · ")
        .replace("<small>", "")
        .replace("</small>", "")
        .replace("#quot;", "\"");
    (!id.is_empty()).then_some((id, label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_followed_by_wide_whitespace_does_not_panic() {
        assert_eq!(split_gutter("12\u{3000}"), ("12\u{3000}", ""));
        let lines = render("```text numbered\n12\u{3000}\n```", 40, Style::default());
        assert!(!lines.is_empty());
    }

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn mermaid_becomes_nodes_with_what_feeds_them() {
        let src = "flowchart TD\n    n_a[\"▶ Forward<br/><small>fwd · open</small>\"]\n    n_b[\"Loss<br/><small>loss · open</small>\"]\n    n_a --> n_b";
        assert_eq!(
            mermaid_to_text(src),
            vec![
                "▶ 1 Forward · fwd · open",
                "· 2 Loss · loss · open",
                "    ← 1"
            ]
        );
    }

    #[test]
    fn bold_code_lists_and_wrapping() {
        let lines = render(
            "## Title\n- **P** is `U·I·cos φ` and it is the mean power\n```mermaid\nflowchart LR\n    a[\"A\"]\n    b[\"B\"]\n    a --> b\n```",
            24,
            Style::default(),
        );
        let text = plain(&lines);
        assert_eq!(text[0], "Title");
        assert!(text[1].starts_with("• P is U·I·cos φ"), "{text:?}");
        assert!(text[2].starts_with("  "), "hanging indent: {text:?}");
        assert!(text.contains(&"    ← 1".to_string()), "{text:?}");
        let tree = plain(&render(
            "```mermaid\nflowchart LR\n    a[\"Alpha\"]\n    b[\"Beta gamma delta epsilon zeta eta\"]\n    a --> b\n    b --> a\n```",
            20,
            Style::default(),
        ));
        let at = tree.iter().position(|l| l.starts_with("· 2 Beta")).unwrap();
        assert!(
            tree[at + 1].starts_with("    ") && !tree[at + 1].starts_with("     "),
            "{tree:?}"
        );
        let bold = lines[1]
            .spans
            .iter()
            .find(|s| s.content.trim() == "P")
            .expect("the bold word");
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = lines[1]
            .spans
            .iter()
            .find(|s| s.content.contains("U·I·cos"))
            .unwrap();
        assert_eq!(code.style.fg, Some(Color::Cyan));
        for l in &lines {
            let w: usize = l.spans.iter().map(|s| s.content.width()).sum();
            assert!(w <= 24, "{l:?} is {w} wide");
        }
    }

    #[test]
    fn long_words_are_cut_without_losing_characters() {
        let word = "(?i)asynchron|ASM|Schlupf|Kloss|Kippmoment";
        let lines = wrap_hanging(&[Span::raw(format!("✓ Grep {word}"))], 20, 3);
        let text = plain(&lines);
        for l in &text {
            assert!(l.width() <= 20, "{text:?}");
        }
        let joined: String = text.iter().map(|l| l.trim()).collect();
        assert!(joined.contains(&word.replace(' ', "")), "{text:?}");
    }

    #[test]
    fn blockquotes_render_bar_and_wrap_with_bar() {
        let lines = render(
            "> Verbatim line from source that wraps cleanly across lines.\n>\n> - quoted list item that also wraps",
            28,
            Style::default(),
        );
        let text = plain(&lines);
        assert!(text.len() >= 4, "{text:?}");
        for l in &text {
            assert!(l.starts_with("│ "), "expected left bar: {l:?}");
            assert!(l.width() <= 28, "width overflow: {l:?}");
        }
        assert!(
            lines[0].spans[0].style.fg == Some(Color::Cyan),
            "styled bar"
        );
        let list_cont = text.last().unwrap();
        assert!(
            list_cont.starts_with("│   "),
            "hanging indent inside quote: {text:?}"
        );
    }

    #[test]
    fn ordered_lists_keep_hanging_indent() {
        let lines = render(
            "1. First item that wraps onto another line\n10. Tenth item with wider prefix indent",
            24,
            Style::default(),
        );
        let text = plain(&lines);
        assert!(text[0].starts_with("1. First"), "{text:?}");
        assert!(text[1].starts_with("   "), "hang=3: {text:?}");
        assert!(!text[1].starts_with("    "), "hang=3: {text:?}");
        let tenth = text.iter().position(|l| l.starts_with("10. ")).unwrap();
        assert!(text[tenth + 1].starts_with("    "), "hang=4: {text:?}");
        for l in &text {
            assert!(l.width() <= 24, "{l:?}");
        }
    }

    #[test]
    fn fenced_code_blocks_preserve_indentation_and_hard_break() {
        let src = "```rust\nfn main() {\n    let x = 1 + 2;\n\n    let long_ident = \"abcdefghijklmnopqrstuvwxyz\";\n}\n```";
        let lines = render(src, 24, Style::default());
        let text = plain(&lines);
        assert_eq!(text[0], "┌─ rust");
        assert_eq!(text[1], "│ fn main() {");
        assert_eq!(text[2], "│     let x = 1 + 2;");
        assert_eq!(text[3], "│ ");
        assert!(text[4].starts_with("│     let long_ident"), "{text:?}");
        assert!(text[5].starts_with("│ "), "{text:?}");
        assert_eq!(text[text.len() - 2], "│ }");
        assert_eq!(*text.last().unwrap(), "└─");
        for l in &text {
            assert!(l.width() <= 24, "{l:?} width={}", l.width());
        }
        // Rejoining code after `│ ` preserves exact characters without dropping spaces.
        let rejoined: String = text[4..text.len() - 2]
            .iter()
            .map(|l| l.strip_prefix("│ ").unwrap())
            .collect();
        assert_eq!(
            rejoined,
            "    let long_ident = \"abcdefghijklmnopqrstuvwxyz\";"
        );
    }

    #[test]
    fn code_breaks_after_a_space_or_punctuation_near_the_edge() {
        let src = "```rust\nlet value = compute(alpha, beta, gamma_delta);\n```";
        let text = plain(&render(src, 32, Style::default()));
        // 30 columns of code: the break falls after `, ` rather than mid-word.
        assert_eq!(text[1], "│ let value = compute(alpha, ");
        assert_eq!(text[2], "│ beta, gamma_delta);");
        // Far from the edge, a long word is cut hard.
        let src = "```text\nx abcdefghijklmnopqrstuvwxyzabcdefghij\n```";
        let text = plain(&render(src, 32, Style::default()));
        assert_eq!(text[1], "│ x abcdefghijklmnopqrstuvwxyzab");
    }

    #[test]
    fn numbered_sources_keep_the_gutter_and_wrap_under_the_code() {
        let src = "```rust numbered\n   20          name: name.to_string(),\n   22              \"Candidate {n} is waiting for the user's verdict. Would sending the draft reply below reveal it?\",\n   23\n   24      ),\n```\n25–391 not shown · `/open app/src/runtime/gate.rs:25-84`";
        let lines = render(src, 60, Style::default());
        let text = plain(&lines);
        assert_eq!(text[0], "   20          name: name.to_string(),");
        // The gutter is dim; the code is not.
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(lines[0].spans[0].content, "   20  ");
        assert!(
            text[1].starts_with("   22              \"Candidate"),
            "{text:?}"
        );
        // Continuations: `┆` under the number, then the code's own indent.
        assert!(text[2].starts_with("    ┆              "), "{text:?}");
        assert!(
            !text[2]["    ┆              ".len()..].starts_with(' '),
            "{text:?}"
        );
        for l in &text {
            assert!(l.width() <= 60, "{l:?} is {} wide", l.width());
        }
        // Rows of a line hold all its words, cut only after a space or mark.
        let blank = text.iter().position(|l| l.trim() == "23").unwrap();
        let words: Vec<&str> = text[1..blank]
            .iter()
            .flat_map(|l| l.split_whitespace())
            .filter(|w| *w != "┆" && *w != "22")
            .collect();
        assert_eq!(
            words.join(" "),
            "\"Candidate {n} is waiting for the user's verdict. Would sending the draft reply below reveal it?\","
        );
        assert_eq!(text[blank + 1], "   24      ),");
        assert_eq!(
            text.last().unwrap(),
            "└─ 25–391 not shown · /open app/src/runtime/gate.rs:25-84"
        );
        let last = &lines.last().unwrap().spans;
        let cmd = last.iter().find(|s| s.content.contains("/open")).unwrap();
        assert_eq!(cmd.style.fg, Some(Color::Cyan));
        let note = last.iter().find(|s| s.content.contains("shown")).unwrap();
        assert!(note.style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn a_far_right_page_label_keeps_its_words_whole() {
        let label = format!("{}FuB 1.19", " ".repeat(72));
        let src = format!("```text numbered\n   2  {label}\n```");
        let rows: Vec<String> = render(&src, 60, Style::default())
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(rows.iter().any(|r| r.ends_with("FuB 1.19")), "{rows:?}");
        assert!(rows.iter().all(|r| r.width() <= 60), "{rows:?}");
        assert_eq!(fit_indent("  short", 40), "  short");
        assert_eq!(fit_indent("      FuB 1.19", 10), "  FuB 1.19");
        assert_eq!(
            fit_indent("      a long line of words", 10),
            "      a long line of words"
        );
        assert_eq!(
            fit_indent("         Maxwell-Gleichungen", 12),
            "Maxwell-Gleichungen"
        );
    }

    #[test]
    fn numbered_prose_breaks_at_words_and_every_block_closes() {
        let src = "```markdown numbered\n    7  Pathfinder makes sure you understand what you are working on, at the moments that matter.\n    8\n```";
        let text = plain(&render(src, 50, Style::default()));
        assert_eq!(text[0], "    7  Pathfinder makes sure you understand what ");
        assert_eq!(text[1], "    ┆  you are working on, at the moments that ");
        assert_eq!(text[2], "    ┆  matter.");
        assert_eq!(text[3].trim_end(), "    8");
        // A block shown whole still gets its closing row.
        assert_eq!(text[4], "└─");
        assert_eq!(text.len(), 5);
        // A line after a block that isn't a footer stays a paragraph.
        let text = plain(&render(
            "```text numbered\n    1  a\n```\nafter",
            40,
            Style::default(),
        ));
        assert_eq!(text, vec!["    1  a", "└─", "after"]);
    }

    #[test]
    fn tables_align_columns_and_fit_width() {
        let src = "| Part | Status | Count |\n| :--- | :---: | ---: |\n| **Forward** | `done` | 12 |\n| Backward propagation | open | 3 |";
        let lines = render(src, 42, Style::default());
        let text = plain(&lines);
        assert!(
            text[0].contains("Part") && text[0].contains("│"),
            "{text:?}"
        );
        assert!(text[1].contains("─┼─"), "{text:?}");
        assert!(
            text[2].contains("Forward") && text[2].contains("done"),
            "{text:?}"
        );
        for l in &text {
            assert!(l.width() <= 42, "{l:?} width={}", l.width());
        }
        // Narrow table wraps long cells instead of breaking pipe layout.
        let narrow = render(src, 28, Style::default());
        let narrow_text = plain(&narrow);
        for l in &narrow_text {
            assert!(l.width() <= 28, "{l:?} width={}", l.width());
        }
        assert!(narrow_text.iter().any(|l| l.contains("Backward")));
        assert!(narrow_text.iter().any(|l| l.contains("propagation")));
    }

    #[test]
    fn relative_links_drop_a_url_that_repeats_the_label() {
        let full = |t: &str| -> String {
            inline(t, Style::default())
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        assert_eq!(
            full("see [gate.rs:20](app/src/runtime/gate.rs#L20) now"),
            "see gate.rs:20 now"
        );
        assert_eq!(full("[README.md](./README.md)"), "README.md");
        assert_eq!(full("[`docs/a.md`](docs/a.md)"), "`docs/a.md`");
        // A label that names something else keeps its target.
        assert_eq!(
            full("[the guide](docs/guide.md)"),
            "the guide (docs/guide.md)"
        );
        assert_eq!(
            full("[gate.rs](https://x.org/gate.rs)"),
            "gate.rs (https://x.org/gate.rs)"
        );
    }

    #[test]
    fn map_rows_show_status_marks_and_numbers() {
        let src = "```mermaid\nflowchart TD\n    n_a[\"✓ Candidate checks<br/><small>cand · understood</small>\"]\n    n_b[\"▶ The verdict prompt<br/><small>verdict · open</small>\"]\n    n_c[\"○ Logging<br/><small>log · open</small>\"]\n    n_a --> n_c\n    n_b --> n_c\n```";
        let lines = render(src, 60, Style::default());
        assert_eq!(
            plain(&lines),
            vec![
                "✓ 1 Candidate checks · cand · understood",
                "▶ 2 The verdict prompt · verdict · open",
                "○ 3 Logging · log · open",
                "    ← 1, 2",
            ]
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Cyan));
        assert!(!plain(&lines).iter().any(|l| l.starts_with("- ")));
    }

    #[test]
    fn inline_italic_links_and_snake_case() {
        let spans = inline(
            "Use *one* and _two_ in snake_case_var, see [docs](https://example.com/a_(b)) and [https://same.org](https://same.org).",
            Style::default(),
        );
        let one = spans.iter().find(|s| s.content == "one").unwrap();
        assert!(one.style.add_modifier.contains(Modifier::ITALIC));
        let two = spans.iter().find(|s| s.content == "two").unwrap();
        assert!(two.style.add_modifier.contains(Modifier::ITALIC));
        let full: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(full.contains("snake_case_var"), "{full}");
        assert!(
            !spans
                .iter()
                .any(|s| s.content == "case" && s.style.add_modifier.contains(Modifier::ITALIC))
        );

        let link_label = spans.iter().find(|s| s.content == "docs").unwrap();
        assert!(link_label.style.add_modifier.contains(Modifier::UNDERLINED));
        let link_url = spans
            .iter()
            .find(|s| s.content == " (https://example.com/a_(b))")
            .unwrap();
        assert!(link_url.style.add_modifier.contains(Modifier::DIM));

        // Same label and url shouldn't duplicate `(url)`.
        assert_eq!(full.matches("https://same.org").count(), 1, "{full}");
    }

    #[test]
    fn wrap_hanging_first_line_uses_full_width_and_handles_wide_chars() {
        // First line must fit 10 columns even when hang=6.
        let lines = wrap_hanging(&[Span::raw("1234567890")], 10, 6);
        assert_eq!(plain(&lines), vec!["1234567890"]);

        // Wide CJK chars must never exceed width=10.
        let cjk = wrap_hanging(&[Span::raw("• 你好世界你好世界")], 10, 2);
        for l in plain(&cjk) {
            assert!(l.width() <= 10, "{l:?} width={}", l.width());
        }
    }

    #[test]
    fn wrap_input_deterministic_and_cursor_exact() {
        // Empty input.
        assert_eq!(wrap_input("", 0, 10), (vec!["".into()], 0, 0));

        // Word wrapping + cursor at space vs start of wrapped word.
        let s = "hello world";
        assert_eq!(
            wrap_input(s, 5, 10),
            (vec!["hello ".into(), "world".into()], 0, 5)
        );
        assert_eq!(
            wrap_input(s, 6, 10),
            (vec!["hello ".into(), "world".into()], 1, 0)
        );
        assert_eq!(
            wrap_input(s, 11, 10),
            (vec!["hello ".into(), "world".into()], 1, 5)
        );

        // Multi-line with empty lines and trailing newline.
        let m = "ab\n\ncd\n";
        assert_eq!(
            wrap_input(m, 3, 10),
            (vec!["ab".into(), "".into(), "cd".into(), "".into()], 1, 0)
        );
        assert_eq!(
            wrap_input(m, m.len(), 10),
            (vec!["ab".into(), "".into(), "cd".into(), "".into()], 3, 0)
        );

        // Overlong word + wide Unicode characters.
        let u = "你好世界你好";
        let (lines, row, col) = wrap_input(u, "你好世界".len(), 5);
        assert_eq!(
            lines,
            vec!["你好".to_string(), "世界".into(), "你好".into()]
        );
        assert_eq!((row, col), (2, 0));
        for l in &lines {
            assert!(l.width() <= 5, "{l:?}");
        }
    }

    #[test]
    fn latex_in_replies_reads_as_unicode_but_code_and_quotes_stay_verbatim() {
        let reply = concat!(
            r"The overlap is \(c_{ij}=\mathcal N(\mu_{a_i};\mu_{b_j},v_{a_i}+v_{b_j})\)",
            "\n",
            r"- weight \(w_{a_i}w_{b_j}c_{ij}\), error below \(2.22\times10^{-16}\)",
            "\n",
            r"Inline code `\(x_1\)` stays.",
            "\n\n```python\n",
            r"s = '\(a_i\)'",
            "\n```\n\n",
            r"  > from the paper: \(\mu_{a_i}\) and $x^2$",
            "\n\n$$\n",
            r"\frac{a+b}{2}",
            "\n$$\nafter",
        );
        let text = plain(&render(reply, 200, Style::default())).join("\n");
        assert!(
            text.contains("The overlap is cᵢⱼ = 𝒩(μ_(aᵢ); μ_(bⱼ), v_(aᵢ) + v_(bⱼ))"),
            "{text}"
        );
        assert!(
            text.contains("• weight w_(aᵢ)w_(bⱼ)cᵢⱼ, error below 2.22×10⁻¹⁶"),
            "{text}"
        );
        // Code, inline and fenced, and a quote from a source: verbatim.
        assert!(text.contains(r"Inline code \(x_1\) stays."), "{text}");
        assert!(text.contains(r"s = '\(a_i\)'"), "{text}");
        assert!(
            text.contains(r"from the paper: \(\mu_{a_i}\) and $x^2$"),
            "{text}"
        );
        // Display math across lines: one indented line.
        assert!(text.contains("\n  (a + b)/2\nafter"), "{text}");
        assert!(!text.contains("$$"), "{text}");
        // A Markdown-escaped citation and shell variables keep their marks.
        let text = plain(&render(
            r"See \[1\] and copy it to $HOME/$USER now.",
            200,
            Style::default(),
        ))
        .join("\n");
        assert!(text.contains("See [1] and copy it to $HOME/$USER now."), "{text}");
    }
}
