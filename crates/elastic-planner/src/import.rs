use crate::{condition, empty_plan, finish, operation};
use elastic_core::{
    CheckStatus, Disk, HardwareReport, Installation, InstallationSource, LeaseResource, OsFamily,
    Partition, Plan, SourceFingerprint, StoragePolicy, SCHEMA_VERSION,
};

/// Refuse stale overlays even when capacity and GPT topology have not changed.
/// The scanner must supply a digest of its documented critical regions and a
/// trusted content generation; a topology-only scan cannot admit a resume.
pub fn validate_overlay_fingerprint(
    expected: &SourceFingerprint,
    current: &SourceFingerprint,
) -> Result<(), String> {
    fingerprint_complete(expected)?;
    fingerprint_complete(current)?;
    let mut expected = expected.clone();
    let mut current = current.clone();
    expected.partuuids.sort();
    current.partuuids.sort();
    if expected != current {
        return Err("Base fingerprint changed; discard/recreate the overlay and reject saved-memory restore".into());
    }
    Ok(())
}

fn fingerprint_complete(fingerprint: &SourceFingerprint) -> Result<(), String> {
    let hash_present = fingerprint
        .content_hash
        .as_ref()
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let identity_present = fingerprint
        .gpt_guid
        .as_ref()
        .is_some_and(|id| !id.is_empty())
        && !fingerprint.partuuids.is_empty()
        && fingerprint.partuuids.iter().all(|id| !id.0.is_empty());
    if fingerprint.size_bytes == 0
        || !identity_present
        || !hash_present
        || !fingerprint
            .generation
            .as_ref()
            .is_some_and(|generation| !generation.is_empty())
    {
        return Err("Insufficient base fingerprint: require size, GPT GUID, PARTUUIDs, critical-region SHA-256 and trusted content generation".into());
    }
    Ok(())
}

