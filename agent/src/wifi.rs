use crate::process::BoundedCommand;
use std::collections::HashMap;
use std::process::Command;

use serde_json::{json, Value};

use crate::handlers::AppState;
use crate::ubus;
use crate::uci_transaction::{self, Change};
use crate::util::MutexExt;

const WIFI_ONOFF_KEY: &str = "wifi_onoff";
const WIFI6_SWITCH_KEY: &str = "wifi6_switch";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The whole `wireless` and `zte_mbb` configs, dumped once per request.
struct WifiConfig {
    wireless: HashMap<String, String>,
    mbb: HashMap<String, String>,
}

impl WifiConfig {
    fn load() -> Self {
        Self {
            wireless: ubus::uci_show("wireless"),
            mbb: ubus::uci_show("zte_mbb"),
        }
    }

    fn get(&self, key: &str) -> String {
        self.wireless.get(key).cloned().unwrap_or_default()
    }

    /// Newer firmware (CN B27+) keeps the global Wi-Fi switches in a separate
    /// `zte_mbb` UCI config instead of `wireless.zte_mbb`. Read both namespaces.
    fn feature(&self, key: &str) -> String {
        if let Some(v) = self.mbb.get(&format!("wifi.{key}")) {
            if !v.is_empty() {
                return v.clone();
            }
        }
        self.get(&format!("zte_mbb.{key}"))
    }
}

