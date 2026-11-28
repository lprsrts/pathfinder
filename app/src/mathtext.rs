//! LaTeX math in model prose, shown as readable Unicode.
//!
//! The prompts ask for plain Unicode math, but models still write
//! `\(c_{ij}=\mathcal N(…)\)` now and then. This turns each delimited
//! piece (`\(…\)`, `\[…\]`, `$$…$$`, `$…$`) into what it means in Unicode:
//! `cᵢⱼ = 𝒩(…)`. It runs only where text is shown, after the gate has
//! approved the exact text (§11, A6), and never inside code spans; callers
//! keep code blocks and quotes from sources away from it, so those stay
//! verbatim. Nothing it doesn't know is dropped: an unknown command keeps
//! its backslash, so the rendering never changes what the math says.

use std::borrow::Cow;

/// `text` with each delimited piece of LaTeX math rendered as Unicode.
/// Code spans (`` `…` ``) are copied untouched.
pub fn prose(text: &str) -> Cow<'_, str> {
    if !text.contains('\\') && !text.contains('$') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(ch) = rest.chars().next() {
        if ch == '`' {
            let ticks = rest.len() - rest.trim_start_matches('`').len();
            let fence = &rest[..ticks];
            if let Some(end) = rest[ticks..].find(fence) {
                let used = ticks + end + ticks;
                out.push_str(&rest[..used]);
                rest = &rest[used..];
                continue;
            }
            out.push_str(fence);
            rest = &rest[ticks..];
            continue;
        }
        if let Some((math, used)) = delimited(rest) {
            out.push_str(&math);
            rest = &rest[used..];
            changed = true;
            continue;
        }
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// The delimited math `rest` starts with, rendered, and how many bytes of
/// `rest` it takes; `None` when `rest` doesn't start with closed math. An
/// escaped dollar (`\$`) is a dollar sign, so it never opens math.
pub fn delimited(rest: &str) -> Option<(String, usize)> {
    if rest.starts_with("\\$") {
        return Some(("$".into(), 2));
    }
    for (open, close) in [("\\(", "\\)"), ("\\[", "\\]"), ("$$", "$$")] {
        if let Some(after) = rest.strip_prefix(open) {
            let end = after.find(close)?;
            let inner = &after[..end];
            if inner.trim().is_empty() {
                return None;
            }
            let used = open.len() + end + close.len();
            // `\[1\]` is a Markdown-escaped citation marker, not math: a
            // bare number or word keeps its brackets.
            if open.starts_with('\\') && is_escaped_word(inner) {
                let (l, r) = (&open[1..], &close[1..]);
                return Some((format!("{l}{inner}{r}"), used));
            }
            return Some((convert(inner), used));
        }
    }
    let after = rest.strip_prefix('$')?;
    // Pandoc's rule, so prices stay prices: `$` opens only before a
    // non-space, closes only after one and not right before a digit.
    if after.starts_with(char::is_whitespace) || after.starts_with('$') {
        return None;
    }
    let mut prev = ' ';
    for (i, c) in after.char_indices() {
        match c {
            '\n' => return None,
            '$' if prev == '\\' => {}
            '$' => {
                let inner = &after[..i];
                let next = after[i + 1..].chars().next();
                if prev.is_whitespace() || next.is_some_and(|n| n.is_ascii_digit()) {
                    return None;
                }
                // `$5 and 6$` reads as money: math starting with a digit
                // has to look like math.
                if inner.starts_with(|c: char| c.is_ascii_digit())
                    && !inner.contains(['\\', '^', '_', '{', '='])
                {
                    return None;
                }
                // `$HOME/$USER` names shell variables: single-dollar math
                // has to look like math.
                if !looks_like_math(inner) {
                    return None;
                }
                return Some((convert(inner), 1 + i + 1));
            }
            _ => {}
        }
        prev = c;
    }
    None
}

/// A bare number or word (`1`, `see`): what `\[…\]` or `\(…\)` around
/// it means in Markdown is escaped brackets. A single letter is math.
fn is_escaped_word(inner: &str) -> bool {
    let t = inner.trim();
    t.chars().all(char::is_alphanumeric)
        && (t.chars().all(|c| c.is_ascii_digit()) || t.chars().count() > 1)
}

/// Whether the inside of `$…$` reads as math rather than shell or prose:
/// a command, a script or a relation, a single letter, or a function
/// application like `f(x)`, and never something ending in punctuation the
/// way `$HOME/$` or `$PATH:$` does.
fn looks_like_math(inner: &str) -> bool {
    if inner.ends_with(['/', ':', ',', '.', ';']) {
        return false;
    }
    let mut chars = inner.chars();
    let first = chars.next();
    if first.is_some_and(char::is_alphabetic) && chars.next().is_none() {
        return true;
    }
    if inner.contains(['\\', '^', '_', '=', '<', '>', '+']) {
        return true;
    }
    first.is_some_and(char::is_alphabetic) && inner.contains('(') && inner.ends_with(')')
}

/// LaTeX math (without its delimiters) as Unicode.
pub fn convert(src: &str) -> String {
    let mut p = Parser {
        s: src,
        i: 0,
        frac: false,
        closers: Vec::new(),
    };
    let out = p.seq(false, false);
    tidy(&out)
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
    /// The command just rendered was a fraction.
    frac: bool,
    /// What each open environment closes with (`)` for `pmatrix`), and
    /// what separates its cells (`, ` between a case and its condition).
    closers: Vec<(&'static str, &'static str)>,
}

/// Operators shown with a space on each side outside scripts.
const SPACED: &[char] = &[
    '=', '+', '<', '>', '≤', '≥', '≠', '≈', '≡', '∼', '≃', '≅', '∝', '→', '←', '↔', '⇒', '⇐', '⇔',
    '↦', '∈', '∉', '⊂', '⊆', '⊃', '⊇', '±', '∓', '≪', '≫', '∣',
];

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        Some(c)
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    /// Everything up to the end, or (in a group) to its closing brace.
    /// `compact` (inside a script): no added spaces, source spaces dropped.
    fn seq(&mut self, group: bool, compact: bool) -> String {
        let mut out = String::new();
        // A fraction just written, as its byte range in `out`: a term or a
        // script right after it takes the whole fraction, so it gets
        // parentheses (`(1/2)mv²`, `(a/b)²`).
        let mut fraction: Option<(usize, usize)> = None;
        while let Some(c) = self.peek() {
            let before = out.len();
            let scripted = matches!(c, '^' | '_');
            self.frac = false;
            match c {
                '}' if group => {
                    self.bump();
                    return out;
                }
                '{' => {
                    self.bump();
                    out.push_str(&self.seq(true, compact));
                }
                '\\' => {
                    self.bump();
                    let piece = self.command(compact);
                    // `a \cdot b` reads `a·b`, as `2\times10` reads `2×10`.
                    if matches!(piece.as_str(), "×" | "·" | "÷" | "∘") && out.ends_with(' ') {
                        out.pop();
                    }
                    out.push_str(&piece);
                }
                '^' | '_' => {
                    self.bump();
                    let arg = self.arg(true);
                    out.push_str(&script(&arg, c == '^'));
                }
                '&' if self.closers.last().is_some_and(|(_, cell)| *cell == ", ") => {
                    self.bump();
                    out.push_str(", ");
                }
                '&' | '~' => {
                    self.bump();
                    if !compact {
                        out.push(' ');
                    }
                }
                c if c.is_whitespace() => {
                    self.bump();
                    if !compact {
                        out.push(' ');
                    }
                }
                '-' => {
                    self.bump();
                    out.push('−');
                }
                ',' | ';' => {
                    self.bump();
                    out.push(c);
                    if !compact {
                        out.push(' ');
                    }
                }
                c if SPACED.contains(&c) && !compact => {
                    self.bump();
                    out.push(' ');
                    out.push(c);
                    out.push(' ');
                }
                c => {
                    self.bump();
                    out.push(c);
                }
            }
            let just_frac = std::mem::take(&mut self.frac) && c == '\\';
            if let Some((start, end)) = fraction {
                let added = out.get(before..).unwrap_or_default().trim_start();
                if !added.is_empty() || scripted {
                    let joins = scripted
                        || added
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '(' | '√'));
                    if joins && end <= out.len() {
                        out.insert(end, ')');
                        out.insert(start, '(');
                    }
                    fraction = None;
                }
            }
            if just_frac {
                let piece = out.get(before..).unwrap_or_default();
                let start = before + (piece.len() - piece.trim_start().len());
                fraction = Some((start, out.len()));
            }
        }
        out
    }

    /// One argument: a group, a command, or a single character.
    fn arg(&mut self, compact: bool) -> String {
        self.skip_space();
        match self.peek() {
            Some('{') => {
                self.bump();
                self.seq(true, compact)
            }
            Some('\\') => {
                self.bump();
                self.command(compact)
            }
            Some('-') => {
                self.bump();
                "−".into()
            }
            Some(_) => self.bump().map(String::from).unwrap_or_default(),
            None => String::new(),
        }
    }

    /// A group's raw text (for `\text{…}`), or the next character.
    fn raw_arg(&mut self) -> String {
        self.skip_space();
        if self.peek() != Some('{') {
            return self.bump().map(String::from).unwrap_or_default();
        }
        self.bump();
        let start = self.i;
        let mut depth = 0usize;
        while let Some(c) = self.bump() {
            match c {
                '{' => depth += 1,
                '}' if depth == 0 => return self.s[start..self.i - 1].to_string(),
                '}' => depth -= 1,
                _ => {}
            }
        }
        self.s[start..].to_string()
    }

    /// The command after a backslash, rendered; notes whether it was a
    /// fraction.
    fn command(&mut self, compact: bool) -> String {
        let name: String = self.s[self.i..]
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();
        let out = self.command_body(compact);
        self.frac = matches!(name.as_str(), "frac" | "dfrac" | "tfrac" | "cfrac");
        out
    }

    fn command_body(&mut self, compact: bool) -> String {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.bump();
        }
        let name = &self.s[start..self.i];
        if name.is_empty() {
            return match self.bump() {
                Some(',' | ':' | ';' | ' ' | '>') if !compact => " ".into(),
                Some(',' | ':' | ';' | ' ' | '>' | '!') => String::new(),
                Some('\\') => "; ".into(),
                Some('|') => "‖".into(),
                Some(c @ ('{' | '}' | '%' | '$' | '&' | '#' | '_')) => c.to_string(),
                Some(c) => format!("\\{c}"),
                None => "\\".into(),
            };
        }
        if let Some(sym) = symbol(name) {
            self.skip_space();
            return if SPACED.iter().any(|c| sym.starts_with(*c)) && !compact {
                format!(" {sym} ")
            } else {
                sym.to_string()
            };
        }
        match name {
            "mathcal" | "mathscr" => map_letters(&self.arg(compact), script_letter),
            "mathbb" => map_letters(&self.arg(compact), double_struck),
            "mathrm" | "mathbf" | "mathit" | "mathsf" | "mathtt" | "mathnormal" | "mathfrak"
            | "boldsymbol" | "bm" | "operatorname" => self.arg(compact),
            "text" | "textrm" | "textit" | "textbf" | "textsf" | "texttt" | "textnormal"
            | "mbox" | "hbox" => self.raw_arg(),
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let a = self.arg(compact);
                let b = self.arg(compact);
                format!("{}/{}", wrap(&a), wrap(&b))
            }
            "sqrt" => {
                self.skip_space();
                let root = if self.peek() == Some('[') {
                    self.bump();
                    let start = self.i;
                    while self.peek().is_some_and(|c| c != ']') {
                        self.bump();
                    }
                    let n = convert(&self.s[start..self.i]);
                    self.bump();
                    match sup_all(&n) {
                        Some(s) => s,
                        None => format!("({n})"),
                    }
                } else {
                    String::new()
                };
                let x = self.arg(compact);
                format!("{root}√({x})")
            }
            "binom" | "dbinom" | "tbinom" => {
                let n = self.arg(compact);
                let k = self.arg(compact);
                format!("C({n}, {k})")
            }
            "hat" | "widehat" => accent(&self.arg(compact), '\u{302}'),
            "bar" | "overline" => accent(&self.arg(compact), '\u{305}'),
            "tilde" | "widetilde" => accent(&self.arg(compact), '\u{303}'),
            "vec" => accent(&self.arg(compact), '\u{20D7}'),
            "dot" => accent(&self.arg(compact), '\u{307}'),
            "ddot" => accent(&self.arg(compact), '\u{308}'),
            "underline" => accent(&self.arg(compact), '\u{332}'),
            "left" | "right" | "middle" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr"
            | "Bigl" | "Bigr" | "biggl" | "biggr" | "Biggl" | "Biggr" => {
                self.skip_space();
                if self.peek() == Some('.') {
                    self.bump();
                }
                String::new()
            }
            "begin" => {
                let env = self.raw_arg();
                let Some((open, close)) = environment(&env) else {
                    // Unknown: kept whole, as written.
                    let from = start - 1;
                    let end = format!("\\end{{{env}}}");
                    self.i = match self.s[self.i..].find(&end) {
                        Some(at) => self.i + at + end.len(),
                        None => self.s.len(),
                    };
                    return self.s[from..self.i].to_string();
                };
                if env == "array" || env == "tabular" {
                    self.raw_arg();
                }
                let cell = if env.ends_with("cases") { ", " } else { " " };
                self.closers.push((close, cell));
                open.to_string()
            }
            "end" => {
                self.raw_arg();
                self.closers
                    .pop()
                    .map(|(close, _)| close)
                    .unwrap_or_default()
                    .to_string()
            }
            "displaystyle" | "textstyle" | "scriptstyle" | "limits" | "nolimits" | "nonumber"
            | "notag" => {
                self.skip_space();
                String::new()
            }
            "quad" | "qquad" => {
                if compact {
                    String::new()
                } else {
                    "  ".into()
                }
            }
            n if OPERATORS.contains(&n) => {
                let save = self.i;
                self.skip_space();
                let next = self.peek();
                if next.is_some_and(|c| c.is_alphanumeric() || c == '\\') && !compact {
                    format!("{n} ")
                } else {
                    if next.is_none() {
                        self.i = save;
                    }
                    n.to_string()
                }
            }
            // Unknown: kept as written, with its arguments, so nothing it
            // means is lost.
            n => {
                let mut out = format!("\\{n}");
                while self.peek() == Some('{') {
                    out.push('{');
                    out.push_str(&self.raw_arg());
                    out.push('}');
                }
                out
            }
        }
    }
}

