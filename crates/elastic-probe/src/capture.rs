//! Bounded, read-only acquisition. Fixtures store this raw snapshot and use the
//! same interpretation code as live probes. No mount, block reads or mutation.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_TEXT: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Capture {
    pub provenance: String,
    pub architecture: Option<String>,
    pub files: BTreeMap<String, String>,
    pub commands: BTreeMap<String, String>,
    pub errors: Vec<String>,
    pub pci: Vec<PciCapture>,
    pub drm: Vec<DrmCapture>,
    /// Canonical device node -> its /dev/disk/by-id names.
    pub disk_aliases: BTreeMap<String, Vec<String>>,
    pub mounted_os: Vec<MountedOsCapture>,
    pub kvm_exists: Option<bool>,
    pub kvm_accessible: Option<bool>,
    pub secure_boot: Option<bool>,
    pub iommu_groups_visible: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PciCapture {
    pub bdf: String,
    pub attributes: BTreeMap<String, String>,
    pub driver: Option<String>,
    pub canonical_path: Option<String>,
    pub iommu_group: Option<String>,
    pub iommu_members: Vec<String>,
    pub mdev_types: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct DrmCapture {
    pub name: String,
    pub status: Option<String>,
    pub card_device: Option<String>,
    pub modes: Vec<String>,
    pub edid_present: bool,
    pub edid_hash: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct MountedOsCapture {
    pub mountpoint: String,
    pub source: String,
    pub os_release: Option<String>,
    pub fstab: Option<String>,
    pub boot_files: Vec<String>,
    pub windows_system32: bool,
    pub windows_bcd: bool,
    pub errors: Vec<String>,
}

pub fn read_text(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{}: not a regular file", path.display()));
    }
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut contents = String::new();
    file.take(MAX_TEXT + 1)
        .read_to_string(&mut contents)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if contents.len() as u64 > MAX_TEXT {
        return Err(format!("{}: exceeds 2 MiB read limit", path.display()));
    }
    Ok(contents)
}

fn bounded_command(executable: &str, args: &[&str]) -> Result<String, String> {
    // GNU timeout is an explicit prerequisite; never fall back to an unbounded
    // command. Both executable paths and arguments are internal constants.
    if !Path::new("/usr/bin/timeout").is_file() {
        return Err("/usr/bin/timeout unavailable; command skipped".into());
    }
    let mut child = Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=1s", "5s", executable])
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("PATH", "/usr/bin:/usr/sbin:/bin:/sbin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{executable}: {e}"))?;
    let stdout = child.stdout.take().ok_or("missing command stdout")?;
    let stderr = child.stderr.take().ok_or("missing command stderr")?;
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_TEXT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let status = child.wait().map_err(|e| format!("{executable}: {e}"))?;
    let bytes = out
        .join()
        .map_err(|_| "stdout reader failed")?
        .map_err(|e| format!("{executable}: {e}"))?;
    let errors = err
        .join()
        .map_err(|_| "stderr reader failed")?
        .map_err(|e| format!("{executable}: {e}"))?;
    if !status.success() {
        return Err(format!(
            "{executable} exited {status}: {}",
            String::from_utf8_lossy(&errors).trim()
        ));
    }
    if bytes.len() as u64 > MAX_TEXT {
        return Err(format!("{executable}: exceeds 2 MiB output limit"));
    }
    String::from_utf8(bytes).map_err(|e| format!("{executable}: {e}"))
}

fn names(path: &Path) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", path.display()))?;
        if result.len() == 8192 {
            return Err(format!(
                "{}: exceeds 8192-entry directory limit; incomplete listing rejected",
                path.display()
            ));
        }
        result.push(entry.file_name().to_string_lossy().into_owned());
    }
    result.sort();
    Ok(result)
}

fn canonical(path: &Path) -> Option<String> {
    fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

fn basename(path: &Path) -> Option<String> {
    fs::canonicalize(path)
        .ok()?
        .file_name()
        .map(|v| v.to_string_lossy().into_owned())
}

fn confined_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let root = fs::canonicalize(root).ok()?;
    let path = fs::canonicalize(root.join(relative)).ok()?;
    path.starts_with(root).then_some(path)
}

fn confined_text(root: &Path, relative: &str) -> Option<String> {
    read_text(&confined_path(root, relative)?).ok()
}

