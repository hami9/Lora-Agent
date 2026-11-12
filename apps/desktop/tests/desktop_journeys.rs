//! Pure-Rust native multi-pane desktop UI & headless diagnostic verification suite
//! (`docs/DESKTOP_UX.md`, `ADR-0004`, `R09`, `R12`).

use iced::{keyboard, window, Event};
use lora_core::{
    verify_desktop_snapshot, ApprovalDecision, ApprovalPromptPort, ApprovalRequest,
    DesktopDashboardViewModel, DesktopSnapshot, LoraApplicationService,
    TrustedConsentDialogViewModel, TuningProfile, VirtualizedTranscriptSlice,
    DEFAULT_SAFETY_RESERVE_BYTES,
};
use lora_desktop::app::{ChannelTab, ChatMessageEntry, LoraDesktopApp, Message, WorkspaceGuild};
use lora_desktop::backend::{AppWorkerEvent, GuiApprovalPromptPort};
use lora_desktop::dashboard::{
    CapriAuraPreset, CapriEyeMode, DashboardPreset, DashboardWidgetKind, HudDensity, WidgetSpan,
};
use lora_desktop::theme::{CyberThemePreset, HudGeometry, SurfaceDepth};
use lora_desktop::ux_studio::{self, AgentPhase};
use lora_desktop::workspace_db::{
    AgentKind, AgentPersona, ExecutionAccessMode, FolderAccessLevel, KvCacheType, ReasoningEffort,
    TaskFilter, TaskKind, TaskPriority, TaskStatus,
};
use lora_platform::{
    resolve_default_worker_spawner, OsScopedFileExecutor, OsStoreLockPort, SysinfoHardwareProbe,
};
use std::fs;
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
fn native_shell_navigation_modal_consent_broker_and_snapshot_journey() {
    let tmp = TempDir::new().expect("tempdir");
    let (mut app, approval_port) = open_test_app(&tmp, false);

    // 1. Experimental channels stay gated when experimental=false, unlock on ExperimentalLab guild
    assert!(!app.experimental_enabled);
    let _ = app.update(Message::SelectChannel(ChannelTab::AdapterStudio));
    assert_eq!(app.active_channel, ChannelTab::Dashboard);

    let _ = app.update(Message::SelectGuild(WorkspaceGuild::ExperimentalLab));
    assert!(app.experimental_enabled);
    assert_eq!(app.active_channel, ChannelTab::AdapterStudio);

    // Render view trees across multiple workspace channels headlessly
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

#[test]
fn desktop_agent_projects_tasks_llamacpp_and_infinite_studio_persist_e2e() {
    let tmp = TempDir::new().expect("create tempdir");
    let custom_dir = tmp
        .path()
        .join("sandbox_project")
        .to_string_lossy()
        .to_string();
    let created_project_id: i64;
    let created_task_id: i64;

    {
        let (mut app, _) = open_test_app(&tmp, true);

        // 1. Create & activate a new scoped project
        let _ = app.update(Message::SelectChannel(ChannelTab::AgentWorkspace));
        let _ = app.update(Message::NewProjectNameChanged(
            "Cyber Lab Alpha".to_string(),
        ));
        let _ = app.update(Message::NewProjectTagChanged("alpha".to_string()));
        let _ = app.update(Message::NewProjectRootChanged(custom_dir.clone()));
        let _ = app.update(Message::NewProjectDescChanged(
            "Isolated local agent test suite".to_string(),
        ));
        let _ = app.update(Message::NewProjectPromptChanged(
            "Always verify SHA-256 digests.".to_string(),
        ));
        let _ = app.update(Message::CreateWorkspaceProject);

        assert!(app
            .projects_cache
            .iter()
            .any(|p| p.name == "Cyber Lab Alpha" && p.accent_tag == "ALPHA"));
        created_project_id = app.active_project_id;
        assert_eq!(app.workspace_root_input, custom_dir);
        drop(app.view());

        // 2. Tasks DB CRUD, filter & Run in Agent dispatch
        let _ = app.update(Message::SelectChannel(ChannelTab::WorkflowPipeline));
        let _ = app.update(Message::NewTaskTitleChanged(
            "Audit local llama.cpp worker params".to_string(),
        ));
        let _ = app.update(Message::NewTaskNotesChanged(
            "Verify Q8_0 KV quantization & batch size".to_string(),
        ));
        let _ = app.update(Message::NewTaskAssigneeChanged("CodexSub".to_string()));
        let _ = app.update(Message::NewTaskPrioritySelected(TaskPriority::P0));
        let _ = app.update(Message::CreateAgentTask);

        let tasks = app.workspace_db.list_tasks(Some(created_project_id));
        assert_eq!(tasks.len(), 1);
        created_task_id = tasks[0].id;
        assert_eq!(tasks[0].priority, TaskPriority::P0);
        assert_eq!(tasks[0].assignee, "CodexSub");
        assert_eq!(tasks[0].status, TaskStatus::Todo);

        let _ = app.update(Message::SelectTaskFilter(TaskFilter::Open));
        let _ = app.update(Message::TaskSearchChanged("llama.cpp".to_string()));
        drop(app.view());

        let _ = app.update(Message::ExecuteTaskInAgent {
            task_id: created_task_id,
            prompt: "Audit local llama.cpp worker params".to_string(),
        });
        let updated = app.workspace_db.list_tasks(Some(created_project_id));
        assert_eq!(updated[0].status, TaskStatus::InProgress);

        // 3. llama.cpp control panel tuning & HIE sync
        let _ = app.update(Message::SelectChannel(ChannelTab::Hardware));
        let _ = app.update(Message::LlamaGpuLayersChanged(48));
        let _ = app.update(Message::LlamaThreadsChanged(12));
        let _ = app.update(Message::LlamaBatchChanged(1024));
        let _ = app.update(Message::LlamaKvQuantChanged(KvCacheType::Q4_0));
        let _ = app.update(Message::LlamaToggleFlashAttention(false));
        let _ = app.update(Message::LlamaToggleKvOffload(false));
        let _ = app.update(Message::LlamaToggleMmap(false));
        let _ = app.update(Message::LlamaToggleMlock(true));
        let _ = app.update(Message::LlamaTempChanged(45));
        let _ = app.update(Message::LlamaTopPChanged(88));
        let _ = app.update(Message::LlamaTopKChanged(25));
        let _ = app.update(Message::LlamaRepeatPenaltyChanged(118));
        let _ = app.update(Message::ContextTokensChanged(8192));

        let cli = app.llama_config.cli_preview(Some("models/qwen-7b.gguf"));
        assert!(cli.contains("-ngl 48"));
        assert!(cli.contains("-t 12"));
        assert!(cli.contains("-b 1024"));
        assert!(cli.contains("-c 8192"));
        assert!(cli.contains("--cache-type-k q4_0"));
        assert!(cli.contains("--mlock"));
        assert!(cli.contains("--no-mmap"));
        assert!(cli.contains("--no-kv-offload"));
        drop(app.view());

        // 4. Infinite Custom Studio controls
        let _ = app.update(Message::SelectChannel(ChannelTab::Settings));
        let _ = app.update(Message::ToggleCustomTheme(true));
        let _ = app.update(Message::CustomPrimaryHueChanged(312));
        let _ = app.update(Message::CustomSecondaryHueChanged(185));
        let _ = app.update(Message::SelectSurfaceDepth(SurfaceDepth::PitchOled));
        let _ = app.update(Message::CornerRadiusChanged(12));
        let _ = app.update(Message::GlowStrengthChanged(85));
        let _ = app.update(Message::SidebarWidthChanged(276));
        let _ = app.update(Message::ToggleIconRail(false));
        let _ = app.update(Message::ToggleStatusFooter(false));
        let _ = app.update(Message::ToggleMonoChrome(true));
        drop(app.view());
    }

    // Reopen on same data dir and verify SQLite + JSON preferences survived restart
    let (mut reopened, _) = open_test_app(&tmp, false);
    assert_eq!(reopened.active_project_id, created_project_id);
    assert_eq!(reopened.active_project_name(), "Cyber Lab Alpha");
    assert_eq!(reopened.workspace_root_input, custom_dir);

    let persisted_tasks = reopened.workspace_db.list_tasks(Some(created_project_id));
    assert_eq!(persisted_tasks.len(), 1);
    assert_eq!(persisted_tasks[0].id, created_task_id);
    assert_eq!(persisted_tasks[0].status, TaskStatus::InProgress);

    assert_eq!(reopened.llama_config.n_gpu_layers, 48);
    assert_eq!(reopened.llama_config.n_threads, 12);
    assert_eq!(reopened.llama_config.n_batch, 1024);
    assert_eq!(reopened.llama_config.ctx_size, 8192);
    assert_eq!(reopened.llama_config.kv_cache_type, KvCacheType::Q4_0);
    assert!(!reopened.llama_config.flash_attention);
    assert!(!reopened.llama_config.kv_offload);
    assert!(!reopened.llama_config.mmap);
    assert!(reopened.llama_config.mlock);
    assert_eq!(reopened.llama_config.temperature_x100, 45);
    assert_eq!(reopened.llama_config.top_p_x100, 88);
    assert_eq!(reopened.llama_config.top_k, 25);
    assert_eq!(reopened.llama_config.repeat_penalty_x100, 118);

    assert!(reopened.dashboard.prefs.custom_theme.enabled);
    assert_eq!(reopened.dashboard.prefs.custom_theme.primary_hue, 312);
    assert_eq!(reopened.dashboard.prefs.custom_theme.secondary_hue, 185);
    assert_eq!(
        reopened.dashboard.prefs.custom_theme.surface_depth,
        SurfaceDepth::PitchOled
    );
    assert_eq!(reopened.dashboard.prefs.corner_radius_px, 12);
    assert_eq!(reopened.dashboard.prefs.glow_strength_pct, 85);
    assert_eq!(reopened.dashboard.prefs.sidebar_width_px, 276);
    assert!(!reopened.dashboard.prefs.show_icon_rail);
    assert!(!reopened.dashboard.prefs.show_status_footer);
    assert!(reopened.dashboard.prefs.use_mono_chrome);

    assert!(reopened.dashboard.prefs.cyber_motion);

    // Smooth delta-time cyber motion verification: trigger rail & theme transition then step ticks
    let initial_phase = reopened.dashboard.motion.phase;
    let _ = reopened.update(Message::ToggleIconRail(true));
    let _ = reopened.update(Message::SelectCyberTheme(CyberThemePreset::TheMatrix1999));
    for _ in 0..8 {
        thread::sleep(Duration::from_millis(12));
        let _ = reopened.update(Message::Tick);
    }
    assert!(reopened.dashboard.motion.phase > initial_phase);
    assert!(reopened.dashboard.animated_rail_width() > 5.0);

    // Turning cyber motion off snaps layout & palette immediately without delay
    let _ = reopened.update(Message::ToggleCyberMotion(false));
    assert!(!reopened.dashboard.prefs.cyber_motion);
    assert_eq!(reopened.dashboard.animated_rail_width(), 74.0);
    assert_eq!(
        reopened.dashboard.palette().accent_primary,
        CyberThemePreset::TheMatrix1999.palette().accent_primary
    );

    // Cleanly delete task & custom project, falling back to first remaining project
    let _ = reopened.update(Message::DeleteAgentTask(created_task_id));
    assert!(reopened
        .workspace_db
        .list_tasks(Some(created_project_id))
        .is_empty());
    let _ = reopened.update(Message::DeleteWorkspaceProject(created_project_id));
    assert_ne!(reopened.active_project_id, created_project_id);
    assert!(!reopened.projects_cache.is_empty());
}

#[test]
fn desktop_modern_ux_cot_artifacts_multimodal_undo_and_palette_e2e() {
    // 1) CoT (<think>) & fenced artifact extraction
    let streaming_cot = ux_studio::parse_structured_response("<think>checking buffer");
    assert!(streaming_cot.thinking_in_progress);
    assert!(streaming_cot.answer.is_empty());

    let parsed = ux_studio::parse_structured_response(
        "<think>Evaluate lock ordering and memory usage.</think>\n\
         Fixed the race cleanly.\n\
         ```rust\nfn safe_sync() -> bool { true }\n```",
    );
    assert!(!parsed.thinking_in_progress);
    assert_eq!(
        parsed.thinking.as_deref(),
        Some("Evaluate lock ordering and memory usage.")
    );
    assert_eq!(parsed.artifacts.len(), 1);
    assert_eq!(parsed.artifacts[0].lang, "rust");
    assert!(parsed.artifacts[0].content.contains("safe_sync"));

    // 2) Full app E2E: Ctrl+K Command Palette, FileDropped multimodal, Persona, Undo & Pause
    let tmp = TempDir::new().expect("tempdir");
    let drop_code = tmp.path().join("sample.rs");
    let drop_img = tmp.path().join("preview.png");
    let drop_gguf = tmp.path().join("tiny.gguf");
    fs::write(&drop_code, "pub fn demo() -> i32 { 42 }\n").expect("write rs");
    fs::write(&drop_img, b"\x89PNG\r\n\x1a\n").expect("write png");
    fs::write(&drop_gguf, b"GGUF\x03\x00\x00\x00").expect("write gguf");

    let (event_tx, event_rx) = mpsc::channel::<AppWorkerEvent>();
    let approval_port = Arc::new(GuiApprovalPromptPort::new(event_tx.clone()));
    let spawner = resolve_default_worker_spawner(None, Some("deterministic"));
    let service = Arc::new(
        LoraApplicationService::open(
            &tmp.path().join("store"),
            DEFAULT_SAFETY_RESERVE_BYTES,
            &OsStoreLockPort,
            Arc::new(SysinfoHardwareProbe),
            Arc::new(OsScopedFileExecutor),
            spawner,
            approval_port.clone(),
        )
        .expect("open LoraApplicationService"),
    );
    let event_rx = Arc::new(Mutex::new(event_rx));

    let mut app = LoraDesktopApp::new(
        Arc::clone(&service),
        Some(approval_port),
        event_tx.clone(),
        Arc::clone(&event_rx),
        false,
    );

    assert_eq!(app.agent_phase(), AgentPhase::Idle);

    // Ctrl+K opens palette & Esc closes it
    let _ = app.update(Message::RuntimeEvent(Event::Keyboard(
        keyboard::Event::KeyPressed {
            key: keyboard::Key::Character("k".into()),
            modified_key: keyboard::Key::Character("k".into()),
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyK),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::CTRL,
            text: None,
            repeat: false,
        },
    )));
    assert!(app.command_palette_open);

    let _ = app.update(Message::RunCommandPaletteAction(Box::new(
        Message::SelectChannel(ChannelTab::Hardware),
    )));
    assert!(!app.command_palette_open);
    assert_eq!(app.active_channel, ChannelTab::Hardware);

    // Drag & Drop stages code, image & audio to Chat, and GGUF to ModelLibrary
    let drop_wav = tmp.path().join("voice.wav");
    fs::write(&drop_wav, b"RIFF\x24\x00\x00\x00WAVE").expect("write wav");
    let _ = app.update(Message::RuntimeEvent(Event::Window(
        window::Event::FileDropped(drop_code),
    )));
    let _ = app.update(Message::RuntimeEvent(Event::Window(
        window::Event::FileDropped(drop_img),
    )));
    let _ = app.update(Message::RuntimeEvent(Event::Window(
        window::Event::FileDropped(drop_wav),
    )));
    assert_eq!(app.active_channel, ChannelTab::Chat);
    assert_eq!(app.staged_attachments.len(), 3);
    assert_eq!(
        app.staged_attachments[0].kind,
        ux_studio::AttachmentKind::CodeDoc
    );
    assert_eq!(
        app.staged_attachments[1].kind,
        ux_studio::AttachmentKind::Image
    );
    assert_eq!(
        app.staged_attachments[2].kind,
        ux_studio::AttachmentKind::Audio
    );

    let _ = app.update(Message::RuntimeEvent(Event::Window(
        window::Event::FileDropped(drop_gguf.clone()),
    )));
    assert_eq!(app.active_channel, ChannelTab::ModelLibrary);
    assert!(app.import_path_input.contains("tiny.gguf"));

    // Undo rollback on deleted task & theme change
    let _ = app.update(Message::SelectCyberTheme(CyberThemePreset::BladeRunner2049));
    let first_task_id = app
        .workspace_db
        .list_tasks(Some(app.active_project_id))
        .first()
        .expect("seeded task")
        .id;
    let initial_task_count = app
        .workspace_db
        .list_tasks(Some(app.active_project_id))
        .len();
    let _ = app.update(Message::DeleteAgentTask(first_task_id));
    assert_eq!(
        app.workspace_db
            .list_tasks(Some(app.active_project_id))
            .len(),
        initial_task_count - 1
    );
    let _ = app.update(Message::UndoLastAction);
    assert_eq!(
        app.workspace_db
            .list_tasks(Some(app.active_project_id))
            .len(),
        initial_task_count
    );
    let _ = app.update(Message::UndoLastAction);
    assert_eq!(
        app.dashboard.prefs.theme_preset,
        CyberThemePreset::NightCity2077
    );

    // Pause / Resume delta buffering during simulated active run
    app.active_run_id = Some("sim-run-1".to_string());
    app.messages
        .push(ChatMessageEntry::assistant("sim-run-1", "qwen", "", true));
    let _ = app.update(Message::TogglePauseRun);
    assert_eq!(app.agent_phase(), AgentPhase::Paused);
    event_tx
        .send(AppWorkerEvent::RunDelta {
            run_id: "sim-run-1".to_string(),
            delta: "<think>trace step</think>\nBuffered token output.".to_string(),
        })
        .expect("send delta");
    let _ = app.update(Message::Tick);
    assert!(app.messages.last().unwrap().raw_text.is_empty());
    let _ = app.update(Message::TogglePauseRun);
    assert_eq!(app.agent_phase(), AgentPhase::Streaming);
    assert!(app
        .messages
        .last()
        .unwrap()
        .raw_text
        .contains("Buffered token output."));
    assert_eq!(
        app.messages.last().unwrap().thinking_text.as_deref(),
        Some("trace step")
    );
    app.active_run_id = None;

    // Animated Lora Mini-Bot poke bounce spring & rotating quip
    assert!(app.dashboard.prefs.show_mini_bot);
    let _ = app.update(Message::PokeMiniBot);
    assert_eq!(app.bot_poke_count, 1);
    assert!(app.dashboard.motion.bot_bounce.value > 0.5);
    assert!(app.status_banner.starts_with("Lora Bot:"));
    thread::sleep(Duration::from_millis(16));
    let _ = app.update(Message::Tick);

    // Artifacts Drawer + ClearProjectChat + Undo rollback + Spring convergence + Reduced Motion snap
    let _ = app.update(Message::ToggleArtifactPanel);
    assert!(app.artifact_panel_open);
    thread::sleep(Duration::from_millis(16));
    let _ = app.update(Message::Tick);
    assert!(app.dashboard.motion.spring_artifacts.value > 0.0);

    app.messages
        .push(ChatMessageEntry::user("usr-hist", "Saved prompt"));
    let _ = app.update(Message::ClearProjectChat);
    assert_eq!(app.messages.len(), 1);
    let _ = app.update(Message::UndoLastAction);
    assert!(app
        .messages
        .iter()
        .any(|m| m.raw_text.contains("Saved prompt")));

    let _ = app.update(Message::ToggleCyberMotion(false));
    let _ = app.update(Message::ToggleArtifactPanel);
    assert!(!app.artifact_panel_open);
    assert_eq!(app.dashboard.motion.spring_artifacts.value, 0.0);

    let _ = app.update(Message::ToggleMiniBot(false));
    assert!(!app.dashboard.prefs.show_mini_bot);
    let _ = app.update(Message::ToggleMiniBot(true));
    assert!(app.dashboard.prefs.show_mini_bot);

    // Persona & project chat history persist cleanly across session reopen
    let _ = app.update(Message::SelectPersona(AgentPersona::StrictAuditor));
    let reopened = LoraDesktopApp::new(service, None, event_tx, event_rx, false);
    assert_eq!(reopened.persona, AgentPersona::StrictAuditor);
    assert!(reopened
        .messages
        .iter()
        .any(|m| m.raw_text.contains("Saved prompt")));
}

