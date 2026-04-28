// Two-phase bootstrap:
//   1. `deploy()` ensures the daemon binary is on the remote host, trying
//      method A (remote curl) → method B (desktop push) → returning
//      `ManualRequired` so the UI can guide the user (method C).
//   2. `launch_daemon()` spawns the binary via `ssh exec`, parses the
//      bootstrap handshake from stdout, and returns the live child.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

use crate::remote::manifest::{RemoteDaemonBinary, RemoteDaemonManifest};
use crate::remote::platform::RemotePlatform;
use crate::remote::ssh_process::posix_single_quote;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DeploymentTarget {
    pub remote_daemon_path: String,
    pub binary: RemoteDaemonBinary,
    pub platform_key: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManualDeployInstructions {
    pub binary_url: String,
    pub binary_sha256: String,
    pub remote_target_path: String,
    pub copy_paste_command: String,
}

pub fn instructions_for(target: &DeploymentTarget, ssh_target: &str) -> ManualDeployInstructions {
    let parent = parent_dir(&target.remote_daemon_path);
    let cmd = format!(
        "ssh {ssh} 'mkdir -p {dir}' && \\\n\
         curl -fL {url} | ssh {ssh} 'tar xz -C {dir} && chmod +x {path}'",
        ssh = ssh_target,
        url = target.binary.url,
        dir = parent,
        path = target.remote_daemon_path,
    );
    ManualDeployInstructions {
        binary_url: target.binary.url.clone(),
        binary_sha256: target.binary.sha256.clone(),
        remote_target_path: target.remote_daemon_path.clone(),
        copy_paste_command: cmd,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error("unsupported platform: {os}/{arch}")]
    UnsupportedPlatform { os: String, arch: String },
    #[error("manifest missing platform: {0}")]
    MissingPlatformInManifest(String),
    #[error("io: {0}")]
    Io(String),
    #[error("method A (remote curl) failed: {0}")]
    RemoteCurl(String),
    #[error("method B (desktop push) failed: {0}")]
    DesktopPush(String),
    #[error("manual deployment required")]
    ManualRequired { target: Box<DeploymentTarget> },
}

pub async fn deploy(
    ssh_args: &[String],
    platform: &RemotePlatform,
    manifest: &RemoteDaemonManifest,
) -> Result<DeploymentTarget, DeployError> {
    let key = platform
        .manifest_key()
        .ok_or_else(|| DeployError::UnsupportedPlatform {
            os: platform.os.clone(),
            arch: platform.arch.clone(),
        })?;
    let bin = manifest
        .binaries
        .get(key)
        .ok_or_else(|| DeployError::MissingPlatformInManifest(key.to_string()))?
        .clone();
    let version = manifest.version.clone();
    let daemon_dir = if platform.daemon_dir.is_empty() {
        format!("{}/.codeg-remote", platform.home_dir.trim_end_matches('/'))
    } else {
        platform.daemon_dir.clone()
    };
    let remote_path = format!("{}/{}/{}", daemon_dir, version, bin.exec_name);
    let target = DeploymentTarget {
        remote_daemon_path: remote_path.clone(),
        binary: bin.clone(),
        platform_key: key.to_string(),
        version: version.clone(),
    };

    if platform.installed_daemon_versions.iter().any(|v| v == &version)
        && verify_remote_sha256(ssh_args, &remote_path, &bin.sha256)
            .await
            .unwrap_or(false)
    {
        return Ok(target);
    }

    if platform.can_download_remote() && !bin.url.is_empty() {
        match deploy_method_remote_curl(ssh_args, &target).await {
            Ok(()) => return Ok(target),
            Err(e) => eprintln!("[Remote] method A (remote curl) failed: {e}"),
        }
    }

    match deploy_method_desktop_push(ssh_args, &target).await {
        Ok(()) => return Ok(target),
        Err(e) => eprintln!("[Remote] method B (desktop push) failed: {e}"),
    }

    Err(DeployError::ManualRequired {
        target: Box::new(target),
    })
}

async fn deploy_method_remote_curl(
    ssh_args: &[String],
    t: &DeploymentTarget,
) -> Result<(), DeployError> {
    let url_q = posix_single_quote(&t.binary.url);
    let path_q = posix_single_quote(&t.remote_daemon_path);
    let exec_q = posix_single_quote(&t.binary.exec_name);
    let sha = t.binary.sha256.clone();

    let sha_check = if sha.is_empty() {
        String::from("# sha256 unknown (fallback manifest); skipping check")
    } else {
        format!(
            "EXPECTED={sha_q}\n\
if command -v sha256sum >/dev/null 2>&1; then\n  ACTUAL=$(sha256sum \"$ARCHIVE\" | awk '{{print $1}}')\n\
elif command -v shasum >/dev/null 2>&1; then\n  ACTUAL=$(shasum -a 256 \"$ARCHIVE\" | awk '{{print $1}}')\n\
else\n  echo no-sha256-tool >&2; exit 12\nfi\n\
if [ \"$EXPECTED\" != \"$ACTUAL\" ]; then\n  echo \"sha256 mismatch: expected $EXPECTED actual $ACTUAL\" >&2; exit 13\nfi",
            sha_q = posix_single_quote(&sha),
        )
    };

    let cmd = format!(
        "set -e\n\
DEST={path}\n\
mkdir -p \"$(dirname \"$DEST\")\"\n\
TMP=$(mktemp -d)\n\
trap 'rm -rf \"$TMP\"' EXIT\n\
ARCHIVE=\"$TMP/dl\"\n\
if command -v curl >/dev/null 2>&1; then\n  curl -fsSL {url} -o \"$ARCHIVE\"\n\
elif command -v wget >/dev/null 2>&1; then\n  wget -q {url} -O \"$ARCHIVE\"\n\
else\n  echo no-fetcher >&2; exit 11\nfi\n\
{sha_check}\n\
mkdir -p \"$TMP/extract\"\n\
tar xzf \"$ARCHIVE\" -C \"$TMP/extract\"\n\
BIN=$(find \"$TMP/extract\" -name {exec} -type f | head -n1)\n\
if [ -z \"$BIN\" ]; then echo binary-not-found >&2; exit 14; fi\n\
chmod +x \"$BIN\"\n\
mv \"$BIN\" \"$DEST\"\n\
echo deployed",
        path = path_q,
        url = url_q,
        sha_check = sha_check,
        exec = exec_q,
    );

    let mut c = Command::new("ssh");
    c.args(ssh_args).arg(cmd);
    c.stdin(std::process::Stdio::null());
    let out = c
        .output()
        .await
        .map_err(|e| DeployError::Io(e.to_string()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(DeployError::RemoteCurl(stderr));
    }
    Ok(())
}

async fn deploy_method_desktop_push(
    ssh_args: &[String],
    t: &DeploymentTarget,
) -> Result<(), DeployError> {
    let local_archive = ensure_local_archive(&t.binary, &t.platform_key, &t.version).await?;

    let parent = parent_dir(&t.remote_daemon_path);
    let dir_q = posix_single_quote(&parent);
    let exec_q = posix_single_quote(&t.binary.exec_name);
    let path_q = posix_single_quote(&t.remote_daemon_path);
    let cmd = format!(
        "set -e\nmkdir -p {dir}\ncd {dir}\ntar xz\nchmod +x {exec}\n# ensure final filename matches\nif [ ! -f {path} ]; then\n  BIN=$(find . -name {exec} -type f | head -n1)\n  if [ -n \"$BIN\" ] && [ \"$BIN\" != \"./$(basename {path})\" ]; then mv \"$BIN\" {path}; fi\nfi",
        dir = dir_q,
        exec = exec_q,
        path = path_q,
    );

    let mut c = Command::new("ssh");
    c.args(ssh_args).arg(cmd);
    c.stdin(std::process::Stdio::piped());
    c.stderr(std::process::Stdio::piped());
    let mut child = c
        .spawn()
        .map_err(|e| DeployError::Io(e.to_string()))?;

    if let Some(mut stdin) = child.stdin.take() {
        let bytes = tokio::fs::read(&local_archive)
            .await
            .map_err(|e| DeployError::Io(format!("read local archive: {e}")))?;
        stdin
            .write_all(&bytes)
            .await
            .map_err(|e| DeployError::Io(e.to_string()))?;
    }

    let out = child
        .wait_with_output()
        .await
        .map_err(|e| DeployError::Io(e.to_string()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(DeployError::DesktopPush(stderr));
    }

    if !t.binary.sha256.is_empty() {
        let ok = verify_remote_sha256(ssh_args, &t.remote_daemon_path, &t.binary.sha256)
            .await
            .map_err(|e| DeployError::DesktopPush(format!("sha256 verify: {e}")))?;
        if !ok {
            return Err(DeployError::DesktopPush(
                "sha256 mismatch after push".into(),
            ));
        }
    }
    Ok(())
}

async fn ensure_local_archive(
    bin: &RemoteDaemonBinary,
    platform_key: &str,
    version: &str,
) -> Result<PathBuf, DeployError> {
    let cache_dir = local_cache_dir().ok_or_else(|| DeployError::Io("no cache dir".into()))?;
    tokio::fs::create_dir_all(&cache_dir).await.ok();
    let ext = if platform_key.starts_with("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    let path = cache_dir.join(format!(
        "codeg-remote-{}-{}.{}",
        platform_key, version, ext
    ));

    if path.exists() {
        if !bin.sha256.is_empty() {
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|e| DeployError::Io(e.to_string()))?;
            if sha256_hex(&bytes) == bin.sha256 {
                return Ok(path);
            }
            eprintln!("[Remote] local cache sha256 mismatch, re-downloading");
        } else {
            return Ok(path);
        }
    }

    if bin.url.is_empty() {
        return Err(DeployError::DesktopPush(
            "no URL in manifest and no local cache".into(),
        ));
    }

    let resp = reqwest::Client::new()
        .get(&bin.url)
        .send()
        .await
        .map_err(|e| DeployError::Io(format!("download: {e}")))?;
    if !resp.status().is_success() {
        return Err(DeployError::Io(format!(
            "download status: {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| DeployError::Io(e.to_string()))?;
    if !bin.sha256.is_empty() {
        let actual = sha256_hex(&bytes);
        if actual != bin.sha256 {
            return Err(DeployError::Io(format!(
                "downloaded sha256 mismatch: {} vs {}",
                actual, bin.sha256
            )));
        }
    }
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| DeployError::Io(e.to_string()))?;
    Ok(path)
}

fn local_cache_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codeg").join("remote-binaries"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        use std::fmt::Write;
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}

async fn verify_remote_sha256(
    ssh_args: &[String],
    remote_path: &str,
    expected: &str,
) -> Result<bool, DeployError> {
    if expected.is_empty() {
        // Cannot verify without a known sha; treat as "trust on first use".
        return Ok(true);
    }
    let path_q = posix_single_quote(remote_path);
    let cmd = format!(
        "if [ ! -f {p} ]; then exit 1; fi\n\
if command -v sha256sum >/dev/null 2>&1; then\n  sha256sum {p} | awk '{{print $1}}'\n\
elif command -v shasum >/dev/null 2>&1; then\n  shasum -a 256 {p} | awk '{{print $1}}'\n\
else\n  exit 12\nfi",
        p = path_q,
    );
    let mut c = Command::new("ssh");
    c.args(ssh_args).arg(cmd);
    c.stdin(std::process::Stdio::null());
    let out = c
        .output()
        .await
        .map_err(|e| DeployError::Io(e.to_string()))?;
    if !out.status.success() {
        return Ok(false);
    }
    let actual = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(actual.eq_ignore_ascii_case(expected))
}

fn parent_dir(p: &str) -> String {
    Path::new(p)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ── launch_daemon ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DaemonHandshake {
    pub version: String,
    pub schema_version: String,
    pub port: u16,
    pub token: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub pid: Option<u32>,
}

pub struct LaunchedDaemon {
    pub handshake: DaemonHandshake,
    /// `ssh exec` child. Drop or `shutdown()` to end the remote daemon.
    ssh_child: Child,
}

impl LaunchedDaemon {
    pub async fn shutdown(mut self) {
        let _ = self.ssh_child.start_kill();
        let _ = self.ssh_child.wait().await;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("spawn ssh failed: {0}")]
    Spawn(String),
    #[error("daemon stdout not piped")]
    NoStdout,
    #[error("daemon exited before handshake")]
    DaemonExitedEarly,
    #[error("read stdout: {0}")]
    ReadStdout(String),
    #[error("handshake timeout (20s)")]
    HandshakeTimeout,
    #[error("parse handshake JSON: {0}")]
    ParseHandshake(String),
}

pub async fn launch_daemon(
    ssh_args: &[String],
    target: &DeploymentTarget,
) -> Result<LaunchedDaemon, LaunchError> {
    let bin_q = posix_single_quote(&target.remote_daemon_path);
    // Use --listen 127.0.0.1:0 to let the OS pick a port; --bootstrap-stdio
    // makes daemon emit a handshake JSON line on stdout before serving.
    let cmd = format!(
        "exec {bin} --listen 127.0.0.1:0 --bootstrap-stdio",
        bin = bin_q
    );

    let mut c = Command::new("ssh");
    c.args(ssh_args).arg(cmd);
    c.stdout(std::process::Stdio::piped());
    c.stderr(std::process::Stdio::piped());
    c.stdin(std::process::Stdio::piped()); // keep stdin open: daemon watches EOF
    let mut child = c
        .spawn()
        .map_err(|e| LaunchError::Spawn(e.to_string()))?;

    // Hold stdin open so the daemon's EOF watchdog doesn't trigger.
    // We deliberately leak the stdin handle for the lifetime of the child.
    let _stdin = child.stdin.take();

    let stdout = child.stdout.take().ok_or(LaunchError::NoStdout)?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    match timeout(Duration::from_secs(20), reader.read_line(&mut line)).await {
        Ok(Ok(0)) => return Err(LaunchError::DaemonExitedEarly),
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(LaunchError::ReadStdout(e.to_string())),
        Err(_) => return Err(LaunchError::HandshakeTimeout),
    }

    let handshake: DaemonHandshake = serde_json::from_str(line.trim())
        .map_err(|e| LaunchError::ParseHandshake(e.to_string()))?;

    // Drain stdout / stderr in background so the daemon never blocks on
    // a full pipe. We discard the bytes for now; CG-002.6 wires this into
    // a daemon-log panel.
    spawn_drain(reader, "stdout", target.remote_daemon_path.clone());
    if let Some(stderr) = child.stderr.take() {
        let r = BufReader::new(stderr);
        spawn_drain(r, "stderr", target.remote_daemon_path.clone());
    }

    Ok(LaunchedDaemon {
        handshake,
        ssh_child: child,
    })
}

fn spawn_drain<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    mut reader: BufReader<R>,
    label: &'static str,
    bin_path: String,
) {
    tokio::spawn(async move {
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = buf.trim_end();
                    if !trimmed.is_empty() {
                        eprintln!("[Remote daemon {label} {bin_path}] {trimmed}");
                    }
                }
                Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dir_basic() {
        assert_eq!(parent_dir("/a/b/c"), "/a/b");
        assert_eq!(parent_dir("file"), "");
    }

    #[test]
    fn sha256_hex_correctness() {
        // sha256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn parses_handshake_json() {
        let j = r#"{"schema_version":"v3","version":"0.12.0","port":41234,"token":"abc","started_at":"2026-04-28T14:32:01Z","pid":1234}"#;
        let h: DaemonHandshake = serde_json::from_str(j).unwrap();
        assert_eq!(h.version, "0.12.0");
        assert_eq!(h.port, 41234);
        assert_eq!(h.token, "abc");
        assert_eq!(h.pid, Some(1234));
    }

    #[test]
    fn parses_handshake_without_pid() {
        let j = r#"{"schema_version":"v3","version":"0.12.0","port":1,"token":"t"}"#;
        let h: DaemonHandshake = serde_json::from_str(j).unwrap();
        assert!(h.pid.is_none());
        assert_eq!(h.started_at, "");
    }
}
