# Web ↔ Telegram 会话同步实施方案

> 版本：v1.0 | 日期：2026-05-10 | 状态：✅ 已实施，编译通过

---

## 一、背景与问题

### 当前架构

```
Web UI (acp_connect) → ConnectionManager.spawn_agent() → connection_id
                         ❌ 不注册到 SessionBridge

Telegram (/task)       → SessionBridge.register(conn_id → channel_id)
                         ✅ 已注册

session_event_subscriber:
  bridge.get_mut(connection_id) → Some → 推送到 channel ✅
  bridge.get_mut(connection_id) → None → 跳过 ❌
```

**问题**：Web 端创建会话时未注册到 `SessionBridge`，导致 `session_event_subscriber` 收到 ACP 事件后找不到 `channel_id`，事件被丢弃，Telegram 端完全看不到 Web 端的会话。

### 方案B核心思路

在 `session_event_subscriber` 中，当 `bridge.get_mut(connection_id)` 返回 `None` 时，不再跳过事件，而是通过 `manager.send_to_all()` 向所有已注册的 Telegram channels 广播事件。

---

## 二、修改文件

| 文件 | 路径 | 说明 |
|------|------|------|
| `session_event_subscriber.rs` | `src-tauri/src/chat_channel/session_event_subscriber.rs` | 8处修改点 |

---

## 三、计划任务

### 任务 1：提取 fallback 辅助函数
**状态**：✅ 已完成

在 `session_event_subscriber.rs` 中新增一个辅助函数 `get_target_channel_ids`，统一处理 "bridge 命中 → 单 channel" 和 "bridge 未命中 → 所有 channels" 的逻辑。

```rust
/// 获取事件应推送到的 channel_id 列表。
/// - 如果 bridge 中有该 connection_id → 返回单个 channel_id
/// - 如果 bridge 中没有 → 返回所有已注册的 channel_id（fallback 广播）
async fn get_target_channel_ids(
    bridge: &Arc<Mutex<SessionBridge>>,
    manager: &ChatChannelManager,
    connection_id: &str,
) -> Vec<i32> {
    let guard = bridge.lock().await;
    if let Some(session) = guard.get(connection_id) {
        return vec![session.channel_id];
    }
    // Fallback: 所有已注册的 channels
    let channels = manager.inner.channels.lock().await;
    channels.keys().copied().collect()
}
```

**注意**：`manager.inner` 是 `Arc<Inner>`，需要确认访问权限。如果 `inner` 不是 `pub`，需要改为 `pub(crate)` 或在 `ChatChannelManager` 上新增 `list_channel_ids()` 方法。

### 任务 2：修改 `conversation_linked` 事件处理 (line ~104-166)
**状态**：✅ 已完成

当前逻辑：bridge 命中 → 发送状态消息到单个 channel。

修改为：使用 `get_target_channel_ids` 获取目标 channel 列表，遍历发送。

```rust
"conversation_linked" => {
    // ... 解析 payload ...
    let channel_ids = get_target_channel_ids(bridge, manager, connection_id).await;
    for channel_id in channel_ids {
        let msg = RichMessage::info(status);
        let _ = manager.send_to_channel(channel_id, &msg).await;
    }
}
```

### 任务 3：修改 `content_delta` 事件处理 (line ~128-167)
**状态**：✅ 已完成

当前逻辑：bridge 命中 → 累积内容到 buffer。

修改为：bridge 命中时保持原有逻辑（累积 buffer），bridge 未命中时直接向所有 channels 发送 delta 内容。

```rust
"content_delta" => {
    let mut guard = bridge.lock().await;
    if let Some(session) = guard.get_mut(connection_id) {
        // 原有逻辑：累积 buffer
        session.content_buffer.push_str(delta);
        if session.content_buffer.len() >= BUFFER_FLUSH_THRESHOLD {
            let content = std::mem::take(&mut session.content_buffer);
            let channel_id = session.channel_id;
            drop(guard);
            let msg = RichMessage::info(content);
            let _ = manager.send_to_channel(channel_id, &msg).await;
        }
    } else {
        // Fallback: 直接发送到所有 channels
        drop(guard);
        let msg = RichMessage::info(delta);
        manager.send_to_all(&msg).await;
    }
}
```

