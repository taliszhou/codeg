use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use crate::db::entities::connection;
use crate::db::error::DbError;
use crate::models::connection::{
    ConnectionConfig, ConnectionInput, ConnectionKind, SshAuthMethod,
};

fn to_config(m: connection::Model) -> ConnectionConfig {
    ConnectionConfig {
        id: m.id,
        name: m.name,
        kind: ConnectionKind::from_db(&m.kind),
        ssh_host: m.ssh_host,
        ssh_user: m.ssh_user,
        ssh_port: m.ssh_port.map(|p| p as u16),
        ssh_alias: m.ssh_alias,
        ssh_key_path: m.ssh_key_path,
        ssh_auth_method: SshAuthMethod::from_db(&m.ssh_auth_method),
        proxy_jump: m.proxy_jump,
        daemon_path: m.daemon_path,
        daemon_version: m.daemon_version,
        auto_connect: m.auto_connect,
        last_connected_at: m.last_connected_at,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

pub async fn list(conn: &DatabaseConnection) -> Result<Vec<ConnectionConfig>, DbError> {
    let rows = connection::Entity::find()
        .order_by_asc(connection::Column::CreatedAt)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(to_config).collect())
}

pub async fn get_by_id(
    conn: &DatabaseConnection,
    id: &str,
) -> Result<Option<ConnectionConfig>, DbError> {
    Ok(connection::Entity::find_by_id(id)
        .one(conn)
        .await?
        .map(to_config))
}

pub async fn create(
    conn: &DatabaseConnection,
    input: ConnectionInput,
) -> Result<ConnectionConfig, DbError> {
    let id = format!("conn_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    let active = connection::ActiveModel {
        id: Set(id),
        name: Set(input.name),
        kind: Set(input.kind.as_str().to_string()),
        ssh_host: Set(input.ssh_host),
        ssh_user: Set(input.ssh_user),
        ssh_port: Set(input.ssh_port.map(|p| p as i32)),
        ssh_alias: Set(input.ssh_alias),
        ssh_key_path: Set(input.ssh_key_path),
        ssh_auth_method: Set(input.ssh_auth_method.as_str().to_string()),
        proxy_jump: Set(input.proxy_jump),
        daemon_path: Set(input
            .daemon_path
            .unwrap_or_else(|| "~/.codeg-remote".to_string())),
        daemon_version: Set(None),
        auto_connect: Set(input.auto_connect.unwrap_or(false)),
        last_connected_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let model = active.insert(conn).await?;
    Ok(to_config(model))
}

pub async fn update(
    conn: &DatabaseConnection,
    id: &str,
    input: ConnectionInput,
) -> Result<ConnectionConfig, DbError> {
    let existing = connection::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Database(sea_orm::DbErr::RecordNotFound(id.to_string())))?;
    let mut active: connection::ActiveModel = existing.into();
    active.name = Set(input.name);
    active.kind = Set(input.kind.as_str().to_string());
    active.ssh_host = Set(input.ssh_host);
    active.ssh_user = Set(input.ssh_user);
    active.ssh_port = Set(input.ssh_port.map(|p| p as i32));
    active.ssh_alias = Set(input.ssh_alias);
    active.ssh_key_path = Set(input.ssh_key_path);
    active.ssh_auth_method = Set(input.ssh_auth_method.as_str().to_string());
    active.proxy_jump = Set(input.proxy_jump);
    if let Some(p) = input.daemon_path {
        active.daemon_path = Set(p);
    }
    if let Some(a) = input.auto_connect {
        active.auto_connect = Set(a);
    }
    active.updated_at = Set(Utc::now());
    let model = active.update(conn).await?;
    Ok(to_config(model))
}

pub async fn delete(conn: &DatabaseConnection, id: &str) -> Result<(), DbError> {
    connection::Entity::delete_many()
        .filter(connection::Column::Id.eq(id))
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn touch_last_connected(
    conn: &DatabaseConnection,
    id: &str,
    daemon_version: Option<String>,
) -> Result<(), DbError> {
    let existing = connection::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Database(sea_orm::DbErr::RecordNotFound(id.to_string())))?;
    let mut active: connection::ActiveModel = existing.into();
    let now = Utc::now();
    active.last_connected_at = Set(Some(now));
    active.updated_at = Set(now);
    if let Some(v) = daemon_version {
        active.daemon_version = Set(Some(v));
    }
    active.update(conn).await?;
    Ok(())
}
