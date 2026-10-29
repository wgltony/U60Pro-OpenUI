use crate::process::BoundedCommand;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::handlers::AppState;
use crate::ubus;

const GADGET_DIR: &str = "/sys/kernel/config/usb_gadget/g1";
const CONFIG_DIR: &str = "/sys/kernel/config/usb_gadget/g1/configs/c.1";
const UDC_PATH: &str = "/sys/kernel/config/usb_gadget/g1/UDC";
const NCM_FUNC: &str = "/sys/kernel/config/usb_gadget/g1/functions/ncm.0";
const ECM_GSI_FUNC: &str = "/sys/kernel/config/usb_gadget/g1/functions/gsi.ecm";
const RNDIS_GSI_FUNC: &str = "/sys/kernel/config/usb_gadget/g1/functions/gsi.rndis";
const MASS_STORAGE_FUNC: &str = "/sys/kernel/config/usb_gadget/g1/functions/mass_storage.0";
const ADB_FUNC: &str = "/sys/kernel/config/usb_gadget/g1/functions/ffs.adb";
const NCM_LAST_ERROR_PATH: &str = "/tmp/zte-agent-usb-ncm.last_error";
const PERSIST_CONFIG_PATH: &str = "/data/local/tmp/usb_config.json";
/// Legacy location: NCM persistence used to share the Wi-Fi snapshot file.
/// On first read we migrate the key out, so the two concerns are decoupled.
const LEGACY_PERSIST_CONFIG_PATH: &str = "/data/local/tmp/wifi_config.json";
const USB_DEFAULT_MODE_KEY: &str = "usb_default_mode";
static USB_BUSY: AtomicBool = AtomicBool::new(false);
pub(crate) struct SwitchGuard;
impl SwitchGuard {
    pub(crate) fn acquire() -> Result<Self, String> {
        USB_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "another USB change is in progress".into())
    }
}
impl Drop for SwitchGuard {
    fn drop(&mut self) {
        USB_BUSY.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Boot persistence
// ---------------------------------------------------------------------------

/// The firmware's own composition switcher, the one boot runs.
const FIRMWARE_USB_SWITCH: &str = "/sbin/usb/compositions/usb_switch";
/// Its compositions as boot sets them up: (mode, product id, functions). RNDIS is the
/// boot default; `ffs` is ADB.
const FIRMWARE_COMPOSITIONS: [(&str, &str, &str); 2] = [
    (
        "rndis",
        "0x1404",
        "rndis_gsi,diag,serial,modem,mass_storage,ffs,dpl,qdss",
    ),
    ("ecm", "0x1405", "ecm_gsi,mass_storage,ffs"),
];

/// B31: no `zwrt_bsp.usb set`, and a configfs rebuild leaves the gadget unbound (tested:
/// the host saw no device until a reboot). Only the firmware's script switches cleanly.
fn firmware_switch_only() -> bool {
    !stock_usb_set_available() && Path::new(FIRMWARE_USB_SWITCH).exists()
}

/// The functions for `mode`. ADB (`ffs`) only with adbd running: FunctionFS without its
/// daemon makes the controller bind fail, and the script tries only once.
fn firmware_functions(mode: &str, adbd_running: bool) -> Option<(&'static str, String)> {
    let &(_, pid, functions) = FIRMWARE_COMPOSITIONS.iter().find(|c| c.0 == mode)?;
    let functions = functions
        .split(',')
        .filter(|f| adbd_running || *f != "ffs")
        .collect::<Vec<_>>()
        .join(",");
    Some((pid, functions))
}

fn adbd_running() -> bool {
    fs::read_dir("/proc").is_ok_and(|entries| {
        entries.flatten().any(|e| {
            fs::read_link(e.path().join("exe")).is_ok_and(|exe| exe == Path::new("/sbin/adbd"))
        })
    })
}

fn firmware_mode_set(mode: &str) -> (u16, Value) {
    let Some((pid, functions)) = firmware_functions(mode, adbd_running()) else {
        return (
            400,
            json!({"ok": false, "error": "This firmware supports RNDIS and ECM only"}),
        );
    };
    if detect_active_usb_mode() == Some(mode) {
        return (
            200,
            json!({"ok": true, "data": {"status": "unchanged", "mode": mode}}),
        );
    }
    let guard = match SwitchGuard::acquire() {
        Ok(guard) => guard,
        Err(error) => return (409, json!({"ok": false, "error": error})),
    };
    // The same serial in every mode, so ADB keeps one device identity.
    let serial = read_trimmed(&format!("{GADGET_DIR}/strings/0x409/serialnumber"))
        .filter(|s| s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "0123456789ABCDEF".into());
    // After a second, so this reply (over USB too) leaves before USB drops.
    thread::spawn(move || {
        let _guard = guard;
        thread::sleep(Duration::from_millis(1000));
        // No timeout: killing the script between unbind and bind leaves USB dead.
        let result = Command::new("sh")
            .args([FIRMWARE_USB_SWITCH, "0x19d2", pid, &functions, &serial])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match result {
            Ok(status) if status.success() => {
                let _ = fs::remove_file(NCM_LAST_ERROR_PATH);
            }
            Ok(status) => {
                let _ = fs::write(NCM_LAST_ERROR_PATH, format!("usb_switch exit {status}"));
            }
            Err(e) => {
                let _ = fs::write(NCM_LAST_ERROR_PATH, format!("usb_switch: {e}"));
            }
        }
    });
    (
        202,
        json!({"ok": true, "data": {"status": "scheduled", "mode": mode, "delay_ms": 1000}}),
    )
}

/// Whether the firmware's `zwrt_bsp.usb` offers `set` (B31 has only `list`).
fn stock_usb_set_available() -> bool {
    Command::new("ubus")
        .args(["-v", "list", "zwrt_bsp.usb"])
        .bounded_output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("\"set\""))
}

/// The boot composition (RNDIS with ADB and the firmware's diagnostic functions), kept
/// for the first agent start of a boot so a later switch can return to it exactly.
const BOOT_SNAPSHOT_PATH: &str = "/tmp/zte-agent-usb-boot.json";

pub fn remember_boot_composition() {
    if Path::new(BOOT_SNAPSHOT_PATH).exists() || detect_active_usb_mode() != Some("rndis") {
        return;
    }
    if let Ok(snapshot) = GadgetSnapshot::capture() {
        if let Ok(bytes) = serde_json::to_vec(&snapshot.to_json()) {
            let _ = crate::storage::atomic_write(Path::new(BOOT_SNAPSHOT_PATH), &bytes);
        }
    }
}

fn load_boot_snapshot() -> Option<GadgetSnapshot> {
    let text = fs::read_to_string(BOOT_SNAPSHOT_PATH).ok()?;
    GadgetSnapshot::from_json(&serde_json::from_str(&text).ok()?)
}

pub fn enforce_usb_mode_on_boot() {
    if parse_usb_default_mode(&read_persisted_config()) != Some("ncm") {
        return;
    }

    // Powering the device off while it stays plugged in soft-reboots into a
    // charging state with no WAN. Re-presenting the NCM tether there makes the
    // tethered host route all traffic through a dead link. Skip enforcement in
    // that state, mirroring the stock Wi-Fi boot guard (zte_start_wlan_at_boot.sh).
    if is_power_off_charging() {
        return;
    }

    thread::spawn(|| {
        let Ok(_guard) = SwitchGuard::acquire() else {
            return;
        };
        if let Err(error) = wait_for_usb_boot_ready(Duration::from_secs(75)) {
            let _ = fs::write(NCM_LAST_ERROR_PATH, error);
            return;
        }
        if read_mode_main_state().is_none() {
            let _ = fs::write(
                NCM_LAST_ERROR_PATH,
                "power state unavailable; boot USB change skipped",
            );
            return;
        }
        // Re-check: mode_main_state can still be settling early in boot, and the
        // readiness wait above can run for up to 75s.
        if is_power_off_charging() {
            return;
        }
        if detect_active_usb_mode() == Some("ncm") {
            let _ = fs::remove_file(NCM_LAST_ERROR_PATH);
            return;
        }

        match transactional_switch("ncm") {
            Ok(()) => {
                let _ = fs::remove_file(NCM_LAST_ERROR_PATH);
            }
            Err(e) => {
                let _ = fs::write(NCM_LAST_ERROR_PATH, format!("boot NCM persistence: {e}"));
            }
        }
    });
}

/// Stock power-off-while-charging detection. When the device is switched off but
/// left plugged into USB, it soft-reboots into a charging state where
/// `zwrt_zte_mc_tmp.mode.mode_main_state` reads one of these values. The stock
/// Wi-Fi boot script guards on the same values; we mirror it so NCM persistence
/// doesn't re-present the tether to a host that has no WAN behind it.
fn is_power_off_charging() -> bool {
    is_power_off_charging_state(read_mode_main_state().as_deref())
}

fn is_power_off_charging_state(state: Option<&str>) -> bool {
    matches!(
        state,
        Some("mode_power_off_charger") | Some("mode_power_off_unreal")
    )
}

fn read_mode_main_state() -> Option<String> {
    let output = Command::new("uci")
        .args(["get", "zwrt_zte_mc_tmp.mode.mode_main_state"])
        .bounded_output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn wait_for_usb_boot_ready(max: Duration) -> Result<(), String> {
    wait_until_ready(max, Duration::from_millis(250), || {
        let configfs_ready = Path::new(GADGET_DIR).exists()
            && Path::new(CONFIG_DIR).exists()
            && Path::new(NCM_FUNC).exists()
            && Path::new(MASS_STORAGE_FUNC).exists();
        let controller_ready = first_udc_name().is_some() || read_trimmed(UDC_PATH).is_some();
        let stock_bridged = bridge_members("br-lan")
            .iter()
            .any(|m| m == "ecm0" || m == "rndis0");
        configfs_ready
            && controller_ready
            && !current_composition_functions().is_empty()
            && stock_bridged
    })
}

fn wait_until_ready(
    max: Duration,
    interval: Duration,
    mut ready: impl FnMut() -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + max;
    let mut consecutive = 0;
    while Instant::now() < deadline {
        consecutive = if ready() { consecutive + 1 } else { 0 };
        if consecutive >= 4 {
            return Ok(());
        }
        thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
    }
    Err("USB readiness timed out; existing composition left unchanged".into())
}

/// Detect the currently active USB function by probing `/sys/class/net/`.
/// `zwrt_bsp.usb list` returns `mode: "user"` (a permission flag, not the
/// function), so we look at which gadget interface actually exists.
pub(crate) fn detect_active_usb_mode() -> Option<&'static str> {
    let functions = current_composition_functions();
    if functions.iter().any(|f| f == "ncm.0") {
        Some("ncm")
    } else if functions.iter().any(|f| f == "gsi.ecm" || f == "ecm.ecm") {
        Some("ecm")
    } else if functions
        .iter()
        .any(|f| f == "gsi.rndis" || f == "rndis.rndis")
    {
        Some("rndis")
    } else if Path::new("/sys/class/net/ncm0").exists() {
        Some("ncm")
    } else if Path::new("/sys/class/net/ecm0").exists() {
        Some("ecm")
    } else if Path::new("/sys/class/net/rndis0").exists() {
        Some("rndis")
    } else {
        None
    }
}

fn current_composition_functions() -> Vec<String> {
    let mut functions = Vec::new();
    let entries = match fs::read_dir(CONFIG_DIR) {
        Ok(v) => v,
        Err(_) => return functions,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with('f') {
            continue;
        }
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        if let Some(function) = target.file_name().and_then(|s| s.to_str()) {
            functions.push(function.to_string());
        }
    }
    functions.sort();
    functions
}

fn bridge_members(bridge: &str) -> Vec<String> {
    checked_bridge_members(bridge).unwrap_or_default()
}
fn checked_bridge_members(bridge: &str) -> Result<Vec<String>, String> {
    let path = format!("/sys/class/net/{bridge}/brif");
    let mut members = fs::read_dir(&path)
        .map_err(|e| format!("read {path}: {e}"))?
        .map(|entry| {
            entry
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    members.sort();
    Ok(members)
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn ncm_interface_name() -> Option<String> {
    let ifname = read_trimmed(&format!("{NCM_FUNC}/ifname"))?;
    if ifname == "(unnamed net_device)" {
        return None;
    }
    Path::new(&format!("/sys/class/net/{ifname}"))
        .exists()
        .then_some(ifname)
}

fn read_persisted_config() -> Value {
    if let Some(value) = fs::read_to_string(PERSIST_CONFIG_PATH)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
    {
        return value;
    }
    // Read the old key without rewriting unrelated Wi-Fi state. The next
    // explicit USB settings update writes the dedicated file atomically.
    fs::read_to_string(LEGACY_PERSIST_CONFIG_PATH)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get(USB_DEFAULT_MODE_KEY).cloned())
        .map(|value| json!({USB_DEFAULT_MODE_KEY: value}))
        .unwrap_or_else(|| json!({}))
}

fn write_persisted_config(persisted: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(persisted).map_err(|e| e.to_string())?;
    crate::storage::atomic_write(Path::new(PERSIST_CONFIG_PATH), &bytes)
        .map_err(|e| format!("write USB config: {e}"))
}

fn parse_usb_default_mode(persisted: &Value) -> Option<&'static str> {
    match persisted
        .get(USB_DEFAULT_MODE_KEY)
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("ncm") => Some("ncm"),
        Some("ecm") => Some("ecm"),
        _ => None,
    }
}

fn set_usb_default_mode(mode: &str) -> Result<(), String> {
    let mut persisted = read_persisted_config();
    if !persisted.is_object() {
        persisted = json!({});
    }
    let Some(obj) = persisted.as_object_mut() else {
        return Err("persistence config is not an object".into());
    };
    obj.insert(USB_DEFAULT_MODE_KEY.into(), json!(mode));
    write_persisted_config(&persisted)
}

fn supported_modes() -> Vec<&'static str> {
    let mut modes = vec!["rndis", "ecm"];
    if Path::new(NCM_FUNC).exists() && !firmware_switch_only() {
        modes.push("ncm");
    }
    modes
}

