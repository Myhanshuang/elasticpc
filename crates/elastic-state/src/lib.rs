//! Durable checkpoints for the software-only controller. A checkpoint never
//! establishes that a guest has stopped: recovery still requires observation.
//!
//! SQLite uses DELETE rollback journals and synchronous=EXTRA (FULL plus a
//! directory sync after journal deletion). Revision comparison and the entire
//! checkpoint update share one IMMEDIATE transaction. Revision checks prevent
//! lost updates, but callers must separately serialize controller execution.
//!
//! Database paths must be ordinary files in an existing, user-owned directory
//! that other users cannot write. This is not a sandbox against hostile code
//! running as the same user. The application must not replace or externally
//! modify open databases or their SQLite sidecars.

use elastic_planner::mock::MockJournal;
use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const APPLICATION_ID: i32 = 0x4550_434a; // EPCJ
const DATABASE_VERSION: i32 = 1;
const CREATE_JOURNAL: &str = "CREATE TABLE journal (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), revision INTEGER NOT NULL CHECK(revision >= 0), payload TEXT NOT NULL) STRICT";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("unsafe state path: {0}")]
    UnsafePath(String),
    #[error("unsupported or unrecognized state database: {0}")]
    UnsupportedDatabase(String),
    #[error("invalid journal: {0}")]
    InvalidJournal(String),
    #[error("journal revision conflict: expected {expected}, found {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("journal revision exhausted")]
    RevisionExhausted,
    #[error("state file I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("SQLite state error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("journal JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJournal {
    pub revision: u64,
    pub journal: MockJournal,
}

pub struct JournalStore {
    connection: Connection,
    path: PathBuf,
    identity: (u64, u64),
    parent_identity: (u64, u64),
}

impl JournalStore {
    /// Open a recognized journal, or create a new database if the path does not
    /// exist. Existing empty or foreign files are never initialized or repaired.
    /// SQLite alone handles hot rollback journals after an interrupted commit.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let path = safe_path(path)?;
        let parent = path.parent().expect("absolute file path has parent");
        let parent_identity = identity(&fs::metadata(parent)?);
        validate_sidecars(&path)?;
        let created = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                validate_regular(&path, &metadata)?;
                validate_header(&path)?;
                false
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                for suffix in ["-journal", "-wal", "-shm"] {
                    let mut sidecar = path.as_os_str().to_os_string();
                    sidecar.push(suffix);
                    if fs::symlink_metadata(sidecar).is_ok() {
                        return Err(StoreError::UnsafePath(
                            "new database has preexisting sidecar files".into(),
                        ));
                    }
                }
                // create_new and NOFOLLOW ensure an existing object cannot be
                // truncated even if a concurrent caller wins creation.
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)?;
                true
            }
            Err(error) => return Err(error.into()),
        };
        let file_identity = identity(&fs::symlink_metadata(&path)?);
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let mut connection = Connection::open_with_flags(&path, flags)?;
        // No implicit retry of a blocked write. The caller decides what to do
        // with SQLITE_BUSY and must not repeat backend side effects.
        connection.busy_timeout(Duration::ZERO)?;
        connection.pragma_update(None, "trusted_schema", false)?;
        if !created {
            validate_database(&connection)?;
            read_journal(&connection)?;
        }
        connection.pragma_update(None, "synchronous", "EXTRA")?;
        let mode: String = connection.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
        let sync: i32 = connection.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
        if mode != "delete" || sync != 3 {
            return Err(StoreError::UnsupportedDatabase(
                "required rollback-journal durability policy unavailable".into(),
            ));
        }
        if created {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.pragma_update(None, "user_version", DATABASE_VERSION)?;
            transaction.execute(CREATE_JOURNAL, [])?;
            transaction.execute(
                "INSERT INTO journal (singleton, revision, payload) VALUES (1, 0, ?1)",
                [serde_json::to_string(&MockJournal::default())?],
            )?;
            transaction.commit()?;
            // Also persist the newly created database directory entry.
            File::open(parent)?.sync_all()?;
        }
        let store = Self {
            connection,
            path,
            identity: file_identity,
            parent_identity,
        };
        store.load()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<StoredJournal, StoreError> {
        self.validate_paths()?;
        let transaction = self.connection.unchecked_transaction()?;
        validate_database(&transaction)?;
        let stored = read_journal(&transaction)?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Atomically compare the persisted revision and replace the checkpoint.
    /// An error must be treated as a controller execution failure; the caller
    /// must obtain a fresh observation before attempting another operation.
    pub fn commit(
        &mut self,
        expected_revision: u64,
        journal: &MockJournal,
    ) -> Result<u64, StoreError> {
        let payload = serde_json::to_string(journal)?;
        decode_journal(&payload)?;
        self.validate_paths()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_database(&transaction)?;
        // Verify the previous payload too: corruption must not be hidden by a
        // subsequent overwrite with an apparently valid new checkpoint.
        let actual = read_journal(&transaction)?.revision;
        if actual != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                actual,
            });
        }
        let revision = i64::try_from(actual)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or(StoreError::RevisionExhausted)?;
        transaction.execute(
            "UPDATE journal SET revision=?1, payload=?2 WHERE singleton=1 AND revision=?3",
            params![revision, payload, actual as i64],
        )?;
        transaction.commit()?;
        Ok(revision as u64)
    }

    fn validate_paths(&self) -> Result<(), StoreError> {
        safe_path(&self.path)?;
        let metadata = fs::symlink_metadata(&self.path)?;
        validate_regular(&self.path, &metadata)?;
        if identity(&metadata) != self.identity
            || identity(&fs::metadata(self.path.parent().unwrap())?) != self.parent_identity
        {
            return Err(StoreError::UnsafePath(
                "database or parent directory was replaced".into(),
            ));
        }
        validate_sidecars(&self.path)
    }
}

