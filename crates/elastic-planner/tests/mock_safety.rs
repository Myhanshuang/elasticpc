use elastic_core::{CapabilityStatus, DomainId, InstallationId, LeaseResource};
use elastic_planner::leases::{
    HostClaim, HostClaimKind, HostObservation, LeaseError, LeaseManager, LeaseMode, LeaseRequest,
    ResourceTopology,
};
use elastic_planner::mock::{
    ControllerError, ControllerState, CrashPoint, MockController, MockHypervisor, ObservedGuest,
    StartFault, StopFault,
};

fn domain(name: &str) -> DomainId {
    DomainId::from(name)
}
fn installation() -> InstallationId {
    InstallationId::from("test-installation")
}
fn disk() -> LeaseResource {
    LeaseResource::BlockDisk("fixture-disk".into())
}
fn part() -> LeaseResource {
    LeaseResource::BlockPartition("fixture-partition".into())
}
fn group() -> LeaseResource {
    LeaseResource::PciIommuGroup("fixture-gpu-hda-group".into())
}
fn write(resource: LeaseResource) -> LeaseRequest {
    LeaseRequest::exclusive(resource)
}
fn read(resource: LeaseResource) -> LeaseRequest {
    LeaseRequest::read_only(resource)
}
fn topology() -> ResourceTopology {
    let mut topology = ResourceTopology::default();
    topology.register_partition("fixture-partition".into(), "fixture-disk".into());
    topology.register_partition("fixture-swap".into(), "fixture-disk".into());
    topology.register_partition("other-partition".into(), "other-disk".into());
    topology
}
fn manager() -> LeaseManager {
    LeaseManager::new(topology(), HostObservation::known(Vec::new()))
}
fn controller() -> MockController {
    MockController::new(topology(), MockHypervisor::default())
}
fn restarted(controller: &MockController) -> MockController {
    MockController::from_journal(
        topology(),
        controller.backend().clone(),
        &controller.journal_json().unwrap(),
    )
    .unwrap()
}

#[test]
fn entire_disk_and_child_partition_conflict_in_either_acquisition_order() {
    for (first, second) in [(disk(), part()), (part(), disk()), (part(), part())] {
        let mut leases = manager();
        leases.acquire(&domain("windows"), &[write(first)]).unwrap();
        assert!(matches!(
            leases.acquire(&domain("manjaro"), &[write(second)]),
            Err(LeaseError::GuestConflict { .. })
        ));
        assert_eq!(leases.leases().len(), 1);
    }
}

#[test]
fn conflicting_batch_is_atomic_including_upgrades() {
    let mut leases = manager();
    leases.acquire(&domain("a"), &[read(part())]).unwrap();
    leases.acquire(&domain("b"), &[write(group())]).unwrap();
    let before = leases.leases().to_vec();
    assert!(leases
        .acquire(&domain("a"), &[write(part()), write(group())])
        .is_err());
    assert_eq!(leases.leases(), before);
    assert!(leases.acquire(&domain("c"), &[read(part())]).is_ok());
}

#[test]
fn same_owner_reacquisition_never_downgrades_a_writer() {
    let mut leases = manager();
    leases.acquire(&domain("a"), &[write(part())]).unwrap();
    leases
        .acquire(&domain("a"), &[read(part()), read(part())])
        .unwrap();
    assert_eq!(leases.leases().len(), 1);
    assert_eq!(leases.leases()[0].request.mode, LeaseMode::ExclusiveWrite);
    assert!(leases.acquire(&domain("b"), &[read(part())]).is_err());
}

#[test]
fn shared_readers_are_allowed_but_iommu_usb_and_display_ownership_is_exclusive() {
    let mut leases = manager();
    leases.acquire(&domain("a"), &[read(disk())]).unwrap();
    leases.acquire(&domain("b"), &[read(part())]).unwrap();
    assert!(leases.acquire(&domain("c"), &[write(part())]).is_err());
    for resource in [
        group(),
        LeaseResource::UsbDevice("fixture-usb".into()),
        LeaseResource::DisplayConnector("fixture-edp".into()),
    ] {
        let mut leases = manager();
        leases
            .acquire(&domain("a"), &[read(resource.clone())])
            .unwrap();
        assert!(leases.acquire(&domain("b"), &[read(resource)]).is_err());
    }
}

