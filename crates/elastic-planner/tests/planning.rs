use elastic_core::{
    CapabilityStatus, CheckStatus, CompatibilityReport, CpuInfo, CpuProfile, DesiredState, Disk,
    DomainId, HardwareReport, Installation, InstallationId, InstallationSource, ManagedDomain,
    MemoryInfo, MemoryProfile, Mount, OsFamily, PartUuid, Partition, SourceFingerprint,
    StoragePolicy,
};
use elastic_planner::{
    build_domain_spec, plan_import, plan_layout, plan_resources, validate_overlay_fingerprint,
    GuestDiskAccess, MemoryRequest, GIB,
};

fn desktop_memory() -> Vec<MemoryRequest> {
    vec![
        MemoryRequest {
            domain_id: "windows".to_owned(),
            bytes: 16 * GIB,
        },
        MemoryRequest {
            domain_id: "manjaro".to_owned(),
            bytes: 6 * GIB,
        },
        MemoryRequest {
            domain_id: "services".to_owned(),
            bytes: 2 * GIB,
        },
    ]
}

#[test]
fn memory_admission_preserves_host_reserve_and_accounts_for_actual_availability() {
    let requests = desktop_memory();
    assert!(plan_resources(Some(32 * GIB), Some(32 * GIB), &requests, 8 * GIB).admitted);
    let low_available = plan_resources(Some(32 * GIB), Some(25 * GIB), &requests, 8 * GIB);
    assert!(!low_available.admitted);
    assert!(low_available
        .capability_gaps
        .iter()
        .any(|gap| gap.contains("MemAvailable")));
    assert!(!plan_resources(Some(32 * GIB), None, &requests, 8 * GIB).admitted);
    assert!(!plan_resources(Some(32 * GIB), Some(32 * GIB), &requests, 0).admitted);
}

#[test]
fn memory_admission_rejects_overflow_duplicate_identity_and_overcommit() {
    let overflow = [MemoryRequest {
        domain_id: "overflow".to_owned(),
        bytes: u64::MAX,
    }];
    assert!(!plan_resources(Some(u64::MAX), Some(u64::MAX), &overflow, 1).admitted);
    let request = MemoryRequest {
        domain_id: "vm".to_owned(),
        bytes: GIB,
    };
    let duplicate = [request.clone(), request];
    assert!(!plan_resources(Some(32 * GIB), Some(32 * GIB), &duplicate, GIB).admitted);
    let full = [MemoryRequest {
        domain_id: "vm".to_owned(),
        bytes: 32 * GIB,
    }];
    assert!(!plan_resources(Some(32 * GIB), Some(32 * GIB), &full, 8 * GIB).admitted);
}

fn storage_fixture() -> (HardwareReport, Installation) {
    let partuuid = PartUuid::from("test-os-partition");
    let fingerprint = SourceFingerprint {
        size_bytes: 100 * GIB,
        gpt_guid: Some("test-gpt-guid".into()),
        partuuids: vec![partuuid.clone()],
        content_hash: Some("a".repeat(64)),
        generation: Some("verified-native-session-1".into()),
    };
    let installation = Installation {
        id: InstallationId::from("verified-linux"),
        family: OsFamily::Linux,
        display_name: "Test installation".into(),
        source: InstallationSource::PhysicalPartitions(vec![partuuid.clone()]),
        volumes: vec![],
        boot_profiles: vec![],
        fingerprint,
        compatibility: CompatibilityReport::default(),
        evidence: vec!["fixture os-release + fstab".into()],
    };
    let report = HardwareReport {
        source: "fixture".into(),
        cpu: CpuInfo {
            architecture: "x86_64".into(),
            logical_cpus: Some(16),
            virtualization: CapabilityStatus::Supported,
            kvm: CapabilityStatus::Supported,
        },
        memory: MemoryInfo {
            total_bytes: Some(32 * GIB),
            available_bytes: Some(32 * GIB),
        },
        disks: vec![Disk {
            path: "/dev/renamable".into(),
            stable_id: Some("test-disk-stable".into()),
            size_bytes: 128 * GIB,
            gpt_guid: Some("test-gpt-guid".into()),
            partition_table: Some("gpt".into()),
            transport: Some("nvme".into()),
            partitions: vec![Partition {
                path: "/dev/renamable1".into(),
                partuuid: Some(partuuid),
                size_bytes: 100 * GIB,
                filesystem: Some("ext4".into()),
                major_minor: Some("259:1".into()),
                ..Partition::default()
            }],
            ..Disk::default()
        }],
        storage_observation_complete: true,
        ..HardwareReport::default()
    };
    (report, installation)
}

