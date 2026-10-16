//! Explicit lab worker for a private libvirt socket. Not a production backend.
//! The harness owns XML, images and daemon; this worker owns durable lifecycle.
use crate::DurableController;
use elastic_core::{DomainId, InstallationId};
use elastic_planner::leases::{HostObservation, ResourceTopology};
use elastic_planner::mock::{
    BackendObservation, ControllerError, ControllerState, CrashPoint, LifecycleBackend,
    MockJournal, ObservedGuest,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Deserialize)]
struct Manifest {
    root: PathBuf,
    virsh: PathBuf,
    library: PathBuf,
    domains: Vec<String>,
    base_sha256: String,
}
struct Backend {
    manifest: Manifest,
}
fn error(e: impl std::fmt::Display) -> ControllerError {
    ControllerError::Backend(e.to_string())
}
impl Backend {
    fn check_path(path: &Path, socket: bool) -> Result<(), ControllerError> {
        let m = fs::symlink_metadata(path).map_err(error)?;
        // SAFETY: geteuid has no preconditions.
        if m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o022 != 0
            || m.file_type().is_symlink()
            || (socket && !m.file_type().is_socket())
        {
            return Err(error("unsafe private path"));
        }
        Ok(())
    }
    fn command(&self, args: &[&str]) -> Result<String, ControllerError> {
        let root = &self.manifest.root;
        Self::check_path(root, false)?;
        let socket = root.join("run/libvirt/virtqemud-sock");
        Self::check_path(&socket, true)?;
        let uri = format!(
            "qemu+unix:///session?socket={}&no_autostart=1",
            socket.display()
        );
        let output = Command::new("/usr/bin/timeout")
            .args(["--signal=KILL", "15s"])
            .arg(&self.manifest.virsh)
            .args(["--connect", &uri])
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .env("LD_LIBRARY_PATH", &self.manifest.library)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("LIBVIRT_AUTOSTART", "0")
            .output()
            .map_err(error)?;
        if !output.status.success() {
            return Err(error(String::from_utf8_lossy(&output.stderr)));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
    fn owned(&self, domain: &DomainId) -> Result<(), ControllerError> {
        if self.manifest.domains.contains(&domain.0) {
            Ok(())
        } else {
            Err(error("foreign domain"))
        }
    }
    fn installation(domain: &DomainId) -> InstallationId {
        InstallationId(format!("private-lab-{}", domain.0))
    }
    fn state(&self, domain: &DomainId) -> Result<ControllerState, ControllerError> {
        self.owned(domain)?;
        // UUID lookup is compulsory, never accept a caller's domain name.
        let info = self.command(&["dominfo", &domain.0])?;
        if info
            .lines()
            .find_map(|line| line.strip_prefix("UUID:"))
            .map(str::trim)
            != Some(domain.0.as_str())
        {
            return Err(error("UUID mismatch"));
        }
        match self.command(&["domstate", &domain.0])?.as_str() {
            "running" => Ok(ControllerState::Running),
            "shut off" => Ok(ControllerState::Stopped),
            _ => Err(ControllerError::UnknownObservation),
        }
    }
}
impl LifecycleBackend for Backend {
    fn validate_start(&self, guest: &ObservedGuest) -> Result<(), ControllerError> {
        self.owned(&guest.domain_id)?;
        if guest.installation_id != Self::installation(&guest.domain_id)
            || !guest.resources.is_empty()
        {
            return Err(error(
                "private laboratory refuses foreign identity and physical claims",
            ));
        }
        Ok(())
    }
    fn validate_stop(
        &self,
        installation: &InstallationId,
        domain: &DomainId,
    ) -> Result<(), ControllerError> {
        self.validate_start(&ObservedGuest {
            installation_id: installation.clone(),
            domain_id: domain.clone(),
            resources: vec![],
        })
    }
    fn validate_journal(&self, journal: &MockJournal) -> Result<(), ControllerError> {
        if !journal.reservations.is_empty() {
            return Err(error("no physical reservations in private laboratory"));
        }
        for op in &journal.operations {
            self.validate_start(&ObservedGuest {
                installation_id: op.installation_id.clone(),
                domain_id: op.domain_id.clone(),
                resources: op.requested_resources.clone(),
            })?;
            if !op.acquired_resources.is_empty() {
                return Err(error("physical claims refused"));
            }
        }
        Ok(())
    }
    fn observe(&self) -> BackendObservation {
        let mut result = BackendObservation {
            available: false,
            host: HostObservation::unknown(),
            guests: BTreeMap::new(),
        };
        for uuid in &self.manifest.domains {
            let domain = DomainId(uuid.clone());
            match self.state(&domain) {
                Ok(ControllerState::Running) => {
                    result.guests.insert(
                        domain.clone(),
                        ObservedGuest {
                            installation_id: Self::installation(&domain),
                            domain_id: domain,
                            resources: vec![],
                        },
                    );
                }
                Ok(ControllerState::Stopped) => {}
                other => {
                    eprintln!("private observation failed for {}: {:?}", uuid, other);
                    return result;
                }
            }
        }
        result.available = true;
        result.host = HostObservation::known(vec![]);
        result
    }
    fn domain_state(&self, domain: &DomainId) -> ControllerState {
        self.state(domain).unwrap_or(ControllerState::Unknown)
    }
    fn start(&mut self, guest: ObservedGuest) -> Result<(), ControllerError> {
        self.validate_start(&guest)?;
        let memory = fs::read_to_string("/proc/meminfo").map_err(error)?;
        let available = memory
            .lines()
            .find_map(|l| l.strip_prefix("MemAvailable:"))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| error("unknown memory budget"))?;
        if available / 1024 < 9216 {
            return Err(error(
                "insufficient available memory for private lab reserve",
            ));
        }
        let base = self.manifest.root.join("base.raw");
        Self::check_path(&base, false)?;
        let metadata = fs::symlink_metadata(&base).map_err(error)?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o222 != 0 {
            return Err(error("base must be a private read-only regular file"));
        }
        let digest = Command::new("/usr/bin/sha256sum")
            .arg(&base)
            .output()
            .map_err(error)?;
        if !digest.status.success()
            || String::from_utf8_lossy(&digest.stdout)
                .split_whitespace()
                .next()
                != Some(self.manifest.base_sha256.as_str())
        {
            return Err(error("stale base fingerprint"));
        }
        let disk = self
            .manifest
            .root
            .join(format!("{}.qcow2", guest.domain_id.0));
        Self::check_path(&disk, false)?;
        let metadata = fs::symlink_metadata(&disk).map_err(error)?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(error("private regular overlay required"));
        }
        let listing = self.command(&["domblklist", &guest.domain_id.0, "--details"])?;
        let entries: Vec<_> = listing
            .lines()
            .filter(|l| l.split_whitespace().next() == Some("file"))
            .collect();
        if entries.len() != 1
            || entries[0].split_whitespace().last() != disk.to_str()
            || listing
                .lines()
                .any(|l| l.split_whitespace().next() == Some("block"))
        {
            return Err(error("unexpected disk source"));
        }
        self.command(&["start", &guest.domain_id.0])?;
        Ok(())
    }
    fn stop(&mut self, domain: &DomainId) -> Result<(), ControllerError> {
        self.owned(domain)?;
        if self.state(domain)? != ControllerState::Stopped {
            self.command(&["destroy", &domain.0])?;
        }
        Ok(())
    }
}
/// Run the bounded laboratory worker from its dedicated executable.
/// Crash injection deliberately kills this process; never call from a service.
pub fn run_worker(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // The dedicated lab harness creates this manifest, never accept a URI or XML.
    if args.len() < 4 {
        return Err("manifest, action and UUID required".into());
    }
    if unsafe { libc::geteuid() } == 0 {
        return Err("non-root only".into());
    }
    let path = Path::new(&args[1]);
    Backend::check_path(path, false)?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(path)?)?;
    Backend::check_path(&manifest.root, false)?;
    if fs::canonicalize(&manifest.root)? != manifest.root
        || path.parent() != Some(manifest.root.as_path())
        || !manifest
            .root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .starts_with("elasticpc-private-")
        || manifest.domains.is_empty()
        || manifest.domains.len() > 2
        || manifest
            .domains
            .iter()
            .any(|s| s.len() != 36 || !s.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'))
    {
        return Err("invalid private manifest".into());
    }
    let db = manifest.root.join("controller.sqlite3");
    let mut controller =
        DurableController::open(&db, ResourceTopology::default(), Backend { manifest })?;
    let action = &args[2];
    let domain = DomainId(args[3].clone());
    let installation = Backend::installation(&domain);
    if action == "hold" {
        println!("ready");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        return Ok(());
    }
    let point = args.get(5).map(String::as_str).unwrap_or("");
    let crash = match point {
        "AfterJournal" => Some(CrashPoint::AfterJournal),
        "AfterLeases" => Some(CrashPoint::AfterLeases),
        "AfterStart" => Some(CrashPoint::AfterStart),
        "AfterStopIntent" => Some(CrashPoint::AfterStopIntent),
        "AfterStop" => Some(CrashPoint::AfterStop),
        "AfterRelease" => Some(CrashPoint::AfterRelease),
        _ => None,
    };
    if let Some(point) = crash {
        controller.controller_mut().inject_crash(point);
    }
    let op = args.get(4).map(String::as_str).unwrap_or("unused");
    let result = match action.as_str() {
        "start" => controller
            .controller_mut()
            .start(op, &installation, &domain, &[])
            .map(|s| format!("{s:?}")),
        "stop" => controller
            .controller_mut()
            .stop(op, &installation, &domain)
            .map(|s| format!("{s:?}")),
        "recover" => controller
            .controller_mut()
            .recover()
            .map(|s| format!("{s:?}")),
        _ => return Err("unknown action".into()),
    };
    if matches!(result, Err(ControllerError::SimulatedCrash(_))) {
        // True process death after a durable boundary; no Rust destructors run.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGKILL);
        }
        unreachable!();
    }
    println!(
        "{}",
        json!({"result": result?, "physical_resource_claims_supported": false})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastic_core::{DiskStableId, LeaseResource};
    use elastic_planner::leases::LeaseRequest;
    fn backend() -> Backend {
        Backend {
            manifest: Manifest {
                root: PathBuf::from("/unreachable"),
                virsh: PathBuf::from("/unreachable/virsh"),
                library: PathBuf::from("/unreachable"),
                domains: vec!["12345678-1234-1234-1234-123456789abc".into()],
                base_sha256: "unused".into(),
            },
        }
    }
    #[test]
    fn foreign_identity_and_physical_claims_refused_without_connection() {
        let backend = backend();
        let domain = DomainId(backend.manifest.domains[0].clone());
        let mut guest = ObservedGuest {
            installation_id: Backend::installation(&domain),
            domain_id: domain.clone(),
            resources: vec![],
        };
        assert!(backend.validate_start(&guest).is_ok());
        guest
            .resources
            .push(LeaseRequest::exclusive(LeaseResource::BlockDisk(
                DiskStableId("physical".into()),
            )));
        assert!(backend.validate_start(&guest).is_err());
        assert!(backend
            .validate_stop(&InstallationId("wrong".into()), &domain)
            .is_err());
        assert!(backend
            .validate_stop(
                &Backend::installation(&DomainId("foreign".into())),
                &DomainId("foreign".into())
            )
            .is_err());
    }
    #[test]
    fn inaccessible_socket_stays_unknown() {
        let backend = backend();
        let observation = backend.observe();
        assert!(!observation.available && !observation.host.complete);
        assert_eq!(
            backend.domain_state(&DomainId(backend.manifest.domains[0].clone())),
            ControllerState::Unknown
        );
    }
    #[test]
    fn symlink_private_path_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("link");
        std::os::unix::fs::symlink(directory.path(), &path).unwrap();
        assert!(Backend::check_path(&path, false).is_err());
    }
}
