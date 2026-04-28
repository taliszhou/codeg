use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    /// Reserved for future use; currently always `Ssh`.
    Local,
    Ssh,
}

impl ConnectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectionKind::Local => "local",
            ConnectionKind::Ssh => "ssh",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "local" => ConnectionKind::Local,
            _ => ConnectionKind::Ssh,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthMethod {
    /// SSH key file (with optional passphrase in keyring).
    Key,
    /// Password (stored in keyring).
    Password,
    /// Inherit everything from `~/.ssh/config` alias (no codeg-managed creds).
    SshConfig,
}

impl SshAuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            SshAuthMethod::Key => "key",
            SshAuthMethod::Password => "password",
            SshAuthMethod::SshConfig => "ssh_config",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "password" => SshAuthMethod::Password,
            "ssh_config" => SshAuthMethod::SshConfig,
            _ => SshAuthMethod::Key,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub kind: ConnectionKind,
    pub ssh_host: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_alias: Option<String>,
    pub ssh_key_path: Option<String>,
    pub ssh_auth_method: SshAuthMethod,
    pub proxy_jump: Option<String>,
    pub daemon_path: String,
    pub daemon_version: Option<String>,
    pub auto_connect: bool,
    pub last_connected_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for create / update. Does not include id (server generates) or
/// timestamps. Credentials are passed via `ConnectionCredentials` separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInput {
    pub name: String,
    pub kind: ConnectionKind,
    pub ssh_host: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_alias: Option<String>,
    pub ssh_key_path: Option<String>,
    pub ssh_auth_method: SshAuthMethod,
    pub proxy_jump: Option<String>,
    pub daemon_path: Option<String>,
    pub auto_connect: Option<bool>,
}

/// Optional credential payload accompanying create / update. `None` for both
/// fields means "no credential change" (update) or "no credential" (create).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectionCredentials {
    pub key_passphrase: Option<String>,
    pub password: Option<String>,
}

/// Read-only view of a `Host` block in `~/.ssh/config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfigEntry {
    pub alias: String,
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}
