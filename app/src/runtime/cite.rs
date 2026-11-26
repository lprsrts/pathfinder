//! Sources only: questions get pointers and
//! verified verbatim hits; `/tell` is the one way to a reasoned answer, and
//! it is recorded as told. `/find`, `/open` and `/web` are the user's own
//! reading, recorded as events.

use super::*;
use crate::sources::{self, Hit};
use crate::understanding::{Concept, NewEvent, slugify};

const CITE_SYSTEM: &str = r#"You are a librarian's catalogue, not a tutor. You never answer, explain, summarise, conclude or calculate. You only point to where in the user's workspace sources the question is addressed.

Use your read-only tools (list, grep, read; PDFs are read by page) to search the workspace. Then return ONLY this JSON object, nothing before or after it:
{"hits": [{"file": "relative/path.pdf", "page": 3, "quote": "exact words copied from the source"}], "search_terms": ["term", "another term"]}

- `quote`: copy one or two lines character for character from the file, at most 200 characters. Never paraphrase, shorten inside, fix typos or translate. A quote that is not literally in the file is thrown away.
- `page`: the PDF page (1-based) or null for other files.
- At most 5 hits, the most directly relevant first. Several hits from different places are fine.
- If nothing in the workspace addresses the question, return "hits": [].
- `search_terms`: up to 5 short keyword phrases the user could search for next (no sentences).
- Never use knowledge that is not in the workspace."#;

const TELL_SYSTEM: &str = r#"The user typed /tell: they explicitly asked to be told the answer, and it is recorded as told (not understood). Answer what they asked, correctly and briefly, like a strong tutor. Where the workspace has a source for it, name the file and page. Write math as plain text or Unicode, never LaTeX. Reply in the user's language."#;

const CHECK_SYSTEM: &str = r#"The user shows their own work (a derivation, calculation, proof or code) and wants to know where it first goes wrong. You never explain why a step is wrong, never give the fix, the right result or a hint.

Find the first step that does not follow from what comes before it (or is wrong on its own). Return ONLY this JSON object:
{"first_wrong": "the step, copied character for character from their work"}
or, if every step holds:
{"first_wrong": null}

Copy one step or line exactly as they wrote it, at most 200 characters. A step that is not literally in their work is thrown away."#;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckReply {
    first_wrong: Option<String>,
}

/// Where a source event is filed when no idea on the record is named.
const SOURCES: &str = "sources";

impl Runtime {
    /// A question: pointers and verified verbatim hits from the workspace
    /// (or online when the workspace has none), or an honest "not found".
    /// No model text reaches the user.
    pub(super) async fn answer_question(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: StreamSink,
        _offer_check: bool,
    ) -> Result<TurnOutput> {
        let reply = self.cite_reply(session, text, files, &sink).await?;
        sink(StreamEvent::TextDelta(reply.clone()));
        session.push_turn("assistant", &reply);
        Ok(TurnOutput::text(String::new()))
    }

    /// The sourced reply to `text`, not yet shown.
    pub(super) async fn cite_reply(
        &self,
        session: &mut Session,
        text: &str,
        files: &[PathBuf],
        sink: &StreamSink,
    ) -> Result<String> {
        // `code`: no model, keyword search only.
        let raw = if crate::config::Config::load().sources_search == "code" {
            String::new()
        } else {
            let req = EngineRequest {
                tier: EngineTier::System2,
                tools: ToolAccess::ReadOnly,
                cwd: session.cwd.clone(),
                system_prompt: CITE_SYSTEM.to_string(),
                prompt: format!(
                    "Workspace snapshot:\n{}\n\nQuestion: {text}\n\nSearch, then return the JSON only.",
                    WorkspaceSnapshot::scan(&session.cwd).render()
                ),
                files: files.to_vec(),
                stream_text: false,
                models: self.models(session)?,
                ctx: crate::engine::turn_ctx(),
            };
            self.engine
                .run_text(&req, gate::held(sink))
                .await
                .unwrap_or_default()
        };
        let (mut hits, terms) = cited_hits(&session.cwd, &raw, text);
        // Nothing in the workspace: the same search online.
        let mut skipped = Vec::new();
        let online = hits.is_empty() && !terms.is_empty();
        if online {
            let found = crate::web::search(&terms).await;
            hits = found.hits;
            skipped = found.notes;
            if !hits.is_empty() {
                self.record_source(session, &terms.join(" "), EventKind::Searched, "web");
            }
        }
        // Only text built from verified hits is shown.
        Ok(sources::render(&hits, &terms, online, &skipped))
    }

