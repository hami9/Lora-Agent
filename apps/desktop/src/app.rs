//! Pure-Rust native Discord-style desktop client (`LoraDesktopApp`) built on `iced 0.14.0`.

use crate::backend::{
    collect_core_telemetry, spawn_broker_attestation, spawn_chat_run, spawn_demo_workflow,
    spawn_desktop_snapshot, spawn_model_delete, spawn_model_import, AppWorkerEvent, CoreTelemetry,
    GuiApprovalPromptPort,
};
use crate::build_consent_preview;
use crate::dashboard::{
    self, DashboardPreferences, DashboardPreset, DashboardState, DashboardWidgetKind, HudDensity,
};
use crate::theme::{
    self, CyberThemePreset, HudGeometry, AMBER, AMBER_SOFT, BG_CARD, BG_COMPOSER, BG_RAIL,
    BG_SELECTED, BG_SIDEBAR, BG_STAGE, BLURPLE, BLURPLE_HOVER, BORDER_SUBTLE, GREEN, LORA_MAGENTA,
    RED, RED_HOVER, RED_SOFT, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    button, checkbox, column, container, image, markdown, progress_bar, row, rule, scrollable,
    space, stack, text, text_input, Column, Row,
};
use iced::{Alignment, Element, Font, Length, Padding, Subscription, Task, Theme};
use lora_core::{
    allocate_context_budget, default_risk_register_matrix, now_epoch_ms,
    slice_virtualized_transcript, ApprovalDecision, BrokerExecutionReceipt, ContextBudgetRequest,
    DesktopScreen, DesktopSnapshot, ExecutionMode, LoraApplicationService,
    TrustedConsentDialogViewModel, TuningProfile,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Left guild rail workspaces modeled on a native Discord client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceGuild {
    LocalCore,
    ScopedDocs,
    TrustGovernance,
    ExperimentalLab,
}

impl WorkspaceGuild {
    pub const ALL: [Self; 4] = [
        Self::LocalCore,
        Self::ScopedDocs,
        Self::TrustGovernance,
        Self::ExperimentalLab,
    ];

    pub fn badge_text(self) -> &'static str {
        match self {
            Self::LocalCore => "LA",
            Self::ScopedDocs => "WS",
            Self::TrustGovernance => "SG",
            Self::ExperimentalLab => "EX",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::LocalCore => "Lora Local Core",
            Self::ScopedDocs => "Workspace & RAG",
            Self::TrustGovernance => "Security & Broker",
            Self::ExperimentalLab => "Experimental Lab",
        }
    }

    pub fn default_channel(self) -> ChannelTab {
        match self {
            Self::LocalCore => ChannelTab::Dashboard,
            Self::ScopedDocs => ChannelTab::Chat,
            Self::TrustGovernance => ChannelTab::BrokerGate,
            Self::ExperimentalLab => ChannelTab::AdapterStudio,
        }
    }
}

/// Channels in the left `#channels` Discord sidebar mapped onto canonical [`DesktopScreen`] states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelTab {
    Dashboard,
    Chat,
    AgentWorkspace,
    ModelLibrary,
    Hardware,
    WorkflowPipeline,
    AuditJournal,
    BrokerGate,
    AdapterStudio,
    ExtensionManager,
    Settings,
}

impl ChannelTab {
    pub const CORE_CHANNELS: [Self; 7] = [
        Self::Dashboard,
        Self::Chat,
        Self::AgentWorkspace,
        Self::ModelLibrary,
        Self::Hardware,
        Self::WorkflowPipeline,
        Self::Settings,
    ];

    pub const SECURITY_CHANNELS: [Self; 2] = [Self::AuditJournal, Self::BrokerGate];

    pub const EXPERIMENTAL_CHANNELS: [Self; 2] = [Self::AdapterStudio, Self::ExtensionManager];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Chat => "chat",
            Self::AgentWorkspace => "agent-workspace",
            Self::ModelLibrary => "model-library",
            Self::Hardware => "hardware-hie",
            Self::WorkflowPipeline => "workflows",
            Self::AuditJournal => "audit-journal",
            Self::BrokerGate => "broker-gate",
            Self::AdapterStudio => "adapter-studio",
            Self::ExtensionManager => "extension-host",
            Self::Settings => "settings",
        }
    }

    pub fn topic(self) -> &'static str {
        match self {
            Self::Dashboard => "System posture, offline readiness & hardware budget",
            Self::Chat => "Streaming local GGUF inference with Markdown & tool citations",
            Self::AgentWorkspace => {
                "Scoped folder grant policy, consent preview & transcript windowing"
            }
            Self::ModelLibrary => "Verified SHA-256 content-addressed GGUF model inventory",
            Self::Hardware => {
                "Hardware Intelligence Engine (HIE), KV admission & context budgeting"
            }
            Self::WorkflowPipeline => {
                "Air-gapped deterministic multi-step workflows & attested receipts"
            }
            Self::AuditJournal => "Redacted SQLite audit log & emergency credential revocation",
            Self::BrokerGate => "Single-use privileged broker permits (`lora.broker.receipt.v1`)",
            Self::AdapterStudio => "Experimental LoRA evaluation & split leakage gates",
            Self::ExtensionManager => "Experimental sandboxed MCP/Wasm extension lifecycle",
            Self::Settings => {
                "Experimental channel toggles, snapshot attestation & runtime build info"
            }
        }
    }

    pub fn guild(self) -> WorkspaceGuild {
        match self {
            Self::Dashboard | Self::ModelLibrary | Self::Hardware | Self::Settings => {
                WorkspaceGuild::LocalCore
            }
            Self::Chat | Self::AgentWorkspace => WorkspaceGuild::ScopedDocs,
            Self::WorkflowPipeline | Self::AuditJournal | Self::BrokerGate => {
                WorkspaceGuild::TrustGovernance
            }
            Self::AdapterStudio | Self::ExtensionManager => WorkspaceGuild::ExperimentalLab,
        }
    }

    pub fn is_experimental(self) -> bool {
        matches!(self, Self::AdapterStudio | Self::ExtensionManager)
    }

    pub fn to_desktop_screen(self) -> DesktopScreen {
        match self {
            Self::Dashboard => DesktopScreen::Dashboard,
            Self::Chat => DesktopScreen::Chat,
            Self::AgentWorkspace => DesktopScreen::AgentWorkspace,
            Self::ModelLibrary => DesktopScreen::ModelLibrary,
            Self::Hardware => DesktopScreen::Hardware,
            Self::WorkflowPipeline
            | Self::AuditJournal
            | Self::BrokerGate
            | Self::ExtensionManager => DesktopScreen::ExtensionManager,
            Self::AdapterStudio => DesktopScreen::AdapterStudio,
            Self::Settings => DesktopScreen::Settings,
        }
    }
}

/// Message item displayed in `#chat` with incremental `markdown::Content`.
#[derive(Debug)]
pub struct ChatMessageEntry {
    pub id: String,
    pub is_user: bool,
    pub author: String,
    pub badge: String,
    pub raw_text: String,
    pub markdown: markdown::Content,
    pub outcome_sha256: Option<String>,
    pub outcome_verified: bool,
    pub citations: Vec<String>,
    pub receipt_count: usize,
    pub is_streaming: bool,
}

impl ChatMessageEntry {
    pub fn user(id: impl Into<String>, text: impl Into<String>) -> Self {
        let raw = text.into();
        Self {
            id: id.into(),
            is_user: true,
            author: "Local Operator".to_string(),
            badge: "OPERATOR".to_string(),
            markdown: markdown::Content::parse(&raw),
            raw_text: raw,
            outcome_sha256: None,
            outcome_verified: false,
            citations: Vec::new(),
            receipt_count: 0,
            is_streaming: false,
        }
    }

    pub fn assistant(
        id: impl Into<String>,
        model_label: impl Into<String>,
        initial_text: &str,
        streaming: bool,
    ) -> Self {
        Self {
            id: id.into(),
            is_user: false,
            author: format!("Lora · {}", model_label.into()),
            badge: "LOCAL".to_string(),
            raw_text: initial_text.to_string(),
            markdown: markdown::Content::parse(initial_text),
            outcome_sha256: None,
            outcome_verified: false,
            citations: Vec::new(),
            receipt_count: 0,
            is_streaming: streaming,
        }
    }

    pub fn append_delta(&mut self, delta: &str) {
        self.raw_text.push_str(delta);
        self.markdown.push_str(delta);
    }

    pub fn replace_content(&mut self, full_text: &str) {
        self.raw_text = full_text.to_string();
        self.markdown = markdown::Content::parse(full_text);
    }
}

