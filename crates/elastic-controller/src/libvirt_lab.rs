//! SQLite controller integration with libvirt's in-process TEST driver only.
//! No host device reservation or production connection can be authorized here.
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ControllerLabReport {
    pub schema_version: u32,
    pub status: String,
    pub evidence_kind: String,
    pub backend_simulated: bool,
    pub physical_resource_leases_supported: bool,
    pub controller_reconstructed: bool,
    pub process_crash_verified: bool,
    pub cleanup_complete: bool,
    pub checks: Vec<String>,
    pub error: Option<String>,
    pub limitations: Vec<String>,
}
impl Default for ControllerLabReport {
    fn default() -> Self {
        Self {
            schema_version: 1, status: "skipped".into(), evidence_kind: "not_run".into(),
            backend_simulated: true, physical_resource_leases_supported: false,
            controller_reconstructed: false, process_crash_verified: false,
            cleanup_complete: false, checks: vec![], error: None,
            limitations: vec![
                "The libvirt TEST driver simulates diskless domains; no guest boot or physical I/O is tested".into(),
                "Only the controller and SQLite connection are reconstructed; the same in-process libvirt session stays alive".into(),
                "TEST state is shared within a process and lost with that process; a new managed session cannot impersonate ownership of previous domains".into(),
                "Production host observations and resource lease release remain unverified".into(),
            ],
        }
    }
}
pub fn run_controller_lab() -> ControllerLabReport {
    #[cfg(feature = "native-libvirt")]
    {
        native::run()
    }
    #[cfg(not(feature = "native-libvirt"))]
    {
        ControllerLabReport {
            error: Some("Enable native-libvirt to run this experiment".into()),
            ..Default::default()
        }
    }
}

#[cfg(feature = "native-libvirt")]
pub mod native {
    use super::ControllerLabReport;
    use crate::DurableMockController;
    use elastic_core::{DomainId, InstallationId};
    use elastic_planner::leases::{HostObservation, ResourceTopology};
    use elastic_planner::mock::{
        BackendObservation, ControllerError, ControllerState, CrashPoint, LifecycleBackend,
        MockJournal, ObservedGuest,
    };
    use elastic_virt_libvirt::{DomainState, TestSession};
    use std::{cell::Cell, collections::BTreeMap, rc::Rc};

