use elastic_controller::{DurableError, DurableMockController};
use elastic_core::{DiskStableId, DomainId, InstallationId, LeaseResource};
use elastic_planner::leases::{LeaseRequest, ResourceTopology};
use elastic_planner::mock::{
    ControllerError, ControllerState, CrashPoint, JournalSink, MockController, MockHypervisor,
    MockJournal, ObservedGuest,
};
use elastic_state::JournalStore;
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

fn identities() -> (
    ResourceTopology,
    InstallationId,
    DomainId,
    Vec<LeaseRequest>,
) {
    let disk = DiskStableId("/dev/disk/by-id/mock-durable-disk".into());
    let mut topology = ResourceTopology::default();
    topology.register_disk(disk.clone());
    (
        topology,
        InstallationId("installation".into()),
        DomainId("guest".into()),
        vec![LeaseRequest::exclusive(LeaseResource::BlockDisk(disk))],
    )
}
fn open(path: &Path, backend: MockHypervisor) -> DurableMockController {
    DurableMockController::open(path, identities().0, backend).unwrap()
}
#[test]
fn process_worker() {
    let Some(path) = std::env::var_os("ELASTICPC_WORKER_DB") else {
        return;
    };
    let mode = std::env::var("ELASTICPC_WORKER_MODE").unwrap();
    let (_, installation, domain, requests) = identities();
    if mode == "lock-held" {
        let _controller = open(Path::new(&path), MockHypervisor::default());
        println!("READY");
        std::io::stdout().flush().unwrap();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        return;
    }
    let point = match mode.as_str() {
        "intent" => CrashPoint::AfterJournal,
        "leases" => CrashPoint::AfterLeases,
        "started" => CrashPoint::AfterStart,
        "stop-intent" => CrashPoint::AfterStopIntent,
        "stopped" => CrashPoint::AfterStop,
        "released" => CrashPoint::AfterRelease,
        _ => panic!("unknown worker mode"),
    };
    let mut controller = open(Path::new(&path), MockHypervisor::default());
    if mode == "stop-intent" || mode == "stopped" || mode == "released" {
        controller
            .controller_mut()
            .start("operation-start", &installation, &domain, &requests)
            .unwrap();
        controller.controller_mut().inject_crash(point);
        assert_eq!(
            controller
                .controller_mut()
                .stop("operation-stop", &installation, &domain),
            Err(ControllerError::SimulatedCrash(point))
        );
        std::process::exit(73);
    }
    controller.controller_mut().inject_crash(point);
    let result =
        controller
            .controller_mut()
            .start("operation-start", &installation, &domain, &requests);
    assert!(matches!(result, Err(ControllerError::SimulatedCrash(_))));
    // No Rust destructors, controller drop, or SQLite graceful close.
    std::process::exit(73);
}
fn child(path: &Path, mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process_worker", "--nocapture"])
        .env("ELASTICPC_WORKER_DB", path)
        .env("ELASTICPC_WORKER_MODE", mode);
    command
}
#[test]
fn abrupt_exit_reopens_committed_intent_and_reconciles_fresh_backend() {
    for mode in ["intent", "leases", "started"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite3");
        let status = child(&path, mode).stdout(Stdio::null()).status().unwrap();
        assert_eq!(status.code(), Some(73));
        let stored = JournalStore::open(&path).unwrap().load().unwrap();
        assert!(!stored.journal.operations.is_empty());
        assert!(!stored.journal.operations[0].completed);
        let (_, installation, domain, requests) = identities();
        let mut observed = MockHypervisor::default();
        if mode == "started" {
            observed.observe_running(ObservedGuest {
                domain_id: domain.clone(),
                installation_id: installation,
                resources: requests,
            });
        }
        let mut controller = open(&path, observed);
        controller.controller_mut().recover().unwrap();
        let expected = if mode == "started" {
            ControllerState::Running
        } else {
            ControllerState::Stopped
        };
        assert_eq!(controller.controller().state(&domain), expected);
        assert_eq!(
            controller.controller().leases().leases().len(),
            usize::from(mode == "started")
        );
    }
}

