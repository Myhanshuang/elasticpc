//! Read-only prerequisites and migration evidence. This is an observation, not
//! permission to modify devices, remove swap, or start a virtualization daemon.
use crate::{capture, parse_mountinfo, unescape_mount};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

const TOOLS: [&str; 4] = ["qemu-img", "qemu-io", "qemu-system-x86_64", "virsh"];
const SERVICE_ROOTS: [(&str, &str); 3] = [
    ("docker", "/var/lib/docker"),
    ("k3s", "/var/lib/rancher/k3s"),
    ("containerd", "/var/lib/containerd"),
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Missing,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ToolObservation {
    pub status: Availability,
    pub path: Option<String>,
}

/// Raw captures use the same keys as the hardware probe's snapshot.json. Missing
/// observations remain unknown; fixtures never inspect the current machine.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ReadinessInput {
    pub files: BTreeMap<String, String>,
    pub commands: BTreeMap<String, String>,
    pub disk_aliases: BTreeMap<String, Vec<String>>,
    pub tools: BTreeMap<String, ToolObservation>,
    pub kvm_exists: Option<bool>,
    pub kvm_accessible: Option<bool>,
    pub kvm_kernel_registered: Option<bool>,
    pub kvm_api: Option<KvmApiObservation>,
    /// Existing udev database text keyed by the observed disk node. These are
    /// recorded aliases, not evidence that the aliases resolve in this namespace.
    pub udev_records: BTreeMap<String, String>,
    pub service_directories: BTreeMap<String, Option<bool>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KvmReadiness {
    pub node_visible: Option<bool>,
    pub read_accessible: Option<bool>,
    pub kernel_registered: Option<bool>,
    pub api_verified: bool,
    pub note: String,
}

/// Offline fixtures describe an observation, never an instruction to open a
/// device. No fixture-specified device path or ioctl is accepted.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum KvmApiObservation {
    Missing,
    AccessDenied,
    InvalidNode,
    ApiVersion { version: i32 },
    IoctlFailed { errno: i32 },
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KvmApiStatus {
    Available,
    Missing,
    AccessDenied,
    InvalidNode,
    ApiMismatch,
    IoctlFailed,
    KernelRegistrationUnverified,
    Unknown,
    NotObserved,
    UnsupportedPlatform,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KvmApiReport {
    pub schema_version: u32,
    pub source: String,
    pub status: KvmApiStatus,
    pub kernel_registered: Option<bool>,
    /// Whether this invocation opened a device (always false for fixtures).
    pub node_opened: bool,
    /// Whether this invocation issued an ioctl (always false for fixtures).
    pub ioctl_attempted: bool,
    pub api_version: Option<i32>,
    /// True only for a successful live KVM_GET_API_VERSION result of 12.
    pub api_verified: bool,
    pub errno: Option<i32>,
    pub note: String,
}

fn api_report(status: KvmApiStatus, kernel_registered: Option<bool>, note: &str) -> KvmApiReport {
    KvmApiReport {
        schema_version: 1,
        source: "live".into(),
        status,
        kernel_registered,
        node_opened: false,
        ioctl_attempted: false,
        api_version: None,
        api_verified: false,
        errno: None,
        note: note.into(),
    }
}

fn kvm_kernel_registered() -> Option<bool> {
    match capture::read_text(Path::new("/sys/class/misc/kvm/dev")) {
        Ok(value) => Some(value.trim() == "10:232"),
        // Missing sysfs evidence may reflect a restricted namespace, not an
        // absent kernel implementation. Never load modules to resolve it.
        Err(_) => None,
    }
}

fn valid_kvm_node(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_char_device()
        && libc::major(metadata.rdev()) == 10
        && libc::minor(metadata.rdev()) == 232
}

/// Explicit, narrowly scoped API probe. Opens only /dev/kvm read-only after
/// rejecting symlinks and wrong device types/numbers. Issues exactly
/// KVM_GET_API_VERSION; never creates a VM, loads a driver or changes a device.
/// See https://docs.kernel.org/virt/kvm/api.html#kvm-get-api-version.
pub fn probe_kvm_api() -> KvmApiReport {
    probe_kvm_api_at(Path::new("/dev/kvm"), kvm_kernel_registered())
}

fn probe_kvm_api_at(path: &Path, kernel_registered: Option<bool>) -> KvmApiReport {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            let (status, note) = match error.kind() {
                std::io::ErrorKind::NotFound => (KvmApiStatus::Missing, "The KVM device node is absent in this namespace; CPU virtualization and host kernel support cannot be inferred from this absence."),
                std::io::ErrorKind::PermissionDenied => (KvmApiStatus::AccessDenied, "Permission denied while inspecting the KVM device node; no escalation attempted."),
                _ => (KvmApiStatus::Unknown, "Could not inspect the KVM device node; no open or ioctl attempted."),
            };
            let mut report = api_report(status, kernel_registered, note);
            report.errno = error.raw_os_error();
            return report;
        }
    };
    if !valid_kvm_node(&metadata) {
        return api_report(KvmApiStatus::InvalidNode, kernel_registered, "Refused unexpected node: expected a non-symlink character device with major 10, minor 232. No open or ioctl attempted.");
    }
    if kernel_registered != Some(true) {
        return api_report(KvmApiStatus::KernelRegistrationUnverified, kernel_registered, "A registered KVM misc device was not observed in sysfs; refused to open a possibly stale node. No module loading attempted.");
    }
    // Linux x86-64 _IO(KVMIO, 0) is 0xAE00. Refuse other targets instead
    // of guessing architecture-dependent ioctl encodings.
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return api_report(KvmApiStatus::UnsupportedPlatform, kernel_registered, "This API probe is implemented only for Linux x86-64; no device open or ioctl attempted.");
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => {
            let status = if error.kind() == std::io::ErrorKind::PermissionDenied {
                KvmApiStatus::AccessDenied
            } else if error.kind() == std::io::ErrorKind::NotFound {
                KvmApiStatus::Missing
            } else {
                KvmApiStatus::Unknown
            };
            let mut report = api_report(
                status,
                kernel_registered,
                "KVM read-only open failed; no ioctl or escalation attempted.",
            );
            report.errno = error.raw_os_error();
            return report;
        }
    };
    let mut report = api_report(
        KvmApiStatus::InvalidNode,
        kernel_registered,
        "KVM device identity changed before opening; no ioctl attempted.",
    );
    report.node_opened = true;
    if !file.metadata().is_ok_and(|opened| {
        valid_kvm_node(&opened) && opened.dev() == metadata.dev() && opened.ino() == metadata.ino()
    }) {
        return report;
    }
    report.ioctl_attempted = true;
    // SAFETY: the FD is alive and checked to be the same KVM character device.
    // KVM_GET_API_VERSION takes no pointer argument and does not create a VM.
    let version = unsafe { libc::ioctl(file.as_raw_fd(), 0xAE00 as libc::c_ulong, 0) };
    if version < 0 {
        report.status = KvmApiStatus::IoctlFailed;
        report.errno = std::io::Error::last_os_error().raw_os_error();
        report.note = "KVM_GET_API_VERSION failed; no VM was created and API availability remains unverified.".into();
    } else {
        report.api_version = Some(version);
        report.api_verified = version == 12;
        report.status = if report.api_verified {
            KvmApiStatus::Available
        } else {
            KvmApiStatus::ApiMismatch
        };
        report.note = if report.api_verified {
            "KVM API version 12 verified by a read-only open and KVM_GET_API_VERSION only. VM creation, CPU extensions and guest execution remain untested."
        } else {
            "Unsupported KVM API version: expected 12. No VM was created."
        }.into();
    }
    report
}

