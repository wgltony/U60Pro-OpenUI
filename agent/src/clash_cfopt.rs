//! Cloudflare IP optimization for Clash nodes that run through Cloudflare (VLESS, VMess
//! or Trojan over WebSocket or gRPC with TLS, on a port Cloudflare proxies), sized for a
//! mobile link. Two stages:
//!
//! 1. Coarse, at the edge: TCP-connect a few hundred sampled addresses and check the
//!    fastest with a TLS request for the node's own domain, which also names the data
//!    center. This costs a few hundred kilobytes and only shortlists.
//! 2. Through the node: a few finalists (one per data center first, plus the addresses
//!    in use) each carry a real request and a short download through a throwaway core
//!    that listens on loopback only. What the edge answers says nothing about the leg
//!    from that data center to the node's own server, and that leg is where addresses
//!    differ: two that look alike at the edge can be seconds apart through the node.
//!
//! The pick is the lowest time to first byte among finalists whose download is not far
//! behind the best; the runner-up becomes the backup. The profile then carries, for each
//! such node, a copy on the pick, a copy on the backup and the original, in a fallback
//! group under the node's own name: groups and rules keep working, and a failing address
//! falls back to the next. Between full runs a single delay check through the running
//! core confirms the pick still holds, so a metered link is not spent on re-testing; a
//! change of uplink (Wi-Fi relay or cellular) starts a full run of its own.
//! Runs on its own thread (the HTTP server has few workers); pages poll the state.
use crate::{clash, clash_ctl, storage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use serde_norway::{Mapping, Value as Yaml};
use std::{
    collections::HashSet,
    fs,
    net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs},
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SETTINGS: &str = "/data/local/tmp/openui-clash/cfopt.json";
const STATE: &str = "/data/local/tmp/openui-clash/cfopt-state.json";
const CURL: &str = "/usr/bin/curl";
/// Cloudflare's published IPv4 ranges (cloudflare.com/ips-v4).
const RANGES: &[&str] = &[
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
];
/// HTTPS ports Cloudflare proxies.
const PORTS: &[u16] = &[443, 2053, 2083, 2087, 2096, 8443];
pub(crate) const INTERVALS: &[u32] = &[0, 1, 2, 3, 4, 6, 8, 12, 24];
/// Sampled addresses per run, one per /24; a run's TCP probes cost about 200 KB.
const SAMPLE: usize = 256;
const PROBE_THREADS: usize = 24;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1000);
/// Addresses checked with a TLS request after the TCP round.
const SHORTLIST: usize = 10;
/// New addresses tested through the node, besides the ones in use and the DNS answer.
const FINALISTS: usize = 4;
/// Requests per finalist for the time to first byte; the fastest counts.
const TTFB_TRIES: usize = 3;
/// Download through the node per finalist: on the Wi-Fi relay, and on cellular data.
const DOWNLOAD_WIFI: u64 = 4_000_000;
const DOWNLOAD_CELL: u64 = 1_000_000;
/// A finalist whose download is under this share of the best one is out.
const SPEED_SHARE: f64 = 0.5;
/// A new address must beat the one in use by this much to replace it, so the core is
/// not reloaded (and connections reset) for noise.
const TTFB_MARGIN: f64 = 1.15;
const TTFB_MARGIN_MS: u32 = 40;
/// Download that warms the path up before anything is measured.
const WARM_UP: u64 = 500_000;
/// The throwaway core: its directory (on tmpfs) and the first loopback port.
const TEST_DIR: &str = "/tmp/openui-cfopt";
const TEST_PORT: u16 = 17900;
/// A full run at least this often; in between, a due run is a delay check when the pick
/// still answers within `HOLD_FACTOR` (plus `HOLD_SLACK_MS`) of its delay when chosen.
const FULL_EVERY_SECS: u64 = 24 * 3600;
const HOLD_FACTOR: f64 = 1.5;
const HOLD_SLACK_MS: u32 = 150;
/// Wait after a failed automatic run before trying again.
const RETRY_SECS: u64 = 1800;
/// Name suffixes of the two halves of an optimized node.
pub(crate) const CF_TAG: &str = "CF优选";
pub(crate) const BAK_TAG: &str = "CF备选";
pub(crate) const DNS_TAG: &str = "原域名";

static RUNNING: AtomicBool = AtomicBool::new(false);
static PHASE: Mutex<&str> = Mutex::new("");
/// The last profile check rejected the rewritten profile; shown until the next run.
static REJECTED: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    revision: u64,
    enabled: bool,
    /// Hours between automatic runs; 0 runs only on request.
    interval_hours: u32,
    speed_test: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: 0,
            enabled: true,
            interval_hours: 4,
            speed_test: true,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Probe {
    ip: String,
    colo: String,
    connect_ms: u32,
    /// TLS handshake at the edge (stage 1).
    tls_ms: Option<u32>,
    /// Download through the node, kbit/s (stage 2).
    speed_kbps: Option<u32>,
    /// Time to first byte of a new HTTPS request through the node (stage 2).
    ttfb_ms: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Node {
    name: String,
    domain: String,
    port: u16,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    applied: Option<Probe>,
    /// The runner-up, second in the fallback group.
    backup: Option<Probe>,
    applied_at: u64,
    /// The uplink the pick was measured on: `wifi` or `cell`.
    uplink: String,
    /// When stage 2 last ran, and the pick's delay through the running core then.
    full_run: u64,
    ref_delay_ms: Option<u32>,
    /// The node's own DNS answer, measured the same way, for comparison.
    baseline: Option<Probe>,
    candidates: Vec<Probe>,
    nodes: Vec<Node>,
    last_run: u64,
    last_attempt: u64,
    last_error: Option<String>,
    tested: u32,
    data_kb: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn load_settings() -> Settings {
    fs::read(SETTINGS)
        .ok()
        .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
        .filter(|s| INTERVALS.contains(&s.interval_hours))
        .unwrap_or_default()
}
fn load_state() -> State {
    fs::read(STATE)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn save_state(st: &State) -> Result<(), String> {
    storage::atomic_write(Path::new(STATE), &serde_json::to_vec(st).unwrap())
        .map_err(|_| "Cannot save the Cloudflare optimization state".into())
}
fn phase(p: &'static str) {
    *PHASE.lock().unwrap() = p;
}

// ---------- Addresses ----------

fn range(cidr: &str) -> (u32, u8) {
    let (ip, len) = cidr.split_once('/').unwrap();
    (
        u32::from(ip.parse::<Ipv4Addr>().unwrap()),
        len.parse().unwrap(),
    )
}
pub(crate) fn cloudflare(ip: Ipv4Addr) -> bool {
    RANGES.iter().any(|c| {
        let (base, len) = range(c);
        u32::from(ip) >> (32 - len) == base >> (32 - len)
    })
}
/// xorshift64*, seeded from the kernel: sampling needs spread, not secrecy.
struct Rng(u64);
impl Rng {
    fn new() -> Self {
        let mut seed = [0u8; 8];
        let _ = fs::File::open("/dev/urandom").and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut seed)
        });
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self(u64::from_le_bytes(seed) ^ nanos | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}
/// `n` addresses in distinct /24s, spread over the ranges by size.
fn sample(n: usize, rng: &mut Rng) -> Vec<Ipv4Addr> {
    let blocks: Vec<(u32, u64)> = RANGES
        .iter()
        .map(|c| {
            let (base, len) = range(c);
            (base, 1u64 << (24 - u32::from(len.min(24))))
        })
        .collect();
    let total: u64 = blocks.iter().map(|b| b.1).sum();
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(n);
    while out.len() < n && (seen.len() as u64) < total {
        let mut pick = rng.next() % total;
        for (base, count) in &blocks {
            if pick < *count {
                let block = base + ((pick as u32) << 8);
                if seen.insert(block) {
                    out.push(Ipv4Addr::from(block + 1 + (rng.next() % 254) as u32));
                }
                break;
            }
            pick -= count;
        }
    }
    out
}

// ---------- Nodes ----------

fn text<'a>(m: &'a Mapping, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Yaml::as_str)
}
fn port(m: &Mapping) -> Option<u16> {
    match m.get("port")? {
        Yaml::Number(n) => n.as_u64().and_then(|p| u16::try_from(p).ok()),
        Yaml::String(s) => s.parse().ok(),
        _ => None,
    }
}
/// Nodes Cloudflare could front, from their shape alone: TLS over WebSocket or gRPC to a
/// domain on a Cloudflare HTTPS port. Whether the domain is on Cloudflare is checked later.
pub(crate) fn shaped(map: &Mapping) -> Vec<Node> {
    let Some(proxies) = map.get("proxies").and_then(Yaml::as_sequence) else {
        return vec![];
    };
    proxies
        .iter()
        .filter_map(Yaml::as_mapping)
        .filter_map(|m| {
            let kind = text(m, "type")?;
            let tls = kind == "trojan" || m.get("tls").and_then(Yaml::as_bool) == Some(true);
            let network = text(m, "network").unwrap_or("tcp");
            let server = text(m, "server")?;
            let port = port(m)?;
            let name = text(m, "name")?;
            let domain = server.parse::<std::net::IpAddr>().is_err()
                && server.contains('.')
                && server
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
            (matches!(kind, "vless" | "vmess" | "trojan")
                && tls
                && matches!(network, "ws" | "grpc")
                && !m.contains_key("reality-opts")
                && PORTS.contains(&port)
                && domain)
                .then(|| Node {
                    name: name.to_string(),
                    domain: server.to_ascii_lowercase(),
                    port,
                })
        })
        .collect()
}
fn resolve(domain: &str, port: u16) -> Vec<Ipv4Addr> {
    (domain, port)
        .to_socket_addrs()
        .map(|a| {
            a.filter_map(|a| match a {
                SocketAddr::V4(v) => Some(*v.ip()),
                SocketAddr::V6(_) => None,
            })
            .collect()
        })
        .unwrap_or_default()
}