#[test]
fn abrupt_stop_exit_requires_fresh_observation_at_all_three_boundaries() {
    for mode in ["stop-intent", "stopped", "released"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        assert_eq!(
            child(&path, mode)
                .stdout(Stdio::null())
                .status()
                .unwrap()
                .code(),
            Some(73)
        );
        let stored = JournalStore::open(&path).unwrap().load().unwrap();
        let stop = stored
            .journal
            .operations
            .iter()
            .find(|record| record.operation_id == "operation-stop")
            .unwrap();
        assert!(!stop.completed);
        let checkpointed_claims = usize::from(mode != "released");
        assert_eq!(stored.journal.reservations.len(), checkpointed_claims);
        let (_, installation, domain, requests) = identities();
        let mut unknown_backend = MockHypervisor::default();
        unknown_backend.set_observation_available(false);
        let mut unknown = open(&path, unknown_backend);
        assert_eq!(
            unknown.controller_mut().recover(),
            Err(ControllerError::UnknownObservation)
        );
        // AfterRelease already records a positively observed stopped guest, so
        // it correctly has no old claims. Observation incompleteness must still
        // forbid allocating the resource, regardless of that empty snapshot.
        assert_eq!(
            unknown.controller().leases().leases().len(),
            checkpointed_claims
        );
        assert_eq!(
            unknown.controller_mut().start(
                "unknown-rival",
                &installation,
                &DomainId("rival".into()),
                &requests
            ),
            Err(ControllerError::UnknownObservation)
        );
        assert_eq!(unknown.controller().backend().start_calls(), 0);
        drop(unknown);

        let mut fresh = MockHypervisor::default();
        if mode == "stop-intent" {
            fresh.observe_running(ObservedGuest {
                domain_id: domain.clone(),
                installation_id: installation.clone(),
                resources: requests.clone(),
            });
        }
        let mut restored = open(&path, fresh);
        restored.controller_mut().recover().unwrap();
        assert_eq!(
            restored.controller().leases().leases().len(),
            usize::from(mode == "stop-intent")
        );
        restored
            .controller_mut()
            .stop("operation-stop", &installation, &domain)
            .unwrap();
        assert_eq!(
            restored.controller().state(&domain),
            ControllerState::Stopped
        );
        assert!(restored.controller().leases().leases().is_empty());
        let calls = restored.controller().backend().stop_calls();
        assert_eq!(calls, usize::from(mode == "stop-intent"));
        restored
            .controller_mut()
            .stop("operation-stop", &installation, &domain)
            .unwrap();
        assert_eq!(restored.controller().backend().stop_calls(), calls);
    }
}
#[test]
fn independent_process_cannot_enter_while_controller_holds_gate_and_sigkill_releases_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite3");
    let mut worker = child(&path, "lock-held")
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(worker.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "worker exited without readiness"
        );
        if line.contains("READY") {
            break;
        }
        line.clear();
    }
    assert!(matches!(
        DurableMockController::open(&path, identities().0, MockHypervisor::default()),
        Err(DurableError::AlreadyRunning)
    ));
    worker.kill().unwrap();
    worker.wait().unwrap();
    let mut restarted = open(&path, MockHypervisor::default());
    restarted.controller_mut().recover().unwrap();
}
#[test]
fn stale_database_revision_poison_prevents_backend_operation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite3");
    let mut controller = open(&path, MockHypervisor::default());
    let mut external = JournalStore::open(&path).unwrap();
    let old = external.load().unwrap();
    external.commit(old.revision, &old.journal).unwrap();
    let (_, installation, domain, requests) = identities();
    assert!(matches!(
        controller
            .controller_mut()
            .start("start", &installation, &domain, &requests),
        Err(ControllerError::Persistence(_))
    ));
    assert_eq!(controller.controller().backend().start_calls(), 0);
    assert!(matches!(
        controller
            .controller_mut()
            .stop("stop", &installation, &domain),
        Err(ControllerError::Persistence(_))
    ));
    assert_eq!(controller.controller().backend().stop_calls(), 0);
}
struct FailingSink {
    count: usize,
    fail_at: usize,
}
impl JournalSink for FailingSink {
    fn commit(&mut self, _: &MockJournal) -> Result<(), String> {
        self.count += 1;
        if self.count >= self.fail_at {
            Err("injected fsync failure".into())
        } else {
            Ok(())
        }
    }
}
#[test]
fn persistence_failure_after_backend_start_retains_claims_and_denies_further_actions() {
    let (topology, installation, domain, requests) = identities();
    let mut controller = MockController::new(topology, MockHypervisor::default());
    // intent, recovery, leases, then backend-return checkpoint.
    controller
        .attach_journal_sink(Box::new(FailingSink {
            count: 0,
            fail_at: 4,
        }))
        .unwrap();
    assert!(matches!(
        controller.start("start", &installation, &domain, &requests),
        Err(ControllerError::Persistence(_))
    ));
    assert_eq!(controller.state(&domain), ControllerState::Running);
    assert_eq!(controller.leases().leases().len(), 1);
    assert!(matches!(
        controller.stop("stop", &installation, &domain),
        Err(ControllerError::Persistence(_))
    ));
    assert_eq!(controller.backend().stop_calls(), 0);
}