    /// `/tell <question>`: the one reasoned answer, recorded as told.
    pub(super) async fn handle_tell(
        &self,
        session: &mut Session,
        arg: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        if arg.is_empty() {
            return Ok(TurnOutput::text(
                "`/tell <question>`: told answer, recorded as told.",
            ));
        }
        session.push_turn("user", format!("/tell {arg}"));
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::ReadOnly,
            cwd: session.cwd.clone(),
            system_prompt: TELL_SYSTEM.to_string(),
            prompt: format!(
                "Conversation so far:\n{}\n\nAnswer: {arg}",
                format_history(session)
            ),
            files: files.to_vec(),
            stream_text: true,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let header = "**Told** (recorded as told, not as understood)\n\n";
        sink(StreamEvent::TextDelta(header.into()));
        let answer = self.engine.run_text(&req, sink.clone()).await?;
        let mut told = self.record_told(session, arg);
        if told.is_empty() {
            // Nothing on the record is named: file it under the question, so
            // the told mark shows on the record and graph.
            let label = sources::keywords(arg)
                .into_iter()
                .take(4)
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(id) = self.own_concept(session, &label) {
                self.append_events(session, vec![NewEvent::new(&id, EventKind::Told)]);
                told.push(label);
            }
        }
        let tail = format!(
            "\n\n{}",
            i18n::line(session.language.as_deref(), i18n::Line::ToldNotShown)
                .replace("{labels}", &told.join(", "))
        );
        sink(StreamEvent::TextDelta(tail.clone()));
        session.push_turn("assistant", format!("{header}{answer}{tail}"));
        Ok(TurnOutput::text(String::new()))
    }

    /// `/check <your work>` or `/check <file>`: the first step that doesn't
    /// hold, quoted from the work itself, with no reason and no fix.
    pub(super) async fn handle_check(
        &self,
        session: &mut Session,
        arg: &str,
        files: &[PathBuf],
        sink: StreamSink,
    ) -> Result<TurnOutput> {
        // The work: a workspace file named as the argument, else the text.
        let (work, file) = match sources::confine(&session.cwd, arg) {
            Some(full) => match sources::pages(&full, arg) {
                Ok(pages) => (pages.join("\n"), Some(arg.trim().to_string())),
                Err(e) => return Ok(TurnOutput::text(e)),
            },
            None => (arg.to_string(), None),
        };
        if work.trim().is_empty() && files.is_empty() {
            return Ok(TurnOutput::text(
                "`/check <your work>` or `/check <file>`: the first step that doesn't hold.",
            ));
        }
        session.push_turn("user", format!("/check {arg}"));
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::None,
            cwd: session.cwd.clone(),
            system_prompt: CHECK_SYSTEM.to_string(),
            prompt: format!("Their work:\n{work}\n\nReturn the JSON only."),
            files: files.to_vec(),
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let raw = self.engine.run_text(&req, gate::held(&sink)).await?;
        let reply = render_check(&work, file.as_deref(), &raw);
        session.push_turn("assistant", &reply);
        Ok(TurnOutput::text(reply))
    }

    /// `/find <terms>`: raw matching lines, recorded as searched.
    pub(super) fn handle_find(&self, session: &mut Session, arg: &str) -> TurnOutput {
        let terms: Vec<String> = arg.split_whitespace().map(str::to_string).collect();
        if terms.is_empty() {
            return TurnOutput::text("`/find <terms>`: search the workspace's files.");
        }
        let hits = sources::find(&session.cwd, &terms, 12);
        self.record_source(session, arg, EventKind::Searched, &format!("find:{arg}"));
        let reply = if hits.is_empty() {
            format!("No line in this workspace contains `{arg}`.")
        } else {
            let mut out = format!("**{}** for `{arg}`:\n", hits.len());
            for h in &hits {
                out.push_str(&format!("\n- `{}`  {}", h.at, h.quote));
            }
            out.push_str("\n\n`/open <file> [page or line]` to read.");
            out
        };
        session.push_turn("user", format!("/find {arg}"));
        session.push_turn("assistant", &reply);
        TurnOutput::text(reply)
    }