/// A known environment's opening and closing brackets: a matrix keeps its
/// delimiters, so a determinant never reads as a matrix.
fn environment(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "pmatrix" | "psmallmatrix" => ("(", ")"),
        "bmatrix" | "bsmallmatrix" => ("[", "]"),
        "Bmatrix" => ("{", "}"),
        "vmatrix" | "vsmallmatrix" => ("|", "|"),
        "Vmatrix" => ("‖", "‖"),
        "cases" | "dcases" => ("{ ", ""),
        "rcases" => ("", " }"),
        "matrix" | "smallmatrix" | "aligned" | "align" | "align*" | "alignat" | "alignat*"
        | "alignedat" | "equation" | "equation*" | "gather" | "gather*" | "gathered" | "split"
        | "array" | "eqnarray" | "eqnarray*" | "multline" | "multline*" | "flalign"
        | "flalign*" => ("", ""),
        _ => return None,
    })
}

/// Function names shown as words (`\sin x` → `sin x`).
const OPERATORS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
    "log", "ln", "lg", "exp", "max", "min", "sup", "inf", "lim", "liminf", "limsup", "det", "arg",
    "deg", "gcd", "dim", "ker", "Pr", "argmax", "argmin", "mod", "bmod", "tr",
];

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" | "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "times" => "×",
        "cdot" => "·",
        "div" => "÷",
        "pm" => "±",
        "mp" => "∓",
        "ast" => "∗",
        "star" => "⋆",
        "circ" => "∘",
        "bullet" => "•",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "le" | "leq" | "leqslant" => "≤",
        "ge" | "geq" | "geqslant" => "≥",
        "ne" | "neq" => "≠",
        "approx" => "≈",
        "sim" => "∼",
        "simeq" => "≃",
        "cong" => "≅",
        "equiv" => "≡",
        "propto" => "∝",
        "ll" => "≪",
        "gg" => "≫",
        "to" | "rightarrow" => "→",
        "leftarrow" | "gets" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "mapsto" => "↦",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "setminus" => "∖",
        "emptyset" | "varnothing" => "∅",
        "forall" => "∀",
        "exists" => "∃",
        "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "mid" => "∣",
        "parallel" => "∥",
        "perp" => "⊥",
        "top" => "⊤",
        "angle" => "∠",
        "infty" => "∞",
        "partial" => "∂",
        "nabla" => "∇",
        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        "ldots" | "dots" | "dotsc" | "dotsb" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "langle" => "⟨",
        "rangle" => "⟩",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "lbrace" => "{",
        "rbrace" => "}",
        "vert" | "lvert" | "rvert" => "|",
        "Vert" | "lVert" | "rVert" => "‖",
        "prime" => "′",
        "dagger" => "†",
        "ell" => "ℓ",
        "hbar" => "ℏ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "degree" => "°",
        _ => return None,
    })
}

