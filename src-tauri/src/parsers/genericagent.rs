use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::models::*;
use crate::parsers::{
    compute_session_stats, folder_name_from_path, relocate_orphaned_tool_results, truncate_str,
    AgentParser, ParseError,
};

/// Persistence layout mirrors the bridge (`genericagent_acp_bridge.py`):
/// `~/.genericagent/projects/<encoded-cwd>/<session_id>.json`, where the file
/// is a JSON array of Claude-style messages (`{"role", "content":[blocks]}`).
pub struct GenericAgentParser {
    base_dir: PathBuf,
}

impl GenericAgentParser {
    pub fn new() -> Self {
        Self {
            base_dir: resolve_base_dir(),
        }
    }

    /// Best-effort decode of the encoded cwd dir name back to a path.
    /// Encoding replaced '/' with '-' and stripped ':' (lossy), so this only
    /// reverses the separator substitution.
    fn decode_folder_path(encoded: &str) -> String {
        encoded.replace('-', "/")
    }

    fn file_mtime(path: &Path) -> DateTime<Utc> {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now())
    }

    fn parse_file(
        &self,
        path: &Path,
        session_id: &str,
        folder_path: Option<String>,
    ) -> Result<ConversationDetail, ParseError> {
        let raw = fs::read_to_string(path)?;
        let history: Vec<serde_json::Value> = serde_json::from_str(&raw)?;

        let messages: Vec<UnifiedMessage> = history
            .iter()
            .enumerate()
            .map(|(idx, msg)| build_message(idx, msg))
            .collect();

        let mut turns = group_into_turns(messages);
        relocate_orphaned_tool_results(&mut turns);

        let timestamp = Self::file_mtime(path);
        let title = first_user_text(&turns).map(|t| truncate_str(&t, 60));
        let folder_name = folder_path.as_deref().map(folder_name_from_path);
        let session_stats = compute_session_stats(&turns);

        let summary = ConversationSummary {
            id: session_id.to_string(),
            agent_type: AgentType::GenericAgent,
            folder_path,
            folder_name,
            title,
            started_at: timestamp,
            ended_at: Some(timestamp),
            message_count: turns.len() as u32,
            model: None,
            git_branch: None,
            parent_id: None,
            parent_tool_use_id: None,
            delegation_call_id: None,
        };

        Ok(ConversationDetail {
            summary,
            turns,
            session_stats,
            transcript_watermark: Some(raw.len() as u64),
        })
    }
}

fn resolve_base_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".genericagent").join("projects")
}

/// Convert a saved history entry into a `UnifiedMessage`.
///
/// The bridge persists `agent.history`, whose entries are either prefixed
/// strings (`[USER]: …` / `[Agent] …`, the GeneraticAgent summary format) or
/// Claude-style `{role, content:[blocks]}` dicts (the llmcore message format).
fn build_message(idx: usize, msg: &serde_json::Value) -> UnifiedMessage {
    let (role, content) = match msg {
        serde_json::Value::String(s) => {
            let (role, text) = parse_string_entry(s);
            let content = if text.is_empty() {
                vec![]
            } else {
                vec![ContentBlock::Text { text }]
            };
            (role, content)
        }
        _ => {
            let role = match msg.get("role").and_then(|r| r.as_str()) {
                Some("assistant") => MessageRole::Assistant,
                Some("system") => MessageRole::System,
                Some("tool") => MessageRole::Tool,
                _ => MessageRole::User,
            };
            (role, convert_content(msg.get("content")))
        }
    };

    UnifiedMessage {
        id: format!("msg-{}", idx),
        role,
        content,
        timestamp: Utc::now(),
        usage: None,
        duration_ms: None,
        agent_message_id: None,
        model: None,
        completed_at: None,
    }
}