struct RecordingFailingSink {
    count: usize,
    fail_at: usize,
    committed: Rc<RefCell<MockJournal>>,
}
impl JournalSink for RecordingFailingSink {
    fn commit(&mut self, journal: &MockJournal) -> Result<(), String> {
        self.count += 1;
        if self.count >= self.fail_at {
            return Err("injected stop checkpoint failure".into());
        }
        *self.committed.borrow_mut() = journal.clone();
        Ok(())
    }
}

#[test]
fn stop_commit_failures_preserve_committed_claims_and_poison_further_execution() {
    // Five successful start checkpoints, then stop recovery, stop intent,
    // backend-return, release, and finish. Exercise each vulnerable stop phase.
    for fail_at in [6, 7, 8, 9] {
        let (topology, installation, domain, requests) = identities();
        let committed = Rc::new(RefCell::new(MockJournal::default()));
        let mut controller = MockController::new(topology.clone(), MockHypervisor::default());
        controller
            .attach_journal_sink(Box::new(RecordingFailingSink {
                count: 0,
                fail_at,
                committed: committed.clone(),
            }))
            .unwrap();
        controller
            .start("start", &installation, &domain, &requests)
            .unwrap();
        assert!(matches!(
            controller.stop("stop", &installation, &domain),
            Err(ControllerError::Persistence(_))
        ));
        let expected_stop_calls = usize::from(fail_at >= 8);
        assert_eq!(controller.backend().stop_calls(), expected_stop_calls);
        assert_eq!(
            committed.borrow().reservations.len(),
            1,
            "last successful checkpoint must retain ownership until the release commit succeeds"
        );
        assert!(matches!(
            controller.start(
                "rival-start",
                &installation,
                &DomainId("rival".into()),
                &requests
            ),
            Err(ControllerError::Persistence(_))
        ));
        assert!(matches!(
            controller.stop("retry-stop", &installation, &domain),
            Err(ControllerError::Persistence(_))
        ));
        assert_eq!(controller.backend().start_calls(), 1);
        assert_eq!(controller.backend().stop_calls(), expected_stop_calls);

        let mut unknown_backend = controller.backend().clone();
        unknown_backend.set_observation_available(false);
        let json = serde_json::to_string(&*committed.borrow()).unwrap();
        let mut recovered = MockController::from_journal(topology, unknown_backend, &json).unwrap();
        assert_eq!(
            recovered.recover(),
            Err(ControllerError::UnknownObservation)
        );
        assert_eq!(recovered.leases().leases().len(), 1);
        recovered.backend_mut().set_observation_available(true);
        recovered.recover().unwrap();
        assert_eq!(recovered.leases().leases().len(), usize::from(fail_at < 8));
    }
}
#[test]
fn private_temp_lab_runs_and_cleans_its_real_database() {
    let result = elastic_controller::simulation::run_recovery_lab().unwrap();
    assert_eq!(result.status, "passed");
    assert!(result.committed_revision > 0 && result.cleanup);
    assert!(!result.physical_device_execution);
}
#[test]
fn special_or_shared_directory_targets_are_refused_before_database_creation() {
    assert!(DurableMockController::open(
        Path::new("/dev/elasticpc-test.sqlite3"),
        identities().0,
        MockHypervisor::default()
    )
    .is_err());
    let dir = tempfile::tempdir().unwrap();
    let linked = dir.path().join("linked");
    std::os::unix::fs::symlink("/dev", &linked).unwrap();
    assert!(DurableMockController::open(
        &linked.join("elasticpc-test.sqlite3"),
        identities().0,
        MockHypervisor::default()
    )
    .is_err());
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".controller.lock");
    name.into()
}

