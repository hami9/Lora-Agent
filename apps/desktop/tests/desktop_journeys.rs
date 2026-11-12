//! Pure-Rust native Discord-style desktop UI & headless diagnostic verification suite
//! (`docs/DESKTOP_UX.md`, `ADR-0004`, `R09`, `R12`).

use lora_core::{
    verify_desktop_snapshot, ApprovalDecision, ApprovalPromptPort, ApprovalRequest,
    DesktopDashboardViewModel, DesktopSnapshot, LoraApplicationService,
    TrustedConsentDialogViewModel, TuningProfile, VirtualizedTranscriptSlice,
    DEFAULT_SAFETY_RESERVE_BYTES,
};
use lora_desktop::app::{ChannelTab, LoraDesktopApp, Message, WorkspaceGuild};
use lora_desktop::backend::{AppWorkerEvent, GuiApprovalPromptPort};
use lora_desktop::dashboard::{DashboardPreset, DashboardWidgetKind, HudDensity, WidgetSpan};
use lora_desktop::theme::{CyberThemePreset, HudGeometry};
use lora_platform::{
    resolve_default_worker_spawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
};
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn open_test_app(
    tmp: &TempDir,
    experimental: bool,
) -> (LoraDesktopApp, Arc<GuiApprovalPromptPort>) {
    let (event_tx, event_rx) = mpsc::channel::<AppWorkerEvent>();
    let approval_port = Arc::new(GuiApprovalPromptPort::new(event_tx.clone()));
    let spawner = resolve_default_worker_spawner(None, Some("deterministic"));
    let service = Arc::new(
        LoraApplicationService::open(
            tmp.path(),
            DEFAULT_SAFETY_RESERVE_BYTES,
            &OsStoreLockPort,
            Arc::new(SysinfoHardwareProbe),
            Arc::new(OsScopedFileExecutor),
            spawner,
            approval_port.clone(),
        )
        .expect("open LoraApplicationService"),
    );

    let app = LoraDesktopApp::new(
        service,
        Some(Arc::clone(&approval_port)),
        event_tx,
        Arc::new(Mutex::new(event_rx)),
        experimental,
    );
    (app, approval_port)
}

#[test]
fn desktop_headless_diagnostics_and_attestation_cli_e2e() {
    let desktop_bin = env!("CARGO_BIN_EXE_lora-desktop");
    let tmp = TempDir::new().expect("tempdir");

    // 1. Default desktop view-model hides experimental channels and keeps Stop reachable
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

    // 2. Passing --experimental enables experimental channels in the status strip
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

    // 3. --preview-consent neutralizes HTML/script injection and sets default_button_focus="deny"
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
    let consent_vm: TrustedConsentDialogViewModel =
        serde_json::from_slice(&consent_out.stdout).expect("parse TrustedConsentDialogViewModel");
    assert_eq!(consent_vm.default_button_focus, "deny");
    assert!(!consent_vm.target_relative_path.contains("<script>"));
    assert!(consent_vm.target_relative_path.contains("&lt;script&gt;"));

    // 4. --slice-transcript bounds visible items to viewport capacity (`R12`)
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
    let slice_vm: VirtualizedTranscriptSlice<String> =
        serde_json::from_slice(&slice_out.stdout).expect("parse VirtualizedTranscriptSlice");
    assert_eq!(slice_vm.total_items, 5000);
    assert_eq!(slice_vm.start_index, 1200);
    assert_eq!(slice_vm.end_index, 1225);
    assert_eq!(slice_vm.visible_items.len(), 25);

    // 5. --dump-snapshot and --verify-snapshot cryptographic attestation
    let dump_out = Command::new(desktop_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--dump-snapshot",
        ])
        .output()
        .expect("run lora-desktop --dump-snapshot");
    assert!(dump_out.status.success());
    let snap: DesktopSnapshot =
        serde_json::from_slice(&dump_out.stdout).expect("parse DesktopSnapshot");
    assert!(snap.snapshot_sha256.is_some());

    let snap_path = tmp.path().join("desktop-snapshot.json");
    std::fs::write(&snap_path, &dump_out.stdout).expect("write snapshot");

    let verify_out = Command::new(desktop_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--verify-snapshot",
            snap_path.to_str().unwrap(),
        ])
        .output()
        .expect("run lora-desktop --verify-snapshot");
    assert!(verify_out.status.success());
    let verify_str = String::from_utf8_lossy(&verify_out.stdout);
    assert!(verify_str.contains("Desktop Snapshot Attestation: verified"));

    // 6. Legacy `--serve` HTTP bridge must not exist in the pure-Rust native shell
    let legacy_serve = Command::new(desktop_bin)
        .args(["--serve", "127.0.0.1:0"])
        .output()
        .expect("check legacy --serve rejection");
    assert!(!legacy_serve.status.success());
}

