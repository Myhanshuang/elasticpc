use crate::import::{partition_mounted, safe_disk_identity, safe_partition_identity};
use crate::{condition, empty_plan, finish, operation};
use elastic_core::{
    CapabilityStatus, CheckStatus, HardwareReport, LeaseResource, Plan, SCHEMA_VERSION,
};

/// Alternatives are explicitly conditional. No command, partition edit, mount,
/// filesystem probe or UEFI write is performed while constructing this value.
pub fn plan_layout(report: &HardwareReport, host_size_bytes: u64) -> Plan {
    let mut plan = empty_plan("host_layout_alternatives");
    plan.requires_approval = true;
    condition(
        &mut plan,
        "observation_schema",
        if report.schema_version == SCHEMA_VERSION {
            CheckStatus::Satisfied
        } else {
            CheckStatus::Blocked
        },
        format!(
            "Observation schema {}; planner supports {}",
            report.schema_version, SCHEMA_VERSION
        ),
    );
    condition(&mut plan, "requested_host_size",
        if host_size_bytes > 0 { CheckStatus::Satisfied } else { CheckStatus::Blocked },
        format!("Requested Host root capacity: {host_size_bytes} bytes; final image, swap, alignment and EFI overhead require separate measurement"));
    condition(&mut plan, "layout_is_not_filesystem_free_space", CheckStatus::Satisfied,
        "Filesystem free space (including df output) is not unallocated GPT space; only a verified sector layout can admit a new partition");
    condition(&mut plan, "independent_recovery", CheckStatus::NeedsVerification,
        "Verify independent full data backup, GPT/ESP backups, a Live recovery medium, and native Windows/Manjaro boot before any physical change");
    condition(&mut plan, "explicit_layout_choice", CheckStatus::NeedsVerification,
        "Choose exactly one mutually exclusive layout alternative and approve its final sector ranges; this combined review plan cannot be applied");

    let mut disks: Vec<_> = report
        .disks
        .iter()
        .filter(|disk| {
            disk.transport.as_deref() != Some("usb")
                && disk.partitions.iter().any(|partition| {
                    matches!(partition.filesystem.as_deref(), Some("ext4" | "swap"))
                })
        })
        .collect();
    disks.sort_by(|a, b| a.stable_id.cmp(&b.stable_id));
    if disks.is_empty() {
        condition(&mut plan, "candidate_internal_disk", CheckStatus::Blocked,
            "No internal disk with verified ext4/swap candidates was observed; provide a fresh storage inventory");
    }
    for disk in disks {
        let Some(id) = &disk.stable_id else {
            condition(&mut plan, "stable_disk_identity", CheckStatus::Blocked,
                "A candidate internal disk has no stable identity; do not target a kernel device name");
            continue;
        };
        if !safe_disk_identity(&id.0) {
            condition(&mut plan, "stable_disk_identity", CheckStatus::Blocked,
                "Candidate disk identity is invalid or a transient kernel path; use a verified stable ID");
            continue;
        }
        if report
            .disks
            .iter()
            .filter(|candidate| candidate.stable_id.as_ref() == Some(id))
            .count()
            != 1
        {
            condition(&mut plan, "unique_disk", CheckStatus::Blocked,
                format!("Disk stable ID {id} is ambiguous; refresh identity before selecting a layout target"));
        }
        plan.affected_devices
            .push(LeaseResource::BlockDisk(id.clone()));
        condition(&mut plan, "gpt_sector_layout", CheckStatus::NeedsVerification,
            format!("For {id}, verify GPT GUID, logical sector size, exact start/end sectors, alignment and contiguous free extents"));
        match disk.unallocated_bytes {
            Some(bytes) if host_size_bytes > 0 && bytes >= host_size_bytes => {
                operation(&mut plan, "alternative_existing_unallocated",
                    format!("For {id}, {bytes} unallocated bytes were reported; only after proving a sufficiently large contiguous extent, consider a {host_size_bytes}-byte Host root without shrinking existing data"), true);
            }
            Some(bytes) => {
                condition(&mut plan, "unallocated_capacity", CheckStatus::NeedsVerification,
                    format!("For {id}, only {bytes} unallocated bytes are observed, below the requested {host_size_bytes}; choose a separate storage-layout alternative"));
            }
            None => {
                condition(&mut plan, "unallocated_capacity", CheckStatus::NeedsVerification,
                    format!("For {id}, unallocated capacity is unknown; no capacity is inferred from filesystem free space or rounded partition sizes"));
            }
        }

        let mut partitions: Vec<_> = disk.partitions.iter().collect();
        partitions.sort_by(|a, b| a.partuuid.cmp(&b.partuuid));
        for partition in partitions {
            let Some(partuuid) = &partition.partuuid else {
                condition(
                    &mut plan,
                    "stable_partition_identity",
                    CheckStatus::Blocked,
                    format!(
                        "A partition on {id} has no PARTUUID; no mutation target can be selected"
                    ),
                );
                continue;
            };
            if !safe_partition_identity(&partuuid.0) {
                condition(
                    &mut plan,
                    "stable_partition_identity",
                    CheckStatus::Blocked,
                    "Invalid PARTUUID cannot select a layout mutation target",
                );
                continue;
            }
            if report
                .disks
                .iter()
                .flat_map(|candidate| &candidate.partitions)
                .filter(|candidate| candidate.partuuid.as_ref() == Some(partuuid))
                .count()
                != 1
            {
                condition(&mut plan, "unique_partition", CheckStatus::Blocked,
                    format!("PARTUUID {partuuid} is ambiguous; no filesystem or GPT mutation may target it"));
            }
            match partition.filesystem.as_deref() {
                Some("swap") => {
                    plan.affected_devices
                        .push(LeaseResource::BlockPartition(partuuid.clone()));
                    let enough = host_size_bytes > 0 && partition.size_bytes >= host_size_bytes;
                    condition(&mut plan, "alternative_a_swap_capacity",
                        if enough { CheckStatus::Satisfied } else { CheckStatus::Blocked },
                        format!("PARTUUID {partuuid}: swap region is {} bytes; Host root needs {host_size_bytes} bytes before alignment and any retained swap", partition.size_bytes));
                    let resume_verified = report.resume.status == CapabilityStatus::Supported
                        && report.resume.references.is_empty();
                    condition(&mut plan, "alternative_a_resume",
                        if resume_verified { CheckStatus::Satisfied } else { CheckStatus::NeedsVerification },
                        "Check fstab, current and configured kernel resume/resume_offset arguments, initramfs, bootloader profiles and hibernation use; absence from the current command line alone is insufficient");
                    condition(&mut plan, "alternative_a_swap_inactive",
                        if partition.active_swap { CheckStatus::Blocked } else { CheckStatus::NeedsVerification },
                        format!("PARTUUID {partuuid} must be inactive and unheld in the approved offline environment; revalidate swap state before editing"));
                    operation(&mut plan, "alternative_a_repurpose_swap",
                        format!("Conditional Plan A: after user accepts reduced/removed disk swap and no hibernation requirement is proven, repartition approved PARTUUID {partuuid} space for a {host_size_bytes}-byte Host root plus feasible retained swap"), true);
                    operation(&mut plan, "alternative_a_update_resume",
                        "Conditional Plan A: update Manjaro fstab, all resume references and initramfs consistently; verify native boot and hibernation policy from recovery media", true);
                }
                Some("ext4") => {
                    plan.affected_devices
                        .push(LeaseResource::BlockPartition(partuuid.clone()));
                    condition(&mut plan, "alternative_b_offline_ext4",
                        if partition_mounted(report, partition) { CheckStatus::Blocked } else { CheckStatus::NeedsVerification },
                        format!("PARTUUID {partuuid}: shrink ext4 only from a Live/offline environment while unmounted; perform filesystem checks and measure minimum size plus operating headroom"));
                    condition(&mut plan, "alternative_b_capacity", CheckStatus::NeedsVerification,
                        "Measure real ext4 used/minimum size, metadata and future operating headroom; clean/transfer data before approving exact filesystem and partition sizes");
                    operation(&mut plan, "alternative_b_offline_shrink",
                        format!("Conditional Plan B: if full swap must remain, inventory services on PARTUUID {partuuid}, migrate/clean data, check filesystem offline, shrink ext4 first, then reduce its GPT extent without changing its starting sector and reserve aligned Host space"), true);
                }
                _ => {}
            }
        }
    }
    condition(&mut plan, "efi_capacity", CheckStatus::NeedsVerification,
        "Measure actual Host boot-file requirements and ESP free bytes; a nominal ESP size cannot prove capacity, and a separate Host ESP may be necessary");
    operation(&mut plan, "preserve_native_efi",
        "After one layout alternative is approved, use a separate \\EFI\\ElasticPC\\ directory and independent boot entry; preserve all existing Windows/Manjaro entries and files, including entries absent from BootOrder", true);
    plan.rollback.extend([
        "Use the verified recovery medium and independent backups; restoring GPT alone does not restore an already-shrunk or overwritten filesystem".into(),
        "Restore approved filesystem/data and boot metadata together with the matching partition map; verify both native operating systems independently".into(),
        "Remove only ElasticPC-owned files/entry after verification; preserve original native boot entries and default order".into(),
    ]);
    plan.capability_gaps.extend([
        "Exact sector ranges, filesystem shrink feasibility, Host image/EFI footprint and recovery rehearsal are not yet measured".into(),
        "Physical partition, filesystem and UEFI mutation is outside P0/P1; no executor is provided".into(),
    ]);
    finish(&mut plan);
    plan
}