#[test]
fn host_mount_swap_and_root_claims_block_overlapping_guest_writes() {
    for kind in [
        HostClaimKind::MountReadOnly,
        HostClaimKind::MountReadWrite,
        HostClaimKind::ActiveSwap,
        HostClaimKind::HostRoot,
    ] {
        let mut leases = LeaseManager::new(
            topology(),
            HostObservation::known(vec![HostClaim {
                resource: part(),
                kind: kind.clone(),
            }]),
        );
        assert!(matches!(
            leases.acquire(&domain("a"), &[write(disk())]),
            Err(LeaseError::HostConflict { .. })
        ));
        if kind != HostClaimKind::MountReadOnly {
            assert!(leases.acquire(&domain("a"), &[read(disk())]).is_err());
        }
    }
    let mut leases = LeaseManager::new(
        topology(),
        HostObservation::known(vec![HostClaim {
            resource: disk(),
            kind: HostClaimKind::MountReadWrite,
        }]),
    );
    assert!(leases.acquire(&domain("a"), &[write(part())]).is_err());
}

#[test]
fn incomplete_host_observation_and_unknown_ancestry_cannot_authorize_leases() {
    let mut leases = LeaseManager::new(topology(), HostObservation::unknown());
    assert_eq!(
        leases.acquire(&domain("a"), &[write(part())]),
        Err(LeaseError::ObservationIncomplete)
    );
    let mut leases = manager();
    assert!(matches!(
        leases.acquire(
            &domain("a"),
            &[write(LeaseResource::BlockPartition("not-observed".into()))]
        ),
        Err(LeaseError::UnknownResource { .. })
    ));
    assert!(matches!(
        leases.acquire(
            &domain("a"),
            &[write(LeaseResource::BlockDisk("not-observed".into()))]
        ),
        Err(LeaseError::UnknownResource { .. })
    ));
    assert!(leases.leases().is_empty());
}

#[test]
fn duplicated_partuuid_quarantines_all_implicated_disks() {
    let mut topology = topology();
    topology.register_partition("fixture-partition".into(), "other-disk".into());
    topology.register_partition("fixture-partition".into(), "third-disk".into());
    assert!(topology.overlaps(&disk(), &part()));
    assert!(topology.overlaps(&LeaseResource::BlockDisk("third-disk".into()), &part()));
    let mut leases = LeaseManager::new(topology, HostObservation::known(Vec::new()));
    for resource in [
        part(),
        disk(),
        LeaseResource::BlockDisk("other-disk".into()),
        LeaseResource::BlockDisk("third-disk".into()),
    ] {
        assert!(matches!(
            leases.acquire(&domain("a"), &[write(resource)]),
            Err(LeaseError::UnknownResource { .. })
        ));
    }
    assert!(leases.leases().is_empty());
}

#[test]
fn empty_stable_identifiers_cannot_authorize_an_allocation() {
    let mut topology = topology();
    topology.register_partition("".into(), "".into());
    let mut leases = LeaseManager::new(topology, HostObservation::known(Vec::new()));
    for resource in [
        LeaseResource::BlockDisk("".into()),
        LeaseResource::BlockPartition("".into()),
        LeaseResource::PciIommuGroup(" ".into()),
        LeaseResource::UsbDevice("".into()),
        LeaseResource::DisplayConnector("".into()),
    ] {
        assert!(matches!(
            leases.acquire(&domain("a"), &[write(resource)]),
            Err(LeaseError::UnknownResource { .. })
        ));
    }
}

#[test]
fn transient_paths_cannot_be_registered_as_stable_lease_identities() {
    let mut topology = topology();
    topology.register_partition("bad/partuuid".into(), "/dev/mock-unstable".into());
    let mut leases = LeaseManager::new(topology, HostObservation::known(Vec::new()));
    for resource in [
        LeaseResource::BlockDisk("/dev/mock-unstable".into()),
        LeaseResource::BlockPartition("bad/partuuid".into()),
    ] {
        assert!(matches!(
            leases.acquire(&domain("a"), &[write(resource)]),
            Err(LeaseError::UnknownResource { .. })
        ));
    }
}

