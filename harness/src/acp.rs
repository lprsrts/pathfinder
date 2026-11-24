use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, AuthMethod, AuthMethodAgent, AuthMethodTerminal, AuthenticateRequest,
    AuthenticateResponse, AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate,
    CancelNotification, Content, ContentBlock, ContentChunk, Diff, EmbeddedResourceResource,
    Implementation, InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    PromptCapabilities, PromptRequest, PromptResponse, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelectGroup,
    SessionConfigSelectOption, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason, ToolCall,
    ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind as AcpToolKind, UnstructuredCommandInput,
};
use agent_client_protocol::{Agent, Client, Error as AcpError, Stdio};
use anyhow::Result;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::auth::Credentials;
use crate::catalog::{self, ModelCache};
use crate::config::{Config, Route};
use crate::domain::{PartStatus, Session};
use crate::engine::{StreamEvent, StreamSink};
use crate::runtime::{EFFORTS, ModelChoices, OptionValue, Runtime, TurnInput};
use crate::stream::{ToolContent, ToolKind};

pub async fn run_stdio(runtime: Arc<Runtime>, creds: Arc<Credentials>) -> Result<()> {
    let rt_new = runtime.clone();
    let creds_new = creds.clone();
    let creds_auth = creds.clone();
    let rt_load = runtime.clone();
    let rt_prompt = runtime.clone();
    let rt_config = runtime.clone();
    // Whether Zed can show on/off switches; set at initialize.
    let switches = Arc::new(AtomicBool::new(false));
    let (sw_init, sw_new, sw_load, sw_config) = (
        switches.clone(),
        switches.clone(),
        switches.clone(),
        switches.clone(),
    );

    Agent
        .v1()
        .name("harness")
        .on_receive_request(
            async move |req: InitializeRequest, responder, _cx| {
                let booleans = req
                    .client_capabilities
                    .session
                    .as_ref()
                    .and_then(|s| s.config_options.as_ref())
                    .is_some_and(|c| c.boolean.is_some());
                sw_init.store(booleans, Ordering::Relaxed);
                let capabilities = AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(PromptCapabilities::new().embedded_context(true));
                let resp = InitializeResponse::new(req.protocol_version)
                    .agent_capabilities(capabilities)
                    .auth_methods(auth_methods(req.client_capabilities.auth.terminal))
                    .agent_info(Implementation::new("harness", env!("CARGO_PKG_VERSION")));
                responder.respond(resp)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: AuthenticateRequest, responder, _cx| {
                if req.method_id.0.as_ref() == "chatgpt" {
                    // stdout carries the protocol, so the URL goes to the log.
                    let session = creds_auth.chatgpt();
                    let login = session.login(|url| {
                        info!("ChatGPT sign-in: {url}");
                        crate::cli::open_browser(url);
                    });
                    if let Err(e) = login.await {
                        warn!("ChatGPT sign-in failed: {e:#}");
                        return responder.respond_with_error(AcpError::auth_required());
                    }
                    catalog::refresh(&creds_auth, &reqwest::Client::new(), &[Route::Chatgpt]).await;
                }
                if ensure_models(&creds_auth).await {
                    responder.respond(AuthenticateResponse::new())
                } else {
                    responder.respond_with_error(AcpError::auth_required())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, cx| {
                if !ensure_models(&creds_new).await {
                    return responder.respond_with_error(AcpError::auth_required());
                }
                let id = Uuid::new_v4().to_string();
                match rt_new.create_session(id.clone(), req.cwd) {
                    Ok(session) => {
                        let sid = SessionId::new(id);
                        let options = model_options(
                            &rt_new.model_choices(&session),
                            sw_new.load(Ordering::Relaxed),
                        );
                        responder.respond(
                            NewSessionResponse::new(sid.clone()).config_options(options),
                        )?;
                        let _ = send_commands(&cx, sid);
                        Ok(())
                    }
                    Err(err) => {
                        error!("create_session failed: {err:#}");
                        responder
                            .respond_with_error(AcpError::internal_error().data(err.to_string()))
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: LoadSessionRequest, responder, cx| {
                let id = req.session_id.0.to_string();
                let loaded = match rt_load.load_session(&id) {
                    Ok(Some(s)) => s,
                    Ok(None) => match rt_load.create_session(id.clone(), req.cwd) {
                        Ok(s) => s,
                        Err(err) => {
                            error!("create fallback session failed: {err:#}");
                            return responder.respond_with_error(
                                AcpError::internal_error().data(err.to_string()),
                            );
                        }
                    },
                    Err(err) => {
                        error!("load_session failed: {err:#}");
                        return responder
                            .respond_with_error(AcpError::internal_error().data(err.to_string()));
                    }
                };
                // ACP: replay the whole conversation before answering the load.
                replay_history(&cx, &req.session_id, &loaded);
                if !loaded.map.parts.is_empty() {
                    let _ = send_plan(&cx, req.session_id.clone(), &loaded);
                }
                let options = model_options(
                    &rt_load.model_choices(&loaded),
                    sw_load.load(Ordering::Relaxed),
                );
                responder.respond(LoadSessionResponse::new().config_options(options))?;
                let _ = send_commands(&cx, req.session_id);
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: SetSessionConfigOptionRequest, responder, _cx| {
                let id = req.session_id.0.to_string();
                let value = match &req.value {
                    SessionConfigOptionValue::Boolean { value } => OptionValue::Bool(*value),
                    SessionConfigOptionValue::ValueId { value } => {
                        OptionValue::Id(value.0.to_string())
                    }
                    _ => {
                        return responder.respond_with_error(
                            AcpError::invalid_params().data("unsupported value"),
                        );
                    }
                };
                match rt_config.set_option(&id, req.config_id.0.as_ref(), value) {
                    Ok(session) => {
                        let options = model_options(
                            &rt_config.model_choices(&session),
                            sw_config.load(Ordering::Relaxed),
                        );
                        responder.respond(SetSessionConfigOptionResponse::new(options))
                    }
                    Err(err) => responder
                        .respond_with_error(AcpError::invalid_params().data(err.to_string())),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: PromptRequest, responder, cx| {
                let rt = rt_prompt.clone();
                let sid = req.session_id.clone();
                let input = extract_turn_input(&req.prompt);

                let cx_task = cx.clone();
                cx.spawn(async move {
                    let id_str = sid.0.to_string();
                    let sink = make_stream_sink(cx_task.clone(), sid.clone());
                    match rt.handle_turn(&id_str, input, sink).await {
                        Ok(out) => {
                            if out.plan_changed {
                                if let Ok(Some(session)) = rt.load_session(&id_str) {
                                    let _ = send_plan(&cx_task, sid.clone(), &session);
                                }
                            }
                            if !out.reply.is_empty() {
                                let _ = send_text(&cx_task, sid.clone(), out.reply);
                            }
                            let _ = responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        Err(err) => {
                            error!("handle_turn failed: {err:#}");
                            let _ = send_text(
                                &cx_task,
                                sid.clone(),
                                format!("Something went wrong running the model: {err}"),
                            );
                            let _ = responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notif: CancelNotification, _cx| {
                warn!("session cancelled: {}", notif.session_id.0);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await?;

    info!("acp stdio connection closed");
    Ok(())
}

/// The two model pickers, with entries grouped by CLI.
/// Sign-in options shown when no model is usable. Terminal methods make Zed
/// re-run this binary with the given arguments in a terminal.
fn auth_methods(terminal: bool) -> Vec<AuthMethod> {
    let chatgpt = "Use your ChatGPT Plus or Pro plan. Opens your browser.";
    let api_key = "Store an Anthropic or OpenAI API key for Pathfinder. Claude Code and Codex models use their own CLI logins instead (see the README).";
    if terminal {
        vec![
            AuthMethod::Terminal(
                AuthMethodTerminal::new("chatgpt", "Sign in with ChatGPT")
                    .args(vec!["auth".into(), "login".into(), "chatgpt".into()])
                    .description(chatgpt.to_string()),
            ),
            AuthMethod::Terminal(
                AuthMethodTerminal::new("api-key", "Add an API key")
                    .args(vec!["auth".into(), "set".into()])
                    .description(api_key.to_string()),
            ),
        ]
    } else {
        vec![
            // Without terminal support the agent runs the browser sign-in itself.
            AuthMethod::Agent(
                AuthMethodAgent::new("chatgpt", "Sign in with ChatGPT")
                    .description(chatgpt.to_string()),
            ),
            AuthMethod::Agent(
                AuthMethodAgent::new("api-key", "Add an API key").description(
                    "Run `harness auth set` in a terminal, then try again.".to_string(),
                ),
            ),
        ]
    }
}

/// True when at least one model is usable. Refreshes the discovered model
/// cache first when it's empty, otherwise in the background when stale.
async fn ensure_models(creds: &Arc<Credentials>) -> bool {
    let cache = ModelCache::load();
    if Config::load().catalog().is_empty() {
        let refreshed = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            catalog::refresh(creds, &reqwest::Client::new(), &Route::DISCOVERED),
        )
        .await;
        if refreshed.is_err() {
            warn!("model discovery timed out");
        }
        return !Config::load().catalog().is_empty();
    }
    if cache.is_stale() {
        let creds = creds.clone();
        tokio::spawn(async move {
            catalog::refresh(&creds, &reqwest::Client::new(), &Route::DISCOVERED).await;
        });
    }
    true
}

/// Sends a loaded session's turns back to the client, in order.
fn replay_history(cx: &ConnectionTo<Client>, session_id: &SessionId, session: &Session) {
    let mut previous_role: Option<&str> = None;
    for turn in &session.history {
        // Consecutive chunks merge into one message; keep turns apart.
        let text = if previous_role == Some(turn.role.as_str()) {
            format!("\n\n{}", turn.content)
        } else {
            turn.content.clone()
        };
        previous_role = Some(turn.role.as_str());
        let chunk = ContentChunk::new(ContentBlock::from(text));
        let update = if turn.role == "user" {
            SessionUpdate::UserMessageChunk(chunk)
        } else {
            SessionUpdate::AgentMessageChunk(chunk)
        };
        let _ = cx.send_notification(SessionNotification::new(session_id.clone(), update));
    }
}

/// The thread's toolbar: main model, effort, fast model, and the Decisions
/// switch with its service picker. Zed shows only the selected option's name
/// on each button, so names stay short.
fn model_options(choices: &ModelChoices, switches: bool) -> Vec<SessionConfigOption> {
    if choices.catalog.is_empty() {
        return Vec::new();
    }
    let grouped = || -> Vec<SessionConfigSelectGroup> {
        let mut groups: Vec<(&str, Vec<SessionConfigSelectOption>)> = Vec::new();
        for entry in &choices.catalog {
            let group = entry.route.label();
            let option = SessionConfigSelectOption::new(
                entry.id.clone(),
                model_name(entry, &choices.catalog),
            );
            match groups.iter_mut().find(|(g, _)| *g == group) {
                Some((_, options)) => options.push(option),
                None => groups.push((group, vec![option])),
            }
        }
        groups
            .into_iter()
            .map(|(g, options)| SessionConfigSelectGroup::new(g, g, options))
            .collect()
    };
    let effort_name = |level: &str| match level {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        _ => "Default",
    };
    let mut options = vec![
        // Zed draws these right to left: Fast model, Main model, then its Effort.
        SessionConfigOption::select(
            "effort",
            "Effort",
            choices.effort.clone(),
            EFFORTS
                .iter()
                .map(|level| SessionConfigSelectOption::new(*level, effort_name(level)))
                .collect::<Vec<_>>(),
        )
        .description("Main model effort".to_string())
        .category(SessionConfigOptionCategory::ThoughtLevel),
        SessionConfigOption::select(
            "system2",
            "Main model",
            choices.system2.clone().unwrap_or_default(),
            grouped(),
        )
        .description("Map, candidates, judging".to_string())
        .category(SessionConfigOptionCategory::Model),
        SessionConfigOption::select(
            "system1",
            "Fast model",
            choices.system1.clone().unwrap_or_default(),
            grouped(),
        )
        .description("Onboarding, replies".to_string())
        .category(SessionConfigOptionCategory::ModelConfig),
    ];
    let Some(current) = choices.decision_model else {
        return options;
    };
    options.push(
        if switches {
            SessionConfigOption::boolean("decisions", "Decisions", choices.decisions_on)
        } else {
            SessionConfigOption::select(
                "decisions",
                "Decisions",
                if choices.decisions_on { "on" } else { "off" },
                vec![
                    SessionConfigSelectOption::new("on", "Decisions on"),
                    SessionConfigSelectOption::new("off", "Decisions off"),
                ],
            )
        }
        .description("Classify each message before the model replies".to_string())
        .category(SessionConfigOptionCategory::ModelConfig),
    );
    if choices.decisions_on && choices.decision_available.len() > 1 {
        options.push(
            SessionConfigOption::select(
                "decision_model",
                "Decision model",
                current.id(),
                choices
                    .decision_available
                    .iter()
                    .map(|p| {
                        SessionConfigSelectOption::new(p.id(), p.short())
                            .description(p.model().to_string())
                    })
                    .collect::<Vec<_>>(),
            )
            .category(SessionConfigOptionCategory::ModelConfig),
        );
    }
    options
}

/// A model's button name: its label without "Claude ", plus the route only
/// when another route offers a model of the same name.
fn model_name(entry: &crate::config::ModelEntry, catalog: &[crate::config::ModelEntry]) -> String {
    let short = |label: &str| label.strip_prefix("Claude ").unwrap_or(label).to_string();
    let name = short(&entry.label);
    let shared = catalog
        .iter()
        .any(|o| o.route != entry.route && short(&o.label).eq_ignore_ascii_case(&name));
    if !shared {
        return name;
    }
    let route = match entry.route {
        Route::Anthropic | Route::Openai => "API",
        other => other.label(),
    };
    format!("{name} · {route}")
}

fn make_stream_sink(cx: ConnectionTo<Client>, session_id: SessionId) -> StreamSink {
    Arc::new(move |ev: StreamEvent| {
        let update = match ev {
            StreamEvent::Thought(text) => {
                SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from(text)))
            }
            StreamEvent::TextDelta(text) => {
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text)))
            }
            StreamEvent::ToolStart(tool) => {
                let kind = match tool.kind {
                    ToolKind::Read => AcpToolKind::Read,
                    ToolKind::Edit => AcpToolKind::Edit,
                    ToolKind::Search => AcpToolKind::Search,
                    ToolKind::Execute => AcpToolKind::Execute,
                    ToolKind::Fetch => AcpToolKind::Fetch,
                    ToolKind::Other => AcpToolKind::Other,
                };
                let locations = tool.path.map(ToolCallLocation::new).into_iter().collect();
                SessionUpdate::ToolCall(
                    ToolCall::new(tool.id, tool.title)
                        .kind(kind)
                        .status(ToolCallStatus::InProgress)
                        .locations(locations),
                )
            }
            StreamEvent::ToolUpdate { id, content } => {
                let content = match content {
                    ToolContent::Diff { path, old, new } => {
                        ToolCallContent::Diff(Diff::new(path, new).old_text(old))
                    }
                    ToolContent::Text(text) => {
                        ToolCallContent::Content(Content::new(ContentBlock::from(text)))
                    }
                };
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    id,
                    ToolCallUpdateFields::new().content(vec![content]),
                ))
            }
            StreamEvent::ToolEnd { id, ok } => SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                id,
                ToolCallUpdateFields::new().status(if ok {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                }),
            )),
        };
        let _ = cx.send_notification(SessionNotification::new(session_id.clone(), update));
    })
}

fn send_text(
    cx: &ConnectionTo<Client>,
    session_id: SessionId,
    text: String,
) -> Result<(), AcpError> {
    cx.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text))),
    ))
}

fn send_commands(cx: &ConnectionTo<Client>, session_id: SessionId) -> Result<(), AcpError> {
    let cmds = vec![
        AvailableCommand::new("plan", "Build the map now from what we discussed"),
        AvailableCommand::new(
            "summary",
            "Explain the current part; you then judge a candidate",
        )
        .input(AvailableCommandInput::Unstructured(
            UnstructuredCommandInput::new("Your explanation of the part"),
        )),
        AvailableCommand::new(
            "handoff",
            "Give an understood part to the agent; you review the result",
        )
        .input(AvailableCommandInput::Unstructured(
            UnstructuredCommandInput::new("Optional instructions"),
        )),
        AvailableCommand::new("approve", "Approve the agent's result for this part"),
        AvailableCommand::new("object", "Send the agent's result back with your reason").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                "What's wrong and why",
            )),
        ),
        AvailableCommand::new("known", "Skip this part as already known"),
        AvailableCommand::new("delegate", "Hand this part off without understanding it").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                "Optional instructions",
            )),
        ),
        AvailableCommand::new("accept", "Accept the proposed map change"),
        AvailableCommand::new("reject", "Reject the proposed map change"),
        AvailableCommand::new("replan", "Ask for a map change").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new("What changed?")),
        ),
        AvailableCommand::new("next", "Move to the next unresolved part"),
        AvailableCommand::new("focus", "Switch to a part by id, number or id prefix").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                "id, number (e.g. 3) or start of the id",
            )),
        ),
        AvailableCommand::new("map", "Show the map"),
    ];
    cx.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(cmds)),
    ))
}

