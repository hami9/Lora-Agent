//! Minimalist Cyberpunk modular HUD & customizable widget dashboard for `lora-desktop`.

use crate::app::{ChannelTab, LoraDesktopApp, Message};
use crate::backend::CoreTelemetry;
use crate::theme::{self, CustomThemeSpec, CyberPalette, CyberThemePreset, HudGeometry};
use crate::ux_studio::SpringFloat;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    button, canvas, checkbox, column, container, progress_bar, row, rule, scrollable, slider,
    space, text, text_input, Column, Row,
};
use iced::{
    mouse, Alignment, Color, Element, Font, Length, Padding, Point, Radians, Rectangle, Renderer,
    Theme,
};
use lora_core::TuningProfile;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};
use std::path::Path;
use std::time::{Duration, Instant};

const HUD_PREFS_FILENAME: &str = "dashboard_hud.json";
const TELEMETRY_HISTORY_CAP: usize = 48;

/// Global spatial density mode across dashboard cards and grids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HudDensity {
    Compact,
    #[default]
    Balanced,
    Spacious,
}

impl HudDensity {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Balanced, Self::Spacious];

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Balanced => "Balanced",
            Self::Spacious => "Spacious",
        }
    }

    pub fn card_padding(self) -> u16 {
        match self {
            Self::Compact => 10,
            Self::Balanced => 14,
            Self::Spacious => 18,
        }
    }

    pub fn grid_gap(self) -> f32 {
        match self {
            Self::Compact => 8.0,
            Self::Balanced => 12.0,
            Self::Spacious => 16.0,
        }
    }
}

/// Column span of a modular widget in the 2-column Bento HUD grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WidgetSpan {
    Half,
    Wide,
}

impl WidgetSpan {
    pub fn toggle(self) -> Self {
        match self {
            Self::Half => Self::Wide,
            Self::Wide => Self::Half,
        }
    }

    pub fn badge(self) -> &'static str {
        match self {
            Self::Half => "1/2",
            Self::Wide => "2/2",
        }
    }
}

/// One-click curated dashboard layout templates inspired by modern local AI command centers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DashboardPreset {
    MinimalFocus,
    #[default]
    OperatorHud,
    TelemetryLab,
    DevStudio,
}

impl DashboardPreset {
    pub const ALL: [Self; 4] = [
        Self::MinimalFocus,
        Self::OperatorHud,
        Self::TelemetryLab,
        Self::DevStudio,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MinimalFocus => "Minimal Focus",
            Self::OperatorHud => "Operator HUD",
            Self::TelemetryLab => "Telemetry Lab",
            Self::DevStudio => "Dev Studio",
        }
    }

    pub fn default_slots(self) -> Vec<WidgetSlot> {
        match self {
            Self::MinimalFocus => vec![
                WidgetSlot::new(DashboardWidgetKind::CommandDeck, true, WidgetSpan::Wide),
                WidgetSlot::new(DashboardWidgetKind::SystemReactor, true, WidgetSpan::Half),
                WidgetSlot::new(DashboardWidgetKind::ModelArsenal, true, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::TelemetrySparkline,
                    false,
                    WidgetSpan::Half,
                ),
                WidgetSlot::new(
                    DashboardWidgetKind::WorkspaceShield,
                    false,
                    WidgetSpan::Half,
                ),
                WidgetSlot::new(DashboardWidgetKind::WorkflowMatrix, false, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::AttestationRadar,
                    false,
                    WidgetSpan::Half,
                ),
                WidgetSlot::new(
                    DashboardWidgetKind::OperatorScratchpad,
                    false,
                    WidgetSpan::Wide,
                ),
            ],
            Self::OperatorHud => vec![
                WidgetSlot::new(DashboardWidgetKind::CommandDeck, true, WidgetSpan::Wide),
                WidgetSlot::new(DashboardWidgetKind::CapriCompanion, true, WidgetSpan::Half),
                WidgetSlot::new(DashboardWidgetKind::SystemReactor, true, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::TelemetrySparkline,
                    true,
                    WidgetSpan::Half,
                ),
                WidgetSlot::new(DashboardWidgetKind::ModelArsenal, true, WidgetSpan::Half),
                WidgetSlot::new(DashboardWidgetKind::WorkspaceShield, true, WidgetSpan::Half),
                WidgetSlot::new(DashboardWidgetKind::WorkflowMatrix, true, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::AttestationRadar,
                    true,
                    WidgetSpan::Half,
                ),
                WidgetSlot::new(
                    DashboardWidgetKind::OperatorScratchpad,
                    false,
                    WidgetSpan::Wide,
                ),
            ],
            Self::TelemetryLab => vec![
                WidgetSlot::new(
                    DashboardWidgetKind::TelemetrySparkline,
                    true,
                    WidgetSpan::Wide,
                ),
                WidgetSlot::new(DashboardWidgetKind::SystemReactor, true, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::AttestationRadar,
                    true,
                    WidgetSpan::Half,
                ),
                WidgetSlot::new(DashboardWidgetKind::WorkflowMatrix, true, WidgetSpan::Half),
                WidgetSlot::new(DashboardWidgetKind::ModelArsenal, true, WidgetSpan::Half),
                WidgetSlot::new(DashboardWidgetKind::CommandDeck, true, WidgetSpan::Wide),
                WidgetSlot::new(DashboardWidgetKind::CapriCompanion, false, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::WorkspaceShield,
                    false,
                    WidgetSpan::Half,
                ),
                WidgetSlot::new(
                    DashboardWidgetKind::OperatorScratchpad,
                    false,
                    WidgetSpan::Wide,
                ),
            ],
            Self::DevStudio => vec![
                WidgetSlot::new(DashboardWidgetKind::CommandDeck, true, WidgetSpan::Wide),
                WidgetSlot::new(DashboardWidgetKind::CapriCompanion, true, WidgetSpan::Wide),
                WidgetSlot::new(DashboardWidgetKind::WorkspaceShield, true, WidgetSpan::Half),
                WidgetSlot::new(DashboardWidgetKind::ModelArsenal, true, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::OperatorScratchpad,
                    true,
                    WidgetSpan::Wide,
                ),
                WidgetSlot::new(DashboardWidgetKind::WorkflowMatrix, true, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::AttestationRadar,
                    true,
                    WidgetSpan::Half,
                ),
                WidgetSlot::new(DashboardWidgetKind::SystemReactor, false, WidgetSpan::Half),
                WidgetSlot::new(
                    DashboardWidgetKind::TelemetrySparkline,
                    false,
                    WidgetSpan::Half,
                ),
            ],
        }
    }
}

/// First-class modular widgets available inside the dashboard bento grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DashboardWidgetKind {
    CommandDeck,
    CapriCompanion,
    SystemReactor,
    TelemetrySparkline,
    ModelArsenal,
    WorkspaceShield,
    WorkflowMatrix,
    AttestationRadar,
    OperatorScratchpad,
}

impl DashboardWidgetKind {
    pub const ALL: [Self; 9] = [
        Self::CommandDeck,
        Self::CapriCompanion,
        Self::SystemReactor,
        Self::TelemetrySparkline,
        Self::ModelArsenal,
        Self::WorkspaceShield,
        Self::WorkflowMatrix,
        Self::AttestationRadar,
        Self::OperatorScratchpad,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::CommandDeck => "COMMAND OMNIBAR",
            Self::CapriCompanion => "CAPRI × LORA WIDGET",
            Self::SystemReactor => "SYSTEM REACTOR",
            Self::TelemetrySparkline => "MEMORY WAVEFORM",
            Self::ModelArsenal => "MODEL ARSENAL",
            Self::WorkspaceShield => "WORKSPACE SHIELD",
            Self::WorkflowMatrix => "WORKFLOW MATRIX",
            Self::AttestationRadar => "TRUST & SNAPSHOT",
            Self::OperatorScratchpad => "MACRO DECK",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::CommandDeck => "CMD-01",
            Self::CapriCompanion => "CAP-09",
            Self::SystemReactor => "SYS-02",
            Self::TelemetrySparkline => "TLM-03",
            Self::ModelArsenal => "MDL-04",
            Self::WorkspaceShield => "SEC-05",
            Self::WorkflowMatrix => "PIP-06",
            Self::AttestationRadar => "SHA-07",
            Self::OperatorScratchpad => "MCR-08",
        }
    }
}

/// Curated aura palettes for the Capri × Lora Glitch-Cyber companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CapriAuraPreset {
    #[default]
    ClassicGlitch,
    CyberGold,
    PhantomViolet,
    IceReactor,
    CustomHue,
}

impl CapriAuraPreset {
    pub const ALL: [Self; 5] = [
        Self::ClassicGlitch,
        Self::CyberGold,
        Self::PhantomViolet,
        Self::IceReactor,
        Self::CustomHue,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ClassicGlitch => "Crimson × Toxic",
            Self::CyberGold => "Solar × Lime",
            Self::PhantomViolet => "Violet × Mint",
            Self::IceReactor => "Frost × Neon",
            Self::CustomHue => "Custom Hues",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::ClassicGlitch => Self::CyberGold,
            Self::CyberGold => Self::PhantomViolet,
            Self::PhantomViolet => Self::IceReactor,
            Self::IceReactor => Self::CustomHue,
            Self::CustomHue => Self::ClassicGlitch,
        }
    }
}

/// Eye expression override mode for the Capri companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CapriEyeMode {
    #[default]
    Dynamic,
    DoubleBlue,
    CrossGlitch,
    Hypno,
}

impl CapriEyeMode {
    pub const ALL: [Self; 4] = [
        Self::Dynamic,
        Self::DoubleBlue,
        Self::CrossGlitch,
        Self::Hypno,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Dynamic => "Dynamic",
            Self::DoubleBlue => "Dual O",
            Self::CrossGlitch => "O + X",
            Self::Hypno => "Hyper X",
        }
    }
}

