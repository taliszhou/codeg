use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteDeviceInfo {
    pub id: i32,
    pub name: String,
    pub base_url: String,
    /// 完整 token (前端写入/编辑用)。
    pub token: String,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteDeviceMasked {
    pub id: i32,
    pub name: String,
    pub base_url: String,
    /// token 仅显示前 4 + 后 4 字符
    pub token_masked: String,
    pub sort_order: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl RemoteDeviceInfo {
    pub fn masked(&self) -> RemoteDeviceMasked {
        let masked = mask_token(&self.token);
        RemoteDeviceMasked {
            id: self.id,
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            token_masked: masked,
            sort_order: self.sort_order,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

fn mask_token(t: &str) -> String {
    let n = t.chars().count();
    if n <= 8 {
        return "•".repeat(n);
    }
    let head: String = t.chars().take(4).collect();
    let tail: String = t.chars().skip(n - 4).collect();
    format!("{head}••••{tail}")
}
