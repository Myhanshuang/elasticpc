use super::*;
use elastic_core::{DesiredState, DomainId, InstallationId};
use elastic_planner::mock::{ControllerState, OperationRecord};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::{Command, Stdio};

fn journal(label: &str) -> MockJournal {
    MockJournal {
        operations: vec![OperationRecord {
            operation_id: label.into(),
            installation_id: InstallationId("fixture-installation".into()),
            domain_id: DomainId("fixture-domain".into()),
            desired_state: DesiredState::Running,
            preconditions: vec!["fresh mock observation required".into()],
            requested_resources: Vec::new(),
            acquired_resources: Vec::new(),
            steps: vec![ControllerState::Planned],
            compensations: Vec::new(),
            completed: false,
            failure: None,
        }],
        ..MockJournal::default()
    }
}

#[test]
fn committed_checkpoint_survives_reopen_and_revision_conflict_preserves_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let mut first = JournalStore::open(&path).unwrap();
    let mut stale = JournalStore::open(&path).unwrap();
    assert_eq!(first.load().unwrap().revision, 0);
    assert_eq!(first.commit(0, &journal("first")).unwrap(), 1);
    assert!(matches!(
        stale.commit(0, &journal("stale")),
        Err(StoreError::Conflict {
            expected: 0,
            actual: 1
        })
    ));
    drop(first);
    drop(stale);
    let reopened = JournalStore::open(&path).unwrap().load().unwrap();
    assert_eq!(reopened.revision, 1);
    assert_eq!(reopened.journal, journal("first"));
    assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn durability_pragmas_are_checked_and_no_wal_is_created() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let mut store = JournalStore::open(&path).unwrap();
    let sync: i32 = store
        .connection
        .query_row("PRAGMA synchronous", [], |r| r.get(0))
        .unwrap();
    let mode: String = store
        .connection
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sync, 3);
    assert_eq!(mode, "delete");
    store.commit(0, &journal("durable")).unwrap();
    assert!(!directory.path().join("state.sqlite-wal").exists());
    assert!(!directory.path().join("state.sqlite-journal").exists());
}

#[test]
fn foreign_empty_future_and_corrupt_databases_are_not_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        ("empty", b"".as_slice()),
        ("foreign", b"important user file".as_slice()),
    ] {
        let path = directory.path().join(name);
        fs::write(&path, bytes).unwrap();
        assert!(JournalStore::open(&path).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    let unrelated = directory.path().join("unrelated.sqlite");
    let connection = Connection::open(&unrelated).unwrap();
    connection
        .execute("CREATE TABLE important(data TEXT)", [])
        .unwrap();
    drop(connection);
    let before = fs::read(&unrelated).unwrap();
    assert!(JournalStore::open(&unrelated).is_err());
    assert_eq!(fs::read(unrelated).unwrap(), before);

    let future = directory.path().join("future.sqlite");
    drop(JournalStore::open(&future).unwrap());
    let connection = Connection::open(&future).unwrap();
    connection.pragma_update(None, "user_version", 999).unwrap();
    drop(connection);
    let before = fs::read(&future).unwrap();
    assert!(JournalStore::open(&future).is_err());
    assert_eq!(fs::read(future).unwrap(), before);

    let corrupt = directory.path().join("corrupt.sqlite");
    drop(JournalStore::open(&corrupt).unwrap());
    let mut bytes = fs::read(&corrupt).unwrap();
    bytes[100..200].fill(0x7f);
    fs::write(&corrupt, &bytes).unwrap();
    assert!(JournalStore::open(&corrupt).is_err());
    assert_eq!(fs::read(corrupt).unwrap(), bytes);
}

#[test]
fn incompatible_schema_and_payload_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let mut store = JournalStore::open(&path).unwrap();
    let mut future = journal("future");
    future.schema_version = 99;
    assert!(store.commit(0, &future).is_err());
    assert_eq!(store.load().unwrap().revision, 0);
    store
        .connection
        .execute("CREATE TABLE surprise(value TEXT)", [])
        .unwrap();
    assert!(store.load().is_err());
    assert!(store.commit(0, &journal("cannot-overwrite")).is_err());
    store.connection.execute("DROP TABLE surprise", []).unwrap();
    store
        .connection
        .execute("UPDATE journal SET payload='{}'", [])
        .unwrap();
    assert!(store.load().is_err());
    assert!(store.commit(0, &journal("cannot-hide-corruption")).is_err());
    let mut value = serde_json::to_value(journal("future-field")).unwrap();
    value["operations"][0]["future_safety_requirement"] = true.into();
    store
        .connection
        .execute("UPDATE journal SET payload=?1", [value.to_string()])
        .unwrap();
    assert!(store.load().is_err());
}

#[test]
fn unknown_enum_missing_row_and_revision_exhaustion_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let mut store = JournalStore::open(&path).unwrap();
    let mut value = serde_json::to_value(journal("bad-action")).unwrap();
    value["operations"][0]["desired_state"] = "future_action".into();
    store
        .connection
        .execute("UPDATE journal SET payload=?1", [value.to_string()])
        .unwrap();
    assert!(store.load().is_err());
    store
        .connection
        .execute(
            "UPDATE journal SET revision=?1, payload=?2",
            params![i64::MAX, serde_json::to_string(&journal("last")).unwrap()],
        )
        .unwrap();
    assert!(matches!(
        store.commit(i64::MAX as u64, &journal("overflow")),
        Err(StoreError::RevisionExhausted)
    ));
    store.connection.execute("DELETE FROM journal", []).unwrap();
    assert!(store.load().is_err());
    assert!(store.commit(0, &journal("missing")).is_err());
}

