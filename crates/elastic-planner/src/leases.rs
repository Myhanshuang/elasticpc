//! Pure resource arbitration. This module never opens a device or changes a mount.
use elastic_core::{DiskStableId, DomainId, LeaseResource, PartUuid};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseMode {
    SharedRead,
    ExclusiveWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub resource: LeaseResource,
    pub mode: LeaseMode,
}

impl LeaseRequest {
    pub fn exclusive(resource: LeaseResource) -> Self {
        Self {
            resource,
            mode: LeaseMode::ExclusiveWrite,
        }
    }

    pub fn read_only(resource: LeaseResource) -> Self {
        Self {
            resource,
            mode: LeaseMode::SharedRead,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub owner: DomainId,
    pub request: LeaseRequest,
}

/// An explicit ancestry table is required: unknown PARTUUIDs cannot be assumed
/// unrelated to a whole-disk reservation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTopology {
    disks: BTreeSet<DiskStableId>,
    partition_disks: BTreeMap<PartUuid, DiskStableId>,
    ambiguous_partitions: BTreeSet<PartUuid>,
    ambiguous_disks: BTreeSet<DiskStableId>,
}

impl ResourceTopology {
    pub fn register_disk(&mut self, disk: DiskStableId) {
        self.disks.insert(disk);
    }

    pub fn register_partition(&mut self, partition: PartUuid, disk: DiskStableId) {
        self.disks.insert(disk.clone());
        if let Some(previous) = self.partition_disks.get(&partition) {
            if previous != &disk || self.ambiguous_partitions.contains(&partition) {
                self.ambiguous_partitions.insert(partition);
                self.ambiguous_disks.insert(previous.clone());
                self.ambiguous_disks.insert(disk);
            }
        } else {
            self.partition_disks.insert(partition, disk);
        }
    }

    fn is_known(&self, resource: &LeaseResource) -> bool {
        match resource {
            LeaseResource::BlockDisk(disk) => {
                !disk.0.trim().is_empty()
                    && crate::import::safe_disk_identity(&disk.0)
                    && self.disks.contains(disk)
                    && !self.ambiguous_disks.contains(disk)
            }
            LeaseResource::BlockPartition(partition) => {
                !partition.0.trim().is_empty()
                    && crate::import::safe_partition_identity(&partition.0)
                    && !self.ambiguous_partitions.contains(partition)
                    && self.partition_disks.get(partition).is_some_and(|disk| {
                        !disk.0.trim().is_empty()
                            && crate::import::safe_disk_identity(&disk.0)
                            && !self.ambiguous_disks.contains(disk)
                    })
            }
            LeaseResource::PciIommuGroup(id) => !id.trim().is_empty(),
            LeaseResource::UsbDevice(id) => !id.0.trim().is_empty(),
            LeaseResource::DisplayConnector(id) => !id.0.trim().is_empty(),
        }
    }

