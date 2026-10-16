//! Read-only discovery. Absence of evidence never becomes permission to use a
//! device. Filesystem detection uses only already-mounted installation roots.
mod capture;
pub mod readiness;

use capture::{Capture, MountedOsCapture};
use elastic_core::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub fn probe_live() -> HardwareReport {
    interpret(capture::live(), "live")
}

/// Load a raw offline capture directory (or its snapshot.json file).
pub fn probe_fixture(path: &Path) -> Result<HardwareReport, String> {
    let path = if path.is_dir() {
        path.join("snapshot.json")
    } else {
        path.to_path_buf()
    };
    let capture: Capture = serde_json::from_str(&capture::read_text(&path)?)
        .map_err(|e| format!("{}: invalid capture: {e}", path.display()))?;
    Ok(interpret(capture, "fixture"))
}

pub fn discover_installations(report: &HardwareReport) -> Vec<Installation> {
    report.installations.clone()
}

fn status(value: Option<bool>) -> CapabilityStatus {
    match value {
        Some(true) => CapabilityStatus::Supported,
        Some(false) => CapabilityStatus::Unsupported,
        None => CapabilityStatus::Unknown,
    }
}

fn file<'a>(capture: &'a Capture, key: &str) -> &'a str {
    capture.files.get(key).map(String::as_str).unwrap_or("")
}

