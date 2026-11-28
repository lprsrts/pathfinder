//! `/web` and online hits for questions: papers from OpenAlex and arXiv,
//! the general web from Brave Search. Every result is fetched, a raw line is
//! picked from the fetched text, and the line is verified against that text
//! before it is shown (`sources::web_hits`).

use std::time::Duration;

use serde_json::Value;

use crate::sources::{self, HEADING, WebResult};

/// The auth file / keychain entry for Brave Search.
pub const BRAVE_KEY: &str = "brave-api-key";
const BRAVE_ENV: &str = "BRAVE_API_KEY";
/// OpenAlex works without a key, within a small daily allowance.
pub const OPENALEX_KEY: &str = "openalex-api-key";
const OPENALEX_ENV: &str = "OPENALEX_API_KEY";

/// Results asked of each service.
const PER_SERVICE: usize = 4;
/// Largest page body read.
const MAX_PAGE: usize = 2 * 1024 * 1024;

/// What a search found: candidate lines (each a result with its quote set,
/// not yet verified), and what couldn't be searched (one line each).
#[derive(Debug, Default)]
pub struct WebFound {
    pub candidates: Vec<WebResult>,
    pub notes: Vec<String>,
}

/// Candidate lines kept per page.
const PER_PAGE: usize = 2;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("pathfinder/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

/// A search key: the environment or the auth file. The keychain is read
/// only by `pathfinder auth import`, never during a search.
fn search_key(name: &str, env: &'static str) -> Option<String> {
    crate::auth::Credentials::system()
        .named_key(name, env)
        .map(|(k, _)| k)
}

/// Largest API response read (search results, abstracts).
const MAX_API: usize = 4 * 1024 * 1024;

/// A response body, read until `cap` bytes and no further: a page can't fill
/// memory however much it sends.
async fn read_capped(mut r: reqwest::Response, cap: usize) -> Result<Vec<u8>, reqwest::Error> {
    let mut out = Vec::new();
    while let Some(chunk) = r.chunk().await? {
        let room = cap - out.len();
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= cap {
            break;
        }
    }
    Ok(out)
}

/// A service's JSON reply, capped.
async fn json_capped(service: &str, r: reqwest::Response) -> Result<Value, String> {
    let body = read_capped(r, MAX_API)
        .await
        .map_err(|e| failed(service, &e))?;
    serde_json::from_slice(&body).map_err(|_| format!("{service} (unreadable reply)"))
}

/// Whether `ip` is on the public internet: not this computer, your network,
/// link-local (cloud metadata), shared or reserved space.
fn public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(a) => {
            let [o1, o2, ..] = a.octets();
            !(a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_documentation()
                || o1 == 0
                || (o1 == 100 && (o2 & 0xc0) == 64)
                || o1 >= 240)
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let first = a.segments()[0];
            !(a.is_loopback()
                || a.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// Whether a page URL may be fetched: http(s), and a public host.
fn public_url(url: &reqwest::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return public_ip(ip);
    }
    !(host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || !host.contains('.'))
}

/// Resolves names to public addresses only, so a page (or a redirect) whose
/// name points into your network isn't fetched.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| public_ip(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host} has no public address").into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The client for fetching result pages: public addresses only, including
/// every redirect.
fn page_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("pathfinder/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .dns_resolver(std::sync::Arc::new(PublicOnly))
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() >= 5 {
                a.error("too many redirects")
            } else if public_url(a.url()) {
                a.follow()
            } else {
                a.error("redirect to a non-public address")
            }
        }))
        .build()
        .unwrap_or_default()
}

/// Searches papers for every query and the web for the first, and returns
/// the lines of each result that hold at least half of `words`. Offline in
/// `local_only` mode and in tests.
pub async fn search(queries: &[String], words: &[String]) -> WebFound {
    let mut found = WebFound::default();
    if queries.is_empty() {
        return found;
    }
    if cfg!(test) {
        found.notes.push("web search is off in tests".into());
        return found;
    }
    if crate::config::Config::load().local_only {
        found.notes.push("web search is off (`local_only`)".into());
        return found;
    }
    let c = client();
    let key = search_key(BRAVE_KEY, BRAVE_ENV);
    let openalex_key = search_key(OPENALEX_KEY, OPENALEX_ENV);
    // One query at a time: the catalogues rate-limit bursts.
    let papers = async {
        let mut out = Vec::new();
        for q in queries {
            out.push(tokio::join!(
                openalex(&c, q, openalex_key.as_deref()),
                arxiv(&c, q)
            ));
        }
        out
    };
    let web = async {
        match &key {
            Some(k) => brave(&c, &queries[0], k).await,
            None => Err("Brave (no key: `pathfinder auth set brave`)".to_string()),
        }
    };
    let (papers, web) = tokio::join!(papers, web);
    let mut results: Vec<WebResult> = Vec::new();
    let mut add = |r: Result<Vec<WebResult>, String>, notes: &mut Vec<String>| match r {
        Ok(rs) => {
            for r in rs {
                let key = sources::source_key(&r.url);
                if !results.iter().any(|o| sources::source_key(&o.url) == key) {
                    results.push(r);
                }
            }
        }
        Err(e) => {
            if !notes.contains(&e) {
                notes.push(e)
            }
        }
    };
    for (oa, ax) in papers {
        add(oa, &mut found.notes);
        add(ax, &mut found.notes);
    }
    add(web, &mut found.notes);
    for r in results {
        for (section, line) in sources::quote_candidates(&r.page_text, words, PER_PAGE) {
            found.candidates.push(WebResult {
                url: r.url.clone(),
                section: section.or(r.section.clone()),
                page_text: r.page_text.clone(),
                quote: line,
            });
        }
    }
    found
}

