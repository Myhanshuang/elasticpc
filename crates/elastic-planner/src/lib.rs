//! Pure planning and in-memory simulation. There are no physical executors.
pub mod domain;
pub mod handoff;
pub mod import;
pub mod layout;
pub mod leases;
pub mod mock;
pub mod resources;

pub use domain::*;
pub use handoff::*;
pub use import::*;
pub use layout::*;
pub use resources::*;

use elastic_core::{CheckStatus, Plan, PlannedOperation, Precondition, SCHEMA_VERSION};

fn empty_plan(kind: &str) -> Plan {
    Plan {
        schema_version: SCHEMA_VERSION,
        id: String::new(),
        kind: kind.to_owned(),
        installation_id: None,
        preconditions: Vec::new(),
        affected_devices: Vec::new(),
        operations: Vec::new(),
        requires_approval: false,
        rollback: Vec::new(),
        capability_gaps: Vec::new(),
    }
}

fn condition(plan: &mut Plan, code: &str, status: CheckStatus, detail: impl Into<String>) {
    plan.preconditions.push(Precondition {
        code: code.to_owned(),
        status,
        detail: detail.into(),
    });
}

fn operation(plan: &mut Plan, action: &str, description: impl Into<String>, writes_source: bool) {
    plan.operations.push(PlannedOperation {
        action: action.to_owned(),
        description: description.into(),
        writes_source,
    });
}

fn finish(plan: &mut Plan) {
    plan.affected_devices.sort();
    plan.affected_devices.dedup();
    plan.capability_gaps.sort();
    plan.capability_gaps.dedup();
    plan.refresh_id();
}