fn trial() -> StoragePolicy {
    StoragePolicy::TrialOverlay {
        overlay_id: "test-overlay".into(),
    }
}

#[test]
fn mounted_partition_blocks_both_trial_and_native_even_with_alias_mount_source() {
    let (mut report, installation) = storage_fixture();
    report.mounts.push(Mount {
        target: "/".into(),
        source: "/dev/root".into(),
        major_minor: Some("259:1".into()),
        read_only: false,
    });
    for policy in [trial(), StoragePolicy::NativeBackedExclusive] {
        let plan = plan_import(&report, &installation, policy);
        assert!(plan
            .preconditions
            .iter()
            .any(|check| check.code == "source_unmounted" && check.status == CheckStatus::Blocked));
        assert!(!plan.ready());
    }
    let inspect = plan_import(&report, &installation, StoragePolicy::InspectReadOnly);
    assert!(inspect
        .operations
        .iter()
        .all(|operation| !operation.writes_source));
    assert!(!inspect.requires_approval);
}

#[test]
fn unknown_occupancy_and_active_swap_deny_guest_writes() {
    let (mut report, installation) = storage_fixture();
    report.storage_observation_complete = false;
    let unknown = plan_import(&report, &installation, trial());
    assert!(unknown
        .preconditions
        .iter()
        .any(|check| check.code == "complete_occupancy_observation"
            && check.status == CheckStatus::Blocked));
    report.storage_observation_complete = true;
    report.disks[0].partitions[0].active_swap = true;
    let swap = plan_import(&report, &installation, trial());
    assert!(swap
        .preconditions
        .iter()
        .any(|check| check.code == "source_unmounted" && check.status == CheckStatus::Blocked));
}

#[test]
fn whole_host_disk_is_never_admitted_as_guest_writable_storage() {
    let (mut report, mut installation) = storage_fixture();
    report.disks[0].partitions[0].mountpoints.push("/".into());
    installation.source =
        InstallationSource::PhysicalDisk(report.disks[0].stable_id.clone().unwrap());
    let plan = plan_import(&report, &installation, StoragePolicy::NativeBackedExclusive);
    assert!(plan
        .preconditions
        .iter()
        .any(|check| check.code == "host_disk_isolation" && check.status == CheckStatus::Blocked));
}

#[test]
fn plan_identity_uses_stable_ids_and_changes_when_fingerprint_changes() {
    let (mut report, mut installation) = storage_fixture();
    let plan = plan_import(&report, &installation, trial());
    report.disks[0].path = "/dev/different".into();
    report.disks[0].partitions[0].path = "/dev/different1".into();
    assert_eq!(plan, plan_import(&report, &installation, trial()));
    installation.fingerprint.content_hash = Some("b".repeat(64));
    assert_ne!(plan.id, plan_import(&report, &installation, trial()).id);
    let json = serde_json::to_string(&plan).unwrap();
    assert!(!json.contains("/dev/renamable"));
    assert_eq!(plan.schema_version, 1);
    for field in [
        "preconditions",
        "affected_devices",
        "operations",
        "requires_approval",
        "rollback",
        "capability_gaps",
    ] {
        assert!(json.contains(field));
    }
}

