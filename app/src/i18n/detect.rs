//! Which language a message of yours is written in, by code alone: the
//! conversation's language follows your own messages, never the system
//! locale, the sources or a model's guess.

/// Function words of the Latin-script languages Pathfinder has lines for.
/// A word two of them share (German "es" and Spanish "es", "die") is left
/// out of both, so one word never decides between them.
const WORDS: &[(&str, &[&str])] = &[
    (
        "en",
        &[
            "the", "and", "is", "are", "what", "how", "why", "this", "that", "it", "you", "my",
            "me", "to", "of", "for", "with", "can", "does", "not", "have", "should", "would",
            "just", "get", "now", "if", "on", "at", "but", "please", "want", "need", "i", "do",
            "your", "there", "which", "where", "when", "it's", "don't", "i'm",
        ],
    ),
    (
        "de",
        &[
            "der", "die", "das", "und", "ist", "ich", "nicht", "wie", "was", "mit", "ein", "eine",
            "auf", "für", "auch", "zu", "den", "dem", "sich", "kann", "wir", "mir", "mich", "mein",
            "meine", "warum", "wo", "wenn", "aber", "oder", "noch", "schon", "jetzt", "bitte",
            "habe", "hab", "ja", "nein", "dass", "diese", "dieser", "von", "im", "wird", "sind",
            "verstehe", "macht",
        ],
    ),
    (
        "fr",
        &[
            "le", "la", "les", "et", "est", "je", "ne", "pas", "qui", "une", "des", "du", "pour",
            "avec", "ce", "cette", "mais", "comment", "pourquoi", "sur", "dans", "vous", "nous",
            "moi", "mon", "suis", "c'est", "j'ai",
        ],
    ),
    (
        "es",
        &[
            "el", "los", "las", "y", "pero", "qué", "cómo", "porque", "está", "esto", "esta", "yo",
            "muy", "también", "sí", "entiendo", "quiero", "hay",
        ],
    ),
    (
        "pt",
        &[
            "os", "não", "é", "um", "você", "isso", "isto", "eu", "meu", "minha", "com", "mas",
            "então", "ao", "dos", "muito", "sim", "quero",
        ],
    ),
    (
        "it",
        &[
            "il", "gli", "è", "sono", "che", "non", "questo", "questa", "perché", "anche", "io",
            "della", "nel", "cosa", "voglio", "capisco",
        ],
    ),
    (
        "tr",
        &[
            "bir",
            "ve",
            "bu",
            "için",
            "ile",
            "çok",
            "nasıl",
            "neden",
            "ben",
            "sen",
            "değil",
            "var",
            "yok",
            "ama",
            "şu",
            "gibi",
            "anladım",
            "istiyorum",
        ],
    ),
    (
        "id",
        &[
            "yang",
            "dan",
            "ini",
            "itu",
            "saya",
            "tidak",
            "apa",
            "dengan",
            "untuk",
            "ada",
            "bagaimana",
            "kenapa",
            "mengapa",
            "bisa",
            "aku",
            "kamu",
            "sudah",
            "juga",
        ],
    ),
    (
        "vi",
        &[
            "và", "là", "của", "không", "tôi", "này", "có", "được", "một", "những", "với", "thế",
            "nào", "gì", "bạn", "hiểu",
        ],
    ),
];

/// The two-letter code of the language `text` is written in, when it is
/// clear: a script of its own, or at least two function words of one
/// language and at most half as many of any other. `None` for a short or
/// mixed message ("ok", "thanks", a formula), which leaves the
/// conversation's language as it was. Quoted text and code are left out:
/// a line copied from a German bill is not you writing German.
pub fn detect(text: &str) -> Option<String> {
    let own = own_words(text);
    if let Some(code) = by_script(&own) {
        return Some(code.to_string());
    }
    let lower = own.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !(c.is_alphabetic() || c == '\''))
        .map(|w| w.trim_matches('\''))
        .filter(|w| !w.is_empty())
        .collect();
    let mut hits: Vec<(&str, usize)> = WORDS
        .iter()
        .map(|(code, list)| (*code, words.iter().filter(|w| list.contains(w)).count()))
        .collect();
    hits.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let (best, n) = hits[0];
    let second = hits[1].1;
    (n >= 2 && n >= 2 * second && n > second).then(|| best.to_string())
}

