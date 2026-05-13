use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::models::*;
use crate::parsers::{folder_name_from_path, AgentParser, ParseError};

/// Reads GenericAgent session history from `~/.genericagent/projects/<encoded-cwd>/<session_id>.json`.
/// GA encodes the cwd by replacing `/` (and `\` on Windows) with `-` and stripping `:`.
/// Each session file is a flat JSON array of strings:
///   `["[USER]: ...", "[Agent] ...", ...]`
pub struct GenericAgentParser {
    base_dir: PathBuf,
}

impl GenericAgentParser {
    pub fn new() -> Self {
        Self {
            base_dir: resolve_base_dir(),
        }
    }
}

fn resolve_base_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".genericagent")
        .join("projects")
}

/// Reverse of GA's cwd encoding. Lossy for cwds containing literal `-`,
/// but adequate for display since POSIX paths rarely use `-` as a separator.
fn decode_folder_path(encoded: &str) -> String {
    let mut decoded = encoded.replace('-', "/");
    if !decoded.starts_with('/') {
        decoded.insert(0, '/');
    }
    decoded
}

fn file_modified(path: &PathBuf) -> DateTime<Utc> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now())
}

fn build_turn(
    idx: usize,
    entry: &str,
    session_id: &str,
    timestamp: DateTime<Utc>,
) -> Option<MessageTurn> {
    let entry_trimmed = entry.trim();
    if entry_trimmed.is_empty() {
        return None;
    }

    let (role, text) = if let Some(rest) = entry_trimmed.strip_prefix("[USER]:") {
        (TurnRole::User, rest.trim().to_string())
    } else if let Some(rest) = entry_trimmed.strip_prefix("[Agent]") {
        (TurnRole::Assistant, rest.trim().to_string())
    } else {
        (TurnRole::Assistant, entry_trimmed.to_string())
    };

    if text.is_empty() {
        return None;
    }

    Some(MessageTurn {
        id: format!("{}-{}", session_id, idx),
        role,
        blocks: vec![ContentBlock::Text { text }],
        timestamp,
        usage: None,
        duration_ms: None,
        model: None,
    })
}

fn parse_session_file(
    path: &PathBuf,
    session_id: &str,
) -> Result<Vec<MessageTurn>, ParseError> {
    let content = fs::read_to_string(path)?;
    let entries: Vec<String> = serde_json::from_str(&content)?;
    let timestamp = file_modified(path);

    let turns = entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| build_turn(idx, entry, session_id, timestamp))
        .collect();
    Ok(turns)
}

impl AgentParser for GenericAgentParser {
    fn list_conversations(&self) -> Result<Vec<ConversationSummary>, ParseError> {
        let mut conversations = Vec::new();
        if !self.base_dir.exists() {
            return Ok(conversations);
        }

        for project_entry in fs::read_dir(&self.base_dir)? {
            let project_entry = match project_entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let project_dir = project_entry.path();
            if !project_dir.is_dir() {
                continue;
            }
            let encoded = project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let folder_path = decode_folder_path(&encoded);
            let folder_name = folder_name_from_path(&folder_path);

            let session_files = match fs::read_dir(&project_dir) {
                Ok(it) => it,
                Err(_) => continue,
            };
            for file_entry in session_files {
                let file_entry = match file_entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let file_path = file_entry.path();
                if file_path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let session_id = match file_path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let started_at = file_modified(&file_path);
                let message_count = fs::read_to_string(&file_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .map(|v| v.len() as u32)
                    .unwrap_or(0);

                conversations.push(ConversationSummary {
                    id: session_id,
                    agent_type: AgentType::GenericAgent,
                    folder_path: Some(folder_path.clone()),
                    folder_name: Some(folder_name.clone()),
                    title: None,
                    started_at,
                    ended_at: None,
                    message_count,
                    model: None,
                    git_branch: None,
                });
            }
        }

        conversations.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(conversations)
    }

    fn get_conversation(&self, conversation_id: &str) -> Result<ConversationDetail, ParseError> {
        if !self.base_dir.exists() {
            return Err(ParseError::ConversationNotFound(conversation_id.to_string()));
        }

        for project_entry in fs::read_dir(&self.base_dir)? {
            let project_entry = match project_entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let project_dir = project_entry.path();
            if !project_dir.is_dir() {
                continue;
            }
            let file_path = project_dir.join(format!("{}.json", conversation_id));
            if !file_path.exists() {
                continue;
            }

            let encoded = project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let folder_path = decode_folder_path(&encoded);
            let folder_name = folder_name_from_path(&folder_path);
            let started_at = file_modified(&file_path);
            let turns = parse_session_file(&file_path, conversation_id)?;
            let message_count = turns.len() as u32;

            return Ok(ConversationDetail {
                summary: ConversationSummary {
                    id: conversation_id.to_string(),
                    agent_type: AgentType::GenericAgent,
                    folder_path: Some(folder_path),
                    folder_name: Some(folder_name),
                    title: None,
                    started_at,
                    ended_at: None,
                    message_count,
                    model: None,
                    git_branch: None,
                },
                turns,
                session_stats: None,
            });
        }

        Err(ParseError::ConversationNotFound(conversation_id.to_string()))
    }
}
