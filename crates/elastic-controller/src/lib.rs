//! SQLite-backed lifecycle control with mock and optional diskless libvirt TEST
//! backends. No production libvirt or physical-device executor is provided.
use elastic_planner::leases::ResourceTopology;
use elastic_planner::mock::{
    ControllerError, JournalSink, LifecycleBackend, MockController, MockHypervisor, MockJournal,
};
use elastic_state::{JournalStore, StoreError};
use std::fs::{File, Metadata, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};

pub mod libvirt_lab;
pub mod private_libvirt_lab;
pub mod simulation;

#[derive(Debug)]
pub enum DurableError {
    Storage(StoreError),
    Controller(ControllerError),
    Io(std::io::Error),
    UnsafePath(String),
    AlreadyRunning,
}
impl std::fmt::Display for DurableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DurableError {}
impl From<StoreError> for DurableError {
    fn from(v: StoreError) -> Self {
        Self::Storage(v)
    }
}
impl From<ControllerError> for DurableError {
    fn from(v: ControllerError) -> Self {
        Self::Controller(v)
    }
}
impl From<std::io::Error> for DurableError {
    fn from(v: std::io::Error) -> Self {
        Self::Io(v)
    }
}

struct SqliteSink {
    store: JournalStore,
    revision: u64,
}
impl JournalSink for SqliteSink {
    fn commit(&mut self, journal: &MockJournal) -> Result<(), String> {
        self.revision = self
            .store
            .commit(self.revision, journal)
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// An advisory process lock supplements SQLite revision compare-and-swap. The
/// lock is held for the whole controller lifetime, including backend operations.
/// It coordinates cooperating ElasticPC processes, not arbitrary host writers.
/// Never remove the lock pathname while a controller might still hold it.
struct ProcessGate {
    _file: File,
    owner_pid: libc::pid_t,
}
impl ProcessGate {
    fn acquire(db: &Path) -> Result<Self, DurableError> {
        let absolute = if db.is_absolute() {
            db.to_owned()
        } else {
            std::env::current_dir()?.join(db)
        };
        let parent = absolute
            .parent()
            .ok_or_else(|| DurableError::UnsafePath("database parent required".into()))?;
        let mut component_path = std::path::PathBuf::new();
        for part in parent.components() {
            if matches!(part, Component::ParentDir | Component::CurDir) {
                return Err(DurableError::UnsafePath(
                    "normalized path without dot components required".into(),
                ));
            }
            component_path.push(part.as_os_str());
            let metadata = std::fs::symlink_metadata(&component_path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(DurableError::UnsafePath(
                    "symlink/non-directory parent rejected".into(),
                ));
            }
        }
        if ["/dev", "/proc", "/sys"]
            .iter()
            .any(|prefix| absolute.starts_with(prefix))
        {
            return Err(DurableError::UnsafePath(
                "special filesystem target rejected".into(),
            ));
        }
        let metadata = std::fs::metadata(parent)?;
        // SAFETY: geteuid has no preconditions or pointers.
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
            return Err(DurableError::UnsafePath(
                "database parent must be owned by this user and not writable by others".into(),
            ));
        }
        // Inspect an existing database before creating any companion file.
        // In particular, never create a lock alongside a symlink or device.
        checked_file(&absolute, uid, 0o022)?;
        let name = absolute
            .file_name()
            .ok_or_else(|| DurableError::UnsafePath("database filename required".into()))?;
        let mut lock_name = name.to_os_string();
        lock_name.push(".controller.lock");
        let lock_path = parent.join(lock_name);
        let existing = checked_file(&lock_path, uid, 0o077)?;
        // Existing special objects must be rejected before an effectful open.
        // create_new also prevents a concurrent creation from being overwritten.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(existing.is_none())
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&lock_path)?;
        let metadata = file.metadata()?;
        validate_file(&metadata, uid, 0o077)?;
        let named_metadata = checked_file(&lock_path, uid, 0o077)?.ok_or_else(|| {
            DurableError::UnsafePath("controller lock disappeared while opening".into())
        })?;
        if file_identity(&metadata) != file_identity(&named_metadata)
            || existing.is_some_and(|before| file_identity(&before) != file_identity(&metadata))
        {
            return Err(DurableError::UnsafePath(
                "controller lock changed while opening".into(),
            ));
        }
        // SAFETY: the file descriptor is valid and flock does not retain memory.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Err(DurableError::AlreadyRunning);
            }
            return Err(error.into());
        }
        Ok(Self {
            _file: file,
            // SAFETY: getpid has no arguments or memory preconditions.
            owner_pid: unsafe { libc::getpid() },
        })
    }
}

