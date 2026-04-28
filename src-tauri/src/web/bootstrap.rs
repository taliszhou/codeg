// Bootstrap handshake JSON emitted on stdout when codeg-server is launched
// with --bootstrap-stdio. Consumed by codeg desktop's SSH bootstrap
// orchestrator (see .docs/dev-design/2026-04-28-cg-002.4-bootstrap-protocol.md).
//
// Stability: schema_version is part of the daemon distribution contract.
// Bumping it requires a coordinated change in src-tauri/src/remote/.

use serde::{Deserialize, Serialize};

pub const BOOTSTRAP_SCHEMA_VERSION: &str = "v3";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapHandshake {
    pub schema_version: String,
    pub version: String,
    pub port: u16,
    pub token: String,
    pub started_at: String,
    pub pid: u32,
}

impl BootstrapHandshake {
    pub fn new(version: &str, port: u16, token: &str) -> Self {
        Self {
            schema_version: BOOTSTRAP_SCHEMA_VERSION.to_string(),
            version: version.to_string(),
            port,
            token: token.to_string(),
            started_at: now_rfc3339(),
            pid: std::process::id(),
        }
    }

    /// Emit one line of JSON to stdout, followed by an explicit flush.
    /// MUST be called exactly once, after the listener is bound and before
    /// any other stdout write, so the desktop side reads it atomically.
    pub fn write_to_stdout(&self) -> std::io::Result<()> {
        use std::io::Write;
        let line = serde_json::to_string(self).map_err(std::io::Error::other)?;
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        writeln!(handle, "{}", line)?;
        handle.flush()
    }
}

fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_rfc3339_utc(secs)
}

fn format_rfc3339_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86400) as i64;
    let sec_of_day = (unix_secs % 86400) as u32;
    let h = sec_of_day / 3600;
    let m = (sec_of_day / 60) % 60;
    let s = sec_of_day % 60;
    let (y, mo, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, m, s)
}

/// Convert "days since 1970-01-01" to (year, month, day).
/// Howard Hinnant's algorithm (public-domain).
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_round_trip() {
        let h = BootstrapHandshake::new("0.12.0", 41234, "tok");
        let json = serde_json::to_string(&h).unwrap();
        let decoded: BootstrapHandshake = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.schema_version, "v3");
        assert_eq!(decoded.version, "0.12.0");
        assert_eq!(decoded.port, 41234);
        assert_eq!(decoded.token, "tok");
        assert!(decoded.pid > 0);
        assert!(decoded.started_at.ends_with('Z'));
    }

    #[test]
    fn rfc3339_format_known_timestamps() {
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339_utc(1577836800), "2020-01-01T00:00:00Z");
        assert_eq!(format_rfc3339_utc(1777334400), "2026-04-28T00:00:00Z");
        assert_eq!(format_rfc3339_utc(1777386721), "2026-04-28T14:32:01Z");
        // End of leap day 2024-02-29 -> 2024-03-01
        assert_eq!(format_rfc3339_utc(1709251199), "2024-02-29T23:59:59Z");
        assert_eq!(format_rfc3339_utc(1709251200), "2024-03-01T00:00:00Z");
    }

    #[test]
    fn ymd_handles_leap_years() {
        // 2000-02-29 (leap year, divisible by 400) — days since epoch = 11016
        assert_eq!(days_to_ymd(11016), (2000, 2, 29));
        // 2100-02-28 (NOT a leap year, divisible by 100 but not 400)
        // The day after must be 2100-03-01 — verify by checking that
        // 2100-03-01 is one more day than 2100-02-28.
        let feb28 = ymd_to_days(2100, 2, 28);
        let mar01 = ymd_to_days(2100, 3, 1);
        assert_eq!(mar01 - feb28, 1, "2100 must NOT be a leap year");
        // 2024-02-29 — days since epoch = 19782
        assert_eq!(days_to_ymd(19782), (2024, 2, 29));
    }

    /// Inverse of days_to_ymd, used only in tests for round-trip checks.
    fn ymd_to_days(y: i32, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y } as i64;
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as u64;
        let m = m as u64;
        let d = d as u64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe as i64 - 719468
    }
}