#[test]
fn dangerous_paths_and_sidecars_are_rejected_without_writing_targets() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("valuable");
    fs::write(&target, "do not touch").unwrap();
    let alias = directory.path().join("symlink.sqlite");
    symlink(&target, &alias).unwrap();
    assert!(JournalStore::open(&alias).is_err());
    let hardlink = directory.path().join("hardlink.sqlite");
    fs::hard_link(&target, &hardlink).unwrap();
    assert!(JournalStore::open(&hardlink).is_err());
    assert!(JournalStore::open(directory.path()).is_err());
    assert!(JournalStore::open(Path::new("/dev/null")).is_err());
    let nested_alias = directory.path().join("alias");
    symlink(directory.path(), &nested_alias).unwrap();
    assert!(JournalStore::open(&nested_alias.join("state.sqlite")).is_err());
    for suffix in ["-journal", "-wal", "-shm"] {
        let path = directory.path().join(format!("state{suffix}.sqlite"));
        let sidecar = directory
            .path()
            .join(format!("state{suffix}.sqlite{suffix}"));
        symlink(&target, &sidecar).unwrap();
        assert!(JournalStore::open(&path).is_err());
        assert!(!path.exists());
    }
    assert_eq!(fs::read_to_string(target).unwrap(), "do not touch");
    let fresh = directory.path().join("orphan.sqlite");
    let orphan = directory.path().join("orphan.sqlite-journal");
    fs::write(&orphan, [0u8; 32]).unwrap();
    assert!(JournalStore::open(&fresh).is_err());
    assert!(!fresh.exists());
    assert_eq!(fs::read(orphan).unwrap(), [0u8; 32]);
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(JournalStore::open(&directory.path().join("unsafe.sqlite")).is_err());
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn replacing_an_open_database_is_detected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let mut store = JournalStore::open(&path).unwrap();
    fs::rename(&path, directory.path().join("old.sqlite")).unwrap();
    let replacement = b"unrelated replacement file";
    fs::write(&path, replacement).unwrap();
    assert!(store.load().is_err());
    assert!(store.commit(0, &journal("replacement")).is_err());
    assert_eq!(fs::read(path).unwrap(), replacement);
}

fn child(path: &Path, stage: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tests::subprocess_worker",
            "--ignored",
            "--nocapture",
        ])
        .env("ELASTIC_STATE_TEST_DB", path)
        .env("ELASTIC_STATE_TEST_STAGE", stage);
    command
}

#[test]
fn abrupt_exit_before_and_after_commit_has_atomic_results() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let mut store = JournalStore::open(&path).unwrap();
    // A large existing row plus a one-page child cache forces modified pages
    // through rollback journaling, exercising recovery beyond in-memory state.
    store.commit(0, &journal(&"old".repeat(100_000))).unwrap();
    drop(store);
    let status = child(&path, "before-commit").status().unwrap();
    assert_eq!(status.code(), Some(87));
    let stored = JournalStore::open(&path).unwrap().load().unwrap();
    assert_eq!(stored.revision, 1);
    assert_eq!(stored.journal, journal(&"old".repeat(100_000)));
    let status = child(&path, "after-commit").status().unwrap();
    assert_eq!(status.code(), Some(87));
    let stored = JournalStore::open(&path).unwrap().load().unwrap();
    assert_eq!(stored.revision, 2);
    assert_eq!(stored.journal, journal("child-committed"));
}

#[test]
fn competing_process_write_is_busy_then_stale_revision_conflicts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let mut store = JournalStore::open(&path).unwrap();
    let mut process = child(&path, "hold-writer")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "child did not become ready"
        );
        if line.trim() == "writer-ready" {
            break;
        }
    }
    let error = store.commit(0, &journal("rival")).unwrap_err();
    assert!(
        matches!(error, StoreError::Sqlite(rusqlite::Error::SqliteFailure(ref code, _)) if code.code == rusqlite::ErrorCode::DatabaseBusy)
    );
    process.stdin.take().unwrap().write_all(b"release").unwrap();
    assert!(process.wait().unwrap().success());
    assert!(matches!(
        store.commit(0, &journal("stale")),
        Err(StoreError::Conflict {
            expected: 0,
            actual: 1
        })
    ));
    assert_eq!(store.load().unwrap().journal, journal("child-committed"));
}

#[test]
#[ignore = "subprocess helper, invoked by durability and contention tests"]
fn subprocess_worker() {
    let path = PathBuf::from(std::env::var_os("ELASTIC_STATE_TEST_DB").expect("test path"));
    let stage = std::env::var("ELASTIC_STATE_TEST_STAGE").expect("test stage");
    let mut store = JournalStore::open(&path).unwrap();
    let revision = store.load().unwrap().revision;
    if stage == "after-commit" {
        store.commit(revision, &journal("child-committed")).unwrap();
        std::process::exit(87); // Deliberately bypass every Rust destructor.
    }
    store
        .connection
        .pragma_update(None, "cache_size", 1)
        .unwrap();
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let changed = if stage == "before-commit" {
        journal(&"uncommitted".repeat(100_000))
    } else {
        journal("child-committed")
    };
    transaction
        .execute(
            "UPDATE journal SET revision=?1,payload=?2",
            params![
                revision as i64 + 1,
                serde_json::to_string(&changed).unwrap()
            ],
        )
        .unwrap();
    if stage == "before-commit" {
        assert!(path.with_file_name("state.sqlite-journal").exists());
        std::process::exit(87);
    }
    assert_eq!(stage, "hold-writer");
    println!("writer-ready");
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
    transaction.commit().unwrap();
}
