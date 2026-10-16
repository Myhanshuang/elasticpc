use elastic_probe::readiness::{
    parse_kvm_api, parse_libvirt_inventory, parse_readiness, probe_kvm_api_fixture,
    probe_readiness_fixture, Availability, KvmApiObservation, KvmApiStatus, ReadinessInput,
    ResumeResolution,
};
use serde_json::json;
use std::path::Path;

fn fixture() -> ReadinessInput {
    serde_json::from_value(json!({
        "files": {
            "/proc/meminfo": "MemTotal: 28533632 kB\nMemAvailable: 1000000 kB\n",
            "/proc/swaps": "Filename Type Size Used Priority\n/dev/nvme0n1p3 partition 29675516 0 -2\n",
            "/proc/cmdline": "quiet resume=UUID=swap-current resume_offset=42",
            "/etc/default/grub": "# resume=UUID=commented\nGRUB_CMDLINE_LINUX_DEFAULT=\"resume=UUID=stale-private quiet\"\n",
            "/proc/self/mountinfo": "1 0 259:2 / / rw - ext4 /dev/nvme0n1p2 rw\n2 1 8:1 / /var/lib/docker rw - ext4 /dev/sda1 rw\n3 1 0:30 / /var/lib/rancher/k3s/agent/pods/123 rw - tmpfs tmpfs rw\n"
        },
        "commands": { "lsblk": json!({ "blockdevices": [
            {"name":"/dev/nvme0n1","type":"disk","model":"NVMe","wwn":"0xunique","children":[
                {"name":"/dev/nvme0n1p3","type":"part","uuid":"swap-current","partuuid":"swap-part","maj:min":"259:3"}
            ]},
            {"name":"/dev/sda","type":"disk","model":"Samsung Portable SSD T7","serial":"private-serial","wwn":null}
        ]}).to_string()},
        "service_directories": {"/var/lib/docker": true, "/var/lib/rancher/k3s": true},
        "kvm_exists": false,
        "kvm_accessible": false,
        "tools": {"qemu-img":{"status":"missing"},"virsh":{"status":"missing"}}
    })).unwrap()
}

#[test]
fn actual_memory_and_missing_tools_remain_distinct_from_unknown() {
    let report = parse_readiness(&fixture());
    assert_eq!(report.memory_total_bytes, Some(28533632 * 1024));
    assert_eq!(report.tools["qemu-img"].status, Availability::Missing);
    assert_eq!(report.tools["qemu-io"].status, Availability::Unknown);
    assert_eq!(report.kvm.node_visible, Some(false));
    assert!(!report.kvm.api_verified);
    assert!(!report.libvirt.live_connection_attempted);
    assert!(!report.libvirt.resource_occupancy_verified);
}

#[test]
fn serial_alone_is_not_a_stable_t7_identity() {
    let report = parse_readiness(&fixture());
    let t7 = report.disks.iter().find(|disk| disk.t7_candidate).unwrap();
    assert!(t7.serial_observed);
    assert!(!t7.unique_stable_identity_observed);
    assert!(report.disks[0].unique_stable_identity_observed);
}

#[test]
fn recorded_udev_alias_is_separate_from_resolved_execution_identity() {
    let mut input = fixture();
    input.udev_records.insert("/dev/sda".into(),
        "S:disk/by-id/usb-private-serial\nS:disk/by-id/usb-private-serial\nS:disk/by-id/usb-private-serial-part1\nS:disk/by-path/pci-123\nS:disk/by-id/../../bad\n".into());
    let report = parse_readiness(&input);
    let t7 = report.disks.iter().find(|disk| disk.t7_candidate).unwrap();
    assert_eq!(
        t7.udev_recorded_by_id,
        ["/dev/disk/by-id/usb-private-serial"]
    );
    assert!(t7.by_id.is_empty());
    assert!(!t7.unique_stable_identity_observed);
    assert!(t7.note.contains("namespace"));
    assert!(!serde_json::to_string(&report.redacted())
        .unwrap()
        .contains("private-serial"));
}

#[test]
fn registered_kernel_without_visible_kvm_node_does_not_establish_api_access() {
    let mut input = fixture();
    input.kvm_kernel_registered = Some(true);
    let report = parse_readiness(&input);
    assert_eq!(report.kvm.kernel_registered, Some(true));
    assert_eq!(report.kvm.node_visible, Some(false));
    assert!(!report.kvm.api_verified);
}