/// A short note for a service that couldn't be searched.
fn failed(service: &str, e: &reqwest::Error) -> String {
    match e.status() {
        Some(s) if s.as_u16() == 429 && service == "OpenAlex" => {
            "OpenAlex (free daily allowance used up; a key: `pathfinder auth set openalex`)".into()
        }
        Some(s) if s.as_u16() == 429 => format!("{service} (rate limited, try again shortly)"),
        Some(s) => format!("{service} (HTTP {})", s.as_u16()),
        None if e.is_timeout() => format!("{service} (timed out)"),
        None => format!("{service} (unreachable)"),
    }
}

/// OpenAlex works: title and abstract (rebuilt from its inverted index).
async fn openalex(
    c: &reqwest::Client,
    q: &str,
    key: Option<&str>,
) -> Result<Vec<WebResult>, String> {
    let mut req = c.get("https://api.openalex.org/works").query(&[
        ("search", q),
        ("per-page", &PER_SERVICE.to_string()),
        ("select", "id,doi,title,abstract_inverted_index"),
    ]);
    if let Some(k) = key {
        req = req.query(&[("api_key", k)]);
    }
    let r = req
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| failed("OpenAlex", &e))?;
    let v = json_capped("OpenAlex", r).await?;
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
        .map_err(|e| failed("arXiv", &e))?;
    let body = read_capped(body, MAX_API)
        .await
        .map_err(|e| failed("arXiv", &e))?;
    Ok(parse_arxiv(&String::from_utf8_lossy(&body)))
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
    let r = c
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .query(&[("q", q), ("count", &PER_SERVICE.to_string())])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| failed("Brave", &e))?;
    let v = json_capped("Brave", r).await?;
    let urls: Vec<String> = v["web"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["url"].as_str().map(str::to_string))
        .take(PER_SERVICE)
        .collect();
    let pc = page_client();
    let pages = futures_util::future::join_all(urls.iter().map(|u| fetch_text(&pc, u))).await;
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
    let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    if !public_url(&parsed) {
        return Err(format!("{url} isn't a public address"));
    }
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
    let bytes = read_capped(r, MAX_PAGE).await.map_err(|e| e.to_string())?;
    let body = String::from_utf8_lossy(&bytes).into_owned();
    Ok(if html_page { html_text(&body) } else { body })
}

/// HTML to text: scripts and styles dropped, block tags as line breaks,
/// h1–h4 marked as headings, entities decoded.
pub fn html_text(html: &str) -> String {
    let mut html = html.to_string();
    for t in [
        "script", "style", "noscript", "svg", "nav", "footer", "header", "math",
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
    fn pages_on_your_own_network_are_never_fetched() {
        let ok = |u: &str| public_url(&reqwest::Url::parse(u).unwrap());
        assert!(ok("https://example.org/paper"));
        assert!(ok("http://93.184.215.14/"));
        for bad in [
            "http://127.0.0.1:11434/api",
            "http://localhost/",
            "http://192.168.1.10/",
            "http://10.0.0.2/",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://printer.local/",
            "http://intranet/",
            "file:///etc/passwd",
        ] {
            assert!(!ok(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_body_is_read_only_up_to_its_cap() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n")
                .await;
            let chunk = vec![b'x'; 64 * 1024];
            // An endless body: only the cap stops the read.
            while sock.write_all(&chunk).await.is_ok() {}
        });
        let r = reqwest::get(format!("http://{addr}/")).await.unwrap();
        let body = read_capped(r, 100_000).await.unwrap();
        assert_eq!(body.len(), 100_000);
    }

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
            "<html><script>x()</script><h2>K-means <b>clustering</b><math><mi>k</mi></math></h2><p>The algorithm assigns points &amp; moves centroids.</p></html>",
        );
        assert_eq!(
            t,
            format!("{HEADING}K-means clustering\nThe algorithm assigns points & moves centroids.")
        );
    }
}