#[test]
fn native_discord_shell_navigation_modal_consent_broker_and_snapshot_journey() {
    let tmp = TempDir::new().expect("tempdir");
    let (mut app, approval_port) = open_test_app(&tmp, false);

    // 1. Experimental channels stay gated when experimental=false, unlock on ExperimentalLab guild
    assert!(!app.experimental_enabled);
    let _ = app.update(Message::SelectChannel(ChannelTab::AdapterStudio));
    assert_eq!(app.active_channel, ChannelTab::Dashboard);

    let _ = app.update(Message::SelectGuild(WorkspaceGuild::ExperimentalLab));
    assert!(app.experimental_enabled);
    assert_eq!(app.active_channel, ChannelTab::AdapterStudio);

    // Render view trees across multiple Discord channels headlessly
    for ch in [
        ChannelTab::Dashboard,
        ChannelTab::Chat,
        ChannelTab::AgentWorkspace,
        ChannelTab::ModelLibrary,
        ChannelTab::Hardware,
        ChannelTab::WorkflowPipeline,
        ChannelTab::AuditJournal,
        ChannelTab::BrokerGate,
        ChannelTab::Settings,
    ] {
        let _ = app.update(Message::SelectChannel(ch));
        assert_eq!(app.active_channel, ch);
        drop(app.view());
    }

    // 2. Hardware profile & external link air-gap guard
    let _ = app.update(Message::SelectTuningProfile(TuningProfile::LowMemory));
    assert_eq!(app.tuning_profile, TuningProfile::LowMemory);
    let _ = app.update(Message::MarkdownLinkIgnored(
        "https://untrusted.example/exfiltrate".to_string(),
    ));
    assert!(app.status_banner.contains("Blocked external link"));

    // 3. Trusted Consent Modal preview & background GuiApprovalPromptPort handshake (`ApproveOnce` vs `Deny`)
    let _ = app.update(Message::PreviewConsentDialogPressed);
    assert!(app.pending_consent.is_some());
    assert_eq!(
        app.pending_consent
            .as_ref()
            .unwrap()
            .view_model
            .default_button_focus,
        "deny"
    );
    drop(app.view());
    let _ = app.update(Message::StopActiveTask);
    assert!(app.pending_consent.is_none());

    let port_worker = Arc::clone(&approval_port);
    let handle = thread::spawn(move || {
        let req = ApprovalRequest {
            run_id: "run-modal-test".to_string(),
            proposal_id: "prop-modal-1".to_string(),
            tool_id: "read_file".to_string(),
            root_id: "ws-1".to_string(),
            root_display_path: "workspace".to_string(),
            relative_path: "docs/GUIDE.md".to_string(),
            max_bytes: 4096,
            purpose: "Read file".to_string(),
            approval_digest: "sha256:test".to_string(),
        };
        port_worker
            .prompt_user(&req)
            .expect("GuiApprovalPromptPort decision")
    });

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let _ = app.update(Message::Tick);
        if let Some(pending) = &app.pending_consent {
            if pending.request_id > 0 {
                let req_id = pending.request_id;
                let _ = app.update(Message::ResolveConsent {
                    request_id: req_id,
                    decision: ApprovalDecision::ApproveOnce,
                });
                break;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for modal");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(handle.join().unwrap(), ApprovalDecision::ApproveOnce);

    // 4. Broker single-use permit attestation & verification directly from native UI channel
    let _ = app.update(Message::SelectChannel(ChannelTab::BrokerGate));
    let _ = app.update(Message::ExecuteBrokerPermitPressed);
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.latest_broker_receipt.is_none() {
        let _ = app.update(Message::Tick);
        assert!(
            Instant::now() < deadline,
            "timed out waiting for broker receipt"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let (broker_rcpt, verified) = app.latest_broker_receipt.as_ref().unwrap();
    assert!(*verified);
    assert!(broker_rcpt.receipt_sha256.is_some());

    // 5. DesktopSnapshot capture & verification directly from native UI
    let _ = app.update(Message::CaptureAttestedSnapshotPressed);
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.latest_snapshot.is_none() {
        let _ = app.update(Message::Tick);
        assert!(Instant::now() < deadline, "timed out waiting for snapshot");
        thread::sleep(Duration::from_millis(10));
    }
    let (snap, snap_ok) = app.latest_snapshot.as_ref().unwrap();
    assert!(*snap_ok);
    assert!(verify_desktop_snapshot(snap).is_ok());
}

#[test]
fn cyberpunk_dashboard_customization_themes_macros_and_persistence_journey() {
    let tmp = TempDir::new().expect("tempdir");
    {
        let (mut app, _) = open_test_app(&tmp, false);
        assert_eq!(app.active_channel, ChannelTab::Dashboard);

        // 1. Cycle all 4 layout presets and render full pure-Rust view trees
        for preset in DashboardPreset::ALL {
            let _ = app.update(Message::SelectDashboardPreset(preset));
            assert_eq!(app.dashboard.prefs.preset, preset);
            drop(app.view());
        }

        // 2. Open Customize HUD drawer and exercise all 11 Cyberpunk cinema/game/anime/FUI themes
        let _ = app.update(Message::ToggleCustomizeHud);
        assert!(app.dashboard.customize_open);
        for theme in CyberThemePreset::ALL {
            let _ = app.update(Message::SelectCyberTheme(theme));
            assert_eq!(app.dashboard.prefs.theme_preset, theme);
            let _ = app.theme();
            drop(app.view());
        }

        // 3. Density, Tactical Sharp vs Sleek Modern frame geometry, and visual toggles
        for density in HudDensity::ALL {
            let _ = app.update(Message::SelectHudDensity(density));
            assert_eq!(app.dashboard.prefs.density, density);
        }
        for geometry in HudGeometry::ALL {
            let _ = app.update(Message::SelectHudGeometry(geometry));
            assert_eq!(app.dashboard.prefs.geometry, geometry);
        }
        let _ = app.update(Message::ToggleNeonBorders(false));
        let _ = app.update(Message::ToggleSparklineGrid(false));
        assert!(!app.dashboard.prefs.neon_borders);
        assert!(!app.dashboard.prefs.show_sparkline_grid);
        drop(app.view());

        // 4. Widget visibility, collapse, span toggle, and reordering
        let _ = app.update(Message::ToggleWidgetCollapsed(
            DashboardWidgetKind::SystemReactor,
        ));
        let _ = app.update(Message::ToggleWidgetSpan(
            DashboardWidgetKind::SystemReactor,
        ));
        let _ = app.update(Message::MoveWidgetUp(DashboardWidgetKind::WorkflowMatrix));
        let _ = app.update(Message::ToggleWidgetVisible(
            DashboardWidgetKind::AttestationRadar,
        ));
        assert!(app
            .dashboard
            .prefs
            .widgets
            .iter()
            .find(|w| w.kind == DashboardWidgetKind::SystemReactor)
            .is_some_and(|w| w.collapsed && w.span == WidgetSpan::Wide));
        drop(app.view());

        // 5. Custom prompt macro creation + quick prompt launch fallback (stages prompt safely)
        let initial_macros = app.dashboard.prefs.custom_macros.len();
        let _ = app.update(Message::MacroLabelChanged("CyberAudit".to_string()));
        let _ = app.update(Message::MacroPromptChanged(
            "Inspect local architecture invariants".to_string(),
        ));
        let _ = app.update(Message::AddPromptMacro);
        assert_eq!(app.dashboard.prefs.custom_macros.len(), initial_macros + 1);

        let _ = app.update(Message::QuickPromptChanged(
            "Summarize host telemetry".to_string(),
        ));
        let _ = app.update(Message::SubmitQuickPrompt);
        assert!(app.dashboard.quick_prompt_input.is_empty());
        assert_eq!(app.composer_input, "Summarize host telemetry");
        assert_eq!(app.active_channel, ChannelTab::ModelLibrary);

        // Pin specific preferences to verify disk persistence in `dashboard_hud.json`
        let _ = app.update(Message::SelectCyberTheme(CyberThemePreset::GhostInTheShell));
        let _ = app.update(Message::SelectHudGeometry(HudGeometry::CyberSharp));
        let _ = app.update(Message::SelectHudDensity(HudDensity::Compact));
    }

    // 6. Re-open service data dir and confirm HUD state & custom macros persist across restarts
    let (mut reopened, _) = open_test_app(&tmp, false);
    assert_eq!(
        reopened.dashboard.prefs.theme_preset,
        CyberThemePreset::GhostInTheShell
    );
    assert_eq!(reopened.dashboard.prefs.geometry, HudGeometry::CyberSharp);
    assert_eq!(reopened.dashboard.prefs.density, HudDensity::Compact);
    assert!(!reopened.dashboard.prefs.neon_borders);
    assert!(reopened
        .dashboard
        .prefs
        .custom_macros
        .iter()
        .any(|m| m.label == "CyberAudit"));
    drop(reopened.view());

    let idx = reopened
        .dashboard
        .prefs
        .custom_macros
        .iter()
        .position(|m| m.label == "CyberAudit")
        .unwrap();
    let _ = reopened.update(Message::RemovePromptMacro(idx));
    assert!(!reopened
        .dashboard
        .prefs
        .custom_macros
        .iter()
        .any(|m| m.label == "CyberAudit"));

    let _ = reopened.update(Message::ResetDashboardLayout);
    assert_eq!(
        reopened.dashboard.prefs.theme_preset,
        CyberThemePreset::NightCity2077
    );
}