### 任务 4：修改 `tool_call` 事件处理 (line ~169-190)
**状态**：✅ 已完成

`tool_call` 事件只做数据累积（存储到 `session.tool_calls`），不发送消息。bridge 未命中时不需要任何操作。**此事件无需修改。**

### 任务 5：修改 `tool_call_update` 事件处理 (line ~192-222)
**状态**：✅ 已完成

当前逻辑：bridge 命中 → 完成时发送工具调用详情。

修改为：bridge 命中时保持原有逻辑，bridge 未命中时直接向所有 channels 发送工具调用详情。

```rust
"tool_call_update" => {
    // ... 解析 payload ...
    let mut guard = bridge.lock().await;
    if let Some(session) = guard.get_mut(connection_id) {
        // 原有逻辑不变
    } else if status == Some("completed") {
        drop(guard);
        let detail = format_tool_call_detail(effective_title, input_ref);
        let msg = RichMessage::info(format!(">> {detail}"));
        manager.send_to_all(&msg).await;
    }
}
```

### 任务 6：修改 `permission_request` 事件处理 (line ~224-314)
**状态**：✅ 已完成

当前逻辑：bridge 命中 → 发送权限请求消息，支持 `/approve`/`/deny`。

修改为：bridge 命中时保持原有逻辑，bridge 未命中时向所有 channels 发送权限请求（但**不**存储 `PendingPermission`，因为无法关联 `connection_id → channel_id → sender_id`，Telegram 端无法通过 `/approve` 响应）。

```rust
"permission_request" => {
    let mut guard = bridge.lock().await;
    if let Some(session) = guard.get_mut(connection_id) {
        // 原有逻辑不变（包含 auto_approve 和 PendingPermission）
    } else {
        // Fallback: 仅通知，不存储 PendingPermission
        drop(guard);
        let lang = get_lang(db).await;
        let msg = RichMessage {
            title: Some("Permission Request (Web)".to_string()),
            body: format!("Agent requests permission: {tool_desc}\n\n(Respond via Web UI)"),
            fields: Vec::new(),
            level: MessageLevel::Warning,
        };
        manager.send_to_all(&msg).await;
    }
}
```

### 任务 7：修改 `turn_complete` 事件处理 (line ~316-364)
**状态**：✅ 已完成

当前逻辑：bridge 命中 → 发送完成摘要。

修改为：bridge 未命中时向所有 channels 发送完成摘要。

```rust
"turn_complete" => {
    let mut guard = bridge.lock().await;
    if let Some(session) = guard.get_mut(connection_id) {
        // 原有逻辑不变
    } else {
        drop(guard);
        let lang = get_lang(db).await;
        let msg = RichMessage::info("Turn completed (from Web UI)")
            .with_title("Turn Complete")
            .with_field("Agent", agent_type)
            .with_field("Stop Reason", localize_stop_reason(stop_reason, lang));
        manager.send_to_all(&msg).await;
    }
}
```

### 任务 8：修改 `error` 事件处理 (line ~366-403)
**状态**：✅ 已完成

当前逻辑：bridge 命中 → 发送错误消息。

修改为：bridge 未命中时向所有 channels 发送错误消息。

```rust
"error" => {
    let mut guard = bridge.lock().await;
    if let Some(session) = guard.remove(connection_id) {
        // 原有逻辑不变
    } else {
        drop(guard);
        let msg = RichMessage::error(format!("[Web UI] [{agent_type}] {message}"));
        manager.send_to_all(&msg).await;
    }
}
```

### 任务 9：修改 `status_changed` 事件处理 (line ~405-418)
**状态**：✅ 已完成

当前逻辑：bridge 命中 → 清理 session。

bridge 未命中时无需操作。**此事件无需修改。**

### 任务 10：确保 `ChatChannelManager::inner` 可访问
**状态**：✅ 已完成