/// `text` without code (fenced or inline), quoted lines (`>`) and text in
/// quotation marks.
fn own_words(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.trim_start().starts_with('>') {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    let mut kept = String::new();
    let mut close: Option<char> = None;
    for c in out.chars() {
        match close {
            Some(end) if c == end => close = None,
            Some(_) => {}
            None => match c {
                '`' => close = Some('`'),
                '"' => close = Some('"'),
                '“' => close = Some('”'),
                '„' => close = Some('“'),
                '«' => close = Some('»'),
                _ => kept.push(c),
            },
        }
    }
    // An unclosed quote drops the rest; fall back to the whole text.
    if close.is_some() { out } else { kept }
}

/// The language of a script that only one of Pathfinder's languages
/// uses, when most of the letters are in it.
fn by_script(text: &str) -> Option<&'static str> {
    let (mut latin, mut kana, mut han, mut hangul, mut cyr, mut deva, mut beng, mut arab) =
        (0usize, 0, 0, 0, 0, 0, 0, 0);
    let mut urdu = false;
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        match c as u32 {
            0x3040..=0x30FF => kana += 1,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF => han += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF => hangul += 1,
            0x0400..=0x04FF => cyr += 1,
            0x0900..=0x097F => deva += 1,
            0x0980..=0x09FF => beng += 1,
            0x0600..=0x06FF | 0x0750..=0x077F => {
                arab += 1;
                // Letters Urdu has and Arabic doesn't: ٹ ڈ ڑ ں ے ہ گ.
                urdu |= matches!(c, 'ٹ' | 'ڈ' | 'ڑ' | 'ں' | 'ے' | 'ہ' | 'گ');
            }
            _ => latin += 1,
        }
    }
    let cjk = kana + han;
    let others = [cjk, hangul, cyr, deva, beng, arab];
    let most = *others.iter().max().unwrap_or(&0);
    if most < 2 || most <= latin {
        return None;
    }
    Some(if most == cjk {
        if kana > 0 { "ja" } else { "zh" }
    } else if most == hangul {
        "ko"
    } else if most == cyr {
        "ru"
    } else if most == deva {
        "hi"
    } else if most == beng {
        "bn"
    } else if urdu {
        "ur"
    } else {
        "ar"
    })
}

#[cfg(test)]
mod tests {
    use super::detect;

    #[test]
    fn your_own_words_decide_the_language() {
        let d = |t: &str| detect(t);
        assert_eq!(
            d("I want to understand my electricity bill").as_deref(),
            Some("en")
        );
        assert_eq!(
            d("Can you write the letter to the supplier?").as_deref(),
            Some("en")
        );
        assert_eq!(
            d("Ich will verstehen, was die Rechnung ist").as_deref(),
            Some("de")
        );
        assert_eq!(
            d("Je ne comprends pas cette facture").as_deref(),
            Some("fr")
        );
        assert_eq!(d("我想了解这个账单").as_deref(), Some("zh"));
        assert_eq!(d("この請求書を理解したい").as_deref(), Some("ja"));
        assert_eq!(d("Я хочу понять счёт").as_deref(), Some("ru"));
        assert_eq!(d("이 청구서를 이해하고 싶어요").as_deref(), Some("ko"));
    }

    #[test]
    fn a_short_or_mixed_message_decides_nothing() {
        for t in [
            "ok",
            "ok, macht Sinn",
            "thanks",
            "4,315 kWh × 0.32 €",
            "Arbeitspreis?",
            "hi",
        ] {
            assert_eq!(detect(t), None, "{t}");
        }
    }

    #[test]
    fn quotes_and_code_from_a_source_are_not_your_language() {
        // English around a German line copied from the bill.
        let t = "What does \"Der Arbeitspreis ist der Preis pro kWh und wird monatlich abgerechnet\" mean for the total?";
        assert_eq!(detect(t).as_deref(), Some("en"));
        let t = "Is this right?\n> Die Rechnung ist für den Zeitraum und das ist der Betrag\n";
        assert_eq!(detect(t).as_deref(), Some("en"));
        let t = "why does `die ist und das` fail?";
        assert_eq!(detect(t).as_deref(), Some("en"));
    }
}