/// This model never opens a block device or allocates an overlay. Preconditions
/// apply again at execution time; a plan is not evidence that a lease is held.
pub fn plan_import(
    report: &HardwareReport,
    installation: &Installation,
    policy: StoragePolicy,
) -> Plan {
    let mut plan = empty_plan(match policy {
        StoragePolicy::InspectReadOnly => "import_inspect",
        StoragePolicy::TrialOverlay { .. } => "import_trial",
        StoragePolicy::NativeBackedExclusive => "import_native_backed",
    });
    plan.installation_id = Some(installation.id.clone());
    let writable_guest = !matches!(policy, StoragePolicy::InspectReadOnly);
    plan.requires_approval = writable_guest;
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
    condition(&mut plan, "refresh_identity", CheckStatus::NeedsVerification,
        "Immediately before access, rediscover stable identity, capacity, GPT GUID and PARTUUIDs; never reuse kernel device paths");
    let mut source = installation.source.clone();
    if let InstallationSource::PhysicalPartitions(ids) = &mut source {
        ids.sort();
        ids.dedup();
    }
    let source_json = serde_json::to_string(&source).expect("serializable installation source");
    condition(
        &mut plan,
        "source_identity",
        CheckStatus::Satisfied,
        source_json,
    );
    let mut fingerprint = installation.fingerprint.clone();
    fingerprint.partuuids.sort();
    condition(
        &mut plan,
        "base_fingerprint",
        CheckStatus::Satisfied,
        serde_json::to_string(&fingerprint).expect("serializable source fingerprint"),
    );

    let mut selected = Vec::new();
    match &installation.source {
        InstallationSource::PhysicalDisk(id) => {
            plan.affected_devices
                .push(LeaseResource::BlockDisk(id.clone()));
            let matches: Vec<_> = report
                .disks
                .iter()
                .filter(|disk| disk.stable_id.as_ref() == Some(id))
                .collect();
            if !safe_disk_identity(&id.0) || matches.len() != 1 {
                condition(
                    &mut plan,
                    "unique_disk",
                    CheckStatus::Blocked,
                    "A single present disk with a stable identity is required",
                );
            } else {
                let disk = matches[0];
                if writable_guest && disk_has_host_root(report, disk) {
                    condition(&mut plan, "host_disk_isolation", CheckStatus::Blocked,
                        "A disk containing the Host root must never be exposed as a writable whole-disk guest source; select approved partitions");
                }
                if writable_guest && disk_mounted(report, disk) {
                    condition(&mut plan, "source_unmounted", CheckStatus::Blocked, "The selected whole disk or a descendant partition is mounted or active swap");
                }
                selected.push(disk);
            }
        }
        InstallationSource::PhysicalPartitions(ids) => {
            if ids.is_empty() {
                condition(
                    &mut plan,
                    "partition_selection",
                    CheckStatus::Blocked,
                    "No physical partitions were selected",
                );
            }
            let mut ids = ids.clone();
            ids.sort();
            ids.dedup();
            for id in ids {
                plan.affected_devices
                    .push(LeaseResource::BlockPartition(id.clone()));
                let matches: Vec<_> = report
                    .disks
                    .iter()
                    .flat_map(|disk| {
                        disk.partitions
                            .iter()
                            .filter(|part| part.partuuid.as_ref() == Some(&id))
                            .map(move |part| (disk, part))
                    })
                    .collect();
                if !safe_partition_identity(&id.0) || matches.len() != 1 {
                    condition(
                        &mut plan,
                        "unique_partition",
                        CheckStatus::Blocked,
                        format!("PARTUUID {id} must identify one present partition"),
                    );
                } else {
                    let (disk, part) = matches[0];
                    if writable_guest
                        && (partition_mounted(report, part) || disk_directly_mounted(report, disk))
                    {
                        condition(&mut plan, "source_unmounted", CheckStatus::Blocked,
                            format!("PARTUUID {id} or its parent disk is mounted or active swap; native and managed access must be exclusive"));
                    }
                    selected.push(disk);
                }
            }
        }
        InstallationSource::Image(_) => {
            condition(&mut plan, "image_admission", CheckStatus::NeedsVerification,
                "Verify an owned regular-file image, explicit format, backing chain and file-level lock; image execution is not implemented");
            plan.capability_gaps
                .push("Image-source provenance and file executor are not implemented".into());
        }
        InstallationSource::Unknown => {
            condition(
                &mut plan,
                "source_identity",
                CheckStatus::Blocked,
                "No verified stable storage identity exists",
            );
        }
    }
    if writable_guest {
        condition(&mut plan, "complete_occupancy_observation",
            if report.storage_observation_complete { CheckStatus::Satisfied } else { CheckStatus::Blocked },
            "Mount, swap, kernel-holder, other namespace and hypervisor ownership must be known; missing observations deny writable guest access");
        condition(&mut plan, "exclusive_lease", CheckStatus::NeedsVerification,
            "Reconcile actual Host/VM/process/kernel ownership and acquire hierarchy-aware exclusive leases before attaching a base or overlay");
    }
    if selected
        .iter()
        .any(|disk| disk.partition_table.as_deref() != Some("gpt") || disk.gpt_guid.is_none())
    {
        condition(
            &mut plan,
            "gpt_identity",
            CheckStatus::NeedsVerification,
            "Verify GPT topology and GUID before importing physical storage",
        );
    }
    if installation.family == OsFamily::Unknown && writable_guest {
        condition(&mut plan, "installation_identity", CheckStatus::Blocked,
            "OS identity is unknown; read Windows/System32 + BCD or Linux os-release + fstab/initramfs evidence before choosing a boot adapter");
    }

    match &policy {
        StoragePolicy::InspectReadOnly => {
            operation(&mut plan, "inspect_read_only", "Collect filesystem and boot evidence using read-only handles with no repair, replay or automatic mount", false);
            plan.rollback.push(
                "Close read-only inspection handles; leave EFI and disk contents unchanged".into(),
            );
        }
        StoragePolicy::TrialOverlay { overlay_id } => {
            if overlay_id.is_empty() {
                condition(
                    &mut plan,
                    "overlay_identity",
                    CheckStatus::Blocked,
                    "An explicit nonempty overlay identity is required",
                );
            }
            if let Err(message) = fingerprint_complete(&installation.fingerprint) {
                condition(
                    &mut plan,
                    "overlay_fingerprint",
                    CheckStatus::NeedsVerification,
                    message,
                );
            }
            operation(&mut plan, "fingerprint_base", "Hash documented GPT/ESP/boot-critical regions and record a trusted content generation before access; a topology digest is insufficient", false);
            operation(&mut plan, "create_trial_overlay", format!("Prepare independent qcow2 overlay {overlay_id:?} with explicit raw read-only physical backing; validate raw block behavior on owned file/loop fixtures first"), false);
            operation(&mut plan, "trial_boot", "Guest writes go only to the overlay; open base read-only and verify critical-region hashes after guest shutdown", false);
            condition(&mut plan, "overlay_backend_validation", CheckStatus::NeedsVerification,
                "Validate explicit backing formats, read-only backing enforcement, base immutability and shutdown on owned files before real devices");
            plan.rollback.extend([
                "Stop the trial domain and verify all I/O has ceased before releasing leases".into(),
                "Preserve diagnostic overlay separately or discard it; never commit/rebase its writes into the physical base automatically".into(),
                "Reject an old overlay and any saved memory if the base fingerprint/generation changed in Native mode".into(),
            ]);
        }
        StoragePolicy::NativeBackedExclusive => {
            condition(&mut plan, "native_recovery_backup", CheckStatus::NeedsVerification,
                "Verify restorable GPT + ESP + BCD/boot backups and a tested native boot recovery path before the first physical write");
            condition(&mut plan, "cold_start", CheckStatus::NeedsVerification,
                "Fully shut down the OS, flush block caches, release overlay sessions and reject saved-memory state from a different base generation");
            operation(&mut plan, "backup_native_boot", "Prepare and verify restorable GPT, ESP and OS boot metadata backups on independent storage", false);
            operation(&mut plan, "attach_native_exclusive", "After explicit approval and revalidation, expose only approved sources with exclusive write leases", true);
            operation(&mut plan, "verify_native_recovery", "After guest shutdown and cache flush, compare boot metadata and verify the preserved native recovery route", false);
            plan.rollback.extend([
                "Stop the guest and verify I/O cessation before releasing physical storage leases".into(),
                "Use a tested offline recovery environment to restore approved boot metadata; GPT/ESP backups do not undo arbitrary OS/user-data writes".into(),
                "Select the preserved original native UEFI entry; do not rewrite its boot files or BootOrder automatically".into(),
            ]);
        }
    }
    if writable_guest {
        operation(
            &mut plan,
            "private_guest_firmware",
            "Plan independent persistent OVMF variable storage; never map Host NVRAM into a guest",
            false,
        );
        match installation.family {
            OsFamily::Windows => {
                operation(&mut plan, "windows_boot_compatibility", "Verify actual Windows OS/EFI/BCD relationship; try compatible SATA/USB controller before VirtIO, with separate persistent vTPM", false);
                plan.capability_gaps.extend([
                    "Windows BitLocker/Device Encryption recovery, driver and activation behavior require guest verification".into(),
                    "T7 block mapping and full USB passthrough differ; no Windows To Go compatibility guarantee".into(),
                ]);
            }
            OsFamily::Linux => {
                operation(&mut plan, "linux_boot_bridge", "Plan a separate guest EFI/boot disk, update-aware kernel/initramfs bridge, private swap and reversible guest fstab/resume handling", false);
                plan.capability_gaps.push("Guest EFI Boot Bridge and kernel/initramfs update tracking are not implemented; an ext4 partition alone is not a bootable disk".into());
                plan.capability_gaps.push("Existing k3s/PV/MCP service data must be inventoried and migrated to separate service storage before concurrent guest writes".into());
            }
            OsFamily::Unknown => {}
        }
        plan.capability_gaps.push("P0/P1 produces plans only; physical-device apply and real libvirt execution are unavailable".into());
    }
    finish(&mut plan);
    plan
}

