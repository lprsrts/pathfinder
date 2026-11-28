//! Cleaning up `pdftotext` output where math fonts break it.
//!
//! Slides made in PowerPoint or Word set math in Microsoft's Symbol font
//! (and bullets in Wingdings). Their text often maps to Unicode's private
//! use area (`U+F072` for Symbol's `r`, which is ρ), and some PDFs map those
//! glyphs to plain Latin letters instead (`r` for ρ, `¶` for ∂). This module
//! maps the private-use codes back to real characters when poppler's
//! `pdftohtml -xml` says which font they came from, and marks the pages whose
//! text is still unreliable, so `/open` can say so and quotes don't present
//! garbled symbols as the source's words.

use std::collections::{HashMap, HashSet};

/// The note `/open` shows above a page whose math didn't extract cleanly.
pub fn unclean_note(page: usize) -> String {
    format!("Math on this page didn't extract cleanly; check it in your PDF viewer: page {page}")
}

/// What a font's family name says about the text poppler gives for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontClass {
    /// Symbol and its clones: Greek letters and math at Latin positions.
    Symbol,
    /// Wingdings, Webdings, Zapf Dingbats: bullets and pictures.
    Dingbat,
    /// Math fonts that have no letters at all (MT Extra, CMEX, MSAM):
    /// a letter in their text is a wrong mapping.
    NoLetters,
    /// Math fonts whose capitals are real (calligraphic, blackboard bold)
    /// but whose lowercase positions are symbols (CMSY, MSBM).
    NoLowercase,
    /// Cambria Math, as Word's equation editor sets it: its ToUnicode map
    /// puts accents and big operators in unrelated scripts.
    OfficeMath,
    Other,
}

/// The class of a font family as `pdffonts` or `pdftohtml` prints it
/// (a subset prefix such as `BCDEEE+` is ignored).
pub fn classify(family: &str) -> FontClass {
    let name = match family.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => family,
    };
    let f: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    if ["wingding", "webding", "dingbat"]
        .iter()
        .any(|k| f.contains(k))
    {
        FontClass::Dingbat
    } else if f.starts_with("symbol")
        || f.starts_with("standardsym")
        || f.starts_with("euclidsymbol")
    {
        FontClass::Symbol
    } else if f.starts_with("mtextra")
        || f.starts_with("euclidextra")
        || f.starts_with("cmex")
        || f.starts_with("msam")
    {
        FontClass::NoLetters
    } else if f.starts_with("cmsy") || f.starts_with("msbm") {
        FontClass::NoLowercase
    } else if f.starts_with("cambriamath") {
        FontClass::OfficeMath
    } else {
        FontClass::Other
    }
}

/// Adobe's Symbol encoding: the character a Symbol font draws for `code`.
pub fn symbol_char(code: u8) -> Option<char> {
    let c = match code {
        0x20..=0x7E => {
            const LOW: &str = " !∀#∃%&∋()∗+,−./0123456789:;<=>?≅ΑΒΧΔΕΦΓΗΙϑΚΛΜΝΟΠΘΡΣΤΥςΩΞΨΖ[∴]⊥_‾αβχδεφγηιϕκλμνοπθρστυϖωξψζ{|}∼";
            return LOW.chars().nth(usize::from(code - 0x20));
        }
        0xA0 => '€',
        0xA1..=0xFE => {
            const HIGH: &str = "ϒ′≤⁄∞ƒ♣♦♥♠↔←↑→↓°±″≥×∝∂•÷≠≡≈…⏐⎯↵ℵℑℜ℘⊗⊕∅∩∪⊃⊇⊄⊂⊆∈∉∠∇®©™∏√⋅¬∧∨⇔⇐⇑⇒⇓◊⟨®©™∑⎛⎜⎝⎡⎢⎣⎧⎨⎩⎪\u{0}⟩∫⌠⎮⌡⎞⎟⎠⎤⎥⎦⎫⎬⎭";
            return HIGH
                .chars()
                .nth(usize::from(code - 0xA1))
                .filter(|c| *c != '\u{0}');
        }
        _ => return None,
    };
    Some(c)
}

