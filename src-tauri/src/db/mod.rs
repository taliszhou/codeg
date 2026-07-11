pub mod entities;
pub mod error;
pub mod migration;
pub mod service;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_helpers;

use std::path::Path;
use std::time::Duration;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};
use sea_orm_migration::MigratorTrait;

use error::DbError;
use migration::Migrator;

pub struct AppDatabase {
    pub conn: DatabaseConnection,
}

pub(crate) fn database_file_name() -> &'static str {
    if cfg!(all(debug_assertions, feature = "tauri-runtime")) {
        "codeg-dev.db"
    } else {
        "codeg.db"
    }
}

pub async fn init_database(
    app_data_dir: impl AsRef<Path>,
    app_version: &str,
) -> Result<AppDatabase, DbError> {
    let app_data_dir = app_data_dir.as_ref();
    std::fs::create_dir_all(app_data_dir)?;

    // Apply any pending restore BEFORE opening a connection — swapping
    // `codeg.db` under a live SQLite handle would corrupt it. A failure here
    // aborts startup loudly (leaving the safety snapshot intact) rather than
    // booting a half-restored data dir.
    match crate::commands::backup::restore::apply_pending_restore_on_startup(app_data_dir) {
        Ok(crate::commands::backup::restore::RestoreApplied::Applied { .. }) => {}
        Ok(crate::commands::backup::restore::RestoreApplied::None) => {}
        Err(e) => return Err(DbError::Io(e)),
    }
    crate::commands::backup::restore::cleanup_transient_dirs(app_data_dir);

    let db_path = app_data_dir.join(database_file_name());
    let db_url = format!(
        "sqlite:{}?mode=rwc",
        urlencoding::encode(&db_path.to_string_lossy())
    );

    // Apply migrations on a dedicated single connection. The runtime pool below
    // keeps several connections open for read concurrency, but sea-orm spreads a
    // migration's statements across whichever pooled connections are free. A
    // statement that references a column an earlier migration just added (e.g.
    // the `is_chat` → `kind` backfill) can then land on a connection whose
    // cached SQLite schema predates the `ALTER TABLE`, producing a flaky
    // `no such column: "is_chat"` under load. One connection observes every DDL
    // change in order, so the schema it compiles against is always current.
    let mut migrate_opts = ConnectOptions::new(db_url.clone());
    migrate_opts
        .max_connections(1)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .sqlx_logging(false);
    let migrate_conn = Database::connect(migrate_opts).await?;
    apply_sqlite_pragmas(&migrate_conn).await?;
    Migrator::up(&migrate_conn, None)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))?;
    migrate_conn.close().await?;

    // Runtime connection pool. Migrations are already applied above, so the
    // schema is stable and spreading queries across pooled connections is safe.
    let mut opts = ConnectOptions::new(db_url);
    opts.max_connections(5)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .sqlx_logging(false);
    let conn = Database::connect(opts).await?;
    apply_sqlite_pragmas(&conn).await?;

    service::app_metadata_service::update_app_version(&conn, app_version).await?;

    // Publish user-registered ACP agents into the process-global launch
    // registry before anything can ask for agent metadata. This is the single
    // chokepoint every runtime (desktop, server) goes through, so custom agents
    // are live from the first `all_acp_agents()` / `get_agent_meta()` call.
    // A failure here must not block startup — the built-in agents still work.
    if let Err(e) = service::custom_agent_service::hydrate_registry(&conn).await {
        tracing::warn!("[custom-agent] failed to hydrate custom agent registry: {e}");
    }

    // Load user-authorized workspace links before any file command can run, so
    // the workspace path guard follows exactly the symlinks the user created
    // and nothing else. A failure here fails *closed* (registry stays empty:
    // linked subtrees look unreadable) rather than blocking startup.
    match crate::folder_links::hydrate(&conn).await {
        Ok(count) if count > 0 => {
            tracing::info!("[folder-link] hydrated {count} workspace link(s)");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("[folder-link] failed to hydrate workspace links: {e}"),
    }

    // codeg: seed 默认 NPU provider + SD卡 claude 凭证
    seed_default_providers(&conn).await?;
    seed_claude_oauth_from_sdcard(&conn).await;)

    Ok(AppDatabase { conn })
}