impl Drop for ProcessGate {
    fn drop(&mut self) {
        // flock belongs to the open file description, not this descriptor.
        // Another thread can fork while this gate is open, leaving a duplicate
        // alive until the child execs even though we set CLOEXEC. Explicitly
        // unlock when the owning controller ends instead of waiting for every
        // transient copy to close. An inherited child must not unlock its
        // parent's live controller when dropping its own copy of the guard.
        // SAFETY: getpid has no preconditions; _file remains open until after
        // this destructor and flock does not retain memory.
        unsafe {
            if libc::getpid() == self.owner_pid {
                libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

fn file_identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

fn validate_file(metadata: &Metadata, uid: u32, forbidden_mode: u32) -> Result<(), DurableError> {
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.nlink() != 1
        || metadata.uid() != uid
        || metadata.mode() & forbidden_mode != 0
    {
        return Err(DurableError::UnsafePath(
            "expected a user-owned ordinary file without hardlinks or unsafe permissions".into(),
        ));
    }
    Ok(())
}

fn checked_file(
    path: &Path,
    uid: u32,
    forbidden_mode: u32,
) -> Result<Option<Metadata>, DurableError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_file(&metadata, uid, forbidden_mode)?;
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub struct DurableMockController<B: LifecycleBackend = MockHypervisor> {
    // Drop the SQLite connection before releasing the gate.
    controller: MockController<B>,
    _gate: ProcessGate,
}
impl<B: LifecycleBackend> DurableMockController<B> {
    pub fn open(path: &Path, topology: ResourceTopology, backend: B) -> Result<Self, DurableError> {
        let gate = ProcessGate::acquire(path)?;
        let store = JournalStore::open(path)?;
        let stored = store.load()?;
        let json = serde_json::to_string(&stored.journal)
            .map_err(|error| DurableError::UnsafePath(error.to_string()))?;
        let mut controller = MockController::from_journal(topology, backend, &json)?;
        controller.attach_journal_sink(Box::new(SqliteSink {
            store,
            revision: stored.revision,
        }))?;
        // Do not recover here: the caller must provide fresh complete observation.
        Ok(Self {
            controller,
            _gate: gate,
        })
    }
    pub fn controller(&self) -> &MockController<B> {
        &self.controller
    }
    pub fn controller_mut(&mut self) -> &mut MockController<B> {
        &mut self.controller
    }
}

/// Backend-neutral name for the same durable controller implementation.
pub type DurableController<B> = DurableMockController<B>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliberate_controller_drop_releases_gate_with_duplicate_descriptor_alive() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let open = || {
            DurableMockController::open(
                &path,
                ResourceTopology::default(),
                MockHypervisor::default(),
            )
        };
        let controller = open().unwrap();
        let transient_copy = controller._gate._file.try_clone().unwrap();
        let second_copy = controller._gate._file.try_clone().unwrap();
        drop(second_copy);
        assert!(matches!(open(), Err(DurableError::AlreadyRunning)));
        drop(controller);
        let reopened = open().expect("owner drop must unlock despite an inherited descriptor");
        // Closing an old descriptor must not release the newly acquired lock.
        drop(transient_copy);
        assert!(matches!(open(), Err(DurableError::AlreadyRunning)));
        drop(reopened);
        assert!(open().is_ok());
    }

    #[test]
    fn dropping_fork_inherited_gate_does_not_unlock_parent_controller() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let gate = ProcessGate::acquire(&path).unwrap();
        // SAFETY: the child only drops ProcessGate (getpid/close, no allocation
        // or shared locks) and calls _exit, all async-signal-safe operations.
        // It never calls SQLite or resumes the Rust test harness after fork.
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            drop(gate);
            // SAFETY: terminate immediately without any other destructors.
            unsafe { libc::_exit(0) };
        }
        let mut status = 0;
        // SAFETY: child is a valid child PID; status is a writable integer.
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert_eq!(status, 0);
        assert!(matches!(
            ProcessGate::acquire(&path),
            Err(DurableError::AlreadyRunning)
        ));
        drop(gate);
        assert!(ProcessGate::acquire(&path).is_ok());
    }
}
