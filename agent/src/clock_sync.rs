//! Set the system clock after boot from a clock-safe time source.
//!
//! The device has no battery-backed RTC and gets no cellular time on the Wi-Fi relay, so
//! it boots into the past (seen: 2025) until something corrects it. A wrong clock breaks
//! TLS everywhere — most visibly Tailscale, which then reports "not signed in" and never
//! connects. OpenWrt's own NTP client is disabled on this firmware, so the agent syncs the
//! clock itself, before Tailscale and independently of it.
//!
//! The time comes from a domestic site's plain-HTTP `Date` header: there is no TLS, so a
//! wrong clock cannot block the fetch, and Clash routes `cn` traffic DIRECT, so the proxy's
//! 120 s skew check cannot block it either (the same source clash_diag::clock uses).

use crate::clash_diag::http_date;
use std::{
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CURL: &str = "/usr/bin/curl";
/// Plain-HTTP sites that answer with a live `Date` header. The first three are domestic
/// and DIRECT-routed by Clash; the last two are reachable where those are not, so the
/// clock does not depend on one region. (Not captive.apple.com: its `Date` is a cached
/// one, seen a day old.)
const SOURCES: [&str; 5] = [
    "http://www.baidu.com",
    "http://www.qq.com",
    "http://www.taobao.com",
    "http://www.gstatic.com/generate_204",
    "http://www.msftconnecttest.com/connecttest.txt",
];
/// Any epoch below this is implausible: a system clock under it is wrong and a fetched
/// time under it is rejected. 2026-06-01 UTC, comfortably before this build.
const FLOOR: i64 = 1_780_000_000;
/// Correct the clock only past this gap, so sub-second fetch latency is not a reason to set.
const SKEW_LIMIT: i64 = 30;
/// Retry quickly until the first good sync (the uplink may not be up yet at boot), then
/// re-check slowly for drift.
const FAST: Duration = Duration::from_secs(10);
const SLOW: Duration = Duration::from_secs(3600);

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A source's `Date` header as epoch seconds, fetched directly (no TLS, Clash sends it
/// DIRECT). `None` when the source is unreachable or answers without a usable date.
fn fetch(url: &str) -> Option<i64> {
    let out = Command::new(CURL)
        .args(["-sI", "--max-time", "6", url])
        .output()
        .ok()?;
    let headers = String::from_utf8_lossy(&out.stdout);
    headers
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("date"))
        })
        .and_then(|(_, v)| http_date(v.trim()))
}

/// First plausible server time across the sources, or `None` when none answered.
fn server_time() -> Option<i64> {
    SOURCES
        .iter()
        .find_map(|u| fetch(u).filter(|&t| t >= FLOOR))
}

/// Whether a fetched server time should replace the system time: plausible, and off by
/// more than the tolerated skew.
fn should_set(server: i64, system: i64) -> bool {
    server >= FLOOR && (server - system).abs() > SKEW_LIMIT
}

/// Set the system clock, leaving the timezone alone. Returns the OS error text on failure.
fn set_clock(epoch: i64) -> Result<(), String> {
    let tv = libc::timeval {
        tv_sec: epoch as _, // the field's platform type (64-bit on musl 1.2)
        tv_usec: 0,
    };
    // SAFETY: `tv` outlives the call; the timezone pointer is null so the zone is untouched.
    if unsafe { libc::settimeofday(&tv, std::ptr::null()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

/// One reconcile. Returns true once the clock is known-good, so the caller can slow down.
fn tick() -> bool {
    let Some(server) = server_time() else {
        return false; // no uplink or no source yet
    };
    if !should_set(server, now()) {
        return true; // already in sync
    }
    let skew = server - now();
    match set_clock(server) {
        Ok(()) => {
            eprintln!("[clock] set from HTTP Date (was off by {skew}s)");
            true
        }
        Err(e) => {
            eprintln!("[clock] cannot set system time: {e}");
            false
        }
    }
}

pub fn start() {
    std::thread::spawn(|| {
        let mut synced = false;
        loop {
            if tick() {
                synced = true;
            }
            std::thread::sleep(if synced { SLOW } else { FAST });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncs_only_when_plausible_and_skewed() {
        let base = 1_793_000_000; // above FLOOR
        assert!(should_set(base, base - 3600)); // an hour behind ⇒ set
        assert!(should_set(base, 1_735_949_422)); // booted into 2025 ⇒ set
        assert!(!should_set(base, base - 5)); // within tolerance ⇒ leave
        assert!(!should_set(base, base + 5)); // slightly ahead ⇒ leave
        assert!(!should_set(1_000, base)); // implausible source ⇒ never set
    }
}
