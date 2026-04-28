// Shared helpers for spawning system `ssh` against a connection.
//
// Centralises the SSH argument vector so probe/test/bootstrap/tunnel paths
// share the same flags and ControlMaster path. ControlMaster lets us
// multiplex multiple SSH operations over a single authenticated connection,
// which is essential for the bootstrap pipeline (probe + deploy + launch +
// tunnel ≈ 4 sequential SSH invocations).

use std::path::PathBuf;

use crate::models::connection::{ConnectionConfig, SshAuthMethod};

/// Resolve `~` / `~/...` in a local path against the host's home dir. We
/// pre-expand because system ssh's tilde handling is shell-dependent.
pub fn expand_tilde_in_local_path(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped).to_string_lossy().into_owned();
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// POSIX single-quote escape: wrap `s` in `'…'`, replace embedded `'` with
/// `'\''`. Safe to drop into a shell command line.
pub fn posix_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Where the per-connection ControlMaster socket lives. UNIX domain sockets
/// have a 104-byte path limit on macOS; if the natural path exceeds that, we
/// fall back to `/tmp` with a short prefix.
pub fn control_path_for(connection_id: &str) -> PathBuf {
    let preferred = dirs::home_dir()
        .map(|h| h.join(".codeg").join("ssh-control"))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(format!("{}.sock", short_id(connection_id)));

    if preferred.to_string_lossy().len() <= 100 {
        preferred
    } else {
        PathBuf::from(format!("/tmp/codeg-cm-{}.sock", short_id(connection_id)))
    }
}

fn short_id(connection_id: &str) -> String {
    // Strip the `conn_` prefix and keep the first 16 hex chars; UUIDs are 32
    // hex chars, so this is unique enough for a control socket name.
    let trimmed = connection_id.strip_prefix("conn_").unwrap_or(connection_id);
    trimmed.chars().take(16).collect()
}

/// Common SSH arg vector ending with the SSH target (user@host or alias).
/// Caller appends operation-specific args (e.g. remote command, `-N -L`).
///
/// Includes ControlMaster directives so subsequent invocations for the same
/// connection reuse the master socket (no re-auth, no re-handshake).
pub fn base_ssh_args(config: &ConnectionConfig) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={}", control_path_for(&config.id).display()),
        "-o".into(),
        "ControlPersist=10m".into(),
    ];

    if let Some(port) = config.ssh_port {
        if config.ssh_alias.is_none() {
            args.push("-p".into());
            args.push(port.to_string());
        }
    }
    if config.ssh_auth_method == SshAuthMethod::Key {
        if let Some(key) = &config.ssh_key_path {
            args.push("-i".into());
            args.push(expand_tilde_in_local_path(key));
        }
    }
    if let Some(jump) = &config.proxy_jump {
        if !jump.trim().is_empty() {
            args.push("-J".into());
            args.push(jump.clone());
        }
    }

    args.push(build_ssh_target(config));
    args
}

pub fn build_ssh_target(config: &ConnectionConfig) -> String {
    if let Some(alias) = &config.ssh_alias {
        return alias.clone();
    }
    let user = config.ssh_user.as_deref().unwrap_or("");
    let host = config.ssh_host.as_deref().unwrap_or("");
    if user.is_empty() {
        host.to_string()
    } else {
        format!("{user}@{host}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::connection::ConnectionKind;

    fn cfg() -> ConnectionConfig {
        ConnectionConfig {
            id: "conn_abc1234567890def".into(),
            name: "test".into(),
            kind: ConnectionKind::Ssh,
            ssh_host: Some("example.com".into()),
            ssh_user: Some("alice".into()),
            ssh_port: Some(2222),
            ssh_alias: None,
            ssh_key_path: Some("~/.ssh/id_ed25519".into()),
            ssh_auth_method: SshAuthMethod::Key,
            proxy_jump: None,
            daemon_path: "~/.codeg-remote".into(),
            daemon_version: None,
            auto_connect: false,
            last_connected_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn base_args_include_control_master() {
        let args = base_ssh_args(&cfg());
        assert!(args.iter().any(|a| a == "ControlMaster=auto"));
        assert!(args.iter().any(|a| a.starts_with("ControlPath=")));
        assert!(args.iter().any(|a| a == "ControlPersist=10m"));
        assert_eq!(args.last().unwrap(), "alice@example.com");
    }

    #[test]
    fn base_args_alias_skips_port() {
        let mut c = cfg();
        c.ssh_host = None;
        c.ssh_alias = Some("dev".into());
        let args = base_ssh_args(&c);
        assert!(!args.iter().any(|a| a == "-p"));
        assert_eq!(args.last().unwrap(), "dev");
    }

    #[test]
    fn base_args_password_method_skips_identity_file() {
        let mut c = cfg();
        c.ssh_auth_method = SshAuthMethod::Password;
        let args = base_ssh_args(&c);
        assert!(!args.iter().any(|a| a == "-i"));
    }

    #[test]
    fn base_args_proxy_jump() {
        let mut c = cfg();
        c.proxy_jump = Some("bastion".into());
        let args = base_ssh_args(&c);
        let i = args.iter().position(|a| a == "-J").unwrap();
        assert_eq!(args[i + 1], "bastion");
    }

    #[test]
    fn target_alias_takes_precedence() {
        let mut c = cfg();
        c.ssh_alias = Some("dev".into());
        assert_eq!(build_ssh_target(&c), "dev");
    }

    #[test]
    fn target_no_user() {
        let mut c = cfg();
        c.ssh_user = None;
        assert_eq!(build_ssh_target(&c), "example.com");
    }

    #[test]
    fn posix_quote_basic() {
        assert_eq!(posix_single_quote("safe"), "'safe'");
        assert_eq!(posix_single_quote("ab'cd"), "'ab'\\''cd'");
    }

    #[test]
    fn local_tilde_expansion() {
        let expanded = expand_tilde_in_local_path("~/foo");
        assert!(!expanded.starts_with('~'));
    }

    #[test]
    fn control_path_short_id_strip_prefix() {
        let p = control_path_for("conn_abc1234567890defXXX");
        let s = p.to_string_lossy();
        assert!(s.contains("abc1234567890def"));
        assert!(!s.contains("conn_"));
    }
}