/// Map a Linux UDC speed string (`current_speed`/`maximum_speed`) to a friendly
/// USB generation label and its line rate in Mbit/s.
fn usb_speed_label(speed: &str) -> Option<(&'static str, f64)> {
    match speed {
        "low-speed" => Some(("USB 1.0", 1.5)),
        "full-speed" => Some(("USB 1.1", 12.0)),
        "high-speed" => Some(("USB 2.0", 480.0)),
        "super-speed" => Some(("USB 3.0", 5000.0)),
        "super-speed-plus" => Some(("USB 3.1 Gen2", 10000.0)),
        _ => None,
    }
}

/// Negotiated vs. maximum USB link speed reported by the device controller. The
/// negotiated value reflects the weakest of {controller, cable, host port}, so a
/// 10 Gbit/s cable reads `super-speed-plus` while a USB 2.0 cable reads
/// `high-speed` even though the SDX75 dwc3 controller can do SuperSpeed+.
fn usb_link_info() -> Option<Value> {
    let udc = read_trimmed(UDC_PATH).or_else(first_udc_name)?;
    let base = format!("/sys/class/udc/{udc}");
    let negotiated = read_trimmed(&format!("{base}/current_speed"));
    let max = read_trimmed(&format!("{base}/maximum_speed"));
    if negotiated.is_none() && max.is_none() {
        return None;
    }

    let mut link = Map::new();
    let mut neg_mbps = None;
    if let Some(n) = negotiated {
        if let Some((label, mbps)) = usb_speed_label(&n) {
            link.insert("negotiated_label".into(), json!(label));
            link.insert("negotiated_mbps".into(), json!(mbps));
            neg_mbps = Some(mbps);
        }
        link.insert("negotiated".into(), json!(n));
    }
    let mut max_mbps = None;
    if let Some(m) = max {
        if let Some((label, mbps)) = usb_speed_label(&m) {
            link.insert("max_label".into(), json!(label));
            link.insert("max_mbps".into(), json!(mbps));
            max_mbps = Some(mbps);
        }
        link.insert("max".into(), json!(m));
    }
    if let (Some(n), Some(mx)) = (neg_mbps, max_mbps) {
        link.insert("at_full_speed".into(), json!(n >= mx));
    }
    Some(Value::Object(link))
}

