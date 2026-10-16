//! Versioned, serializable values shared by observation and pure planning.
//! No privileged execution or device mutation belongs in this crate.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub mod redact;

pub const SCHEMA_VERSION: u32 = 1;
pub const GIB: u64 = 1024 * 1024 * 1024;

macro_rules! id_type {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl From<&str> for $name { fn from(s: &str) -> Self { Self(s.to_owned()) } }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
        }
    )+};
}
id_type!(
    InstallationId,
    DomainId,
    DiskStableId,
    PartUuid,
    UsbStableId,
    ConnectorId
);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Supported,
    Experimental,
    Unsupported,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub code: String,
    pub status: CapabilityStatus,
    pub message: String,
}
impl Finding {
    pub fn unknown(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            status: CapabilityStatus::Unknown,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuInfo {
    pub architecture: String,
    pub logical_cpus: Option<u32>,
    pub virtualization: CapabilityStatus,
    pub kvm: CapabilityStatus,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PciDevice {
    pub bdf: String,
    pub vendor_id: String,
    pub device_id: String,
    pub class: String,
    pub driver: Option<String>,
    pub iommu_group: Option<String>,
    pub reset_methods: Vec<String>,
    pub sriov_totalvfs: Option<u32>,
    pub sriov_numvfs: Option<u32>,
    pub mdev_types: Vec<String>,
    pub sysfs_path: Option<String>,
    pub link_speed: Option<String>,
    pub link_width: Option<u32>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IommuGroup {
    pub id: String,
    pub members: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    pub target: String,
    pub source: String,
    pub major_minor: Option<String>,
    pub read_only: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Partition {
    pub path: String,
    pub partuuid: Option<PartUuid>,
    pub uuid: Option<String>,
    pub part_type: Option<String>,
    pub filesystem: Option<String>,
    pub size_bytes: u64,
    pub start_bytes: Option<u64>,
    pub major_minor: Option<String>,
    pub mountpoints: Vec<String>,
    pub active_swap: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disk {
    /// Observed kernel path, diagnostic only; never an execution identity.
    pub path: String,
    pub stable_id: Option<DiskStableId>,
    pub model: Option<String>,
    pub transport: Option<String>,
    pub size_bytes: u64,
    pub gpt_guid: Option<String>,
    pub partition_table: Option<String>,
    pub major_minor: Option<String>,
    pub mountpoints: Vec<String>,
    pub partitions: Vec<Partition>,
    /// Only set when the partition layout actually proves unallocated capacity.
    pub unallocated_bytes: Option<u64>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EfiEntry {
    pub number: String,
    pub active: bool,
    pub label: String,
    pub device_path: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EfiInfo {
    pub available: bool,
    pub boot_current: Option<String>,
    pub boot_order: Vec<String>,
    pub boot_next: Option<String>,
    pub entries: Vec<EfiEntry>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayConnector {
    pub id: ConnectorId,
    pub card: String,
    pub pci_bdf: Option<String>,
    pub status: String,
    pub modes: Vec<String>,
    /// Content digest only; never log EDID serial numbers.
    pub edid_hash: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeInfo {
    pub references: Vec<String>,
    /// Unknown unless all relevant sources have actually been checked.
    pub status: CapabilityStatus,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareReport {
    pub schema_version: u32,
    /// "live" or "fixture"; fixture assertions are not measurements of this host.
    pub source: String,
    pub kernel: Option<String>,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub secure_boot: CapabilityStatus,
    pub iommu: CapabilityStatus,
    pub pci_devices: Vec<PciDevice>,
    pub iommu_groups: Vec<IommuGroup>,
    pub disks: Vec<Disk>,
    pub mounts: Vec<Mount>,
    pub resume: ResumeInfo,
    pub efi: EfiInfo,
    pub displays: Vec<DisplayConnector>,
    pub installations: Vec<Installation>,
    /// False for incomplete mount, swap, block holder or namespace visibility.
    pub storage_observation_complete: bool,
    pub findings: Vec<Finding>,
}
impl Default for HardwareReport {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            source: "unknown".into(),
            kernel: None,
            cpu: CpuInfo::default(),
            memory: MemoryInfo::default(),
            secure_boot: CapabilityStatus::Unknown,
            iommu: CapabilityStatus::Unknown,
            pci_devices: vec![],
            iommu_groups: vec![],
            disks: vec![],
            mounts: vec![],
            resume: ResumeInfo::default(),
            efi: EfiInfo::default(),
            displays: vec![],
            installations: vec![],
            storage_observation_complete: false,
            findings: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsFamily {
    Windows,
    Linux,
    #[default]
    #[serde(other)]
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum InstallationSource {
    PhysicalDisk(DiskStableId),
    PhysicalPartitions(Vec<PartUuid>),
    Image(String),
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeRef {
    pub partuuid: Option<PartUuid>,
    pub role: String,
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BootProfile {
    Native {
        efi_entry: String,
    },
    ManagedGuest {
        firmware: String,
        needs_boot_bridge: bool,
    },
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    pub size_bytes: u64,
    pub gpt_guid: Option<String>,
    pub partuuids: Vec<PartUuid>,
    /// Empty when content was not read; topology alone cannot validate an overlay.
    pub content_hash: Option<String>,
    pub generation: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityReport {
    pub status: CapabilityStatus,
    pub findings: Vec<Finding>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    pub id: InstallationId,
    pub family: OsFamily,
    pub display_name: String,
    pub source: InstallationSource,
    pub volumes: Vec<VolumeRef>,
    pub boot_profiles: Vec<BootProfile>,
    pub fingerprint: SourceFingerprint,
    pub compatibility: CompatibilityReport,
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoragePolicy {
    InspectReadOnly,
    TrialOverlay { overlay_id: String },
    NativeBackedExclusive,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum LeaseResource {
    BlockDisk(DiskStableId),
    BlockPartition(PartUuid),
    PciIommuGroup(String),
    UsbDevice(UsbStableId),
    DisplayConnector(ConnectorId),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredState {
    Running,
    Stopped,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuProfile {
    pub vcpus: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryProfile {
    pub bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedDomain {
    pub id: DomainId,
    pub installation_id: InstallationId,
    pub backend: String,
    pub cpu: CpuProfile,
    pub memory: MemoryProfile,
    pub devices: Vec<LeaseResource>,
    pub storage_policy: StoragePolicy,
    pub display_profile: String,
    pub desired_state: DesiredState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Satisfied,
    Blocked,
    NeedsVerification,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precondition {
    pub code: String,
    pub status: CheckStatus,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedOperation {
    pub action: String,
    pub description: String,
    pub writes_source: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub schema_version: u32,
    pub id: String,
    pub kind: String,
    pub installation_id: Option<InstallationId>,
    pub preconditions: Vec<Precondition>,
    pub affected_devices: Vec<LeaseResource>,
    pub operations: Vec<PlannedOperation>,
    pub requires_approval: bool,
    pub rollback: Vec<String>,
    pub capability_gaps: Vec<String>,
}
impl Plan {
    pub fn ready(&self) -> bool {
        self.preconditions
            .iter()
            .all(|p| p.status == CheckStatus::Satisfied)
            && self.capability_gaps.is_empty()
    }
    /// Stable content address. Deliberately excludes its own identifier.
    pub fn refresh_id(&mut self) {
        self.id.clear();
        self.id = format!(
            "plan-{}",
            digest(&serde_json::to_vec(self).expect("serializable plan"))
        );
    }
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Versions are rejected before typed deserialization; unknown observations may be
/// retained as Unknown but unknown action enums must never authorize execution.
pub fn check_schema(value: &serde_json::Value) -> Result<(), String> {
    match value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(v) if v == u64::from(SCHEMA_VERSION) => Ok(()),
        other => Err(format!("unsupported or missing schema_version: {other:?}")),
    }
}
