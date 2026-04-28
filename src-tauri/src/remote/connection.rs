// Per-connection state machine. One `ConnectionTask` per registered
// connection; the task owns the SSH children (master, daemon-exec, tunnel)
// and is driven by `ControlMessage`s sent through an mpsc channel.
//
// CG-002.4 M0 ships the happy path: NotAttempted → Probing → Deploying →
// Launching → Handshaking → Live, plus user-driven Disconnect. The
// reconnect supervisor / generation re-check / hard reset hooks are
// stubbed so M1 can fill them in without restructuring this module.

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};

use crate::models::connection::ConnectionConfig;
use crate::remote::bootstrap::{
    deploy, instructions_for, launch_daemon, DaemonHandshake, DeployError, DeploymentTarget,
    LaunchedDaemon, ManualDeployInstructions,
};
use crate::remote::http_client::{CapabilitiesResponse, DaemonClient};
use crate::remote::manifest::RemoteDaemonManifest;
use crate::remote::platform::probe;
use crate::remote::ssh_process::{base_ssh_args, build_ssh_target};
use crate::remote::tunnel::{establish_forward, TunnelHandle};
use crate::web::event_bridge::{emit_event, EventEmitter};

pub const STATUS_EVENT: &str = "connection://status";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectionStatus {
    NotAttempted,
    Probing,
    Deploying,
    AwaitingManual,
    Launching,
    Handshaking,
    Live,
    Reconnecting { attempt: u32 },
    Cached,
    Error,
    Disconnected,
}

impl ConnectionStatus {
    fn channel_label(&self) -> &'static str {
        match self {
            Self::NotAttempted => "not_attempted",
            Self::Probing => "probing",
            Self::Deploying => "deploying",
            Self::AwaitingManual => "awaiting_manual",
            Self::Launching => "launching",
            Self::Handshaking => "handshaking",
            Self::Live => "live",
            Self::Reconnecting { .. } => "reconnecting",
            Self::Cached => "cached",
            Self::Error => "error",
            Self::Disconnected => "disconnected",
        }
    }
}

/// Snapshot of the runtime exposed to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ConnectionRuntime {
    pub connection_id: String,
    pub status: ConnectionStatus,
    pub handshake: Option<DaemonHandshake>,
    pub capabilities: Option<CapabilitiesResponse>,
    pub local_port: Option<u16>,
    pub last_error: Option<String>,
    pub deployment_target: Option<DeploymentTarget>,
    pub manual_instructions: Option<ManualDeployInstructions>,
}

/// Live, mutable state. `ConnectionTask` keeps it behind an `RwLock` so the
/// manager can take read snapshots without blocking the task loop.
pub(super) struct RuntimeState {
    pub status: ConnectionStatus,
    pub handshake: Option<DaemonHandshake>,
    pub capabilities: Option<CapabilitiesResponse>,
    pub local_port: Option<u16>,
    pub last_error: Option<String>,
    pub last_status_change_at: Instant,
    pub deployment_target: Option<DeploymentTarget>,
    pub manual_instructions: Option<ManualDeployInstructions>,
    /// Handles owned by the running pipeline. Replaced on every connect.
    pub launched: Option<LaunchedDaemon>,
    pub tunnel: Option<TunnelHandle>,
}

impl RuntimeState {
    pub(super) fn snapshot(&self, connection_id: &str) -> ConnectionRuntime {
        ConnectionRuntime {
            connection_id: connection_id.to_string(),
            status: self.status.clone(),
            handshake: self.handshake.clone(),
            capabilities: self.capabilities.clone(),
            local_port: self.local_port,
            last_error: self.last_error.clone(),
            deployment_target: self.deployment_target.clone(),
            manual_instructions: self.manual_instructions.clone(),
        }
    }
}

#[derive(Debug)]
pub enum ControlMessage {
    Connect,
    Disconnect,
    ResumeAfterManual,
    HardReset,
}

pub struct ConnectionTask {
    pub config: ConnectionConfig,
    pub(super) state: Arc<RwLock<RuntimeState>>,
    pub control_tx: mpsc::Sender<ControlMessage>,
}