/// The Wingdings glyphs slides use as bullets, arrows and check marks.
/// Other Wingdings pictures have no safe text form.
pub fn dingbat_char(code: u8) -> Option<char> {
    Some(match code {
        0x6C => '●',
        0x6E => '■',
        0x6F => '□',
        0x71 => '❑',
        0x72 => '❒',
        0x75 => '◆',
        0x76 => '❖',
        0x77 => '⬥',
        0x9F => '•',
        0xA7 => '▪',
        0xA8 => '◻',
        0xD8 => '➢',
        0xE0 => '→',
        0xE8 => '➔',
        0xF0 => '⇨',
        0xFB => '✗',
        0xFC => '✓',
        0xFD => '☒',
        0xFE => '☑',
        _ => return None,
    })
}

/// Private use, where symbol fonts' codes land (`U+F000 + code`).
fn is_private(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c)
}

/// The symbol-font code behind a private-use character, if it is one.
fn symbol_code(c: char) -> Option<u8> {
    let n = u32::from(c);
    (0xF020..=0xF0FF).contains(&n).then(|| (n - 0xF000) as u8)
}

/// What one page's text runs say, from `pdftohtml -xml`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PageFonts {
    /// Private-use characters and the classes of the fonts they came from.
    pub private: HashMap<char, HashSet<FontClass>>,
    /// A symbol or math font's run had letters it can't have: its glyphs
    /// were mapped to the wrong characters.
    pub wrong_letters: bool,
    /// Some text on the page is set in Cambria Math (see
    /// [`FontClass::OfficeMath`]).
    pub office_math: bool,
    /// The page's text from runs without wrong letters, in poppler's
    /// order; a run with wrong letters ends a segment, so no segment joins
    /// the words on either side of one.
    pub trusted: Vec<String>,
}

/// How far a page's text can be trusted as the source's own words.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PageTrust {
    /// Extracted cleanly: every line can be quoted.
    #[default]
    Clean,
    /// Some math is garbled: only plain prose lines can be quoted.
    Shaky,
    /// A math font's glyphs came out as ordinary letters (`X` for ∑, `m`
    /// for μ), and poppler said which runs they are in: only plain prose
    /// whose letters all come from the other runs can be quoted.
    Lettered,
    /// A math font's glyphs came out as ordinary letters and nothing else
    /// on the page can be told apart from them: no line can be quoted.
    Unquotable,
}

impl PageTrust {
    /// Whether `/open` notes the page as not extracted cleanly.
    pub fn marked(self) -> bool {
        self != PageTrust::Clean
    }
}

/// Reads `pdftohtml -xml -i` output into each page's font facts, by page
/// number (1-based).
pub fn parse_xml(xml: &str) -> HashMap<usize, PageFonts> {
    let mut fonts: HashMap<String, FontClass> = HashMap::new();
    let mut pages: HashMap<usize, PageFonts> = HashMap::new();
    let mut page = 0usize;
    for line in xml.lines() {
        let line = line.trim_start();
        if line.starts_with("<page ") {
            page = attr(line, "number")
                .and_then(|n| n.parse().ok())
                .unwrap_or(page + 1);
        } else if line.starts_with("<fontspec ") {
            if let (Some(id), Some(family)) = (attr(line, "id"), attr(line, "family")) {
                fonts.insert(id.to_string(), classify(&unescape(family)));
            }
        } else if line.starts_with("<text ") {
            let class = attr(line, "font")
                .and_then(|id| fonts.get(id))
                .copied()
                .unwrap_or(FontClass::Other);
            let Some(start) = line.find('>') else {
                continue;
            };
            let body = line[start + 1..]
                .rsplit_once("</text>")
                .map_or(&line[start + 1..], |(b, _)| b);
            let text = unescape(&strip_tags(body));
            let facts = pages.entry(page).or_default();
            if class == FontClass::OfficeMath {
                facts.office_math = true;
            }
            for c in text.chars().filter(|c| is_private(*c)) {
                facts.private.entry(c).or_default().insert(class);
            }
            if wrong_letters(class, &text) {
                facts.wrong_letters = true;
                facts.trusted.push(String::new());
            } else {
                match facts.trusted.last_mut() {
                    Some(seg) => {
                        seg.push_str(&text);
                        seg.push('\n');
                    }
                    None => facts.trusted.push(format!("{text}\n")),
                }
            }
        }
    }
    pages
}

