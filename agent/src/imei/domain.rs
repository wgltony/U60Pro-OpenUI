//! Pure validators. NV/EFS layout derived from the MIT Sadykov reference pinned
//! at 2f7e24c49125ba06d375def56ea5f3e7c1f770aa; see tools/imei-native provenance.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub(super) type Result<T> = std::result::Result<T, &'static str>;
pub(super) const ROUTER_HASH: &str =
    "55c54f74aaa427940254a2f16c36771e675a80a002363e4f10b0dfcb604d9c6f";
pub(super) const SOURCE: &str = "2f7e24c49125ba06d375def56ea5f3e7c1f770aa";
pub(super) const MAX_REVISION: u64 = 9_007_199_254_740_991;
pub(super) const CONFIG_SIZE: usize = 15073;
pub(super) const MAX_FILE: usize = 65536;

pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn unhex(value: &str, size: usize) -> Result<Vec<u8>> {
    if !hex(value, size * 2) {
        return Err("helper_protocol_invalid");
    }
    (0..size)
        .map(|i| {
            u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|_| "helper_protocol_invalid")
        })
        .collect()
}
pub(super) fn digits(value: &str) -> bool {
    value.len() == 15 && value.bytes().all(|b| b.is_ascii_digit())
}
pub(super) fn valid_imei(value: &str) -> bool {
    digits(value)
        && value.bytes().any(|b| b != b'0')
        && value
            .bytes()
            .enumerate()
            .map(|(i, b)| {
                let n = (b - b'0') as u32 * if i % 2 == 1 { 2 } else { 1 };
                if n > 9 {
                    n - 9
                } else {
                    n
                }
            })
            .sum::<u32>()
            % 10
            == 0
}
pub(super) fn masked(value: &str) -> String {
    format!("***********{}", &value[11..])
}
pub(super) fn decode(record: &[u8]) -> Result<String> {
    if record.len() != 128 || record[0] != 8 || record[1] & 15 != 10 {
        return Err("original_validation_failed");
    }
    let mut ds = vec![record[1] >> 4];
    for b in &record[2..9] {
        ds.push(b & 15);
        ds.push(b >> 4);
    }
    if ds.iter().any(|b| *b > 9) {
        return Err("original_validation_failed");
    }
    let s: String = ds.iter().map(|b| char::from(b'0' + *b)).collect();
    if !valid_imei(&s) {
        return Err("original_validation_failed");
    }
    Ok(s)
}
pub(super) fn encode(value: &str, original: &[u8]) -> Result<Vec<u8>> {
    decode(original)?;
    if !valid_imei(value) {
        return Err("invalid_target");
    }
    let ds: Vec<_> = value.bytes().map(|b| b - b'0').collect();
    let mut r = original.to_vec();
    r[0] = 8;
    r[1] = ds[0] << 4 | 10;
    for i in 0..7 {
        r[i + 2] = ds[1 + 2 * i] | ds[2 + 2 * i] << 4;
    }
    Ok(r)
}
fn u32le(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().expect("checked bounds"))
}
pub(super) fn config_crc(d: &[u8]) -> u32 {
    let mut crc = 0u32;
    for (i, b) in d.iter().enumerate() {
        crc ^= u32::from(if (12..16).contains(&i) { 0 } else { *b }) << 24;
        for _ in 0..8 {
            crc = if crc & 0x80000000 != 0 {
                (crc << 1) ^ 0x04c11db7
            } else {
                crc << 1
            };
        }
    }
    crc
}
pub(super) fn config_flag(d: &[u8]) -> Result<u8> {
    if d.len() != CONFIG_SIZE
        || u32le(d, 0) != 0x78563412
        || u32le(d, 4) != 249
        || u32le(d, 8) != CONFIG_SIZE as u32
        || u32le(d, 12) != config_crc(d)
        || u32le(d, CONFIG_SIZE - 4) != 0x21436587
    {
        return Err("config_schema_unsupported");
    }
    let mut offset = 16usize;
    let mut flag = None;
    let mut ids = HashSet::new();
    for index in 0..249 {
        if offset + 16 > CONFIG_SIZE - 4 {
            return Err("config_schema_unsupported");
        }
        let id = u32le(d, offset);
        let len = u32le(d, offset + 4) as usize;
        if len < 16
            || len > CONFIG_SIZE - 4 - offset
            || u32le(d, offset + 8) != 0x18080820
            || id > 65535
            || !ids.insert(id)
        {
            return Err("config_schema_unsupported");
        }
        if id == 102 {
            if index != 5
                || offset != 483
                || len != 17
                || u32le(d, offset + 12) != 0
                || d[offset + 16] > 1
            {
                return Err("config_schema_unsupported");
            }
            flag = Some(d[offset + 16]);
        }
        offset += len;
    }
    if offset != CONFIG_SIZE - 4 {
        return Err("config_schema_unsupported");
    }
    flag.ok_or("config_schema_unsupported")
}
pub(super) fn candidate(original: &[u8]) -> Result<Vec<u8>> {
    if config_flag(original)? != 0 {
        return Err("config_already_enabled");
    }
    let mut d = original.to_vec();
    d[499] = 1;
    let crc = config_crc(&d);
    d[12..16].copy_from_slice(&crc.to_le_bytes());
    config_flag(&d)?;
    Ok(d)
}