/// Full visual & widget customization profile for the Capri companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CapriStyleSpec {
    pub aura_preset: CapriAuraPreset,
    pub crown_hue: u16,
    pub visor_hue: u16,
    pub glitch_intensity_pct: u8,
    pub show_aura_shards: bool,
    pub show_lightning: bool,
    pub always_toothy_grin: bool,
    pub eye_mode: CapriEyeMode,
    pub widget_scale_px: u16,
    pub dock_widget_in_chat: bool,
    pub floating_badge_chip: bool,
}

impl Default for CapriStyleSpec {
    fn default() -> Self {
        Self {
            aura_preset: CapriAuraPreset::ClassicGlitch,
            crown_hue: 342,
            visor_hue: 148,
            glitch_intensity_pct: 80,
            show_aura_shards: true,
            show_lightning: true,
            always_toothy_grin: false,
            eye_mode: CapriEyeMode::Dynamic,
            widget_scale_px: 56,
            dock_widget_in_chat: true,
            floating_badge_chip: true,
        }
    }
}

impl CapriStyleSpec {
    /// Resolve `(crown, visor, eye_blue, glitch_violet)` colors for `LoraMiniBotCanvas`.
    pub fn resolve_colors(self, pal: CyberPalette) -> (Color, Color, Color, Color) {
        match self.aura_preset {
            CapriAuraPreset::ClassicGlitch => (
                Color::from_rgb(1.0, 0.04, 0.36),
                Color::from_rgb(0.0, 1.0, 0.48),
                Color::from_rgb(0.05, 0.58, 1.0),
                Color::from_rgb(0.52, 0.18, 0.98),
            ),
            CapriAuraPreset::CyberGold => (
                Color::from_rgb(1.0, 0.72, 0.08),
                Color::from_rgb(0.32, 1.0, 0.24),
                Color::from_rgb(0.0, 0.85, 1.0),
                pal.accent_primary,
            ),
            CapriAuraPreset::PhantomViolet => (
                Color::from_rgb(0.68, 0.20, 1.0),
                Color::from_rgb(0.16, 1.0, 0.76),
                Color::from_rgb(0.95, 0.25, 0.78),
                pal.accent_secondary,
            ),
            CapriAuraPreset::IceReactor => (
                Color::from_rgb(0.06, 0.78, 1.0),
                Color::from_rgb(0.68, 1.0, 0.14),
                Color::from_rgb(0.12, 0.48, 1.0),
                Color::from_rgb(1.0, 0.18, 0.56),
            ),
            CapriAuraPreset::CustomHue => (
                theme::hsl_color(self.crown_hue as f32, 0.96, 0.54),
                theme::hsl_color(self.visor_hue as f32, 0.96, 0.52),
                pal.accent_secondary,
                pal.accent_primary,
            ),
        }
    }
}

/// Layout slot configuration for one widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetSlot {
    pub kind: DashboardWidgetKind,
    pub visible: bool,
    pub collapsed: bool,
    pub span: WidgetSpan,
}

impl WidgetSlot {
    pub fn new(kind: DashboardWidgetKind, visible: bool, span: WidgetSpan) -> Self {
        Self {
            kind,
            visible,
            collapsed: false,
            span,
        }
    }
}

/// Reusable one-click prompt macro pinned on the dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptMacro {
    pub label: String,
    pub prompt: String,
}

/// Versioned user customization preferences persisted cleanly in `data_dir`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DashboardPreferences {
    pub schema_version: u32,
    pub preset: DashboardPreset,
    pub theme_preset: CyberThemePreset,
    pub density: HudDensity,
    pub geometry: HudGeometry,
    pub neon_borders: bool,
    pub show_sparkline_grid: bool,
    pub custom_theme: CustomThemeSpec,
    pub capri: CapriStyleSpec,
    pub corner_radius_px: u8,
    pub glow_strength_pct: u8,
    pub sidebar_width_px: u16,
    pub show_icon_rail: bool,
    pub show_status_footer: bool,
    pub use_mono_chrome: bool,
    pub cyber_motion: bool,
    #[serde(default = "default_true")]
    pub native_notifications: bool,
    #[serde(default = "default_true")]
    pub show_mini_bot: bool,
    pub widgets: Vec<WidgetSlot>,
    pub custom_macros: Vec<PromptMacro>,
}

fn default_true() -> bool {
    true
}

impl Default for DashboardPreferences {
    fn default() -> Self {
        Self {
            schema_version: 1,
            preset: DashboardPreset::OperatorHud,
            theme_preset: CyberThemePreset::NightCity2077,
            density: HudDensity::Balanced,
            geometry: HudGeometry::SleekModern,
            neon_borders: true,
            show_sparkline_grid: true,
            custom_theme: CustomThemeSpec::default(),
            capri: CapriStyleSpec::default(),
            corner_radius_px: 8,
            glow_strength_pct: 65,
            sidebar_width_px: 244,
            show_icon_rail: true,
            show_status_footer: true,
            use_mono_chrome: false,
            cyber_motion: true,
            native_notifications: true,
            show_mini_bot: true,
            widgets: DashboardPreset::OperatorHud.default_slots(),
            custom_macros: vec![
                PromptMacro {
                    label: "Workspace Brief".to_string(),
                    prompt: "Inspect active local workspace structure and summarize key files."
                        .to_string(),
                },
                PromptMacro {
                    label: "Hardware Readiness".to_string(),
                    prompt:
                        "Evaluate available host RAM and recommend an optimal GGUF context plan."
                            .to_string(),
                },
                PromptMacro {
                    label: "Trust Verification".to_string(),
                    prompt: "List recent policy permits, audit checks, and integrity guarantees."
                        .to_string(),
                },
            ],
        }
    }
}

impl DashboardPreferences {
    /// Ensure every widget kind appears at most once and any missing widget kind is appended.
    pub fn sanitize(&mut self) {
        self.schema_version = 1;
        let mut seen = Vec::new();
        self.widgets.retain(|slot| {
            if seen.contains(&slot.kind) {
                false
            } else {
                seen.push(slot.kind);
                true
            }
        });
        for kind in DashboardWidgetKind::ALL {
            if !seen.contains(&kind) {
                let default_span = match kind {
                    DashboardWidgetKind::CommandDeck | DashboardWidgetKind::OperatorScratchpad => {
                        WidgetSpan::Wide
                    }
                    _ => WidgetSpan::Half,
                };
                self.widgets
                    .push(WidgetSlot::new(kind, false, default_span));
            }
        }
    }

    pub fn load_from_dir(data_dir: &Path) -> Self {
        let path = data_dir.join(HUD_PREFS_FILENAME);
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(mut parsed) = serde_json::from_str::<Self>(&raw) else {
            return Self::default();
        };
        parsed.sanitize();
        parsed
    }

    pub fn save_to_dir(&self, data_dir: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(data_dir.join(HUD_PREFS_FILENAME), json);
        }
    }
}

/// Delta-time smoothed animation state for buttery 60 FPS theme morphing & cyberpunk micro-motion.
#[derive(Debug, Clone)]
pub struct MotionState {
    pub palette: CyberPalette,
    pub ram_ratio: f32,
    pub sidebar_width: f32,
    pub rail_width: f32,
    pub radius_px: f32,
    pub phase: f32,
    pub channel_sweep: f32,
    pub pulse_burst: f32,
    pub spring_palette: SpringFloat,
    pub spring_artifacts: SpringFloat,
    pub spring_inspector: SpringFloat,
    pub bot_bounce: SpringFloat,
    last_frame_at: Instant,
    last_telemetry_at: Instant,
    pub settling: bool,
}

impl MotionState {
    fn new(prefs: &DashboardPreferences, initial_ram: f32) -> Self {
        let now = Instant::now();
        let target_pal = if prefs.custom_theme.enabled {
            prefs.custom_theme.build_palette()
        } else {
            prefs.theme_preset.palette()
        };
        Self {
            palette: target_pal,
            ram_ratio: (initial_ram * 0.65).clamp(0.03, 1.0),
            sidebar_width: prefs.sidebar_width_px.clamp(210, 300) as f32,
            rail_width: if prefs.show_icon_rail { 74.0 } else { 0.0 },
            radius_px: prefs.corner_radius_px.min(18) as f32,
            phase: 0.0,
            channel_sweep: 1.0,
            pulse_burst: 0.85,
            spring_palette: SpringFloat::new(0.0, 280.0, 26.0),
            spring_artifacts: SpringFloat::new(0.0, 240.0, 24.0),
            spring_inspector: SpringFloat::new(0.0, 240.0, 24.0),
            bot_bounce: SpringFloat::new(0.0, 340.0, 14.0),
            last_frame_at: now,
            last_telemetry_at: now,
            settling: true,
        }
    }
}

/// Live runtime state for the customizable dashboard and vector telemetry canvases.
#[derive(Debug, Clone)]
pub struct DashboardState {
    pub prefs: DashboardPreferences,
    pub customize_open: bool,
    pub quick_prompt_input: String,
    pub macro_label_input: String,
    pub macro_prompt_input: String,
    pub ram_history: VecDeque<f32>,
    pub motion: MotionState,
}

impl DashboardState {
    pub fn new(initial_telemetry: &CoreTelemetry) -> Self {
        let prefs = DashboardPreferences::default();
        let total = initial_telemetry.dashboard.total_ram_mib.max(1) as f32;
        let avail = (initial_telemetry.dashboard.available_ram_mib as f32).min(total);
        let init_ram = ((total - avail) / total).clamp(0.02, 0.99);
        let mut state = Self {
            motion: MotionState::new(&prefs, init_ram),
            prefs,
            customize_open: false,
            quick_prompt_input: String::new(),
            macro_label_input: String::new(),
            macro_prompt_input: String::new(),
            ram_history: VecDeque::with_capacity(TELEMETRY_HISTORY_CAP),
        };
        state.record_telemetry_sample(initial_telemetry);
        state
    }

