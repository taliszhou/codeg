use std::sync::Arc;
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::i18n::Lang;
use super::session_bridge::{PendingPermission, SessionBridge};
use super::types::{MessageLevel, RichMessage};
use crate::acp::manager::ConnectionManager;
use crate::acp::types::PromptInputBlock;
use crate::db::service::{app_metadata_service, conversation_service, sender_context_service};
use crate::web::event_bridge::WebEventBroadcaster;

use super::manager::ChatChannelManager;

const FLUSH_INTERVAL_SECS: u64 = 10;
const BUFFER_FLUSH_THRESHOLD: usize = 500;
const MAX_MESSAGE_LEN: usize = 2000;
const MESSAGE_LANGUAGE_KEY: &str = "chat_message_language";
const COMMAND_PREFIX_KEY: &str = "chat_command_prefix";
const DEFAULT_COMMAND_PREFIX: &str = "/";

pub fn spawn_session_event_subscriber(
    broadcaster: Arc<WebEventBroadcaster>,
    bridge: Arc<Mutex<SessionBridge>>,
    manager: ChatChannelManager,
    conn_mgr: ConnectionManager,
    db_conn: DatabaseConnection,
) -> JoinHandle<()> {
    let mut rx = broadcaster.subscribe();

    tokio::spawn(async move {
        let mut last_heartbeat = Instant::now();

        loop {
            tokio::select! {
                result = rx.recv() => {
                    let event = match result {
                        Ok(e) => e,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("[SessionEventSub] lagged {n} events");
                            continue;
                        }
                        Err(_) => break,
                    };

                    if event.channel == "acp://event" {
                        handle_acp_event_payload(
                            event.payload.as_ref(),
                            &bridge,
                            &manager,
                            &conn_mgr,
                            &db_conn,
                        )
                        .await;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(FLUSH_INTERVAL_SECS)) => {
                    if last_heartbeat.elapsed() >= Duration::from_secs(FLUSH_INTERVAL_SECS) {
                        flush_progress(&bridge, &manager, &db_conn).await;
                        last_heartbeat = Instant::now();
                    }
                }
            }
        }
    })
}

async fn get_lang(db: &DatabaseConnection) -> Lang {
    app_metadata_service::get_value(db, MESSAGE_LANGUAGE_KEY)
        .await
        .ok()
        .flatten()
        .map(|v| Lang::from_str_lossy(&v))
        .unwrap_or_default()
}

async fn get_prefix(db: &DatabaseConnection) -> String {
    app_metadata_service::get_value(db, COMMAND_PREFIX_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_COMMAND_PREFIX.to_string())
}