fn report_value(report: Option<&Value>, key: &str) -> String {
    report
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn iw_info(iface: &str) -> (String, String) {
    let output = Command::new("iw")
        .args([iface, "info"])
        .bounded_output()
        .ok();
    let out = output
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let channel = out
        .lines()
        .find_map(|l| {
            let l = l.trim();
            if l.starts_with("channel ") {
                l.split_whitespace().nth(1).map(|s| s.to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();
    let bw = out
        .lines()
        .find_map(|l| {
            let l = l.trim();
            if let Some(pos) = l.find("width:") {
                let rest = l[pos + 6..].trim();
                let end = rest.find("MHz").map(|i| i + 3).unwrap_or(rest.len());
                Some(rest[..end].trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();
    (channel, bw)
}

/// Count associated stations. Calls `iw` directly and counts in Rust — the
/// old `sh -c "iw ... | grep -c Station"` spawned three processes per band.
fn station_count(iface: &str) -> u64 {
    let Ok(output) = Command::new("iw")
        .args([iface, "station", "dump"])
        .bounded_output()
    else {
        return 0;
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| l.trim_start().starts_with("Station "))
        .count() as u64
}

fn bandwidth_options(hwmode: &str, standards: &str, is_5g: bool) -> Vec<String> {
    let mode = hwmode.to_ascii_lowercase();
    let supported: Vec<String> = standards
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .collect();
    let has = |standard: &str| supported.iter().any(|value| value == standard);
    let prefix = if mode.contains("be") || has("be") {
        "EHT"
    } else if mode.contains("ax") || has("ax") {
        "HE"
    } else if is_5g && (mode.contains("ac") || has("ac")) {
        "VHT"
    } else {
        "HT"
    };
    let widths: &[u16] = if is_5g { &[20, 40, 80, 160] } else { &[20, 40] };
    widths
        .iter()
        .map(|width| format!("{prefix}{width}"))
        .collect()
}

// ---------------------------------------------------------------------------
// GET /api/wifi/status
// ---------------------------------------------------------------------------

pub fn wifi_status(_state: &AppState) -> (u16, Value) {
    let mut result = serde_json::Map::new();
    let report = ubus::call("zwrt_wlan", "report", Some("{}")).ok();
    let cfg = WifiConfig::load();

    // Global switches from wireless feature config (both UCI namespaces),
    // with report fallback when exposed there.
    let mut wifi_onoff = cfg.feature(WIFI_ONOFF_KEY);
    if wifi_onoff.is_empty() {
        wifi_onoff = report_value(report.as_ref(), WIFI_ONOFF_KEY);
    }
    let wifi_onoff_supported = !wifi_onoff.is_empty();
    if wifi_onoff_supported {
        result.insert("wifi_onoff".into(), json!(wifi_onoff));
    }
    result.insert("wifi_onoff_supported".into(), json!(wifi_onoff_supported));

    let mut wifi6_switch = cfg.feature(WIFI6_SWITCH_KEY);
    if wifi6_switch.is_empty() {
        wifi6_switch = report_value(report.as_ref(), WIFI6_SWITCH_KEY);
    }
    let wifi6_supported = !wifi6_switch.is_empty();
    if wifi6_supported {
        result.insert("wifi6_switch".into(), json!(wifi6_switch));
    }
    result.insert("wifi6_supported".into(), json!(wifi6_supported));

    // Radio config
    result.insert("radio2_disabled".into(), json!(cfg.get("wifi0.disabled")));
    result.insert("radio5_disabled".into(), json!(cfg.get("wifi1.disabled")));
    result.insert("channel_2g".into(), json!(cfg.get("wifi0.channel")));
    result.insert("channel_5g".into(), json!(cfg.get("wifi1.channel")));
    result.insert("txpower_2g".into(), json!(cfg.get("wifi0.txpowerpercent")));
    result.insert("txpower_5g".into(), json!(cfg.get("wifi1.txpowerpercent")));
    result.insert("htmode_2g".into(), json!(cfg.get("wifi0.htmode")));
    result.insert("htmode_5g".into(), json!(cfg.get("wifi1.htmode")));
    let hwmode_2g = cfg.get("wifi0.hwmode");
    let hwmode_5g = cfg.get("wifi1.hwmode");
    let standards_2g = cfg.get("wifi0.SupportedStandards");
    let standards_5g = cfg.get("wifi1.SupportedStandards");
    result.insert("hwmode_2g".into(), json!(hwmode_2g));
    result.insert("hwmode_5g".into(), json!(hwmode_5g));
    result.insert("supported_standards_2g".into(), json!(standards_2g));
    result.insert("supported_standards_5g".into(), json!(standards_5g));
    result.insert(
        "bandwidth_options_2g".into(),
        json!(bandwidth_options(&hwmode_2g, &standards_2g, false)),
    );
    result.insert(
        "bandwidth_options_5g".into(),
        json!(bandwidth_options(&hwmode_5g, &standards_5g, true)),
    );
    result.insert(
        "wifi7_supported".into(),
        json!(
            standards_2g.split(',').any(|s| s.trim() == "be")
                || standards_5g.split(',').any(|s| s.trim() == "be")
        ),
    );
    result.insert("country_code".into(), json!(cfg.get("wifi0.country")));

    // Interface config
    result.insert("ssid_2g".into(), json!(cfg.get("main_2g.ssid")));
    result.insert("ssid_5g".into(), json!(cfg.get("main_5g.ssid")));
    result.insert("key_2g".into(), json!(cfg.get("main_2g.key")));
    result.insert("key_5g".into(), json!(cfg.get("main_5g.key")));
    result.insert(
        "has_key_2g".into(),
        json!(!cfg.get("main_2g.key").is_empty()),
    );
    result.insert(
        "has_key_5g".into(),
        json!(!cfg.get("main_5g.key").is_empty()),
    );
    result.insert("encryption_2g".into(), json!(cfg.get("main_2g.encryption")));
    result.insert("encryption_5g".into(), json!(cfg.get("main_5g.encryption")));
    result.insert("hidden_2g".into(), json!(cfg.get("main_2g.hidden")));
    result.insert("hidden_5g".into(), json!(cfg.get("main_5g.hidden")));

    // Runtime info from iw
    let (ch2, bw2) = iw_info("wlan0");
    let (ch5, bw5) = iw_info("wlan2");
    result.insert("actual_channel_2g".into(), json!(ch2));
    result.insert("actual_bw_2g".into(), json!(bw2));
    result.insert("actual_channel_5g".into(), json!(ch5));
    result.insert("actual_bw_5g".into(), json!(bw5));

    // Client counts
    let c2g = station_count("wlan0");
    let c5g = station_count("wlan2");
    result.insert("clients_2g".into(), json!(c2g));
    result.insert("clients_5g".into(), json!(c5g));
    result.insert("clients_total".into(), json!(c2g + c5g));

    // Guest WiFi summary
    result.insert(
        "guest_disabled_2g".into(),
        json!(cfg.get("guest_2g.disabled")),
    );
    result.insert(
        "guest_disabled_5g".into(),
        json!(cfg.get("guest_5g.disabled")),
    );
    result.insert("guest_ssid".into(), json!(cfg.get("guest_2g.ssid")));

    (200, json!({"ok": true, "data": result}))
}

// ---------------------------------------------------------------------------
// PUT /api/wifi/settings
// ---------------------------------------------------------------------------

const WIFI_FIELDS: &[(&str, &str)] = &[
    ("ssid_2g", "wireless.main_2g.ssid"),
    ("ssid_5g", "wireless.main_5g.ssid"),
    ("key_2g", "wireless.main_2g.key"), // gitleaks:allow -- UCI option path, not a credential.
    ("key_5g", "wireless.main_5g.key"), // gitleaks:allow -- UCI option path, not a credential.
    ("encryption_2g", "wireless.main_2g.encryption"),
    ("encryption_5g", "wireless.main_5g.encryption"),
    ("hidden_2g", "wireless.main_2g.hidden"),
    ("hidden_5g", "wireless.main_5g.hidden"),
    ("channel_2g", "wireless.wifi0.channel"),
    ("channel_5g", "wireless.wifi1.channel"),
    ("txpower_2g", "wireless.wifi0.txpowerpercent"),
    ("txpower_5g", "wireless.wifi1.txpowerpercent"),
    ("htmode_2g", "wireless.wifi0.htmode"),
    ("htmode_5g", "wireless.wifi1.htmode"),
    ("radio2_disabled", "wireless.wifi0.disabled"),
    ("radio5_disabled", "wireless.wifi1.disabled"),
];

fn wifi_value(key: &str, value: &Value, cfg: &WifiConfig) -> Result<String, String> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Bool(v) => if *v { "1" } else { "0" }.into(),
        Value::Number(n) if n.as_u64().is_some() => n.to_string(),
        _ => return Err(format!("{key} has an invalid type")),
    };
    if text.chars().any(char::is_control) {
        return Err(format!("{key} contains a control character"));
    }
    let valid = if key.starts_with("ssid_") {
        // Reject unsupported shell metacharacters explicitly instead of silently
        // renaming the SSID. Firmware downstream handling is not shell-audited.
        (1..=32).contains(&text.len())
            && !text.chars().any(|c| {
                matches!(
                    c,
                    '\'' | '"' | ';' | '$' | '`' | '\\' | '|' | '<' | '>' | '&'
                )
            })
    } else if key.starts_with("key_") {
        (8..=63).contains(&text.len())
            || (text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()))
    } else if key.starts_with("encryption_") {
        [
            "none",
            "psk2",
            "psk2+ccmp",
            "psk2+aes",
            "psk3",
            "psk3-mixed",
            "sae",
            "sae-mixed",
        ]
        .contains(&text.as_str())
    } else if key.starts_with("txpower_") {
        text.parse::<u8>().is_ok_and(|n| (1..=100).contains(&n))
    } else if key.starts_with("channel_") {
        text == "auto"
            || text == "0"
            || text.parse::<u16>().is_ok_and(|n| {
                if key.ends_with("2g") {
                    (1..=13).contains(&n)
                } else {
                    [
                        36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128,
                        132, 136, 140, 144, 149, 153, 157, 161, 165,
                    ]
                    .contains(&n)
                }
            })
    } else if key.starts_with("htmode_") {
        let radio = if key.ends_with("2g") {
            "wifi0"
        } else {
            "wifi1"
        };
        bandwidth_options(
            &cfg.get(&format!("{radio}.hwmode")),
            &cfg.get(&format!("{radio}.SupportedStandards")),
            key.ends_with("5g"),
        )
        .contains(&text)
    } else {
        text == "0" || text == "1"
    };
    if valid {
        Ok(text)
    } else {
        Err(format!("{key} is outside the supported values"))
    }
}

fn plan_wifi(parsed: &Value, cfg: &WifiConfig) -> Result<Vec<Change>, String> {
    let obj = parsed
        .as_object()
        .ok_or("expected a Wi-Fi settings object")?;
    if obj.is_empty() || obj.len() > WIFI_FIELDS.len() + 2 {
        return Err("empty or oversized Wi-Fi update".into());
    }
    let mut changes = Vec::new();
    for (key, value) in obj {
        let mut paths = Vec::new();
        if key == WIFI_ONOFF_KEY || key == WIFI6_SWITCH_KEY {
            for path in [
                format!("wireless.zte_mbb.{key}"),
                format!("zte_mbb.wifi.{key}"),
            ] {
                let (config, tail) = path.split_once('.').unwrap();
                if (if config == "wireless" {
                    &cfg.wireless
                } else {
                    &cfg.mbb
                })
                .contains_key(tail)
                {
                    paths.push(path);
                }
            }
            if key == WIFI_ONOFF_KEY && cfg.wireless.contains_key("zte_mbb.wifi_onoff_by_user") {
                paths.push("wireless.zte_mbb.wifi_onoff_by_user".into());
            }
        } else {
            paths.push(
                WIFI_FIELDS
                    .iter()
                    .find(|(name, _)| name == key)
                    .ok_or_else(|| format!("unknown Wi-Fi setting: {key}"))?
                    .1
                    .into(),
            );
        }
        if paths.is_empty() {
            return Err(format!("{key} is not supported by this firmware"));
        }
        if key.starts_with("key_") && value.as_str() == Some("••••••••") {
            continue;
        }
        let after = wifi_value(key, value, cfg)?;
        for path in paths {
            let (config, tail) = path.split_once('.').unwrap();
            let before = (if config == "wireless" {
                &cfg.wireless
            } else {
                &cfg.mbb
            })
            .get(tail)
            .ok_or_else(|| format!("{key} is unavailable on this firmware"))?;
            if before != &after {
                changes.push(Change {
                    key: path.clone(),
                    before: before.clone(),
                    after: after.clone(),
                });
            }
        }
    }
    for suffix in ["2g", "5g"] {
        if obj.contains_key(&format!("encryption_{suffix}"))
            || obj.contains_key(&format!("key_{suffix}"))
        {
            let effective = |field: &str| {
                let path = format!("wireless.main_{suffix}.{field}");
                changes
                    .iter()
                    .find(|c| c.key == path)
                    .map(|c| c.after.clone())
                    .unwrap_or_else(|| cfg.get(&format!("main_{suffix}.{field}")))
            };
            if effective("encryption") != "none" {
                wifi_value(
                    &format!("key_{suffix}"),
                    &Value::String(effective("key")),
                    cfg,
                )?;
            }
        }
    }
    Ok(changes)
}

pub fn wifi_set(_state: &AppState, body: &[u8]) -> (u16, Value) {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(error) => {
            return (
                400,
                json!({"ok": false, "error": format!("invalid Wi-Fi settings: {error}")}),
            )
        }
    };
    let _guard = uci_transaction::WIFI_CHANGE.safe_lock();
    let changes = match plan_wifi(&parsed, &WifiConfig::load()) {
        Ok(changes) => changes,
        Err(error) => return (400, json!({"ok": false, "error": error})),
    };
    match uci_transaction::apply(&changes) {
        Ok(()) => (
            200,
            json!({"ok": true, "data": {"status": "ok", "changed": !changes.is_empty()}}),
        ),
        Err(error) => (503, json!({"ok": false, "error": error})),
    }
}

// ---------------------------------------------------------------------------
// Blocklist (kick)
// ---------------------------------------------------------------------------

/// The hotspot sections the firmware's MAC filter covers; guests only while enabled.
const ACL_SECTIONS: [&str; 4] = ["main_2g", "main_5g", "guest_2g", "guest_5g"];
const MAX_BLOCKED: usize = 32;
/// Names of blocked devices, kept because a blocked device leaves the client list.
const BLOCKED_NAMES: &str = "/data/local/tmp/openui-wifi-blocked.json";

fn uci_get(key: &str) -> Option<String> {
    Command::new("uci")
        .args(["-q", "get", key])
        .bounded_output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

pub fn valid_mac(mac: &str) -> bool {
    mac.len() == 17
        && mac
            .split(':')
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Blocked MACs (lowercase, sorted) from the main hotspots' deny lists.
pub fn blocked_macs() -> Vec<String> {
    let mut out: Vec<String> = ["main_2g", "main_5g"]
        .iter()
        .filter_map(|s| uci_get(&format!("wireless.{s}.denymaclist")))
        .flat_map(|l| {
            l.split_whitespace()
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        })
        .filter(|m| valid_mac(m))
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn blocked_names() -> HashMap<String, String> {
    std::fs::read(BLOCKED_NAMES)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_blocked_names(macs: &[String], name: Option<&str>) {
    let mut names = blocked_names();
    for mac in macs {
        match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => names.insert(mac.clone(), n.chars().take(48).collect()),
            None => names.remove(mac),
        };
    }
    if let Ok(bytes) = serde_json::to_vec(&names) {
        let _ = crate::storage::atomic_write(std::path::Path::new(BLOCKED_NAMES), &bytes);
    }
}

/// Names too common to identify one device: matching them would block strangers.
const GENERIC_NAMES: [&str; 8] = [
    "iphone",
    "ipad",
    "android",
    "localhost",
    "unknown",
    "galaxy",
    "macbook",
    "esp32",
];

/// The device name to match on, or `None` for a name too generic or short to trust.
fn match_name(name: &str) -> Option<String> {
    let n = name.trim().to_lowercase();
    (n.chars().count() >= 4 && n != "-" && !GENERIC_NAMES.contains(&n.as_str())).then_some(n)
}

/// `mac` and every blocked MAC saved under the same (matchable) name: one device that
/// changed its random MAC, unblocked together.
fn macs_sharing_name(mac: &str, names: &HashMap<String, String>) -> Vec<String> {
    let mut out = vec![mac.to_string()];
    if let Some(wanted) = names.get(mac).and_then(|n| match_name(n)) {
        out.extend(
            names
                .iter()
                .filter(|(m, n)| {
                    m.as_str() != mac && match_name(n).as_deref() == Some(wanted.as_str())
                })
                .map(|(m, _)| m.clone()),
        );
    }
    out.sort();
    out
}

/// Connected stations whose name is a blocked device's while their MAC is not blocked:
/// phones with random MACs come back under a new one after a kick.
fn stations_to_autoblock(
    clients: &[Value],
    blocked: &[String],
    names: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let wanted: std::collections::HashSet<String> =
        names.values().filter_map(|n| match_name(n)).collect();
    clients
        .iter()
        .filter(|c| c["medium"] == "wifi")
        .filter_map(|c| {
            Some((
                c["mac"].as_str()?.to_lowercase(),
                c["hostname"].as_str()?.to_string(),
            ))
        })
        .filter(|(mac, name)| {
            valid_mac(mac)
                && !blocked.contains(mac)
                && match_name(name).is_some_and(|n| wanted.contains(&n))
        })
        .collect()
}

/// One writer at a time: the panel's kick and the name guard both rewrite the list.
static BLOCK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Block (kick) or unblock a station through the firmware's own ACL path (`zwrt_wlan
/// set` with the whole deny list): it saves the lists, updates hostapd's deny ACL and
/// disconnects blocked stations, without restarting Wi-Fi. Unblocking also unblocks the
/// other MACs saved under the same name, or the name guard would block it again.
pub fn set_blocked(mac: &str, blocked: bool, name: Option<&str>) -> (u16, Value) {
    let mac = mac.to_lowercase();
    if !valid_mac(&mac) {
        return (400, json!({"ok": false, "error": "Invalid MAC address"}));
    }
    let _lock = BLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if uci_get("wireless.main_2g.macfilter").as_deref() == Some("allow") {
        return (
            409,
            json!({"ok": false, "error": "Wi-Fi allowlist mode is on; blocking is unavailable"}),
        );
    }
    let targets = if blocked {
        vec![mac.clone()]
    } else {
        macs_sharing_name(&mac, &blocked_names())
    };
    let done = |list: &[String]| targets.iter().all(|t| list.contains(t) == blocked);
    let mut list = blocked_macs();
    if !done(&list) {
        if blocked {
            if list.len() >= MAX_BLOCKED {
                return (409, json!({"ok": false, "error": "The blocklist is full"}));
            }
            list.push(mac.clone());
            list.sort();
        } else {
            list.retain(|m| !targets.contains(m));
        }
        let mut body = serde_json::Map::new();
        for s in ACL_SECTIONS {
            if s.starts_with("main")
                || uci_get(&format!("wireless.{s}.disabled")).as_deref() == Some("0")
            {
                body.insert(s.into(), json!({"macfilter": "deny", "denymaclist": list}));
            }
        }
        if let Err(e) = ubus::call("zwrt_wlan", "set", Some(&Value::Object(body).to_string())) {
            return (503, json!({"ok": false, "error": e}));
        }
        // The firmware applies it in the background; wait for the saved list.
        for _ in 0..20 {
            if done(&blocked_macs()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if !done(&blocked_macs()) {
            return (
                500,
                json!({"ok": false, "error": "The Wi-Fi blocklist did not change"}),
            );
        }
    }
    save_blocked_names(&targets, if blocked { name } else { None });
    eprintln!(
        "[wifi] {} {}{}",
        if blocked { "blocked" } else { "unblocked" },
        targets.join(" "),
        name.map(|n| format!(" ({n})")).unwrap_or_default()
    );
    if blocked {
        for ifname in ["wlan0", "wlan2"] {
            let _ = Command::new("hostapd_cli")
                .args([
                    "-p",
                    "/data/vendor/wifi/hostapd",
                    "-i",
                    ifname,
                    "disassociate",
                    &mac,
                ])
                .bounded_output();
        }
    }
    (
        200,
        json!({"ok": true, "data": {"blocked": blocked_macs()}}),
    )
}

/// The blocklist as the web page shows it: each MAC with the name it was blocked under.
fn blocked_view() -> Value {
    let names = blocked_names();
    let list: Vec<Value> = blocked_macs()
        .into_iter()
        .map(|m| json!({"name": names.get(&m), "mac": m}))
        .collect();
    json!({
        "blocked": list,
        "allowlist": uci_get("wireless.main_2g.macfilter").as_deref() == Some("allow"),
        "max": MAX_BLOCKED,
    })
}

pub fn blocked_get() -> (u16, Value) {
    (200, json!({"ok": true, "data": blocked_view()}))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockRequest {
    mac: String,
    blocked: bool,
    /// The device's name when blocked: a later station under it is blocked too.
    #[serde(default)]
    name: Option<String>,
}

/// `PUT /api/wifi/blocked`: the same block (kick) and unblock as the screen's.
pub fn blocked_set(body: &[u8]) -> (u16, Value) {
    let Ok(r) = serde_json::from_slice::<BlockRequest>(body) else {
        return (400, json!({"ok": false, "error": "Invalid request"}));
    };
    let name = r
        .name
        .map(|n| n.chars().filter(|c| !c.is_control()).collect::<String>());
    let (code, reply) = set_blocked(&r.mac, r.blocked, name.as_deref());
    if code != 200 {
        return (code, reply);
    }
    (200, json!({"ok": true, "data": blocked_view()}))
}

/// Every 5 s while a blocked device has a usable name: kick and block a station that
/// connects under that name with a new MAC.
pub fn start_name_guard() {
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let names = blocked_names();
        if !names.values().any(|n| match_name(n).is_some()) {
            continue;
        }
        let clients = crate::network_ext::clients();
        for (mac, name) in stations_to_autoblock(&clients, &blocked_macs(), &names) {
            let (code, _) = set_blocked(&mac, true, Some(&name));
            eprintln!("[wifi] blocked {mac} by its name \"{name}\": {code}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn web_block_requests_are_strict_and_reject_bad_macs() {
        let r: BlockRequest =
            serde_json::from_str(r#"{"mac":"AA:BB:CC:00:00:01","blocked":true,"name":"Pixel-8"}"#)
                .unwrap();
        assert!(r.blocked && r.name.as_deref() == Some("Pixel-8"));
        assert!(serde_json::from_str::<BlockRequest>(
            r#"{"mac":"aa:bb:cc:00:00:01","blocked":false}"#
        )
        .unwrap()
        .name
        .is_none());
        for bad in [
            r#"{"mac":"aa:bb:cc:00:00:01"}"#,
            r#"{"mac":"aa:bb:cc:00:00:01","blocked":"yes"}"#,
            r#"{"mac":"aa:bb:cc:00:00:01","blocked":true,"cmd":"x"}"#,
        ] {
            assert!(serde_json::from_str::<BlockRequest>(bad).is_err(), "{bad}");
        }
        // A bad MAC is refused before any uci or ubus call.
        assert_eq!(
            blocked_set(br#"{"mac":"aa:bb:cc:00:00:0g","blocked":true}"#).0,
            400
        );
        assert_eq!(
            blocked_set(br#"{"mac":"aa:bb:cc:00:00:01;reboot","blocked":true}"#).0,
            400
        );
        assert_eq!(blocked_set(b"not json").0, 400);
    }
    #[test]
    fn name_guard_matches_specific_names_only() {
        let names = HashMap::from([
            (
                "02:11:22:33:44:56".to_string(),
                "Alex-s-Galaxy-Fold".to_string(),
            ),
            ("aa:aa:aa:aa:aa:01".to_string(), "iPhone".to_string()),
        ]);
        let blocked = vec![
            "02:11:22:33:44:56".to_string(),
            "aa:aa:aa:aa:aa:01".to_string(),
        ];
        let clients = vec![
            json!({"mac":"06:AA:BB:CC:DD:EF","medium":"wifi","hostname":"alex-s-galaxy-fold"}),
            json!({"mac":"bb:bb:bb:bb:bb:01","medium":"wifi","hostname":"iPhone"}),
            json!({"mac":"bb:bb:bb:bb:bb:02","medium":"usb-c","hostname":"Alex-s-Galaxy-Fold"}),
            json!({"mac":"02:11:22:33:44:56","medium":"wifi","hostname":"Alex-s-Galaxy-Fold"}),
            json!({"mac":"bb:bb:bb:bb:bb:03","medium":"wifi","hostname":null}),
        ];
        assert_eq!(
            stations_to_autoblock(&clients, &blocked, &names),
            vec![(
                "06:aa:bb:cc:dd:ef".to_string(),
                "alex-s-galaxy-fold".to_string()
            )]
        );
        assert_eq!(match_name(" iPad "), None);
        assert_eq!(match_name("abc"), None);
    }

    #[test]
    fn unblocking_takes_every_mac_of_the_same_name() {
        let names = HashMap::from([
            (
                "02:11:22:33:44:56".to_string(),
                "Alex-s-Galaxy-Fold".to_string(),
            ),
            (
                "06:aa:bb:cc:dd:ef".to_string(),
                "ALEX-S-GALAXY-FOLD".to_string(),
            ),
            ("aa:aa:aa:aa:aa:01".to_string(), "iPhone".to_string()),
            ("aa:aa:aa:aa:aa:02".to_string(), "iPhone".to_string()),
        ]);
        assert_eq!(
            macs_sharing_name("02:11:22:33:44:56", &names),
            vec![
                "02:11:22:33:44:56".to_string(),
                "06:aa:bb:cc:dd:ef".to_string()
            ]
        );
        // Generic names are never grouped.
        assert_eq!(
            macs_sharing_name("aa:aa:aa:aa:aa:01", &names),
            vec!["aa:aa:aa:aa:aa:01".to_string()]
        );
        assert_eq!(
            macs_sharing_name("cc:cc:cc:cc:cc:01", &names),
            vec!["cc:cc:cc:cc:cc:01".to_string()]
        );
    }

    fn config() -> WifiConfig {
        let mut wireless = HashMap::new();
        for (_, path) in WIFI_FIELDS {
            wireless.insert(path.strip_prefix("wireless.").unwrap().into(), "0".into());
        }
        wireless.insert("wifi0.hwmode".into(), "11beg".into());
        wireless.insert("wifi0.SupportedStandards".into(), "b,g,n,ax,be".into());
        wireless.insert("main_2g.key".into(), "oldpassword".into());
        wireless.insert("main_2g.encryption".into(), "psk3-mixed".into());
        WifiConfig {
            wireless,
            mbb: HashMap::new(),
        }
    }
    #[test]
    fn validates_entire_request_and_rejects_unknown_fields() {
        let cfg = config();
        for body in [
            json!({"ssid_2g": "Valid", "txpower_2g": 101}),
            json!({"key_2g": "short"}),
            json!({"channel_2g": 36}),
            json!({"ssid_2g": "line\nbreak"}),
            json!({"ssid_2g": "a".repeat(33)}),
            json!({"radio2_disabled": 3}),
            json!({"encryption_2g": "invalid"}),
            json!({"unknown": true}),
            json!({"htmode_2g": "HT40"}),
        ] {
            assert!(plan_wifi(&body, &cfg).is_err(), "{body}");
        }
    }
    #[test]
    fn accepts_special_character_passwords_without_changing_them() {
        let key = r#"Pass$word'";\|<>&`!"#;
        let plan = plan_wifi(&json!({"key_2g": key, "htmode_2g": "EHT40"}), &config()).unwrap();
        assert_eq!(
            plan.iter().find(|c| c.key.ends_with(".key")).unwrap().after,
            key
        );
    }
    #[test]
    fn wifi7_radios_use_eht_bandwidth_names() {
        assert_eq!(
            bandwidth_options("11beg", "b,g,n,ax,be", false),
            ["EHT20", "EHT40"]
        );
        assert_eq!(
            bandwidth_options("11bea", "a,n,ac,ax,be", true),
            ["EHT20", "EHT40", "EHT80", "EHT160"]
        );
    }
}
