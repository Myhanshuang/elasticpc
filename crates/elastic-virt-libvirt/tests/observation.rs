use elastic_virt_libvirt::{
    parse_domain_xml, read_test_driver_inventory, validate_test_uri, AdapterError, DomainState,
    ResourceKind, XmlScope, TEST_URI,
};

const UUID: &str = "6695eb01-f6a4-4304-8e72-1f8ce2876365";

fn xml(devices: &str) -> String {
    format!("<domain type='test'><name>fixture</name><uuid>{UUID}</uuid><devices>{devices}</devices></domain>")
}

#[test]
fn only_exact_test_uri_is_accepted_before_native_code() {
    assert!(validate_test_uri(TEST_URI).is_ok());
    for uri in [
        "",
        "default",
        "qemu:///system",
        "qemu:///session",
        "test:///tmp/custom.xml",
        "test:///default?x=1",
        "test://remote/default",
        " test:///default",
        "TEST:///default",
        "test:///default\0",
    ] {
        assert!(
            matches!(validate_test_uri(uri), Err(AdapterError::UnsafeUri)),
            "{uri:?}"
        );
        assert!(
            matches!(
                read_test_driver_inventory(uri),
                Err(AdapterError::UnsafeUri)
            ),
            "{uri:?}"
        );
    }
}