fn interpret(capture: Capture, source: &str) -> HardwareReport {
    let mut report = HardwareReport {
        source: source.into(),
        ..HardwareReport::default()
    };
    report
        .findings
        .push(Finding::unknown("capture_provenance", &capture.provenance));
    report.findings.extend(
        capture
            .errors
            .iter()
            .map(|error| Finding::unknown("read_unavailable", error)),
    );
    let cpuinfo = file(&capture, "/proc/cpuinfo");
    let flags: Vec<_> = cpuinfo
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| ["flags", "Features"].contains(&key.trim()))
        .flat_map(|(_, flags)| flags.split_whitespace())
        .collect();
    let logical = cpuinfo
        .lines()
        .filter(|line| {
            line.split_once(':')
                .is_some_and(|(key, _)| key.trim() == "processor")
        })
        .count();
    report.cpu = CpuInfo {
        architecture: capture
            .architecture
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        logical_cpus: (logical > 0).then_some(logical as u32),
        virtualization: if flags.contains(&"svm") || flags.contains(&"vmx") {
            CapabilityStatus::Supported
        } else if flags.is_empty()
            || !matches!(capture.architecture.as_deref(), Some("x86_64" | "x86"))
        {
            CapabilityStatus::Unknown
        } else {
            CapabilityStatus::Unsupported
        },
        kvm: if capture.kvm_accessible == Some(true) {
            CapabilityStatus::Experimental
        } else if capture.kvm_exists == Some(false) {
            CapabilityStatus::Unsupported
        } else {
            CapabilityStatus::Unknown
        },
    };
    report.findings.push(Finding::unknown(
        "kvm_ioctl_unverified",
        format!(
            "/dev/kvm exists={:?}, read-accessible={:?}; KVM API and VM creation not tested",
            capture.kvm_exists, capture.kvm_accessible
        ),
    ));
    let memory: BTreeMap<_, _> = file(&capture, "/proc/meminfo")
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let number = value.split_whitespace().next()?.parse::<u64>().ok()?;
            Some((key, number.checked_mul(1024)?))
        })
        .collect();
    report.memory = MemoryInfo {
        total_bytes: memory.get("MemTotal").copied(),
        available_bytes: memory.get("MemAvailable").copied(),
    };
    report.kernel = capture.files.get("/proc/version").map(|v| v.trim().into());
    report.secure_boot = status(capture.secure_boot);
    report.findings.push(Finding {
        code: "secure_boot_state".into(),
        status: report.secure_boot,
        message: match capture.secure_boot {
            Some(true) => "EFI SecureBoot is enabled",
            Some(false) => "EFI SecureBoot is disabled",
            None => "EFI SecureBoot state is unreadable or unavailable",
        }
        .into(),
    });
    report.iommu = status(capture.iommu_groups_visible);
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for pci in &capture.pci {
        let attr = |name: &str| {
            pci.attributes
                .get(name)
                .map(|value| value.trim().to_owned())
        };
        if let Some(group) = &pci.iommu_group {
            groups
                .entry(group.clone())
                .or_default()
                .extend(pci.iommu_members.clone());
            if !pci.iommu_members.contains(&pci.bdf) {
                report.findings.push(Finding::unknown(
                    "iommu_members_incomplete",
                    format!(
                        "{} group {} membership could not be fully verified",
                        pci.bdf, group
                    ),
                ));
            }
        }
        if pci.mdev_types.is_none() {
            report.findings.push(Finding::unknown(
                "mdev_unreadable",
                format!("{} mdev supported types were not read", pci.bdf),
            ));
        }
        for required in ["vendor", "device", "class"] {
            if attr(required).is_none() {
                report.findings.push(Finding::unknown(
                    "pci_attribute_unavailable",
                    format!("{} {required} was not observed", pci.bdf),
                ));
            }
        }
        report.pci_devices.push(PciDevice {
            bdf: pci.bdf.clone(),
            vendor_id: attr("vendor")
                .unwrap_or_default()
                .trim_start_matches("0x")
                .into(),
            device_id: attr("device")
                .unwrap_or_default()
                .trim_start_matches("0x")
                .into(),
            class: attr("class")
                .unwrap_or_default()
                .trim_start_matches("0x")
                .into(),
            driver: pci.driver.clone(),
            iommu_group: pci.iommu_group.clone(),
            reset_methods: attr("reset_method")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            sriov_totalvfs: attr("sriov_totalvfs").and_then(|s| s.parse().ok()),
            sriov_numvfs: attr("sriov_numvfs").and_then(|s| s.parse().ok()),
            mdev_types: pci.mdev_types.clone().unwrap_or_default(),
            sysfs_path: pci.canonical_path.clone(),
            link_speed: attr("current_link_speed"),
            link_width: attr("current_link_width").and_then(|s| s.parse().ok()),
        });
    }
    report.iommu_groups = groups
        .into_iter()
        .map(|(id, members)| IommuGroup {
            id,
            members: members.into_iter().collect(),
        })
        .collect();
    report.mounts = parse_mountinfo(file(&capture, "/proc/self/mountinfo"));
    if !capture.files.contains_key("/proc/self/mountinfo") {
        report.findings.push(Finding::unknown(
            "mounts_unavailable",
            "Current mount namespace cannot be inspected",
        ));
    }
    let swaps: Vec<String> = file(&capture, "/proc/swaps")
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(unescape_mount)
        .collect();
    if !capture.files.contains_key("/proc/swaps") {
        report.findings.push(Finding::unknown(
            "swaps_unavailable",
            "Active swap cannot be inspected",
        ));
    }
    if let Some(raw) = capture.commands.get("lsblk") {
        match parse_disks(raw, &capture.disk_aliases, &report.mounts, &swaps) {
            Ok(disks) => report.disks = disks,
            Err(error) => report
                .findings
                .push(Finding::unknown("lsblk_parse_error", error)),
        }
    } else {
        report.findings.push(Finding::unknown(
            "lsblk_unavailable",
            "Block topology is unavailable",
        ));
    }
    report.findings.push(Finding::unknown("storage_observation_incomplete", "Current namespace mounts and swaps do not exclude writers in other namespaces, QEMU, device-mapper, or open block handles; execution must revalidate"));
    for disk in &report.disks {
        if disk.stable_id.is_none() {
            report.findings.push(Finding::unknown(
                "disk_identity_missing",
                format!(
                    "{} has no observed by-id/WWN identity; kernel path is diagnostic only",
                    disk.path
                ),
            ));
        }
    }
    report.resume = parse_resume(&capture);
    report.findings.push(Finding::unknown("resume_not_excluded", "No initramfs images or firmware hibernation policy inspected; absence of resume= does not prove swap may be removed"));
    report.efi = capture
        .commands
        .get("efibootmgr")
        .map(|raw| parse_efi(raw))
        .unwrap_or_default();
    if !report.efi.available {
        report.findings.push(Finding::unknown(
            "efi_unavailable",
            "UEFI boot variables unavailable; native entries may still exist",
        ));
    }
    for connector in &capture.drm {
        // The final PCI component of a canonical sysfs path is authoritative;
        // card numbers and connector names never imply a GPU vendor.
        let bdf = connector.card_device.as_ref().and_then(|path| {
            path.split('/')
                .rev()
                .find(|part| is_bdf(part))
                .map(str::to_owned)
        });
        if bdf.is_none() {
            report.findings.push(Finding::unknown(
                "drm_mapping_unknown",
                format!("{} has no resolved card/device PCI path", connector.name),
            ));
        }
        report.displays.push(DisplayConnector {
            id: ConnectorId(connector.name.clone()),
            card: connector.name.split('-').next().unwrap_or("").into(),
            pci_bdf: bdf,
            status: connector.status.clone().unwrap_or_else(|| "unknown".into()),
            modes: connector.modes.clone(),
            edid_hash: connector.edid_hash.clone(),
        });
    }
    report.installations = installations(&report, &capture.mounted_os);
    report
}