    /// Target palette resolved directly from active preset or custom Hue Matrix.
    pub fn target_palette(&self) -> CyberPalette {
        if self.prefs.custom_theme.enabled {
            self.prefs.custom_theme.build_palette()
        } else {
            self.prefs.theme_preset.palette()
        }
    }

    /// Snap visual motion caches to saved preferences on startup without interrupting entry sweep.
    pub fn sync_motion_to_prefs(&mut self) {
        self.motion.palette = self.target_palette();
        self.motion.sidebar_width = self.prefs.sidebar_width_px.clamp(210, 300) as f32;
        self.motion.rail_width = if self.prefs.show_icon_rail { 74.0 } else { 0.0 };
        self.motion.radius_px = self.prefs.corner_radius_px.min(18) as f32;
    }

    pub fn palette(&self) -> CyberPalette {
        if self.prefs.cyber_motion {
            self.motion.palette
        } else {
            self.target_palette()
        }
    }

    pub fn active_theme_code(&self) -> &'static str {
        if self.prefs.custom_theme.enabled {
            "CUSTOM-RGB"
        } else {
            self.prefs.theme_preset.code()
        }
    }

    pub fn card_radius(&self) -> f32 {
        if self.prefs.cyber_motion {
            self.motion.radius_px.clamp(0.0, 18.0)
        } else {
            self.prefs.corner_radius_px.min(18) as f32
        }
    }

    pub fn control_radius(&self) -> f32 {
        (self.card_radius() * 0.75).clamp(0.0, 12.0)
    }

    pub fn animated_sidebar_width(&self) -> f32 {
        if self.prefs.cyber_motion {
            self.motion.sidebar_width.clamp(210.0, 300.0)
        } else {
            self.prefs.sidebar_width_px.clamp(210, 300) as f32
        }
    }

    pub fn animated_rail_width(&self) -> f32 {
        if self.prefs.cyber_motion {
            self.motion.rail_width.clamp(0.0, 74.0)
        } else if self.prefs.show_icon_rail {
            74.0
        } else {
            0.0
        }
    }

    pub fn glow_breath(&self) -> f32 {
        if !self.prefs.cyber_motion {
            return 1.0;
        }
        let sine = 0.84 + 0.16 * (self.motion.phase * 1.7).sin();
        (sine + self.motion.pulse_burst * 0.25).clamp(0.75, 1.35)
    }

    pub fn trigger_transition(&mut self) {
        if self.prefs.cyber_motion {
            self.motion.channel_sweep = 1.0;
            self.motion.pulse_burst = 0.8;
            self.motion.settling = true;
        }
    }

    pub fn trigger_pulse(&mut self) {
        if self.prefs.cyber_motion {
            self.motion.pulse_burst = 1.0;
            self.motion.settling = true;
        }
    }

    pub fn is_animating(&self) -> bool {
        self.prefs.cyber_motion && self.motion.settling
    }

    pub fn step_motion(&mut self, active_run: bool) {
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.motion.last_frame_at)
            .as_secs_f32()
            .clamp(0.004, 0.05);
        self.motion.last_frame_at = now;

        let target_pal = self.target_palette();
        let target_ram = self.ram_history.back().copied().unwrap_or(0.35);
        let target_sidebar = self.prefs.sidebar_width_px.clamp(210, 300) as f32;
        let target_rail = if self.prefs.show_icon_rail { 74.0 } else { 0.0 };
        let target_rad = self.prefs.corner_radius_px.min(18) as f32;

        if !self.prefs.cyber_motion {
            self.motion.palette = target_pal;
            self.motion.ram_ratio = target_ram;
            self.motion.sidebar_width = target_sidebar;
            self.motion.rail_width = target_rail;
            self.motion.radius_px = target_rad;
            self.motion.channel_sweep = 0.0;
            self.motion.pulse_burst = 0.0;
            self.motion.spring_palette.step(dt, true);
            self.motion.spring_artifacts.step(dt, true);
            self.motion.spring_inspector.step(dt, true);
            self.motion.bot_bounce.step(dt, true);
            self.motion.settling = false;
            return;
        }

        let sp_pal = self.motion.spring_palette.step(dt, false);
        let sp_art = self.motion.spring_artifacts.step(dt, false);
        let sp_insp = self.motion.spring_inspector.step(dt, false);
        let sp_bot = self.motion.bot_bounce.step(dt, false);

        let ease = |speed: f32| -> f32 { (1.0 - (-dt * speed).exp()).clamp(0.0, 1.0) };

        let pal_t = ease(10.5);
        self.motion.palette = self.motion.palette.lerp(&target_pal, pal_t);
        self.motion.ram_ratio = theme::lerp_f32(self.motion.ram_ratio, target_ram, ease(6.0));
        self.motion.sidebar_width =
            theme::lerp_f32(self.motion.sidebar_width, target_sidebar, ease(14.0));
        self.motion.rail_width = theme::lerp_f32(self.motion.rail_width, target_rail, ease(14.5));
        self.motion.radius_px = theme::lerp_f32(self.motion.radius_px, target_rad, ease(13.0));
        self.motion.channel_sweep *= (-dt * 4.0).exp();
        self.motion.pulse_burst *= (-dt * 3.2).exp();

        if self.motion.channel_sweep < 0.008 {
            self.motion.channel_sweep = 0.0;
        }
        if self.motion.pulse_burst < 0.008 {
            self.motion.pulse_burst = 0.0;
        }

        let phase_speed = if active_run { 2.6 } else { 1.15 };
        self.motion.phase = (self.motion.phase + dt * phase_speed) % (TAU * 512.0);

        let pal_diff = self.motion.palette.max_channel_diff(&target_pal);
        if pal_diff < 0.003 {
            self.motion.palette = target_pal;
        }
        if (self.motion.sidebar_width - target_sidebar).abs() < 0.35 {
            self.motion.sidebar_width = target_sidebar;
        }
        if (self.motion.rail_width - target_rail).abs() < 0.35 {
            self.motion.rail_width = target_rail;
        }
        if (self.motion.radius_px - target_rad).abs() < 0.12 {
            self.motion.radius_px = target_rad;
        }

        self.motion.settling = pal_diff >= 0.003
            || (self.motion.ram_ratio - target_ram).abs() > 0.0025
            || (self.motion.sidebar_width - target_sidebar).abs() >= 0.35
            || (self.motion.rail_width - target_rail).abs() >= 0.35
            || (self.motion.radius_px - target_rad).abs() >= 0.12
            || self.motion.channel_sweep > 0.008
            || self.motion.pulse_burst > 0.008
            || sp_pal
            || sp_art
            || sp_insp
            || sp_bot;
    }

    pub fn ui_font(&self) -> Font {
        if self.prefs.use_mono_chrome {
            Font::MONOSPACE
        } else {
            Font::DEFAULT
        }
    }

    pub fn apply_preset(&mut self, preset: DashboardPreset) {
        self.prefs.preset = preset;
        self.prefs.widgets = preset.default_slots();
        self.prefs.sanitize();
    }

    pub fn toggle_widget_visible(&mut self, kind: DashboardWidgetKind) {
        if let Some(slot) = self.prefs.widgets.iter_mut().find(|w| w.kind == kind) {
            slot.visible = !slot.visible;
        }
    }

    pub fn toggle_widget_collapsed(&mut self, kind: DashboardWidgetKind) {
        if let Some(slot) = self.prefs.widgets.iter_mut().find(|w| w.kind == kind) {
            slot.collapsed = !slot.collapsed;
        }
    }

    pub fn toggle_widget_span(&mut self, kind: DashboardWidgetKind) {
        if let Some(slot) = self.prefs.widgets.iter_mut().find(|w| w.kind == kind) {
            slot.span = slot.span.toggle();
        }
    }

    pub fn move_widget(&mut self, kind: DashboardWidgetKind, delta: isize) {
        let Some(idx) = self.prefs.widgets.iter().position(|w| w.kind == kind) else {
            return;
        };
        let target = idx as isize + delta;
        if target < 0 || target >= self.prefs.widgets.len() as isize {
            return;
        }
        self.prefs.widgets.swap(idx, target as usize);
    }

    pub fn reset_layout(&mut self) {
        let macros = std::mem::take(&mut self.prefs.custom_macros);
        self.prefs = DashboardPreferences {
            custom_macros: macros,
            ..DashboardPreferences::default()
        };
    }

    pub fn add_custom_macro(&mut self) -> bool {
        let label = self.macro_label_input.trim();
        let prompt = self.macro_prompt_input.trim();
        if label.is_empty() || prompt.is_empty() {
            return false;
        }
        self.prefs.custom_macros.push(PromptMacro {
            label: label.to_string(),
            prompt: prompt.to_string(),
        });
        self.macro_label_input.clear();
        self.macro_prompt_input.clear();
        true
    }

    pub fn remove_custom_macro(&mut self, idx: usize) -> bool {
        if idx < self.prefs.custom_macros.len() {
            self.prefs.custom_macros.remove(idx);
            true
        } else {
            false
        }
    }

    /// Record normalized RAM utilization ratio (0.0..=1.0) into a bounded ring buffer.
    pub fn record_telemetry_sample(&mut self, telemetry: &CoreTelemetry) {
        let vm = &telemetry.dashboard;
        let total = vm.total_ram_mib.max(1) as f32;
        let avail = (vm.available_ram_mib as f32).min(total);
        let used_ratio = ((total - avail) / total).clamp(0.02, 0.99);
        if self.ram_history.len() >= TELEMETRY_HISTORY_CAP {
            self.ram_history.pop_front();
        }
        self.ram_history.push_back(used_ratio);
    }

    /// Throttle periodic sampling using wall-clock time so 60 FPS animations don't spam probes.
    pub fn should_sample_on_tick(&mut self) -> bool {
        if self.motion.last_telemetry_at.elapsed() >= Duration::from_millis(1400) {
            self.motion.last_telemetry_at = Instant::now();
            true
        } else {
            false
        }
    }
}