/// Apply SQLite performance and reliability pragmas to a freshly opened
/// connection. `journal_mode=WAL` persists in the database header; the rest are
/// per-connection settings that must be re-applied every time a connection opens.
async fn apply_sqlite_pragmas(conn: &DatabaseConnection) -> Result<(), DbError> {
    for pragma in [
        "PRAGMA journal_mode=WAL;",
        "PRAGMA busy_timeout=5000;",
        "PRAGMA synchronous=NORMAL;",
        "PRAGMA foreign_keys=ON;",
        "PRAGMA cache_size=-8000;",
    ] {
        conn.execute(Statement::from_string(DbBackend::Sqlite, pragma.to_owned()))
            .await?;
    }
    Ok(())
}

/// 当前: 注册 codeg 内置的本地 NPU LLM (Ktor bridge @ 11434),
///       让用户开箱即用 — 不必先去 settings 手动添加 provider。
async fn seed_default_providers(conn: &DatabaseConnection) -> Result<(), DbError> {
    // agent_types_json 必须跟前端 AgentType 枚举字符串对齐
    // (web/src/lib/types.ts MODEL_PROVIDER_AGENT_TYPES — generic_agent 已加白名单)
    // model 字段是发给 LLM API 的 model id,跟 codeg Ktor bridge 注册的一致。
    service::model_provider_service::ensure_by_name(
        conn,
        "Local Gemma 4 E2B",
        "http://127.0.0.1:11434/v1",
        "",
        "generic_agent",
        "gemma-4-e2b",
    )
    .await?;
    Ok(())
}

/// 启动时如果检测到 SD 卡 `/media/sd/mobilega/claude-credentials.json`(host adb push 过去的
/// macOS Keychain 导出),自动:
///   1. 拷贝到容器 `~/.claude/.credentials.json`(Claude SDK 读取标准位置)
///   2. 在 model_provider 表里 ensure 一行 "My Claude Code (OAuth)"
/// 让用户在手机上可以直接用本机 Claude Max 订阅,无需每次手动导入。
async fn seed_claude_oauth_from_sdcard(conn: &DatabaseConnection) {
    use std::fs;
    use std::path::PathBuf;

    let seed_candidates = [
        PathBuf::from("/media/sd/mobilega/claude-credentials.json"),
        PathBuf::from("/sdcard/mobilega/claude-credentials.json"),
    ];
    let seed_path = match seed_candidates.iter().find(|p| p.exists()) {
        Some(p) => p.clone(),
        None => return,
    };

    let raw = match fs::read_to_string(&seed_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[SERVER] claude seed: read {} failed: {e}", seed_path.display());
            return;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[SERVER] claude seed: parse failed: {e}");
            return;
        }
    };
    let oauth = match parsed.get("claudeAiOauth") {
        Some(v) => v,
        None => {
            eprintln!("[SERVER] claude seed: no claudeAiOauth field");
            return;
        }
    };
    let access_token = match oauth.get("accessToken").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            eprintln!("[SERVER] claude seed: no accessToken");
            return;
        }
    };

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/root"));
    let claude_dir = home.join(".claude");
    if let Err(e) = fs::create_dir_all(&claude_dir) {
        eprintln!("[SERVER] claude seed: mkdir failed: {e}");
        return;
    }
    let creds_path = claude_dir.join(".credentials.json");
    // 仅当目标不存在或内容不一致才写,避免覆盖用户手动改过的 token。
    let need_write = match fs::read_to_string(&creds_path) {
        Ok(existing) => existing.trim() != raw.trim(),
        Err(_) => true,
    };
    if need_write {
        if let Err(e) = fs::write(&creds_path, &raw) {
            eprintln!("[SERVER] claude seed: write {} failed: {e}", creds_path.display());
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&creds_path, fs::Permissions::from_mode(0o600));
        }
        eprintln!("[SERVER] claude seed: wrote {}", creds_path.display());
    }

    if let Err(e) = service::model_provider_service::ensure_by_name(
        conn,
        "My Claude Code (OAuth)",
        "https://api.anthropic.com",
        &access_token,
        "claude_code",
        "",
    )
    .await
    {
        eprintln!("[SERVER] claude seed: ensure_by_name failed: {e}");
    } else {
        eprintln!(
            "[SERVER] claude seed: provider 'My Claude Code (OAuth)' ensured from {}",
            seed_path.display()
        );
    }
}
