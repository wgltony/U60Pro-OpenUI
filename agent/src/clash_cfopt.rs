//! Cloudflare IP optimization for Clash nodes that run through Cloudflare (VLESS, VMess
//! or Trojan over WebSocket or gRPC with TLS, on a port Cloudflare proxies). In the spirit
//! of CloudflareSpeedTest, but sized for a mobile link: TCP-connect a few hundred sampled
//! edge addresses, check the fastest with a TLS request for the node's own domain, and
//! speed-test the best three with 2 MB each. The profile then carries, for each such node,
//! a copy pointed at the winner and the original, in a fallback group under the node's
//! own name: groups and rules keep working, and a failing address falls back to DNS.
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
/// Addresses speed-tested, and the size of each test.
const SPEED_TESTS: usize = 3;
const SPEED_BYTES: u64 = 2_000_000;
/// A new address must beat the current one by this much to replace it, so the core is
/// not reloaded (and connections reset) for noise.
const SPEED_MARGIN: f64 = 1.15;
const LATENCY_MARGIN_MS: u32 = 15;
/// Wait after a failed automatic run before trying again.
const RETRY_SECS: u64 = 1800;
/// Name suffixes of the two halves of an optimized node.
pub(crate) const CF_TAG: &str = "CF优选";
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
    tls_ms: Option<u32>,
    speed_kbps: Option<u32>,
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
    applied_at: u64,
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

