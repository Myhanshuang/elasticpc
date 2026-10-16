# Hardware evidence and offline fixtures

The repository contains the development specification, but not the original
command transcripts cited by it. `fixtures/acacia-redacted/snapshot.json` is a
reconstruction of that specification, not a new measurement of this host and not
an anonymized byte-for-byte copy of the missing transcripts.

| Fixture | Purpose | Evidence boundary |
| --- | --- | --- |
| `acacia-redacted` | Manjaro root, T7 layout, GPU/audio IOMMU grouping, reset methods, EFI discovery | Facts described by the specification; missing facts remain unknown |
| `generic/synthetic-topology` | Canonical DRM mapping, AMD SVM, memory, link and resume parser tests | Entirely synthetic extension; cannot establish the target PC's display wiring |
| `generic/windows-on-fourth-partition` | Windows content on partition 4 while partition 3 is NTFS but unknown | Entirely synthetic; prevents assuming that T7 partition 3 is Windows |
| `generic/degraded` | Missing commands and inaccessible kernel sources | Synthetic failure observations |
| `generic/intel-headless` | Intel VMX, unreadable KVM, no disks or displays | Synthetic headless scenario |

Every fixture is interpreted by the production parsing functions after the same
acquisition boundary used for live scans. Probe results carry `source: "fixture"`
and a `capture_provenance` finding. JSON serialization is deterministic.

The acacia fixture preserves these specification facts:

- NVIDIA `0000:01:00.0` and audio `0000:01:00.1` share a group whose observed
  members are exactly those two functions; GPU reset methods are `flr bus`.
- NVIDIA is on the `0000:00:01.2` branch. This does not establish USB4 tunneling.
- Manjaro root is mounted on `nvme0n1p1`; the ESP is `nvme0n1p2`; swap partition
  `nvme0n1p3` is active. The T7 first partition is an ESP.
- `Boot0002` exists even though it is absent from `BootOrder`; `Boot0006` is
  current. EFI entries are parsed independently of order membership.
- `card0-DP-3` and `card1-eDP-1` are connected, but their real GPU mappings remain
  unknown because canonical `card/device` paths were not supplied.

Stable disk names, UUID/PARTUUID values, exact byte sizes derived from rounded
capacities, IOMMU group number, kernel string and the Manjaro `os-release` text
are synthetic/redacted. The fixture's `os-release` text reconstructs the already
confirmed Manjaro identity; it is not claimed to have been read on this host.
CPU architecture/flags, RAM, KVM access, Secure Boot, fstab/resume and link rates
remain unknown in this fixture. No fixture infers Windows from NTFS. Unmounted
T7 NTFS volumes remain `UnknownInstallation` with an explicit evidence gap.

Live discovery runs only fixed read-only commands under `LC_ALL=C`, bounded by
GNU `timeout` (5 seconds plus 1 second kill grace), and caps file/command output
reads. It never mounts filesystems, reads raw disk content, changes EFI variables
or changes driver bindings. OS discovery only inspects already-mounted roots;
symlinks that escape a mounted root are rejected. EDID is represented by a digest,
not its raw serial-bearing bytes.

`storage_observation_complete` stays false: current-namespace mount/swap evidence
cannot exclude writers in other namespaces, existing QEMU processes or open
device handles. Block content hashes are absent. `unallocated_bytes` stays null
because `FSAVAIL` does not establish unallocated GPT capacity. KVM readability
is only `experimental`; no `KVM_CREATE_VM` ioctl is performed. Resume references
are reported, but missing references cannot establish that removing swap is safe.

Run offline verification with `cargo test -p elastic-probe`. No root access,
physical devices, mounts or network access are needed by these tests.
