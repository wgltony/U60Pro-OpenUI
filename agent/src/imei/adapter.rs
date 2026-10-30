//! Production adapter: no shell, AT, path override, arbitrary DIAG or raw write.
//! Constructing this object performs no I/O. Every session is inside an explicit
//! queued action and every helper is freshly hash/ownership checked.
use super::domain::*;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const HELPERS: &str = "/data/local/tmp/openui-imei-helpers";
const LIBRARY_HASH: &str = "c0fee4f76b302dd7db2a91dcd5bf1a3f771f674b4637f21468d864a789d00392";
const PROFILE: &str = "mu5250-b31-libdiag-v1";
const MUSL_HASH: &str = "d9e8fd8847efd5f63c7ff6476d8654f187be2a48d59d5551eb7c3df9c70e01dd";
const OUTPUT_LIMIT: usize = 192 * 1024;
#[derive(Clone, Copy)]
pub(super) enum Mutation {
    Enable,
    Apply,
    RestoreConfig,
    RestoreNv,
}
pub(super) trait Adapter: Send + Sync {
    /// Metadata-only deployment exclusion; never probes DIAG or creates state.
    fn guard(&self) -> Result<()> {
        Ok(())
    }
    fn inspect(&self) -> Result<Snapshot>;
    fn identity(&self) -> Result<(Identity, String)>;
    fn mutate(&self, kind: Mutation, nv: &Path, config: &Path) -> Result<()>;
    fn reboot(&self) -> Result<()>;
}
pub(super) struct Production;
#[derive(Deserialize)]
struct Manifest {
    schema: u8,
    profile: String,
    source_commit: String,
    router_sha256: String,
    libdiag_sha256: String,
    platform: String,
    interpreter: String,
    musl_sha256: String,
    helpers: BTreeMap<String, String>,
}
fn ancestors(path: &Path) -> Result<()> {
    for p in path.parent().ok_or("helper_unavailable")?.ancestors() {
        let m = fs::symlink_metadata(p).map_err(|_| "helper_unavailable")?;
        if !m.is_dir() || m.uid() != 0 || m.mode() & 0o022 != 0 {
            return Err("unsafe_helper_path");
        }
    }
    Ok(())
}
fn secure_read(path: &Path, limit: usize, executable: bool) -> Result<Vec<u8>> {
    ancestors(path)?;
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "helper_unavailable")?;
    let b = f.metadata().map_err(|_| "helper_unavailable")?;
    if !b.is_file()
        || b.uid() != 0
        || b.nlink() != 1
        || b.permissions().mode() & 0o022 != 0
        || b.len() > limit as u64
        || (executable && b.mode() & 0o111 == 0)
    {
        return Err("helper_integrity_failed");
    }
    let mut bytes = Vec::new();
    (&mut f)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "helper_unavailable")?;
    let a = f.metadata().map_err(|_| "helper_unavailable")?;
    if bytes.len() > limit
        || bytes.len() as u64 != b.len()
        || a.len() != b.len()
        || a.mtime() != b.mtime()
        || a.ctime() != b.ctime()
        || a.nlink() != 1
    {
        return Err("helper_integrity_failed");
    }
    Ok(bytes)
}
fn runtime_hash(path: &str, limit: usize) -> Result<String> {
    // Stock library/loader can be symlinks. Resolve their fixed destination, then
    // require every resolved ancestor and the regular inode to be root owned.
    let resolved = fs::canonicalize(path).map_err(|_| "profile_unavailable")?;
    if !resolved.starts_with("/usr/lib") && !resolved.starts_with("/lib") {
        return Err("profile_mismatch");
    }
    Ok(hash(&secure_read(&resolved, limit, false)?))
}
fn manifest(bytes: &[u8]) -> Result<Manifest> {
    let m: Manifest = serde_json::from_slice(bytes).map_err(|_| "helper_manifest_invalid")?;
    let names = ["zte_config", "zte_config_read", "zte_nv"];
    if m.schema != 1
        || m.profile != PROFILE
        || m.source_commit != SOURCE
        || m.router_sha256 != ROUTER_HASH
        || m.libdiag_sha256 != LIBRARY_HASH
        || m.platform != "aarch64-linux-musl"
        || m.interpreter != "/lib/ld-musl-aarch64.so.1"
        || m.musl_sha256 != MUSL_HASH
        || m.helpers.len() != 3
        || names
            .iter()
            .any(|n| !m.helpers.get(*n).is_some_and(|h| hex(h, 64)))
    {
        return Err("helper_manifest_invalid");
    }
    Ok(m)
}
fn elf(binary: &[u8]) -> bool {
    // ELF64 little-endian ET_DYN AArch64 and exactly the audited loader.
    if binary.len() < 64
        || &binary[..4] != b"\x7fELF"
        || binary[4] != 2
        || binary[5] != 1
        || binary[6] != 1
        || u16::from_le_bytes([binary[16], binary[17]]) != 3
        || u16::from_le_bytes([binary[18], binary[19]]) != 183
    {
        return false;
    }
    let phoff = u64::from_le_bytes(binary[32..40].try_into().unwrap());
    let entsize = u16::from_le_bytes([binary[54], binary[55]]) as u64;
    let count = u16::from_le_bytes([binary[56], binary[57]]) as u64;
    if entsize < 56 || count == 0 || count > 256 {
        return false;
    }
    let mut interpreter = false;
    for i in 0..count {
        let Some(at) = phoff
            .checked_add(i * entsize)
            .and_then(|n| usize::try_from(n).ok())
        else {
            return false;
        };
        let Some(header) = binary.get(at..at.saturating_add(56)) else {
            return false;
        };
        if u32::from_le_bytes(header[..4].try_into().unwrap()) != 3 {
            continue;
        }
        if interpreter {
            return false;
        }
        let offset = u64::from_le_bytes(header[8..16].try_into().unwrap());
        let size = u64::from_le_bytes(header[32..40].try_into().unwrap());
        let Some(start) = usize::try_from(offset).ok() else {
            return false;
        };
        let Some(end) = offset
            .checked_add(size)
            .and_then(|n| usize::try_from(n).ok())
        else {
            return false;
        };
        if binary.get(start..end) != Some(b"/lib/ld-musl-aarch64.so.1\0".as_slice()) {
            return false;
        }
        interpreter = true;
    }
    interpreter
}
fn profile() -> Result<(Manifest, String, String)> {
    if unsafe { libc::geteuid() } != 0 || unsafe { libc::getuid() } != 0 {
        return Err("root_required");
    }
    let bytes = secure_read(&Path::new(HELPERS).join("manifest.json"), 16384, false)?;
    let m = manifest(&bytes)?;
    let names = ["zte_config", "zte_config_read", "zte_nv"];
    let router = secure_read(Path::new("/usr/bin/diag-router"), 8 * 1024 * 1024, true)?;
    if hash(&router) != ROUTER_HASH {
        return Err("profile_mismatch");
    }
    let library = runtime_hash("/usr/lib/libdiag.so.1", 16 * 1024 * 1024)?;
    if library != LIBRARY_HASH {
        return Err("profile_mismatch");
    }
    // Loader must exist with trusted ownership. No environment-specified loader.
    if runtime_hash("/lib/ld-musl-aarch64.so.1", 8 * 1024 * 1024)? != MUSL_HASH
        || runtime_hash("/lib/libc.so", 8 * 1024 * 1024)? != MUSL_HASH
    {
        return Err("profile_mismatch");
    }
    for name in names {
        let binary = secure_read(&Path::new(HELPERS).join(name), 4 * 1024 * 1024, true)?;
        if m.helpers.get(name) != Some(&hash(&binary)) || !elf(&binary) {
            return Err("helper_integrity_failed");
        }
    }
    Ok((m, hash(&bytes), library))
}
fn file_text(path: &str, limit: usize) -> Result<String> {
    // Only fixed kernel-provided identity files; not user paths. Bounded and no
    // output/logging. These are read solely on explicitly submitted actions.
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "identity_unavailable")?;
    let mut b = Vec::new();
    (&mut f)
        .take(limit as u64 + 1)
        .read_to_end(&mut b)
        .map_err(|_| "identity_unavailable")?;
    if b.len() > limit {
        return Err("identity_unavailable");
    }
    String::from_utf8(b)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "identity_unavailable")
}
fn measured() -> Result<(Identity, String)> {
    let (_, manifest, library) = profile()?;
    let firmware = match fs::symlink_metadata("/firmware/image/modem.b16") {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Absence is meaningful only with an existent trusted image parent.
            ancestors(Path::new("/firmware/image/modem.b16"))?;
            "absent".to_owned()
        }
        Err(_) => return Err("identity_unavailable"),
        Ok(_) => hash(&secure_read(
            Path::new("/firmware/image/modem.b16"),
            32 * 1024 * 1024,
            false,
        )?),
    };
    let id = Identity {
        cid: file_text("/sys/block/mmcblk0/device/cid", 128)?,
        firmware,
        router: ROUTER_HASH.into(),
        library,
        manifest,
    };
    let boot = file_text("/proc/sys/kernel/random/boot_id", 128)?;
    id.validate()?;
    if !valid_boot(&boot) {
        return Err("identity_unavailable");
    }
    Ok((id, boot))
}
fn helper(name: &str, mode: &str, plan: Option<&Path>) -> Result<String> {
    let permitted = matches!(
        (name, mode),
        ("zte_nv", "--snapshot-available")
            | ("zte_nv", "--apply-plan")
            | ("zte_nv", "--restore-plan-original")
            | ("zte_config_read", "--read-config")
            | ("zte_config", "--enable-flag")
            | ("zte_config", "--restore-original-config")
    );
    if !permitted {
        return Err("helper_mode_refused");
    }
    profile()?;
    if plan.is_some()
        != matches!(
            mode,
            "--apply-plan"
                | "--restore-plan-original"
                | "--enable-flag"
                | "--restore-original-config"
        )
    {
        return Err("helper_mode_refused");
    }
    let mut c = Command::new(Path::new(HELPERS).join(name));
    c.env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "C")
        .arg(mode);
    if let Some(p) = plan {
        if !p.is_absolute() || !p.starts_with("/data/openui-imei/operations") {
            return Err("unsafe_plan_path");
        }
        c.arg(p);
    }
    let out = process_runner::output(&mut c, None, Duration::from_secs(180), OUTPUT_LIMIT)
        .map_err(|_| "helper_transport_uncertain")?;
    // Raw stdout and stderr intentionally never enter logs or error strings.
    let text = String::from_utf8(out.stdout).map_err(|_| "helper_protocol_invalid")?;
    if text.lines().any(|l| l.len() > 4096) {
        return Err("helper_protocol_invalid");
    }
    if !out.status.success() {
        if name == "zte_nv"
            && mode == "--snapshot-available"
            && out.status.code() == Some(23)
            && require_fields(
                &text,
                "PAIR_RESULT ",
                &[
                    ("outcome", "23"),
                    ("released", "1"),
                    ("deinit", "1"),
                    ("signal", "0"),
                ],
            )
            .is_ok()
        {
            return Err("original_validation_failed");
        }
        return Err("helper_failed");
    }
    Ok(text)
}
fn fields<'a>(line: &'a str, prefix: &str) -> Result<HashMap<&'a str, &'a str>> {
    let mut map = HashMap::new();
    for part in line
        .strip_prefix(prefix)
        .ok_or("helper_protocol_invalid")?
        .split(' ')
    {
        let (k, v) = part.split_once('=').ok_or("helper_protocol_invalid")?;
        if k.is_empty() || v.is_empty() || map.insert(k, v).is_some() {
            return Err("helper_protocol_invalid");
        }
    }
    Ok(map)
}
fn one_line<'a>(text: &'a str, prefix: &str) -> Result<&'a str> {
    let mut lines = text.lines().filter(|l| l.starts_with(prefix));
    let line = lines.next().ok_or("helper_protocol_invalid")?;
    if lines.next().is_some() {
        return Err("helper_protocol_invalid");
    }
    Ok(line)
}
fn require_fields(text: &str, prefix: &str, expected: &[(&str, &str)]) -> Result<()> {
    let m = fields(one_line(text, prefix)?, prefix)?;
    if expected.iter().any(|(k, v)| m.get(k) != Some(v)) {
        return Err("helper_cleanup_unconfirmed");
    }
    Ok(())
}
fn parse_nv(text: &str) -> Result<Vec<Vec<u8>>> {
    let count = match one_line(text, "APP_NV_TOPOLOGY ")? {
        "APP_NV_TOPOLOGY count=1 secondary=not-active-status5" => 1,
        "APP_NV_TOPOLOGY count=2 secondary=active" => 2,
        _ => return Err("helper_protocol_invalid"),
    };
    let mut records: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    for l in text.lines().filter(|l| l.starts_with("APP_NV ")) {
        let m = fields(l, "APP_NV ")?;
        if m.len() != 2 {
            return Err("helper_protocol_invalid");
        }
        let i = match m.get("index") {
            Some(&"0") => 0,
            Some(&"1") => 1,
            _ => return Err("helper_protocol_invalid"),
        };
        let record = unhex(m.get("data").ok_or("helper_protocol_invalid")?, 128)?;
        decode(&record)?;
        if records.insert(i, record).is_some() {
            return Err("helper_protocol_invalid");
        }
    }
    if records.len() != count
        || !records.contains_key(&0)
        || (count == 2 && !records.contains_key(&1))
    {
        return Err("helper_protocol_invalid");
    }
    // NV emits APP records only after released + DeInit and outcome zero. The
    // final summary must independently agree (not merely a grep for VERIFY OK).
    require_fields(
        text,
        "PAIR_RESULT ",
        &[
            ("outcome", "0"),
            ("released", "1"),
            ("deinit", "1"),
            ("signal", "0"),
        ],
    )?;
    Ok(records.into_values().collect())
}
fn parse_config(text: &str) -> Result<(Vec<u8>, u32)> {
    let mut d = Vec::new();
    for l in text.lines().filter(|l| l.starts_with("EFS_DATA_HEX ")) {
        let m = fields(l, "EFS_DATA_HEX ")?;
        if m.len() != 3 {
            return Err("helper_protocol_invalid");
        }
        let off = m
            .get("offset")
            .and_then(|v| v.parse::<usize>().ok())
            .ok_or("helper_protocol_invalid")?;
        let len = m
            .get("length")
            .and_then(|v| v.parse::<usize>().ok())
            .ok_or("helper_protocol_invalid")?;
        if off != d.len() || len == 0 || len > 1024 || len > CONFIG_SIZE - d.len() {
            return Err("helper_protocol_invalid");
        }
        d.extend(unhex(m.get("data").ok_or("helper_protocol_invalid")?, len)?);
    }
    require_fields(
        text,
        "EFS_FILE_COMPLETE ",
        &[("path", "/config"), ("length", "15073")],
    )?;
    require_fields(
        text,
        "EFS_RESULT ",
        &[
            ("outcome", "0"),
            ("bytes", "15073"),
            ("file_closed", "1"),
            ("released", "1"),
            ("deinit", "1"),
            ("signal", "0"),
            ("bad_crc_frames", "0"),
        ],
    )?;
    let stats: Vec<_> = text
        .lines()
        .filter(|l| l.starts_with("EFS_STAT "))
        .collect();
    if stats.len() != 2 {
        return Err("helper_protocol_invalid");
    }
    let mut mode = None;
    for l in stats {
        let m = fields(l, "EFS_STAT ")?;
        if m.get("errno") != Some(&"0")
            || m.get("size") != Some(&"15073")
            || m.get("nlink") != Some(&"1")
        {
            return Err("config_schema_unsupported");
        }
        let v = m
            .get("mode")
            .and_then(|v| v.strip_prefix("0x"))
            .and_then(|s| u32::from_str_radix(s, 16).ok())
            .ok_or("helper_protocol_invalid")?;
        if mode.is_some_and(|old| old != v) {
            return Err("snapshot_changed");
        }
        mode = Some(v);
    }
    config_flag(&d)?;
    Ok((d, mode.ok_or("helper_protocol_invalid")?))
}
fn api(method: &str) -> Result<String> {
    let mut c = Command::new("/bin/ubus");
    c.env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .args(["-t", "5", "call", "zwrt_zte_mdm.api", method, "{}"]);
    let out = process_runner::output(&mut c, None, Duration::from_secs(7), 4096)
        .map_err(|_| "api_unavailable")?;
    if !out.status.success() {
        return Err("api_unavailable");
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| "api_readback_mismatch")?;
    let obj = v.as_object().ok_or("api_readback_mismatch")?;
    let values: Vec<_> = obj
        .values()
        .filter_map(Value::as_str)
        .filter(|v| digits(v))
        .collect();
    if values.len() != 1 {
        return Err("api_readback_mismatch");
    }
    Ok(values[0].to_owned())
}
fn deployment_guard() -> Result<()> {
    let active = Path::new("/data/local/tmp/open-u60-transactions/active");
    match fs::symlink_metadata(active) {
        Ok(_) => Err("deployment_active"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Missing authority is safe only if no existing ancestor is a
            // symlink/non-directory or writable by an unprivileged user.
            for p in active
                .parent()
                .ok_or("deployment_state_unsafe")?
                .ancestors()
            {
                match fs::symlink_metadata(p) {
                    Ok(m) if m.is_dir() && m.uid() == 0 && m.mode() & 0o022 == 0 => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err("deployment_state_unsafe"),
                }
            }
            Ok(())
        }
        Err(_) => Err("deployment_state_unsafe"),
    }
}
impl Adapter for Production {
    fn guard(&self) -> Result<()> {
        deployment_guard()
    }
    fn identity(&self) -> Result<(Identity, String)> {
        measured()
    }
    fn inspect(&self) -> Result<Snapshot> {
        let (identity, boot) = measured()?;
        let records = parse_nv(&helper("zte_nv", "--snapshot-available", None)?)?;
        let (config, mode) = parse_config(&helper("zte_config_read", "--read-config", None)?)?;
        let mut observed = vec![api("get_imei")?];
        if records.len() == 2 {
            observed.push(api("get_imei2")?);
        }
        let after = measured()?;
        if after != (identity.clone(), boot.clone()) {
            return Err("identity_changed");
        }
        let s = Snapshot {
            identity,
            boot,
            records,
            config,
            mode,
            observed,
            stamp: super::now(),
        };
        s.validate()?;
        Ok(s)
    }
    fn mutate(&self, kind: Mutation, nv: &Path, config: &Path) -> Result<()> {
        let (name, mode, p) = match kind {
            Mutation::Enable => ("zte_config", "--enable-flag", config),
            Mutation::Apply => ("zte_nv", "--apply-plan", nv),
            Mutation::RestoreConfig => ("zte_config", "--restore-original-config", config),
            Mutation::RestoreNv => ("zte_nv", "--restore-plan-original", nv),
        };
        let output = helper(name, mode, Some(p))?;
        if name == "zte_nv" {
            require_fields(
                &output,
                "PAIR_RESULT ",
                &[
                    ("outcome", "0"),
                    ("released", "1"),
                    ("deinit", "1"),
                    ("signal", "0"),
                ],
            )
        } else {
            require_fields(
                &output,
                "CONFIG_RESULT ",
                &[
                    ("outcome", "0"),
                    ("target_verified", "1"),
                    ("transport_uncertain", "0"),
                    ("retained_stage", "0"),
                    ("retained_rollback", "0"),
                    ("released", "1"),
                    ("deinit", "1"),
                    ("signal", "0"),
                ],
            )
        }
    }
    fn reboot(&self) -> Result<()> {
        profile()?;
        let mut c = Command::new("/bin/ubus");
        c.env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .args([
                "-t",
                "5",
                "call",
                "zwrt_mc.device.manager",
                "device_reboot",
                r#"{"moduleName":"web"}"#,
            ]);
        let o = process_runner::output(&mut c, None, Duration::from_secs(8), 4096)
            .map_err(|_| "reboot_unconfirmed")?;
        if o.status.success() {
            Ok(())
        } else {
            Err("reboot_unconfirmed")
        }
    }
}
#[cfg(test)]
#[path = "adapter_tests.rs"]
mod tests;