/// Sleek 3px animated cyberpunk circuit scan & packet pulse bar placed beneath the header.
pub struct CyberScanStripCanvas {
    pub primary: Color,
    pub secondary: Color,
    pub rail: Color,
    pub phase: f32,
    pub sweep: f32,
    pub active_run: bool,
    pub motion_enabled: bool,
}

impl canvas::Program<Message> for CyberScanStripCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width.max(40.0);
        let h = bounds.height.max(2.0);
        let mid_y = h * 0.5;

        frame.stroke(
            &canvas::Path::line(Point::new(0.0, mid_y), Point::new(w, mid_y)),
            canvas::Stroke::default()
                .with_color(Color {
                    a: 0.42,
                    ..self.rail
                })
                .with_width(1.0),
        );

        if !self.motion_enabled {
            return vec![frame.into_geometry()];
        }

        // Gliding smooth cyber packet beam
        let cycle = (self.phase * if self.active_run { 0.42 } else { 0.16 }).sin() * 0.5 + 0.5;
        let eased = cycle * cycle * (3.0 - 2.0 * cycle);
        let beam_w = if self.active_run { 160.0 } else { 110.0 };
        let head_x = eased * (w - beam_w) + beam_w * 0.5;
        let tail_start = (head_x - beam_w * 0.5).max(0.0);
        let tail_end = (head_x + beam_w * 0.5).min(w);

        frame.stroke(
            &canvas::Path::line(Point::new(tail_start, mid_y), Point::new(tail_end, mid_y)),
            canvas::Stroke::default()
                .with_color(Color {
                    a: if self.active_run { 0.48 } else { 0.28 },
                    ..self.primary
                })
                .with_width(2.5)
                .with_line_cap(canvas::LineCap::Round),
        );

        let core_w = 24.0;
        frame.stroke(
            &canvas::Path::line(
                Point::new((head_x - core_w).max(0.0), mid_y),
                Point::new((head_x + core_w).min(w), mid_y),
            ),
            canvas::Stroke::default()
                .with_color(Color {
                    a: 0.92,
                    ..self.secondary
                })
                .with_width(2.5)
                .with_line_cap(canvas::LineCap::Round),
        );

        if self.sweep > 0.01 {
            let sweep_x = (1.0 - self.sweep) * w;
            let sx0 = (sweep_x - 90.0).max(0.0);
            let sx1 = (sweep_x + 90.0).min(w);
            frame.stroke(
                &canvas::Path::line(Point::new(sx0, mid_y), Point::new(sx1, mid_y)),
                canvas::Stroke::default()
                    .with_color(Color {
                        a: self.sweep * 0.85,
                        ..self.primary
                    })
                    .with_width(3.0),
            );
        }

        vec![frame.into_geometry()]
    }
}

/// Custom vector arc gauge drawn with `iced::widget::canvas` with smooth motion & cyber reticle.
struct ReactorArcCanvas {
    ratio: f32,
    accent: Color,
    secondary: Color,
    track: Color,
    phase: f32,
    breath: f32,
    motion_enabled: bool,
}

impl canvas::Program<Message> for ReactorArcCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let side = bounds.width.min(bounds.height);
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = (side / 2.0 - 8.0).max(16.0);
        let start = PI * 0.75;
        let sweep = PI * 1.5;

        let track_path = canvas::Path::new(|b| {
            b.arc(canvas::path::Arc {
                center,
                radius,
                start_angle: Radians(start),
                end_angle: Radians(start + sweep),
            });
        });
        frame.stroke(
            &track_path,
            canvas::Stroke::default()
                .with_color(self.track)
                .with_width(6.0)
                .with_line_cap(canvas::LineCap::Round),
        );

        let clamped = self.ratio.clamp(0.03, 1.0);
        let fill_end = start + sweep * clamped;
        let fill_color = if clamped > 0.86 {
            self.secondary
        } else {
            self.accent
        };
        let active_path = canvas::Path::new(|b| {
            b.arc(canvas::path::Arc {
                center,
                radius,
                start_angle: Radians(start),
                end_angle: Radians(fill_end),
            });
        });

        // Soft animated neon bloom under the active gauge arc
        frame.stroke(
            &active_path,
            canvas::Stroke::default()
                .with_color(Color {
                    a: (0.16 * self.breath).clamp(0.08, 0.30),
                    ..fill_color
                })
                .with_width(11.0)
                .with_line_cap(canvas::LineCap::Round),
        );

        frame.stroke(
            &active_path,
            canvas::Stroke::default()
                .with_color(fill_color)
                .with_width(6.0)
                .with_line_cap(canvas::LineCap::Round),
        );

        let dot_pos = Point::new(
            center.x + radius * fill_end.cos(),
            center.y + radius * fill_end.sin(),
        );
        if self.motion_enabled {
            frame.fill(
                &canvas::Path::circle(dot_pos, 5.2 + 1.4 * (self.breath - 0.84)),
                Color {
                    a: 0.28,
                    ..fill_color
                },
            );
        }
        frame.fill(&canvas::Path::circle(dot_pos, 3.8), fill_color);

        // Rotating segmented inner cyberpunk reticle
        let inner_r = (radius - 10.0).max(8.0);
        let rot = if self.motion_enabled {
            self.phase * 0.45
        } else {
            0.0
        };
        for seg in 0..4 {
            let s = rot + seg as f32 * (TAU * 0.25);
            let e = s + 0.85;
            let seg_path = canvas::Path::new(|b| {
                b.arc(canvas::path::Arc {
                    center,
                    radius: inner_r,
                    start_angle: Radians(s),
                    end_angle: Radians(e),
                });
            });
            frame.stroke(
                &seg_path,
                canvas::Stroke::default()
                    .with_color(Color {
                        a: if seg % 2 == 0 { 0.40 } else { 0.18 },
                        ..fill_color
                    })
                    .with_width(1.2),
            );
        }

        vec![frame.into_geometry()]
    }
}

/// Custom cyberpunk vector telemetry sparkline drawn with `iced::widget::canvas`.
struct TelemetryWaveformCanvas<'a> {
    samples: &'a VecDeque<f32>,
    accent: Color,
    grid_color: Color,
    show_grid: bool,
    phase: f32,
    breath: f32,
    motion_enabled: bool,
}

impl canvas::Program<Message> for TelemetryWaveformCanvas<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width.max(40.0);
        let h = bounds.height.max(40.0);
        let pad_x = 6.0;
        let pad_y = 8.0;
        let plot_w = (w - pad_x * 2.0).max(10.0);
        let plot_h = (h - pad_y * 2.0).max(10.0);

        if self.show_grid {
            let dash_offset = if self.motion_enabled {
                (self.phase * 10.0) as usize
            } else {
                0
            };
            for frac in [0.25_f32, 0.5, 0.75] {
                let y = pad_y + plot_h * frac;
                let line = canvas::Path::line(Point::new(pad_x, y), Point::new(w - pad_x, y));
                frame.stroke(
                    &line,
                    canvas::Stroke {
                        style: canvas::Style::Solid(Color {
                            a: 0.45,
                            ..self.grid_color
                        }),
                        width: 1.0,
                        line_dash: canvas::LineDash {
                            segments: &[4.0, 5.0],
                            offset: dash_offset,
                        },
                        ..canvas::Stroke::default()
                    },
                );
            }
        }

        let count = self.samples.len();
        if count == 0 {
            return vec![frame.into_geometry()];
        }

        let mut pts = Vec::with_capacity(count.max(2));
        if count == 1 {
            let val = self.samples[0].clamp(0.02, 0.98);
            let y = pad_y + plot_h * (1.0 - val);
            pts.push(Point::new(pad_x, y));
            pts.push(Point::new(w - pad_x, y));
        } else {
            let step_x = plot_w / (count - 1) as f32;
            for (i, &val) in self.samples.iter().enumerate() {
                let x = pad_x + i as f32 * step_x;
                let y = pad_y + plot_h * (1.0 - val.clamp(0.02, 0.98));
                pts.push(Point::new(x, y));
            }
        }

        let trace_smooth_wave = |b: &mut canvas::path::Builder| {
            b.move_to(pts[0]);
            if pts.len() == 2 {
                b.line_to(pts[1]);
            } else {
                for win in pts.windows(2) {
                    let mid = Point::new((win[0].x + win[1].x) * 0.5, (win[0].y + win[1].y) * 0.5);
                    b.quadratic_curve_to(win[0], mid);
                }
                if let Some(last) = pts.last() {
                    b.line_to(*last);
                }
            }
        };

        let area_path = canvas::Path::new(|b| {
            trace_smooth_wave(b);
            if let Some(last) = pts.last() {
                b.line_to(Point::new(last.x, pad_y + plot_h));
            }
            b.line_to(Point::new(pts[0].x, pad_y + plot_h));
            b.close();
        });
        frame.fill(
            &area_path,
            Color {
                a: 0.14,
                ..self.accent
            },
        );

        let line_path = canvas::Path::new(trace_smooth_wave);
        frame.stroke(
            &line_path,
            canvas::Stroke::default()
                .with_color(Color {
                    a: 0.22,
                    ..self.accent
                })
                .with_width(4.5)
                .with_line_cap(canvas::LineCap::Round)
                .with_line_join(canvas::LineJoin::Round),
        );
        frame.stroke(
            &line_path,
            canvas::Stroke::default()
                .with_color(self.accent)
                .with_width(1.9)
                .with_line_cap(canvas::LineCap::Round)
                .with_line_join(canvas::LineJoin::Round),
        );

        if let Some(last) = pts.last() {
            let halo_r = if self.motion_enabled {
                5.5 + 3.2 * (0.5 + 0.5 * (self.phase * 2.8).sin())
            } else {
                5.5
            };
            frame.fill(
                &canvas::Path::circle(*last, halo_r),
                Color {
                    a: (0.24 * self.breath).clamp(0.12, 0.36),
                    ..self.accent
                },
            );
            frame.fill(&canvas::Path::circle(*last, 3.0), self.accent);
        }

        vec![frame.into_geometry()]
    }
}

