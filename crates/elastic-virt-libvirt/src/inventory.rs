use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};

const MAX_XML_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainState {
    Unknown,
    Running,
    Blocked,
    Paused,
    Shutdown,
    Shutoff,
    Crashed,
    PmSuspended,
}

impl DomainState {
    pub fn from_libvirt(value: u32) -> Self {
        match value {
            1 => Self::Running,
            2 => Self::Blocked,
            3 => Self::Paused,
            4 => Self::Shutdown,
            5 => Self::Shutoff,
            6 => Self::Crashed,
            7 => Self::PmSuspended,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XmlScope {
    Live,
    Inactive,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    File,
    BlockDevice,
    PciDevice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceReference {
    pub kind: ResourceKind,
    pub location: String,
    pub role: String,
    /// XML declaration only, not proof that a host process cannot write it.
    pub read_only_declared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainObservation {
    pub uuid: String,
    pub active: bool,
    pub state: DomainState,
    pub xml_scope: XmlScope,
    /// Only covers explicitly supported storage/hostdev XML references.
    pub resource_references_complete: bool,
    pub resources: Vec<ResourceReference>,
    pub unknown_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryReport {
    pub schema_version: u32,
    pub uri: String,
    pub backend_simulated: bool,
    pub domain_listing_complete: bool,
    pub xml_resource_references_complete: bool,
    pub observation_is_atomic: bool,
    /// Always false: XML cannot rule out host mounts, other processes or races.
    pub storage_observation_complete: bool,
    pub domains: Vec<DomainObservation>,
    pub limitations: Vec<String>,
}

/// Parse an observed domain's current XML, never the next-boot XML of an active
/// domain. On every incomplete or unsupported form the observation stays
/// unknown; references already found are retained as conservative claims.
/// This function never opens paths referenced in XML.
pub fn parse_domain_xml(
    uuid: &str,
    active: bool,
    state: DomainState,
    xml: Option<&str>,
) -> DomainObservation {
    let mut result = DomainObservation {
        uuid: uuid.into(),
        active,
        state,
        xml_scope: XmlScope::Unavailable,
        resource_references_complete: false,
        resources: Vec::new(),
        unknown_reasons: Vec::new(),
    };
    if state == DomainState::Unknown || (active == (state == DomainState::Shutoff)) {
        result
            .unknown_reasons
            .push("Domain activity/state is unknown or inconsistent".into());
    }
    if !valid_uuid(uuid) {
        result.unknown_reasons.push("Domain UUID is invalid".into());
    }
    let Some(xml) = xml else {
        result
            .unknown_reasons
            .push("Current domain XML was not observed".into());
        return result;
    };
    if xml.len() > MAX_XML_BYTES {
        result
            .unknown_reasons
            .push("Domain XML exceeds the bounded parser limit".into());
        return result;
    }
    let document = match Document::parse(xml) {
        Ok(document) => document,
        Err(_) => {
            result
                .unknown_reasons
                .push("Domain XML is malformed or contains a forbidden DTD".into());
            return result;
        }
    };
    let root = document.root_element();
    if !root.has_tag_name("domain") || root.tag_name().namespace().is_some() {
        result
            .unknown_reasons
            .push("Expected a plain domain XML root".into());
        return result;
    }
    result.xml_scope = if active {
        XmlScope::Live
    } else {
        XmlScope::Inactive
    };
    let uuids: Vec<_> = root
        .children()
        .filter(|node| node.has_tag_name("uuid"))
        .collect();
    if uuids.len() != 1 || uuids[0].text().map(str::trim) != Some(uuid) {
        result
            .unknown_reasons
            .push("XML UUID does not match the observed domain UUID".into());
    }
    // Namespaced passthrough can introduce resources not represented in the
    // ordinary XML tree. Metadata is also conservatively treated as unknown.
    if root
        .descendants()
        .any(|node| node.is_element() && node.tag_name().namespace().is_some())
    {
        result
            .unknown_reasons
            .push("Namespaced XML is outside the supported resource model".into());
    }
    for child in root.children().filter(Node::is_element) {
        if !matches!(
            child.tag_name().name(),
            "name"
                | "uuid"
                | "title"
                | "description"
                | "memory"
                | "currentMemory"
                | "maxMemory"
                | "vcpu"
                | "vcpus"
                | "iothreads"
                | "iothreadids"
                | "numatune"
                | "cputune"
                | "memtune"
                | "blkiotune"
                | "os"
                | "features"
                | "cpu"
                | "clock"
                | "on_poweroff"
                | "on_reboot"
                | "on_crash"
                | "on_lockfailure"
                | "pm"
                | "devices"
                | "seclabel"
        ) {
            result.unknown_reasons.push(format!(
                "Unsupported domain element: {}",
                child.tag_name().name()
            ));
        }
    }
    for os in root.children().filter(|node| node.has_tag_name("os")) {
        for child in os.children().filter(Node::is_element) {
            if matches!(
                child.tag_name().name(),
                "nvram" | "loader" | "kernel" | "initrd" | "dtb"
            ) {
                if let Some(path) = child.text().map(str::trim).filter(|path| !path.is_empty()) {
                    add_path(
                        &mut result,
                        ResourceKind::File,
                        path,
                        child.tag_name().name(),
                        child.attribute("readonly") == Some("yes"),
                    );
                } else {
                    result.unknown_reasons.push(format!(
                        "{} source is not a simple file path",
                        child.tag_name().name()
                    ));
                }
                if child.children().any(|node| node.is_element()) {
                    result
                        .unknown_reasons
                        .push("Structured firmware source is unsupported".into());
                }
                if let Some(path) = child.attribute("template") {
                    add_path(
                        &mut result,
                        ResourceKind::File,
                        path,
                        "firmware_template",
                        true,
                    );
                }
            } else if !matches!(
                child.tag_name().name(),
                "type"
                    | "boot"
                    | "bootmenu"
                    | "smbios"
                    | "bios"
                    | "cmdline"
                    | "init"
                    | "initarg"
                    | "initdir"
                    | "inituser"
                    | "initgroup"
            ) {
                result.unknown_reasons.push(format!(
                    "Unsupported OS element: {}",
                    child.tag_name().name()
                ));
            }
        }
    }
    let devices: Vec<_> = root
        .children()
        .filter(|node| node.has_tag_name("devices"))
        .collect();
    if devices.len() != 1 {
        result
            .unknown_reasons
            .push("Exactly one devices element is required".into());
    }
    for devices in devices {
        for device in devices.children().filter(Node::is_element) {
            match device.tag_name().name() {
                "disk" => parse_disk(device, &mut result),
                "hostdev" => parse_hostdev(device, &mut result),
                // These ordinary emulated devices do not identify host block
                // storage or PCI devices. Source-bearing variants remain unknown.
                "controller" | "video" | "input" | "sound" | "audio" | "memballoon"
                | "watchdog" | "panic" | "hub" | "iommu" | "vsock" => {
                    if device
                        .descendants()
                        .any(|node| node.has_tag_name("source") || node.has_tag_name("backend"))
                    {
                        result.unknown_reasons.push(format!(
                            "Source-bearing {} is unsupported",
                            device.tag_name().name()
                        ));
                    }
                }
                "interface"
                    if device.attribute("type") == Some("network")
                        || device.attribute("type") == Some("user") =>
                {
                    parse_interface(device, &mut result);
                }
                other => result
                    .unknown_reasons
                    .push(format!("Unsupported resource-bearing device: {other}")),
            }
        }
    }
    if root
        .children()
        .any(|node| node.has_tag_name("memoryBacking"))
    {
        result
            .unknown_reasons
            .push("Host memory backing resources are outside this inventory model".into());
    }
    result.unknown_reasons.sort();
    result.unknown_reasons.dedup();
    result.resource_references_complete = result.unknown_reasons.is_empty();
    result
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn add_path(
    result: &mut DomainObservation,
    kind: ResourceKind,
    path: &str,
    role: &str,
    readonly: bool,
) {
    if !path.starts_with('/')
        || path.contains('\0')
        || path.split('/').any(|part| part == ".." || part == ".")
    {
        result
            .unknown_reasons
            .push(format!("{role} path is not an unambiguous absolute path"));
    }
    result.resources.push(ResourceReference {
        kind,
        location: path.into(),
        role: role.into(),
        read_only_declared: readonly,
    });
}

fn parse_disk(disk: Node<'_, '_>, result: &mut DomainObservation) {
    let readonly = disk.children().any(|node| node.has_tag_name("readonly"));
    let role = disk
        .children()
        .find(|node| node.has_tag_name("target"))
        .and_then(|node| node.attribute("dev"))
        .unwrap_or("disk");
    for node in disk.children().filter(Node::is_element) {
        if !matches!(
            node.tag_name().name(),
            "source"
                | "target"
                | "driver"
                | "readonly"
                | "shareable"
                | "serial"
                | "wwn"
                | "vendor"
                | "product"
                | "boot"
                | "alias"
                | "address"
                | "iotune"
                | "blockio"
                | "geometry"
                | "backingStore"
        ) {
            result.unknown_reasons.push(format!(
                "Unsupported disk child: {}",
                node.tag_name().name()
            ));
        }
    }
    let has_source = disk.children().any(|node| node.has_tag_name("source"));
    let empty_removable =
        !has_source && matches!(disk.attribute("device"), Some("cdrom" | "floppy"));
    if !empty_removable {
        parse_storage_source(disk, disk.attribute("type"), role, readonly, result);
    }
    let backing: Vec<_> = disk
        .children()
        .filter(|node| node.has_tag_name("backingStore"))
        .collect();
    if backing.len() > 1 {
        result
            .unknown_reasons
            .push("Multiple sibling backing stores are ambiguous".into());
    }
    for node in &backing {
        parse_backing(*node, result, 0);
    }
    let format = disk
        .children()
        .find(|node| node.has_tag_name("driver"))
        .and_then(|node| node.attribute("type"));
    if !empty_removable
        && matches!(disk.attribute("type"), Some("file" | "block"))
        && format != Some("raw")
        && backing.is_empty()
    {
        result.unknown_reasons.push(
            "Non-raw or unknown disk format lacks an observed backing-chain terminator".into(),
        );
    }
}

fn parse_backing(node: Node<'_, '_>, result: &mut DomainObservation, depth: usize) {
    if depth >= 16 {
        result
            .unknown_reasons
            .push("Backing chain exceeds the bounded parser depth".into());
        return;
    }
    let children: Vec<_> = node.children().filter(Node::is_element).collect();
    if children.is_empty() && node.attributes().len() == 0 {
        return; // libvirt's explicit end-of-chain marker
    }
    parse_storage_source(node, node.attribute("type"), "backing_store", true, result);
    for child in &children {
        if !matches!(
            child.tag_name().name(),
            "source" | "format" | "backingStore"
        ) {
            result.unknown_reasons.push(format!(
                "Unsupported backing-store child: {}",
                child.tag_name().name()
            ));
        }
    }
    let children: Vec<_> = children
        .into_iter()
        .filter(|node| node.has_tag_name("backingStore"))
        .collect();
    if children.len() != 1 {
        result
            .unknown_reasons
            .push("Backing chain is missing a unique observed terminator".into());
    }
    for child in children {
        parse_backing(child, result, depth + 1);
    }
}

fn parse_storage_source(
    node: Node<'_, '_>,
    kind: Option<&str>,
    role: &str,
    readonly: bool,
    result: &mut DomainObservation,
) {
    let (attribute, resource_kind) = match kind {
        Some("file") => ("file", ResourceKind::File),
        Some("block") => ("dev", ResourceKind::BlockDevice),
        _ => {
            result
                .unknown_reasons
                .push("Unsupported or missing disk source type".into());
            return;
        }
    };
    let sources: Vec<_> = node
        .children()
        .filter(|node| node.has_tag_name("source"))
        .collect();
    if sources.len() != 1 {
        result
            .unknown_reasons
            .push("Disk source is missing or ambiguous".into());
    }
    for source in sources {
        match source
            .attribute(attribute)
            .filter(|value| !value.is_empty())
        {
            Some(path) => add_path(result, resource_kind, path, role, readonly),
            None => result
                .unknown_reasons
                .push("Disk source path is absent".into()),
        }
        if source.children().any(|node| node.is_element())
            || source
                .attributes()
                .any(|attr| ![attribute, "index", "startupPolicy"].contains(&attr.name()))
        {
            result
                .unknown_reasons
                .push("Structured or additional disk source is unsupported".into());
        }
    }
}

fn parse_hostdev(device: Node<'_, '_>, result: &mut DomainObservation) {
    for child in device.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "rom" => parse_rom(child, result),
            "source" | "driver" | "boot" | "alias" | "address" => (),
            other => result
                .unknown_reasons
                .push(format!("Unsupported hostdev child: {other}")),
        }
    }
    if device.attribute("type") != Some("pci")
        || !matches!(device.attribute("mode"), None | Some("subsystem"))
    {
        result
            .unknown_reasons
            .push("Only explicit PCI subsystem hostdev addresses are supported".into());
        return;
    }
    let sources: Vec<_> = device
        .children()
        .filter(|node| node.has_tag_name("source"))
        .collect();
    if sources.len() != 1
        || sources.iter().any(|source| {
            source
                .children()
                .filter(Node::is_element)
                .any(|child| !child.has_tag_name("address"))
        })
    {
        result
            .unknown_reasons
            .push("PCI hostdev source is ambiguous or contains unsupported children".into());
    }
    let addresses: Vec<_> = sources
        .into_iter()
        .flat_map(|source| {
            source
                .children()
                .filter(|node| node.has_tag_name("address"))
        })
        .collect();
    if addresses.len() != 1 {
        result
            .unknown_reasons
            .push("PCI hostdev source address is missing or ambiguous".into());
        return;
    }
    let address = addresses[0];
    let parts: Option<Vec<_>> = ["domain", "bus", "slot", "function"]
        .iter()
        .map(|name| {
            address
                .attribute(*name)
                .and_then(|value| value.strip_prefix("0x"))
                .and_then(|value| u32::from_str_radix(value, 16).ok())
        })
        .collect();
    match parts {
        Some(parts)
            if parts[0] <= 0xffff && parts[1] <= 0xff && parts[2] <= 0x1f && parts[3] <= 7 =>
        {
            result.resources.push(ResourceReference {
                kind: ResourceKind::PciDevice,
                location: format!(
                    "{:04x}:{:02x}:{:02x}.{}",
                    parts[0], parts[1], parts[2], parts[3]
                ),
                role: "hostdev".into(),
                read_only_declared: false,
            })
        }
        _ => result
            .unknown_reasons
            .push("PCI hostdev address is malformed or out of range".into()),
    }
}

fn parse_rom(rom: Node<'_, '_>, result: &mut DomainObservation) {
    if let Some(path) = rom.attribute("file") {
        add_path(result, ResourceKind::File, path, "device_rom", true);
    }
    if rom.children().any(|node| node.is_element())
        || rom
            .attributes()
            .any(|attribute| !["file", "bar", "enabled"].contains(&attribute.name()))
    {
        result
            .unknown_reasons
            .push("Unsupported device ROM source".into());
    }
}

fn parse_interface(device: Node<'_, '_>, result: &mut DomainObservation) {
    for child in device.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "rom" => parse_rom(child, result),
            "source" if device.attribute("type") == Some("network") => {
                if child.children().any(|node| node.is_element())
                    || child.attributes().any(|attribute| {
                        !["network", "portgroup", "portid"].contains(&attribute.name())
                    })
                {
                    result.unknown_reasons.push(
                        "Network interface source is outside the supported reference model".into(),
                    );
                }
            }
            "mac" | "model" | "target" | "alias" | "address" | "boot" | "link" | "mtu" => (),
            other => result
                .unknown_reasons
                .push(format!("Unsupported interface child: {other}")),
        }
    }
}