pub fn usb_status(_state: &AppState) -> (u16, Value) {
    let mut payload = match ubus::call("zwrt_bsp.usb", "list", Some("{}")) {
        Ok(Value::Object(m)) => m,
        Ok(_) => Map::new(),
        Err(e) => return (503, json!({"ok": false, "error": e})),
    };
    let ncm_available = Path::new(NCM_FUNC).exists();
    let bridge_members = bridge_members("br-lan");
    let persisted = read_persisted_config();
    let default_mode = parse_usb_default_mode(&persisted).unwrap_or("ecm");
    payload.insert("active_mode".into(), json!(detect_active_usb_mode()));
    payload.insert("default_mode".into(), json!(default_mode));
    payload.insert("ncm_persist_on_boot".into(), json!(default_mode == "ncm"));
    payload.insert("supported_modes".into(), json!(supported_modes()));
    payload.insert(
        "experimental_modes".into(),
        json!(if ncm_available {
            vec!["ncm"]
        } else {
            Vec::<&str>::new()
        }),
    );
    payload.insert(
        "mode_capabilities".into(),
        json!([
            {
                "mode": "rndis",
                "supported": Path::new(RNDIS_GSI_FUNC).exists(),
                "experimental": false,
                "function": "gsi.rndis"
            },
            {
                "mode": "ecm",
                "supported": Path::new(ECM_GSI_FUNC).exists(),
                "experimental": false,
                "function": "gsi.ecm"
            },
            {
                "mode": "ncm",
                "supported": ncm_available,
                "experimental": true,
                "function": "ncm.0",
                "note": "configfs NCM exists, but ZTE's ubus USB switch does not expose it"
            }
        ]),
    );
    payload.insert(
        "configfs".into(),
        json!({
            "present": Path::new(GADGET_DIR).exists(),
            "ncm": ncm_available,
            "gsi_ecm": Path::new(ECM_GSI_FUNC).exists(),
            "gsi_rndis": Path::new(RNDIS_GSI_FUNC).exists(),
        }),
    );
    payload.insert(
        "composition_functions".into(),
        json!(current_composition_functions()),
    );
    payload.insert(
        "bridge".into(),
        json!({"name": "br-lan", "members": bridge_members}),
    );
    payload.insert(
        "interfaces".into(),
        json!({
            "ecm0": Path::new("/sys/class/net/ecm0").exists(),
            "rndis0": Path::new("/sys/class/net/rndis0").exists(),
            "ncm0": Path::new("/sys/class/net/ncm0").exists(),
            "ncm_ifname": if detect_active_usb_mode() == Some("ncm") {
                ncm_interface_name()
            } else {
                None
            },
        }),
    );
    payload.insert(
        "usb_ids".into(),
        json!({
            "vendor": read_trimmed(&format!("{GADGET_DIR}/idVendor")),
            "product": read_trimmed(&format!("{GADGET_DIR}/idProduct")),
        }),
    );
    if let Some(last_error) = read_trimmed(NCM_LAST_ERROR_PATH) {
        payload.insert("ncm_last_error".into(), json!(last_error));
    }
    if let Some(link) = usb_link_info() {
        payload.insert("link".into(), link);
    }
    payload.insert("lan".into(), usb_lan_json(&usb_lan_view()));
    (200, json!({"ok": true, "data": payload}))
}

pub fn usb_mode_set(_state: &AppState, body: &[u8]) -> (u16, Value) {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return (400, json!({"ok": false, "error": "invalid JSON"})),
    };
    let mode = match parsed["mode"].as_str() {
        Some(v) => v,
        None => return (400, json!({"ok": false, "error": "mode is required"})),
    };
    if firmware_switch_only() {
        return firmware_mode_set(mode);
    }
    if mode == "ncm" {
        let confirmed = parsed["confirm_experimental"].as_bool().unwrap_or(false);
        if !confirmed {
            return (
                400,
                json!({
                    "ok": false,
                    "error": "NCM is experimental and disrupts USB. Retry with confirm_experimental=true from a Wi-Fi management path."
                }),
            );
        }
        if let Err(e) = preflight_ncm_switch() {
            return (400, json!({"ok": false, "error": e}));
        }
        if let Err(error) = schedule_switch("ncm") {
            return (409, json!({"ok": false, "error": error}));
        }
        return (
            202,
            json!({
                "ok": true,
                "data": {
                    "status": "scheduled",
                    "mode": "ncm",
                    "experimental": true,
                    "delay_ms": 1000,
                    "rollback": "reboot or switch back to ECM after reconnecting"
                }
            }),
        );
    }
    // ECM is always built through configfs (as NCM is), keeping exposed ADB: this
    // firmware's zwrt_bsp.usb has no `set` method, so the stock path cannot switch.
    if mode == "ecm"
        && (current_composition_functions().iter().any(|f| f == "ncm.0")
            || !stock_usb_set_available())
    {
        if let Err(e) = preflight_ecm_switch() {
            return (400, json!({"ok": false, "error": e}));
        }
        if let Err(error) = schedule_switch("ecm") {
            return (409, json!({"ok": false, "error": error}));
        }
        return (
            202,
            json!({
                "ok": true,
                "data": {
                    "status": "scheduled",
                    "mode": "ecm",
                    "delay_ms": 1000
                }
            }),
        );
    }

    if !["ecm", "rndis"].contains(&mode) {
        return (400, json!({"ok": false, "error": "unsupported USB mode"}));
    }
    if mode == "rndis" && !stock_usb_set_available() {
        if detect_active_usb_mode() == Some("rndis") {
            return (
                200,
                json!({"ok": true, "data": {"status": "unchanged", "mode": "rndis"}}),
            );
        }
        // RNDIS is the boot composition: going back means restoring what boot set up.
        if load_boot_snapshot().is_none() {
            return (
                409,
                json!({"ok": false, "error": "Restart the device to return to RNDIS"}),
            );
        }
        if let Err(error) = schedule_switch("rndis") {
            return (409, json!({"ok": false, "error": error}));
        }
        return (
            202,
            json!({"ok": true, "data": {"status": "scheduled", "mode": "rndis", "delay_ms": 1000}}),
        );
    }
    let _guard = match SwitchGuard::acquire() {
        Ok(guard) => guard,
        Err(error) => return (409, json!({"ok": false, "error": error})),
    };
    match ubus::call(
        "zwrt_bsp.usb",
        "set",
        Some(&json!({"mode": mode}).to_string()),
    ) {
        Ok(data) => (200, json!({"ok": true, "data": data})),
        Err(e) => (503, json!({"ok": false, "error": e})),
    }
}

