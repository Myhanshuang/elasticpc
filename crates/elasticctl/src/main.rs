use clap::{Parser, Subcommand, ValueEnum};
use elastic_core::*;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

mod doctor;

#[derive(Parser)]
#[command(
    name = "elasticctl",
    version,
    about = "ElasticPC discovery, planning and isolated file laboratories",
    long_about = "Discover hardware, prepare plans and run explicit laboratories using freshly created temporary files or the in-process libvirt test driver. This build has no physical apply, production VM control, EFI write or device binding executor."
)]
struct Cli {
    /// Read a fixture directory instead of this host. No live reads in this mode.
    #[arg(long, global = true)]
    fixture: Option<PathBuf>,
    /// Emit versioned machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Remove local device identities and user paths from presentation output.
    #[arg(long, global = true)]
    redact: bool,
    /// Plan only; laboratories do not create files or launch tools in this mode.
    #[arg(long, global = true)]
    dry_run: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Doctor,
    /// Read-only KVM, tool, disk identity, resume and service-storage review.
    Readiness {
        /// Explicitly query KVM_GET_API_VERSION; never creates a VM or loads modules.
        #[arg(long)]
        check_kvm_api: bool,
    },
    /// Opt-in experiments writing only their newly created temporary files.
    Lab {
        #[command(subcommand)]
        command: LabCommand,
    },
    Probe {
        #[arg(
            long,
            required_unless_present = "installations",
            conflicts_with = "installations"
        )]
        hardware: bool,
        #[arg(long)]
        installations: bool,
    },
    Installations {
        #[command(subcommand)]
        command: InstallationCommand,
    },
    Import {
        #[command(subcommand)]
        command: ImportCommand,
    },
    Layout {
        #[command(subcommand)]
        command: LayoutCommand,
    },
}
#[derive(Subcommand)]
enum LabCommand {
    /// Check raw backing + qcow2 writes using real qemu-img/qemu-io.
    FileCow {
        /// Directory of separately verified QEMU binaries; fixed executable names only.
        #[arg(long)]
        tool_dir: Option<PathBuf>,
    },
    /// Exercise a real SQLite journal with a simulated VM backend.
    Recovery,
    /// Boot an isolated, diskless Linux guest (TCG by default).
    GuestBoot {
        /// Explicitly create a KVM VM; requires host authorization, never falls back to TCG.
        #[arg(long)]
        kvm: bool,
        /// Read-only source Linux kernel; copied into the lab's private directory.
        #[arg(long)]
        kernel: PathBuf,
        /// Directory containing a separately verified qemu-system-x86_64 binary.
        #[arg(long)]
        tool_dir: Option<PathBuf>,
        /// Directory containing separately verified SeaBIOS and Linux loader files.
        #[arg(long)]
        firmware_dir: Option<PathBuf>,
    },
    /// Validate native libvirt bindings with the isolated test:///default driver.
    LibvirtTest,
    /// Reconstruct the SQLite controller against one retained libvirt TEST session.
    LibvirtRecovery,
}
#[derive(Subcommand)]
enum InstallationCommand {
    List,
}
#[derive(Subcommand)]
enum ImportCommand {
    /// Review a one-time return to the preserved original EFI entry.
    NativeReturn {
        #[arg(long)]
        installation: String,
    },
    /// Review complete-group Windows passthrough while preserving driver packages.
    GpuPlan {
        #[arg(long)]
        installation: String,
        #[arg(long)]
        gpu: String,
    },
    Plan {
        #[arg(long)]
        installation: String,
        #[arg(long, value_enum, default_value = "trial")]
        mode: ImportMode,
    },
}
#[derive(ValueEnum, Clone, Copy)]
enum ImportMode {
    Inspect,
    Trial,
    Native,
}
#[derive(Subcommand)]
enum LayoutCommand {
    Plan {
        #[arg(long, default_value = "20GiB", value_parser = parse_size)]
        host_size: u64,
    },
}

