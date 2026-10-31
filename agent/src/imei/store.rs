//! Root-only bounded private storage. Startup/status only read; creation is an
//! explicit action. Unknown paths are never chmod'ed, imported, or removed.
use super::domain::*;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(super) struct Store {
    pub root: PathBuf,
    owner: u32,
    production: bool,
}
pub(super) struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
impl Store {
    pub fn production() -> Self {
        Self {
            root: PathBuf::from("/data/openui-imei"),
            owner: 0,
            production: true,
        }
    }
    #[cfg(test)]
    pub fn testing(root: PathBuf) -> Self {
        Self {
            root,
            owner: unsafe { libc::geteuid() },
            production: false,
        }
    }
    fn root_user(&self) -> Result<()> {
        if self.production && (unsafe { libc::geteuid() } != 0 || unsafe { libc::getuid() } != 0) {
            return Err("root_required");
        }
        Ok(())
    }
    fn directory(&self, p: &Path, private: bool) -> Result<()> {
        let m = fs::symlink_metadata(p).map_err(|_| "state_unavailable")?;
        if !m.is_dir()
            || m.uid() != self.owner
            || m.permissions().mode() & 0o022 != 0
            || (private && m.permissions().mode() & 0o777 != 0o700)
        {
            return Err("unsafe_state_path");
        }
        Ok(())
    }
    fn ancestors(&self) -> Result<()> {
        self.root_user()?;
        if !self.root.is_absolute() {
            return Err("unsafe_state_path");
        }
        if self.production {
            for p in self.root.parent().ok_or("unsafe_state_path")?.ancestors() {
                self.directory(p, false)?;
            }
        }
        Ok(())
    }
    fn sync_dir(&self, p: &Path) -> Result<()> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(p)
            .and_then(|f| f.sync_all())
            .map_err(|_| "persistence_failed")
    }
    fn mkdir(&self, p: &Path) -> Result<()> {
        match fs::DirBuilder::new().mode(0o700).create(p) {
            Ok(()) => {
                self.sync_dir(p.parent().ok_or("unsafe_state_path")?)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("persistence_failed"),
        }
        self.directory(p, true)
    }
    pub fn ensure(&self) -> Result<()> {
        self.ancestors()?;
        self.mkdir(&self.root)?;
        self.mkdir(&self.root.join("operations"))?;
        Ok(())
    }
    pub fn lease(&self) -> Result<Lease> {
        self.ensure()?;
        let p = self.root.join("controller.lock");
        let f = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&p)
        {
            Ok(f) => {
                f.sync_all().map_err(|_| "persistence_failed")?;
                self.sync_dir(&self.root)?;
                f
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.open_file(&p, 0)?,
            Err(_) => return Err("state_unavailable"),
        };
        self.check_file(&f, 0)?;
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("busy");
        }
        Ok(Lease(f))
    }
    fn check_file(&self, f: &File, max: usize) -> Result<()> {
        let m = f.metadata().map_err(|_| "state_unavailable")?;
        if !m.is_file()
            || m.uid() != self.owner
            || m.nlink() != 1
            || m.permissions().mode() & 0o777 != 0o600
            || m.len() > max as u64
        {
            return Err("unsafe_state_file");
        }
        Ok(())
    }
    fn open_file(&self, p: &Path, max: usize) -> Result<File> {
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(p)
            .map_err(|_| "state_unavailable")?;
        self.check_file(&f, max)?;
        Ok(f)
    }
    fn read(&self, p: &Path, max: usize) -> Result<Vec<u8>> {
        let mut f = self.open_file(p, max)?;
        let before = f.metadata().map_err(|_| "state_unavailable")?;
        let mut bytes = Vec::new();
        (&mut f)
            .take(max as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "state_unavailable")?;
        self.check_file(&f, max)?;
        let after = f.metadata().map_err(|_| "state_unavailable")?;
        if bytes.len() > max
            || before.len() != bytes.len() as u64
            || before.ino() != after.ino()
            || before.dev() != after.dev()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
        {
            return Err("state_changed");
        }
        Ok(bytes)
    }
    fn exclusive(&self, p: &Path, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_FILE {
            return Err("persistence_failed");
        }
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(p)
            .map_err(|_| "state_path_collision")?;
        self.check_file(&f, MAX_FILE)?;
        f.write_all(bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| "persistence_failed")?;
        self.sync_dir(p.parent().ok_or("unsafe_state_path")?)
    }
    pub fn paths(&self, id: &str) -> Result<(PathBuf, PathBuf)> {
        if !hex(id, 32) {
            return Err("journal_invalid");
        }
        let dir = self.root.join("operations").join(id);
        self.directory(&dir, true)?;
        Ok((dir.join("nv.plan"), dir.join("config.plan")))
    }
    pub fn prepare(&self, j: &Journal, p: &Plan) -> Result<()> {
        self.ensure()?;
        j.validate()?;
        p.validate()?;
        if p.original.len() != j.count
            || hash(&p.nv_bytes()) != j.nv_hash
            || hash(&p.config_bytes()) != j.config_hash
        {
            return Err("backup_invalid");
        }
        let history = self.root.join("operations");
        // Retain every backup; do not silently delete history to make room.
        if fs::read_dir(&history)
            .map_err(|_| "state_unavailable")?
            .take(33)
            .count()
            >= 32
        {
            return Err("backup_history_full");
        }
        let dir = history.join(&j.id);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|_| "state_path_collision")?;
        self.sync_dir(&history)?;
        self.directory(&dir, true)?;
        self.exclusive(&dir.join("nv.plan"), &p.nv_bytes())?;
        self.exclusive(&dir.join("config.plan"), &p.config_bytes())?;
        self.exclusive(
            &dir.join("manifest.json"),
            &serde_json::to_vec(j).map_err(|_| "persistence_failed")?,
        )?;
        self.verify_plan(j)?;
        self.save(j)
    }
    pub fn save(&self, j: &Journal) -> Result<()> {
        j.validate()?;
        self.directory(&self.root, true)?;
        let dest = self.root.join("current.json");
        match fs::symlink_metadata(&dest) {
            Ok(_) => {
                self.open_file(&dest, MAX_FILE)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("state_unavailable"),
        }
        let bytes = serde_json::to_vec(j).map_err(|_| "persistence_failed")?;
        let temp = self.root.join(format!(".journal-{}", random_id()?));
        self.exclusive(&temp, &bytes)?;
        // Both paths are literal children of a verified non-writable root-only
        // directory. The old durable authority survives until this rename.
        fs::rename(&temp, &dest).map_err(|_| "persistence_failed")?;
        self.sync_dir(&self.root)
    }
    fn verify_plan(&self, j: &Journal) -> Result<Plan> {
        let (nv_path, cfg_path) = self.paths(&j.id)?;
        let nv = self.read(&nv_path, 512)?;
        let cfg = self.read(&cfg_path, CONFIG_SIZE * 2)?;
        if nv.len() != j.count * 256
            || cfg.len() != CONFIG_SIZE * 2
            || hash(&nv) != j.nv_hash
            || hash(&cfg) != j.config_hash
        {
            return Err("backup_integrity_failed");
        }
        let split = j.count * 128;
        let p = Plan {
            original: nv[..split].chunks_exact(128).map(|s| s.to_vec()).collect(),
            target: nv[split..].chunks_exact(128).map(|s| s.to_vec()).collect(),
            config: cfg[..CONFIG_SIZE].to_vec(),
            candidate: cfg[CONFIG_SIZE..].to_vec(),
        };
        p.validate()?;
        let manifest = self.read(
            &nv_path
                .parent()
                .ok_or("backup_invalid")?
                .join("manifest.json"),
            MAX_FILE,
        )?;
        let m: Journal = serde_json::from_slice(&manifest).map_err(|_| "backup_invalid")?;
        m.validate()?;
        if m.id != j.id
            || m.identity != j.identity
            || m.count != j.count
            || m.nv_hash != j.nv_hash
            || m.config_hash != j.config_hash
            || m.initial_boot != j.initial_boot
            || m.mode != j.mode
        {
            return Err("backup_integrity_failed");
        }
        Ok(p)
    }
    pub fn load(&self) -> Result<Option<(Journal, Plan)>> {
        // No mkdir/permission repair/child execution on status or startup.
        match fs::symlink_metadata(&self.root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("state_unavailable"),
            Ok(_) => {}
        }
        self.ancestors()?;
        self.directory(&self.root, true)?;
        let path = self.root.join("current.json");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("state_unavailable"),
            Ok(_) => {}
        }
        // The software deployment lock may precede the first explicit action.
        // No current journal means idle, without requiring or creating history.
        // Once a journal exists, missing/unsafe history still fails closed.
        self.directory(&self.root.join("operations"), true)?;
        let bytes = self.read(&path, MAX_FILE)?;
        let j: Journal = serde_json::from_slice(&bytes).map_err(|_| "journal_invalid")?;
        j.validate()?;
        let p = self.verify_plan(&j)?;
        Ok(Some((j, p)))
    }
    pub fn reload_plan(&self, j: &Journal) -> Result<Plan> {
        self.verify_plan(j)
    }
    /// Read-only, like load(): a missing root or file is simply no default.
    pub fn baseline(&self) -> Result<Option<Baseline>> {
        match fs::symlink_metadata(&self.root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("state_unavailable"),
            Ok(_) => {}
        }
        self.ancestors()?;
        self.directory(&self.root, true)?;
        let path = self.root.join("default.json");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("state_unavailable"),
            Ok(_) => {}
        }
        let b: Baseline =
            serde_json::from_slice(&self.read(&path, MAX_FILE)?).map_err(|_| "baseline_invalid")?;
        b.validate()?;
        Ok(Some(b))
    }
    /// Written once, under the lease, and only while no operation has ever
    /// existed: after a change the live records are no longer the default.
    pub fn record_baseline(&self, b: &Baseline) -> Result<()> {
        b.validate()?;
        self.ensure()?;
        let dest = self.root.join("default.json");
        match fs::symlink_metadata(&dest) {
            Ok(_) => return Err("baseline_exists"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("state_unavailable"),
        }
        if fs::read_dir(self.root.join("operations"))
            .map_err(|_| "state_unavailable")?
            .next()
            .is_some()
            || fs::symlink_metadata(self.root.join("current.json")).is_ok()
        {
            return Err("baseline_unknown");
        }
        let bytes = serde_json::to_vec(b).map_err(|_| "persistence_failed")?;
        let temp = self.root.join(format!(".default-{}", random_id()?));
        self.exclusive(&temp, &bytes)?;
        fs::rename(&temp, &dest).map_err(|_| "persistence_failed")?;
        self.sync_dir(&self.root)
    }
}
pub(super) fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|_| "entropy_unavailable")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