/// Whether a run in a font of `class` has letters that font can't draw.
fn wrong_letters(class: FontClass, text: &str) -> bool {
    match class {
        // Symbol's Latin positions are Greek; its Latin-1 half is math.
        // Correct text from it has no Latin letters and none of the
        // Latin-1 signs it lacks (¶, §, ¨, accented letters).
        FontClass::Symbol => text.chars().any(|c| {
            c.is_ascii_alphabetic()
                || (('\u{A1}'..='\u{FF}').contains(&c)
                    && !matches!(c, '°' | '±' | '×' | '÷' | '¬' | '®' | '©' | '·' | 'µ'))
        }),
        FontClass::NoLetters => text.chars().any(|c| c.is_ascii_alphabetic()),
        FontClass::NoLowercase => text.chars().any(|c| c.is_ascii_lowercase()),
        FontClass::Dingbat | FontClass::OfficeMath | FontClass::Other => false,
    }
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let at = tag.find(&key)? + key.len();
    let len = tag[at..].find('"')?;
    Some(&tag[at..at + len])
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' if inside => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether any page has private-use characters (worth asking poppler
/// which fonts they came from).
pub fn has_private(pages: &[String]) -> bool {
    pages.iter().any(|p| p.chars().any(is_private))
}

/// Whether `pdffonts` output lists a font this module cares about.
pub fn lists_math_fonts(pdffonts: &str) -> bool {
    pdffonts
        .lines()
        .skip(2)
        .filter_map(|l| l.split_whitespace().next())
        .any(|name| classify(name) != FontClass::Other)
}

/// Maps private-use symbol codes back to characters where the page's fonts
/// say which font drew them, and Cambria Math's known misplaced accents
/// and operators back to what they are, and returns how far each page can
/// be trusted. `fonts` is None when poppler couldn't say.
pub fn repair(pages: &mut [String], fonts: Option<&HashMap<usize, PageFonts>>) -> Vec<PageTrust> {
    let empty = PageFonts::default();
    pages
        .iter_mut()
        .enumerate()
        .map(|(i, page)| {
            let facts = fonts.and_then(|f| f.get(&(i + 1))).unwrap_or(&empty);
            if page.chars().any(is_private) {
                *page = page
                    .chars()
                    .map(|c| mapped(c, facts).unwrap_or(c))
                    .collect();
            }
            if facts.office_math && page.chars().any(|c| office_math_char(c).is_some()) {
                *page = page
                    .chars()
                    .map(|c| office_math_char(c).unwrap_or(c))
                    .collect();
            }
            if facts.wrong_letters {
                if facts
                    .trusted
                    .iter()
                    .any(|s| s.chars().any(char::is_alphanumeric))
                {
                    PageTrust::Lettered
                } else {
                    PageTrust::Unquotable
                }
            } else if unreliable(page) {
                PageTrust::Shaky
            } else {
                PageTrust::Clean
            }
        })
        .collect()
}

/// A [`PageTrust::Lettered`] page's trusted segments (see
/// [`PageFonts::trusted`]), with symbol codes and Cambria Math's misplaced
/// characters mapped as [`repair`] maps the page.
pub fn trusted_text(facts: &PageFonts) -> Vec<String> {
    facts
        .trusted
        .iter()
        .map(|seg| {
            seg.chars()
                .map(|c| mapped(c, facts).unwrap_or(c))
                .map(|c| {
                    if facts.office_math {
                        office_math_char(c).unwrap_or(c)
                    } else {
                        c
                    }
                })
                .collect()
        })
        .collect()
}

/// What Cambria Math's ToUnicode map really means by the few characters
/// it misplaces that are known: dot and double dot above (ẋ, ẍ), the
/// vector arrow, and the sum sign.
fn office_math_char(c: char) -> Option<char> {
    Some(match c {
        '\u{1236}' => '\u{0307}',
        '\u{1237}' => '\u{0308}',
        '\u{0526}' => '\u{20D7}',
        '\u{0DCD}' => '∑',
        _ => return None,
    })
}

/// Scripts a German or English physics slide doesn't use, where Cambria
/// Math's ToUnicode map puts accents, braces and big operators.
fn is_misplaced_script(c: char) -> bool {
    matches!(c,
        '\u{0500}'..='\u{052F}'
            | '\u{0590}'..='\u{05FF}'
            | '\u{0D80}'..='\u{0DFF}'
            | '\u{0E00}'..='\u{0E7F}'
            | '\u{1200}'..='\u{137F}'
    )
}

/// Mathematical italic and other math alphanumerics (𝑥, 𝐸): Word's
/// equations.
fn is_math_alnum(c: char) -> bool {
    ('\u{1D400}'..='\u{1D7FF}').contains(&c)
}

/// The real character for a private-use one, when its font is known.
fn mapped(c: char, facts: &PageFonts) -> Option<char> {
    let code = symbol_code(c)?;
    let classes = facts.private.get(&c)?;
    if classes.len() != 1 {
        // Drawn by two kinds of font on this page: ambiguous.
        return None;
    }
    match classes.iter().next()? {
        FontClass::Symbol => symbol_char(code),
        FontClass::Dingbat => dingbat_char(code),
        _ => None,
    }
}

/// Whether a page's text still shows signs of a broken extraction: private
/// use or replacement characters, stray control characters, pieces of tall
/// brackets and integrals, a line that is nothing but math symbols with a
/// big operator among them (its operand landed elsewhere), or Word math
/// (math italic letters) next to letters from scripts it misplaces accents
/// and operators into. A lone ∂, −, ∗, ∞ or arrow on its own line is a
/// fraction's numerator, a limit or a diagram's arrow, not a sign.
pub fn unreliable(page: &str) -> bool {
    if page.chars().any(|c| {
        is_private(c)
            || c == '\u{FFFD}'
            || (c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}'))
    }) {
        return true;
    }
    if page.chars().any(is_bracket_piece) {
        return true;
    }
    if page.chars().any(is_math_alnum) && page.chars().any(is_misplaced_script) {
        return true;
    }
    page.lines().any(|l| {
        let symbols: Vec<char> = l.chars().filter(|c| !c.is_whitespace()).collect();
        !symbols.is_empty()
            && symbols.iter().all(|c| is_math_symbol(*c))
            && symbols.iter().any(|c| is_big_operator(*c))
    })
}

/// Operators whose operand sits beside them: alone on a line, it was cut
/// off from it.
fn is_big_operator(c: char) -> bool {
    matches!(
        c,
        '∑' | '∏' | '∐' | '∫' | '∬' | '∭' | '∮' | '∯' | '∰' | '⨀'..='⨆' | '⨋'..='⨜'
    )
}

/// Pieces of a bracket, brace or integral drawn from several glyphs.
fn is_bracket_piece(c: char) -> bool {
    matches!(
        c,
        '\u{239B}'..='\u{23AD}' | '\u{2320}' | '\u{2321}' | '\u{23D0}'
    )
}

/// Math operators and relations (not letters, digits, ASCII punctuation or
/// bullets).
fn is_math_symbol(c: char) -> bool {
    matches!(c,
        '\u{2200}'..='\u{22FF}'
            | '\u{2A00}'..='\u{2AFF}'
            | '\u{27C0}'..='\u{27EF}'
            | '\u{2980}'..='\u{29FF}'
            | '\u{2190}'..='\u{21FF}'
    )
}

/// Whether a line of a [`PageTrust::Shaky`] page can be shown as the
/// source's own words: letters (Greek included: on such a page it came from
/// poppler or the Symbol table, so it is right), digits and plain
/// punctuation only, so no symbol in it can be a wrong one.
pub fn plain_prose(line: &str) -> bool {
    line.chars().any(char::is_alphabetic) && line.chars().all(prose_char)
}

/// A character [`plain_prose`] allows.
pub fn prose_char(c: char) -> bool {
    c.is_alphanumeric() && !is_private(c) && !is_misplaced_script(c) && !is_math_alnum(c)
        || c.is_whitespace()
        || ".,;:!?'\"()[]-–—„“”‘’«»/%&§".contains(c)
}

/// Small one-page PDFs for tests: Helvetica text with a second font `F1`.
#[cfg(test)]
pub(crate) mod fixture {
    /// A PDF whose page draws `content` (a content stream using `/F1` and
    /// `/F2`, Helvetica), with `F1` set by `font` (a font dictionary) and,
    /// when given, a ToUnicode CMap mapping its one-byte codes as `to_unicode`.
    pub fn pdf(font: &str, to_unicode: Option<&[(u8, u32)]>, content: &str) -> Vec<u8> {
        let mut font = font.to_string();
        let mut cmap = String::new();
        if let Some(pairs) = to_unicode {
            font = format!(
                "{} /ToUnicode 7 0 R >>",
                font.trim_end_matches(">>").trim_end()
            );
            cmap.push_str("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def 1 begincodespacerange <00> <FF> endcodespacerange ");
            cmap.push_str(&format!("{} beginbfchar\n", pairs.len()));
            for (code, u) in pairs {
                cmap.push_str(&format!("<{code:02X}> <{u:04X}>\n"));
            }
            cmap.push_str(
                "endbfchar endcmap CMapName currentdict /CMap defineresource pop end end",
            );
        }
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 400] /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            font,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
            format!("<< /Length {} >>\nstream\n{cmap}\nendstream", cmap.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// A Maxwell slide: `div D = ρ` and `∂B` with ρ and ∂ from `F1`
    /// (codes 0x72 and 0xB6, as in Symbol), a page label set as two runs
    /// (`Fu`, `B 1.19`) and a line of prose.
    pub const MAXWELL: &str = "BT /F2 24 Tf 50 300 Td (div D = ) Tj /F1 24 Tf (r) Tj ET \
BT /F1 24 Tf 50 250 Td (\\266) Tj /F2 24 Tf (B) Tj ET \
BT /F2 12 Tf 400 350 Td (Fu) Tj (B 1.19) Tj ET \
BT /F2 14 Tf 50 150 Td (Maxwell equations describe the fields) Tj ET";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_codes_map_to_greek_and_math() {
        assert_eq!(symbol_char(0x72), Some('ρ'));
        assert_eq!(symbol_char(0xB6), Some('∂'));
        assert_eq!(symbol_char(0x61), Some('α'));
        assert_eq!(symbol_char(0x57), Some('Ω'));
        assert_eq!(symbol_char(0x2D), Some('−'));
        assert_eq!(symbol_char(0xD1), Some('∇'));
        assert_eq!(symbol_char(0xE5), Some('∑'));
        assert_eq!(symbol_char(0xF2), Some('∫'));
        assert_eq!(symbol_char(0xAE), Some('→'));
        assert_eq!(symbol_char(0xB9), Some('≠'));
        assert_eq!(symbol_char(0xFE), Some('⎭'));
        assert_eq!(symbol_char(0xF0), None);
        assert_eq!(symbol_char(0x10), None);
        assert_eq!(symbol_char(0xFF), None);
    }

    #[test]
    fn font_families_are_classified() {
        assert_eq!(classify("SymbolMT"), FontClass::Symbol);
        assert_eq!(classify("ABCDEF+Symbol"), FontClass::Symbol);
        assert_eq!(classify("Wingdings-Regular"), FontClass::Dingbat);
        assert_eq!(classify("ZapfDingbats"), FontClass::Dingbat);
        assert_eq!(classify("MT Extra"), FontClass::NoLetters);
        assert_eq!(classify("XYZABC+CMEX10"), FontClass::NoLetters);
        assert_eq!(classify("CMSY10"), FontClass::NoLowercase);
        assert_eq!(classify("CMMI10"), FontClass::Other);
        assert_eq!(classify("ArialMT"), FontClass::Other);
    }

    const XML: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<pdf2xml producer="poppler" version="26.11.0">
<page number="1" position="absolute" top="0" left="0" height="810" width="1170">
	<fontspec id="0" size="38" family="ArialMT" color="#000000"/>
	<fontspec id="2" size="40" family="SymbolMT" color="#000000"/>
	<fontspec id="3" size="40" family="Wingdings" color="#000000"/>
<text top="208" left="25" width="91" height="35" font="0"><i>div D</i></text>
<text top="195" left="158" width="21" height="48" font="2">&#xf072;</text>
<text top="324" left="180" width="19" height="46" font="2">&#xf0b6;</text>
<text top="24" left="18" width="19" height="46" font="3">&#xf0a7;</text>
</page>
<page number="2" position="absolute" top="0" left="0" height="810" width="1170">
	<fontspec id="4" size="40" family="Symbol" color="#000000"/>
<text top="114" left="198" width="20" height="47" font="4">r</text>
<text top="114" left="198" width="20" height="47" font="4">= &amp; ±</text>
</page>
</pdf2xml>"##;

    #[test]
    fn runs_with_wrong_letters_split_the_trusted_text() {
        // LaTeX's CMEX draws ∑ with the code of `X`; the prose around it is
        // in a text font.
        let xml = r##"<page number="1">
	<fontspec id="0" size="16" family="ABCDEF+SFRM1095" color="#000000"/>
	<fontspec id="1" size="16" family="ABCDEF+CMEX10" color="#000000"/>
<text top="10" left="10" width="90" height="16" font="0">sources for</text>
<text top="10" left="100" width="9" height="16" font="1">P</text>
<text top="10" left="110" width="90" height="16" font="0">one and the same</text>
<text top="30" left="10" width="90" height="16" font="0">Proposition 3 (Sum).</text>
</page>
<page number="2">
<text top="10" left="10" width="9" height="16" font="1">X</text>
</page>"##;
        let facts = parse_xml(xml);
        assert!(facts[&1].wrong_letters);
        assert_eq!(
            trusted_text(&facts[&1]),
            vec![
                "sources for\n".to_string(),
                "one and the same\nProposition 3 (Sum).\n".to_string()
            ]
        );
        let mut pages = vec![
            "sources forP one and the same\nProposition 3 (Sum).\n".to_string(),
            "X\n".to_string(),
        ];
        // Nothing on page 2 but the wrong letter: nothing to tell apart.
        assert_eq!(
            repair(&mut pages, Some(&facts)),
            vec![PageTrust::Lettered, PageTrust::Unquotable]
        );
        assert!(PageTrust::Lettered.marked());
    }

    #[test]
    fn xml_says_which_font_drew_each_private_char() {
        let facts = parse_xml(XML);
        let p1 = &facts[&1];
        assert_eq!(p1.private[&'\u{F072}'], HashSet::from([FontClass::Symbol]));
        assert_eq!(p1.private[&'\u{F0B6}'], HashSet::from([FontClass::Symbol]));
        assert_eq!(p1.private[&'\u{F0A7}'], HashSet::from([FontClass::Dingbat]));
        assert!(!p1.wrong_letters);
        // Symbol's `r` is ρ: a Latin letter from it is a wrong mapping.
        assert!(facts[&2].wrong_letters);
    }

    #[test]
    fn private_symbol_codes_are_mapped_and_pages_marked() {
        let facts = parse_xml(XML);
        let mut pages = vec![
            "\u{F0A7} div D = \u{F072}\n   \u{F0B6}B\nrot E = −\n   \u{F0B6}t\n".to_string(),
            "div D = r\n".to_string(),
            "plain text\n".to_string(),
        ];
        let trust = repair(&mut pages, Some(&facts));
        assert_eq!(pages[0], "▪ div D = ρ\n   ∂B\nrot E = −\n   ∂t\n");
        assert_eq!(
            trust,
            vec![PageTrust::Clean, PageTrust::Unquotable, PageTrust::Clean]
        );
    }

    #[test]
    fn unknown_fonts_leave_private_chars_and_mark_the_page() {
        let mut pages = vec!["x = \u{F072}".to_string()];
        assert_eq!(repair(&mut pages, None), vec![PageTrust::Shaky]);
        assert_eq!(pages[0], "x = \u{F072}");
        // The same code from two kinds of font is ambiguous.
        let mut both = PageFonts::default();
        both.private.insert(
            '\u{F072}',
            HashSet::from([FontClass::Symbol, FontClass::Dingbat]),
        );
        let mut pages = vec!["x = \u{F072}".to_string()];
        let facts = HashMap::from([(1, both)]);
        assert_eq!(repair(&mut pages, Some(&facts)), vec![PageTrust::Shaky]);
    }

    #[test]
    fn broken_math_layouts_are_unreliable() {
        assert!(unreliable("a \u{FFFD} b"));
        assert!(unreliable("⎛ a ⎞\n⎝ b ⎠"));
        assert!(unreliable("  ∑\n i=1"));
        assert!(unreliable("  ∫\n f dx"));
        assert!(!unreliable("div D = ρ\n∂B\nrot E = −"));
        assert!(!unreliable("• a bullet\n- another"));
        // A fraction's ∂, a limit's ∞, a diagram's arrow: not signs.
        assert!(!unreliable("     ∂\nrot E = − B\n     ∂t"));
        assert!(!unreliable("lim\n ∞\n →\n σ = e ( μn n + μp p )"));
    }

    #[test]
    fn word_math_in_misplaced_scripts_is_unreliable_and_mapped_back() {
        // ẍ and ẋ from Cambria Math: dots above as Ethiopic letters.
        let page = "𝑧 ⋅ 𝑚𝑒 ⋅ 𝑥ሷ + 𝑘𝑑 𝑧 𝑚𝑒 ⋅ 𝑥ሶ + 𝑘\nweist von –Q nach +Q\n";
        assert!(unreliable(page));
        // Thai or Hebrew letters in German prose without Word math: fine.
        assert!(!unreliable("ไทย\n"));
        let facts = HashMap::from([(
            1,
            PageFonts {
                office_math: true,
                ..Default::default()
            },
        )]);
        let mut pages = vec![page.to_string(), "𝑝Ԧ = 𝑄 ⋅ 𝑑Ԧ, 𝑃 = ෍ 𝑝Ԧ𝑖ต\n".to_string()];
        let mut both = facts.clone();
        both.insert(2, both[&1].clone());
        let trust = repair(&mut pages, Some(&both));
        assert_eq!(
            pages[0],
            "𝑧 ⋅ 𝑚𝑒 ⋅ 𝑥\u{308} + 𝑘𝑑 𝑧 𝑚𝑒 ⋅ 𝑥\u{307} + 𝑘\nweist von –Q nach +Q\n"
        );
        assert_eq!(pages[1], "𝑝\u{20D7} = 𝑄 ⋅ 𝑑\u{20D7}, 𝑃 = ∑ 𝑝\u{20D7}𝑖ต\n");
        // All known ones mapped: the first page is clean; the Thai brace
        // piece left on the second marks it.
        assert_eq!(trust, vec![PageTrust::Clean, PageTrust::Shaky]);
        assert!(!plain_prose("𝑝⃗ weist von –Q nach +Q"));
        assert_eq!(classify("BCDFEE+CambriaMath"), FontClass::OfficeMath);
    }

    #[test]
    fn only_plain_prose_is_quotable_from_an_unreliable_page() {
        assert!(plain_prose(
            "Erlauben eine Vorhersage der zeitlichen Entwicklung"
        ));
        assert!(plain_prose(
            "„Felder“: Dinge, die ganze Raumbereiche erfüllen"
        ));
        assert!(!plain_prose("div D = r"));
        assert!(plain_prose(
            "Quantitativ wird die Leitfähigkeit σ damit berechnet"
        ));
        assert!(!plain_prose("∂B elektrischer Flussdichte"));
        assert!(!plain_prose("x \u{F072}"));
        assert!(!plain_prose("12 34"));
        // A reference and a section sign are prose; an equals sign isn't.
        assert!(plain_prose("admits a closed-form expression [18], because"));
        assert!(plain_prose("§ 6.8 Reduction of mixtures"));
        assert!(!plain_prose("with M = K L components"));
    }

    #[test]
    fn pdffonts_listing_is_checked_for_math_fonts() {
        let listing = "name                                 type              encoding         emb sub uni object ID\n------------------------------------ ----------------- ---------------- --- --- --- ---------\nArialMT                              TrueType          WinAnsi          no  no  no       7  0\n";
        assert!(!lists_math_fonts(listing));
        let with = format!(
            "{listing}SymbolMT                             CID TrueType      Identity-H       yes no  yes     93  0\n"
        );
        assert!(lists_math_fonts(&with));
    }
}