async fn handle_acp_event_payload(
    payload: &serde_json::Value,
    bridge: &Arc<Mutex<SessionBridge>>,
    manager: &ChatChannelManager,
    conn_mgr: &ConnectionManager,
    db: &DatabaseConnection,
) {
    let event_type = match payload.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return,
    };
    let connection_id = match payload.get("connection_id").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return,
    };

    match event_type {
        "session_started" => {
            let session_id = payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let mut guard = bridge.lock().await;
            if let Some(session) = guard.get_mut(connection_id) {
                let _ = conversation_service::update_external_id(
                    db,
                    session.conversation_id,
                    session_id.to_string(),
                )
                .await;

                if let Some(prompt_text) = session.pending_prompt.take() {
                    let blocks = vec![PromptInputBlock::Text { text: prompt_text }];
                    let source = format!("chat:{}:{}", session.channel_id, session.sender_id);
                    if let Err(e) = conn_mgr.send_prompt(connection_id, blocks, &source).await {
                        eprintln!("[SessionEventSub] failed to send pending prompt: {e}");
                        let channel_id = session.channel_id;
                        let msg = RichMessage::error(format!("Failed to send task: {e}"));
                        let _ = manager.send_to_channel(channel_id, &msg).await;
                    }
                }
            }
        }

        "user_prompt_sent" => {
            // Echo a user-authored prompt to the bound chat channel iff the
            // bridge has a session for this connection AND the prompt did
            // not originate from that same (channel_id, sender_id) — the
            // user already saw their own message in their own chat
            // (Telegram / WeChat / Lark / etc.). The `chat:` prefix is
            // channel-agnostic; only the (cid, sid) pair matters for dedup.
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let source = payload
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() {
                return;
            }

            let target: Option<i32> = {
                let guard = bridge.lock().await;
                guard.get(connection_id).and_then(|session| {
                    let self_source =
                        format!("chat:{}:{}", session.channel_id, session.sender_id);
                    if source == self_source {
                        None
                    } else {
                        Some(session.channel_id)
                    }
                })
            };

            if let Some(channel_id) = target {
                // Long user prompts get split into Telegram-sized chunks
                // rather than hard-truncated; the originating side already
                // has the full text but the receiving channel should see
                // it in full too. Only the first chunk carries the "User"
                // title; the rest are bare continuations.
                let chunks = split_into_chunks(text, MAX_MESSAGE_LEN);
                for (idx, chunk) in chunks.into_iter().enumerate() {
                    let mut msg = RichMessage::info(chunk);
                    if idx == 0 {
                        msg = msg.with_title("User");
                    }
                    let _ = manager.send_to_channel(channel_id, &msg).await;
                }
            }
        }

        "content_delta" => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");

            // Drain at most `MAX_MESSAGE_LEN` chars from the front of the
            // buffer when (a) accumulated bytes ≥ BUFFER_FLUSH_THRESHOLD and
            // (b) at least 2s have passed since the last flush. The
            // per-flush size cap keeps every emitted message inside
            // Telegram's 4096-char/message ceiling without ever needing to
            // chunk a single drain. Any residual stays in the buffer and
            // gets drained by subsequent content_delta flushes (still 2s
            // apart), the 10s heartbeat, or finally `turn_complete`.
            let chunk: Option<(i32, String)> = {
                let mut guard = bridge.lock().await;
                match guard.get_mut(connection_id) {
                    Some(session) => {
                        session.content_buffer.push_str(text);
                        if session.content_buffer.len() >= BUFFER_FLUSH_THRESHOLD
                            && session.last_flushed.elapsed() >= Duration::from_secs(2)
                        {
                            session.last_flushed = Instant::now();
                            let drained =
                                take_chunk_front(&mut session.content_buffer, MAX_MESSAGE_LEN);
                            Some((session.channel_id, drained))
                        } else {
                            None
                        }
                    }
                    None => None,
                }
            };

            if let Some((channel_id, body)) = chunk {
                let msg = RichMessage::info(body);
                let _ = manager.send_to_channel(channel_id, &msg).await;
            }
        }

        "tool_call" => {
            let title = payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("tool");
            let tool_call_id = payload
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let raw_input = payload.get("raw_input").and_then(|v| v.as_str());

            let mut guard = bridge.lock().await;
            if let Some(session) = guard.get_mut(connection_id) {
                // Store title for progress indicator; store raw_input for later
                session.tool_calls.push(title.to_string());
                if let Some(input) = raw_input {
                    session
                        .tool_call_inputs
                        .insert(tool_call_id.to_string(), input.to_string());
                }
            }
        }

        "tool_call_update" => {
            let title = payload.get("title").and_then(|v| v.as_str());
            let status = payload.get("status").and_then(|v| v.as_str());
            let tool_call_id = payload
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let raw_input = payload.get("raw_input").and_then(|v| v.as_str());

            let mut guard = bridge.lock().await;
            if let Some(session) = guard.get_mut(connection_id) {
                // Accumulate raw_input if newly available
                if let Some(input) = raw_input {
                    session
                        .tool_call_inputs
                        .insert(tool_call_id.to_string(), input.to_string());
                }

                if status == Some("completed") {
                    let stored_input = session.tool_call_inputs.remove(tool_call_id);
                    let effective_title = title.unwrap_or("tool");
                    let input_ref = stored_input.as_deref().or(raw_input);
                    let detail = format_tool_call_detail(effective_title, input_ref);
                    let channel_id = session.channel_id;
                    drop(guard);

                    let msg = RichMessage::info(format!(">> {detail}"));
                    let _ = manager.send_to_channel(channel_id, &msg).await;
                }
            }
        }

        "permission_request" => {
            let request_id = payload
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let tool_call = payload
                .get("tool_call")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let options: Vec<crate::acp::types::PermissionOptionInfo> = payload
                .get("options")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            let mut guard = bridge.lock().await;
            if let Some(session) = guard.get_mut(connection_id) {
                let channel_id = session.channel_id;
                let sender_id = session.sender_id.clone();

                let auto_approve =
                    sender_context_service::get_or_create(db, channel_id, &sender_id)
                        .await
                        .map(|ctx| ctx.auto_approve)
                        .unwrap_or(false);

                if auto_approve {
                    let option_id = options
                        .iter()
                        .find(|o| o.kind == "allow" || o.kind == "allowForSession")
                        .or_else(|| options.first())
                        .map(|o| o.option_id.clone());

                    drop(guard);

                    if let Some(oid) = option_id {
                        let _ = conn_mgr
                            .respond_permission(connection_id, request_id, &oid)
                            .await;
                    }
                    return;
                }

                let tool_title = tool_call
                    .get("title")
                    .and_then(|v| v.as_str())
                    .or_else(|| tool_call.get("tool_name").and_then(|v| v.as_str()))
                    .unwrap_or("Unknown tool");

                // Extract detail from rawInput / raw_input in the tool_call object
                let raw_input_str = tool_call
                    .get("rawInput")
                    .or_else(|| tool_call.get("raw_input"))
                    .and_then(|v| match v {
                        serde_json::Value::String(s) => Some(s.clone()),
                        serde_json::Value::Null => None,
                        other => Some(other.to_string()),
                    });
                let tool_desc = format_tool_call_detail(tool_title, raw_input_str.as_deref());

                session.permission_pending = Some(PendingPermission {
                    request_id: request_id.to_string(),
                    tool_description: tool_desc.clone(),
                    options,
                    sent_message_id: None,
                });

                drop(guard);

                let lang = get_lang(db).await;
                let prefix = get_prefix(db).await;
                let body = match lang {
                    Lang::ZhCn | Lang::ZhTw => {
                        format!("Agent 请求权限: {tool_desc}\n\n{prefix}approve 批准 | {prefix}deny 拒绝 | {prefix}approve always 自动批准")
                    }
                    _ => {
                        format!("Agent requests permission: {tool_desc}\n\n{prefix}approve | {prefix}deny | {prefix}approve always")
                    }
                };

                let msg = RichMessage {
                    title: Some(match lang {
                        Lang::ZhCn | Lang::ZhTw => "权限请求".to_string(),
                        _ => "Permission Request".to_string(),
                    }),
                    body,
                    fields: Vec::new(),
                    level: MessageLevel::Warning,
                };
                let _ = manager.send_to_channel(channel_id, &msg).await;
            }
        }

        "turn_complete" => {
            let stop_reason = payload
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let agent_type = payload
                .get("agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");

            let mut guard = bridge.lock().await;
            if let Some(session) = guard.get_mut(connection_id) {
                let channel_id = session.channel_id;
                let conv_id = session.conversation_id;
                let residual = std::mem::take(&mut session.content_buffer);
                let tool_count = session.tool_calls.len();
                session.tool_calls.clear();
                session.last_flushed = Instant::now();
                drop(guard);

                // Emit any sub-threshold tail content first, split into one
                // or more Telegram-sized chunks. Streaming has already shipped
                // anything that crossed the per-flush threshold, so this is
                // strictly the unsent remainder.
                for chunk in split_into_chunks(&residual, MAX_MESSAGE_LEN) {
                    let msg = RichMessage::info(chunk);
                    let _ = manager.send_to_channel(channel_id, &msg).await;
                }

                // Then a compact completion footer carrying the metadata
                // fields. Kept short so it never collides with the 4096-char
                // cap regardless of how the agent finished.
                let lang = get_lang(db).await;
                let footer_body = match lang {
                    Lang::ZhCn | Lang::ZhTw => format!("✓ 完成 ({tool_count} 次工具调用)"),
                    _ => format!("✓ Done ({tool_count} tool calls)"),
                };
                let footer = RichMessage::info(footer_body)
                    .with_title(match lang {
                        Lang::ZhCn | Lang::ZhTw => "任务完成",
                        _ => "Turn Complete",
                    })
                    .with_field("Agent", agent_type)
                    .with_field(
                        match lang {
                            Lang::ZhCn | Lang::ZhTw => "结束原因",
                            _ => "Stop Reason",
                        },
                        localize_stop_reason(stop_reason, lang),
                    );
                let _ = manager.send_to_channel(channel_id, &footer).await;

                if stop_reason == "end_turn" {
                    let _ = conversation_service::update_status(
                        db,
                        conv_id,
                        crate::db::entities::conversation::ConversationStatus::Completed,
                    )
                    .await;
                }
            }
        }

        "error" => {
            let message = payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            let agent_type = payload
                .get("agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");

            let mut guard = bridge.lock().await;
            if let Some(session) = guard.remove(connection_id) {
                let channel_id = session.channel_id;
                let sender_id = session.sender_id.clone();
                let conv_id = session.conversation_id;
                drop(guard);

                let lang = get_lang(db).await;
                let msg = RichMessage {
                    title: Some(match lang {
                        Lang::ZhCn | Lang::ZhTw => "Agent 错误".to_string(),
                        _ => "Agent Error".to_string(),
                    }),
                    body: format!("[{agent_type}] {message}"),
                    fields: Vec::new(),
                    level: MessageLevel::Error,
                };
                let _ = manager.send_to_channel(channel_id, &msg).await;

                let _ = conversation_service::update_status(
                    db,
                    conv_id,
                    crate::db::entities::conversation::ConversationStatus::Cancelled,
                )
                .await;
                let _ = sender_context_service::clear_session(db, channel_id, &sender_id).await;
            }
        }

        "status_changed" => {
            let status = payload.get("status").and_then(|v| v.as_str()).unwrap_or("");

            if status == "disconnected" || status == "error" {
                let mut guard = bridge.lock().await;
                if let Some(session) = guard.remove(connection_id) {
                    let channel_id = session.channel_id;
                    let sender_id = session.sender_id.clone();
                    drop(guard);

                    let _ = sender_context_service::clear_session(db, channel_id, &sender_id).await;
                }
            }
        }

        _ => {}
    }
}