/// `x` as a fraction's side: parenthesized unless it is one term, which
/// is one symbol or number with its scripts, a symbol applied to one
/// bracketed group (`f(x)`), a root (`√(2π)`) or one bracketed group.
/// A product like `2c` or `σ√(2π)` is more than one term.
fn wrap(x: &str) -> String {
    let x = x.trim();
    if one_term(x) {
        x.to_string()
    } else {
        format!("({x})")
    }
}

fn one_term(x: &str) -> bool {
    let c: Vec<char> = x.chars().collect();
    let n = c.len();
    let mut i = 0;
    // The end of the bracketed group opening at `i`.
    let group = |i: usize| -> Option<usize> {
        let mut depth = 0i32;
        for (j, ch) in c.iter().enumerate().skip(i) {
            match ch {
                '(' | '[' | '⟨' => depth += 1,
                ')' | ']' | '⟩' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j + 1);
                    }
                }
                _ => {}
            }
        }
        None
    };
    let is_script = |ch: char| {
        SUP.iter().any(|(_, t)| *t == ch)
            || SUB.iter().any(|(_, t)| *t == ch)
            || RAISED.contains(&ch)
            || ('\u{300}'..='\u{36F}').contains(&ch)
            || ('\u{20D0}'..='\u{20FF}').contains(&ch)
    };
    if n == 0 {
        return false;
    }
    // A root's index (`³√`).
    let mut k = i;
    while k < n && SUP.iter().any(|(_, t)| *t == c[k]) {
        k += 1;
    }
    if k < n && c[k] == '√' {
        i = k + 1;
        if c.get(i) != Some(&'(') {
            return false;
        }
        match group(i) {
            Some(e) => i = e,
            None => return false,
        }
    } else if matches!(c[0], '(' | '[' | '⟨') {
        match group(0) {
            Some(e) => i = e,
            None => return false,
        }
    } else if c[0].is_ascii_digit() {
        while i < n && (c[i].is_ascii_digit() || c[i] == '.') {
            i += 1;
        }
    } else if c[0].is_whitespace() || "+-−±∓·×÷/=<>≤≥≠≈,;∣|)]⟩_^".contains(c[0])
    {
        return false;
    } else {
        let letter = c[0].is_alphabetic();
        i = 1;
        while i < n && is_script(c[i]) {
            i += 1;
        }
        if letter && c.get(i) == Some(&'(') {
            match group(i) {
                Some(e) => i = e,
                None => return false,
            }
        }
    }
    while i < n && is_script(c[i]) {
        i += 1;
    }
    i == n
}