fn is_bdf(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 12
        && bytes[4] == b':'
        && bytes[7] == b':'
        && bytes[10] == b'.'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, byte)| [4, 7, 10].contains(&i) || byte.is_ascii_hexdigit())
}

pub(crate) fn unescape_mount(value: &str) -> String {
    // Decode only proc's defined escapes, without recursively interpreting
    // escaped backslashes (\\134040 represents a literal \\040).
    let mut result = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let lookahead: String = chars.clone().take(3).collect();
            let decoded = match lookahead.as_str() {
                "040" => Some(' '),
                "011" => Some('\t'),
                "012" => Some('\n'),
                "134" => Some('\\'),
                _ => None,
            };
            if let Some(decoded) = decoded {
                for _ in 0..3 {
                    chars.next();
                }
                result.push(decoded);
                continue;
            }
        }
        result.push(ch);
    }
    result
}

pub fn parse_mountinfo(raw: &str) -> Vec<Mount> {
    raw.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let left: Vec<_> = left.split_whitespace().collect();
            let right: Vec<_> = right.split_whitespace().collect();
            if left.len() < 6 || right.len() < 3 {
                return None;
            }
            // Both the per-mount and superblock flags constrain writability.
            Some(Mount {
                target: unescape_mount(left[4]),
                source: unescape_mount(right[1]),
                major_minor: Some(left[2].into()),
                read_only: left[5].split(',').any(|flag| flag == "ro")
                    || right[2].split(',').any(|flag| flag == "ro"),
            })
        })
        .collect()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .map(str::to_owned)
}
fn number(value: &Value, key: &str) -> Option<u64> {
    let value = value.get(key)?;
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}
fn points(value: &Value, mounts: &[Mount]) -> Vec<String> {
    let path = text(value, "name").unwrap_or_default();
    let major = text(value, "maj:min");
    let mut result: BTreeSet<String> = value
        .get("mountpoints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|p| *p != "[SWAP]")
        .map(str::to_owned)
        .collect();
    result.extend(
        mounts
            .iter()
            .filter(|m| m.source == path || (major.is_some() && m.major_minor == major))
            .map(|m| m.target.clone()),
    );
    result.into_iter().collect()
}

fn descendant_points(value: &Value, mounts: &[Mount]) -> Vec<String> {
    let mut result: BTreeSet<_> = points(value, mounts).into_iter().collect();
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            result.extend(descendant_points(child, mounts));
        }
    }
    result.into_iter().collect()
}

fn descendant_swap(
    value: &Value,
    swaps: &[String],
    aliases: &BTreeMap<String, Vec<String>>,
) -> bool {
    let path = text(value, "name").unwrap_or_default();
    swaps.iter().any(|swap| {
        swap == &path
            || aliases.get(&path).is_some_and(|names| {
                names
                    .iter()
                    .any(|alias| swap == &format!("/dev/disk/by-id/{alias}"))
            })
    }) || value
        .get("mountpoints")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("[SWAP]")))
        || value
            .get("children")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children
                    .iter()
                    .any(|child| descendant_swap(child, swaps, aliases))
            })
}

