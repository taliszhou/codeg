#[cfg(feature = "tauri-runtime")]
const SERVICE_NAME: &str = "codeg";

fn token_key(account_id: &str) -> String {
    format!("github-token:{}", account_id)
}

fn channel_token_key(channel_id: i32) -> String {
    format!("chat-channel:{}", channel_id)
}

/// Credential kinds for remote SSH connections.
/// Stored alongside the connection_id under the same `codeg` keyring service.
#[derive(Debug, Clone, Copy)]
pub enum SshCredentialKind {
    KeyPassphrase,
    Password,
}

impl SshCredentialKind {
    fn suffix(self) -> &'static str {
        match self {
            SshCredentialKind::KeyPassphrase => "key_passphrase",
            SshCredentialKind::Password => "password",
        }
    }

    pub fn all() -> [SshCredentialKind; 2] {
        [SshCredentialKind::KeyPassphrase, SshCredentialKind::Password]
    }
}

fn ssh_credential_key(connection_id: &str, kind: SshCredentialKind) -> String {
    format!("ssh:{}:{}", connection_id, kind.suffix())
}

// ── Tauri mode: OS keyring ──

#[cfg(feature = "tauri-runtime")]
pub fn set_token(account_id: &str, token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("keyring set error: {e}"))
}

#[cfg(feature = "tauri-runtime")]
pub fn get_token(account_id: &str) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id)).ok()?;
    entry.get_password().ok()
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_token(account_id: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

// ── Server mode: file-based token store ──

#[cfg(not(feature = "tauri-runtime"))]
fn tokens_file_path() -> std::path::PathBuf {
    let dir = std::env::var("CODEG_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .map(|d| d.join("codeg"))
                .unwrap_or_else(|| std::path::PathBuf::from(".codeg-data"))
        });
    dir.join("tokens.json")
}

#[cfg(not(feature = "tauri-runtime"))]
fn read_tokens() -> std::collections::HashMap<String, String> {
    let path = tokens_file_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[cfg(not(feature = "tauri-runtime"))]
fn write_tokens(tokens: &std::collections::HashMap<String, String>) -> Result<(), String> {
    let path = tokens_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create token store directory: {e}"))?;
    }
    let json = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("failed to serialize tokens: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("failed to write token store: {e}"))
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn set_token(account_id: &str, token: &str) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.insert(token_key(account_id), token.to_string());
    write_tokens(&tokens)
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn get_token(account_id: &str) -> Option<String> {
    read_tokens().get(&token_key(account_id)).cloned()
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn delete_token(account_id: &str) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.remove(&token_key(account_id));
    write_tokens(&tokens)
}

// ── Chat channel token helpers ──
// Reuse the same storage mechanism (keyring or file) with a different key prefix.

#[cfg(feature = "tauri-runtime")]
pub fn set_channel_token(channel_id: i32, token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("keyring set error: {e}"))
}

#[cfg(feature = "tauri-runtime")]
pub fn get_channel_token(channel_id: i32) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id)).ok()?;
    entry.get_password().ok()
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_channel_token(channel_id: i32) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn set_channel_token(channel_id: i32, token: &str) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.insert(channel_token_key(channel_id), token.to_string());
    write_tokens(&tokens)
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn get_channel_token(channel_id: i32) -> Option<String> {
    read_tokens().get(&channel_token_key(channel_id)).cloned()
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn delete_channel_token(channel_id: i32) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.remove(&channel_token_key(channel_id));
    write_tokens(&tokens)
}

// ── Remote SSH credential helpers ──
// One service name + per-connection key prefix, like channel tokens.

#[cfg(feature = "tauri-runtime")]
pub fn set_ssh_credential(
    connection_id: &str,
    kind: SshCredentialKind,
    secret: &str,
) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &ssh_credential_key(connection_id, kind))
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(secret)
        .map_err(|e| format!("keyring set error: {e}"))
}

#[cfg(feature = "tauri-runtime")]
pub fn get_ssh_credential(connection_id: &str, kind: SshCredentialKind) -> Option<String> {
    let entry =
        keyring::Entry::new(SERVICE_NAME, &ssh_credential_key(connection_id, kind)).ok()?;
    entry.get_password().ok()
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_ssh_credential(
    connection_id: &str,
    kind: SshCredentialKind,
) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &ssh_credential_key(connection_id, kind))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn set_ssh_credential(
    connection_id: &str,
    kind: SshCredentialKind,
    secret: &str,
) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.insert(ssh_credential_key(connection_id, kind), secret.to_string());
    write_tokens(&tokens)
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn get_ssh_credential(connection_id: &str, kind: SshCredentialKind) -> Option<String> {
    read_tokens()
        .get(&ssh_credential_key(connection_id, kind))
        .cloned()
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn delete_ssh_credential(
    connection_id: &str,
    kind: SshCredentialKind,
) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.remove(&ssh_credential_key(connection_id, kind));
    write_tokens(&tokens)
}

/// Best-effort delete of every credential associated with a connection.
/// Errors on individual kinds are aggregated; a `NoEntry` is treated as success.
pub fn delete_all_ssh_credentials(connection_id: &str) -> Result<(), String> {
    let mut errors: Vec<String> = Vec::new();
    for kind in SshCredentialKind::all() {
        if let Err(e) = delete_ssh_credential(connection_id, kind) {
            errors.push(e);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
