//! Typed management of an independently supervised tailscaled: sign-in (browser link or
//! auth key) and sign-out through fixed CLI arguments, then mode, exit node and remote
//! access. Never touch identity files or accept command strings from clients.
use crate::{clash, clash_ctl, process::BoundedCommand, storage, wireguard};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    net::Ipv4Addr,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

/// Where deploy-components.py --tailscale installs the CLI; an older layout keeps working.
const CLIS: [&str; 2] = ["/data/bin/tailscale", "/data/mu5250-admin/bin/tailscale"];
fn cli_path() -> &'static str {
    CLIS.iter()
        .copied()
        .find(|p| Path::new(p).is_file())
        .unwrap_or(CLIS[0])
}
const SOCKET: &str = "/var/run/tailscale/tailscaled.sock";
const DIR: &str = "/data/local/tmp/openui-tailscale";
const CONFIG: &str = "/data/local/tmp/openui-tailscale/config.json";
/// Auth key handed to `tailscale login` as `file:`, so it never appears in argv.
const KEY_FILE: &str = "/data/local/tmp/openui-tailscale/authkey";
const CHANGED: &str = "Configuration changed; refresh before saving";
static BLOCK_NATIVE: AtomicBool = AtomicBool::new(true);
static ERROR: Mutex<Option<String>> = Mutex::new(None);
/// Why remote access is not active although chosen (it then falls back to fully down).
static REMOTE_ERROR: Mutex<Option<String>> = Mutex::new(None);
/// The running `tailscale login`; while it runs the supervisor leaves the daemon up.
static LOGIN: Mutex<Option<Child>> = Mutex::new(None);
/// Why the last sign-in ended without enrolling.
static LOGIN_ERROR: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    revision: u64,
    enabled: bool,
    exit_node: String,
    /// Remote management: keep tailscaled up advertising the LAN as a subnet route, so
    /// tailnet devices reach 192.168.0.1 (SSH, OpenUI) even with Clash or WireGuard on.
    remote_access: bool,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Update {
    Connect {
        expected_revision: u64,
    },
    Disconnect {
        expected_revision: u64,
    },
    SelectExit {
        expected_revision: u64,
        exit_node: String,
    },
    RemoteAccess {
        expected_revision: u64,
        enabled: bool,
    },
    /// Without `auth_key` the daemon offers a browser link (`auth_url`) to finish.
    Login {
        expected_revision: u64,
        #[serde(default)]
        hostname: String,
        #[serde(default)]
        auth_key: String,
    },
    CancelLogin {
        expected_revision: u64,
    },
    Logout {
        expected_revision: u64,
    },
}
impl Update {
    fn revision(&self) -> u64 {
        match self {
            Self::Connect { expected_revision }
            | Self::Disconnect { expected_revision }
            | Self::SelectExit {
                expected_revision, ..
            }
            | Self::RemoteAccess {
                expected_revision, ..
            }
            | Self::Login {
                expected_revision, ..
            }
            | Self::CancelLogin { expected_revision }
            | Self::Logout { expected_revision } => *expected_revision,
        }
    }
}
/// A DNS label: what tailscale accepts for --hostname.
fn valid_hostname(s: &str) -> bool {
    (1..=63).contains(&s.len())
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn valid_auth_key(s: &str) -> bool {
    s.starts_with("tskey-")
        && (16..=256).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
/// Fixed arguments: a fresh profile without tailnet DNS or routes; the supervisor then
/// applies the saved mode. A key goes by file only.
fn login_args(hostname: &str, key_file: Option<&str>) -> Vec<String> {
    let mut a = vec![
        format!("--socket={SOCKET}"),
        "login".into(),
        "--accept-dns=false".into(),
        "--accept-routes=false".into(),
        "--timeout=600s".into(),
    ];
    if !hostname.is_empty() {
        a.push(format!("--hostname={hostname}"));
    }
    if let Some(f) = key_file {
        a.push(format!("--auth-key=file:{f}"));
    }
    a
}
/// Reaps a finished login; true while one runs.
fn login_active() -> bool {
    let mut slot = LOGIN.lock().unwrap();
    let Some(child) = slot.as_mut() else {
        return false;
    };
    match child.try_wait() {
        Ok(None) => true,
        done => {
            let ok = matches!(done, Ok(Some(s)) if s.success());
            *slot = None;
            let _ = fs::remove_file(KEY_FILE);
            if !ok && !status().is_ok_and(|v| enrolled(&v)) {
                *LOGIN_ERROR.lock().unwrap() = Some("Tailscale sign-in did not finish. Check the auth key, or try again and complete the link within 10 minutes.".into());
            }
            false
        }
    }
}
fn stop_login() {
    if let Some(mut child) = LOGIN.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = fs::remove_file(KEY_FILE);
}
fn start_login(hostname: &str, auth_key: &str) -> Result<(), String> {
    let key_file = if auth_key.is_empty() {
        None
    } else {
        fs::create_dir_all(DIR).map_err(|_| "Cannot create Tailscale settings directory")?;
        fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot protect Tailscale settings directory")?;
        let _ = fs::remove_file(KEY_FILE);
        {
            use std::{io::Write, os::unix::fs::OpenOptionsExt};
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(KEY_FILE)
                .map_err(|_| "Cannot store the auth key")?;
            f.write_all(auth_key.as_bytes())
                .map_err(|_| "Cannot store the auth key")?;
        }
        Some(KEY_FILE)
    };
    let child = Command::new(cli_path())
        .args(login_args(hostname, key_file))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(c) => {
            *LOGIN.lock().unwrap() = Some(c);
            *LOGIN_ERROR.lock().unwrap() = None;
            Ok(())
        }
        Err(_) => {
            let _ = fs::remove_file(KEY_FILE);
            Err("Cannot start Tailscale sign-in".into())
        }
    }
}
/// The daemon's sign-in link, only while it waits for one and only a plain https URL.
fn auth_url(v: &Value) -> Option<String> {
    if v["BackendState"].as_str() != Some("NeedsLogin") {
        return None;
    }
    let url = v["AuthURL"].as_str()?;
    let rest = url.strip_prefix("https://")?;
    (url.len() <= 300
        && !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=%".contains(&b)))
    .then(|| url.to_string())
}
fn load() -> Result<Settings, String> {
    match fs::read(CONFIG) {
        Ok(b) => {
            let s: Settings =
                serde_json::from_slice(&b).map_err(|_| "Saved Tailscale settings are invalid")?;
            if !s.exit_node.is_empty() && !stable_id(&s.exit_node) {
                return Err("Saved Tailscale exit node is invalid".into());
            }
            Ok(s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("Cannot read Tailscale settings".into()),
    }
}
fn persist(s: &Settings) -> Result<(), String> {
    fs::create_dir_all(DIR).map_err(|_| "Cannot create Tailscale settings directory")?;
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot protect Tailscale settings directory")?;
    storage::atomic_write(Path::new(CONFIG), &serde_json::to_vec(s).unwrap())
        .map_err(|_| "Cannot save Tailscale settings".into())
}
fn next_revision(s: &mut Settings, expected: u64) -> Result<(), String> {
    if s.revision != expected {
        return Err(CHANGED.into());
    }
    s.revision = s
        .revision
        .checked_add(1)
        .ok_or("Configuration revision exhausted")?;
    Ok(())
}
fn cli(args: &[&str]) -> Result<Value, String> {
    let output = Command::new(cli_path())
        .arg(format!("--socket={SOCKET}"))
        .args(args)
        .bounded_output()
        .map_err(|_| "Cannot query Tailscale daemon")?;
    if !output.status.success() {
        return Err("Tailscale daemon query failed".into());
    }
    // Raw status/prefs can contain private material. Never log or return them.
    serde_json::from_slice(&output.stdout).map_err(|_| "Invalid Tailscale daemon response".into())
}
fn status() -> Result<Value, String> {
    cli(&["status", "--json"])
}
fn prefs() -> Result<Value, String> {
    cli(&["debug", "prefs"])
}
fn accept_dns(running: bool, exit: &str) -> bool {
    running && !exit.is_empty()
}
/// The subnet routes `s` advertises: the LAN with remote access, else none.
fn routes(s: &Settings) -> Result<Vec<String>, String> {
    if s.remote_access {
        Ok(vec![lan_cidr()?])
    } else {
        Ok(Vec::new())
    }
}
/// AdvertiseRoutes as saved in prefs (null when none), only well-formed private IPv4.
fn advertised(p: &Value) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for r in p["AdvertiseRoutes"].as_array().into_iter().flatten() {
        let r = r.as_str()?;
        if private_lan_cidr(r).as_deref() != Some(r) {
            return None;
        }
        out.push(r.to_string());
    }
    out.sort();
    Some(out)
}
fn patch(running: bool, exit: &str, routes: &[String]) -> Result<(), String> {
    // MaskedPrefs: explicit Set bits are required by tailscaled's local API.
    let body = json!({"WantRunning":running,"WantRunningSet":true,
        "AdvertiseRoutes":routes,"AdvertiseRoutesSet":true,
        "CorpDNS":accept_dns(running, exit),"CorpDNSSet":true,"RouteAll":false,"RouteAllSet":true,
        "ExitNodeID":exit,"ExitNodeIDSet":true,"ExitNodeIP":"","ExitNodeIPSet":true,
        "ExitNodeAllowLANAccess":true,"ExitNodeAllowLANAccessSet":true});
    let (code, _) = clash_ctl::request_with_host(
        Path::new(SOCKET),
        "PATCH",
        "/localapi/v0/prefs",
        Some(&body),
        Duration::from_secs(10),
        "local-tailscaled.sock",
    )
    .map_err(|_| "Cannot update Tailscale preferences")?;
    if !(200..300).contains(&code) {
        return Err("Tailscale rejected preference update".into());
    }
    Ok(())
}
fn tailnet_ip(s: &str) -> bool {
    s.parse::<Ipv4Addr>().is_ok_and(|a| {
        let b = a.octets();
        b[0] == 100 && (64..=127).contains(&b[1])
    })
}
fn stable_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn safe_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(120).collect()
}
fn display_name(peer: &Value, fallback: &str) -> String {
    // DNSName carries the tailnet-assigned/custom node name; do not expose its domain.
    peer["DNSName"]
        .as_str()
        .and_then(|s| s.trim().trim_end_matches('.').split('.').next())
        .map(safe_name)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            peer["HostName"]
                .as_str()
                .map(safe_name)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| fallback.to_string())
}
fn eligible_peer(peer: &Value) -> Option<Value> {
    let id = peer["ID"].as_str()?;
    if !stable_id(id) || peer["Online"] != true || peer["ExitNodeOption"] != true {
        return None;
    }
    let ip = peer["TailscaleIPs"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|s| tailnet_ip(s))?;
    Some(json!({"id":id,"name":display_name(peer, "Exit node"),
        "hostname":safe_name(peer["HostName"].as_str().unwrap_or("")),"ip":ip,"online":true}))
}
fn exits(v: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = v["Peer"]
        .as_object()
        .into_iter()
        .flat_map(|p| p.values())
        .filter_map(eligible_peer)
        .collect();
    out.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    out
}
fn enrolled(v: &Value) -> bool {
    matches!(v["BackendState"].as_str(), Some("Running" | "Stopped"))
        && v["Self"]["ID"].as_str().is_some_and(stable_id)
}
fn verify(p: &Value, running: bool, exit: &str, routes: &[String]) -> bool {
    p["WantRunning"].as_bool() == Some(running)
        && advertised(p).as_deref() == Some(routes)
        && p["CorpDNS"].as_bool() == Some(accept_dns(running, exit))
        && p["RouteAll"].as_bool() == Some(false)
        && p["ExitNodeID"].as_str().unwrap_or("") == exit
        && p["ExitNodeIP"]
            .as_str()
            .is_none_or(|s| s.is_empty() || s == "invalid IP")
        && (exit.is_empty() || p["ExitNodeAllowLANAccess"].as_bool() == Some(true))
}
/// Conservative read-only dataplane withdrawal evidence. Never delete foreign routes.
/// A down daemon must leave table 52 completely empty in BOTH address families:
/// even a residual blackhole/throw can change the next backend's forwarding semantics.
fn route_destination(s: &str) -> bool {
    if s == "default" {
        return true;
    }
    let (address, prefix) = s.split_once('/').map_or((s, None), |(a, p)| (a, Some(p)));
    let Ok(ip) = address.parse::<std::net::IpAddr>() else {
        return false;
    };
    prefix.is_none_or(|p| {
        !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u8>()
                .is_ok_and(|n| n <= if ip.is_ipv4() { 32 } else { 128 })
    })
}
fn empty_route_table(success: bool, stdout: &[u8], stderr: &[u8]) -> bool {
    // Positive full inventory is required: old kernels may report bare "Dump terminated"
    // for a nonexistent single table. Never reinterpret an unsuccessful dump as empty.
    if !success
        || !stderr.iter().all(u8::is_ascii_whitespace)
        || !stdout.is_ascii()
        || stdout
            .iter()
            .any(|b| b.is_ascii_control() && !b.is_ascii_whitespace())
    {
        return false;
    }
    let Ok(text) = std::str::from_utf8(stdout) else {
        return false;
    };
    for row in text.lines().filter(|line| !line.trim().is_empty()) {
        let tokens: Vec<&str> = row.split_ascii_whitespace().collect();
        let start = match tokens[0] {
            "unicast" | "local" | "broadcast" | "multicast" | "anycast"
            | "unreachable" | "prohibit" | "blackhole" | "throw" | "nat"
            // ip -N prints RTN_* types numerically as well as table/proto IDs.
            | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "10" | "11" => 1,
            _ => 0,
        };
        if !tokens.get(start).is_some_and(|s| route_destination(s)) {
            return false;
        }
        let mut table_seen = false;
        for (i, token) in tokens.iter().enumerate() {
            if *token == "table" {
                if table_seen {
                    return false;
                }
                table_seen = true;
                let Some(id) = tokens.get(i + 1) else {
                    return false;
                };
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                    return false;
                }
                let Ok(id) = id.parse::<u32>() else {
                    return false;
                };
                if id == 52 {
                    return false;
                }
            } else if token.starts_with("table") {
                return false;
            }
        }
    }
    true
}
const LAN_CHAIN: &str = "OU_TS_LAN4";
const LAN_OWNER: &str = "openui-tailscale-lan4";
fn private_lan_cidr(value: &str) -> Option<String> {
    let (address, prefix) = value.split_once('/')?;
    let ip: Ipv4Addr = address.parse().ok()?;
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let prefix: u8 = prefix.parse().ok()?;
    let b = ip.octets();
    let minimum = if b[0] == 10 {
        8
    } else if b[0] == 172 && (16..=31).contains(&b[1]) {
        12
    } else if b[0] == 192 && b[1] == 168 {
        16
    } else {
        return None;
    };
    if !(minimum..=30).contains(&prefix) {
        return None;
    }
    let network = u32::from(ip) & (u32::MAX << (32 - prefix));
    Some(format!("{}/{prefix}", Ipv4Addr::from(network)))
}
fn lan_cidr_from_addresses(text: &str) -> Result<String, String> {
    let mut networks = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        for pair in fields.windows(2).filter(|p| p[0] == "inet") {
            let cidr = private_lan_cidr(pair[1])
                .ok_or("br-lan IPv4 address is not a supported private LAN subnet")?;
            if !networks.contains(&cidr) {
                networks.push(cidr);
            }
        }
    }
    if networks.len() != 1 {
        return Err("br-lan must have exactly one private IPv4 LAN subnet".into());
    }
    Ok(networks.remove(0))
}
/// The router's own LAN address (what tailnet devices use once the route is approved).
fn lan_ip() -> Option<String> {
    let out = Command::new("ip")
        .args([
            "-o", "-4", "addr", "show", "dev", "br-lan", "scope", "global",
        ])
        .bounded_output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    text.split_ascii_whitespace()
        .skip_while(|f| *f != "inet")
        .nth(1)
        .and_then(|a| a.split('/').next())
        .and_then(|a| a.parse::<Ipv4Addr>().ok())
        .map(|a| a.to_string())
}
fn lan_cidr() -> Result<String, String> {
    let out = Command::new("ip")
        .args([
            "-o", "-4", "addr", "show", "dev", "br-lan", "scope", "global",
        ])
        .bounded_output()
        .map_err(|_| "Cannot read br-lan IPv4 scope")?;
    if !out.status.success() || !out.stderr.iter().all(u8::is_ascii_whitespace) {
        return Err("Cannot read br-lan IPv4 scope".into());
    }
    lan_cidr_from_addresses(
        std::str::from_utf8(&out.stdout).map_err(|_| "Invalid br-lan address response")?,
    )
}
fn lan_nat_rules(cidr: &str) -> Vec<Vec<String>> {
    [("POSTROUTING", LAN_CHAIN), (LAN_CHAIN, "MASQUERADE")]
        .into_iter()
        .map(|(from, to)| {
            [
                "-A",
                from,
                "-s",
                cidr,
                "-o",
                "tailscale0",
                "-m",
                "comment",
                "--comment",
                LAN_OWNER,
                "-j",
                to,
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        })
        .collect()
}
#[derive(Default)]
struct LanNat {
    chain: bool,
    rules: Vec<Vec<String>>,
}
fn parse_lan_nat(text: &str) -> Result<LanNat, String> {
    if !text.is_ascii()
        || !text.lines().any(|line| {
            line.split_ascii_whitespace()
                .eq(["-P", "POSTROUTING", "ACCEPT"])
        })
    {
        return Err("Cannot positively verify NAT POSTROUTING inventory".into());
    }
    let mut owned = LanNat::default();
    for line in text.lines() {
        let row: Vec<String> = line
            .split_ascii_whitespace()
            .map(|s| s.trim_matches('"').to_string())
            .collect();
        if !row.iter().any(|s| s == LAN_CHAIN || s == LAN_OWNER) {
            continue;
        }
        if row == ["-N", LAN_CHAIN].map(str::to_string) {
            if owned.chain {
                return Err("Duplicate Tailscale LAN NAT chain declaration".into());
            }
            owned.chain = true;
            continue;
        }
        let valid = row
            .get(3)
            .and_then(|s| private_lan_cidr(s))
            .is_some_and(|cidr| row[3] == cidr && lan_nat_rules(&cidr).contains(&row));
        if !valid || owned.rules.len() >= 16 {
            return Err(
                "Tailscale LAN NAT ownership collision; foreign rules left untouched".into(),
            );
        }
        owned.rules.push(row);
    }
    if !owned.chain && !owned.rules.is_empty() {
        return Err("Invalid Tailscale LAN NAT inventory".into());
    }
    Ok(owned)
}
fn nat_command(args: &[String]) -> Result<String, String> {
    lan_table_command("nat", args)
}
fn lan_table_command(table: &str, args: &[String]) -> Result<String, String> {
    let out = Command::new("iptables")
        .args(["-w", "2", "-t", table])
        .args(args)
        .bounded_output()
        .map_err(|_| "Cannot access Tailscale LAN rule ownership")?;
    if !out.status.success() || !out.stderr.iter().all(u8::is_ascii_whitespace) {
        return Err("Tailscale LAN rule operation failed".into());
    }
    String::from_utf8(out.stdout).map_err(|_| "Invalid Tailscale LAN rule inventory".into())
}
const LAN_MSS_OWNER: &str = "openui-tailscale-lan4-mss";
fn lan_mss_rules(cidr: &str) -> Vec<Vec<String>> {
    [
        ("-s", "br-lan", "tailscale0"),
        ("-d", "tailscale0", "br-lan"),
    ]
    .into_iter()
    .map(|(scope, from, to)| {
        [
            "-A",
            "FORWARD",
            scope,
            cidr,
            "-i",
            from,
            "-o",
            to,
            "-p",
            "tcp",
            "-m",
            "tcp",
            "--tcp-flags",
            "SYN,RST",
            "SYN",
            "-m",
            "comment",
            "--comment",
            LAN_MSS_OWNER,
            "-j",
            "TCPMSS",
            "--clamp-mss-to-pmtu",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    })
    .collect()
}
fn parse_lan_mss(text: &str) -> Result<Vec<Vec<String>>, String> {
    if !text.is_ascii()
        || !text.lines().any(|line| {
            line.split_ascii_whitespace()
                .eq(["-P", "FORWARD", "ACCEPT"])
        })
    {
        return Err("Cannot positively verify mangle FORWARD inventory".into());
    }
    let mut owned = Vec::new();
    for line in text.lines() {
        let row: Vec<String> = line
            .split_ascii_whitespace()
            .map(|s| s.trim_matches('"').to_string())
            .collect();
        if !row.iter().any(|s| s == LAN_MSS_OWNER) {
            continue;
        }
        let valid = row
            .get(3)
            .and_then(|s| private_lan_cidr(s))
            .is_some_and(|cidr| row[3] == cidr && lan_mss_rules(&cidr).contains(&row));
        if !valid || owned.len() >= 16 {
            return Err(
                "Tailscale LAN MSS ownership collision; foreign rules left untouched".into(),
            );
        }
        owned.push(row);
    }
    Ok(owned)
}
fn lan_mss_inventory() -> Result<Vec<Vec<String>>, String> {
    parse_lan_mss(&lan_table_command("mangle", &["-S".into()])?)
}
fn lan_mss_deletions(mut rules: Vec<Vec<String>>) -> Vec<Vec<String>> {
    for rule in &mut rules {
        rule[0] = "-D".into();
    }
    rules
}
fn verify_lan_mss_absent() -> Result<(), String> {
    if !lan_mss_inventory()?.is_empty() {
        return Err(
            "Tailscale LAN MSS withdrawal is not verified; native activation blocked".into(),
        );
    }
    Ok(())
}
fn cleanup_lan_mss() -> Result<(), String> {
    for rule in lan_mss_deletions(lan_mss_inventory()?) {
        lan_table_command("mangle", &rule)?;
    }
    verify_lan_mss_absent()
}
fn sync_lan_mss(cidr: &str) -> Result<(), String> {
    let desired = lan_mss_rules(cidr);
    let owned = lan_mss_inventory()?;
    if owned.len() == 2 && desired.iter().all(|r| owned.contains(r)) {
        return Ok(());
    }
    cleanup_lan_mss()?;
    for rule in &desired {
        lan_table_command("mangle", rule)?;
    }
    let actual = lan_mss_inventory()?;
    if actual.len() != 2 || !desired.iter().all(|r| actual.contains(r)) {
        return Err("Tailscale LAN MSS installation is not verified".into());
    }
    Ok(())
}
fn lan_nat_inventory() -> Result<LanNat, String> {
    parse_lan_nat(&nat_command(&["-S".into()])?)
}
fn verify_lan_nat_absent() -> Result<(), String> {
    let owned = lan_nat_inventory()?;
    if owned.chain || !owned.rules.is_empty() {
        return Err(
            "Tailscale LAN NAT withdrawal is not verified; native activation blocked".into(),
        );
    }
    Ok(())
}
fn lan_nat_deletions(mut owned: LanNat) -> Vec<Vec<String>> {
    // Delete only exact comment-tagged owner rules; jumps go first. Never flush a chain.
    owned.rules.sort_by_key(|r| r[1] != "POSTROUTING");
    let mut commands = owned.rules;
    for rule in &mut commands {
        rule[0] = "-D".into();
    }
    if owned.chain {
        commands.push(vec!["-X".into(), LAN_CHAIN.into()]);
    }
    commands
}
fn cleanup_lan_nat_rules() -> Result<(), String> {
    for rule in lan_nat_deletions(lan_nat_inventory()?) {
        nat_command(&rule)?;
    }
    verify_lan_nat_absent()
}
fn cleanup_lan_nat() -> Result<(), String> {
    // Both owned families are attempted even when either inventory/cleanup fails.
    let nat = cleanup_lan_nat_rules();
    let mss = cleanup_lan_mss();
    nat.and(mss)
}
fn sync_lan_nat(exit: &str) -> Result<(), String> {
    if exit.is_empty() {
        return cleanup_lan_nat();
    }
    let cidr = lan_cidr()?;
    let desired = lan_nat_rules(&cidr);
    let owned = lan_nat_inventory()?;
    if owned.chain && owned.rules.len() == 2 && desired.iter().all(|r| owned.rules.contains(r)) {
        return sync_lan_mss(&cidr);
    }
    cleanup_lan_nat()?;
    nat_command(&["-N".into(), LAN_CHAIN.into()])?;
    // Populate the owned chain before adding its narrowly-scoped POSTROUTING jump.
    for rule in desired.iter().rev() {
        nat_command(rule)?;
    }
    let actual = lan_nat_inventory()?;
    if !actual.chain || actual.rules.len() != 2 || !desired.iter().all(|r| actual.rules.contains(r))
    {
        return Err("Tailscale LAN NAT installation is not verified".into());
    }
    sync_lan_mss(&cidr)
}
fn cleanup_then_stop(
    cleanup: impl FnOnce() -> Result<(), String>,
    stop: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    // Stopping tailscaled MUST still happen after a failed owned NAT cleanup.
    let cleaned = cleanup();
    let stopped = stop();
    cleaned.and(stopped)
}
fn stop_owned_and_daemon() -> Result<(), String> {
    cleanup_then_stop(cleanup_lan_nat, || patch(false, "", &[]))
}
fn selected_lan_nat(exit: &str) -> Result<(), String> {
    let result = sync_lan_nat(exit);
    if result.is_err() {
        let _ = stop_owned_and_daemon();
    }
    result
}
fn resolver_released(contents: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(contents) else {
        return false;
    };
    if text
        .bytes()
        .any(|b| b.is_ascii_control() && !b.is_ascii_whitespace())
        || text.to_ascii_lowercase().contains("generated by tailscale")
    {
        return false;
    }
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("nameserver") {
            continue;
        }
        let Some(address) = fields.next() else {
            return false;
        };
        let address = address.split('%').next().unwrap_or("");
        let Ok(ip) = address.parse::<std::net::IpAddr>() else {
            return false;
        };
        if matches!(
            ip.to_string().as_str(),
            "100.100.100.100" | "fd7a:115c:a1e0::53"
        ) {
            return false;
        }
    }
    true
}
fn verify_resolver_stopped() -> Result<(), String> {
    use std::io::Read;
    const LIMIT: u64 = 64 * 1024;
    let file = fs::File::open("/etc/resolv.conf")
        .map_err(|_| "Cannot verify Tailscale resolver withdrawal; native activation blocked")?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err("Resolver file is not verifiable; native activation blocked".into());
    }
    let mut contents = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut contents)
        .map_err(|_| "Cannot verify Tailscale resolver withdrawal; native activation blocked")?;
    if contents.len() as u64 > LIMIT || !resolver_released(&contents) {
        return Err(
            "Tailscale resolver withdrawal is not verified; native activation blocked".into(),
        );
    }
    Ok(())
}
fn verify_dataplane_stopped() -> Result<(), String> {
    for flag in ["-4", "-6"] {
        let output = Command::new("ip")
            .args([flag, "-N", "route", "show", "table", "all"])
            .bounded_output()
            .map_err(|_| "Cannot verify Tailscale route withdrawal; native activation blocked")?;
        if !empty_route_table(output.status.success(), &output.stdout, &output.stderr) {
            return Err(
                "Tailscale table 52 is not verified empty; native activation blocked".into(),
            );
        }
    }
    verify_resolver_stopped()?;
    verify_lan_nat_absent()?;
    verify_lan_mss_absent()
}
fn verify_stopped() -> Result<(), String> {
    if !verify(&prefs()?, false, "", &[]) {
        return Err("Tailscale did not stop; native activation blocked".into());
    }
    if !matches!(
        status()?["BackendState"].as_str(),
        Some("Stopped" | "NeedsLogin" | "NoState" | "NeedsMachineAuth")
    ) {
        return Err("Tailscale is not stopped; native activation blocked".into());
    }
    verify_dataplane_stopped()
}
/// Called ONLY with transition lock. Does not take native backend locks.
pub(crate) fn ensure_down() -> Result<(), String> {
    let mut s = load()?;
    // Remote access only is not "Tailscale mode": it leaves native backends allowed.
    // Leaving Tailscale mode drops straight to it, without a stop in between.
    if s.remote_access {
        if s.enabled {
            s.enabled = false;
            s.revision = s
                .revision
                .checked_add(1)
                .ok_or("Configuration revision exhausted")?;
            persist(&s)?;
        }
        match ensure_remote_only(&s) {
            Ok(()) => {
                *REMOTE_ERROR.lock().unwrap() = None;
                return Ok(());
            }
            Err(e) => *REMOTE_ERROR.lock().unwrap() = Some(e),
        }
    }
    BLOCK_NATIVE.store(true, Ordering::SeqCst);
    let cleanup = cleanup_lan_nat();
    // Never let cleanup failure bypass a daemon shutdown attempt.
    let stopped = (|| -> Result<(), String> {
        if !Path::new(SOCKET).exists() {
            if s.enabled || Path::new("/sys/class/net/tailscale0").exists() {
                return Err("Tailscale daemon unavailable; cannot verify shutdown".into());
            }
            verify_dataplane_stopped()?;
        } else {
            if !verify(&prefs()?, false, "", &[]) {
                patch(false, "", &[])?;
            }
            verify_stopped()?;
        }
        Ok(())
    })();
    cleanup?;
    stopped?;
    if s.enabled {
        s.enabled = false;
        s.revision = s
            .revision
            .checked_add(1)
            .ok_or("Configuration revision exhausted")?;
        persist(&s)?;
    }
    BLOCK_NATIVE.store(false, Ordering::SeqCst);
    Ok(())
}
/// Remote access without the Tailscale backend: up, advertising only the LAN, with no
/// exit node, no tailnet DNS, no accepted routes and no owned LAN NAT. Table 52 then
/// holds tailnet peers only, so Clash and WireGuard keep routing the LAN.
fn ensure_remote_only(s: &Settings) -> Result<(), String> {
    if !enrolled(&status()?) {
        return Err("Tailscale is not signed in; sign in on the Tailscale page first".into());
    }
    cleanup_lan_nat()?;
    let routes = routes(s)?;
    if !prefs().is_ok_and(|p| verify(&p, true, "", &routes)) {
        patch(true, "", &routes)?;
        if !verify(&prefs()?, true, "", &routes) {
            return Err("Tailscale remote access verification failed".into());
        }
    }
    BLOCK_NATIVE.store(false, Ordering::SeqCst);
    Ok(())
}
pub(crate) fn native_allowed() -> bool {
    !BLOCK_NATIVE.load(Ordering::SeqCst) && load().is_ok_and(|s| !s.enabled)
}
pub(crate) fn require_native_allowed() -> Result<(), String> {
    if native_allowed() {
        Ok(())
    } else {
        Err("Tailscale mode or unverified shutdown blocks native tunnel activation".into())
    }
}
pub(crate) fn native_off() -> Result<(), String> {
    let _wg = wireguard::lock();
    // Both policies must clean up successfully, including stale owned resources.
    clash::disable_for_tailscale()?;
    wireguard::disable_for_tailscale()
}
fn start_selected(s: &Settings) -> Result<(), String> {
    let result = start_selected_inner(s);
    if result.is_err() {
        // Never silently retain an old exit selection or fall back to direct routing.
        // If shutdown itself fails, native backends remain blocked and error is retained.
        let _ = stop_owned_and_daemon();
    }
    result
}
fn start_selected_inner(s: &Settings) -> Result<(), String> {
    let v = status()?;
    if !enrolled(&v) {
        return Err("Tailscale is not signed in; sign in on the Tailscale page first".into());
    }
    if !s.exit_node.is_empty() && !exits(&v).iter().any(|p| p["id"] == s.exit_node) {
        return Err("Selected exit node is not an online eligible tailnet peer".into());
    }
    native_off()?;
    let routes = routes(s)?;
    patch(true, &s.exit_node, &routes)?;
    if !verify(&prefs()?, true, &s.exit_node, &routes) {
        return Err("Tailscale preference verification failed".into());
    }
    sync_lan_nat(&s.exit_node)
}
/// Only enum-like public state strings; never AuthURL, keys, raw prefs or health text.
fn sanitized_state(v: &Value) -> &'static str {
    match v["BackendState"].as_str() {
        Some("Running") => "Running",
        Some("Stopped") => "Stopped",
        Some("NeedsLogin") => "NeedsLogin",
        Some("NeedsMachineAuth") => "NeedsMachineAuth",
        Some("Starting") => "Starting",
        _ => "Unavailable",
    }
}
fn sanitized(s: &Settings, v: &Value, error: Option<String>) -> Value {
    let me = &v["Self"];
    let own = me["ID"].as_str().filter(|id| stable_id(id)).map(|id| {
        let ips: Vec<&str> = me["TailscaleIPs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|ip| tailnet_ip(ip))
            .collect();
        json!({"id":id,"name":display_name(me, ""),
            "hostname":safe_name(me["HostName"].as_str().unwrap_or("")),
            "ips":ips,"online":me["Online"].as_bool().unwrap_or(false)})
    });
    let state = sanitized_state(v);
    json!({"revision":s.revision,"enabled":s.enabled,"backend_state":state,"self":own,
        "exit_nodes":exits(v),"selected_exit_node":s.exit_node,"error":error,
        "lan_forwarding_verified":false,"auth_url":auth_url(v)})
}
fn response() -> Result<Value, String> {
    let s = load()?;
    let mut error = ERROR.lock().unwrap().clone();
    let v = status().unwrap_or_else(|e| {
        error = Some(e);
        Value::Null
    });
    let actual = prefs().ok();
    let mut result = sanitized(&s, &v, error);
    result["want_running"] = actual
        .as_ref()
        .and_then(|p| p["WantRunning"].as_bool())
        .map_or(Value::Null, |b| json!(b));
    result["actual_exit_node"] = actual
        .as_ref()
        .and_then(|p| p["ExitNodeID"].as_str())
        .filter(|id| id.is_empty() || stable_id(id))
        .map_or(Value::Null, |id| json!(id));
    let lan = lan_cidr().ok();
    let advertised_routes = actual.as_ref().and_then(advertised).unwrap_or_default();
    result["remote_access"] = json!(s.remote_access);
    result["lan_cidr"] = json!(lan);
    result["lan_ip"] = json!(lan_ip());
    result["advertised_routes"] = json!(advertised_routes);
    result["route_approved"] = json!(route_approved(&v, lan.as_deref()));
    result["remote_error"] = json!(REMOTE_ERROR.lock().unwrap().clone());
    result["installed"] = json!(Path::new(cli_path()).is_file());
    result["login_active"] = json!(login_active());
    result["login_error"] = json!(LOGIN_ERROR.lock().unwrap().clone());
    // The signed-in account's login name, for "signed in as"; nothing else from the profile.
    result["account"] = json!(enrolled(&v)
        .then(|| actual
            .as_ref()
            .and_then(|p| p["Config"]["UserProfile"]["LoginName"].as_str())
            .map(safe_name))
        .flatten());
    Ok(result)
}
/// The admin console approved (and serves) our LAN route: it is in Self.PrimaryRoutes.
fn route_approved(v: &Value, lan: Option<&str>) -> bool {
    lan.is_some_and(|lan| {
        v["Self"]["PrimaryRoutes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r.as_str() == Some(lan))
    })
}
pub fn get() -> (u16, Value) {
    reply(response())
}
pub(crate) fn summary() -> Result<Value, String> {
    let s = load()?;
    let ready = Path::new(cli_path()).is_file() && Path::new(SOCKET).exists();
    Ok(
        json!({"enabled":s.enabled,"revision":s.revision,"ready":ready,
        "reason":if ready { Value::Null } else { json!("Tailscale daemon is unavailable") },
        "error":ERROR.lock().unwrap().clone()}),
    )
}
fn reply(result: Result<Value, String>) -> (u16, Value) {
    match result {
        Ok(v) => (200, json!({"ok":true,"data":v})),
        Err(e) => (
            if e == CHANGED { 409 } else { 400 },
            json!({"ok":false,"error":e}),
        ),
    }
}
/// Commit OFF intent before touching the daemon: failed withdrawal must never
/// leave a persisted ON intent for the supervisor to reconnect on its next tick.
fn disconnect_intent(
    s: &mut Settings,
    mut save: impl FnMut(&Settings) -> Result<(), String>,
    mut stop: impl FnMut() -> Result<(), String>,
    mut verify: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    s.enabled = false;
    save(s)?;
    stop()?;
    verify()
}
pub fn update(body: &[u8]) -> (u16, Value) {
    let _transition = crate::transition::lock();
    update_locked(body)
}
pub(crate) fn update_locked(body: &[u8]) -> (u16, Value) {
    let mut action = "invalid";
    let result = (|| {
        let u: Update = serde_json::from_slice(body).map_err(|_| "Invalid Tailscale request")?;
        action = match &u {
            Update::Connect { .. } => "connect",
            Update::Disconnect { .. } => "disconnect",
            Update::SelectExit { .. } => "select exit node",
            Update::RemoteAccess { enabled: true, .. } => "remote access on",
            Update::RemoteAccess { .. } => "remote access off",
            Update::Login { auth_key, .. } if !auth_key.is_empty() => "sign in with key",
            Update::Login { .. } => "sign in with link",
            Update::CancelLogin { .. } => "cancel sign-in",
            Update::Logout { .. } => "sign out",
        };
        let mut s = load()?;
        next_revision(&mut s, u.revision())?;
        match u {
            Update::Disconnect { .. } if s.remote_access => {
                // Leave Tailscale mode but stay up for remote management.
                BLOCK_NATIVE.store(true, Ordering::SeqCst);
                s.enabled = false;
                persist(&s)?;
                ensure_remote_only(&s)?;
            }
            Update::RemoteAccess { enabled, .. } => {
                if enabled {
                    if !enrolled(&status()?) {
                        return Err(
                            "Tailscale is not signed in; sign in on the Tailscale page first"
                                .into(),
                        );
                    }
                    lan_cidr()?;
                }
                s.remote_access = enabled;
                persist(&s)?;
                *REMOTE_ERROR.lock().unwrap() = None;
                if s.enabled {
                    BLOCK_NATIVE.store(true, Ordering::SeqCst);
                    start_selected(&s)?;
                } else if enabled {
                    ensure_remote_only(&s)?;
                } else {
                    ensure_down()?;
                }
            }
            Update::Disconnect { .. } => {
                BLOCK_NATIVE.store(true, Ordering::SeqCst);
                // Commit caller's OFF revision first. A failed patch/withdrawal leaves
                // native blocked and the supervisor retrying ensure_down, never connect.
                disconnect_intent(&mut s, persist, stop_owned_and_daemon, verify_stopped)?;
                BLOCK_NATIVE.store(false, Ordering::SeqCst);
            }
            Update::SelectExit { exit_node, .. } => {
                if !exit_node.is_empty()
                    && (!stable_id(&exit_node)
                        || !exits(&status()?).iter().any(|p| p["id"] == exit_node))
                {
                    return Err("Exit node must be an online eligible tailnet peer".into());
                }
                s.exit_node = exit_node;
                if s.enabled {
                    BLOCK_NATIVE.store(true, Ordering::SeqCst);
                }
                persist(&s)?;
                if s.enabled {
                    start_selected(&s)?;
                }
            }
            Update::Login {
                hostname, auth_key, ..
            } => {
                let hostname = hostname.trim().to_string();
                if !hostname.is_empty() && !valid_hostname(&hostname) {
                    return Err(
                        "Device name may use only letters, digits and hyphens (up to 63)".into(),
                    );
                }
                if !auth_key.is_empty() && !valid_auth_key(&auth_key) {
                    return Err(
                        "Auth key must be a tskey-… key from the Tailscale admin console".into(),
                    );
                }
                if login_active() {
                    return Err("A Tailscale sign-in is already in progress".into());
                }
                let v = status()?;
                if enrolled(&v) {
                    return Err("Tailscale is already signed in; sign out first".into());
                }
                if v["BackendState"].as_str() == Some("NeedsMachineAuth") {
                    return Err(
                        "This router waits for approval in the Tailscale admin console".into(),
                    );
                }
                persist(&s)?;
                start_login(&hostname, &auth_key)?;
            }
            Update::CancelLogin { .. } => {
                persist(&s)?;
                stop_login();
                *LOGIN_ERROR.lock().unwrap() = None;
            }
            Update::Logout { .. } => {
                stop_login();
                // Intent first: nothing may reconnect or re-advertise this LAN afterwards.
                BLOCK_NATIVE.store(true, Ordering::SeqCst);
                s.enabled = false;
                s.remote_access = false;
                s.exit_node.clear();
                persist(&s)?;
                *REMOTE_ERROR.lock().unwrap() = None;
                let cleaned = cleanup_lan_nat();
                let out = Command::new(cli_path())
                    .arg(format!("--socket={SOCKET}"))
                    .arg("logout")
                    .bounded_output()
                    .map_err(|_| "Cannot run Tailscale sign-out")?;
                if !out.status.success() {
                    return Err("Tailscale sign-out failed".into());
                }
                cleaned?;
                ensure_down()?;
            }
            Update::Connect { .. } => {
                let v = status()?;
                if !enrolled(&v) {
                    return Err(
                        "Tailscale is not signed in; sign in on the Tailscale page first".into(),
                    );
                }
                if !s.exit_node.is_empty() && !exits(&v).iter().any(|p| p["id"] == s.exit_node) {
                    return Err("Selected exit node is not an online eligible tailnet peer".into());
                }
                BLOCK_NATIVE.store(true, Ordering::SeqCst);
                // Cleanup failure must never start Tailscale or create an ON intent.
                native_off()?;
                s.enabled = true;
                persist(&s)?;
                start_selected(&s)?;
            }
        }
        *ERROR.lock().unwrap() = None;
        response()
    })();
    if let Err(e) = &result {
        *ERROR.lock().unwrap() = Some(e.clone());
    }
    match &result {
        Ok(_) => eprintln!("[tailscale] {action}: ok"),
        Err(e) => eprintln!("[tailscale] {action}: {e}"),
    }
    reply(result)
}
/// Reconcile under the same lock as HTTP/screen/core updates. Daemon itself is NOT started here.
pub fn start() {
    // A key file can only be left by a sign-in the agent did not live to reap.
    let _ = fs::remove_file(KEY_FILE);
    std::thread::spawn(|| {
        let mut first = true;
        let mut seen = String::new();
        loop {
            {
                let _transition = crate::transition::lock();
                let result = load().and_then(|s| {
                    if s.enabled {
                        BLOCK_NATIVE.store(true, Ordering::SeqCst);
                        if first
                            || wireguard::stored_enabled()
                            || clash::stored_enabled()
                            || !routes(&s).is_ok_and(|r| {
                                prefs().is_ok_and(|p| verify(&p, true, &s.exit_node, &r))
                            })
                        {
                            start_selected(&s)
                        } else {
                            selected_lan_nat(&s.exit_node)
                        }
                    } else if login_active() {
                        Ok(())
                    } else {
                        ensure_down()
                    }
                });
                first = result.is_err();
                let mut error = ERROR.lock().unwrap();
                let next = result.err();
                // Only changes: the supervisor retries every 15 s with the same error.
                match (&*error, &next) {
                    (None, Some(e)) => eprintln!("[tailscale] {e}"),
                    (Some(old), Some(e)) if old != e => eprintln!("[tailscale] {e}"),
                    (Some(_), None) => eprintln!("[tailscale] recovered"),
                    _ => {}
                }
                *error = next;
                drop(error);
                // The daemon's own state (signed out, offline…), logged when it changes.
                let now = status()
                    .map(|v| {
                        format!(
                            "{} {}",
                            sanitized_state(&v),
                            if v["Self"]["Online"].as_bool() == Some(true) {
                                "online"
                            } else {
                                "offline"
                            }
                        )
                    })
                    .unwrap_or_else(|_| "daemon unavailable".into());
                if now != seen {
                    eprintln!("[tailscale] {now}");
                    seen = now;
                }
            }
            std::thread::sleep(Duration::from_secs(15));
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_access_routes_are_verified_and_reported() {
        let p = json!({"WantRunning":true,"CorpDNS":false,"RouteAll":false,"ExitNodeID":"","ExitNodeIP":"",
            "AdvertiseRoutes":["192.168.0.0/24"]});
        let lan = vec!["192.168.0.0/24".to_string()];
        assert!(verify(&p, true, "", &lan));
        assert!(!verify(&p, true, "", &[]));
        let none = json!({"WantRunning":false,"CorpDNS":false,"RouteAll":false,"ExitNodeID":"","AdvertiseRoutes":null});
        assert!(verify(&none, false, "", &[]) && !verify(&none, false, "", &lan));
        // A route that is not a clean private LAN prefix fails verification.
        assert_eq!(advertised(&json!({"AdvertiseRoutes":["0.0.0.0/0"]})), None);
        assert_eq!(
            advertised(&json!({"AdvertiseRoutes":["192.168.0.1/24"]})),
            None
        );
        let v = json!({"Self":{"PrimaryRoutes":["192.168.0.0/24"]}});
        assert!(route_approved(&v, Some("192.168.0.0/24")));
        assert!(
            !route_approved(&json!({"Self":{}}), Some("192.168.0.0/24"))
                && !route_approved(&v, None)
        );
        let u: Update = serde_json::from_str(
            r#"{"action":"remote_access","expected_revision":4,"enabled":true}"#,
        )
        .unwrap();
        assert_eq!(u.revision(), 4);
        assert!(serde_json::from_str::<Update>(
            r#"{"action":"remote_access","expected_revision":4,"enabled":"yes"}"#
        )
        .is_err());
        // Old saved settings without the field still load, with remote access off.
        let old: Settings =
            serde_json::from_str(r#"{"revision":3,"enabled":false,"exit_node":""}"#).unwrap();
        assert!(!old.remote_access);
    }
    #[test]
    fn off_intent_is_durable_before_patch_or_withdrawal_failure() {
        use std::cell::RefCell;
        for fail_patch in [false, true] {
            let events = RefCell::new(Vec::new());
            let saved = RefCell::new(Settings {
                revision: 8,
                enabled: true,
                exit_node: String::new(),
                remote_access: false,
            });
            let mut next = saved.borrow().clone();
            next_revision(&mut next, 8).unwrap();
            let result = disconnect_intent(
                &mut next,
                |s| {
                    events.borrow_mut().push("save-off");
                    *saved.borrow_mut() = s.clone();
                    Ok(())
                },
                || {
                    events.borrow_mut().push("patch");
                    if fail_patch {
                        Err("patch failed".into())
                    } else {
                        Ok(())
                    }
                },
                || {
                    events.borrow_mut().push("verify");
                    Err("withdrawal unverified".into())
                },
            );
            assert!(result.is_err());
            assert!(
                !saved.borrow().enabled,
                "supervisor must retry shutdown, not reconnect"
            );
            assert_eq!(saved.borrow().revision, 9);
            let expected = if fail_patch {
                vec!["save-off", "patch"]
            } else {
                vec!["save-off", "patch", "verify"]
            };
            assert_eq!(*events.borrow(), expected);
        }
    }
    #[test]
    fn route_withdrawal_requires_successful_numeric_inventory_without_table52() {
        let native = b"default via 10.0.0.1 dev rmnet_data0 proto 16\nlocal 192.168.0.1 dev bridge0 table 255 proto 2 scope 254 src 192.168.0.1\n10.0.0.0/24 dev rmnet_data0 table 254 proto 2 scope 253\nfd00::/64 dev bridge0 table 254 proto 2 metric 256 pref medium\n";
        assert!(empty_route_table(true, native, b""));
        assert!(empty_route_table(true, b"\n", b""));
        // Literal safe native rows: -N renders RTN_LOCAL as 2.
        assert!(empty_route_table(
            true,
            b"2 127.0.0.1 dev lo table 255 proto 2 scope 254 src 127.0.0.1",
            b""
        ));
        assert!(empty_route_table(
            true,
            b"2 ::1 dev lo table 255 proto 2 metric 0 pref medium",
            b""
        ));
        for kind in 1..=11 {
            let row = format!("{kind} 127.0.0.1 dev lo table 255 proto 2");
            assert!(empty_route_table(true, row.as_bytes(), b""));
            let blocked = format!("{kind} default table 52");
            assert!(!empty_route_table(true, blocked.as_bytes(), b""));
        }
        for kind in ["0", "12", "52", "255", "99999999999999999999"] {
            let row = format!("{kind} default table 255");
            assert!(!empty_route_table(true, row.as_bytes(), b""));
        }
        for row in [
            "100.64.0.1 dev tailscale0 table 52",
            "default dev tailscale0 table 52",
            "fd7a:115c:a1e0::1 dev tailscale0 table 52",
            "throw 192.168.0.0/24 table 52",
            "blackhole default table 52",
            "default dev tailscale0 table 052",
        ] {
            assert!(!empty_route_table(true, row.as_bytes(), b""));
        }
        assert!(empty_route_table(
            true,
            b"blackhole default table 152\n",
            b""
        ));
        for row in [
            "default table main",
            "default table",
            "default table nope",
            "default table 4294967296",
            "default table=52",
            "default table 254 table 254",
            "unknown-route-output",
            "10.0.0.0/99 dev eth0 table 254",
            "default dev \u{00e9} table 254",
        ] {
            assert!(!empty_route_table(true, row.as_bytes(), b""));
        }
        assert!(!empty_route_table(false, b"", b"Dump terminated\n"));
        assert!(!empty_route_table(false, native, b""));
        assert!(!empty_route_table(
            true,
            native,
            b"warning: unknown route result\n"
        ));
        assert!(!empty_route_table(false, b"", b"Operation not permitted\n"));
    }
    #[test]
    fn eligible_exit_requires_stable_id_online_option_and_cgnat() {
        let p = json!({"ID":"n123","Online":true,"ExitNodeOption":true,"TailscaleIPs":["100.64.0.1"],"HostName":"node"});
        assert!(eligible_peer(&p).is_some());
        for (key, value) in [
            ("ID", json!("--bad;cmd")),
            ("Online", json!(false)),
            ("ExitNodeOption", json!(false)),
            ("TailscaleIPs", json!(["100.63.0.1"])),
        ] {
            let mut q = p.clone();
            q[key] = value;
            assert!(eligible_peer(&q).is_none());
        }
        assert!(tailnet_ip("100.127.255.255"));
        assert!(!tailnet_ip("100.128.0.1"));
    }
    #[test]
    fn custom_assigned_dns_label_is_primary_without_tailnet_domain() {
        let mut peer = json!({"ID":"n1","HostName":"instance-original","DNSName":"us-oracle-a1.tailnet.ts.net.",
            "Online":true,"ExitNodeOption":true,"TailscaleIPs":["100.64.0.1"]});
        let exit = eligible_peer(&peer).unwrap();
        assert_eq!(exit["name"], "us-oracle-a1");
        assert_eq!(exit["hostname"], "instance-original");
        let own = sanitized(&Settings::default(), &json!({"Self":peer.clone()}), None);
        assert_eq!(own["self"]["name"], "us-oracle-a1");
        assert_eq!(own["self"]["hostname"], "instance-original");
        assert!(!own.to_string().contains("tailnet.ts.net"));
        assert!(!exit.to_string().contains("tailnet.ts.net"));
        peer.as_object_mut().unwrap().remove("DNSName");
        assert_eq!(eligible_peer(&peer).unwrap()["name"], "instance-original");
        peer["DNSName"] = json!(".");
        assert_eq!(display_name(&peer, "Exit node"), "instance-original");
    }
    #[test]
    fn strict_requests_and_revision() {
        assert!(serde_json::from_str::<Update>(
            r#"{"action":"connect","expected_revision":0,"argv":"x"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Update>(
            r#"{"action":"select_exit","expected_revision":0}"#
        )
        .is_err());
        let mut s = Settings::default();
        assert!(next_revision(&mut s, 1).is_err());
        assert_eq!(s.revision, 0);
        next_revision(&mut s, 0).unwrap();
        assert_eq!(s.revision, 1);
    }
    #[test]
    fn response_never_contains_raw_auth_or_prefs() {
        let v = json!({"BackendState":"NeedsLogin","AuthURL":"secret","PrivateKey":"secret","Self":{"ID":"n1","HostName":"node\n","TailscaleIPs":["100.64.0.1"]}});
        let out = sanitized(&Settings::default(), &v, None).to_string();
        assert!(!out.contains("secret"));
        assert!(!out.contains("AuthURL"));
    }
    #[test]
    fn sign_in_link_and_arguments_are_fixed() {
        let mut v =
            json!({"BackendState":"NeedsLogin","AuthURL":"https://login.tailscale.com/a/1a2b3c"});
        assert_eq!(
            sanitized(&Settings::default(), &v, None)["auth_url"],
            "https://login.tailscale.com/a/1a2b3c"
        );
        for bad in [
            "http://login.tailscale.com/a/1",
            "https://",
            "https://x y",
            "javascript:alert(1)",
            "https://a\nb",
        ] {
            v["AuthURL"] = json!(bad);
            assert_eq!(auth_url(&v), None, "{bad}");
        }
        // Once signed in (or before the daemon asks), no link.
        assert_eq!(
            auth_url(
                &json!({"BackendState":"Running","AuthURL":"https://login.tailscale.com/a/1"})
            ),
            None
        );
        assert!(valid_hostname("u60-pro") && valid_hostname("A1"));
        for bad in ["", "-a", "a-", "a b", "a.b", "a;rm", &"a".repeat(64)] {
            assert!(!valid_hostname(bad), "{bad}");
        }
        assert!(valid_auth_key("tskey-auth-kAbC123CNTRL-xyz789"));
        for bad in [
            "tskey-",
            "abc-tskey-auth-1234567890",
            "tskey-auth 1234567890",
            "tskey-auth-123456789\n0",
        ] {
            assert!(!valid_auth_key(bad), "{bad}");
        }
        assert_eq!(
            login_args("u60", Some(KEY_FILE)),
            vec![
                format!("--socket={SOCKET}"),
                "login".into(),
                "--accept-dns=false".into(),
                "--accept-routes=false".into(),
                "--timeout=600s".into(),
                "--hostname=u60".into(),
                format!("--auth-key=file:{KEY_FILE}")
            ]
        );
        assert!(!login_args("", None)
            .iter()
            .any(|a| a.starts_with("--hostname") || a.starts_with("--auth-key")));
        let u: Update =
            serde_json::from_str(r#"{"action":"login","expected_revision":2}"#).unwrap();
        assert!(
            matches!(u, Update::Login { ref hostname, ref auth_key, .. } if hostname.is_empty() && auth_key.is_empty())
        );
        assert_eq!(
            serde_json::from_str::<Update>(r#"{"action":"logout","expected_revision":5}"#)
                .unwrap()
                .revision(),
            5
        );
        assert!(serde_json::from_str::<Update>(
            r#"{"action":"login","expected_revision":2,"argv":"x"}"#
        )
        .is_err());
    }
    #[test]
    fn prefs_require_explicit_safe_values() {
        let p = json!({"WantRunning":true,"CorpDNS":true,"RouteAll":false,"ExitNodeID":"n1","ExitNodeAllowLANAccess":true});
        assert!(verify(&p, true, "n1", &[]));
        assert!(!verify(&p, false, "n1", &[]));
        let mut q = p.clone();
        q["RouteAll"] = json!(true);
        assert!(!verify(&q, true, "n1", &[]));
        q = p.clone();
        q["CorpDNS"] = json!(false);
        assert!(!verify(&q, true, "n1", &[]));
        q["ExitNodeID"] = json!("");
        assert!(verify(&q, true, "", &[]));
        q["WantRunning"] = json!(false);
        assert!(verify(&q, false, "", &[]));
        q["CorpDNS"] = json!(true);
        assert!(!verify(&q, false, "", &[]));
        assert!(accept_dns(true, "n1"));
        assert!(!accept_dns(true, ""));
        assert!(!accept_dns(false, "n1"));
    }
    #[test]
    fn lan_scope_is_dynamic_private_and_unambiguous() {
        assert_eq!(
            private_lan_cidr("10.21.4.8/24").as_deref(),
            Some("10.21.4.0/24")
        );
        assert_eq!(
            private_lan_cidr("172.30.7.5/16").as_deref(),
            Some("172.30.0.0/16")
        );
        assert_eq!(
            private_lan_cidr("192.168.8.1/24").as_deref(),
            Some("192.168.8.0/24")
        );
        for bad in [
            "8.8.8.8/24",
            "100.64.0.1/24",
            "169.254.1.1/16",
            "192.168.1.1/15",
            "172.16.1.1/11",
            "10.0.0.1/7",
            "10.0.0.1/31",
            "10.0.0.1/32",
            "10.0.0.1/+24",
            "10.0.0.1/x",
        ] {
            assert!(private_lan_cidr(bad).is_none(), "{bad}");
        }
        assert_eq!(
            lan_cidr_from_addresses(
                "4: br-lan inet 10.21.4.8/24 brd 10.21.4.255 scope global br-lan\n"
            )
            .unwrap(),
            "10.21.4.0/24"
        );
        assert!(lan_cidr_from_addresses("").is_err());
        assert!(lan_cidr_from_addresses(
            "4: br-lan inet 10.21.4.8/24\n4: br-lan inet 172.30.0.1/16\n"
        )
        .is_err());
    }
    #[test]
    fn owned_nat_scope_and_exact_cleanup_fixture() {
        let cidr = "10.21.4.0/24";
        let desired = lan_nat_rules(cidr);
        let fixture = format!("-P POSTROUTING ACCEPT\n-N {LAN_CHAIN}\n{}\n{}\n-A POSTROUTING -s 172.20.0.0/16 -j MASQUERADE\n", desired[1].join(" "), desired[0].join(" "));
        let owned =
            parse_lan_nat(&fixture.replace(LAN_OWNER, &format!("\"{LAN_OWNER}\""))).unwrap();
        assert!(owned.chain);
        assert_eq!(owned.rules.len(), 2);
        let commands = lan_nat_deletions(owned);
        let mut jump = desired[0].clone();
        jump[0] = "-D".into();
        let mut masquerade = desired[1].clone();
        masquerade[0] = "-D".into();
        assert_eq!(
            commands,
            vec![
                jump,
                masquerade,
                vec!["-X".to_string(), LAN_CHAIN.to_string()]
            ]
        );
        for rule in desired {
            assert_eq!(&rule[2..6], ["-s", cidr, "-o", "tailscale0"]);
            assert!(rule.iter().any(|s| s == LAN_OWNER));
        }
        assert!(lan_nat_deletions(
            parse_lan_nat("-P POSTROUTING ACCEPT\n-A POSTROUTING -o eth0 -j MASQUERADE\n").unwrap()
        )
        .is_empty());
    }
    #[test]
    fn nat_collision_fails_closed_without_foreign_cleanup() {
        for foreign in [format!("-N {LAN_CHAIN}\n-A {LAN_CHAIN} -j ACCEPT\n"),
            format!("-N {LAN_CHAIN}\n-A OTHER -j {LAN_CHAIN}\n"),
            format!("-N {LAN_CHAIN}\n-A {LAN_CHAIN} -j MASQUERADE\n"),
            format!("-N {LAN_CHAIN}\n-A POSTROUTING -s 8.8.8.0/24 -o tailscale0 -m comment --comment {LAN_OWNER} -j {LAN_CHAIN}\n")] {
            assert!(parse_lan_nat(&format!("-P POSTROUTING ACCEPT\n{foreign}")).is_err());
        }
        assert!(parse_lan_nat("").is_err());
        assert!(parse_lan_nat("unrecognized successful output").is_err());
    }
    #[test]
    fn mss_both_directions_and_exact_cleanup_fixture() {
        let cidr = "192.168.0.0/24";
        let desired = lan_mss_rules(cidr);
        assert_eq!(desired[0][2], "-s");
        assert_eq!(desired[1][2], "-d");
        assert_eq!(&desired[0][4..8], ["-i", "br-lan", "-o", "tailscale0"]);
        assert_eq!(&desired[1][4..8], ["-i", "tailscale0", "-o", "br-lan"]);
        for rule in &desired {
            assert_eq!(rule[3], cidr);
            assert_eq!(
                &rule[8..15],
                ["-p", "tcp", "-m", "tcp", "--tcp-flags", "SYN,RST", "SYN"]
            );
            assert_eq!(&rule[19..], ["-j", "TCPMSS", "--clamp-mss-to-pmtu"]);
        }
        let fixture = format!(
            "-P FORWARD ACCEPT\n{}\n{}\n-A FORWARD -o eth0 -p tcp -j ACCEPT\n",
            desired[0].join(" "),
            desired[1].join(" ")
        );
        let owned = parse_lan_mss(&fixture.replace(LAN_MSS_OWNER, &format!("\"{LAN_MSS_OWNER}\"")))
            .unwrap();
        assert_eq!(owned, desired);
        let mut expected = desired.clone();
        for rule in &mut expected {
            rule[0] = "-D".into();
        }
        assert_eq!(lan_mss_deletions(owned), expected);
        assert!(lan_mss_deletions(
            parse_lan_mss("-P FORWARD ACCEPT\n-A FORWARD -j ACCEPT\n").unwrap()
        )
        .is_empty());
    }
    #[test]
    fn mss_inventory_and_collision_fail_closed() {
        assert!(parse_lan_mss("").is_err());
        assert!(parse_lan_mss("unrecognized successful output").is_err());
        let valid = lan_mss_rules("10.21.4.0/24")[0].join(" ");
        for changed in [
            valid.replace("-s 10.21.4.0/24", "-s 8.8.8.0/24"),
            valid.replace("-o tailscale0", "-o eth0"),
            valid.replace("-A FORWARD", "-A OTHER"),
            valid.replace("--clamp-mss-to-pmtu", "--set-mss 1200"),
            valid.replace("--tcp-flags SYN,RST SYN", "--tcp-flags SYN SYN"),
        ] {
            assert!(parse_lan_mss(&format!("-P FORWARD ACCEPT\n{changed}\n")).is_err());
        }
    }
    #[test]
    fn failed_nat_cleanup_still_stops_daemon() {
        use std::cell::RefCell;
        let events = RefCell::new(Vec::new());
        let result = cleanup_then_stop(
            || {
                events.borrow_mut().push("cleanup");
                Err("cleanup failed".into())
            },
            || {
                events.borrow_mut().push("stop-daemon");
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err(), "cleanup failed");
        assert_eq!(*events.borrow(), vec!["cleanup", "stop-daemon"]);
    }
    #[test]
    fn stale_tailscale_resolver_blocks_native_handoff() {
        assert!(resolver_released(
            b"# native resolver\nnameserver 192.0.2.53\nsearch lan\n"
        ));
        assert!(resolver_released(b"nameserver 127.0.0.1\n"));
        assert!(!resolver_released(
            b"# resolv.conf(5) file generated by tailscale\nnameserver 192.0.2.53\n"
        ));
        assert!(!resolver_released(b"nameserver 100.100.100.100\n"));
        assert!(!resolver_released(
            b"nameserver fd7a:115c:a1e0:0000:0000:0000:0000:0053\n"
        ));
        assert!(!resolver_released(b"nameserver\n"));
        assert!(!resolver_released(b"nameserver not-an-address\n"));
        assert!(!resolver_released(b"\xff\n"));
    }
}