fn parse_disks(
    raw: &str,
    aliases: &BTreeMap<String, Vec<String>>,
    mounts: &[Mount],
    swaps: &[String],
) -> Result<Vec<Disk>, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let devices = value
        .get("blockdevices")
        .and_then(Value::as_array)
        .ok_or("missing lsblk blockdevices array")?;
    let mut disks = Vec::new();
    for value in devices {
        if text(value, "type").as_deref() != Some("disk") {
            continue;
        }
        let path = text(value, "name").ok_or("disk missing name")?;
        let stable_id = aliases
            .get(&path)
            .and_then(|a| {
                a.iter()
                    .filter(|a| !a.contains("-part"))
                    .min_by_key(|a| (!a.starts_with("wwn-"), a.as_str()))
            })
            .map(|a| DiskStableId(format!("/dev/disk/by-id/{a}")))
            .or_else(|| text(value, "wwn").map(|wwn| DiskStableId(format!("wwn:{wwn}"))));
        let mut partitions = Vec::new();
        if let Some(children) = value.get("children").and_then(Value::as_array) {
            for child in children {
                collect_partitions(child, mounts, swaps, aliases, &mut partitions)?;
            }
        }
        disks.push(Disk {
            path,
            stable_id,
            model: text(value, "model"),
            transport: text(value, "tran"),
            size_bytes: number(value, "size").ok_or("disk missing numeric size")?,
            gpt_guid: text(value, "ptuuid"),
            partition_table: text(value, "pttype"),
            major_minor: text(value, "maj:min"),
            mountpoints: points(value, mounts),
            partitions,
            // Filesystem FSAVAIL is deliberately never used as unallocated space.
            unallocated_bytes: None,
        });
    }
    let mut counts = BTreeMap::new();
    for disk in &disks {
        if let Some(id) = &disk.stable_id {
            *counts.entry(id.clone()).or_insert(0) += 1;
        }
    }
    for disk in &mut disks {
        if disk.stable_id.as_ref().is_some_and(|id| counts[id] != 1) {
            disk.stable_id = None;
        }
    }
    Ok(disks)
}

fn collect_partitions(
    value: &Value,
    mounts: &[Mount],
    swaps: &[String],
    aliases: &BTreeMap<String, Vec<String>>,
    result: &mut Vec<Partition>,
) -> Result<(), String> {
    if text(value, "type").as_deref() == Some("part") {
        let path = text(value, "name").ok_or("partition missing name")?;
        let active_swap = descendant_swap(value, swaps, aliases);
        result.push(Partition {
            path,
            partuuid: text(value, "partuuid").map(PartUuid),
            uuid: text(value, "uuid"),
            part_type: text(value, "parttype"),
            filesystem: text(value, "fstype"),
            size_bytes: number(value, "size").ok_or("partition missing numeric size")?,
            start_bytes: None,
            major_minor: text(value, "maj:min"),
            mountpoints: descendant_points(value, mounts),
            active_swap,
        });
    }
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            collect_partitions(child, mounts, swaps, aliases, result)?;
        }
    }
    Ok(())
}

pub fn parse_efi(raw: &str) -> EfiInfo {
    let mut result = EfiInfo::default();
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix("BootCurrent:") {
            result.boot_current = Some(value.trim().to_ascii_uppercase());
        }
        if let Some(value) = line.strip_prefix("BootNext:") {
            result.boot_next = Some(value.trim().to_ascii_uppercase());
        }
        if let Some(value) = line.strip_prefix("BootOrder:") {
            result.boot_order = value
                .trim()
                .split(',')
                .filter(|v| !v.is_empty())
                .map(|v| v.trim().to_ascii_uppercase())
                .collect();
        }
        let Some(rest) = line.strip_prefix("Boot") else {
            continue;
        };
        let Some(number) = rest.get(..4) else {
            continue;
        };
        if !number.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let tail = &rest[4..];
        let active = tail.starts_with('*');
        let tail = tail.trim_start_matches('*').trim();
        let split = tail.find('\t').or_else(|| {
            ["HD(", "PciRoot(", "VenHw(", "BBS(", "FvFile("]
                .iter()
                .filter_map(|prefix| tail.find(prefix))
                .min()
        });
        let (label, device_path) = split
            .map(|i| (&tail[..i], &tail[i..]))
            .unwrap_or((tail, ""));
        result.entries.push(EfiEntry {
            number: number.to_ascii_uppercase(),
            active,
            label: label.trim().into(),
            device_path: device_path.trim().into(),
        });
    }
    result.available = result.boot_current.is_some() || !result.entries.is_empty();
    result
}

fn parse_resume(capture: &Capture) -> ResumeInfo {
    let mut references = Vec::new();
    for path in [
        "/proc/cmdline",
        "/etc/default/grub",
        "/etc/mkinitcpio.conf",
        "/etc/initramfs-tools/conf.d/resume",
        "/etc/fstab",
    ] {
        for line in file(capture, path)
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
        {
            if line.contains("resume")
                || line.contains("RESUME")
                || (path.ends_with("fstab") && line.split_whitespace().nth(2) == Some("swap"))
            {
                references.push(format!("{path}: {}", line.trim()));
            }
        }
    }
    ResumeInfo {
        status: if references
            .iter()
            .any(|r| r.contains("resume=") || r.contains("RESUME="))
        {
            CapabilityStatus::Supported
        } else {
            CapabilityStatus::Unknown
        },
        references,
    }
}

