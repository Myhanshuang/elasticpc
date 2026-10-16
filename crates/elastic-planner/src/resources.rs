//! Conservative memory admission for a fresh start, without ballooning assumptions.
use serde::{Deserialize, Serialize};

pub const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRequest {
    pub domain_id: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcePlan {
    pub schema_version: u32,
    pub requests: Vec<MemoryRequest>,
    pub host_reserve_bytes: u64,
    pub guest_total_bytes: Option<u64>,
    pub admitted: bool,
    pub preconditions: Vec<String>,
    pub affected_devices: Vec<String>,
    pub operations: Vec<String>,
    pub requires_approval: bool,
    pub rollback: Vec<String>,
    pub capability_gaps: Vec<String>,
}

/// `available` is a fresh MemAvailable observation. Requests cover *new* guest
/// allocations, including service domains; already allocated RAM is not counted
/// as reclaimable. Pausing a guest is not treated as releasing its RAM.
pub fn plan_resources(
    total: Option<u64>,
    available: Option<u64>,
    requests: &[MemoryRequest],
    host_reserve_bytes: u64,
) -> ResourcePlan {
    let mut requests = requests.to_vec();
    requests.sort_by(|a, b| a.domain_id.cmp(&b.domain_id));
    let guest_total = requests
        .iter()
        .try_fold(0_u64, |sum, request| sum.checked_add(request.bytes));
    let mut gaps = Vec::new();
    if host_reserve_bytes == 0 {
        gaps.push("A nonzero Host/iGPU/I/O memory reserve is required".to_owned());
    }
    if requests
        .iter()
        .any(|request| request.bytes == 0 || request.domain_id.is_empty())
    {
        gaps.push("Each domain requires a nonempty identity and nonzero memory".to_owned());
    }
    if requests
        .windows(2)
        .any(|pair| pair[0].domain_id == pair[1].domain_id)
    {
        gaps.push("Duplicate domain identities in the memory request".to_owned());
    }
    let required = guest_total.and_then(|value| value.checked_add(host_reserve_bytes));
    match (total, available, required) {
        (_, _, None) => gaps.push("Memory arithmetic overflow; allocation is denied".to_owned()),
        (None, _, _) | (_, None, _) => {
            gaps.push("Physical total and current MemAvailable must both be observed".to_owned());
        }
        (Some(total), Some(available), Some(required)) => {
            if available > total {
                gaps.push(
                    "MemAvailable exceeds physical total; refresh inconsistent observations"
                        .to_owned(),
                );
            }
            if required > total {
                gaps.push("Guest allocations plus Host reserve exceed physical RAM".to_owned());
            }
            if required > available {
                gaps.push(
                    "Fresh MemAvailable cannot cover guest allocations plus Host reserve"
                        .to_owned(),
                );
            }
        }
    }
    ResourcePlan {
        schema_version: 1,
        requests,
        host_reserve_bytes,
        guest_total_bytes: guest_total,
        admitted: gaps.is_empty(),
        preconditions: vec![
            "Refresh MemAvailable and reconcile existing domains before allocation".to_owned(),
            "Include service-domain RAM and guest/device overhead in the reservation".to_owned(),
            "Do not rely on ballooning or paused guests to free passthrough guest memory"
                .to_owned(),
        ],
        affected_devices: Vec::new(),
        operations: vec!["Reserve requested memory only after admission is revalidated".to_owned()],
        requires_approval: false,
        rollback: vec![
            "Release only uncommitted reservations; never stop unrelated domains".to_owned(),
        ],
        capability_gaps: gaps,
    }
}