    pub fn overlaps(&self, left: &LeaseResource, right: &LeaseResource) -> bool {
        if left == right {
            return true;
        }
        match (left, right) {
            (LeaseResource::BlockDisk(disk), LeaseResource::BlockPartition(partition))
            | (LeaseResource::BlockPartition(partition), LeaseResource::BlockDisk(disk)) => {
                // Missing ancestry is a possible overlap, never proof of isolation.
                self.ambiguous_partitions.contains(partition)
                    || self
                        .partition_disks
                        .get(partition)
                        .is_none_or(|parent| parent == disk)
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostClaimKind {
    MountReadOnly,
    MountReadWrite,
    ActiveSwap,
    HostRoot,
    OtherExclusive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostClaim {
    pub resource: LeaseResource,
    pub kind: HostClaimKind,
}

impl HostClaim {
    fn mode(&self) -> LeaseMode {
        match self.kind {
            HostClaimKind::MountReadOnly => LeaseMode::SharedRead,
            _ => LeaseMode::ExclusiveWrite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostObservation {
    /// False if mount, swap, or other resource occupancy could not be read.
    pub complete: bool,
    pub claims: Vec<HostClaim>,
}

impl HostObservation {
    pub fn known(claims: Vec<HostClaim>) -> Self {
        Self {
            complete: true,
            claims,
        }
    }
    pub fn unknown() -> Self {
        Self {
            complete: false,
            claims: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum LeaseError {
    ObservationIncomplete,
    UnknownResource {
        resource: LeaseResource,
    },
    HostConflict {
        requested: LeaseResource,
        occupied: LeaseResource,
        kind: HostClaimKind,
    },
    GuestConflict {
        requested: LeaseResource,
        occupied: LeaseResource,
        owner: DomainId,
    },
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for LeaseError {}

#[derive(Debug, Clone)]
pub struct LeaseManager {
    topology: ResourceTopology,
    host: HostObservation,
    leases: Vec<Lease>,
    guest_observation_complete: bool,
}

fn modes_conflict(left: LeaseMode, right: LeaseMode, resource: &LeaseResource) -> bool {
    // Shared-read applies only to block data. Device ownership is always exclusive.
    !matches!(
        resource,
        LeaseResource::BlockDisk(_) | LeaseResource::BlockPartition(_)
    ) || left == LeaseMode::ExclusiveWrite
        || right == LeaseMode::ExclusiveWrite
}

impl LeaseManager {
    pub fn new(topology: ResourceTopology, host: HostObservation) -> Self {
        Self {
            topology,
            host,
            leases: Vec::new(),
            guest_observation_complete: true,
        }
    }

    pub fn leases(&self) -> &[Lease] {
        &self.leases
    }
    pub fn host_observation(&self) -> &HostObservation {
        &self.host
    }

    pub fn for_domain(&self, owner: &DomainId) -> Vec<LeaseRequest> {
        self.leases
            .iter()
            .filter(|lease| &lease.owner == owner)
            .map(|lease| lease.request.clone())
            .collect()
    }

    /// Check the entire batch before committing any reservation. An idempotent
    /// reacquisition cannot downgrade an existing exclusive reservation.
    pub fn acquire(
        &mut self,
        owner: &DomainId,
        requests: &[LeaseRequest],
    ) -> Result<(), LeaseError> {
        if !self.host.complete || !self.guest_observation_complete {
            return Err(LeaseError::ObservationIncomplete);
        }
        for request in requests {
            if !self.topology.is_known(&request.resource) {
                return Err(LeaseError::UnknownResource {
                    resource: request.resource.clone(),
                });
            }
            for claim in &self.host.claims {
                if self.topology.overlaps(&request.resource, &claim.resource)
                    && modes_conflict(request.mode, claim.mode(), &request.resource)
                {
                    return Err(LeaseError::HostConflict {
                        requested: request.resource.clone(),
                        occupied: claim.resource.clone(),
                        kind: claim.kind.clone(),
                    });
                }
            }
            for lease in &self.leases {
                if &lease.owner != owner
                    && self
                        .topology
                        .overlaps(&request.resource, &lease.request.resource)
                    && modes_conflict(request.mode, lease.request.mode, &request.resource)
                {
                    return Err(LeaseError::GuestConflict {
                        requested: request.resource.clone(),
                        occupied: lease.request.resource.clone(),
                        owner: lease.owner.clone(),
                    });
                }
            }
        }
        for request in requests {
            self.reserve_observed(owner, request);
        }
        Ok(())
    }

    /// Caller must independently establish that all guest I/O has stopped.
    /// MockController only reaches this after observing an absent/stopped guest.
    pub fn release_after_io_stopped(&mut self, owner: &DomainId) {
        self.leases.retain(|lease| &lease.owner != owner);
    }

    pub(crate) fn reserve_observed(&mut self, owner: &DomainId, request: &LeaseRequest) {
        if let Some(lease) = self
            .leases
            .iter_mut()
            .find(|lease| &lease.owner == owner && lease.request.resource == request.resource)
        {
            if request.mode == LeaseMode::ExclusiveWrite {
                lease.request.mode = request.mode;
            }
        } else {
            self.leases.push(Lease {
                owner: owner.clone(),
                request: request.clone(),
            });
        }
    }

    pub(crate) fn mark_observation_unknown(&mut self) {
        self.guest_observation_complete = false;
    }

    pub(crate) fn replace_observations(&mut self, host: HostObservation, observed: Vec<Lease>) {
        self.host = host;
        self.leases = observed;
        self.guest_observation_complete = true;
    }
}
