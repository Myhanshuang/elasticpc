//! Presentation-only redaction. Redacted reports are never execution identities.
use crate::HardwareReport;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Replace identities consistently throughout the JSON, including evidence and
/// free-text diagnostics. Mappings are scoped to this report, not persisted.
pub fn redact_value(report: &HardwareReport, value: &mut Value) {
    let mut replacements = BTreeMap::<String, String>::new();
    for (di, disk) in report.disks.iter().enumerate() {
        let tag = format!("disk-{:03}", di + 1);
        if let Some(id) = &disk.stable_id {
            replacements.insert(id.0.clone(), format!("/dev/disk/by-id/REDACTED-{tag}"));
        }
        if let Some(id) = &disk.gpt_guid {
            identity_variants(&mut replacements, id, &format!("gpt-{tag}"));
        }
        for (pi, part) in disk.partitions.iter().enumerate() {
            let ptag = format!("{tag}-part-{:03}", pi + 1);
            if let Some(id) = &part.partuuid {
                identity_variants(&mut replacements, &id.0, &format!("partuuid-{ptag}"));
            }
            if let Some(id) = &part.uuid {
                identity_variants(&mut replacements, id, &format!("uuid-{ptag}"));
            }
        }
    }
    let mut path_index = 0;
    for path in report
        .mounts
        .iter()
        .map(|m| &m.target)
        .chain(report.disks.iter().flat_map(|d| d.mountpoints.iter()))
        .chain(
            report
                .disks
                .iter()
                .flat_map(|d| d.partitions.iter().flat_map(|p| p.mountpoints.iter())),
        )
    {
        let system_path = matches!(
            path.as_str(),
            "/" | "/boot"
                | "/boot/efi"
                | "/efi"
                | "[SWAP]"
                | "/dev"
                | "/sys"
                | "/proc"
                | "/run"
                | "/tmp"
                | "/usr"
                | "/etc"
                | "/var"
                | "/home"
        ) || ["/dev/", "/sys/", "/proc/"]
            .iter()
            .any(|prefix| path.starts_with(prefix));
        if !system_path && !path.is_empty() {
            path_index += 1;
            replacements
                .entry(path.clone())
                .or_insert_with(|| format!("/redacted/mount-{path_index:03}"));
        }
    }
    for (i, installation) in report.installations.iter().enumerate() {
        replacements.insert(
            installation.id.0.clone(),
            format!("installation-{:03}", i + 1),
        );
        if let Some(h) = &installation.fingerprint.content_hash {
            replacements.insert(h.clone(), "redacted-content-hash".into());
        }
        if let Some(g) = &installation.fingerprint.generation {
            replacements.insert(g.clone(), "redacted-generation".into());
        }
    }
    for display in &report.displays {
        if let Some(h) = &display.edid_hash {
            replacements.insert(h.clone(), "redacted-edid-hash".into());
        }
    }
    // Resume/boot configuration may contain stale UUIDs no longer present in
    // lsblk. These still identify the user's hardware and must be redacted.
    let mut extra = BTreeSet::new();
    collect_extra_identities(value, &mut extra);
    for (index, identity) in extra.into_iter().enumerate() {
        if !replacements.contains_key(&identity) {
            identity_variants(
                &mut replacements,
                &identity,
                &format!("unmapped-identity-{:03}", index + 1),
            );
        }
    }
    // Replace the longest strings first so nested mount paths remain consistent.
    let mut replacements: Vec<_> = replacements
        .into_iter()
        .filter(|(k, _)| !k.is_empty())
        .collect();
    replacements.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    walk(value, &replacements);
    if let Some(object) = value.as_object_mut() {
        object.insert("redacted".into(), Value::Bool(true));
    }
}
fn collect_extra_identities(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::String(s) => {
            for candidate in s.split(|c: char| !c.is_ascii_hexdigit() && c != '-') {
                let bytes = candidate.as_bytes();
                if bytes.len() == 36
                    && bytes.iter().enumerate().all(|(i, b)| {
                        if [8, 13, 18, 23].contains(&i) {
                            *b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }
                    })
                {
                    found.insert(candidate.to_owned());
                }
            }
            for token in
                s.split(|c: char| c.is_whitespace() || ['\'', '"', ',', ';', ')', '('].contains(&c))
            {
                if token.starts_with("/dev/disk/by-id/") {
                    found.insert(token.to_owned());
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_extra_identities(item, found);
            }
        }
        Value::Object(items) => {
            for item in items.values() {
                collect_extra_identities(item, found);
            }
        }
        _ => {}
    }
}
fn identity_variants(map: &mut BTreeMap<String, String>, from: &str, to: &str) {
    for variant in [
        from.to_owned(),
        from.to_ascii_lowercase(),
        from.to_ascii_uppercase(),
    ] {
        map.insert(variant, to.to_owned());
    }
}
fn walk(value: &mut Value, replacements: &[(String, String)]) {
    match value {
        Value::String(s) => {
            for (from, to) in replacements {
                *s = s.replace(from, to);
            }
            // Evidence can contain a user path that was not a mount target.
            for prefix in ["/home/", "/Users/"] {
                while let Some(start) = s.find(prefix) {
                    let end = s[start..]
                        .find(|c: char| {
                            c.is_whitespace() || ['\'', '"', ',', ';', ')'].contains(&c)
                        })
                        .map(|i| start + i)
                        .unwrap_or(s.len());
                    s.replace_range(start..end, "/redacted/user-path");
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, replacements);
            }
        }
        Value::Object(items) => {
            for (key, item) in items.iter_mut() {
                if key == "device_path" && item.is_string() {
                    // EFI USB WWID/vendor paths may contain serials independent
                    // of the by-id spelling observed on this boot.
                    *item = Value::String("[redacted EFI device path]".into());
                } else if key == "kernel" {
                    if let Some(raw) = item.as_str() {
                        let release = raw
                            .strip_prefix("Linux version ")
                            .unwrap_or(raw)
                            .split_whitespace()
                            .next()
                            .unwrap_or("unknown");
                        *item = Value::String(release.to_owned());
                    }
                } else {
                    walk(item, replacements);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    #[test]
    fn identities_and_free_text_are_redacted_consistently() {
        let report = HardwareReport {
            disks: vec![Disk {
                stable_id: Some(DiskStableId("/dev/disk/by-id/usb-SERIAL_123".into())),
                gpt_guid: Some("SECRET-GUID".into()),
                partitions: vec![Partition {
                    partuuid: Some(PartUuid("SECRET-PART".into())),
                    uuid: Some("SECRET-FS".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut v = serde_json::json!({"id":"SECRET-PART", "detail":"/dev/disk/by-id/usb-SERIAL_123 SECRET-GUID SECRET-FS /home/alice/private"});
        redact_value(&report, &mut v);
        let s = v.to_string();
        for secret in ["SERIAL_123", "SECRET", "alice"] {
            assert!(!s.contains(secret));
        }
        assert_eq!(v["redacted"], true);
    }
    #[test]
    fn reject_unknown_action_and_schema_but_preserve_unknown_observation() {
        assert!(check_schema(&serde_json::json!({"schema_version":2})).is_err());
        assert!(serde_json::from_str::<StoragePolicy>(r#"{"kind":"future_write_mode"}"#).is_err());
        assert_eq!(
            serde_json::from_str::<CapabilityStatus>("\"future_status\"").unwrap(),
            CapabilityStatus::Unknown
        );
    }
    #[test]
    fn preserves_public_sysfs_paths_but_hides_efi_serial_and_build_identity() {
        let report = HardwareReport {
            mounts: vec![
                Mount {
                    target: "/dev".into(),
                    ..Default::default()
                },
                Mount {
                    target: "/sys".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut value = serde_json::json!({"sysfs_path":"/sys/devices/pci0000:00/0000:01:00.0", "device_path":"UsbWwid(SERIAL-SECRET)","kernel":"Linux version 6.1 (alice@host-private) compiler"});
        redact_value(&report, &mut value);
        assert_eq!(value["sysfs_path"], "/sys/devices/pci0000:00/0000:01:00.0");
        assert_eq!(value["kernel"], "6.1");
        assert!(!value.to_string().contains("SERIAL-SECRET"));
        assert!(!value.to_string().contains("alice"));
    }
    #[test]
    fn stale_resume_identities_missing_from_lsblk_are_still_private() {
        let mut value = serde_json::json!({"references":["resume=UUID=abcdef12-3456-7890-abcd-123456789012", "resume=UUID=ABCDEF12-3456-7890-ABCD-123456789012", "/dev/disk/by-id/unknown-private-serial"]});
        redact_value(&HardwareReport::default(), &mut value);
        let output = value.to_string();
        assert!(!output.contains("abcdef12"));
        assert!(!output.contains("ABCDEF12"));
        assert!(!output.contains("private-serial"));
        assert_eq!(value["references"][0], value["references"][1]);
    }
}
