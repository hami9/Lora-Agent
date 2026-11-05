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
    assert!(html.contains("context-budget-live"));
    assert!(html.contains("broker-status-card"));
    assert!(html.contains("doctor-live"));

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

    // 5. Verify --preview-consent neutralizes HTML/script injection and sets default_button_focus="deny"
    let consent_out = Command::new(desktop_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--preview-consent",
            "docs/<script>alert(1)</script>.md",
        ])
        .output()
        .expect("run lora-desktop --preview-consent");
    assert!(consent_out.status.success());
    let consent_vm: lora_core::TrustedConsentDialogViewModel =
        serde_json::from_slice(&consent_out.stdout).expect("parse TrustedConsentDialogViewModel");
    assert_eq!(consent_vm.default_button_focus, "deny");
    assert!(!consent_vm.target_relative_path.contains("<script>"));
    assert!(consent_vm.target_relative_path.contains("&lt;script&gt;"));

    // 6. Verify --slice-transcript bounds visible DOM items to viewport capacity (`R12`)
    let slice_out = Command::new(desktop_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--slice-transcript",
            "5000,1200,25",
        ])
        .output()
        .expect("run lora-desktop --slice-transcript");
    assert!(slice_out.status.success());
    let slice_vm: lora_core::VirtualizedTranscriptSlice<String> =
        serde_json::from_slice(&slice_out.stdout).expect("parse VirtualizedTranscriptSlice");
    assert_eq!(slice_vm.total_items, 5000);
    assert_eq!(slice_vm.start_index, 1200);
    assert_eq!(slice_vm.end_index, 1225);
    assert_eq!(slice_vm.visible_items.len(), 25);
}

fn http_request(addr: &str, raw: &str) -> (String, String) {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).expect("connect loopback bridge");
    stream.write_all(raw.as_bytes()).expect("write request");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read response");
    let (head, body) = resp.split_once("\r\n\r\n").expect("http response");
    (head.to_string(), body.to_string())
}

#[test]
fn desktop_loopback_bridge_serves_ui_and_view_models() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let desktop_bin = env!("CARGO_BIN_EXE_lora-desktop");
    let tmp = TempDir::new().expect("tempdir");

    // Non-loopback binds are refused outright.
    let refused = Command::new(desktop_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--serve",
            "0.0.0.0:0",
            "--once",
        ])
        .output()
        .expect("run lora-desktop --serve 0.0.0.0");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("non-loopback"));

    let mut child = Command::new(desktop_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--serve",
            "127.0.0.1:0",
            "--max-requests",
            "14",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lora-desktop --serve");
    let mut first_line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first_line)
        .expect("read listening line");
    let addr = first_line
        .trim()
        .rsplit("http://")
        .next()
        .unwrap()
        .trim_end_matches('/')
        .to_string();
    let get = |path: &str| {
        http_request(
            &addr,
            &format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        )
    };

    // 1. Index HTML with strict CSP headers
    let (head, body) = get("/");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(head.contains("connect-src 'none'"));
    assert!(head.contains("frame-ancestors 'none'"));
    assert!(head.contains("X-Content-Type-Options: nosniff"));
    assert!(body.contains("screen-dashboard"));

    // 2. Dashboard view-model JSON
    let (head, body) = get("/api/dashboard");
    assert!(head.starts_with("HTTP/1.1 200"));
    let vm: DesktopDashboardViewModel = serde_json::from_str(&body).expect("dashboard json");
    assert!(vm.status_strip.stop_command_reachable);

    // 3. Consent preview escapes injected markup and defaults focus to Deny
    let (head, body) = get("/api/consent-preview?path=docs%2F%3Cscript%3E.md");
    assert!(head.starts_with("HTTP/1.1 200"));
    let consent: lora_core::TrustedConsentDialogViewModel =
        serde_json::from_str(&body).expect("consent json");
    assert_eq!(consent.default_button_focus, "deny");
    assert!(!consent.target_relative_path.contains("<script>"));

    // 4. Transcript slicing endpoint
    let (_, body) = get("/api/slice-transcript?total=500&offset=10&capacity=20");
    let slice: lora_core::VirtualizedTranscriptSlice<String> =
        serde_json::from_str(&body).expect("slice json");
    assert_eq!(slice.visible_items.len(), 20);

    // 5. Risk matrix endpoint returns all verified risk items
    let (head, body) = get("/api/risk-matrix");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(body.contains("\"risk_id\": \"R01\""));
    assert!(body.contains("\"risk_id\": \"R18\""));

    // 6. Probed hardware snapshot endpoint
    let (head, body) = get("/api/hardware");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(body.contains("physical_cores"));

    // 7. Installed extensions endpoint
    let (head, body) = get("/api/plugins");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(body.starts_with('['));

    // 8. Stored hardware auto-tuning receipts endpoint
    let (head, body) = get("/api/tuning");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(body.starts_with('['));

    // 9. Audit events journal endpoint
    let (head, body) = get("/api/audit");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(body.starts_with('['));

    // 10. Context budget deterministic allocation endpoint
    let (head, body) = get("/api/context-budget");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(body.contains("\"context_window_tokens\": 2048"));
    assert!(body.contains("\"reserved_output_tokens\": 512"));

    // 11. Privileged broker status endpoint
    let (head, body) = get("/api/broker-status");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(body.contains("\"status\": \"ready\""));
    assert!(body.contains("\"replay_defense\": \"enforced\""));

    // 12. Holistic system doctor diagnosis endpoint
    let (head, body) = get("/api/doctor");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(body.contains("\"status\": \"healthy\""));
    assert!(body.contains("physical_cores"));

    // 13. DNS-rebinding defense: foreign Host header is rejected
    let (head, _) = http_request(
        &addr,
        "GET /api/dashboard HTTP/1.1\r\nHost: evil.example:80\r\nConnection: close\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 403"));

    // 14. Read-only bridge: non-GET methods are rejected
    let (head, _) = http_request(
        &addr,
        &format!("POST /api/dashboard HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 0\r\n\r\n"),
    );
    assert!(head.starts_with("HTTP/1.1 405"));

    let status = child.wait().expect("bridge exits after max requests");
    assert!(status.success());
}