async fn flush_progress(
    bridge: &Arc<Mutex<SessionBridge>>,
    manager: &ChatChannelManager,
    _db: &DatabaseConnection,
) {
    // Heartbeat: drain at most `MAX_MESSAGE_LEN` chars of residual content
    // for any session whose buffer has been sitting idle for >=10s. Empty
    // buffers are skipped — we don't broadcast bare "still working"
    // indicators (they spam the chat and burn Telegram's 1 msg/s/chat
    // budget for no information value). Anything beyond the per-message
    // cap stays in the buffer for the next heartbeat / streaming flush /
    // turn_complete.
    let updates: Vec<(i32, String)> = {
        let mut guard = bridge.lock().await;
        let mut out = Vec::new();
        for session in guard.all_sessions_mut() {
            if !session.content_buffer.is_empty()
                && session.last_flushed.elapsed() >= Duration::from_secs(FLUSH_INTERVAL_SECS)
            {
                session.last_flushed = Instant::now();
                let drained = take_chunk_front(&mut session.content_buffer, MAX_MESSAGE_LEN);
                out.push((session.channel_id, drained));
            }
        }
        out
    };

    for (channel_id, body) in updates {
        let msg = RichMessage::info(body);
        let _ = manager.send_to_channel(channel_id, &msg).await;
    }
}

