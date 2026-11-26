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

const TRANSCRIBE_SYSTEM: &str = r#"You copy handwritten or photographed work into text, exactly as written. You are a scanner: never fix a mistake, never fill in a missing step, never reorder, never comment, never solve anything. A wrong step must stay wrong. Keep the user's own notation, written as plain text or Unicode (x², √, ∫, →), never LaTeX. One step or line of the work per entry, in the order written; crossed-out text is left out. Where something can't be read, write [?] in its place.

Return ONLY this JSON object:
{"lines": ["first line as written", "second line as written"]}
If there is no work in the image, return {"lines": []}."#;

/// The lines of a transcription; `None` if the reply is broken or empty.
fn parse_transcription(raw: &str) -> Option<Vec<String>> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct T {
        lines: Vec<String>,
    }
    let t = raw
        .trim()
        .trim_start_matches("```json")
        .trim_matches('`')
        .trim();
    let lines: Vec<String> = serde_json::from_str::<T>(t)
        .ok()?
        .lines
        .into_iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(200)
        .collect();
    (!lines.is_empty()).then_some(lines)
}

/// Images the model reads (the formats every route accepts).
fn is_photo(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp"
        )
    })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckReply {
    first_wrong: Option<String>,
}

const FOCUS_SYSTEM: &str = r#"You turn a question into web search queries for papers and pages that address it. You never answer the question.

Return ONLY this JSON object:
{"queries": ["query one", "query two"]}

At most 3 queries, each a few precise keywords (names, technical terms, the paper or field it is about), at most 10 words, no question marks. The first one goes to the general web, all of them to paper catalogues."#;

const PICK_SYSTEM: &str = r#"You pick which numbered lines from search results bear directly on the user's question: lines a reader would want to open to answer it themselves. You never answer the question.

Return ONLY this JSON object:
{"pick": [3, 0]}