/// Top-level renderer for the redesigned modern-minimal cyberpunk `#dashboard` workspace.
pub fn view_dashboard<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let gap = app.dashboard.prefs.density.grid_gap();

    let mut content = Column::new().spacing(gap).padding(Padding::from([14, 18]));
    content = content.push(view_hud_bar(app, pal));

    if app.dashboard.customize_open {
        content = content.push(view_customize_drawer(app, pal));
    }

    content = content.push(view_bento_grid(app, pal, gap));

    scrollable(content)
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
}

fn view_hud_bar<'a>(app: &'a LoraDesktopApp, pal: CyberPalette) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let card_rad = app.dashboard.card_radius();

    let mut presets = Row::new().spacing(6).align_y(Alignment::Center);
    presets = presets.push(
        text("LAYOUT")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for preset in DashboardPreset::ALL {
        let active = app.dashboard.prefs.preset == preset;
        presets = presets.push(
            button(text(preset.label()).size(11))
                .padding(Padding::from([5, 10]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectDashboardPreset(preset)),
        );
    }

    let visible_count = app
        .dashboard
        .prefs
        .widgets
        .iter()
        .filter(|w| w.visible)
        .count();

    let theme_pill = container(
        text(format!(
            "{} · {visible_count}/{}",
            app.dashboard.active_theme_code(),
            DashboardWidgetKind::ALL.len()
        ))
        .size(10)
        .font(Font::MONOSPACE)
        .wrapping(text::Wrapping::None)
        .color(pal.accent_secondary),
    )
    .padding(Padding::from([4, 10]))
    .style(theme::pill_container(
        pal.accent_secondary_soft,
        pal.accent_secondary,
        Color {
            a: 0.42,
            ..pal.accent_secondary
        },
    ));

    let customize_btn = button(
        text(if app.dashboard.customize_open {
            "Done"
        } else {
            "Customize HUD"
        })
        .size(11)
        .wrapping(text::Wrapping::None),
    )
    .padding(Padding::from([5, 12]))
    .style(theme::cyber_chip_button_with_radius(
        app.dashboard.customize_open,
        pal.accent_primary,
        pal.bg_elevated,
        pal.border_accent,
        pal.text_primary,
        pal.text_primary,
        c_rad,
    ))
    .on_press(Message::ToggleCustomizeHud);

    let bar = row![presets, space::horizontal(), theme_pill, customize_btn]
        .spacing(10)
        .align_y(Alignment::Center);

    container(bar)
        .padding(Padding::from([8, 12]))
        .width(Length::Fill)
        .style(theme::palette_cyber_card(
            pal.bg_card,
            pal.text_primary,
            pal.border_subtle,
            None,
            card_rad,
        ))
        .into()
}

fn view_customize_drawer<'a>(app: &'a LoraDesktopApp, pal: CyberPalette) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let card_rad = app.dashboard.card_radius();

    // Render all 11 iconic Cyberpunk game/cinema/anime themes cleanly in 3 wrapped rows.
    let mut gallery_rows = Column::new().spacing(6);
    for chunk in CyberThemePreset::ALL.chunks(4) {
        let mut r = Row::new().spacing(6);
        for &t in chunk {
            let active = app.dashboard.prefs.theme_preset == t;
            let tp = t.palette();
            let swatch = container(space::Space::new().width(8).height(8)).style(
                theme::pill_container(tp.accent_primary, tp.accent_primary, tp.accent_secondary),
            );
            let card_btn = button(
                row![
                    swatch,
                    column![
                        text(t.label()).size(11).color(if active {
                            pal.text_primary
                        } else {
                            pal.text_secondary
                        }),
                        text(t.origin())
                            .size(9)
                            .font(Font::MONOSPACE)
                            .color(pal.text_muted),
                    ]
                    .spacing(1),
                ]
                .spacing(7)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([6, 9]))
            .width(Length::FillPortion(1))
            .style(theme::cyber_chip_button_with_radius(
                active,
                tp.accent_primary,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.text_muted,
                c_rad,
            ))
            .on_press(Message::SelectCyberTheme(t));
            r = r.push(card_btn);
        }
        for _ in chunk.len()..4 {
            r = r.push(space::Space::new().width(Length::FillPortion(1)));
        }
        gallery_rows = gallery_rows.push(r);
    }

    let mut density_row = Row::new().spacing(6).align_y(Alignment::Center);
    density_row = density_row.push(
        text("DENSITY")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for d in HudDensity::ALL {
        let active = app.dashboard.prefs.density == d;
        density_row = density_row.push(
            button(text(d.label()).size(11))
                .padding(Padding::from([4, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectHudDensity(d)),
        );
    }

    let mut geom_row = Row::new().spacing(6).align_y(Alignment::Center);
    geom_row = geom_row.push(
        text("FRAME")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for g in HudGeometry::ALL {
        let active = app.dashboard.prefs.geometry == g;
        geom_row = geom_row.push(
            button(text(g.label()).size(11))
                .padding(Padding::from([4, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    g.control_radius(),
                ))
                .on_press(Message::SelectHudGeometry(g)),
        );
    }

    let toggles_row = row![
        checkbox(app.dashboard.prefs.neon_borders)
            .label("Neon Border Glow")
            .size(14)
            .text_size(11)
            .on_toggle(Message::ToggleNeonBorders),
        checkbox(app.dashboard.prefs.show_sparkline_grid)
            .label("Telemetry Grid")
            .size(14)
            .text_size(11)
            .on_toggle(Message::ToggleSparklineGrid),
        space::horizontal(),
        button(text("Reset Default Layout").size(11))
            .padding(Padding::from([5, 10]))
            .style(theme::solid_button_with_radius(
                pal.bg_elevated,
                pal.critical,
                pal.text_primary,
                c_rad,
            ))
            .on_press(Message::ResetDashboardLayout),
    ]
    .spacing(14)
    .align_y(Alignment::Center);

    let mut left_col = Column::new().spacing(6).width(Length::FillPortion(1));
    let mut right_col = Column::new().spacing(6).width(Length::FillPortion(1));

    for (idx, slot) in app.dashboard.prefs.widgets.iter().enumerate() {
        let kind = slot.kind;
        let item = container(
            row![
                button(text(if slot.visible { "ON" } else { "OFF" }).size(10))
                    .padding(Padding::from([3, 8]))
                    .style(theme::cyber_chip_button_with_radius(
                        slot.visible,
                        pal.nominal,
                        pal.bg_card,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_muted,
                        c_rad,
                    ))
                    .on_press(Message::ToggleWidgetVisible(kind)),
                text(format!("{} · {}", kind.code(), kind.title()))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(if slot.visible {
                        pal.text_primary
                    } else {
                        pal.text_muted
                    })
                    .width(Length::Fill),
                button(text(slot.span.badge()).size(10))
                    .padding(Padding::from([3, 7]))
                    .style(theme::cyber_chip_button_with_radius(
                        slot.span == WidgetSpan::Wide,
                        pal.accent_secondary,
                        pal.bg_card,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_muted,
                        c_rad,
                    ))
                    .on_press(Message::ToggleWidgetSpan(kind)),
                button(text("Up").size(10))
                    .padding(Padding::from([3, 7]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_card,
                        pal.accent_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::MoveWidgetUp(kind)),
                button(text("Dn").size(10))
                    .padding(Padding::from([3, 7]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_card,
                        pal.accent_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::MoveWidgetDown(kind)),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([6, 8]))
        .width(Length::Fill)
        .style(theme::card_container(
            pal.bg_elevated,
            pal.border_subtle,
            c_rad,
        ));

        if idx % 2 == 0 {
            left_col = left_col.push(item);
        } else {
            right_col = right_col.push(item);
        }
    }

    container(
        column![
            row![
                text("CYBERPUNK UNIVERSE THEMES & WIDGET STUDIO")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.accent_primary),
                space::horizontal(),
                text("Saved locally in dashboard_hud.json")
                    .size(10)
                    .color(pal.text_muted),
            ]
            .align_y(Alignment::Center),
            rule::horizontal(1).style(theme::divider_rule),
            gallery_rows,
            row![density_row, space::horizontal(), geom_row].align_y(Alignment::Center),
            toggles_row,
            row![left_col, right_col].spacing(8),
        ]
        .spacing(10),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::cyber_card(
        pal.bg_card,
        pal.border_accent,
        Some(pal.accent_primary),
        card_rad,
    ))
    .into()
}

fn view_bento_grid<'a>(
    app: &'a LoraDesktopApp,
    pal: CyberPalette,
    gap: f32,
) -> Element<'a, Message> {
    let card_rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();
    let visible: Vec<&WidgetSlot> = app
        .dashboard
        .prefs
        .widgets
        .iter()
        .filter(|w| w.visible)
        .collect();

    if visible.is_empty() {
        return container(
            column![
                text("NO ACTIVE DASHBOARD MODULES")
                    .size(13)
                    .font(Font::MONOSPACE)
                    .color(pal.accent_primary),
                text("Open 'Customize HUD' above or pick a Layout Preset to restore widgets.")
                    .size(12)
                    .color(pal.text_muted),
                button(text("Restore Operator HUD").size(12))
                    .padding(Padding::from([8, 16]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_primary,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::SelectDashboardPreset(DashboardPreset::OperatorHud)),
            ]
            .spacing(10)
            .align_x(Alignment::Center),
        )
        .padding(32)
        .width(Length::Fill)
        .align_x(Horizontal::Center)
        .style(theme::cyber_card(
            pal.bg_card,
            pal.border_subtle,
            None,
            card_rad,
        ))
        .into();
    }

    let mut grid = Column::new().spacing(gap);
    let mut pending_half: Option<Element<'a, Message>> = None;

    for slot in visible {
        let rendered = render_widget_slot(app, slot, pal);
        match slot.span {
            WidgetSpan::Wide => {
                if let Some(left) = pending_half.take() {
                    grid = grid.push(container(left).width(Length::Fill));
                }
                grid = grid.push(rendered);
            }
            WidgetSpan::Half => {
                if let Some(left) = pending_half.take() {
                    grid = grid.push(
                        row![
                            container(left).width(Length::FillPortion(1)),
                            container(rendered).width(Length::FillPortion(1)),
                        ]
                        .spacing(gap),
                    );
                } else {
                    pending_half = Some(rendered);
                }
            }
        }
    }

    if let Some(tail) = pending_half.take() {
        grid = grid.push(container(tail).width(Length::Fill));
    }

    grid.width(Length::Fill).into()
}

fn render_widget_slot<'a>(
    app: &'a LoraDesktopApp,
    slot: &WidgetSlot,
    pal: CyberPalette,
) -> Element<'a, Message> {
    let body: Option<Element<'a, Message>> = if slot.collapsed {
        None
    } else {
        Some(match slot.kind {
            DashboardWidgetKind::CommandDeck => widget_command_deck(app, pal),
            DashboardWidgetKind::CapriCompanion => widget_capri_companion(app, pal),
            DashboardWidgetKind::SystemReactor => widget_system_reactor(app, pal),
            DashboardWidgetKind::TelemetrySparkline => widget_telemetry_sparkline(app, pal),
            DashboardWidgetKind::ModelArsenal => widget_model_arsenal(app, pal),
            DashboardWidgetKind::WorkspaceShield => widget_workspace_shield(app, pal),
            DashboardWidgetKind::WorkflowMatrix => widget_workflow_matrix(app, pal),
            DashboardWidgetKind::AttestationRadar => widget_attestation_radar(app, pal),
            DashboardWidgetKind::OperatorScratchpad => widget_operator_scratchpad(app, pal),
        })
    };

    widget_frame(app, slot, pal, body)
}

fn widget_frame<'a>(
    app: &'a LoraDesktopApp,
    slot: &WidgetSlot,
    pal: CyberPalette,
    body: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let kind = slot.kind;
    let pad = app.dashboard.prefs.density.card_padding();
    let card_rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();
    let customizing = app.dashboard.customize_open;

    let semantic_accent = match kind {
        DashboardWidgetKind::CommandDeck | DashboardWidgetKind::CapriCompanion => {
            pal.accent_primary
        }
        DashboardWidgetKind::SystemReactor
        | DashboardWidgetKind::TelemetrySparkline
        | DashboardWidgetKind::ModelArsenal => pal.accent_secondary,
        DashboardWidgetKind::WorkspaceShield | DashboardWidgetKind::AttestationRadar => pal.nominal,
        DashboardWidgetKind::WorkflowMatrix | DashboardWidgetKind::OperatorScratchpad => {
            pal.warning
        }
    };
    let indicator_dot = container(space::Space::new().width(6).height(6)).style(
        theme::pill_container(semantic_accent, semantic_accent, semantic_accent),
    );

    let mut header = row![
        indicator_dot,
        text(kind.code())
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.accent_secondary),
        text(kind.title())
            .size(11)
            .font(Font::MONOSPACE)
            .color(pal.text_secondary)
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    if customizing {
        header = header
            .push(
                button(text(slot.span.badge()).size(10))
                    .padding(Padding::from([2, 6]))
                    .style(theme::cyber_chip_button_with_radius(
                        slot.span == WidgetSpan::Wide,
                        pal.accent_secondary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_muted,
                        c_rad,
                    ))
                    .on_press(Message::ToggleWidgetSpan(kind)),
            )
            .push(
                button(text("^").size(10))
                    .padding(Padding::from([2, 6]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_elevated,
                        pal.accent_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::MoveWidgetUp(kind)),
            )
            .push(
                button(text("v").size(10))
                    .padding(Padding::from([2, 6]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_elevated,
                        pal.accent_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::MoveWidgetDown(kind)),
            )
            .push(
                button(text("x").size(10))
                    .padding(Padding::from([2, 6]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_elevated,
                        pal.critical,
                        pal.text_primary,
                        c_rad,
                    ))
                    .on_press(Message::ToggleWidgetVisible(kind)),
            );
    }

    header = header.push(
        button(text(if slot.collapsed { "+" } else { "-" }).size(11))
            .padding(Padding::from([1, 7]))
            .style(theme::solid_button_with_radius(
                pal.bg_elevated,
                pal.accent_primary,
                pal.text_muted,
                c_rad,
            ))
            .on_press(Message::ToggleWidgetCollapsed(kind)),
    );

    let mut col = Column::new().spacing(10).push(header);
    if let Some(content) = body {
        col = col.push(rule::horizontal(1).style(theme::palette_divider_rule(pal)));
        col = col.push(content);
    }

    let is_hero = kind == DashboardWidgetKind::CommandDeck;
    let breath = app.dashboard.glow_breath();
    let glow_alpha =
        ((app.dashboard.prefs.glow_strength_pct as f32 / 100.0) * 0.23 * breath).clamp(0.02, 0.34);
    let (border_color, glow) = if customizing {
        (
            pal.border_accent,
            Some(Color {
                a: glow_alpha,
                ..pal.accent_secondary
            }),
        )
    } else if app.dashboard.prefs.neon_borders
        && is_hero
        && app.dashboard.prefs.glow_strength_pct > 0
    {
        (
            Color {
                a: (0.62 * breath).clamp(0.45, 0.92),
                ..pal.accent_primary
            },
            Some(Color {
                a: glow_alpha,
                ..pal.accent_primary
            }),
        )
    } else {
        (pal.border_subtle, None)
    };

    container(col)
        .padding(pad)
        .width(Length::Fill)
        .style(theme::palette_cyber_card(
            pal.bg_card,
            pal.text_primary,
            border_color,
            glow,
            card_rad,
        ))
        .into()
}

fn widget_command_deck<'a>(app: &'a LoraDesktopApp, pal: CyberPalette) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let mut prompt_row = Row::new().spacing(8).align_y(Alignment::Center);
    if app.dashboard.prefs.show_mini_bot {
        prompt_row = prompt_row.push(crate::ux_studio::view_mini_bot_avatar(app, 48.0, None));
    }
    let prompt_bar = prompt_row
        .push(
            text_input(
                "Ask Lora anything locally or jump straight into execution...",
                &app.dashboard.quick_prompt_input,
            )
            .on_input(Message::QuickPromptChanged)
            .on_submit(Message::SubmitQuickPrompt)
            .padding(10)
            .size(13)
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
        )
        .push(
            button(text("Execute in #chat").size(12))
                .padding(Padding::from([10, 16]))
                .style(theme::solid_button_with_radius(
                    pal.accent_primary,
                    pal.accent_secondary,
                    pal.on_accent_text,
                    c_rad,
                ))
                .on_press(Message::SubmitQuickPrompt),
        );

    let mut macros_row = Row::new().spacing(6).align_y(Alignment::Center);
    macros_row = macros_row.push(
        text("MACROS")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
    );
    for m in app.dashboard.prefs.custom_macros.iter().take(4) {
        macros_row = macros_row.push(
            button(text(&m.label).size(11))
                .padding(Padding::from([4, 10]))
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

    let jumps = row![
        text("OPEN")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
        jump_btn("Agent Chat", ChannelTab::Chat, pal, c_rad),
        jump_btn("Code & Env", ChannelTab::CodeWorkspace, pal, c_rad),
        jump_btn("Tasks & Daily", ChannelTab::WorkflowPipeline, pal, c_rad),
        jump_btn("Projects", ChannelTab::AgentWorkspace, pal, c_rad),
        jump_btn("llama.cpp", ChannelTab::Hardware, pal, c_rad),
        jump_btn("Models", ChannelTab::ModelLibrary, pal, c_rad),
        jump_btn("Studio", ChannelTab::Settings, pal, c_rad),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    column![prompt_bar, macros_row, jumps].spacing(8).into()
}

fn widget_capri_companion<'a>(app: &'a LoraDesktopApp, pal: CyberPalette) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let capri = &app.dashboard.prefs.capri;
    let phase = app.agent_phase();
    let usage = crate::ux_studio::compute_context_budget_usage(
        &app.messages,
        &app.staged_attachments,
        &app.composer_input,
        app.requested_context,
    );
    let trust = app
        .workspace_db
        .folder_trust_for_path(app.active_project_id, &app.workspace_root_input);
    let scale = (capri.widget_scale_px as f32).clamp(44.0, 76.0);

    let mode_tag = format!(
        "{} · {} · Effort:{} · Trust:{}",
        app.access_mode.label(),
        app.agent_kind.label(),
        app.reasoning_effort.label(),
        trust.label()
    );

    let info_col = column![
        row![
            text(format!("CAPRI × LORA · {}", phase.label()))
                .size(11)
                .font(Font::MONOSPACE)
                .color(pal.accent_primary),
            text(mode_tag)
                .size(10)
                .font(Font::MONOSPACE)
                .color(pal.accent_secondary),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        text(app.bot_quip.as_str())
            .size(11)
            .color(pal.text_secondary),
        row![
            text(format!(
                "CTX: {} / {} tok ({:.0}%)",
                usage.total_tokens, usage.max_tokens, usage.pct
            ))
            .size(10)
            .font(Font::MONOSPACE)
            .color(if usage.pct > 80.0 {
                pal.warning
            } else {
                pal.nominal
            }),
            space::Space::new().width(8),
            progress_bar(0.0..=100.0, usage.pct),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(5)
    .width(Length::Fill);

    let quick_controls = column![
        row![
            button(
                text(format!("Aura: {}", capri.aura_preset.label()))
                    .size(10)
                    .font(Font::MONOSPACE)
            )
            .padding(Padding::from([4, 9]))
            .style(theme::cyber_chip_button_with_radius(
                true,
                pal.accent_primary,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.text_secondary,
                c_rad,
            ))
            .on_press(Message::CapriAuraPresetChanged(capri.aura_preset.next())),
            button(
                text(if capri.always_toothy_grin {
                    "Grin: Teeth"
                } else {
                    "Mouth: Auto"
                })
                .size(10)
                .font(Font::MONOSPACE)
            )
            .padding(Padding::from([4, 9]))
            .style(theme::cyber_chip_button_with_radius(
                capri.always_toothy_grin,
                pal.accent_secondary,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.text_secondary,
                c_rad,
            ))
            .on_press(Message::CapriToggleGrin(!capri.always_toothy_grin)),
        ]
        .spacing(6),
        row![
            button(
                text(format!("Route: {}", app.access_mode.label()))
                    .size(10)
                    .font(Font::MONOSPACE)
            )
            .padding(Padding::from([4, 9]))
            .style(theme::cyber_chip_button_with_radius(
                app.access_mode == crate::workspace_db::ExecutionAccessMode::Api,
                pal.warning,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.nominal,
                c_rad,
            ))
            .on_press(Message::SelectAccessMode(match app.access_mode {
                crate::workspace_db::ExecutionAccessMode::Local => {
                    crate::workspace_db::ExecutionAccessMode::Api
                }
                crate::workspace_db::ExecutionAccessMode::Api => {
                    crate::workspace_db::ExecutionAccessMode::Local
                }
            })),
            button(text("Customize Capri ->").size(10).font(Font::MONOSPACE))
                .padding(Padding::from([4, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    false,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.accent_secondary,
                    c_rad,
                ))
                .on_press(Message::SelectChannel(ChannelTab::Settings)),
        ]
        .spacing(6),
    ]
    .spacing(5)
    .align_x(Horizontal::Right);

    row![
        crate::ux_studio::view_mini_bot_avatar(app, scale, None),
        info_col,
        quick_controls,
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

fn jump_btn<'a>(
    label: &'a str,
    target: ChannelTab,
    pal: CyberPalette,
    radius: f32,
) -> Element<'a, Message> {
    button(text(label).size(11).font(Font::MONOSPACE))
        .padding(Padding::from([4, 9]))
        .style(theme::cyber_chip_button_with_radius(
            false,
            pal.accent_primary,
            pal.bg_elevated,
            pal.border_subtle,
            pal.text_primary,
            pal.text_muted,
            radius,
        ))
        .on_press(Message::SelectChannel(target))
        .into()
}

fn widget_system_reactor<'a>(app: &'a LoraDesktopApp, pal: CyberPalette) -> Element<'a, Message> {
    let vm = &app.telemetry.dashboard;
    let total = vm.total_ram_mib.max(1) as f32;
    let avail = (vm.available_ram_mib as f32).min(total);
    let used_mib = vm.total_ram_mib.saturating_sub(vm.available_ram_mib);
    let used_ratio = if app.dashboard.prefs.cyber_motion {
        app.dashboard.motion.ram_ratio.clamp(0.0, 1.0)
    } else {
        ((total - avail) / total).clamp(0.0, 1.0)
    };
    let pct = (used_ratio * 100.0).round() as u32;

    let arc = canvas(ReactorArcCanvas {
        ratio: used_ratio,
        accent: pal.accent_secondary,
        secondary: pal.warning,
        track: pal.bg_elevated,
        phase: app.dashboard.motion.phase,
        breath: app.dashboard.glow_breath(),
        motion_enabled: app.dashboard.prefs.cyber_motion,
    })
    .width(Length::Fixed(104.0))
    .height(Length::Fixed(104.0));

    let gauge = iced::widget::stack![
        arc,
        container(
            column![
                text(format!("{pct}%"))
                    .size(18)
                    .font(Font::MONOSPACE)
                    .color(pal.text_primary),
                text("RAM USED")
                    .size(9)
                    .font(Font::MONOSPACE)
                    .color(pal.text_muted),
            ]
            .align_x(Alignment::Center),
        )
        .width(Length::Fixed(104.0))
        .height(Length::Fixed(104.0))
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center),
    ];

    let headroom_color = if vm.available_ram_mib < 2048 {
        pal.warning
    } else {
        pal.nominal
    };

    let stats = column![
        text(&vm.cpu_summary).size(12).color(pal.text_primary),
        text(format!(
            "Free: {} MiB  |  Used: {} MiB  |  Total: {} MiB",
            vm.available_ram_mib, used_mib, vm.total_ram_mib
        ))
        .size(11)
        .font(Font::MONOSPACE)
        .color(headroom_color),
        text(format!(
            "Worker: {}  ·  State: {}",
            vm.status_strip.execution_destination,
            if app.active_run_id.is_some() {
                "INFERENCE ACTIVE"
            } else {
                "AIR-GAPPED IDLE"
            }
        ))
        .size(11)
        .font(Font::MONOSPACE)
        .color(pal.text_muted),
        progress_bar(0.0..=1.0, used_ratio).girth(5.0),
    ]
    .spacing(6)
    .width(Length::Fill);

    row![gauge, stats]
        .spacing(14)
        .align_y(Alignment::Center)
        .into()
}

fn widget_telemetry_sparkline<'a>(
    app: &'a LoraDesktopApp,
    pal: CyberPalette,
) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let history = &app.dashboard.ram_history;
    let cur = history.back().copied().unwrap_or(0.0) * 100.0;
    let min = history
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min)
        .clamp(0.0, 1.0)
        * 100.0;
    let max = history
        .iter()
        .copied()
        .fold(0.0_f32, f32::max)
        .clamp(0.0, 1.0)
        * 100.0;

    let chart = canvas(TelemetryWaveformCanvas {
        samples: history,
        accent: pal.accent_primary,
        grid_color: pal.border_subtle,
        show_grid: app.dashboard.prefs.show_sparkline_grid,
        phase: app.dashboard.motion.phase,
        breath: app.dashboard.glow_breath(),
        motion_enabled: app.dashboard.prefs.cyber_motion,
    })
    .width(Length::Fill)
    .height(Length::Fixed(86.0));

    let meta = row![
        text(format!("CUR {cur:.1}%"))
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.accent_primary),
        text(format!("MIN {min:.1}%"))
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.text_muted),
        text(format!("PEAK {max:.1}%"))
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.accent_secondary),
        space::horizontal(),
        button(text("Sample").size(10))
            .padding(Padding::from([3, 9]))
            .style(theme::cyber_chip_button_with_radius(
                false,
                pal.accent_secondary,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.text_muted,
                c_rad,
            ))
            .on_press(Message::RefreshTelemetry),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    column![chart, meta].spacing(8).into()
}