fn localize_stop_reason(reason: &str, lang: Lang) -> String {
    match lang {
        Lang::ZhCn => match reason {
            "end_turn" => "正常结束",
            "cancelled" => "已取消",
            "max_tokens" => "达到最大长度",
            "stop_sequence" => "遇到停止序列",
            "error" => "错误",
            "timeout" => "超时",
            other => other,
        },
        Lang::ZhTw => match reason {
            "end_turn" => "正常結束",
            "cancelled" => "已取消",
            "max_tokens" => "達到最大長度",
            "stop_sequence" => "遇到停止序列",
            "error" => "錯誤",
            "timeout" => "逾時",
            other => other,
        },
        Lang::Ja => match reason {
            "end_turn" => "正常終了",
            "cancelled" => "キャンセル",
            "max_tokens" => "最大トークン数到達",
            "stop_sequence" => "停止シーケンス",
            "error" => "エラー",
            "timeout" => "タイムアウト",
            other => other,
        },
        Lang::Ko => match reason {
            "end_turn" => "정상 종료",
            "cancelled" => "취소됨",
            "max_tokens" => "최대 길이 도달",
            "stop_sequence" => "정지 시퀀스",
            "error" => "오류",
            "timeout" => "시간 초과",
            other => other,
        },
        Lang::Es => match reason {
            "end_turn" => "Finalizado",
            "cancelled" => "Cancelado",
            "max_tokens" => "Longitud máxima alcanzada",
            "error" => "Error",
            "timeout" => "Tiempo agotado",
            other => other,
        },
        Lang::De => match reason {
            "end_turn" => "Abgeschlossen",
            "cancelled" => "Abgebrochen",
            "max_tokens" => "Maximale Länge erreicht",
            "error" => "Fehler",
            "timeout" => "Zeitüberschreitung",
            other => other,
        },
        Lang::Fr => match reason {
            "end_turn" => "Terminé",
            "cancelled" => "Annulé",
            "max_tokens" => "Longueur maximale atteinte",
            "error" => "Erreur",
            "timeout" => "Délai dépassé",
            other => other,
        },
        Lang::Pt => match reason {
            "end_turn" => "Concluído",
            "cancelled" => "Cancelado",
            "max_tokens" => "Comprimento máximo atingido",
            "error" => "Erro",
            "timeout" => "Tempo esgotado",
            other => other,
        },
        Lang::Ar => match reason {
            "end_turn" => "اكتمل",
            "cancelled" => "ملغى",
            "max_tokens" => "تم بلوغ الحد الأقصى",
            "error" => "خطأ",
            "timeout" => "انتهت المهلة",
            other => other,
        },
        Lang::En => match reason {
            "end_turn" => "Completed",
            "cancelled" => "Cancelled",
            "max_tokens" => "Max length reached",
            "stop_sequence" => "Stop sequence",
            "error" => "Error",
            "timeout" => "Timeout",
            other => other,
        },
    }
    .to_string()
}

