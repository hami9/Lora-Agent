//! Pure-Rust native multi-pane desktop client (`LoraDesktopApp`) built on `iced 0.14.0`.

use crate::agent_views;
use crate::backend::{
    collect_core_telemetry, spawn_api_chat_run, spawn_broker_attestation, spawn_chat_run,
    spawn_demo_workflow, spawn_desktop_snapshot, spawn_model_delete, spawn_model_import,
    AppWorkerEvent, CoreTelemetry, GuiApprovalPromptPort,
};
use crate::build_consent_preview;
use crate::dashboard::{
    self, CapriAuraPreset, CapriEyeMode, DashboardPreferences, DashboardPreset, DashboardState,
    DashboardWidgetKind, HudDensity,
};
use crate::theme::{self, CyberPalette, CyberThemePreset, HudGeometry, SurfaceDepth};
use crate::ux_studio::{
    self, AgentPhase, MorphingPhaseCanvas, SkeletonShimmerCanvas, StagedAttachment, CHAT_SCROLL_ID,
    CHAT_SEARCH_INPUT_ID, COMMAND_PALETTE_INPUT_ID, OPERATOR_TEMPLATES,
};
use crate::workspace_db::{
    AgentKind, AgentPersona, AgentTask, ApiEndpointConfig, EnvVarEntry, ExecutionAccessMode,
    FolderAccessLevel, KvCacheType, LlamaRuntimeConfig, PersistedChatRow, ReasoningEffort,
    TaskFilter, TaskKind, TaskPriority, TaskStatus, TrustedFolderEntry, WorkspaceDb,
    WorkspaceProject,
};
use iced::alignment::{Horizontal, Vertical};
use iced::clipboard;
use iced::keyboard;
use iced::mouse;
use iced::widget::{
    button, canvas, checkbox, column, container, image, markdown, operation, row, rule, scrollable,
    space, stack, text, text_input, Column, Row,
};
use iced::window;
use iced::{
    Alignment, Color, Element, Event, Font, Length, Padding, Point, Rectangle, Renderer,
    Subscription, Task, Theme,
};
use lora_core::{
    default_risk_register_matrix, now_epoch_ms, ApprovalDecision, BrokerExecutionReceipt,
    DesktopScreen, DesktopSnapshot, ExecutionMode, LoraApplicationService,
    TrustedConsentDialogViewModel, TuningProfile,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Left workspace rail badges for the native multi-pane client.
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

/// Pure-Rust geometric vector glyph rendered inside the far-left workspace rail.
struct GuildIconCanvas {
    guild: WorkspaceGuild,
    color: Color,
    active: bool,
    phase: f32,
    motion_enabled: bool,
}

impl canvas::Program<Message> for GuildIconCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let cx = bounds.width * 0.5;
        let cy = bounds.height * 0.5;
        let pulse = if self.active && self.motion_enabled {
            1.0 + 0.08 * (self.phase * 2.2).sin()
        } else {
            1.0
        };
        let r = bounds.width.min(bounds.height) * 0.36 * pulse;

        if self.active && self.motion_enabled {
            let rot = self.phase * 0.7;
            for q in 0..2 {
                let start = rot + q as f32 * std::f32::consts::PI;
                let reticle = canvas::Path::new(|b| {
                    b.arc(canvas::path::Arc {
                        center: Point::new(cx, cy),
                        radius: bounds.width.min(bounds.height) * 0.46,
                        start_angle: iced::Radians(start),
                        end_angle: iced::Radians(start + 0.9),
                    });
                });
                frame.stroke(
                    &reticle,
                    canvas::Stroke::default()
                        .with_color(Color {
                            a: 0.45,
                            ..self.color
                        })
                        .with_width(1.1)
                        .with_line_cap(canvas::LineCap::Round),
                );
            }
        }

        let stroke = canvas::Stroke::default()
            .with_color(self.color)
            .with_width(1.65)
            .with_line_cap(canvas::LineCap::Round)
            .with_line_join(canvas::LineJoin::Round);

        match self.guild {
            WorkspaceGuild::LocalCore => {
                let hex = canvas::Path::new(|b| {
                    for i in 0..6 {
                        let angle = -std::f32::consts::FRAC_PI_2
                            + (i as f32) * (std::f32::consts::TAU / 6.0);
                        let p = Point::new(cx + r * angle.cos(), cy + r * angle.sin());
                        if i == 0 {
                            b.move_to(p);
                        } else {
                            b.line_to(p);
                        }
                    }
                    b.close();
                });
                frame.stroke(&hex, stroke);
                frame.fill(
                    &canvas::Path::circle(Point::new(cx, cy), r * 0.25),
                    self.color,
                );
            }
            WorkspaceGuild::ScopedDocs => {
                let brackets = canvas::Path::new(|b| {
                    b.move_to(Point::new(cx - r * 0.35, cy - r * 0.55));
                    b.line_to(Point::new(cx - r * 0.85, cy));
                    b.line_to(Point::new(cx - r * 0.35, cy + r * 0.55));

                    b.move_to(Point::new(cx + r * 0.35, cy - r * 0.55));
                    b.line_to(Point::new(cx + r * 0.85, cy));
                    b.line_to(Point::new(cx + r * 0.35, cy + r * 0.55));

                    b.move_to(Point::new(cx - r * 0.16, cy + r * 0.62));
                    b.line_to(Point::new(cx + r * 0.16, cy - r * 0.62));
                });
                frame.stroke(&brackets, stroke);
            }
            WorkspaceGuild::TrustGovernance => {
                let shield = canvas::Path::new(|b| {
                    b.move_to(Point::new(cx, cy - r * 0.92));
                    b.line_to(Point::new(cx + r * 0.78, cy - r * 0.52));
                    b.line_to(Point::new(cx + r * 0.68, cy + r * 0.22));
                    b.line_to(Point::new(cx, cy + r * 0.92));
                    b.line_to(Point::new(cx - r * 0.68, cy + r * 0.22));
                    b.line_to(Point::new(cx - r * 0.78, cy - r * 0.52));
                    b.close();
                });
                frame.stroke(&shield, stroke);
                frame.fill(
                    &canvas::Path::circle(Point::new(cx, cy - r * 0.04), r * 0.21),
                    self.color,
                );
            }
            WorkspaceGuild::ExperimentalLab => {
                frame.stroke(&canvas::Path::circle(Point::new(cx, cy), r * 0.78), stroke);
                let cross = canvas::Path::new(|b| {
                    b.move_to(Point::new(cx - r * 0.95, cy));
                    b.line_to(Point::new(cx + r * 0.95, cy));
                    b.move_to(Point::new(cx, cy - r * 0.95));
                    b.line_to(Point::new(cx, cy + r * 0.95));
                });
                frame.stroke(&cross, stroke);
                frame.fill(
                    &canvas::Path::circle(Point::new(cx, cy), r * 0.25),
                    self.color,
                );
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Channels in the left `#channels` navigation sidebar mapped onto canonical [`DesktopScreen`] states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelTab {
    Dashboard,
    Chat,
    CodeWorkspace,
    WorkflowPipeline,
    AgentWorkspace,
    ModelLibrary,
    Hardware,
    AuditJournal,
    BrokerGate,
    AdapterStudio,
    ExtensionManager,
    Settings,
}

impl ChannelTab {
    pub const CORE_CHANNELS: [Self; 8] = [
        Self::Dashboard,
        Self::Chat,
        Self::CodeWorkspace,
        Self::WorkflowPipeline,
        Self::AgentWorkspace,
        Self::ModelLibrary,
        Self::Hardware,
        Self::Settings,
    ];

    pub const SECURITY_CHANNELS: [Self; 2] = [Self::AuditJournal, Self::BrokerGate];

    pub const EXPERIMENTAL_CHANNELS: [Self; 2] = [Self::AdapterStudio, Self::ExtensionManager];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Dashboard => "Command HUD",
            Self::Chat => "Agent Chat",
            Self::CodeWorkspace => "Code & Env",
            Self::AgentWorkspace => "Projects",
            Self::ModelLibrary => "Models",
            Self::Hardware => "llama.cpp",
            Self::WorkflowPipeline => "Tasks & Daily",
            Self::AuditJournal => "Audit Log",
            Self::BrokerGate => "Security Broker",
            Self::AdapterStudio => "Adapter Studio",
            Self::ExtensionManager => "Extensions",
            Self::Settings => "Custom Studio",
        }
    }

    pub fn topic(self) -> &'static str {
        match self {
            Self::Dashboard => "Command deck, telemetry radar & quick agent launch",
            Self::Chat => "Streaming local/API inference, Agent Kind, Effort, Context & Drag-Drop",
            Self::CodeWorkspace => {
                "Workspace code explorer, Trusted Folders access control & scoped .env secrets"
            }
            Self::AgentWorkspace => {
                "Multi-project workspaces, scoped directory grants & sandbox rules"
            }
            Self::ModelLibrary => "Verified SHA-256 content-addressed GGUF model inventory",
            Self::Hardware => {
                "llama.cpp backend parameters, GPU offload, KV quant & HIE context budget"
            }
            Self::WorkflowPipeline => {
                "SQLite Daily log, Tasks DB, completed archive & deterministic verification"
            }
            Self::AuditJournal => "Redacted SQLite audit log & emergency credential revocation",
            Self::BrokerGate => "Single-use privileged broker permits (`lora.broker.receipt.v1`)",
            Self::AdapterStudio => "Experimental LoRA evaluation & split leakage gates",
            Self::ExtensionManager => "Experimental sandboxed MCP/Wasm extension lifecycle",
            Self::Settings => "Infinite theme engine, Capri widget customizer & shell studio",
        }
    }

    pub fn guild(self) -> WorkspaceGuild {
        match self {
            Self::Dashboard | Self::ModelLibrary | Self::Hardware | Self::Settings => {
                WorkspaceGuild::LocalCore
            }
            Self::Chat | Self::CodeWorkspace | Self::AgentWorkspace => WorkspaceGuild::ScopedDocs,
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
            Self::CodeWorkspace | Self::AgentWorkspace => DesktopScreen::AgentWorkspace,
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
    pub answer_markdown: markdown::Content,
    pub thinking_text: Option<String>,
    pub thinking_in_progress: bool,
    pub artifacts: Vec<ux_studio::ExtractedArtifact>,
    pub outcome_sha256: Option<String>,
    pub outcome_verified: bool,
    pub citations: Vec<String>,
    pub receipt_count: usize,
    pub is_streaming: bool,
    pub show_thinking: bool,
    pub show_trace: bool,
    pub token_estimate: usize,
    pub latency_ms: u64,
}

impl ChatMessageEntry {
    pub fn user(id: impl Into<String>, text: impl Into<String>) -> Self {
        let raw = text.into();
        let token_estimate = raw.split_whitespace().count().max(1);
        Self {
            id: id.into(),
            is_user: true,
            author: "Local Operator".to_string(),
            badge: "OPERATOR".to_string(),
            markdown: markdown::Content::parse(&raw),
            answer_markdown: markdown::Content::parse(&raw),
            thinking_text: None,
            thinking_in_progress: false,
            artifacts: Vec::new(),
            raw_text: raw,
            outcome_sha256: None,
            outcome_verified: false,
            citations: Vec::new(),
            receipt_count: 0,
            is_streaming: false,
            show_thinking: false,
            show_trace: false,
            token_estimate,
            latency_ms: 0,
        }
    }

    pub fn assistant(
        id: impl Into<String>,
        model_label: impl Into<String>,
        initial_text: &str,
        streaming: bool,
    ) -> Self {
        let token_estimate = initial_text.split_whitespace().count();
        let mut entry = Self {
            id: id.into(),
            is_user: false,
            author: format!("Lora · {}", model_label.into()),
            badge: "LOCAL".to_string(),
            raw_text: initial_text.to_string(),
            markdown: markdown::Content::parse(initial_text),
            answer_markdown: markdown::Content::parse(""),
            thinking_text: None,
            thinking_in_progress: false,
            artifacts: Vec::new(),
            outcome_sha256: None,
            outcome_verified: false,
            citations: Vec::new(),
            receipt_count: 0,
            is_streaming: streaming,
            show_thinking: false,
            show_trace: false,
            token_estimate,
            latency_ms: 0,
        };
        entry.refresh_parsed_fields();
        entry
    }

    pub fn from_persisted(row: &PersistedChatRow) -> Self {
        let mut entry = Self {
            id: row.msg_id.clone(),
            is_user: row.is_user,
            author: row.author.clone(),
            badge: row.badge.clone(),
            raw_text: row.raw_text.clone(),
            markdown: markdown::Content::parse(&row.raw_text),
            answer_markdown: markdown::Content::parse(""),
            thinking_text: None,
            thinking_in_progress: false,
            artifacts: Vec::new(),
            outcome_sha256: row.outcome_sha256.clone(),
            outcome_verified: row.outcome_verified,
            citations: row.citations.clone(),
            receipt_count: row.receipt_count,
            is_streaming: false,
            show_thinking: false,
            show_trace: false,
            token_estimate: row.raw_text.split_whitespace().count(),
            latency_ms: 0,
        };
        entry.refresh_parsed_fields();
        entry
    }

    fn refresh_parsed_fields(&mut self) {
        if self.is_user {
            self.answer_markdown = markdown::Content::parse(&self.raw_text);
            self.thinking_text = None;
            self.thinking_in_progress = false;
            self.artifacts.clear();
            return;
        }
        let parsed = ux_studio::parse_structured_response(&self.raw_text);
        self.thinking_text = parsed.thinking;
        self.thinking_in_progress = parsed.thinking_in_progress;
        self.answer_markdown = markdown::Content::parse(&parsed.answer);
        self.artifacts = parsed.artifacts;
    }

    pub fn to_persisted(&self) -> PersistedChatRow {
        PersistedChatRow {
            msg_id: self.id.clone(),
            is_user: self.is_user,
            author: self.author.clone(),
            badge: self.badge.clone(),
            raw_text: self.raw_text.clone(),
            outcome_sha256: self.outcome_sha256.clone(),
            outcome_verified: self.outcome_verified,
            citations: self.citations.clone(),
            receipt_count: self.receipt_count,
        }
    }

    pub fn append_delta(&mut self, delta: &str) {
        self.raw_text.push_str(delta);
        self.markdown.push_str(delta);
        self.token_estimate = self.raw_text.split_whitespace().count();
        self.refresh_parsed_fields();
    }

    pub fn replace_content(&mut self, full_text: &str) {
        self.raw_text = full_text.to_string();
        self.markdown = markdown::Content::parse(full_text);
        self.token_estimate = self.raw_text.split_whitespace().count();
        self.refresh_parsed_fields();
    }
}

/// Reversible actions stored for instant Ctrl+Z / footer Undo rollback.
#[derive(Debug, Clone)]
pub enum UndoAction {
    RestoreDeletedTask(AgentTask),
    RestoreTaskStatus {
        task_id: i64,
        previous: TaskStatus,
    },
    RestoreProjectChat {
        project_id: i64,
        rows: Vec<PersistedChatRow>,
    },
    RestoreTheme {
        preset: CyberThemePreset,
        custom: theme::CustomThemeSpec,
    },
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
    // Tasks DB
    SelectTaskFilter(TaskFilter),
    ToggleTaskAllProjects,
    TaskSearchChanged(String),
    NewTaskTitleChanged(String),
    NewTaskNotesChanged(String),
    NewTaskAssigneeChanged(String),
    NewTaskPrioritySelected(TaskPriority),
    CreateAgentTask,
    SetTaskStatus {
        task_id: i64,
        status: TaskStatus,
    },
    DeleteAgentTask(i64),
    ExecuteTaskInAgent {
        task_id: i64,
        prompt: String,
    },
    // Multi-project switcher & creation
    NewProjectNameChanged(String),
    NewProjectTagChanged(String),
    NewProjectRootChanged(String),
    NewProjectDescChanged(String),
    NewProjectPromptChanged(String),
    CreateWorkspaceProject,
    SelectWorkspaceProject(i64),
    DeleteWorkspaceProject(i64),
    // llama.cpp engine control panel
    ResetLlamaConfig,
    LlamaGpuLayersChanged(u32),
    LlamaThreadsChanged(u32),
    LlamaBatchChanged(u32),
    LlamaKvQuantChanged(KvCacheType),
    LlamaToggleFlashAttention(bool),
    LlamaToggleKvOffload(bool),
    LlamaToggleMmap(bool),
    LlamaToggleMlock(bool),
    LlamaTempChanged(u32),
    LlamaTopPChanged(u32),
    LlamaTopKChanged(u32),
    LlamaRepeatPenaltyChanged(u32),
    // Infinite appearance & workspace studio
    ToggleCustomTheme(bool),
    CustomPrimaryHueChanged(u16),
    CustomSecondaryHueChanged(u16),
    SelectSurfaceDepth(SurfaceDepth),
    CornerRadiusChanged(u8),
    GlowStrengthChanged(u8),
    SidebarWidthChanged(u16),
    ToggleIconRail(bool),
    ToggleStatusFooter(bool),
    ToggleMonoChrome(bool),
    ToggleCyberMotion(bool),
    // Modern UI/UX/CX/Psychology/Motion controls
    RuntimeEvent(Event),
    OpenCommandPalette,
    CloseCommandPalette,
    CommandPaletteQueryChanged(String),
    RunCommandPaletteAction(Box<Message>),
    TogglePauseRun,
    ToggleMessageThinking(String),
    ToggleMessageTrace(String),
    EditPromptMessage(String),
    ClearProjectChat,
    UndoLastAction,
    ToggleChatSearch,
    ChatSearchChanged(String),
    ChatScrolled(bool),
    JumpToChatBottom,
    PasteClipboardPressed,
    ClipboardLoaded(Option<String>),
    RemoveStagedAttachment(usize),
    CopyTextToClipboard(String),
    ToggleArtifactPanel,
    SelectArtifactTab(usize),
    SelectPersona(AgentPersona),
    ToggleNativeNotifications(bool),
    PokeMiniBot,
    ToggleMiniBot(bool),
    // Agent Type, Reasoning Effort, Execution Access {Local, Api}, Folder Trust, .env & Capri Customizer
    SelectAgentKind(AgentKind),
    SelectReasoningEffort(ReasoningEffort),
    SelectAccessMode(ExecutionAccessMode),
    ApiBaseUrlChanged(String),
    ApiModelNameChanged(String),
    ApiKeyEnvChanged(String),
    NewTaskKindSelected(TaskKind),
    SummarizeDailyTasksInAgent,
    AttachPathChanged(String),
    AttachPathSubmit,
    QuickAttachOrTrustPath(String),
    NewTrustPathChanged(String),
    NewTrustLevelSelected(FolderAccessLevel),
    AddTrustedFolder,
    TrustActiveWorkspaceRoot,
    CycleTrustedFolderLevel {
        folder_id: i64,
        next_level: FolderAccessLevel,
    },
    DeleteTrustedFolder(i64),
    NewEnvKeyChanged(String),
    NewEnvValChanged(String),
    NewEnvSecretToggled(bool),
    ToggleEnvScopeGlobal,
    SaveEnvVar,
    ToggleRevealSecret(i64),
    ToggleEnvVarEnabled {
        env_id: i64,
        enabled: bool,
    },
    DeleteEnvVar(i64),
    ImportWorkspaceDotenv,
    CapriAuraPresetChanged(CapriAuraPreset),
    CapriCrownHueChanged(u16),
    CapriVisorHueChanged(u16),
    CapriGlitchChanged(u8),
    CapriScaleChanged(u16),
    CapriEyeModeChanged(CapriEyeMode),
    CapriToggleCrown(bool),
    CapriToggleLightning(bool),
    CapriToggleGrin(bool),
    CapriToggleDockInChat(bool),
    CapriToggleHeaderChip(bool),
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

    // SQLite agent workspace DB, tasks & projects
    pub workspace_db: WorkspaceDb,
    pub projects_cache: Vec<WorkspaceProject>,
    pub active_project_id: i64,
    pub task_filter: TaskFilter,
    pub task_scope_all_projects: bool,
    pub task_search_input: String,
    pub new_task_title: String,
    pub new_task_notes: String,
    pub new_task_assignee: String,
    pub new_task_priority: TaskPriority,
    pub new_project_name: String,
    pub new_project_tag: String,
    pub new_project_root: String,
    pub new_project_desc: String,
    pub new_project_prompt: String,
    pub llama_config: LlamaRuntimeConfig,
    // Modern AI UX/CX/Psychology/Motion systems
    pub persona: AgentPersona,
    pub staged_attachments: Vec<StagedAttachment>,
    pub run_paused: bool,
    pub paused_delta_buffer: String,
    pub active_run_started_ms: Option<u64>,
    pub last_run_elapsed_ms: u64,
    pub command_palette_open: bool,
    pub command_palette_query: String,
    pub chat_search_open: bool,
    pub chat_search_query: String,
    pub chat_auto_scroll: bool,
    pub artifact_panel_open: bool,
    pub selected_artifact_idx: usize,
    pub undo_stack: Vec<UndoAction>,
    pub bot_poke_count: u64,
    pub bot_quip: String,
    // Agent Type, Effort, Access {Local, Api}, Folder Trust, .env & Drag-Drop hover
    pub agent_kind: AgentKind,
    pub reasoning_effort: ReasoningEffort,
    pub access_mode: ExecutionAccessMode,
    pub api_config: ApiEndpointConfig,
    pub new_task_kind: TaskKind,
    pub trusted_folders: Vec<TrustedFolderEntry>,
    pub new_trust_path: String,
    pub new_trust_level: FolderAccessLevel,
    pub env_vars: Vec<EnvVarEntry>,
    pub new_env_key: String,
    pub new_env_val: String,
    pub new_env_secret: bool,
    pub env_scope_global: bool,
    pub reveal_secret_ids: Vec<i64>,
    pub attach_path_input: String,
    pub drag_hover_path: Option<String>,
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
        dashboard.sync_motion_to_prefs();

        let default_root = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        let workspace_db = WorkspaceDb::open_or_memory(service.data_dir(), &default_root);
        let projects_cache = workspace_db.list_projects();
        let active_project_id = workspace_db
            .load_active_project_id()
            .filter(|id| projects_cache.iter().any(|p| p.id == *id))
            .unwrap_or_else(|| projects_cache.first().map(|p| p.id).unwrap_or(1));
        let workspace_root_input = projects_cache
            .iter()
            .find(|p| p.id == active_project_id)
            .map(|p| p.root_path.clone())
            .unwrap_or_else(|| default_root.clone());
        let llama_config = workspace_db.load_llama_config();
        let requested_context = llama_config.ctx_size.clamp(512, 16384);
        let persona = workspace_db.load_persona();
        let agent_kind = workspace_db.load_agent_kind();
        let reasoning_effort = workspace_db.load_reasoning_effort();
        let access_mode = workspace_db.load_access_mode();
        let api_config = workspace_db.load_api_config();
        let trusted_folders = workspace_db.list_trusted_folders(Some(active_project_id));
        let env_vars = workspace_db.list_env_vars(Some(active_project_id));
        let saved_rows = workspace_db.load_project_chat(active_project_id);

        let selected_model = telemetry.models.first().map(|m| m.display_alias.clone());
        let messages: Vec<ChatMessageEntry> = if saved_rows.is_empty() {
            vec![Self::welcome_message()]
        } else {
            saved_rows
                .iter()
                .map(ChatMessageEntry::from_persisted)
                .collect()
        };

        Self {
            service,
            approval_port,
            event_tx,
            event_rx,
            active_guild: WorkspaceGuild::LocalCore,
            active_channel: ChannelTab::Dashboard,
            show_inspector: false,
            experimental_enabled,
            telemetry,
            status_banner: "Air-gapped native agent ready · local_cpu_worker".to_string(),
            messages,
            composer_input: String::new(),
            selected_model,
            read_only_assistant: false,
            use_rag: false,
            workspace_root_input,
            requested_context,
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
            workspace_db,
            projects_cache,
            active_project_id,
            task_filter: TaskFilter::All,
            task_scope_all_projects: false,
            task_search_input: String::new(),
            new_task_title: String::new(),
            new_task_notes: String::new(),
            new_task_assignee: "lora-agent".to_string(),
            new_task_priority: TaskPriority::P1,
            new_project_name: String::new(),
            new_project_tag: String::new(),
            new_project_root: default_root,
            new_project_desc: String::new(),
            new_project_prompt: String::new(),
            llama_config,
            persona,
            staged_attachments: Vec::new(),
            run_paused: false,
            paused_delta_buffer: String::new(),
            active_run_started_ms: None,
            last_run_elapsed_ms: 0,
            command_palette_open: false,
            command_palette_query: String::new(),
            chat_search_open: false,
            chat_search_query: String::new(),
            chat_auto_scroll: true,
            artifact_panel_open: false,
            selected_artifact_idx: 0,
            undo_stack: Vec::new(),
            bot_poke_count: 0,
            bot_quip: "Capri × Lora glitch-spark ready · poke for tips!".to_string(),
            agent_kind,
            reasoning_effort,
            access_mode,
            api_config,
            new_task_kind: TaskKind::General,
            trusted_folders,
            new_trust_path: String::new(),
            new_trust_level: FolderAccessLevel::ReadWrite,
            env_vars,
            new_env_key: String::new(),
            new_env_val: String::new(),
            new_env_secret: false,
            env_scope_global: false,
            reveal_secret_ids: Vec::new(),
            attach_path_input: String::new(),
            drag_hover_path: None,
        }
    }

    fn welcome_message() -> ChatMessageEntry {
        ChatMessageEntry::assistant(
            "welcome-0",
            "System Shell",
            "Welcome to **Lora Agent Desktop (`v0.2.23`)** — 100% pure-Rust native AI agent studio.\n\n\
             - **Air-Gapped Runtime**: Zero WebView, zero HTML/JS bridge, zero external egress.\n\
             - **SQLite Tasks & Multi-Project Scope**: Manage tasks in `Tasks DB`, isolate folders in `Projects`, and tune `llama.cpp` inference locally.\n\n\
             Press `Ctrl+K` for the Command Palette, drag & drop any code/image/audio/`.gguf` file, or pick a template below to start.",
            false,
        )
    }

    pub fn agent_phase(&self) -> AgentPhase {
        if self.pending_consent.is_some() {
            return AgentPhase::Error;
        }
        if self.active_run_id.is_some() {
            if self.run_paused {
                return AgentPhase::Paused;
            }
            if let Some(last) = self.messages.last() {
                if !last.is_user && last.is_streaming && last.raw_text.trim().is_empty() {
                    return AgentPhase::Thinking;
                }
            }
            return AgentPhase::Streaming;
        }
        if self
            .messages
            .last()
            .is_some_and(|m| !m.is_user && m.outcome_verified)
        {
            return AgentPhase::Verified;
        }
        AgentPhase::Idle
    }

    fn push_undo(&mut self, action: UndoAction) {
        self.undo_stack.push(action);
        if self.undo_stack.len() > 24 {
            self.undo_stack.remove(0);
        }
    }

    fn persist_project_chat(&self) {
        let rows: Vec<PersistedChatRow> = self
            .messages
            .iter()
            .filter(|m| !m.is_streaming && m.id != "welcome-0")
            .map(ChatMessageEntry::to_persisted)
            .collect();
        self.workspace_db
            .replace_project_chat(self.active_project_id, &rows);
    }

    fn sync_project_chat_from_db(&mut self) {
        let saved = self.workspace_db.load_project_chat(self.active_project_id);
        self.messages = if saved.is_empty() {
            vec![Self::welcome_message()]
        } else {
            saved.iter().map(ChatMessageEntry::from_persisted).collect()
        };
    }

    pub fn active_project_name(&self) -> &str {
        self.projects_cache
            .iter()
            .find(|p| p.id == self.active_project_id)
            .map(|p| p.name.as_str())
            .unwrap_or("Lora Core")
    }

    pub fn title(&self) -> String {
        format!(
            "Lora Agent · {} [{}]",
            self.active_channel.slug(),
            self.active_project_name()
        )
    }

    pub fn theme(&self) -> Theme {
        self.dashboard.palette().iced_theme()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let interval_ms = if self.active_run_id.is_some()
            || self.pending_consent.is_some()
            || self.dashboard.is_animating()
        {
            16
        } else if self.dashboard.prefs.cyber_motion {
            28
        } else {
            120
        };
        Subscription::batch([
            iced::time::every(Duration::from_millis(interval_ms)).map(|_| Message::Tick),
            iced::event::listen().map(Message::RuntimeEvent),
        ])
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

    fn refresh_workspace_caches(&mut self) {
        self.trusted_folders = self
            .workspace_db
            .list_trusted_folders(Some(self.active_project_id));
        self.env_vars = self
            .workspace_db
            .list_env_vars(Some(self.active_project_id));
    }

    fn handle_dropped_or_staged_path(&mut self, path: &std::path::Path) {
        if path.is_dir() {
            let folder_str = path.to_string_lossy().to_string();
            let _ = self.workspace_db.upsert_trusted_folder(
                self.active_project_id,
                &folder_str,
                FolderAccessLevel::ReadWrite,
            );
            self.refresh_workspace_caches();
            self.status_banner = format!(
                "Added folder '{}' as READ/WRITE in Trusted Folders.",
                folder_str
            );
            return;
        }
        if ux_studio::is_dotenv_path(path) {
            match std::fs::read_to_string(path) {
                Ok(raw) => {
                    let target_pid = if self.env_scope_global {
                        0
                    } else {
                        self.active_project_id
                    };
                    let imported = self.workspace_db.import_dotenv_text(target_pid, &raw);
                    self.refresh_workspace_caches();
                    self.active_channel = ChannelTab::CodeWorkspace;
                    self.active_guild = WorkspaceGuild::ScopedDocs;
                    self.status_banner = format!(
                        "Imported {imported} variables from '{}' into Env Manager.",
                        path.file_name().and_then(|n| n.to_str()).unwrap_or(".env")
                    );
                }
                Err(e) => {
                    self.status_banner = format!("Failed to read .env file: {e}");
                }
            }
            return;
        }
        let att = ux_studio::classify_dropped_path(path);
        if att.kind == ux_studio::AttachmentKind::GgufModel {
            self.import_path_input = path.to_string_lossy().to_string();
            self.active_channel = ChannelTab::ModelLibrary;
            self.active_guild = WorkspaceGuild::LocalCore;
            self.status_banner = format!(
                "Staged GGUF model '{}' for SHA-256 verification.",
                att.label
            );
        } else {
            self.status_banner = format!(
                "Attached [{}] '{}' ({} bytes) to agent prompt.",
                att.kind.badge(),
                att.label,
                att.bytes
            );
            self.staged_attachments.push(att);
            if self.active_channel != ChannelTab::CodeWorkspace {
                self.active_channel = ChannelTab::Chat;
                self.active_guild = WorkspaceGuild::ScopedDocs;
            }
        }
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

        let (model, api_key_opt) = match self.access_mode {
            ExecutionAccessMode::Local => {
                let Some(m) = self.selected_model.clone() else {
                    self.composer_input = prompt;
                    self.status_banner =
                        "No verified GGUF model imported yet. Prompt staged — import a model in Models or switch Access -> API."
                            .to_string();
                    self.active_channel = ChannelTab::ModelLibrary;
                    self.active_guild = WorkspaceGuild::LocalCore;
                    return;
                };
                (m, None)
            }
            ExecutionAccessMode::Api => {
                let key_name = self.api_config.api_key_env.trim();
                let resolved = self
                    .workspace_db
                    .resolve_env_value(self.active_project_id, key_name);
                if resolved.is_none() || self.api_config.base_url.trim().is_empty() {
                    self.composer_input = prompt;
                    self.active_channel = ChannelTab::CodeWorkspace;
                    self.active_guild = WorkspaceGuild::ScopedDocs;
                    self.status_banner = format!(
                        "API mode needs Base URL and env var '{}' configured in Code & Env.",
                        key_name
                    );
                    return;
                }
                (self.api_config.model_name.clone(), resolved)
            }
        };

        let seq = self.next_run_seq;
        self.next_run_seq += 1;
        let run_id = format!("desk-run-{}-{seq}", now_epoch_ms());
        self.composer_input.clear();
        let staged = std::mem::take(&mut self.staged_attachments);
        self.active_channel = ChannelTab::Chat;
        self.active_guild = WorkspaceGuild::ScopedDocs;
        self.run_paused = false;
        self.paused_delta_buffer.clear();
        self.active_run_started_ms = Some(now_epoch_ms());
        self.chat_auto_scroll = true;

        let display_prompt = if staged.is_empty() {
            prompt.clone()
        } else {
            let tags: Vec<String> = staged
                .iter()
                .map(|a| format!("[{}: {}]", a.kind.badge(), a.label))
                .collect();
            format!("{prompt}\n\nAttachments: {}", tags.join(" · "))
        };
        self.messages.push(ChatMessageEntry::user(
            format!("usr-{seq}"),
            &display_prompt,
        ));
        self.messages
            .push(ChatMessageEntry::assistant(&run_id, &model, "", true));

        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.active_cancel_flag = Some(Arc::clone(&cancel_flag));
        self.active_run_id = Some(run_id.clone());

        let folder_trust = self
            .workspace_db
            .folder_trust_for_path(self.active_project_id, &self.workspace_root_input);
        let proj_rule = self
            .projects_cache
            .iter()
            .find(|p| p.id == self.active_project_id)
            .map(|p| p.system_prompt.trim().to_string())
            .filter(|s| !s.is_empty());

        let mut context_blocks = vec![
            format!(
                "[Agent Kind: {} · {}]",
                self.agent_kind.label(),
                self.agent_kind.directive()
            ),
            format!(
                "[Reasoning Effort: {} · {}]",
                self.reasoning_effort.label(),
                self.reasoning_effort.directive()
            ),
            format!(
                "[Folder Trust: {} -> {}]",
                self.workspace_root_input,
                folder_trust.label()
            ),
        ];
        if !self.persona.directive().is_empty() {
            context_blocks.push(format!("[Persona Directive: {}]", self.persona.directive()));
        }
        if let Some(rule) = proj_rule {
            context_blocks.push(format!("[Project Context: {rule}]"));
        }
        let active_env_keys: Vec<String> = self
            .env_vars
            .iter()
            .filter(|e| e.enabled)
            .map(|e| e.env_key.clone())
            .collect();
        if !active_env_keys.is_empty() {
            context_blocks.push(format!("[Active Env Keys: {}]", active_env_keys.join(", ")));
        }
        for att in &staged {
            let preview = att
                .text_preview
                .as_deref()
                .unwrap_or("binary local media attached");
            let path_str = att
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "clipboard".to_string());
            context_blocks.push(format!(
                "[Attached {} '{}' ({}, {} bytes)]:\n{}",
                att.kind.badge(),
                att.label,
                path_str,
                att.bytes,
                preview
            ));
        }
        let effective_prompt = format!("{}\n\n{prompt}", context_blocks.join("\n"));

        if let Some(api_key) = api_key_opt {
            self.status_banner = format!(
                "Running {run_id} via API ({}, effort={})...",
                model,
                self.reasoning_effort.label()
            );
            spawn_api_chat_run(
                self.event_tx.clone(),
                run_id,
                self.api_config.base_url.clone(),
                model,
                api_key,
                effective_prompt,
                self.reasoning_effort.code().to_string(),
                cancel_flag,
            );
        } else {
            self.status_banner = format!(
                "Running {run_id} on '{model}' (local, kind={}, effort={})...",
                self.agent_kind.label(),
                self.reasoning_effort.label()
            );
            let mode = if self.read_only_assistant && folder_trust != FolderAccessLevel::Deny {
                ExecutionMode::ReadOnlyAssistant
            } else {
                ExecutionMode::ChatOnly
            };
            let ws_path = (self.read_only_assistant && folder_trust != FolderAccessLevel::Deny)
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
                effective_prompt,
                self.use_rag,
                cancel_flag,
            );
        }
    }

    fn drain_worker_events(&mut self) -> bool {
        let Ok(rx) = self.event_rx.try_lock() else {
            return false;
        };
        let mut needs_refresh = false;
        let mut should_scroll = false;
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
                    if self.run_paused && self.active_run_id.as_deref() == Some(run_id.as_str()) {
                        self.paused_delta_buffer.push_str(&delta);
                    } else if let Some(msg) =
                        self.messages.iter_mut().rev().find(|m| m.id == run_id)
                    {
                        msg.append_delta(&delta);
                        should_scroll = true;
                    }
                }
                AppWorkerEvent::RunFinished { run_id, result } => {
                    let buffered = std::mem::take(&mut self.paused_delta_buffer);
                    let started_at = self.active_run_started_ms.take();
                    let elapsed_ms = started_at
                        .map(|s| now_epoch_ms().saturating_sub(s))
                        .unwrap_or(0);
                    self.last_run_elapsed_ms = elapsed_ms;
                    self.run_paused = false;
                    self.active_run_id = None;
                    self.active_cancel_flag = None;
                    self.pending_consent = None;
                    let notify_ok;
                    let notify_summary;
                    if let Some(msg) = self.messages.iter_mut().rev().find(|m| m.id == run_id) {
                        if !buffered.is_empty() {
                            msg.append_delta(&buffered);
                        }
                        msg.is_streaming = false;
                        msg.latency_ms = elapsed_ms;
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
                                let s = format!(
                                    "Run {run_id} finished in {}ms ({:?}, verified={})",
                                    elapsed_ms,
                                    summary.outcome.final_state,
                                    summary.outcome_verified
                                );
                                self.status_banner = s.clone();
                                notify_ok = true;
                                notify_summary = s;
                            }
                            Err(err) => {
                                msg.replace_content(&format!("**Run Stopped / Error:** `{err}`"));
                                self.status_banner = err.clone();
                                notify_ok = false;
                                notify_summary = err;
                            }
                        }
                    } else {
                        notify_ok = false;
                        notify_summary = format!("Run {run_id} completed");
                    }
                    self.persist_project_chat();
                    if elapsed_ms >= 3000 || !notify_ok {
                        ux_studio::notify_desktop_completion(
                            self.dashboard.prefs.native_notifications,
                            if notify_ok {
                                "Lora Agent Completed"
                            } else {
                                "Lora Agent Attention"
                            },
                            &notify_summary,
                        );
                    }
                    should_scroll = true;
                    needs_refresh = true;
                }
                AppWorkerEvent::ApiRunFinished {
                    run_id,
                    model_label,
                    result,
                } => {
                    let _ = std::mem::take(&mut self.paused_delta_buffer);
                    let started_at = self.active_run_started_ms.take();
                    let elapsed_ms = started_at
                        .map(|s| now_epoch_ms().saturating_sub(s))
                        .unwrap_or(0);
                    self.last_run_elapsed_ms = elapsed_ms;
                    self.run_paused = false;
                    self.active_run_id = None;
                    self.active_cancel_flag = None;
                    self.pending_consent = None;
                    if let Some(msg) = self.messages.iter_mut().rev().find(|m| m.id == run_id) {
                        msg.is_streaming = false;
                        msg.latency_ms = elapsed_ms;
                        match result {
                            Ok(answer) => {
                                msg.badge = "API".to_string();
                                msg.replace_content(&answer);
                                self.status_banner = format!(
                                    "API response from '{}' finished in {}ms",
                                    model_label, elapsed_ms
                                );
                            }
                            Err(err) => {
                                msg.badge = "API ERR".to_string();
                                msg.replace_content(&format!("**API Error:** `{err}`"));
                                self.status_banner = err;
                            }
                        }
                    }
                    self.persist_project_chat();
                    should_scroll = true;
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
        should_scroll
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let should_scroll = self.drain_worker_events();
                if self.dashboard.should_sample_on_tick() {
                    self.refresh_telemetry();
                }
                self.dashboard.step_motion(self.active_run_id.is_some());
                if should_scroll && self.chat_auto_scroll && self.active_channel == ChannelTab::Chat
                {
                    return operation::snap_to_end(iced::widget::Id::new(CHAT_SCROLL_ID));
                }
            }
            Message::SelectGuild(guild) => {
                self.active_guild = guild;
                let next = guild.default_channel();
                if next.is_experimental() {
                    self.experimental_enabled = true;
                }
                self.active_channel = next;
                self.dashboard.trigger_transition();
                self.refresh_telemetry();
            }
            Message::SelectChannel(ch) => {
                if ch.is_experimental() && !self.experimental_enabled {
                    self.status_banner =
                        "Enable experimental channels in Custom Studio or select the [EX] guild."
                            .to_string();
                } else {
                    if self.active_channel != ch {
                        self.dashboard.trigger_transition();
                    }
                    self.active_channel = ch;
                    self.active_guild = ch.guild();
                    self.refresh_telemetry();
                }
            }
            Message::ToggleInspector => {
                self.show_inspector = !self.show_inspector;
                let reduced = !self.dashboard.prefs.cyber_motion;
                self.dashboard
                    .motion
                    .spring_inspector
                    .set_target(if self.show_inspector { 1.0 } else { 0.0 }, reduced);
                self.dashboard.trigger_pulse();
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
                let clamped = ctx.clamp(512, 32768);
                self.requested_context = clamped;
                self.llama_config.ctx_size = clamped;
                self.workspace_db.save_llama_config(&self.llama_config);
                self.status_banner = format!("Context budget & llama.cpp window: {clamped} tokens");
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
                self.dashboard.trigger_pulse();
                self.persist_dashboard_prefs();
                self.status_banner = format!("Dashboard preset active: {}", preset.label());
            }
            Message::SelectCyberTheme(preset) => {
                self.push_undo(UndoAction::RestoreTheme {
                    preset: self.dashboard.prefs.theme_preset,
                    custom: self.dashboard.prefs.custom_theme.clone(),
                });
                self.dashboard.prefs.custom_theme.enabled = false;
                self.dashboard.prefs.theme_preset = preset;
                self.dashboard.trigger_pulse();
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
                self.dashboard.prefs.corner_radius_px = geometry.card_radius() as u8;
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
            // Tasks DB
            Message::SelectTaskFilter(f) => {
                self.task_filter = f;
            }
            Message::ToggleTaskAllProjects => {
                self.task_scope_all_projects = !self.task_scope_all_projects;
            }
            Message::TaskSearchChanged(q) => {
                self.task_search_input = q;
            }
            Message::NewTaskTitleChanged(v) => {
                self.new_task_title = v;
            }
            Message::NewTaskNotesChanged(v) => {
                self.new_task_notes = v;
            }
            Message::NewTaskAssigneeChanged(v) => {
                self.new_task_assignee = v;
            }
            Message::NewTaskPrioritySelected(p) => {
                self.new_task_priority = p;
            }
            Message::CreateAgentTask => {
                let title = self.new_task_title.trim().to_string();
                if title.is_empty() {
                    self.status_banner = "Enter a task objective first.".to_string();
                } else if let Some(id) = self.workspace_db.create_task_with_kind(
                    self.active_project_id,
                    &title,
                    self.new_task_notes.trim(),
                    self.new_task_priority,
                    self.new_task_kind,
                    self.new_task_assignee.trim(),
                ) {
                    self.new_task_title.clear();
                    self.new_task_notes.clear();
                    self.status_banner = format!(
                        "{} task #{id} saved to SQLite workspace DB.",
                        self.new_task_kind.label()
                    );
                } else {
                    self.status_banner = "Failed to save task to SQLite DB.".to_string();
                }
            }
            Message::SetTaskStatus { task_id, status } => {
                let prev = self
                    .workspace_db
                    .list_tasks(None)
                    .into_iter()
                    .find(|t| t.id == task_id)
                    .map(|t| t.status);
                if self.workspace_db.update_task_status(task_id, status) {
                    if let Some(previous) = prev {
                        if previous != status {
                            self.push_undo(UndoAction::RestoreTaskStatus { task_id, previous });
                        }
                    }
                    self.status_banner = format!("Task #{task_id} updated to {}.", status.label());
                }
            }
            Message::DeleteAgentTask(task_id) => {
                let snapshot = self
                    .workspace_db
                    .list_tasks(None)
                    .into_iter()
                    .find(|t| t.id == task_id);
                if self.workspace_db.delete_task(task_id) {
                    if let Some(task) = snapshot {
                        self.push_undo(UndoAction::RestoreDeletedTask(task));
                    }
                    self.status_banner =
                        format!("Deleted task #{task_id} (reversible via Undo / Ctrl+Z).");
                }
            }
            Message::ExecuteTaskInAgent { task_id, prompt } => {
                let _ = self
                    .workspace_db
                    .update_task_status(task_id, TaskStatus::InProgress);
                self.dispatch_chat_prompt(prompt);
            }
            // Projects
            Message::NewProjectNameChanged(v) => {
                self.new_project_name = v;
            }
            Message::NewProjectTagChanged(v) => {
                self.new_project_tag = v;
            }
            Message::NewProjectRootChanged(v) => {
                self.new_project_root = v;
            }
            Message::NewProjectDescChanged(v) => {
                self.new_project_desc = v;
            }
            Message::NewProjectPromptChanged(v) => {
                self.new_project_prompt = v;
            }
            Message::CreateWorkspaceProject => {
                let name = self.new_project_name.trim().to_string();
                if name.is_empty() {
                    self.status_banner = "Enter a project name before creating.".to_string();
                } else {
                    let tag = if self.new_project_tag.trim().is_empty() {
                        "PROJ".to_string()
                    } else {
                        self.new_project_tag.trim().to_ascii_uppercase()
                    };
                    let root = if self.new_project_root.trim().is_empty() {
                        self.workspace_root_input.clone()
                    } else {
                        self.new_project_root.trim().to_string()
                    };
                    let desc = if self.new_project_desc.trim().is_empty() {
                        "Scoped local agent workspace".to_string()
                    } else {
                        self.new_project_desc.trim().to_string()
                    };
                    if let Some(pid) = self.workspace_db.create_project(
                        &name,
                        &root,
                        &desc,
                        self.new_project_prompt.trim(),
                        &tag,
                    ) {
                        self.persist_project_chat();
                        self.projects_cache = self.workspace_db.list_projects();
                        self.active_project_id = pid;
                        self.workspace_db.save_active_project_id(pid);
                        self.workspace_root_input = root;
                        self.sync_project_chat_from_db();
                        self.refresh_workspace_caches();
                        self.new_project_name.clear();
                        self.new_project_tag.clear();
                        self.new_project_desc.clear();
                        self.new_project_prompt.clear();
                        self.status_banner =
                            format!("Created & activated project '{name}' [{tag}].");
                    } else {
                        self.status_banner = "Failed to create workspace project.".to_string();
                    }
                }
            }
            Message::SelectWorkspaceProject(pid) => {
                if pid != self.active_project_id {
                    if let Some(proj) = self.projects_cache.iter().find(|p| p.id == pid).cloned() {
                        self.persist_project_chat();
                        self.active_project_id = pid;
                        self.workspace_db.save_active_project_id(pid);
                        self.workspace_root_input = proj.root_path.clone();
                        self.sync_project_chat_from_db();
                        self.refresh_workspace_caches();
                        self.dashboard.trigger_transition();
                        self.status_banner =
                            format!("Active project: {} ({})", proj.name, proj.root_path);
                    }
                }
            }
            Message::DeleteWorkspaceProject(pid) => {
                if self.projects_cache.len() > 1 && self.workspace_db.delete_project(pid) {
                    self.projects_cache = self.workspace_db.list_projects();
                    if self.active_project_id == pid {
                        if let Some(first) = self.projects_cache.first().cloned() {
                            self.active_project_id = first.id;
                            self.workspace_db.save_active_project_id(first.id);
                            self.workspace_root_input = first.root_path;
                            self.sync_project_chat_from_db();
                            self.refresh_workspace_caches();
                        }
                    }
                    self.status_banner = "Removed workspace project.".to_string();
                }
            }
            // llama.cpp engine control panel
            Message::ResetLlamaConfig => {
                self.llama_config = LlamaRuntimeConfig::default();
                self.requested_context = self.llama_config.ctx_size;
                self.workspace_db.save_llama_config(&self.llama_config);
                self.status_banner =
                    "Reset llama.cpp configuration to balanced defaults.".to_string();
            }
            Message::LlamaGpuLayersChanged(ngl) => {
                self.llama_config.n_gpu_layers = ngl.min(99);
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaThreadsChanged(thr) => {
                self.llama_config.n_threads = thr.clamp(1, 64);
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaBatchChanged(batch) => {
                self.llama_config.n_batch = batch.clamp(64, 4096);
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaKvQuantChanged(kv) => {
                self.llama_config.kv_cache_type = kv;
                self.workspace_db.save_llama_config(&self.llama_config);
                self.status_banner = format!("llama.cpp KV cache precision: {}", kv.label());
            }
            Message::LlamaToggleFlashAttention(v) => {
                self.llama_config.flash_attention = v;
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaToggleKvOffload(v) => {
                self.llama_config.kv_offload = v;
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaToggleMmap(v) => {
                self.llama_config.mmap = v;
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaToggleMlock(v) => {
                self.llama_config.mlock = v;
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaTempChanged(t) => {
                self.llama_config.temperature_x100 = t.min(200);
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaTopPChanged(p) => {
                self.llama_config.top_p_x100 = p.clamp(10, 100);
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaTopKChanged(k) => {
                self.llama_config.top_k = k.clamp(1, 100);
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            Message::LlamaRepeatPenaltyChanged(rp) => {
                self.llama_config.repeat_penalty_x100 = rp.clamp(100, 150);
                self.workspace_db.save_llama_config(&self.llama_config);
            }
            // Infinite appearance & workspace studio
            Message::ToggleCustomTheme(enabled) => {
                self.dashboard.prefs.custom_theme.enabled = enabled;
                self.persist_dashboard_prefs();
                self.status_banner = if enabled {
                    "Activated custom Hue Matrix cyberpunk theme.".to_string()
                } else {
                    format!(
                        "Switched back to preset: {}",
                        self.dashboard.prefs.theme_preset.label()
                    )
                };
            }
            Message::CustomPrimaryHueChanged(h) => {
                self.dashboard.prefs.custom_theme.enabled = true;
                self.dashboard.prefs.custom_theme.primary_hue = h % 360;
                self.persist_dashboard_prefs();
            }
            Message::CustomSecondaryHueChanged(h) => {
                self.dashboard.prefs.custom_theme.enabled = true;
                self.dashboard.prefs.custom_theme.secondary_hue = h % 360;
                self.persist_dashboard_prefs();
            }
            Message::SelectSurfaceDepth(depth) => {
                self.dashboard.prefs.custom_theme.enabled = true;
                self.dashboard.prefs.custom_theme.surface_depth = depth;
                self.persist_dashboard_prefs();
            }
            Message::CornerRadiusChanged(r) => {
                self.dashboard.prefs.corner_radius_px = r.min(16);
                self.persist_dashboard_prefs();
            }
            Message::GlowStrengthChanged(g) => {
                self.dashboard.prefs.glow_strength_pct = g.min(100);
                self.persist_dashboard_prefs();
            }
            Message::SidebarWidthChanged(w) => {
                self.dashboard.prefs.sidebar_width_px = w.clamp(210, 300);
                self.persist_dashboard_prefs();
            }
            Message::ToggleIconRail(v) => {
                self.dashboard.prefs.show_icon_rail = v;
                self.persist_dashboard_prefs();
            }
            Message::ToggleStatusFooter(v) => {
                self.dashboard.prefs.show_status_footer = v;
                self.persist_dashboard_prefs();
            }
            Message::ToggleMonoChrome(v) => {
                self.dashboard.prefs.use_mono_chrome = v;
                self.persist_dashboard_prefs();
            }
            Message::ToggleCyberMotion(v) => {
                self.dashboard.prefs.cyber_motion = v;
                if v {
                    self.dashboard.trigger_pulse();
                } else {
                    self.dashboard.sync_motion_to_prefs();
                }
                self.persist_dashboard_prefs();
                self.status_banner = if v {
                    "Enabled smooth 60 FPS motion & cyberpunk scanline FX.".to_string()
                } else {
                    "Reduced Motion enabled (instant static transitions).".to_string()
                };
            }
            Message::RuntimeEvent(ev) => match ev {
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    if modifiers.command() {
                        if let keyboard::Key::Character(ref ch) = key {
                            if ch.as_str().eq_ignore_ascii_case("k") {
                                self.command_palette_open = !self.command_palette_open;
                                let reduced = !self.dashboard.prefs.cyber_motion;
                                self.dashboard.motion.spring_palette.set_target(
                                    if self.command_palette_open { 1.0 } else { 0.0 },
                                    reduced,
                                );
                                self.dashboard.trigger_pulse();
                                if self.command_palette_open {
                                    self.command_palette_query.clear();
                                    return operation::focus(iced::widget::Id::new(
                                        COMMAND_PALETTE_INPUT_ID,
                                    ));
                                }
                            } else if ch.as_str().eq_ignore_ascii_case("f")
                                && self.active_channel == ChannelTab::Chat
                            {
                                self.chat_search_open = !self.chat_search_open;
                                if self.chat_search_open {
                                    return operation::focus(iced::widget::Id::new(
                                        CHAT_SEARCH_INPUT_ID,
                                    ));
                                } else {
                                    self.chat_search_query.clear();
                                }
                            } else if ch.as_str().eq_ignore_ascii_case("z")
                                && !self.command_palette_open
                                && self.pending_consent.is_none()
                            {
                                return self.update(Message::UndoLastAction);
                            }
                        }
                    } else if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
                        if self.command_palette_open {
                            return self.update(Message::CloseCommandPalette);
                        } else if self.chat_search_open {
                            self.chat_search_open = false;
                            self.chat_search_query.clear();
                        } else if self.artifact_panel_open {
                            return self.update(Message::ToggleArtifactPanel);
                        }
                    }
                }
                Event::Window(window::Event::FileHovered(path)) => {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| path.to_string_lossy().to_string());
                    self.drag_hover_path = Some(name);
                }
                Event::Window(window::Event::FilesHoveredLeft) => {
                    self.drag_hover_path = None;
                }
                Event::Window(window::Event::FileDropped(path)) => {
                    self.drag_hover_path = None;
                    self.handle_dropped_or_staged_path(&path);
                }
                _ => {}
            },
            Message::OpenCommandPalette => {
                self.command_palette_open = true;
                self.command_palette_query.clear();
                let reduced = !self.dashboard.prefs.cyber_motion;
                self.dashboard
                    .motion
                    .spring_palette
                    .set_target(1.0, reduced);
                self.dashboard.trigger_pulse();
                return operation::focus(iced::widget::Id::new(COMMAND_PALETTE_INPUT_ID));
            }
            Message::CloseCommandPalette => {
                self.command_palette_open = false;
                let reduced = !self.dashboard.prefs.cyber_motion;
                self.dashboard
                    .motion
                    .spring_palette
                    .set_target(0.0, reduced);
            }
            Message::CommandPaletteQueryChanged(q) => {
                self.command_palette_query = q;
            }
            Message::RunCommandPaletteAction(inner) => {
                self.command_palette_open = false;
                self.command_palette_query.clear();
                let reduced = !self.dashboard.prefs.cyber_motion;
                self.dashboard
                    .motion
                    .spring_palette
                    .set_target(0.0, reduced);
                return self.update(*inner);
            }
            Message::TogglePauseRun => {
                if self.active_run_id.is_some() {
                    self.run_paused = !self.run_paused;
                    if !self.run_paused && !self.paused_delta_buffer.is_empty() {
                        let buffered = std::mem::take(&mut self.paused_delta_buffer);
                        if let Some(run_id) = &self.active_run_id {
                            if let Some(msg) =
                                self.messages.iter_mut().rev().find(|m| &m.id == run_id)
                            {
                                msg.append_delta(&buffered);
                            }
                        }
                        self.status_banner =
                            "Streaming resumed (buffered tokens flushed).".to_string();
                        if self.chat_auto_scroll && self.active_channel == ChannelTab::Chat {
                            return operation::snap_to_end(iced::widget::Id::new(CHAT_SCROLL_ID));
                        }
                    } else if self.run_paused {
                        self.status_banner =
                            "UI streaming paused — worker continues buffering tokens safely."
                                .to_string();
                    }
                }
            }
            Message::ToggleMessageThinking(msg_id) => {
                if let Some(m) = self.messages.iter_mut().find(|m| m.id == msg_id) {
                    m.show_thinking = !m.show_thinking;
                }
            }
            Message::ToggleMessageTrace(msg_id) => {
                if let Some(m) = self.messages.iter_mut().find(|m| m.id == msg_id) {
                    m.show_trace = !m.show_trace;
                }
            }
            Message::EditPromptMessage(msg_id) => {
                if let Some(m) = self.messages.iter().find(|m| m.id == msg_id && m.is_user) {
                    let clean_prompt = m
                        .raw_text
                        .split("\n\nAttachments:")
                        .next()
                        .unwrap_or(&m.raw_text)
                        .to_string();
                    self.composer_input = clean_prompt;
                    if self.active_run_id.is_some() {
                        if let Some(flag) = &self.active_cancel_flag {
                            flag.store(true, Ordering::SeqCst);
                        }
                        self.run_paused = false;
                    }
                    self.active_channel = ChannelTab::Chat;
                    self.status_banner =
                        "Loaded prompt back into composer for revision & re-run.".to_string();
                }
            }
            Message::ClearProjectChat => {
                let rows: Vec<PersistedChatRow> = self
                    .messages
                    .iter()
                    .filter(|m| !m.is_streaming && m.id != "welcome-0")
                    .map(ChatMessageEntry::to_persisted)
                    .collect();
                if !rows.is_empty() {
                    self.push_undo(UndoAction::RestoreProjectChat {
                        project_id: self.active_project_id,
                        rows,
                    });
                }
                self.messages = vec![Self::welcome_message()];
                self.persist_project_chat();
                self.status_banner =
                    "Cleared active project chat (reversible via Undo / Ctrl+Z).".to_string();
            }
            Message::UndoLastAction => {
                if let Some(action) = self.undo_stack.pop() {
                    match action {
                        UndoAction::RestoreDeletedTask(task) => {
                            if let Some(new_id) = self.workspace_db.create_task_with_kind(
                                task.project_id,
                                &task.title,
                                &task.notes,
                                task.priority,
                                task.task_kind,
                                &task.assignee,
                            ) {
                                let _ = self.workspace_db.update_task_status(new_id, task.status);
                                self.status_banner =
                                    format!("Restored task '{}' (#{new_id}).", task.title);
                            }
                        }
                        UndoAction::RestoreTaskStatus { task_id, previous } => {
                            if self.workspace_db.update_task_status(task_id, previous) {
                                self.status_banner = format!(
                                    "Reverted task #{task_id} status to {}.",
                                    previous.label()
                                );
                            }
                        }
                        UndoAction::RestoreProjectChat { project_id, rows } => {
                            self.workspace_db.replace_project_chat(project_id, &rows);
                            if project_id == self.active_project_id {
                                self.sync_project_chat_from_db();
                            }
                            self.status_banner =
                                format!("Restored {} messages to project chat.", rows.len());
                        }
                        UndoAction::RestoreTheme { preset, custom } => {
                            self.dashboard.prefs.theme_preset = preset;
                            self.dashboard.prefs.custom_theme = custom;
                            self.dashboard.trigger_pulse();
                            self.persist_dashboard_prefs();
                            self.status_banner =
                                format!("Reverted cyber theme to {}.", preset.label());
                        }
                    }
                } else {
                    self.status_banner = "Nothing to undo in local history stack.".to_string();
                }
            }
            Message::ToggleChatSearch => {
                self.chat_search_open = !self.chat_search_open;
                if self.chat_search_open {
                    return operation::focus(iced::widget::Id::new(CHAT_SEARCH_INPUT_ID));
                } else {
                    self.chat_search_query.clear();
                }
            }
            Message::ChatSearchChanged(q) => {
                self.chat_search_query = q;
            }
            Message::ChatScrolled(at_bottom) => {
                self.chat_auto_scroll = at_bottom;
            }
            Message::JumpToChatBottom => {
                self.chat_auto_scroll = true;
                return operation::snap_to_end(iced::widget::Id::new(CHAT_SCROLL_ID));
            }
            Message::PasteClipboardPressed => {
                return clipboard::read().map(Message::ClipboardLoaded);
            }
            Message::ClipboardLoaded(maybe_txt) => {
                if let Some(txt) = maybe_txt.filter(|s| !s.trim().is_empty()) {
                    if let Some(att) = ux_studio::stage_clipboard_text(&txt) {
                        self.status_banner =
                            format!("Staged clipboard snippet ({} bytes) for #chat.", att.bytes);
                        self.staged_attachments.push(att);
                    }
                } else {
                    self.status_banner = "Clipboard has no text snippet to stage.".to_string();
                }
            }
            Message::RemoveStagedAttachment(idx) => {
                if idx < self.staged_attachments.len() {
                    let removed = self.staged_attachments.remove(idx);
                    self.status_banner = format!("Removed staged attachment '{}'.", removed.label);
                }
            }
            Message::CopyTextToClipboard(txt) => {
                self.status_banner = format!("Copied {} bytes to system clipboard.", txt.len());
                return clipboard::write(txt);
            }
            Message::ToggleArtifactPanel => {
                self.artifact_panel_open = !self.artifact_panel_open;
                let reduced = !self.dashboard.prefs.cyber_motion;
                self.dashboard
                    .motion
                    .spring_artifacts
                    .set_target(if self.artifact_panel_open { 1.0 } else { 0.0 }, reduced);
                self.dashboard.trigger_pulse();
            }
            Message::SelectArtifactTab(idx) => {
                self.selected_artifact_idx = idx;
            }
            Message::SelectPersona(persona) => {
                self.persona = persona;
                self.workspace_db.save_persona(persona);
                self.status_banner = format!("Agent persona active: {}", persona.label());
            }
            Message::ToggleNativeNotifications(enabled) => {
                self.dashboard.prefs.native_notifications = enabled;
                self.persist_dashboard_prefs();
                self.status_banner = if enabled {
                    "Native desktop completion notifications enabled.".to_string()
                } else {
                    "Native desktop notifications silenced.".to_string()
                };
            }
            Message::PokeMiniBot => {
                self.bot_poke_count = self.bot_poke_count.saturating_add(1);
                let open_tasks = self
                    .workspace_db
                    .list_tasks(Some(self.active_project_id))
                    .into_iter()
                    .filter(|t| t.status != TaskStatus::Done)
                    .count();
                let quip = ux_studio::next_bot_quip(
                    self.bot_poke_count,
                    self.agent_phase(),
                    self.active_project_name(),
                    open_tasks,
                );
                self.bot_quip = quip.clone();
                if self.dashboard.prefs.cyber_motion {
                    self.dashboard.motion.bot_bounce.value = 1.0;
                    self.dashboard.motion.bot_bounce.velocity = 1.6;
                    self.dashboard.motion.bot_bounce.set_target(0.0, false);
                    self.dashboard.trigger_pulse();
                } else {
                    self.dashboard.motion.bot_bounce.snap(0.0);
                }
                self.status_banner = format!("Lora Bot: {quip}");
            }
            Message::ToggleMiniBot(enabled) => {
                self.dashboard.prefs.show_mini_bot = enabled;
                self.persist_dashboard_prefs();
                self.status_banner = if enabled {
                    "Enabled animated Capri × Lora Mini-Bot companion.".to_string()
                } else {
                    "Hid Capri × Lora Mini-Bot companion for minimal chrome.".to_string()
                };
            }
            Message::SelectAgentKind(kind) => {
                self.agent_kind = kind;
                self.workspace_db.save_agent_kind(kind);
                self.status_banner =
                    format!("Agent type set to: {} [{}]", kind.label(), kind.code());
            }
            Message::SelectReasoningEffort(effort) => {
                self.reasoning_effort = effort;
                self.workspace_db.save_reasoning_effort(effort);
                self.status_banner = format!(
                    "Reasoning effort: {} (~{} tok reserve)",
                    effort.label(),
                    effort.thinking_reserve_tokens()
                );
            }
            Message::SelectAccessMode(mode) => {
                self.access_mode = mode;
                self.workspace_db.save_access_mode(mode);
                self.status_banner = match mode {
                    ExecutionAccessMode::Local => {
                        "Execution access: LOCAL AIR-GAPPED (llama.cpp engine)".to_string()
                    }
                    ExecutionAccessMode::Api => format!(
                        "Execution access: API ROUTE ({} via env '{}')",
                        self.api_config.model_name, self.api_config.api_key_env
                    ),
                };
            }
            Message::ApiBaseUrlChanged(val) => {
                self.api_config.base_url = val;
                self.workspace_db.save_api_config(&self.api_config);
            }
            Message::ApiModelNameChanged(val) => {
                self.api_config.model_name = val;
                self.workspace_db.save_api_config(&self.api_config);
            }
            Message::ApiKeyEnvChanged(val) => {
                self.api_config.api_key_env = val;
                self.workspace_db.save_api_config(&self.api_config);
            }
            Message::NewTaskKindSelected(kind) => {
                self.new_task_kind = kind;
            }
            Message::SummarizeDailyTasksInAgent => {
                let tasks = self.workspace_db.list_tasks(Some(self.active_project_id));
                let daily_done: Vec<String> = tasks
                    .iter()
                    .filter(|t| t.task_kind == TaskKind::Daily && t.status == TaskStatus::Done)
                    .map(|t| format!("- [DONE] {}", t.title))
                    .collect();
                let daily_open: Vec<String> = tasks
                    .iter()
                    .filter(|t| t.task_kind == TaskKind::Daily && t.status != TaskStatus::Done)
                    .map(|t| format!("- [{}] {}", t.status.label(), t.title))
                    .collect();
                let prompt = format!(
                    "Summarize today's Daily tasks for project '{}':\nCompleted ({}):\n{}\n\nRemaining ({}):\n{}\n\nGive a crisp tactical plan and next top 3 priorities.",
                    self.active_project_name(),
                    daily_done.len(),
                    if daily_done.is_empty() {
                        "- None yet".to_string()
                    } else {
                        daily_done.join("\n")
                    },
                    daily_open.len(),
                    if daily_open.is_empty() {
                        "- All daily items complete!".to_string()
                    } else {
                        daily_open.join("\n")
                    }
                );
                self.agent_kind = AgentKind::Planner;
                self.workspace_db.save_agent_kind(AgentKind::Planner);
                self.active_channel = ChannelTab::Chat;
                self.active_guild = WorkspaceGuild::ScopedDocs;
                self.dispatch_chat_prompt(prompt);
            }
            Message::AttachPathChanged(val) => {
                self.attach_path_input = val;
            }
            Message::AttachPathSubmit => {
                let raw = self.attach_path_input.trim().trim_matches('"').to_string();
                if raw.is_empty() {
                    self.status_banner =
                        "Enter or drop a file, folder, or .env path first.".to_string();
                } else {
                    let candidate = {
                        let p = std::path::PathBuf::from(&raw);
                        if p.is_absolute() || p.exists() {
                            p
                        } else {
                            std::path::Path::new(&self.workspace_root_input).join(&raw)
                        }
                    };
                    if candidate.exists() {
                        self.attach_path_input.clear();
                        self.handle_dropped_or_staged_path(&candidate);
                    } else {
                        self.status_banner = format!("Path not found: '{}'", candidate.display());
                    }
                }
            }
            Message::QuickAttachOrTrustPath(raw) => {
                let path = std::path::PathBuf::from(&raw);
                if path.exists() {
                    self.handle_dropped_or_staged_path(&path);
                } else {
                    self.status_banner = format!("Path not found: '{raw}'");
                }
            }
            Message::NewTrustPathChanged(val) => {
                self.new_trust_path = val;
            }
            Message::NewTrustLevelSelected(level) => {
                self.new_trust_level = level;
            }
            Message::AddTrustedFolder => {
                let folder = self.new_trust_path.trim().trim_matches('"').to_string();
                if folder.is_empty() {
                    self.status_banner = "Enter a folder path to grant trust access.".to_string();
                } else if self
                    .workspace_db
                    .upsert_trusted_folder(self.active_project_id, &folder, self.new_trust_level)
                    .is_some()
                {
                    self.new_trust_path.clear();
                    self.refresh_workspace_caches();
                    self.status_banner = format!(
                        "Trusted folder '{}' set to {}.",
                        folder,
                        self.new_trust_level.label()
                    );
                } else {
                    self.status_banner = "Failed to save trusted folder rule.".to_string();
                }
            }
            Message::TrustActiveWorkspaceRoot => {
                let root = self.workspace_root_input.trim().to_string();
                if !root.is_empty() {
                    let _ = self.workspace_db.upsert_trusted_folder(
                        self.active_project_id,
                        &root,
                        FolderAccessLevel::Trusted,
                    );
                    self.refresh_workspace_caches();
                    self.status_banner =
                        format!("Workspace root '{}' promoted to FULL TRUSTED.", root);
                }
            }
            Message::CycleTrustedFolderLevel {
                folder_id,
                next_level,
            } => {
                if self
                    .workspace_db
                    .update_trusted_folder_level(folder_id, next_level)
                {
                    self.refresh_workspace_caches();
                    self.status_banner =
                        format!("Folder access level updated to {}.", next_level.label());
                }
            }
            Message::DeleteTrustedFolder(folder_id) => {
                if self.workspace_db.delete_trusted_folder(folder_id) {
                    self.refresh_workspace_caches();
                    self.status_banner = "Removed trusted folder policy entry.".to_string();
                }
            }
            Message::NewEnvKeyChanged(k) => {
                self.new_env_key = k;
            }
            Message::NewEnvValChanged(v) => {
                self.new_env_val = v;
            }
            Message::NewEnvSecretToggled(secret) => {
                self.new_env_secret = secret;
            }
            Message::ToggleEnvScopeGlobal => {
                self.env_scope_global = !self.env_scope_global;
                self.status_banner = if self.env_scope_global {
                    "Env scope set to GLOBAL (shared across all projects).".to_string()
                } else {
                    format!(
                        "Env scope set to PROJECT ('{}').",
                        self.active_project_name()
                    )
                };
            }
            Message::SaveEnvVar => {
                let key = self.new_env_key.trim().to_string();
                if key.is_empty() {
                    self.status_banner =
                        "Enter an env variable KEY first (e.g. OPENAI_API_KEY).".to_string();
                } else {
                    let pid = if self.env_scope_global {
                        0
                    } else {
                        self.active_project_id
                    };
                    if self
                        .workspace_db
                        .upsert_env_var(pid, &key, &self.new_env_val, self.new_env_secret)
                        .is_some()
                    {
                        self.new_env_key.clear();
                        self.new_env_val.clear();
                        self.refresh_workspace_caches();
                        self.status_banner =
                            format!("Saved env variable '{key}' to SQLite .env store.");
                    } else {
                        self.status_banner = "Failed to save env variable.".to_string();
                    }
                }
            }
            Message::ToggleRevealSecret(env_id) => {
                if let Some(pos) = self.reveal_secret_ids.iter().position(|id| *id == env_id) {
                    self.reveal_secret_ids.remove(pos);
                } else {
                    self.reveal_secret_ids.push(env_id);
                }
            }
            Message::ToggleEnvVarEnabled { env_id, enabled } => {
                if self.workspace_db.toggle_env_var(env_id, enabled) {
                    self.refresh_workspace_caches();
                    self.status_banner = format!(
                        "Env variable #{env_id} {}.",
                        if enabled { "enabled" } else { "muted" }
                    );
                }
            }
            Message::DeleteEnvVar(env_id) => {
                self.reveal_secret_ids.retain(|id| *id != env_id);
                if self.workspace_db.delete_env_var(env_id) {
                    self.refresh_workspace_caches();
                    self.status_banner = format!("Deleted env variable #{env_id}.");
                }
            }
            Message::ImportWorkspaceDotenv => {
                let dotenv_path = std::path::Path::new(&self.workspace_root_input).join(".env");
                match std::fs::read_to_string(&dotenv_path) {
                    Ok(raw) => {
                        let pid = if self.env_scope_global {
                            0
                        } else {
                            self.active_project_id
                        };
                        let count = self.workspace_db.import_dotenv_text(pid, &raw);
                        self.refresh_workspace_caches();
                        self.status_banner = format!(
                            "Imported {count} variables from '{}'.",
                            dotenv_path.display()
                        );
                    }
                    Err(e) => {
                        self.status_banner = format!(
                            "No readable .env at '{}' ({e}). You can drag & drop any .env file anywhere.",
                            dotenv_path.display()
                        );
                    }
                }
            }
            Message::CapriAuraPresetChanged(preset) => {
                self.dashboard.prefs.capri.aura_preset = preset;
                match preset {
                    CapriAuraPreset::ClassicGlitch => {
                        self.dashboard.prefs.capri.crown_hue = 344;
                        self.dashboard.prefs.capri.visor_hue = 136;
                    }
                    CapriAuraPreset::CyberGold => {
                        self.dashboard.prefs.capri.crown_hue = 42;
                        self.dashboard.prefs.capri.visor_hue = 108;
                    }
                    CapriAuraPreset::PhantomViolet => {
                        self.dashboard.prefs.capri.crown_hue = 278;
                        self.dashboard.prefs.capri.visor_hue = 162;
                    }
                    CapriAuraPreset::IceReactor => {
                        self.dashboard.prefs.capri.crown_hue = 196;
                        self.dashboard.prefs.capri.visor_hue = 148;
                    }
                    CapriAuraPreset::CustomHue => {}
                }
                self.dashboard.trigger_pulse();
                self.persist_dashboard_prefs();
                self.status_banner = format!("Capri aura preset: {}", preset.label());
            }
            Message::CapriCrownHueChanged(h) => {
                self.dashboard.prefs.capri.aura_preset = CapriAuraPreset::CustomHue;
                self.dashboard.prefs.capri.crown_hue = h % 360;
                self.persist_dashboard_prefs();
            }
            Message::CapriVisorHueChanged(h) => {
                self.dashboard.prefs.capri.aura_preset = CapriAuraPreset::CustomHue;
                self.dashboard.prefs.capri.visor_hue = h % 360;
                self.persist_dashboard_prefs();
            }
            Message::CapriGlitchChanged(g) => {
                self.dashboard.prefs.capri.glitch_intensity_pct = g.min(100);
                self.persist_dashboard_prefs();
            }
            Message::CapriScaleChanged(s) => {
                self.dashboard.prefs.capri.widget_scale_px = s.clamp(40, 96);
                self.persist_dashboard_prefs();
            }
            Message::CapriEyeModeChanged(eye_mode) => {
                self.dashboard.prefs.capri.eye_mode = eye_mode;
                self.dashboard.trigger_pulse();
                self.persist_dashboard_prefs();
            }
            Message::CapriToggleCrown(v) => {
                self.dashboard.prefs.capri.show_aura_shards = v;
                self.persist_dashboard_prefs();
            }
            Message::CapriToggleLightning(v) => {
                self.dashboard.prefs.capri.show_lightning = v;
                self.persist_dashboard_prefs();
            }
            Message::CapriToggleGrin(v) => {
                self.dashboard.prefs.capri.always_toothy_grin = v;
                self.persist_dashboard_prefs();
            }
            Message::CapriToggleDockInChat(v) => {
                self.dashboard.prefs.capri.dock_widget_in_chat = v;
                self.persist_dashboard_prefs();
            }
            Message::CapriToggleHeaderChip(v) => {
                self.dashboard.prefs.capri.floating_badge_chip = v;
                self.persist_dashboard_prefs();
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let mut columns = Row::new();
        if self.dashboard.animated_rail_width() > 1.5 {
            columns = columns.push(self.view_guild_rail());
        }
        columns = columns
            .push(self.view_channel_sidebar())
            .push(self.view_main_stage());

        let inspector_ratio = if self.dashboard.prefs.cyber_motion {
            self.dashboard.motion.spring_inspector.value
        } else if self.show_inspector {
            1.0
        } else {
            0.0
        };
        if self.show_inspector || inspector_ratio > 0.02 {
            columns = columns.push(self.view_telemetry_inspector());
        }

        let shell: Element<'_, Message> =
            container(columns.height(Length::Fill).width(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::palette_surface_container(
                    pal.bg_stage,
                    pal.text_primary,
                ))
                .into();

        if let Some(prompt) = &self.pending_consent {
            if self.command_palette_open {
                stack![
                    shell,
                    ux_studio::view_command_palette(self),
                    self.view_consent_modal(prompt)
                ]
                .into()
            } else {
                stack![shell, self.view_consent_modal(prompt)].into()
            }
        } else if self.command_palette_open {
            stack![shell, ux_studio::view_command_palette(self)].into()
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
        rail = rail.push(rule::horizontal(1).style(theme::palette_divider_rule(pal)));

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
            let glyph_color = if active {
                pal.on_accent_text
            } else {
                pal.text_secondary
            };
            let btn = button(
                container(
                    column![
                        canvas(GuildIconCanvas {
                            guild,
                            color: glyph_color,
                            active,
                            phase: self.dashboard.motion.phase,
                            motion_enabled: self.dashboard.prefs.cyber_motion,
                        })
                        .width(Length::Fixed(22.0))
                        .height(Length::Fixed(22.0)),
                        text(guild.badge_text())
                            .size(9)
                            .font(Font::MONOSPACE)
                            .color(glyph_color),
                    ]
                    .spacing(2)
                    .align_x(Alignment::Center),
                )
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

        rail = rail.push(rule::horizontal(1).style(theme::palette_divider_rule(pal)));
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
            Color {
                a: 0.84,
                ..pal.critical
            },
            pal.on_accent_text,
            self.dashboard.control_radius(),
        ))
        .on_press(Message::StopActiveTask);

        rail = rail.push(stop_btn);

        container(rail)
            .width(Length::Fixed(self.dashboard.animated_rail_width()))
            .height(Length::Fill)
            .style(theme::palette_surface_container(
                pal.bg_rail,
                pal.text_primary,
            ))
            .into()
    }

    fn view_channel_sidebar(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let ui_font = self.dashboard.ui_font();
        let c_rad = self.dashboard.control_radius();
        let theme_code = self.dashboard.active_theme_code();
        let active_proj = self
            .projects_cache
            .iter()
            .find(|p| p.id == self.active_project_id);
        let active_proj_name = active_proj.map(|p| p.name.as_str()).unwrap_or("Lora Core");
        let active_proj_tag = active_proj.map(|p| p.accent_tag.as_str()).unwrap_or("CORE");
        let open_tasks = self
            .workspace_db
            .list_tasks(Some(self.active_project_id))
            .into_iter()
            .filter(|t| t.status != TaskStatus::Done)
            .count();

        let header = container(
            row![
                image(&self.logo_handle)
                    .width(Length::Fixed(24.0))
                    .height(Length::Fixed(24.0)),
                column![
                    text("Lora Agent")
                        .size(15)
                        .font(ui_font)
                        .color(pal.text_primary),
                    text(theme_code)
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(pal.text_muted),
                ]
                .spacing(1)
                .width(Length::Fill),
                button(
                    text(if self.dashboard.prefs.show_icon_rail {
                        "Focus"
                    } else {
                        "Rail"
                    })
                    .size(10)
                )
                .padding(Padding::from([3, 7]))
                .style(theme::cyber_chip_button_with_radius(
                    !self.dashboard.prefs.show_icon_rail,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::ToggleIconRail(
                    !self.dashboard.prefs.show_icon_rail
                )),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([10, 12]))
        .width(Length::Fill);

        // Modern AI agent active project selector card (+ New Project shortcut)
        let mut proj_pills = Row::new().spacing(5).align_y(Alignment::Center);
        for p in self.projects_cache.iter().take(4) {
            let active = p.id == self.active_project_id;
            proj_pills = proj_pills.push(
                button(text(&p.accent_tag).size(10).font(Font::MONOSPACE))
                    .padding(Padding::from([3, 7]))
                    .style(theme::cyber_chip_button_with_radius(
                        active,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_muted,
                        c_rad,
                    ))
                    .on_press(Message::SelectWorkspaceProject(p.id)),
            );
        }

        let project_card = container(
            column![
                row![
                    text("ACTIVE PROJECT")
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(pal.text_muted)
                        .width(Length::Fill),
                    button(text("+ New").size(10))
                        .padding(Padding::from([2, 8]))
                        .style(theme::cyber_chip_button_with_radius(
                            false,
                            pal.accent_secondary,
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.accent_secondary,
                            c_rad,
                        ))
                        .on_press(Message::SelectChannel(ChannelTab::AgentWorkspace)),
                ]
                .align_y(Alignment::Center),
                row![
                    container(text(active_proj_tag).size(10).font(Font::MONOSPACE))
                        .padding(Padding::from([2, 6]))
                        .style(theme::pill_container(
                            pal.accent_primary_soft,
                            pal.accent_primary,
                            pal.accent_primary,
                        )),
                    text(active_proj_name)
                        .size(12)
                        .font(ui_font)
                        .color(pal.text_primary),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                proj_pills,
            ]
            .spacing(6),
        )
        .padding(10)
        .width(Length::Fill)
        .style(theme::palette_card_container(
            pal.bg_card,
            pal.text_primary,
            pal.border_subtle,
            c_rad.max(6.0),
        ));

        let mut list = Column::new().spacing(4).padding(Padding::from([8, 8]));
        list = list.push(project_card);
        list = list.push(space::Space::new().height(8));
        list = list.push(
            text("AGENT WORKSPACE")
                .size(10)
                .font(Font::MONOSPACE)
                .color(pal.text_muted),
        );
        for ch in ChannelTab::CORE_CHANNELS {
            let badge = (ch == ChannelTab::WorkflowPipeline && open_tasks > 0)
                .then(|| open_tasks.to_string());
            list = list.push(self.channel_row_btn(ch, badge));
        }

        list = list.push(space::Space::new().height(10));
        list = list.push(
            text("TRUST & GOVERNANCE")
                .size(10)
                .font(Font::MONOSPACE)
                .color(pal.text_muted),
        );
        for ch in ChannelTab::SECURITY_CHANNELS {
            list = list.push(self.channel_row_btn(ch, None));
        }

        if self.experimental_enabled {
            list = list.push(space::Space::new().height(10));
            list = list.push(
                text("EXPERIMENTAL LAB")
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(pal.warning),
            );
            for ch in ChannelTab::EXPERIMENTAL_CHANNELS {
                list = list.push(self.channel_row_btn(ch, None));
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
                        if busy {
                            Color {
                                a: 0.18,
                                ..pal.warning
                            }
                        } else {
                            pal.nominal_soft
                        },
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
                        c_rad,
                    ))
                    .on_press(Message::RefreshTelemetry),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(10)
        .width(Length::Fill)
        .style(theme::palette_surface_container(
            pal.bg_dock,
            pal.text_primary,
        ));

        let mut sidebar_col = column![
            header,
            rule::horizontal(1).style(theme::palette_divider_rule(pal)),
            scrollable(list).height(Length::Fill),
        ];

        if self.dashboard.prefs.show_mini_bot {
            let phase = self.agent_phase();
            let bot_card = button(
                row![
                    canvas(ux_studio::LoraMiniBotCanvas {
                        phase,
                        clock: self.dashboard.motion.phase,
                        bounce: self.dashboard.motion.bot_bounce.value,
                        palette: pal,
                        motion_enabled: self.dashboard.prefs.cyber_motion,
                        style: self.dashboard.prefs.capri,
                    })
                    .width(Length::Fixed(52.0))
                    .height(Length::Fixed(52.0)),
                    column![
                        row![
                            text("CAPRI · LORA")
                                .size(9)
                                .font(Font::MONOSPACE)
                                .color(pal.accent_primary)
                                .width(Length::Fill),
                            text(phase.label())
                                .size(9)
                                .font(Font::MONOSPACE)
                                .color(pal.accent_secondary),
                        ]
                        .align_y(Alignment::Center),
                        text(self.bot_quip.as_str())
                            .size(10)
                            .color(pal.text_secondary),
                    ]
                    .spacing(3)
                    .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([8, 10]))
            .width(Length::Fill)
            .style(theme::palette_channel_button(
                self.active_run_id.is_some(),
                pal.bg_elevated,
                pal.bg_card,
                pal.border_accent,
                pal.text_primary,
                pal.text_secondary,
                pal.text_muted,
            ))
            .on_press(Message::PokeMiniBot);

            sidebar_col = sidebar_col
                .push(rule::horizontal(1).style(theme::palette_divider_rule(pal)))
                .push(
                    container(bot_card)
                        .padding(Padding::from([6, 8]))
                        .width(Length::Fill),
                );
        }

        sidebar_col = sidebar_col
            .push(rule::horizontal(1).style(theme::palette_divider_rule(pal)))
            .push(user_dock)
            .height(Length::Fill);

        container(sidebar_col)
            .width(Length::Fixed(self.dashboard.animated_sidebar_width()))
            .height(Length::Fill)
            .style(theme::palette_surface_container(
                pal.bg_sidebar,
                pal.text_primary,
            ))
            .into()
    }

    fn channel_row_btn(&self, ch: ChannelTab, badge: Option<String>) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let ui_font = self.dashboard.ui_font();
        let active = self.active_channel == ch;
        let indicator = container(space::Space::new())
            .width(Length::Fixed(3.0))
            .height(Length::Fixed(15.0))
            .style(theme::palette_surface_container(
                if active {
                    pal.accent_primary
                } else {
                    pal.bg_sidebar
                },
                pal.text_primary,
            ));

        let mut content = row![
            indicator,
            text(ch.slug()).size(13).font(ui_font).width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        if let Some(count) = badge {
            content = content.push(
                container(text(count).size(10).font(Font::MONOSPACE))
                    .padding(Padding::from([1, 6]))
                    .style(theme::pill_container(
                        pal.accent_secondary_soft,
                        pal.accent_secondary,
                        pal.border_subtle,
                    )),
            );
        }

        button(content)
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
        let ui_font = self.dashboard.ui_font();
        let proj_pill = container(
            text(self.active_project_name())
                .size(11)
                .font(Font::MONOSPACE),
        )
        .padding(Padding::from([4, 9]))
        .style(theme::pill_container(
            pal.accent_primary_soft,
            pal.accent_primary,
            pal.accent_primary,
        ));

        let model_pill =
            container(text(self.selected_model.as_deref().unwrap_or("No Active Model")).size(11))
                .padding(Padding::from([4, 10]))
                .style(theme::pill_container(
                    pal.accent_secondary_soft,
                    pal.accent_secondary,
                    pal.border_accent,
                ));

        let airgap_pill = match self.access_mode {
            ExecutionAccessMode::Local => container(text("AIR-GAPPED").size(11))
                .padding(Padding::from([4, 9]))
                .style(theme::pill_container(
                    pal.nominal_soft,
                    pal.nominal,
                    pal.nominal,
                )),
            ExecutionAccessMode::Api => container(text("API ROUTE").size(11))
                .padding(Padding::from([4, 9]))
                .style(theme::pill_container(
                    Color {
                        a: 0.16,
                        ..pal.warning
                    },
                    pal.warning,
                    pal.warning,
                )),
        };

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
                Color {
                    a: 0.84,
                    ..pal.critical
                },
                pal.on_accent_text,
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

        let open_tasks = self
            .workspace_db
            .list_tasks(Some(self.active_project_id))
            .into_iter()
            .filter(|t| t.status != TaskStatus::Done)
            .count();
        let zeigarnik_btn = button(
            text(format!("{open_tasks} Open Tasks"))
                .size(11)
                .font(Font::MONOSPACE),
        )
        .padding(Padding::from([4, 9]))
        .style(theme::cyber_chip_button_with_radius(
            open_tasks > 0,
            pal.warning,
            pal.bg_elevated,
            pal.border_subtle,
            pal.text_primary,
            pal.text_muted,
            self.dashboard.control_radius(),
        ))
        .on_press(Message::SelectChannel(ChannelTab::WorkflowPipeline));

        let phase = self.agent_phase();
        let phase_color = match phase {
            AgentPhase::Idle => pal.text_muted,
            AgentPhase::Thinking | AgentPhase::Paused => pal.warning,
            AgentPhase::Streaming => pal.accent_secondary,
            AgentPhase::Verified => pal.nominal,
            AgentPhase::Error => pal.critical,
        };
        let phase_pill = container(
            row![
                canvas(MorphingPhaseCanvas {
                    phase,
                    clock: self.dashboard.motion.phase,
                    color: phase_color,
                    motion_enabled: self.dashboard.prefs.cyber_motion,
                })
                .width(Length::Fixed(18.0))
                .height(Length::Fixed(12.0)),
                text(phase.label())
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(phase_color),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([4, 9]))
        .style(theme::pill_container(
            pal.bg_elevated,
            phase_color,
            pal.border_subtle,
        ));

        let palette_btn = button(text("Ctrl+K").size(11).font(Font::MONOSPACE))
            .padding(Padding::from([5, 9]))
            .style(theme::solid_button_with_radius(
                pal.bg_elevated,
                pal.accent_secondary,
                pal.accent_secondary,
                self.dashboard.control_radius(),
            ))
            .on_press(Message::OpenCommandPalette);

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

        let mut header_row = row![column![
            text(self.active_channel.slug())
                .size(15)
                .font(ui_font)
                .color(pal.text_primary),
            text(self.active_channel.topic())
                .size(11)
                .color(pal.text_muted),
        ]
        .spacing(1)
        .width(Length::Fill),]
        .spacing(8)
        .align_y(Alignment::Center);

        if self.dashboard.prefs.show_mini_bot && self.dashboard.prefs.capri.floating_badge_chip {
            header_row = header_row.push(ux_studio::view_mini_bot_avatar(self, 28.0, None));
        }

        header_row = header_row
            .push(phase_pill)
            .push(zeigarnik_btn)
            .push(proj_pill)
            .push(airgap_pill)
            .push(model_pill)
            .push(palette_btn)
            .push(stop_btn)
            .push(inspector_btn);

        container(header_row)
            .padding(Padding::from([8, 16]))
            .width(Length::Fill)
            .style(theme::palette_surface_container(
                pal.bg_stage,
                pal.text_primary,
            ))
            .into()
    }

    fn view_main_stage(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let body: Element<'_, Message> = match self.active_channel {
            ChannelTab::Dashboard => self.view_dashboard_channel(),
            ChannelTab::Chat => self.view_chat_channel(),
            ChannelTab::CodeWorkspace => agent_views::view_code_env_channel(self),
            ChannelTab::AgentWorkspace => agent_views::view_projects_channel(self),
            ChannelTab::ModelLibrary => self.view_models_channel(),
            ChannelTab::Hardware => agent_views::view_llamacpp_channel(self),
            ChannelTab::WorkflowPipeline => agent_views::view_tasks_db_channel(self),
            ChannelTab::AuditJournal => self.view_audit_channel(),
            ChannelTab::BrokerGate => self.view_broker_channel(),
            ChannelTab::AdapterStudio => self.view_adapters_channel(),
            ChannelTab::ExtensionManager => self.view_extensions_channel(),
            ChannelTab::Settings => agent_views::view_studio_settings_channel(self),
        };

        let scan_strip = canvas(dashboard::CyberScanStripCanvas {
            primary: pal.accent_primary,
            secondary: pal.accent_secondary,
            rail: pal.border_subtle,
            phase: self.dashboard.motion.phase,
            sweep: self.dashboard.motion.channel_sweep,
            active_run: self.active_run_id.is_some(),
            motion_enabled: self.dashboard.prefs.cyber_motion,
        })
        .width(Length::Fill)
        .height(Length::Fixed(3.0));

        let mut col = column![self.view_top_header(), scan_strip];

        if let Some(hover_name) = &self.drag_hover_path {
            col = col.push(
                container(
                    row![
                        text("DROP TARGET READY")
                            .size(11)
                            .font(Font::MONOSPACE)
                            .color(pal.accent_secondary),
                        text(format!(
                            "Release '{hover_name}' -> file attaches to #chat · folder adds to Trusted Folders · .env imports secrets · .gguf stages local model"
                        ))
                        .size(11)
                        .color(pal.text_primary),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([8, 16]))
                .width(Length::Fill)
                .style(theme::palette_card_container(
                    pal.accent_secondary_soft,
                    pal.text_primary,
                    pal.accent_secondary,
                    0.0,
                )),
            );
        }

        col = col
            .push(container(body).width(Length::Fill).height(Length::Fill))
            .height(Length::Fill)
            .width(Length::Fill);

        if self.dashboard.prefs.show_status_footer {
            let mut footer_row = row![
                text(self.status_banner.as_str())
                    .size(11)
                    .color(pal.text_muted),
                space::horizontal(),
            ]
            .spacing(10)
            .align_y(Alignment::Center);

            if !self.undo_stack.is_empty() {
                footer_row = footer_row.push(
                    button(
                        text(format!("Undo ({}) Ctrl+Z", self.undo_stack.len()))
                            .size(10)
                            .font(Font::MONOSPACE),
                    )
                    .padding(Padding::from([2, 8]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_elevated,
                        pal.accent_secondary,
                        pal.accent_secondary,
                        self.dashboard.control_radius(),
                    ))
                    .on_press(Message::UndoLastAction),
                );
            }

            footer_row = footer_row.push(
                text(format!(
                    "{} · {} · {} · lora.desktop.native.v2",
                    self.dashboard.active_theme_code(),
                    self.active_project_name(),
                    self.persona.label(),
                ))
                .size(11)
                .font(Font::MONOSPACE)
                .color(pal.text_muted),
            );

            let footer = container(footer_row)
                .padding(Padding::from([6, 16]))
                .width(Length::Fill)
                .style(theme::palette_surface_container(
                    pal.bg_dock,
                    pal.text_primary,
                ));

            col = col
                .push(rule::horizontal(1).style(theme::palette_divider_rule(pal)))
                .push(footer);
        }

        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::palette_surface_container(
                pal.bg_stage,
                pal.text_primary,
            ))
            .into()
    }

    fn view_dashboard_channel(&self) -> Element<'_, Message> {
        dashboard::view_dashboard(self)
    }

    fn view_chat_channel(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let card_rad = self.dashboard.card_radius();
        let c_rad = self.dashboard.control_radius();

        let all_artifacts: Vec<ux_studio::ExtractedArtifact> = self
            .messages
            .iter()
            .flat_map(|m| m.artifacts.iter().cloned())
            .collect();

        let mode_bar = container(
            column![
                agent_views::view_agent_execution_bar(self),
                row![
                    checkbox(self.read_only_assistant)
                        .label("Read-Only")
                        .size(13)
                        .text_size(11)
                        .on_toggle(Message::ToggleAssistantMode),
                    checkbox(self.use_rag)
                        .label("RAG")
                        .size(13)
                        .text_size(11)
                        .on_toggle(Message::ToggleRag),
                    button(
                        text(format!("Persona: {}", self.persona.label()))
                            .size(10)
                            .font(Font::MONOSPACE)
                    )
                    .padding(Padding::from([3, 8]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.accent_secondary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_secondary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::SelectPersona(match self.persona {
                        AgentPersona::Balanced => AgentPersona::Concise,
                        AgentPersona::Concise => AgentPersona::DeepArchitect,
                        AgentPersona::DeepArchitect => AgentPersona::StrictAuditor,
                        AgentPersona::StrictAuditor => AgentPersona::Balanced,
                    })),
                    space::horizontal(),
                    button(text("Find Ctrl+F").size(10).font(Font::MONOSPACE))
                        .padding(Padding::from([4, 8]))
                        .style(theme::cyber_chip_button_with_radius(
                            self.chat_search_open,
                            pal.accent_secondary,
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.text_secondary,
                            c_rad,
                        ))
                        .on_press(Message::ToggleChatSearch),
                    button(
                        text(format!("Artifacts ({})", all_artifacts.len()))
                            .size(10)
                            .font(Font::MONOSPACE)
                    )
                    .padding(Padding::from([4, 9]))
                    .style(theme::cyber_chip_button_with_radius(
                        self.artifact_panel_open,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::ToggleArtifactPanel),
                    button(text("Clear").size(10))
                        .padding(Padding::from([4, 8]))
                        .style(theme::solid_button_with_radius(
                            pal.bg_elevated,
                            pal.critical,
                            pal.text_muted,
                            c_rad,
                        ))
                        .on_press(Message::ClearProjectChat),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(6),
        )
        .padding(Padding::from([8, 16]))
        .width(Length::Fill)
        .style(theme::palette_surface_container(
            pal.bg_card,
            pal.text_primary,
        ));

        let md_style = theme::palette_markdown_style(pal);
        let md_settings = markdown::Settings::with_text_size(14.0, md_style);
        let search_filter = self.chat_search_query.trim().to_ascii_lowercase();

        let mut feed = Column::new().spacing(12).padding(Padding::from([16, 22]));
        for entry in &self.messages {
            if !search_filter.is_empty()
                && !entry.raw_text.to_ascii_lowercase().contains(&search_filter)
                && !entry.author.to_ascii_lowercase().contains(&search_filter)
            {
                continue;
            }
            let accent = if entry.is_user {
                pal.accent_primary
            } else {
                pal.accent_secondary
            };
            let mut header_row = row![
                text(&entry.author).size(13).color(accent),
                container(text(&entry.badge).size(10))
                    .padding(Padding::from([2, 7]))
                    .style(theme::pill_container(
                        pal.bg_rail,
                        accent,
                        pal.border_subtle
                    )),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            if entry.is_streaming {
                if !entry.raw_text.is_empty() && self.dashboard.prefs.show_mini_bot {
                    header_row = header_row.push(ux_studio::view_mini_bot_avatar(
                        self,
                        26.0,
                        Some(if self.run_paused {
                            AgentPhase::Paused
                        } else {
                            AgentPhase::Streaming
                        }),
                    ));
                }
                header_row = header_row.push(
                    text(if self.run_paused {
                        "paused (buffering)"
                    } else {
                        "streaming..."
                    })
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.warning),
                );
            } else if entry.outcome_verified {
                header_row = header_row.push(
                    text("SHA-256 Verified")
                        .size(11)
                        .font(Font::MONOSPACE)
                        .color(pal.nominal),
                );
            }

            if entry.token_estimate > 0 {
                header_row = header_row.push(
                    text(if entry.latency_ms > 0 {
                        format!("~{} tok · {}ms", entry.token_estimate, entry.latency_ms)
                    } else {
                        format!("~{} tok", entry.token_estimate)
                    })
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(pal.text_muted),
                );
            }

            header_row = header_row.push(space::horizontal());
            if entry.is_user {
                header_row = header_row.push(
                    button(text("Edit & Re-run").size(10))
                        .padding(Padding::from([2, 8]))
                        .style(theme::solid_button_with_radius(
                            pal.bg_rail,
                            pal.accent_primary,
                            pal.text_secondary,
                            c_rad,
                        ))
                        .on_press(Message::EditPromptMessage(entry.id.clone())),
                );
            } else {
                if !entry.raw_text.trim().is_empty() {
                    header_row = header_row.push(
                        button(text("Copy").size(10))
                            .padding(Padding::from([2, 8]))
                            .style(theme::solid_button_with_radius(
                                pal.bg_rail,
                                pal.accent_secondary,
                                pal.text_secondary,
                                c_rad,
                            ))
                            .on_press(Message::CopyTextToClipboard(entry.raw_text.clone())),
                    );
                }
                if !entry.citations.is_empty() || entry.outcome_sha256.is_some() {
                    header_row = header_row.push(
                        button(
                            text(if entry.show_trace {
                                "Hide Trace"
                            } else {
                                "Trace & SHA"
                            })
                            .size(10)
                            .font(Font::MONOSPACE),
                        )
                        .padding(Padding::from([2, 8]))
                        .style(theme::cyber_chip_button_with_radius(
                            entry.show_trace,
                            pal.nominal,
                            pal.bg_rail,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.text_muted,
                            c_rad,
                        ))
                        .on_press(Message::ToggleMessageTrace(entry.id.clone())),
                    );
                }
            }

            let mut msg_col = Column::new().spacing(8);
            msg_col = msg_col.push(header_row);

            if !entry.is_user && (entry.thinking_text.is_some() || entry.thinking_in_progress) {
                let think_label = if entry.thinking_in_progress {
                    "Chain of Thought (reasoning live...)"
                } else if entry.show_thinking {
                    "Hide Chain of Thought"
                } else {
                    "Show Chain of Thought (CoT)"
                };
                let mut cot_col = Column::new().spacing(6);
                cot_col = cot_col.push(
                    button(text(think_label).size(10).font(Font::MONOSPACE))
                        .padding(Padding::from([3, 9]))
                        .style(theme::cyber_chip_button_with_radius(
                            entry.show_thinking,
                            pal.warning,
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.warning,
                            c_rad,
                        ))
                        .on_press(Message::ToggleMessageThinking(entry.id.clone())),
                );
                if entry.show_thinking {
                    if let Some(thought) = &entry.thinking_text {
                        cot_col = cot_col.push(
                            text(thought.as_str())
                                .size(11)
                                .font(Font::MONOSPACE)
                                .color(pal.text_secondary),
                        );
                    }
                }
                msg_col = msg_col.push(container(cot_col).padding(8).width(Length::Fill).style(
                    theme::palette_card_container(
                        pal.bg_rail,
                        pal.text_secondary,
                        pal.border_subtle,
                        c_rad,
                    ),
                ));
            }

            let body: Element<'_, Message> = if entry.raw_text.is_empty() && entry.is_streaming {
                row![
                    ux_studio::view_mini_bot_avatar(self, 46.0, Some(AgentPhase::Thinking)),
                    column![
                        canvas(SkeletonShimmerCanvas {
                            clock: self.dashboard.motion.phase,
                            base: pal.bg_elevated,
                            shimmer: pal.accent_secondary,
                            motion_enabled: self.dashboard.prefs.cyber_motion,
                        })
                        .width(Length::Fill)
                        .height(Length::Fixed(40.0)),
                        text("Lora is thinking in isolated local workspace context...")
                            .size(11)
                            .color(pal.text_muted),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .into()
            } else {
                markdown::view(entry.answer_markdown.items(), md_settings)
                    .map(Message::MarkdownLinkIgnored)
            };
            msg_col = msg_col.push(body);

            if !entry.artifacts.is_empty() {
                msg_col = msg_col.push(
                    button(
                        text(format!(
                            "Inspect {} Code / Diagram Artifact(s) ->",
                            entry.artifacts.len()
                        ))
                        .size(11)
                        .font(Font::MONOSPACE),
                    )
                    .padding(Padding::from([4, 10]))
                    .style(theme::cyber_chip_button_with_radius(
                        self.artifact_panel_open,
                        pal.accent_secondary,
                        pal.bg_elevated,
                        pal.border_accent,
                        pal.text_primary,
                        pal.accent_secondary,
                        c_rad,
                    ))
                    .on_press(Message::ToggleArtifactPanel),
                );
            }

            if entry.show_trace && (!entry.citations.is_empty() || entry.outcome_sha256.is_some()) {
                let mut meta = Column::new().spacing(4);
                if !entry.citations.is_empty() {
                    meta = meta.push(
                        text(format!("Citations: {}", entry.citations.join(", ")))
                            .size(11)
                            .color(pal.nominal),
                    );
                }
                if let Some(sha) = &entry.outcome_sha256 {
                    meta = meta.push(
                        text(format!(
                            "SHA-256 Digest: {sha} · {} receipts",
                            entry.receipt_count
                        ))
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(pal.text_muted),
                    );
                }
                msg_col = msg_col.push(container(meta).padding(8).width(Length::Fill).style(
                    theme::palette_card_container(
                        pal.bg_rail,
                        pal.text_primary,
                        pal.border_subtle,
                        c_rad,
                    ),
                ));
            }

            let left_stripe = container(space::Space::new().width(3).height(Length::Fill))
                .style(theme::palette_surface_container(accent, accent));
            let card_bg = if entry.is_user {
                pal.bg_elevated
            } else {
                pal.bg_card
            };
            let border = if entry.outcome_verified {
                pal.nominal
            } else if entry.is_user {
                pal.accent_primary
            } else {
                pal.border_subtle
            };

            feed = feed.push(
                container(
                    row![
                        left_stripe,
                        container(msg_col).padding(12).width(Length::Fill)
                    ]
                    .align_y(Alignment::Start),
                )
                .width(Length::Fill)
                .style(theme::palette_card_container(
                    card_bg,
                    pal.text_primary,
                    border,
                    card_rad,
                )),
            );
        }

        if self.messages.len() <= 1 {
            let mut templates_row = Row::new().spacing(8).align_y(Alignment::Center);
            for tpl in OPERATOR_TEMPLATES {
                templates_row = templates_row.push(
                    button(
                        column![
                            text(tpl.title).size(12).color(pal.text_primary),
                            text(tpl.subtitle).size(10).color(pal.text_muted),
                        ]
                        .spacing(2),
                    )
                    .padding(Padding::from([8, 12]))
                    .width(Length::FillPortion(1))
                    .style(theme::palette_channel_button(
                        false,
                        pal.bg_elevated,
                        pal.bg_card,
                        pal.border_accent,
                        pal.text_primary,
                        pal.text_secondary,
                        pal.text_muted,
                    ))
                    .on_press(Message::RunPromptMacro(tpl.prompt.to_string())),
                );
            }
            feed = feed.push(
                column![
                    text("ZERO-FRICTION OPERATOR TEMPLATES")
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(pal.accent_secondary),
                    templates_row,
                ]
                .spacing(6),
            );
            if !self.dashboard.prefs.custom_macros.is_empty() {
                let mut starters = Row::new().spacing(8).align_y(Alignment::Center);
                starters = starters.push(
                    text("MACROS")
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(pal.text_muted),
                );
                for m in self.dashboard.prefs.custom_macros.iter().take(4) {
                    starters = starters.push(
                        button(text(&m.label).size(11))
                            .padding(Padding::from([5, 11]))
                            .style(theme::cyber_chip_button_with_radius(
                                false,
                                pal.accent_secondary,
                                pal.bg_elevated,
                                pal.border_subtle,
                                pal.text_primary,
                                pal.text_secondary,
                                c_rad,
                            ))
                            .on_press(Message::RunPromptMacro(m.prompt.clone())),
                    );
                }
                feed = feed.push(starters);
            }
        }

        let hie_badge = container(
            text(format!(
                "{} · {} · {} tok",
                self.tuning_profile.as_str(),
                self.reasoning_effort.code(),
                self.requested_context
            ))
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.accent_secondary),
        )
        .padding(Padding::from([6, 10]))
        .style(theme::pill_container(
            pal.accent_secondary_soft,
            pal.accent_secondary,
            pal.border_subtle,
        ));

        let paste_btn = button(text("Paste").size(11))
            .padding(Padding::from([8, 12]))
            .style(theme::solid_button_with_radius(
                pal.bg_elevated,
                pal.accent_secondary,
                pal.text_secondary,
                c_rad,
            ))
            .on_press(Message::PasteClipboardPressed);

        let attach_row = row![
            text("ATTACH")
                .size(10)
                .font(Font::MONOSPACE)
                .color(pal.text_muted),
            text_input(
                "Drop file/folder/.env anywhere or type path here (Enter to attach)...",
                &self.attach_path_input
            )
            .on_input(Message::AttachPathChanged)
            .on_submit(Message::AttachPathSubmit)
            .padding(Padding::from([5, 10]))
            .size(11)
            .style(theme::palette_input_style(
                pal.bg_elevated,
                pal.border_subtle,
                pal.accent_secondary,
                pal.accent_secondary_soft,
                pal.text_primary,
                pal.text_muted,
                pal.text_muted,
                c_rad,
            ))
            .width(Length::Fill),
            button(text("+ Attach").size(10).font(Font::MONOSPACE))
                .padding(Padding::from([5, 10]))
                .style(theme::solid_button_with_radius(
                    pal.bg_elevated,
                    pal.accent_secondary,
                    pal.accent_secondary,
                    c_rad,
                ))
                .on_press(Message::AttachPathSubmit),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let mut composer_controls = Row::new().spacing(8).align_y(Alignment::Center);
        composer_controls = composer_controls.push(paste_btn).push(hie_badge).push(
            text_input(
                "Message #chat (Enter to execute · Drop files/folders/.env/.gguf anywhere)...",
                &self.composer_input,
            )
            .on_input(Message::ChatComposerChanged)
            .on_submit(Message::SendChatMessage)
            .padding(Padding::from([10, 14]))
            .style(theme::palette_input_style(
                pal.bg_elevated,
                pal.border_subtle,
                pal.accent_primary,
                pal.accent_primary_soft,
                pal.text_primary,
                pal.text_muted,
                pal.text_muted,
                c_rad,
            ))
            .width(Length::Fill),
        );

        if self.active_run_id.is_some() {
            composer_controls = composer_controls.push(
                button(text(if self.run_paused { "Resume" } else { "Pause" }).size(12))
                    .padding(Padding::from([10, 14]))
                    .style(theme::solid_button_with_radius(
                        pal.warning,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::TogglePauseRun),
            );
            composer_controls = composer_controls.push(
                button(text("STOP").size(12))
                    .padding(Padding::from([10, 18]))
                    .style(theme::solid_button_with_radius(
                        pal.critical,
                        Color {
                            a: 0.84,
                            ..pal.critical
                        },
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::StopActiveTask),
            );
        } else {
            composer_controls = composer_controls.push(
                button(text("Send").size(13))
                    .padding(Padding::from([10, 18]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_primary,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::SendChatMessage),
            );
        }

        let mut composer_col = Column::new().spacing(8);
        if !self.staged_attachments.is_empty() {
            let mut att_row = Row::new().spacing(6).align_y(Alignment::Center);
            for (idx, att) in self.staged_attachments.iter().enumerate() {
                att_row = att_row.push(
                    button(
                        text(format!("[{}] {} ×", att.kind.badge(), att.label))
                            .size(10)
                            .font(Font::MONOSPACE),
                    )
                    .padding(Padding::from([4, 8]))
                    .style(theme::cyber_chip_button_with_radius(
                        true,
                        pal.accent_secondary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::RemoveStagedAttachment(idx)),
                );
            }
            composer_col = composer_col.push(att_row);
        }
        composer_col = composer_col.push(attach_row).push(composer_controls);

        let composer = container(composer_col)
            .padding(Padding::from([10, 16]))
            .width(Length::Fill)
            .style(theme::palette_surface_container(
                pal.bg_dock,
                pal.text_primary,
            ));

        let mut chat_col = Column::new()
            .push(mode_bar)
            .push(rule::horizontal(1).style(theme::palette_divider_rule(pal)));

        if self.dashboard.prefs.show_mini_bot && self.dashboard.prefs.capri.dock_widget_in_chat {
            chat_col = chat_col
                .push(
                    container(ux_studio::view_capri_chat_widget(self))
                        .padding(Padding::from([6, 16]))
                        .width(Length::Fill),
                )
                .push(rule::horizontal(1).style(theme::palette_divider_rule(pal)));
        }

        if self.chat_search_open {
            chat_col = chat_col.push(
                container(
                    row![
                        text("SEARCH CHAT")
                            .size(10)
                            .font(Font::MONOSPACE)
                            .color(pal.accent_secondary),
                        text_input(
                            "Filter messages in active project...",
                            &self.chat_search_query
                        )
                        .id(iced::widget::Id::new(CHAT_SEARCH_INPUT_ID))
                        .on_input(Message::ChatSearchChanged)
                        .padding(Padding::from([6, 10]))
                        .style(theme::palette_input_style(
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.accent_secondary,
                            pal.accent_secondary_soft,
                            pal.text_primary,
                            pal.text_muted,
                            pal.text_muted,
                            c_rad,
                        ))
                        .width(Length::Fill),
                        button(text("Close").size(10))
                            .padding(Padding::from([5, 10]))
                            .style(theme::solid_button_with_radius(
                                pal.bg_elevated,
                                pal.accent_primary,
                                pal.text_primary,
                                c_rad,
                            ))
                            .on_press(Message::ToggleChatSearch),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([7, 16]))
                .width(Length::Fill)
                .style(theme::palette_surface_container(
                    pal.bg_sidebar,
                    pal.text_primary,
                )),
            );
        }

        let feed_scroll = scrollable(feed)
            .id(iced::widget::Id::new(CHAT_SCROLL_ID))
            .on_scroll(|vp: scrollable::Viewport| {
                Message::ChatScrolled(vp.relative_offset().y >= 0.92)
            })
            .height(Length::Fill);

        let feed_stage: Element<'_, Message> = if !self.chat_auto_scroll {
            stack![
                feed_scroll,
                container(
                    button(text("Jump to Latest ↓").size(11).font(Font::MONOSPACE))
                        .padding(Padding::from([6, 12]))
                        .style(theme::solid_button_with_radius(
                            pal.accent_secondary,
                            pal.accent_primary,
                            pal.on_accent_text,
                            c_rad,
                        ))
                        .on_press(Message::JumpToChatBottom),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(14)
                .align_x(Horizontal::Right)
                .align_y(Vertical::Bottom)
            ]
            .into()
        } else {
            feed_scroll.into()
        };

        chat_col = chat_col
            .push(feed_stage)
            .push(rule::horizontal(1).style(theme::palette_divider_rule(pal)))
            .push(composer)
            .height(Length::Fill)
            .width(Length::Fill);

        let art_ratio = if self.dashboard.prefs.cyber_motion {
            self.dashboard
                .motion
                .spring_artifacts
                .value
                .clamp(0.0, 1.15)
        } else if self.artifact_panel_open {
            1.0
        } else {
            0.0
        };
        if self.artifact_panel_open || art_ratio > 0.02 {
            let drawer_w = (360.0 * art_ratio.max(0.04)).clamp(1.0, 400.0);
            row![
                chat_col,
                rule::vertical(1).style(theme::palette_divider_rule(pal)),
                ux_studio::view_artifact_drawer(self, &all_artifacts, drawer_w),
            ]
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
        } else {
            chat_col.into()
        }
    }

    fn view_models_channel(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let card_rad = self.dashboard.card_radius();
        let c_rad = self.dashboard.control_radius();

        let import_box = container(
            column![
                text("Import Local GGUF Artifact")
                    .size(14)
                    .color(pal.text_primary),
                row![
                    text_input("Path to verified .gguf file...", &self.import_path_input)
                        .on_input(Message::ImportPathChanged)
                        .padding(Padding::from([8, 12]))
                        .style(theme::palette_input_style(
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.accent_primary,
                            pal.accent_primary_soft,
                            pal.text_primary,
                            pal.text_muted,
                            pal.text_muted,
                            c_rad,
                        ))
                        .width(Length::Fill),
                    text_input("Display alias (optional)", &self.import_alias_input)
                        .on_input(Message::ImportAliasChanged)
                        .padding(Padding::from([8, 12]))
                        .style(theme::palette_input_style(
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.accent_secondary,
                            pal.accent_secondary_soft,
                            pal.text_primary,
                            pal.text_muted,
                            pal.text_muted,
                            c_rad,
                        ))
                        .width(Length::Fixed(200.0)),
                    button(text("Import GGUF").size(12))
                        .padding(Padding::from([8, 16]))
                        .style(theme::solid_button_with_radius(
                            pal.accent_primary,
                            pal.accent_secondary,
                            pal.on_accent_text,
                            c_rad,
                        ))
                        .on_press(Message::ImportModelPressed),
                ]
                .spacing(8),
            ]
            .spacing(8),
        )
        .padding(14)
        .width(Length::Fill)
        .style(theme::palette_card_container(
            pal.bg_card,
            pal.text_primary,
            pal.border_subtle,
            card_rad,
        ));

        let mut list = Column::new().spacing(8);
        if self.telemetry.models.is_empty() {
            list = list.push(
                container(
                    text("No content-addressed GGUF models registered yet in this data directory.")
                        .size(12)
                        .color(pal.text_muted),
                )
                .padding(14)
                .width(Length::Fill)
                .style(theme::palette_card_container(
                    pal.bg_card,
                    pal.text_primary,
                    pal.border_subtle,
                    card_rad,
                )),
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
                let active = self.selected_model.as_deref() == Some(alias.as_str());
                list = list.push(
                    container(
                        row![
                            column![
                                text(&m.display_alias).size(14).color(if active {
                                    pal.accent_primary
                                } else {
                                    pal.text_primary
                                }),
                                text(format!(
                                    "arch={} · quant={} · {} MiB · tools={} · sha256:{sha}...",
                                    m.probed.architecture,
                                    m.probed.quantization,
                                    size_mib,
                                    m.probed.supports_tool_calling
                                ))
                                .size(11)
                                .font(Font::MONOSPACE)
                                .color(pal.text_muted),
                            ]
                            .spacing(3)
                            .width(Length::Fill),
                            button(text(if active { "Active" } else { "Use" }).size(11))
                                .padding(Padding::from([6, 12]))
                                .style(theme::cyber_chip_button_with_radius(
                                    active,
                                    pal.accent_primary,
                                    pal.bg_elevated,
                                    pal.border_subtle,
                                    pal.text_primary,
                                    pal.text_secondary,
                                    c_rad,
                                ))
                                .on_press(Message::SelectModel(alias.clone())),
                            button(text("Retire").size(11))
                                .padding(Padding::from([6, 12]))
                                .style(theme::solid_button_with_radius(
                                    Color {
                                        a: 0.16,
                                        ..pal.critical
                                    },
                                    pal.critical,
                                    pal.text_primary,
                                    c_rad,
                                ))
                                .on_press(Message::DeleteModelPressed(alias)),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding(12)
                    .width(Length::Fill)
                    .style(theme::palette_card_container(
                        pal.bg_card,
                        pal.text_primary,
                        if active {
                            pal.accent_primary
                        } else {
                            pal.border_subtle
                        },
                        card_rad,
                    )),
                );
            }
        }

        scrollable(column![import_box, list].spacing(12).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_audit_channel(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let card_rad = self.dashboard.card_radius();
        let c_rad = self.dashboard.control_radius();

        let bar = container(
            row![
                column![
                    text("Redacted SQLite Audit Journal & Incident Recovery")
                        .size(14)
                        .color(pal.text_primary),
                    text("Excerpts are excluded; revoking revokes all session workspace grants immediately.")
                        .size(11)
                        .color(pal.text_muted),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text("Revoke All Grants").size(12))
                    .padding(Padding::from([8, 14]))
                    .style(theme::solid_button_with_radius(
                        pal.critical,
                        Color {
                            a: 0.84,
                            ..pal.critical
                        },
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::RunIncidentRecoveryPressed),
            ]
            .align_y(Alignment::Center),
        )
        .padding(14)
        .width(Length::Fill)
        .style(theme::palette_card_container(
            pal.bg_card,
            pal.text_primary,
            pal.border_subtle,
            card_rad,
        ));

        let mut rows = Column::new().spacing(6);
        for (seq, outcome, resource, summary) in &self.telemetry.recent_audits {
            rows = rows.push(
                container(
                    column![
                        text(format!("#{seq} · {outcome} · {resource}"))
                            .size(12)
                            .color(pal.text_primary),
                        text(summary).size(11).color(pal.text_muted),
                    ]
                    .spacing(2),
                )
                .padding(10)
                .width(Length::Fill)
                .style(theme::palette_card_container(
                    pal.bg_card,
                    pal.text_primary,
                    pal.border_subtle,
                    card_rad.min(6.0),
                )),
            );
        }

        scrollable(column![bar, rows].spacing(12).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_broker_channel(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let card_rad = self.dashboard.card_radius();
        let c_rad = self.dashboard.control_radius();

        let services = ["lora-local-service", "lora-worker", "lora-daemon"];
        let mut svc_btns = Row::new().spacing(8);
        for s in services {
            let active = self.broker_service_name == s;
            svc_btns = svc_btns.push(
                button(text(s).size(12))
                    .padding(Padding::from([6, 12]))
                    .style(theme::cyber_chip_button_with_radius(
                        active,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::SelectBrokerService(s.to_string())),
            );
        }

        let gate_card = container(
            column![
                text("Privileged Broker Single-Use Permit & Receipt Attestation (R09)")
                    .size(14)
                    .color(pal.text_primary),
                text("Domains: lora.broker.permit.v1 & lora.broker.receipt.v1 · Replay-protected one-shot nonces.")
                    .size(11)
                    .color(pal.text_muted),
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
                        .style(theme::solid_button_with_radius(
                            pal.accent_primary,
                            pal.accent_secondary,
                            pal.on_accent_text,
                            c_rad,
                        ))
                        .on_press(Message::ExecuteBrokerPermitPressed),
                ]
                .align_y(Alignment::Center),
            ]
            .spacing(12),
        )
        .padding(16)
        .width(Length::Fill)
        .style(theme::palette_card_container(
            pal.bg_card,
            pal.text_primary,
            pal.border_subtle,
            card_rad,
        ));

        let receipt_card = if let Some((rcpt, verified)) = &self.latest_broker_receipt {
            container(
                column![
                    text(format!("BrokerExecutionReceipt · verified={verified}"))
                        .size(13)
                        .color(if *verified { pal.nominal } else { pal.critical }),
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
                    .color(pal.text_secondary),
                ]
                .spacing(6),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::palette_card_container(
                pal.bg_card,
                pal.text_primary,
                if *verified { pal.nominal } else { pal.critical },
                card_rad,
            ))
        } else {
            container(
                text("No broker receipt attested in this UI session yet.")
                    .size(12)
                    .color(pal.text_muted),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::palette_card_container(
                pal.bg_card,
                pal.text_primary,
                pal.border_subtle,
                card_rad,
            ))
        };

        scrollable(column![gate_card, receipt_card].spacing(12).padding(18))
            .height(Length::Fill)
            .into()
    }

    fn view_adapters_channel(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let card_rad = self.dashboard.card_radius();
        let matrix = default_risk_register_matrix();
        let mut cards = Column::new().spacing(8);
        for r in matrix {
            cards = cards.push(
                container(
                    row![
                        text(r.risk_id)
                            .size(12)
                            .font(Font::MONOSPACE)
                            .color(pal.accent_primary),
                        column![
                            text(r.mitigation_control).size(12).color(pal.text_primary),
                            text(r.fallback_behavior).size(11).color(pal.text_muted),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        text(if r.verified { "VERIFIED" } else { "OPEN" })
                            .size(11)
                            .color(if r.verified { pal.nominal } else { pal.warning }),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(10)
                .width(Length::Fill)
                .style(theme::palette_card_container(
                    pal.bg_card,
                    pal.text_primary,
                    pal.border_subtle,
                    card_rad.min(6.0),
                )),
            );
        }

        scrollable(
            column![
                text("Experimental Adapter Studio & Risk Qualification Register (R01..R18)")
                    .size(14)
                    .color(pal.text_primary),
                cards,
            ]
            .spacing(12)
            .padding(18),
        )
        .height(Length::Fill)
        .into()
    }

    fn view_extensions_channel(&self) -> Element<'_, Message> {
        let pal = self.dashboard.palette();
        let card_rad = self.dashboard.card_radius();
        let host_summary = format!(
            "Registered sandboxed extensions in store: {}",
            self.telemetry.extension_count
        );
        scrollable(
            column![container(
                column![
                    text("Extension Host Manager (MCP / Wasm Sandboxed)")
                        .size(14)
                        .color(pal.text_primary),
                    text(host_summary).size(12).color(pal.text_muted),
                ]
                .spacing(6),
            )
            .padding(16)
            .width(Length::Fill)
            .style(theme::palette_card_container(
                pal.bg_card,
                pal.text_primary,
                pal.border_subtle,
                card_rad,
            )),]
            .padding(18),
        )
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
                .style(theme::palette_card_container(
                    pal.bg_card,
                    pal.text_primary,
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
            .style(theme::palette_card_container(
                pal.bg_card,
                pal.text_primary,
                pal.border_subtle,
                radius
            )),
            text("RECENT AUDIT TRAIL").size(11).color(pal.text_muted),
            scrollable(audits_col).height(Length::Fill),
        ]
        .spacing(10)
        .padding(12)
        .height(Length::Fill);

        let insp_ratio = if self.dashboard.prefs.cyber_motion {
            self.dashboard
                .motion
                .spring_inspector
                .value
                .clamp(0.0, 1.15)
        } else {
            1.0
        };
        let insp_width = (268.0 * insp_ratio).clamp(1.0, 300.0);

        container(panel)
            .width(Length::Fixed(insp_width))
            .height(Length::Fill)
            .style(theme::palette_surface_container(
                pal.bg_sidebar,
                pal.text_primary,
            ))
            .into()
    }

    fn view_consent_modal<'a>(&'a self, prompt: &'a PendingConsentPrompt) -> Element<'a, Message> {
        let pal = self.dashboard.palette();
        let card_rad = self.dashboard.card_radius();
        let c_rad = self.dashboard.control_radius();
        let vm = &prompt.view_model;
        let card = container(
            column![
                text("TRUSTED CONSENT REQUIRED").size(12).color(pal.warning),
                text("Authorize Bounded Local File Access")
                    .size(17)
                    .color(pal.text_primary),
                text(
                    "Untrusted file content is never rendered inside this trusted approval prompt."
                )
                .size(12)
                .color(pal.text_muted),
                container(
                    column![
                        Self::kv_row("Tool Identity", &vm.tool_id, pal),
                        Self::kv_row("Run ID", &vm.run_id, pal),
                        Self::kv_row("Scope Root", &vm.scope_root_id, pal),
                        Self::kv_row("Relative Target", &vm.target_relative_path, pal),
                        Self::kv_row(
                            "Read Byte Limit",
                            format!("{} bytes", vm.read_byte_limit),
                            pal
                        ),
                        Self::kv_row("Destination", &vm.destination_label, pal),
                        Self::kv_row("Grant Lifetime", &vm.grant_lifetime_label, pal),
                        Self::kv_row("Action Digest", &vm.action_digest, pal),
                        Self::kv_row("Default Focus", &vm.default_button_focus, pal),
                    ]
                    .spacing(5),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::palette_card_container(
                    pal.bg_elevated,
                    pal.text_primary,
                    pal.border_subtle,
                    card_rad,
                )),
                row![
                    // Least-privilege Deny button sits primary & prominent by design (`docs/DESKTOP_UX.md`).
                    button(text("Deny (Default Safe)").size(13))
                        .padding(Padding::from([10, 18]))
                        .style(theme::solid_button_with_radius(
                            pal.critical,
                            Color {
                                a: 0.84,
                                ..pal.critical
                            },
                            pal.on_accent_text,
                            c_rad,
                        ))
                        .on_press(Message::ResolveConsent {
                            request_id: prompt.request_id,
                            decision: ApprovalDecision::Deny,
                        }),
                    space::horizontal(),
                    button(text("Approve Once").size(12))
                        .padding(Padding::from([10, 14]))
                        .style(theme::cyber_chip_button_with_radius(
                            false,
                            pal.accent_primary,
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.text_primary,
                            c_rad,
                        ))
                        .on_press(Message::ResolveConsent {
                            request_id: prompt.request_id,
                            decision: ApprovalDecision::ApproveOnce,
                        }),
                    button(text("Approve Session").size(12))
                        .padding(Padding::from([10, 14]))
                        .style(theme::cyber_chip_button_with_radius(
                            false,
                            pal.nominal,
                            pal.bg_elevated,
                            pal.border_subtle,
                            pal.text_primary,
                            pal.nominal,
                            c_rad,
                        ))
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
        .style(theme::palette_modal_card(pal, card_rad));

        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .style(theme::modal_backdrop_style)
            .into()
    }

    fn kv_row<'a>(
        label: &'a str,
        val: impl Into<String>,
        pal: CyberPalette,
    ) -> Element<'a, Message> {
        row![
            text(label)
                .size(11)
                .color(pal.text_muted)
                .width(Length::Fixed(128.0)),
            text(val.into())
                .size(11)
                .font(Font::MONOSPACE)
                .color(pal.text_primary),
        ]
        .spacing(8)
        .into()
    }
}