fn widget_model_arsenal<'a>(app: &'a LoraDesktopApp, pal: CyberPalette) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let mut model_chips = Row::new().spacing(6).align_y(Alignment::Center);
    if app.telemetry.models.is_empty() {
        model_chips = model_chips
            .push(text("No local GGUF loaded").size(11).color(pal.warning))
            .push(space::horizontal())
            .push(
                button(text("Import GGUF ->").size(11))
                    .padding(Padding::from([4, 10]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_primary,
                        pal.accent_secondary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::SelectChannel(ChannelTab::ModelLibrary)),
            );
    } else {
        for m in app.telemetry.models.iter().take(4) {
            let active = app.selected_model.as_deref() == Some(m.display_alias.as_str());
            model_chips = model_chips.push(
                button(text(&m.display_alias).size(11))
                    .padding(Padding::from([4, 9]))
                    .style(theme::cyber_chip_button_with_radius(
                        active,
                        pal.accent_primary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.text_secondary,
                        c_rad,
                    ))
                    .on_press(Message::SelectModel(m.display_alias.clone())),
            );
        }
    }

    let ctx_row = row![
        text(format!("CTX {}", app.requested_context))
            .size(11)
            .font(Font::MONOSPACE)
            .color(pal.accent_secondary)
            .width(Length::Fixed(74.0)),
        slider(
            512..=8192,
            app.requested_context,
            Message::ContextTokensChanged
        )
        .step(512_u32)
        .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut profile_row = Row::new().spacing(5).align_y(Alignment::Center);
    for (p, tag) in [
        (TuningProfile::Automatic, "AUTO"),
        (TuningProfile::Balanced, "BAL"),
        (TuningProfile::MaximumPerformance, "MAX"),
        (TuningProfile::LowMemory, "LOW-MEM"),
        (TuningProfile::BatterySaver, "ECO"),
    ] {
        let active = app.tuning_profile == p;
        profile_row = profile_row.push(
            button(text(tag).size(10).font(Font::MONOSPACE))
                .padding(Padding::from([4, 7]))
                .style(theme::cyber_chip_button_with_radius(
                    active,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectTuningProfile(p)),
        );
    }

    column![model_chips, ctx_row, profile_row].spacing(8).into()
}

fn compact_scope_tail(path: &str) -> String {
    let parts: Vec<&str> = path.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    if parts.len() <= 2 {
        path.to_string()
    } else {
        format!(".../{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    }
}

fn widget_workspace_shield<'a>(app: &'a LoraDesktopApp, pal: CyberPalette) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let scope_badge = row![
        text("SCOPE")
            .size(10)
            .font(Font::MONOSPACE)
            .color(pal.nominal),
        text(compact_scope_tail(&app.workspace_root_input))
            .size(11)
            .font(Font::MONOSPACE)
            .color(pal.text_secondary),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let root_input = text_input("Workspace root directory...", &app.workspace_root_input)
        .on_input(Message::WorkspacePathChanged)
        .padding(7)
        .size(11)
        .font(Font::MONOSPACE)
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
        .width(Length::Fill);

    let toggles = row![
        checkbox(app.read_only_assistant)
            .label("Read-Only Shield")
            .size(14)
            .text_size(11)
            .on_toggle(Message::ToggleAssistantMode),
        checkbox(app.use_rag)
            .label("Local RAG")
            .size(14)
            .text_size(11)
            .on_toggle(Message::ToggleRag),
    ]
    .spacing(14)
    .align_y(Alignment::Center);

    let actions = row![
        button(text("Open #agent-workspace").size(11))
            .padding(Padding::from([5, 10]))
            .style(theme::cyber_chip_button_with_radius(
                false,
                pal.accent_secondary,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.text_secondary,
                c_rad,
            ))
            .on_press(Message::SelectChannel(ChannelTab::AgentWorkspace)),
        button(text("Consent Modal Preview").size(11))
            .padding(Padding::from([5, 10]))
            .style(theme::cyber_chip_button_with_radius(
                false,
                pal.warning,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.warning,
                c_rad,
            ))
            .on_press(Message::PreviewConsentDialogPressed),
    ]
    .spacing(8);

    column![scope_badge, root_input, toggles, actions]
        .spacing(7)
        .into()
}

fn widget_workflow_matrix<'a>(app: &'a LoraDesktopApp, pal: CyberPalette) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let (checked, valid) = app.telemetry.workflow_integrity;
    let chain_ok = checked == valid;

    let status_line = row![
        text(format!("Receipts: {valid}/{checked} verified"))
            .size(12)
            .font(Font::MONOSPACE)
            .color(if chain_ok { pal.nominal } else { pal.critical }),
        space::horizontal(),
        button(text("Execute Pipeline").size(11))
            .padding(Padding::from([5, 12]))
            .style(theme::solid_button_with_radius(
                pal.accent_secondary,
                pal.accent_primary,
                pal.on_accent_text,
                c_rad,
            ))
            .on_press(Message::RunWorkflowPressed),
    ]
    .align_y(Alignment::Center);

    let latest = if let Some(rcpt) = app.telemetry.workflow_receipts.first() {
        let short_sha: String = rcpt.receipt_sha256.chars().take(18).collect();
        text(format!(
            "{} · {}/{} steps · {}ms · sha256:{short_sha}...",
            rcpt.workflow_id, rcpt.completed_steps, rcpt.total_steps, rcpt.total_duration_ms
        ))
        .size(11)
        .font(Font::MONOSPACE)
        .color(pal.text_muted)
    } else {
        text("No workflow receipts yet. Trigger local verification pipeline above.")
            .size(11)
            .color(pal.text_muted)
    };

    column![status_line, latest].spacing(8).into()
}

fn widget_attestation_radar<'a>(
    app: &'a LoraDesktopApp,
    pal: CyberPalette,
) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let vm = &app.telemetry.dashboard;
    let top = row![
        text(format!(
            "Audit: {}  ·  Tuning: {}",
            vm.recent_audit_count, app.telemetry.tuning_receipts_count
        ))
        .size(11)
        .font(Font::MONOSPACE)
        .color(pal.text_secondary),
        space::horizontal(),
        button(text("Recovery Drill").size(10))
            .padding(Padding::from([4, 9]))
            .style(theme::cyber_chip_button_with_radius(
                false,
                pal.warning,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.text_muted,
                c_rad,
            ))
            .on_press(Message::RunIncidentRecoveryPressed),
        button(text("Capture Snapshot").size(10))
            .padding(Padding::from([4, 10]))
            .style(theme::solid_button_with_radius(
                pal.nominal,
                pal.accent_secondary,
                pal.on_accent_text,
                c_rad,
            ))
            .on_press(Message::CaptureAttestedSnapshotPressed),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let snap_line = if let Some((snap, verified)) = &app.latest_snapshot {
        let short_sha: String = snap
            .snapshot_sha256
            .as_deref()
            .unwrap_or("none")
            .chars()
            .take(24)
            .collect();
        text(format!(
            "snapshot={:?} · verified={verified} · sha256:{short_sha}...",
            snap.screen
        ))
        .size(11)
        .font(Font::MONOSPACE)
        .color(if *verified { pal.nominal } else { pal.critical })
    } else {
        text("Domain-separated `lora.desktop.snapshot.v1` ready for capture.")
            .size(11)
            .color(pal.text_muted)
    };

    column![top, snap_line].spacing(8).into()
}