fn accent(x: &str, mark: char) -> String {
    let mut out = String::new();
    for c in x.chars() {
        out.push(c);
        if c.is_alphanumeric() {
            out.push(mark);
        }
    }
    out
}

fn map_letters(x: &str, f: fn(char) -> Option<char>) -> String {
    x.chars().map(|c| f(c).unwrap_or(c)).collect()
}

fn script_letter(c: char) -> Option<char> {
    Some(match c {
        'B' => 'ℬ',
        'E' => 'ℰ',
        'F' => 'ℱ',
        'H' => 'ℋ',
        'I' => 'ℐ',
        'L' => 'ℒ',
        'M' => 'ℳ',
        'R' => 'ℛ',
        'e' => 'ℯ',
        'g' => 'ℊ',
        'o' => 'ℴ',
        'A'..='Z' => char::from_u32(0x1D49C + (c as u32 - 'A' as u32))?,
        'a'..='z' => char::from_u32(0x1D4B6 + (c as u32 - 'a' as u32))?,
        _ => return None,
    })
}

fn double_struck(c: char) -> Option<char> {
    Some(match c {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        'A'..='Z' => char::from_u32(0x1D538 + (c as u32 - 'A' as u32))?,
        'a'..='z' => char::from_u32(0x1D552 + (c as u32 - 'a' as u32))?,
        '0'..='9' => char::from_u32(0x1D7D8 + (c as u32 - '0' as u32))?,
        _ => return None,
    })
}

