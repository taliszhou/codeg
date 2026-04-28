// Top-level scheduler for remote connections. Holds one `ConnectionTask`
// per connection id and dispatches `ControlMessage`s to it. Manifest is
// fetched lazily and cached for the desktop session.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::models::connection::ConnectionConfig;
use crate::remote::connection::{ConnectionRuntime, ConnectionTask, ControlMessage};
use crate::remote::manifest::{self, RemoteDaemonManifest};
use crate::web::event_bridge::EventEmitter;

#[derive(Clone)]
pub struct RemoteConnectionManager {
    inner: Arc<Inner>,
}

struct Inner {
    tasks: RwLock<HashMap<String, ConnectionTask>>,
    manifest: RwLock<Option<Arc<RemoteDaemonManifest>>>,
    emitter: RwLock<EventEmitter>,
}

impl Default for RemoteConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteConnectionManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                tasks: RwLock::new(HashMap::new()),
                manifest: RwLock::new(None),
                emitter: RwLock::new(EventEmitter::Noop),
            }),
        }
    }

    /// Shallow clone sharing the same state, mirroring the pattern used by
    /// `ChatChannelManager` / ACP `ConnectionManager`.
    pub fn clone_ref(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }

    /// Replace the emitter (e.g. once Tauri's AppHandle is available at
    /// setup time). Existing tasks keep the snapshot they were spawned
    /// with — new tasks pick up the latest emitter.
    pub async fn set_emitter(&self, emitter: EventEmitter) {
        *self.inner.emitter.write().await = emitter;
    }

    /// Fetch and cache the manifest. Tolerant: failures are logged and the
    /// manager continues; `connect()` will retry on demand.
    pub async fn warm_up(&self) {
        let v = manifest::REMOTE_DAEMON_VERSION;
        match manifest::get_manifest(v).await {
            Ok(m) => {
                *self.inner.manifest.write().await = Some(Arc::new(m));
            }
            Err(e) => {
                eprintln!("[Remote] manifest warm-up failed: {e}");
            }
        }
    }

    pub async fn connect(&self, config: ConnectionConfig) -> Result<(), ConnectError> {
        let manifest = self.ensure_manifest().await?;
        let mut tasks = self.inner.tasks.write().await;
        if !tasks.contains_key(&config.id) {
            let emitter = self.inner.emitter.read().await.clone();
            let task = ConnectionTask::spawn(config.clone(), emitter, manifest);
            tasks.insert(config.id.clone(), task);
        }
        let task = tasks
            .get(&config.id)
            .expect("just inserted");
        task.control_tx
            .send(ControlMessage::Connect)
            .await
            .map_err(|_| ConnectError::TaskClosed)?;
        Ok(())
    }

    pub async fn disconnect(&self, connection_id: &str) -> Result<(), ConnectError> {
        let tasks = self.inner.tasks.read().await;
        if let Some(task) = tasks.get(connection_id) {
            task.control_tx
                .send(ControlMessage::Disconnect)
                .await
                .map_err(|_| ConnectError::TaskClosed)?;
        }
        Ok(())
    }

    pub async fn hard_reset(&self, connection_id: &str) -> Result<(), ConnectError> {
        let tasks = self.inner.tasks.read().await;
        if let Some(task) = tasks.get(connection_id) {
            task.control_tx
                .send(ControlMessage::HardReset)
                .await
                .map_err(|_| ConnectError::TaskClosed)?;
        }
        Ok(())
    }

    pub async fn resume_after_manual(&self, connection_id: &str) -> Result<(), ConnectError> {
        let tasks = self.inner.tasks.read().await;
        if let Some(task) = tasks.get(connection_id) {
            task.control_tx
                .send(ControlMessage::ResumeAfterManual)
                .await
                .map_err(|_| ConnectError::TaskClosed)?;
        }
        Ok(())
    }

    pub async fn current_runtime(&self, connection_id: &str) -> Option<ConnectionRuntime> {
        let tasks = self.inner.tasks.read().await;
        let task = tasks.get(connection_id)?;
        let s = task.state.read().await;
        Some(s.snapshot(connection_id))
    }

    /// Send Disconnect to every task. Used at desktop shutdown.
    pub async fn disconnect_all(&self) {
        let tasks = self.inner.tasks.read().await;
        for task in tasks.values() {
            let _ = task.control_tx.send(ControlMessage::Disconnect).await;
        }
    }

    async fn ensure_manifest(&self) -> Result<Arc<RemoteDaemonManifest>, ConnectError> {
        if let Some(m) = self.inner.manifest.read().await.clone() {
            return Ok(m);
        }
        let v = manifest::REMOTE_DAEMON_VERSION;
        let m = manifest::get_manifest(v)
            .await
            .map_err(|e| ConnectError::Manifest(e.to_string()))?;
        let arc = Arc::new(m);
        *self.inner.manifest.write().await = Some(arc.clone());
        Ok(arc)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("task channel closed")]
    TaskClosed,
}