#[test]
fn start_stop_and_operation_replay_are_idempotent() {
    let mut controller = controller();
    let vm = domain("desktop");
    for op in ["start-1", "start-1", "start-2"] {
        assert_eq!(
            controller
                .start(op, &installation(), &vm, &[write(disk()), write(group())])
                .unwrap(),
            ControllerState::Running
        );
    }
    assert_eq!(controller.backend().start_calls(), 1);
    assert_eq!(controller.leases().leases().len(), 2);
    for op in ["stop-1", "stop-1", "stop-2"] {
        assert_eq!(
            controller.stop(op, &installation(), &vm).unwrap(),
            ControllerState::Stopped
        );
    }
    assert_eq!(controller.backend().stop_calls(), 1);
    assert!(controller.leases().leases().is_empty());
    // Replaying a completed request never resurrects a guest stopped later.
    assert_eq!(
        controller
            .start(
                "start-1",
                &installation(),
                &vm,
                &[write(group()), write(disk())]
            )
            .unwrap(),
        ControllerState::Stopped
    );
    assert_eq!(controller.backend().start_calls(), 1);
    assert_eq!(
        controller.start(
            "start-1",
            &installation(),
            &domain("different"),
            &[write(disk())]
        ),
        Err(ControllerError::OperationIdReused)
    );
}

#[test]
fn all_start_crash_boundaries_recover_using_actual_guest_ownership() {
    for point in [
        CrashPoint::AfterJournal,
        CrashPoint::AfterLeases,
        CrashPoint::AfterStart,
    ] {
        let mut original = controller();
        let vm = domain("desktop");
        original.inject_crash(point);
        assert_eq!(
            original.start(
                "start",
                &installation(),
                &vm,
                &[write(disk()), write(group())]
            ),
            Err(ControllerError::SimulatedCrash(point))
        );
        let mut recovered = restarted(&original);
        let report = recovered.recover().unwrap();
        if point == CrashPoint::AfterStart {
            assert_eq!(report.retained_resources, 2);
            assert_eq!(recovered.state(&vm), ControllerState::Running);
            assert!(recovered
                .start(
                    "rival-start",
                    &installation(),
                    &domain("rival"),
                    &[write(part())]
                )
                .is_err());
        } else {
            assert_eq!(report.retained_resources, 0);
            assert_eq!(recovered.state(&vm), ControllerState::Stopped);
        }
        assert_eq!(
            recovered
                .start(
                    "start",
                    &installation(),
                    &vm,
                    &[write(disk()), write(group())]
                )
                .unwrap(),
            ControllerState::Running
        );
        assert_eq!(recovered.backend().start_calls(), 1);
        assert_eq!(recovered.leases().leases().len(), 2);
    }
}

#[test]
fn all_stop_crash_boundaries_hold_claims_until_observed_exit() {
    for point in [
        CrashPoint::AfterStopIntent,
        CrashPoint::AfterStop,
        CrashPoint::AfterRelease,
    ] {
        let mut original = controller();
        let vm = domain("desktop");
        original
            .start("start", &installation(), &vm, &[write(part())])
            .unwrap();
        original.inject_crash(point);
        assert_eq!(
            original.stop("stop", &installation(), &vm),
            Err(ControllerError::SimulatedCrash(point))
        );
        let mut recovered = restarted(&original);
        recovered.recover().unwrap();
        assert_eq!(
            recovered.leases().leases().len(),
            usize::from(point == CrashPoint::AfterStopIntent)
        );
        recovered.stop("stop", &installation(), &vm).unwrap();
        assert_eq!(recovered.backend().stop_calls(), 1);
        assert!(recovered.leases().leases().is_empty());
    }
}

#[test]
fn failed_stop_preserves_leases_and_successful_retry_releases_them() {
    let mut controller = controller();
    let vm = domain("desktop");
    controller
        .start("start", &installation(), &vm, &[write(disk())])
        .unwrap();
    controller.backend_mut().fail_next_stop(StopFault::Reject);
    assert!(controller
        .stop("stop-failed", &installation(), &vm)
        .is_err());
    assert_eq!(controller.state(&vm), ControllerState::Running);
    assert_eq!(controller.leases().leases().len(), 1);
    assert!(controller
        .start(
            "rival-start",
            &installation(),
            &domain("rival"),
            &[write(part())]
        )
        .is_err());
    controller.stop("stop-retry", &installation(), &vm).unwrap();
    assert!(controller.leases().leases().is_empty());
    assert!(!controller
        .journal()
        .operations
        .iter()
        .find(|op| op.operation_id == "stop-failed")
        .unwrap()
        .compensations
        .is_empty());
}