/// Interpret explicitly captured API evidence without probing the current host.
/// Missing observations remain not_observed, even when /dev/kvm exists locally.
pub fn parse_kvm_api(input: &ReadinessInput) -> KvmApiReport {
    let mut report = api_report(
        KvmApiStatus::NotObserved,
        input.kvm_kernel_registered,
        "Fixture contains no KVM API observation; no live fallback was attempted.",
    );
    report.source = "fixture".into();
    if let Some(observation) = &input.kvm_api {
        report.status = match observation {
            KvmApiObservation::Missing => KvmApiStatus::Missing,
            KvmApiObservation::AccessDenied => KvmApiStatus::AccessDenied,
            KvmApiObservation::InvalidNode => KvmApiStatus::InvalidNode,
            KvmApiObservation::Unknown => KvmApiStatus::Unknown,
            KvmApiObservation::ApiVersion { version } => {
                report.api_version = Some(*version);
                if *version == 12 {
                    KvmApiStatus::Available
                } else {
                    KvmApiStatus::ApiMismatch
                }
            }
            KvmApiObservation::IoctlFailed { errno } => {
                report.errno = Some(*errno);
                KvmApiStatus::IoctlFailed
            }
        };
        report.note = "Offline captured KVM API observation only; this invocation did not open a device or issue an ioctl and does not verify the current host.".into();
    }
    report
}