/// Remove and return up to `max_chars` Unicode chars from the front of `buf`.
/// Char-boundary safe (never splits a multi-byte UTF-8 codepoint). Used to
/// drain at most one Telegram-message-worth of streaming content per flush
/// while leaving any tail in the buffer for the next flush.
fn take_chunk_front(buf: &mut String, max_chars: usize) -> String {
    let mut split_byte = buf.len();
    let mut count = 0usize;
    for (i, _) in buf.char_indices() {
        if count >= max_chars {
            split_byte = i;
            break;
        }
        count += 1;
    }
    if split_byte >= buf.len() {
        return std::mem::take(buf);
    }
    let tail = buf.split_off(split_byte);
    std::mem::replace(buf, tail)
}

/// Split `text` into chunks of at most `max_chars` Unicode chars, preferring
/// line boundaries when packing. Lines longer than `max_chars` are hard-split
/// at char boundaries. Empty input returns an empty Vec.
fn split_into_chunks(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;

    for line in text.split_inclusive('\n') {
        let line_chars = line.chars().count();
        if line_chars > max_chars {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
                current_chars = 0;
            }
            // Hard-split a line that's longer than the per-message budget.
            let mut remaining = line;
            while !remaining.is_empty() {
                let mut split_byte = remaining.len();
                let mut count = 0usize;
                for (i, _) in remaining.char_indices() {
                    if count >= max_chars {
                        split_byte = i;
                        break;
                    }
                    count += 1;
                }
                let (head, tail) = remaining.split_at(split_byte);
                out.push(head.to_string());
                remaining = tail;
            }
        } else if current_chars + line_chars > max_chars {
            out.push(std::mem::take(&mut current));
            current.push_str(line);
            current_chars = line_chars;
        } else {
            current.push_str(line);
            current_chars += line_chars;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Extract a concise detail string from a tool call's `raw_input` JSON.
///
/// Returns a formatted string like `"Read: src/main.rs"` or `"Bash: npm test"`.
/// Falls back to the original title if no detail can be extracted.
fn format_tool_call_detail(title: &str, raw_input: Option<&str>) -> String {
    let parsed = raw_input.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());

    let normalized_title = title.to_lowercase().replace([' ', '-'], "_");

    if let Some(ref obj) = parsed {
        // File operations: read, edit, write, delete
        if let Some(path) = obj
            .get("file_path")
            .or_else(|| obj.get("path"))
            .or_else(|| obj.get("notebook_path"))
            .and_then(|v| v.as_str())
        {
            let short = short_path(path);
            let label = match normalized_title.as_str() {
                s if s.contains("write") => "Write",
                s if s.contains("edit") || s.contains("change") || s.contains("update") => "Edit",
                s if s.contains("delete") => "Delete",
                _ => "Read",
            };
            return format!("{label}: {short}");
        }

        // Bash / shell commands
        if let Some(cmd) = obj
            .get("command")
            .or_else(|| obj.get("cmd"))
            .and_then(|v| v.as_str())
        {
            let short = truncate_str(cmd.lines().next().unwrap_or(cmd), 80);
            return format!("Bash: {short}");
        }

        // Grep / search
        if let Some(pattern) = obj.get("pattern").and_then(|v| v.as_str()) {
            let path = obj.get("path").and_then(|v| v.as_str());
            return if let Some(p) = path {
                format!(
                    "Grep: \"{}\" in {}",
                    truncate_str(pattern, 40),
                    short_path(p)
                )
            } else {
                format!("Grep: \"{}\"", truncate_str(pattern, 60))
            };
        }

        // Glob
        if let Some(pat) = obj.get("glob").and_then(|v| v.as_str()) {
            return format!("Glob: {pat}");
        }

        // Agent / task
        if obj.get("subagent_type").is_some()
            || obj.get("task_id").is_some()
            || obj.get("subject").is_some()
        {
            let desc = obj
                .get("description")
                .or_else(|| obj.get("subject"))
                .or_else(|| obj.get("prompt"))
                .and_then(|v| v.as_str());
            if let Some(d) = desc {
                return format!("Agent: {}", truncate_str(d, 60));
            }
        }

        // Web fetch
        if let Some(url) = obj.get("url").and_then(|v| v.as_str()) {
            return format!("Fetch: {}", truncate_str(url, 80));
        }

        // Web search
        if let Some(query) = obj.get("query").and_then(|v| v.as_str()) {
            return format!("Search: {}", truncate_str(query, 60));
        }

        // TodoWrite
        if obj.get("todos").is_some() {
            return "TodoWrite".to_string();
        }
    }

    // Fallback: if raw_input is a plain string (e.g. a bare command), use it directly
    if let Some(raw) = raw_input {
        if !raw.starts_with('{') && !raw.starts_with('[') {
            let short = truncate_str(raw.lines().next().unwrap_or(raw), 80);
            if normalized_title.contains("bash")
                || normalized_title.contains("shell")
                || normalized_title.contains("exec")
            {
                return format!("Bash: {short}");
            }
        }
    }

    title.to_string()
}

fn short_path(path: &str) -> &str {
    // Show last 2 path components at most, or the full path if short enough
    if path.len() <= 60 {
        return path;
    }
    let parts: Vec<&str> = path.rsplitn(3, '/').collect();
    if parts.len() >= 2 {
        // e.g. "src/main.rs" from "/very/long/path/src/main.rs"
        let tail = &path[path.len() - parts[0].len() - parts[1].len() - 1..];
        if tail.len() < path.len() {
            return tail;
        }
    }
    path
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}