#[test]
fn a_failed_start_response_does_not_prove_the_guest_released_resources() {
    for fault in [StartFault::Reject, StartFault::ErrorAfterRunning] {
        let mut controller = controller();
        let vm = domain("desktop");
        controller.backend_mut().fail_next_start(fault);
        assert!(controller
            .start("start", &installation(), &vm, &[write(group())])
            .is_err());
        if fault == StartFault::ErrorAfterRunning {
            assert_eq!(controller.state(&vm), ControllerState::Running);
            assert_eq!(controller.leases().leases().len(), 1);
            assert!(controller
                .start(
                    "rival",
                    &installation(),
                    &domain("rival"),
                    &[write(group())]
                )
                .is_err());
        } else {
            assert_eq!(controller.state(&vm), ControllerState::Stopped);
            assert!(controller.leases().leases().is_empty());
        }
        assert!(!controller.journal().operations[0].compensations.is_empty());
    }
}

#[test]
fn lost_stop_response_can_release_only_after_observation_proves_stopped_io() {
    let mut controller = controller();
    let vm = domain("desktop");
    controller
        .start("start", &installation(), &vm, &[write(disk())])
        .unwrap();
    controller
        .backend_mut()
        .fail_next_stop(StopFault::ErrorAfterStopped);
    assert_eq!(
        controller.stop("stop", &installation(), &vm).unwrap(),
        ControllerState::Stopped
    );
    assert!(controller.leases().leases().is_empty());
    assert!(!controller.journal().operations[1].compensations.is_empty());
}

#[test]
fn unknown_or_partial_recovery_never_frees_checkpointed_resources() {
    let mut original = controller();
    let vm = domain("desktop");
    original
        .start("start", &installation(), &vm, &[write(disk())])
        .unwrap();
    original.backend_mut().set_observation_available(false);
    let mut recovered = restarted(&original);
    assert_eq!(
        recovered.recover(),
        Err(ControllerError::UnknownObservation)
    );
    assert_eq!(recovered.leases().leases().len(), 1);
    assert!(recovered
        .start("rival", &installation(), &domain("rival"), &[write(part())])
        .is_err());
    assert!(recovered
        .stop("stop-unobserved", &installation(), &vm)
        .is_err());
    assert!(!recovered.leases().for_domain(&vm).is_empty());
    // The mock stop really exited, but incomplete host state still cannot clear
    // a recovery quarantine or authorize a fresh allocation.
    recovered.backend_mut().set_observation_available(true);
    recovered
        .backend_mut()
        .set_host_observation(HostObservation::unknown());
    assert_eq!(
        recovered.recover(),
        Err(ControllerError::UnknownObservation)
    );
    assert!(!recovered.leases().for_domain(&vm).is_empty());
    recovered
        .backend_mut()
        .set_host_observation(HostObservation::known(Vec::new()));
    recovered.recover().unwrap();
    assert!(recovered.leases().leases().is_empty());
}

#[test]
fn stale_journal_cannot_override_actual_guest_or_host_ownership() {
    let mut original = controller();
    let old = domain("old");
    original
        .start("old-start", &installation(), &old, &[write(disk())])
        .unwrap();
    original.backend_mut().observe_stopped(&old);
    original.backend_mut().observe_running(ObservedGuest {
        domain_id: domain("external"),
        installation_id: installation(),
        resources: vec![write(part())],
    });
    original
        .backend_mut()
        .set_host_observation(HostObservation::known(vec![HostClaim {
            resource: group(),
            kind: HostClaimKind::OtherExclusive,
        }]));
    let mut recovered = restarted(&original);
    recovered.recover().unwrap();
    assert!(recovered.leases().for_domain(&old).is_empty());
    assert_eq!(
        recovered.leases().for_domain(&domain("external")),
        vec![write(part())]
    );
    assert!(recovered
        .start(
            "new-start",
            &installation(),
            &domain("new"),
            &[write(disk())]
        )
        .is_err());
    assert!(recovered
        .start(
            "gpu-start",
            &installation(),
            &domain("gpu"),
            &[write(group())]
        )
        .is_err());
}