/// A copy of node `m` under `name`, connecting to `ip` instead of its domain. The address
/// replaces the domain, so TLS and WebSocket must still name it.
fn pointed(m: &Mapping, domain: &str, name: &str, ip: &str) -> Mapping {
    let mut copy = m.clone();
    copy.insert("name".into(), name.into());
    copy.insert("server".into(), ip.into());
    let sni = if text(m, "type") == Some("trojan") {
        "sni"
    } else {
        "servername"
    };
    if text(m, sni).is_none() {
        copy.insert(sni.into(), domain.into());
    }
    if text(m, "network") == Some("ws") {
        let mut ws = copy
            .get("ws-opts")
            .and_then(Yaml::as_mapping)
            .cloned()
            .unwrap_or_default();
        let mut headers = ws
            .get("headers")
            .and_then(Yaml::as_mapping)
            .cloned()
            .unwrap_or_default();
        if !headers
            .keys()
            .any(|k| k.as_str().is_some_and(|k| k.eq_ignore_ascii_case("host")))
        {
            headers.insert("Host".into(), domain.into());
            ws.insert("headers".into(), Yaml::Mapping(headers));
            copy.insert("ws-opts".into(), Yaml::Mapping(ws));
        }
    }
    copy
}

/// Points each optimized node at `ip`: the node's name becomes a fallback group of a copy
/// on `ip`, a copy on `backup` when there is one, and the original. Returns how many
/// nodes changed.
pub(crate) fn apply_to(map: &mut Mapping, nodes: &[Node], ip: &str, backup: Option<&str>) -> usize {
    let shapes = shaped(map);
    let mut names: HashSet<String> = HashSet::new();
    for k in ["proxies", "proxy-groups"] {
        for m in map
            .get(k)
            .and_then(Yaml::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(Yaml::as_mapping)
        {
            if let Some(n) = text(m, "name") {
                names.insert(n.to_string());
            }
        }
    }
    let Some(proxies) = map.get_mut("proxies").and_then(Yaml::as_sequence_mut) else {
        return 0;
    };
    let backup = backup.filter(|b| *b != ip);
    let mut groups = vec![];
    let mut i = 0;
    while i < proxies.len() {
        let Some(m) = proxies[i].as_mapping() else {
            i += 1;
            continue;
        };
        let name = text(m, "name").unwrap_or("").to_string();
        let wanted = nodes.iter().find(|n| n.name == name);
        let shape = shapes.iter().find(|n| n.name == name);
        let (cf, bak, dns) = (
            format!("{name} | {CF_TAG}"),
            format!("{name} | {BAK_TAG}"),
            format!("{name} | {DNS_TAG}"),
        );
        match (wanted, shape) {
            (Some(n), Some(s))
                if n == s && ![&cf, &bak, &dns].iter().any(|x| names.contains(*x)) =>
            {
                let mut original = m.clone();
                original.insert("name".into(), dns.clone().into());
                let mut members = vec![Yaml::Mapping(pointed(m, &n.domain, &cf, ip))];
                let mut order: Vec<Yaml> = vec![cf.into()];
                if let Some(b) = backup {
                    members.push(Yaml::Mapping(pointed(m, &n.domain, &bak, b)));
                    order.push(bak.into());
                }
                members.push(Yaml::Mapping(original));
                order.push(dns.into());
                let count = members.len();
                proxies.splice(i..=i, members);
                let mut g = Mapping::new();
                g.insert("name".into(), name.into());
                g.insert("type".into(), "fallback".into());
                g.insert("proxies".into(), Yaml::Sequence(order));
                g.insert("url".into(), clash::DELAY_URL.into());
                g.insert("interval".into(), 300.into());
                groups.push(Yaml::Mapping(g));
                i += count;
            }
            _ => i += 1,
        }
    }
    let changed = groups.len();
    if changed > 0 {
        match map.get_mut("proxy-groups").and_then(Yaml::as_sequence_mut) {
            Some(existing) => existing.extend(groups),
            None => {
                map.insert("proxy-groups".into(), Yaml::Sequence(groups));
            }
        }
    }
    changed
}
/// For the profile writer: applies the saved pick when optimization is on.
pub(crate) fn rewrite(map: &mut Mapping) -> bool {
    if !load_settings().enabled {
        return false;
    }
    let st = load_state();
    match &st.applied {
        Some(p) => {
            apply_to(
                map,
                &st.nodes,
                &p.ip,
                st.backup.as_ref().map(|b| b.ip.as_str()),
            ) > 0
        }
        None => false,
    }
}
/// The core refused the rewritten profile; the profile writer fell back to the original.
pub(crate) fn rejected(e: &str) {
    *REJECTED.lock().unwrap() = Some(e.chars().take(200).collect());
}

// ---------- Measurements ----------

fn curl(args: &[&str], secs: u64) -> String {
    process_runner::output(
        Command::new(CURL).args(args),
        None,
        Duration::from_secs(secs),
        64 * 1024,
    )
    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    .unwrap_or_default()
}
/// Two TCP connects to each address; both must succeed. Average milliseconds.
fn tcp_round(ips: &[Ipv4Addr], port: u16) -> Vec<(Ipv4Addr, u32)> {
    let next = AtomicUsize::new(0);
    let found = Mutex::new(vec![]);
    std::thread::scope(|scope| {
        for _ in 0..PROBE_THREADS.min(ips.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(ip) = ips.get(i) else { break };
                let addr = SocketAddr::from((*ip, port));
                let mut total = 0u128;
                let ok = (0..2).all(|_| {
                    let start = Instant::now();
                    let up = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok();
                    total += start.elapsed().as_millis();
                    up
                });
                if ok {
                    found.lock().unwrap().push((*ip, (total / 2) as u32));
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap();
    found.sort_by_key(|f| f.1);
    found
}
/// `/cdn-cgi/trace` for the node's domain at `ip`: Cloudflare answers it at the edge, so
/// it proves that address serves the domain and names the data center.
fn trace(n: &Node, ip: Ipv4Addr) -> Option<Probe> {
    let resolve = format!("{}:{}:{ip}", n.domain, n.port);
    let url = format!("https://{}:{}/cdn-cgi/trace", n.domain, n.port);
    let out = curl(
        &[
            "-s",
            "--resolve",
            &resolve,
            "-m",
            "5",
            "-w",
            "\n%{http_code} %{time_connect} %{time_appconnect}",
            &url,
        ],
        8,
    );
    parse_trace(&out, ip)
}
fn parse_trace(out: &str, ip: Ipv4Addr) -> Option<Probe> {
    let (body, last) = out.rsplit_once('\n')?;
    let mut f = last.split_whitespace();
    let (code, connect, tls) = (f.next()?, f.next()?, f.next()?);
    let ms = |s: &str| s.parse::<f64>().ok().map(|v| (v * 1000.0).round() as u32);
    let colo = body
        .lines()
        .find_map(|l| l.strip_prefix("colo="))?
        .trim()
        .chars()
        .take(8)
        .collect::<String>();
    (code == "200" && !colo.is_empty()).then(|| Probe {
        ip: ip.to_string(),
        colo,
        connect_ms: ms(connect).unwrap_or(0),
        tls_ms: ms(tls).filter(|t| *t > 0),
        speed_kbps: None,
        ttfb_ms: None,
    })
}
/// What curl reports for a download of `wanted` bytes: kbit/s when the measurement
/// counts, and the bytes received.
fn parse_speed(out: &str, wanted: u64) -> (Option<u32>, u64) {
    let mut f = out.split_whitespace();
    let code = f.next().unwrap_or("");
    let rate = f.next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let size = f.next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) as u64;
    // A timed-out download still measured something once it got far enough.
    let ok = (code == "200" || size >= wanted / 10) && rate > 0.0 && size > 0;
    (ok.then(|| (rate * 8.0 / 1000.0).round() as u32), size)
}
/// `"<status> <seconds to first byte>"` of a request that reached its target.
fn parse_ttfb(out: &str) -> Option<u32> {
    let mut f = out.split_whitespace();
    let code = f.next()?;
    let secs = f.next()?.parse::<f64>().ok()?;
    (matches!(code, "200" | "204") && secs > 0.0).then(|| (secs * 1000.0).round() as u32)
}

// ---------- Through the node ----------

/// The uplink carrying the router's traffic, which the measurements describe.
fn uplink() -> &'static str {
    if crate::wifi_uplink::online() {
        "wifi"
    } else {
        "cell"
    }
}
/// The throwaway core's profile: one copy of `node` per address, each behind its own
/// loopback listener, and nothing of the user's (no TUN, DNS, controller or rules).
fn test_profile(node: &Mapping, domain: &str, ips: &[Ipv4Addr]) -> Mapping {
    let mut map = Mapping::new();
    map.insert("allow-lan".into(), false.into());
    map.insert("mode".into(), "rule".into());
    map.insert("log-level".into(), "warning".into());
    map.insert("ipv6".into(), false.into());
    let mut dns = Mapping::new();
    dns.insert("enable".into(), false.into());
    map.insert("dns".into(), Yaml::Mapping(dns));
    let (mut proxies, mut listeners) = (vec![], vec![]);
    for (i, ip) in ips.iter().enumerate() {
        let name = format!("c{i}");
        proxies.push(Yaml::Mapping(pointed(node, domain, &name, &ip.to_string())));
        let mut l = Mapping::new();
        l.insert("name".into(), format!("l{i}").into());
        l.insert("type".into(), "mixed".into());
        l.insert("listen".into(), "127.0.0.1".into());
        l.insert("port".into(), (i32::from(TEST_PORT) + i as i32).into());
        l.insert("proxy".into(), name.into());
        listeners.push(Yaml::Mapping(l));
    }
    map.insert("proxies".into(), Yaml::Sequence(proxies));
    map.insert("listeners".into(), Yaml::Sequence(listeners));
    map.insert("rules".into(), Yaml::Sequence(vec!["MATCH,DIRECT".into()]));
    map
}
/// The throwaway core; dropping it stops the process and removes its directory.
struct TestCore(std::process::Child);
impl Drop for TestCore {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = fs::remove_dir_all(TEST_DIR);
    }
}
/// Removes what a run that did not live to clean up left behind: its core, when the
/// recorded process still is that core, and its directory.
fn sweep_test_core() {
    let pid = fs::read_to_string(format!("{TEST_DIR}/pid"))
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|p| *p > 1);
    if let Some(pid) = pid {
        let ours = fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| {
            let c = String::from_utf8_lossy(&c);
            c.starts_with(clash::MIHOMO) && c.contains(TEST_DIR)
        });
        if ours {
            // SAFETY: a plain signal to a process just identified as the leftover core.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
    let _ = fs::remove_dir_all(TEST_DIR);
}
fn start_test_core(profile: &Mapping, listeners: usize) -> Result<TestCore, String> {
    use std::os::unix::fs::DirBuilderExt;
    const FAILED: &str = "Cannot start the test core for the node measurements";
    sweep_test_core();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(TEST_DIR)
        .map_err(|_| FAILED)?;
    let config = format!("{TEST_DIR}/config.yaml");
    let text = serde_norway::to_string(profile).map_err(|_| FAILED)?;
    // The profile holds the node's credentials: private file in a private directory.
    storage::atomic_write(Path::new(&config), text.as_bytes()).map_err(|_| FAILED)?;
    let child = Command::new(clash::MIHOMO)
        .args(["-d", TEST_DIR, "-f", &config])
        .env("GOMEMLIMIT", "64MiB")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| FAILED)?;
    let _ = fs::write(format!("{TEST_DIR}/pid"), child.id().to_string());
    let mut core = TestCore(child);
    let deadline = Instant::now() + Duration::from_secs(12);
    let up = |i: usize| {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, TEST_PORT + i as u16));
        TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
    };
    while Instant::now() < deadline {
        if !matches!(core.0.try_wait(), Ok(None)) {
            break;
        }
        if (0..listeners).all(up) {
            return Ok(core);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(FAILED.into())
}
/// One finalist through the node, by its listener: the best time to first byte of a few
/// new HTTPS requests, then a download of `bytes` (none when 0). Also the bytes received.
fn through(port: u16, bytes: u64) -> (Option<u32>, Option<u32>, u64) {
    let proxy = format!("http://127.0.0.1:{port}");
    let mut ttfb: Option<u32> = None;
    for _ in 0..TTFB_TRIES {
        let out = curl(
            &[
                "-s",
                "-o",
                "/dev/null",
                "-m",
                "8",
                "-x",
                &proxy,
                "-w",
                "%{http_code} %{time_starttransfer}",
                clash::DELAY_URL,
            ],
            10,
        );
        match parse_ttfb(&out) {
            Some(ms) => ttfb = Some(ttfb.map_or(ms, |best| best.min(ms))),
            // An address that does not carry the node is not asked again.
            None if ttfb.is_none() => break,
            None => {}
        }
    }
    if ttfb.is_none() || bytes == 0 {
        return (ttfb, None, 0);
    }
    let url = format!("https://speed.cloudflare.com/__down?bytes={bytes}");
    let out = curl(
        &[
            "-s",
            "-o",
            "/dev/null",
            "-m",
            "12",
            "-x",
            &proxy,
            "-w",
            "%{http_code} %{speed_download} %{size_download}",
            &url,
        ],
        15,
    );
    let (kbps, size) = parse_speed(&out, bytes);
    (ttfb, kbps, size)
}
/// Which of `verified` (fastest edge handshake first) go through the node: the addresses
/// in `keep` (in use, backup, DNS answer), then `FINALISTS` new ones, one per data center
/// first. The leg from the data center to the node's server is what separates them, so
/// several addresses of one data center say less than one address of each.
fn finalists(verified: &[Probe], keep: &[&str]) -> Vec<usize> {
    let mut out: Vec<usize> = (0..verified.len())
        .filter(|&i| keep.contains(&verified[i].ip.as_str()))
        .collect();
    let kept = out.len();
    let mut colos: HashSet<&str> = HashSet::new();
    for (i, p) in verified.iter().enumerate() {
        if out.len() - kept < FINALISTS && !out.contains(&i) && colos.insert(p.colo.as_str()) {
            out.push(i);
        }
    }
    for i in 0..verified.len() {
        if out.len() - kept < FINALISTS && !out.contains(&i) {
            out.push(i);
        }
    }
    out
}
/// Whether `held` keeps its place against the best `other`: the other must be clearly
/// faster, by a share and by an absolute gap, to take it.
fn stays(held: u32, other: u32) -> bool {
    f64::from(other) * TTFB_MARGIN > f64::from(held) || other + TTFB_MARGIN_MS > held
}
/// The pick and the backup among the finalists, as indexes. An address that did not carry
/// a request is out, and with downloads measured so is one far behind the best download.
/// Of the rest the lowest time to first byte wins, except that the addresses in use stay
/// unless clearly beaten.
fn select(
    tested: &[Probe],
    current: Option<&str>,
    backup: Option<&str>,
    speed_test: bool,
) -> Option<(usize, Option<usize>)> {
    let mut left: Vec<usize> = (0..tested.len())
        .filter(|&i| tested[i].ttfb_ms.is_some())
        .collect();
    let best = left.iter().filter_map(|&i| tested[i].speed_kbps).max();
    if let (true, Some(best)) = (speed_test, best) {
        left.retain(|&i| {
            tested[i]
                .speed_kbps
                .is_some_and(|k| f64::from(k) >= f64::from(best) * SPEED_SHARE)
        });
    }
    left.sort_by_key(|&i| tested[i].ttfb_ms);
    let ttfb = |i: usize| tested[i].ttfb_ms.unwrap_or(u32::MAX);
    let prefer = |left: &[usize], held: Option<&str>| -> Option<usize> {
        let first = *left.first()?;
        let held = left
            .iter()
            .copied()
            .find(|&i| Some(tested[i].ip.as_str()) == held);
        Some(match held {
            Some(h) if stays(ttfb(h), ttfb(first)) => h,
            _ => first,
        })
    };
    let pick = prefer(&left, current)?;
    left.retain(|&i| i != pick);
    Some((pick, prefer(&left, backup)))
}
/// Whether the pick still holds between full runs: its delay through the running core
/// now, against the delay it had when chosen.
fn still_good(reference: u32, now: u32) -> bool {
    f64::from(now) <= f64::from(reference) * HOLD_FACTOR + f64::from(HOLD_SLACK_MS)
}

// ---------- A run ----------

struct Outcome {
    st: State,
    reload: bool,
}
fn measure(s: &Settings, previous: &State) -> Result<Outcome, String> {
    let mut st = previous.clone();
    phase("nodes");
    let map = clash::active_source()?;
    let mut nodes = vec![];
    let mut dns_ips = vec![];
    for n in shaped(&map) {
        let ips: Vec<Ipv4Addr> = resolve(&n.domain, n.port)
            .into_iter()
            .filter(|ip| cloudflare(*ip))
            .collect();
        if !ips.is_empty() {
            if nodes.is_empty() {
                dns_ips = ips;
            }
            nodes.push(n);
        }
    }
    st.nodes = nodes.clone();
    let Some(first) = nodes.first() else {
        return Err("No node in the active subscription runs through Cloudflare".into());
    };
    let current = st
        .applied
        .as_ref()
        .and_then(|p| p.ip.parse::<Ipv4Addr>().ok());
    let mut ips = sample(SAMPLE, &mut Rng::new());
    let held = st
        .backup
        .as_ref()
        .and_then(|p| p.ip.parse::<Ipv4Addr>().ok());
    let mut keep: Vec<Ipv4Addr> = current.into_iter().chain(held).collect();
    keep.extend(dns_ips.iter().take(2));
    keep.extend(
        previous
            .candidates
            .iter()
            .filter_map(|p| p.ip.parse::<Ipv4Addr>().ok()),
    );
    for ip in &keep {
        if !ips.contains(ip) {
            ips.push(*ip);
        }
    }
    phase("probing");
    let reached = tcp_round(&ips, first.port);
    let mut bytes = ips.len() as u64 * 2 * 200;
    let mut short: Vec<Ipv4Addr> = reached.iter().take(SHORTLIST).map(|r| r.0).collect();
    for ip in current
        .iter()
        .chain(held.iter())
        .chain(dns_ips.iter().take(1))
    {
        if !short.contains(ip) {
            short.push(*ip);
        }
    }
    phase("verifying");
    let mut verified: Vec<Probe> = vec![];
    for ip in &short {
        // The faster of two requests: the first may pay for a cold edge.
        let a = trace(first, *ip);
        let b = a.as_ref().and_then(|_| trace(first, *ip));
        bytes += 2 * 7000;
        let best = match (a, b) {
            (Some(a), Some(b)) => Some(if b.tls_ms < a.tls_ms { b } else { a }),
            (a, b) => a.or(b),
        };
        if let Some(p) = best.filter(|p| p.tls_ms.is_some()) {
            verified.push(p);
        }
    }
    verified.sort_by_key(|p| p.tls_ms.unwrap_or(u32::MAX));
    if verified.is_empty() {
        return Err("No Cloudflare address answered; check the mobile data connection".into());
    }
    let cur_ip = current.map(|c| c.to_string());
    let held_ip = held.map(|c| c.to_string());
    let dns_ip = dns_ips.first().map(|d| d.to_string());
    let keep: Vec<&str> = [&cur_ip, &held_ip, &dns_ip]
        .into_iter()
        .filter_map(|ip| ip.as_deref())
        .collect();
    let chosen: Vec<(usize, Ipv4Addr)> = finalists(&verified, &keep)
        .into_iter()
        .filter_map(|i| Some((i, verified[i].ip.parse().ok()?)))
        .collect();
    phase("speed");
    let node = map
        .get("proxies")
        .and_then(Yaml::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(Yaml::as_mapping)
        .find(|m| text(m, "name") == Some(first.name.as_str()))
        .ok_or("No node in the active subscription runs through Cloudflare")?;
    let size = match (s.speed_test, uplink()) {
        (false, _) => 0,
        (true, "wifi") => DOWNLOAD_WIFI,
        (true, _) => DOWNLOAD_CELL,
    };
    {
        let ips: Vec<Ipv4Addr> = chosen.iter().map(|c| c.1).collect();
        let _core = start_test_core(&test_profile(node, &first.domain, &ips), ips.len())?;
        // Whatever goes through first is slow: measured on the device, the first finalist
        // took twice the time to first byte and half the download of the same address
        // tested later, whichever address it was. The first finalist is the one in use,
        // so without this every run would find it beaten. Not counted, only paid for.
        let (_, _, used) = through(TEST_PORT, WARM_UP.min(size));
        bytes += used + TTFB_TRIES as u64 * 6000;
        for (slot, (i, _)) in chosen.iter().enumerate() {
            let (ttfb, kbps, used) = through(TEST_PORT + slot as u16, size);
            bytes += used + TTFB_TRIES as u64 * 6000;
            verified[*i].ttfb_ms = ttfb;
            verified[*i].speed_kbps = kbps;
        }
    }
    let mut tested: Vec<Probe> = chosen.iter().map(|c| verified[c.0].clone()).collect();
    let Some((pick, second)) = select(&tested, cur_ip.as_deref(), held_ip.as_deref(), s.speed_test)
    else {
        return Err("No Cloudflare address worked through the node; kept the current one".into());
    };
    let applied = tested[pick].clone();
    let backup = second.map(|i| tested[i].clone());
    let ip_of = |p: Option<&Probe>| p.map(|p| p.ip.clone());
    let profile_has_it =
        fs::read_to_string(clash::PROFILE).is_ok_and(|p| p.contains(&format!("| {CF_TAG}")));
    let moved = ip_of(previous.applied.as_ref()).as_ref() != Some(&applied.ip);
    let reload = moved
        || ip_of(previous.backup.as_ref()) != ip_of(backup.as_ref())
        || previous.nodes != st.nodes
        || !profile_has_it;
    if moved {
        st.applied_at = now();
    }
    st.baseline = dns_ip
        .as_ref()
        .and_then(|d| tested.iter().find(|p| &p.ip == d))
        .cloned();
    st.applied = Some(applied);
    st.backup = backup;
    tested.sort_by_key(|p| p.ttfb_ms.unwrap_or(u32::MAX));
    st.candidates = tested.into_iter().take(6).collect();
    st.tested = ips.len() as u32;
    st.data_kb = bytes / 1000;
    st.uplink = uplink().into();
    st.full_run = now();
    Ok(Outcome { st, reload })
}
/// The delay of the optimized copy of the first node, through the running core.
fn live_delay(st: &State) -> Option<u32> {
    let n = st.nodes.first()?;
    let url: String = clash::DELAY_URL
        .bytes()
        .map(|b| match b {
            b':' => "%3A".into(),
            b'/' => "%2F".into(),
            _ => (b as char).to_string(),
        })
        .collect();
    let name = format!("{} | {CF_TAG}", n.name);
    clash::ctl(
        "GET",
        &format!(
            "/proxies/{}/delay?url={url}&timeout=6000",
            clash_ctl::segment(&name)
        ),
        None,
        16,
    )
    .ok()
    .and_then(|v| v["delay"].as_u64())
    .filter(|d| *d > 0)
    .and_then(|d| u32::try_from(d).ok())
}
fn optimize() -> Result<(), String> {
    let s = load_settings();
    if !s.enabled {
        return Err("Turn on Cloudflare IP optimization first".into());
    }
    if !clash::stored_enabled() || !clash::core_running() {
        return Err("Clash is not running".into());
    }
    *REJECTED.lock().unwrap() = None;
    let previous = load_state();
    let Outcome { mut st, reload } = match measure(&s, &previous) {
        Ok(o) => o,
        Err(e) => {
            let mut st = load_state();
            st.last_attempt = now();
            st.last_error = Some(e.clone());
            if e.starts_with("No node") {
                st.nodes.clear();
            }
            let _ = save_state(&st);
            return Err(e);
        }
    };
    phase("applying");
    st.last_run = now();
    st.last_attempt = st.last_run;
    st.last_error = None;
    save_state(&st)?;
    if reload {
        let failed = match clash::reload_profile() {
            Err(e) => Some(e),
            Ok(()) if REJECTED.lock().unwrap().is_some() => {
                Some("The core rejected the optimized profile".into())
            }
            Ok(()) => {
                st.ref_delay_ms = live_delay(&st);
                st.ref_delay_ms.is_none().then(|| {
                    "The new address did not work through the node; kept the previous one".into()
                })
            }
        };
        if let Some(e) = failed {
            let mut back = previous;
            back.last_attempt = now();
            back.last_error = Some(e.clone());
            let _ = save_state(&back);
            let _ = clash::reload_profile();
            return Err(e);
        }
    } else {
        // Same addresses: the reference for the checks between full runs is refreshed.
        st.ref_delay_ms = live_delay(&st).or(st.ref_delay_ms);
    }
    save_state(&st)
}
fn spawn() {
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            phase("");
            RUNNING.store(false, Ordering::Release);
        }
    }
    std::thread::spawn(|| {
        let _done = Done;
        if let Err(e) = optimize() {
            clash::log_line(&format!("openui: Cloudflare IP optimization: {e}"));
        }
    });
}
fn next_run(s: &Settings, st: &State) -> Option<u64> {
    if !s.enabled || s.interval_hours == 0 {
        return None;
    }
    Some(
        if st.last_error.is_some() && st.last_attempt >= st.last_run {
            st.last_attempt + RETRY_SECS
        } else {
            st.last_run + u64::from(s.interval_hours) * 3600
        },
    )
}
/// A due run that needs no measuring: the pick was measured on this uplink within the
/// day and still answers through the running core about as fast as when it was chosen.
/// Recorded as a run, so the next one is an interval away. Costs one small request.
fn holds(st: &State, t: u64) -> bool {
    let fresh = st.full_run <= t && t - st.full_run < FULL_EVERY_SECS;
    if st.applied.is_none() || st.last_error.is_some() || st.uplink != uplink() || !fresh {
        return false;
    }
    let (Some(reference), Some(delay)) = (st.ref_delay_ms, live_delay(st)) else {
        return false;
    };
    if !still_good(reference, delay) {
        return false;
    }
    let mut st = st.clone();
    st.last_run = t;
    st.last_attempt = t;
    save_state(&st).is_ok()
}
/// Runs when due while Clash runs; checks every minute. Also runs once the uplink has
/// been a different one than the pick was measured on for two checks in a row: the best
/// address through a Wi-Fi relay is rarely the best on cellular data.
pub fn start() {
    std::thread::spawn(|| {
        sweep_test_core();
        std::thread::sleep(Duration::from_secs(90));
        let mut moved = 0u32;
        loop {
            let (s, st) = (load_settings(), load_state());
            let t = now();
            // A clock set back would otherwise postpone the next run by as much.
            let due = next_run(&s, &st)
                .is_some_and(|n| t >= n || st.last_attempt.max(st.last_run) > t + 3600);
            let elsewhere = s.enabled
                && s.interval_hours != 0
                && st.applied.is_some()
                && !st.uplink.is_empty()
                && st.uplink != uplink()
                && (st.last_error.is_none() || t >= st.last_attempt + RETRY_SECS);
            moved = if elsewhere { moved + 1 } else { 0 };
            if clash::stored_enabled()
                && clash::core_running()
                && !RUNNING.load(Ordering::Acquire)
                && (moved >= 2 || (due && !holds(&st, t)))
            {
                moved = 0;
                spawn();
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    });
}

// ---------- API ----------

fn view() -> Value {
    let (s, st) = (load_settings(), load_state());
    let running = RUNNING.load(Ordering::Acquire);
    let rejected = REJECTED.lock().unwrap().clone();
    let t = now();
    json!({"revision":s.revision,"enabled":s.enabled,"interval_hours":s.interval_hours,
        "intervals":INTERVALS,"speed_test":s.speed_test,"running":running,
        "phase":running.then(|| *PHASE.lock().unwrap()),
        "applied":st.applied,"backup":st.backup,"applied_at":st.applied_at,"baseline":st.baseline,
        "uplink":st.uplink,"full_run_age_s":(st.full_run > 0).then(|| t.saturating_sub(st.full_run)),
        "candidates":st.candidates,"nodes":st.nodes.iter().map(|n| &n.name).collect::<Vec<_>>(),
        "last_run":st.last_run,"last_attempt":st.last_attempt,"error":rejected.or(st.last_error.clone()),
        "next_run":next_run(&s, &st),"tested":st.tested,"data_kb":st.data_kb,
        // Relative to the device clock, which may be hours off; pages show these.
        "last_run_age_s":(st.last_run > 0).then(|| t.saturating_sub(st.last_run)),
        "next_run_in_s":next_run(&s, &st).map(|n| n.saturating_sub(t)),
        "clash_running":clash::stored_enabled() && clash::core_running()})
}
pub fn get() -> (u16, Value) {
    (200, json!({"ok":true,"data":view()}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Update {
    expected_revision: u64,
    enabled: Option<bool>,
    interval_hours: Option<u32>,
    speed_test: Option<bool>,
}
static UPDATE: Mutex<()> = Mutex::new(());
pub(crate) fn set(u: Update) -> Result<Value, String> {
    let _guard = UPDATE.lock().unwrap();
    let old = load_settings();
    if u.expected_revision != old.revision {
        return Err("Configuration changed; refresh and try again".into());
    }
    let mut s = old.clone();
    if let Some(v) = u.enabled {
        s.enabled = v;
    }
    if let Some(v) = u.interval_hours {
        if !INTERVALS.contains(&v) {
            return Err("Invalid optimization interval".into());
        }
        s.interval_hours = v;
    }
    if let Some(v) = u.speed_test {
        s.speed_test = v;
    }
    if s == old {
        return Ok(view());
    }
    s.revision = old.revision.saturating_add(1);
    storage::atomic_write(Path::new(SETTINGS), &serde_json::to_vec(&s).unwrap())
        .map_err(|_| "Cannot save Cloudflare optimization settings")?;
    let running = clash::stored_enabled() && clash::core_running();
    if s.enabled != old.enabled && running && load_state().applied.is_some() {
        // Off takes the optimized copies out of the profile; on puts the last pick back.
        clash::reload_profile()?;
    }
    if s.enabled && !old.enabled && running {
        spawn();
    }
    Ok(view())
}
fn reply(result: Result<Value, String>) -> (u16, Value) {
    match result {
        Ok(v) => (200, json!({"ok":true,"data":v})),
        Err(e) => (
            if e.starts_with("Configuration changed;") {
                409
            } else {
                400
            },
            json!({"ok":false,"error":e}),
        ),
    }
}
pub fn update(body: &[u8]) -> (u16, Value) {
    reply(
        serde_json::from_slice::<Update>(body)
            .map_err(|_| "Invalid request".to_string())
            .and_then(set),
    )
}
/// Starts a run unless one is in progress; either way returns the current state.
pub fn run() -> (u16, Value) {
    reply((|| {
        if !load_settings().enabled {
            return Err("Turn on Cloudflare IP optimization first".into());
        }
        if !(clash::stored_enabled() && clash::core_running()) {
            return Err("Clash is not running".into());
        }
        spawn();
        Ok(view())
    })())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> Mapping {
        clash_config_parse(
            r#"
proxies:
  - {name: CDN, type: vless, server: cdn.example.com, port: 443, uuid: x, network: ws, tls: true,
     servername: cdn.example.com, ws-opts: {path: /p, headers: {Host: cdn.example.com}}}
  - {name: Bare, type: vmess, server: bare.example.com, port: 8443, uuid: x, cipher: auto,
     network: ws, tls: true}
  - {name: Grpc, type: trojan, server: g.example.com, port: 2053, password: x, network: grpc}
  - {name: Reality, type: vless, server: r.example.com, port: 443, uuid: x, tls: true,
     network: grpc, reality-opts: {public-key: k}}
  - {name: Plain, type: vless, server: p.example.com, port: 443, uuid: x, network: ws}
  - {name: Raw, type: vless, server: 1.2.3.4, port: 443, uuid: x, network: ws, tls: true}
  - {name: Odd, type: vless, server: o.example.com, port: 8080, uuid: x, network: ws, tls: true}
proxy-groups:
  - {name: Pick, type: select, proxies: [CDN, Bare, Plain]}
rules: [MATCH,Pick]
"#,
        )
    }
    fn clash_config_parse(s: &str) -> Mapping {
        crate::clash_config::parse(s).unwrap()
    }
    #[test]
    fn only_tls_websocket_or_grpc_domains_on_cloudflare_ports_qualify() {
        let names: Vec<String> = shaped(&profile()).into_iter().map(|n| n.name).collect();
        assert_eq!(names, ["CDN", "Bare", "Grpc"]);
        assert_eq!(shaped(&profile())[1].port, 8443);
    }
    #[test]
    fn applying_keeps_names_working_through_a_fallback_group() {
        let mut map = profile();
        let nodes = shaped(&map);
        assert_eq!(apply_to(&mut map, &nodes, "104.16.1.1", None), 3);
        let proxies = map["proxies"].as_sequence().unwrap();
        let find = |name: &str| {
            proxies
                .iter()
                .filter_map(Yaml::as_mapping)
                .find(|m| text(m, "name") == Some(name))
                .cloned()
                .unwrap()
        };
        let cf = find("CDN | CF优选");
        assert_eq!(text(&cf, "server"), Some("104.16.1.1"));
        assert_eq!(
            text(&find("CDN | 原域名"), "server"),
            Some("cdn.example.com")
        );
        // Missing SNI and Host are filled in from the domain the address replaces.
        let bare = find("Bare | CF优选");
        assert_eq!(text(&bare, "servername"), Some("bare.example.com"));
        assert_eq!(
            bare["ws-opts"]["headers"]["Host"].as_str(),
            Some("bare.example.com")
        );
        assert_eq!(text(&find("Grpc | CF优选"), "sni"), Some("g.example.com"));
        assert!(!find("Grpc | CF优选").contains_key("ws-opts"));
        let groups = map["proxy-groups"].as_sequence().unwrap();
        let g = groups
            .iter()
            .filter_map(Yaml::as_mapping)
            .find(|g| text(g, "name") == Some("CDN"))
            .unwrap();
        assert_eq!(text(g, "type"), Some("fallback"));
        assert_eq!(
            g["proxies"].as_sequence().unwrap().len(),
            2,
            "copy first, then the original"
        );
        // The rest is untouched, and the select group still names "CDN".
        assert!(proxies.iter().any(|p| p["name"].as_str() == Some("Plain")));
        assert_eq!(groups[0]["proxies"][0].as_str(), Some("CDN"));
        // A node whose server changed since the run is left alone.
        let mut moved = profile();
        let mut stale = shaped(&moved);
        stale[0].domain = "old.example.com".into();
        assert_eq!(apply_to(&mut moved, &stale[..1], "104.16.1.1", None), 0);
    }
    #[test]
    fn a_backup_address_sits_between_the_pick_and_the_original() {
        let mut map = profile();
        let nodes = shaped(&map);
        assert_eq!(
            apply_to(&mut map, &nodes[..1], "104.16.1.1", Some("172.64.1.1")),
            1
        );
        let names: Vec<&str> = map["proxies"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|p| p["name"].as_str())
            .collect();
        assert_eq!(
            names[..4],
            ["CDN | CF优选", "CDN | CF备选", "CDN | 原域名", "Bare"]
        );
        let backup = map["proxies"][1].as_mapping().unwrap();
        assert_eq!(text(backup, "server"), Some("172.64.1.1"));
        assert_eq!(text(backup, "servername"), Some("cdn.example.com"));
        let group = map["proxy-groups"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|g| g["name"].as_str() == Some("CDN"))
            .unwrap();
        let order: Vec<&str> = group["proxies"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(Yaml::as_str)
            .collect();
        assert_eq!(order, ["CDN | CF优选", "CDN | CF备选", "CDN | 原域名"]);
        // A backup equal to the pick adds nothing.
        let mut same = profile();
        apply_to(&mut same, &nodes[..1], "104.16.1.1", Some("104.16.1.1"));
        assert_eq!(same["proxies"][1]["name"].as_str(), Some("CDN | 原域名"));
    }
    #[test]
    fn the_test_core_only_listens_on_loopback_for_its_copies() {
        let map = profile();
        let node = map["proxies"][1].as_mapping().unwrap();
        let ips: Vec<Ipv4Addr> = vec!["104.16.1.1".parse().unwrap(), "172.64.1.1".parse().unwrap()];
        let t = test_profile(node, "bare.example.com", &ips);
        let text_of = serde_norway::to_string(&t).unwrap();
        for absent in [
            "tun",
            "external-controller",
            "mixed-port",
            "bare.example.com:",
        ] {
            assert!(!t.contains_key(absent), "{absent}");
        }
        assert_eq!(t["dns"]["enable"].as_bool(), Some(false));
        assert_eq!(t["allow-lan"].as_bool(), Some(false));
        let listeners = t["listeners"].as_sequence().unwrap();
        assert_eq!(listeners.len(), 2);
        for (i, l) in listeners.iter().enumerate() {
            assert_eq!(l["listen"].as_str(), Some("127.0.0.1"));
            assert_eq!(l["port"].as_u64(), Some(u64::from(TEST_PORT) + i as u64));
            assert_eq!(l["proxy"].as_str(), t["proxies"][i]["name"].as_str());
        }
        assert_eq!(t["proxies"][1]["server"].as_str(), Some("172.64.1.1"));
        assert_eq!(
            t["proxies"][1]["servername"].as_str(),
            Some("bare.example.com")
        );
        assert!(text_of.contains("MATCH,DIRECT"));
    }
    #[test]
    fn samples_are_cloudflare_addresses_in_distinct_blocks() {
        let mut rng = Rng(42);
        let ips = sample(SAMPLE, &mut rng);
        assert_eq!(ips.len(), SAMPLE);
        assert!(ips.iter().all(|ip| cloudflare(*ip)));
        let blocks: HashSet<u32> = ips.iter().map(|ip| u32::from(*ip) >> 8).collect();
        assert_eq!(blocks.len(), SAMPLE);
        assert!(ips
            .iter()
            .all(|ip| ip.octets()[3] != 0 && ip.octets()[3] != 255));
        assert!(cloudflare("162.159.45.145".parse().unwrap()));
        assert!(!cloudflare("8.8.8.8".parse().unwrap()));
    }
    #[test]
    fn trace_and_speed_output_parse() {
        let ip: Ipv4Addr = "104.16.1.1".parse().unwrap();
        let out = "fl=1\nh=x\nip=1.2.3.4\ncolo=EWR\nloc=US\n\n200 0.039265 0.226155";
        let p = parse_trace(out, ip).unwrap();
        assert_eq!(
            (p.colo.as_str(), p.connect_ms, p.tls_ms),
            ("EWR", 39, Some(226))
        );
        assert!(parse_trace("\n000 0.000000 0.000000", ip).is_none());
        assert!(parse_trace("colo=EWR\n\n403 0.03 0.2", ip).is_none());
        let size = 2_000_000;
        assert_eq!(
            parse_speed("200 2708269 2000000", size),
            (Some(21666), 2_000_000)
        );
        assert_eq!(parse_speed("000 0 0", size), (None, 0));
        // A download cut off at the time limit still counts once it got far enough.
        assert_eq!(parse_speed("200 50000 300000", size).0, Some(400));
        assert_eq!(parse_speed("000 90000 900000", size).0, Some(720));
        assert_eq!(parse_speed("000 90000 100000", size).0, None);
        assert_eq!(parse_ttfb("204 0.425293"), Some(425));
        assert_eq!(parse_ttfb("200 1.5"), Some(1500));
        assert_eq!(parse_ttfb("000 0.000000"), None);
        assert_eq!(parse_ttfb("502 0.3"), None);
        assert_eq!(parse_ttfb(""), None);
    }
    fn tested(ip: &str, colo: &str, ttfb: Option<u32>, kbps: Option<u32>) -> Probe {
        Probe {
            ip: ip.into(),
            colo: colo.into(),
            connect_ms: 10,
            tls_ms: Some(200),
            speed_kbps: kbps,
            ttfb_ms: ttfb,
        }
    }
    #[test]
    fn finalists_cover_the_addresses_in_use_and_one_per_data_center_first() {
        let v: Vec<Probe> = [
            ("1.0.0.1", "NRT"),
            ("1.0.0.2", "NRT"),
            ("1.0.0.3", "NRT"),
            ("1.0.0.4", "SIN"),
            ("1.0.0.5", "NRT"),
            ("1.0.0.6", "HKG"),
            ("1.0.0.7", "NRT"),
            ("1.0.0.8", "LAX"),
        ]
        .iter()
        .map(|(ip, colo)| tested(ip, colo, None, None))
        .collect();
        let ips = |keep: &[&str]| -> Vec<&str> {
            finalists(&v, keep)
                .into_iter()
                .map(|i| v[i].ip.as_str())
                .collect()
        };
        // One per data center, in edge order; the kept ones come on top of the four.
        assert_eq!(ips(&[]), ["1.0.0.1", "1.0.0.4", "1.0.0.6", "1.0.0.8"]);
        assert_eq!(
            ips(&["1.0.0.5", "9.9.9.9"]),
            ["1.0.0.5", "1.0.0.1", "1.0.0.4", "1.0.0.6", "1.0.0.8"]
        );
        // Fewer data centers than places: the fastest remaining fill up.
        assert_eq!(
            finalists(&v[..3], &[]).len().min(FINALISTS),
            3,
            "all of a short list"
        );
        let two: Vec<Probe> = v[..5].to_vec();
        let picked: Vec<&str> = finalists(&two, &[])
            .into_iter()
            .map(|i| two[i].ip.as_str())
            .collect();
        assert_eq!(picked, ["1.0.0.1", "1.0.0.4", "1.0.0.2", "1.0.0.3"]);
    }
    #[test]
    fn the_pick_is_the_fastest_through_the_node_not_at_the_edge() {
        // The measured case: two data centers alike at the edge, one useless through the node.
        let t = vec![
            tested("1.0.0.1", "SIN", Some(2800), Some(300)),
            tested("1.0.0.2", "NRT", Some(770), Some(140_000)),
            tested("1.0.0.3", "NRT", Some(790), Some(146_000)),
            tested("1.0.0.4", "AMS", None, None),
        ];
        assert_eq!(select(&t, None, None, true), Some((1, Some(2))));
        // Without downloads only an address that carried nothing is out.
        assert_eq!(select(&t, None, None, false), Some((1, Some(2))));
        let slow_but_quick = vec![
            tested("1.0.0.1", "A", Some(300), Some(1_000)),
            tested("1.0.0.2", "B", Some(500), Some(40_000)),
        ];
        assert_eq!(select(&slow_but_quick, None, None, true), Some((1, None)));
        assert_eq!(
            select(&slow_but_quick, None, None, false),
            Some((0, Some(1)))
        );
        assert_eq!(select(&t[3..], None, None, true), None);
        // Downloads that all failed do not empty the list.
        let no_speed = vec![
            tested("1.0.0.1", "A", Some(300), None),
            tested("1.0.0.2", "B", Some(500), None),
        ];
        assert_eq!(select(&no_speed, None, None, true), Some((0, Some(1))));
    }
    #[test]
    fn the_addresses_in_use_stay_unless_clearly_beaten() {
        let t = vec![
            tested("1.0.0.1", "A", Some(430), Some(40_000)),
            tested("1.0.0.2", "A", Some(470), Some(40_000)),
            tested("1.0.0.3", "A", Some(480), Some(40_000)),
            tested("1.0.0.4", "A", Some(900), Some(40_000)),
        ];
        // Within the margin: both the pick and the backup in use are kept.
        assert_eq!(
            select(&t, Some("1.0.0.2"), Some("1.0.0.3"), true),
            Some((1, Some(2)))
        );
        // Clearly beaten: replaced, and a backup that is gone is replaced too.
        assert_eq!(
            select(&t, Some("1.0.0.4"), Some("9.9.9.9"), true),
            Some((0, Some(1)))
        );
        // The pick in use dropped out for its download: the next best takes over.
        let mut slow = t.clone();
        slow[1].speed_kbps = Some(2_000);
        assert_eq!(
            select(&slow, Some("1.0.0.2"), Some("1.0.0.3"), true),
            Some((0, Some(2)))
        );
        assert!(stays(500, 450) && stays(520, 490) && !stays(600, 500));
        // A small absolute gap never moves a fast pick, whatever the ratio.
        assert!(stays(60, 30));
    }
    #[test]
    fn between_full_runs_the_pick_holds_while_its_delay_does() {
        assert!(still_good(400, 420) && still_good(400, 750));
        assert!(!still_good(400, 751) && !still_good(200, 900));
        let saved: State = serde_json::from_str(
            r#"{"applied":{"ip":"1.0.0.1","colo":"DTW","connect_ms":16,"tls_ms":204,"speed_kbps":40059},
                "applied_at":5,"nodes":[],"last_run":7}"#,
        )
        .unwrap();
        // A state file from before the backup and the node measurements still loads.
        assert!(saved.backup.is_none() && saved.uplink.is_empty() && saved.full_run == 0);
        assert_eq!(saved.applied.unwrap().ttfb_ms, None);
    }
    #[test]
    fn schedule_waits_the_interval_or_the_retry() {
        let s = Settings::default();
        let mut st = State {
            last_run: 1000,
            last_attempt: 1000,
            ..State::default()
        };
        assert_eq!(next_run(&s, &st), Some(1000 + 4 * 3600));
        st.last_attempt = 2000;
        st.last_error = Some("x".into());
        assert_eq!(next_run(&s, &st), Some(2000 + RETRY_SECS));
        let manual = Settings {
            interval_hours: 0,
            ..Settings::default()
        };
        assert_eq!(next_run(&manual, &st), None);
        let off = Settings {
            enabled: false,
            ..Settings::default()
        };
        assert_eq!(next_run(&off, &st), None);
        assert_eq!(next_run(&s, &State::default()), Some(4 * 3600));
    }
}