const SUP: &[(char, char)] = &[
    ('0', '⁰'),
    ('1', '¹'),
    ('2', '²'),
    ('3', '³'),
    ('4', '⁴'),
    ('5', '⁵'),
    ('6', '⁶'),
    ('7', '⁷'),
    ('8', '⁸'),
    ('9', '⁹'),
    ('+', '⁺'),
    ('-', '⁻'),
    ('−', '⁻'),
    ('=', '⁼'),
    ('(', '⁽'),
    (')', '⁾'),
    ('a', 'ᵃ'),
    ('b', 'ᵇ'),
    ('c', 'ᶜ'),
    ('d', 'ᵈ'),
    ('e', 'ᵉ'),
    ('f', 'ᶠ'),
    ('g', 'ᵍ'),
    ('h', 'ʰ'),
    ('i', 'ⁱ'),
    ('j', 'ʲ'),
    ('k', 'ᵏ'),
    ('l', 'ˡ'),
    ('m', 'ᵐ'),
    ('n', 'ⁿ'),
    ('o', 'ᵒ'),
    ('p', 'ᵖ'),
    ('r', 'ʳ'),
    ('s', 'ˢ'),
    ('t', 'ᵗ'),
    ('u', 'ᵘ'),
    ('v', 'ᵛ'),
    ('w', 'ʷ'),
    ('x', 'ˣ'),
    ('y', 'ʸ'),
    ('z', 'ᶻ'),
    ('A', 'ᴬ'),
    ('B', 'ᴮ'),
    ('D', 'ᴰ'),
    ('E', 'ᴱ'),
    ('G', 'ᴳ'),
    ('H', 'ᴴ'),
    ('I', 'ᴵ'),
    ('J', 'ᴶ'),
    ('K', 'ᴷ'),
    ('L', 'ᴸ'),
    ('M', 'ᴹ'),
    ('N', 'ᴺ'),
    ('O', 'ᴼ'),
    ('P', 'ᴾ'),
    ('R', 'ᴿ'),
    ('T', 'ᵀ'),
    ('U', 'ᵁ'),
    ('V', 'ⱽ'),
    ('W', 'ᵂ'),
    ('⊤', 'ᵀ'),
    ('α', 'ᵅ'),
    ('β', 'ᵝ'),
    ('γ', 'ᵞ'),
    ('δ', 'ᵟ'),
    ('ε', 'ᵋ'),
    ('θ', 'ᶿ'),
    ('φ', 'ᵠ'),
    ('χ', 'ᵡ'),
];

