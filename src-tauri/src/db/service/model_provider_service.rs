use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set,
};

use crate::db::entities::model_provider;
use crate::db::error::DbError;

pub async fn create(
    conn: &DatabaseConnection,
    name: String,
    api_url: String,
    api_key: String,
    agent_type: String,
    model: Option<String>,
) -> Result<model_provider::Model, DbError> {
    let now = Utc::now();
    let agent_types_json =
        serde_json::to_string(&vec![agent_type.clone()]).unwrap_or_else(|_| "[]".to_string());
    let active = model_provider::ActiveModel {
        id: NotSet,
        name: Set(name),
        api_url: Set(api_url),
        api_key: Set(api_key),
        agent_types_json: Set(agent_types_json),
        agent_type: Set(agent_type),
        model: Set(model),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(active.insert(conn).await?)
}

pub async fn update(
    conn: &DatabaseConnection,
    id: i32,
    name: Option<String>,
    api_url: Option<String>,
    api_key: Option<String>,
    agent_type: Option<String>,
    model: Option<Option<String>>,
) -> Result<model_provider::Model, DbError> {
    let model_row = model_provider::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("model provider not found: {id}")))?;

    let mut active = model_row.into_active_model();
    if let Some(v) = name {
        active.name = Set(v);
    }
    if let Some(v) = api_url {
        active.api_url = Set(v);
    }
    if let Some(v) = api_key {
        active.api_key = Set(v);
    }
    if let Some(v) = agent_type {
        let agent_types_json =
            serde_json::to_string(&vec![v.clone()]).unwrap_or_else(|_| "[]".to_string());
        active.agent_types_json = Set(agent_types_json);
        active.agent_type = Set(v);
    }
    if let Some(v) = model {
        active.model = Set(v);
    }
    active.updated_at = Set(Utc::now());
    Ok(active.update(conn).await?)
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    model_provider::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn get_by_id(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<Option<model_provider::Model>, DbError> {
    Ok(model_provider::Entity::find_by_id(id).one(conn).await?)
}

pub async fn list_all(conn: &DatabaseConnection) -> Result<Vec<model_provider::Model>, DbError> {
    Ok(model_provider::Entity::find()
        .order_by_asc(model_provider::Column::Id)
        .all(conn)
        .await?)
}

// ===== codeg 特化: 按 name 幂等建/取 (claude_oauth 导入用) =====
pub async fn find_by_name(
    conn: &DatabaseConnection,
    name: &str,
) -> Result<Option<model_provider::Model>, DbError> {
    Ok(model_provider::Entity::find()
        .filter(model_provider::Column::Name.eq(name))
        .one(conn)
        .await?)
}

pub async fn ensure_by_name(
    conn: &DatabaseConnection,
    name: &str,
    api_url: &str,
    api_key: &str,
    agent_type: &str,
    model: &str,
) -> Result<model_provider::Model, DbError> {
    if let Some(existing) = find_by_name(conn, name).await? {
        // 老 row 缺 model 字段时填回
        if existing.model.as_deref().unwrap_or("").is_empty() && !model.is_empty() {
            return update(
                conn,
                existing.id,
                None,
                None,
                None,
                None,
                Some(Some(model.to_string())),
            )
            .await;
        }
        return Ok(existing);
    }
    create(
        conn,
        name.to_string(),
        api_url.to_string(),
        api_key.to_string(),
        agent_type.to_string(),
        if model.is_empty() {
            None
        } else {
            Some(model.to_string())
        },
    )
    .await
}
