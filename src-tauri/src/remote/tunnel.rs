// Local→remote port forwarding via system ssh.
//
// We bind a local port (OS-assigned), spawn `ssh -N -L <local>:127.0.0.1:<remote>`,
// then poll the local port until something on the other end accepts. The
// returned `TunnelHandle` owns the ssh child process; dropping it tears the
// forward down.

use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::time::sleep;

pub struct TunnelHandle {
    pub local_port: u16,
    /// Owned ssh `-N -L` child. Dropping kills the forward.
    ssh_child: Child,
}

impl TunnelHandle {
    /// Best-effort kill (used at disconnect time).
    pub async fn shutdown(mut self) {
        let _ = self.ssh_child.start_kill();
        let _ = self.ssh_child.wait().await;
    }
}

/// `base_ssh_args` is the same vector returned by `ssh_process::base_ssh_args`,
/// ending with the SSH target. We splice the `-L` / `-N` options before the
/// target so they apply to this invocation.
pub async fn establish_forward(
    base_ssh_args: &[String],
    remote_port: u16,
) -> Result<TunnelHandle, TunnelError> {
    let local_port = pick_local_port().await?;

    if base_ssh_args.is_empty() {
        return Err(TunnelError::Spawn("empty ssh args".into()));
    }
    let target_idx = base_ssh_args.len() - 1;
    let mut args = base_ssh_args.to_vec();
    args.insert(target_idx, "-N".into());
    args.insert(target_idx, format!("-L{}:127.0.0.1:{}", local_port, remote_port));
    args.insert(target_idx, "ExitOnForwardFailure=yes".into());
    args.insert(target_idx, "-o".into());

    let mut cmd = Command::new("ssh");
    cmd.args(&args);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| TunnelError::Spawn(e.to_string()))?;

    // Poll local port for connectivity (up to ~3s). `ExitOnForwardFailure`
    // means a failed forward exits ssh promptly, but it doesn't tell us
    // *when* the forward is ready, so we poll.
    let mut connectable = false;
    for _ in 0..30 {
        if TcpStream::connect(("127.0.0.1", local_port)).await.is_ok() {
            connectable = true;
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    if !connectable {
        let mut child = child;
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(TunnelError::NotReady);
    }

    Ok(TunnelHandle {
        local_port,
        ssh_child: child,
    })
}

async fn pick_local_port() -> Result<u16, TunnelError> {
    // Bind ephemeral port, immediately release. ExitOnForwardFailure makes
    // the small TOCTOU window non-silent: if another process snatches it,
    // ssh exits and the caller retries connect().
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| TunnelError::Bind(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| TunnelError::Bind(e.to_string()))?
        .port();
    drop(listener);
    Ok(port)
}

#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("bind local port: {0}")]
    Bind(String),
    #[error("spawn ssh: {0}")]
    Spawn(String),
    #[error("port forward not ready in 3s")]
    NotReady,
}
