// Probes a remote host over SSH and parses the output into a structured
// `RemotePlatform`. Used by the bootstrap pipeline to (a) pick the right
// manifest binary, (b) decide whether the remote can self-download.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

const PROBE_SCRIPT: &str = include_str!("probe.sh");
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RemotePlatform {
    pub os: String,
    pub arch: String,
    pub home_dir: String,
    pub daemon_dir: String,
    pub has_curl: bool,
    pub has_wget: bool,
    pub has_tar: bool,
    pub has_sha256_tool: bool,
    pub installed_daemon_versions: Vec<String>,
}

impl RemotePlatform {
    /// Map (os, arch) onto the manifest binary key.
    pub fn manifest_key(&self) -> Option<&'static str> {
        match (self.os.as_str(), self.arch.as_str()) {
            ("Linux", "x86_64") => Some("linux-x64-musl"),
            ("Linux", "aarch64") | ("Linux", "arm64") => Some("linux-arm64-musl"),
            ("Darwin", "x86_64") => Some("darwin-x64"),
            ("Darwin", "arm64") | ("Darwin", "aarch64") => Some("darwin-arm64"),
            (os, _) if os.starts_with("MINGW") || os.starts_with("CYGWIN") => Some("windows-x64"),
            _ => None,
        }
    }

    /// Whether the remote has the tools needed to fetch + verify + extract a
    /// release tarball on its own.
    pub fn can_download_remote(&self) -> bool {
        (self.has_curl || self.has_wget) && self.has_tar && self.has_sha256_tool
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("spawn ssh failed: {0}")]
    Spawn(String),
    #[error("ssh io error: {0}")]
    Io(String),
    #[error("remote probe failed (exit {status:?}): {stderr}")]
    RemoteFailed { status: Option<i32>, stderr: String },
    #[error("probe output missing required keys")]
    ParseError,
    #[error("probe timed out")]
    Timeout,
}

/// Run the probe script on the remote host. `ssh_args` is a fully built SSH
/// arg vector (target included) — see [`super::ssh_process::base_ssh_args`].
pub async fn probe(ssh_args: &[String]) -> Result<RemotePlatform, ProbeError> {
    let fut = run_probe(ssh_args);
    match timeout(PROBE_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => Err(ProbeError::Timeout),
    }
}

async fn run_probe(ssh_args: &[String]) -> Result<RemotePlatform, ProbeError> {
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_args).arg("sh -s");
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| ProbeError::Spawn(e.to_string()))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(PROBE_SCRIPT.as_bytes())
            .await
            .map_err(|e| ProbeError::Io(e.to_string()))?;
        // dropping stdin sends EOF
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| ProbeError::Io(e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(ProbeError::RemoteFailed {
            status: output.status.code(),
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_probe_output(&stdout).ok_or(ProbeError::ParseError)
}

fn parse_probe_output(s: &str) -> Option<RemotePlatform> {
    let mut kv: HashMap<&str, String> = HashMap::new();
    for line in s.lines() {
        if let Some((k, v)) = line.split_once('=') {
            kv.insert(k.trim(), v.trim().to_string());
        }
    }

    let os = kv.remove("OS")?;
    let arch = kv.remove("ARCH")?;

    Some(RemotePlatform {
        os,
        arch,
        home_dir: kv.remove("HOME_DIR").unwrap_or_default(),
        daemon_dir: kv.remove("DAEMON_DIR").unwrap_or_default(),
        has_curl: kv.get("HAS_CURL").is_some_and(|v| v == "yes"),
        has_wget: kv.get("HAS_WGET").is_some_and(|v| v == "yes"),
        has_tar: kv.get("HAS_TAR").is_some_and(|v| v == "yes"),
        has_sha256_tool: kv.get("HAS_SHA256").is_some_and(|v| v == "yes"),
        installed_daemon_versions: kv
            .remove("DAEMON_VERSIONS")
            .map(|v| {
                v.split(',')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_output() {
        let out = "OS=Linux\nARCH=x86_64\nHAS_CURL=yes\nHAS_WGET=no\nHAS_TAR=yes\n\
                   HAS_SHA256=yes\nHOME_DIR=/home/alice\nDAEMON_DIR=/home/alice/.codeg-remote\n\
                   DAEMON_EXISTS=yes\nDAEMON_VERSIONS=0.11.0,0.12.0\n";
        let p = parse_probe_output(out).unwrap();
        assert_eq!(p.os, "Linux");
        assert_eq!(p.arch, "x86_64");
        assert!(p.has_curl);
        assert!(!p.has_wget);
        assert!(p.has_tar);
        assert!(p.has_sha256_tool);
        assert_eq!(p.home_dir, "/home/alice");
        assert_eq!(p.installed_daemon_versions, vec!["0.11.0", "0.12.0"]);
    }

    #[test]
    fn parses_no_daemon_dir() {
        let out =
            "OS=Darwin\nARCH=arm64\nHAS_CURL=yes\nHAS_WGET=no\nHAS_TAR=yes\nHAS_SHA256=yes\n\
             HOME_DIR=/Users/bob\nDAEMON_DIR=/Users/bob/.codeg-remote\nDAEMON_EXISTS=no\n";
        let p = parse_probe_output(out).unwrap();
        assert!(p.installed_daemon_versions.is_empty());
    }

    #[test]
    fn manifest_key_linux_x64() {
        let p = mock_platform("Linux", "x86_64");
        assert_eq!(p.manifest_key(), Some("linux-x64-musl"));
    }

    #[test]
    fn manifest_key_linux_aarch64() {
        let p = mock_platform("Linux", "aarch64");
        assert_eq!(p.manifest_key(), Some("linux-arm64-musl"));
    }

    #[test]
    fn manifest_key_darwin_arm64() {
        let p = mock_platform("Darwin", "arm64");
        assert_eq!(p.manifest_key(), Some("darwin-arm64"));
    }

    #[test]
    fn manifest_key_unknown() {
        let p = mock_platform("FreeBSD", "amd64");
        assert!(p.manifest_key().is_none());
    }

    #[test]
    fn can_download_remote_requires_all_three() {
        let mut p = mock_platform("Linux", "x86_64");
        p.has_curl = true;
        p.has_tar = true;
        p.has_sha256_tool = true;
        assert!(p.can_download_remote());

        p.has_tar = false;
        assert!(!p.can_download_remote());

        p.has_tar = true;
        p.has_curl = false;
        p.has_wget = true;
        assert!(p.can_download_remote());
    }

    fn mock_platform(os: &str, arch: &str) -> RemotePlatform {
        RemotePlatform {
            os: os.into(),
            arch: arch.into(),
            home_dir: "/h".into(),
            daemon_dir: "/h/.codeg-remote".into(),
            has_curl: false,
            has_wget: false,
            has_tar: false,
            has_sha256_tool: false,
            installed_daemon_versions: vec![],
        }
    }
}