The most relevant first, at most 5. A line must be about the same people, work and thing the question means; the files the user is working with tell you what "the paper" or "the authors" refers to. Leave out lines that only share words with the question but are about someone or something else. If none bear on it, return {"pick": []}."#;

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
            (hits, skipped) = self.online_hits(session, text, &terms, true, sink).await;
            if !hits.is_empty() {
                self.record_source(session, &terms.join(" "), EventKind::Searched, "web");
            }
        }
        // Only text built from verified hits is shown.
        Ok(sources::render(
            session.language.as_deref(),
            &hits,
            &terms,
            online,
            &skipped,
        ))
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
            return Ok(TurnOutput::text("`/tell <question>`"));
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
        let header = format!(
            "{}\n\n",
            i18n::line(session.language.as_deref(), i18n::Line::Told)
        );
        sink(StreamEvent::TextDelta(header.clone()));
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
        // The work: a photo or scan (named or attached), a text file, or the
        // text itself.
        let named = sources::confine(&session.cwd, arg);
        let mut photos: Vec<PathBuf> = files.iter().filter(|f| is_photo(f)).cloned().collect();
        let mut file = None;
        let mut work = String::new();
        match &named {
            Some(full) if is_photo(full) => photos.push(full.clone()),
            Some(full) => match sources::pages(full, arg) {
                // A scanned PDF has no text layer: read it as a photo.
                Ok(pages) if pages.iter().all(|p| p.trim().is_empty()) => photos.push(full.clone()),
                Ok(pages) => {
                    work = pages.join("\n");
                    file = Some(arg.trim().to_string());
                }
                Err(e) => return Ok(TurnOutput::text(e)),
            },
            None => work = arg.to_string(),
        }
        if work.trim().is_empty() && photos.is_empty() {
            return Ok(TurnOutput::text("`/check <work, file or photo>`"));
        }
        session.push_turn("user", format!("/check {arg}"));
        let mut shown = String::new();
        if !photos.is_empty() {
            let raw = self
                .engine
                .run_text(
                    &EngineRequest {
                        tier: EngineTier::System2,
                        tools: ToolAccess::None,
                        cwd: session.cwd.clone(),
                        system_prompt: TRANSCRIBE_SYSTEM.to_string(),
                        prompt: "Copy the work in the attached image(s). Return the JSON only."
                            .into(),
                        files: photos.clone(),
                        stream_text: false,
                        models: self.models(session)?,
                        ctx: crate::engine::turn_ctx(),
                    },
                    gate::held(&sink),
                )
                .await?;
            let Some(lines) = parse_transcription(&raw) else {
                let reply = i18n::line(session.language.as_deref(), i18n::Line::PhotoUnreadable);
                session.push_turn("assistant", reply);
                return Ok(TurnOutput::text(reply));
            };
            work = lines.join("\n");
            let names: Vec<String> = photos
                .iter()
                .map(|p| {
                    p.strip_prefix(&session.cwd)
                        .unwrap_or(p)
                        .display()
                        .to_string()
                })
                .collect();
            let files = format!("`{}`", names.join("`, `"));
            shown = format!(
                "{}\n\n```text\n{}```\n\n",
                i18n::line(session.language.as_deref(), i18n::Line::PhotoCopy)
                    .replace("{files}", &files),
                lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{:>3}  {l}\n", i + 1))
                    .collect::<String>()
            );
        }
        let req = EngineRequest {
            tier: EngineTier::System2,
            tools: ToolAccess::None,
            cwd: session.cwd.clone(),
            system_prompt: CHECK_SYSTEM.to_string(),
            prompt: format!("Their work:\n{work}\n\nReturn the JSON only."),
            // The step is checked against text; images were copied above.
            files: files.iter().filter(|f| !is_photo(f)).cloned().collect(),
            stream_text: false,
            models: self.models(session)?,
            ctx: crate::engine::turn_ctx(),
        };
        let raw = self.engine.run_text(&req, gate::held(&sink)).await?;
        let reply = format!(
            "{shown}{}",
            render_check(session.language.as_deref(), &work, file.as_deref(), &raw)
        );
        session.push_turn("assistant", &reply);
        Ok(TurnOutput::text(reply))
    }

    /// `/find <terms>`: raw matching lines, recorded as searched.
    pub(super) fn handle_find(&self, session: &mut Session, arg: &str) -> TurnOutput {
        let terms: Vec<String> = arg.split_whitespace().map(str::to_string).collect();
        if terms.is_empty() {
            return TurnOutput::text("`/find <terms>`");
        }
        let hits = sources::find(&session.cwd, &terms, 12);
        self.record_source(session, arg, EventKind::Searched, &format!("find:{arg}"));
        let reply = if hits.is_empty() {
            i18n::line(session.language.as_deref(), i18n::Line::FindNone)
                .replace("{terms}", &format!("`{arg}`"))
        } else {
            let mut out = format!(
                "{}\n",
                i18n::line(session.language.as_deref(), i18n::Line::FindHits)
                    .replace("{n}", &hits.len().to_string())
                    .replace("{terms}", &format!("`{arg}`"))
            );
            for h in &hits {
                out.push_str(&format!("\n- `{}`  {}", h.at, h.quote));
            }
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
    pub(super) async fn handle_web(
        &self,
        session: &mut Session,
        arg: &str,
        sink: StreamSink,
    ) -> TurnOutput {
        let terms: Vec<String> = arg.split_whitespace().map(str::to_string).collect();
        if terms.is_empty() {
            return TurnOutput::text("`/web <terms>`");
        }
        self.record_source(session, arg, EventKind::Searched, &format!("web:{arg}"));
        let (hits, notes) = self.online_hits(session, arg, &terms, false, &sink).await;
        let reply = if hits.is_empty() {
            let lang = session.language.as_deref();
            let mut out =
                i18n::line(lang, i18n::Line::WebNone).replace("{terms}", &format!("`{arg}`"));
            if !notes.is_empty() {
                out.push_str("\n\n");
                out.push_str(
                    &i18n::line(lang, i18n::Line::NotSearched)
                        .replace("{services}", &notes.join(", ")),
                );
            }
            out
        } else {
            sources::render(session.language.as_deref(), &hits, &[], true, &notes)
        };
        session.push_turn("user", format!("/web {arg}"));
        session.push_turn("assistant", &reply);
        TurnOutput::text(reply)
    }

    /// Online hits for `question`: the model turns it into focused queries,
    /// code searches and extracts candidate lines, the model picks the ones
    /// that bear on the question, and code verifies each against its page.
    /// With `sources_search: "code"`, no model: the terms as the query and
    /// the best line per page. Also returns what couldn't be searched.
    /// `from_workspace`: the question came up while reading the workspace,
    /// so its files tell what "the paper" means; for `/web` the terms alone
    /// are the topic.
    async fn online_hits(
        &self,
        session: &Session,
        question: &str,
        terms: &[String],
        from_workspace: bool,
        sink: &StreamSink,
    ) -> (Vec<Hit>, Vec<String>) {
        let context = if from_workspace {
            format!(
                "Files the user is working with:\n{}\n",
                workspace_files(&session.cwd)
            )
        } else {
            String::new()
        };
        if cfg!(test) {
            return (Vec::new(), vec!["web search is off in tests".into()]);
        }
        let use_model = crate::config::Config::load().sources_search != "code";
        let mut queries = Vec::new();
        if use_model {
            let raw = self
                .small_call(
                    session,
                    FOCUS_SYSTEM,
                    format!(
                        "{context}Question: {question}\nSearch terms so far: {}",
                        terms.join(", ")
                    ),
                    sink,
                )
                .await;
            queries = sources::parse_queries(&raw);
        }
        if queries.is_empty() {
            queries.push(terms.join(" "));
        }
        // Lines are kept by the user's own terms, not the model's queries.
        let words = sources::keywords(&terms.join(" "));
        let found = crate::web::search(&queries, &words).await;
        let cands = found.candidates;
        let picked: Vec<sources::WebResult> = match use_model && !cands.is_empty() {
            true => {
                let list: String = cands
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let line: String = c.quote.chars().take(300).collect();
                        format!("[{i}] {}: {line}\n", c.url)
                    })
                    .collect();
                let raw = self
                    .small_call(
                        session,
                        PICK_SYSTEM,
                        format!("{context}Question: {question}\n\nLines:\n{list}"),
                        sink,
                    )
                    .await;
                match sources::parse_pick(&raw, cands.len()) {
                    Some(ids) => ids.into_iter().map(|i| cands[i].clone()).collect(),
                    None => best_per_page(cands),
                }
            }
            false => best_per_page(cands),
        };
        (sources::web_hits(&picked), found.notes)
    }

    /// A short JSON-only call to the fast model, with no tools; "" on error.
    async fn small_call(
        &self,
        session: &Session,
        system: &str,
        prompt: String,
        sink: &StreamSink,
    ) -> String {
        let Ok(models) = self.models(session) else {
            return String::new();
        };
        let req = EngineRequest {
            tier: EngineTier::System1,
            tools: ToolAccess::None,
            cwd: session.cwd.clone(),
            system_prompt: system.to_string(),
            prompt: format!("{prompt}\n\nReturn the JSON only."),
            files: vec![],
            stream_text: false,
            models,
            ctx: crate::engine::turn_ctx(),
        };
        self.engine
            .run_text(&req, gate::held(sink))
            .await
            .unwrap_or_default()
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
                keywords
                    .iter()
                    .filter(|k| sources::mentions(&h.quote, k))
                    .count()
                    >= 2
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
pub(crate) fn render_check(
    lang: Option<&str>,
    work: &str,
    file: Option<&str>,
    raw: &str,
) -> String {
    let unclear = i18n::line(lang, i18n::Line::CheckUnclear);
    let t = raw
        .trim()
        .trim_start_matches("```json")
        .trim_matches('`')
        .trim();
    let Ok(reply) = serde_json::from_str::<CheckReply>(t) else {
        return unclear.into();
    };
    let Some(step) = reply.first_wrong.filter(|s| !s.trim().is_empty()) else {
        return i18n::line(lang, i18n::Line::NoWrongStep).into();
    };
    let lines: Vec<&str> = work.lines().collect();
    let Some((line, own)) = sources::locate(&lines, &step) else {
        return unclear.into();
    };
    let at = match file {
        Some(f) => format!("`{f}:{line}`"),
        None => i18n::line(lang, i18n::Line::StepAt).replace("{n}", &line.to_string()),
    };
    format!(
        "{}\n\n> {}",
        i18n::line(lang, i18n::Line::FirstWrongStep).replace("{at}", &at),
        sources::cap(&own)
    )
}

/// The workspace's source files with their first lines (often the title),
/// so "the paper" in a question can be resolved.
fn workspace_files(cwd: &std::path::Path) -> String {
    sources::source_files(cwd)
        .into_iter()
        .take(30)
        .map(|(full, rel)| {
            let first = sources::pages(&full, &rel)
                .ok()
                .and_then(|p| {
                    p.iter()
                        .flat_map(|pg| pg.lines())
                        .map(str::trim)
                        .filter(|l| l.split_whitespace().count() >= 3)
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" / ")
                        .chars()
                        .take(240)
                        .collect::<String>()
                        .into()
                })
                .unwrap_or_default();
            format!("- {rel}: {first}\n")
        })
        .collect()
}

