//! Firmware update lock. ZTE FOTA (`zte_dm`) asks its DM servers for an update,
//! downloads the delta to /cache and `zte_topsw_dua` flashes it. While locked:
//! the DM servers resolve to the broadcast address (tagged /etc/hosts lines, which keep working
//! across reboots even before the agent starts), automatic updates stay off, and
//! a package that still arrives (TR-069, an unlisted server) is cancelled and
//! removed. An install that has already started is never touched: interrupting a
//! flash is worse than letting it finish.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::{Mutex, Once};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::MutexExt;
use crate::{storage, ubus};

const SETTINGS: &str = "/data/local/tmp/openui-firmware-lock.json";
const HOSTS: &str = "/etc/hosts";
const TAG: &str = "# openui-firmware-lock";
/// TCP connect to broadcast fails at once (ENETUNREACH). 0.0.0.0 would reach the
/// device's own port 443, where zte_dm leaves sockets open after the TLS failure.
const NOWHERE: &[&str] = &["255.255.255.255", "::ffff:255.255.255.255"];
/// Every DM server named by zte_dm and its DM trees on B31.
const SERVERS: &[&str] = &[
    "dmcn.ztems.com",
    "dmcan.ztems.com",
    "dmcan6.ztems.com",
    "dmeu.ztems.com",
    "dmeu6.ztems.com",
    "dms.ztems.com",
    "dms6.ztems.com",
];
/// Where zte_dm downloads the update and zte_topsw_dua reads it.
const PACKAGES: &[&str] = &[
    "/cache/delta.package",
    "/cache/delta.package1",
    "/cache/delta.bin",
    "/cache/delta.sign",
];
const INSTALLING: &str = "/tmp/fota_install_processing";
const MAX_REVISION: u64 = 9_007_199_254_740_991;
/// Package checks run every second; settings, hosts and FOTA state every 15.
const RECHECK_TICKS: u64 = 15;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    revision: u64,
    enabled: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub revision: u64,
    pub enabled: bool,
}

struct State {
    settings: Settings,
    servers_blocked: bool,
    auto_update_off: Option<bool>,
    upgrade_state: Option<String>,
    installing: bool,
    blocked: u64,
    last_blocked: Option<u64>,
    error: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    settings: Settings {
        revision: 0,
        enabled: false,
    },
    servers_blocked: false,
    auto_update_off: None,
    upgrade_state: None,
    installing: false,
    blocked: 0,
    last_blocked: None,
    error: None,
});
/// Serializes the guard's actions with switch changes; STATE is only held briefly.
static WORK: Mutex<()> = Mutex::new(());
static START: Once = Once::new();

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Missing means unlocked. A damaged file locks: this switch only ever protects.
fn load() -> (Settings, Option<String>) {
    match fs::read(SETTINGS) {
        Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
            Ok(s) if s.revision <= MAX_REVISION => (s, None),
            _ => (
                Settings {
                    revision: 0,
                    enabled: true,
                },
                Some("Saved firmware lock settings were invalid; the lock stays on".into()),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(e) => (
            Settings {
                revision: 0,
                enabled: true,
            },
            Some(format!("Reading firmware lock settings: {e}")),
        ),
    }
}

fn next(current: Settings, update: Update) -> Result<Settings, &'static str> {
    if update.revision != current.revision {
        return Err("Firmware lock changed; reload status");
    }
    let revision = current
        .revision
        .checked_add(1)
        .filter(|r| *r <= MAX_REVISION)
        .ok_or("Firmware lock revision exhausted")?;
    Ok(Settings {
        revision,
        enabled: update.enabled,
    })
}

/// The hosts file without our lines, plus a null address per server when locked.
fn hosts_with(text: &str, enabled: bool) -> String {
    let mut out: String = text
        .lines()
        .filter(|l| !l.trim_end().ends_with(TAG))
        .flat_map(|l| [l, "\n"])
        .collect();
    if enabled {
        for server in SERVERS {
            for addr in NOWHERE {
                out.push_str(&format!("{addr} {server} {TAG}\n"));
            }
        }
    }
    out
}

fn hosts_blocked(text: &str) -> bool {
    SERVERS.iter().all(|server| {
        NOWHERE.iter().all(|addr| {
            text.lines()
                .any(|l| l.trim() == format!("{addr} {server} {TAG}"))
        })
    })
}

