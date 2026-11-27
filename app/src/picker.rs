//! A list to pick from, with section headers and type-to-filter, and a
//! one-line text field: shared by the TUI's dialogs and the terminal prompts
//! of `pathfinder auth`, so both move, filter and go back the same way.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Whether an item of a filterable list is a section header: an empty id.
/// Headers are drawn but never selected.
pub fn is_header(item: &(String, String)) -> bool {
    item.0.is_empty()
}

/// The header an item sits under, if any.
fn header_of(items: &[(String, String)], idx: usize) -> Option<usize> {
    items[..idx].iter().rposition(is_header)
}

/// How well `query` matches `text`, best first: 0 a prefix, 1 the start of
/// a word, 2 anywhere, 3 its letters in order (only from 3 letters, so a
/// short query doesn't find everything). Case-insensitive; `query` is
/// expected lower case. Shared by every list that filters as you type (the
/// command palette, the sign-in lists, the model chooser), so they rank
/// alike.
pub fn rank(text: &str, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    if text.starts_with(query) {
        return Some(0);
    }
    let mut prev_word = false;
    let mut found = false;
    for (i, c) in text.char_indices() {
        let word = c.is_alphanumeric();
        if word && !prev_word && text[i..].starts_with(query) {
            return Some(1);
        }
        if !found && text[i..].starts_with(query) {
            found = true;
        }
        prev_word = word;
    }
    if found {
        return Some(2);
    }
    if query.chars().count() >= 3 {
        let mut want = query.chars().filter(|c| !c.is_whitespace()).peekable();
        for c in text.chars() {
            if want.peek() == Some(&c) {
                want.next();
            }
        }
        if want.peek().is_none() {
            return Some(3);
        }
    }
    None
}

/// How well `query` (lower case) matches item `idx` itself, its section
/// header aside: its name, or its id when it has no note.
pub fn own_rank(items: &[(String, String)], idx: usize, query: &str) -> Option<u8> {
    let (id, label) = &items[idx];
    match split_note(label) {
        (name, "") => rank(name, query).into_iter().chain(rank(id, query)).min(),
        (name, _) => rank(name, query),
    }
}

/// Indices into `items` of the selectable items matching `query`, the best
/// matches first within each section ([`rank`] on the id or label, or on
/// their section header). A label with a note (a login row) matches on its
/// name only: never on the note or the internal id, so `o` doesn't find
/// every row that is "stored" or every `endpoint:` server.
pub fn filter_matches(items: &[(String, String)], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut ranked: Vec<(Option<usize>, u8, usize)> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| !is_header(item))
        .filter_map(|(i, _)| {
            let header = header_of(items, i);
            // A header counts as a whole word or a part of one, never by
            // its letters in order (a long header has most letters).
            let tier = own_rank(items, i, &q)
                .into_iter()
                .chain(header.and_then(|h| rank(&items[h].1, &q).filter(|&r| r < 3)))
                .min()?;
            Some((header, tier, i))
        })
        .collect();
    ranked.sort_by_key(|&(header, tier, i)| (header, tier, i));
    ranked.into_iter().map(|(_, _, i)| i).collect()
}

/// The rows to draw for `query`: each match, after its section's header
/// (once per section, and only for sections with a match). Each row is an
/// index into `items`.
pub fn display_rows(items: &[(String, String)], query: &str) -> Vec<usize> {
    let mut rows = Vec::new();
    let mut last = None;
    for idx in filter_matches(items, query) {
        let header = header_of(items, idx);
        if header.is_some() && header != last {
            rows.extend(header);
            last = header;
        }
        rows.push(idx);
    }
    rows
}

/// Lists longer than this filter as you type; shorter ones take j/k too.
pub const FILTER_FROM: usize = 5;

/// Between a label's name and its note (a state, drawn in a column of its
/// own): a control character no name or note has.
const NOTE: char = '\u{1f}';

/// A label that draws as a name and, after it, a note.
pub fn with_note(name: &str, note: &str) -> String {
    if note.is_empty() {
        name.to_string()
    } else {
        format!("{name}{NOTE}{note}")
    }
}

