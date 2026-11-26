//! `/web` and online hits for questions: papers from OpenAlex and arXiv,
//! the general web from Brave Search. Every result is fetched, a raw line is
//! picked from the fetched text, and the line is verified against that text
//! before it is shown (`sources::web_hits`).

use std::time::Duration;

use serde_json::Value;

use crate::sources::{self, HEADING, Hit, WebResult};

/// The auth file / keychain entry for Brave Search.
pub const BRAVE_KEY: &str = "brave-api-key";
const BRAVE_ENV: &str = "BRAVE_API_KEY";

/// Results asked of each service.
const PER_SERVICE: usize = 4;
/// Largest page body read.
const MAX_PAGE: usize = 2 * 1024 * 1024;

/// What a search found, and what couldn't be searched (one line each).
#[derive(Debug, Default)]
pub struct WebFound {
    pub hits: Vec<Hit>,
    pub notes: Vec<String>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("pathfinder/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

fn brave_key() -> Option<String> {
    let creds = crate::auth::Credentials::system();
    creds
        .named_key(BRAVE_KEY, BRAVE_ENV)
        .map(|(k, _)| k)
        .or_else(|| {
            // The keychain item, copied in on first use.
            creds.import_named(BRAVE_KEY).ok().filter(|ok| *ok)?;
            creds.stored(BRAVE_KEY)
        })
}

/// Searches all services. Offline in `local_only` mode.
pub async fn search(terms: &[String]) -> WebFound {
    let mut found = WebFound::default();
    if terms.is_empty() {
        return found;
    }
    // Tests never reach the network.
    if cfg!(test) {
        found.notes.push("web search is off in tests".into());
        return found;
    }
    if crate::config::Config::load().local_only {
        found.notes.push("web search is off (`local_only`)".into());
        return found;
    }
    let q = terms.join(" ");
    let c = client();
    let key = brave_key();
    let (oa, ax, br) = tokio::join!(openalex(&c, &q), arxiv(&c, &q), async {
        match &key {
            Some(k) => brave(&c, &q, k).await,
            None => Err("Brave: no key (`pathfinder auth set brave`)".to_string()),
        }
    });
    let mut results: Vec<WebResult> = Vec::new();
    for r in [oa, ax, br] {
        match r {
            Ok(rs) => results.extend(rs),
            Err(e) => found.notes.push(e),
        }
    }
    let results: Vec<WebResult> = results
        .into_iter()
        .filter_map(|mut r| {
            let (section, line) = sources::pick_quote(&r.page_text, terms)?;
            r.section = section.or(r.section);
            r.quote = line;
            Some(r)
        })
        .collect();
    found.hits = sources::web_hits(&results);
    found
}

/// OpenAlex works: title and abstract (rebuilt from its inverted index).
async fn openalex(c: &reqwest::Client, q: &str) -> Result<Vec<WebResult>, String> {
    let v: Value = c
        .get("https://api.openalex.org/works")
        .query(&[
            ("search", q),
            ("per-page", &PER_SERVICE.to_string()),
            ("select", "id,doi,title,abstract_inverted_index"),
        ])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("OpenAlex: {e}"))?
        .json()
        .await
        .map_err(|e| format!("OpenAlex: {e}"))?;
    Ok(v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let url = w["doi"].as_str().or(w["id"].as_str())?.to_string();
            let title = w["title"].as_str().unwrap_or_default();
            let abs = rebuild_abstract(&w["abstract_inverted_index"]);
            Some(WebResult {
                url,
                section: Some("Abstract".into()),
                page_text: format!(
                    "{HEADING}Title\n{title}\n{HEADING}Abstract\n{}",
                    sentences(&abs)
                ),
                quote: String::new(),
            })
        })
        .collect())
}

/// OpenAlex stores abstracts as word → positions.
fn rebuild_abstract(index: &Value) -> String {
    let Some(map) = index.as_object() else {
        return String::new();
    };
    let mut words: Vec<(u64, &str)> = Vec::new();
    for (w, at) in map {
        for p in at
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
        {
            words.push((p, w));
        }
    }
    words.sort();
    words.iter().map(|(_, w)| *w).collect::<Vec<_>>().join(" ")
}

/// One sentence per line, so a quote is a sentence.
fn sentences(text: &str) -> String {
    let mut out = String::new();
    for w in text.split_whitespace() {
        out.push_str(w);
        let end = w.ends_with(['.', '?', '!']) && w.len() > 3;
        out.push(if end { '\n' } else { ' ' });
    }
    out
}

/// arXiv's Atom API: title and abstract per entry.
async fn arxiv(c: &reqwest::Client, q: &str) -> Result<Vec<WebResult>, String> {
    let body = c
        .get("https://export.arxiv.org/api/query")
        .query(&[
            ("search_query", format!("all:{q}").as_str()),
            ("max_results", &PER_SERVICE.to_string()),
        ])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("arXiv: {e}"))?
        .text()
        .await
        .map_err(|e| format!("arXiv: {e}"))?;
    Ok(parse_arxiv(&body))
}

