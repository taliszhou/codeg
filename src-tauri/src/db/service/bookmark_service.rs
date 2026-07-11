// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// 内置浏览器的收藏夹 CRUD。

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, DatabaseConnection, EntityTrait, IntoActiveModel,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::bookmark;
use crate::db::error::DbError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkInfo {
    pub id: i32,
    pub title: String,
    pub url: String,
    pub sort_order: i32,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

fn to_info(m: bookmark::Model) -> BookmarkInfo {
    BookmarkInfo {
        id: m.id,
        title: m.title,
        url: m.url,
        sort_order: m.sort_order,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

pub async fn list(conn: &DatabaseConnection) -> Result<Vec<BookmarkInfo>, DbError> {
    let rows = bookmark::Entity::find()
        .order_by_asc(bookmark::Column::SortOrder)
        .order_by_asc(bookmark::Column::Title)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(to_info).collect())
}

pub async fn create(
    conn: &DatabaseConnection,
    title: &str,
    url: &str,
) -> Result<BookmarkInfo, DbError> {
    let now = Utc::now();
    let max_order = bookmark::Entity::find()
        .order_by_desc(bookmark::Column::SortOrder)
        .one(conn)
        .await?
        .map(|m| m.sort_order)
        .unwrap_or(-1);
    let active = bookmark::ActiveModel {
        id: NotSet,
        title: Set(title.trim().to_string()),
        url: Set(url.trim().to_string()),
        sort_order: Set(max_order + 1),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(to_info(active.insert(conn).await?))
}

pub async fn update(
    conn: &DatabaseConnection,
    id: i32,
    title: &str,
    url: &str,
) -> Result<BookmarkInfo, DbError> {
    let row = bookmark::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("bookmark {id} not found")))?;
    let mut active = row.into_active_model();
    active.title = Set(title.trim().to_string());
    active.url = Set(url.trim().to_string());
    active.updated_at = Set(Utc::now());
    Ok(to_info(active.update(conn).await?))
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    bookmark::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn reorder(
    conn: &DatabaseConnection,
    ids: Vec<i32>,
) -> Result<(), DbError> {
    use sea_orm::TransactionTrait;
    let now = Utc::now();
    let txn = conn.begin().await?;
    for (idx, id) in ids.into_iter().enumerate() {
        if let Some(row) = bookmark::Entity::find_by_id(id).one(&txn).await? {
            let mut active = row.into_active_model();
            active.sort_order = Set(idx as i32);
            active.updated_at = Set(now);
            active.update(&txn).await?;
        }
    }
    txn.commit().await?;
    Ok(())
}