pub(crate) fn partition_mounted(report: &HardwareReport, part: &Partition) -> bool {
    part.active_swap
        || !part.mountpoints.is_empty()
        || report.mounts.iter().any(|mount| {
            mount.source == part.path
                || (part.major_minor.is_some() && part.major_minor == mount.major_minor)
                || part.partuuid.as_ref().is_some_and(|id| {
                    mount.source == format!("/dev/disk/by-partuuid/{id}")
                        || mount.source == format!("PARTUUID={id}")
                })
                || part.uuid.as_ref().is_some_and(|uuid| {
                    mount.source == format!("/dev/disk/by-uuid/{uuid}")
                        || mount.source == format!("UUID={uuid}")
                })
        })
}

pub(crate) fn disk_mounted(report: &HardwareReport, disk: &Disk) -> bool {
    disk_directly_mounted(report, disk)
        || disk
            .partitions
            .iter()
            .any(|part| partition_mounted(report, part))
}

fn disk_directly_mounted(report: &HardwareReport, disk: &Disk) -> bool {
    !disk.mountpoints.is_empty()
        || report.mounts.iter().any(|mount| {
            mount.source == disk.path
                || disk
                    .stable_id
                    .as_ref()
                    .is_some_and(|id| mount.source == id.0)
                || (disk.major_minor.is_some() && disk.major_minor == mount.major_minor)
        })
}

pub(crate) fn safe_disk_identity(id: &str) -> bool {
    !id.is_empty()
        && !id.chars().any(char::is_control)
        && (if let Some(alias) = id.strip_prefix("/dev/disk/by-id/") {
            safe_partition_identity(alias)
        } else {
            !id.contains('/')
        })
}

pub(crate) fn safe_partition_identity(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains('/')
        && !id.chars().any(char::is_control)
}

fn disk_has_host_root(report: &HardwareReport, disk: &Disk) -> bool {
    disk.mountpoints.iter().any(|mount| mount == "/")
        || disk
            .partitions
            .iter()
            .any(|part| part.mountpoints.iter().any(|mount| mount == "/"))
        || report.mounts.iter().any(|mount| {
            mount.target == "/"
                && (mount.source == disk.path
                    || disk
                        .stable_id
                        .as_ref()
                        .is_some_and(|id| mount.source == id.0)
                    || (disk.major_minor.is_some() && disk.major_minor == mount.major_minor)
                    || disk.partitions.iter().any(|part| {
                        mount.source == part.path
                            || part.partuuid.as_ref().is_some_and(|id| {
                                mount.source == format!("/dev/disk/by-partuuid/{id}")
                                    || mount.source == format!("PARTUUID={id}")
                            })
                            || part.uuid.as_ref().is_some_and(|uuid| {
                                mount.source == format!("/dev/disk/by-uuid/{uuid}")
                                    || mount.source == format!("UUID={uuid}")
                            })
                            || (part.major_minor.is_some() && part.major_minor == mount.major_minor)
                    }))
        })
}