#[test]
fn stopped_guest_resources_stay_quarantined_while_host_observation_is_partial() {
    let mut controller = controller();
    let vm = domain("desktop");
    controller
        .start("start", &installation(), &vm, &[write(disk())])
        .unwrap();
    controller
        .backend_mut()
        .set_host_observation(HostObservation::unknown());
    assert_eq!(
        controller.stop("partial-stop", &installation(), &vm),
        Err(ControllerError::UnknownObservation)
    );
    assert_eq!(controller.state(&vm), ControllerState::Stopped);
    assert_eq!(controller.leases().for_domain(&vm), vec![write(disk())]);
    controller
        .backend_mut()
        .set_host_observation(HostObservation::known(Vec::new()));
    controller.recover().unwrap();
    assert!(controller.leases().leases().is_empty());
}

#[test]
fn observed_resource_drift_replaces_stale_journal_claims_for_same_owner() {
    let mut original = controller();
    let vm = domain("desktop");
    original
        .start("start", &installation(), &vm, &[write(disk())])
        .unwrap();
    original.backend_mut().observe_running(ObservedGuest {
        domain_id: vm.clone(),
        installation_id: installation(),
        resources: vec![write(group())],
    });
    let mut recovered = restarted(&original);
    recovered.recover().unwrap();
    assert_eq!(recovered.leases().for_domain(&vm), vec![write(group())]);
    assert!(recovered
        .start(
            "new-disk-owner",
            &installation(),
            &domain("new"),
            &[write(disk())]
        )
        .is_ok());
    assert!(recovered
        .start(
            "gpu-rival",
            &installation(),
            &domain("gpu"),
            &[write(group())]
        )
        .is_err());
}

#[test]
fn changed_running_configuration_does_not_erase_existing_reservations() {
    let mut controller = controller();
    let vm = domain("desktop");
    controller
        .start("start", &installation(), &vm, &[write(disk())])
        .unwrap();
    assert_eq!(
        controller.start("changed", &installation(), &vm, &[write(group())]),
        Err(ControllerError::RunningConfigurationChanged)
    );
    assert_eq!(controller.leases().for_domain(&vm), vec![write(disk())]);
    assert!(controller
        .start("rival", &installation(), &domain("rival"), &[write(part())])
        .is_err());
}

#[test]
fn actual_external_conflicts_are_reported_without_dropping_either_owner() {
    let mut backend = MockHypervisor::default();
    for name in ["a", "b"] {
        backend.observe_running(ObservedGuest {
            domain_id: domain(name),
            installation_id: installation(),
            resources: vec![write(group())],
        });
    }
    let mut controller = MockController::new(topology(), backend);
    let report = controller.recover().unwrap();
    assert_eq!(report.conflicts.len(), 2);
    assert_eq!(controller.leases().leases().len(), 2);
    assert!(controller
        .start("third", &installation(), &domain("c"), &[write(group())])
        .is_err());
}

#[test]
fn journal_versions_and_unknown_actions_fail_closed() {
    let mut controller = controller();
    controller
        .start(
            "start",
            &installation(),
            &domain("desktop"),
            &[write(part())],
        )
        .unwrap();
    let json = controller.journal_json().unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["schema_version"] = 999.into();
    assert!(MockController::from_journal(
        topology(),
        MockHypervisor::default(),
        &value.to_string()
    )
    .is_err());
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["operations"][0]["desired_state"] = "future_unrecognized_action".into();
    assert!(MockController::from_journal(
        topology(),
        MockHypervisor::default(),
        &value.to_string()
    )
    .is_err());
    assert_eq!(
        controller
            .backend()
            .capabilities()
            .physical_device_execution,
        CapabilityStatus::Unsupported
    );
    assert_eq!(
        controller.backend().capabilities().durable_journal,
        CapabilityStatus::Unsupported
    );
}