fn widget_operator_scratchpad<'a>(
    app: &'a LoraDesktopApp,
    pal: CyberPalette,
) -> Element<'a, Message> {
    let c_rad = app.dashboard.control_radius();
    let creator = row![
        text_input("Macro name...", &app.dashboard.macro_label_input)
            .on_input(Message::MacroLabelChanged)
            .padding(7)
            .size(11)
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
            .width(Length::Fixed(160.0)),
        text_input(
            "Prompt payload executed on click...",
            &app.dashboard.macro_prompt_input,
        )
        .on_input(Message::MacroPromptChanged)
        .on_submit(Message::AddPromptMacro)
        .padding(7)
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
        button(text("+ Pin Macro").size(11))
            .padding(Padding::from([7, 12]))
            .style(theme::solid_button_with_radius(
                pal.accent_primary,
                pal.accent_secondary,
                pal.on_accent_text,
                c_rad,
            ))
            .on_press(Message::AddPromptMacro),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut list = Column::new().spacing(5);
    for (idx, m) in app.dashboard.prefs.custom_macros.iter().enumerate() {
        list = list.push(
            row![
                button(text(&m.label).size(11))
                    .padding(Padding::from([4, 10]))
                    .style(theme::cyber_chip_button_with_radius(
                        false,
                        pal.accent_secondary,
                        pal.bg_elevated,
                        pal.border_subtle,
                        pal.text_primary,
                        pal.accent_secondary,
                        c_rad,
                    ))
                    .on_press(Message::RunPromptMacro(m.prompt.clone())),
                text(&m.prompt)
                    .size(11)
                    .color(pal.text_muted)
                    .width(Length::Fill),
                button(text("x").size(10))
                    .padding(Padding::from([3, 7]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_elevated,
                        pal.critical,
                        pal.text_primary,
                        c_rad,
                    ))
                    .on_press(Message::RemovePromptMacro(idx)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    column![creator, list].spacing(8).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lora_core::{DesktopDashboardViewModel, StatusStripState};
    use tempfile::TempDir;

    fn sample_telemetry(avail: u64, total: u64) -> CoreTelemetry {
        CoreTelemetry {
            dashboard: DesktopDashboardViewModel {
                status_strip: StatusStripState {
                    execution_destination: "local_cpu_worker".to_string(),
                    active_model_alias: None,
                    active_task_busy: false,
                    stop_command_reachable: true,
                    experimental_screens_enabled: false,
                },
                installed_model_count: 0,
                available_ram_mib: avail,
                total_ram_mib: total,
                cpu_summary: "Test CPU".to_string(),
                unavailable_sensors: vec![],
                recent_audit_count: 0,
            },
            host: None,
            models: vec![],
            recent_audits: vec![],
            workflow_receipts: vec![],
            workflow_integrity: (0, 0),
            extension_count: 0,
            tuning_receipts_count: 0,
        }
    }

    #[test]
    fn preferences_roundtrip_and_sanitize_recovers_missing_widgets() {
        let tmp = TempDir::new().expect("tempdir");
        let mut prefs = DashboardPreferences {
            theme_preset: CyberThemePreset::BladeRunner2049,
            density: HudDensity::Compact,
            geometry: HudGeometry::CyberSharp,
            preset: DashboardPreset::MinimalFocus,
            ..DashboardPreferences::default()
        };
        prefs.widgets.truncate(2);
        prefs.save_to_dir(tmp.path());

        let loaded = DashboardPreferences::load_from_dir(tmp.path());
        assert_eq!(loaded.theme_preset, CyberThemePreset::BladeRunner2049);
        assert_eq!(loaded.density, HudDensity::Compact);
        assert_eq!(loaded.geometry, HudGeometry::CyberSharp);
        assert_eq!(loaded.widgets.len(), DashboardWidgetKind::ALL.len());
    }

    #[test]
    fn all_eleven_cyberpunk_universe_themes_build_valid_palettes() {
        assert_eq!(CyberThemePreset::ALL.len(), 11);
        for preset in CyberThemePreset::ALL {
            let pal = preset.palette();
            assert_ne!(pal.bg_stage, pal.accent_primary);
            assert!(!preset.label().is_empty());
            assert!(!preset.origin().is_empty());
            assert!(!preset.code().is_empty());
        }
    }

    #[test]
    fn corrupt_preferences_fall_back_gracefully_without_panicking() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join(HUD_PREFS_FILENAME), "{corrupt json").unwrap();
        let loaded = DashboardPreferences::load_from_dir(tmp.path());
        assert_eq!(loaded.preset, DashboardPreset::OperatorHud);
        assert_eq!(loaded.widgets.len(), DashboardWidgetKind::ALL.len());
    }

    #[test]
    fn widget_reorder_span_visibility_and_telemetry_ring_buffer_behave_cleanly() {
        let mut state = DashboardState::new(&sample_telemetry(8192, 16384));
        let first_kind = state.prefs.widgets[0].kind;
        let second_kind = state.prefs.widgets[1].kind;

        state.move_widget(second_kind, -1);
        assert_eq!(state.prefs.widgets[0].kind, second_kind);
        assert_eq!(state.prefs.widgets[1].kind, first_kind);

        state.toggle_widget_span(second_kind);
        assert_eq!(state.prefs.widgets[0].span, WidgetSpan::Wide);

        state.toggle_widget_visible(second_kind);
        assert!(!state.prefs.widgets[0].visible);

        for i in 0..64 {
            state.record_telemetry_sample(&sample_telemetry(4000 + i * 10, 16384));
        }
        assert_eq!(state.ram_history.len(), TELEMETRY_HISTORY_CAP);
    }
}