fn send_plan(
    cx: &ConnectionTo<Client>,
    session_id: SessionId,
    session: &Session,
) -> Result<(), AcpError> {
    let entries: Vec<PlanEntry> = session
        .map
        .parts
        .iter()
        .map(|part| {
            let is_active = session.active.as_deref() == Some(part.id.as_str());
            let status = match part.status {
                PartStatus::Open if is_active => PlanEntryStatus::InProgress,
                PartStatus::Open => PlanEntryStatus::Pending,
                PartStatus::InReview => PlanEntryStatus::InProgress,
                PartStatus::Understood
                | PartStatus::Done
                | PartStatus::Known
                | PartStatus::Delegated => PlanEntryStatus::Completed,
            };
            let priority = if is_active {
                PlanEntryPriority::High
            } else {
                PlanEntryPriority::Medium
            };
            let tag = match part.status {
                PartStatus::Open => String::new(),
                other => format!(" [{}]", other.label()),
            };
            PlanEntry::new(
                format!("[{}] {}{tag}", part.id, part.title),
                priority,
                status,
            )
        })
        .collect();

    cx.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::Plan(Plan::new(entries)),
    ))
}

fn extract_turn_input(blocks: &[ContentBlock]) -> TurnInput {
    let mut text_parts = Vec::new();
    let mut files = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text(t) => {
                if !t.text.trim().is_empty() {
                    text_parts.push(t.text.clone());
                }
            }
            ContentBlock::ResourceLink(link) => {
                if let Some(path) = file_uri_to_path(&link.uri) {
                    files.push(path);
                } else {
                    text_parts.push(format!("[Resource: {} ({})]", link.name, link.uri));
                }
            }
            ContentBlock::Resource(embedded) => match &embedded.resource {
                EmbeddedResourceResource::TextResourceContents(tr) => {
                    if let Some(path) = file_uri_to_path(&tr.uri) {
                        files.push(path);
                    }
                    text_parts.push(format!("File `{}`:\n{}", tr.uri, tr.text));
                }
                EmbeddedResourceResource::BlobResourceContents(br) => {
                    if let Some(path) = file_uri_to_path(&br.uri) {
                        files.push(path);
                    }
                }
                _ => {}
            },
            ContentBlock::Image(img) => {
                if let Some(uri) = img.uri.as_deref().and_then(file_uri_to_path) {
                    files.push(uri);
                }
            }
            _ => {}
        }
    }

    TurnInput {
        text: text_parts.join("\n\n"),
        files,
    }
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    Some(PathBuf::from(decoded))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelEntry, Route};
    use crate::decide::Provider;

    #[test]
    fn picker_names_are_short_and_name_the_route_only_when_needed() {
        let entry = |id: &str, route| ModelEntry {
            id: id.into(),
            label: "GPT-6 Luna".into(),
            route,
            model: String::new(),
            effort: None,
        };
        let choices = ModelChoices {
            catalog: vec![
                entry("anthropic-luna", Route::Anthropic),
                entry("codex-luna", Route::Codex),
            ],
            system1: Some("anthropic-luna".into()),
            system2: Some("codex-luna".into()),
            effort: "default".into(),
            decision_available: vec![],
            decisions_on: false,
            decision_model: None,
        };
        let json = serde_json::to_value(model_options(&choices, true)).unwrap();
        let names: Vec<&str> = json[2]["options"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["options"].as_array().unwrap())
            .map(|o| o["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["GPT-6 Luna · API", "GPT-6 Luna · Codex"]);
        assert_eq!(
            json[2]["options"][1]["options"][0]["name"],
            "GPT-6 Luna · Codex"
        );
        let solo = ModelEntry {
            id: "h".into(),
            label: "Claude Haiku 5.5".into(),
            route: Route::Anthropic,
            model: String::new(),
            effort: None,
        };
        assert_eq!(model_name(&solo, &[solo.clone()]), "Haiku 5.5");
        // No decision service with a key: no Decisions controls.
        assert_eq!(json.as_array().unwrap().len(), 3);
    }

    #[test]
    fn decisions_show_as_a_switch_and_a_service_picker() {
        let entry = ModelEntry {
            id: "m".into(),
            label: "M".into(),
            route: Route::Anthropic,
            model: String::new(),
            effort: None,
        };
        let choices = |on| ModelChoices {
            catalog: vec![entry.clone()],
            system1: Some("m".into()),
            system2: Some("m".into()),
            effort: "high".into(),
            decision_available: vec![Provider::Typesafe, Provider::Openai],
            decisions_on: on,
            decision_model: Some(Provider::Typesafe),
        };
        let json = serde_json::to_value(model_options(&choices(true), true)).unwrap();
        let ids: Vec<&str> = json
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "effort",
                "system2",
                "system1",
                "decisions",
                "decision_model"
            ]
        );
        assert_eq!(json[0]["category"], "thought_level");
        assert_eq!(json[0]["currentValue"], "high");
        assert_eq!(json[3]["type"], "boolean");
        assert_eq!(json[3]["currentValue"], true);
        assert_eq!(json[4]["currentValue"], "typesafe");

        // Off: no service picker. Without switch support: an Off/On dropdown.
        let json = serde_json::to_value(model_options(&choices(false), false)).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 4);
        assert_eq!(json[3]["type"], "select");
        assert_eq!(json[3]["currentValue"], "off");
    }
}