pub fn usb_default_set(_state: &AppState, body: &[u8]) -> (u16, Value) {
    let _guard = match SwitchGuard::acquire() {
        Ok(guard) => guard,
        Err(error) => return (409, json!({"ok": false, "error": error})),
    };
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return (400, json!({"ok": false, "error": "invalid JSON"})),
    };
    let mode = match parsed["mode"].as_str() {
        Some("ecm") => "ecm",
        Some("ncm") => {
            let confirmed = parsed["confirm_experimental"].as_bool().unwrap_or(false);
            if !confirmed {
                return (
                    400,
                    json!({
                        "ok": false,
                        "error": "NCM persistence is experimental. Retry with confirm_experimental=true from a Wi-Fi management path."
                    }),
                );
            }
            if let Err(e) = preflight_ncm_switch() {
                return (400, json!({"ok": false, "error": e}));
            }
            "ncm"
        }
        Some(_) => {
            return (
                400,
                json!({"ok": false, "error": "mode must be ecm or ncm"}),
            )
        }
        None => return (400, json!({"ok": false, "error": "mode is required"})),
    };

    match set_usb_default_mode(mode) {
        Ok(()) => (
            200,
            json!({
                "ok": true,
                "data": {
                    "default_mode": mode,
                    "ncm_persist_on_boot": mode == "ncm"
                }
            }),
        ),
        Err(e) => (500, json!({"ok": false, "error": e})),
    }
}

/// PUT /api/usb/powerbank — control the USB powerbank/OTG output
/// (passthrough to `zwrt_bsp.powerbank set`)
pub fn usb_powerbank_set(_state: &AppState, body: &[u8]) -> (u16, Value) {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return (400, json!({"ok": false, "error": "invalid JSON"})),
    };
    if let Err(e) = crate::validate::validate_ubus_input(&parsed) {
        return (400, json!({"ok": false, "error": e}));
    }
    match ubus::call("zwrt_bsp.powerbank", "set", Some(&parsed.to_string())) {
        Ok(data) => (200, json!({"ok": true, "data": data})),
        Err(e) => (503, json!({"ok": false, "error": e})),
    }
}

// ---------------------------------------------------------------------------
// USB LAN sharing
// ---------------------------------------------------------------------------

/// Whether the active USB network function is bridged into the LAN, so the USB host
/// gets an address and internet. Off takes the interface out of `br-lan` and down; the
/// USB composition (and ADB with it) stays as it is, so nothing re-enumerates. The
/// choice is persisted and re-applied: the firmware re-bridges the interface on replug
/// and the NCM switch bridges it itself.
const USB_LAN_KEY: &str = "usb_lan_enabled";
const LAN_BRIDGE: &str = "br-lan";
const USB_LAN_ENFORCE_EVERY: Duration = Duration::from_secs(3);
static USB_LAN_LOCK: Mutex<()> = Mutex::new(());

fn parse_usb_lan_enabled(persisted: &Value) -> bool {
    persisted
        .get(USB_LAN_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// The network interface of the active USB function, when it exists.
fn usb_lan_interface() -> Option<String> {
    let name = match detect_active_usb_mode()? {
        "ncm" => return ncm_interface_name(),
        "ecm" => "ecm0",
        _ => "rndis0",
    };
    Path::new(&format!("/sys/class/net/{name}"))
        .exists()
        .then(|| name.to_string())
}

fn interface_up(ifname: &str) -> bool {
    read_trimmed(&format!("/sys/class/net/{ifname}/flags"))
        .and_then(|f| u32::from_str_radix(f.trim_start_matches("0x"), 16).ok())
        .is_some_and(|flags| flags & 1 != 0)
}

#[derive(Debug, PartialEq)]
pub(crate) struct UsbLan {
    /// The saved choice.
    pub enabled: bool,
    /// The interface is up and bridged right now.
    pub active: bool,
    /// Active, and a USB host has the link (carrier); RNDIS on a Mac never gets one.
    pub linked: bool,
    pub interface: Option<String>,
}

pub(crate) fn usb_lan_view() -> UsbLan {
    let enabled = parse_usb_lan_enabled(&read_persisted_config());
    let interface = usb_lan_interface();
    let active = interface.as_deref().is_some_and(|ifname| {
        interface_up(ifname) && bridge_members(LAN_BRIDGE).iter().any(|m| m == ifname)
    });
    let linked = active
        && interface
            .as_deref()
            .and_then(|ifname| read_trimmed(&format!("/sys/class/net/{ifname}/carrier")))
            .is_some_and(|c| c == "1");
    UsbLan {
        enabled,
        active,
        linked,
        interface,
    }
}

fn usb_lan_json(view: &UsbLan) -> Value {
    json!({
        "enabled": view.enabled,
        "active": view.active,
        "linked": view.linked,
        "interface": view.interface,
        "mode": detect_active_usb_mode(),
    })
}

/// Bring the active USB interface in line with `enabled`. No interface: nothing to do.
fn apply_usb_lan(enabled: bool) -> Result<(), String> {
    let Some(ifname) = usb_lan_interface() else {
        return Ok(());
    };
    if enabled {
        run_command("ip", &["link", "set", "dev", &ifname, "up"])?;
        add_bridge_member(LAN_BRIDGE, &ifname)
    } else {
        remove_bridge_member(LAN_BRIDGE, &ifname)?;
        run_command("ip", &["link", "set", "dev", &ifname, "down"])
    }
}

/// Keeps a saved "off" applied. A USB mode switch in progress owns the bridge until it
/// finishes, so the check waits for it.
pub fn start_usb_lan_enforcer() {
    thread::spawn(|| loop {
        thread::sleep(USB_LAN_ENFORCE_EVERY);
        if USB_BUSY.load(Ordering::Acquire) {
            continue;
        }
        let Ok(_lock) = USB_LAN_LOCK.try_lock() else {
            continue;
        };
        let view = usb_lan_view();
        let shared = view.interface.as_deref().is_some_and(|ifname| {
            interface_up(ifname) || bridge_members(LAN_BRIDGE).iter().any(|m| m == ifname)
        });
        if !view.enabled && shared {
            if let Err(e) = apply_usb_lan(false) {
                eprintln!("[usb] USB LAN off: {e}");
            }
        }
    });
}

/// GET /api/usb/lan
pub fn usb_lan_get() -> (u16, Value) {
    (
        200,
        json!({"ok": true, "data": usb_lan_json(&usb_lan_view())}),
    )
}

/// PUT /api/usb/lan `{"enabled": bool}`: save the choice and apply it now.
pub fn usb_lan_set(body: &[u8]) -> (u16, Value) {
    let enabled = match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(m)) if m.len() == 1 => match m.get("enabled") {
            Some(Value::Bool(b)) => *b,
            _ => {
                return (
                    400,
                    json!({"ok": false, "error": "enabled must be true or false"}),
                )
            }
        },
        _ => {
            return (
                400,
                json!({"ok": false, "error": "expected {\"enabled\": bool}"}),
            )
        }
    };
    let _lock = USB_LAN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = match SwitchGuard::acquire() {
        Ok(guard) => guard,
        Err(error) => return (409, json!({"ok": false, "error": error})),
    };
    let mut persisted = read_persisted_config();
    if !persisted.is_object() {
        persisted = json!({});
    }
    if let Some(obj) = persisted.as_object_mut() {
        obj.insert(USB_LAN_KEY.into(), json!(enabled));
    }
    if let Err(e) = write_persisted_config(&persisted) {
        return (500, json!({"ok": false, "error": e}));
    }
    if let Err(e) = apply_usb_lan(enabled) {
        return (500, json!({"ok": false, "error": e}));
    }
    let view = usb_lan_view();
    if view.interface.is_some() && view.active != enabled {
        return (
            500,
            json!({"ok": false, "error": "USB LAN state did not change"}),
        );
    }
    (200, json!({"ok": true, "data": usb_lan_json(&view)}))
}