#[test]
fn active_xml_collects_disk_backing_firmware_and_pci_references() {
    let xml = format!("<domain><uuid>{UUID}</uuid><os><loader readonly='yes'>/firmware/CODE.fd</loader><nvram>/private/VARS.fd</nvram></os><devices>
        <disk type='file' device='disk'><driver type='qcow2'/><source file='/private/overlay.qcow2'/><target dev='vda'/>
          <backingStore type='block'><format type='raw'/><source dev='/dev/disk/by-id/example'/><backingStore/></backingStore>
        </disk>
        <hostdev mode='subsystem' type='pci'><source><address domain='0x0000' bus='0x01' slot='0x00' function='0x1'/></source><address type='pci' domain='0x0000' bus='0x00' slot='0x02' function='0x0'/></hostdev>
        </devices></domain>");
    let result = parse_domain_xml(UUID, true, DomainState::Running, Some(&xml));
    assert!(
        result.resource_references_complete,
        "{:?}",
        result.unknown_reasons
    );
    assert_eq!(result.xml_scope, XmlScope::Live);
    assert_eq!(result.resources.len(), 5);
    assert_eq!(result.resources[3].kind, ResourceKind::BlockDevice);
    assert_eq!(result.resources[4].location, "0000:01:00.1");
}

#[test]
fn inactive_xml_is_never_labeled_live() {
    let result = parse_domain_xml(UUID, false, DomainState::Shutoff, Some(&xml("")));
    assert!(result.resource_references_complete);
    assert_eq!(result.xml_scope, XmlScope::Inactive);
}

#[test]
fn paused_and_crashed_domains_retain_their_resource_claims() {
    let xml =
        xml("<disk type='block'><source dev='/dev/example'/><target dev='vda'/><readonly/></disk>");
    for state in [
        DomainState::Paused,
        DomainState::Crashed,
        DomainState::Shutdown,
    ] {
        let result = parse_domain_xml(UUID, true, state, Some(&xml));
        assert_eq!(result.resources.len(), 1);
        assert!(result.resources[0].read_only_declared);
        assert!(result.active);
    }
}

#[test]
fn unsupported_sources_keep_observed_claims_but_never_look_complete() {
    for unknown in [
        "<disk type='network'><source protocol='rbd' name='pool/disk'/></disk>",
        "<hostdev type='usb'><source><vendor id='0x1234'/></source></hostdev>",
        "<filesystem type='mount'><source dir='/host/shared'/></filesystem>",
        "<disk type='file'><driver type='raw'/><source file='/private/disk.raw'/><dataStore type='file'><source file='/private/data.raw'/></dataStore></disk>",
        "<interface type='network'><source network='default'/><actual type='hostdev'/></interface>",
    ] {
        let xml = xml(&format!("<disk type='block'><source dev='/dev/claimed'/></disk>{unknown}"));
        let result = parse_domain_xml(UUID, true, DomainState::Running, Some(&xml));
        assert!(!result.resource_references_complete, "{unknown}");
        assert!(result.resources.iter().any(|reference| reference.location == "/dev/claimed"));
    }
}

#[test]
fn missing_malformed_or_wrong_uuid_xml_stays_unknown() {
    for input in [
        None,
        Some("<domain>"),
        Some("<domain><uuid>wrong</uuid><devices/></domain>"),
        Some("<!DOCTYPE domain [<!ENTITY x SYSTEM '/etc/passwd'>]><domain>&x;</domain>"),
    ] {
        let result = parse_domain_xml(UUID, true, DomainState::Running, input);
        assert!(!result.resource_references_complete);
        assert!(!result.unknown_reasons.is_empty());
    }
    let result = parse_domain_xml("bad-uuid", true, DomainState::Running, Some(&xml("")));
    assert!(!result.resource_references_complete);
}

#[test]
fn non_raw_file_requires_explicit_backing_chain_end() {
    for chain in ["", "<backingStore type='file'><format type='raw'/><source file='/private/base.raw'/></backingStore>"] {
        let xml = xml(&format!("<disk type='file'><driver type='qcow2'/><source file='/private/overlay'/>{chain}</disk>"));
        assert!(!parse_domain_xml(UUID, true, DomainState::Running, Some(&xml)).resource_references_complete);
    }
    let xml = xml("<disk type='file'><driver type='qcow2'/><source file='/private/no-backing.qcow2'/><backingStore/></disk>");
    assert!(
        parse_domain_xml(UUID, true, DomainState::Running, Some(&xml)).resource_references_complete
    );
}

#[test]
fn block_sources_with_non_raw_or_unknown_format_need_backing_evidence() {
    for driver in ["", "<driver type='qcow2'/>"] {
        let xml = xml(&format!(
            "<disk type='block'>{driver}<source dev='/dev/example'/></disk>"
        ));
        assert!(
            !parse_domain_xml(UUID, true, DomainState::Running, Some(&xml))
                .resource_references_complete
        );
    }
    let xml = xml("<disk type='block'><driver type='raw'/><source dev='/dev/example'/></disk>");
    assert!(
        parse_domain_xml(UUID, true, DomainState::Running, Some(&xml)).resource_references_complete
    );
}

#[test]
fn interface_and_hostdev_rom_files_are_retained() {
    for device in [
        "<interface type='network'><source network='default'/><rom file='/private/network.rom'/></interface>",
        "<interface type='user'><rom file='/private/network.rom'/></interface>",
        "<hostdev type='pci'><source><address domain='0x0000' bus='0x01' slot='0x00' function='0x0'/></source><rom file='/private/network.rom'/></hostdev>",
    ] {
        let result = parse_domain_xml(UUID, true, DomainState::Running, Some(&xml(device)));
        assert!(result.resource_references_complete, "{:?}", result.unknown_reasons);
        assert!(result.resources.iter().any(|resource| resource.location == "/private/network.rom" && resource.kind == ResourceKind::File));
    }
}

#[test]
fn unknown_root_os_and_interface_sources_cannot_look_complete() {
    for document in [
        format!("<domain><uuid>{UUID}</uuid><futureStorage file='/private/data'/><devices/></domain>"),
        format!("<domain><uuid>{UUID}</uuid><os><acpi><table>/private/slic</table></acpi></os><devices/></domain>"),
        xml("<interface type='user'><backend type='passt' logFile='/private/log'/></interface>"),
        xml("<interface type='network'><source network='default' dev='/dev/example'/></interface>"),
    ] {
        assert!(!parse_domain_xml(UUID, true, DomainState::Running, Some(&document)).resource_references_complete);
    }
}

#[test]
fn ambiguous_paths_pci_addresses_and_qemu_passthrough_stay_unknown() {
    for device in [
        "<disk type='block'><source dev='/dev/a'/><source dev='/dev/b'/></disk>",
        "<disk type='block'><source dev='../dev/a'/></disk>",
        "<disk type='block'><source dev='/dev/../hidden'/></disk>",
        "<hostdev type='pci'><source><address domain='0x0000' bus='0x01' slot='0x20' function='0x0'/></source></hostdev>",
        "<hostdev type='pci'><source><address domain='0x0000' bus='0x01' slot='0x00' function='0x0'/><unknown/></source></hostdev>",
        "<qemu:commandline xmlns:qemu='http://libvirt.org/schemas/domain/qemu/1.0'><qemu:arg value='-drive'/></qemu:commandline>",
    ] {
        assert!(!parse_domain_xml(UUID, true, DomainState::Running, Some(&xml(device))).resource_references_complete, "{device}");
    }
}

#[test]
fn unknown_or_inconsistent_states_and_oversized_xml_are_rejected() {
    assert_eq!(DomainState::from_libvirt(99), DomainState::Unknown);
    for (active, state) in [
        (true, DomainState::Unknown),
        (true, DomainState::Shutoff),
        (false, DomainState::Running),
    ] {
        assert!(
            !parse_domain_xml(UUID, active, state, Some(&xml(""))).resource_references_complete
        );
    }
    let oversized = xml(&" ".repeat(1024 * 1024));
    assert!(
        !parse_domain_xml(UUID, true, DomainState::Running, Some(&oversized))
            .resource_references_complete
    );
}

#[cfg(not(feature = "native-libvirt"))]
#[test]
fn missing_native_feature_reports_skipped_not_passed() {
    assert!(matches!(
        read_test_driver_inventory(TEST_URI),
        Err(AdapterError::NativeUnavailable)
    ));
    let report = elastic_virt_libvirt::run_test_driver_lab();
    assert_eq!(report.status, elastic_virt_libvirt::LabStatus::Skipped);
    assert_eq!(report.evidence_kind, "not_run");
    assert!(!report.guest_boot_verified);
    assert!(!report.cleanup_complete);
}

#[cfg(feature = "native-libvirt")]
#[test]
fn native_inventory_queries_the_test_driver_without_host_occupancy_claims() {
    let inventory = read_test_driver_inventory(TEST_URI).unwrap();
    assert_eq!(inventory.uri, TEST_URI);
    assert!(inventory.backend_simulated);
    assert!(inventory.domain_listing_complete);
    assert!(!inventory.storage_observation_complete);
    assert!(!inventory.observation_is_atomic);
    assert!(!inventory.domains.is_empty());
}

#[cfg(feature = "native-libvirt")]
#[test]
fn real_libvirt_test_driver_lifecycle_preserves_existing_domains() {
    let report = elastic_virt_libvirt::run_test_driver_lab();
    assert_eq!(
        report.status,
        elastic_virt_libvirt::LabStatus::Passed,
        "{}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    assert_eq!(report.evidence_kind, "real_libvirt_test_driver");
    assert!(report.backend_simulated);
    assert!(!report.guest_boot_verified);
    assert!(!report.physical_execution);
    assert!(report.cleanup_complete);
    assert_eq!(report.checks.len(), 9);
}
