use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate,
    CancelNotification, ContentBlock, ContentChunk, EmbeddedResourceResource, Implementation,
    InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    PromptCapabilities, PromptRequest, PromptResponse, SessionId, SessionNotification,
    SessionUpdate, StopReason, UnstructuredCommandInput,
};
use agent_client_protocol::{Agent, Client, Error as AcpError, Stdio};
use anyhow::Result;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::domain::{ItemStatus, Session};
use crate::runtime::{Runtime, TurnInput};

pub async fn run_stdio(runtime: Arc<Runtime>) -> Result<()> {
    let rt_new = runtime.clone();
    let rt_load = runtime.clone();
    let rt_prompt = runtime.clone();

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
                    Ok(_) => {
                        let sid = SessionId::new(id);
                        responder.respond(NewSessionResponse::new(sid.clone()))?;
                        let _ = send_commands(&cx, sid);
                        Ok(())
                    }
                    Err(err) => {
                        error!("create_session failed: {err:#}");
                        responder.respond_with_error(AcpError::internal_error().data(err.to_string()))
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
                        return responder.respond_with_error(
                            AcpError::internal_error().data(err.to_string()),
                        );
                    }
                };
                responder.respond(LoadSessionResponse::new())?;
                let _ = send_commands(&cx, req.session_id.clone());
                if !loaded.plan.items.is_empty() {
                    let _ = send_plan(&cx, req.session_id, &loaded);
                }
                Ok(())
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
                    match rt.handle_turn(&id_str, input).await {
                        Ok(out) => {
                            if out.plan_changed {
                                if let Ok(Some(session)) = rt.load_session(&id_str) {
                                    let _ = send_plan(&cx_task, sid.clone(), &session);
                                }
                            }
                            let _ = send_text(&cx_task, sid.clone(), out.reply);
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

fn send_text(cx: &ConnectionTo<Client>, session_id: SessionId, text: String) -> Result<(), AcpError> {
    cx.send_notification(SessionNotification::new(
        session_id,
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text))),
    ))
}

fn send_commands(cx: &ConnectionTo<Client>, session_id: SessionId) -> Result<(), AcpError> {
    let cmds = vec![
        AvailableCommand::new("plan", "Build the path now from what we discussed"),
        AvailableCommand::new("check", "Verify you understand the current part").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                "Explain what you did or understood (optional if in files)",
            )),
        ),
        AvailableCommand::new("next", "Move to the next ready part after passing /check"),
        AvailableCommand::new("handoff", "Let the agent finish remaining labor after /check").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                "Optional instructions for the handed-off work",
            )),
        ),
        AvailableCommand::new("skip", "Skip learning this part and let the agent handle it").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                "Optional instructions, or `none` to skip without agent edits",
            )),
        ),
        AvailableCommand::new("replan", "Update the remaining path based on new findings").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                "What changed?",
            )),
        ),
        AvailableCommand::new("map", "Show the dependency map and current status"),
        AvailableCommand::new("focus", "Switch focus to another ready part by ID").input(
            AvailableCommandInput::Unstructured(UnstructuredCommandInput::new("item id")),
        ),
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
        .plan
        .items
        .iter()
        .map(|item| {
            let is_active = session.active_item.as_deref() == Some(item.id.as_str());
            let status = match item.status {
                ItemStatus::Pending => PlanEntryStatus::Pending,
                ItemStatus::Working => PlanEntryStatus::InProgress,
                ItemStatus::Understood | ItemStatus::HandedOff | ItemStatus::Skipped => {
                    PlanEntryStatus::Completed
                }
            };
            let priority = if is_active {
                PlanEntryPriority::High
            } else {
                PlanEntryPriority::Medium
            };
            let tag = match item.status {
                ItemStatus::HandedOff => " [handed off]",
                ItemStatus::Skipped => " [skipped]",
                ItemStatus::Understood => " [understood]",
                ItemStatus::Working | ItemStatus::Pending => "",
            };
            let content = format!("[{}] {}{}", item.id, item.title, tag);
            PlanEntry::new(content, priority, status)
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
