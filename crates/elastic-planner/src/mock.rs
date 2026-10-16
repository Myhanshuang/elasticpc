//! Backend-neutral lifecycle and lease logic, with a default in-memory backend.
//! External execution and durable storage are supplied through explicit traits;
//! this module contains no physical-device executor.
use crate::leases::{
    HostObservation, Lease, LeaseError, LeaseManager, LeaseMode, LeaseRequest, ResourceTopology,
};
use elastic_core::{
    CapabilityStatus, DesiredState, DomainId, InstallationId, LeaseResource, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockCapabilities {
    pub lifecycle_simulation: CapabilityStatus,
    pub physical_device_execution: CapabilityStatus,
    pub durable_journal: CapabilityStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedGuest {
    pub domain_id: DomainId,
    pub installation_id: InstallationId,
    pub resources: Vec<LeaseRequest>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StartFault {
    #[default]
    None,
    Reject,
    /// Simulates losing the response after QEMU has already acquired devices.
    ErrorAfterRunning,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StopFault {
    #[default]
    None,
    Reject,
    ErrorAfterStopped,
}

/// The backend's state survives a simulated controller crash independently of
/// the controller journal. All resources below are inert, typed identifiers.
#[derive(Debug, Clone)]
pub struct MockHypervisor {
    guests: BTreeMap<DomainId, ObservedGuest>,
    host: HostObservation,
    observation_available: bool,
    next_start_fault: StartFault,
    next_stop_fault: StopFault,
    start_calls: usize,
    stop_calls: usize,
}

impl Default for MockHypervisor {
    fn default() -> Self {
        Self {
            guests: BTreeMap::new(),
            host: HostObservation::known(Vec::new()),
            observation_available: true,
            next_start_fault: StartFault::None,
            next_stop_fault: StopFault::None,
            start_calls: 0,
            stop_calls: 0,
        }
    }
}

impl MockHypervisor {
    pub fn capabilities(&self) -> MockCapabilities {
        MockCapabilities {
            lifecycle_simulation: CapabilityStatus::Supported,
            physical_device_execution: CapabilityStatus::Unsupported,
            durable_journal: CapabilityStatus::Unsupported,
        }
    }

    pub fn set_host_observation(&mut self, host: HostObservation) {
        self.host = host;
    }
    pub fn set_observation_available(&mut self, available: bool) {
        self.observation_available = available;
    }
    pub fn fail_next_start(&mut self, fault: StartFault) {
        self.next_start_fault = fault;
    }
    pub fn fail_next_stop(&mut self, fault: StopFault) {
        self.next_stop_fault = fault;
    }
    pub fn start_calls(&self) -> usize {
        self.start_calls
    }
    pub fn stop_calls(&self) -> usize {
        self.stop_calls
    }

    /// Model a preexisting guest or a change by an external supervisor.
    pub fn observe_running(&mut self, mut guest: ObservedGuest) {
        guest.resources = canonical_requests(&guest.resources);
        self.guests.insert(guest.domain_id.clone(), guest);
    }

    /// Model a positively observed process exit, including cessation of I/O.
    pub fn observe_stopped(&mut self, domain: &DomainId) {
        self.guests.remove(domain);
    }

    fn start(&mut self, guest: ObservedGuest) -> Result<(), ControllerError> {
        if self.guests.contains_key(&guest.domain_id) {
            return Ok(());
        }
        self.start_calls += 1;
        let fault = std::mem::take(&mut self.next_start_fault);
        if fault == StartFault::Reject {
            return Err(ControllerError::Backend("mock start rejected".into()));
        }
        self.observe_running(guest);
        if fault == StartFault::ErrorAfterRunning {
            return Err(ControllerError::Backend(
                "mock start response lost after guest began I/O".into(),
            ));
        }
        Ok(())
    }

    fn stop(&mut self, domain: &DomainId) -> Result<(), ControllerError> {
        if !self.guests.contains_key(domain) {
            return Ok(());
        }
        self.stop_calls += 1;
        let fault = std::mem::take(&mut self.next_stop_fault);
        if fault == StopFault::Reject {
            return Err(ControllerError::Backend(
                "mock stop rejected; guest still owns devices".into(),
            ));
        }
        self.observe_stopped(domain);
        if fault == StopFault::ErrorAfterStopped {
            return Err(ControllerError::Backend(
                "mock stop response lost after guest exit".into(),
            ));
        }
        Ok(())
    }

    fn domain_state(&self, domain: &DomainId) -> ControllerState {
        if !self.observation_available {
            ControllerState::Unknown
        } else if self.guests.contains_key(domain) {
            ControllerState::Running
        } else {
            ControllerState::Stopped
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerState {
    Discovered,
    Planned,
    Validated,
    Leasing,
    Attaching,
    Running,
    GracefulShutdown,
    Detaching,
    LeaseReleased,
    Stopped,
    Rollback,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashPoint {
    AfterJournal,
    AfterLeases,
    AfterStart,
    AfterStopIntent,
    AfterStop,
    AfterRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub operation_id: String,
    pub installation_id: InstallationId,
    pub domain_id: DomainId,
    pub desired_state: DesiredState,
    pub preconditions: Vec<String>,
    pub requested_resources: Vec<LeaseRequest>,
    pub acquired_resources: Vec<LeaseRequest>,
    pub steps: Vec<ControllerState>,
    pub compensations: Vec<String>,
    pub completed: bool,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockJournal {
    pub schema_version: u32,
    pub operations: Vec<OperationRecord>,
    pub reservations: Vec<Lease>,
}

impl Default for MockJournal {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            operations: Vec::new(),
            reservations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    Lease(LeaseError),
    Backend(String),
    UnknownObservation,
    OperationIdReused,
    PreviousOperationFailed(String),
    RunningConfigurationChanged,
    SimulatedCrash(CrashPoint),
    InvalidJournal(String),
    Persistence(String),
}

impl std::fmt::Display for ControllerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ControllerError {}
impl From<LeaseError> for ControllerError {
    fn from(value: LeaseError) -> Self {
        Self::Lease(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub running_domains: Vec<DomainId>,
    pub retained_resources: usize,
    /// Existing unsafe external ownership is reported and retained, never hidden
    /// by dropping one of the observed owners.
    pub conflicts: Vec<LeaseError>,
}

/// A durable adapter must commit synchronously before returning success. A sink
/// error poisons this controller: reconstruct it from storage and fresh backend
/// observation before any further lifecycle side effects.
pub trait JournalSink {
    fn commit(&mut self, journal: &MockJournal) -> Result<(), String>;
}

/// One backend observation. A complete host view is meaningful only within the
/// backend's declared scope; the test-driver adapter accepts no physical claims.
#[derive(Clone, Debug)]
pub struct BackendObservation {
    pub available: bool,
    pub host: HostObservation,
    pub guests: BTreeMap<DomainId, ObservedGuest>,
}

/// Backend observations must be fresh and conservative. A failed request never
/// establishes stopped I/O. `Stopped` must positively establish cessation within
/// the backend's scope; incomplete observations must remain unknown. Adapters
/// must reject resources outside their scope before any lifecycle side effect.
pub trait LifecycleBackend {
    fn observe(&self) -> BackendObservation;
    fn start(&mut self, guest: ObservedGuest) -> Result<(), ControllerError>;
    fn stop(&mut self, domain: &DomainId) -> Result<(), ControllerError>;
    fn domain_state(&self, domain: &DomainId) -> ControllerState;
    fn validate_start(&self, _guest: &ObservedGuest) -> Result<(), ControllerError> {
        Ok(())
    }
    fn validate_stop(
        &self,
        _installation: &InstallationId,
        _domain: &DomainId,
    ) -> Result<(), ControllerError> {
        Ok(())
    }
    fn validate_journal(&self, _journal: &MockJournal) -> Result<(), ControllerError> {
        Ok(())
    }
}

impl LifecycleBackend for MockHypervisor {
    fn observe(&self) -> BackendObservation {
        BackendObservation {
            available: self.observation_available,
            host: self.host.clone(),
            guests: self.guests.clone(),
        }
    }
    fn start(&mut self, guest: ObservedGuest) -> Result<(), ControllerError> {
        MockHypervisor::start(self, guest)
    }
    fn stop(&mut self, domain: &DomainId) -> Result<(), ControllerError> {
        MockHypervisor::stop(self, domain)
    }
    fn domain_state(&self, domain: &DomainId) -> ControllerState {
        MockHypervisor::domain_state(self, domain)
    }
}

pub struct MockController<B: LifecycleBackend = MockHypervisor> {
    leases: LeaseManager,
    backend: B,
    journal: MockJournal,
    crash_point: Option<CrashPoint>,
    journal_sink: Option<Box<dyn JournalSink>>,
    persistence_failed: bool,
}

fn canonical_requests(requests: &[LeaseRequest]) -> Vec<LeaseRequest> {
    let mut normalized: BTreeMap<LeaseResource, LeaseMode> = BTreeMap::new();
    for request in requests {
        normalized
            .entry(request.resource.clone())
            .and_modify(|mode| {
                if request.mode == LeaseMode::ExclusiveWrite {
                    *mode = LeaseMode::ExclusiveWrite;
                }
            })
            .or_insert(request.mode);
    }
    normalized
        .into_iter()
        .map(|(resource, mode)| LeaseRequest { resource, mode })
        .collect()
}

impl<B: LifecycleBackend> MockController<B> {
    pub fn new(topology: ResourceTopology, backend: B) -> Self {
        Self {
            leases: LeaseManager::new(topology, HostObservation::unknown()),
            backend,
            journal: MockJournal::default(),
            crash_point: None,
            journal_sink: None,
            persistence_failed: false,
        }
    }

    pub fn attach_journal_sink(
        &mut self,
        sink: Box<dyn JournalSink>,
    ) -> Result<(), ControllerError> {
        if self.journal_sink.is_some() || self.persistence_failed {
            return Err(ControllerError::Persistence(
                "journal sink cannot be replaced on a live controller".into(),
            ));
        }
        self.journal_sink = Some(sink);
        Ok(())
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn leases(&self) -> &LeaseManager {
        &self.leases
    }
    pub fn journal(&self) -> &MockJournal {
        &self.journal
    }
    pub fn inject_crash(&mut self, point: CrashPoint) {
        self.crash_point = Some(point);
    }
    pub fn state(&self, domain: &DomainId) -> ControllerState {
        self.backend.domain_state(domain)
    }

    pub fn journal_json(&self) -> Result<String, ControllerError> {
        let mut checkpoint = self.journal.clone();
        checkpoint.reservations = self.leases.leases().to_vec();
        serde_json::to_string_pretty(&checkpoint)
            .map_err(|error| ControllerError::InvalidJournal(error.to_string()))
    }

    /// Construction quarantines checkpointed claims until a fresh complete
    /// observation is obtained. Unknown enum variants or schema versions fail.
    pub fn from_journal(
        topology: ResourceTopology,
        backend: B,
        json: &str,
    ) -> Result<Self, ControllerError> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|error| ControllerError::InvalidJournal(error.to_string()))?;
        elastic_core::check_schema(&value).map_err(ControllerError::InvalidJournal)?;
        let journal: MockJournal = serde_json::from_value(value)
            .map_err(|error| ControllerError::InvalidJournal(error.to_string()))?;
        backend.validate_journal(&journal)?;
        let mut controller = Self {
            leases: LeaseManager::new(topology, HostObservation::unknown()),
            backend,
            journal,
            crash_point: None,
            journal_sink: None,
            persistence_failed: false,
        };
        controller.retain_uncertain_reservations();
        Ok(controller)
    }

    fn checkpoint(&mut self) -> Result<(), ControllerError> {
        self.journal.reservations = self.leases.leases().to_vec();
        if self.persistence_failed {
            return Err(ControllerError::Persistence(
                "previous journal commit failed; reconstruct controller before further operations"
                    .into(),
            ));
        }
        if let Some(sink) = self.journal_sink.as_mut() {
            if let Err(message) = sink.commit(&self.journal) {
                self.persistence_failed = true;
                return Err(ControllerError::Persistence(message));
            }
        }
        Ok(())
    }

    fn crash(&mut self, point: CrashPoint) -> Result<(), ControllerError> {
        self.checkpoint()?;
        if self.crash_point == Some(point) {
            self.crash_point = None;
            return Err(ControllerError::SimulatedCrash(point));
        }
        Ok(())
    }

    fn retain_uncertain_reservations(&mut self) {
        for lease in &self.journal.reservations {
            self.leases.reserve_observed(&lease.owner, &lease.request);
        }
        // An interrupted attach may have acquired devices after the most recent
        // persisted reservation checkpoint. Keep its intended claims as well.
        for record in &self.journal.operations {
            if !record.completed && record.desired_state == DesiredState::Running {
                for request in &record.requested_resources {
                    self.leases.reserve_observed(&record.domain_id, request);
                }
            }
        }
        self.leases.mark_observation_unknown();
    }

    pub fn recover(&mut self) -> Result<RecoveryReport, ControllerError> {
        let observed_backend = self.backend.observe();
        if !observed_backend.available || !observed_backend.host.complete {
            if observed_backend.available {
                for guest in observed_backend.guests.values() {
                    for request in &guest.resources {
                        self.leases.reserve_observed(&guest.domain_id, request);
                    }
                }
            }
            self.retain_uncertain_reservations();
            self.checkpoint()?;
            return Err(ControllerError::UnknownObservation);
        }
        let observed = observed_backend
            .guests
            .values()
            .flat_map(|guest| {
                guest.resources.iter().map(|request| Lease {
                    owner: guest.domain_id.clone(),
                    request: request.clone(),
                })
            })
            .collect();
        self.leases
            .replace_observations(observed_backend.host.clone(), observed);
        let mut conflicts = Vec::new();
        for guest in observed_backend.guests.values() {
            if let Err(error) = self.leases.acquire(&guest.domain_id, &guest.resources) {
                conflicts.push(error);
            }
        }
        self.checkpoint()?;
        Ok(RecoveryReport {
            running_domains: observed_backend.guests.keys().cloned().collect(),
            retained_resources: self.leases.leases().len(),
            conflicts,
        })
    }

    fn operation(
        &mut self,
        operation_id: &str,
        installation_id: &InstallationId,
        domain_id: &DomainId,
        desired_state: DesiredState,
        requests: &[LeaseRequest],
    ) -> Result<usize, ControllerError> {
        if let Some(index) = self
            .journal
            .operations
            .iter()
            .position(|op| op.operation_id == operation_id)
        {
            let record = &self.journal.operations[index];
            if &record.installation_id != installation_id
                || &record.domain_id != domain_id
                || record.desired_state != desired_state
                || (desired_state == DesiredState::Running
                    && record.requested_resources != requests)
            {
                return Err(ControllerError::OperationIdReused);
            }
            if let Some(failure) = &record.failure {
                return Err(ControllerError::PreviousOperationFailed(failure.clone()));
            }
            return Ok(index);
        }
        self.journal.operations.push(OperationRecord {
            operation_id: operation_id.into(),
            installation_id: installation_id.clone(),
            domain_id: domain_id.clone(),
            desired_state,
            preconditions: vec![
                "complete fresh guest and host occupancy observation".into(),
                "no conflicting resource owner".into(),
            ],
            requested_resources: requests.to_vec(),
            acquired_resources: Vec::new(),
            steps: vec![ControllerState::Discovered, ControllerState::Planned],
            compensations: Vec::new(),
            completed: false,
            failure: None,
        });
        Ok(self.journal.operations.len() - 1)
    }

    fn finish(&mut self, index: usize, state: ControllerState) -> Result<(), ControllerError> {
        let record = &mut self.journal.operations[index];
        record.steps.push(state);
        record.completed = true;
        self.checkpoint()
    }

    fn fail(&mut self, index: usize, error: &ControllerError) -> Result<(), ControllerError> {
        self.journal.operations[index]
            .steps
            .push(ControllerState::Failed);
        self.journal.operations[index].failure = Some(error.to_string());
        self.checkpoint()
    }

    pub fn start(
        &mut self,
        operation_id: &str,
        installation_id: &InstallationId,
        domain_id: &DomainId,
        requests: &[LeaseRequest],
    ) -> Result<ControllerState, ControllerError> {
        let requests = canonical_requests(requests);
        self.backend.validate_start(&ObservedGuest {
            domain_id: domain_id.clone(),
            installation_id: installation_id.clone(),
            resources: requests.clone(),
        })?;
        let index = self.operation(
            operation_id,
            installation_id,
            domain_id,
            DesiredState::Running,
            &requests,
        )?;
        self.crash(CrashPoint::AfterJournal)?;
        self.recover()?;
        if self.journal.operations[index].completed {
            return Ok(self.state(domain_id));
        }
        let observed_backend = self.backend.observe();
        if !observed_backend.available || !observed_backend.host.complete {
            return Err(ControllerError::UnknownObservation);
        }
        if let Some(guest) = observed_backend.guests.get(domain_id) {
            if &guest.installation_id != installation_id || guest.resources != requests {
                return Err(ControllerError::RunningConfigurationChanged);
            }
            self.finish(index, ControllerState::Running)?;
            return Ok(ControllerState::Running);
        }
        self.journal.operations[index]
            .steps
            .push(ControllerState::Validated);
        if let Err(error) = self.leases.acquire(domain_id, &requests) {
            let error = ControllerError::Lease(error);
            self.fail(index, &error)?;
            return Err(error);
        }
        self.journal.operations[index].acquired_resources = requests.clone();
        self.journal.operations[index]
            .steps
            .push(ControllerState::Leasing);
        self.crash(CrashPoint::AfterLeases)?;
        self.journal.operations[index]
            .steps
            .push(ControllerState::Attaching);
        let outcome = self.backend.start(ObservedGuest {
            domain_id: domain_id.clone(),
            installation_id: installation_id.clone(),
            resources: requests,
        });
        self.crash(CrashPoint::AfterStart)?;
        if let Err(error) = outcome {
            self.journal.operations[index]
                .steps
                .push(ControllerState::Rollback);
            // A failed RPC is not evidence that the guest failed to start.
            if self.state(domain_id) == ControllerState::Stopped {
                self.leases.release_after_io_stopped(domain_id);
                self.journal.operations[index]
                    .compensations
                    .push("observed no guest I/O; released reservations".into());
            } else {
                self.journal.operations[index]
                    .compensations
                    .push("retained reservations: guest I/O running or unknown".into());
            }
            self.fail(index, &error)?;
            return Err(error);
        }
        self.finish(index, ControllerState::Running)?;
        Ok(ControllerState::Running)
    }

    pub fn stop(
        &mut self,
        operation_id: &str,
        installation_id: &InstallationId,
        domain_id: &DomainId,
    ) -> Result<ControllerState, ControllerError> {
        self.backend.validate_stop(installation_id, domain_id)?;
        let index = self.operation(
            operation_id,
            installation_id,
            domain_id,
            DesiredState::Stopped,
            &[],
        )?;
        // Stopping an existing guest is safe even if inspection is unavailable;
        // releasing its resources still requires a positive stopped observation.
        match self.recover() {
            Ok(_) | Err(ControllerError::UnknownObservation) => {}
            Err(error) => return Err(error),
        }
        if self.journal.operations[index].completed {
            return Ok(self.state(domain_id));
        }
        let observed_backend = self.backend.observe();
        if observed_backend.available
            && observed_backend
                .guests
                .get(domain_id)
                .is_some_and(|guest| &guest.installation_id != installation_id)
        {
            return Err(ControllerError::RunningConfigurationChanged);
        }
        self.journal.operations[index]
            .steps
            .push(ControllerState::GracefulShutdown);
        self.crash(CrashPoint::AfterStopIntent)?;
        let outcome = self.backend.stop(domain_id);
        self.crash(CrashPoint::AfterStop)?;
        match self.state(domain_id) {
            ControllerState::Stopped
                if {
                    let observation = self.backend.observe();
                    observation.available && observation.host.complete
                } =>
            {
                self.journal.operations[index]
                    .steps
                    .push(ControllerState::Detaching);
                self.leases.release_after_io_stopped(domain_id);
                self.journal.operations[index]
                    .steps
                    .push(ControllerState::LeaseReleased);
                if outcome.is_err() {
                    self.journal.operations[index].compensations.push(
                        "lost stop response resolved by observed guest exit and stopped I/O".into(),
                    );
                }
                self.crash(CrashPoint::AfterRelease)?;
                self.finish(index, ControllerState::Stopped)?;
                Ok(ControllerState::Stopped)
            }
            _ => {
                let error = outcome.err().unwrap_or(ControllerError::UnknownObservation);
                self.journal.operations[index]
                    .compensations
                    .push("retained reservations until stopped I/O can be observed".into());
                self.fail(index, &error)?;
                Err(error)
            }
        }
    }
}

/// Backend-neutral name; MockController remains as a compatibility entry point.
pub type LifecycleController<B> = MockController<B>;