/// Non-spoofable consent prompt state rendered inside the modal overlay (`docs/DESKTOP_UX.md`).
#[derive(Debug, Clone)]
pub struct PendingConsentPrompt {
    pub request_id: u64,
    pub view_model: TrustedConsentDialogViewModel,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    SelectGuild(WorkspaceGuild),
    SelectChannel(ChannelTab),
    ToggleInspector,
    ToggleExperimental(bool),
    RefreshTelemetry,
    StopActiveTask,
    // Chat interactions
    ChatComposerChanged(String),
    SendChatMessage,
    SelectModel(String),
    ToggleAssistantMode(bool),
    ToggleRag(bool),
    WorkspacePathChanged(String),
    SelectTuningProfile(TuningProfile),
    ContextTokensChanged(u32),
    MarkdownLinkIgnored(markdown::Uri),
    // Model management
    ImportPathChanged(String),
    ImportAliasChanged(String),
    ImportModelPressed,
    DeleteModelPressed(String),
    // Workflow / Broker / Recovery / Snapshot / Workspace preview
    RunWorkflowPressed,
    SelectBrokerService(String),
    ToggleBrokerRestart(bool),
    ExecuteBrokerPermitPressed,
    RunIncidentRecoveryPressed,
    CaptureAttestedSnapshotPressed,
    PreviewConsentDialogPressed,
    SliceTranscriptStep(usize),
    // Modal approval choices (Deny is always the primary safe default)
    ResolveConsent {
        request_id: u64,
        decision: ApprovalDecision,
    },
    // Modular Dashboard & Cyberpunk HUD customization
    SelectDashboardPreset(DashboardPreset),
    SelectCyberTheme(CyberThemePreset),
    SelectHudDensity(HudDensity),
    SelectHudGeometry(HudGeometry),
    ToggleCustomizeHud,
    ToggleNeonBorders(bool),
    ToggleSparklineGrid(bool),
    ToggleWidgetVisible(DashboardWidgetKind),
    ToggleWidgetCollapsed(DashboardWidgetKind),
    ToggleWidgetSpan(DashboardWidgetKind),
    MoveWidgetUp(DashboardWidgetKind),
    MoveWidgetDown(DashboardWidgetKind),
    ResetDashboardLayout,
    QuickPromptChanged(String),
    SubmitQuickPrompt,
    RunPromptMacro(String),
    MacroLabelChanged(String),
    MacroPromptChanged(String),
    AddPromptMacro,
    RemovePromptMacro(usize),
}

/// Top-level `iced` application state for `lora-desktop`.
pub struct LoraDesktopApp {
    service: Arc<LoraApplicationService>,
    approval_port: Option<Arc<GuiApprovalPromptPort>>,
    event_tx: mpsc::Sender<AppWorkerEvent>,
    event_rx: Arc<Mutex<mpsc::Receiver<AppWorkerEvent>>>,

    pub active_guild: WorkspaceGuild,
    pub active_channel: ChannelTab,
    pub show_inspector: bool,
    pub experimental_enabled: bool,
    pub telemetry: CoreTelemetry,
    pub status_banner: String,

    // Chat state
    pub messages: Vec<ChatMessageEntry>,
    pub composer_input: String,
    pub selected_model: Option<String>,
    pub read_only_assistant: bool,
    pub use_rag: bool,
    pub workspace_root_input: String,
    pub requested_context: u32,
    pub tuning_profile: TuningProfile,
    pub active_run_id: Option<String>,
    active_cancel_flag: Option<Arc<AtomicBool>>,
    next_run_seq: u64,

    // Model import form
    pub import_path_input: String,
    pub import_alias_input: String,

    // Broker state
    pub broker_service_name: String,
    pub broker_restart_verb: bool,
    pub latest_broker_receipt: Option<(BrokerExecutionReceipt, bool)>,

    // Latest snapshot & workspace diagnostics
    pub latest_snapshot: Option<(DesktopSnapshot, bool)>,
    pub transcript_offset: usize,
    pub pending_consent: Option<PendingConsentPrompt>,
    pub dashboard: DashboardState,
    logo_handle: image::Handle,
}

impl LoraDesktopApp {
    /// Construct the native desktop shell state over an open [`LoraApplicationService`].
    pub fn new(
        service: Arc<LoraApplicationService>,
        approval_port: Option<Arc<GuiApprovalPromptPort>>,
        event_tx: mpsc::Sender<AppWorkerEvent>,
        event_rx: Arc<Mutex<mpsc::Receiver<AppWorkerEvent>>>,
        experimental_enabled: bool,
    ) -> Self {
        let telemetry = collect_core_telemetry(&service, experimental_enabled)
            .unwrap_or_else(|e| panic!("failed to collect initial desktop telemetry: {e}"));
        let mut dashboard = DashboardState::new(&telemetry);
        dashboard.prefs = DashboardPreferences::load_from_dir(service.data_dir());
        dashboard.prefs.sanitize();
        let selected_model = telemetry.models.first().map(|m| m.display_alias.clone());
        let intro = ChatMessageEntry::assistant(
            "welcome-0",
            "System Shell",
            "Welcome to **Lora Agent Desktop (`v0.2.23`)** — 100% pure-Rust native GUI.\n\n\
             - **Air-Gapped Runtime**: Zero WebView, zero HTML/JS bridge, zero external egress.\n\
             - **Cryptographic Attestation**: Every `AgentRunOutcome`, `WorkflowExecutionReceipt`, `BrokerExecutionReceipt`, and `DesktopSnapshot` is SHA-256 verified before display.\n\n\
             Use `#model-library` to import a local `.gguf` artifact, then send prompts directly here in `#chat`.",
            false,
        );

        Self {
            service,
            approval_port,
            event_tx,
            event_rx,
            active_guild: WorkspaceGuild::LocalCore,
            active_channel: ChannelTab::Dashboard,
            show_inspector: true,
            experimental_enabled,
            telemetry,
            status_banner: "Air-gapped native engine ready · local_cpu_worker".to_string(),
            messages: vec![intro],
            composer_input: String::new(),
            selected_model,
            read_only_assistant: false,
            use_rag: false,
            workspace_root_input: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            requested_context: 2048,
            tuning_profile: TuningProfile::Balanced,
            active_run_id: None,
            active_cancel_flag: None,
            next_run_seq: 1,
            import_path_input: String::new(),
            import_alias_input: String::new(),
            broker_service_name: "lora-local-service".to_string(),
            broker_restart_verb: false,
            latest_broker_receipt: None,
            latest_snapshot: None,
            transcript_offset: 0,
            pending_consent: None,
            dashboard,
            logo_handle: crate::load_logo_handle(),
        }
    }

    pub fn title(&self) -> String {
        format!(
            "Lora Agent · #{} ({})",
            self.active_channel.slug(),
            self.active_guild.title()
        )
    }

    pub fn theme(&self) -> Theme {
        self.dashboard.palette().iced_theme()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let interval_ms = if self.active_run_id.is_some() || self.pending_consent.is_some() {
            16
        } else {
            120
        };
        iced::time::every(Duration::from_millis(interval_ms)).map(|_| Message::Tick)
    }

    fn refresh_telemetry(&mut self) {
        if let Ok(fresh) = collect_core_telemetry(&self.service, self.experimental_enabled) {
            if self.selected_model.is_none()
                || !fresh
                    .models
                    .iter()
                    .any(|m| Some(&m.display_alias) == self.selected_model.as_ref())
            {
                self.selected_model = fresh.models.first().map(|m| m.display_alias.clone());
            }
            self.dashboard.record_telemetry_sample(&fresh);
            self.telemetry = fresh;
        }
    }

    fn persist_dashboard_prefs(&self) {
        self.dashboard.prefs.save_to_dir(self.service.data_dir());
    }

    fn dispatch_chat_prompt(&mut self, prompt: String) {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        if self.active_run_id.is_some() {
            self.composer_input = prompt;
            self.status_banner =
                "A task is already running — press STOP before dispatching another turn."
                    .to_string();
            return;
        }
        let Some(model) = self.selected_model.clone() else {
            self.composer_input = prompt;
            self.status_banner =
                "No verified GGUF model imported yet. Prompt staged — import a model in #model-library."
                    .to_string();
            self.active_channel = ChannelTab::ModelLibrary;
            self.active_guild = WorkspaceGuild::LocalCore;
            return;
        };

        let seq = self.next_run_seq;
        self.next_run_seq += 1;
        let run_id = format!("desk-run-{}-{seq}", now_epoch_ms());
        self.composer_input.clear();
        self.active_channel = ChannelTab::Chat;
        self.active_guild = WorkspaceGuild::LocalCore;
        self.messages
            .push(ChatMessageEntry::user(format!("usr-{seq}"), &prompt));
        self.messages
            .push(ChatMessageEntry::assistant(&run_id, &model, "", true));

        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.active_cancel_flag = Some(Arc::clone(&cancel_flag));
        self.active_run_id = Some(run_id.clone());
        self.status_banner = format!("Running {run_id} on '{model}' (local worker)...");

        let mode = if self.read_only_assistant {
            ExecutionMode::ReadOnlyAssistant
        } else {
            ExecutionMode::ChatOnly
        };
        let ws_path = self
            .read_only_assistant
            .then(|| std::path::PathBuf::from(self.workspace_root_input.trim()));

        spawn_chat_run(
            Arc::clone(&self.service),
            self.event_tx.clone(),
            run_id,
            model,
            self.requested_context,
            self.tuning_profile,
            mode,
            ws_path,
            prompt,
            self.use_rag,
            cancel_flag,
        );
    }