/// Without the model: the first (best) candidate line of each page.
fn best_per_page(cands: Vec<sources::WebResult>) -> Vec<sources::WebResult> {
    let mut seen: Vec<String> = Vec::new();
    cands
        .into_iter()
        .filter(|c| {
            let new = !seen.contains(&c.url);
            seen.push(c.url.clone());
            new
        })
        .collect()
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
        let out = render_check(None, work, None, r#"{"first_wrong": "x = 2 or x = 4"}"#);
        assert!(
            out.starts_with("**First step that doesn't hold** (line 3):"),
            "{out}"
        );
        assert!(out.contains("> x = 2 or x = 4"));
        // A reason smuggled in is a broken contract.
        let out = render_check(
            None,
            work,
            None,
            r#"{"first_wrong": "x = 2 or x = 4", "why": "x + 2 = 0 gives -2"}"#,
        );
        assert!(out.starts_with("Couldn't point"));
        // A step that isn't in the work is not shown.
        let out = render_check(None, work, None, r#"{"first_wrong": "x = -2"}"#);
        assert!(out.starts_with("Couldn't point"));
        assert_eq!(
            render_check(None, work, Some("w.md"), r#"{"first_wrong": null}"#),
            "**No step found that doesn't hold.**"
        );
        assert!(
            render_check(
                None,
                work,
                Some("w.md"),
                r#"{"first_wrong":"(x - 2)(x + 2) = 0"}"#
            )
            .contains("(`w.md:2`)")
        );
    }

    #[test]
    fn transcriptions_keep_their_shape() {
        assert_eq!(
            parse_transcription(r#"{"lines": ["x² - 4 = 0", " ", "x = 2"]}"#),
            Some(vec!["x² - 4 = 0".to_string(), "x = 2".to_string()])
        );
        assert_eq!(parse_transcription(r#"{"lines": []}"#), None);
        assert_eq!(
            parse_transcription(r#"{"lines": ["a"], "note": "fixed it"}"#),
            None
        );
        assert!(is_photo(std::path::Path::new("work.JPG")));
        assert!(!is_photo(std::path::Path::new("work.md")));
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