#[cfg(test)]
mod tests {
    use super::parse_usb_default_mode;
    use serde_json::json;

    #[test]
    fn usb_default_mode_accepts_supported_modes() {
        assert_eq!(
            parse_usb_default_mode(&json!({"usb_default_mode": "ncm"})),
            Some("ncm")
        );
        assert_eq!(
            parse_usb_default_mode(&json!({"usb_default_mode": "ECM"})),
            Some("ecm")
        );
    }

    #[test]
    fn usb_default_mode_ignores_unknown_values() {
        assert_eq!(parse_usb_default_mode(&json!({})), None);
        assert_eq!(
            parse_usb_default_mode(&json!({"usb_default_mode": "rndis"})),
            None
        );
        assert_eq!(
            parse_usb_default_mode(&json!({"usb_default_mode": true})),
            None
        );
    }

    #[test]
    fn usb_lan_defaults_on_and_reads_only_booleans() {
        use super::parse_usb_lan_enabled;
        assert!(parse_usb_lan_enabled(&json!({})));
        assert!(parse_usb_lan_enabled(&json!({"usb_lan_enabled": true})));
        assert!(!parse_usb_lan_enabled(&json!({"usb_lan_enabled": false})));
        assert!(parse_usb_lan_enabled(&json!({"usb_lan_enabled": "false"})));
        assert!(!parse_usb_lan_enabled(
            &json!({"usb_default_mode": "ncm", "usb_lan_enabled": false})
        ));
    }

    #[test]
    fn usb_speed_label_maps_known_speeds() {
        assert_eq!(
            super::usb_speed_label("high-speed"),
            Some(("USB 2.0", 480.0))
        );
        assert_eq!(
            super::usb_speed_label("super-speed-plus"),
            Some(("USB 3.1 Gen2", 10000.0))
        );
        assert_eq!(super::usb_speed_label("UNKNOWN"), None);
    }

    #[test]
    fn power_off_charging_matches_stock_guard() {
        assert!(super::is_power_off_charging_state(Some(
            "mode_power_off_charger"
        )));
        assert!(super::is_power_off_charging_state(Some(
            "mode_power_off_unreal"
        )));
        assert!(!super::is_power_off_charging_state(Some(
            "mode_power_on_charger"
        )));
        assert!(!super::is_power_off_charging_state(Some("mode_power_on")));
        assert!(!super::is_power_off_charging_state(None));
    }
}

fn preflight_ncm_switch() -> Result<(), String> {
    if !Path::new(GADGET_DIR).exists() {
        return Err("USB gadget configfs is not mounted".into());
    }
    if !Path::new(NCM_FUNC).exists() {
        return Err("NCM configfs function is not available on this firmware".into());
    }
    if !Path::new(MASS_STORAGE_FUNC).exists() {
        return Err(
            "mass_storage.0 function is missing; refusing to build partial composition".into(),
        );
    }
    if first_udc_name().is_none() && read_trimmed(UDC_PATH).is_none() {
        return Err("no USB device controller is available".into());
    }
    Ok(())
}

fn preflight_ecm_switch() -> Result<(), String> {
    if !Path::new(GADGET_DIR).exists() {
        return Err("USB gadget configfs is not mounted".into());
    }
    if !Path::new(ECM_GSI_FUNC).exists() {
        return Err("GSI ECM configfs function is not available on this firmware".into());
    }
    if !Path::new(MASS_STORAGE_FUNC).exists() {
        return Err(
            "mass_storage.0 function is missing; refusing to build partial composition".into(),
        );
    }
    if first_udc_name().is_none() && read_trimmed(UDC_PATH).is_none() {
        return Err("no USB device controller is available".into());
    }
    Ok(())
}

fn schedule_switch(mode: &'static str) -> Result<(), String> {
    let guard = SwitchGuard::acquire()?;
    thread::spawn(move || {
        let _guard = guard;
        thread::sleep(Duration::from_millis(1000));
        match transactional_switch(mode) {
            Ok(()) => {
                let _ = fs::remove_file(NCM_LAST_ERROR_PATH);
            }
            Err(error) => {
                let _ = fs::write(NCM_LAST_ERROR_PATH, error);
            }
        }
    });
    Ok(())
}

