//! The agent's own log. The firmware's syslogd keeps only `local0`, so the agent's
//! output used to be lost: `zte-agent log-writer` reads it on stdin and keeps it in two
//! rotating files (about 1 MiB together). The relay band guard writes its own file the
//! same way. `get` serves the newest lines of both to the web page.
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    time::{SystemTime, UNIX_EPOCH},
};

const DIR: &str = "/data/local/tmp/openui-logs";
pub const AGENT: &str = "/data/local/tmp/openui-logs/agent.log";
const RELAY_GUARD: &str = "/data/local/tmp/openui-logs/relay-guard.log";
/// Each file; the previous one is kept as `.1`.
const ROTATE_AT: u64 = 512 * 1024;
const MAX_LINE: usize = 1024;
const TAIL: usize = 400;

/// UTC `YYYY-MM-DD HH:MM:SS` (civil date from days, Howard Hinnant's algorithm).
fn stamp(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

fn open(path: &str) -> Option<fs::File> {
    let _ = fs::create_dir_all(DIR);
    let _ = fs::set_permissions(DIR, fs::Permissions::from_mode(0o700));
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .ok()
}

/// One log line: control characters dropped, length capped.
fn entry(now: u64, raw: &[u8]) -> String {
    let text: String = String::from_utf8_lossy(raw)
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .take(MAX_LINE)
        .collect();
    format!("{} {}\n", stamp(now), text.trim_end())
}

/// Until stdin closes (the agent exited). Always keeps reading, even when the file
/// cannot be written, so the agent never blocks on a full pipe.
pub fn run_writer(path: &str) {
    let mut file = open(path);
    let mut size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    for line in std::io::stdin().lock().split(b'\n') {
        let Ok(raw) = line else { break };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let text = entry(now, &raw);
        if size + text.len() as u64 > ROTATE_AT {
            drop(file.take());
            let _ = fs::rename(path, format!("{path}.1"));
            file = open(path);
            size = 0;
        }
        if let Some(f) = file.as_mut() {
            if f.write_all(text.as_bytes()).is_ok() {
                size += text.len() as u64;
            }
        }
    }
}

/// The newest `n` lines of `path` and its rotated predecessor, oldest first.
fn tail(path: &str, n: usize) -> Vec<String> {
    let mut text = String::new();
    for p in [format!("{path}.1"), path.to_string()] {
        if let Ok(f) = fs::File::open(&p) {
            let mut bytes = Vec::new();
            let _ = f.take(2 * ROTATE_AT).read_to_end(&mut bytes);
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// `GET /api/system/logs`.
pub fn get() -> (u16, Value) {
    (
        200,
        json!({"ok": true, "data": {
            "agent": tail(AGENT, TAIL),
            "relay_guard": tail(RELAY_GUARD, TAIL),
        }}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stamps_and_lines_are_clean() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00");
        assert_eq!(stamp(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(stamp(1_793_220_055), "2026-10-28 20:40:55");
        assert_eq!(
            entry(0, b"[relay] online\r"),
            "1970-01-01 00:00:00 [relay] online\n"
        );
        assert_eq!(entry(0, b"a\x1b[31mb\x07"), "1970-01-01 00:00:00 a[31mb\n");
        assert_eq!(entry(0, &[b'x'; 5000]).len(), 20 + MAX_LINE + 1);
    }
    #[test]
    fn rotation_keeps_the_newest_lines() {
        let dir = std::env::temp_dir().join(format!("agent-log-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.log");
        let p = path.to_str().unwrap();
        fs::write(format!("{p}.1"), "old 1\nold 2\n").unwrap();
        fs::write(p, "new 1\nnew 2\nnew 3\n").unwrap();
        assert_eq!(tail(p, 4), ["old 2", "new 1", "new 2", "new 3"]);
        assert_eq!(tail(p, 100).len(), 5);
        assert!(tail(dir.join("missing.log").to_str().unwrap(), 10).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