    fn drain_worker_events(&mut self) {
        let Ok(rx) = self.event_rx.try_lock() else {
            return;
        };
        let mut needs_refresh = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AppWorkerEvent::ApprovalRequested {
                    request_id,
                    view_model,
                } => {
                    self.pending_consent = Some(PendingConsentPrompt {
                        request_id,
                        view_model,
                    });
                    self.status_banner = format!(
                        "Consent required before reading '{}'",
                        self.pending_consent
                            .as_ref()
                            .map(|p| p.view_model.target_relative_path.as_str())
                            .unwrap_or("")
                    );
                }
                AppWorkerEvent::RunDelta { run_id, delta } => {
                    if let Some(msg) = self.messages.iter_mut().rev().find(|m| m.id == run_id) {
                        msg.append_delta(&delta);
                    }
                }
                AppWorkerEvent::RunFinished { run_id, result } => {
                    self.active_run_id = None;
                    self.active_cancel_flag = None;
                    self.pending_consent = None;
                    if let Some(msg) = self.messages.iter_mut().rev().find(|m| m.id == run_id) {
                        msg.is_streaming = false;
                        match result {
                            Ok(summary) => {
                                if msg.raw_text.trim().is_empty()
                                    && !summary.outcome.answer.is_empty()
                                {
                                    msg.replace_content(&summary.outcome.answer);
                                }
                                msg.outcome_sha256 = summary.outcome.outcome_sha256.clone();
                                msg.outcome_verified = summary.outcome_verified;
                                msg.citations = summary
                                    .outcome
                                    .citations
                                    .iter()
                                    .map(|c| {
                                        let lines = c
                                            .line_range
                                            .map(|(s, e)| format!(":L{s}-L{e}"))
                                            .unwrap_or_default();
                                        format!("{}{lines}", c.relative_path)
                                    })
                                    .collect();
                                msg.receipt_count = summary.outcome.receipts.len();
                                self.status_banner = format!(
                                    "Run {run_id} finished ({:?}, verified={})",
                                    summary.outcome.final_state, summary.outcome_verified
                                );
                            }
                            Err(err) => {
                                msg.replace_content(&format!("**Run Stopped / Error:** `{err}`"));
                                self.status_banner = err;
                            }
                        }
                    }
                    needs_refresh = true;
                }
                AppWorkerEvent::ModelImported { result } => {
                    match result {
                        Ok(summary) => self.status_banner = format!("Model ready: {summary}"),
                        Err(err) => self.status_banner = format!("Import failed: {err}"),
                    }
                    needs_refresh = true;
                }
                AppWorkerEvent::ModelDeleted { result } => {
                    match result {
                        Ok(summary) => self.status_banner = summary,
                        Err(err) => self.status_banner = format!("Delete failed: {err}"),
                    }
                    needs_refresh = true;
                }
                AppWorkerEvent::WorkflowFinished { result } => {
                    match result {
                        Ok(receipt) => {
                            self.status_banner = format!(
                                "Workflow '{}' finished in {}ms (sha256:{}...)",
                                receipt.workflow_id,
                                receipt.total_duration_ms,
                                receipt.receipt_sha256.chars().take(12).collect::<String>()
                            );
                        }
                        Err(err) => self.status_banner = format!("Workflow error: {err}"),
                    }
                    needs_refresh = true;
                }
                AppWorkerEvent::BrokerAttested { result } => match result {
                    Ok((receipt, verified)) => {
                        self.status_banner = format!(
                            "Broker receipt '{}' attested (verified={verified})",
                            receipt.receipt_id
                        );
                        self.latest_broker_receipt = Some((receipt, verified));
                    }
                    Err(err) => self.status_banner = format!("Broker error: {err}"),
                },
                AppWorkerEvent::SnapshotCaptured { result } => match result {
                    Ok((snap, verified)) => {
                        self.status_banner = format!(
                            "DesktopSnapshot '{}' captured (verified={verified})",
                            snap.anti_spoofing_token
                        );
                        self.latest_snapshot = Some((snap, verified));
                    }
                    Err(err) => self.status_banner = format!("Snapshot error: {err}"),
                },
            }
        }
        if needs_refresh {
            drop(rx);
            self.refresh_telemetry();
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.drain_worker_events();
                if self.dashboard.should_sample_on_tick() {
                    self.refresh_telemetry();
                }
            }
            Message::SelectGuild(guild) => {
                self.active_guild = guild;
                let next = guild.default_channel();
                if next.is_experimental() {
                    self.experimental_enabled = true;
                }
                self.active_channel = next;
                self.refresh_telemetry();
            }
            Message::SelectChannel(ch) => {
                if ch.is_experimental() && !self.experimental_enabled {
                    self.status_banner =
                        "Enable experimental channels in #settings or select the [EX] guild."
                            .to_string();
                } else {
                    self.active_channel = ch;
                    self.active_guild = ch.guild();
                    self.refresh_telemetry();
                }
            }
            Message::ToggleInspector => {
                self.show_inspector = !self.show_inspector;
            }
            Message::ToggleExperimental(enabled) => {
                self.experimental_enabled = enabled;
                if !enabled && self.active_channel.is_experimental() {
                    self.active_channel = ChannelTab::Dashboard;
                    self.active_guild = WorkspaceGuild::LocalCore;
                }
                self.refresh_telemetry();
            }
            Message::RefreshTelemetry => {
                self.refresh_telemetry();
                self.status_banner = "Telemetry & SQLite state synchronized.".to_string();
            }
            Message::StopActiveTask => {
                if let Some(flag) = &self.active_cancel_flag {
                    flag.store(true, Ordering::SeqCst);
                }
                if let Some(port) = &self.approval_port {
                    port.deny_all_pending();
                }
                self.pending_consent = None;
                self.status_banner =
                    "Cooperative cancellation + fail-closed Deny signaled.".to_string();
            }
            Message::ChatComposerChanged(val) => {
                self.composer_input = val;
            }
            Message::SendChatMessage => {
                let prompt = self.composer_input.clone();
                self.dispatch_chat_prompt(prompt);
            }
            Message::SelectModel(alias) => {
                self.selected_model = Some(alias.clone());
                self.status_banner = format!("Selected local model '{alias}'.");
            }
            Message::ToggleAssistantMode(ro) => {
                self.read_only_assistant = ro;
            }
            Message::ToggleRag(enabled) => {
                self.use_rag = enabled;
            }
            Message::WorkspacePathChanged(path) => {
                self.workspace_root_input = path;
            }
            Message::SelectTuningProfile(profile) => {
                self.tuning_profile = profile;
                self.status_banner = format!("Active HIE profile: {}", profile.as_str());
            }
            Message::ContextTokensChanged(ctx) => {
                self.requested_context = ctx;
            }
            Message::MarkdownLinkIgnored(uri) => {
                self.status_banner =
                    format!("Blocked external link in air-gapped desktop mode: {}", uri);
            }
            Message::ImportPathChanged(p) => {
                self.import_path_input = p;
            }
            Message::ImportAliasChanged(a) => {
                self.import_alias_input = a;
            }
            Message::ImportModelPressed => {
                let path = self.import_path_input.trim();
                if path.is_empty() {
                    self.status_banner = "Enter a local .gguf path first.".to_string();
                    return Task::none();
                }
                let alias = (!self.import_alias_input.trim().is_empty())
                    .then(|| self.import_alias_input.trim().to_string());
                self.status_banner = format!("Importing GGUF '{path}'...");
                spawn_model_import(
                    Arc::clone(&self.service),
                    self.event_tx.clone(),
                    path.into(),
                    alias,
                );
            }
            Message::DeleteModelPressed(alias) => {
                self.status_banner = format!("Retiring model '{alias}'...");
                spawn_model_delete(Arc::clone(&self.service), self.event_tx.clone(), alias);
            }
            Message::RunWorkflowPressed => {
                let flag = Arc::new(AtomicBool::new(false));
                self.active_cancel_flag = Some(Arc::clone(&flag));
                let wf_id = format!("desk-wf-{}", now_epoch_ms());
                self.status_banner = format!("Executing workflow '{wf_id}'...");
                spawn_demo_workflow(
                    Arc::clone(&self.service),
                    self.event_tx.clone(),
                    wf_id,
                    flag,
                );
            }
            Message::SelectBrokerService(svc) => {
                self.broker_service_name = svc;
            }
            Message::ToggleBrokerRestart(restart) => {
                self.broker_restart_verb = restart;
            }
            Message::ExecuteBrokerPermitPressed => {
                self.status_banner = format!(
                    "Attesting single-use broker permit for '{}'...",
                    self.broker_service_name
                );
                spawn_broker_attestation(
                    self.event_tx.clone(),
                    self.broker_service_name.clone(),
                    self.broker_restart_verb,
                    format!("desk-sess-{}", now_epoch_ms()),
                );
            }
            Message::RunIncidentRecoveryPressed => {
                match self.service.execute_incident_recovery(None) {
                    Ok(rcpt) => {
                        self.status_banner = format!(
                            "Incident recovery complete: revocation_epoch={}, interrupted={}",
                            rcpt.new_revocation_epoch,
                            rcpt.interrupted_runs.len()
                        );
                        self.refresh_telemetry();
                    }
                    Err(e) => {
                        self.status_banner = format!("Recovery error: [{:?}] {e}", e.reason_code());
                    }
                }
            }
            Message::CaptureAttestedSnapshotPressed => {
                spawn_desktop_snapshot(
                    Arc::clone(&self.service),
                    self.event_tx.clone(),
                    self.active_channel.to_desktop_screen(),
                    self.experimental_enabled,
                    format!("desk-ui-{}", now_epoch_ms()),
                );
            }
            Message::PreviewConsentDialogPressed => {
                let vm = build_consent_preview(
                    self.service.data_dir(),
                    "docs/<script>probe.md".to_string(),
                );
                self.pending_consent = Some(PendingConsentPrompt {
                    request_id: 0,
                    view_model: vm,
                });
                self.status_banner =
                    "Opened local Trusted Consent preview (default focus: Deny).".to_string();
            }
            Message::SliceTranscriptStep(offset) => {
                self.transcript_offset = offset;
            }
            Message::ResolveConsent {
                request_id,
                decision,
            } => {
                self.pending_consent = None;
                if request_id > 0 {
                    if let Some(port) = &self.approval_port {
                        let _ = port.resolve(request_id, decision);
                    }
                }
                self.status_banner = format!("Consent decision submitted: {decision:?}");
            }
            Message::SelectDashboardPreset(preset) => {
                self.dashboard.apply_preset(preset);
                self.persist_dashboard_prefs();
                self.status_banner = format!("Dashboard preset active: {}", preset.label());
            }
            Message::SelectCyberTheme(preset) => {
                self.dashboard.prefs.theme_preset = preset;
                self.persist_dashboard_prefs();
                self.status_banner =
                    format!("Cyber theme: {} · {}", preset.label(), preset.origin());
            }
            Message::SelectHudDensity(density) => {
                self.dashboard.prefs.density = density;
                self.persist_dashboard_prefs();
                self.status_banner = format!("HUD density: {}", density.label());
            }
            Message::SelectHudGeometry(geometry) => {
                self.dashboard.prefs.geometry = geometry;
                self.persist_dashboard_prefs();
                self.status_banner = format!("HUD frame geometry: {}", geometry.label());
            }
            Message::ToggleCustomizeHud => {
                self.dashboard.customize_open = !self.dashboard.customize_open;
            }
            Message::ToggleNeonBorders(enabled) => {
                self.dashboard.prefs.neon_borders = enabled;
                self.persist_dashboard_prefs();
            }
            Message::ToggleSparklineGrid(enabled) => {
                self.dashboard.prefs.show_sparkline_grid = enabled;
                self.persist_dashboard_prefs();
            }
            Message::ToggleWidgetVisible(kind) => {
                self.dashboard.toggle_widget_visible(kind);
                self.persist_dashboard_prefs();
            }
            Message::ToggleWidgetCollapsed(kind) => {
                self.dashboard.toggle_widget_collapsed(kind);
                self.persist_dashboard_prefs();
            }
            Message::ToggleWidgetSpan(kind) => {
                self.dashboard.toggle_widget_span(kind);
                self.persist_dashboard_prefs();
            }
            Message::MoveWidgetUp(kind) => {
                self.dashboard.move_widget(kind, -1);
                self.persist_dashboard_prefs();
            }
            Message::MoveWidgetDown(kind) => {
                self.dashboard.move_widget(kind, 1);
                self.persist_dashboard_prefs();
            }
            Message::ResetDashboardLayout => {
                self.dashboard.reset_layout();
                self.persist_dashboard_prefs();
                self.status_banner = "Dashboard HUD & widgets reset to defaults.".to_string();
            }
            Message::QuickPromptChanged(val) => {
                self.dashboard.quick_prompt_input = val;
            }
            Message::SubmitQuickPrompt => {
                let prompt = self.dashboard.quick_prompt_input.trim().to_string();
                if !prompt.is_empty() {
                    self.dashboard.quick_prompt_input.clear();
                    self.dispatch_chat_prompt(prompt);
                }
            }
            Message::RunPromptMacro(prompt) => {
                self.dispatch_chat_prompt(prompt);
            }
            Message::MacroLabelChanged(val) => {
                self.dashboard.macro_label_input = val;
            }
            Message::MacroPromptChanged(val) => {
                self.dashboard.macro_prompt_input = val;
            }
            Message::AddPromptMacro => {
                if self.dashboard.add_custom_macro() {
                    self.persist_dashboard_prefs();
                    self.status_banner = "Saved custom command macro to dashboard.".to_string();
                } else {
                    self.status_banner =
                        "Enter both macro label and prompt text (up to 10 macros).".to_string();
                }
            }
            Message::RemovePromptMacro(idx) => {
                if self.dashboard.remove_custom_macro(idx) {
                    self.persist_dashboard_prefs();
                    self.status_banner = "Removed dashboard command macro.".to_string();
                }
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let mut columns = Row::new()
            .push(self.view_guild_rail())
            .push(self.view_channel_sidebar())
            .push(self.view_main_stage());

        if self.show_inspector {
            columns = columns.push(self.view_telemetry_inspector());
        }

        let shell: Element<'_, Message> =
            container(columns.height(Length::Fill).width(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::surface_container(pal.bg_stage))
                .into();

        if let Some(prompt) = &self.pending_consent {
            stack![shell, self.view_consent_modal(prompt)].into()
        } else {
            shell
        }
    }

    fn view_guild_rail(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let mut rail = Column::new()
            .spacing(10)
            .padding(Padding::from([12, 10]))
            .align_x(Alignment::Center);

        let brand_btn = button(
            container(
                image(&self.logo_handle)
                    .width(Length::Fixed(46.0))
                    .height(Length::Fixed(46.0)),
            )
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(48.0))
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center),
        )
        .padding(0)
        .style(theme::palette_rail_button(
            self.active_channel == ChannelTab::Dashboard,
            pal.accent_primary,
            pal.accent_secondary,
            pal.bg_elevated,
            pal.text_primary,
        ))
        .on_press(Message::SelectChannel(ChannelTab::Dashboard));
        rail = rail.push(brand_btn);
        rail = rail.push(rule::horizontal(1).style(theme::divider_rule));

        for guild in WorkspaceGuild::ALL {
            let active = self.active_guild == guild;
            let indicator =
                container(
                    space::Space::new()
                        .width(4)
                        .height(if active { 28 } else { 8 }),
                )
                .style(theme::pill_container(
                    if active {
                        pal.accent_secondary
                    } else {
                        pal.text_muted
                    },
                    pal.text_primary,
                    if active {
                        pal.accent_secondary
                    } else {
                        pal.bg_rail
                    },
                ));
            let btn = button(
                container(text(guild.badge_text()).size(14).font(Font::MONOSPACE))
                    .width(Length::Fixed(46.0))
                    .height(Length::Fixed(46.0))
                    .align_x(Horizontal::Center)
                    .align_y(Vertical::Center),
            )
            .padding(0)
            .style(theme::palette_rail_button(
                active,
                pal.accent_primary,
                pal.accent_secondary,
                pal.bg_elevated,
                pal.text_primary,
            ))
            .on_press(Message::SelectGuild(guild));

            rail = rail.push(row![indicator, btn].spacing(6).align_y(Alignment::Center));
        }

        rail = rail.push(rule::horizontal(1).style(theme::divider_rule));
        rail = rail.push(space::vertical());

        let stop_btn = button(
            container(text("STOP").size(11))
                .width(Length::Fixed(48.0))
                .height(Length::Fixed(34.0))
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center),
        )
        .padding(0)
        .style(theme::solid_button_with_radius(
            pal.critical,
            RED_HOVER,
            TEXT_PRIMARY,
            self.dashboard.control_radius(),
        ))
        .on_press(Message::StopActiveTask);

        rail = rail.push(stop_btn);

        container(rail)
            .width(Length::Fixed(74.0))
            .height(Length::Fill)
            .style(theme::surface_container(pal.bg_rail))
            .into()
    }

    fn view_channel_sidebar(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let header = container(
            row![
                image(&self.logo_handle)
                    .width(Length::Fixed(24.0))
                    .height(Length::Fixed(24.0)),
                column![
                    text("Lora Agent").size(16).color(pal.text_primary),
                    text(format!(
                        "{} · {}",
                        self.active_guild.title(),
                        self.dashboard.prefs.theme_preset.code()
                    ))
                    .size(11)
                    .color(pal.text_muted),
                ]
                .spacing(2),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([12, 14]))
        .width(Length::Fill);

        let mut list = Column::new().spacing(4).padding(Padding::from([8, 8]));
        list = list.push(text("CORE CHANNELS").size(11).color(pal.text_muted));
        for ch in ChannelTab::CORE_CHANNELS {
            list = list.push(self.channel_row_btn(ch));
        }

        list = list.push(space::Space::new().height(10));
        list = list.push(text("TRUST & GOVERNANCE").size(11).color(pal.text_muted));
        for ch in ChannelTab::SECURITY_CHANNELS {
            list = list.push(self.channel_row_btn(ch));
        }

        if self.experimental_enabled {
            list = list.push(space::Space::new().height(10));
            list = list.push(text("EXPERIMENTAL LAB").size(11).color(pal.warning));
            for ch in ChannelTab::EXPERIMENTAL_CHANNELS {
                list = list.push(self.channel_row_btn(ch));
            }
        }

        let model_label = self.selected_model.as_deref().unwrap_or("no-model-loaded");
        let busy =
            self.active_run_id.is_some() || self.telemetry.dashboard.status_strip.active_task_busy;
        let dock_dot_color = if busy { pal.warning } else { pal.nominal };

        let user_dock = container(
            row![
                container(text(if busy { "RUN" } else { "OK" }).size(10))
                    .padding(Padding::from([3, 6]))
                    .style(theme::pill_container(
                        if busy { AMBER_SOFT } else { pal.nominal_soft },
                        dock_dot_color,
                        dock_dot_color,
                    )),
                column![
                    text(model_label).size(12).color(pal.text_primary),
                    text(
                        self.telemetry
                            .dashboard
                            .status_strip
                            .execution_destination
                            .as_str()
                    )
                    .size(11)
                    .color(pal.text_muted),
                ]
                .spacing(1)
                .width(Length::Fill),
                button(text("Sync").size(11))
                    .padding(Padding::from([4, 8]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_elevated,
                        pal.accent_primary,
                        pal.text_primary,
                        self.dashboard.control_radius(),
                    ))
                    .on_press(Message::RefreshTelemetry),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(10)
        .width(Length::Fill)
        .style(theme::surface_container(pal.bg_dock));

        container(
            column![
                header,
                rule::horizontal(1).style(theme::divider_rule),
                scrollable(list).height(Length::Fill),
                rule::horizontal(1).style(theme::divider_rule),
                user_dock,
            ]
            .height(Length::Fill),
        )
        .width(Length::Fixed(236.0))
        .height(Length::Fill)
        .style(theme::surface_container(pal.bg_sidebar))
        .into()
    }

    fn channel_row_btn(&self, ch: ChannelTab) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let active = self.active_channel == ch;
        button(
            row![
                text("#").size(14).color(if active {
                    pal.accent_secondary
                } else {
                    pal.text_muted
                }),
                text(ch.slug()).size(13),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([7, 10]))
        .width(Length::Fill)
        .style(theme::palette_channel_button(
            active,
            pal.bg_elevated,
            pal.bg_card,
            pal.border_accent,
            pal.text_primary,
            pal.text_secondary,
            pal.text_muted,
        ))
        .on_press(Message::SelectChannel(ch))
        .into()
    }

    fn view_top_header(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let model_pill =
            container(text(self.selected_model.as_deref().unwrap_or("No Active Model")).size(11))
                .padding(Padding::from([4, 10]))
                .style(theme::pill_container(
                    pal.accent_secondary_soft,
                    pal.accent_secondary,
                    pal.border_accent,
                ));

        let airgap_pill = container(text("AIR-GAPPED").size(11))
            .padding(Padding::from([4, 9]))
            .style(theme::pill_container(
                pal.nominal_soft,
                pal.nominal,
                pal.nominal,
            ));

        let stop_btn = button(
            text(if self.active_run_id.is_some() {
                "STOP RUN"
            } else {
                "Stop Ready"
            })
            .size(12),
        )
        .padding(Padding::from([5, 11]))
        .style(if self.active_run_id.is_some() {
            theme::solid_button_with_radius(
                pal.critical,
                RED_HOVER,
                TEXT_PRIMARY,
                self.dashboard.control_radius(),
            )
        } else {
            theme::solid_button_with_radius(
                pal.bg_elevated,
                pal.critical,
                pal.text_secondary,
                self.dashboard.control_radius(),
            )
        })
        .on_press(Message::StopActiveTask);

        let inspector_btn = button(
            text(if self.show_inspector {
                "Hide Radar"
            } else {
                "Show Radar"
            })
            .size(12),
        )
        .padding(Padding::from([5, 10]))
        .style(theme::solid_button_with_radius(
            pal.bg_elevated,
            pal.accent_primary,
            pal.text_primary,
            self.dashboard.control_radius(),
        ))
        .on_press(Message::ToggleInspector);

        container(
            row![
                text("#").size(18).color(pal.accent_secondary),
                column![
                    text(self.active_channel.slug())
                        .size(15)
                        .color(pal.text_primary),
                    text(self.active_channel.topic())
                        .size(11)
                        .color(pal.text_muted),
                ]
                .spacing(1)
                .width(Length::Fill),
                airgap_pill,
                model_pill,
                stop_btn,
                inspector_btn,
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([8, 16]))
        .width(Length::Fill)
        .style(theme::surface_container(pal.bg_stage))
        .into()
    }

    fn view_main_stage(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let body: Element<'_, Message> = match self.active_channel {
            ChannelTab::Dashboard => self.view_dashboard_channel(),
            ChannelTab::Chat => self.view_chat_channel(),
            ChannelTab::AgentWorkspace => self.view_workspace_channel(),
            ChannelTab::ModelLibrary => self.view_models_channel(),
            ChannelTab::Hardware => self.view_hardware_channel(),
            ChannelTab::WorkflowPipeline => self.view_workflows_channel(),
            ChannelTab::AuditJournal => self.view_audit_channel(),
            ChannelTab::BrokerGate => self.view_broker_channel(),
            ChannelTab::AdapterStudio => self.view_adapters_channel(),
            ChannelTab::ExtensionManager => self.view_extensions_channel(),
            ChannelTab::Settings => self.view_settings_channel(),
        };

        let footer = container(
            row![
                text(self.status_banner.as_str())
                    .size(11)
                    .color(pal.text_muted),
                space::horizontal(),
                text(format!(
                    "{} · lora.desktop.native.v1",
                    self.dashboard.prefs.theme_preset.code()
                ))
                .size(11)
                .color(pal.text_muted),
            ]
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([6, 16]))
        .width(Length::Fill)
        .style(theme::surface_container(pal.bg_dock));

        container(
            column![
                self.view_top_header(),
                rule::horizontal(1).style(theme::divider_rule),
                container(body).width(Length::Fill).height(Length::Fill),
                rule::horizontal(1).style(theme::divider_rule),
                footer,
            ]
            .height(Length::Fill)
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::surface_container(pal.bg_stage))
        .into()
    }

    fn view_dashboard_channel(&self) -> Element<'_, Message> {
        dashboard::view_dashboard(self)
    }

    fn view_chat_channel(&self) -> Element<'_, Message> {
        let mut model_chips = Row::new().spacing(6).align_y(Alignment::Center);
        model_chips = model_chips.push(text("Model:").size(12).color(TEXT_MUTED));
        if self.telemetry.models.is_empty() {
            model_chips = model_chips.push(
                button(text("Import GGUF ->").size(11))
                    .padding(Padding::from([4, 10]))
                    .style(theme::solid_button(AMBER_SOFT, BLURPLE, AMBER))
                    .on_press(Message::SelectChannel(ChannelTab::ModelLibrary)),
            );
        } else {
            for m in self.telemetry.models.iter().take(5) {
                let is_sel = self.selected_model.as_deref() == Some(m.display_alias.as_str());
                model_chips = model_chips.push(
                    button(text(&m.display_alias).size(11))
                        .padding(Padding::from([4, 10]))
                        .style(theme::solid_button(
                            if is_sel { BLURPLE } else { BG_SELECTED },
                            BLURPLE_HOVER,
                            TEXT_PRIMARY,
                        ))
                        .on_press(Message::SelectModel(m.display_alias.clone())),
                );
            }
        }

        let mode_bar = container(
            row![
                model_chips,
                space::horizontal(),
                checkbox(self.read_only_assistant)
                    .label("Read-Only Assistant (Consent Gated)")
                    .size(16)
                    .text_size(12)
                    .on_toggle(Message::ToggleAssistantMode),
                checkbox(self.use_rag)
                    .label("FTS5 RAG")
                    .size(16)
                    .text_size(12)
                    .on_toggle(Message::ToggleRag),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([8, 16]))
        .width(Length::Fill)
        .style(theme::surface_container(BG_CARD));

        let md_style = theme::chat_markdown_style();
        let md_settings = markdown::Settings::with_text_size(14.0, md_style);

        let mut feed = Column::new().spacing(12).padding(Padding::from([14, 18]));
        for entry in &self.messages {
            let badge_color = if entry.is_user { BLURPLE } else { LORA_MAGENTA };
            let mut header_row = row![
                text(&entry.author).size(13).color(badge_color),
                container(text(&entry.badge).size(10))
                    .padding(Padding::from([2, 6]))
                    .style(theme::pill_container(BG_RAIL, badge_color, BORDER_SUBTLE,)),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            if entry.is_streaming {
                header_row = header_row.push(text("streaming...").size(11).color(AMBER));
            } else if entry.outcome_verified {
                header_row = header_row.push(text("SHA-256 Verified").size(11).color(GREEN));
            }

            let body: Element<'_, Message> = if entry.raw_text.is_empty() && entry.is_streaming {
                text("Thinking in isolated worker...")
                    .size(13)
                    .color(TEXT_MUTED)
                    .into()
            } else {
                markdown::view(entry.markdown.items(), md_settings)
                    .map(Message::MarkdownLinkIgnored)
            };

            let mut msg_col = column![header_row, body].spacing(6);

            if !entry.citations.is_empty() || entry.outcome_sha256.is_some() {
                let mut meta = Row::new().spacing(8);
                if !entry.citations.is_empty() {
                    meta = meta.push(
                        text(format!("Citations: {}", entry.citations.join(", ")))
                            .size(11)
                            .color(GREEN),
                    );
                }
                if let Some(sha) = &entry.outcome_sha256 {
                    let short: String = sha.chars().take(16).collect();
                    meta = meta.push(
                        text(format!(
                            "digest:{short}... · {} receipts",
                            entry.receipt_count
                        ))
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(TEXT_MUTED),
                    );
                }
                msg_col = msg_col.push(meta);
            }

            feed = feed.push(container(msg_col).padding(12).width(Length::Fill).style(
                theme::card_container(
                    if entry.is_user { BG_SIDEBAR } else { BG_CARD },
                    if entry.outcome_verified {
                        GREEN
                    } else {
                        BORDER_SUBTLE
                    },
                    8.0,
                ),
            ));
        }

        let composer = container(
            row![
                text_input(
                    "Message #chat (Enter to send to local isolated worker)...",
                    &self.composer_input,
                )
                .on_input(Message::ChatComposerChanged)
                .on_submit(Message::SendChatMessage)
                .padding(Padding::from([10, 14]))
                .style(theme::composer_input_style)
                .width(Length::Fill),
                button(text("Send").size(13))
                    .padding(Padding::from([10, 18]))
                    .style(theme::solid_button(BLURPLE, BLURPLE_HOVER, TEXT_PRIMARY))
                    .on_press(Message::SendChatMessage),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([10, 16]))
        .width(Length::Fill)
        .style(theme::surface_container(BG_STAGE));

        column![
            mode_bar,
            rule::horizontal(1).style(theme::divider_rule),
            scrollable(feed).height(Length::Fill),
            composer,
        ]
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
    }

    fn view_models_channel(&self) -> Element<'_, Message> {
        let import_box = container(
            column![
                text("Import Local GGUF Artifact").size(14),
                row![
                    text_input("Path to verified .gguf file...", &self.import_path_input)
                        .on_input(Message::ImportPathChanged)
                        .padding(Padding::from([8, 12]))
                        .style(theme::composer_input_style)
                        .width(Length::Fill),
                    text_input("Display alias (optional)", &self.import_alias_input)
                        .on_input(Message::ImportAliasChanged)
                        .padding(Padding::from([8, 12]))
                        .style(theme::composer_input_style)
                        .width(Length::Fixed(200.0)),
                    button(text("Import GGUF").size(12))
                        .padding(Padding::from([8, 16]))
                        .style(theme::solid_button(BLURPLE, BLURPLE_HOVER, TEXT_PRIMARY))
                        .on_press(Message::ImportModelPressed),
                ]
                .spacing(8),
            ]
            .spacing(8),
        )
        .padding(14)
        .width(Length::Fill)
        .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0));

        let mut list = Column::new().spacing(8);
        if self.telemetry.models.is_empty() {
            list = list.push(
                container(
                    text("No content-addressed GGUF models registered yet in this data directory.")
                        .size(12)
                        .color(TEXT_MUTED),
                )
                .padding(14)
                .width(Length::Fill)
                .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0)),
            );
        } else {
            for m in &self.telemetry.models {
                let sha = m
                    .files
                    .first()
                    .map(|f| f.sha256.chars().take(18).collect::<String>())
                    .unwrap_or_default();
                let size_mib = m.files.iter().map(|f| f.expected_size).sum::<u64>() / (1024 * 1024);
                let alias = m.display_alias.clone();
                list = list.push(
                    container(
                        row![
                            column![
                                text(&m.display_alias).size(14),
                                text(format!(
                                    "arch={} · quant={} · {} MiB · tools={} · sha256:{sha}...",
                                    m.probed.architecture,
                                    m.probed.quantization,
                                    size_mib,
                                    m.probed.supports_tool_calling
                                ))
                                .size(11)
                                .font(Font::MONOSPACE)
                                .color(TEXT_MUTED),
                            ]
                            .spacing(3)
                            .width(Length::Fill),
                            button(text("Use").size(11))
                                .padding(Padding::from([6, 12]))
                                .style(theme::solid_button(BLURPLE, BLURPLE_HOVER, TEXT_PRIMARY))
                                .on_press(Message::SelectModel(alias.clone())),
                            button(text("Retire").size(11))
                                .padding(Padding::from([6, 12]))
                                .style(theme::solid_button(RED_SOFT, RED, TEXT_PRIMARY))
                                .on_press(Message::DeleteModelPressed(alias)),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding(12)
                    .width(Length::Fill)
                    .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0)),
                );
            }
        }

        scrollable(column![import_box, list].spacing(12).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_hardware_channel(&self) -> Element<'_, Message> {
        let host_card = if let Some(h) = &self.telemetry.host {
            let total_mib = (h.total_ram_bytes / (1024 * 1024)).max(1) as f32;
            let avail_mib = (h.available_ram_bytes / (1024 * 1024)) as f32;
            let used_pct = ((total_mib - avail_mib) / total_mib * 100.0).clamp(0.0, 100.0);

            container(
                column![
                    text(format!("{} · {} ({})", h.cpu_brand, h.os_name, h.cpu_arch)).size(15),
                    text(format!(
                        "{} physical / {} logical cores · Free disk: {} GiB · Power: {:?}",
                        h.physical_cores,
                        h.logical_cores,
                        h.free_disk_bytes / (1024 * 1024 * 1024),
                        h.charging_state
                    ))
                    .size(12)
                    .color(TEXT_MUTED),
                    progress_bar(0.0..=100.0, used_pct),
                    text(format!(
                        "ISA: {}",
                        if h.cpu_isa_features.is_empty() {
                            "baseline".to_string()
                        } else {
                            h.cpu_isa_features.join(", ")
                        }
                    ))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
                ]
                .spacing(8),
            )
            .padding(16)
            .width(Length::Fill)
            .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0))
        } else {
            container(text("Hardware telemetry unavailable").size(12))
                .padding(16)
                .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0))
        };

        let profiles = [
            TuningProfile::Automatic,
            TuningProfile::Balanced,
            TuningProfile::MaximumPerformance,
            TuningProfile::LowMemory,
            TuningProfile::BatterySaver,
        ];
        let mut prof_row = Row::new().spacing(8);
        for p in profiles {
            let active = self.tuning_profile == p;
            prof_row = prof_row.push(
                button(text(p.as_str()).size(11))
                    .padding(Padding::from([6, 12]))
                    .style(theme::solid_button(
                        if active { LORA_MAGENTA } else { BG_SELECTED },
                        BLURPLE,
                        TEXT_PRIMARY,
                    ))
                    .on_press(Message::SelectTuningProfile(p)),
            );
        }

        let mut ctx_row = Row::new().spacing(8);
        for ctx in [1024_u32, 2048, 4096, 8192] {
            let active = self.requested_context == ctx;
            ctx_row = ctx_row.push(
                button(text(format!("{ctx} tok")).size(11))
                    .padding(Padding::from([6, 12]))
                    .style(theme::solid_button(
                        if active { BLURPLE } else { BG_SELECTED },
                        BLURPLE_HOVER,
                        TEXT_PRIMARY,
                    ))
                    .on_press(Message::ContextTokensChanged(ctx)),
            );
        }

        let budget_summary = match allocate_context_budget(&ContextBudgetRequest {
            context_window_tokens: self.requested_context,
            reserved_output_tokens: 512,
            control_schema_overhead_tokens: 128,
            user_request_tokens: 256,
            recent_turns_tokens: 128,
            tool_receipts_tokens: 64,
            ranked_excerpts: vec![],
        }) {
            Ok(alloc) => format!(
                "Context Budget Ready: window={} · allocated={} · headroom={} · reserved_out={}",
                alloc.context_window_tokens,
                alloc.total_allocated_tokens,
                alloc.remaining_headroom_tokens,
                alloc.reserved_output_tokens
            ),
            Err(e) => format!("Budget Error: {e}"),
        };

        let controls_card = container(
            column![
                text("HIE Profile & Context Window Allocation").size(14),
                prof_row,
                ctx_row,
                text(budget_summary)
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(GREEN),
            ]
            .spacing(10),
        )
        .padding(16)
        .width(Length::Fill)
        .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0));

        scrollable(column![host_card, controls_card].spacing(14).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_workflows_channel(&self) -> Element<'_, Message> {
        let (checked, valid) = self.telemetry.workflow_integrity;
        let top = container(
            row![
                column![
                    text("Deterministic Workflow Pipeline").size(14),
                    text(format!(
                        "Stored receipt integrity: {valid}/{checked} verified (chain_intact={})",
                        checked == valid
                    ))
                    .size(12)
                    .color(if checked == valid { GREEN } else { RED }),
                ]
                .spacing(3)
                .width(Length::Fill),
                button(text("Execute Verification Pipeline").size(12))
                    .padding(Padding::from([8, 16]))
                    .style(theme::solid_button(BLURPLE, BLURPLE_HOVER, TEXT_PRIMARY))
                    .on_press(Message::RunWorkflowPressed),
            ]
            .align_y(Alignment::Center),
        )
        .padding(14)
        .width(Length::Fill)
        .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0));

        let mut list = Column::new().spacing(8);
        for r in &self.telemetry.workflow_receipts {
            let short_sha: String = r.receipt_sha256.chars().take(18).collect();
            list = list.push(
                container(
                    column![
                        text(format!(
                            "{} · steps {}/{} · {} ms · ok={}",
                            r.workflow_id,
                            r.completed_steps,
                            r.total_steps,
                            r.total_duration_ms,
                            r.successful
                        ))
                        .size(13),
                        text(format!(
                            "receipt_sha256:{short_sha}... · deadline_exceeded={}",
                            r.deadline_exceeded
                        ))
                        .size(11)
                        .font(Font::MONOSPACE)
                        .color(TEXT_MUTED),
                    ]
                    .spacing(4),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0)),
            );
        }

        scrollable(column![top, list].spacing(12).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_audit_channel(&self) -> Element<'_, Message> {
        let bar = container(
            row![
                column![
                    text("Redacted SQLite Audit Journal & Incident Recovery").size(14),
                    text("Excerpts are excluded; revoking revokes all session workspace grants immediately.")
                        .size(11)
                        .color(TEXT_MUTED),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("Revoke All Grants").size(12))
                    .padding(Padding::from([8, 14]))
                    .style(theme::solid_button(RED, RED_HOVER, TEXT_PRIMARY))
                    .on_press(Message::RunIncidentRecoveryPressed),
            ]
            .align_y(Alignment::Center),
        )
        .padding(14)
        .width(Length::Fill)
        .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0));

        let mut rows = Column::new().spacing(6);
        for (seq, outcome, resource, summary) in &self.telemetry.recent_audits {
            rows = rows.push(
                container(
                    column![
                        text(format!("#{seq} · {outcome} · {resource}"))
                            .size(12)
                            .color(TEXT_PRIMARY),
                        text(summary).size(11).color(TEXT_MUTED),
                    ]
                    .spacing(2),
                )
                .padding(10)
                .width(Length::Fill)
                .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 6.0)),
            );
        }

        scrollable(column![bar, rows].spacing(12).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_broker_channel(&self) -> Element<'_, Message> {
        let services = ["lora-local-service", "lora-worker", "lora-daemon"];
        let mut svc_btns = Row::new().spacing(8);
        for s in services {
            let active = self.broker_service_name == s;
            svc_btns = svc_btns.push(
                button(text(s).size(12))
                    .padding(Padding::from([6, 12]))
                    .style(theme::solid_button(
                        if active { LORA_MAGENTA } else { BG_SELECTED },
                        BLURPLE,
                        TEXT_PRIMARY,
                    ))
                    .on_press(Message::SelectBrokerService(s.to_string())),
            );
        }

        let gate_card = container(
            column![
                text("Privileged Broker Single-Use Permit & Receipt Attestation (R09)").size(14),
                text("Domains: lora.broker.permit.v1 & lora.broker.receipt.v1 · Replay-protected one-shot nonces.")
                    .size(11)
                    .color(TEXT_MUTED),
                svc_btns,
                row![
                    checkbox(self.broker_restart_verb)
                        .label("Use RestartAllowlistedService verb (instead of QueryServiceStatus)")
                        .size(16)
                        .text_size(12)
                        .on_toggle(Message::ToggleBrokerRestart),
                    space::horizontal(),
                    button(text("Execute & Attest Permit").size(12))
                        .padding(Padding::from([8, 16]))
                        .style(theme::solid_button(BLURPLE, BLURPLE_HOVER, TEXT_PRIMARY))
                        .on_press(Message::ExecuteBrokerPermitPressed),
                ]
                .align_y(Alignment::Center),
            ]
            .spacing(12),
        )
        .padding(16)
        .width(Length::Fill)
        .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0));

        let receipt_card = if let Some((rcpt, verified)) = &self.latest_broker_receipt {
            container(
                column![
                    text(format!("BrokerExecutionReceipt · verified={verified}"))
                        .size(13)
                        .color(if *verified { GREEN } else { RED }),
                    text(format!(
                        "receipt_id: {}\npermit_id: {}\nnonce: {}\nstatus: {}\nrequest_digest: {}\nreceipt_sha256: {}",
                        rcpt.receipt_id,
                        rcpt.permit_id,
                        rcpt.challenge_nonce,
                        rcpt.status.as_str(),
                        rcpt.request_digest,
                        rcpt.receipt_sha256.as_deref().unwrap_or("none")
                    ))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_SECONDARY),
                ]
                .spacing(6),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::card_container(BG_CARD, GREEN, 8.0))
        } else {
            container(
                text("No broker receipt attested in this UI session yet.")
                    .size(12)
                    .color(TEXT_MUTED),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0))
        };

        scrollable(column![gate_card, receipt_card].spacing(12).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_workspace_channel(&self) -> Element<'_, Message> {
        let items: Vec<String> = (0..120)
            .map(|i| format!("transcript-entry-{i:04}"))
            .collect();
        let slice = slice_virtualized_transcript(&items, self.transcript_offset, 10);

        let scope_card = container(
            column![
                text("Approved Read-Only Workspace Scope").size(14),
                text_input("Workspace root directory...", &self.workspace_root_input)
                    .on_input(Message::WorkspacePathChanged)
                    .padding(Padding::from([8, 12]))
                    .style(theme::composer_input_style)
                    .width(Length::Fill),
                button(text("Test Trusted Consent Modal (Default Deny)").size(12))
                    .padding(Padding::from([8, 14]))
                    .style(theme::solid_button(AMBER_SOFT, BLURPLE, AMBER))
                    .on_press(Message::PreviewConsentDialogPressed),
            ]
            .spacing(10),
        )
        .padding(16)
        .width(Length::Fill)
        .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0));

        let slice_card = container(
            column![
                row![
                    text(format!(
                        "Virtualized Transcript Window (R12): {}..{} of {}",
                        slice.start_index, slice.end_index, slice.total_items
                    ))
                    .size(13),
                    space::horizontal(),
                    button(text("Prev").size(11))
                        .padding(Padding::from([4, 10]))
                        .style(theme::solid_button(BG_SELECTED, BLURPLE, TEXT_PRIMARY))
                        .on_press(Message::SliceTranscriptStep(
                            self.transcript_offset.saturating_sub(10)
                        )),
                    button(text("Next").size(11))
                        .padding(Padding::from([4, 10]))
                        .style(theme::solid_button(BG_SELECTED, BLURPLE, TEXT_PRIMARY))
                        .on_press(Message::SliceTranscriptStep(
                            (self.transcript_offset + 10).min(110)
                        )),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                text(slice.visible_items.join("  ·  "))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
            ]
            .spacing(8),
        )
        .padding(16)
        .width(Length::Fill)
        .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0));

        scrollable(column![scope_card, slice_card].spacing(12).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_adapters_channel(&self) -> Element<'_, Message> {
        let matrix = default_risk_register_matrix();
        let mut cards = Column::new().spacing(8);
        for r in matrix {
            cards = cards.push(
                container(
                    row![
                        text(r.risk_id)
                            .size(12)
                            .font(Font::MONOSPACE)
                            .color(LORA_MAGENTA),
                        column![
                            text(r.mitigation_control).size(12),
                            text(r.fallback_behavior).size(11).color(TEXT_MUTED),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        text(if r.verified { "VERIFIED" } else { "OPEN" })
                            .size(11)
                            .color(if r.verified { GREEN } else { AMBER }),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(10)
                .width(Length::Fill)
                .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 6.0)),
            );
        }

        scrollable(
            column![
                text("Experimental Adapter Studio & Risk Qualification Register (R01..R18)")
                    .size(14),
                cards,
            ]
            .spacing(12)
            .padding(18),
        )
        .height(Length::Fill)
        .into()
    }

    fn view_extensions_channel(&self) -> Element<'_, Message> {
        let host_summary = format!(
            "Registered sandboxed extensions in store: {}",
            self.telemetry.extension_count
        );
        scrollable(
            column![container(
                column![
                    text("Extension Host Manager (MCP / Wasm Sandboxed)").size(14),
                    text(host_summary).size(12).color(TEXT_MUTED),
                ]
                .spacing(6),
            )
            .padding(16)
            .width(Length::Fill)
            .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0)),]
            .padding(18),
        )
        .height(Length::Fill)
        .into()
    }

    fn view_settings_channel(&self) -> Element<'_, Message> {
        let card = container(
            column![
                text("Desktop Client Settings & Attestation").size(14),
                checkbox(self.experimental_enabled)
                    .label("Enable experimental lab channels (#adapter-studio, #extension-host)")
                    .size(16)
                    .text_size(12)
                    .on_toggle(Message::ToggleExperimental),
                row![
                    button(text("Capture & Verify DesktopSnapshot").size(12))
                        .padding(Padding::from([8, 14]))
                        .style(theme::solid_button(BLURPLE, BLURPLE_HOVER, TEXT_PRIMARY))
                        .on_press(Message::CaptureAttestedSnapshotPressed),
                    button(text("Preview Trusted Consent Modal").size(12))
                        .padding(Padding::from([8, 14]))
                        .style(theme::solid_button(BG_SELECTED, AMBER, TEXT_PRIMARY))
                        .on_press(Message::PreviewConsentDialogPressed),
                ]
                .spacing(10),
                rule::horizontal(1).style(theme::divider_rule),
                text("Engine Build: Lora Agent Native Rust Desktop (iced 0.14) · NC-EDGERUNNER-02::CAPRI")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(TEXT_MUTED),
            ]
            .spacing(12),
        )
        .padding(16)
        .width(Length::Fill)
        .style(theme::card_container(BG_CARD, BORDER_SUBTLE, 8.0));

        scrollable(column![card].padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_telemetry_inspector(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let radius = self.dashboard.card_radius();
        let vm = &self.telemetry.dashboard;
        let mut audits_col = Column::new().spacing(6);
        for (seq, outcome, resource, _) in self.telemetry.recent_audits.iter().take(6) {
            audits_col = audits_col.push(
                container(
                    column![
                        text(format!("#{seq} {outcome}"))
                            .size(11)
                            .color(pal.text_primary),
                        text(resource).size(10).color(pal.text_muted),
                    ]
                    .spacing(1),
                )
                .padding(8)
                .width(Length::Fill)
                .style(theme::card_container(
                    pal.bg_card,
                    pal.border_subtle,
                    radius.min(6.0),
                )),
            );
        }

        let panel = column![
            text("LIVE TELEMETRY RADAR").size(11).color(pal.text_muted),
            container(
                column![
                    text("Host Memory").size(12).color(pal.text_primary),
                    text(format!(
                        "{} / {} MiB",
                        vm.available_ram_mib, vm.total_ram_mib
                    ))
                    .size(13)
                    .color(pal.nominal),
                    text(&vm.cpu_summary).size(11).color(pal.text_muted),
                ]
                .spacing(4),
            )
            .padding(10)
            .width(Length::Fill)
            .style(theme::card_container(
                pal.bg_card,
                pal.border_subtle,
                radius
            )),
            text("RECENT AUDIT TRAIL").size(11).color(pal.text_muted),
            scrollable(audits_col).height(Length::Fill),
        ]
        .spacing(10)
        .padding(12)
        .height(Length::Fill);

        container(panel)
            .width(Length::Fixed(268.0))
            .height(Length::Fill)
            .style(theme::surface_container(pal.bg_sidebar))
            .into()
    }

    fn view_consent_modal<'a>(&'a self, prompt: &'a PendingConsentPrompt) -> Element<'a, Message> {
        let vm = &prompt.view_model;
        let card = container(
            column![
                text("TRUSTED CONSENT REQUIRED").size(12).color(AMBER),
                text("Authorize Bounded Local File Access").size(17),
                text(
                    "Untrusted file content is never rendered inside this trusted approval prompt."
                )
                .size(12)
                .color(TEXT_MUTED),
                container(
                    column![
                        Self::kv_row("Tool Identity", &vm.tool_id),
                        Self::kv_row("Run ID", &vm.run_id),
                        Self::kv_row("Scope Root", &vm.scope_root_id),
                        Self::kv_row("Relative Target", &vm.target_relative_path),
                        Self::kv_row("Read Byte Limit", format!("{} bytes", vm.read_byte_limit)),
                        Self::kv_row("Destination", &vm.destination_label),
                        Self::kv_row("Grant Lifetime", &vm.grant_lifetime_label),
                        Self::kv_row("Action Digest", &vm.action_digest),
                        Self::kv_row("Default Focus", &vm.default_button_focus),
                    ]
                    .spacing(5),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::card_container(BG_COMPOSER, BORDER_SUBTLE, 8.0)),
                row![
                    // Least-privilege Deny button sits primary & prominent by design (`docs/DESKTOP_UX.md`).
                    button(text("Deny (Default Safe)").size(13))
                        .padding(Padding::from([10, 18]))
                        .style(theme::solid_button(RED, RED_HOVER, TEXT_PRIMARY))
                        .on_press(Message::ResolveConsent {
                            request_id: prompt.request_id,
                            decision: ApprovalDecision::Deny,
                        }),
                    space::horizontal(),
                    button(text("Approve Once").size(12))
                        .padding(Padding::from([10, 14]))
                        .style(theme::solid_button(BG_SELECTED, BLURPLE, TEXT_PRIMARY))
                        .on_press(Message::ResolveConsent {
                            request_id: prompt.request_id,
                            decision: ApprovalDecision::ApproveOnce,
                        }),
                    button(text("Approve Session").size(12))
                        .padding(Padding::from([10, 14]))
                        .style(theme::solid_button(BG_SELECTED, GREEN, TEXT_PRIMARY))
                        .on_press(Message::ResolveConsent {
                            request_id: prompt.request_id,
                            decision: ApprovalDecision::ApproveSessionScope,
                        }),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            ]
            .spacing(12),
        )
        .padding(20)
        .width(Length::Fixed(580.0))
        .style(theme::modal_card_style);

        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .style(theme::modal_backdrop_style)
            .into()
    }

    fn kv_row<'a>(label: &'a str, val: impl Into<String>) -> Element<'a, Message> {
        row![
            text(label)
                .size(11)
                .color(TEXT_MUTED)
                .width(Length::Fixed(128.0)),
            text(val.into())
                .size(11)
                .font(Font::MONOSPACE)
                .color(TEXT_PRIMARY),
        ]
        .spacing(8)
        .into()
    }
}