fn fifo(path: &Path) {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: the path is a valid NUL-terminated string; mkfifo creates only the
    // named ordinary test FIFO in our private temporary directory.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
}

fn watch_opens(path: &Path) -> std::fs::File {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: inotify_init1 takes only flags and returns a fresh descriptor.
    let descriptor = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(descriptor >= 0);
    // SAFETY: the successful call above transfers a valid owned descriptor.
    let file = unsafe { std::fs::File::from_raw_fd(descriptor) };
    // SAFETY: descriptor and NUL-terminated path remain valid during the call.
    assert!(unsafe { libc::inotify_add_watch(descriptor, path.as_ptr(), libc::IN_OPEN) } >= 0);
    file
}

#[test]
fn unsafe_database_objects_are_rejected_before_companion_lock_creation() {
    let directory = tempfile::tempdir().unwrap();
    let valuable = directory.path().join("valuable");
    std::fs::write(&valuable, "preserve original contents").unwrap();
    for kind in ["symlink", "hardlink", "directory", "fifo"] {
        let path = directory.path().join(format!("{kind}.sqlite3"));
        match kind {
            "symlink" => std::os::unix::fs::symlink(&valuable, &path).unwrap(),
            "hardlink" => std::fs::hard_link(&valuable, &path).unwrap(),
            "directory" => std::fs::create_dir(&path).unwrap(),
            "fifo" => fifo(&path),
            _ => unreachable!(),
        }
        assert!(matches!(
            DurableMockController::open(&path, identities().0, MockHypervisor::default()),
            Err(DurableError::UnsafePath(_))
        ));
        assert!(!lock_path(&path).exists());
    }
    assert_eq!(
        std::fs::read_to_string(&valuable).unwrap(),
        "preserve original contents"
    );
}

#[test]
fn unsafe_existing_lock_objects_are_rejected_without_opening_or_database_creation() {
    let directory = tempfile::tempdir().unwrap();
    let valuable = directory.path().join("valuable");
    std::fs::write(&valuable, "preserve original contents").unwrap();
    std::fs::set_permissions(&valuable, std::fs::Permissions::from_mode(0o600)).unwrap();
    for kind in ["symlink", "hardlink", "directory", "fifo", "public-file"] {
        let path = directory.path().join(format!("{kind}.sqlite3"));
        let lock = lock_path(&path);
        match kind {
            "symlink" => std::os::unix::fs::symlink(&valuable, &lock).unwrap(),
            "hardlink" => std::fs::hard_link(&valuable, &lock).unwrap(),
            "directory" => std::fs::create_dir(&lock).unwrap(),
            "fifo" => fifo(&lock),
            "public-file" => {
                std::fs::write(&lock, "existing user file").unwrap();
                std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            _ => unreachable!(),
        }
        let mut open_events = (kind == "fifo").then(|| watch_opens(&lock));
        assert!(matches!(
            DurableMockController::open(&path, identities().0, MockHypervisor::default()),
            Err(DurableError::UnsafePath(_))
        ));
        assert!(!path.exists());
        if let Some(events) = open_events.as_mut() {
            let mut buffer = [0u8; 64];
            // Nonblocking event read, not a timed/polled status query. Merely
            // returning an error after opening the FIFO would fail this check.
            assert_eq!(
                events.read(&mut buffer).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
        if kind == "public-file" {
            assert_eq!(std::fs::read_to_string(lock).unwrap(), "existing user file");
        }
    }
    assert_eq!(
        std::fs::read_to_string(&valuable).unwrap(),
        "preserve original contents"
    );
}

#[test]
fn existing_private_ordinary_lock_contents_are_never_truncated() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite3");
    let lock = lock_path(&path);
    std::fs::write(&lock, "existing lock marker").unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
    drop(open(&path, MockHypervisor::default()));
    assert_eq!(
        std::fs::read_to_string(lock).unwrap(),
        "existing lock marker"
    );
}
