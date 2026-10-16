//! Non-executing native/GPU handoff plans. Driver packages are never removed.
use crate::{condition, empty_plan, finish, operation};
use elastic_core::*;
use std::collections::BTreeSet;

pub fn plan_native_return(report: &HardwareReport, installation: &Installation) -> Plan {
    let mut plan = empty_plan("native_return");
    plan.installation_id = Some(installation.id.clone());
    plan.requires_approval = true;
    let entries: BTreeSet<_> = installation
        .boot_profiles
        .iter()
        .filter_map(|p| match p {
            BootProfile::Native { efi_entry } => Some(efi_entry.as_str()),
            _ => None,
        })
        .collect();
    let valid = entries.len() == 1
        && entries.iter().all(|number| {
            number.len() == 4
                && number.bytes().all(|b| b.is_ascii_hexdigit())
                && report
                    .efi
                    .entries
                    .iter()
                    .filter(|e| e.number == **number && e.active && !e.device_path.is_empty())
                    .count()
                    == 1
        });
    condition(&mut plan, "native_entry", if report.efi.available && valid { CheckStatus::Satisfied } else { CheckStatus::Blocked },
        "Require one active original EFI entry linked to this installation; never guess by label or create/replace an entry");
    condition(&mut plan, "fresh_native_identity", CheckStatus::NeedsVerification,
        "Re-read the original EFI device path and match its GPT partition identity and loader hash to the preserved installation record");
    condition(&mut plan, "all_io_stopped", CheckStatus::NeedsVerification,
        "Prove all installation domains stopped, storage flushed, hypervisor handles closed, and all storage/GPU leases released; unknown denies handoff");
    condition(&mut plan, "native_recovery", CheckStatus::NeedsVerification,
        "Verify original native boot and rescue access; native-backed writes additionally require independently restorable backups");
    condition(
        &mut plan,
        "bootnext_conflict",
        if report.efi.boot_next.is_none() {
            CheckStatus::Satisfied
        } else {
            CheckStatus::Blocked
        },
        "Do not overwrite another pending BootNext request",
    );
    operation(&mut plan, "invalidate_trial_generation", "Durably invalidate every overlay and saved-memory generation for this installation BEFORE scheduling native boot; an interrupted handoff remains invalidated", false);
    operation(&mut plan, "restore_gpu_ownership", "After QEMU exit, verify every detached function returned to its recorded original driver; retain driver packages and do not unload shared modules", false);
    if valid {
        operation(&mut plan, "one_time_native_boot", format!("After approval and fresh validation, set BootNext to existing entry {}; preserve BootOrder and every original EFI file", entries.first().unwrap()), false);
    }
    operation(&mut plan, "verify_native_return", "User-approved reboot and actual native login are required; on the next managed start fully re-inspect storage and rebuild Trial/Boot Bridge", false);
    plan.rollback.push("If scheduling fails, keep generations invalidated; use the original firmware-menu entry. Clear only a BootNext value proven owned by this operation; never restore stale BootOrder wholesale".into());
    plan.capability_gaps.push(
        "Review-only plan: no EFI writer, reboot executor or physical native boot evidence".into(),
    );
    finish(&mut plan);
    plan
}

fn valid_bdf(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 12
        && bytes[4] == b':'
        && bytes[7] == b':'
        && bytes[10] == b'.'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7 | 10) || b.is_ascii_hexdigit())
        && u8::from_str_radix(&value[8..10], 16).is_ok_and(|slot| slot <= 31)
        && bytes[11] <= b'7'
}

