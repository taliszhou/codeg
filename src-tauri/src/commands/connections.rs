use crate::app_error::AppCommandError;
use crate::db::service::connection_service;
use crate::db::AppDatabase;
use crate::keyring_store::{self, SshCredentialKind};
use crate::models::connection::{
    ConnectionConfig, ConnectionCredentials, ConnectionInput, SshAuthMethod, SshConfigEntry,
};
use crate::remote::connection::ConnectionRuntime;
use crate::remote::{connection_test, ssh_config, RemoteConnectionManager};
use crate::web::event_bridge::EventEmitter;

#[cfg(feature = "tauri-runtime")]
use tauri::State;

/// Persist any non-empty fields of `creds` into the OS keyring under the
/// connection id. Empty strings are treated as "no change". `None` and ""
/// are both no-ops to make UI plumbing easier (forms send `null` for
/// untouched fields and "" for "user cleared this field").
fn write_credentials(
    connection_id: &str,
    creds: &ConnectionCredentials,
) -> Result<(), AppCommandError> {
    if let Some(p) = creds.key_passphrase.as_ref() {
        if !p.is_empty() {
            keyring_store::set_ssh_credential(
                connection_id,
                SshCredentialKind::KeyPassphrase,
                p,
            )
            .map_err(|e| AppCommandError::io_error(format!("save key passphrase: {e}")))?;
        }
    }
    if let Some(p) = creds.password.as_ref() {
        if !p.is_empty() {
            keyring_store::set_ssh_credential(connection_id, SshCredentialKind::Password, p)
                .map_err(|e| AppCommandError::io_error(format!("save password: {e}")))?;
        }
    }
    Ok(())
}

/// Validation rules shared by create and update.
fn validate_input(input: &ConnectionInput) -> Result<(), AppCommandError> {
    if input.name.trim().is_empty() {
        return Err(AppCommandError::invalid_input("connection name is required"));
    }
    let has_alias = input.ssh_alias.as_deref().map(str::trim).is_some_and(|s| !s.is_empty());
    let has_host = input.ssh_host.as_deref().map(str::trim).is_some_and(|s| !s.is_empty());
    if !has_alias && !has_host {
        return Err(AppCommandError::invalid_input(
            "ssh_host or ssh_alias is required",
        ));
    }
    if let Some(port) = input.ssh_port {
        if port == 0 {
            return Err(AppCommandError::invalid_input("ssh_port must be 1-65535"));
        }
    }
    if input.ssh_auth_method == SshAuthMethod::Key
        && input
            .ssh_key_path
            .as_deref()
            .map(str::trim)
            .is_none_or(|s| s.is_empty())
        && !has_alias
    {
        // Allow alias mode without explicit key path (delegated to ~/.ssh/config).
        return Err(AppCommandError::invalid_input(
            "ssh_key_path is required for key auth when not using ssh_config alias",
        ));
    }
    Ok(())
}

pub async fn list_connections_inner(
    db: &AppDatabase,
) -> Result<Vec<ConnectionConfig>, AppCommandError> {
    Ok(connection_service::list(&db.conn).await?)
}

pub async fn create_connection_inner(
    db: &AppDatabase,
    input: ConnectionInput,
    creds: ConnectionCredentials,
) -> Result<ConnectionConfig, AppCommandError> {
    validate_input(&input)?;
    let config = connection_service::create(&db.conn, input).await?;
    write_credentials(&config.id, &creds)?;
    Ok(config)
}

pub async fn update_connection_inner(
    db: &AppDatabase,
    id: &str,
    input: ConnectionInput,
    creds: ConnectionCredentials,
) -> Result<ConnectionConfig, AppCommandError> {
    validate_input(&input)?;
    let config = connection_service::update(&db.conn, id, input).await?;
    write_credentials(&config.id, &creds)?;
    Ok(config)
}

pub async fn delete_connection_inner(
    db: &AppDatabase,
    id: &str,
) -> Result<(), AppCommandError> {
    connection_service::delete(&db.conn, id).await?;
    keyring_store::delete_all_ssh_credentials(id)
        .map_err(|e| AppCommandError::io_error(format!("delete keyring entries: {e}")))?;
    Ok(())
}

pub async fn list_ssh_config_aliases_inner() -> Result<Vec<SshConfigEntry>, AppCommandError> {
    ssh_config::list_aliases()
        .map_err(|e| AppCommandError::io_error(format!("read ssh config: {e}")))
}

pub async fn test_connection_inner(
    db: &AppDatabase,
    emitter: &EventEmitter,
    rcm: Option<&RemoteConnectionManager>,
    id: &str,
    test_id: &str,
) -> Result<Vec<connection_test::StageResult>, AppCommandError> {
    let config = connection_service::get_by_id(&db.conn, id)
        .await?
        .ok_or_else(|| AppCommandError::not_found(format!("connection {id} not found")))?;
    Ok(connection_test::run_test(&config, emitter, test_id, rcm).await)
}

