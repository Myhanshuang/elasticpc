# Offline probe captures

Each directory contains a `snapshot.json` acquisition record, not a precomputed
hardware report. `elastic-probe` interprets these records through the same parser
used after live read-only acquisition. Live scanning never creates mounts.

- `acacia-redacted`: reconstruction of facts in the development specification.
  The original command transcripts are not present in this repository. Device
  identities, UUIDs, exact byte counts, IOMMU group number, kernel label and
  `os-release` sample are synthetic/redacted. Manjaro root, T7 partition layout,
  GPU + audio group membership, reset methods and EFI facts follow the spec.
  CPU/KVM/RAM, Secure Boot, link speed and DRM card ownership remain unknown.
- `generic/synthetic-topology`: explicitly synthetic CPU, memory, resume and
  canonical DRM card mappings. This tests mapping behavior, not the target PC.
- `generic/windows-on-fourth-partition`: synthetic Windows/System32 on the fourth
  partition. The third NTFS partition remains unknown; filesystem type cannot
  establish an operating system.
- `generic/degraded`: missing commands and inaccessible kernel sources.
- `generic/intel-headless`: synthetic Intel VMX host without connected displays.

`commands` contain captured machine-readable `lsblk` and textual `efibootmgr`
output. `pci` and `drm` contain read-only sysfs attributes and canonical paths.
`files` contain proc/config inputs; `mounted_os` only represents evidence read
from already-mounted filesystems. A missing source remains unknown. No fixture
claims complete writer/namespace visibility or a block-content fingerprint.

Run `cargo test -p elastic-probe` without root. Fixture paths are local files;
they cannot select executables, mount devices or request a privileged action.