/// Preserve the exact composition (including any firmware ADB/diagnostic links),
/// attributes and USB bridge membership before unbinding anything.
struct GadgetSnapshot {
    udc: String,
    attributes: Vec<(String, String)>,
    links: Vec<(PathBuf, PathBuf)>,
    bridge: Vec<String>,
}
impl GadgetSnapshot {
    fn to_json(&self) -> Value {
        json!({
            "udc": self.udc,
            "attributes": self.attributes,
            "links": self.links.iter().map(|(p, t)| (p.to_string_lossy(), t.to_string_lossy())).collect::<Vec<_>>(),
            "bridge": self.bridge,
        })
    }
    fn from_json(v: &Value) -> Option<Self> {
        let pairs = |key: &str| -> Option<Vec<(String, String)>> {
            v[key]
                .as_array()?
                .iter()
                .map(|p| Some((p[0].as_str()?.to_string(), p[1].as_str()?.to_string())))
                .collect()
        };
        let links: Vec<(PathBuf, PathBuf)> = pairs("links")?
            .into_iter()
            .map(|(p, t)| (PathBuf::from(p), PathBuf::from(t)))
            .collect();
        // Only links inside this gadget's configuration are ever restored.
        if links.is_empty() || !links.iter().all(|(p, _)| p.starts_with(CONFIG_DIR)) {
            return None;
        }
        Some(Self {
            udc: v["udc"].as_str().filter(|s| !s.is_empty())?.to_string(),
            attributes: pairs("attributes")?
                .into_iter()
                .filter(|(p, _)| p.starts_with(GADGET_DIR))
                .collect(),
            links,
            bridge: v["bridge"]
                .as_array()?
                .iter()
                .filter_map(|m| m.as_str().map(str::to_string))
                .filter(|m| is_usb_interface(m))
                .collect(),
        })
    }
    fn capture() -> Result<Self, String> {
        let udc = fs::read_to_string(UDC_PATH).map_err(|e| format!("snapshot UDC: {e}"))?;
        let mut attributes = Vec::new();
        for suffix in [
            "idVendor",
            "idProduct",
            "strings/0x409/product",
            "configs/c.1/strings/0x409/configuration",
            "bDeviceClass",
            "os_desc/use",
            "os_desc/b_vendor_code",
            "os_desc/qw_sign",
        ] {
            let path = format!("{GADGET_DIR}/{suffix}");
            if Path::new(&path).exists() {
                attributes.push((
                    path.clone(),
                    fs::read_to_string(&path)
                        .map_err(|e| format!("snapshot {path}: {e}"))?
                        .trim()
                        .into(),
                ));
            }
        }
        let mut links = Vec::new();
        for entry in fs::read_dir(CONFIG_DIR).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name().to_string_lossy().starts_with('f')
                && entry.file_type().map_err(|e| e.to_string())?.is_symlink()
            {
                links.push((
                    entry.path(),
                    fs::read_link(entry.path()).map_err(|e| e.to_string())?,
                ));
            }
        }
        if links.is_empty() {
            return Err("cannot snapshot an empty USB composition".into());
        }
        Ok(Self {
            udc: udc.trim().into(),
            attributes,
            links,
            bridge: checked_bridge_members("br-lan")?
                .into_iter()
                .filter(|s| is_usb_interface(s))
                .collect(),
        })
    }
    fn restore(&self) -> Result<(), String> {
        self.restore_with(&RestoreHardware)
    }
    fn restore_with(&self, io: &impl RestoreIo) -> Result<(), String> {
        let mut errors = Vec::new();
        let mut record = |result: Result<(), String>| {
            if let Err(error) = result {
                errors.push(error);
            }
        };
        record(io.write(UDC_PATH, ""));
        record(io.remove_links());
        for (path, value) in &self.attributes {
            record(io.write(path, value));
        }
        for (path, target) in &self.links {
            record(io.link(target, path));
        }
        // Try rebinding even if an earlier restore step failed.
        record(io.write(UDC_PATH, &self.udc));
        let members = match io.members() {
            Ok(members) => members,
            Err(e) => {
                record(Err(e));
                vec![]
            }
        };
        for member in members.into_iter().filter(|s| is_usb_interface(s)) {
            if !self.bridge.contains(&member) {
                record(io.remove_member(&member));
            }
        }
        for member in &self.bridge {
            record(io.restore_member(member));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}
trait RestoreIo {
    fn write(&self, path: &str, value: &str) -> Result<(), String>;
    fn remove_links(&self) -> Result<(), String>;
    fn link(&self, target: &Path, path: &Path) -> Result<(), String>;
    fn members(&self) -> Result<Vec<String>, String>;
    fn remove_member(&self, member: &str) -> Result<(), String>;
    fn restore_member(&self, member: &str) -> Result<(), String>;
}
struct RestoreHardware;
impl RestoreIo for RestoreHardware {
    fn write(&self, path: &str, value: &str) -> Result<(), String> {
        if path == UDC_PATH && !value.is_empty() {
            return bind_udc(value).map_err(|e| format!("restore {e}"));
        }
        fs::write(path, value).map_err(|e| format!("restore {path}: {e}"))
    }
    fn remove_links(&self) -> Result<(), String> {
        remove_config_links()
    }
    fn link(&self, target: &Path, path: &Path) -> Result<(), String> {
        unix_fs::symlink(target, path).map_err(|e| format!("restore {}: {e}", path.display()))
    }
    fn members(&self) -> Result<Vec<String>, String> {
        checked_bridge_members("br-lan")
    }
    fn remove_member(&self, member: &str) -> Result<(), String> {
        remove_bridge_member("br-lan", member)
    }
    fn restore_member(&self, member: &str) -> Result<(), String> {
        wait_for_interface(member, Duration::from_secs(10))
            .ok_or_else(|| format!("restore interface {member} did not appear"))?;
        run_command("ifconfig", &[member, "up"])?;
        add_bridge_member("br-lan", member)
    }
}

fn is_usb_interface(name: &str) -> bool {
    ["ecm", "rndis", "ncm", "usb"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}
fn with_rollback(
    mut apply: impl FnMut() -> Result<(), String>,
    mut restore: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    match apply() {
        Ok(()) => Ok(()),
        Err(error) => match restore() {
            Ok(()) => Err(format!("{error}; previous USB composition restored")),
            Err(rollback) => Err(format!(
                "{error}; USB restoration failed: {rollback}; reboot may be required"
            )),
        },
    }
}
/// Truth about existing exposure only, never a request to add a USB function.
pub(crate) fn adb_exposed() -> bool {
    if read_trimmed(UDC_PATH).is_none_or(|s| s.is_empty()) {
        return false;
    }
    fs::read_dir(CONFIG_DIR).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_name().to_string_lossy().starts_with('f')
                && entry.file_type().is_ok_and(|t| t.is_symlink())
                && fs::canonicalize(entry.path()).is_ok_and(|p| p == Path::new(ADB_FUNC))
        })
    })
}
/// Pure layout check. Preserve the original link name/target, never relocate it into
/// a free slot. f1/f2 belong to the existing fixed network/mass-storage layout.
fn adb_preservation_layout(
    links: &[(PathBuf, PathBuf)],
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut preserved = Vec::new();
    for (path, target) in links {
        if target.file_name().is_none_or(|name| name != "ffs.adb") {
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let number = name
            .strip_prefix('f')
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok());
        if path.parent() != Some(Path::new(CONFIG_DIR)) || number.is_none_or(|n| n <= 2) {
            return Err(
                "existing ADB link conflicts with network layout; USB left unchanged".into(),
            );
        }
        preserved.push((path.clone(), target.clone()));
    }
    if preserved.len() > 1 {
        return Err("multiple ADB links; USB left unchanged".into());
    }
    Ok(preserved)
}
fn validated_adb_links(snapshot: &GadgetSnapshot) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let preserved = adb_preservation_layout(&snapshot.links)?;
    for (path, _) in &preserved {
        if fs::canonicalize(path).map_err(|_| "existing ADB target unavailable")?
            != Path::new(ADB_FUNC)
        {
            return Err("unexpected ADB target; USB left unchanged".into());
        }
    }
    Ok(preserved)
}
fn restore_preserved_adb(links: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    for (path, target) in links {
        unix_fs::symlink(target, path).map_err(|e| format!("preserve existing ADB link: {e}"))?;
    }
    Ok(())
}
fn transactional_switch(mode: &str) -> Result<(), String> {
    let snapshot = GadgetSnapshot::capture()?;
    // Validate collisions and canonical target BEFORE unbinding anything. No function
    // is created; the successful rebuild carries forward only already exposed ADB.
    let adb_links = validated_adb_links(&snapshot)?;
    with_rollback(
        || match mode {
            "ncm" => switch_to_ncm_now(&adb_links),
            "rndis" => load_boot_snapshot()
                .ok_or_else(|| "boot USB composition was not recorded".to_string())?
                .restore(),
            _ => switch_to_ecm_now(&adb_links),
        },
        || snapshot.restore(),
    )
}