// ── Tauri command wrappers ──

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_connections(
    db: State<'_, AppDatabase>,
) -> Result<Vec<ConnectionConfig>, AppCommandError> {
    list_connections_inner(&db).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn create_connection(
    db: State<'_, AppDatabase>,
    input: ConnectionInput,
    key_passphrase: Option<String>,
    password: Option<String>,
) -> Result<ConnectionConfig, AppCommandError> {
    create_connection_inner(
        &db,
        input,
        ConnectionCredentials {
            key_passphrase,
            password,
        },
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn update_connection(
    db: State<'_, AppDatabase>,
    id: String,
    input: ConnectionInput,
    key_passphrase: Option<String>,
    password: Option<String>,
) -> Result<ConnectionConfig, AppCommandError> {
    update_connection_inner(
        &db,
        &id,
        input,
        ConnectionCredentials {
            key_passphrase,
            password,
        },
    )
    .await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn delete_connection(
    db: State<'_, AppDatabase>,
    id: String,
) -> Result<(), AppCommandError> {
    delete_connection_inner(&db, &id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn list_ssh_config_aliases() -> Result<Vec<SshConfigEntry>, AppCommandError> {
    list_ssh_config_aliases_inner().await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn test_connection(
    db: State<'_, AppDatabase>,
    rcm: State<'_, RemoteConnectionManager>,
    app: tauri::AppHandle,
    id: String,
    test_id: String,
) -> Result<Vec<connection_test::StageResult>, AppCommandError> {
    let emitter = EventEmitter::Tauri(app);
    test_connection_inner(&db, &emitter, Some(&rcm), &id, &test_id).await
}

// ── CG-002.4: connect lifecycle commands ──

pub async fn open_connection_inner(
    db: &AppDatabase,
    rcm: &RemoteConnectionManager,
    id: &str,
) -> Result<(), AppCommandError> {
    let cfg = connection_service::get_by_id(&db.conn, id)
        .await?
        .ok_or_else(|| AppCommandError::not_found(format!("connection {id} not found")))?;
    rcm.connect(cfg)
        .await
        .map_err(|e| AppCommandError::external_command("open connection", e.to_string()))?;
    Ok(())
}

pub async fn close_connection_inner(
    rcm: &RemoteConnectionManager,
    id: &str,
) -> Result<(), AppCommandError> {
    rcm.disconnect(id)
        .await
        .map_err(|e| AppCommandError::external_command("close connection", e.to_string()))?;
    Ok(())
}

pub async fn resume_connection_after_manual_inner(
    rcm: &RemoteConnectionManager,
    id: &str,
) -> Result<(), AppCommandError> {
    rcm.resume_after_manual(id)
        .await
        .map_err(|e| AppCommandError::external_command("resume connection", e.to_string()))?;
    Ok(())
}

pub async fn hard_reset_connection_inner(
    rcm: &RemoteConnectionManager,
    id: &str,
) -> Result<(), AppCommandError> {
    rcm.hard_reset(id)
        .await
        .map_err(|e| AppCommandError::external_command("hard reset connection", e.to_string()))?;
    Ok(())
}

pub async fn get_connection_runtime_inner(
    rcm: &RemoteConnectionManager,
    id: &str,
) -> Option<ConnectionRuntime> {
    rcm.current_runtime(id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn open_connection(
    db: State<'_, AppDatabase>,
    rcm: State<'_, RemoteConnectionManager>,
    id: String,
) -> Result<(), AppCommandError> {
    open_connection_inner(&db, &rcm, &id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn close_connection(
    rcm: State<'_, RemoteConnectionManager>,
    id: String,
) -> Result<(), AppCommandError> {
    close_connection_inner(&rcm, &id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn resume_connection_after_manual(
    rcm: State<'_, RemoteConnectionManager>,
    id: String,
) -> Result<(), AppCommandError> {
    resume_connection_after_manual_inner(&rcm, &id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn hard_reset_connection(
    rcm: State<'_, RemoteConnectionManager>,
    id: String,
) -> Result<(), AppCommandError> {
    hard_reset_connection_inner(&rcm, &id).await
}

#[cfg(feature = "tauri-runtime")]
#[cfg_attr(feature = "tauri-runtime", tauri::command)]
pub async fn get_connection_runtime(
    rcm: State<'_, RemoteConnectionManager>,
    id: String,
) -> Result<Option<ConnectionRuntime>, AppCommandError> {
    Ok(get_connection_runtime_inner(&rcm, &id).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::connection::ConnectionKind;

    fn base_input() -> ConnectionInput {
        ConnectionInput {
            name: "Dev".into(),
            kind: ConnectionKind::Ssh,
            ssh_host: Some("dev.example.com".into()),
            ssh_user: Some("alice".into()),
            ssh_port: Some(22),
            ssh_alias: None,
            ssh_key_path: Some("~/.ssh/id_ed25519".into()),
            ssh_auth_method: SshAuthMethod::Key,
            proxy_jump: None,
            daemon_path: None,
            auto_connect: None,
        }
    }

    #[test]
    fn validate_ok() {
        assert!(validate_input(&base_input()).is_ok());
    }

    #[test]
    fn validate_rejects_empty_name() {
        let mut i = base_input();
        i.name = "  ".into();
        assert!(validate_input(&i).is_err());
    }

    #[test]
    fn validate_rejects_no_host_or_alias() {
        let mut i = base_input();
        i.ssh_host = None;
        i.ssh_alias = None;
        assert!(validate_input(&i).is_err());
    }

    #[test]
    fn validate_accepts_alias_only() {
        let mut i = base_input();
        i.ssh_host = None;
        i.ssh_alias = Some("dev".into());
        i.ssh_auth_method = SshAuthMethod::SshConfig;
        i.ssh_key_path = None;
        assert!(validate_input(&i).is_ok());
    }

    #[test]
    fn validate_rejects_zero_port() {
        let mut i = base_input();
        i.ssh_port = Some(0);
        assert!(validate_input(&i).is_err());
    }

    #[test]
    fn validate_rejects_key_method_without_key_path() {
        let mut i = base_input();
        i.ssh_key_path = None;
        assert!(validate_input(&i).is_err());
    }
}