fn parse_size(s: &str) -> Result<u64, String> {
    let (digits, unit) = s
        .strip_suffix("GiB")
        .map(|v| (v, GIB))
        .or_else(|| s.strip_suffix("MiB").map(|v| (v, 1024 * 1024)))
        .unwrap_or((s, 1));
    digits
        .parse::<u64>()
        .ok()
        .and_then(|v| v.checked_mul(unit))
        .filter(|v| *v > 0)
        .ok_or_else(|| "size must be positive bytes, MiB or GiB (e.g. 20GiB)".into())
}
#[derive(Serialize)]
struct InstallationList<'a> {
    schema_version: u32,
    source: &'a str,
    installations: &'a [Installation],
    findings: &'a [Finding],
}
fn result_value(cli: &Cli, report: &HardwareReport) -> Result<Value, String> {
    match &cli.command {
        Command::Import {
            command: ImportCommand::NativeReturn { installation },
        } => {
            let inst = report
                .installations
                .iter()
                .find(|i| i.id.0 == *installation)
                .ok_or_else(|| format!("unknown installation {installation:?}"))?;
            serde_json::to_value(elastic_planner::plan_native_return(report, inst))
        }
        Command::Import {
            command: ImportCommand::GpuPlan { installation, gpu },
        } => {
            let inst = report
                .installations
                .iter()
                .find(|i| i.id.0 == *installation)
                .ok_or_else(|| format!("unknown installation {installation:?}"))?;
            serde_json::to_value(elastic_planner::plan_windows_gpu(report, inst, gpu))
        }
        Command::Doctor => serde_json::to_value(doctor::diagnose(report)),
        Command::Readiness { .. } | Command::Lab { .. } => {
            unreachable!("dedicated commands handled before hardware acquisition")
        }
        Command::Probe { hardware: true, .. } => serde_json::to_value(report),
        Command::Probe { .. } | Command::Installations { .. } => {
            serde_json::to_value(InstallationList {
                schema_version: SCHEMA_VERSION,
                source: &report.source,
                installations: &report.installations,
                findings: &report.findings,
            })
        }
        Command::Import {
            command: ImportCommand::Plan { installation, mode },
        } => {
            let inst = report
                .installations
                .iter()
                .find(|i| i.id.0 == *installation)
                .ok_or_else(|| {
                    format!("unknown installation {installation:?}; use installations list")
                })?;
            let policy = match mode {
                ImportMode::Inspect => StoragePolicy::InspectReadOnly,
                ImportMode::Trial => StoragePolicy::TrialOverlay {
                    overlay_id: format!("trial-{}", &digest(inst.id.0.as_bytes())[..16]),
                },
                ImportMode::Native => StoragePolicy::NativeBackedExclusive,
            };
            serde_json::to_value(elastic_planner::plan_import(report, inst, policy))
        }
        Command::Layout {
            command: LayoutCommand::Plan { host_size },
        } => serde_json::to_value(elastic_planner::plan_layout(report, *host_size)),
    }
    .map_err(|e| e.to_string())
}
fn run(cli: &Cli) -> Result<i32, String> {
    match &cli.command {
        Command::Readiness { check_kvm_api } => {
            let report = match &cli.fixture {
                Some(path) => elastic_probe::readiness::probe_readiness_fixture(path)?,
                None => elastic_probe::readiness::probe_readiness(),
            };
            let report = if cli.redact {
                report.redacted()
            } else {
                report
            };
            let baseline = elastic_planner::plan_resources(
                report.memory_total_bytes,
                report.memory_available_bytes,
                &[
                    elastic_planner::MemoryRequest {
                        domain_id: "windows".into(),
                        bytes: 16 * GIB,
                    },
                    elastic_planner::MemoryRequest {
                        domain_id: "manjaro".into(),
                        bytes: 6 * GIB,
                    },
                    elastic_planner::MemoryRequest {
                        domain_id: "services".into(),
                        bytes: 2 * GIB,
                    },
                ],
                8 * GIB,
            );
            let mut value = serde_json::to_value(report).map_err(|e| e.to_string())?;
            value["spec_baseline_memory_plan"] =
                serde_json::to_value(baseline).map_err(|e| e.to_string())?;
            if *check_kvm_api {
                value["kvm_api_probe"] = if cli.dry_run {
                    serde_json::json!({"schema_version":SCHEMA_VERSION,"status":"planned",
                        "node_opened":false,"ioctl_attempted":false,"api_verified":false,
                        "operation":"KVM_GET_API_VERSION only; no VM creation"})
                } else {
                    let probe = match &cli.fixture {
                        Some(path) => elastic_probe::readiness::probe_kvm_api_fixture(path)?,
                        None => elastic_probe::readiness::probe_kvm_api(),
                    };
                    serde_json::to_value(probe).map_err(|e| e.to_string())?
                };
            }
            emit(cli, &value)?;
            return Ok(0);
        }
        Command::Lab { command } => {
            if cli.fixture.is_some() {
                return Err(
                    "laboratories create their own isolated inputs; --fixture is unsupported"
                        .into(),
                );
            }
            let value = if cli.dry_run {
                match command {
                    LabCommand::FileCow { tool_dir } => {
                        let dependencies = match tool_dir {
                            Some(path) => elastic_lab::inspect_dependencies_in(path),
                            None => elastic_lab::inspect_dependencies(),
                        };
                        serde_json::json!({"schema_version":SCHEMA_VERSION,"status":"planned","evidence_kind":"not_run",
                            "dependencies":dependencies,"temporary_files_created":false,"commands_executed":false,
                            "operations":["create private temporary raw base and qcow2 overlay", "write/read known patterns with QEMU", "verify whole-base hashes", "invalidate stale overlay after controlled base edit", "remove owned temporary directory"]})
                    }
                    LabCommand::Recovery => {
                        serde_json::json!({"schema_version":SCHEMA_VERSION,"status":"planned","evidence_kind":"not_run",
                        "temporary_files_created":false,"commands_executed":false,
                        "operations":["create private temporary SQLite journal", "simulate guest lifecycle and restart", "reconcile actual mock ownership", "remove owned temporary database"]})
                    }
                    LabCommand::GuestBoot {
                        kvm,
                        kernel,
                        tool_dir,
                        firmware_dir,
                    } => {
                        let options = guest_options(kernel, tool_dir, firmware_dir);
                        serde_json::json!({"schema_version":SCHEMA_VERSION,"status":"planned","evidence_kind":"not_run",
                            "dependencies":elastic_lab::guest::inspect_guest_dependencies(&options),
                            "temporary_files_created":false,"commands_executed":false,"acceleration":if *kvm { "kvm" } else { "tcg" },"creates_kvm_vm":kvm,
                            "operations":["copy kernel and firmware into a private temporary directory", "assemble bundled PID1 and create initramfs", "boot diskless Linux with 1 vCPU and 256 MiB RAM", "verify serial handshake and guest poweroff", "remove owned temporary directory"]})
                    }
                    LabCommand::LibvirtRecovery => {
                        serde_json::json!({"schema_version":SCHEMA_VERSION,"status":"planned","evidence_kind":"not_run",
                            "temporary_files_created":false,"commands_executed":false,"connection_attempted":false,
                            "uri":"test:///default","physical_resource_leases_supported":false,
                            "operations":["retain one private diskless TEST session", "commit controller lifecycle to temporary SQLite", "reconstruct controller against that same session", "verify unknown observation refusal and idempotent replay", "stop only owned domains and remove temporary state"]})
                    }
                    LabCommand::LibvirtTest => {
                        serde_json::json!({"schema_version":SCHEMA_VERSION,"status":"planned","evidence_kind":"not_run",
                            "temporary_files_created":false,"commands_executed":false,"connection_attempted":false,
                            "uri":"test:///default","native_bindings_enabled":cfg!(feature="native-libvirt"),
                            "operations":["connect to libvirt in-process test driver", "create and inspect only a uniquely named test domain", "stop and remove that test domain", "close test connection"]})
                    }
                }
            } else {
                match command {
                    LabCommand::FileCow { tool_dir } => serde_json::to_value(match tool_dir {
                        Some(path) => elastic_lab::run_file_lab_with_tools(path),
                        None => elastic_lab::run_file_lab(),
                    })
                    .map_err(|e| e.to_string())?,
                    LabCommand::Recovery => {
                        serde_json::to_value(elastic_controller::simulation::run_recovery_lab()?)
                            .map_err(|e| e.to_string())?
                    }
                    LabCommand::GuestBoot {
                        kvm,
                        kernel,
                        tool_dir,
                        firmware_dir,
                    } => {
                        let options = guest_options(kernel, tool_dir, firmware_dir);
                        let report = if *kvm {
                            elastic_lab::guest::run_kvm_guest_lab(&options)
                        } else {
                            elastic_lab::guest::run_guest_lab(&options)
                        };
                        serde_json::to_value(report).map_err(|e| e.to_string())?
                    }
                    LabCommand::LibvirtRecovery => {
                        serde_json::to_value(elastic_controller::libvirt_lab::run_controller_lab())
                            .map_err(|e| e.to_string())?
                    }
                    LabCommand::LibvirtTest => {
                        serde_json::to_value(elastic_virt_libvirt::run_test_driver_lab())
                            .map_err(|e| e.to_string())?
                    }
                }
            };
            let mut value = value;
            if cli.redact {
                elastic_core::redact::redact_value(&HardwareReport::default(), &mut value);
            }
            let exit_code = match value.get("status").and_then(Value::as_str) {
                Some("failed" | "refused") => 1,
                Some("skipped") => 3,
                _ => 0,
            };
            emit(cli, &value)?;
            return Ok(exit_code);
        }
        _ => {}
    }
    let report = match &cli.fixture {
        Some(path) => elastic_probe::probe_fixture(path)?,
        None => elastic_probe::probe_live(),
    };
    if report.schema_version != SCHEMA_VERSION {
        return Err("unsupported hardware report schema".into());
    }
    let mut value = result_value(cli, &report)?;
    if cli.redact {
        elastic_core::redact::redact_value(&report, &mut value);
    }
    emit(cli, &value)?;
    Ok(0)
}
fn guest_options(
    kernel: &std::path::Path,
    tool_dir: &Option<PathBuf>,
    firmware_dir: &Option<PathBuf>,
) -> elastic_lab::guest::GuestLabOptions {
    elastic_lab::guest::GuestLabOptions {
        kernel: kernel.to_owned(),
        tool_directory: tool_dir.clone(),
        firmware_directory: firmware_dir.clone(),
    }
}
fn emit(cli: &Cli, value: &Value) -> Result<(), String> {
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
        );
    } else {
        print_human(value);
    }
    Ok(())
}
fn print_human(value: &Value) {
    // Human output retains all preconditions and gaps; JSON is the stable API.
    if let Some(checks) = value
        .get("checks")
        .and_then(Value::as_array)
        .filter(|_| value.get("ready_for_physical_execution").is_some())
    {
        println!(
            "ElasticPC doctor — {}",
            value["source"].as_str().unwrap_or("unknown")
        );
        for c in checks {
            println!(
                "[{}] {}: {}",
                c["status"].as_str().unwrap_or("unknown"),
                c["code"].as_str().unwrap_or(""),
                c["detail"].as_str().unwrap_or("")
            );
        }
    } else if value.get("preconditions").is_some() {
        println!(
            "Plan {} ({})",
            value["id"].as_str().unwrap_or(""),
            value["kind"].as_str().unwrap_or("")
        );
        println!(
            "Review only. Future execution requires approval: {}",
            value["requires_approval"]
        );
        println!(
            "{}",
            serde_json::to_string_pretty(value).expect("JSON value")
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).expect("JSON value")
        );
    }
}
fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let wants_json = std::env::args_os().any(|arg| arg == "--json");
            if wants_json && error.use_stderr() {
                let redact = std::env::args_os().any(|arg| arg == "--redact");
                let message = if redact {
                    "Invalid arguments; private input details withheld by --redact. Use --help for syntax.".into()
                } else {
                    error.to_string()
                };
                println!(
                    "{}",
                    serde_json::json!({"schema_version":SCHEMA_VERSION,"error":{"code":"invalid_arguments","message":message}})
                );
                std::process::exit(2);
            }
            if error.use_stderr() && std::env::args_os().any(|arg| arg == "--redact") {
                eprintln!("elasticctl: invalid arguments; private input details withheld by --redact. Use --help for syntax.");
                std::process::exit(2);
            }
            error.exit();
        }
    };
    let outcome = run(&cli);
    if let Ok(code) = outcome {
        std::process::exit(code);
    }
    if let Err(error) = outcome {
        let error = if cli.redact {
            "Request failed; private details withheld by --redact. Verify supplied paths and identifiers locally.".to_owned()
        } else {
            error
        };
        if cli.json {
            println!(
                "{}",
                serde_json::json!({"schema_version":SCHEMA_VERSION,"error":{"code":"request_failed","message":error}})
            );
        } else {
            eprintln!("elasticctl: {error}");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_rejects_mutation_verbs() {
        for args in [
            vec!["elasticctl", "vm", "start", "windows"],
            vec!["elasticctl", "import", "apply", "--plan", "x"],
            vec!["elasticctl", "boot", "native", "--target", "windows"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
    #[test]
    fn sizes_checked_and_probe_explicit() {
        assert_eq!(parse_size("20GiB").unwrap(), 20 * GIB);
        for bad in ["0", "-1", "1TB", "18446744073709551615GiB"] {
            assert!(parse_size(bad).is_err());
        }
        assert!(Cli::try_parse_from(["elasticctl", "probe"]).is_err());
        assert!(
            Cli::try_parse_from(["elasticctl", "probe", "--hardware", "--installations"]).is_err()
        );
    }
}