/// Parse a prefixed history string into (role, display text). `[USER]:` marks a
/// user turn; everything else (notably `[Agent]`) is treated as assistant, with
/// the recognized leading tag stripped for cleaner rendering.
fn parse_string_entry(s: &str) -> (MessageRole, String) {
    let trimmed = s.trim_start();
    if let Some(rest) = trimmed.strip_prefix("[USER]:") {
        return (MessageRole::User, rest.trim().to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("[USER]") {
        return (MessageRole::User, rest.trim().to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("[Agent]") {
        return (MessageRole::Assistant, rest.trim().to_string());
    }
    (MessageRole::Assistant, trimmed.to_string())
}

/// Convert a message `content` field (string or array of blocks) into blocks.
fn convert_content(content: Option<&serde_json::Value>) -> Vec<ContentBlock> {
    match content {
        Some(serde_json::Value::String(s)) => {
            if s.trim().is_empty() {
                vec![]
            } else {
                vec![ContentBlock::Text { text: s.clone() }]
            }
        }
        Some(serde_json::Value::Array(arr)) => {
            arr.iter().filter_map(convert_block).collect()
        }
        _ => vec![],
    }
}

fn convert_block(block: &serde_json::Value) -> Option<ContentBlock> {
    match block.get("type").and_then(|t| t.as_str()) {
        Some("text") => {
            let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if text.trim().is_empty() {
                None
            } else {
                Some(ContentBlock::Text {
                    text: text.to_string(),
                })
            }
        }
        Some("thinking") => block
            .get("thinking")
            .and_then(|t| t.as_str())
            .filter(|t| !t.trim().is_empty())
            .map(|t| ContentBlock::Thinking {
                text: t.to_string(),
            }),
        Some("tool_use") => {
            let tool_name = block
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("tool")
                .to_string();
            let tool_use_id = block
                .get("id")
                .and_then(|i| i.as_str())
                .map(|s| s.to_string());
            let input_preview = block
                .get("input")
                .map(|v| serde_json::to_string(v).unwrap_or_default());
            Some(ContentBlock::ToolUse {
                tool_use_id,
                tool_name,
                input_preview,
                status: None,
                meta: None,
            })
        }
        Some("tool_result") => {
            let tool_use_id = block
                .get("tool_use_id")
                .and_then(|i| i.as_str())
                .map(|s| s.to_string());
            let is_error = block
                .get("is_error")
                .and_then(|e| e.as_bool())
                .unwrap_or(false);
            Some(ContentBlock::ToolResult {
                tool_use_id,
                output_preview: extract_tool_result_text(block.get("content")),
                is_error,
                agent_stats: None,
                images: vec![],
            })
        }
        Some("image") => {
            let source = block.get("source")?;
            let data = source.get("data").and_then(|d| d.as_str())?.to_string();
            let mime_type = source
                .get("media_type")
                .and_then(|m| m.as_str())
                .unwrap_or("image/png")
                .to_string();
            Some(ContentBlock::Image {
                data,
                mime_type,
                uri: None,
            })
        }
        _ => None,
    }
}

/// Tool result `content` is either a string or an array of text blocks.
fn extract_tool_result_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(arr)) => {
            let texts: Vec<String> = arr
                .iter()
                .filter_map(|c| {
                    if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                        c.get("text").and_then(|t| t.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        _ => None,
    }
}

fn is_tool_result_only(msg: &UnifiedMessage) -> bool {
    matches!(msg.role, MessageRole::User)
        && !msg.content.is_empty()
        && msg
            .content
            .iter()
            .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

/// Group flat messages into turns: an assistant message absorbs the following
/// tool-result-only user messages (Claude convention).
fn group_into_turns(messages: Vec<UnifiedMessage>) -> Vec<MessageTurn> {
    let mut turns = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];
        let role = match msg.role {
            MessageRole::Assistant => TurnRole::Assistant,
            MessageRole::System => TurnRole::System,
            _ => TurnRole::User,
        };

        if matches!(msg.role, MessageRole::Assistant) {
            let mut blocks = msg.content.clone();
            let timestamp = msg.timestamp;
            i += 1;
            while i < messages.len() && is_tool_result_only(&messages[i]) {
                blocks.extend(messages[i].content.clone());
                i += 1;
            }
            if blocks.is_empty() {
                continue;
            }
            turns.push(MessageTurn {
                id: format!("turn-{}", turns.len()),
                role: TurnRole::Assistant,
                blocks,
                timestamp,
                usage: None,
                duration_ms: None,
                model: None,
                completed_at: None,
                agent_message_id: None,
            });
        } else {
            if msg.content.is_empty() {
                i += 1;
                continue;
            }
            turns.push(MessageTurn {
                id: format!("turn-{}", turns.len()),
                role,
                blocks: msg.content.clone(),
                timestamp: msg.timestamp,
                usage: None,
                duration_ms: None,
                agent_message_id: None,
                model: None,
                completed_at: None,
            });
            i += 1;
        }
    }

    turns
}

fn first_user_text(turns: &[MessageTurn]) -> Option<String> {
    turns
        .iter()
        .find(|t| matches!(t.role, TurnRole::User))
        .and_then(|t| {
            t.blocks.iter().find_map(|b| match b {
                ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            })
        })
}

impl AgentParser for GenericAgentParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }

        let mut summaries = Vec::new();
        for project in fs::read_dir(&self.base_dir)? {
            let project = match project {
                Ok(p) => p,
                Err(_) => continue,
            };
            let project_dir = project.path();
            if !project_dir.is_dir() {
                continue;
            }
            let folder_path = project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(Self::decode_folder_path);

            for entry in fs::read_dir(&project_dir)? {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let session_id = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if let Ok(detail) = self.parse_file(&path, &session_id, folder_path.clone()) {
                    summaries.push(detail.summary);
                }
            }
        }

        summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(summaries)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        if !self.base_dir.exists() {
            return Err(ParseError::ConversationNotFound(conversation_id.to_string()));
        }

        // The session file lives under some <encoded-cwd>/ dir, but the caller
        // only has the session id — scan project dirs for `<id>.json`.
        for project in fs::read_dir(&self.base_dir)? {
            let project = match project {
                Ok(p) => p,
                Err(_) => continue,
            };
            let project_dir = project.path();
            if !project_dir.is_dir() {
                continue;
            }
            let file_path = project_dir.join(format!("{}.json", conversation_id));
            if file_path.exists() {
                let folder_path = project_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(Self::decode_folder_path);
                return self.parse_file(&file_path, conversation_id, folder_path);
            }
        }

        Err(ParseError::ConversationNotFound(conversation_id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn write_session(dir: &Path, id: &str, history: serde_json::Value) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{}.json", id));
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(history.to_string().as_bytes()).unwrap();
        path
    }

    #[test]
    fn parses_prefixed_string_history() {
        let tmp = std::env::temp_dir().join(format!("ga_test_str_{}", std::process::id()));
        let project = tmp.join("-opt-GenericAgent");
        let history = json!([
            "[USER]: 介绍一下你自己",
            "[Agent] 直接回答了用户问题"
        ]);
        let path = write_session(&project, "ga_str", history);

        let parser = GenericAgentParser {
            base_dir: tmp.clone(),
        };
        let detail = parser.get_conversation("ga_str").unwrap();
        assert_eq!(detail.turns.len(), 2);
        assert!(matches!(detail.turns[0].role, TurnRole::User));
        assert!(matches!(detail.turns[1].role, TurnRole::Assistant));
        assert_eq!(detail.summary.title.as_deref(), Some("介绍一下你自己"));
        assert_eq!(detail.summary.folder_path.as_deref(), Some("/opt/GenericAgent"));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn parses_text_and_tool_turns() {
        let tmp = std::env::temp_dir().join(format!("ga_test_{}", std::process::id()));
        let project = tmp.join("-Users-demo-proj");
        let history = json!([
            {"role": "user", "content": [{"type": "text", "text": "hi there"}]},
            {"role": "assistant", "content": [
                {"type": "text", "text": "let me check"},
                {"type": "tool_use", "id": "t1", "name": "bash", "input": {"cmd": "ls"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "file.txt", "is_error": false}
            ]},
            {"role": "assistant", "content": [{"type": "text", "text": "done"}]}
        ]);
        let path = write_session(&project, "ga_abc", history);

        let parser = GenericAgentParser {
            base_dir: tmp.clone(),
        };
        let detail = parser.get_conversation("ga_abc").unwrap();
        // user turn + assistant(absorbs tool_result) + assistant
        assert_eq!(detail.turns.len(), 3);
        assert!(matches!(detail.turns[0].role, TurnRole::User));
        assert_eq!(detail.summary.title.as_deref(), Some("hi there"));
        // tool_result merged into the assistant turn holding tool_use
        let has_result = detail.turns[1]
            .blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
        assert!(has_result);

        let _ = fs::remove_dir_all(&path.parent().unwrap());
    }

    #[test]
    fn missing_session_is_not_found() {
        let tmp = std::env::temp_dir().join(format!("ga_test_missing_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let parser = GenericAgentParser {
            base_dir: tmp.clone(),
        };
        assert!(matches!(
            parser.get_conversation("nope"),
            Err(ParseError::ConversationNotFound(_))
        ));
        let _ = fs::remove_dir_all(&tmp);
    }
}