#[test]
fn desktop_agent_options_trusted_folders_dotenv_daily_and_capri_e2e() {
    let tmp = TempDir::new().expect("temp dir");
    let workspace_dir = tmp.path().join("workspace");
    let extra_folder = tmp.path().join("trusted_repo");
    fs::create_dir_all(&workspace_dir).expect("create workspace dir");
    fs::create_dir_all(&extra_folder).expect("create trusted repo dir");
    fs::write(workspace_dir.join("main.rs"), "fn main() {}").expect("write main.rs");
    fs::write(
        workspace_dir.join(".env"),
        "OPENAI_API_KEY=sk-test-secret-999\nWORKSPACE_MODE=cyber_dev\n",
    )
    .expect("write .env");

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
        .expect("open core service"),
    );
    let event_rx = Arc::new(Mutex::new(event_rx));
    let mut app = LoraDesktopApp::new(
        Arc::clone(&service),
        Some(approval_port.clone()),
        event_tx.clone(),
        Arc::clone(&event_rx),
        false,
    );
    app.workspace_root_input = workspace_dir.to_string_lossy().to_string();

    // 1. Agent Type, Reasoning Effort, Execution Access {Local, Api}
    let _ = app.update(Message::SelectAgentKind(AgentKind::Coder));
    let _ = app.update(Message::SelectReasoningEffort(ReasoningEffort::High));
    let _ = app.update(Message::SelectAccessMode(ExecutionAccessMode::Api));
    let _ = app.update(Message::ApiBaseUrlChanged(
        "https://api.example.invalid/v1".to_string(),
    ));
    let _ = app.update(Message::ApiModelNameChanged("gpt-4o-mini".to_string()));
    assert_eq!(app.agent_kind, AgentKind::Coder);
    assert_eq!(app.reasoning_effort, ReasoningEffort::High);
    assert_eq!(app.access_mode, ExecutionAccessMode::Api);

    // 2. Daily & Completed Tasks tracking + Undo preservation of TaskKind
    let _ = app.update(Message::NewTaskKindSelected(TaskKind::Daily));
    let _ = app.update(Message::NewTaskTitleChanged(
        "Morning codebase audit".to_string(),
    ));
    let _ = app.update(Message::CreateAgentTask);
    let daily_task = app
        .workspace_db
        .list_tasks(Some(app.active_project_id))
        .into_iter()
        .find(|t| t.title == "Morning codebase audit")
        .expect("daily task created");
    assert_eq!(daily_task.task_kind, TaskKind::Daily);
    let _ = app.update(Message::SetTaskStatus {
        task_id: daily_task.id,
        status: TaskStatus::Done,
    });
    let completed = app
        .workspace_db
        .list_tasks(Some(app.active_project_id))
        .into_iter()
        .find(|t| t.id == daily_task.id)
        .expect("daily task still exists");
    assert_eq!(completed.status, TaskStatus::Done);
    assert!(completed.completed_at_ms.is_some());
    let _ = app.update(Message::SelectTaskFilter(TaskFilter::Done));
    assert_eq!(app.task_filter, TaskFilter::Done);

    // 3. Drag-and-drop Folder -> Trusted Folders, Drag-and-drop .env -> Scoped Env Manager
    let _ = app.update(Message::RuntimeEvent(Event::Window(
        window::Event::FileHovered(extra_folder.clone()),
    )));
    assert!(app.drag_hover_path.is_some());
    let _ = app.update(Message::RuntimeEvent(Event::Window(
        window::Event::FileDropped(extra_folder.clone()),
    )));
    assert!(app.drag_hover_path.is_none());
    let _ = app.update(Message::NewTrustLevelSelected(FolderAccessLevel::Trusted));
    assert!(app
        .trusted_folders
        .iter()
        .any(|f| f.path == extra_folder.to_string_lossy()));

    let dotenv_file = workspace_dir.join(".env");
    let _ = app.update(Message::RuntimeEvent(Event::Window(
        window::Event::FileDropped(dotenv_file),
    )));
    assert_eq!(app.active_channel, ChannelTab::CodeWorkspace);
    assert!(app.env_vars.iter().any(|e| e.env_key == "OPENAI_API_KEY"));
    assert_eq!(
        app.workspace_db
            .resolve_env_value(app.active_project_id, "WORKSPACE_MODE")
            .as_deref(),
        Some("cyber_dev")
    );

    // Secret reveal & toggle & manual upsert
    let _ = app.update(Message::NewEnvKeyChanged("LOCAL_TOKEN".to_string()));
    let _ = app.update(Message::NewEnvValChanged("secret-abc".to_string()));
    let _ = app.update(Message::NewEnvSecretToggled(true));
    let _ = app.update(Message::SaveEnvVar);
    let local_env_id = app
        .env_vars
        .iter()
        .find(|e| e.env_key == "LOCAL_TOKEN")
        .expect("saved LOCAL_TOKEN")
        .id;
    let _ = app.update(Message::ToggleRevealSecret(local_env_id));
    assert!(app.reveal_secret_ids.contains(&local_env_id));

    // Attach workspace file by path submit
    let _ = app.update(Message::AttachPathChanged("main.rs".to_string()));
    let _ = app.update(Message::AttachPathSubmit);
    assert!(app
        .staged_attachments
        .iter()
        .any(|a| a.label.contains("main.rs")));

    // 4. Context Token Budget calculation
    let ctx_usage = ux_studio::compute_context_budget_usage(
        &app.messages,
        &app.staged_attachments,
        &app.composer_input,
        app.requested_context,
    );
    assert!(ctx_usage.total_tokens > 0);
    assert!(ctx_usage.max_tokens >= 2048);

    // 5. Capri Customizer & Companion Widget
    let _ = app.update(Message::CapriAuraPresetChanged(
        CapriAuraPreset::PhantomViolet,
    ));
    let _ = app.update(Message::CapriEyeModeChanged(CapriEyeMode::Hypno));
    let _ = app.update(Message::CapriGlitchChanged(85));
    let _ = app.update(Message::CapriScaleChanged(68));
    let _ = app.update(Message::CapriToggleGrin(true));
    let _ = app.update(Message::CapriToggleDockInChat(true));
    let _ = app.update(Message::CapriToggleHeaderChip(true));
    assert_eq!(
        app.dashboard.prefs.capri.aura_preset,
        CapriAuraPreset::PhantomViolet
    );
    assert_eq!(app.dashboard.prefs.capri.eye_mode, CapriEyeMode::Hypno);
    assert_eq!(app.dashboard.prefs.capri.glitch_intensity_pct, 85);
    assert_eq!(app.dashboard.prefs.capri.widget_scale_px, 68);
    assert!(app.dashboard.prefs.capri.always_toothy_grin);

    // 6. Persist Agent Kind, Effort, Access Mode & Capri Prefs across restart
    let _ = app.update(Message::SelectAccessMode(ExecutionAccessMode::Local));
    let reopened = LoraDesktopApp::new(service, Some(approval_port), event_tx, event_rx, false);
    assert_eq!(reopened.agent_kind, AgentKind::Coder);
    assert_eq!(reopened.reasoning_effort, ReasoningEffort::High);
    assert_eq!(reopened.access_mode, ExecutionAccessMode::Local);
    assert_eq!(
        reopened.dashboard.prefs.capri.aura_preset,
        CapriAuraPreset::PhantomViolet
    );
    assert_eq!(reopened.dashboard.prefs.capri.eye_mode, CapriEyeMode::Hypno);
}