#[test]
fn fixture_api_versions_never_claim_live_verification_or_issue_syscalls() {
    for (version, status) in [
        (12, KvmApiStatus::Available),
        (11, KvmApiStatus::ApiMismatch),
        (13, KvmApiStatus::ApiMismatch),
    ] {
        let mut input = fixture();
        input.kvm_api = Some(KvmApiObservation::ApiVersion { version });
        let report = parse_kvm_api(&input);
        assert_eq!(report.status, status);
        assert_eq!(report.api_version, Some(version));
        assert_eq!(report.source, "fixture");
        assert!(!report.node_opened);
        assert!(!report.ioctl_attempted);
        assert!(!report.api_verified);
    }
}

#[test]
fn fixture_kvm_failures_remain_distinct_and_missing_observation_never_falls_back() {
    for (observation, status) in [
        (KvmApiObservation::Missing, KvmApiStatus::Missing),
        (KvmApiObservation::AccessDenied, KvmApiStatus::AccessDenied),
        (KvmApiObservation::InvalidNode, KvmApiStatus::InvalidNode),
        (
            KvmApiObservation::IoctlFailed {
                errno: libc::ENOTTY,
            },
            KvmApiStatus::IoctlFailed,
        ),
    ] {
        let mut input = fixture();
        input.kvm_api = Some(observation);
        let report = parse_kvm_api(&input);
        assert_eq!(report.status, status);
        assert!(!report.node_opened);
        assert!(!report.ioctl_attempted);
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/acacia-redacted");
    let report = probe_kvm_api_fixture(&path).unwrap();
    assert_eq!(report.status, KvmApiStatus::NotObserved);
    assert_eq!(report.source, "fixture");
    assert!(!report.node_opened);
    assert!(probe_kvm_api_fixture(&path.join("missing.json")).is_err());
}

#[test]
fn duplicate_wwn_is_not_unique_but_distinct_by_id_is_evidence() {
    let mut input = fixture();
    input.commands.insert(
        "lsblk".into(),
        json!({"blockdevices":[
            {"name":"/dev/sda","type":"disk","wwn":"duplicate"},
            {"name":"/dev/sdb","type":"disk","wwn":"duplicate"}
        ]})
        .to_string(),
    );
    assert!(parse_readiness(&input)
        .disks
        .iter()
        .all(|disk| !disk.unique_stable_identity_observed));
    input
        .disk_aliases
        .insert("/dev/sda".into(), vec!["usb-unique".into()]);
    let report = parse_readiness(&input);
    assert!(report.disks[0].unique_stable_identity_observed);
    assert!(!report.disks[1].unique_stable_identity_observed);
}

#[test]
fn resolves_current_resume_and_exposes_stale_grub_reference() {
    let report = parse_readiness(&fixture());
    assert_eq!(report.resume_references.len(), 2);
    assert_eq!(
        report.resume_references[0].resolution,
        ResumeResolution::ActiveSwap
    );
    assert_eq!(
        report.resume_references[1].resolution,
        ResumeResolution::Unresolved
    );
    assert_eq!(report.resume_references[1].target, "UUID=stale-private");
    assert!(report.resume_references[1].note.contains("not proof"));
}

#[test]
fn missing_or_malformed_swap_observation_is_not_inactive_swap() {
    let mut input = fixture();
    input.files.remove("/proc/swaps");
    assert_eq!(
        parse_readiness(&input).resume_references[0].resolution,
        ResumeResolution::Unknown
    );
    input
        .files
        .insert("/proc/swaps".into(), "permission denied".into());
    assert_eq!(
        parse_readiness(&input).resume_references[0].resolution,
        ResumeResolution::Unknown
    );
    input.files.insert(
        "/proc/swaps".into(),
        "Filename Type Size Used Priority\n".into(),
    );
    assert_eq!(
        parse_readiness(&input).resume_references[0].resolution,
        ResumeResolution::ObservedNotActiveSwap
    );
}

#[test]
fn duplicate_resume_uuid_is_ambiguous_and_partuuid_alias_can_resolve() {
    let mut input = fixture();
    input.commands.insert(
        "lsblk".into(),
        json!({"blockdevices":[
            {"name":"/dev/sda1","type":"part","uuid":"swap-current","partuuid":"one"},
            {"name":"/dev/sdb1","type":"part","uuid":"swap-current","partuuid":"two"}
        ]})
        .to_string(),
    );
    assert_eq!(
        parse_readiness(&input).resume_references[0].resolution,
        ResumeResolution::Ambiguous
    );
    input
        .files
        .insert("/proc/cmdline".into(), "resume=PARTUUID=one".into());
    input.files.insert(
        "/proc/swaps".into(),
        "Filename Type Size Used Priority\n/dev/disk/by-partuuid/one partition 100 0 -2\n".into(),
    );
    assert_eq!(
        parse_readiness(&input).resume_references[0].resolution,
        ResumeResolution::ActiveSwap
    );
}

#[test]
fn malformed_topology_and_memory_are_unknown() {
    let mut input = fixture();
    input.commands.insert("lsblk".into(), "{broken".into());
    input.files.insert(
        "/proc/meminfo".into(),
        "MemTotal: 123 bananas\nMemAvailable: 18446744073709551615 kB".into(),
    );
    let report = parse_readiness(&input);
    assert!(!report.storage_topology_observed);
    assert_eq!(report.memory_total_bytes, None);
    assert_eq!(report.memory_available_bytes, None);
    assert_eq!(
        report.resume_references[0].resolution,
        ResumeResolution::Unknown
    );
    input.commands.insert(
        "lsblk".into(),
        "{\"blockdevices\":[{\"type\":\"disk\"}]}".into(),
    );
    let report = parse_readiness(&input);
    assert!(!report.storage_topology_observed);
    assert_eq!(
        report.resume_references[0].resolution,
        ResumeResolution::Unknown
    );
}

#[test]
fn service_candidates_use_longest_mount_boundary_without_reading_data() {
    let report = parse_readiness(&fixture());
    let docker = report
        .service_storage
        .iter()
        .find(|service| service.service == "docker")
        .unwrap();
    assert_eq!(docker.backing_mount.as_deref(), Some("/var/lib/docker"));
    assert_eq!(docker.backing_source.as_deref(), Some("/dev/sda1"));
    let k3s = report
        .service_storage
        .iter()
        .find(|service| service.service == "k3s")
        .unwrap();
    assert_eq!(k3s.backing_mount.as_deref(), Some("/"));
    assert_eq!(k3s.observed_mounts_under_path, 1);
}

#[test]
fn offline_domain_inventory_never_claims_device_occupancy() {
    let uuid = "12345678-1234-1234-1234-1234567890ab";
    let inventory = parse_libvirt_inventory(&format!("{uuid}\n{uuid}\n")).unwrap();
    assert_eq!(inventory.domain_uuids, [uuid]);
    assert!(!inventory.resource_occupancy_verified);
    assert!(!inventory.live_connection_attempted);
    assert!(parse_libvirt_inventory(" Id Name State\n1 windows running").is_err());
    assert!(
        !parse_libvirt_inventory("")
            .unwrap()
            .resource_occupancy_verified
    );
}

#[test]
fn redaction_covers_stale_non_uuid_identity_and_service_paths() {
    let mut input = fixture();
    input
        .disk_aliases
        .insert("/dev/sda".into(), vec!["usb-private-serial".into()]);
    input.files.insert(
        "/proc/self/mountinfo".into(),
        "1 0 8:1 / / rw - ext4 /dev/mapper/private-volume rw\n".into(),
    );
    let value = serde_json::to_string(&parse_readiness(&input).redacted()).unwrap();
    for private in [
        "private-serial",
        "stale-private",
        "swap-current",
        "0xunique",
        "private-volume",
    ] {
        assert!(!value.contains(private), "leaked {private}");
    }
    assert!(value.contains("\"redacted\":true"));
}

#[test]
fn fixture_parser_uses_existing_hardware_snapshot_without_live_fallback() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/acacia-redacted");
    let report = probe_readiness_fixture(&path).unwrap();
    assert_eq!(report.source, "fixture");
    assert!(report.memory_total_bytes.is_none());
    assert!(report.storage_topology_observed);
    assert!(report.disks.iter().any(|disk| disk.t7_candidate));
    assert_eq!(report.tools["qemu-img"].status, Availability::Unknown);
    assert!(!report.libvirt.live_connection_attempted);
    // An absent fixture is an error; it must never silently probe the machine.
    assert!(probe_readiness_fixture(&path.join("missing-fixture.json")).is_err());
}