fn switch_to_ncm_now(adb_links: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    preflight_ncm_switch()?;
    let udc = read_trimmed(UDC_PATH)
        .or_else(first_udc_name)
        .unwrap_or_default();

    fs::write(UDC_PATH, "").map_err(|e| format!("unbind UDC: {e}"))?;
    remove_config_links()?;

    write_if_exists(&format!("{GADGET_DIR}/idVendor"), "0x19d2")?;
    write_if_exists(&format!("{GADGET_DIR}/idProduct"), "0x1406")?;
    write_if_exists(
        &format!("{GADGET_DIR}/strings/0x409/product"),
        "ZTE Mobile Broadband",
    )?;
    write_if_exists(
        &format!("{CONFIG_DIR}/strings/0x409/configuration"),
        "NCM_MASS_STORAGE",
    )?;
    write_if_exists(&format!("{GADGET_DIR}/bDeviceClass"), "0x02")?;
    write_if_exists(&format!("{GADGET_DIR}/os_desc/use"), "1")?;
    write_if_exists(&format!("{GADGET_DIR}/os_desc/b_vendor_code"), "0x04")?;
    write_if_exists(&format!("{GADGET_DIR}/os_desc/qw_sign"), "MSFT100")?;

    unix_fs::symlink(NCM_FUNC, format!("{CONFIG_DIR}/f1"))
        .map_err(|e| format!("link ncm.0: {e}"))?;
    unix_fs::symlink(MASS_STORAGE_FUNC, format!("{CONFIG_DIR}/f2"))
        .map_err(|e| format!("link mass_storage.0: {e}"))?;
    restore_preserved_adb(adb_links)?;

    bind_udc(&udc)?;
    let ifname = wait_for_ncm_interface(Duration::from_secs(10))
        .ok_or_else(|| "NCM interface did not appear after binding".to_string())?;
    run_command("ifconfig", &[&ifname, "up"])?;
    add_bridge_member("br-lan", &ifname)?;
    Ok(())
}

fn switch_to_ecm_now(adb_links: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    preflight_ecm_switch()?;
    let udc = read_trimmed(UDC_PATH)
        .or_else(first_udc_name)
        .unwrap_or_default();
    let ncm_ifaces = ncm_bridge_candidates();

    for ifname in &ncm_ifaces {
        remove_bridge_member("br-lan", ifname)?;
    }

    fs::write(UDC_PATH, "").map_err(|e| format!("unbind UDC: {e}"))?;
    remove_config_links()?;

    write_if_exists(&format!("{GADGET_DIR}/idVendor"), "0x19d2")?;
    write_if_exists(&format!("{GADGET_DIR}/idProduct"), "0x1405")?;
    write_if_exists(
        &format!("{GADGET_DIR}/strings/0x409/product"),
        "ZTE Mobile Broadband",
    )?;
    write_if_exists(
        &format!("{CONFIG_DIR}/strings/0x409/configuration"),
        "ECM_MASS_STORAGE",
    )?;
    write_if_exists(&format!("{GADGET_DIR}/bDeviceClass"), "0x02")?;
    write_if_exists(&format!("{GADGET_DIR}/os_desc/use"), "1")?;
    write_if_exists(&format!("{GADGET_DIR}/os_desc/b_vendor_code"), "0x04")?;
    write_if_exists(&format!("{GADGET_DIR}/os_desc/qw_sign"), "MSFT100")?;

    unix_fs::symlink(ECM_GSI_FUNC, format!("{CONFIG_DIR}/f1"))
        .map_err(|e| format!("link gsi.ecm: {e}"))?;
    unix_fs::symlink(MASS_STORAGE_FUNC, format!("{CONFIG_DIR}/f2"))
        .map_err(|e| format!("link mass_storage.0: {e}"))?;
    restore_preserved_adb(adb_links)?;

    bind_udc(&udc)?;
    wait_for_interface("ecm0", Duration::from_secs(10))
        .ok_or_else(|| "ECM interface did not appear after binding".to_string())?;
    run_command("ifconfig", &["ecm0", "up"])?;
    add_bridge_member("br-lan", "ecm0")?;
    for ifname in ncm_ifaces {
        remove_bridge_member("br-lan", &ifname)?;
    }
    Ok(())
}

/// Bind the gadget to its controller. Binding fails while a FunctionFS function (ADB)
/// has no daemon ready; adbd reopens it after the unbind, so retry for a few seconds
/// instead of failing the switch on the first attempt.
fn bind_udc(udc: &str) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..20 {
        match fs::write(UDC_PATH, udc) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!("bind UDC {udc}: {last}"))
}
fn first_udc_name() -> Option<String> {
    fs::read_dir("/sys/class/udc")
        .ok()?
        .flatten()
        .find_map(|entry| entry.file_name().to_str().map(|s| s.to_string()))
}

fn write_if_exists(path: &str, value: &str) -> Result<(), String> {
    if Path::new(path).exists() {
        fs::write(path, value).map_err(|e| format!("write {path}: {e}"))?;
    }
    Ok(())
}

fn remove_config_links() -> Result<(), String> {
    let entries = fs::read_dir(CONFIG_DIR).map_err(|e| format!("read {CONFIG_DIR}: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('f') && fs::read_link(entry.path()).is_ok() {
            fs::remove_file(entry.path()).map_err(|e| format!("remove {name}: {e}"))?;
        }
    }
    Ok(())
}

fn wait_for_ncm_interface(timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(ifname) = ncm_interface_name() {
            return Some(ifname);
        }
        if Path::new("/sys/class/net/ncm0").exists() {
            return Some("ncm0".into());
        }
        if current_composition_functions().iter().any(|f| f == "ncm.0")
            && Path::new("/sys/class/net/usb0").exists()
        {
            return Some("usb0".into());
        }
        thread::sleep(Duration::from_millis(250));
    }
    None
}

fn ncm_bridge_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(ifname) = read_trimmed(&format!("{NCM_FUNC}/ifname")) {
        if ifname != "(unnamed net_device)" {
            candidates.push(ifname);
        }
    }
    candidates.push("ncm0".into());
    candidates.push("usb0".into());
    candidates.sort();
    candidates.dedup();
    candidates
}

fn wait_for_interface(ifname: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    let path = format!("/sys/class/net/{ifname}");
    while Instant::now() < deadline {
        if Path::new(&path).exists() {
            return Some(ifname.to_string());
        }
        thread::sleep(Duration::from_millis(250));
    }
    None
}

fn add_bridge_member(bridge: &str, ifname: &str) -> Result<(), String> {
    if bridge_members(bridge).iter().any(|member| member == ifname) {
        return Ok(());
    }
    run_command("brctl", &["addif", bridge, ifname])
}

