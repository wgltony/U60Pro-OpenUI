//! Entirely synthetic tests. This adapter never launches a process or reads
//! production identities, helpers, NV, EFS, credentials or device paths.
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn synthetic_imei(serial: u64) -> String {
    // Synthetic 00000000 TAC: never a real hardware identifier.
    let base = format!("00000000{serial:06}");
    (0..10)
        .map(|n| format!("{base}{n}"))
        .find(|v| valid_imei(v))
        .unwrap()
}
fn record(serial: u64) -> Vec<u8> {
    let value = synthetic_imei(serial);
    let ds: Vec<_> = value.bytes().map(|b| b - b'0').collect();
    let mut r = vec![0x5au8; 128];
    r[0] = 8;
    r[1] = ds[0] << 4 | 10;
    for i in 0..7 {
        r[i + 2] = ds[1 + 2 * i] | ds[2 + 2 * i] << 4;
    }
    r
}
fn put(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn config() -> Vec<u8> {
    let mut d = vec![0u8; CONFIG_SIZE];
    put(&mut d, 0, 0x78563412);
    put(&mut d, 4, 249);
    put(&mut d, 8, CONFIG_SIZE as u32);
    let mut off = 16;
    for i in 0..249 {
        let len = if i == 4 {
            403
        } else if i == 5 {
            17
        } else if i == 248 {
            10697
        } else {
            16
        };
        let id = if i == 5 { 102 } else { 1000 + i as u32 };
        put(&mut d, off, id);
        put(&mut d, off + 4, len as u32);
        put(&mut d, off + 8, 0x18080820);
        off += len;
    }
    assert_eq!(off, CONFIG_SIZE - 4);
    put(&mut d, off, 0x21436587);
    let crc = config_crc(&d);
    put(&mut d, 12, crc);
    assert_eq!(config_flag(&d).unwrap(), 0);
    d
}
fn snapshot(count: usize) -> Snapshot {
    let records = (0..count).map(|n| record(n as u64 + 1)).collect::<Vec<_>>();
    Snapshot {
        identity: Identity {
            cid: "1".repeat(32),
            firmware: "2".repeat(64),
            router: ROUTER_HASH.into(),
            library: "3".repeat(64),
            manifest: "4".repeat(64),
        },
        boot: "00000000-0000-0000-0000-000000000001".into(),
        observed: records.iter().map(|r| decode(r).unwrap()).collect(),
        records,
        config: config(),
        mode: 0x8d00,
        stamp: now(),
    }
}
struct Fake {
    state: Mutex<Snapshot>,
    calls: Mutex<Vec<&'static str>>,
    fail_after: Mutex<Option<&'static str>>,
    inspects: AtomicUsize,
    deployment: AtomicBool,
}
impl Fake {
    fn new(count: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(snapshot(count)),
            calls: Mutex::new(Vec::new()),
            fail_after: Mutex::new(None),
            inspects: AtomicUsize::new(0),
            deployment: AtomicBool::new(false),
        })
    }
    fn boot(&self, n: u8) {
        let mut s = self.state.lock().unwrap();
        s.boot = format!("00000000-0000-0000-0000-{n:012}");
        // Model the pinned B31 boot transition, not arbitrary mode widening.
        if matches!(s.mode, 0x8d00 | 0x8d20) {
            s.mode = 0x8d20;
        }
    }
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}
impl Adapter for Fake {
    fn guard(&self) -> Result<()> {
        if self.deployment.load(Ordering::SeqCst) {
            Err("deployment_active")
        } else {
            Ok(())
        }
    }
    fn inspect(&self) -> Result<Snapshot> {
        self.inspects.fetch_add(1, Ordering::SeqCst);
        let mut s = self.state.lock().unwrap().clone();
        s.stamp = now();
        s.validate()?;
        Ok(s)
    }
    fn identity(&self) -> Result<(Identity, String)> {
        let s = self.state.lock().unwrap();
        Ok((s.identity.clone(), s.boot.clone()))
    }
    fn mutate(&self, kind: Mutation, nv: &Path, cfg: &Path) -> Result<()> {
        let name = match kind {
            Mutation::Enable => "enable",
            Mutation::Apply => "apply",
            Mutation::RestoreConfig => "config_restore",
            Mutation::RestoreNv => "nv_restore",
        };
        self.calls.lock().unwrap().push(name);
        let mut s = self.state.lock().unwrap();
        match kind {
            Mutation::Enable => {
                let d = std::fs::read(cfg).unwrap();
                s.config = d[CONFIG_SIZE..].to_vec();
            }
            Mutation::RestoreConfig => {
                let d = std::fs::read(cfg).unwrap();
                s.config = d[..CONFIG_SIZE].to_vec();
            }
            Mutation::Apply | Mutation::RestoreNv => {
                let d = std::fs::read(nv).unwrap();
                let split = d.len() / 2;
                let selected = if matches!(kind, Mutation::Apply) {
                    &d[split..]
                } else {
                    &d[..split]
                };
                s.records = selected.chunks_exact(128).map(|b| b.to_vec()).collect();
                s.observed = s.records.iter().map(|r| decode(r).unwrap()).collect();
            }
        }
        if matches!(kind, Mutation::Enable | Mutation::RestoreConfig)
            && matches!(s.mode, 0x8d00 | 0x8d20)
        {
            // Pinned native staging uses create_mode 0xd00 for either B31 live mode.
            s.mode = 0x8d00;
        }
        if *self.fail_after.lock().unwrap() == Some(name) {
            return Err("helper_transport_uncertain");
        }
        Ok(())
    }
    fn reboot(&self) -> Result<()> {
        self.calls.lock().unwrap().push("reboot");
        Err("reboot_unconfirmed")
    }
}
struct Fixture {
    controller: Arc<Controller>,
    fake: Arc<Fake>,
    root: PathBuf,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let root = std::env::temp_dir().join(format!("u60-imei-unit-{}", random_id().unwrap()));
        let fake = Fake::new(count);
        let controller = Controller::new(fake.clone(), Store::testing(root.clone()));
        Self {
            controller,
            fake,
            root,
        }
    }
    fn act(&self, action: Action) {
        let work = self.controller.reserve(&action).unwrap();
        self.controller.execute(action, work);
    }
    fn rev(&self) -> u64 {
        self.controller.status()["revision"].as_u64().unwrap()
    }
    fn id(&self) -> String {
        self.controller.status()["operation_id"]
            .as_str()
            .unwrap()
            .into()
    }
    fn check(&self) {
        self.act(Action::Check {
            revision: self.rev(),
        });
        assert_eq!(self.controller.status()["code"], "ready");
    }
    fn preview(&self) {
        self.act(Action::Preview {
            revision: self.rev(),
            targets: vec![
                Some(synthetic_imei(10)),
                if self.fake.state.lock().unwrap().records.len() == 2 {
                    Some(synthetic_imei(11))
                } else {
                    None
                },
            ],
        });
        assert_eq!(self.controller.status()["phase"], "preview");
    }
    fn begin(&self) {
        let token = self.controller.status()["preview_id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.act(Action::Begin {
            revision: self.rev(),
            preview_id: token,
            acknowledge_risk: true,
            confirm_text: "CHANGE IMEI".into(),
        });
    }
    fn continued(&self) {
        self.act(Action::Continue {
            revision: self.rev(),
            operation_id: self.id(),
            acknowledge_risk: true,
            confirm_text: "CHANGE IMEI".into(),
        });
    }
    fn verify(&self) {
        self.act(Action::Verify {
            revision: self.rev(),
            operation_id: self.id(),
        });
    }
    fn reboot(&self) {
        self.act(Action::Reboot {
            revision: self.rev(),
            operation_id: self.id(),
            acknowledge_reboot: true,
        });
    }
    fn started(count: usize) -> Self {
        let f = Self::new(count);
        f.check();
        f.preview();
        f.begin();
        assert_eq!(f.controller.status()["phase"], "reboot_to_enable");
        f
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Test-owned literal randomized child of temp_dir only; no production paths.
        let intended = std::env::temp_dir().join(self.root.file_name().unwrap());
        assert_eq!(self.root, intended);
        assert!(self
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("u60-imei-unit-"));
        if self.root.exists() {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
}
#[test]
fn pure_validation_preserves_tail_and_handles_anomalies() {
    let old = record(1);
    let target = synthetic_imei(10);
    let new = encode(&target, &old).unwrap();
    assert_eq!(new[9..], old[9..]);
    assert_eq!(decode(&new).unwrap(), target);
    assert!(!valid_imei(&"0".repeat(15)));
    assert!(!valid_imei("１２３４５６７８９０１２３４５"));
    assert!(!valid_imei("123"));
    let mut anomalous = old.clone();
    anomalous[8] ^= 0x10;
    assert_eq!(decode(&anomalous).err(), Some("original_validation_failed"));
    let cfg = config();
    let c = candidate(&cfg).unwrap();
    assert_eq!(config_flag(&c).unwrap(), 1);
    for i in 0..CONFIG_SIZE {
        if i != 499 && !(12..16).contains(&i) {
            assert_eq!(cfg[i], c[i]);
        }
    }
    let mut corrupt = cfg;
    corrupt[100] ^= 1;
    assert!(config_flag(&corrupt).is_err());
}
#[test]
fn action_strictness_and_redacted_debug() {
    assert!(
        serde_json::from_str::<Action>(r#"{"action":"check","revision":1,"extra":1}"#).is_err()
    );
    assert!(parse_screen("uimei check 9007199254740992").is_err());
    assert!(parse_screen("uimei  check 1").is_err());
    assert!(parse_screen("uimei check 1\n").is_err());
    assert!(parse_screen("uimei reboot 1 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 0").is_err());
    let a = Action::Preview {
        revision: 1,
        targets: vec![Some(synthetic_imei(10)), None],
    };
    let debug = format!("{a:?}");
    assert!(!debug.contains(&synthetic_imei(10)));
    assert!(parse_screen(&format!("uimei preview 1 {} -", synthetic_imei(10))).is_ok());
}
#[test]
fn status_has_no_io_and_no_worker_automatic_mutation() {
    let f = Fixture::new(2);
    for _ in 0..10 {
        let v = f.controller.status();
        assert_eq!(v["phase"], "idle");
        assert_eq!(v.as_object().unwrap().len(), 15);
    }
    assert_eq!(f.fake.inspects.load(Ordering::SeqCst), 0);
    assert!(f.fake.calls().is_empty());
    assert!(!f.root.exists());
}
#[test]
fn manual_two_reboot_flow_is_not_ack_success() {
    let f = Fixture::started(2);
    assert_eq!(f.fake.calls(), vec!["enable"]);
    f.continued();
    assert_eq!(f.fake.calls(), vec!["enable"]);
    assert_eq!(f.controller.status()["phase"], "reboot_to_enable");
    f.reboot();
    assert_eq!(f.fake.calls(), vec!["enable", "reboot"]);
    assert!(!f.controller.status()["allowed"]
        .as_array()
        .unwrap()
        .contains(&json!("reboot")));
    f.fake.boot(2);
    f.continued();
    assert_eq!(
        f.fake.calls(),
        vec!["enable", "reboot", "apply", "config_restore"]
    );
    assert_eq!(f.controller.status()["phase"], "reboot_to_verify");
    f.verify();
    assert_ne!(f.controller.status()["phase"], "complete");
    f.fake.boot(3);
    f.verify();
    assert_eq!(f.controller.status()["code"], "verified");
    assert_eq!(
        config_flag(&f.fake.state.lock().unwrap().config).unwrap(),
        0
    );
}
#[test]
fn inactive_secondary_keep_and_no_unchanged_or_generated_target() {
    let f = Fixture::new(1);
    f.check();
    let s = f
        .controller
        .state
        .lock()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .clone();
    assert!(Plan::from_targets(&s, &[None, None]).is_err());
    assert!(Plan::from_targets(&s, &[Some(synthetic_imei(10)), Some(synthetic_imei(11))]).is_err());
    f.preview();
    f.begin();
    f.fake.boot(2);
    f.continued();
    f.fake.boot(3);
    f.verify();
    assert_eq!(f.fake.state.lock().unwrap().records.len(), 1);
    assert_eq!(f.controller.status()["secondary"], "inactive");
}
#[test]
fn stale_revision_token_and_busy_never_invoke_adapter() {
    let f = Fixture::new(2);
    f.check();
    f.preview();
    assert!(f
        .controller
        .reserve(&Action::Begin {
            revision: f.rev(),
            preview_id: "0".repeat(32),
            acknowledge_risk: true,
            confirm_text: "CHANGE IMEI".into()
        })
        .is_err());
    assert!(f
        .controller
        .reserve(&Action::Check { revision: 1 })
        .is_err());
    let a = Action::Check { revision: f.rev() };
    let _reserved = f.controller.reserve(&a).unwrap();
    assert!(f.controller.reserve(&a).is_err());
    assert!(f.fake.calls().is_empty());
}
#[test]
fn restart_reads_journal_without_probe_and_never_replays_nv() {
    let f = Fixture::started(2);
    let before = f.fake.inspects.load(Ordering::SeqCst);
    let restarted = Controller::new(f.fake.clone(), Store::testing(f.root.clone()));
    assert_eq!(restarted.status()["phase"], "reboot_to_enable");
    assert_eq!(f.fake.inspects.load(Ordering::SeqCst), before);
    assert_eq!(f.fake.calls(), vec!["enable"]);
    let work = f.controller.state.lock().unwrap();
    let mut j = work.journal.clone().unwrap();
    drop(work);
    j.stage = Stage::Writing;
    j.nv_started = true;
    f.controller.store.save(&j).unwrap();
    let crashed = Controller::new(f.fake.clone(), Store::testing(f.root.clone()));
    assert_eq!(crashed.status()["phase"], "recovery_required");
    assert!(!crashed.status()["allowed"]
        .as_array()
        .unwrap()
        .contains(&json!("continue")));
    assert_eq!(f.fake.calls(), vec!["enable"]);
}
#[test]
fn ambiguous_apply_requires_explicit_restore_no_replay() {
    let f = Fixture::started(2);
    f.fake.boot(2);
    *f.fake.fail_after.lock().unwrap() = Some("apply");
    f.continued();
    assert_eq!(f.controller.status()["phase"], "recovery_required");
    assert_eq!(f.fake.calls(), vec!["enable", "apply"]);
    assert!(!f.controller.status()["allowed"]
        .as_array()
        .unwrap()
        .contains(&json!("continue")));
    *f.fake.fail_after.lock().unwrap() = None;
    f.act(Action::Restore {
        revision: f.rev(),
        operation_id: f.id(),
        acknowledge_risk: true,
        confirm_text: "RESTORE IMEI".into(),
    });
    assert_eq!(
        f.fake.calls(),
        vec!["enable", "apply", "nv_restore", "config_restore"]
    );
    f.fake.boot(3);
    f.verify();
    assert_eq!(f.controller.status()["code"], "restored");
}
#[test]
fn unknown_live_identity_and_backup_tamper_fail_closed() {
    let f = Fixture::started(2);
    f.fake.boot(2);
    f.fake.state.lock().unwrap().identity.cid = "9".repeat(32);
    f.continued();
    assert_eq!(f.fake.calls(), vec!["enable"]);
    let mut s = f.controller.state.lock().unwrap();
    s.journal.as_mut().unwrap().stage = Stage::RebootEnable;
    s.journal.as_mut().unwrap().uncertain = false;
    s.recovered = false;
    s.operation_phase();
    drop(s);
    let id = f.id();
    let path = f.root.join("operations").join(id).join("nv.plan");
    std::fs::write(&path, [0u8; 512]).unwrap();
    assert!(f.controller.store.load().is_err());
    assert_eq!(f.fake.calls(), vec!["enable"]);
}
#[test]
fn cancel_restores_flag_but_waits_for_final_boot() {
    let f = Fixture::started(2);
    f.act(Action::Cancel {
        revision: f.rev(),
        operation_id: f.id(),
        acknowledge_risk: true,
        confirm_text: "CANCEL IMEI".into(),
    });
    assert_eq!(f.fake.calls(), vec!["enable", "config_restore"]);
    assert_eq!(f.controller.status()["phase"], "reboot_to_verify");
    f.fake.boot(2);
    f.verify();
    assert_eq!(f.controller.status()["code"], "cancelled");
}
#[test]
fn deployment_guard_creates_no_state_or_diag_session() {
    let f = Fixture::new(2);
    f.fake.deployment.store(true, Ordering::SeqCst);
    let error = f
        .controller
        .reserve(&Action::Check { revision: f.rev() })
        .err()
        .unwrap();
    assert_eq!(error.1["error"], "deployment_active");
    assert!(!f.root.exists());
    assert_eq!(f.fake.inspects.load(Ordering::SeqCst), 0);
    assert!(f.fake.calls().is_empty());
    // The second gate also covers deployment beginning after reservation.
    f.fake.deployment.store(false, Ordering::SeqCst);
    let action = Action::Check { revision: f.rev() };
    let work = f.controller.reserve(&action).unwrap();
    f.fake.deployment.store(true, Ordering::SeqCst);
    f.controller.execute(action, work);
    assert!(!f.root.exists());
    assert_eq!(f.fake.inspects.load(Ordering::SeqCst), 0);
}
#[test]
fn authority_change_never_overwrites_newer_journal() {
    let f = Fixture::started(2);
    let mut newest = f.controller.store.load().unwrap().unwrap().0;
    newest.revision += 10;
    f.controller.store.save(&newest).unwrap();
    let before = std::fs::read(f.root.join("current.json")).unwrap();
    f.act(Action::Check { revision: f.rev() });
    assert_eq!(f.controller.status()["phase"], "recovery_required");
    assert_eq!(std::fs::read(f.root.join("current.json")).unwrap(), before);
    assert_eq!(f.fake.calls(), vec!["enable"]);
}
#[test]
fn occupied_lease_cannot_probe_or_change_authority() {
    let f = Fixture::started(2);
    let before = std::fs::read(f.root.join("current.json")).unwrap();
    let reads = f.fake.inspects.load(Ordering::SeqCst);
    let lease = f.controller.store.lease().unwrap();
    f.act(Action::Check { revision: f.rev() });
    assert_eq!(std::fs::read(f.root.join("current.json")).unwrap(), before);
    assert_eq!(f.fake.inspects.load(Ordering::SeqCst), reads);
    drop(lease);
}
#[test]
fn expired_preview_and_changed_baseline_never_begin() {
    let f = Fixture::new(2);
    f.check();
    f.preview();
    f.controller
        .state
        .lock()
        .unwrap()
        .preview
        .as_mut()
        .unwrap()
        .created = Instant::now() - std::time::Duration::from_secs(61);
    let token = f
        .controller
        .state
        .lock()
        .unwrap()
        .preview
        .as_ref()
        .unwrap()
        .id
        .clone();
    assert!(f
        .controller
        .reserve(&Action::Begin {
            revision: f.rev(),
            preview_id: token,
            acknowledge_risk: true,
            confirm_text: "CHANGE IMEI".into()
        })
        .is_err());
    assert!(f.fake.calls().is_empty());
    f.check();
    f.preview();
    f.fake.state.lock().unwrap().records[0] = record(3);
    f.fake.state.lock().unwrap().observed[0] = synthetic_imei(3);
    f.begin();
    assert_eq!(f.controller.status()["code"], "snapshot_changed");
    assert!(f.controller.store.load().unwrap().is_none());
    assert!(f.fake.calls().is_empty());
}
#[test]
fn original_anomaly_or_api_mismatch_is_read_only_failure() {
    let f = Fixture::new(2);
    f.fake.state.lock().unwrap().records[0][8] ^= 0x10;
    f.act(Action::Check { revision: f.rev() });
    assert_eq!(f.controller.status()["code"], "original_validation_failed");
    assert!(f.fake.calls().is_empty());
    let f = Fixture::new(2);
    f.fake.state.lock().unwrap().observed[0] = synthetic_imei(3);
    f.act(Action::Check { revision: f.rev() });
    assert_eq!(f.controller.status()["code"], "api_readback_mismatch");
    assert!(f.fake.calls().is_empty());
}
#[test]
fn cross_slot_original_targets_and_unsafe_old_plans_are_rejected() {
    let s = snapshot(2);
    let a = decode(&s.records[0]).unwrap();
    let b = decode(&s.records[1]).unwrap();
    let c = synthetic_imei(10);
    for targets in [
        vec![Some(b.clone()), Some(a.clone())],
        vec![Some(b.clone()), Some(c.clone())],
        vec![Some(c.clone()), Some(a.clone())],
    ] {
        assert_eq!(
            Plan::from_targets(&s, &targets).err(),
            Some("invalid_targets")
        );
    }
    assert!(Plan::from_targets(&s, &[None, Some(c.clone())]).is_ok());
    assert!(Plan::from_targets(&s, &[Some(c), None]).is_ok());
    let safe =
        Plan::from_targets(&s, &[Some(synthetic_imei(10)), Some(synthetic_imei(11))]).unwrap();
    let mut crossed = safe.clone();
    crossed.target[0] = encode(&b, &s.records[0]).unwrap();
    assert_eq!(crossed.validate().err(), Some("backup_invalid"));
    crossed.target[1] = encode(&a, &s.records[1]).unwrap();
    assert_eq!(crossed.validate().err(), Some("backup_invalid"));
    let mut duplicate = safe;
    duplicate.original[1] = duplicate.original[0].clone();
    assert_eq!(duplicate.validate().err(), Some("backup_invalid"));
}
#[test]
fn every_safe_per_slot_hybrid_remains_readable_and_explicitly_restorable() {
    for mask in 0..4 {
        let mut f = Fixture::started(2);
        let p = f.controller.state.lock().unwrap().plan.clone().unwrap();
        let mut j = f.controller.state.lock().unwrap().journal.clone().unwrap();
        j.stage = Stage::Writing;
        j.nv_started = true;
        j.uncertain = true;
        f.controller.store.save(&j).unwrap();
        f.fake.boot(2);
        {
            let mut live = f.fake.state.lock().unwrap();
            live.records = (0..2)
                .map(|i| {
                    if mask & (1 << i) == 0 {
                        p.original[i].clone()
                    } else {
                        p.target[i].clone()
                    }
                })
                .collect();
            live.observed = live.records.iter().map(|r| decode(r).unwrap()).collect();
            live.validate().unwrap();
            p.known(&live).unwrap();
        }
        f.controller = Controller::new(f.fake.clone(), Store::testing(f.root.clone()));
        assert_eq!(f.controller.status()["phase"], "recovery_required");
        f.act(Action::Restore {
            revision: f.rev(),
            operation_id: f.id(),
            acknowledge_risk: true,
            confirm_text: "RESTORE IMEI".into(),
        });
        assert_eq!(f.controller.status()["phase"], "reboot_to_verify");
        f.fake.boot(3);
        f.verify();
        assert_eq!(f.controller.status()["code"], "restored");
        assert_eq!(f.fake.state.lock().unwrap().records, p.original);
        assert_eq!(
            f.fake
                .calls()
                .iter()
                .filter(|c| **c == "nv_restore")
                .count(),
            usize::from(mask != 0)
        );
    }
}
#[test]
fn unsafe_legacy_plan_fails_closed_without_deleting_original_backup() {
    let f = Fixture::started(2);
    let mut p = f.controller.state.lock().unwrap().plan.clone().unwrap();
    let mut j = f.controller.state.lock().unwrap().journal.clone().unwrap();
    p.target[0] = encode(&decode(&p.original[1]).unwrap(), &p.original[0]).unwrap();
    let bytes = p.nv_bytes();
    j.nv_hash = hash(&bytes);
    let (nv, cfg) = f.controller.store.paths(&j.id).unwrap();
    // Synthetic private fixture only: represent an old, hash-consistent plan.
    std::fs::write(&nv, &bytes).unwrap();
    std::fs::write(
        nv.parent().unwrap().join("manifest.json"),
        serde_json::to_vec(&j).unwrap(),
    )
    .unwrap();
    f.controller.store.save(&j).unwrap();
    assert_eq!(f.controller.store.load().err(), Some("backup_invalid"));
    assert_eq!(std::fs::read(&nv).unwrap(), bytes);
    assert!(cfg.is_file());
    assert_eq!(f.fake.calls(), vec!["enable"]);
}
#[test]
fn live_mode_equivalence_is_only_the_audited_native_class() {
    let modes = [0x8d00, 0x8d20, 0x81a4, 0x8d10, 0x8d40, 0x81a5, 0, 0xffff];
    for expected in modes {
        for observed in modes {
            let allowed = matches!(
                (expected, observed),
                (0x8d00 | 0x8d20, 0x8d00 | 0x8d20) | (0x81a4, 0x81a4)
            );
            assert_eq!(live_config_mode_matches(expected, observed), allowed);
        }
    }
    let source = snapshot(1);
    let mut changed = source.clone();
    changed.mode = 0x8d20;
    assert!(!source.same_source(&changed));
}
#[test]
fn audited_live_mode_transitions_allow_full_change_and_restore() {
    for initial in [0x8d00, 0x8d20, 0x81a4] {
        let f = Fixture::new(2);
        f.fake.state.lock().unwrap().mode = initial;
        let original = f.fake.state.lock().unwrap().records.clone();
        let recreated = if initial == 0x81a4 { 0x81a4 } else { 0x8d00 };
        let booted = if initial == 0x81a4 { 0x81a4 } else { 0x8d20 };
        f.check();
        f.preview();
        f.begin();
        assert_eq!(f.controller.status()["phase"], "reboot_to_enable");
        assert_eq!(f.fake.state.lock().unwrap().mode, recreated);
        f.fake.boot(2);
        assert_eq!(f.fake.state.lock().unwrap().mode, booted);
        f.continued();
        assert_eq!(f.controller.status()["phase"], "reboot_to_verify");
        assert_eq!(f.fake.state.lock().unwrap().mode, recreated);
        f.fake.boot(3);
        f.verify();
        assert_eq!(f.controller.status()["code"], "verified");
        assert_eq!(f.fake.state.lock().unwrap().mode, booted);
        f.act(Action::Restore {
            revision: f.rev(),
            operation_id: f.id(),
            acknowledge_risk: true,
            confirm_text: "RESTORE IMEI".into(),
        });
        assert_eq!(f.controller.status()["phase"], "reboot_to_enable");
        assert_eq!(f.fake.state.lock().unwrap().mode, recreated);
        f.fake.boot(4);
        f.continued();
        assert_eq!(f.controller.status()["phase"], "reboot_to_verify");
        assert_eq!(f.fake.state.lock().unwrap().mode, recreated);
        f.fake.boot(5);
        f.verify();
        assert_eq!(f.controller.status()["code"], "restored");
        assert_eq!(f.fake.state.lock().unwrap().mode, booted);
        assert_eq!(f.fake.state.lock().unwrap().records, original);
        assert_eq!(f.controller.store.load().unwrap().unwrap().0.mode, initial);
        assert_eq!(
            f.fake.calls(),
            vec![
                "enable",
                "apply",
                "config_restore",
                "enable",
                "nv_restore",
                "config_restore"
            ]
        );
    }
}
#[test]
fn b31_recovery_after_recreation_allows_explicit_restore() {
    let f = Fixture::new(1);
    f.fake.state.lock().unwrap().mode = 0x8d20;
    f.check();
    f.preview();
    *f.fake.fail_after.lock().unwrap() = Some("enable");
    f.begin();
    assert_eq!(f.controller.status()["phase"], "recovery_required");
    assert_eq!(f.fake.state.lock().unwrap().mode, 0x8d00);
    *f.fake.fail_after.lock().unwrap() = None;
    f.act(Action::Restore {
        revision: f.rev(),
        operation_id: f.id(),
        acknowledge_risk: true,
        confirm_text: "RESTORE IMEI".into(),
    });
    assert_eq!(f.controller.status()["phase"], "reboot_to_verify");
    assert_eq!(f.fake.calls(), vec!["enable", "config_restore"]);
    f.fake.boot(2);
    f.verify();
    assert_eq!(f.controller.status()["code"], "restored");
    assert_eq!(f.fake.state.lock().unwrap().mode, 0x8d20);
}
#[test]
fn unknown_or_cross_class_modes_fail_closed_and_preview_stays_exact() {
    let f = Fixture::new(1);
    f.check();
    f.preview();
    f.fake.state.lock().unwrap().mode = 0x8d20;
    f.begin();
    assert_eq!(f.controller.status()["code"], "snapshot_changed");
    assert!(f.fake.calls().is_empty());
    assert!(f.controller.store.load().unwrap().is_none());
    for (initial, observed, code) in [
        (0x8d00, 0x8d10, "config_mode_unsupported"),
        (0x8d20, 0x8d40, "config_mode_unsupported"),
        (0x81a4, 0x81a5, "config_mode_unsupported"),
        (0x8d00, 0x81a4, "identity_changed"),
        (0x81a4, 0x8d00, "identity_changed"),
    ] {
        let f = Fixture::new(1);
        f.fake.state.lock().unwrap().mode = initial;
        f.check();
        f.preview();
        f.begin();
        f.fake.boot(2);
        f.fake.state.lock().unwrap().mode = observed;
        let calls = f.fake.calls();
        {
            let state = f.controller.state.lock().unwrap();
            assert_eq!(f.controller.inspect_bound(&state).err(), Some(code));
        }
        f.continued();
        assert_eq!(f.controller.status()["code"], "recovery_required");
        assert_eq!(f.fake.calls(), calls);
        f.act(Action::Restore {
            revision: f.rev(),
            operation_id: f.id(),
            acknowledge_risk: true,
            confirm_text: "RESTORE IMEI".into(),
        });
        assert_eq!(f.controller.status()["code"], "recovery_required");
        assert_eq!(f.fake.calls(), calls);
    }
}
#[test]
fn restoration_after_verified_change_requires_two_manual_boots() {
    let f = Fixture::started(2);
    f.fake.boot(2);
    f.continued();
    f.fake.boot(3);
    f.verify();
    f.act(Action::Restore {
        revision: f.rev(),
        operation_id: f.id(),
        acknowledge_risk: true,
        confirm_text: "RESTORE IMEI".into(),
    });
    assert_eq!(f.controller.status()["phase"], "reboot_to_enable");
    let calls = f.fake.calls();
    assert_eq!(calls.iter().filter(|n| **n == "nv_restore").count(), 0);
    f.continued();
    assert_eq!(f.fake.calls(), calls);
    f.fake.boot(4);
    f.continued();
    assert_eq!(f.controller.status()["phase"], "reboot_to_verify");
    f.verify();
    assert_ne!(f.controller.status()["code"], "restored");
    f.fake.boot(5);
    f.verify();
    assert_eq!(f.controller.status()["code"], "restored");
}
#[test]
fn queued_submit_returns_while_worker_is_blocked_and_rejects_duplicate() {
    use std::sync::mpsc::{self, Receiver, SyncSender};
    struct Gate {
        fake: Arc<Fake>,
        started: SyncSender<()>,
        release: Mutex<Receiver<()>>,
    }
    impl Adapter for Gate {
        fn inspect(&self) -> Result<Snapshot> {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            self.fake.inspect()
        }
        fn identity(&self) -> Result<(Identity, String)> {
            self.fake.identity()
        }
        fn mutate(&self, k: Mutation, nv: &Path, cfg: &Path) -> Result<()> {
            self.fake.mutate(k, nv, cfg)
        }
        fn reboot(&self) -> Result<()> {
            self.fake.reboot()
        }
    }
    let f = Fixture::new(2);
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let c = Controller::new(
        Arc::new(Gate {
            fake: f.fake.clone(),
            started: started_tx,
            release: Mutex::new(release_rx),
        }),
        Store::testing(f.root.clone()),
    );
    assert_eq!(
        c.submit(Action::Check { revision: 1 }).unwrap(),
        json!({"ok":true})
    );
    started_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    assert_eq!(c.status()["busy"], true);
    assert!(c
        .submit(Action::Check {
            revision: c.status()["revision"].as_u64().unwrap()
        })
        .is_err());
    assert!(f.fake.calls().is_empty());
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    while c.status()["busy"] == true {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(c.status()["code"], "ready");
}
#[test]
fn reboot_once_marker_survives_restart_without_reboot_replay() {
    let f = Fixture::started(2);
    f.reboot();
    let restarted = Controller::new(f.fake.clone(), Store::testing(f.root.clone()));
    assert!(!restarted.status()["allowed"]
        .as_array()
        .unwrap()
        .contains(&json!("reboot")));
    assert_eq!(f.fake.calls().iter().filter(|n| **n == "reboot").count(), 1);
}
#[test]
fn malformed_journal_stage_cannot_skip_reboot_or_authorize_recovery() {
    let f = Fixture::started(1);
    let baseline = f.controller.state.lock().unwrap().journal.clone().unwrap();
    let mut j = baseline.clone();
    j.stage = Stage::RebootVerify;
    j.verify_boot = None;
    assert!(j.validate().is_err());
    let mut j = baseline.clone();
    j.stage = Stage::Writing;
    j.nv_started = false;
    assert!(j.validate().is_err());
    let mut j = baseline.clone();
    j.stage = Stage::Complete;
    j.verify_boot = Some(j.enable_boot.clone());
    j.uncertain = true;
    assert!(j.validate().is_err());
    let mut j = baseline.clone();
    j.direction = Direction::Cancel;
    j.nv_started = true;
    assert!(j.validate().is_err());
    let mut j = baseline;
    j.reboot_sent_boot = Some("00000000-0000-0000-0000-000000000099".into());
    assert!(j.validate().is_err());
}
#[test]
fn secure_store_rejects_symlinks_hardlinks_and_permissions() {
    let f = Fixture::started(1);
    let j = f.controller.state.lock().unwrap().journal.clone().unwrap();
    let (nv, _) = f.controller.store.paths(&j.id).unwrap();
    let alias = f.root.join("alias");
    std::os::unix::fs::symlink(&f.root, &alias).unwrap();
    assert!(Store::testing(alias).load().is_err());
    let meta = std::fs::symlink_metadata(&nv).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(meta.nlink(), 1);
    std::fs::set_permissions(&nv, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(f.controller.store.load().is_err());
    std::fs::set_permissions(&nv, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&nv, f.root.join("hardlink")).unwrap();
    assert!(f.controller.store.load().is_err());
}