fn os_value(raw: &str, key: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().trim_matches('"').trim_matches('\'').to_owned())
    })
}

fn linux_identity(raw: &str) -> Option<String> {
    let id = os_value(raw, "ID")?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || b"._-".contains(&ch))
    {
        return None;
    }
    os_value(raw, "PRETTY_NAME")
        .filter(|name| !name.is_empty())
        .or_else(|| os_value(raw, "NAME").filter(|name| !name.is_empty()))
}

fn installations(report: &HardwareReport, observations: &[MountedOsCapture]) -> Vec<Installation> {
    let mut result = Vec::new();
    for disk in &report.disks {
        for partition in &disk.partitions {
            if partition.active_swap || partition.filesystem.as_deref() == Some("swap") {
                continue;
            }
            let mounted = observations.iter().find(|o| {
                o.source == partition.path || partition.mountpoints.contains(&o.mountpoint)
            });
            let (family, display_name) = if let Some(name) = mounted
                .and_then(|o| o.os_release.as_ref())
                .and_then(|raw| linux_identity(raw))
            {
                (OsFamily::Linux, name)
            } else if mounted.is_some_and(|o| o.windows_system32) {
                (
                    OsFamily::Windows,
                    "Windows installation (version unverified)".into(),
                )
            } else {
                (OsFamily::Unknown, "UnknownInstallation".into())
            };
            if family == OsFamily::Unknown
                && !matches!(
                    partition.filesystem.as_deref(),
                    Some(
                        "ext2"
                            | "ext3"
                            | "ext4"
                            | "btrfs"
                            | "xfs"
                            | "ntfs"
                            | "BitLocker"
                            | "crypto_LUKS"
                    )
                )
            {
                continue;
            }
            let mut evidence = Vec::new();
            if let Some(observation) = mounted {
                evidence.push(format!(
                    "Already-mounted root inspected at {}",
                    observation.mountpoint
                ));
                if observation.os_release.is_some() {
                    evidence.push("Read etc/os-release; no filesystem-type OS inference".into());
                }
                if observation.fstab.is_some() {
                    evidence.push("Read etc/fstab".into());
                }
                if observation.windows_system32 {
                    evidence.push("Windows/System32 directory observed".into());
                }
                if observation.windows_bcd {
                    evidence.push("Windows BCD path exists; BCD contents not decoded".into());
                }
                evidence.extend(
                    observation
                        .boot_files
                        .iter()
                        .map(|name| format!("boot/{name} observed")),
                );
                evidence.extend(
                    observation
                        .errors
                        .iter()
                        .map(|error| format!("Observation limitation: {error}")),
                );
            } else {
                evidence.push(
                    "Unmounted volume: contents deliberately not mounted or inspected".into(),
                );
            }
            if family == OsFamily::Unknown {
                evidence.push("Filesystem type and EFI label do not establish an operating-system installation".into());
            }
            let mut findings = vec![Finding::unknown(
                "content_fingerprint_missing",
                "No block content was read; topology fingerprint cannot authorize overlay reuse",
            )];
            if family == OsFamily::Windows
                || matches!(partition.filesystem.as_deref(), Some("ntfs" | "BitLocker"))
            {
                findings.push(Finding::unknown("windows_encryption_unknown", "BitLocker/device encryption, TPM recovery, volume role and BCD require separate verification"));
            }
            if matches!(
                partition.filesystem.as_deref(),
                Some("BitLocker" | "crypto_LUKS")
            ) {
                findings.push(Finding::unknown(
                    "needs_user_input",
                    "Encrypted volume was not unlocked",
                ));
            }
            let source = if family == OsFamily::Windows {
                disk.stable_id
                    .clone()
                    .map(InstallationSource::PhysicalDisk)
                    .unwrap_or(InstallationSource::Unknown)
            } else {
                partition
                    .partuuid
                    .clone()
                    .map(|id| InstallationSource::PhysicalPartitions(vec![id]))
                    .unwrap_or(InstallationSource::Unknown)
            };
            let mut boot_profiles = Vec::new();
            let os_id = mounted
                .and_then(|o| o.os_release.as_ref())
                .and_then(|raw| os_value(raw, "ID"));
            for entry in &report.efi.entries {
                let path = entry.device_path.to_ascii_lowercase();
                let partition_on_disk = disk
                    .partitions
                    .iter()
                    .filter_map(|p| p.partuuid.as_ref())
                    .any(|id| path.contains(&format!("gpt,{},", id.0.to_ascii_lowercase())));
                let matching_loader = match family {
                    OsFamily::Linux => os_id.as_ref().is_some_and(|id| {
                        path.contains(&format!("\\efi\\{}\\", id.to_ascii_lowercase()))
                    }),
                    OsFamily::Windows => path.contains("\\efi\\microsoft\\"),
                    OsFamily::Unknown => false,
                };
                if partition_on_disk && matching_loader {
                    boot_profiles.push(BootProfile::Native {
                        efi_entry: entry.number.clone(),
                    });
                }
            }
            let prefix = match family {
                OsFamily::Linux => "linux",
                OsFamily::Windows => "windows",
                OsFamily::Unknown => "unknown",
            };
            let suffix = partition
                .partuuid
                .as_ref()
                .map(|id| id.0.clone())
                .unwrap_or_else(|| {
                    format!("snapshot-{}", &digest(partition.path.as_bytes())[..12])
                });
            result.push(Installation {
                id: InstallationId(format!("{prefix}-{suffix}")),
                family,
                display_name,
                source,
                volumes: vec![VolumeRef {
                    partuuid: partition.partuuid.clone(),
                    role: if family == OsFamily::Unknown {
                        "unknown"
                    } else {
                        "system"
                    }
                    .into(),
                    evidence: evidence.clone(),
                }],
                boot_profiles,
                fingerprint: SourceFingerprint {
                    size_bytes: if family == OsFamily::Windows {
                        disk.size_bytes
                    } else {
                        partition.size_bytes
                    },
                    gpt_guid: disk.gpt_guid.clone(),
                    partuuids: if family == OsFamily::Windows {
                        disk.partitions
                            .iter()
                            .filter_map(|p| p.partuuid.clone())
                            .collect()
                    } else {
                        partition.partuuid.clone().into_iter().collect()
                    },
                    content_hash: None,
                    generation: None,
                },
                compatibility: CompatibilityReport {
                    status: CapabilityStatus::Unknown,
                    findings,
                },
                evidence,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_or_malformed_os_release_does_not_establish_linux() {
        for invalid in [
            "",
            "random contents",
            "NAME=Linux\n",
            "ID=\nNAME=Linux\n",
            "ID=invalid/id\nNAME=Linux\n",
        ] {
            assert_eq!(linux_identity(invalid), None);
        }
        assert_eq!(
            linux_identity("ID=manjaro\nNAME=\"Manjaro Linux\"\n"),
            Some("Manjaro Linux".into())
        );
    }

    #[test]
    fn mounted_mapper_and_swap_holders_are_attributed_to_underlying_partition() {
        let raw = json!({"blockdevices": [{"name":"/dev/sda","type":"disk","size":1000,"children":[
            {"name":"/dev/sda1","type":"part","size":900,"partuuid":"p1","children":[
                {"name":"/dev/mapper/root","type":"crypt","maj:min":"253:0","mountpoints":[]},
                {"name":"/dev/mapper/swap","type":"lvm","mountpoints":[]}
            ]}
        ]}]})
        .to_string();
        let mounts = parse_mountinfo("1 0 253:0 / / rw - ext4 /dev/mapper/root rw\n");
        let disks = parse_disks(
            &raw,
            &BTreeMap::new(),
            &mounts,
            &["/dev/mapper/swap".into()],
        )
        .unwrap();
        assert_eq!(disks[0].partitions[0].mountpoints, ["/"]);
        assert!(disks[0].partitions[0].active_swap);
    }

    #[test]
    fn kernel_names_and_duplicate_wwns_never_become_unique_identities() {
        let raw = json!({"blockdevices":[
            {"name":"/dev/sda","type":"disk","size":1000,"wwn":"same"},
            {"name":"/dev/sdb","type":"disk","size":1000,"wwn":"same"},
            {"name":"/dev/sdc","type":"disk","size":1000,"fsavail":900}
        ]})
        .to_string();
        let disks = parse_disks(&raw, &BTreeMap::new(), &[], &[]).unwrap();
        assert!(disks.iter().all(|disk| disk.stable_id.is_none()));
        assert!(disks.iter().all(|disk| disk.unallocated_bytes.is_none()));
    }

    #[test]
    fn malformed_block_capture_degrades_instead_of_panicking() {
        let mut capture = Capture::default();
        capture.commands.insert("lsblk".into(), "{broken".into());
        let report = interpret(capture, "fixture");
        assert!(report.disks.is_empty());
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.code == "lsblk_parse_error"));
    }
}
