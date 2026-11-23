use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate,
    CancelNotification, ContentBlock, ContentChunk, EmbeddedResourceResource, Implementation,
    InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    PromptCapabilities, PromptRequest, PromptResponse, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelectGroup,
    SessionConfigSelectOption, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
    UnstructuredCommandInput,
};
use agent_client_protocol::{Agent, Client, Error as AcpError, Stdio};
use anyhow::Result;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::domain::{PartStatus, Session};
use crate::engine::{StreamEvent, StreamSink};
use crate::runtime::{ModelChoices, Runtime, TurnInput};

pub async fn run_stdio(runtime: Arc<Runtime>) -> Result<()> {
    let rt_new = runtime.clone();
    let rt_load = runtime.clone();
    let rt_prompt = runtime.clone();
    let rt_config = runtime.clone();

    Agent
        .v1()
        .name("harness")
        .on_receive_request(
            async move |req: InitializeRequest, responder, _cx| {
                let capabilities = AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(PromptCapabilities::new().embedded_context(true));
                let resp = InitializeResponse::new(req.protocol_version)
                    .agent_capabilities(capabilities)
                    .agent_info(Implementation::new("harness", env!("CARGO_PKG_VERSION")));
                responder.respond(resp)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, cx| {
                let id = Uuid::new_v4().to_string();
                match rt_new.create_session(id.clone(), req.cwd) {
                    Ok(session) => {
                        let sid = SessionId::new(id);
                        let options = model_options(&rt_new.model_choices(&session));
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
                let options = model_options(&rt_load.model_choices(&loaded));
                responder.respond(LoadSessionResponse::new().config_options(options))?;
                let _ = send_commands(&cx, req.session_id.clone());
                if !loaded.map.parts.is_empty() {
                    let _ = send_plan(&cx, req.session_id, &loaded);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: SetSessionConfigOptionRequest, responder, _cx| {
                let id = req.session_id.0.to_string();
                let system2 = match req.config_id.0.as_ref() {
                    "system1" => false,
                    "system2" => true,
                    other => {
                        return responder.respond_with_error(
                            AcpError::invalid_params().data(format!("unknown option `{other}`")),
                        );
                    }
                };
                let SessionConfigOptionValue::ValueId { value } = &req.value else {
                    return responder.respond_with_error(
                        AcpError::invalid_params().data("expected a model id"),
                    );
                };
                match rt_config.set_model(&id, system2, &value.0) {
                    Ok(session) => {
                        let options = model_options(&rt_config.model_choices(&session));
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
/// The two model pickers. Zed's buttons show only the selected option's name
/// (the picker's name and description appear on hover), so each option names
/// its CLI: pi and Codex offer the same model names.
fn model_options(choices: &ModelChoices) -> Vec<SessionConfigOption> {
    let grouped = || -> Vec<SessionConfigSelectGroup> {
        let mut groups: Vec<(&str, Vec<SessionConfigSelectOption>)> = Vec::new();
        for entry in &choices.catalog {
            let cli = entry.backend.label();
            let option =
                SessionConfigSelectOption::new(entry.id.clone(), option_label(&entry.label, cli));
            match groups.iter_mut().find(|(g, _)| *g == cli) {
                Some((_, options)) => options.push(option),
                None => groups.push((cli, vec![option])),
            }
        }
        groups
            .into_iter()
            .map(|(g, options)| SessionConfigSelectGroup::new(g, g, options))
            .collect()
    };
    vec![
        SessionConfigOption::select("system1", "Fast model", choices.system1.clone(), grouped())
            .description("System 1: onboarding and log replies".to_string())
            .category(SessionConfigOptionCategory::ModelConfig),
        SessionConfigOption::select("system2", "Main model", choices.system2.clone(), grouped())
            .description("System 2: map, candidates, judgment and agent work".to_string())
            .category(SessionConfigOptionCategory::Model),
    ]
}

fn option_label(model: &str, cli: &str) -> String {
    format!("{model} ({cli})")
}

fn make_stream_sink(cx: ConnectionTo<Client>, session_id: SessionId) -> StreamSink {
    Arc::new(move |ev: StreamEvent| match ev {
        StreamEvent::Thought(text) => {
            let _ = send_thought(&cx, session_id.clone(), text);
        }
        StreamEvent::TextDelta(text) => {
            let _ = send_text(&cx, session_id.clone(), text);
        }
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

fn send_thought(
    cx: &ConnectionTo<Client>,
    session_id: SessionId,
    text: String,
) -> Result<(), AcpError> {
    cx.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from(text))),
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
        AvailableCommand::new("focus", "Switch to a part by id").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new("part id")),
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
    use crate::config::{Backend, ModelEntry};

    #[test]
    fn picker_options_name_the_cli() {
        let entry = |id: &str, backend| ModelEntry {
            id: id.into(),
            label: "GPT-6 Luna".into(),
            backend,
            model: String::new(),
            effort: None,
        };
        let choices = ModelChoices {
            catalog: vec![
                entry("pi-luna", Backend::Pi),
                entry("codex-luna", Backend::Codex),
            ],
            system1: "pi-luna".into(),
            system2: "codex-luna".into(),
        };
        let json = serde_json::to_value(model_options(&choices)).unwrap();
        let names: Vec<&str> = json[0]["options"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["options"].as_array().unwrap())
            .map(|o| o["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["GPT-6 Luna (pi)", "GPT-6 Luna (Codex)"]);
        assert_eq!(
            json[1]["options"][1]["options"][0]["name"],
            "GPT-6 Luna (Codex)"
        );
    }
}
