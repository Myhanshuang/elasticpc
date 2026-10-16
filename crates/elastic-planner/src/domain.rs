use crate::{plan_import, plan_resources, MemoryRequest, ResourcePlan, GIB};
use elastic_core::{
    digest, CapabilityStatus, HardwareReport, Installation, LeaseResource, ManagedDomain, OsFamily,
    Plan, StoragePolicy, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestDiskAccess {
    ReadOnly,
    OverlayOnly,
    NativeExclusive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainDiskSpec {
    pub source: LeaseResource,
    pub base_format: String,
    pub base_read_only: bool,
    pub guest_access: GuestDiskAccess,
    pub overlay_id: Option<String>,
    pub controller: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestFirmwareSpec {
    pub firmware: String,
    /// Intended paths only; this pure model never creates files/directories.
    pub private_nvram_path: String,
    pub private_vtpm_state_path: Option<String>,
    pub shares_host_nvram: bool,
}

/// Deliberately not runnable XML. Missing boot artifacts, backend capability
/// discovery and firmware templates must be supplied by a later adapter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainSpec {
    pub schema_version: u32,
    pub domain: ManagedDomain,
    pub hypervisor_uri: String,
    pub machine: String,
    pub architecture: Option<String>,
    pub firmware: GuestFirmwareSpec,
    pub disks: Vec<DomainDiskSpec>,
    pub boot_artifacts_required: Vec<String>,
    pub resources: ResourcePlan,
    pub import_plan: Plan,
    pub executable: bool,
    pub capability_gaps: Vec<String>,
}

pub fn build_domain_spec(
    report: &HardwareReport,
    installation: &Installation,
    domain: &ManagedDomain,
) -> DomainSpec {
    let import_plan = plan_import(report, installation, domain.storage_policy.clone());
    let mut gaps = import_plan.capability_gaps.clone();
    if domain.installation_id != installation.id {
        gaps.push("Domain installation identity does not match the selected installation".into());
    }
    if domain.id.0.is_empty() || domain.cpu.vcpus == 0 {
        gaps.push("Nonempty domain identity and a nonzero vCPU count are required".into());
    }
    match report.cpu.logical_cpus {
        Some(count) if domain.cpu.vcpus <= count => {}
        Some(_) => gaps.push("Requested vCPU count exceeds observed logical CPUs; Host CPU reservation requires policy review".into()),
        None => gaps.push("Logical CPU availability is unknown".into()),
    }
    if report.cpu.architecture != "x86_64" {
        gaps.push("Q35/OVMF profile is only planned for a verified x86_64 host".into());
    }
    if !matches!(domain.backend.as_str(), "libvirt" | "mock") {
        gaps.push("Requested hypervisor backend has no domain-spec adapter".into());
    }
    if report.cpu.virtualization != CapabilityStatus::Supported
        || report.cpu.kvm != CapabilityStatus::Supported
    {
        gaps.push("CPU virtualization and accessible KVM are not both confirmed supported".into());
    }
    let state_key = digest(domain.id.0.as_bytes());
    let firmware = GuestFirmwareSpec {
        firmware: "ovmf".into(),
        private_nvram_path: format!("/var/lib/elasticpc/domains/{state_key}/OVMF_VARS.fd"),
        private_vtpm_state_path: (installation.family == OsFamily::Windows)
            .then(|| format!("/var/lib/elasticpc/domains/{state_key}/vtpm")),
        shares_host_nvram: false,
    };
    let (base_read_only, guest_access, overlay_id) = match &domain.storage_policy {
        StoragePolicy::InspectReadOnly => (true, GuestDiskAccess::ReadOnly, None),
        StoragePolicy::TrialOverlay { overlay_id } => {
            (true, GuestDiskAccess::OverlayOnly, Some(overlay_id.clone()))
        }
        StoragePolicy::NativeBackedExclusive => (false, GuestDiskAccess::NativeExclusive, None),
    };
    let controller = match installation.family {
        OsFamily::Windows => "sata_compatibility_trial",
        OsFamily::Linux => "virtio_after_initramfs_verification",
        OsFamily::Unknown => "undetermined",
    };
    let disks = import_plan
        .affected_devices
        .iter()
        .filter(|source| {
            matches!(
                source,
                LeaseResource::BlockDisk(_) | LeaseResource::BlockPartition(_)
            )
        })
        .map(|source| DomainDiskSpec {
            source: source.clone(),
            base_format: "raw".into(),
            base_read_only,
            guest_access: guest_access.clone(),
            overlay_id: overlay_id.clone(),
            controller: controller.into(),
        })
        .collect();
    let boot_artifacts_required = match installation.family {
        OsFamily::Linux => vec![
            "Separate guest EFI/boot disk with an update-aware Boot Bridge; physical root partition alone cannot boot via OVMF".into(),
            "Guest-specific swap and reversible fstab/resume configuration; never reuse Host active swap".into(),
            "Verified virtio-capable initramfs; direct kernel boot may be a development-only adapter".into(),
        ],
        OsFamily::Windows => vec![
            "Verified OS volume, ESP/BCD relationship and compatible storage driver (OS volume is not inferred from partition position)".into(),
            "Version-matched OVMF code/vars template, persistent vTPM and confirmed encryption recovery material".into(),
        ],
        OsFamily::Unknown => vec!["Identify OS and its boot evidence before generating a boot adapter".into()],
    };
    let resources = plan_resources(
        report.memory.total_bytes,
        report.memory.available_bytes,
        &[MemoryRequest {
            domain_id: domain.id.0.clone(),
            bytes: domain.memory.bytes,
        }],
        8 * GIB,
    );
    gaps.extend(resources.capability_gaps.clone());
    gaps.extend([
        "Resolve and verify installed QEMU/libvirt domain capabilities and matching OVMF templates; model does not assert firmware availability".into(),
        "Boot artifacts and real storage/backend adapters remain unimplemented; this JSON is not executable libvirt XML".into(),
        "Validate the whole multi-domain CPU/RAM budget, including services, before admitting any guest".into(),
    ]);
    for resource in &domain.devices {
        if matches!(
            resource,
            LeaseResource::BlockDisk(_) | LeaseResource::BlockPartition(_)
        ) && !import_plan.affected_devices.contains(resource)
        {
            gaps.push(
                "Additional block devices were requested outside the reviewed import source".into(),
            );
        }
        if let LeaseResource::PciIommuGroup(id) = resource {
            if !report
                .iommu_groups
                .iter()
                .any(|group| &group.id == id && !group.members.is_empty())
            {
                gaps.push(format!(
                    "Requested IOMMU group {id} is absent or incomplete"
                ));
            }
            gaps.push(format!("IOMMU group {id}: revalidate all functions, Host display ownership, reset behavior and exclusive lease before any driver changes"));
        }
        if matches!(
            resource,
            LeaseResource::UsbDevice(_) | LeaseResource::DisplayConnector(_)
        ) {
            gaps.push("USB/display attachment requires a future broker and verified Host rescue input/output path".into());
        }
    }
    gaps.sort();
    gaps.dedup();
    DomainSpec {
        schema_version: SCHEMA_VERSION,
        domain: domain.clone(),
        hypervisor_uri: "qemu:///system".into(),
        machine: "q35".into(),
        architecture: (!report.cpu.architecture.is_empty())
            .then(|| report.cpu.architecture.clone()),
        firmware,
        disks,
        boot_artifacts_required,
        resources,
        import_plan,
        executable: false,
        capability_gaps: gaps,
    }
}