    /// `/open <file> [page|line]`, also `file:line`: the source, numbered,
    /// recorded as read.
    pub(super) fn handle_open(&self, session: &mut Session, arg: &str) -> TurnOutput {
        let (file, at) = parse_open(arg);
        if file.is_empty() {
            return TurnOutput::text("`/open <file> [page or line]`");
        }
        let reply = match sources::open(&session.cwd, &file, at) {
            Ok(view) => {
                let r = match at {
                    Some(n) => format!("{file}#{n}"),
                    None => file.clone(),
                };
                self.record_source(session, &file, EventKind::Read, &r);
                view
            }
            Err(e) => e,
        };
        session.push_turn("user", format!("/open {arg}"));
        TurnOutput::text(reply)
    }

    /// `/web <terms>`: raw web hits, each checked against the fetched page.
    pub(super) async fn handle_web(&self, session: &mut Session, arg: &str) -> TurnOutput {
        let terms: Vec<String> = arg.split_whitespace().map(str::to_string).collect();
        if terms.is_empty() {
            return TurnOutput::text("`/web <terms>`: search papers and the web for raw hits.");
        }
        self.record_source(session, arg, EventKind::Searched, &format!("web:{arg}"));
        let found = crate::web::search(&terms).await;
        let reply = if found.hits.is_empty() {
            let mut out = format!("Nothing online matched `{arg}`.");
            if !found.notes.is_empty() {
                out.push_str(&format!("\n\nNot searched: {}.", found.notes.join("; ")));
            }
            out
        } else {
            sources::render(&found.hits, &[], true, &found.notes)
        };
        session.push_turn("user", format!("/web {arg}"));
        session.push_turn("assistant", &reply);
        TurnOutput::text(reply)
    }

    /// Files reading/searching on the ideas `text` names, else on `sources`.
    fn record_source(&self, session: &Session, text: &str, kind: EventKind, reference: &str) {
        let concepts = self
            .store
            .concepts(&Self::workspace(session))
            .unwrap_or_default();
        let named = crate::understanding::named_in(text, &concepts);
        let ids: Vec<String> = if named.is_empty() {
            // The reading itself becomes a node: the file, or the terms.
            self.own_concept(session, text)
                .map_or_else(|| vec![SOURCES.to_string()], |id| vec![id])
        } else {
            named.iter().map(|c| c.id.clone()).collect()
        };
        self.append_events(
            session,
            ids.iter()
                .map(|id| NewEvent::new(id, kind).reference(reference))
                .collect(),
        );
    }
}

impl Runtime {
    /// A user-made idea labelled `label`, created if new; its id.
    fn own_concept(&self, session: &Session, label: &str) -> Option<String> {
        let label = label.trim();
        let id = slugify(label);
        if id.is_empty() {
            return None;
        }
        let concept = Concept {
            id: id.clone(),
            label: label.to_string(),
            aliases: Vec::new(),
            created_by: "user".into(),
        };
        self.store
            .upsert_concept(&Self::workspace(session), &concept)
            .ok()?;
        Some(id)
    }
}

/// Verified hits from the model's reply, or, if it broke the contract or
/// found nothing that checks out, from a search done by code alone.
pub(crate) fn cited_hits(
    cwd: &std::path::Path,
    raw: &str,
    question: &str,
) -> (Vec<Hit>, Vec<String>) {
    let keywords = sources::keywords(question);
    let (mut hits, terms) = match sources::parse_reply(raw) {
        Ok(reply) => {
            let mut hits: Vec<Hit> = Vec::new();
            for p in &reply.hits {
                match sources::verify_local(cwd, p) {
                    Ok(h) if !hits.contains(&h) => hits.push(h),
                    Ok(_) => {}
                    Err(why) => tracing::info!("dropped a cited hit: {why:?}"),
                }
            }
            (hits, sources::clean_terms(&reply.search_terms))
        }
        Err(why) => {
            tracing::info!("cite reply rejected: {why}");
            (Vec::new(), Vec::new())
        }
    };
    if hits.is_empty() && keywords.len() >= 2 {
        // Only lines holding at least two of the question's words count.
        hits = sources::find(cwd, &keywords, sources::MAX_HITS * 4)
            .into_iter()
            .filter(|h| {
                let low = h.quote.to_lowercase();
                keywords.iter().filter(|k| low.contains(k.as_str())).count() >= 2
            })
            .take(3)
            .collect();
    }
    hits.truncate(sources::MAX_HITS);
    let terms = if terms.is_empty() {
        keywords.into_iter().take(3).collect()
    } else {
        terms
    };
    (hits, terms)
}

