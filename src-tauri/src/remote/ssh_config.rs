use std::path::PathBuf;

use crate::models::connection::SshConfigEntry;

const CONFIG_FILE_NAME: &str = "config";

fn ssh_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh"))
}

#[derive(Debug, thiserror::Error)]
pub enum SshConfigError {
    #[error("ssh config io: {0}")]
    Io(String),
}

/// Read ~/.ssh/config and return the parsed `Host` blocks. Wildcard hosts
/// (containing `*`) are filtered out. Missing file returns an empty list
/// (not an error).
pub fn list_aliases() -> Result<Vec<SshConfigEntry>, SshConfigError> {
    let path = match ssh_dir() {
        Some(p) => p.join(CONFIG_FILE_NAME),
        None => return Ok(Vec::new()),
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| SshConfigError::Io(format!("read {:?}: {}", path, e)))?;
    Ok(parse_config(&content))
}

/// Minimal SSH config parser:
///
/// - Recognises `Host <alias [...]>` blocks and emits one entry per first
///   alias listed (additional aliases on the same line are ignored).
/// - Extracts `HostName` / `User` / `Port` / `IdentityFile` / `ProxyJump`.
/// - Filters out wildcard aliases (`*`, `*.example.com`, etc.) since they
///   are not friendly UI candidates.
/// - Ignores `Match`, `Include`, and any other keyword — those are handled
///   by the system `ssh` at runtime, not by codeg.
fn parse_config(content: &str) -> Vec<SshConfigEntry> {
    let mut entries: Vec<SshConfigEntry> = Vec::new();
    let mut current: Option<SshConfigEntry> = None;

    let push_if_real = |entries: &mut Vec<SshConfigEntry>, prev: Option<SshConfigEntry>| {
        if let Some(prev) = prev {
            if !prev.alias.is_empty() && !prev.alias.contains('*') && !prev.alias.contains('?') {
                entries.push(prev);
            }
        }
    };

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // SSH config allows both `key value` and `key=value` forms.
        let (key, value) = match line
            .split_once(|c: char| c.is_whitespace() || c == '=')
        {
            Some((k, v)) => (k.to_lowercase(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        if key == "host" {
            push_if_real(&mut entries, current.take());
            let alias = value.split_whitespace().next().unwrap_or("").to_string();
            current = Some(SshConfigEntry {
                alias,
                host: None,
                user: None,
                port: None,
                identity_file: None,
                proxy_jump: None,
            });
        } else if let Some(entry) = current.as_mut() {
            match key.as_str() {
                "hostname" => entry.host = Some(value.to_string()),
                "user" => entry.user = Some(value.to_string()),
                "port" => entry.port = value.parse().ok(),
                "identityfile" => entry.identity_file = Some(value.to_string()),
                "proxyjump" => entry.proxy_jump = Some(value.to_string()),
                _ => {}
            }
        }
    }
    push_if_real(&mut entries, current.take());
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let cfg = "\
Host dev
  HostName dev.example.com
  User alice
  Port 2222
  IdentityFile ~/.ssh/id_ed25519

Host gpu
  HostName gpu.example.com
  ProxyJump bastion

Host *
  ServerAliveInterval 60
";
        let entries = parse_config(cfg);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].alias, "dev");
        assert_eq!(entries[0].host.as_deref(), Some("dev.example.com"));
        assert_eq!(entries[0].user.as_deref(), Some("alice"));
        assert_eq!(entries[0].port, Some(2222));
        assert_eq!(
            entries[0].identity_file.as_deref(),
            Some("~/.ssh/id_ed25519")
        );
        assert_eq!(entries[1].alias, "gpu");
        assert_eq!(entries[1].proxy_jump.as_deref(), Some("bastion"));
    }

    #[test]
    fn parse_filters_wildcards() {
        let cfg = "\
Host *
  User root

Host *.internal
  User svc

Host real
  HostName r.example.com
";
        let entries = parse_config(cfg);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].alias, "real");
    }

    #[test]
    fn parse_ignores_comments_and_empty_lines() {
        let cfg = "\
# this is a comment
Host alpha
   # indented comment

  HostName a.example.com
";
        let entries = parse_config(cfg);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].host.as_deref(), Some("a.example.com"));
    }

    #[test]
    fn parse_handles_eq_form() {
        let cfg = "\
Host beta
  HostName=b.example.com
  Port=2200
";
        let entries = parse_config(cfg);
        assert_eq!(entries[0].host.as_deref(), Some("b.example.com"));
        assert_eq!(entries[0].port, Some(2200));
    }

    #[test]
    fn parse_handles_match_block_skipped() {
        // Match blocks are not parsed — we just don't attribute their
        // settings to any Host alias. We must NOT crash.
        let cfg = "\
Match user gitlab-ci
  IdentityFile ~/.ssh/ci

Host real
  HostName r.example.com
";
        let entries = parse_config(cfg);
        assert!(entries.iter().any(|e| e.alias == "real"));
    }
}
