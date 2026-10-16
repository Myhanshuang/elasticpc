//! Conservative libvirt XML observation and an explicitly invoked test-driver lab.
//!
//! The only connection URI accepted is `test:///default`. The native feature
//! uses the official `virt` binding, but the libvirt test driver simulates guests.
//! This crate cannot connect to the host's QEMU driver or authorize physical I/O.

mod inventory;
#[cfg(feature = "native-libvirt")]
mod native;

pub use inventory::{
    parse_domain_xml, DomainObservation, DomainState, InventoryReport, ResourceKind,
    ResourceReference, XmlScope,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const TEST_URI: &str = "test:///default";

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("only the exact in-process test:///default URI is permitted")]
    UnsafeUri,
    #[error("native libvirt support was not compiled; enable native-libvirt")]
    NativeUnavailable,
    #[error("libvirt test-driver operation failed: {0}")]
    Native(String),
}

/// Validate before any libvirt call, including native initialization. Empty,
/// default, remote, session, system and test-file URIs are all rejected.
pub fn validate_test_uri(uri: &str) -> Result<(), AdapterError> {
    if uri == TEST_URI {
        Ok(())
    } else {
        Err(AdapterError::UnsafeUri)
    }
}

/// Read-only inventory of libvirt's simulated test domains. No host daemon
/// socket is opened, even when a default URI is configured in the environment.
pub fn read_test_driver_inventory(uri: &str) -> Result<InventoryReport, AdapterError> {
    validate_test_uri(uri)?;
    #[cfg(feature = "native-libvirt")]
    {
        native::read_inventory()
    }
    #[cfg(not(feature = "native-libvirt"))]
    {
        Err(AdapterError::NativeUnavailable)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDriverLabReport {
    pub schema_version: u32,
    pub status: LabStatus,
    pub evidence_kind: String,
    pub uri: String,
    pub binding_version: String,
    pub libvirt_version: Option<u32>,
    pub backend_simulated: bool,
    pub guest_boot_verified: bool,
    pub physical_execution: bool,
    pub cleanup_complete: bool,
    pub domain_uuid: Option<String>,
    pub initial_inventory: Option<InventoryReport>,
    pub running_domain: Option<DomainObservation>,
    pub checks: Vec<LabCheck>,
    pub limitations: Vec<String>,
    pub error: Option<String>,
}

impl TestDriverLabReport {
    fn pending() -> Self {
        Self {
            schema_version: 1,
            status: LabStatus::Skipped,
            evidence_kind: "not_run".into(),
            uri: TEST_URI.into(),
            binding_version: "virt 0.4.3".into(),
            libvirt_version: None,
            backend_simulated: true,
            guest_boot_verified: false,
            physical_execution: false,
            cleanup_complete: false,
            domain_uuid: None,
            initial_inventory: None,
            running_domain: None,
            checks: Vec::new(),
            limitations: vec![
                "The real libvirt C API uses its in-process simulated test driver; no guest CPU or disk I/O is executed".into(),
                "Host QEMU, KVM, device occupancy, lease release and physical execution remain unverified".into(),
                "Domain snapshots are sequential observations, not an atomic host-wide storage lock".into(),
            ],
            error: None,
        }
    }

    #[cfg(feature = "native-libvirt")]
    fn check(&mut self, name: &str, passed: bool, detail: impl Into<String>) {
        self.checks.push(LabCheck {
            name: name.into(),
            passed,
            detail: detail.into(),
        });
    }
}

/// Define/start/observe/stop/undefine only a freshly named domain on the exact
/// in-process test driver. The default fixture domain is never mutated.
pub fn run_test_driver_lab() -> TestDriverLabReport {
    #[cfg(feature = "native-libvirt")]
    {
        native::run_lab()
    }
    #[cfg(not(feature = "native-libvirt"))]
    {
        let mut report = TestDriverLabReport::pending();
        report.error = Some(AdapterError::NativeUnavailable.to_string());
        report
    }
}

#[cfg(feature = "native-libvirt")]
pub use native::TestSession;