fn identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments or memory preconditions.
    unsafe { libc::geteuid() }
}

fn safe_path(path: &Path) -> Result<PathBuf, StoreError> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err(StoreError::UnsafePath("a file path is required".into()));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    for forbidden in ["/dev", "/proc", "/sys"] {
        if absolute.starts_with(forbidden) {
            return Err(StoreError::UnsafePath(
                "kernel/device paths are forbidden".into(),
            ));
        }
    }
    let mut checked = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                return Err(StoreError::UnsafePath(
                    "parent traversal is forbidden".into(),
                ));
            }
            Component::CurDir => continue,
            other => checked.push(other.as_os_str()),
        }
        match fs::symlink_metadata(&checked) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(StoreError::UnsafePath(format!(
                    "symlink component: {}",
                    checked.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && checked == absolute => {}
            Err(error) => return Err(error.into()),
        }
    }
    let parent = checked
        .parent()
        .ok_or_else(|| StoreError::UnsafePath("no parent".into()))?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.uid() != effective_uid() || metadata.mode() & 0o022 != 0 {
        return Err(StoreError::UnsafePath(
            "parent must be a user-owned directory without group/other write permission".into(),
        ));
    }
    Ok(checked)
}

fn validate_regular(path: &Path, metadata: &Metadata) -> Result<(), StoreError> {
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.nlink() != 1
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o022 != 0
    {
        return Err(StoreError::UnsafePath(format!(
            "expected a user-owned regular file without hardlinks or other writers: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_sidecars(path: &Path) -> Result<(), StoreError> {
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        match fs::symlink_metadata(&sidecar) {
            Ok(metadata) => {
                validate_regular(&sidecar, &metadata)?;
                if suffix != "-journal" {
                    return Err(StoreError::UnsupportedDatabase(
                        "WAL sidecars are incompatible with this store's DELETE policy".into(),
                    ));
                }
                if metadata.len() >= 8 {
                    let mut header = [0u8; 8];
                    OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(&sidecar)?
                        .read_exact(&mut header)?;
                    if header != [0; 8]
                        && header != [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]
                    {
                        return Err(StoreError::UnsupportedDatabase(
                            "unrecognized rollback journal header".into(),
                        ));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn validate_header(path: &Path) -> Result<(), StoreError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut header = [0u8; 100];
    if file.read_exact(&mut header).is_err()
        || &header[..16] != b"SQLite format 3\0"
        || header[18] != 1
        || header[19] != 1
        || i32::from_be_bytes(header[68..72].try_into().unwrap()) != APPLICATION_ID
        || i32::from_be_bytes(header[60..64].try_into().unwrap()) != DATABASE_VERSION
    {
        return Err(StoreError::UnsupportedDatabase(
            "unknown file header, application ID, or schema version".into(),
        ));
    }
    Ok(())
}

fn validate_database(connection: &Connection) -> Result<(), StoreError> {
    let app_id: i32 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i32 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if app_id != APPLICATION_ID || version != DATABASE_VERSION {
        return Err(StoreError::UnsupportedDatabase(format!(
            "application ID {app_id}, schema version {version}"
        )));
    }
    let integrity: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(StoreError::UnsupportedDatabase(format!(
            "integrity check failed: {integrity}"
        )));
    }
    let objects: Vec<(String, String, String)> = connection
        .prepare("SELECT type, name, sql FROM sqlite_schema ORDER BY name")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;
    if objects != vec![("table".into(), "journal".into(), CREATE_JOURNAL.into())] {
        return Err(StoreError::UnsupportedDatabase(
            "journal table definition or schema objects differ".into(),
        ));
    }
    Ok(())
}

fn read_journal(connection: &Connection) -> Result<StoredJournal, StoreError> {
    let (count, singleton, revision, payload): (i64, i64, i64, String) = connection.query_row(
        "SELECT (SELECT count(*) FROM journal), singleton, revision, payload FROM journal",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if count != 1 || singleton != 1 || revision < 0 {
        return Err(StoreError::InvalidJournal("invalid checkpoint row".into()));
    }
    Ok(StoredJournal {
        revision: revision as u64,
        journal: decode_journal(&payload)?,
    })
}

fn decode_journal(payload: &str) -> Result<MockJournal, StoreError> {
    let value: serde_json::Value = serde_json::from_str(payload)?;
    elastic_core::check_schema(&value).map_err(StoreError::InvalidJournal)?;
    let journal: MockJournal = serde_json::from_value(value.clone())?;
    // Serde structs otherwise ignore unknown fields, which could silently
    // discard a future producer's safety preconditions during the next save.
    if serde_json::to_value(&journal)? != value {
        return Err(StoreError::InvalidJournal(
            "unrecognized or noncanonical checkpoint fields".into(),
        ));
    }
    Ok(journal)
}

#[cfg(test)]
mod tests;