/// Same-directory replace that keeps /etc/hosts readable to every user throughout.
/// The agent runs with umask 077, so the original mode is set explicitly: dnsmasq
/// and the stock web server run as `nobody` and must still read it.
fn write_hosts(text: &str) -> std::io::Result<()> {
    let mode = fs::metadata(HOSTS).map_or(0o644, |m| m.permissions().mode() & 0o777) | 0o444;
    let temp = format!("{HOSTS}.openui-{}.tmp", std::process::id());
    let _ = fs::remove_file(&temp);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, HOSTS)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn sync_hosts(enabled: bool) -> Result<bool, String> {
    let text = fs::read_to_string(HOSTS).map_err(|e| format!("Reading {HOSTS}: {e}"))?;
    let wanted = hosts_with(&text, enabled);
    if wanted != text {
        write_hosts(&wanted).map_err(|e| format!("Writing {HOSTS}: {e}"))?;
    }
    Ok(hosts_blocked(&wanted))
}

fn upgrade_state() -> Option<String> {
    ubus::call("zwrt_zte_dm", "get_update_info", None)
        .ok()
        .and_then(|v| v["current_upgrade_state"].as_str().map(str::to_owned))
}

fn installing(state: Option<&str>) -> bool {
    state == Some("fota_upgrade") || Path::new(INSTALLING).exists()
}