#[test]
fn parent_disk_mount_blocks_child_but_unrelated_sibling_mount_does_not() {
    let (mut report, installation) = storage_fixture();
    report.mounts.push(Mount {
        target: "/whole-disk-mount".into(),
        source: report.disks[0].stable_id.as_ref().unwrap().0.clone(),
        major_minor: None,
        read_only: true,
    });
    let blocked = plan_import(&report, &installation, trial());
    assert!(blocked
        .preconditions
        .iter()
        .any(|check| check.code == "source_unmounted" && check.status == CheckStatus::Blocked));
    report.mounts.clear();
    report.disks[0].partitions.push(Partition {
        path: "/dev/renamable2".into(),
        partuuid: Some("host-root".into()),
        mountpoints: vec!["/".into()],
        major_minor: Some("259:2".into()),
        ..Partition::default()
    });
    let sibling = plan_import(&report, &installation, trial());
    assert!(!sibling
        .preconditions
        .iter()
        .any(|check| check.code == "source_unmounted"));
}

#[test]
fn ambiguous_or_transient_identity_is_rejected() {
    let (mut report, mut installation) = storage_fixture();
    installation.source = InstallationSource::PhysicalDisk("/dev/sda".into());
    report.disks[0].stable_id = Some("/dev/sda".into());
    let transient = plan_import(&report, &installation, trial());
    assert!(transient
        .preconditions
        .iter()
        .any(|check| check.code == "unique_disk" && check.status == CheckStatus::Blocked));
    installation.source = InstallationSource::PhysicalPartitions(vec!["test-os-partition".into()]);
    report.disks.push(report.disks[0].clone());
    let ambiguous = plan_import(&report, &installation, trial());
    assert!(ambiguous
        .preconditions
        .iter()
        .any(|check| check.code == "unique_partition" && check.status == CheckStatus::Blocked));
}

#[test]
fn filesystem_uuid_mount_aliases_block_partition_and_whole_host_disk() {
    for source in ["UUID=test-filesystem", "/dev/disk/by-uuid/test-filesystem"] {
        let (mut report, mut installation) = storage_fixture();
        report.disks[0].partitions[0].uuid = Some("test-filesystem".into());
        report.mounts.push(Mount {
            target: "/".into(),
            source: source.into(),
            major_minor: None,
            read_only: false,
        });
        let child = plan_import(&report, &installation, trial());
        assert!(child
            .preconditions
            .iter()
            .any(|check| check.code == "source_unmounted" && check.status == CheckStatus::Blocked));
        installation.source =
            InstallationSource::PhysicalDisk(report.disks[0].stable_id.clone().unwrap());
        let whole = plan_import(&report, &installation, StoragePolicy::NativeBackedExclusive);
        assert!(whole.preconditions.iter().any(
            |check| check.code == "host_disk_isolation" && check.status == CheckStatus::Blocked
        ));
    }
}

#[test]
fn layout_reports_ambiguous_disk_and_partition_targets() {
    let (mut report, _) = storage_fixture();
    report.disks.push(report.disks[0].clone());
    let layout = plan_layout(&report, 20 * GIB);
    assert!(layout
        .preconditions
        .iter()
        .any(|check| check.code == "unique_disk" && check.status == CheckStatus::Blocked));
    assert!(layout
        .preconditions
        .iter()
        .any(|check| check.code == "unique_partition" && check.status == CheckStatus::Blocked));
    assert!(!layout.ready());
}

#[test]
fn unknown_os_is_not_given_a_boot_adapter_and_trial_never_writes_base() {
    let (report, mut installation) = storage_fixture();
    installation.family = OsFamily::Unknown;
    let plan = plan_import(&report, &installation, trial());
    assert!(
        plan.preconditions
            .iter()
            .any(|check| check.code == "installation_identity"
                && check.status == CheckStatus::Blocked)
    );
    assert!(plan
        .operations
        .iter()
        .all(|operation| !operation.writes_source));
    assert!(!plan
        .operations
        .iter()
        .any(|operation| operation.action == "windows_boot_compatibility"));
    assert!(!plan
        .operations
        .iter()
        .any(|operation| operation.action == "linux_boot_bridge"));
}