    #[derive(Clone)]
    pub struct TestDriverBackend {
        session: Rc<TestSession>,
        available: Rc<Cell<bool>>,
        start_calls: Rc<Cell<usize>>,
        stop_calls: Rc<Cell<usize>>,
    }
    impl TestDriverBackend {
        pub fn new(session: Rc<TestSession>) -> Self {
            Self {
                session,
                available: Rc::new(Cell::new(true)),
                start_calls: Rc::new(Cell::new(0)),
                stop_calls: Rc::new(Cell::new(0)),
            }
        }
        pub fn installation(domain: &DomainId) -> InstallationId {
            InstallationId(format!("test-install-{}", domain.0))
        }
        /// Fault injection for this laboratory, never a source of positive evidence.
        pub fn set_observation_available(&self, available: bool) {
            self.available.set(available);
        }
        pub fn start_calls(&self) -> usize {
            self.start_calls.get()
        }
        pub fn stop_calls(&self) -> usize {
            self.stop_calls.get()
        }
        fn owned(&self, domain: &DomainId) -> bool {
            self.session.domain_ids().contains(&domain.0)
        }
        fn error(e: impl std::fmt::Display) -> ControllerError {
            ControllerError::Backend(e.to_string())
        }
    }
    impl LifecycleBackend for TestDriverBackend {
        fn validate_start(&self, guest: &ObservedGuest) -> Result<(), ControllerError> {
            if !self.owned(&guest.domain_id)
                || guest.installation_id != Self::installation(&guest.domain_id)
                || !guest.resources.is_empty()
            {
                return Err(Self::error("TEST backend accepts only its own diskless domain identities and zero resource claims"));
            }
            Ok(())
        }
        fn validate_stop(
            &self,
            installation: &InstallationId,
            domain: &DomainId,
        ) -> Result<(), ControllerError> {
            self.validate_start(&ObservedGuest {
                domain_id: domain.clone(),
                installation_id: installation.clone(),
                resources: vec![],
            })
        }
        fn validate_journal(&self, journal: &MockJournal) -> Result<(), ControllerError> {
            if !journal.reservations.is_empty() {
                return Err(Self::error(
                    "TEST backend refuses persisted resource leases",
                ));
            }
            for op in &journal.operations {
                self.validate_start(&ObservedGuest {
                    domain_id: op.domain_id.clone(),
                    installation_id: op.installation_id.clone(),
                    resources: op.requested_resources.clone(),
                })?;
                if !op.acquired_resources.is_empty() {
                    return Err(Self::error("TEST backend refuses acquired resource claims"));
                }
            }
            Ok(())
        }
        fn observe(&self) -> BackendObservation {
            let mut result = BackendObservation {
                available: false,
                host: HostObservation::unknown(),
                guests: BTreeMap::new(),
            };
            if !self.available.get() {
                return result;
            }
            for uuid in self.session.domain_ids() {
                let observation = match self.session.observe_owned(&uuid) {
                    Ok(o) => o,
                    Err(_) => return result,
                };
                if observation.active {
                    let domain = DomainId(uuid);
                    result.guests.insert(
                        domain.clone(),
                        ObservedGuest {
                            installation_id: Self::installation(&domain),
                            domain_id: domain,
                            resources: vec![],
                        },
                    );
                } else if observation.state != DomainState::Shutoff {
                    return result;
                }
            }
            // Completeness is ONLY for this private, diskless TEST universe.
            // validate_start/validate_journal prohibit all physical claims.
            result.available = true;
            result.host = HostObservation::known(vec![]);
            result
        }
        fn domain_state(&self, domain: &DomainId) -> ControllerState {
            if !self.available.get() || !self.owned(domain) {
                return ControllerState::Unknown;
            }
            match self.session.observe_owned(&domain.0) {
                Ok(o) if !o.active && o.state == DomainState::Shutoff => ControllerState::Stopped,
                Ok(o) if o.active && o.state == DomainState::Running => ControllerState::Running,
                _ => ControllerState::Unknown,
            }
        }
        fn start(&mut self, guest: ObservedGuest) -> Result<(), ControllerError> {
            self.validate_start(&guest)?;
            self.start_calls.set(self.start_calls.get() + 1);
            self.session
                .start_owned(&guest.domain_id.0)
                .map_err(Self::error)
        }
        fn stop(&mut self, domain: &DomainId) -> Result<(), ControllerError> {
            if !self.owned(domain) {
                return Err(Self::error("refusing to stop an unowned TEST domain"));
            }
            self.stop_calls.set(self.stop_calls.get() + 1);
            self.session.stop_owned(&domain.0).map_err(Self::error)
        }
    }
    pub(super) fn run() -> ControllerLabReport {
        let mut report = ControllerLabReport {
            status: "failed".into(),
            ..Default::default()
        };
        let mut session = match TestSession::diskless(2) {
            Ok(s) => Rc::new(s),
            Err(e) => {
                report.error = Some(e.to_string());
                return report;
            }
        };
        report.evidence_kind = "real_sqlite_and_libvirt_test_driver".into();
        let directory = match tempfile::tempdir() {
            Ok(d) => d,
            Err(e) => {
                report.error = Some(e.to_string());
                return report;
            }
        };
        let outcome = (|| -> Result<(), String> {
            let ids = session.domain_ids();
            let service = DomainId(ids[0].clone());
            let desktop = DomainId(ids[1].clone());
            let backend = TestDriverBackend::new(session.clone());
            let db = directory.path().join("controller.sqlite3");
            let open = || {
                DurableMockController::open(&db, ResourceTopology::default(), backend.clone())
                    .map_err(|e| e.to_string())
            };
            let mut controller = open()?;
            controller
                .controller_mut()
                .start(
                    "service-start",
                    &TestDriverBackend::installation(&service),
                    &service,
                    &[],
                )
                .map_err(|e| e.to_string())?;
            controller
                .controller_mut()
                .inject_crash(CrashPoint::AfterStart);
            if !matches!(
                controller.controller_mut().start(
                    "desktop-start",
                    &TestDriverBackend::installation(&desktop),
                    &desktop,
                    &[]
                ),
                Err(ControllerError::SimulatedCrash(CrashPoint::AfterStart))
            ) {
                return Err("Expected interrupted desktop start".into());
            }
            drop(controller);
            report.controller_reconstructed = true;
            let mut controller = open()?;
            controller
                .controller_mut()
                .recover()
                .map_err(|e| e.to_string())?;
            if controller.controller().state(&desktop) != ControllerState::Running {
                return Err("Fresh libvirt observation did not find desktop running".into());
            }
            let calls = backend.start_calls();
            controller
                .controller_mut()
                .start(
                    "desktop-start",
                    &TestDriverBackend::installation(&desktop),
                    &desktop,
                    &[],
                )
                .map_err(|e| e.to_string())?;
            if backend.start_calls() != calls {
                return Err("Replay repeated backend start".into());
            }
            report.checks.push("SQLite reconstruction reconciles the retained libvirt session without repeating start".into());
            backend.set_observation_available(false);
            if !matches!(
                controller.controller_mut().recover(),
                Err(ControllerError::UnknownObservation)
            ) {
                return Err("Unavailable observation was incorrectly accepted".into());
            }
            backend.set_observation_available(true);
            controller
                .controller_mut()
                .stop(
                    "desktop-stop",
                    &TestDriverBackend::installation(&desktop),
                    &desktop,
                )
                .map_err(|e| e.to_string())?;
            if controller.controller().state(&service) != ControllerState::Running {
                return Err("Stopping desktop changed service domain".into());
            }
            let calls = backend.stop_calls();
            controller
                .controller_mut()
                .stop(
                    "desktop-stop",
                    &TestDriverBackend::installation(&desktop),
                    &desktop,
                )
                .map_err(|e| e.to_string())?;
            if backend.stop_calls() != calls {
                return Err("Replay repeated backend stop".into());
            }
            controller
                .controller_mut()
                .stop(
                    "service-stop",
                    &TestDriverBackend::installation(&service),
                    &service,
                )
                .map_err(|e| e.to_string())?;
            report.checks.push("Unknown observations denied; stopping desktop preserves service and stop replay is idempotent".into());
            Ok(())
        })();
        if let Err(e) = outcome {
            report.error = Some(e);
        }
        let closed = Rc::get_mut(&mut session)
            .ok_or("Session still referenced")
            .and_then(|s| s.close().map_err(|_| "Test session cleanup failed"));
        let removed = directory.close();
        report.cleanup_complete = closed.is_ok() && removed.is_ok();
        if let Err(e) = closed {
            report.error.get_or_insert(e.into());
        }
        if let Err(e) = removed {
            report.error.get_or_insert(e.to_string());
        }
        if report.error.is_none() && report.cleanup_complete {
            report.status = "passed".into();
        }
        report
    }
}