fn parse_arxiv(atom: &str) -> Vec<WebResult> {
    let tag = |s: &str, name: &str| -> Option<String> {
        let re = regex::Regex::new(&format!(r"(?s)<{name}[^>]*>(.*?)</{name}>")).ok()?;
        re.captures(s).map(|c| decode_entities(c[1].trim()))
    };
    atom.split("<entry>")
        .skip(1)
        .filter_map(|e| {
            let url = tag(e, "id")?;
            let title = tag(e, "title").unwrap_or_default();
            let summary = tag(e, "summary").unwrap_or_default();
            Some(WebResult {
                url,
                section: Some("Abstract".into()),
                page_text: format!(
                    "{HEADING}Title\n{}\n{HEADING}Abstract\n{}",
                    title.split_whitespace().collect::<Vec<_>>().join(" "),
                    sentences(&summary)
                ),
                quote: String::new(),
            })
        })
        .collect()
}

/// Brave web results; each page is then fetched and read as text.
async fn brave(c: &reqwest::Client, q: &str, key: &str) -> Result<Vec<WebResult>, String> {
    let v: Value = c
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .query(&[("q", q), ("count", &PER_SERVICE.to_string())])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Brave: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Brave: {e}"))?;
    let urls: Vec<String> = v["web"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["url"].as_str().map(str::to_string))
        .take(PER_SERVICE)
        .collect();
    let pages = futures_util::future::join_all(urls.iter().map(|u| fetch_text(c, u))).await;
    Ok(urls
        .into_iter()
        .zip(pages)
        .filter_map(|(url, page)| {
            Some(WebResult {
                url,
                section: None,
                page_text: page.ok()?,
                quote: String::new(),
            })
        })
        .collect())
}

/// A page's readable text, headings marked with [`HEADING`].
async fn fetch_text(c: &reqwest::Client, url: &str) -> Result<String, String> {
    let r = c
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    let html_page = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_none_or(|t| t.contains("html"));
    let bytes = r.bytes().await.map_err(|e| e.to_string())?;
    let body = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_PAGE)]).into_owned();
    Ok(if html_page { html_text(&body) } else { body })
}

/// HTML to text: scripts and styles dropped, block tags as line breaks,
/// h1–h4 marked as headings, entities decoded.
pub fn html_text(html: &str) -> String {
    let mut html = html.to_string();
    for t in [
        "script", "style", "noscript", "svg", "nav", "footer", "header",
    ] {
        let re = regex::Regex::new(&format!(r"(?is)<{t}\b.*?</{t}\s*>")).unwrap();
        html = re.replace_all(&html, " ").into_owned();
    }
    let heading = regex::Regex::new(r"(?is)<h[1-4]\b[^>]*>(.*?)</h[1-4]\s*>").unwrap();
    let html = heading.replace_all(&html, |c: &regex::Captures| {
        format!("\n{HEADING}{}\n", c[1].replace('\n', " "))
    });
    let block =
        regex::Regex::new(r"(?i)</?(p|div|br|li|tr|section|article|blockquote|pre|dd|dt)\b[^>]*>")
            .unwrap();
    let html = block.replace_all(&html, "\n");
    let tag = regex::Regex::new(r"(?s)<[^>]*>").unwrap();
    let text = decode_entities(&tag.replace_all(&html, ""));
    text.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|l| match l.strip_prefix(HEADING) {
            Some(h) => format!("{HEADING}{}", h.trim()),
            None => l.replace(HEADING, ""),
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_entities(s: &str) -> String {
    let s = s
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&rsquo;", "’")
        .replace("&lsquo;", "‘")
        .replace("&ldquo;", "“")
        .replace("&rdquo;", "”")
        .replace("&ndash;", "–")
        .replace("&mdash;", "—");
    let num = regex::Regex::new(r"&#(\d+);").unwrap();
    let s = num.replace_all(&s, |c: &regex::Captures| {
        c[1].parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default()
    });
    s.replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openalex_abstracts_are_rebuilt_in_order() {
        let idx = serde_json::json!({"means": [1], "K": [0], "clusters": [2], "points.": [3]});
        assert_eq!(rebuild_abstract(&idx), "K means clusters points.");
    }

    #[test]
    fn arxiv_entries_parse() {
        let atom = r#"<feed><entry><id>http://arxiv.org/abs/1706.03762v7</id><title>Attention Is
  All You Need</title><summary> The dominant sequence transduction models are based on recurrent networks. We propose the Transformer.</summary></entry></feed>"#;
        let r = parse_arxiv(atom);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "http://arxiv.org/abs/1706.03762v7");
        assert!(r[0].page_text.contains("Attention Is All You Need"));
        // Too short a sentence to be a hit.
        assert!(sources::pick_quote(&r[0].page_text, &["transformer".into()]).is_none());
        let (_, line) = sources::pick_quote(&r[0].page_text, &["recurrent".into()]).unwrap();
        assert!(line.starts_with("The dominant sequence"));
    }

    #[test]
    fn html_becomes_text_with_headings() {
        let t = html_text(
            "<html><script>x()</script><h2>K-means <b>clustering</b></h2><p>The algorithm assigns points &amp; moves centroids.</p></html>",
        );
        assert_eq!(
            t,
            format!("{HEADING}K-means clustering\nThe algorithm assigns points & moves centroids.")
        );
    }
}