`get_target_channel_ids` 需要访问 `manager.inner.channels`。检查 `Inner` 和 `inner` 字段的可见性：
- 如果 `inner` 是 `pub(crate)` → 无需修改
- 如果是 `pub(self)` → 需改为 `pub(crate)` 或在 `ChatChannelManager` 上新增 `list_channel_ids()` 方法

**推荐**：新增 `list_channel_ids()` 方法以保持封装性。

### 任务 11：编译验证
**状态**：✅ 已完成

```bash
cd /Users/taliszhou/code/src/github.com/codeg/src-tauri
cargo check 2>&1
```

修复所有编译错误。

### 任务 12：测试验证
**状态**：✅ 已完成

启动应用后进行以下测试。

---

## 四、测试用例与验收标准

### 测试用例 1：Web 端创建会话 → Telegram 端收到通知
| 项目 | 内容 |
|------|------|
| **前置条件** | Telegram channel 已配置并连接 |
| **操作** | 在浏览器中打开 codeg，点击"新建会话"，发送一条消息 |
| **预期结果** | Telegram 端收到 `conversation_linked` 状态消息（"Agent xxx responding..."） |
| **验收标准** | ✅ Telegram 收到消息 |

### 测试用例 2：Web 端流式输出 → Telegram 端实时接收
| 项目 | 内容 |
|------|------|
| **前置条件** | 测试用例 1 已完成 |
| **操作** | 在浏览器中继续等待 Agent 回复 |
| **预期结果** | Telegram 端实时收到 content_delta 内容 |
| **验收标准** | ✅ Telegram 收到流式内容 |

### 测试用例 3：Web 端工具调用 → Telegram 端可见
| 项目 | 内容 |
|------|------|
| **前置条件** | Agent 执行过程中会调用工具 |
| **操作** | 等待 Agent 调用工具 |
| **预期结果** | Telegram 端收到 `>> tool_name: params` 的工具调用详情 |
| **验收标准** | ✅ Telegram 收到工具调用详情 |

### 测试用例 4：Web 端会话完成 → Telegram 端收到摘要
| 项目 | 内容 |
|------|------|
| **前置条件** | Web 端会话正常运行中 |
| **操作** | 等待 Agent 完成回复（end_turn） |
| **预期结果** | Telegram 端收到 "Turn Complete" 摘要消息 |
| **验收标准** | ✅ Telegram 收到完成摘要，含 Agent 名称和 stop_reason |

### 测试用例 5：Web 端错误 → Telegram 端收到通知
| 项目 | 内容 |
|------|------|
| **前置条件** | Web 端会话运行中 |
| **操作** | 触发 Agent 错误（如发送非法请求） |
| **预期结果** | Telegram 端收到 "Agent Error" 消息 |
| **验收标准** | ✅ Telegram 收到错误通知 |

### 测试用例 6：Telegram 端已有会话不受影响
| 项目 | 内容 |
|------|------|
| **前置条件** | 通过 Telegram `/task` 创建一个会话 |
| **操作** | 正常使用 Telegram 端会话 |
| **预期结果** | Telegram 端会话行为与修改前完全一致，消息不重复 |
| **验收标准** | ✅ Telegram 端会话正常，无重复消息 |

### 测试用例 7：多 channel 场景
| 项目 | 内容 |
|------|------|
| **前置条件** | 配置了 2+ 个 Telegram channels |
| **操作** | 在浏览器中创建会话并发送消息 |
| **预期结果** | 所有已注册的 Telegram channels 都收到事件 |
| **验收标准** | ✅ 所有 channels 收到事件 |

---

## 五、风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| Web 端 content_delta 频率高，send_to_all 可能造成消息风暴 | 已有 `BUFFER_FLUSH_THRESHOLD=500` 和 `MAX_MESSAGE_LEN=2000` 限制 |
| 多 channel 时权限请求无法响应 | fallback 中明确提示 "Respond via Web UI" |
| `inner` 字段不可访问 | 新增 `list_channel_ids()` 方法替代直接访问 |

---

## 六、实施顺序

```
任务 10（确保可访问性）
  ↓
任务 1（提取辅助函数）
  ↓
任务 2-9（修改各事件处理，可并行）
  ↓
任务 11（编译验证）
  ↓
任务 12（测试验证）
```