use crate::DurableMockController;
use elastic_core::{DiskStableId, DomainId, InstallationId, LeaseResource, SCHEMA_VERSION};
use elastic_planner::leases::{LeaseRequest, ResourceTopology};
use elastic_planner::mock::{ControllerError, ControllerState, CrashPoint, MockHypervisor};
use elastic_state::JournalStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct RecoveryLabReport {
    pub schema_version: u32,
    pub status: String,
    pub evidence_kind: String,
    pub checks: Vec<String>,
    pub committed_revision: u64,
    pub physical_device_execution: bool,
    pub cleanup: bool,
    pub limitations: Vec<String>,
}

/// An end-to-end SQLite example with only inert mock device identities. Real
/// SIGKILL/abrupt-process-exit recovery is covered by subprocess integration tests.
pub fn run_recovery_lab() -> Result<RecoveryLabReport, String> {
    let directory = tempfile::Builder::new()
        .prefix("elasticpc-recovery-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let path = directory.path().join("journal.sqlite3");
    let mut topology = ResourceTopology::default();
    let service_disk = DiskStableId("/dev/disk/by-id/mock-services".into());
    let desktop_disk = DiskStableId("/dev/disk/by-id/mock-desktop".into());
    topology.register_disk(service_disk.clone());
    topology.register_disk(desktop_disk.clone());
    let service = DomainId("service-domain".into());
    let desktop = DomainId("desktop-domain".into());
    let installation = InstallationId("synthetic-installation".into());
    let service_resources = [LeaseRequest::exclusive(LeaseResource::BlockDisk(
        service_disk,
    ))];
    let desktop_resources = [LeaseRequest::exclusive(LeaseResource::BlockDisk(
        desktop_disk,
    ))];
    let mut controller =
        DurableMockController::open(&path, topology.clone(), MockHypervisor::default())
            .map_err(|e| e.to_string())?;
    controller
        .controller_mut()
        .start("service-start", &installation, &service, &service_resources)
        .map_err(|e| e.to_string())?;
    controller
        .controller_mut()
        .inject_crash(CrashPoint::AfterStart);
    let result = controller.controller_mut().start(
        "desktop-start",
        &installation,
        &desktop,
        &desktop_resources,
    );
    if result != Err(ControllerError::SimulatedCrash(CrashPoint::AfterStart)) {
        return Err(format!("unexpected crash outcome: {result:?}"));
    }
    let backend = controller.controller().backend().clone();
    drop(controller);
    let mut restored =
        DurableMockController::open(&path, topology, backend).map_err(|e| e.to_string())?;
    let recovery = restored
        .controller_mut()
        .recover()
        .map_err(|e| e.to_string())?;
    if recovery.running_domains.len() != 2 || !recovery.conflicts.is_empty() {
        return Err("fresh backend recovery lost or conflicted ownership".into());
    }
    let duplicate = restored.controller_mut().start(
        "duplicate-start",
        &installation,
        &DomainId("other-desktop".into()),
        &desktop_resources,
    );
    if !matches!(duplicate, Err(ControllerError::Lease(_))) {
        return Err("conflicting desktop resource was not denied".into());
    }
    restored
        .controller_mut()
        .stop("desktop-stop", &installation, &desktop)
        .map_err(|e| e.to_string())?;
    if restored.controller().state(&service) != ControllerState::Running {
        return Err("desktop shutdown affected the service domain".into());
    }
    let before = restored.controller().backend().stop_calls();
    restored
        .controller_mut()
        .stop("desktop-stop", &installation, &desktop)
        .map_err(|e| e.to_string())?;
    if restored.controller().backend().stop_calls() != before {
        return Err("operation replay repeated stop".into());
    }
    restored
        .controller_mut()
        .stop("service-stop", &installation, &service)
        .map_err(|e| e.to_string())?;
    drop(restored);
    let stored = JournalStore::open(&path)
        .map_err(|e| e.to_string())?
        .load()
        .map_err(|e| e.to_string())?;
    if !stored.journal.reservations.is_empty() {
        return Err("stopped domains retained committed leases".into());
    }
    directory.close().map_err(|e| e.to_string())?;
    Ok(RecoveryLabReport {
        schema_version: SCHEMA_VERSION, status: "passed".into(),
        evidence_kind: "real_sqlite_with_mock_backend".into(),
        checks: vec!["operation intent and leases committed before backend steps".into(),
            "controller reconstructed from SQLite and fresh independent mock inventory".into(),
            "overlapping disk owner rejected".into(), "desktop shutdown preserved service-domain lifecycle".into(),
            "operation replay idempotent".into(), "stopped I/O released committed reservations".into()],
        committed_revision: stored.revision, physical_device_execution: false, cleanup: true,
        limitations: vec!["Backend devices and guest I/O are simulated; this does not certify libvirt/native boot".into(),
            "Subprocess crash tests are separate from this interactive demonstration; power-loss durability depends on filesystem and hardware sync semantics".into()],
    })
}