const SUB: &[(char, char)] = &[
    ('0', '₀'),
    ('1', '₁'),
    ('2', '₂'),
    ('3', '₃'),
    ('4', '₄'),
    ('5', '₅'),
    ('6', '₆'),
    ('7', '₇'),
    ('8', '₈'),
    ('9', '₉'),
    ('+', '₊'),
    ('-', '₋'),
    ('−', '₋'),
    ('=', '₌'),
    ('(', '₍'),
    (')', '₎'),
    ('a', 'ₐ'),
    ('e', 'ₑ'),
    ('h', 'ₕ'),
    ('i', 'ᵢ'),
    ('j', 'ⱼ'),
    ('k', 'ₖ'),
    ('l', 'ₗ'),
    ('m', 'ₘ'),
    ('n', 'ₙ'),
    ('o', 'ₒ'),
    ('p', 'ₚ'),
    ('r', 'ᵣ'),
    ('s', 'ₛ'),
    ('t', 'ₜ'),
    ('u', 'ᵤ'),
    ('v', 'ᵥ'),
    ('x', 'ₓ'),
    ('β', 'ᵦ'),
    ('γ', 'ᵧ'),
    ('ρ', 'ᵨ'),
    ('φ', 'ᵩ'),
    ('χ', 'ᵪ'),
];

/// Marks that read the same raised (`f′`, `x*`).
const RAISED: &[char] = &['′', '″', '‴', '*', '∗', '†', '‡', '\''];

fn map_all(x: &str, table: &[(char, char)], keep: &[char]) -> Option<String> {
    x.chars()
        .map(|c| {
            table
                .iter()
                .find(|(from, _)| *from == c)
                .map(|(_, to)| *to)
                .or_else(|| keep.contains(&c).then_some(c))
        })
        .collect()
}

fn sup_all(x: &str) -> Option<String> {
    map_all(x, SUP, RAISED)
}