/// Points each optimized node at `ip`: the node's name becomes a fallback group of a copy
/// on `ip` and the original. Returns how many nodes changed.
pub(crate) fn apply_to(map: &mut Mapping, nodes: &[Node], ip: &str) -> usize {
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
        let (cf, dns) = (format!("{name} | {CF_TAG}"), format!("{name} | {DNS_TAG}"));
        match (wanted, shape) {
            (Some(n), Some(s)) if n == s && !names.contains(&cf) && !names.contains(&dns) => {
                let mut original = m.clone();
                original.insert("name".into(), dns.clone().into());
                let mut copy = m.clone();
                copy.insert("name".into(), cf.clone().into());
                copy.insert("server".into(), ip.into());
                // The address replaces the domain, so TLS and WebSocket must still name it.
                let sni = if text(m, "type") == Some("trojan") {
                    "sni"
                } else {
                    "servername"
                };
                if text(m, sni).is_none() {
                    copy.insert(sni.into(), n.domain.clone().into());
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
                        headers.insert("Host".into(), n.domain.clone().into());
                        ws.insert("headers".into(), Yaml::Mapping(headers));
                        copy.insert("ws-opts".into(), Yaml::Mapping(ws));
                    }
                }
                proxies[i] = Yaml::Mapping(copy);
                proxies.insert(i + 1, Yaml::Mapping(original));
                let mut g = Mapping::new();
                g.insert("name".into(), name.into());
                g.insert("type".into(), "fallback".into());
                g.insert(
                    "proxies".into(),
                    Yaml::Sequence(vec![cf.into(), dns.into()]),
                );
                g.insert("url".into(), clash::DELAY_URL.into());
                g.insert("interval".into(), 300.into());
                groups.push(Yaml::Mapping(g));
                i += 2;
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
        Some(p) => apply_to(map, &st.nodes, &p.ip) > 0,
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
    })
}
/// Downloads `SPEED_BYTES` from Cloudflare's speed test at `ip`; kbit/s and bytes received.
fn speed(ip: &str) -> (Option<u32>, u64) {
    let resolve = format!("speed.cloudflare.com:443:{ip}");
    let url = format!("https://speed.cloudflare.com/__down?bytes={SPEED_BYTES}");
    let out = curl(
        &[
            "-s",
            "--resolve",
            &resolve,
            "-m",
            "10",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code} %{speed_download} %{size_download}",
            &url,
        ],
        13,
    );
    parse_speed(&out)
}
fn parse_speed(out: &str) -> (Option<u32>, u64) {
    let mut f = out.split_whitespace();
    let code = f.next().unwrap_or("");
    let rate = f.next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let size = f.next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) as u64;
    // A timed-out download still measured something once it got far enough.
    let ok = (code == "200" || size >= SPEED_BYTES / 10) && rate > 0.0;
    (ok.then(|| (rate * 8.0 / 1000.0).round() as u32), size)
}
/// The pick between the current address and the best new one: the current one stays
/// unless the new one is clearly better.
fn choose(current: Option<&Probe>, best: &Probe, speed_test: bool) -> bool {
    let Some(cur) = current else { return true };
    if cur.ip == best.ip {
        return false;
    }
    match (
        speed_test,
        cur.speed_kbps,
        best.speed_kbps,
        cur.tls_ms,
        best.tls_ms,
    ) {
        (true, Some(c), Some(b), _, _) => f64::from(b) >= f64::from(c) * SPEED_MARGIN,
        (true, None, Some(_), _, _) => true,
        (_, _, _, Some(c), Some(b)) => b + LATENCY_MARGIN_MS <= c,
        _ => false,
    }
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
    let mut keep: Vec<Ipv4Addr> = current.into_iter().collect();
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
    for ip in current.iter().chain(dns_ips.iter().take(1)) {
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
    if s.speed_test {
        phase("speed");
        let mut tests: Vec<usize> = (0..verified.len().min(SPEED_TESTS)).collect();
        if let Some(i) = verified.iter().position(|p| Some(&p.ip) == cur_ip.as_ref()) {
            if !tests.contains(&i) {
                tests.pop();
                tests.push(i);
            }
        }
        for i in tests {
            let (kbps, size) = speed(&verified[i].ip);
            bytes += size;
            verified[i].speed_kbps = kbps;
        }
    }
    let best = if s.speed_test {
        verified
            .iter()
            .filter(|p| p.speed_kbps.is_some())
            .max_by_key(|p| p.speed_kbps)
            .unwrap_or(&verified[0])
    } else {
        &verified[0]
    }
    .clone();
    let cur = verified.iter().find(|p| Some(&p.ip) == cur_ip.as_ref());
    let switch = choose(cur, &best, s.speed_test);
    let applied = if switch {
        best
    } else {
        cur.cloned().unwrap_or(best)
    };
    let profile_has_it =
        fs::read_to_string(clash::PROFILE).is_ok_and(|p| p.contains(&format!("| {CF_TAG}")));
    let reload = previous.applied.as_ref().map(|p| &p.ip) != Some(&applied.ip)
        || previous.nodes != st.nodes
        || !profile_has_it;
    if previous.applied.as_ref().map(|p| &p.ip) != Some(&applied.ip) {
        st.applied_at = now();
    }
    st.baseline = dns_ips
        .first()
        .and_then(|d| verified.iter().find(|p| p.ip == d.to_string()))
        .cloned();
    st.applied = Some(applied);
    st.candidates = verified.into_iter().take(5).collect();
    st.tested = ips.len() as u32;
    st.data_kb = bytes / 1000;
    Ok(Outcome { st, reload })
}
/// The delay of the optimized copy of the first node, through the core.
fn works(st: &State) -> bool {
    let Some(n) = st.nodes.first() else {
        return false;
    };
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
    .is_ok_and(|v| v["delay"].as_u64().is_some_and(|d| d > 0))
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
            Ok(()) if !works(&st) => {
                Some("The new address did not work through the node; kept the previous one".into())
            }
            Ok(()) => None,
        };
        if let Some(e) = failed {
            let mut back = previous;
            back.last_attempt = now();
            back.last_error = Some(e.clone());
            let _ = save_state(&back);
            let _ = clash::reload_profile();
            return Err(e);
        }
    }
    Ok(())
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
/// Runs when due while Clash runs; checks every minute.
pub fn start() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(90));
        loop {
            let (s, st) = (load_settings(), load_state());
            let t = now();
            // A clock set back would otherwise postpone the next run by as much.
            let due = next_run(&s, &st)
                .is_some_and(|n| t >= n || st.last_attempt.max(st.last_run) > t + 3600);
            if due && clash::stored_enabled() && clash::core_running() {
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
        "applied":st.applied,"applied_at":st.applied_at,"baseline":st.baseline,
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
        assert_eq!(apply_to(&mut map, &nodes, "104.16.1.1"), 3);
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
        assert_eq!(apply_to(&mut moved, &stale[..1], "104.16.1.1"), 0);
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
        assert_eq!(parse_speed("200 2708269 2000000"), (Some(21666), 2_000_000));
        assert_eq!(parse_speed("000 0 0"), (None, 0));
        // A download cut off at the time limit still counts once it got far enough.
        assert_eq!(parse_speed("200 50000 300000").0, Some(400));
        assert_eq!(parse_speed("000 90000 900000").0, Some(720));
    }
    #[test]
    fn the_current_address_stays_unless_clearly_beaten() {
        let p = |ip: &str, tls: u32, kbps: Option<u32>| Probe {
            ip: ip.into(),
            colo: "X".into(),
            connect_ms: 10,
            tls_ms: Some(tls),
            speed_kbps: kbps,
        };
        let cur = p("1.1.1.1", 200, Some(20_000));
        assert!(choose(None, &p("2.2.2.2", 300, None), true));
        assert!(!choose(Some(&cur), &p("2.2.2.2", 150, Some(22_000)), true));
        assert!(choose(Some(&cur), &p("2.2.2.2", 250, Some(23_000)), true));
        assert!(!choose(Some(&cur), &cur, true));
        assert!(!choose(Some(&cur), &p("2.2.2.2", 190, None), false));
        assert!(choose(Some(&cur), &p("2.2.2.2", 180, None), false));
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