pub fn plan_windows_gpu(report: &HardwareReport, installation: &Installation, bdf: &str) -> Plan {
    let mut plan = empty_plan("windows_gpu_preserve_drivers");
    plan.installation_id = Some(installation.id.clone());
    plan.requires_approval = true;
    condition(
        &mut plan,
        "windows_installation",
        if installation.family == OsFamily::Windows {
            CheckStatus::Satisfied
        } else {
            CheckStatus::Blocked
        },
        "GPU profile requires identified Windows, not an NTFS-only candidate",
    );
    condition(
        &mut plan,
        "iommu",
        if report.iommu == CapabilityStatus::Supported {
            CheckStatus::Satisfied
        } else {
            CheckStatus::Blocked
        },
        "IOMMU must be observed enabled; ACS override is not an isolation substitute",
    );
    let devices: Vec<_> = report.pci_devices.iter().filter(|d| d.bdf == bdf).collect();
    let gpu = devices.first().copied();
    let group = gpu.and_then(|d| d.iommu_group.as_ref()).and_then(|id| {
        let groups: Vec<_> = report.iommu_groups.iter().filter(|g| &g.id == id).collect();
        (groups.len() == 1).then(|| groups[0])
    });
    let gpu_valid = valid_bdf(bdf)
        && devices.len() == 1
        && gpu.is_some_and(|d| d.class.trim_start_matches("0x").starts_with("03"));
    condition(
        &mut plan,
        "gpu_identity",
        if gpu_valid {
            CheckStatus::Satisfied
        } else {
            CheckStatus::Blocked
        },
        "Require one display-class PCI function at an explicit BDF",
    );
    let complete = gpu_valid
        && group.is_some_and(|g| {
            let unique: BTreeSet<_> = g.members.iter().collect();
            !g.members.is_empty()
                && unique.len() == g.members.len()
                && g.members.iter().any(|m| m == bdf)
                && g.members.iter().all(|m| {
                    valid_bdf(m)
                        && report
                            .pci_devices
                            .iter()
                            .filter(|d| &d.bdf == m && d.iommu_group.as_ref() == Some(&g.id))
                            .count()
                            == 1
                })
                && report
                    .pci_devices
                    .iter()
                    .filter(|d| d.iommu_group.as_ref() == Some(&g.id))
                    .all(|d| unique.contains(&d.bdf))
        });
    condition(&mut plan, "complete_group", if complete { CheckStatus::Satisfied } else { CheckStatus::Blocked }, "Cross-check group membership in both directions; include every GPU/HDA function and reject incomplete or duplicate observations");
    if let Some(group) = group.filter(|_| complete) {
        plan.affected_devices
            .push(LeaseResource::PciIommuGroup(group.id.clone()));
        let allowed = group.members.iter().all(|m| {
            report.pci_devices.iter().any(|d| {
                &d.bdf == m && {
                    let class = d.class.trim_start_matches("0x");
                    class.starts_with("03") || class.starts_with("0403")
                }
            })
        });
        condition(&mut plan, "group_scope", if allowed { CheckStatus::Satisfied } else { CheckStatus::Blocked }, "This first adapter accepts display/audio only; extra USB, bridge or storage functions require a separate reviewed adapter");
        let connected: Vec<_> = report
            .displays
            .iter()
            .filter(|c| c.status == "connected")
            .collect();
        let known = !connected.is_empty()
            && connected.iter().all(|c| {
                c.pci_bdf
                    .as_ref()
                    .is_some_and(|id| report.pci_devices.iter().any(|d| &d.bdf == id))
            });
        let rescue = connected.iter().any(|c| {
            c.pci_bdf
                .as_ref()
                .is_some_and(|id| !group.members.contains(id))
        });
        condition(&mut plan, "host_rescue_display", if known && rescue { CheckStatus::NeedsVerification } else { CheckStatus::Blocked }, "Connected display outside the passthrough group plus verified management input are required; connector presence alone does not prove compositor independence");
        for c in connected.iter().filter(|c| {
            c.pci_bdf
                .as_ref()
                .is_some_and(|id| group.members.contains(id))
        }) {
            plan.affected_devices
                .push(LeaseResource::DisplayConnector(c.id.clone()));
        }
        for m in &group.members {
            let d = report.pci_devices.iter().find(|d| &d.bdf == m).unwrap();
            condition(&mut plan, &format!("driver_restore_{m}"), if d.driver.as_ref().is_some_and(|s| !s.is_empty() && s != "vfio-pci") { CheckStatus::NeedsVerification } else { CheckStatus::Blocked },
                format!("Persist original driver {:?}, driver_override and identity for {m}; refuse unknown original ownership", d.driver));
        }
    }
    condition(&mut plan, "exclusive_gpu_lease", CheckStatus::NeedsVerification, "Globally inspect compositor, DRM, CUDA, audio, other VMs and device handles; detach only after all owners release the whole group");
    condition(&mut plan, "reset_reliability", CheckStatus::NeedsVerification, "Exposed reset methods are not proof: verify repeated Windows shutdown/start and original-driver reattachment on this host");
    operation(&mut plan, "preserve_driver_packages", "Keep NVIDIA/AMD/HDA and Windows native storage drivers; no package removal, global blacklist, module unload, initramfs replacement or default native boot modification", false);
    operation(&mut plan, "managed_pci_handoff", "After approval, durable whole-group lease and owner checks, use libvirt managed PCI hostdev detach/reattach for every function; persist recovery intent before detaching the first function", false);
    operation(&mut plan, "guest_compatibility", "Use Q35, private OVMF vars and persistent vTPM; start Windows using its existing compatible storage bus, add VirtIO only after a successful boot, retain original drivers", false);
    operation(&mut plan, "validate_windows_gpu", "Verify real GPU identity, NVIDIA driver, display and CUDA, then repeated shutdown/restart/reattach; record native and guest boot times before/after", false);
    plan.rollback.push("On partial detach/start failure, first prove QEMU exited, then reattach each changed function to its recorded driver. Unknown observation retains leases and requests recovery; never force-reset an in-use device".into());
    plan.capability_gaps.push("No physical binding executor in this build. Runtime handoff is experimental; if display release/reset fails, use a separately approved dedicated Host boot profile without changing native driver packages".into());
    finish(&mut plan);
    plan
}