/// Settings, hosts lines, automatic-update mode and the FOTA state.
fn recheck(enabled: bool) -> Result<(), String> {
    let blocked = sync_hosts(enabled);
    let mut auto_off = ubus::uci_get("zwrt_zte_dm.dm_update.dm_update_mode")
        .ok()
        .map(|m| m == "0");
    let mut mode_error = None;
    if enabled && auto_off == Some(false) {
        mode_error = ubus::call(
            "zwrt_zte_dm",
            "set_update_mode",
            Some(r#"{"dm_update_mode":"0"}"#),
        )
        .err();
        auto_off = ubus::uci_get("zwrt_zte_dm.dm_update.dm_update_mode")
            .ok()
            .map(|m| m == "0");
    }
    let upgrade = upgrade_state();
    let mut state = STATE.safe_lock();
    state.servers_blocked = *blocked.as_ref().unwrap_or(&false);
    state.auto_update_off = auto_off;
    state.installing = installing(upgrade.as_deref());
    state.upgrade_state = upgrade;
    let error = blocked.err().or(mode_error).or_else(|| {
        (enabled && auto_off != Some(true))
            .then(|| "Automatic updates could not be turned off".into())
    });
    state.error = error.clone();
    error.map_or(Ok(()), Err)
}

/// A package is on disk: cancel the download and remove it unless an install runs.
fn intercept() {
    let upgrade = upgrade_state();
    if installing(upgrade.as_deref()) {
        STATE.safe_lock().installing = true;
        return;
    }
    let _ = ubus::call("zwrt_zte_dm", "cancel_download", Some("{}"));
    for package in PACKAGES {
        let _ = fs::remove_file(package);
    }
    eprintln!(
        "[firmware_lock] removed a firmware update package (state {})",
        upgrade.as_deref().unwrap_or("unknown")
    );
    let mut state = STATE.safe_lock();
    state.blocked += 1;
    state.last_blocked = Some(now());
    state.upgrade_state = upgrade;
}

fn tick(n: u64) {
    let _work = WORK.safe_lock();
    let enabled = STATE.safe_lock().settings.enabled;
    if enabled && !Path::new(INSTALLING).exists() && PACKAGES.iter().any(|p| Path::new(p).exists())
    {
        intercept();
    }
    if n.is_multiple_of(RECHECK_TICKS) {
        if let Err(e) = recheck(enabled) {
            eprintln!("[firmware_lock] {e}");
        }
    }
}

pub fn start() {
    START.call_once(|| {
        let (settings, error) = load();
        {
            let mut state = STATE.safe_lock();
            state.settings = settings;
            state.error = error;
        }
        std::thread::spawn(|| {
            let mut n: u64 = 0;
            loop {
                tick(n);
                n = n.wrapping_add(1);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    });
}

fn snapshot() -> Value {
    let s = STATE.safe_lock();
    json!({
        "revision": s.settings.revision,
        "enabled": s.settings.enabled,
        "servers_blocked": s.servers_blocked,
        "auto_update_off": s.auto_update_off,
        "upgrade_state": s.upgrade_state,
        "installing": s.installing,
        "blocked": s.blocked,
        "last_blocked": s.last_blocked,
        "error": s.error,
    })
}

/// GET /api/firmware/lock — cached; never waits on ubus.
pub fn get() -> (u16, Value) {
    (200, json!({"ok": true, "data": snapshot()}))
}

/// PUT /api/firmware/lock — `{"revision": n, "enabled": bool}`.
pub fn put(body: &[u8]) -> (u16, Value) {
    let Ok(update) = serde_json::from_slice::<Update>(body) else {
        return (
            400,
            json!({"ok": false, "error": "Invalid firmware lock request"}),
        );
    };
    let _work = WORK.safe_lock();
    let current = STATE.safe_lock().settings;
    let settings = match next(current, update) {
        Ok(s) => s,
        Err(e) => return (409, json!({"ok": false, "error": e})),
    };
    let bytes = serde_json::to_vec(&settings).unwrap_or_default();
    if let Err(e) = storage::atomic_write(Path::new(SETTINGS), &bytes) {
        return (
            500,
            json!({"ok": false, "error": format!("Saving firmware lock: {e}")}),
        );
    }
    STATE.safe_lock().settings = settings;
    if settings.enabled
        && !Path::new(INSTALLING).exists()
        && PACKAGES.iter().any(|p| Path::new(p).exists())
    {
        intercept();
    }
    match recheck(settings.enabled) {
        Ok(()) => get(),
        Err(e) => (
            500,
            json!({"ok": false, "error": format!("Firmware lock saved, but: {e}")}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: &str = "127.0.0.1 localhost OpenWrt\n\n::1     localhost ip6-localhost ip6-loopback\nff02::1 ip6-allnodes\nff02::2 ip6-allrouters\n";

    #[test]
    fn hosts_lines_are_added_once_and_removed_cleanly() {
        let locked = hosts_with(STOCK, true);
        assert!(locked.starts_with(STOCK));
        assert!(hosts_blocked(&locked));
        assert!(!hosts_blocked(STOCK));
        assert_eq!(hosts_with(&locked, true), locked);
        assert_eq!(
            locked.lines().filter(|l| l.ends_with(TAG)).count(),
            SERVERS.len() * 2
        );
        assert!(locked.contains("255.255.255.255 dmcn.ztems.com # openui-firmware-lock\n"));
        assert!(locked.contains("::ffff:255.255.255.255 dmcn.ztems.com # openui-firmware-lock\n"));
        // Lines from the first build (0.0.0.0) are replaced, not kept beside the new ones.
        let old = format!("{STOCK}0.0.0.0 dmcn.ztems.com {TAG}\n:: dmcn.ztems.com {TAG}\n");
        assert_eq!(hosts_with(&old, true), locked);
        assert_eq!(hosts_with(&locked, false), STOCK);
    }

    #[test]
    fn unrelated_hosts_lines_are_kept() {
        let custom = "127.0.0.1 localhost\n10.0.0.2 nas.lan\n0.0.0.0 dmcn.ztems.com\n";
        let locked = hosts_with(custom, true);
        assert!(locked.starts_with(custom));
        // The user's own untagged entry is left alone when the lock goes off.
        assert_eq!(hosts_with(&locked, false), custom);
        // A missing final newline is completed rather than glued to our lines.
        assert_eq!(
            hosts_with("127.0.0.1 localhost", false),
            "127.0.0.1 localhost\n"
        );
    }

    #[test]
    fn partial_block_is_not_reported_as_blocked() {
        let partial: String = hosts_with(STOCK, true)
            .lines()
            .filter(|l| !l.starts_with("::ffff:255.255.255.255 dms6"))
            .flat_map(|l| [l, "\n"])
            .collect();
        assert!(!hosts_blocked(&partial));
    }

    #[test]
    fn switch_requires_the_current_revision() {
        let s = Settings::default();
        let on = next(
            s,
            Update {
                revision: 0,
                enabled: true,
            },
        )
        .unwrap();
        assert_eq!(
            on,
            Settings {
                revision: 1,
                enabled: true
            }
        );
        assert!(next(
            on,
            Update {
                revision: 0,
                enabled: false
            }
        )
        .is_err());
        assert!(next(
            Settings {
                revision: MAX_REVISION,
                enabled: true
            },
            Update {
                revision: MAX_REVISION,
                enabled: false
            }
        )
        .is_err());
    }

    #[test]
    fn strict_request_and_settings() {
        assert!(serde_json::from_str::<Update>(r#"{"revision":0,"enabled":true}"#).is_ok());
        for bad in [
            r#"{"revision":0,"enabled":1}"#,
            r#"{"revision":"0","enabled":true}"#,
            r#"{"revision":0}"#,
            r#"{"revision":0,"enabled":true,"servers":[]}"#,
        ] {
            assert!(serde_json::from_str::<Update>(bad).is_err());
        }
        assert!(
            serde_json::from_str::<Settings>(r#"{"revision":3,"enabled":false,"x":1}"#).is_err()
        );
    }
}
