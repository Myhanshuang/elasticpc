//! Opt-in QEMU block-I/O laboratory using only newly created ordinary files.
//!
//! The separate [`guest`] module provides a diskless Linux boot laboratory (TCG by default, explicit KVM opt-in).
//! Neither laboratory grants permission to access a physical disk or libvirt.
//! The block-I/O API accepts no image paths or commands. A separately verified
//! tool directory may supply only the fixed qemu-img and qemu-io executables.
//! QEMU syntax: https://www.qemu.org/docs/master/tools/qemu-img.html and
//! https://gitlab.com/qemu-project/qemu/-/blob/master/qemu-io-cmds.c.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod guest;

const IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 8192;
const COMMAND_TIMEOUT_SECONDS: u32 = 30;
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabStatus {
    Passed,
    Skipped,
    Failed,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    NotRun,
    RealQemuFileIo,
    SimulatedHarness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabDependency {
    pub name: String,
    pub path: Option<PathBuf>,
    pub available: bool,
    pub detail: String,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEvidence {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabCleanup {
    pub directory_created: bool,
    pub removed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLabReport {
    pub schema_version: u32,
    pub status: LabStatus,
    pub evidence_kind: EvidenceKind,
    pub dependencies: Vec<LabDependency>,
    pub checks: Vec<LabCheck>,
    pub commands: Vec<CommandEvidence>,
    pub image_bytes: u64,
    pub baseline_sha256: Option<String>,
    pub after_overlay_sha256: Option<String>,
    pub after_base_change_sha256: Option<String>,
    pub cleanup: LabCleanup,
    pub detail: String,
    pub limitations: Vec<String>,
}

impl FileLabReport {
    fn pending(dependencies: Vec<LabDependency>) -> Self {
        Self {
            schema_version: 1,
            status: LabStatus::Skipped,
            evidence_kind: EvidenceKind::NotRun,
            dependencies,
            checks: Vec::new(),
            commands: Vec::new(),
            image_bytes: IMAGE_BYTES,
            baseline_sha256: None,
            after_overlay_sha256: None,
            after_base_change_sha256: None,
            cleanup: LabCleanup {
                directory_created: false,
                removed: false,
                detail: "No temporary directory created".into(),
            },
            detail: String::new(),
            limitations: vec![
                "File-backed QEMU block I/O only; no guest OS boot or KVM validation".into(),
                "Does not validate physical disks, VFIO, libvirt, EFI, or production services".into(),
                "The full-file SHA-256 gate is a lab binding, not the physical-disk fingerprint policy".into(),
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
}

/// Read-only executable discovery. PATH and environment overrides are ignored.
pub fn inspect_dependencies() -> Vec<LabDependency> {
    ["qemu-img", "qemu-io", "timeout"]
        .into_iter()
        .map(find_dependency)
        .collect()
}

/// Inspect a caller-selected, separately verified QEMU tool bundle. This does
/// not verify a package signature: callers must provision and verify the bundle.
/// The timeout executable still comes exclusively from fixed system directories.
pub fn inspect_dependencies_in(directory: &Path) -> Vec<LabDependency> {
    let validate = || -> Result<(), String> {
        if !directory.is_absolute()
            || fs::canonicalize(directory).map_err(|e| e.to_string())? != directory
        {
            return Err(
                "Tool directory must be an absolute canonical path without symlinks".into(),
            );
        }
        let metadata = fs::symlink_metadata(directory).map_err(|e| e.to_string())?;
        if !metadata.is_dir() || metadata.mode() & 0o022 != 0 {
            return Err(
                "Tool directory must be a regular directory without group/other write permissions"
                    .into(),
            );
        }
        Ok(())
    };
    let validation = validate();
    let mut dependencies: Vec<_> = ["qemu-img", "qemu-io"].into_iter().map(|name| {
        let candidate = directory.join(name);
        let result = validation.clone().and_then(|()| {
            let metadata = fs::symlink_metadata(&candidate).map_err(|e| e.to_string())?;
            if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o6022 != 0 || metadata.mode() & 0o111 == 0 {
                return Err("Selected tool must be a regular executable with one link, no symlink, no set-id bits, and no group/other write permission".into());
            }
            Ok(())
        });
        LabDependency {
            name: name.into(), available: result.is_ok(), path: result.is_ok().then_some(candidate),
            detail: result.err().unwrap_or_else(|| "Fixed QEMU executable selected; package authenticity must be verified by the caller".into()),
            provenance: "user_selected_tool_directory".into(),
        }
    }).collect();
    dependencies.push(find_dependency("timeout"));
    dependencies
}

fn find_dependency(name: &str) -> LabDependency {
    let path = ["/usr/bin", "/bin"].into_iter().find_map(|directory| {
        let candidate = Path::new(directory).join(name);
        let canonical = fs::canonicalize(candidate).ok()?;
        // Both the location and basename must remain inside the fixed system set.
        if canonical.file_name()? != name
            || !matches!(canonical.parent()?.to_str(), Some("/usr/bin" | "/bin"))
        {
            return None;
        }
        let metadata = fs::symlink_metadata(&canonical).ok()?;
        (metadata.is_file() && metadata.mode() & 0o6022 == 0 && metadata.mode() & 0o111 != 0)
            .then_some(canonical)
    });
    LabDependency {
        name: name.into(),
        available: path.is_some(),
        detail: if path.is_some() {
            "Executable found in fixed system directory; runtime version recorded if executed"
                .into()
        } else {
            "Missing executable in /usr/bin or /bin; no installation attempted".into()
        },
        path,
        provenance: "fixed_system_directory".into(),
    }
}

/// Execute an explicitly requested lab. Missing dependencies produce `Skipped`,
/// never a passing result. Root execution is refused before creating any files.
pub fn run_file_lab() -> FileLabReport {
    let dependencies = inspect_dependencies();
    // SAFETY: These libc calls have no arguments or memory preconditions.
    let root = unsafe { libc::getuid() == 0 || libc::geteuid() == 0 };
    run_with_dependencies(dependencies, root, &mut RealRunner)
}

/// Like `run_file_lab`, with a separately verified tool bundle. Only fixed QEMU
/// executable names are selected; there is no image path or command override.
pub fn run_file_lab_with_tools(directory: &Path) -> FileLabReport {
    let dependencies = inspect_dependencies_in(directory);
    // SAFETY: These libc calls have no arguments or memory preconditions.
    let root = unsafe { libc::getuid() == 0 || libc::geteuid() == 0 };
    run_with_dependencies(dependencies, root, &mut RealRunner)
}

fn run_with_dependencies(
    dependencies: Vec<LabDependency>,
    root: bool,
    runner: &mut impl Runner,
) -> FileLabReport {
    let mut report = FileLabReport::pending(dependencies);
    if root {
        report.status = LabStatus::Refused;
        report.detail = "File laboratory must run as a non-root user".into();
        return report;
    }
    if report.dependencies.iter().any(|item| !item.available) {
        report.detail =
            "QEMU file laboratory skipped because required executables are missing".into();
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
    report.evidence_kind = runner.evidence_kind();
    match run_checks(&directory, runner, &mut report) {
        Ok(()) => {
            report.status = LabStatus::Passed;
            report.detail = "Owned-file COW isolation and stale-base refusal checks passed".into();
        }
        Err(error) => {
            report.status = LabStatus::Failed;
            report.detail = error;
        }
    }
    match directory.remove() {
        Ok(()) => {
            report.cleanup.removed = true;
            report.cleanup.detail = "All generated images and the private directory removed".into();
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
    runner: &mut impl Runner,
    report: &mut FileLabReport,
) -> Result<(), String> {
    execute(
        report,
        runner,
        directory,
        "qemu_img_version",
        "qemu-img",
        &["--version"],
    )?;
    execute(
        report,
        runner,
        directory,
        "qemu_io_version",
        "qemu-io",
        &["--version"],
    )?;
    let base = directory.path.join("base.raw");
    let overlay = directory.path.join("overlay.qcow2");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&base)
        .map_err(|error| error.to_string())?;
    for _ in 0..IMAGE_BYTES / 65536 {
        file.write_all(&[0x11; 65536])
            .map_err(|error| error.to_string())?;
    }
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    directory.validate_file(&base)?;
    fs::set_permissions(&base, fs::Permissions::from_mode(0o400))
        .map_err(|error| error.to_string())?;
    let baseline = hash_owned_file(directory, &base)?;
    report.baseline_sha256 = Some(baseline.clone());
    report.check(
        "base_read_only",
        fs::metadata(&base).map_err(|e| e.to_string())?.mode() & 0o222 == 0,
        "Owned raw base has no write permission bits during QEMU I/O",
    )?;
    let base_arg = base.to_str().ok_or("Non-UTF-8 lab path")?;
    let overlay_arg = overlay.to_str().ok_or("Non-UTF-8 lab path")?;
    // overlay is absent inside a newly created private directory. No external
    // backing path, block device, image-opts, commit, or rebase is accepted.
    directory.validate_new_path(&overlay)?;
    execute(
        report,
        runner,
        directory,
        "create_overlay",
        "qemu-img",
        &[
            "create",
            "-f",
            "qcow2",
            "-F",
            "raw",
            "-b",
            base_arg,
            overlay_arg,
        ],
    )?;
    directory.validate_file(&overlay)?;
    execute(
        report,
        runner,
        directory,
        "verify_inherited_data",
        "qemu-io",
        &[
            "-r",
            "-f",
            "qcow2",
            "-c",
            "read -P 0x11 1048576 4096",
            overlay_arg,
        ],
    )?;
    let overlay_before = hash_owned_file(directory, &overlay)?;
    execute(
        report,
        runner,
        directory,
        "write_overlay",
        "qemu-io",
        &[
            "-f",
            "qcow2",
            "-c",
            "write -P 0x5a 65536 4096",
            "-c",
            "flush",
            overlay_arg,
        ],
    )?;
    let after_overlay = hash_owned_file(directory, &base)?;
    report.after_overlay_sha256 = Some(after_overlay.clone());
    report.check(
        "base_unchanged",
        baseline == after_overlay,
        "Full raw-base SHA-256 is identical before and after overlay writes",
    )?;
    let overlay_after = hash_owned_file(directory, &overlay)?;
    report.check(
        "overlay_changed",
        overlay_before != overlay_after,
        "Overlay bytes changed after the write command",
    )?;
    execute(
        report,
        runner,
        directory,
        "verify_overlay_write",
        "qemu-io",
        &[
            "-r",
            "-f",
            "qcow2",
            "-c",
            "read -P 0x5a 65536 4096",
            overlay_arg,
        ],
    )?;
    execute(
        report,
        runner,
        directory,
        "verify_raw_unchanged_region",
        "qemu-io",
        &["-r", "-f", "raw", "-c", "read -P 0x11 65536 4096", base_arg],
    )?;
    execute(
        report,
        runner,
        directory,
        "check_overlay_structure",
        "qemu-img",
        &["check", "-f", "qcow2", overlay_arg],
    )?;
    report.check(
        "unchanged_base_resume_gate",
        verify_binding(&baseline, &after_overlay).is_ok(),
        "Matching base fingerprint allows the lab resume gate",
    )?;
    // Every QEMU command has exited. Simulate a native-side base edit solely in
    // our disposable raw file, then refuse the old overlay before any more I/O.
    directory.validate_file(&base)?;
    fs::set_permissions(&base, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&base)
        .map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(2 * 1024 * 1024))
        .map_err(|e| e.to_string())?;
    file.write_all(&[0x33; 4096]).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    fs::set_permissions(&base, fs::Permissions::from_mode(0o400)).map_err(|e| e.to_string())?;
    let after_change = hash_owned_file(directory, &base)?;
    report.after_base_change_sha256 = Some(after_change.clone());
    report.check(
        "controlled_base_change",
        after_change != baseline,
        "Controlled edit in the disposable base produces a different full-file SHA-256",
    )?;
    report.check(
        "stale_overlay_refused",
        verify_binding(&baseline, &after_change).is_err(),
        "Old overlay rejected by fingerprint gate; no QEMU command runs after the base change",
    )?;
    Ok(())
}

fn verify_binding(expected: &str, observed: &str) -> Result<(), &'static str> {
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid baseline fingerprint");
    }
    if expected != observed {
        return Err("Base changed: stale overlay resume refused");
    }
    Ok(())
}

struct OwnedDirectory {
    path: PathBuf,
    uid: u32,
    removed: bool,
}

impl OwnedDirectory {
    fn create() -> io::Result<Self> {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!(
            "/tmp/elasticpc-file-lab-{}-{epoch}-{serial}",
            std::process::id()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        let metadata = fs::symlink_metadata(&path)?;
        Ok(Self {
            path: fs::canonicalize(path)?,
            uid: metadata.uid(),
            removed: false,
        })
    }

    fn validate_new_path(&self, path: &Path) -> Result<(), String> {
        if path.parent() != Some(self.path.as_path()) || path.file_name().is_none() {
            return Err("Path is not a direct child of the owned private directory".into());
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            _ => Err("Refusing an already existing image path".into()),
        }
    }

    fn validate_file(&self, path: &Path) -> Result<(), String> {
        if path.parent() != Some(self.path.as_path()) {
            return Err("Image path escapes the owned private directory".into());
        }
        let root = fs::symlink_metadata(&self.path).map_err(|e| e.to_string())?;
        if !root.is_dir() || root.uid() != self.uid || root.mode() & 0o077 != 0 {
            return Err("Lab directory ownership or privacy changed".into());
        }
        let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.uid() != self.uid || metadata.nlink() != 1 {
            return Err(
                "Image must be an owned regular file with one link, never a symlink or device"
                    .into(),
            );
        }
        Ok(())
    }

    fn remove(&mut self) -> io::Result<()> {
        // remove_dir_all does not follow symbolic links. No caller-controlled
        // path ever enters this type.
        fs::remove_dir_all(&self.path)?;
        self.removed = true;
        Ok(())
    }
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if !self.removed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn hash_owned_file(directory: &OwnedDirectory, path: &Path) -> Result<String, String> {
    directory.validate_file(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let bytes = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if bytes == 0 {
            break;
        }
        hasher.update(&buffer[..bytes]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

trait Runner {
    fn evidence_kind(&self) -> EvidenceKind;
    fn run(
        &mut self,
        timeout: &Path,
        program: &Path,
        args: &[&str],
        directory: &Path,
    ) -> Result<CommandEvidence, String>;
}

struct RealRunner;

impl Runner for RealRunner {
    fn evidence_kind(&self) -> EvidenceKind {
        EvidenceKind::RealQemuFileIo
    }

    fn run(
        &mut self,
        timeout: &Path,
        program: &Path,
        args: &[&str],
        directory: &Path,
    ) -> Result<CommandEvidence, String> {
        let mut child = Command::new(timeout)
            .arg("--signal=KILL")
            .arg(format!("{COMMAND_TIMEOUT_SECONDS}s"))
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
        let out_reader = std::thread::spawn(move || drain_capped(stdout));
        let err_reader = std::thread::spawn(move || drain_capped(stderr));
        // GNU timeout enforces the command deadline; no status polling loop.
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
            program: program.to_path_buf(),
            args: args.iter().map(|v| (*v).into()).collect(),
            exit_code: status.code(),
            timed_out: matches!(status.code(), Some(124 | 137))
                || status.signal() == Some(libc::SIGKILL),
            stdout,
            stderr,
            output_truncated: out_truncated || err_truncated,
        })
    }
}

fn drain_capped(mut stream: impl Read) -> io::Result<(String, bool)> {
    let mut collected = Vec::with_capacity(OUTPUT_LIMIT);
    let mut truncated = false;
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let keep = count.min(OUTPUT_LIMIT - collected.len());
        collected.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok((String::from_utf8_lossy(&collected).into_owned(), truncated))
}

fn execute(
    report: &mut FileLabReport,
    runner: &mut impl Runner,
    directory: &OwnedDirectory,
    label: &str,
    tool: &str,
    args: &[&str],
) -> Result<(), String> {
    let resolve = |name: &str| {
        report
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .and_then(|dependency| dependency.path.clone())
            .ok_or_else(|| format!("Missing dependency: {name}"))
    };
    let mut result = runner.run(&resolve("timeout")?, &resolve(tool)?, args, &directory.path)?;
    result.name = label.into();
    let success = result.exit_code == Some(0) && !result.timed_out;
    let diagnostic = format!(
        "{label}: exit {:?}, timed_out={}, stderr={}",
        result.exit_code, result.timed_out, result.stderr
    );
    report.commands.push(result);
    report.check(
        label,
        success,
        if success {
            "QEMU command completed successfully"
        } else {
            &diagnostic
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::unix::fs::symlink;

    struct FailingRunner {
        calls: usize,
        directory: Option<PathBuf>,
        timed_out: bool,
    }
    impl Runner for FailingRunner {
        fn evidence_kind(&self) -> EvidenceKind {
            EvidenceKind::SimulatedHarness
        }
        fn run(
            &mut self,
            _: &Path,
            program: &Path,
            args: &[&str],
            directory: &Path,
        ) -> Result<CommandEvidence, String> {
            self.calls += 1;
            self.directory = Some(directory.into());
            Ok(CommandEvidence {
                name: String::new(),
                program: program.into(),
                args: args.iter().map(|v| (*v).into()).collect(),
                // A timeout must fail even if a runner reports exit code zero.
                exit_code: Some(if self.timed_out { 0 } else { 1 }),
                timed_out: self.timed_out,
                stdout: String::new(),
                stderr: "injected failure".into(),
                output_truncated: false,
            })
        }
    }

    fn dependencies(available: bool) -> Vec<LabDependency> {
        ["qemu-img", "qemu-io", "timeout"]
            .into_iter()
            .map(|name| LabDependency {
                name: name.into(),
                path: available.then(|| PathBuf::from(format!("/usr/bin/{name}"))),
                available,
                detail: "test fixture".into(),
                provenance: "unit_test_fixture".into(),
            })
            .collect()
    }

    #[test]
    fn missing_dependencies_are_not_success_and_create_nothing() {
        let mut runner = FailingRunner {
            calls: 0,
            directory: None,
            timed_out: false,
        };
        let report = run_with_dependencies(dependencies(false), false, &mut runner);
        assert_eq!(report.status, LabStatus::Skipped);
        assert_eq!(report.evidence_kind, EvidenceKind::NotRun);
        assert!(!report.cleanup.directory_created);
        assert!(report.checks.is_empty());
        assert!(report.baseline_sha256.is_none());
        assert_eq!(runner.calls, 0);
    }

    #[test]
    fn root_is_refused_before_execution() {
        let mut runner = FailingRunner {
            calls: 0,
            directory: None,
            timed_out: false,
        };
        let report = run_with_dependencies(dependencies(true), true, &mut runner);
        assert_eq!(report.status, LabStatus::Refused);
        assert!(!report.cleanup.directory_created);
        assert_eq!(runner.calls, 0);
    }

    #[test]
    fn failed_command_is_not_real_evidence_and_cleans_up() {
        let mut runner = FailingRunner {
            calls: 0,
            directory: None,
            timed_out: false,
        };
        let report = run_with_dependencies(dependencies(true), false, &mut runner);
        assert_eq!(report.status, LabStatus::Failed);
        assert_eq!(report.evidence_kind, EvidenceKind::SimulatedHarness);
        assert_eq!(runner.calls, 1);
        assert!(report.cleanup.removed);
        assert!(!runner.directory.unwrap().exists());
        assert!(!report.checks[0].passed);
    }

    #[test]
    fn timeout_never_passes_and_cleans_owned_files() {
        let mut runner = FailingRunner {
            calls: 0,
            directory: None,
            timed_out: true,
        };
        let report = run_with_dependencies(dependencies(true), false, &mut runner);
        assert_eq!(report.status, LabStatus::Failed);
        assert!(report.commands[0].timed_out);
        assert!(!report.checks[0].passed);
        assert!(report.cleanup.removed);
        assert!(!runner.directory.unwrap().exists());
    }

    #[test]
    fn guards_reject_devices_symlinks_hardlinks_and_outside_files() {
        let directory = OwnedDirectory::create().unwrap();
        assert!(directory.validate_file(Path::new("/dev/null")).is_err());
        let linked = directory.path.join("device-link");
        symlink("/dev/null", &linked).unwrap();
        assert!(directory.validate_file(&linked).is_err());
        assert!(directory.validate_new_path(&linked).is_err());
        let original = directory.path.join("owned");
        File::create(&original).unwrap();
        directory.validate_file(&original).unwrap();
        let hardlink = directory.path.join("hardlink");
        fs::hard_link(&original, &hardlink).unwrap();
        assert!(directory.validate_file(&hardlink).is_err());
        assert!(directory.validate_file(&original).is_err());
        assert!(directory
            .validate_new_path(Path::new("/tmp/escape"))
            .is_err());
    }

    #[test]
    fn changed_or_invalid_fingerprints_are_refused() {
        let old = "a".repeat(64);
        assert!(verify_binding(&old, &old).is_ok());
        assert!(verify_binding(&old, &"b".repeat(64)).is_err());
        assert!(verify_binding("", "").is_err());
        assert!(verify_binding(&"z".repeat(64), &"z".repeat(64)).is_err());
    }

    #[test]
    fn custom_tool_directory_rejects_symlinks_and_writable_executables() {
        let directory = OwnedDirectory::create().unwrap();
        let img = directory.path.join("qemu-img");
        let io = directory.path.join("qemu-io");
        symlink("/usr/bin/true", &img).unwrap();
        File::create(&io).unwrap();
        fs::set_permissions(&io, fs::Permissions::from_mode(0o777)).unwrap();
        let dependencies = inspect_dependencies_in(&directory.path);
        assert!(!dependencies[0].available);
        assert!(!dependencies[1].available);
        assert_eq!(dependencies[0].provenance, "user_selected_tool_directory");
        fs::set_permissions(&io, fs::Permissions::from_mode(0o4755)).unwrap();
        assert!(!inspect_dependencies_in(&directory.path)[1].available);
    }

    #[test]
    fn capped_output_is_drained_but_not_retained_without_bound() {
        let (output, truncated) =
            drain_capped(io::Cursor::new(vec![b'x'; OUTPUT_LIMIT + 1000])).unwrap();
        assert_eq!(output.len(), OUTPUT_LIMIT);
        assert!(truncated);
    }

    #[test]
    fn report_round_trips_with_explicit_missing_evidence() {
        let report = FileLabReport::pending(dependencies(false));
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<FileLabReport>(&json).unwrap(),
            report
        );
        assert!(json.contains("\"evidence_kind\":\"not_run\""));
    }

    /// The offline test suite never silently turns this into a mock pass.
    /// Run explicitly with `cargo test -p elastic-lab -- --ignored` once QEMU is installed.
    /// A separately verified bundle may be selected with the test-only environment
    /// variable `ELASTICPC_TEST_QEMU_TOOL_DIR` (no runtime environment override exists).
    #[test]
    #[ignore = "requires explicitly provisioned qemu-img and qemu-io; performs owned-file I/O"]
    fn real_qemu_file_cow_lab() {
        let report = match std::env::var_os("ELASTICPC_TEST_QEMU_TOOL_DIR") {
            Some(directory) => run_file_lab_with_tools(Path::new(&directory)),
            None => run_file_lab(),
        };
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        assert_eq!(report.status, LabStatus::Passed, "{report:#?}");
        assert_eq!(report.evidence_kind, EvidenceKind::RealQemuFileIo);
        assert!(report.cleanup.removed);
        assert_eq!(report.baseline_sha256, report.after_overlay_sha256);
        assert_ne!(report.baseline_sha256, report.after_base_change_sha256);
    }
}