// These types deliberately do not implement Debug. They contain private identity,
// raw NV and full targets and must never pass into generic logging.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub cid: String,
    pub firmware: String,
    pub router: String,
    pub library: String,
    pub manifest: String,
}
impl Identity {
    pub fn validate(&self) -> Result<()> {
        if !hex(&self.cid, 32)
            || !(hex(&self.firmware, 64) || self.firmware == "absent")
            || self.router != ROUTER_HASH
            || !hex(&self.library, 64)
            || !hex(&self.manifest, 64)
        {
            return Err("identity_unavailable");
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(super) struct Snapshot {
    pub identity: Identity,
    pub boot: String,
    pub records: Vec<Vec<u8>>,
    pub config: Vec<u8>,
    pub mode: u32,
    pub observed: Vec<String>,
    pub stamp: u64,
}
// Only the pinned native zte_config.c:238-248 live-mode transition is allowed:
// B31 staging recreates 0x8d00 and boot may add 0x20; B04 stays exactly 0x81a4.
// This is for a durable operation's live checks, never preview/source equality.
pub(super) fn live_config_mode_matches(expected: u32, observed: u32) -> bool {
    match expected {
        0x8d00 | 0x8d20 => matches!(observed, 0x8d00 | 0x8d20),
        0x81a4 => observed == 0x81a4,
        _ => false,
    }
}
impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        if !valid_boot(&self.boot)
            || !(1..=2).contains(&self.records.len())
            || self.observed.len() != self.records.len()
        {
            return Err("snapshot_invalid");
        }
        if ![0x8d00, 0x8d20, 0x81a4].contains(&self.mode) {
            return Err("config_mode_unsupported");
        }
        config_flag(&self.config)?;
        for (record, api) in self.records.iter().zip(&self.observed) {
            if decode(record)? != *api {
                return Err("api_readback_mismatch");
            }
        }
        if self.observed.len() == 2 && self.observed[0] == self.observed[1] {
            return Err("original_validation_failed");
        }
        Ok(())
    }
    pub fn same_source(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.boot == other.boot
            && self.records == other.records
            && self.config == other.config
            && self.mode == other.mode
    }
}
pub(super) fn valid_boot(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
#[derive(Clone)]
pub(super) struct Plan {
    pub original: Vec<Vec<u8>>,
    pub target: Vec<Vec<u8>>,
    pub config: Vec<u8>,
    pub candidate: Vec<u8>,
}
impl Plan {
    pub fn from_targets(s: &Snapshot, targets: &[Option<String>]) -> Result<Self> {
        s.validate()?;
        if config_flag(&s.config)? != 0 {
            return Err("config_already_enabled");
        }
        if targets.len() != 2 || (s.records.len() == 1 && targets[1].is_some()) {
            return Err("invalid_targets");
        }
        let target = s
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| match &targets[i] {
                Some(v) => encode(v, r),
                None => Ok(r.clone()),
            })
            .collect::<Result<Vec<_>>>()?;
        let p = Self {
            original: s.records.clone(),
            target,
            config: s.config.clone(),
            candidate: candidate(&s.config)?,
        };
        if p.crosses_other_original()? {
            return Err("invalid_targets");
        }
        p.validate()?;
        if p.original == p.target {
            return Err("unchanged_targets");
        }
        Ok(p)
    }
    // Every per-slot original/target hybrid must remain unique, so a crash
    // between indexed NV writes remains readable by the strict snapshot gate.
    fn crosses_other_original(&self) -> Result<bool> {
        for (i, target) in self.target.iter().enumerate() {
            let value = decode(target)?;
            for (j, original) in self.original.iter().enumerate() {
                if i != j && value == decode(original)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub fn validate(&self) -> Result<()> {
        if !(1..=2).contains(&self.original.len())
            || self.original.len() != self.target.len()
            || candidate(&self.config)? != self.candidate
        {
            return Err("backup_invalid");
        }
        let mut originals = HashSet::new();
        let mut targets = HashSet::new();
        for (o, t) in self.original.iter().zip(&self.target) {
            let original = decode(o)?;
            let v = decode(t)?;
            if o[9..] != t[9..] || !originals.insert(original) || !targets.insert(v) {
                return Err("backup_invalid");
            }
        }
        if self.crosses_other_original()? {
            return Err("backup_invalid");
        }
        Ok(())
    }
    pub fn known(&self, s: &Snapshot) -> Result<()> {
        s.validate()?;
        if s.records.len() != self.original.len()
            || !(s.config == self.config || s.config == self.candidate)
        {
            return Err("unknown_live_state");
        }
        for ((v, o), t) in s.records.iter().zip(&self.original).zip(&self.target) {
            if v != o && v != t {
                return Err("unknown_live_state");
            }
        }
        Ok(())
    }
    pub fn nv_bytes(&self) -> Vec<u8> {
        self.original
            .iter()
            .chain(&self.target)
            .flatten()
            .copied()
            .collect()
    }
    pub fn config_bytes(&self) -> Vec<u8> {
        self.config.iter().chain(&self.candidate).copied().collect()
    }
    pub fn target_masks(&self) -> Vec<String> {
        self.target
            .iter()
            .filter_map(|r| decode(r).ok())
            .map(|v| masked(&v))
            .collect()
    }
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stage {
    Prepared,
    Enabling,
    RebootEnable,
    Writing,
    RestoringConfig,
    RebootVerify,
    Complete,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Direction {
    Change,
    Restore,
    Cancel,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    pub schema: u8,
    pub id: String,
    pub revision: u64,
    pub identity: Identity,
    pub count: usize,
    pub nv_hash: String,
    pub config_hash: String,
    pub mode: u32,
    pub stage: Stage,
    pub direction: Direction,
    pub initial_boot: String,
    pub enable_boot: String,
    pub verify_boot: Option<String>,
    pub reboot_sent_boot: Option<String>,
    pub nv_started: bool,
    pub uncertain: bool,
}
impl Journal {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        if self.schema != 1
            || !hex(&self.id, 32)
            || self.revision == 0
            || self.revision > MAX_REVISION
            || !(1..=2).contains(&self.count)
            || !hex(&self.nv_hash, 64)
            || !hex(&self.config_hash, 64)
            || ![0x8d00, 0x8d20, 0x81a4].contains(&self.mode)
            || !valid_boot(&self.initial_boot)
            || !valid_boot(&self.enable_boot)
            || self.verify_boot.as_ref().is_some_and(|b| !valid_boot(b))
            || self
                .reboot_sent_boot
                .as_ref()
                .is_some_and(|b| !valid_boot(b))
        {
            return Err("journal_invalid");
        }
        if (matches!(self.stage, Stage::RebootVerify | Stage::Complete)
            && self.verify_boot.is_none())
            || (self.stage == Stage::Writing && !self.nv_started)
            || (self.stage == Stage::Complete && self.uncertain)
            || (self.direction == Direction::Cancel && self.nv_started)
            || (self.stage == Stage::RebootVerify
                && self.direction == Direction::Change
                && !self.nv_started)
            || self
                .reboot_sent_boot
                .as_ref()
                .is_some_and(|b| b != &self.enable_boot && Some(b) != self.verify_boot.as_ref())
        {
            return Err("journal_invalid");
        }
        Ok(())
    }
}
