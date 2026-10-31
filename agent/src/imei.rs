//! Shared, explicitly driven IMEI transaction controller. GET/startup never
//! probe DIAG, reboot or resume writes. Native and HTTP share this one controller.
#[path = "imei/adapter.rs"]
mod adapter;
#[path = "imei/domain.rs"]
mod domain;
#[path = "imei/store.rs"]
mod store;

use adapter::{Adapter, Mutation, Production};
use domain::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use store::{random_id, Store};

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Check {
        revision: u64,
    },
    Preview {
        revision: u64,
        targets: Vec<Option<String>>,
    },
    /// A preview whose targets are the recorded default, not caller input.
    RestoreDefault {
        revision: u64,
    },
    Begin {
        revision: u64,
        preview_id: String,
        acknowledge_risk: bool,
        confirm_text: String,
    },
    Continue {
        revision: u64,
        operation_id: String,
        acknowledge_risk: bool,
        confirm_text: String,
    },
    Verify {
        revision: u64,
        operation_id: String,
    },
    Restore {
        revision: u64,
        operation_id: String,
        acknowledge_risk: bool,
        confirm_text: String,
    },
    Reboot {
        revision: u64,
        operation_id: String,
        acknowledge_reboot: bool,
    },
    Cancel {
        revision: u64,
        operation_id: String,
        acknowledge_risk: bool,
        confirm_text: String,
    },
}
impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImeiAction")
            .field("action", &self.name())
            .finish_non_exhaustive()
    }
}
impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Check { .. } => "check",
            Self::Preview { .. } => "preview",
            Self::RestoreDefault { .. } => "restore_default",
            Self::Begin { .. } => "begin",
            Self::Continue { .. } => "continue",
            Self::Verify { .. } => "verify",
            Self::Restore { .. } => "restore",
            Self::Reboot { .. } => "reboot",
            Self::Cancel { .. } => "cancel",
        }
    }
    fn revision(&self) -> u64 {
        match self {
            Self::Check { revision }
            | Self::Preview { revision, .. }
            | Self::RestoreDefault { revision }
            | Self::Begin { revision, .. }
            | Self::Continue { revision, .. }
            | Self::Verify { revision, .. }
            | Self::Restore { revision, .. }
            | Self::Reboot { revision, .. }
            | Self::Cancel { revision, .. } => *revision,
        }
    }
    fn operation(&self) -> Option<&str> {
        match self {
            Self::Continue { operation_id, .. }
            | Self::Verify { operation_id, .. }
            | Self::Restore { operation_id, .. }
            | Self::Reboot { operation_id, .. }
            | Self::Cancel { operation_id, .. } => Some(operation_id),
            _ => None,
        }
    }
    fn validate(&self) -> Result<()> {
        if self.revision() == 0 || self.revision() > MAX_REVISION {
            return Err("invalid_revision");
        }
        if self.operation().is_some_and(|v| !hex(v, 32)) {
            return Err("invalid_operation");
        }
        match self {
            Self::Preview { targets, .. } => {
                if targets.len() != 2 || targets.iter().flatten().any(|v| !valid_imei(v)) {
                    return Err("invalid_targets");
                }
            }
            Self::Begin {
                preview_id,
                acknowledge_risk,
                confirm_text,
                ..
            } => {
                if !hex(preview_id, 32) {
                    return Err("invalid_preview");
                }
                if !*acknowledge_risk || confirm_text != "CHANGE IMEI" {
                    return Err("confirmation_required");
                }
            }
            Self::Continue {
                acknowledge_risk,
                confirm_text,
                ..
            } => {
                if !*acknowledge_risk || confirm_text != "CHANGE IMEI" {
                    return Err("confirmation_required");
                }
            }
            Self::Restore {
                acknowledge_risk,
                confirm_text,
                ..
            } => {
                if !*acknowledge_risk || confirm_text != "RESTORE IMEI" {
                    return Err("confirmation_required");
                }
            }
            Self::Cancel {
                acknowledge_risk,
                confirm_text,
                ..
            } => {
                if !*acknowledge_risk || confirm_text != "CANCEL IMEI" {
                    return Err("confirmation_required");
                }
            }
            Self::Reboot {
                acknowledge_reboot, ..
            } => {
                if !*acknowledge_reboot {
                    return Err("confirmation_required");
                }
            }
            _ => {}
        }
        Ok(())
    }
}
#[derive(Clone)]
struct Preview {
    id: String,
    source: Snapshot,
    plan: Plan,
    created: Instant,
    epoch: u64,
}
impl Preview {
    fn fresh(&self) -> bool {
        let t = now();
        t >= self.epoch && t - self.epoch <= 60 && self.created.elapsed().as_secs() < 60
    }
}
#[derive(Clone)]
struct State {
    revision: u64,
    busy: bool,
    phase: &'static str,
    code: &'static str,
    snapshot: Option<Snapshot>,
    preview: Option<Preview>,
    journal: Option<Journal>,
    plan: Option<Plan>,
    recovered: bool,
    baseline: Option<Baseline>,
}
impl State {
    fn initial(store: &Store) -> Self {
        let mut s = Self {
            revision: 1,
            busy: false,
            phase: "idle",
            code: "not_checked",
            snapshot: None,
            preview: None,
            journal: None,
            plan: None,
            recovered: false,
            // An unreadable default only hides restore_default; it never blocks.
            baseline: store.baseline().ok().flatten(),
        };
        match store.load() {
            Ok(Some((j, p))) => {
                s.revision = j.revision.saturating_add(1).min(MAX_REVISION);
                s.recovered = j.uncertain
                    || matches!(
                        j.stage,
                        Stage::Prepared | Stage::Enabling | Stage::Writing | Stage::RestoringConfig
                    );
                s.journal = Some(j);
                s.plan = Some(p);
                s.operation_phase();
            }
            Ok(None) => {}
            Err(code) => {
                s.phase = "recovery_required";
                s.code = code;
                s.recovered = true;
            }
        }
        s
    }
    fn operation_phase(&mut self) {
        let Some(j) = &self.journal else {
            return;
        };
        if self.recovered || j.uncertain {
            self.phase = "recovery_required";
            self.code = "recovery_required";
            return;
        }
        match j.stage {
            Stage::RebootEnable => {
                let changed = self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.boot != j.enable_boot);
                self.phase = if changed {
                    "ready_to_write"
                } else {
                    "reboot_to_enable"
                };
                self.code = if changed {
                    "awaiting_write_confirmation"
                } else {
                    "awaiting_first_reboot"
                };
            }
            Stage::RebootVerify => {
                self.phase = "reboot_to_verify";
                self.code = "awaiting_final_reboot";
            }
            Stage::Complete => {
                self.phase = "complete";
                self.code = match j.direction {
                    Direction::Change => "verified",
                    Direction::Restore => "restored",
                    Direction::Cancel => "cancelled",
                };
            }
            _ => {
                self.phase = "recovery_required";
                self.code = "recovery_required";
            }
        }
    }
    fn allowed(&self) -> Vec<&'static str> {
        if self.busy || self.revision >= MAX_REVISION {
            return Vec::new();
        }
        if let Some(j) = &self.journal {
            let mut a = vec!["check"];
            if self.plan.is_none() {
                return a;
            }
            if j.stage == Stage::Complete {
                if j.direction == Direction::Change {
                    a.push("restore");
                }
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| now() >= s.stamp && now() - s.stamp <= 60)
                {
                    a.push("preview");
                    if self.default_targets().is_some() {
                        a.push("restore_default");
                    }
                    if self.preview.as_ref().is_some_and(Preview::fresh) {
                        a.push("begin");
                    }
                }
                return a;
            }
            if j.stage == Stage::RebootVerify {
                a.push("verify");
            }
            if j.stage == Stage::RebootEnable && !j.uncertain && !self.recovered {
                a.push("continue");
            }
            if matches!(j.stage, Stage::RebootEnable | Stage::RebootVerify) && !j.uncertain {
                let expected = if j.stage == Stage::RebootEnable {
                    Some(j.enable_boot.as_str())
                } else {
                    j.verify_boot.as_deref()
                };
                let already = j.reboot_sent_boot.as_deref() == expected;
                let changed = self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| Some(s.boot.as_str()) != expected);
                if !already && !changed {
                    a.push("reboot");
                }
            }
            a.push("restore");
            if !j.nv_started && j.direction != Direction::Restore {
                a.push("cancel");
            }
            return a;
        }
        let mut a = vec!["check"];
        if matches!(self.phase, "checked" | "preview")
            && self
                .snapshot
                .as_ref()
                .is_some_and(|s| now() >= s.stamp && now() - s.stamp <= 60)
        {
            a.push("preview");
            if self.default_targets().is_some() {
                a.push("restore_default");
            }
            if self.preview.as_ref().is_some_and(Preview::fresh) {
                a.push("begin");
            }
        }
        a
    }
    fn default_targets(&self) -> Option<Vec<Option<String>>> {
        self.baseline.as_ref()?.targets(self.snapshot.as_ref()?)
    }
    fn json(&self) -> Value {
        let p = self.preview.as_ref().filter(|p| p.fresh());
        let masks = p
            .map(|p| p.plan.target_masks())
            .or_else(|| self.plan.as_ref().map(Plan::target_masks))
            .unwrap_or_default();
        let values = self
            .snapshot
            .as_ref()
            .map(|s| s.observed.clone())
            .unwrap_or_default();
        let secondary = self
            .snapshot
            .as_ref()
            .map(|s| {
                if s.records.len() == 2 {
                    "active"
                } else {
                    "inactive"
                }
            })
            .unwrap_or("unknown");
        let expires = p
            .map(|p| {
                60u64
                    .saturating_sub(p.created.elapsed().as_secs())
                    .min(60u64.saturating_sub(now().saturating_sub(p.epoch)))
            })
            .unwrap_or(0);
        json!({"protocol":1,"revision":self.revision,"stamp":now(),"busy":self.busy,"phase":self.phase,"code":self.code,
            "imei1":values.first(),"imei2":values.get(1),"secondary":secondary,"operation_id":self.journal.as_ref().map(|j|&j.id),
            "preview_id":p.map(|p|&p.id),"target1_masked":masks.first(),"target2_masked":masks.get(1),"allowed":self.allowed(),"preview_expires_in":expires})
    }
}
struct Controller {
    state: Mutex<State>,
    adapter: Arc<dyn Adapter>,
    store: Store,
}
impl Controller {
    fn new(adapter: Arc<dyn Adapter>, store: Store) -> Arc<Self> {
        let state = State::initial(&store);
        Arc::new(Self {
            state: Mutex::new(state),
            adapter,
            store,
        })
    }
    fn status(&self) -> Value {
        match self.state.lock() {
            Ok(s) => s.json(),
            Err(_) => {
                json!({"protocol":1,"revision":1,"stamp":now(),"busy":false,"phase":"error","code":"controller_unavailable","imei1":null,"imei2":null,"secondary":"unknown","operation_id":null,"preview_id":null,"target1_masked":null,"target2_masked":null,"allowed":[],"preview_expires_in":0})
            }
        }
    }
    fn reserve(&self, action: &Action) -> std::result::Result<State, (u16, Value)> {
        action.validate().map_err(|c| failure(400, c))?;
        self.adapter.guard().map_err(|c| failure(503, c))?;
        let mut s = self
            .state
            .lock()
            .map_err(|_| failure(503, "controller_unavailable"))?;
        if s.busy {
            return Err(failure(409, "busy"));
        }
        if action.revision() != s.revision {
            return Err(failure(409, "stale_revision"));
        }
        if !s.allowed().contains(&action.name()) {
            return Err(failure(409, "action_not_allowed"));
        }
        if let Some(id) = action.operation() {
            if s.journal.as_ref().map(|j| j.id.as_str()) != Some(id) {
                return Err(failure(409, "stale_operation"));
            }
        }
        if let Action::Begin { preview_id, .. } = action {
            if !s
                .preview
                .as_ref()
                .is_some_and(|p| p.id == *preview_id && p.fresh())
            {
                return Err(failure(409, "stale_preview"));
            }
        }
        if s.revision >= MAX_REVISION - 2 {
            return Err(failure(503, "revision_exhausted"));
        }
        s.busy = true;
        s.revision += 1;
        s.phase = match action {
            Action::Check { .. } => "checking",
            Action::Begin { .. } => "preparing",
            Action::Continue { .. } => "writing",
            _ => s.phase,
        };
        s.code = "working";
        Ok(s.clone())
    }
    fn submit(self: &Arc<Self>, action: Action) -> std::result::Result<Value, (u16, Value)> {
        let work = self.reserve(&action)?;
        let controller = self.clone();
        let spawn = std::thread::Builder::new()
            .name("imei-operation".into())
            .spawn(move || {
                controller.execute(action, work);
            });
        if spawn.is_err() {
            if let Ok(mut s) = self.state.lock() {
                s.busy = false;
                s.revision += 1;
                s.operation_phase();
                if s.journal.is_none() {
                    s.phase = "error";
                    s.code = "worker_unavailable";
                }
            }
            return Err(failure(503, "worker_unavailable"));
        }
        Ok(json!({"ok":true}))
    }
    fn execute(&self, action: Action, mut work: State) {
        // Retain the cross-process lease through the final journal fsync as well
        // as the native calls; a failed lease never authorizes a journal write.
        let lease = self.adapter.guard().and_then(|_| self.store.lease());
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &lease {
            Ok(_) => self
                .adapter
                .guard()
                .and_then(|_| self.run(action, &mut work)),
            Err(code) => Err(*code),
        }));
        let error = match r {
            Ok(Ok(())) => None,
            Ok(Err(c)) => Some(c),
            Err(_) => Some("operation_interrupted"),
        };
        let may_persist = lease.is_ok()
            && !matches!(
                error,
                Some(
                    "journal_changed"
                        | "journal_missing"
                        | "deployment_active"
                        | "deployment_state_unsafe"
                )
            );
        if let Some(code) = error {
            work.preview = None;
            if let Some(j) = work.journal.as_mut() {
                // Every ambiguous step stays durable and requires an explicit
                // recovery action. No worker, startup, CHECK or GET replays it.
                if matches!(
                    j.stage,
                    Stage::Enabling | Stage::Writing | Stage::RestoringConfig
                ) {
                    j.uncertain = true;
                }
                work.recovered = true;
                work.phase = "recovery_required";
                work.code = "recovery_required";
            } else {
                work.phase = if matches!(
                    code,
                    "helper_unavailable"
                        | "profile_unavailable"
                        | "profile_mismatch"
                        | "helper_manifest_invalid"
                        | "helper_integrity_failed"
                        | "config_schema_unsupported"
                        | "config_mode_unsupported"
                        | "root_required"
                ) {
                    "unsupported"
                } else {
                    "error"
                };
                work.code = code;
                work.snapshot = None;
            }
        }
        work.busy = false;
        work.revision = work.revision.saturating_add(1).min(MAX_REVISION);
        if let Some(j) = work.journal.as_mut() {
            if may_persist {
                j.revision = work.revision;
                if self.store.save(j).is_err() {
                    work.phase = "recovery_required";
                    work.code = "recovery_required";
                    work.recovered = true;
                }
            }
        }
        if let Ok(mut current) = self.state.lock() {
            *current = work;
        }
    }
    fn persist(&self, s: &mut State) -> Result<()> {
        let j = s.journal.as_mut().ok_or("journal_invalid")?;
        j.revision = s.revision;
        self.store.save(j)
    }
    fn inspect_bound(&self, s: &State) -> Result<Snapshot> {
        let current = self.adapter.inspect()?;
        current.validate()?;
        if let Some(j) = &s.journal {
            if current.identity != j.identity || !live_config_mode_matches(j.mode, current.mode) {
                return Err("identity_changed");
            }
            s.plan.as_ref().ok_or("backup_invalid")?.known(&current)?;
        }
        Ok(current)
    }
    fn mutation(&self, s: &mut State, kind: Mutation, stage: Stage) -> Result<()> {
        let j = s.journal.as_mut().ok_or("journal_invalid")?;
        j.stage = stage;
        if matches!(kind, Mutation::Apply | Mutation::RestoreNv) {
            j.nv_started = true;
        }
        self.persist(s)?;
        let j = s.journal.as_ref().ok_or("journal_invalid")?;
        self.store.reload_plan(j)?;
        let (nv, cfg) = self.store.paths(&j.id)?;
        // Rebind identity immediately before every mutating native invocation.
        let (identity, boot) = self.adapter.identity()?;
        let snap = s.snapshot.as_ref().ok_or("snapshot_invalid")?;
        if identity != j.identity || boot != snap.boot {
            return Err("identity_changed");
        }
        self.adapter.guard()?;
        self.adapter.mutate(kind, &nv, &cfg)
    }
    fn final_reboot(&self, s: &mut State, current: Snapshot) -> Result<()> {
        let j = s.journal.as_mut().ok_or("journal_invalid")?;
        j.stage = Stage::RebootVerify;
        j.verify_boot = Some(current.boot.clone());
        j.reboot_sent_boot = None;
        j.uncertain = false;
        s.snapshot = Some(current);
        s.recovered = false;
        self.persist(s)?;
        s.operation_phase();
        Ok(())
    }
    fn restore_config(&self, s: &mut State) -> Result<Snapshot> {
        let before = self.inspect_bound(s)?;
        s.snapshot = Some(before.clone());
        let plan = s.plan.as_ref().ok_or("backup_invalid")?.clone();
        if before.config != plan.config {
            self.mutation(s, Mutation::RestoreConfig, Stage::RestoringConfig)?;
        }
        let after = self.inspect_bound(s)?;
        if after.boot != before.boot
            || after.records != before.records
            || after.config != plan.config
        {
            return Err("restore_readback_failed");
        }
        Ok(after)
    }
    fn preview(&self, s: &mut State, targets: &[Option<String>]) -> Result<()> {
        let old = s.snapshot.as_ref().ok_or("check_required")?;
        if now() < old.stamp || now() - old.stamp > 60 {
            return Err("snapshot_expired");
        }
        let fresh = self.adapter.inspect()?;
        fresh.validate()?;
        if !old.same_source(&fresh) {
            return Err("snapshot_changed");
        }
        let plan = Plan::from_targets(&fresh, targets)?;
        s.preview = Some(Preview {
            id: random_id()?,
            source: fresh.clone(),
            plan,
            created: Instant::now(),
            epoch: now(),
        });
        s.snapshot = Some(fresh);
        s.phase = "preview";
        s.code = "preview_ready";
        Ok(())
    }
    fn run(&self, action: Action, s: &mut State) -> Result<()> {
        // Another process may have completed/changed a durable operation while
        // this process was idle. Never overwrite a different operation authority.
        if let Some((disk, _)) = self.store.load()? {
            if s.journal
                .as_ref()
                .is_none_or(|j| j.id != disk.id || j.revision != disk.revision)
            {
                return Err("journal_changed");
            }
        } else if s.journal.is_some() {
            return Err("journal_missing");
        }
        match action {
            Action::Check { .. } => {
                s.preview = None;
                let current = self.inspect_bound(s)?;
                if let Some(j) = &s.journal {
                    // A changed boot is observed, not caused. CHECK never mutates.
                    let plan = s.plan.as_ref().ok_or("backup_invalid")?;
                    if !j.uncertain
                        && !s.recovered
                        && j.stage == Stage::RebootEnable
                        && current.records == plan.original
                        && current.config == plan.candidate
                    {
                        s.snapshot = Some(current);
                        s.operation_phase();
                    } else {
                        s.snapshot = Some(current);
                        s.operation_phase();
                    }
                } else {
                    if config_flag(&current.config)? != 0 {
                        return Err("config_already_enabled");
                    }
                    if s.baseline.is_none() {
                        // Best effort: a failure only leaves restore_default unavailable.
                        if let Ok(b) = Baseline::of(&current) {
                            if self.store.record_baseline(&b).is_ok() {
                                s.baseline = Some(b);
                            }
                        }
                    }
                    s.snapshot = Some(current);
                    s.preview = None;
                    s.phase = "checked";
                    s.code = "ready";
                }
                Ok(())
            }
            Action::Preview { targets, .. } => self.preview(s, &targets),
            Action::RestoreDefault { .. } => {
                let targets = s.default_targets().ok_or("default_unavailable")?;
                self.preview(s, &targets)
            }
            Action::Begin { preview_id, .. } => {
                let preview = s.preview.take().ok_or("stale_preview")?;
                if preview.id != preview_id || !preview.fresh() {
                    return Err("stale_preview");
                }
                let fresh = self.adapter.inspect()?;
                fresh.validate()?;
                if !preview.source.same_source(&fresh) {
                    return Err("snapshot_changed");
                }
                let plan = preview.plan;
                plan.validate()?;
                let j = Journal {
                    schema: 1,
                    id: random_id()?,
                    revision: s.revision,
                    identity: fresh.identity.clone(),
                    count: fresh.records.len(),
                    nv_hash: hash(&plan.nv_bytes()),
                    config_hash: hash(&plan.config_bytes()),
                    mode: fresh.mode,
                    stage: Stage::Prepared,
                    direction: Direction::Change,
                    initial_boot: fresh.boot.clone(),
                    enable_boot: fresh.boot.clone(),
                    verify_boot: None,
                    reboot_sent_boot: None,
                    nv_started: false,
                    uncertain: false,
                };
                // This must succeed durably BEFORE enabling the EFS flag.
                self.store.prepare(&j, &plan)?;
                s.journal = Some(j);
                s.plan = Some(plan);
                s.snapshot = Some(fresh.clone());
                self.mutation(s, Mutation::Enable, Stage::Enabling)?;
                let after = self.inspect_bound(s)?;
                let p = s.plan.as_ref().ok_or("backup_invalid")?;
                if after.boot != fresh.boot
                    || after.records != p.original
                    || after.config != p.candidate
                {
                    return Err("enable_readback_failed");
                }
                let j = s.journal.as_mut().ok_or("journal_invalid")?;
                j.stage = Stage::RebootEnable;
                s.snapshot = Some(after);
                s.recovered = false;
                self.persist(s)?;
                s.operation_phase();
                Ok(())
            }
            Action::Continue { .. } => {
                let j = s.journal.as_ref().ok_or("journal_invalid")?.clone();
                if j.stage != Stage::RebootEnable || j.uncertain || s.recovered {
                    return Err("recovery_required");
                }
                let current = self.inspect_bound(s)?;
                s.snapshot = Some(current.clone());
                let p = s.plan.as_ref().ok_or("backup_invalid")?.clone();
                if current.boot == j.enable_boot {
                    s.operation_phase();
                    return Ok(());
                }
                if current.config != p.candidate {
                    return Err("write_flag_not_enabled");
                }
                let expected = if j.direction == Direction::Change {
                    p.target.clone()
                } else {
                    p.original.clone()
                };
                // Ordinary writes require the entire original baseline. Partial
                // apply/crash states are never silently replayed as a new plan.
                if j.direction == Direction::Change && current.records != p.original {
                    return Err("unknown_live_state");
                }
                if j.direction == Direction::Cancel {
                    return Err("action_not_allowed");
                }
                self.mutation(
                    s,
                    if j.direction == Direction::Change {
                        Mutation::Apply
                    } else {
                        Mutation::RestoreNv
                    },
                    Stage::Writing,
                )?;
                let after = self.inspect_bound(s)?;
                if after.boot != current.boot || after.records != expected {
                    return Err("nv_readback_failed");
                }
                s.snapshot = Some(after);
                let restored = self.restore_config(s)?;
                if restored.records != expected {
                    return Err("nv_readback_failed");
                }
                self.final_reboot(s, restored)
            }
            Action::Reboot { .. } => {
                let j = s.journal.as_ref().ok_or("journal_invalid")?.clone();
                if j.uncertain || !matches!(j.stage, Stage::RebootEnable | Stage::RebootVerify) {
                    return Err("action_not_allowed");
                }
                let (identity, boot) = self.adapter.identity()?;
                if identity != j.identity {
                    return Err("identity_changed");
                }
                let expected = if j.stage == Stage::RebootEnable {
                    j.enable_boot.as_str()
                } else {
                    j.verify_boot.as_deref().ok_or("journal_invalid")?
                };
                if boot != expected {
                    s.operation_phase();
                    return Ok(());
                }
                if j.reboot_sent_boot.as_deref() == Some(&boot) {
                    s.operation_phase();
                    return Ok(());
                }
                s.journal
                    .as_mut()
                    .ok_or("journal_invalid")?
                    .reboot_sent_boot = Some(boot);
                self.persist(s)?; // Durable intent before one RPC, even if ACK lost.
                self.adapter.guard()?;
                let _ack = self.adapter.reboot(); // Disconnection is not success or a reason to retry.
                s.operation_phase();
                Ok(())
            }
            Action::Verify { .. } => {
                let j = s.journal.as_ref().ok_or("journal_invalid")?.clone();
                if j.stage != Stage::RebootVerify {
                    return Err("verification_not_ready");
                }
                let current = self.inspect_bound(s)?;
                s.snapshot = Some(current.clone());
                if j.verify_boot.as_deref() == Some(&current.boot) {
                    s.operation_phase();
                    return Ok(());
                }
                let p = s.plan.as_ref().ok_or("backup_invalid")?;
                let expected = if j.direction == Direction::Change {
                    &p.target
                } else {
                    &p.original
                };
                if current.records != *expected || current.config != p.config {
                    return Err("final_verification_failed");
                }
                let j = s.journal.as_mut().ok_or("journal_invalid")?;
                j.stage = Stage::Complete;
                j.uncertain = false;
                s.recovered = false;
                self.persist(s)?;
                s.operation_phase();
                Ok(())
            }
            Action::Restore { .. } => {
                let before = self.inspect_bound(s)?;
                let p = s.plan.as_ref().ok_or("backup_invalid")?.clone();
                s.snapshot = Some(before.clone());
                let j = s.journal.as_mut().ok_or("journal_invalid")?;
                j.direction = Direction::Restore;
                j.uncertain = false;
                j.reboot_sent_boot = None;
                s.recovered = false;
                if before.records == p.original {
                    let restored = self.restore_config(s)?;
                    return self.final_reboot(s, restored);
                }
                if before.config == p.config {
                    s.journal.as_mut().ok_or("journal_invalid")?.enable_boot = before.boot.clone();
                    self.mutation(s, Mutation::Enable, Stage::Enabling)?;
                    let enabled = self.inspect_bound(s)?;
                    if enabled.boot != before.boot
                        || enabled.records != before.records
                        || enabled.config != p.candidate
                    {
                        return Err("enable_readback_failed");
                    }
                    s.snapshot = Some(enabled);
                    s.journal.as_mut().ok_or("journal_invalid")?.stage = Stage::RebootEnable;
                    self.persist(s)?;
                    s.operation_phase();
                    return Ok(());
                }
                // An already enabled known flag does not prove a boot boundary.
                // Force a new explicit reboot before restoring NV unless a prior
                // operation's enabling boot has observably changed.
                let j = s.journal.as_ref().ok_or("journal_invalid")?;
                if before.boot == j.enable_boot {
                    s.journal.as_mut().ok_or("journal_invalid")?.stage = Stage::RebootEnable;
                    self.persist(s)?;
                    s.operation_phase();
                    return Ok(());
                }
                self.mutation(s, Mutation::RestoreNv, Stage::Writing)?;
                let restored_nv = self.inspect_bound(s)?;
                if restored_nv.boot != before.boot || restored_nv.records != p.original {
                    return Err("restore_readback_failed");
                }
                s.snapshot = Some(restored_nv);
                let restored = self.restore_config(s)?;
                self.final_reboot(s, restored)
            }
            Action::Cancel { .. } => {
                let j = s.journal.as_ref().ok_or("journal_invalid")?;
                if j.nv_started || j.direction == Direction::Restore {
                    return Err("cancel_after_write_refused");
                }
                let current = self.inspect_bound(s)?;
                if current.records != s.plan.as_ref().ok_or("backup_invalid")?.original {
                    return Err("cancel_after_write_refused");
                }
                s.snapshot = Some(current);
                s.journal.as_mut().ok_or("journal_invalid")?.direction = Direction::Cancel;
                let restored = self.restore_config(s)?;
                self.final_reboot(s, restored)
            }
        }
    }
}
fn failure(status: u16, code: &'static str) -> (u16, Value) {
    (status, json!({"ok":false,"error":code}))
}
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(MAX_REVISION)
}
static CONTROLLER: OnceLock<Arc<Controller>> = OnceLock::new();
fn global() -> &'static Arc<Controller> {
    CONTROLLER.get_or_init(|| Controller::new(Arc::new(Production), Store::production()))
}
pub fn status() -> Value {
    global().status()
}
pub fn submit(action: Action) -> std::result::Result<Value, (u16, Value)> {
    global().submit(action)
}
pub fn screen_records() -> String {
    let v = status();
    let field = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("-").to_owned();
    let allowed = v["allowed"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| match s {
                    "check" => 1,
                    "preview" => 2,
                    "restore_default" => 256,
                    "begin" => 4,
                    "continue" => 8,
                    "verify" => 16,
                    "restore" => 32,
                    "reboot" => 64,
                    "cancel" => 128,
                    _ => 0,
                })
                .fold(0u16, |a, b| a | b)
        })
        .unwrap_or(0);
    format!(
        "I1\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        v["revision"].as_u64().unwrap_or(1),
        v["stamp"].as_u64().unwrap_or(0),
        u8::from(v["busy"].as_bool().unwrap_or(false)),
        field("phase"),
        field("code"),
        field("imei1"),
        field("imei2"),
        field("secondary"),
        field("operation_id"),
        field("preview_id"),
        field("target1_masked"),
        field("target2_masked"),
        allowed,
        v["preview_expires_in"].as_u64().unwrap_or(0)
    )
}
pub fn parse_screen(line: &str) -> std::result::Result<Action, &'static str> {
    if line.len() > 192
        || line.is_empty()
        || line.bytes().any(|b| b < 32 || b > 126)
        || line.contains("  ")
        || line.ends_with(' ')
        || line.starts_with(' ')
    {
        return Err("invalid_imei_command");
    }
    let p: Vec<_> = line.split(' ').collect();
    if p.len() < 3 || p[0] != "uimei" {
        return Err("invalid_imei_command");
    }
    if p[2].is_empty() || !p[2].bytes().all(|b| b.is_ascii_digit()) || p[2].len() > 16 {
        return Err("invalid_revision");
    }
    let revision = p[2].parse().map_err(|_| "invalid_revision")?;
    let action = match p[1] {
        "check" if p.len() == 3 => Action::Check { revision },
        "restore_default" if p.len() == 3 => Action::RestoreDefault { revision },
        "preview" if p.len() == 5 => Action::Preview {
            revision,
            targets: p[3..5]
                .iter()
                .map(|s| {
                    if *s == "-" {
                        None
                    } else {
                        Some((*s).to_owned())
                    }
                })
                .collect(),
        },
        "begin" if p.len() == 5 && p[4] == "1" => Action::Begin {
            revision,
            preview_id: p[3].into(),
            acknowledge_risk: true,
            confirm_text: "CHANGE IMEI".into(),
        },
        "continue" if p.len() == 5 && p[4] == "1" => Action::Continue {
            revision,
            operation_id: p[3].into(),
            acknowledge_risk: true,
            confirm_text: "CHANGE IMEI".into(),
        },
        "verify" if p.len() == 4 => Action::Verify {
            revision,
            operation_id: p[3].into(),
        },
        "restore" if p.len() == 5 && p[4] == "1" => Action::Restore {
            revision,
            operation_id: p[3].into(),
            acknowledge_risk: true,
            confirm_text: "RESTORE IMEI".into(),
        },
        "reboot" if p.len() == 5 && p[4] == "1" => Action::Reboot {
            revision,
            operation_id: p[3].into(),
            acknowledge_reboot: true,
        },
        "cancel" if p.len() == 5 && p[4] == "1" => Action::Cancel {
            revision,
            operation_id: p[3].into(),
            acknowledge_risk: true,
            confirm_text: "CANCEL IMEI".into(),
        },
        _ => return Err("invalid_imei_command"),
    };
    action.validate()?;
    Ok(action)
}
#[cfg(test)]
#[path = "imei/tests.rs"]
mod tests;
