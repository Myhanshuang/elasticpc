use elastic_core::{BootProfile, CapabilityStatus, InstallationSource, OsFamily};
use elastic_probe::{parse_efi, parse_mountinfo, probe_fixture};
use std::path::Path;

fn fixture(name: &str) -> elastic_core::HardwareReport {
    probe_fixture(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name),
    )
    .unwrap()
}

#[test]
fn acacia_preserves_observed_facts_without_inventing_unknowns() {
    let report = fixture("acacia-redacted");
    assert_eq!(report.source, "fixture");
    assert_eq!(report.cpu.virtualization, CapabilityStatus::Unknown);
    assert_eq!(report.cpu.kvm, CapabilityStatus::Unknown);
    assert_eq!(report.memory.total_bytes, None);
    assert_eq!(report.secure_boot, CapabilityStatus::Unknown);
    let gpu = report
        .pci_devices
        .iter()
        .find(|p| p.bdf == "0000:01:00.0")
        .unwrap();
    let audio = report
        .pci_devices
        .iter()
        .find(|p| p.bdf == "0000:01:00.1")
        .unwrap();
    assert_eq!(gpu.iommu_group, audio.iommu_group);
    let group = report
        .iommu_groups
        .iter()
        .find(|group| Some(&group.id) == gpu.iommu_group.as_ref())
        .unwrap();
    assert_eq!(group.members, ["0000:01:00.0", "0000:01:00.1"]);
    assert_eq!(gpu.reset_methods, ["flr", "bus"]);
    assert_eq!(gpu.sriov_totalvfs, Some(1));
    assert_eq!(gpu.sriov_numvfs, Some(0));
    assert!(gpu.mdev_types.is_empty());
    assert!(gpu.sysfs_path.as_ref().unwrap().contains("0000:00:01.2"));
    assert!(report
        .displays
        .iter()
        .all(|output| output.pci_bdf.is_none()));
    assert!(report
        .efi
        .entries
        .iter()
        .any(|entry| entry.number == "0002"));
    assert!(!report.efi.boot_order.iter().any(|number| number == "0002"));
    assert_eq!(report.efi.boot_current.as_deref(), Some("0006"));
    let root_disk = report
        .disks
        .iter()
        .find(|disk| disk.transport.as_deref() == Some("nvme"))
        .unwrap();
    assert!(root_disk.partitions[0].mountpoints.contains(&"/".into()));
    assert!(root_disk.partitions[2].active_swap);
    assert_eq!(root_disk.unallocated_bytes, None);
    let usb = report
        .disks
        .iter()
        .find(|disk| disk.transport.as_deref() == Some("usb"))
        .unwrap();
    assert!(usb
        .stable_id
        .as_ref()
        .unwrap()
        .0
        .starts_with("/dev/disk/by-id/"));
    assert_eq!(
        usb.partitions[0].part_type.as_deref(),
        Some("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
    );
    let linux = report
        .installations
        .iter()
        .find(|i| i.family == OsFamily::Linux)
        .unwrap();
    assert!(linux.boot_profiles.contains(&BootProfile::Native {
        efi_entry: "0006".into()
    }));
    assert!(report
        .installations
        .iter()
        .all(|installation| installation.family != OsFamily::Windows));
    assert_eq!(
        report
            .installations
            .iter()
            .filter(|i| i.family == OsFamily::Unknown)
            .count(),
        4
    );
    assert!(!report.storage_observation_complete);
}

#[test]
fn synthetic_drm_mapping_uses_canonical_pci_paths() {
    let report = fixture("generic/synthetic-topology");
    assert_eq!(report.displays[0].pci_bdf.as_deref(), Some("0000:01:00.0"));
    assert_eq!(report.displays[1].pci_bdf.as_deref(), Some("0000:65:00.0"));
    assert_eq!(report.cpu.virtualization, CapabilityStatus::Supported);
    assert_eq!(report.cpu.kvm, CapabilityStatus::Experimental);
    assert_eq!(report.memory.total_bytes, Some(32 * elastic_core::GIB));
    assert!(report
        .resume
        .references
        .iter()
        .any(|r| r.contains("resume=UUID=fixture-swap")));
    assert_eq!(report.pci_devices[0].link_width, Some(4));
}

#[test]
fn windows_is_identified_by_mounted_content_not_ntfs_or_partition_order() {
    let report = fixture("generic/windows-on-fourth-partition");
    let windows = report
        .installations
        .iter()
        .find(|i| i.family == OsFamily::Windows)
        .unwrap();
    assert!(windows.id.0.ends_with("222224"));
    assert!(
        matches!(&windows.source, InstallationSource::PhysicalDisk(id) if id.0.contains("by-id"))
    );
    assert!(windows.fingerprint.content_hash.is_none());
    assert!(report
        .installations
        .iter()
        .any(|i| i.id.0.ends_with("222223") && i.family == OsFamily::Unknown));
    assert!(windows
        .compatibility
        .findings
        .iter()
        .any(|finding| finding.code == "windows_encryption_unknown"));
}

#[test]
fn inaccessible_sources_and_headless_hosts_return_degraded_reports() {
    let report = fixture("generic/degraded");
    assert!(report.pci_devices.is_empty());
    assert!(report.displays.is_empty());
    assert!(report.disks.is_empty());
    assert_eq!(report.cpu.kvm, CapabilityStatus::Unknown);
    assert!(!report.efi.available);
    assert!(report
        .findings
        .iter()
        .any(|finding| finding.code == "lsblk_unavailable"));
    let intel = fixture("generic/intel-headless");
    assert_eq!(intel.cpu.virtualization, CapabilityStatus::Supported);
    assert!(intel.displays.is_empty());
    assert_eq!(intel.cpu.kvm, CapabilityStatus::Unknown);
}

#[test]
fn mountinfo_decodes_paths_and_keeps_readonly_evidence() {
    let mounts = parse_mountinfo("31 1 8:1 / /mnt/a\\040b ro,relatime - ext4 /dev/sda1 rw\n32 1 8:2 / /mnt/c rw - ext4 /dev/sda2 ro\n33 1 8:3 / /mnt/\\134040 rw - ext4 /dev/sda3 rw\nmalformed\n");
    assert_eq!(mounts.len(), 3);
    assert_eq!(mounts[0].target, "/mnt/a b");
    assert!(mounts[0].read_only);
    assert!(mounts[1].read_only);
    assert_eq!(mounts[2].target, "/mnt/\\040");
    assert!(!mounts[2].read_only);
}

#[test]
fn efi_keeps_inactive_entries_and_bootnext_outside_order() {
    let efi = parse_efi("BootOrder: 0001\nBootNext: 00AF\nBoot0001* Linux\tHD(1,GPT,redacted,0,0)/File(\\EFI\\Linux\\boot.efi)\nBoot00AF Rescue Device\tPciRoot(0x0)/USB(0,0)\n");
    assert_eq!(efi.entries.len(), 2);
    assert!(!efi.entries[1].active);
    assert_eq!(efi.entries[1].label, "Rescue Device");
    assert_eq!(efi.boot_next.as_deref(), Some("00AF"));
}

#[test]
fn report_has_deterministic_machine_readable_roundtrip() {
    let first = fixture("generic/synthetic-topology");
    let json = serde_json::to_string(&first).unwrap();
    let second: elastic_core::HardwareReport = serde_json::from_str(&json).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        json,
        serde_json::to_string(&fixture("generic/synthetic-topology")).unwrap()
    );
}