#[test]
fn overlay_resume_rejects_insufficient_hash_stale_content_and_native_generation() {
    let (_, installation) = storage_fixture();
    let original = installation.fingerprint;
    assert!(validate_overlay_fingerprint(&original, &original).is_ok());
    let mut changed = original.clone();
    changed.content_hash = Some("b".repeat(64));
    assert!(validate_overlay_fingerprint(&original, &changed).is_err());
    changed = original.clone();
    changed.generation = Some("native-session-2".into());
    assert!(validate_overlay_fingerprint(&original, &changed).is_err());
    changed = original.clone();
    changed.content_hash = None;
    assert!(validate_overlay_fingerprint(&changed, &changed).is_err());
    changed = original.clone();
    changed.content_hash = Some("topology-is-not-content".into());
    assert!(validate_overlay_fingerprint(&changed, &changed).is_err());
}

#[test]
fn layout_preserves_swap_resume_gate_and_requires_offline_ext4_measurement() {
    let (mut report, _) = storage_fixture();
    report.disks[0].unallocated_bytes = Some(0);
    report.disks[0].partitions[0].mountpoints.push("/".into());
    report.disks[0].partitions.push(Partition {
        path: "/dev/renamable2".into(),
        partuuid: Some("test-swap".into()),
        size_bytes: 28 * GIB,
        filesystem: Some("swap".into()),
        active_swap: true,
        ..Partition::default()
    });
    let plan = plan_layout(&report, 20 * GIB);
    assert!(plan.requires_approval);
    assert!(!plan.ready());
    assert!(plan
        .preconditions
        .iter()
        .any(|check| check.code == "alternative_a_resume"
            && check.status == CheckStatus::NeedsVerification));
    assert!(plan
        .preconditions
        .iter()
        .any(|check| check.code == "alternative_b_offline_ext4"
            && check.status == CheckStatus::Blocked));
    assert!(plan
        .operations
        .iter()
        .any(|op| op.action == "alternative_a_repurpose_swap"));
    assert!(plan
        .operations
        .iter()
        .any(|op| op.action == "alternative_b_offline_shrink"));
    assert!(!plan
        .operations
        .iter()
        .any(|op| op.action == "alternative_existing_unallocated"));
    assert_ne!(plan.id, plan_layout(&report, 24 * GIB).id);
}

#[test]
fn domain_spec_has_private_state_stable_sources_and_explicit_unbootable_gaps() {
    let (report, installation) = storage_fixture();
    let domain = ManagedDomain {
        id: DomainId::from("test-domain"),
        installation_id: installation.id.clone(),
        backend: "libvirt".into(),
        cpu: CpuProfile { vcpus: 4 },
        memory: MemoryProfile { bytes: 6 * GIB },
        devices: vec![],
        storage_policy: trial(),
        display_profile: "virtio_2d".into(),
        desired_state: DesiredState::Stopped,
    };
    let first = build_domain_spec(&report, &installation, &domain);
    assert!(!first.executable);
    assert!(!first.firmware.shares_host_nvram);
    assert!(first
        .disks
        .iter()
        .all(|disk| disk.base_read_only && disk.guest_access == GuestDiskAccess::OverlayOnly));
    assert!(first
        .boot_artifacts_required
        .iter()
        .any(|gap| gap.contains("Boot Bridge")));
    assert_eq!(first, build_domain_spec(&report, &installation, &domain));
    let mut other_domain = domain.clone();
    other_domain.id = DomainId::from("second-domain");
    let mut windows = installation;
    windows.family = OsFamily::Windows;
    let second = build_domain_spec(&report, &windows, &other_domain);
    assert_ne!(
        first.firmware.private_nvram_path,
        second.firmware.private_nvram_path
    );
    assert!(second.firmware.private_vtpm_state_path.is_some());
    assert!(second
        .disks
        .iter()
        .all(|disk| disk.controller == "sata_compatibility_trial"));
}

#[test]
fn resource_planning_is_deterministic_for_request_order() {
    let mut requests = desktop_memory();
    let original = plan_resources(Some(32 * GIB), Some(32 * GIB), &requests, 8 * GIB);
    requests.reverse();
    assert_eq!(
        original,
        plan_resources(Some(32 * GIB), Some(32 * GIB), &requests, 8 * GIB)
    );
}
