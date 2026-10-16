use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn cli(fixture: &str, args: &[&str]) -> Output {
    // With an empty PATH a fixture command must still work: acquisition must not
    // fall through to the host. Fixtures require neither root nor host utilities.
    Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .env("PATH", "")
        .args([
            "--fixture",
            root().join("fixtures").join(fixture).to_str().unwrap(),
            "--json",
        ])
        .args(args)
        .output()
        .expect("run local CLI")
}
fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).expect("machine readable stdout")
}
#[test]
fn every_public_command_has_versioned_offline_json() {
    for args in [
        vec!["doctor"],
        vec!["probe", "--hardware"],
        vec!["probe", "--installations"],
        vec!["installations", "list"],
        vec!["layout", "plan", "--host-size", "20GiB"],
        vec![
            "import",
            "native-return",
            "--installation",
            "linux-11111111-1111-1111-1111-111111111111",
        ],
        vec![
            "import",
            "gpu-plan",
            "--installation",
            "linux-11111111-1111-1111-1111-111111111111",
            "--gpu",
            "0000:01:00.0",
        ],
        vec![
            "import",
            "plan",
            "--installation",
            "linux-11111111-1111-1111-1111-111111111111",
            "--mode",
            "trial",
        ],
    ] {
        let out = cli("acacia-redacted", &args);
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(json(&out)["schema_version"], 1);
        assert!(out.stderr.is_empty());
    }
}
#[test]
fn dry_run_returns_identical_plan_and_fixture_bytes_do_not_change() {
    let fixture = root().join("fixtures/acacia-redacted/snapshot.json");
    let before = std::fs::read(&fixture).unwrap();
    let one = cli("acacia-redacted", &["layout", "plan"]);
    let two = cli("acacia-redacted", &["--dry-run", "layout", "plan"]);
    assert!(one.status.success() && two.status.success());
    assert_eq!(json(&one), json(&two));
    assert_eq!(before, std::fs::read(&fixture).unwrap());
}
#[test]
fn missing_fixture_and_unsupported_mutation_are_json_errors() {
    let out = cli("does-not-exist", &["probe", "--hardware"]);
    assert!(!out.status.success());
    assert_eq!(json(&out)["error"]["code"], "request_failed");
    let out = cli(
        "acacia-redacted",
        &["import", "apply", "--plan", "anything"],
    );
    assert!(!out.status.success());
    assert_eq!(json(&out)["error"]["code"], "invalid_arguments");
}
#[test]
fn unknown_installation_and_invalid_size_do_not_make_plans() {
    let out = cli(
        "acacia-redacted",
        &["import", "plan", "--installation", "not-a-real-id"],
    );
    assert!(!out.status.success());
    assert!(json(&out).get("operations").is_none());
    let out = cli("acacia-redacted", &["layout", "plan", "--host-size", "-1"]);
    assert!(!out.status.success());
    assert_eq!(json(&out)["error"]["code"], "invalid_arguments");
}
#[test]
fn redaction_is_marked_and_scrubs_fixture_ids() {
    let out = cli("acacia-redacted", &["--redact", "probe", "--hardware"]);
    assert!(out.status.success());
    assert_eq!(json(&out)["redacted"], true);
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(!s.contains("11111111-1111-1111-1111-111111111111"));
}
#[test]
fn degraded_report_does_not_certify_execution() {
    let out = cli("generic/degraded", &["doctor"]);
    assert!(out.status.success());
    let value = json(&out);
    assert_eq!(value["ready_for_physical_execution"], false);
    assert!(value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["status"] == "needs-verification"));
}
#[test]
fn layout_golden() {
    let out = cli("acacia-redacted", &["layout", "plan"]);
    assert!(out.status.success());
    let expected: Value = serde_json::from_slice(
        &std::fs::read(root().join("tests/plan-golden/acacia-layout.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        json(&out),
        expected,
        "Review intentional plan changes and regenerate tests/plan-golden/acacia-layout.json"
    );
}
#[test]
fn import_golden() {
    let out = cli(
        "acacia-redacted",
        &[
            "import",
            "plan",
            "--installation",
            "linux-11111111-1111-1111-1111-111111111111",
            "--mode",
            "trial",
        ],
    );
    assert!(out.status.success());
    let expected: Value = serde_json::from_slice(
        &std::fs::read(root().join("tests/plan-golden/acacia-import-trial.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        json(&out),
        expected,
        "Review intentional plan changes and regenerate tests/plan-golden/acacia-import-trial.json"
    );
}
#[test]
fn redacted_errors_never_echo_user_paths_or_ids() {
    for args in [
        vec![
            "--fixture",
            "/home/alice/private/missing",
            "--redact",
            "--json",
            "probe",
            "--hardware",
        ],
        vec![
            "--redact",
            "--json",
            "layout",
            "plan",
            "--host-size",
            "/home/alice/private/input",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(json(&output).get("error").is_some());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("alice"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("alice"));
    }
}

#[test]
fn readiness_is_offline_and_redacts_resume_targets() {
    let out = cli("acacia-redacted", &["--redact", "readiness"]);
    assert!(out.status.success());
    let value = json(&out);
    assert_eq!(value["source"], "fixture");
    assert_eq!(value["redacted"], true);
    assert_eq!(value["libvirt"]["live_connection_attempted"], false);
    assert!(value.get("spec_baseline_memory_plan").is_some());
}
#[test]
fn lab_dry_run_cannot_create_files_even_with_invalid_temp_root() {
    let parent = tempfile::tempdir().unwrap();
    let absent = parent.path().join("must-remain-absent");
    for mode in ["file-cow", "recovery", "libvirt-test", "libvirt-recovery"] {
        let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
            .env("TMPDIR", &absent)
            .args(["--json", "--dry-run", "lab", mode])
            .output()
            .unwrap();
        assert!(output.status.success());
        let value = json(&output);
        assert_eq!(value["status"], "planned");
        assert_eq!(value["temporary_files_created"], false);
        assert_eq!(value["commands_executed"], false);
        assert!(!absent.exists());
    }
}
#[test]
fn sqlite_lab_reports_real_metadata_and_mock_backend() {
    let parent = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .env("TMPDIR", parent.path())
        .args(["--json", "lab", "recovery"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = json(&output);
    assert_eq!(value["status"], "passed");
    assert_eq!(value["evidence_kind"], "real_sqlite_with_mock_backend");
    assert_eq!(value["physical_device_execution"], false);
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
}
#[test]
fn labs_reject_fixture_inputs_before_writing_anything() {
    for args in [
        vec!["lab", "recovery"],
        vec!["lab", "libvirt-test"],
        vec!["lab", "libvirt-recovery"],
        vec!["lab", "guest-boot", "--kernel", "/must-not-read/kernel"],
    ] {
        let out = cli("acacia-redacted", &args);
        assert!(!out.status.success());
        assert_eq!(json(&out)["error"]["code"], "request_failed");
    }
}

#[test]
fn guest_dry_run_does_not_launch_tools_or_create_artifacts() {
    let parent = tempfile::tempdir().unwrap();
    let absent = parent.path().join("must-remain-absent");
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .env("TMPDIR", &absent)
        .env("PATH", "")
        .args(["--json", "--dry-run", "lab", "guest-boot", "--kernel"])
        .arg(absent.join("kernel"))
        .arg("--tool-dir")
        .arg(&absent)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["status"], "planned");
    assert_eq!(value["evidence_kind"], "not_run");
    assert_eq!(value["commands_executed"], false);
    assert_eq!(value["temporary_files_created"], false);
    assert_eq!(value["acceleration"], "tcg");
    assert!(!absent.exists());
}

#[test]
fn kvm_guest_dry_run_is_explicit_and_has_no_side_effects() {
    let parent = tempfile::tempdir().unwrap();
    let absent = parent.path().join("must-remain-absent");
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .env("TMPDIR", &absent)
        .env("PATH", "")
        .args([
            "--json",
            "--dry-run",
            "lab",
            "guest-boot",
            "--kvm",
            "--kernel",
        ])
        .arg(absent.join("kernel"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["acceleration"], "kvm");
    assert_eq!(value["creates_kvm_vm"], true);
    assert_eq!(value["commands_executed"], false);
    assert_eq!(value["temporary_files_created"], false);
    assert!(!absent.exists());
    let rejected = cli(
        "acacia-redacted",
        &[
            "lab",
            "guest-boot",
            "--kvm",
            "--kernel",
            "/must-not-read/kernel",
        ],
    );
    assert!(!rejected.status.success());
}

#[test]
fn explicit_kvm_probe_respects_fixture_and_dry_run() {
    let out = cli("acacia-redacted", &["readiness", "--check-kvm-api"]);
    assert!(out.status.success());
    let value = json(&out);
    assert_eq!(value["kvm_api_probe"]["source"], "fixture");
    assert_eq!(value["kvm_api_probe"]["api_verified"], false);
    assert_eq!(value["kvm_api_probe"]["ioctl_attempted"], false);
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .args(["--json", "--dry-run", "readiness", "--check-kvm-api"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["kvm_api_probe"]["status"], "planned");
    assert_eq!(value["kvm_api_probe"]["node_opened"], false);
    assert_eq!(value["kvm_api_probe"]["ioctl_attempted"], false);
}

#[test]
fn libvirt_dry_run_never_connects_and_has_no_uri_override() {
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .args(["--json", "--dry-run", "lab", "libvirt-test"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["connection_attempted"], false);
    assert_eq!(value["uri"], "test:///default");
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .args(["--json", "lab", "libvirt-test", "--uri", "qemu:///system"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json(&output)["error"]["code"], "invalid_arguments");
}

#[cfg(not(feature = "native-libvirt"))]
#[test]
fn libvirt_without_native_feature_is_not_a_pass() {
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .args(["--json", "lab", "libvirt-test"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let value = json(&output);
    assert_eq!(value["status"], "skipped");
    assert_eq!(value["guest_boot_verified"], false);
}
#[test]
fn missing_lab_tools_are_not_a_successful_test_run() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .args(["--json", "lab", "file-cow", "--tool-dir"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value = json(&output);
    assert!(matches!(
        value["status"].as_str(),
        Some("skipped" | "refused")
    ));
    assert_eq!(value["evidence_kind"], "not_run");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn durable_libvirt_lab_reports_its_actual_scope() {
    let output = Command::new(env!("CARGO_BIN_EXE_elasticctl"))
        .args(["--json", "lab", "libvirt-recovery"])
        .output()
        .unwrap();
    let value = json(&output);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["physical_resource_leases_supported"], false);
    assert_eq!(value["process_crash_verified"], false);
    if cfg!(feature = "native-libvirt") {
        assert!(output.status.success(), "{value}");
        assert_eq!(value["status"], "passed");
        assert_eq!(value["cleanup_complete"], true);
        assert_eq!(value["controller_reconstructed"], true);
    } else {
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(value["evidence_kind"], "not_run");
    }
}