pub fn probe_kvm_api_fixture(path: &Path) -> Result<KvmApiReport, String> {
    let path = if path.is_dir() {
        path.join("snapshot.json")
    } else {
        path.to_path_buf()
    };
    let input: ReadinessInput = serde_json::from_str(&capture::read_text(&path)?)
        .map_err(|error| format!("{}: invalid readiness capture: {error}", path.display()))?;
    Ok(parse_kvm_api(&input))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiskIdentityReadiness {
    pub path: String,
    pub model: Option<String>,
    pub t7_candidate: bool,
    pub by_id: Vec<String>,
    pub udev_recorded_by_id: Vec<String>,
    pub wwn: Option<String>,
    pub serial_observed: bool,
    pub unique_stable_identity_observed: bool,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumeResolution {
    ActiveSwap,
    ObservedNotActiveSwap,
    Ambiguous,
    Unresolved,
    Unknown,
    Disabled,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResumeReference {
    pub source: String,
    pub target: String,
    pub resolution: ResumeResolution,
    pub matched_devices: Vec<String>,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServiceStorageCandidate {
    pub service: String,
    pub conventional_path: String,
    pub directory_observed: Option<bool>,
    pub backing_mount: Option<String>,
    pub backing_source: Option<String>,
    pub observed_mounts_under_path: usize,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LibvirtInventory {
    /// Captured `LC_ALL=C virsh --readonly list --all --uuid` output only.
    pub domain_uuids: Vec<String>,
    pub live_connection_attempted: bool,
    pub resource_occupancy_verified: bool,
    pub note: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReadinessReport {
    pub schema_version: u32,
    pub source: String,
    pub redacted: bool,
    pub memory_total_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub tools: BTreeMap<String, ToolObservation>,
    pub kvm: KvmReadiness,
    pub storage_topology_observed: bool,
    pub disks: Vec<DiskIdentityReadiness>,
    pub active_swap: Option<Vec<String>>,
    pub resume_references: Vec<ResumeReference>,
    pub service_storage: Vec<ServiceStorageCandidate>,
    pub libvirt: LibvirtInventory,
    pub limitations: Vec<String>,
}

impl ReadinessReport {
    /// Redact the full identifying fields, including unresolved resume targets
    /// and arbitrary non-UUID identities. The returned report is presentation only.
    pub fn redacted(&self) -> Self {
        let mut result = self.clone();
        result.redacted = true;
        for tool in result.tools.values_mut() {
            tool.path = tool.path.as_deref().map(|path| {
                if ["/usr/bin/", "/usr/sbin/", "/bin/", "/sbin/"]
                    .iter()
                    .any(|prefix| path.starts_with(prefix))
                {
                    path.into()
                } else {
                    "[redacted executable path]".into()
                }
            });
        }
        for (index, disk) in result.disks.iter_mut().enumerate() {
            disk.path = redact_path(&disk.path);
            for (alias, value) in disk.by_id.iter_mut().enumerate() {
                *value = format!(
                    "/dev/disk/by-id/REDACTED-disk-{}-alias-{}",
                    index + 1,
                    alias + 1
                );
            }
            for (alias, value) in disk.udev_recorded_by_id.iter_mut().enumerate() {
                *value = format!(
                    "/dev/disk/by-id/REDACTED-disk-{}-recorded-alias-{}",
                    index + 1,
                    alias + 1
                );
            }
            if disk.wwn.is_some() {
                disk.wwn = Some(format!("REDACTED-wwn-{}", index + 1));
            }
        }
        for reference in &mut result.resume_references {
            if reference.resolution != ResumeResolution::Disabled {
                reference.target = "[redacted resume target]".into();
            }
            for device in &mut reference.matched_devices {
                *device = redact_path(device);
            }
        }
        if let Some(swaps) = &mut result.active_swap {
            for swap in swaps {
                *swap = redact_path(swap);
            }
        }
        for service in &mut result.service_storage {
            service.backing_mount = service.backing_mount.as_deref().map(redact_path);
            service.backing_source = service.backing_source.as_deref().map(redact_path);
        }
        for uuid in &mut result.libvirt.domain_uuids {
            *uuid = "[redacted domain UUID]".into();
        }
        result
    }
}

fn redact_path(path: &str) -> String {
    if matches!(path, "/" | "/var" | "/home" | "/swapfile")
        || (path.starts_with("/dev/")
            && !path.starts_with("/dev/disk/")
            && !path.starts_with("/dev/mapper/"))
    {
        path.into()
    } else {
        "[redacted path]".into()
    }
}

/// Uses the established, bounded, read-only acquisition layer. Tool discovery
/// checks executable metadata in system bin directories; it does not run tools.
/// In particular, no libvirt socket connection or daemon activation is attempted.
pub fn probe_readiness() -> ReadinessReport {
    let captured = capture::live();
    let mut input = ReadinessInput {
        files: captured.files,
        commands: captured.commands,
        disk_aliases: captured.disk_aliases,
        ..ReadinessInput::default()
    };
    // Unlike Path::exists(), permission errors must not become evidence of absence.
    input.kvm_exists = captured.kvm_exists;
    input.kvm_accessible = captured.kvm_accessible;
    input.kvm_kernel_registered = kvm_kernel_registered();
    collect_udev_records(&mut input);
    for tool in TOOLS {
        input.tools.insert(tool.into(), discover_tool(tool));
    }
    for (_, path) in SERVICE_ROOTS {
        let observed = match fs::metadata(path) {
            Ok(metadata) => Some(metadata.is_dir()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
            Err(_) => None,
        };
        input.service_directories.insert(path.into(), observed);
    }
    let mut report = parse_readiness(&input);
    report.source = "live".into();
    report
}

fn collect_udev_records(input: &mut ReadinessInput) {
    let Some(topology) = input
        .commands
        .get("lsblk")
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    else {
        return;
    };
    let Some(devices) = topology.get("blockdevices").and_then(Value::as_array) else {
        return;
    };
    for device in devices {
        if string(device, "type").as_deref() != Some("disk") {
            continue;
        }
        let (Some(path), Some(number)) = (string(device, "name"), string(device, "maj:min")) else {
            continue;
        };
        let Some((major, minor)) = number.split_once(':') else {
            continue;
        };
        let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>()) else {
            continue;
        };
        let record_path = format!("/run/udev/data/b{major}:{minor}");
        if let Ok(record) = capture::read_text(Path::new(&record_path)) {
            input.udev_records.insert(path, record);
        }
    }
}

fn recorded_by_id(record: &str) -> Vec<String> {
    record
        .lines()
        .filter_map(|line| line.strip_prefix("S:disk/by-id/"))
        .filter(|name| {
            !name.is_empty()
                && !name.contains('/')
                && !name.contains("-part")
                && !name.chars().any(char::is_whitespace)
        })
        .map(|name| format!("/dev/disk/by-id/{name}"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn discover_tool(name: &str) -> ToolObservation {
    use std::os::unix::fs::PermissionsExt;
    let mut status = Availability::Missing;
    for directory in ["/usr/bin", "/usr/sbin", "/bin", "/sbin"] {
        let path = Path::new(directory).join(name);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 => {
                return ToolObservation {
                    status: Availability::Available,
                    path: Some(path.to_string_lossy().into()),
                };
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => status = Availability::Unknown,
        }
    }
    ToolObservation { status, path: None }
}

pub fn probe_readiness_fixture(path: &Path) -> Result<ReadinessReport, String> {
    let path = if path.is_dir() {
        path.join("snapshot.json")
    } else {
        path.to_path_buf()
    };
    let input: ReadinessInput = serde_json::from_str(&capture::read_text(&path)?)
        .map_err(|error| format!("{}: invalid readiness capture: {error}", path.display()))?;
    Ok(parse_readiness(&input))
}

pub fn parse_readiness(input: &ReadinessInput) -> ReadinessReport {
    let memory = input.files.get("/proc/meminfo");
    let topology = input
        .commands
        .get("lsblk")
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let devices = topology
        .as_ref()
        .and_then(|value| value.get("blockdevices"))
        .and_then(Value::as_array);
    let mut nodes = Vec::new();
    if let Some(devices) = devices {
        for device in devices {
            collect_nodes(device, &mut nodes);
        }
    }
    let topology_observed = devices.is_some()
        && nodes.iter().all(|node| {
            string(node, "name").is_some()
                && string(node, "type").is_some()
                && node
                    .get("children")
                    .is_none_or(|children| children.is_array())
        });
    let active_swap = input
        .files
        .get("/proc/swaps")
        .and_then(|raw| parse_swaps(raw));
    let mounts = input
        .files
        .get("/proc/self/mountinfo")
        .map(|raw| parse_mountinfo(raw));
    let mut disks: Vec<_> = nodes
        .iter()
        .filter(|node| {
            string(node, "type").as_deref() == Some("disk") && string(node, "name").is_some()
        })
        .map(|node| {
            let path = string(node, "name").unwrap_or_default();
            let model = string(node, "model");
            let by_id = input
                .disk_aliases
                .get(&path)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|alias| !alias.contains("-part"))
                .map(|alias| format!("/dev/disk/by-id/{alias}"))
                .collect();
            DiskIdentityReadiness {
                udev_recorded_by_id: input
                    .udev_records
                    .get(&path)
                    .map(|record| recorded_by_id(record))
                    .unwrap_or_default(),
                path,
                t7_candidate: model.as_deref().is_some_and(|model| {
                    model
                        .split(|ch: char| !ch.is_ascii_alphanumeric())
                        .any(|part| part.eq_ignore_ascii_case("T7"))
                }),
                model,
                by_id,
                wwn: string(node, "wwn"),
                serial_observed: string(node, "serial").is_some(),
                unique_stable_identity_observed: false,
                note: String::new(),
            }
        })
        .collect();
    let mut identity_counts = BTreeMap::<String, usize>::new();
    for disk in &disks {
        let identities: BTreeSet<_> = disk
            .by_id
            .iter()
            .cloned()
            .chain(disk.wwn.iter().map(|wwn| format!("wwn:{wwn}")))
            .collect();
        for identity in identities {
            *identity_counts.entry(identity).or_default() += 1;
        }
    }
    for disk in &mut disks {
        disk.unique_stable_identity_observed = disk
            .by_id
            .iter()
            .cloned()
            .chain(disk.wwn.iter().map(|wwn| format!("wwn:{wwn}")))
            .any(|identity| identity_counts.get(&identity) == Some(&1));
        disk.note = if disk.unique_stable_identity_observed {
            "A unique by-id or WWN was observed in this snapshot; revalidate identity and all writers before any future operation."
        } else if !disk.udev_recorded_by_id.is_empty() {
            "The udev database records a by-id alias, but no unique resolved by-id/WWN was observed in this namespace; recorded metadata alone is not an execution identity."
        } else if disk.serial_observed {
            "A serial was reported, but no unique by-id/WWN identity was observed; serial alone is not an execution identity."
        } else {
            "No unique by-id/WWN identity was observed; kernel device names are diagnostic only."
        }.into();
    }
    let mut references = Vec::new();
    for source in [
        "/proc/cmdline",
        "/etc/default/grub",
        "/etc/initramfs-tools/conf.d/resume",
    ] {
        if let Some(raw) = input.files.get(source) {
            for target in resume_targets(raw) {
                references.push(resolve_resume(
                    source,
                    &target,
                    &nodes,
                    topology_observed,
                    active_swap.as_deref(),
                    &input.disk_aliases,
                ));
            }
        }
    }
    let service_storage = SERVICE_ROOTS.into_iter().map(|(service, path)| {
        let backing = mounts.as_ref().and_then(|mounts| mounts.iter().filter(|mount| path_under(path, &mount.target)).max_by_key(|mount| mount.target.len()));
        ServiceStorageCandidate {
            service: service.into(), conventional_path: path.into(),
            directory_observed: input.service_directories.get(path).copied().flatten(),
            backing_mount: backing.map(|mount| mount.target.clone()), backing_source: backing.map(|mount| mount.source.clone()),
            observed_mounts_under_path: mounts.as_ref().map(|mounts| mounts.iter().filter(|mount| path_under(&mount.target, path)).count()).unwrap_or(0),
            note: "Conventional data path and current mount namespace metadata only; custom data roots, service state and contents were not inspected.".into(),
        }
    }).collect();
    let libvirt = match input.commands.get("virsh-readonly-list-all-uuid") {
        Some(raw) => parse_libvirt_inventory(raw).unwrap_or_else(|_| unobserved_libvirt("Captured libvirt UUID inventory was malformed; domain and resource state remain unknown.")),
        None => unobserved_libvirt("No connection attempted: even a read-only socket connection can activate a libvirt daemon. Domain/device occupancy remains unknown."),
    };
    ReadinessReport {
        schema_version: 1, source: "fixture".into(), redacted: false,
        memory_total_bytes: memory.and_then(|raw| memory_value(raw, "MemTotal")),
        memory_available_bytes: memory.and_then(|raw| memory_value(raw, "MemAvailable")),
        tools: TOOLS.into_iter().map(|name| (name.into(), input.tools.get(name).cloned().unwrap_or_default())).collect(),
        kvm: KvmReadiness {
            node_visible: input.kvm_exists, read_accessible: input.kvm_accessible, api_verified: false,
            kernel_registered: input.kvm_kernel_registered,
            note: "Default live readiness uses metadata only and does not open /dev/kvm. Recorded fixture access evidence is not a live access test. API availability requires an explicit probe; node absence does not establish absent CPU virtualization.".into(),
        },
        storage_topology_observed: topology_observed, disks, active_swap, resume_references: references, service_storage, libvirt,
        limitations: vec![
            "Tool availability checks fixed system bin paths and executable mode bits; it does not verify versions, effective execution permission or compatibility.".into(),
            "Missing resume references do not authorize swap removal; initramfs images, firmware hibernation policy and encrypted devices remain unverified.".into(),
            "Unresolved resume references may be stale or outside the visible topology; no configuration was changed.".into(),
            "Memory values reflect this process's current host view; workload headroom must be planned separately and rechecked before launch.".into(),
            "Storage observations do not exclude writers or mounts in other namespaces; service migration candidates are metadata only.".into(),
        ],
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn collect_nodes<'a>(value: &'a Value, nodes: &mut Vec<&'a Value>) {
    nodes.push(value);
    if let Some(children) = value.get("children").and_then(Value::as_array) {
        for child in children {
            collect_nodes(child, nodes);
        }
    }
}

fn memory_value(raw: &str, key: &str) -> Option<u64> {
    raw.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name != key {
            return None;
        }
        let mut fields = value.split_whitespace();
        let number = fields.next()?.parse::<u64>().ok()?;
        if fields.next() != Some("kB") {
            return None;
        }
        number.checked_mul(1024)
    })
}

fn parse_swaps(raw: &str) -> Option<Vec<String>> {
    let mut lines = raw.lines();
    if lines.next()?.split_whitespace().next() != Some("Filename") {
        return None;
    }
    let mut swaps = Vec::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 5 {
            return None;
        }
        swaps.push(unescape_mount(fields[0]));
    }
    Some(swaps)
}

fn resume_targets(raw: &str) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    for line in raw
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
    {
        // Quoted GRUB command lines contain ordinary whitespace-delimited kernel
        // parameters. Full shell syntax is deliberately not executed/interpreted.
        let line = match line.split_once('=') {
            Some((key, value))
                if matches!(
                    key.trim(),
                    "GRUB_CMDLINE_LINUX" | "GRUB_CMDLINE_LINUX_DEFAULT"
                ) =>
            {
                value
            }
            _ => line,
        };
        for token in line
            .split_whitespace()
            .take_while(|token| !token.starts_with('#'))
        {
            let token = token.trim_matches(['\'', '"']);
            if let Some(target) = token
                .strip_prefix("resume=")
                .or_else(|| token.strip_prefix("RESUME="))
            {
                targets.insert(target.trim_matches(['\'', '"']).to_owned());
            }
        }
    }
    targets
}

fn node_matches(target: &str, node: &Value, aliases: &BTreeMap<String, Vec<String>>) -> bool {
    if let Some(uuid) = target.strip_prefix("UUID=") {
        return string(node, "uuid").is_some_and(|value| value.eq_ignore_ascii_case(uuid));
    }
    if let Some(uuid) = target.strip_prefix("PARTUUID=") {
        return string(node, "partuuid").is_some_and(|value| value.eq_ignore_ascii_case(uuid));
    }
    if let Some(uuid) = target.strip_prefix("/dev/disk/by-uuid/") {
        return string(node, "uuid").is_some_and(|value| value.eq_ignore_ascii_case(uuid));
    }
    if let Some(uuid) = target.strip_prefix("/dev/disk/by-partuuid/") {
        return string(node, "partuuid").is_some_and(|value| value.eq_ignore_ascii_case(uuid));
    }
    let path = string(node, "name").unwrap_or_default();
    target == path
        || string(node, "maj:min").as_deref() == Some(target)
        || aliases.get(&path).is_some_and(|names| {
            names
                .iter()
                .any(|name| target == format!("/dev/disk/by-id/{name}"))
        })
}

fn resolve_resume(
    source: &str,
    target: &str,
    nodes: &[&Value],
    topology_observed: bool,
    swaps: Option<&[String]>,
    aliases: &BTreeMap<String, Vec<String>>,
) -> ResumeReference {
    let matched: Vec<_> = nodes
        .iter()
        .copied()
        .filter(|node| node_matches(target, node, aliases))
        .collect();
    let matched_devices: Vec<_> = matched
        .iter()
        .filter_map(|node| string(node, "name"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let (resolution, note) = if target.eq_ignore_ascii_case("none") {
        (ResumeResolution::Disabled, "This individual configuration reference disables resume; other references and boot images may differ.")
    } else if !topology_observed || target.is_empty() || target.contains('$') {
        (
            ResumeResolution::Unknown,
            "Target could not be interpreted against an observed topology.",
        )
    } else if matched_devices.len() > 1 {
        (ResumeResolution::Ambiguous, "More than one observed device matches; duplicate identities cannot resolve a resume target.")
    } else if matched_devices.is_empty() {
        (ResumeResolution::Unresolved, "Target not present in observed topology; possibly stale or not visible, not proof that resume is unused.")
    } else if let Some(swaps) = swaps {
        if swaps
            .iter()
            .any(|swap| matched.iter().any(|node| node_matches(swap, node, aliases)))
        {
            (ResumeResolution::ActiveSwap, "Target resolves to observed active swap; retain it pending an explicit hibernation decision.")
        } else {
            (ResumeResolution::ObservedNotActiveSwap, "Target is observed but not active swap in this snapshot; absence from /proc/swaps does not make removal safe.")
        }
    } else {
        (
            ResumeResolution::Unknown,
            "Target resolved, but active swap observation is unavailable or malformed.",
        )
    };
    ResumeReference {
        source: source.into(),
        target: target.into(),
        resolution,
        matched_devices,
        note: note.into(),
    }
}

fn path_under(path: &str, root: &str) -> bool {
    root == "/"
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn unobserved_libvirt(note: &str) -> LibvirtInventory {
    LibvirtInventory {
        domain_uuids: Vec::new(),
        live_connection_attempted: false,
        resource_occupancy_verified: false,
        note: note.into(),
    }
}

/// Parse externally captured UUID-only output. Does not contact libvirt, inspect
/// guest XML, or infer device availability from an empty list.
pub fn parse_libvirt_inventory(raw: &str) -> Result<LibvirtInventory, String> {
    let mut uuids = BTreeSet::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let bytes = line.as_bytes();
        if bytes.len() != 36
            || !bytes.iter().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
        {
            return Err("expected UUID-only output from virsh --readonly list --all --uuid".into());
        }
        uuids.insert(line.to_ascii_lowercase());
    }
    Ok(LibvirtInventory {
        domain_uuids: uuids.into_iter().collect(), live_connection_attempted: false,
        resource_occupancy_verified: false,
        note: "Offline captured domain UUIDs only; freshness, running states, block-device and PCI ownership remain unverified.".into(),
    })
}

#[cfg(test)]
mod kvm_tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    #[test]
    fn wrong_kvm_nodes_are_refused_before_open() {
        let directory = tempfile::tempdir().unwrap();
        let regular = directory.path().join("regular");
        let fifo = directory.path().join("fifo");
        let link = directory.path().join("link");
        fs::write(&regular, b"untouched").unwrap();
        symlink("/dev/null", &link).unwrap();
        let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: NUL-terminated local temporary pathname, no shared device.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
        // Watch the private directory to establish that neither the regular
        // file nor FIFO was opened; a mistaken FIFO open must never block.
        // SAFETY: valid flags, followed by owning the returned descriptor.
        let watch = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(watch >= 0);
        use std::os::fd::FromRawFd;
        // SAFETY: the fresh descriptor has exactly one owner.
        let watch = unsafe { std::fs::File::from_raw_fd(watch) };
        let watched = CString::new(directory.path().as_os_str().as_bytes()).unwrap();
        // SAFETY: valid inotify descriptor and NUL-terminated path.
        assert!(
            unsafe { libc::inotify_add_watch(watch.as_raw_fd(), watched.as_ptr(), libc::IN_OPEN) }
                >= 0
        );
        for path in [
            regular.as_path(),
            fifo.as_path(),
            link.as_path(),
            directory.path(),
            Path::new("/dev/null"),
        ] {
            let report = probe_kvm_api_at(path, Some(true));
            assert_eq!(
                report.status,
                KvmApiStatus::InvalidNode,
                "{}",
                path.display()
            );
            assert!(!report.node_opened);
            assert!(!report.ioctl_attempted);
        }
        use std::io::Read;
        let mut events = [0; 512];
        assert_eq!(
            (&watch).read(&mut events).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(fs::read(regular).unwrap(), b"untouched");
    }

    #[test]
    fn missing_kvm_node_keeps_registered_kernel_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let report = probe_kvm_api_at(&directory.path().join("missing"), Some(true));
        assert_eq!(report.status, KvmApiStatus::Missing);
        assert_eq!(report.kernel_registered, Some(true));
        assert!(!report.node_opened);
        assert!(!report.ioctl_attempted);
        assert!(!report.api_verified);
    }

    #[test]
    fn denied_node_inspection_does_not_become_missing_or_open_a_device() {
        // Root bypasses Unix directory mode checks; the supported test mode is
        // unprivileged and must exercise a real EACCES, not a simulated error.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o0)).unwrap();
        let report = probe_kvm_api_at(&directory.path().join("kvm"), Some(true));
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(report.status, KvmApiStatus::AccessDenied);
        assert_eq!(report.errno, Some(libc::EACCES));
        assert!(!report.node_opened);
        assert!(!report.ioctl_attempted);
    }
}
