//! Owned native ADB/SSH lifecycle only. No credentials, gadget writes or generic kills.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, Once, RwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SETTINGS: &str = "/etc/openui-admin-services.json";
const HELPER: &str = "/etc/openui-admin-services-boot.sh";
const MARKER: &str = "# OpenUI admin-services boot guard v1";
// Frozen with the parent's separately validated native helper, not an arbitrary shell hook.
const HELPER_SHA256: &str = "8147ddc047ddbb8b20db21b62e8c9020a44fad357f512749d541d2b8bb21fd43";
const MAX_CONFIG: u64 = 4096;
const MAX_REVISION: u64 = 9_007_199_254_740_991; // JSON/JavaScript exact integer ceiling.
const SSH_ARGS: &[&str] = &[
    "/data/mu5250-admin/bin/dropbear",
    "-F",
    "-E",
    "-T",
    "3",
    "-K",
    "60",
    "-I",
    "1800",
    "-p",
    "127.0.0.1:22",
    "-p",
    "192.168.0.1:22",
    "-r",
    "/etc/dropbear/dropbear_ed25519_host_key",
    "-P",
    "/var/run/mu5250-ssh.pid",
];
static SERIAL: Mutex<()> = Mutex::new(());
static BUSY: AtomicBool = AtomicBool::new(false);
static CACHE: RwLock<Option<Snapshot>> = RwLock::new(None);
static START: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Adb,
    Ssh,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Runtime,
    Boot,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub revision: u64,
    pub service: Service,
    pub action: Action,
    pub enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    revision: u64,
    adb_boot: bool,
    ssh_boot: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: 0,
            adb_boot: true,
            ssh_boot: true,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub revision: u64,
    pub stamp: u64,
    pub busy: bool,
    pub adb_running: bool,
    pub adb_exposed: bool,
    pub adb_boot: bool,
    pub ssh_running: bool,
    pub ssh_boot: bool,
    pub adb_available: bool,
    pub ssh_available: bool,
}
impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::Adb => "adbd.init",
            Self::Ssh => "mu5250-ssh",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Adb => "adb",
            Self::Ssh => "ssh",
        }
    }
    fn init(self) -> &'static str {
        match self {
            Self::Adb => "/etc/init.d/adbd.init",
            Self::Ssh => "/etc/init.d/mu5250-ssh",
        }
    }
    fn boot_link(self) -> &'static str {
        match self {
            Self::Adb => "/etc/rc.d/S10adbd.init",
            Self::Ssh => "/etc/rc.d/S99mu5250-ssh",
        }
    }
    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Adb => &["/sbin/adbd"],
            Self::Ssh => SSH_ARGS,
        }
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn failure(code: u16, error: &'static str) -> (u16, Value) {
    (code, json!({"ok":false,"error":error}))
}
fn parse_settings(bytes: &[u8]) -> Result<Settings, &'static str> {
    // The native boot helper has the same three-key schema and deliberately rejects
    // escaped key spellings. Settings contain no string values needing escapes.
    if bytes.len() > MAX_CONFIG as usize || bytes.contains(&b'\\') {
        return Err("Admin settings invalid");
    }
    let s: Settings = serde_json::from_slice(bytes).map_err(|_| "Admin settings invalid")?;
    if s.revision > MAX_REVISION {
        return Err("Admin settings revision invalid");
    }
    Ok(s)
}
/// No symlink following on the security authority; missing is the only default case.
fn load() -> Result<Settings, &'static str> {
    let mut f = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(SETTINGS)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(_) => return Err("Admin settings unavailable"),
    };
    let m = f.metadata().map_err(|_| "Admin settings unavailable")?;
    if !m.is_file() || m.uid() != 0 || m.permissions().mode() & 0o077 != 0 {
        return Err("Admin settings ownership or mode invalid");
    }
    let mut bytes = Vec::new();
    (&mut f)
        .take(MAX_CONFIG + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Admin settings unavailable")?;
    parse_settings(&bytes)
}
fn save(s: &Settings) -> Result<(), &'static str> {
    // Refuse an existing symlink/non-file even though atomic rename itself would replace it.
    match fs::symlink_metadata(SETTINGS) {
        Ok(m) if !m.is_file() || m.uid() != 0 || m.permissions().mode() & 0o077 != 0 => {
            return Err("Admin settings path collision")
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err("Admin settings unavailable")
        }
        _ => {}
    }
    let bytes = serde_json::to_vec(s).map_err(|_| "Cannot encode admin settings")?;
    crate::storage::atomic_write(Path::new(SETTINGS), &bytes)
        .map_err(|_| "Cannot persist admin settings")
}
fn secure_regular(path: &str, executable: bool) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        m.is_file()
            && m.uid() == 0
            && m.permissions().mode() & 0o022 == 0
            && (!executable || m.permissions().mode() & 0o111 != 0)
    })
}
fn guard_line(service: Service) -> String {
    format!(
        "[ \"${{OPENUI_SERVICE_MANUAL:-0}}\" = 1 ] || /bin/sh {HELPER} {} || return 0",
        service.key()
    )
}
fn boot_target_valid(service: Service, target: &Path) -> bool {
    target == Path::new(service.init())
        || target == Path::new(&format!("../init.d/{}", service.name()))
}
fn hooks_available(service: Service) -> bool {
    if !secure_regular(HELPER, true)
        || !secure_regular(service.init(), true)
        || !secure_regular(service.args()[0], true)
    {
        return false;
    }
    let Ok(helper) = fs::read_to_string(HELPER) else {
        return false;
    };
    let Ok(init) = fs::read_to_string(service.init()) else {
        return false;
    };
    let expected = guard_line(service);
    if format!("{:x}", Sha256::digest(helper.as_bytes())) != HELPER_SHA256
        || !helper.lines().any(|l| l.trim() == MARKER)
        || !init.lines().any(|l| l.trim() == MARKER)
        || !init.lines().any(|l| l.trim() == expected)
        || !init.lines().any(|l| l.trim() == "USE_PROCD=1")
    {
        return false;
    }
    // Installed boot hooks alone do not prove that the firmware will invoke them.
    fs::symlink_metadata(service.boot_link())
        .is_ok_and(|m| m.file_type().is_symlink() && m.uid() == 0)
        && fs::read_link(service.boot_link()).is_ok_and(|p| boot_target_valid(service, &p))
}
fn command_matches(service: Service, command: &Value) -> bool {
    command.as_array().is_some_and(|a| {
        a.len() == service.args().len()
            && a.iter()
                .zip(service.args())
                .all(|(a, b)| a.as_str() == Some(*b))
    })
}
/// Pure parser: no wildcard service/instance/PID fallback is permitted.
fn procd_owner(service: Service, response: &Value) -> Result<Option<u32>, &'static str> {
    let all = response
        .as_object()
        .ok_or("Invalid native service response")?;
    if all.is_empty() {
        return Ok(None);
    }
    if all.len() != 1 {
        return Err("Unexpected native service response");
    }
    let object = all
        .get(service.name())
        .and_then(Value::as_object)
        .ok_or("Unexpected native service owner")?;
    let instances = object
        .get("instances")
        .and_then(Value::as_object)
        .ok_or("Invalid native service instances")?;
    if instances.is_empty() {
        return Ok(None);
    }
    if instances.len() != 1 {
        return Err("Unexpected native service instances");
    }
    let instance = instances
        .get("instance1")
        .ok_or("Unexpected native service instance")?;
    if !command_matches(service, &instance["command"]) {
        return Err("Native service command changed");
    }
    match instance["running"].as_bool() {
        Some(false) => Ok(None),
        Some(true) => instance["pid"]
            .as_u64()
            .filter(|p| *p > 1 && *p <= i32::MAX as u64)
            .map(|p| Some(p as u32))
            .ok_or("Invalid native service PID"),
        None => Err("Native service state unknown"),
    }
}
fn query_owner(service: Service) -> Result<Option<u32>, &'static str> {
    let payload = json!({"name":service.name()}).to_string();
    let out = process_runner::output(
        Command::new("/bin/ubus").args(["-t", "2", "call", "service", "list", &payload]),
        None,
        Duration::from_secs(3),
        64 * 1024,
    )
    .map_err(|_| "Native service status unavailable")?;
    if !out.status.success() {
        return Err("Native service status unavailable");
    }
    let v: Value =
        serde_json::from_slice(&out.stdout).map_err(|_| "Invalid native service response")?;
    let pid = procd_owner(service, &v)?;
    if let Some(pid) = pid {
        let exe = fs::read_link(format!("/proc/{pid}/exe"))
            .map_err(|_| "Native service owner disappeared")?;
        let cmd = fs::read(format!("/proc/{pid}/cmdline"))
            .map_err(|_| "Native service owner unavailable")?;
        if exe != Path::new(service.args()[0]) || !argv_matches(service, &cmd) {
            return Err("Native service process ownership mismatch");
        }
    }
    Ok(pid)
}
fn argv_matches(service: Service, bytes: &[u8]) -> bool {
    let mut fields: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    if fields.last() == Some(&&b""[..]) {
        fields.pop();
    }
    fields.len() == service.args().len()
        && fields
            .iter()
            .zip(service.args())
            .all(|(a, b)| *a == b.as_bytes())
}
/// If an exact daemon command exists outside the verified procd owner, never start a
/// second listener or send a stop aimed at a different process. SSH session children
/// with different argv are deliberately neither selected nor signalled.
fn reject_unowned(service: Service, owner: Option<u32>) -> Result<(), &'static str> {
    for entry in fs::read_dir("/proc").map_err(|_| "Process ownership unavailable")? {
        let entry = entry.map_err(|_| "Process ownership unavailable")?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if Some(pid) == owner {
            continue;
        }
        if fs::read_link(entry.path().join("exe")).is_ok_and(|p| p == Path::new(service.args()[0]))
            && fs::read(entry.path().join("cmdline")).is_ok_and(|v| argv_matches(service, &v))
        {
            return Err("Unowned native service process present");
        }
    }
    Ok(())
}
fn scan(settings: &Settings) -> Result<Snapshot, &'static str> {
    let adb = query_owner(Service::Adb)?;
    let ssh = query_owner(Service::Ssh)?;
    // An unowned exact daemon must not be rendered as a known OFF native service.
    reject_unowned(Service::Adb, adb)?;
    reject_unowned(Service::Ssh, ssh)?;
    Ok(Snapshot {
        revision: settings.revision,
        stamp: now(),
        busy: BUSY.load(Ordering::Acquire),
        adb_running: adb.is_some(),
        adb_exposed: crate::usb::adb_exposed(),
        adb_boot: settings.adb_boot,
        ssh_running: ssh.is_some(),
        ssh_boot: settings.ssh_boot,
        adb_available: hooks_available(Service::Adb),
        ssh_available: hooks_available(Service::Ssh),
    })
}
fn publish(value: Option<Snapshot>) {
    *CACHE.write().unwrap_or_else(|p| p.into_inner()) = value;
}
fn refresh_locked() -> Result<Snapshot, &'static str> {
    let result = load().and_then(|s| scan(&s));
    publish(result.as_ref().ok().cloned());
    result
}
pub fn start() {
    START.call_once(|| {
        std::thread::spawn(|| loop {
            if let Ok(_serial) = SERIAL.try_lock() {
                let _ = refresh_locked();
            }
            std::thread::sleep(Duration::from_secs(2));
        });
    });
}
/// Cached only: never spawn ubus or block the screen status socket on native IPC.
pub fn snapshot() -> Option<Snapshot> {
    let mut s = CACHE.read().unwrap_or_else(|p| p.into_inner()).clone()?;
    s.busy = BUSY.load(Ordering::Acquire);
    Some(s)
}
pub fn get() -> (u16, Value) {
    match snapshot() {
        Some(s) => (200, json!({"ok":true,"data":s})),
        None => failure(
            503,
            "Admin service status unavailable; reload after checking native hooks and settings",
        ),
    }
}
fn next_settings(s: &Settings, u: Update) -> Result<Settings, &'static str> {
    if u.revision > MAX_REVISION || s.revision > MAX_REVISION {
        return Err("Admin revision invalid");
    }
    if u.revision != s.revision {
        return Err("Admin services changed; reload status");
    }
    if s.revision == MAX_REVISION {
        return Err("Admin revision exhausted");
    }
    let mut next = s.clone();
    next.revision = s
        .revision
        .checked_add(1)
        .ok_or("Admin revision exhausted")?;
    if u.action == Action::Boot {
        match u.service {
            Service::Adb => next.adb_boot = u.enabled,
            Service::Ssh => next.ssh_boot = u.enabled,
        }
    }
    Ok(next)
}
struct BusyEnd;
impl Drop for BusyEnd {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
        // Release the visible mutation flag on every exit, including a panic.
        // apply() separately invalidates cached truth before invoking native lifecycle.
    }
}
fn run_native(service: Service, enabled: bool) -> Result<(), &'static str> {
    let mut command = Command::new(service.init());
    command
        .arg(if enabled { "start" } else { "stop" })
        .env("OPENUI_SERVICE_MANUAL", "1");
    let result = process_runner::output(&mut command, None, Duration::from_secs(8), 16 * 1024);
    // Discard ALL stdout/stderr, including potentially changed native script messages.
    let command_ok = result.is_ok_and(|o| o.status.success());
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(owner) = query_owner(service) {
            if owner.is_some() == enabled && reject_unowned(service, owner).is_ok() {
                return if command_ok {
                    Ok(())
                } else {
                    Err("Native service command failed; refresh status")
                };
            }
        }
        if Instant::now() >= deadline {
            return Err("Native service outcome not verified; refresh status");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub fn put(body: &[u8]) -> (u16, Value) {
    let update: Update = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return failure(400, "Invalid admin service request"),
    };
    apply(update)
}
pub fn apply(update: Update) -> (u16, Value) {
    let Ok(_serial) = SERIAL.try_lock() else {
        return failure(409, "Admin service operation in progress");
    };
    BUSY.store(true, Ordering::Release);
    let busy = BusyEnd;
    let settings = match load() {
        Ok(s) => s,
        Err(e) => {
            publish(None);
            return failure(503, e);
        }
    };
    let next = match next_settings(&settings, update) {
        Ok(s) => s,
        Err(e) => return failure(409, e),
    };
    let _usb = if update.service == Service::Adb {
        match crate::usb::SwitchGuard::acquire() {
            Ok(g) => Some(g),
            Err(_) => return failure(409, "USB change in progress"),
        }
    } else {
        None
    };
    if !hooks_available(update.service) {
        return failure(
            409,
            "Native service hooks, boot link or executable unavailable",
        );
    }
    let owner = match query_owner(update.service) {
        Ok(v) => v,
        Err(e) => {
            publish(None);
            return failure(409, e);
        }
    };
    if let Err(e) = reject_unowned(update.service, owner) {
        return failure(409, e);
    }
    if update.service == Service::Adb
        && update.action == Action::Runtime
        && update.enabled
        && !crate::usb::adb_exposed()
    {
        return failure(
            409,
            "ADB USB function is not exposed; USB composition was not changed",
        );
    }
    // A durable reservation invalidates stale UI even when native lifecycle times out.
    // Runtime reservations alter ONLY revision, never either boot preference.
    if let Err(e) = save(&next) {
        drop(busy);
        let _ = refresh_locked();
        return failure(500, e);
    }
    // Do not allow pre-operation status to look fresh if a worker panics.
    publish(None);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if update.action == Action::Runtime && owner.is_some() != update.enabled {
            run_native(update.service, update.enabled)
        } else {
            Ok(())
        }
    }))
    .unwrap_or(Err("Admin service operation stopped; refresh status"));
    drop(busy);
    // This publication precedes return, and therefore precedes DevUI JobEnd/FRESH.
    let fresh = refresh_locked();
    match (result, fresh) {
        (Ok(()), Ok(s)) => (200, json!({"ok":true,"data":s})),
        (Err(e), _) => failure(500, e),
        (_, Err(e)) => failure(503, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_request_and_config() {
        assert!(serde_json::from_str::<Update>(
            r#"{"revision":0,"service":"adb","action":"runtime","enabled":true}"#
        )
        .is_ok());
        for text in [
            r#"{"revision":0,"service":"adb","action":"runtime","enabled":1}"#,
            r#"{"revision":0,"service":"dropbear","action":"runtime","enabled":true}"#,
            r#"{"revision":0,"service":"ssh","action":"stop","enabled":true}"#,
            r#"{"revision":0,"service":"ssh","action":"boot","enabled":false,"command":"x"}"#,
        ] {
            assert!(serde_json::from_str::<Update>(text).is_err());
        }
        assert!(parse_settings(b"garbage").is_err());
        assert!(parse_settings(br#"{"revision":0,"adb_boot":true}"#).is_err());
        assert!(
            parse_settings(br#"{"revision":0,"adb_boot":true,"ssh_boot":true,"x":0}"#).is_err()
        );
        assert!(parse_settings(br#"{"revision":"0","adb_boot":true,"ssh_boot":true}"#).is_err());
        assert!(parse_settings(br#"{"revision":-1,"adb_boot":true,"ssh_boot":true}"#).is_err());
    }
    #[test]
    fn independent_boot_runtime_and_revision() {
        let s = Settings::default();
        assert!(s.adb_boot && s.ssh_boot);
        let runtime = Update {
            revision: 0,
            service: Service::Adb,
            action: Action::Runtime,
            enabled: false,
        };
        let n = next_settings(&s, runtime).unwrap();
        assert_eq!(n.revision, 1);
        assert!(n.adb_boot && n.ssh_boot);
        assert!(next_settings(&n, runtime).is_err());
        let boot = next_settings(
            &n,
            Update {
                revision: 1,
                action: Action::Boot,
                ..runtime
            },
        )
        .unwrap();
        assert!(!boot.adb_boot && boot.ssh_boot);
        assert_eq!(boot.revision, 2);
        assert!(next_settings(
            &Settings {
                revision: MAX_REVISION,
                ..s.clone()
            },
            Update {
                revision: MAX_REVISION,
                ..runtime
            }
        )
        .is_err());
        assert!(next_settings(
            &Settings {
                revision: MAX_REVISION + 1,
                ..s
            },
            Update {
                revision: MAX_REVISION + 1,
                ..runtime
            }
        )
        .is_err());
        assert!(parse_settings(
            br#"{"revision":9007199254740992,"adb_boot":true,"ssh_boot":true}"#
        )
        .is_err());
    }
    #[test]
    fn reject_boot_link_collision() {
        assert!(boot_target_valid(
            Service::Adb,
            Path::new("../init.d/adbd.init")
        ));
        assert!(boot_target_valid(
            Service::Ssh,
            Path::new("/etc/init.d/mu5250-ssh")
        ));
        for p in [
            "../init.d/dropbear",
            "/tmp/adbd.init",
            "../init.d/../init.d/adbd.init",
        ] {
            assert!(!boot_target_valid(Service::Adb, Path::new(p)));
        }
    }
    #[test]
    fn exact_procd_ownership() {
        let v = json!({"adbd.init":{"instances":{"instance1":{"running":true,"pid":42,"command":["/sbin/adbd"]}}}});
        assert_eq!(procd_owner(Service::Adb, &v), Ok(Some(42)));
        assert!(procd_owner(Service::Ssh, &v).is_err());
        for bad in [
            json!({"adbd.init":{"instances":{"instance1":{"running":true,"pid":1,"command":["/sbin/adbd"]}}}}),
            json!({"adbd.init":{"instances":{"instance1":{"running":true,"pid":42,"command":["/sbin/adbd","-x"]}}}}),
            json!({"adbd.init":{"instances":{"unexpected":{"running":true,"pid":42,"command":["/sbin/adbd"]}}}}),
        ] {
            assert!(procd_owner(Service::Adb, &bad).is_err());
        }
        assert_eq!(procd_owner(Service::Adb, &json!({})), Ok(None));
        assert!(argv_matches(Service::Adb, b"/sbin/adbd\0"));
        assert!(!argv_matches(Service::Adb, b"/sbin/adbd\0--other\0"));
        let mut cmd = SSH_ARGS.join("\0").into_bytes();
        cmd.push(0);
        assert!(argv_matches(Service::Ssh, &cmd));
        assert!(!argv_matches(Service::Adb, &cmd));
    }
}