impl ConnectionTask {
    pub fn spawn(
        config: ConnectionConfig,
        emitter: EventEmitter,
        manifest: Arc<RemoteDaemonManifest>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(8);
        let state = Arc::new(RwLock::new(RuntimeState {
            status: ConnectionStatus::NotAttempted,
            handshake: None,
            capabilities: None,
            local_port: None,
            last_error: None,
            last_status_change_at: Instant::now(),
            deployment_target: None,
            manual_instructions: None,
            launched: None,
            tunnel: None,
        }));

        let cfg = config.clone();
        let st = state.clone();
        tokio::spawn(async move {
            run_loop(cfg, st, emitter, manifest, rx).await;
        });

        Self {
            config,
            state,
            control_tx: tx,
        }
    }
}

async fn run_loop(
    config: ConnectionConfig,
    state: Arc<RwLock<RuntimeState>>,
    emitter: EventEmitter,
    manifest: Arc<RemoteDaemonManifest>,
    mut rx: mpsc::Receiver<ControlMessage>,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            ControlMessage::Connect => {
                connect_pipeline(&config, &state, &emitter, &manifest).await;
            }
            ControlMessage::Disconnect => {
                disconnect(&config, &state, &emitter).await;
            }
            ControlMessage::ResumeAfterManual => {
                connect_pipeline(&config, &state, &emitter, &manifest).await;
            }
            ControlMessage::HardReset => {
                disconnect(&config, &state, &emitter).await;
                connect_pipeline(&config, &state, &emitter, &manifest).await;
            }
        }
    }
    // Channel closed → drop owned children.
    disconnect(&config, &state, &emitter).await;
}

async fn connect_pipeline(
    config: &ConnectionConfig,
    state: &Arc<RwLock<RuntimeState>>,
    emitter: &EventEmitter,
    manifest: &Arc<RemoteDaemonManifest>,
) {
    let ssh_args = base_ssh_args(config);

    set_status(state, emitter, &config.id, ConnectionStatus::Probing).await;
    let platform = match probe(&ssh_args).await {
        Ok(p) => p,
        Err(e) => {
            return set_error(state, emitter, &config.id, format!("probe: {e}")).await;
        }
    };

    set_status(state, emitter, &config.id, ConnectionStatus::Deploying).await;
    let target = match deploy(&ssh_args, &platform, manifest).await {
        Ok(t) => t,
        Err(DeployError::ManualRequired { target }) => {
            let instr = instructions_for(&target, &build_ssh_target(config));
            {
                let mut s = state.write().await;
                s.deployment_target = Some(*target);
                s.manual_instructions = Some(instr);
            }
            return set_status(state, emitter, &config.id, ConnectionStatus::AwaitingManual).await;
        }
        Err(e) => {
            return set_error(state, emitter, &config.id, format!("deploy: {e}")).await;
        }
    };
    {
        let mut s = state.write().await;
        s.deployment_target = Some(target.clone());
    }

    set_status(state, emitter, &config.id, ConnectionStatus::Launching).await;
    let launched = match launch_daemon(&ssh_args, &target).await {
        Ok(l) => l,
        Err(e) => {
            return set_error(state, emitter, &config.id, format!("launch: {e}")).await;
        }
    };
    let handshake = launched.handshake.clone();
    let remote_port = handshake.port;
    let token = handshake.token.clone();
    {
        let mut s = state.write().await;
        s.handshake = Some(handshake);
        s.launched = Some(launched);
    }

    let tunnel = match establish_forward(&ssh_args, remote_port).await {
        Ok(t) => t,
        Err(e) => {
            return set_error(state, emitter, &config.id, format!("tunnel: {e}")).await;
        }
    };
    let local_port = tunnel.local_port;
    {
        let mut s = state.write().await;
        s.local_port = Some(local_port);
        s.tunnel = Some(tunnel);
    }

    set_status(state, emitter, &config.id, ConnectionStatus::Handshaking).await;
    let client = DaemonClient::new(local_port, token);
    let caps = match client.capabilities().await {
        Ok(c) => c,
        Err(e) => {
            return set_error(state, emitter, &config.id, format!("capabilities: {e}")).await;
        }
    };

    if let Err(reason) = check_version_compat(&caps.version) {
        return set_error(state, emitter, &config.id, reason).await;
    }
    {
        let mut s = state.write().await;
        s.capabilities = Some(caps);
    }

    set_status(state, emitter, &config.id, ConnectionStatus::Live).await;
}