/// A label's name and note (empty when it has none).
pub fn split_note(label: &str) -> (&str, &str) {
    label.split_once(NOTE).unwrap_or((label, ""))
}

/// A label as one line of text: the name, two spaces, the note.
pub fn plain_label(label: &str) -> String {
    match split_note(label) {
        (name, "") => name.to_string(),
        (name, note) => format!("{name}  {note}"),
    }
}

/// A list of `(id, label)` items (an empty id is a section header) with a
/// selection that counts matches only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Picker {
    pub items: Vec<(String, String)>,
    pub query: String,
    pub sel: usize,
    /// Typing filters however short the list is (a filter line shows).
    pub filter: bool,
}

/// What a key did to a [`Picker`] or a [`Field`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Moved, filtered or edited; nothing to act on.
    Stay,
    /// Enter: the picked item's id, or the field's text.
    Submit(String),
    /// Esc: go back a step.
    Back,
}

impl Picker {
    pub fn new(items: Vec<(String, String)>) -> Self {
        Self {
            items,
            query: String::new(),
            sel: 0,
            filter: false,
        }
    }

    /// Typing filters, with a filter line, however short the list is.
    pub fn filtering(mut self) -> Self {
        self.filter = true;
        self
    }

    /// Starts on the item with this id, when it is there.
    pub fn select(mut self, id: &str) -> Self {
        if let Some(i) = self.matches().iter().position(|&m| self.items[m].0 == id) {
            self.sel = i;
        }
        self
    }

    /// Whether typing filters (a long list) rather than moving.
    pub fn filterable(&self) -> bool {
        self.filter || self.items.iter().filter(|i| !is_header(i)).count() > FILTER_FROM
    }

    pub fn matches(&self) -> Vec<usize> {
        filter_matches(&self.items, &self.query)
    }

    /// The selected item's id.
    pub fn selected(&self) -> Option<&str> {
        let m = self.matches();
        m.get(self.sel.min(m.len().saturating_sub(1)))
            .map(|&i| self.items[i].0.as_str())
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        let len = self.matches().len();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);
        let vi = !self.filterable() && plain;
        match key.code {
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.sel = 0;
            }
            KeyCode::Esc => return Outcome::Back,
            KeyCode::Enter => {
                return match self.selected() {
                    Some(id) => Outcome::Submit(id.to_string()),
                    None => Outcome::Stay,
                };
            }
            KeyCode::Up | KeyCode::BackTab => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('k') if vi => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('p') if ctrl => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => self.sel = (self.sel + 1).min(len.saturating_sub(1)),
            KeyCode::Char('j') if vi => self.sel = (self.sel + 1).min(len.saturating_sub(1)),
            KeyCode::Char('n') if ctrl => self.sel = (self.sel + 1).min(len.saturating_sub(1)),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = len.saturating_sub(1),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(5),
            KeyCode::PageDown => self.sel = (self.sel + 5).min(len.saturating_sub(1)),
            KeyCode::Backspace => {
                self.query.pop();
                self.sel = 0;
            }
            // As in a text field: clears what was typed.
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.sel = 0;
            }
            KeyCode::Char(c) if plain && self.filterable() && !c.is_control() => {
                self.query.push(c);
                self.sel = 0;
            }
            _ => {}
        }
        Outcome::Stay
    }

    /// Pasted text filters a long list.
    pub fn on_paste(&mut self, text: &str) {
        if self.filterable() {
            self.query.extend(text.chars().filter(|c| !c.is_control()));
            self.sel = 0;
        }
    }
}

/// One line of text with a cursor; a masked field shows a dot per
/// character and never its text.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Field {
    text: String,
    /// In chars.
    cursor: usize,
    pub masked: bool,
}

impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Field")
            .field("text", &self.shown())
            .field("masked", &self.masked)
            .finish()
    }
}

/// Longest text a field takes.
const FIELD_MAX: usize = 4096;

impl Field {
    pub fn plain(initial: &str) -> Self {
        Self {
            text: initial.to_string(),
            cursor: initial.chars().count(),
            masked: false,
        }
    }

