use elastic_core::*;
use elastic_planner::{plan_native_return, plan_windows_gpu};

fn installation() -> Installation {
    Installation {
        id: InstallationId("windows-test".into()),
        family: OsFamily::Windows,
        display_name: "fixture".into(),
        source: InstallationSource::Unknown,
        volumes: vec![],
        boot_profiles: vec![BootProfile::Native {
            efi_entry: "0002".into(),
        }],
        fingerprint: SourceFingerprint::default(),
        compatibility: CompatibilityReport::default(),
        evidence: vec![],
    }
}
fn hardware() -> HardwareReport {
    let mut report = HardwareReport {
        iommu: CapabilityStatus::Supported,
        ..Default::default()
    };
    for (bdf, class, driver, group) in [
        ("0000:01:00.0", "030000", "nvidia", Some("15")),
        ("0000:01:00.1", "040300", "snd_hda_intel", Some("15")),
        ("0000:65:00.0", "030000", "amdgpu", None),
    ] {
        report.pci_devices.push(PciDevice {
            bdf: bdf.into(),
            class: class.into(),
            driver: Some(driver.into()),
            iommu_group: group.map(str::to_owned),
            ..Default::default()
        });
    }
    report.iommu_groups.push(IommuGroup {
        id: "15".into(),
        members: vec!["0000:01:00.0".into(), "0000:01:00.1".into()],
    });
    report.displays.push(DisplayConnector {
        id: ConnectorId("eDP-1".into()),
        pci_bdf: Some("0000:65:00.0".into()),
        status: "connected".into(),
        ..Default::default()
    });
    report.efi.available = true;
    report.efi.entries.push(EfiEntry {
        number: "0002".into(),
        active: true,
        device_path: "fixture-path".into(),
        ..Default::default()
    });
    report
}
fn status(plan: &Plan, code: &str) -> CheckStatus {
    plan.preconditions
        .iter()
        .find(|c| c.code == code)
        .unwrap()
        .status
}
#[test]
fn complete_group_retains_drivers_and_never_claims_ready() {
    let plan = plan_windows_gpu(&hardware(), &installation(), "0000:01:00.0");
    assert_eq!(status(&plan, "complete_group"), CheckStatus::Satisfied);
    assert_eq!(
        status(&plan, "host_rescue_display"),
        CheckStatus::NeedsVerification
    );
    assert_eq!(
        status(&plan, "reset_reliability"),
        CheckStatus::NeedsVerification
    );
    assert!(plan
        .operations
        .iter()
        .any(|o| o.action == "preserve_driver_packages"));
    assert_eq!(
        plan.affected_devices,
        vec![LeaseResource::PciIommuGroup("15".into())]
    );
}
#[test]
fn missing_duplicate_or_unlisted_audio_denies_group() {
    for mutation in 0..4 {
        let mut report = hardware();
        match mutation {
            0 => {
                report.pci_devices.remove(1);
            }
            1 => {
                report.iommu_groups[0].members.pop();
            }
            2 => {
                report.iommu_groups[0].members.push("0000:01:00.1".into());
            }
            _ => {
                report.iommu_groups.push(report.iommu_groups[0].clone());
            }
        }
        assert_eq!(
            status(
                &plan_windows_gpu(&report, &installation(), "0000:01:00.0"),
                "complete_group"
            ),
            CheckStatus::Blocked
        );
    }
}
#[test]
fn sole_display_or_unknown_connector_denies_handoff() {
    for owner in [
        None,
        Some("0000:01:00.0".into()),
        Some("0000:99:00.0".into()),
    ] {
        let mut report = hardware();
        report.displays[0].pci_bdf = owner;
        assert_eq!(
            status(
                &plan_windows_gpu(&report, &installation(), "0000:01:00.0"),
                "host_rescue_display"
            ),
            CheckStatus::Blocked
        );
    }
}
#[test]
fn foreign_functions_and_unknown_original_driver_are_blocked() {
    let mut report = hardware();
    report.pci_devices[1].class = "010802".into();
    report.pci_devices[0].driver = Some("vfio-pci".into());
    let plan = plan_windows_gpu(&report, &installation(), "0000:01:00.0");
    assert_eq!(status(&plan, "group_scope"), CheckStatus::Blocked);
    assert_eq!(
        status(&plan, "driver_restore_0000:01:00.0"),
        CheckStatus::Blocked
    );
}
#[test]
fn unknown_os_iommu_and_malformed_bdf_are_blocked() {
    let mut report = hardware();
    report.iommu = CapabilityStatus::Unknown;
    let mut inst = installation();
    inst.family = OsFamily::Unknown;
    let plan = plan_windows_gpu(&report, &inst, "0000:01:ff.8");
    for code in [
        "windows_installation",
        "iommu",
        "gpu_identity",
        "complete_group",
    ] {
        assert_eq!(status(&plan, code), CheckStatus::Blocked);
    }
}
#[test]
fn native_invalidation_precedes_bootnext_and_does_not_change_bootorder() {
    let report = hardware();
    let before = report.clone();
    let plan = plan_native_return(&report, &installation());
    assert_eq!(report, before);
    assert_eq!(status(&plan, "native_entry"), CheckStatus::Satisfied);
    let actions: Vec<_> = plan.operations.iter().map(|o| o.action.as_str()).collect();
    assert!(
        actions
            .iter()
            .position(|a| *a == "invalidate_trial_generation")
            < actions.iter().position(|a| *a == "one_time_native_boot")
    );
    assert_eq!(
        status(&plan, "all_io_stopped"),
        CheckStatus::NeedsVerification
    );
}
#[test]
fn native_conflicting_bootnext_missing_or_duplicate_entry_blocks() {
    let mut report = hardware();
    report.efi.boot_next = Some("0008".into());
    assert_eq!(
        status(
            &plan_native_return(&report, &installation()),
            "bootnext_conflict"
        ),
        CheckStatus::Blocked
    );
    report.efi.entries.push(report.efi.entries[0].clone());
    assert_eq!(
        status(
            &plan_native_return(&report, &installation()),
            "native_entry"
        ),
        CheckStatus::Blocked
    );
    report.efi.entries.clear();
    let plan = plan_native_return(&report, &installation());
    assert!(!plan
        .operations
        .iter()
        .any(|o| o.action == "one_time_native_boot"));
}
