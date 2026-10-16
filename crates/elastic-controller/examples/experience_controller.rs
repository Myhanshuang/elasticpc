//! File-only preview adapter for the existing durable lifecycle controller.
use elastic_controller::DurableController;
use elastic_core::{DomainId, InstallationId};
use elastic_planner::leases::{HostObservation, ResourceTopology};
use elastic_planner::mock::{
    BackendObservation, ControllerError, ControllerState, CrashPoint, LifecycleBackend,
    MockJournal, ObservedGuest,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

struct Backend {
    root: PathBuf,
    id: String,
    force: bool,
}
fn error(value: impl std::fmt::Display) -> ControllerError {
    ControllerError::Backend(value.to_string())
}
impl Backend {
    fn request(&self, action: &str) -> Result<Value, ControllerError> {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/experience.py");
        let output = Command::new("/usr/bin/timeout")
            .args(["--signal=KILL", "45s", "/usr/bin/python3"])
            .arg(script)
            .args(["_backend", "--directory"])
            .arg(&self.root)
            .args(["--backend-action", action])
            .output()
            .map_err(error)?;
        if !output.status.success() {
            return Err(error(String::from_utf8_lossy(&output.stdout)));
        }
        serde_json::from_slice(&output.stdout).map_err(error)
    }
    fn guest(&self) -> ObservedGuest {
        ObservedGuest {
            domain_id: DomainId(self.id.clone()),
            installation_id: InstallationId(format!("preview-{}", self.id)),
            resources: vec![],
        }
    }
    fn owned(&self, domain: &DomainId) -> Result<(), ControllerError> {
        if domain.0 == self.id {
            Ok(())
        } else {
            Err(error("Foreign domain refused"))
        }
    }
}
impl LifecycleBackend for Backend {
    fn validate_start(&self, guest: &ObservedGuest) -> Result<(), ControllerError> {
        self.owned(&guest.domain_id)?;
        if guest.installation_id != self.guest().installation_id || !guest.resources.is_empty() {
            return Err(error(
                "Physical resources and foreign installations refused",
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
            return Err(error("Physical reservations refused"));
        }
        for op in &journal.operations {
            self.validate_start(&ObservedGuest {
                installation_id: op.installation_id.clone(),
                domain_id: op.domain_id.clone(),
                resources: op.requested_resources.clone(),
            })?;
            if !op.acquired_resources.is_empty() {
                return Err(error("Physical leases refused"));
            }
        }
        Ok(())
    }
    fn domain_state(&self, domain: &DomainId) -> ControllerState {
        if self.owned(domain).is_err() {
            return ControllerState::Unknown;
        }
        match self
            .request("status")
            .ok()
            .and_then(|v| v["state"].as_str().map(str::to_owned))
            .as_deref()
        {
            Some("Running") => ControllerState::Running,
            Some("Stopped") => ControllerState::Stopped,
            _ => ControllerState::Unknown,
        }
    }
    fn observe(&self) -> BackendObservation {
        let guest = self.guest();
        let mut guests = BTreeMap::new();
        let state = self.domain_state(&guest.domain_id);
        let available = matches!(state, ControllerState::Running | ControllerState::Stopped);
        if state == ControllerState::Running {
            guests.insert(guest.domain_id.clone(), guest);
        }
        BackendObservation {
            available,
            host: if available {
                HostObservation::known(vec![])
            } else {
                HostObservation::unknown()
            },
            guests,
        }
    }
    fn start(&mut self, guest: ObservedGuest) -> Result<(), ControllerError> {
        self.validate_start(&guest)?;
        self.request("start")?;
        Ok(())
    }
    fn stop(&mut self, domain: &DomainId) -> Result<(), ControllerError> {
        self.owned(domain)?;
        self.request(if self.force { "force" } else { "stop" })?;
        Ok(())
    }
}
fn crash_exit<T>(result: Result<T, ControllerError>) -> Result<T, ControllerError> {
    if matches!(&result, Err(ControllerError::SimulatedCrash(_))) {
        // SAFETY: explicit lab injection targets only this worker process.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGKILL);
        }
        unreachable!();
    }
    result
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 4 {
        return Err("directory, action, operation id required".into());
    }
    let root = std::fs::canonicalize(&args[1])?;
    let manifest: Value = serde_json::from_slice(&std::fs::read(root.join("manifest.json"))?)?;
    let id = manifest["trial_id"]
        .as_str()
        .ok_or("missing Trial identity")?
        .to_owned();
    let backend = Backend {
        root: root.clone(),
        id,
        force: args.get(4).is_some_and(|s| s == "force"),
    };
    let guest = backend.guest();
    let mut durable = DurableController::open(
        &root.join("controller.sqlite3"),
        ResourceTopology::default(),
        backend,
    )?;
    let controller = durable.controller_mut();
    match args.get(5).map(String::as_str) {
        Some("AfterStart") => controller.inject_crash(CrashPoint::AfterStart),
        Some("AfterStop") => controller.inject_crash(CrashPoint::AfterStop),
        Some(_) => return Err("Unknown crash injection".into()),
        None => {}
    }
    let result = match args[2].as_str() {
        "start" => {
            controller.recover()?;
            format!(
                "{:?}",
                crash_exit(controller.start(
                    &args[3],
                    &guest.installation_id,
                    &guest.domain_id,
                    &[]
                ))?
            )
        }
        "stop" => {
            controller.recover()?;
            format!(
                "{:?}",
                crash_exit(controller.stop(&args[3], &guest.installation_id, &guest.domain_id))?
            )
        }
        "recover" => format!("{:?}", controller.recover()?),
        "status" => format!("{:?}", controller.state(&guest.domain_id)),
        _ => return Err("unknown action".into()),
    };
    println!(
        "{}",
        json!({"result":result,"trial_id":guest.domain_id.0,"journal_operations":controller.journal().operations.len(),"scope":"private-file-preview"})
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foreign_identity_is_rejected_before_backend_call() {
        let backend = Backend {
            root: PathBuf::from("/unused"),
            id: "owned".into(),
            force: false,
        };
        let mut guest = backend.guest();
        guest.domain_id = DomainId("foreign".into());
        assert!(backend.validate_start(&guest).is_err());
        assert_eq!(
            backend.domain_state(&guest.domain_id),
            ControllerState::Unknown
        );
        guest = backend.guest();
        guest.installation_id = InstallationId("foreign".into());
        assert!(backend.validate_start(&guest).is_err());
    }
}