    pub fn masked() -> Self {
        Self {
            masked: true,
            ..Self::default()
        }
    }

    /// The text: for the caller to use, never to show when masked.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// What to draw: the text, or a dot per character.
    pub fn shown(&self) -> String {
        if self.masked {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    /// The cursor, in chars of [`Field::shown`].
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn insert(&mut self, c: char) {
        if c.is_control() || self.text.len() + c.len_utf8() > FIELD_MAX {
            return;
        }
        let at = self.byte_at(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    pub fn on_paste(&mut self, text: &str) {
        // A key pasted with its line break: keep the key, drop the break.
        for c in text.trim_end_matches(['\r', '\n']).chars() {
            self.insert(c);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Esc => return Outcome::Back,
            KeyCode::Enter => return Outcome::Submit(self.text.trim().to_string()),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = len,
            KeyCode::Char('b') if ctrl => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('f') if ctrl => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Char('u') if ctrl => {
                let at = self.byte_at(self.cursor);
                self.text.replace_range(..at, "");
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => {
                let at = self.byte_at(self.cursor);
                self.text.truncate(at);
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let at = self.byte_at(self.cursor - 1);
                self.text.remove(at);
                self.cursor -= 1;
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Char(c) if !ctrl && !alt => self.insert(c),
            _ => {}
        }
        Outcome::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn items(n: usize) -> Vec<(String, String)> {
        let mut out = vec![(String::new(), "Section".to_string())];
        out.extend((0..n).map(|i| (format!("id{i}"), format!("Item {i}"))));
        out
    }

    #[test]
    fn moving_skips_headers_and_stops_at_the_ends() {
        let mut p = Picker::new(items(3));
        assert_eq!(p.selected(), Some("id0"));
        assert_eq!(p.on_key(key(KeyCode::Up)), Outcome::Stay);
        assert_eq!(p.selected(), Some("id0"));
        p.on_key(key(KeyCode::Down));
        p.on_key(key(KeyCode::Char('j')));
        p.on_key(key(KeyCode::Down));
        assert_eq!(p.selected(), Some("id2"));
        p.on_key(key(KeyCode::Char('k')));
        assert_eq!(p.selected(), Some("id1"));
        p.on_key(key(KeyCode::Home));
        assert_eq!(p.selected(), Some("id0"));
        p.on_key(key(KeyCode::End));
        assert_eq!(p.on_key(key(KeyCode::Enter)), Outcome::Submit("id2".into()));
    }

    #[test]
    fn a_long_list_filters_as_you_type_and_esc_clears_then_goes_back() {
        let mut p = Picker::new(items(8));
        assert!(p.filterable());
        p.on_key(key(KeyCode::Char('7')));
        assert_eq!(p.query, "7");
        assert_eq!(p.selected(), Some("id7"));
        // j is text here, not a move.
        p.on_key(key(KeyCode::Char('j')));
        assert_eq!(p.selected(), None);
        assert_eq!(p.on_key(key(KeyCode::Enter)), Outcome::Stay);
        p.on_key(key(KeyCode::Backspace));
        assert_eq!(p.selected(), Some("id7"));
        assert_eq!(p.on_key(key(KeyCode::Esc)), Outcome::Stay);
        assert!(p.query.is_empty());
        assert_eq!(p.on_key(key(KeyCode::Esc)), Outcome::Back);
        // Ctrl+U clears the filter, as it clears a field.
        p.on_key(key(KeyCode::Char('4')));
        p.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(p.query.is_empty());
        // The header matches its whole section.
        p.on_paste("section");
        assert_eq!(p.matches().len(), 8);
    }

    #[test]
    fn a_filtering_list_edits_its_filter_and_labels_split_into_name_and_note() {
        let mut p = Picker::new(vec![
            ("anthropic".into(), with_note("Anthropic", "✓ stored")),
            ("openai".into(), with_note("OpenAI", "not configured")),
        ])
        .filtering();
        assert!(p.filterable());
        for c in "openx".chars() {
            p.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.query, "openx");
        assert_eq!(p.selected(), None);
        p.on_key(key(KeyCode::Backspace));
        assert_eq!(p.query, "open");
        assert_eq!(p.selected(), Some("openai"));
        // Only the name filters: never the note, nor an internal id.
        p.query = "stored".into();
        assert_eq!(p.selected(), None);
        p.items
            .push(("endpoint:lab".into(), with_note("lab", "no key needed")));
        // "e" is in "stored", "no key needed" and "endpoint:", but in no
        // name other than OpenAI's.
        p.query = "e".into();
        assert_eq!(p.matches(), [1]);
        p.query = "LAB".into();
        assert_eq!(p.selected(), Some("endpoint:lab"));
        assert_eq!(split_note(&p.items[0].1), ("Anthropic", "✓ stored"));
        assert_eq!(split_note("Plain"), ("Plain", ""));
        assert_eq!(plain_label(&p.items[1].1), "OpenAI  not configured");
    }

    #[test]
    fn matches_rank_prefix_then_word_start_then_anywhere_then_letters() {
        assert_eq!(rank("OpenAI", "open"), Some(0));
        assert_eq!(rank("Mistral AI", "ai"), Some(1));
        assert_eq!(rank("OpenAI", "nai"), Some(2));
        assert_eq!(rank("OpenAI", "oai"), Some(3));
        // Letters in order only from three.
        assert_eq!(rank("OpenAI", "oa"), None);
        assert_eq!(rank("x", ""), Some(0));

        let items: Vec<(String, String)> = ["Mistral AI", "Gemini", "AI21", "Kimi"]
            .iter()
            .map(|n| (n.to_lowercase(), n.to_string()))
            .collect();
        // The prefix first, then the word start; the rest left out.
        assert_eq!(filter_matches(&items, "ai"), [2, 0]);
        assert_eq!(filter_matches(&items, "mi"), [0, 1, 3]);
        let p = Picker::new(items.clone()).filtering();
        let mut p = p;
        p.query = "ai".into();
        assert_eq!(p.selected(), Some("ai21"));

        // Sections stay in order; within each the best match leads.
        let mut sectioned = vec![(String::new(), "One".to_string())];
        sectioned.extend(items[..2].iter().cloned());
        sectioned.push((String::new(), "Two".to_string()));
        sectioned.extend(items[2..].iter().cloned());
        assert_eq!(display_rows(&sectioned, "i"), [0, 1, 2, 3, 4, 5]);
        assert_eq!(filter_matches(&sectioned, "ai"), [1, 4]);
        assert_eq!(filter_matches(&sectioned, "mi"), [1, 2, 5]);
    }

    #[test]
    fn a_short_list_does_not_filter() {
        let mut p = Picker::new(items(3)).select("id2");
        assert!(!p.filterable());
        assert_eq!(p.selected(), Some("id2"));
        p.on_key(key(KeyCode::Char('x')));
        p.on_paste("x");
        assert!(p.query.is_empty());
    }

    #[test]
    fn a_field_edits_like_a_line_and_masks_its_text() {
        let mut f = Field::plain("http://x/v1");
        f.on_key(key(KeyCode::Left));
        f.on_key(key(KeyCode::Left));
        f.on_key(key(KeyCode::Left));
        f.on_key(key(KeyCode::Char('y')));
        assert_eq!(f.text(), "http://xy/v1");
        f.on_key(key(KeyCode::Home));
        f.on_key(key(KeyCode::Delete));
        f.on_key(key(KeyCode::End));
        f.on_key(key(KeyCode::Backspace));
        assert_eq!(f.text(), "ttp://xy/v");
        assert_eq!(f.on_key(key(KeyCode::Esc)), Outcome::Back);

        let mut secret = Field::masked();
        secret.on_paste("sk-ant-secret-value-1234\n");
        assert_eq!(secret.text(), "sk-ant-secret-value-1234");
        assert_eq!(secret.shown(), "•".repeat(24));
        assert!(!format!("{secret:?}").contains("secret"));
        assert_eq!(
            secret.on_key(key(KeyCode::Enter)),
            Outcome::Submit("sk-ant-secret-value-1234".into())
        );
    }
}
