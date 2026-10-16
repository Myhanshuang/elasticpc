use elastic_core::*;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorStatus {
    KnownSupported,
    MissingPrerequisite,
    NeedsVerification,
}
#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub code: String,
    pub status: DoctorStatus,
    pub detail: String,
}
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub source: String,
    pub checks: Vec<DoctorCheck>,
    pub ready_for_physical_execution: bool,
}
fn check(code: &str, status: DoctorStatus, detail: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        code: code.into(),
        status,
        detail: detail.into(),
    }
}
fn capability(code: &str, status: CapabilityStatus, detail: &str) -> DoctorCheck {
    check(
        code,
        match status {
            CapabilityStatus::Supported => DoctorStatus::KnownSupported,
            CapabilityStatus::Unsupported => DoctorStatus::MissingPrerequisite,
            _ => DoctorStatus::NeedsVerification,
        },
        detail,
    )
}
pub fn diagnose(r: &HardwareReport) -> DoctorReport {
    use DoctorStatus::*;
    let mut checks = vec![
        capability(
            "cpu_virtualization",
            r.cpu.virtualization,
            "CPU virtualization flag observation; firmware and KVM require separate checks",
        ),
        capability(
            "kvm",
            r.cpu.kvm,
            "KVM device access; a real KVM API and VM test are still required",
        ),
        capability(
            "iommu",
            r.iommu,
            "Kernel IOMMU group visibility; this does not certify passthrough",
        ),
        check(
            "secure_boot",
            if r.secure_boot == CapabilityStatus::Unknown {
                NeedsVerification
            } else {
                KnownSupported
            },
            format!(
                "Secure Boot observation: {:?}; Guest firmware policy is independent",
                r.secure_boot
            ),
        ),
    ];
    checks.push(match (r.memory.total_bytes, r.memory.available_bytes) {
        (Some(total), Some(available)) if total >= 32 * GIB && available >= 24 * GIB =>
            check("memory", KnownSupported, "Observed RAM can fit the starting 24 GiB guest allocation with 8 GiB Host reserve; recheck before start"),
        (Some(total), Some(available)) => check("memory", NeedsVerification,
            format!("Observed total {total} bytes, available {available} bytes; compute a workload budget before start")),
        _ => check("memory", NeedsVerification, "Total/available physical RAM could not be fully read"),
    });
    checks.push(check("storage_occupancy", if r.storage_observation_complete { KnownSupported } else { NeedsVerification },
        "Snapshot is not an exclusive lock. Recheck mounts, swap, block holders, processes, namespaces and libvirt before execution"));
    checks.push(check(
        "efi_entries",
        if r.efi.available {
            KnownSupported
        } else {
            NeedsVerification
        },
        format!(
            "Observed {} EFI entries, including entries outside BootOrder",
            r.efi.entries.len()
        ),
    ));
    let disk_ids: std::collections::BTreeSet<_> = r
        .disks
        .iter()
        .filter_map(|d| d.stable_id.as_ref())
        .collect();
    let parts: Vec<_> = r.disks.iter().flat_map(|d| &d.partitions).collect();
    let part_ids: std::collections::BTreeSet<_> = parts
        .iter()
        .filter_map(|p| p.partuuid.as_ref().map(|id| id.0.to_ascii_lowercase()))
        .collect();
    let stable = !r.disks.is_empty()
        && disk_ids.len() == r.disks.len()
        && disk_ids.iter().all(|id| !id.0.is_empty())
        && part_ids.len() == parts.len()
        && part_ids.iter().all(|id| !id.is_empty());
    checks.push(check("storage_identity", if stable { KnownSupported } else { NeedsVerification },
        "Physical disk plans require stable disk identity, partition UUIDs and revalidation; /dev/sdX is diagnostic only"));
    let mounted = r.disks.iter().any(|d| {
        !d.mountpoints.is_empty()
            || d.partitions
                .iter()
                .any(|p| !p.mountpoints.is_empty() || p.active_swap)
    });
    checks.push(check("source_exclusivity", if mounted { MissingPrerequisite } else { NeedsVerification },
        if mounted { "Mounted or active swap source exists; writable guest import must reject overlapping storage" } else { "No known mount is not proof that a device has no other writer" }));
    checks.push(check(
        "resume",
        NeedsVerification,
        if r.resume.references.is_empty() {
            "No confirmed hibernation configuration; empty observations do not approve swap removal"
                .into()
        } else {
            format!(
                "Found {} resume/swap configuration references; verify them before layout changes",
                r.resume.references.len()
            )
        },
    ));
    let gpu = r
        .pci_devices
        .iter()
        .filter(|p| p.class.trim_start_matches("0x").starts_with("03"))
        .count();
    checks.push(check("gpu_passthrough", NeedsVerification,
        format!("Observed {gpu} GPU functions. Full IOMMU group, Host rescue display, binding and repeated reset tests remain required")));
    let connected = r
        .displays
        .iter()
        .filter(|d| d.status == "connected")
        .collect::<Vec<_>>();
    checks.push(check("display_topology", if !connected.is_empty() && connected.iter().all(|d| d.pci_bdf.is_some()) { KnownSupported } else { NeedsVerification },
        format!("{} connected outputs. PCI ownership comes from canonical sysfs links; no card-number assumptions", connected.len())));
    for f in &r.findings {
        // Secure Boot disabled is an observation, not a missing prerequisite.
        if f.code == "secure_boot_state" {
            continue;
        }
        checks.push(capability(&f.code, f.status, &f.message));
    }
    checks.push(check("physical_executor", MissingPrerequisite, "P0/P1 intentionally provides no physical apply executor; mock tests do not certify native boot or VFIO"));
    DoctorReport {
        schema_version: SCHEMA_VERSION,
        source: r.source.clone(),
        checks,
        ready_for_physical_execution: false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_observations_never_certify_execution() {
        let d = diagnose(&HardwareReport::default());
        assert!(!d.ready_for_physical_execution);
        assert!(!d
            .checks
            .iter()
            .any(|c| c.status == DoctorStatus::KnownSupported));
    }
    #[test]
    fn flags_alone_do_not_certify_vfio() {
        let r = HardwareReport {
            iommu: CapabilityStatus::Supported,
            ..Default::default()
        };
        let d = diagnose(&r);
        assert_eq!(
            d.checks
                .iter()
                .find(|c| c.code == "gpu_passthrough")
                .unwrap()
                .status,
            DoctorStatus::NeedsVerification
        );
    }
}