pub fn live() -> Capture {
    let mut capture = Capture {
        provenance: "live read-only host snapshot".into(),
        architecture: Some(std::env::consts::ARCH.into()),
        ..Capture::default()
    };
    for path in [
        "/proc/cpuinfo",
        "/proc/meminfo",
        "/proc/version",
        "/proc/cmdline",
        "/proc/swaps",
        "/proc/self/mountinfo",
        "/etc/fstab",
        "/etc/default/grub",
        "/etc/mkinitcpio.conf",
        "/etc/initramfs-tools/conf.d/resume",
    ] {
        match read_text(Path::new(path)) {
            Ok(value) => {
                capture.files.insert(path.into(), value);
            }
            Err(e) => capture.errors.push(e),
        }
    }
    for (name, executable, args) in [
        ("lsblk", "/usr/bin/lsblk", vec!["--json", "--bytes", "--paths", "--output", "NAME,KNAME,PKNAME,MAJ:MIN,TYPE,SIZE,MODEL,SERIAL,WWN,TRAN,PTTYPE,PTUUID,PARTUUID,PARTTYPE,UUID,FSTYPE,MOUNTPOINTS,FSAVAIL,FSUSE%"]),
        ("lspci", "/usr/bin/lspci", vec!["-D", "-nn", "-vv"]),
        ("efibootmgr", "/usr/bin/efibootmgr", vec!["-v"]),
    ] {
        match bounded_command(executable, &args) {
            Ok(value) => { capture.commands.insert(name.into(), value); }
            Err(e) => capture.errors.push(e),
        }
    }
    capture.kvm_exists = match fs::symlink_metadata("/dev/kvm") {
        Ok(_) => Some(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    };
    // Default capture never opens the KVM device. Explicit API probing is a
    // separate opt-in operation; metadata cannot establish access permission.
    capture.kvm_accessible = capture.kvm_exists.filter(|exists| !exists);
    capture.iommu_groups_visible = match names(Path::new("/sys/kernel/iommu_groups")) {
        Ok(groups) => Some(!groups.is_empty()),
        Err(e) => {
            capture.errors.push(e);
            None
        }
    };
    match names(Path::new("/sys/firmware/efi/efivars")) {
        Ok(vars) => {
            if let Some(name) = vars.iter().find(|name| name.starts_with("SecureBoot-")) {
                let path = Path::new("/sys/firmware/efi/efivars").join(name);
                match File::open(&path).and_then(|file| {
                    let mut bytes = Vec::new();
                    file.take(5).read_to_end(&mut bytes).map(|_| bytes)
                }) {
                    Ok(bytes) if bytes.len() == 5 && bytes[4] <= 1 => {
                        capture.secure_boot = Some(bytes[4] == 1)
                    }
                    Ok(_) => capture
                        .errors
                        .push("SecureBoot EFI variable has invalid size or value".into()),
                    Err(e) => capture.errors.push(format!("SecureBoot EFI variable: {e}")),
                }
            }
        }
        Err(e) => capture.errors.push(e),
    }
    match names(Path::new("/sys/bus/pci/devices")) {
        Ok(devices) => {
            for bdf in devices {
                let base = Path::new("/sys/bus/pci/devices").join(&bdf);
                let mut pci = PciCapture {
                    bdf,
                    driver: basename(&base.join("driver")),
                    canonical_path: canonical(&base),
                    iommu_group: basename(&base.join("iommu_group")),
                    ..PciCapture::default()
                };
                for name in [
                    "vendor",
                    "device",
                    "class",
                    "reset_method",
                    "sriov_totalvfs",
                    "sriov_numvfs",
                    "current_link_speed",
                    "current_link_width",
                ] {
                    if let Ok(value) = read_text(&base.join(name)) {
                        pci.attributes.insert(name.into(), value.trim().into());
                    }
                }
                if pci.iommu_group.is_some() {
                    match names(&base.join("iommu_group/devices")) {
                        Ok(members) => pci.iommu_members = members,
                        Err(e) => capture.errors.push(e),
                    }
                }
                if base.join("mdev_supported_types").exists() {
                    match names(&base.join("mdev_supported_types")) {
                        Ok(types) => pci.mdev_types = Some(types),
                        Err(e) => capture.errors.push(e),
                    }
                } else {
                    pci.mdev_types = Some(Vec::new());
                }
                capture.pci.push(pci);
            }
        }
        Err(e) => capture.errors.push(e),
    }
    match names(Path::new("/sys/class/drm")) {
        Ok(entries) => {
            for name in entries {
                let Some((card, _)) = name.split_once('-') else {
                    continue;
                };
                if !card.starts_with("card") {
                    continue;
                }
                let base = Path::new("/sys/class/drm").join(&name);
                let card_device = canonical(&Path::new("/sys/class/drm").join(card).join("device"));
                let status = read_text(&base.join("status"))
                    .ok()
                    .map(|v| v.trim().into());
                let modes = read_text(&base.join("modes"))
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_owned)
                    .collect();
                let edid = File::open(base.join("edid"))
                    .and_then(|file| {
                        let mut bytes = Vec::new();
                        file.take(64 * 1024 + 1)
                            .read_to_end(&mut bytes)
                            .map(|_| bytes)
                    })
                    .ok()
                    .filter(|bytes| !bytes.is_empty() && bytes.len() <= 64 * 1024);
                let edid_present = edid.is_some();
                let edid_hash = edid.as_ref().map(|bytes| elastic_core::digest(bytes));
                capture.drm.push(DrmCapture {
                    name,
                    status,
                    card_device,
                    modes,
                    edid_present,
                    edid_hash,
                });
            }
        }
        Err(e) => capture.errors.push(e),
    }
    match names(Path::new("/dev/disk/by-id")) {
        Ok(aliases) => {
            for alias in aliases {
                if let Some(device) = canonical(&Path::new("/dev/disk/by-id").join(&alias)) {
                    capture.disk_aliases.entry(device).or_default().push(alias);
                }
            }
        }
        Err(e) => capture.errors.push(e),
    }
    // Only inspect already-mounted disk filesystems; never mount or unlock one.
    if let Some(mountinfo) = capture.files.get("/proc/self/mountinfo") {
        let mut inspected = BTreeSet::new();
        for line in mountinfo.lines() {
            let Some((left, right)) = line.split_once(" - ") else {
                continue;
            };
            let fields: Vec<_> = left.split_whitespace().collect();
            let trailing: Vec<_> = right.split_whitespace().collect();
            if fields.len() < 6
                || trailing.len() < 2
                || ![
                    "ext4", "ext3", "ext2", "btrfs", "xfs", "ntfs", "ntfs3", "fuseblk", "vfat",
                ]
                .contains(&trailing[0])
            {
                continue;
            }
            let mountpoint = super::unescape_mount(fields[4]);
            // Ignore arbitrary subtree bind mounts. A host root mounted from a
            // btrfs subvolume is valid even though its filesystem root is /@.
            if fields[3] != "/" && mountpoint != "/" {
                continue;
            }
            if !inspected.insert((fields[2], fields[3])) {
                continue;
            }
            let source = super::unescape_mount(trailing[1]);
            let base = PathBuf::from(&mountpoint);
            // Absolute symlinks in a mounted guest must not redirect evidence
            // reads into the host's own /etc or /usr.
            let os_release = confined_text(&base, "etc/os-release");
            let fstab = confined_text(&base, "etc/fstab");
            let boot_files = confined_path(&base, "boot")
                .and_then(|path| names(&path).ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|name| {
                    name.starts_with("vmlinuz")
                        || name.starts_with("initramfs")
                        || name.starts_with("initrd")
                })
                .collect();
            capture.mounted_os.push(MountedOsCapture {
                mountpoint,
                source,
                os_release,
                fstab,
                boot_files,
                windows_system32: confined_path(&base, "Windows/System32")
                    .is_some_and(|path| path.is_dir()),
                windows_bcd: ["EFI/Microsoft/Boot/BCD", "Boot/BCD"]
                    .iter()
                    .any(|relative| {
                        confined_path(&base, relative).is_some_and(|path| path.is_file())
                    }),
                errors: Vec::new(),
            });
        }
    }
    capture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounted_guest_symlink_cannot_read_outside_its_root() {
        let unique = format!(
            "elastic-probe-confine-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let base = std::env::temp_dir().join(unique);
        let root = base.join("guest");
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::write(base.join("host-release"), "host-only").unwrap();
        std::os::unix::fs::symlink(base.join("host-release"), root.join("etc/os-release")).unwrap();
        assert!(confined_text(&root, "etc/os-release").is_none());
        assert!(read_text(&root).is_err());
        fs::remove_file(root.join("etc/os-release")).unwrap();
        fs::write(root.join("release"), "guest-only").unwrap();
        std::os::unix::fs::symlink("../release", root.join("etc/os-release")).unwrap();
        assert_eq!(
            confined_text(&root, "etc/os-release").as_deref(),
            Some("guest-only")
        );
        fs::remove_dir_all(base).unwrap();
    }
}
