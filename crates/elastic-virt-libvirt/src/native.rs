use crate::{
    parse_domain_xml, AdapterError, DomainObservation, DomainState, InventoryReport, LabStatus,
    TestDriverLabReport, TEST_URI,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use virt::{connect::Connect, domain::Domain};

static NEXT_NAME: AtomicU64 = AtomicU64::new(0);

fn native_error(error: impl std::fmt::Display) -> AdapterError {
    AdapterError::Native(error.to_string())
}

// Connect has no Drop implementation in virt 0.4.3. Every path must close it;
// callers keep Domain handles inside this connection's lifetime.
struct Connection(Option<Connect>);

impl Connection {
    fn open(readonly: bool) -> Result<Self, AdapterError> {
        let connection = if readonly {
            Connect::open_read_only(Some(TEST_URI))
        } else {
            Connect::open(Some(TEST_URI))
        }
        .map_err(native_error)?;
        let result = Self(Some(connection));
        if result.get().get_uri().map_err(native_error)? != TEST_URI
            || result.get().get_type().map_err(native_error)? != "TEST"
        {
            return Err(native_error(
                "the connection did not resolve to the test driver",
            ));
        }
        Ok(result)
    }

    fn get(&self) -> &Connect {
        self.0
            .as_ref()
            .expect("connection remains open in its scope")
    }

    fn close(&mut self) -> Result<(), AdapterError> {
        if let Some(mut connection) = self.0.take() {
            let references = connection.close().map_err(native_error)?;
            if references != 0 {
                return Err(native_error(
                    "libvirt connection retained unexpected references",
                ));
            }
        }
        Ok(())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn observe(domain: &Domain) -> Result<DomainObservation, AdapterError> {
    let uuid = domain.get_uuid_string().map_err(native_error)?;
    let active = domain.is_active().map_err(native_error)?;
    let (state, _) = domain.get_state().map_err(native_error)?;
    // Flags 0 explicitly request the live XML of an active domain. Never use
    // VIR_DOMAIN_XML_INACTIVE or VIR_DOMAIN_XML_SECURE here.
    let xml = domain.get_xml_desc(0).map_err(native_error)?;
    let mut observation =
        parse_domain_xml(&uuid, active, DomainState::from_libvirt(state), Some(&xml));
    let active_after = domain.is_active().map_err(native_error)?;
    let (state_after, _) = domain.get_state().map_err(native_error)?;
    if active != active_after || state != state_after {
        observation.resource_references_complete = false;
        observation
            .unknown_reasons
            .push("Domain state changed during XML observation".into());
    }
    Ok(observation)
}

fn inventory(connection: &Connect) -> Result<InventoryReport, AdapterError> {
    // virConnectListAllDomains(0) includes both active and inactive domains.
    // A failure of any domain read returns an error, never an empty complete list.
    let mut domains: Vec<_> = connection
        .list_all_domains(0)
        .map_err(native_error)?
        .iter()
        .map(observe)
        .collect::<Result<_, _>>()?;
    domains.sort_by(|left, right| left.uuid.cmp(&right.uuid));
    if domains.windows(2).any(|pair| pair[0].uuid == pair[1].uuid) {
        return Err(native_error("duplicate domain UUID in inventory"));
    }
    Ok(InventoryReport {
        schema_version: 1,
        uri: TEST_URI.into(),
        backend_simulated: true,
        domain_listing_complete: true,
        xml_resource_references_complete: domains.iter().all(|domain| domain.resource_references_complete),
        observation_is_atomic: false,
        storage_observation_complete: false,
        domains,
        limitations: vec![
            "Only libvirt's test driver was queried; this is not the host QEMU inventory".into(),
            "XML paths are references only: identity, mounts, open handles and backing-file contents are not inspected".into(),
            "Unsupported XML forms remain unknown; all references require fresh host reconciliation before leasing".into(),
        ],
    })
}

pub(super) fn read_inventory() -> Result<InventoryReport, AdapterError> {
    let mut connection = Connection::open(true)?;
    let result = inventory(connection.get());
    connection.close()?;
    result
}

struct OwnedDomain {
    domain: Domain,
    uuid: String,
    name: String,
    removed: bool,
}

impl OwnedDomain {
    fn verify_identity(&self) -> Result<(), AdapterError> {
        if self.domain.get_uuid_string().map_err(native_error)? != self.uuid
            || self.domain.get_name().map_err(native_error)? != self.name
        {
            return Err(native_error(
                "refusing to mutate a domain whose owned identity changed",
            ));
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), AdapterError> {
        if self.removed {
            return Ok(());
        }
        self.verify_identity()?;
        if self.domain.is_active().map_err(native_error)? {
            self.domain.destroy().map_err(native_error)?;
        }
        if self.domain.is_active().map_err(native_error)? {
            return Err(native_error("owned test domain is still active after stop"));
        }
        self.domain.undefine().map_err(native_error)?;
        self.removed = true;
        Ok(())
    }
}

impl Drop for OwnedDomain {
    fn drop(&mut self) {
        // Explicit cleanup supplies report evidence. This is only a best-effort
        // fallback for unwinding and can target only the exact owned handle.
        let _ = self.cleanup();
    }
}

pub(super) fn run_lab() -> TestDriverLabReport {
    let mut report = TestDriverLabReport::pending();
    report.status = LabStatus::Failed;
    let mut connection = match Connection::open(false) {
        Ok(connection) => connection,
        Err(error) => {
            report.error = Some(error.to_string());
            return report;
        }
    };
    report.evidence_kind = "real_libvirt_test_driver".into();
    report.check(
        "test_uri_and_driver",
        true,
        "Connected only to test:///default; reported driver is TEST",
    );
    let mut owned: Option<OwnedDomain> = None;
    let outcome = (|| -> Result<(), AdapterError> {
        report.libvirt_version = Some(connection.get().get_lib_version().map_err(native_error)?);
        let baseline = inventory(connection.get())?;
        report.check(
            "initial_inventory",
            true,
            format!(
                "Observed {} existing simulated domain(s)",
                baseline.domains.len()
            ),
        );
        report.initial_inventory = Some(baseline);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(native_error)?
            .as_nanos();
        let name = format!(
            "elasticpc-lab-{}-{timestamp}-{}",
            std::process::id(),
            NEXT_NAME.fetch_add(1, Ordering::Relaxed)
        );
        let existing = connection.get().list_all_domains(0).map_err(native_error)?;
        for domain in &existing {
            if domain.get_name().map_err(native_error)? == name {
                return Err(native_error(
                    "new lab domain name unexpectedly already exists",
                ));
            }
        }
        drop(existing);
        // Built internally from numeric identifier components. No user XML,
        // physical disks, hostdevs, network or firmware paths are accepted.
        let xml = format!("<domain type='test'><name>{name}</name><memory unit='KiB'>8192</memory><vcpu>1</vcpu><os><type>hvm</type></os><devices/></domain>");
        let domain = Domain::define_xml(connection.get(), &xml).map_err(native_error)?;
        let uuid = domain.get_uuid_string().map_err(native_error)?;
        report.domain_uuid = Some(uuid.clone());
        owned = Some(OwnedDomain {
            domain,
            uuid,
            name,
            removed: false,
        });
        let domain = owned.as_ref().expect("newly owned domain");
        domain.verify_identity()?;
        let defined = observe(&domain.domain)?;
        let defined_ok = !defined.active && defined.state == DomainState::Shutoff;
        report.check(
            "defined_inactive",
            defined_ok,
            "Fresh definition is inactive and shut off",
        );
        if !defined_ok {
            return Err(native_error("fresh definition was not inactive"));
        }
        domain.domain.create().map_err(native_error)?;
        let running = observe(&domain.domain)?;
        let running_ok = running.active && running.state == DomainState::Running;
        report.check(
            "started_and_observed",
            running_ok,
            "Test driver reports running state after create",
        );
        let xml_ok = running.xml_scope == crate::XmlScope::Live
            && running.resource_references_complete
            && running.resources.is_empty();
        report.check(
            "live_xml_without_host_resources",
            xml_ok,
            "Flags-0 live XML has the owned UUID and no storage or hostdev references",
        );
        report.running_domain = Some(running);
        if !running_ok || !xml_ok {
            return Err(native_error("running domain observation was incomplete"));
        }
        domain.verify_identity()?;
        domain.domain.destroy().map_err(native_error)?;
        let stopped = observe(&domain.domain)?;
        let stopped_ok = !stopped.active && stopped.state == DomainState::Shutoff;
        report.check(
            "stopped_and_observed",
            stopped_ok,
            "Only the owned test domain was stopped; observed shutoff state",
        );
        if !stopped_ok {
            return Err(native_error("stopped domain observation was incomplete"));
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        report.error = Some(error.to_string());
    }
    let cleanup_result = match owned.as_mut() {
        Some(domain) => domain.cleanup(),
        None => Ok(()),
    };
    let cleanup_ok = cleanup_result.is_ok();
    report.check(
        "owned_domain_undefined",
        cleanup_ok,
        "Stopped and undefined only the freshly owned test definition",
    );
    if let Err(error) = cleanup_result {
        report.error.get_or_insert_with(|| error.to_string());
    }
    // Release all Domain handles before closing Connect.
    drop(owned);
    let restored = match (
        report.initial_inventory.as_ref(),
        inventory(connection.get()),
    ) {
        (Some(before), Ok(after)) => before.domains == after.domains,
        _ => false,
    };
    report.check(
        "existing_domains_preserved",
        restored,
        "Final UUID/state/resource observations equal the initial test-driver inventory",
    );
    let closed = connection.close();
    let closed_ok = closed.is_ok();
    report.check(
        "connection_closed",
        closed_ok,
        "Closed the libvirt connection with no outstanding references",
    );
    if let Err(error) = closed {
        report.error.get_or_insert_with(|| error.to_string());
    }
    report.cleanup_complete = cleanup_ok && restored && closed_ok;
    if report.error.is_none()
        && report.cleanup_complete
        && report.checks.iter().all(|check| check.passed)
    {
        report.status = LabStatus::Passed;
    }
    report
}

/// A retained in-process test-driver connection. Keeping this session alive
/// across controller reconstruction is essential. TEST state is per-process;
/// a process restart cannot prove recovery of real QEMU processes.
pub struct TestSession {
    domains: Vec<OwnedDomain>,
    baseline: DomainObservation,
    connection: Connection,
    closed: bool,
}
impl TestSession {
    pub fn diskless(count: usize) -> Result<Self, AdapterError> {
        if !(1..=8).contains(&count) {
            return Err(native_error("test session requires 1..=8 domains"));
        }
        let connection = Connection::open(false)?;
        let baseline =
            observe(&Domain::lookup_by_name(connection.get(), "test").map_err(native_error)?)?;
        let mut session = Self {
            domains: Vec::new(),
            baseline,
            connection,
            closed: false,
        };
        for _ in 0..count {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(native_error)?
                .as_nanos();
            let name = format!(
                "elasticpc-controller-{}-{timestamp}-{}",
                std::process::id(),
                NEXT_NAME.fetch_add(1, Ordering::Relaxed)
            );
            for existing in session
                .connection
                .get()
                .list_all_domains(0)
                .map_err(native_error)?
            {
                if existing.get_name().map_err(native_error)? == name {
                    return Err(native_error("test name collision"));
                }
            }
            let xml = format!("<domain type='test'><name>{name}</name><memory unit='KiB'>8192</memory><vcpu>1</vcpu><os><type>hvm</type></os><devices/></domain>");
            let domain =
                Domain::define_xml(session.connection.get(), &xml).map_err(native_error)?;
            let uuid = domain.get_uuid_string().map_err(native_error)?;
            session.domains.push(OwnedDomain {
                domain,
                uuid,
                name,
                removed: false,
            });
        }
        Ok(session)
    }
    pub fn domain_ids(&self) -> Vec<String> {
        self.domains.iter().map(|d| d.uuid.clone()).collect()
    }
    fn owned(&self, uuid: &str) -> Result<&OwnedDomain, AdapterError> {
        if self.closed {
            return Err(native_error("test session closed"));
        }
        let domain = self
            .domains
            .iter()
            .find(|d| d.uuid == uuid)
            .ok_or_else(|| native_error("domain not owned by this test session"))?;
        domain.verify_identity()?;
        Ok(domain)
    }
    pub fn observe_owned(&self, uuid: &str) -> Result<DomainObservation, AdapterError> {
        let observation = observe(&self.owned(uuid)?.domain)?;
        if !observation.resource_references_complete || !observation.resources.is_empty() {
            return Err(native_error(
                "owned test domain is no longer a verified diskless configuration",
            ));
        }
        Ok(observation)
    }
    pub fn start_owned(&self, uuid: &str) -> Result<(), AdapterError> {
        let observed = self.observe_owned(uuid)?;
        if !observed.active {
            if observed.state != DomainState::Shutoff {
                return Err(native_error("test domain state is uncertain"));
            }
            self.owned(uuid)?.domain.create().map_err(native_error)?;
        }
        Ok(())
    }
    pub fn stop_owned(&self, uuid: &str) -> Result<(), AdapterError> {
        let observed = self.observe_owned(uuid)?;
        if observed.active {
            self.owned(uuid)?.domain.destroy().map_err(native_error)?;
        }
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), AdapterError> {
        if self.closed {
            return Ok(());
        }
        for domain in &mut self.domains {
            domain.cleanup()?;
        }
        self.domains.clear();
        if observe(&Domain::lookup_by_name(self.connection.get(), "test").map_err(native_error)?)?
            != self.baseline
        {
            return Err(native_error(
                "test session changed the default fixture domain",
            ));
        }
        let result = self.connection.close();
        self.closed = true;
        result
    }
}
impl Drop for TestSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
