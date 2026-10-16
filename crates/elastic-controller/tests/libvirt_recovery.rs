#[cfg(not(feature = "native-libvirt"))]
#[test]
fn native_feature_absence_never_reports_execution() {
    let report = elastic_controller::libvirt_lab::run_controller_lab();
    assert_eq!(report.status, "skipped");
    assert_eq!(report.evidence_kind, "not_run");
    assert!(!report.controller_reconstructed);
}

#[cfg(feature = "native-libvirt")]
mod native {
    use elastic_controller::{
        libvirt_lab::{native::TestDriverBackend, run_controller_lab},
        DurableController,
    };
    use elastic_core::{DomainId, InstallationId, LeaseResource};
    use elastic_planner::{
        leases::{Lease, LeaseRequest, ResourceTopology},
        mock::{ControllerError, ControllerState, CrashPoint, LifecycleBackend, MockJournal},
    };
    use elastic_state::JournalStore;
    use elastic_virt_libvirt::TestSession;
    use std::{path::Path, rc::Rc};

    fn open(path: &Path, backend: &TestDriverBackend) -> DurableController<TestDriverBackend> {
        DurableController::open(path, ResourceTopology::default(), backend.clone()).unwrap()
    }
    #[test]
    fn real_test_driver_reconstruction_and_service_isolation() {
        let report = run_controller_lab();
        assert_eq!(report.status, "passed", "{report:?}");
        assert!(report.cleanup_complete && report.controller_reconstructed);
        assert!(!report.physical_resource_leases_supported && !report.process_crash_verified);
    }
    #[test]
    fn all_six_boundaries_recover_against_retained_native_session() {
        for point in [
            CrashPoint::AfterJournal,
            CrashPoint::AfterLeases,
            CrashPoint::AfterStart,
            CrashPoint::AfterStopIntent,
            CrashPoint::AfterStop,
            CrashPoint::AfterRelease,
        ] {
            let session = Rc::new(TestSession::diskless(1).unwrap());
            let backend = TestDriverBackend::new(session.clone());
            let domain = DomainId(session.domain_ids()[0].clone());
            let installation = TestDriverBackend::installation(&domain);
            let directory = tempfile::tempdir().unwrap();
            let db = directory.path().join("state.sqlite3");
            let is_start = matches!(
                point,
                CrashPoint::AfterJournal | CrashPoint::AfterLeases | CrashPoint::AfterStart
            );
            let mut controller = open(&db, &backend);
            if !is_start {
                controller
                    .controller_mut()
                    .start("start", &installation, &domain, &[])
                    .unwrap();
            }
            controller.controller_mut().inject_crash(point);
            let result = if is_start {
                controller
                    .controller_mut()
                    .start("start", &installation, &domain, &[])
            } else {
                controller
                    .controller_mut()
                    .stop("stop", &installation, &domain)
            };
            assert!(matches!(result, Err(ControllerError::SimulatedCrash(p)) if p==point));
            drop(controller);
            backend.set_observation_available(false);
            let mut controller = open(&db, &backend);
            assert_eq!(
                controller.controller_mut().recover().unwrap_err(),
                ControllerError::UnknownObservation
            );
            backend.set_observation_available(true);
            controller.controller_mut().recover().unwrap();
            if is_start {
                controller
                    .controller_mut()
                    .start("start", &installation, &domain, &[])
                    .unwrap();
            }
            assert_eq!(backend.start_calls(), 1, "{point:?}");
            controller
                .controller_mut()
                .stop("stop", &installation, &domain)
                .unwrap();
            assert_eq!(
                controller.controller().state(&domain),
                ControllerState::Stopped
            );
            let calls = backend.stop_calls();
            controller
                .controller_mut()
                .stop("stop", &installation, &domain)
                .unwrap();
            assert_eq!(backend.stop_calls(), calls);
            assert!(controller
                .controller()
                .journal()
                .operations
                .iter()
                .all(|o| o.completed));
            drop(controller);
            drop(backend);
            Rc::try_unwrap(session).ok().unwrap().close().unwrap();
        }
    }
    #[test]
    fn test_backend_refuses_physical_and_foreign_identity_before_journaling() {
        let session = Rc::new(TestSession::diskless(1).unwrap());
        let backend = TestDriverBackend::new(session.clone());
        let domain = DomainId(session.domain_ids()[0].clone());
        let directory = tempfile::tempdir().unwrap();
        let mut controller = open(&directory.path().join("state.sqlite3"), &backend);
        let request = LeaseRequest::exclusive(LeaseResource::PciIommuGroup("15".into()));
        assert!(controller
            .controller_mut()
            .start(
                "physical",
                &TestDriverBackend::installation(&domain),
                &domain,
                std::slice::from_ref(&request)
            )
            .is_err());
        assert!(controller
            .controller_mut()
            .start("identity", &InstallationId("wrong".into()), &domain, &[])
            .is_err());
        backend.set_observation_available(false);
        assert!(controller
            .controller_mut()
            .stop("wrong-stop", &InstallationId("wrong".into()), &domain)
            .is_err());
        assert!(controller.controller().journal().operations.is_empty());
        assert_eq!(backend.start_calls(), 0);
        let journal = MockJournal {
            reservations: vec![Lease {
                owner: domain,
                request,
            }],
            ..Default::default()
        };
        assert!(backend.validate_journal(&journal).is_err());
    }
    #[test]
    fn stale_sqlite_writer_poison_prevents_native_start_and_stop() {
        let session = Rc::new(TestSession::diskless(1).unwrap());
        let backend = TestDriverBackend::new(session.clone());
        let domain = DomainId(session.domain_ids()[0].clone());
        let installation = TestDriverBackend::installation(&domain);
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("state.sqlite3");
        let mut controller = open(&db, &backend);
        controller
            .controller_mut()
            .start("initial-start", &installation, &domain, &[])
            .unwrap();
        let mut other = JournalStore::open(&db).unwrap();
        let previous = other.load().unwrap();
        other.commit(previous.revision, &previous.journal).unwrap();
        assert!(matches!(
            controller
                .controller_mut()
                .start("start", &installation, &domain, &[]),
            Err(ControllerError::Persistence(_))
        ));
        assert!(matches!(
            controller
                .controller_mut()
                .stop("stop", &installation, &domain),
            Err(ControllerError::Persistence(_))
        ));
        assert_eq!((backend.start_calls(), backend.stop_calls()), (1, 0));
        assert_eq!(backend.domain_state(&domain), ControllerState::Running);
        drop(controller);
        drop(other);
        let mut restored = open(&db, &backend);
        restored
            .controller_mut()
            .start("start", &installation, &domain, &[])
            .unwrap();
        assert_eq!(
            restored.controller().state(&domain),
            ControllerState::Running
        );
    }
    #[test]
    fn reconnecting_test_driver_cannot_impersonate_previous_backend() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("state.sqlite3");
        let session = Rc::new(TestSession::diskless(1).unwrap());
        let backend = TestDriverBackend::new(session.clone());
        let domain = DomainId(session.domain_ids()[0].clone());
        let mut controller = open(&db, &backend);
        controller
            .controller_mut()
            .start(
                "start",
                &TestDriverBackend::installation(&domain),
                &domain,
                &[],
            )
            .unwrap();
        drop(controller);
        let foreign = TestDriverBackend::new(Rc::new(TestSession::diskless(1).unwrap()));
        assert!(DurableController::open(&db, ResourceTopology::default(), foreign).is_err());
        assert_eq!(backend.domain_state(&domain), ControllerState::Running);
    }
    #[test]
    fn unknown_stop_never_commits_completed_operation() {
        let session = Rc::new(TestSession::diskless(1).unwrap());
        let backend = TestDriverBackend::new(session.clone());
        let domain = DomainId(session.domain_ids()[0].clone());
        let installation = TestDriverBackend::installation(&domain);
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("state.sqlite3");
        let mut controller = open(&db, &backend);
        controller
            .controller_mut()
            .start("start", &installation, &domain, &[])
            .unwrap();
        backend.set_observation_available(false);
        assert_eq!(
            controller
                .controller_mut()
                .stop("unknown-stop", &installation, &domain)
                .unwrap_err(),
            ControllerError::UnknownObservation
        );
        assert!(
            !JournalStore::open(&db)
                .unwrap()
                .load()
                .unwrap()
                .journal
                .operations
                .last()
                .unwrap()
                .completed
        );
        backend.set_observation_available(true);
        controller.controller_mut().recover().unwrap();
        controller
            .controller_mut()
            .stop("verified-stop", &installation, &domain)
            .unwrap();
        assert_eq!(
            controller.controller().state(&domain),
            ControllerState::Stopped
        );
    }
}
