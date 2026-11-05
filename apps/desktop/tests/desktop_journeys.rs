//! Desktop UX & Tauri capability verification suite (`docs/DESKTOP_UX.md`, `ADR-0004`, `R12`).

use lora_core::DesktopDashboardViewModel;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn desktop_shell_view_model_csp_and_capabilities_e2e() {
    let desktop_bin = env!("CARGO_BIN_EXE_lora-desktop");
    let tmp = TempDir::new().expect("tempdir");

    // 1. Default desktop view-model hides experimental screens and keeps Stop reachable
    let out = Command::new(desktop_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--dump-view-model",
        ])
        .output()
        .expect("run lora-desktop --dump-view-model");
    assert!(out.status.success());
    let vm: DesktopDashboardViewModel =
        serde_json::from_slice(&out.stdout).expect("parse DesktopDashboardViewModel");
    assert_eq!(vm.status_strip.execution_destination, "local_cpu_worker");
    assert!(vm.status_strip.stop_command_reachable);
    assert!(!vm.status_strip.experimental_screens_enabled);

    // 2. Passing --experimental enables experimental screens in the status strip
    let exp_out = Command::new(desktop_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--experimental",
            "--dump-view-model",
        ])
        .output()
        .expect("run lora-desktop --experimental --dump-view-model");
    assert!(exp_out.status.success());
    let exp_vm: DesktopDashboardViewModel = serde_json::from_slice(&exp_out.stdout)
        .expect("parse experimental DesktopDashboardViewModel");
    assert!(exp_vm.status_strip.experimental_screens_enabled);

    // 3. Verify UI HTML shell enforces connect-src 'none', 8 screens, Deny autofocus, and Capri marker
    let html = include_str!("../ui/index.html");
    assert!(html.contains("connect-src 'none'"));
    for screen_id in [
        "screen-dashboard",
        "screen-chat",
        "screen-agent-workspace",
        "screen-model-library",
        "screen-hardware",
        "screen-adapter-studio",
        "screen-extension-manager",
        "screen-settings",
    ] {
        assert!(
            html.contains(screen_id),
            "missing required desktop screen {screen_id}"
        );
    }
    assert!(html.contains("id=\"consent-deny-btn\" autofocus"));
    assert!(html.contains("NC-EDGERUNNER-02::CAPRI"));

    // 4. Verify Tauri v2 capability manifest disallows remote URLs and shell/fs ambient plugins
    let cap_raw = include_str!("../capabilities/main-window.json");
    let cap_json: serde_json::Value =
        serde_json::from_str(cap_raw).expect("valid main-window.json");
    assert_eq!(cap_json["local"], serde_json::Value::Bool(true));
    assert_eq!(
        cap_json["remote"]["urls"].as_array().map(|a| a.len()),
        Some(0)
    );
    let perms = cap_json["permissions"].to_string();
    assert!(!perms.contains("shell:"));
    assert!(!perms.contains("fs:allow-all"));
}
