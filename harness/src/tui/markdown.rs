//! Markdown for the terminal: the subset Pathfinder's replies use
//! (headings, bold, inline code, lists, code blocks, tables as they are),
//! word-wrapped to a width. Mermaid blocks, which Zed draws, become a list
//! of nodes with what feeds into each.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Styled lines for `text`, wrapped to `width` columns.
pub fn render(text: &str, width: u16, base: Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(lang) = trimmed.strip_prefix("```") {
            let mut block = Vec::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                block.push(l.to_string());
            }
            if lang.trim() == "mermaid" {
                for l in mermaid_to_text(&block.join("\n")) {
                    // Continuations line up under the text, past "- " or "← ".
                    let lead = l.len() - l.trim_start().len();
                    let hang = lead + 2;
                    out.extend(wrap_hanging(&inline(&l, base), width, hang));
                }
            } else {
                let code = base.add_modifier(Modifier::DIM);
                for l in block {
                    out.extend(wrap(&[Span::styled(format!("  {l}"), code)], width));
                }
            }
            continue;
        }
        if let Some(h) = heading(trimmed) {
            let style = base.add_modifier(Modifier::BOLD).fg(Color::Cyan);
            out.extend(wrap(&[Span::styled(h.to_string(), style)], width));
            continue;
        }
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let indent = " ".repeat(line.len() - trimmed.len());
            let mut spans = vec![Span::styled(format!("{indent}• "), base)];
            spans.extend(inline(item, base));
            out.extend(wrap_hanging(&spans, width, indent.len() + 2));
            continue;
        }
        if trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && trimmed.contains("--") {
            // A table's separator row.
            out.push(Line::from(Span::styled(
                trimmed.to_string(),
                base.add_modifier(Modifier::DIM),
            )));
            continue;
        }
        let lead = line.len() - trimmed.len();
        out.extend(wrap_hanging(&inline(line, base), width, lead));
    }
    out
}

fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' '))
        .flatten()
}

/// `**bold**` and `` `code` `` within a line.
pub fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;
    let mut buf = String::new();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**")
        {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base));
            }
            spans.push(Span::styled(
                after[..end].to_string(),
                base.add_modifier(Modifier::BOLD),
            ));
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base));
            }
            spans.push(Span::styled(
                after[..end].to_string(),
                base.fg(Color::Yellow),
            ));
            rest = &after[end + 1..];
            continue;
        }
        let ch = rest.chars().next().unwrap();
        buf.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    if !buf.is_empty() || spans.is_empty() {
        spans.push(Span::styled(buf, base));
    }
    spans
}

/// Word-wraps styled spans to `width` columns.
pub fn wrap(spans: &[Span<'static>], width: u16) -> Vec<Line<'static>> {
    wrap_hanging(spans, width, 0)
}

/// Like `wrap`, with continuation lines indented by `hang` columns.
pub fn wrap_hanging(spans: &[Span<'static>], width: u16, hang: usize) -> Vec<Line<'static>> {
    let width = (width as usize).max(10);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
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
                lines.push(Line::from(std::mem::take(current)));
                let trimmed = word.trim_start().to_string();
                *current = vec![Span::raw(" ".repeat(hang))];
                *used = hang + trimmed.width();
                current.push(Span::styled(trimmed, span.style));
            } else {
                *used += w;
                current.push(Span::styled(word.clone(), span.style));
            }
            word.clear();
        };
        for ch in span.content.chars() {
            if ch == ' ' && !word.trim().is_empty() {
                flush(&mut word, &mut current, &mut used, &mut lines);
            }
            word.push(ch);
            // A word wider than a continuation line is cut there.
            if word.width() >= width.saturating_sub(hang).max(1) {
                flush(&mut word, &mut current, &mut used, &mut lines);
            }
        }
        flush(&mut word, &mut current, &mut used, &mut lines);
    }
    lines.push(Line::from(current));
    lines
}

/// A Mermaid flowchart as text: each node, then the nodes feeding into it.
pub fn mermaid_to_text(source: &str) -> Vec<String> {
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
    let label = |id: &str| {
        nodes
            .iter()
            .find(|(n, _)| n == id)
            .map(|(_, l)| l.split(" · ").next().unwrap_or(l).to_string())
            .unwrap_or_else(|| id.to_string())
    };
    let mut out = Vec::new();
    for (id, text) in &nodes {
        out.push(format!("- {text}"));
        let feeds: Vec<String> = edges
            .iter()
            .filter(|(_, to)| to == id)
            .map(|(from, _)| label(from))
            .collect();
        if !feeds.is_empty() {
            out.push(format!("    ← {}", feeds.join(", ")));
        }
    }
    out
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
                "- ▶ Forward · fwd · open",
                "- Loss · loss · open",
                "    ← ▶ Forward"
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
        assert!(text.contains(&"    ← A".to_string()), "{text:?}");
        let tree = plain(&render(
            "```mermaid\nflowchart LR\n    a[\"Alpha\"]\n    b[\"Beta gamma delta epsilon zeta eta\"]\n    a --> b\n    b --> a\n```",
            20,
            Style::default(),
        ));
        let at = tree.iter().position(|l| l.starts_with("- Beta")).unwrap();
        assert!(
            tree[at + 1].starts_with("  ") && !tree[at + 1].starts_with("   "),
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
        assert_eq!(code.style.fg, Some(Color::Yellow));
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
}