async fn disconnect(
    config: &ConnectionConfig,
    state: &Arc<RwLock<RuntimeState>>,
    emitter: &EventEmitter,
) {
    let (launched, tunnel) = {
        let mut s = state.write().await;
        (s.launched.take(), s.tunnel.take())
    };
    if let Some(t) = tunnel {
        t.shutdown().await;
    }
    if let Some(d) = launched {
        d.shutdown().await;
    }
    {
        let mut s = state.write().await;
        s.local_port = None;
        s.handshake = None;
        s.capabilities = None;
    }
    set_status(state, emitter, &config.id, ConnectionStatus::Disconnected).await;
}

async fn set_status(
    state: &Arc<RwLock<RuntimeState>>,
    emitter: &EventEmitter,
    id: &str,
    status: ConnectionStatus,
) {
    {
        let mut s = state.write().await;
        s.status = status.clone();
        s.last_status_change_at = Instant::now();
    }
    let payload = serde_json::json!({
        "connection_id": id,
        "status": status.channel_label(),
        "detail": status,
    });
    emit_event(emitter, STATUS_EVENT, payload);
}

async fn set_error(
    state: &Arc<RwLock<RuntimeState>>,
    emitter: &EventEmitter,
    id: &str,
    msg: String,
) {
    {
        let mut s = state.write().await;
        s.status = ConnectionStatus::Error;
        s.last_error = Some(msg.clone());
        s.last_status_change_at = Instant::now();
    }
    let payload = serde_json::json!({
        "connection_id": id,
        "status": "error",
        "error": msg,
    });
    emit_event(emitter, STATUS_EVENT, payload);
}

fn check_version_compat(daemon_version: &str) -> Result<(), String> {
    let desktop = env!("CARGO_PKG_VERSION");
    let d = parse_semver(desktop).map_err(|e| format!("desktop version invalid: {e}"))?;
    let r = parse_semver(daemon_version).map_err(|e| format!("daemon version invalid: {e}"))?;
    if d.0 != r.0 {
        return Err(format!(
            "Major version mismatch: desktop {desktop} vs daemon {daemon_version}. \
             Please align them on the same major version."
        ));
    }
    if d.1 != r.1 {
        eprintln!(
            "[Remote] minor version mismatch: desktop {desktop} vs daemon {daemon_version} (continuing)"
        );
    }
    Ok(())
}

fn parse_semver(s: &str) -> Result<(u32, u32, u32), String> {
    let trimmed = s.trim_start_matches('v');
    let parts: Vec<&str> = trimmed.splitn(3, '.').collect();
    if parts.len() != 3 {
        return Err(format!("expected MAJOR.MINOR.PATCH, got {s}"));
    }
    let major: u32 = parts[0].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let minor: u32 = parts[1].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let patch: u32 = parts[2]
        .split('-')
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    Ok((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_basic() {
        assert_eq!(parse_semver("0.12.0").unwrap(), (0, 12, 0));
        assert_eq!(parse_semver("v0.12.0").unwrap(), (0, 12, 0));
    }

    #[test]
    fn semver_with_prerelease() {
        assert_eq!(parse_semver("0.12.0-rc.1").unwrap(), (0, 12, 0));
    }

    #[test]
    fn semver_invalid() {
        assert!(parse_semver("foo").is_err());
        assert!(parse_semver("1.2").is_err());
    }

    #[test]
    fn version_compat_major_diff_rejected() {
        // desktop = env!("CARGO_PKG_VERSION") (e.g. 0.12.0); daemon = 1.0.0
        assert!(check_version_compat("1.0.0").is_err());
    }

    #[test]
    fn version_compat_minor_diff_ok() {
        let pkg = env!("CARGO_PKG_VERSION");
        let (maj, _min, _) = parse_semver(pkg).unwrap();
        let bumped = format!("{maj}.99.0");
        assert!(check_version_compat(&bumped).is_ok());
    }
}
