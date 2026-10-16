//! Opt-in, diskless Linux boot. TCG is the default; KVM requires a separate explicit call.
//!
//! The host kernel is copied read-only into a private directory; a tiny bundled
//! static PID 1 and a newc initramfs provide nonce-bound serial boot evidence.
//! QEMU invocation: <https://www.qemu.org/docs/master/system/invocation.html>.
//! Initramfs: <https://docs.kernel.org/driver-api/early-userspace/buffer-format.html>.

use super::{
    find_dependency, CommandEvidence, LabCheck, LabCleanup, LabDependency, LabStatus,
    OwnedDirectory,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const GUEST_MEMORY_MIB: u32 = 256;
pub const GUEST_TIMEOUT_SECONDS: u32 = 60;
pub const HOST_RESERVE_MIB: u64 = 8192;
pub const QEMU_OVERHEAD_MIB: u64 = 256;
const REQUIRED_AVAILABLE_MIB: u64 = GUEST_MEMORY_MIB as u64 + HOST_RESERVE_MIB + QEMU_OVERHEAD_MIB;
const OUTPUT_LIMIT: usize = 65536;
const MAX_KERNEL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_FIRMWARE_BYTES: u64 = 4 * 1024 * 1024;
// The emulated PC may request the APIC option ROM even while using TCG.
const FIRMWARE_FILES: [&str; 3] = ["bios-256k.bin", "linuxboot_dma.bin", "kvmvapic.bin"];
const INIT_SOURCE: &str = include_str!("../assets/guest-init.S");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestLabOptions {
    pub tool_directory: Option<PathBuf>,
    pub firmware_directory: Option<PathBuf>,
    pub kernel: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestEvidenceKind {
    NotRun,
    RealQemuTcgAttempt,
    RealQemuTcgGuestBoot,
    RealQemuKvmAttempt,
    RealQemuKvmGuestBoot,
    SimulatedHarness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestLabReport {
    pub schema_version: u32,
    pub status: LabStatus,
    pub evidence_kind: GuestEvidenceKind,
    pub acceleration: String,
    pub kvm_validated: bool,
    pub memory_mib: u32,
    pub vcpus: u32,
    pub timeout_seconds: u32,
    pub host_reserve_mib: u64,
    pub qemu_overhead_allowance_mib: u64,
    pub observed_mem_available_mib: Option<u64>,
    pub physical_device_access: bool,
    pub guest_disk_attached: bool,
    pub network_enabled: bool,
    pub dependencies: Vec<LabDependency>,
    pub checks: Vec<LabCheck>,
    pub commands: Vec<CommandEvidence>,
    pub kernel_sha256: Option<String>,
    pub init_sha256: Option<String>,
    pub initramfs_sha256: Option<String>,
    pub firmware_sha256: Vec<(String, String)>,
    pub serial_handshake: bool,
    pub guest_poweroff: bool,
    pub cleanup: LabCleanup,
    pub detail: String,
    pub limitations: Vec<String>,
}

impl GuestLabReport {
    fn pending(dependencies: Vec<LabDependency>) -> Self {
        Self {
            schema_version: 1,
            status: LabStatus::Skipped,
            evidence_kind: GuestEvidenceKind::NotRun,
            acceleration: "tcg".into(),
            kvm_validated: false,
            memory_mib: GUEST_MEMORY_MIB,
            vcpus: 1,
            timeout_seconds: GUEST_TIMEOUT_SECONDS,
            host_reserve_mib: HOST_RESERVE_MIB,
            qemu_overhead_allowance_mib: QEMU_OVERHEAD_MIB,
            observed_mem_available_mib: None,
            physical_device_access: false,
            guest_disk_attached: false,
            network_enabled: false,
            dependencies,
            checks: Vec::new(),
            commands: Vec::new(),
            kernel_sha256: None,
            init_sha256: None,
            initramfs_sha256: None,
            firmware_sha256: Vec::new(),
            serial_handshake: false,
            guest_poweroff: false,
            cleanup: LabCleanup { directory_created: false, removed: false, detail: "No temporary directory created".into() },
            detail: String::new(),
            limitations: vec![
                "Diskless Linux kernel and minimal PID 1 only; no installed OS or desktop login validation".into(),
                "TCG software emulation only; does not validate KVM, VFIO, libvirt, OVMF, or physical disk import".into(),
                "The selected QEMU and firmware bundle must be authenticated separately by the caller".into(),
            ],
        }
    }

    fn check(&mut self, name: &str, passed: bool, detail: &str) -> Result<(), String> {
        self.checks.push(LabCheck {
            name: name.into(),
            passed,
            detail: detail.into(),
        });
        if passed {
            Ok(())
        } else {
            Err(format!("{name}: {detail}"))
        }
    }

    fn dependency(&self, name: &str) -> Result<PathBuf, String> {
        self.dependencies
            .iter()
            .find(|d| d.name == name && d.available)
            .and_then(|d| d.path.clone())
            .ok_or_else(|| format!("Missing dependency: {name}"))
    }
}

/// Metadata-only inspection: no child process, temporary file, kernel read,
/// device probe, daemon connection, or automatic installation takes place.
pub fn inspect_guest_dependencies(options: &GuestLabOptions) -> Vec<LabDependency> {
    let mut dependencies = vec![match &options.tool_directory {
        Some(directory) => selected_dependency("qemu-system-x86_64", directory, true, None),
        None => find_dependency("qemu-system-x86_64"),
    }];
    dependencies.extend(["as", "ld", "timeout"].into_iter().map(find_dependency));
    let firmware = options
        .firmware_directory
        .as_deref()
        .unwrap_or(Path::new("/usr/share/qemu"));
    dependencies.extend(
        FIRMWARE_FILES
            .into_iter()
            .map(|name| selected_dependency(name, firmware, false, Some(MAX_FIRMWARE_BYTES))),
    );
    let validation = validate_input_file(&options.kernel, MAX_KERNEL_BYTES);
    dependencies.push(LabDependency {
        name: "linux_kernel".into(),
        path: validation.as_ref().ok().map(|_| options.kernel.clone()),
        available: validation.is_ok(),
        detail: validation.err().unwrap_or_else(|| {
            "Regular kernel input; will be copied into the private lab directory".into()
        }),
        provenance: "selected_read_only_kernel_input".into(),
    });
    dependencies
}

fn selected_dependency(
    name: &str,
    directory: &Path,
    executable: bool,
    limit: Option<u64>,
) -> LabDependency {
    let candidate = directory.join(name);
    let validation = (|| {
        validate_directory(directory)?;
        let metadata = validate_input_file(&candidate, limit.unwrap_or(512 * 1024 * 1024))?;
        if executable && (metadata.mode() & 0o111 == 0 || metadata.mode() & 0o6000 != 0) {
            return Err("Selected QEMU must be executable without set-id bits".into());
        }
        Ok(())
    })();
    LabDependency {
        name: name.into(),
        path: validation.as_ref().ok().map(|()| candidate),
        available: validation.is_ok(),
        detail: validation.err().unwrap_or_else(|| {
            "Fixed filename selected; bundle authenticity must be verified separately".into()
        }),
        provenance: "selected_tool_or_firmware_directory".into(),
    }
}

fn validate_directory(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || fs::canonicalize(path).map_err(|e| e.to_string())? != path {
        return Err("Input must use an absolute canonical path without symlinks".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.mode() & 0o022 != 0 {
        return Err(
            "Input directory must be a directory without group/other write permission".into(),
        );
    }
    Ok(())
}

fn validate_input_file(path: &Path, max_bytes: u64) -> Result<fs::Metadata, String> {
    if !path.is_absolute()
        || ["/dev", "/proc", "/sys"]
            .iter()
            .any(|p| path.starts_with(p))
    {
        return Err("Input must be an absolute ordinary file outside /dev, /proc, and /sys".into());
    }
    // lstat before canonicalization or open rejects FIFO/device endpoints without
    // opening them; the descriptor is independently checked before reading.
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o022 != 0 {
        return Err("Input must be a regular single-link file without symlinks or group/other write permission".into());
    }
    if metadata.len() == 0 || metadata.len() > max_bytes {
        return Err(format!(
            "Input file size must be between 1 and {max_bytes} bytes"
        ));
    }
    if fs::canonicalize(path).map_err(|e| e.to_string())? != path {
        return Err("Input path must be canonical without symlink components".into());
    }
    Ok(metadata)
}

pub fn run_guest_lab(options: &GuestLabOptions) -> GuestLabReport {
    run_selected_guest_lab(options, false)
}

/// Explicitly authorized diskless KVM experiment. Never falls back to TCG.
/// This creates a VM and therefore exceeds a read-only KVM API probe.
pub fn run_kvm_guest_lab(options: &GuestLabOptions) -> GuestLabReport {
    run_selected_guest_lab(options, true)
}

fn run_selected_guest_lab(options: &GuestLabOptions, kvm: bool) -> GuestLabReport {
    // SAFETY: getuid/geteuid take no arguments and have no memory preconditions.
    let root = unsafe { libc::getuid() == 0 || libc::geteuid() == 0 };
    let available = fs::read_to_string("/proc/meminfo")
        .map_err(|e| e.to_string())
        .and_then(|text| parse_available_memory(&text));
    run_with_dependencies(
        inspect_guest_dependencies(options),
        root,
        available,
        &mut RealGuestRunner { kvm },
    )
}

fn parse_available_memory(text: &str) -> Result<u64, String> {
    let entries: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("MemAvailable:"))
        .collect();
    if entries.len() != 1 {
        return Err("Expected exactly one MemAvailable entry".into());
    }
    let fields: Vec<_> = entries[0].split_whitespace().collect();
    if fields.len() != 2 || fields[1] != "kB" {
        return Err("Invalid MemAvailable units".into());
    }
    fields[0]
        .parse::<u64>()
        .map(|kb| kb / 1024)
        .map_err(|e| e.to_string())
}

fn run_with_dependencies(
    dependencies: Vec<LabDependency>,
    root: bool,
    available_memory: Result<u64, String>,
    runner: &mut impl GuestRunner,
) -> GuestLabReport {
    let mut report = GuestLabReport::pending(dependencies);
    if runner.uses_kvm() {
        report.acceleration = "kvm".into();
        report.limitations[1] = "Diskless KVM only; does not validate VFIO, libvirt, OVMF, physical disk import or production workloads".into();
    }
    if root {
        report.status = LabStatus::Refused;
        report.detail = "Guest laboratory must run as a non-root user".into();
        return report;
    }
    if report.dependencies.iter().any(|d| !d.available) {
        report.detail =
            "Guest laboratory skipped because required inputs are missing or unsafe".into();
        return report;
    }
    let available = match available_memory {
        Ok(available) => available,
        Err(error) => {
            report.status = LabStatus::Refused;
            report.detail = format!("Could not validate current available-memory budget: {error}");
            return report;
        }
    };
    report.observed_mem_available_mib = Some(available);
    let detail = format!("MemAvailable={available} MiB; requires {REQUIRED_AVAILABLE_MIB} MiB including {HOST_RESERVE_MIB} MiB Host reserve and {QEMU_OVERHEAD_MIB} MiB QEMU allowance");
    if report
        .check(
            "fresh_memory_budget",
            available >= REQUIRED_AVAILABLE_MIB,
            &detail,
        )
        .is_err()
    {
        report.status = LabStatus::Refused;
        report.detail = detail;
        return report;
    }
    let mut directory = match OwnedDirectory::create() {
        Ok(directory) => directory,
        Err(error) => {
            report.status = LabStatus::Failed;
            report.detail = format!("Could not create private lab directory: {error}");
            return report;
        }
    };
    report.cleanup.directory_created = true;
    match run_checks(&directory, runner, &mut report) {
        Ok(()) => {
            report.status = LabStatus::Passed;
            if report.evidence_kind == GuestEvidenceKind::RealQemuTcgAttempt {
                report.evidence_kind = GuestEvidenceKind::RealQemuTcgGuestBoot;
            }
            if report.evidence_kind == GuestEvidenceKind::RealQemuKvmAttempt {
                report.evidence_kind = GuestEvidenceKind::RealQemuKvmGuestBoot;
                report.kvm_validated = true;
            }
            report.detail = format!(
                "Diskless Linux reached PID 1 and powered off cleanly under QEMU {}",
                report.acceleration
            );
        }
        Err(error) => {
            report.status = LabStatus::Failed;
            report.detail = error;
        }
    }
    match directory.remove() {
        Ok(()) => {
            report.cleanup.removed = true;
            report.cleanup.detail =
                "Private kernel, firmware copies, initramfs, and build files removed".into();
        }
        Err(error) => {
            report.status = LabStatus::Failed;
            report.cleanup.detail =
                format!("Cleanup failed at {}: {error}", directory.path.display());
        }
    }
    report
}

fn run_checks(
    directory: &OwnedDirectory,
    runner: &mut impl GuestRunner,
    report: &mut GuestLabReport,
) -> Result<(), String> {
    execute(
        report,
        runner,
        directory,
        "qemu_version",
        "qemu-system-x86_64",
        &["--version"],
        15,
    )?;
    let kernel = directory.path.join("kernel");
    report.kernel_sha256 = Some(copy_input(
        &report.dependency("linux_kernel")?,
        &kernel,
        MAX_KERNEL_BYTES,
        directory,
    )?);
    for name in FIRMWARE_FILES {
        let digest = copy_input(
            &report.dependency(name)?,
            &directory.path.join(name),
            MAX_FIRMWARE_BYTES,
            directory,
        )?;
        report.firmware_sha256.push((name.into(), digest));
    }
    report.check(
        "private_read_only_inputs",
        true,
        "Kernel and fixed firmware files copied to private read-only regular files",
    )?;

    let nonce = directory
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid lab directory")?;
    let nonce = format!("{:x}", Sha256::digest(nonce.as_bytes()));
    let source = INIT_SOURCE.replace("@NONCE@", &nonce);
    let source_path = directory.path.join("init.S");
    write_owned(directory, &source_path, source.as_bytes(), 0o400)?;
    let object = directory.path.join("init.o");
    let init = directory.path.join("init");
    execute(
        report,
        runner,
        directory,
        "assemble_pid1",
        "as",
        &["--64", "-o", path_str(&object)?, path_str(&source_path)?],
        15,
    )?;
    directory.validate_file(&object)?;
    execute(
        report,
        runner,
        directory,
        "link_pid1",
        "ld",
        &[
            "-m",
            "elf_x86_64",
            "-static",
            "--build-id=none",
            "-z",
            "noexecstack",
            "-o",
            path_str(&init)?,
            path_str(&object)?,
        ],
        15,
    )?;
    directory.validate_file(&init)?;
    let init_bytes = fs::read(&init).map_err(|e| e.to_string())?;
    report.check(
        "static_pid1_elf",
        init_bytes.starts_with(b"\x7fELF") && init_bytes.get(4) == Some(&2),
        "Built x86-64 ELF PID 1 from bundled assembly using a static link",
    )?;
    report.init_sha256 = Some(format!("{:x}", Sha256::digest(&init_bytes)));
    let archive = initramfs(&init_bytes);
    report.initramfs_sha256 = Some(format!("{:x}", Sha256::digest(&archive)));
    let initrd = directory.path.join("initramfs.cpio");
    write_owned(directory, &initrd, &archive, 0o400)?;
    let args = qemu_args(directory, &kernel, &initrd, runner.uses_kvm())?;
    let refs: Vec<_> = args.iter().map(String::as_str).collect();
    report.evidence_kind = runner.evidence_kind();
    execute(
        report,
        runner,
        directory,
        "boot_diskless_linux",
        "qemu-system-x86_64",
        &refs,
        GUEST_TIMEOUT_SECONDS,
    )?;
    let result = report.commands.last().ok_or("Missing boot evidence")?;
    let outcome = evaluate_boot(result, &nonce);
    report.serial_handshake = outcome.handshake;
    report.guest_poweroff = outcome.poweroff;
    report.check(
        "serial_pid1_handshake",
        outcome.handshake,
        "Serial output contains the generated nonce from the bundled guest PID 1",
    )?;
    report.check("clean_guest_poweroff", outcome.poweroff, "Guest requested poweroff; kernel confirmed power down and QEMU exited zero without timeout")?;
    Ok(())
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "Input path is not UTF-8".into())
}

fn copy_input(
    source: &Path,
    destination: &Path,
    max_bytes: u64,
    directory: &OwnedDirectory,
) -> Result<String, String> {
    let before = validate_input_file(source, max_bytes)?;
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)
        .map_err(|e| e.to_string())?;
    let opened = input.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file()
        || opened.dev() != before.dev()
        || opened.ino() != before.ino()
        || opened.nlink() != 1
    {
        return Err("Input changed between metadata validation and read-only open".into());
    }
    let mut bytes = Vec::new();
    (&mut input)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = input.metadata().map_err(|e| e.to_string())?;
    if bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
        || after.ctime() != before.ctime()
        || after.ctime_nsec() != before.ctime_nsec()
    {
        return Err("Input changed while being copied; guest boot refused".into());
    }
    write_owned(directory, destination, &bytes, 0o400)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

fn write_owned(
    directory: &OwnedDirectory,
    path: &Path,
    bytes: &[u8],
    mode: u32,
) -> Result<(), String> {
    directory.validate_new_path(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|e| e.to_string())?;
    directory.validate_file(path)
}

fn qemu_args(
    directory: &OwnedDirectory,
    kernel: &Path,
    initrd: &Path,
    kvm: bool,
) -> Result<Vec<String>, String> {
    Ok([
        "-no-user-config",
        "-nodefaults",
        "-machine",
        if kvm {
            "pc,accel=kvm,mem-merge=off,dump-guest-core=off"
        } else {
            "pc,accel=tcg,mem-merge=off,dump-guest-core=off"
        },
        "-cpu",
        "qemu64",
        "-smp",
        "1",
        "-m",
        "256",
        "-display",
        "none",
        "-serial",
        "stdio",
        "-monitor",
        "none",
        "-nic",
        "none",
        "-no-reboot",
        "-sandbox",
        "on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny",
        "-L",
        path_str(&directory.path)?,
        "-bios",
        path_str(&directory.path.join("bios-256k.bin"))?,
        "-kernel",
        path_str(kernel)?,
        "-initrd",
        path_str(initrd)?,
        "-append",
        "console=ttyS0,115200 rdinit=/init panic=-1 quiet nokaslr",
    ]
    .into_iter()
    .map(String::from)
    .collect())
}

/// Build only the fixed in-memory guest tree. Special file metadata is encoded
/// in the archive; no host mknod or device open is performed.
fn initramfs(init: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    cpio_entry(&mut bytes, 1, "dev", 0o040755, 0, 0, &[]);
    cpio_entry(&mut bytes, 2, "dev/console", 0o020600, 5, 1, &[]);
    cpio_entry(&mut bytes, 3, "init", 0o100755, 0, 0, init);
    cpio_entry(&mut bytes, 4, "TRAILER!!!", 0, 0, 0, &[]);
    bytes
}

fn cpio_entry(
    archive: &mut Vec<u8>,
    inode: u32,
    name: &str,
    mode: u32,
    rdev_major: u32,
    rdev_minor: u32,
    data: &[u8],
) {
    archive.extend_from_slice(b"070701");
    for field in [
        inode,
        mode,
        0,
        0,
        1,
        0,
        data.len() as u32,
        0,
        0,
        rdev_major,
        rdev_minor,
        name.len() as u32 + 1,
        0,
    ] {
        archive.extend_from_slice(format!("{field:08x}").as_bytes());
    }
    archive.extend_from_slice(name.as_bytes());
    archive.push(0);
    while archive.len() % 4 != 0 {
        archive.push(0);
    }
    archive.extend_from_slice(data);
    while archive.len() % 4 != 0 {
        archive.push(0);
    }
}

struct BootOutcome {
    handshake: bool,
    poweroff: bool,
}

fn evaluate_boot(result: &CommandEvidence, nonce: &str) -> BootOutcome {
    let lines: Vec<_> = result
        .stdout
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let ready = format!("ELASTICPC_GUEST_READY:{nonce}:PID1");
    let poweroff = format!("ELASTICPC_GUEST_POWEROFF:{nonce}");
    let ready_at = lines.iter().position(|line| *line == ready);
    let poweroff_at = lines.iter().position(|line| *line == poweroff);
    let clean = result.exit_code == Some(0)
        && !result.timed_out
        && !result.output_truncated
        && !result.stdout.contains("ELASTICPC_GUEST_FAILURE")
        && !result.stdout.contains("Kernel panic");
    BootOutcome {
        handshake: ready_at.is_some() && clean,
        poweroff: clean
            && matches!((ready_at, poweroff_at), (Some(a), Some(b)) if a < b)
            && lines
                .iter()
                .any(|line| line.ends_with("reboot: Power down")),
    }
}

trait GuestRunner {
    fn uses_kvm(&self) -> bool {
        false
    }
    fn evidence_kind(&self) -> GuestEvidenceKind;
    fn run(
        &mut self,
        timeout: &Path,
        program: &Path,
        args: &[&str],
        directory: &Path,
        seconds: u32,
    ) -> Result<CommandEvidence, String>;
}

struct RealGuestRunner {
    kvm: bool,
}

impl GuestRunner for RealGuestRunner {
    fn uses_kvm(&self) -> bool {
        self.kvm
    }

    fn evidence_kind(&self) -> GuestEvidenceKind {
        if self.kvm {
            GuestEvidenceKind::RealQemuKvmAttempt
        } else {
            GuestEvidenceKind::RealQemuTcgAttempt
        }
    }

    fn run(
        &mut self,
        timeout: &Path,
        program: &Path,
        args: &[&str],
        directory: &Path,
        seconds: u32,
    ) -> Result<CommandEvidence, String> {
        let mut child = Command::new(timeout)
            .args(["--signal=KILL", &format!("{seconds}s")])
            .arg(program)
            .args(args)
            .current_dir(directory)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .env("TMPDIR", directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdout = child.stdout.take().ok_or("Missing stdout pipe")?;
        let stderr = child.stderr.take().ok_or("Missing stderr pipe")?;
        let out_reader = std::thread::spawn(move || drain_output(stdout));
        let err_reader = std::thread::spawn(move || drain_output(stderr));
        // GNU timeout owns the fixed deadline. Waiting is event driven, never a
        // status-poll/retry loop, and no process survives successful cleanup.
        let status = child.wait().map_err(|e| e.to_string())?;
        let (stdout, out_truncated) = out_reader
            .join()
            .map_err(|_| "stdout reader panicked")?
            .map_err(|e| e.to_string())?;
        let (stderr, err_truncated) = err_reader
            .join()
            .map_err(|_| "stderr reader panicked")?
            .map_err(|e| e.to_string())?;
        Ok(CommandEvidence {
            name: String::new(),
            program: program.into(),
            args: args.iter().map(|a| (*a).into()).collect(),
            exit_code: status.code(),
            timed_out: matches!(status.code(), Some(124 | 137))
                || status.signal() == Some(libc::SIGKILL),
            stdout,
            stderr,
            output_truncated: out_truncated || err_truncated,
        })
    }
}

fn drain_output(mut stream: impl Read) -> io::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let keep = count.min(OUTPUT_LIMIT - bytes.len());
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep != count;
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

fn execute(
    report: &mut GuestLabReport,
    runner: &mut impl GuestRunner,
    directory: &OwnedDirectory,
    label: &str,
    tool: &str,
    args: &[&str],
    seconds: u32,
) -> Result<(), String> {
    let mut result = runner.run(
        &report.dependency("timeout")?,
        &report.dependency(tool)?,
        args,
        &directory.path,
        seconds,
    )?;
    result.name = label.into();
    let passed = result.exit_code == Some(0) && !result.timed_out;
    let detail = if passed {
        "Command exited successfully within its deadline".into()
    } else {
        format!(
            "exit={:?}, timed_out={}, stderr={}",
            result.exit_code, result.timed_out, result.stderr
        )
    };
    report.commands.push(result);
    report.check(label, passed, &detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct FailingRunner {
        calls: usize,
        directory: Option<PathBuf>,
        timed_out: bool,
    }
    impl GuestRunner for FailingRunner {
        fn evidence_kind(&self) -> GuestEvidenceKind {
            GuestEvidenceKind::SimulatedHarness
        }
        fn run(
            &mut self,
            _: &Path,
            program: &Path,
            args: &[&str],
            directory: &Path,
            _: u32,
        ) -> Result<CommandEvidence, String> {
            self.calls += 1;
            self.directory = Some(directory.into());
            Ok(CommandEvidence {
                name: String::new(),
                program: program.into(),
                args: args.iter().map(|a| (*a).into()).collect(),
                exit_code: Some(if self.timed_out { 0 } else { 1 }),
                timed_out: self.timed_out,
                stdout: String::new(),
                stderr: "Injected command failure".into(),
                output_truncated: false,
            })
        }
    }

    fn dependencies(available: bool) -> Vec<LabDependency> {
        ["qemu-system-x86_64", "as", "ld", "timeout"]
            .into_iter()
            .map(|name| LabDependency {
                name: name.into(),
                path: available.then(|| PathBuf::from(format!("/usr/bin/{name}"))),
                available,
                detail: "fixture".into(),
                provenance: "unit_test_fixture".into(),
            })
            .collect()
    }

    #[test]
    fn missing_dependencies_and_root_never_create_or_execute() {
        for (available, root, expected) in [
            (false, false, LabStatus::Skipped),
            (true, true, LabStatus::Refused),
        ] {
            let mut runner = FailingRunner {
                calls: 0,
                directory: None,
                timed_out: false,
            };
            let report = run_with_dependencies(
                dependencies(available),
                root,
                Ok(REQUIRED_AVAILABLE_MIB),
                &mut runner,
            );
            assert_eq!(report.status, expected);
            assert_eq!(report.evidence_kind, GuestEvidenceKind::NotRun);
            assert!(!report.cleanup.directory_created);
            assert_eq!(runner.calls, 0);
        }
    }

    #[test]
    fn command_failure_and_timeout_fail_and_cleanup() {
        for timed_out in [false, true] {
            let mut runner = FailingRunner {
                calls: 0,
                directory: None,
                timed_out,
            };
            let report = run_with_dependencies(
                dependencies(true),
                false,
                Ok(REQUIRED_AVAILABLE_MIB),
                &mut runner,
            );
            assert_eq!(report.status, LabStatus::Failed);
            assert!(!report.serial_handshake);
            assert!(!report.guest_poweroff);
            assert!(!report.kvm_validated);
            assert!(report.cleanup.removed);
            assert_eq!(runner.calls, 1);
            assert!(!runner.directory.unwrap().exists());
        }
    }

    #[test]
    fn insufficient_or_unknown_memory_refuses_before_any_execution() {
        assert_eq!(
            parse_available_memory("MemAvailable: 8912896 kB\n").unwrap(),
            REQUIRED_AVAILABLE_MIB
        );
        assert!(parse_available_memory("MemAvailable: 1 MB\n").is_err());
        assert!(parse_available_memory("MemAvailable: 1 kB\nMemAvailable: 2 kB\n").is_err());
        for available in [
            Ok(REQUIRED_AVAILABLE_MIB - 1),
            Err("missing MemAvailable".into()),
        ] {
            let mut runner = FailingRunner {
                calls: 0,
                directory: None,
                timed_out: false,
            };
            let report = run_with_dependencies(dependencies(true), false, available, &mut runner);
            assert_eq!(report.status, LabStatus::Refused);
            assert!(!report.cleanup.directory_created);
            assert_eq!(runner.calls, 0);
        }
    }

    fn boot_output(stdout: &str) -> CommandEvidence {
        CommandEvidence {
            name: "boot".into(),
            program: "/usr/bin/qemu-system-x86_64".into(),
            args: Vec::new(),
            exit_code: Some(0),
            timed_out: false,
            stdout: stdout.into(),
            stderr: String::new(),
            output_truncated: false,
        }
    }

    #[test]
    fn requires_nonce_bound_pid1_then_kernel_poweroff_and_clean_exit() {
        let output = "ELASTICPC_GUEST_READY:abc:PID1\r\nELASTICPC_GUEST_POWEROFF:abc\r\n[ 1.23] reboot: Power down\r\n";
        let valid = boot_output(output);
        assert!(evaluate_boot(&valid, "abc").handshake);
        assert!(evaluate_boot(&valid, "abc").poweroff);
        assert!(!evaluate_boot(&valid, "other").handshake);
        for invalid in [
            "",
            "Kernel command line: ELASTICPC_GUEST_READY:abc:PID1\n",
            "ELASTICPC_GUEST_POWEROFF:abc\nELASTICPC_GUEST_READY:abc:PID1\nreboot: Power down\n",
            "ELASTICPC_GUEST_READY:abc:PID1\nELASTICPC_GUEST_POWEROFF:abc\n",
        ] {
            assert!(!evaluate_boot(&boot_output(invalid), "abc").poweroff);
        }
        for modified in [
            CommandEvidence {
                timed_out: true,
                ..valid.clone()
            },
            CommandEvidence {
                exit_code: Some(1),
                ..valid.clone()
            },
            CommandEvidence {
                output_truncated: true,
                ..valid.clone()
            },
            CommandEvidence {
                stdout: format!("{output}Kernel panic"),
                ..valid
            },
        ] {
            assert!(!evaluate_boot(&modified, "abc").poweroff);
        }
    }

    #[test]
    fn newc_archive_encodes_only_guest_console_and_pid1() {
        let archive = initramfs(b"ELF-test");
        let mut offset = 0;
        let mut entries = Vec::new();
        while offset < archive.len() {
            assert_eq!(&archive[offset..offset + 6], b"070701");
            let field = |i: usize| {
                u32::from_str_radix(
                    std::str::from_utf8(&archive[offset + 6 + i * 8..offset + 14 + i * 8]).unwrap(),
                    16,
                )
                .unwrap()
            };
            let (mode, size, major, minor, name_size) =
                (field(1), field(6), field(9), field(10), field(11));
            let name =
                std::str::from_utf8(&archive[offset + 110..offset + 109 + name_size as usize])
                    .unwrap()
                    .to_string();
            offset = (offset + 110 + name_size as usize + 3) & !3;
            let data = archive[offset..offset + size as usize].to_vec();
            offset = (offset + size as usize + 3) & !3;
            entries.push((name, mode, major, minor, data));
        }
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[1], ("dev/console".into(), 0o020600, 5, 1, vec![]));
        assert_eq!(
            entries[2],
            ("init".into(), 0o100755, 0, 0, b"ELF-test".to_vec())
        );
        assert_eq!(entries[3].0, "TRAILER!!!");
    }

    #[test]
    fn input_guards_reject_symlinks_hardlinks_devices_and_writable_files() {
        let directory = OwnedDirectory::create().unwrap();
        let file = directory.path.join("kernel");
        write_owned(&directory, &file, b"kernel bytes", 0o400).unwrap();
        assert!(validate_input_file(&file, 100).is_ok());
        assert!(validate_input_file(&file, 2).is_err());
        assert!(validate_input_file(Path::new("/dev/null"), 100).is_err());
        let link = directory.path.join("symlink");
        symlink(&file, &link).unwrap();
        assert!(validate_input_file(&link, 100).is_err());
        let hard = directory.path.join("hardlink");
        fs::hard_link(&file, &hard).unwrap();
        assert!(validate_input_file(&file, 100).is_err());
        fs::remove_file(hard).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(validate_input_file(&file, 100).is_err());
        let fifo = directory.path.join("fifo");
        let c_path = std::ffi::CString::new(path_str(&fifo).unwrap()).unwrap();
        // SAFETY: c_path is a valid NUL-terminated path in the owned test directory.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(validate_input_file(&fifo, 100).is_err());
    }

    #[test]
    fn dependency_inspection_is_metadata_only_and_missing_is_not_evidence() {
        let directory = OwnedDirectory::create().unwrap();
        let options = GuestLabOptions {
            tool_directory: Some(directory.path.clone()),
            firmware_directory: Some(directory.path.clone()),
            kernel: directory.path.join("absent"),
        };
        let before = fs::read_dir(&directory.path).unwrap().count();
        let dependencies = inspect_guest_dependencies(&options);
        assert!(dependencies.iter().any(|d| !d.available));
        assert_eq!(fs::read_dir(&directory.path).unwrap().count(), before);
        let report = GuestLabReport::pending(dependencies);
        assert_eq!(report.evidence_kind, GuestEvidenceKind::NotRun);
        assert!(!report.cleanup.directory_created);
        assert!(report.commands.is_empty());
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<GuestLabReport>(&json).unwrap(),
            report
        );
    }

    #[test]
    fn qemu_arguments_force_diskless_networkless_tcg() {
        let directory = OwnedDirectory::create().unwrap();
        let args = qemu_args(
            &directory,
            &directory.path.join("kernel"),
            &directory.path.join("initramfs.cpio"),
            false,
        )
        .unwrap();
        assert!(args.iter().any(|v| v.contains("accel=tcg")));
        assert!(args.windows(2).any(|v| v == ["-nic", "none"]));
        assert!(args.windows(2).any(|v| v == ["-smp", "1"]));
        for forbidden in [
            "-drive",
            "-blockdev",
            "-enable-kvm",
            "-virtfs",
            "-netdev",
            "-device",
        ] {
            assert!(!args.iter().any(|v| v == forbidden));
        }
        assert!(!args
            .iter()
            .any(|v| v.contains("/dev/") || v.contains("@NONCE@")));
    }

    #[test]
    fn kvm_arguments_have_no_fallback_or_storage_and_keep_limits() {
        let directory = OwnedDirectory::create().unwrap();
        let args = qemu_args(
            &directory,
            &directory.path.join("kernel"),
            &directory.path.join("initramfs.cpio"),
            true,
        )
        .unwrap();
        assert!(args
            .windows(2)
            .any(|v| v == ["-machine", "pc,accel=kvm,mem-merge=off,dump-guest-core=off"]));
        assert!(!args.iter().any(|v| v.contains("tcg")));
        for pair in [
            ["-nic", "none"],
            ["-smp", "1"],
            ["-m", "256"],
            ["-monitor", "none"],
        ] {
            assert!(args.windows(2).any(|v| v == pair));
        }
        for forbidden in [
            "-drive",
            "-blockdev",
            "-virtfs",
            "-netdev",
            "-device",
            "-daemonize",
        ] {
            assert!(!args.iter().any(|v| v == forbidden));
        }
        assert!(!args.iter().any(|v| v.contains("/dev/")));
    }

    #[test]
    fn unavailable_kvm_inputs_do_not_claim_validation_or_create_files() {
        let mut runner = RealGuestRunner { kvm: true };
        let report = run_with_dependencies(
            dependencies(false),
            false,
            Ok(REQUIRED_AVAILABLE_MIB),
            &mut runner,
        );
        assert_eq!(report.acceleration, "kvm");
        assert_eq!(report.status, LabStatus::Skipped);
        assert_eq!(report.evidence_kind, GuestEvidenceKind::NotRun);
        assert!(!report.kvm_validated);
        assert!(!report.cleanup.directory_created);
        assert!(report.commands.is_empty());
    }

    #[test]
    fn kvm_root_and_insufficient_memory_are_refused_before_execution() {
        for (root, memory) in [
            (true, REQUIRED_AVAILABLE_MIB),
            (false, REQUIRED_AVAILABLE_MIB - 1),
        ] {
            let report = run_with_dependencies(
                dependencies(true),
                root,
                Ok(memory),
                &mut RealGuestRunner { kvm: true },
            );
            assert_eq!(report.status, LabStatus::Refused);
            assert_eq!(report.evidence_kind, GuestEvidenceKind::NotRun);
            assert!(!report.kvm_validated);
            assert!(!report.cleanup.directory_created);
            assert!(report.commands.is_empty());
        }
    }

    #[test]
    #[ignore = "requires authenticated QEMU system/firmware bundle and readable Linux kernel; boots a private diskless TCG guest"]
    fn real_qemu_tcg_guest_boot() {
        let options = GuestLabOptions {
            tool_directory: std::env::var_os("ELASTICPC_TEST_GUEST_TOOL_DIR").map(PathBuf::from),
            firmware_directory: std::env::var_os("ELASTICPC_TEST_GUEST_FIRMWARE_DIR")
                .map(PathBuf::from),
            kernel: std::env::var_os("ELASTICPC_TEST_GUEST_KERNEL")
                .map(PathBuf::from)
                .expect("explicit ELASTICPC_TEST_GUEST_KERNEL is required for real boot"),
        };
        let report = run_guest_lab(&options);
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        assert_eq!(report.status, LabStatus::Passed, "{report:#?}");
        assert_eq!(
            report.evidence_kind,
            GuestEvidenceKind::RealQemuTcgGuestBoot
        );
        assert!(report.serial_handshake && report.guest_poweroff && report.cleanup.removed);
        assert!(!report.kvm_validated);
    }
}