/// A super- or subscript: Unicode where every character has one, else
/// `^x` / `_x` (`^(…)` for more than one character). A nested script
/// (`2^{3^2}`, `\mu_{a_i}`) never has one, so it reads `2^(3²)` and
/// `μ_(aᵢ)`, not `2³²` and `μₐᵢ`.
fn script(arg: &str, sup: bool) -> String {
    let arg = arg.trim();
    if arg.is_empty() {
        return String::new();
    }
    let mapped = if sup {
        sup_all(arg)
    } else {
        map_all(arg, SUB, &[])
    };
    if let Some(m) = mapped {
        return m;
    }
    let mark = if sup { '^' } else { '_' };
    if arg.chars().count() == 1 {
        format!("{mark}{arg}")
    } else {
        format!("{mark}({arg})")
    }
}

/// Collapsed spaces, none before a comma or a closing bracket or after an
/// opening one.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ' ' {
            if out.is_empty() || out.ends_with([' ', '(', '[', '{', '⟨']) {
                continue;
            }
            out.push(' ');
            continue;
        }
        if matches!(c, ',' | ';' | ')' | ']' | '}' | '⟩') && out.ends_with(' ') {
            out.pop();
        }
        out.push(c);
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_research_demo_strings_read_as_unicode() {
        assert_eq!(
            prose(r"\(c_{ij}=\mathcal N(\mu_{a_i};\mu_{b_j},v_{a_i}+v_{b_j})\)"),
            "cᵢⱼ = 𝒩(μ_(aᵢ); μ_(bⱼ), v_(aᵢ) + v_(bⱼ))"
        );
        assert_eq!(prose(r"\(w_{a_i}w_{b_j}c_{ij}\)"), "w_(aᵢ)w_(bⱼ)cᵢⱼ");
        assert_eq!(prose(r"\(2.22\times10^{-16}\)"), "2.22×10⁻¹⁶");
        assert_eq!(
            prose(r"The weight is \(w_{a_i}w_{b_j}c_{ij}\), up to \(2.22\times10^{-16}\)."),
            "The weight is w_(aᵢ)w_(bⱼ)cᵢⱼ, up to 2.22×10⁻¹⁶."
        );
    }

    #[test]
    fn every_delimiter_is_rendered() {
        assert_eq!(prose(r"\[x^2 \le y\]"), "x² ≤ y");
        assert_eq!(prose(r"$$\alpha+\beta$$"), "α + β");
        assert_eq!(prose(r"so $x_1 \neq x_2$ here"), "so x₁ ≠ x₂ here");
        assert_eq!(prose(r"$a \approx b$"), "a ≈ b");
    }

    #[test]
    fn commands_read_as_their_symbols() {
        assert_eq!(convert(r"\frac{a}{b}"), "a/b");
        assert_eq!(convert(r"\frac{a+b}{2c}"), "(a + b)/(2c)");
        assert_eq!(convert(r"\frac{1}{\sigma\sqrt{2\pi}}"), "1/(σ√(2π))");
        assert_eq!(convert(r"\sqrt{x}"), "√(x)");
        assert_eq!(convert(r"\sqrt[3]{x}"), "³√(x)");
        assert_eq!(convert(r"\sum_{i=1}^{n} x_i"), "∑ᵢ₌₁ⁿ xᵢ");
        assert_eq!(convert(r"\prod_k p_k"), "∏ₖ pₖ");
        assert_eq!(convert(r"\int_0^1 f(x)\,dx"), "∫₀¹ f(x) dx");
        assert_eq!(convert(r"\left(\frac{a}{b}\right)"), "(a/b)");
        assert_eq!(convert(r"\left.x\right|"), "x|");
        assert_eq!(convert(r"x \ge 0 \text{ for all } x"), "x ≥ 0 for all x");
        assert_eq!(convert(r"\mathrm{Var}(X)"), "Var(X)");
        assert_eq!(convert(r"\mathbb{R}^n"), "ℝⁿ");
        assert_eq!(convert(r"\mathcal{L}"), "ℒ");
        assert_eq!(convert(r"x^{\top}A"), "xᵀA");
        assert_eq!(convert(r"f'(x)"), "f'(x)");
        assert_eq!(convert(r"e^{i\pi}"), "e^(iπ)");
        assert_eq!(convert(r"x_{\mathrm{max}}"), "xₘₐₓ");
        assert_eq!(convert(r"x_{\mathrm{b}}"), "x_b");
        assert_eq!(convert(r"\sin x \cdot \cos y"), "sin x·cos y");
        assert_eq!(convert(r"a \times b"), "a×b");
        assert_eq!(convert(r"\lim_{x\to 0}"), "lim_(x→0)");
        assert_eq!(convert(r"\Gamma(\alpha)"), "Γ(α)");
    }

    #[test]
    fn unknown_commands_keep_their_backslash() {
        assert_eq!(convert(r"\foo{x}"), r"\foo{x}");
        assert_eq!(convert(r"a \weird b"), r"a \weird b");
    }

    #[test]
    fn prices_and_code_stay_as_written() {
        for s in [
            "It costs $5 and saves $3 a month.",
            "$5-$10",
            "between $5 and 6$ per unit",
            "US$ 4",
            r"a `\(x\)` code span and `$a_b$`",
            "no math here",
            r"a lone \( without a close",
        ] {
            assert_eq!(prose(s), s, "{s}");
        }
        assert_eq!(prose(r"\$5 and \$6"), "$5 and $6");
        assert_eq!(prose(r"`\(x\)` but \(y^2\)"), r"`\(x\)` but y²");
    }

    #[test]
    fn fractions_keep_their_grouping() {
        assert_eq!(convert(r"\frac{1}{2}mv^2"), "(1/2)mv²");
        assert_eq!(convert(r"\frac{a}{b}^2"), "(a/b)²");
        assert_eq!(convert(r"\frac{1}{2} \pi"), "(1/2) π");
        assert_eq!(convert(r"\frac{a}{b} + c"), "a/b + c");
        assert_eq!(convert(r"\frac{a}{b}\cdot c"), "a/b·c");
        assert_eq!(convert(r"\frac{1}{\sqrt{2\pi}}"), "1/√(2π)");
        assert_eq!(convert(r"\frac{f(x)}{x^2}"), "f(x)/x²");
        assert_eq!(convert(r"\frac{\frac{a}{b}}{c}"), "(a/b)/c");
        assert_eq!(convert(r"x^\frac{1}{2} y"), "x^(1/2) y");
        assert_eq!(convert(r"\sqrt{\frac{a}{b}}c"), "√(a/b)c");
    }

    #[test]
    fn nested_scripts_stay_nested() {
        assert_eq!(convert(r"2^{3^2}"), "2^(3²)");
        assert_eq!(convert(r"10^{2^3}"), "10^(2³)");
        assert_eq!(convert(r"\mu_{a_i}"), "μ_(aᵢ)");
        assert_eq!(convert(r"\mu_{ai}"), "μₐᵢ");
        assert_eq!(convert(r"x^{-1}"), "x⁻¹");
        assert_eq!(convert(r"f^{\prime}"), "f′");
    }

    #[test]
    fn environments_keep_their_brackets() {
        let m = r"a & b \\ c & d";
        assert_eq!(
            convert(&format!(r"\begin{{pmatrix}} {m} \end{{pmatrix}}")),
            "(a b; c d)"
        );
        assert_eq!(
            convert(&format!(r"\begin{{bmatrix}} {m} \end{{bmatrix}}")),
            "[a b; c d]"
        );
        assert_eq!(
            convert(&format!(r"\begin{{vmatrix}} {m} \end{{vmatrix}}")),
            "| a b; c d |"
        );
        assert_eq!(
            convert(&format!(r"\begin{{Vmatrix}} {m} \end{{Vmatrix}}")),
            "‖ a b; c d ‖"
        );
        assert_eq!(
            convert(r"f(x) = \begin{cases} 1 & x > 0 \\ 0 & x \le 0 \end{cases}"),
            "f(x) = {1, x > 0; 0, x ≤ 0"
        );
        assert_eq!(
            convert(r"\begin{weird}{x} a \end{weird} + 1"),
            r"\begin{weird}{x} a \end{weird} + 1"
        );
    }

    #[test]
    fn escapes_and_shell_variables_stay_as_written() {
        assert_eq!(
            prose("copy it to $HOME/$USER now"),
            "copy it to $HOME/$USER now"
        );
        assert_eq!(prose("set $PATH:$HOME_DIR/bin"), "set $PATH:$HOME_DIR/bin");
        assert_eq!(
            prose(r"See \[1\] and `\(code\)`."),
            r"See [1] and `\(code\)`."
        );
        assert_eq!(prose(r"see \(note\) here"), "see (note) here");
        assert_eq!(prose(r"so \(x\) is free"), "so x is free");
        assert_eq!(prose("$x$ and $f(x)$"), "x and f(x)");
    }
}