fn remove_bridge_member(bridge: &str, ifname: &str) -> Result<(), String> {
    if !bridge_members(bridge).iter().any(|member| member == ifname) {
        return Ok(());
    }
    run_command("brctl", &["delif", bridge, ifname])
}

fn run_command(command: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(command)
        .args(args)
        .bounded_output()
        .map_err(|e| format!("{command} exec: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "{command} {} failed: {}",
        args.join(" "),
        stderr.trim()
    ))
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    #[test]
    fn firmware_compositions_keep_adb_only_with_its_daemon() {
        assert_eq!(
            firmware_functions("ecm", true),
            Some(("0x1405", "ecm_gsi,mass_storage,ffs".to_string()))
        );
        assert_eq!(
            firmware_functions("ecm", false),
            Some(("0x1405", "ecm_gsi,mass_storage".to_string()))
        );
        assert_eq!(
            firmware_functions("rndis", true).unwrap().1,
            "rndis_gsi,diag,serial,modem,mass_storage,ffs,dpl,qdss"
        );
        assert!(!firmware_functions("rndis", false)
            .unwrap()
            .1
            .contains("ffs"));
        assert_eq!(firmware_functions("ncm", true), None);
    }

    #[test]
    fn boot_snapshot_round_trips_and_stays_inside_the_gadget() {
        let snap = GadgetSnapshot {
            udc: "a600000.dwc3".into(),
            attributes: vec![(format!("{GADGET_DIR}/idProduct"), "0x1404".into())],
            links: vec![
                (
                    Path::new(CONFIG_DIR).join("f1"),
                    PathBuf::from(RNDIS_GSI_FUNC),
                ),
                (Path::new(CONFIG_DIR).join("f6"), PathBuf::from(ADB_FUNC)),
            ],
            bridge: vec!["rndis0".into()],
        };
        let back = GadgetSnapshot::from_json(&snap.to_json()).unwrap();
        assert_eq!(
            (back.udc.as_str(), back.links.len(), back.bridge.clone()),
            ("a600000.dwc3", 2, vec!["rndis0".to_string()])
        );
        assert_eq!(back.attributes, snap.attributes);
        let mut v = snap.to_json();
        v["links"] = json!([["/etc/passwd", "x"]]);
        assert!(GadgetSnapshot::from_json(&v).is_none());
        let mut v = snap.to_json();
        v["udc"] = json!("");
        assert!(GadgetSnapshot::from_json(&v).is_none());
        let mut v = snap.to_json();
        v["attributes"] = json!([["/etc/shadow", "x"]]);
        assert!(GadgetSnapshot::from_json(&v).unwrap().attributes.is_empty());
    }

    #[test]
    fn preserve_only_existing_adb_without_layout_collisions() {
        let link = |n: &str, target: &str| (Path::new(CONFIG_DIR).join(n), PathBuf::from(target));
        let adb = link("f6", "../../functions/ffs.adb");
        assert_eq!(
            adb_preservation_layout(&[
                link("f1", ECM_GSI_FUNC),
                link("f2", MASS_STORAGE_FUNC),
                adb.clone(),
            ])
            .unwrap(),
            vec![adb.clone()]
        );
        assert!(adb_preservation_layout(&[link("f1", ADB_FUNC)]).is_err());
        assert!(adb_preservation_layout(&[link("f2", ADB_FUNC)]).is_err());
        assert!(adb_preservation_layout(&[link("f-1", ADB_FUNC)]).is_err());
        assert!(adb_preservation_layout(&[link("f3", ADB_FUNC), adb]).is_err());
        assert!(adb_preservation_layout(&[link("f1", ECM_GSI_FUNC)])
            .unwrap()
            .is_empty());
        assert!(
            adb_preservation_layout(&[(PathBuf::from("/tmp/f6"), PathBuf::from(ADB_FUNC))])
                .is_err()
        );
    }
    #[test]
    fn restoration_attempts_exact_composition_and_bridge_after_each_failure() {
        use std::cell::RefCell;
        struct Fake {
            actions: RefCell<Vec<String>>,
            fail_at: usize,
        }
        impl Fake {
            fn action(&self, action: String) -> Result<(), String> {
                let mut actions = self.actions.borrow_mut();
                actions.push(action);
                if actions.len() == self.fail_at {
                    Err("injected restore failure".into())
                } else {
                    Ok(())
                }
            }
        }
        impl RestoreIo for Fake {
            fn write(&self, path: &str, value: &str) -> Result<(), String> {
                self.action(format!("write {path}={value}"))
            }
            fn remove_links(&self) -> Result<(), String> {
                self.action("remove links".into())
            }
            fn link(&self, target: &Path, path: &Path) -> Result<(), String> {
                self.action(format!("link {} -> {}", path.display(), target.display()))
            }
            fn members(&self) -> Result<Vec<String>, String> {
                Ok(vec!["ncm0".into(), "wlan0".into()])
            }
            fn remove_member(&self, member: &str) -> Result<(), String> {
                self.action(format!("remove {member}"))
            }
            fn restore_member(&self, member: &str) -> Result<(), String> {
                self.action(format!("restore {member}"))
            }
        }
        let snapshot = GadgetSnapshot {
            udc: "original-controller".into(),
            attributes: vec![("product".into(), "original-product".into())],
            links: vec![
                (PathBuf::from("f1"), PathBuf::from("ecm")),
                (PathBuf::from("f2"), PathBuf::from("adb")),
            ],
            bridge: vec!["ecm0".into()],
        };
        let expected = vec![
            format!("write {UDC_PATH}="),
            "remove links".into(),
            "write product=original-product".into(),
            "link f1 -> ecm".into(),
            "link f2 -> adb".into(),
            format!("write {UDC_PATH}=original-controller"),
            "remove ncm0".into(),
            "restore ecm0".into(),
        ];
        for fail_at in 0..=expected.len() {
            let fake = Fake {
                actions: RefCell::default(),
                fail_at,
            };
            assert_eq!(snapshot.restore_with(&fake).is_err(), fail_at > 0);
            assert_eq!(*fake.actions.borrow(), expected);
        }
    }
    #[test]
    fn readiness_timeout_is_a_failure() {
        assert!(
            wait_until_ready(Duration::from_millis(5), Duration::from_millis(1), || false).is_err()
        );
        assert!(
            wait_until_ready(Duration::from_millis(50), Duration::from_millis(1), || true).is_ok()
        );
    }
    #[test]
    fn only_one_switch_can_be_reserved() {
        let guard = SwitchGuard::acquire().unwrap();
        assert!(SwitchGuard::acquire().is_err());
        drop(guard);
        assert!(SwitchGuard::acquire().is_ok());
    }
    #[test]
    fn failure_restores_and_reports_restoration_failure() {
        let restored = std::cell::Cell::new(false);
        assert!(with_rollback(
            || Err("bind failed".into()),
            || {
                restored.set(true);
                Ok(())
            }
        )
        .unwrap_err()
        .contains("restored"));
        assert!(restored.get());
        assert!(with_rollback(
            || Err("bind failed".into()),
            || Err("restore failed".into())
        )
        .unwrap_err()
        .contains("restoration failed"));
    }
}