/// The `/check` reply: the first wrong step if the model's quote is really
/// in the work, else nothing it said.
pub(crate) fn render_check(work: &str, file: Option<&str>, raw: &str) -> String {
    let unclear =
        "Couldn't point to a step in your work. Write it out one step per line and `/check` again.";
    let t = raw
        .trim()
        .trim_start_matches("```json")
        .trim_matches('`')
        .trim();
    let Ok(reply) = serde_json::from_str::<CheckReply>(t) else {
        return unclear.into();
    };
    let Some(step) = reply.first_wrong.filter(|s| !s.trim().is_empty()) else {
        return "**No step found that doesn't hold.**".into();
    };
    let lines: Vec<&str> = work.lines().collect();
    let Some((line, own)) = sources::locate(&lines, &step) else {
        return unclear.into();
    };
    let at = match file {
        Some(f) => format!("`{f}:{line}`"),
        None => format!("line {line}"),
    };
    format!(
        "**First step that doesn't hold** ({at}):\n\n> {}\n\nWhy is for you to find: `/find`, `/open`, or `/tell` (recorded as told).",
        sources::cap(&own)
    )
}

/// `paper.pdf 3`, `notes.md:120` or `notes.md`.
fn parse_open(arg: &str) -> (String, Option<usize>) {
    let arg = arg.trim();
    if let Some((f, n)) = arg.rsplit_once(char::is_whitespace)
        && let Ok(n) = n.trim().parse()
    {
        return (f.trim().to_string(), Some(n));
    }
    if let Some((f, n)) = arg.rsplit_once(':')
        && let Ok(n) = n.parse()
    {
        return (f.to_string(), Some(n));
    }
    (arg.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_names_the_step_and_nothing_else() {
        let work = "x^2 - 4 = 0\n(x - 2)(x + 2) = 0\nx = 2 or x = 4";
        let out = render_check(work, None, r#"{"first_wrong": "x = 2 or x = 4"}"#);
        assert!(
            out.starts_with("**First step that doesn't hold** (line 3):"),
            "{out}"
        );
        assert!(out.contains("> x = 2 or x = 4"));
        // A reason smuggled in is a broken contract.
        let out = render_check(
            work,
            None,
            r#"{"first_wrong": "x = 2 or x = 4", "why": "x + 2 = 0 gives -2"}"#,
        );
        assert!(out.starts_with("Couldn't point"));
        // A step that isn't in the work is not shown.
        let out = render_check(work, None, r#"{"first_wrong": "x = -2"}"#);
        assert!(out.starts_with("Couldn't point"));
        assert_eq!(
            render_check(work, Some("w.md"), r#"{"first_wrong": null}"#),
            "**No step found that doesn't hold.**"
        );
        assert!(
            render_check(
                work,
                Some("w.md"),
                r#"{"first_wrong":"(x - 2)(x + 2) = 0"}"#
            )
            .contains("(`w.md:2`)")
        );
    }

    #[test]
    fn open_arguments() {
        assert_eq!(parse_open("paper.pdf 3"), ("paper.pdf".into(), Some(3)));
        assert_eq!(parse_open("notes.md:12"), ("notes.md".into(), Some(12)));
        assert_eq!(parse_open("my notes.md"), ("my notes.md".into(), None));
    }

    #[test]
    fn model_prose_never_survives_the_contract() {
        let d = std::env::temp_dir().join(format!("pf-cite-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("n.md"), "Phasors rotate at the angular frequency.\n").unwrap();
        // Prose answer: rejected; code finds the line itself.
        let (hits, _) = cited_hits(
            &d,
            "Phasors are rotating vectors, so...",
            "why do phasors rotate",
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].quote, "Phasors rotate at the angular frequency.");
        // An invented quote is dropped; nothing else matches: not found.
        let (hits, terms) = cited_hits(
            &d,
            r#"{"hits":[{"file":"n.md","page":null,"quote":"Phasors are complex amplitudes."}],"search_terms":["phasor"]}"#,
            "what is impedance",
        );
        assert!(hits.is_empty());
        assert_eq!(terms, ["phasor"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
