//! Minimalist Void-Cyberpunk + iconic Cyberpunk cinema/game/anime native `iced` palettes.

use iced::border::{self, Border};
use iced::theme::Palette;
use iced::widget::{button, container, markdown, rule, text_input};
use iced::{Background, Color, Shadow, Theme, Vector};
use serde::{Deserialize, Serialize};

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a)
}

// Baseline void-cyberpunk tokens shared across secondary views.
pub const BG_RAIL: Color = rgb(0x09, 0x0A, 0x10);
pub const BG_SIDEBAR: Color = rgb(0x0D, 0x0E, 0x15);
pub const BG_STAGE: Color = rgb(0x0A, 0x0C, 0x13);
pub const BG_CARD: Color = rgb(0x11, 0x14, 0x20);
pub const BG_COMPOSER: Color = rgb(0x16, 0x1A, 0x29);
pub const BG_HOVER: Color = rgb(0x1B, 0x21, 0x33);
pub const BG_SELECTED: Color = rgb(0x20, 0x27, 0x3C);
pub const BG_DOCK: Color = rgb(0x0A, 0x0B, 0x12);

pub const BORDER_SUBTLE: Color = rgb(0x1E, 0x26, 0x3A);
pub const BORDER_ACCENT: Color = rgba(0xFF, 0x00, 0x7F, 0.58);

pub const TEXT_PRIMARY: Color = rgb(0xF1, 0xF5, 0xF9);
pub const TEXT_SECONDARY: Color = rgb(0xCB, 0xD5, 0xE1);
pub const TEXT_MUTED: Color = rgb(0x7C, 0x8B, 0xA6);
pub const TEXT_CHANNEL: Color = rgb(0x64, 0x74, 0x8B);

pub const BLURPLE: Color = rgb(0x00, 0xF0, 0xFF);
pub const BLURPLE_HOVER: Color = rgb(0x00, 0xB8, 0xD4);
pub const CYAN_SOFT: Color = rgba(0x00, 0xF0, 0xFF, 0.14);
pub const LORA_MAGENTA: Color = rgb(0xFF, 0x2E, 0x93);
pub const LORA_MAGENTA_SOFT: Color = rgba(0xFF, 0x2E, 0x93, 0.15);
pub const GREEN: Color = rgb(0x00, 0xE5, 0x99);
pub const GREEN_SOFT: Color = rgba(0x00, 0xE5, 0x99, 0.14);
pub const AMBER: Color = rgb(0xFF, 0xE6, 0x00);
pub const AMBER_SOFT: Color = rgba(0xFF, 0xE6, 0x00, 0.14);
pub const RED: Color = rgb(0xFF, 0x00, 0x3C);
pub const RED_HOVER: Color = rgb(0xCC, 0x00, 0x30);
pub const RED_SOFT: Color = rgba(0xFF, 0x00, 0x3C, 0.16);

pub const MODAL_BACKDROP: Color = rgba(0x04, 0x05, 0x09, 0.85);

/// Surface darkness depth preset for infinite theme tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SurfaceDepth {
    #[default]
    Obsidian,
    PitchOled,
    CyberSlate,
}

impl SurfaceDepth {
    pub const ALL: [Self; 3] = [Self::Obsidian, Self::PitchOled, Self::CyberSlate];

    pub fn label(self) -> &'static str {
        match self {
            Self::Obsidian => "Obsidian Dark",
            Self::PitchOled => "Pitch OLED (#000)",
            Self::CyberSlate => "Midnight Slate",
        }
    }
}

/// Infinite live theme generator controlled via RGB/Hue sliders in Custom Studio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomThemeSpec {
    pub enabled: bool,
    pub primary_hue: u16,
    pub secondary_hue: u16,
    pub surface_depth: SurfaceDepth,
}

impl Default for CustomThemeSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            primary_hue: 332,
            secondary_hue: 186,
            surface_depth: SurfaceDepth::Obsidian,
        }
    }
}

impl CustomThemeSpec {
    pub fn build_palette(&self) -> CyberPalette {
        let p = hsl_color((self.primary_hue % 360) as f32, 0.92, 0.56);
        let s = hsl_color((self.secondary_hue % 360) as f32, 0.90, 0.54);
        let (bg_rail, bg_sidebar, bg_stage, bg_card, bg_elevated, bg_dock, border_subtle) =
            match self.surface_depth {
                SurfaceDepth::Obsidian => (
                    rgb(0x06, 0x07, 0x0D),
                    rgb(0x0A, 0x0D, 0x17),
                    rgb(0x0C, 0x10, 0x1C),
                    rgb(0x11, 0x16, 0x26),
                    rgb(0x18, 0x1F, 0x33),
                    rgb(0x08, 0x0A, 0x12),
                    rgb(0x21, 0x29, 0x42),
                ),
                SurfaceDepth::PitchOled => (
                    rgb(0x01, 0x02, 0x04),
                    rgb(0x04, 0x05, 0x09),
                    rgb(0x05, 0x06, 0x0B),
                    rgb(0x0A, 0x0C, 0x14),
                    rgb(0x11, 0x15, 0x22),
                    rgb(0x03, 0x04, 0x07),
                    rgb(0x19, 0x1F, 0x30),
                ),
                SurfaceDepth::CyberSlate => (
                    rgb(0x0B, 0x10, 0x1B),
                    rgb(0x10, 0x16, 0x24),
                    rgb(0x13, 0x1A, 0x2A),
                    rgb(0x19, 0x22, 0x36),
                    rgb(0x21, 0x2C, 0x44),
                    rgb(0x0D, 0x12, 0x1F),
                    rgb(0x2A, 0x37, 0x54),
                ),
            };
        CyberPalette {
            bg_rail,
            bg_sidebar,
            bg_stage,
            bg_card,
            bg_elevated,
            bg_dock,
            border_subtle,
            border_accent: Color { a: 0.72, ..p },
            accent_primary: p,
            accent_primary_soft: Color { a: 0.16, ..p },
            accent_secondary: s,
            accent_secondary_soft: Color { a: 0.15, ..s },
            nominal: rgb(0x1E, 0xE6, 0x9A),
            nominal_soft: rgba(0x1E, 0xE6, 0x9A, 0.14),
            warning: rgb(0xFF, 0xB8, 0x30),
            critical: rgb(0xFF, 0x3B, 0x5C),
            text_primary: rgb(0xF3, 0xF5, 0xFB),
            text_secondary: rgb(0xC2, 0xCA, 0xDC),
            text_muted: rgb(0x7E, 0x89, 0xA6),
            on_accent_text: best_on_accent(p),
        }
        .ensure_wcag()
    }
}

pub fn hsl_color(h_deg: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = h_deg / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    Color::from_rgb(
        (r1 + m).clamp(0.0, 1.0),
        (g1 + m).clamp(0.0, 1.0),
        (b1 + m).clamp(0.0, 1.0),
    )
}

/// Corner geometry style for cards and FUI containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HudGeometry {
    #[default]
    SleekModern,
    CyberSharp,
}

impl HudGeometry {
    pub const ALL: [Self; 2] = [Self::SleekModern, Self::CyberSharp];

    pub fn label(self) -> &'static str {
        match self {
            Self::SleekModern => "Sleek Modern (8px)",
            Self::CyberSharp => "Tactical FUI (2px)",
        }
    }

    pub fn card_radius(self) -> f32 {
        match self {
            Self::SleekModern => 8.0,
            Self::CyberSharp => 2.0,
        }
    }

    pub fn control_radius(self) -> f32 {
        match self {
            Self::SleekModern => 6.0,
            Self::CyberSharp => 2.0,
        }
    }
}

/// Curated cyberpunk themes derived from iconic games, films, anime, and FUI templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CyberThemePreset {
    #[default]
    NightCity2077,
    ArasakaCorp,
    LucyNetrunner,
    DavidSandevistan,
    BladeRunner2049,
    GhostInTheShell,
    AkiraNeoTokyo,
    TheMatrix1999,
    DeusExRenaissance,
    TronLegacyGrid,
    NeuromancerSynth,
}

impl CyberThemePreset {
    pub const ALL: [Self; 11] = [
        Self::NightCity2077,
        Self::ArasakaCorp,
        Self::LucyNetrunner,
        Self::DavidSandevistan,
        Self::BladeRunner2049,
        Self::GhostInTheShell,
        Self::AkiraNeoTokyo,
        Self::TheMatrix1999,
        Self::DeusExRenaissance,
        Self::TronLegacyGrid,
        Self::NeuromancerSynth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::NightCity2077 => "Night City 2077",
            Self::ArasakaCorp => "Arasaka Corp",
            Self::LucyNetrunner => "Lucy Netrunner",
            Self::DavidSandevistan => "Sandevistan OC",
            Self::BladeRunner2049 => "Blade Runner 2049",
            Self::GhostInTheShell => "Ghost in the Shell",
            Self::AkiraNeoTokyo => "Akira Neo-Tokyo",
            Self::TheMatrix1999 => "The Matrix Grid",
            Self::DeusExRenaissance => "Deus Ex: HR Gold",
            Self::TronLegacyGrid => "Tron: Legacy",
            Self::NeuromancerSynth => "Neuromancer Outrun",
        }
    }

    pub fn origin(self) -> &'static str {
        match self {
            Self::NightCity2077 => "Cyberpunk 2077 HUD",
            Self::ArasakaCorp => "Cyberpunk · Arasaka",
            Self::LucyNetrunner => "Edgerunners · Lucy",
            Self::DavidSandevistan => "Edgerunners · David",
            Self::BladeRunner2049 => "Blade Runner 2049",
            Self::GhostInTheShell => "Ghost in the Shell (1995)",
            Self::AkiraNeoTokyo => "Akira (1988)",
            Self::TheMatrix1999 => "The Matrix (1999)",
            Self::DeusExRenaissance => "Deus Ex: Human Revolution",
            Self::TronLegacyGrid => "Tron: Legacy (2010)",
            Self::NeuromancerSynth => "Neuromancer · Outrun FUI",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::NightCity2077 => "NC-2077",
            Self::ArasakaCorp => "ARASAKA",
            Self::LucyNetrunner => "LUCY-MW",
            Self::DavidSandevistan => "SANDEV-01",
            Self::BladeRunner2049 => "BR-2049",
            Self::GhostInTheShell => "SEC-09",
            Self::AkiraNeoTokyo => "AKIRA-88",
            Self::TheMatrix1999 => "ZION-99",
            Self::DeusExRenaissance => "SARIF-AU",
            Self::TronLegacyGrid => "TRON-GRID",
            Self::NeuromancerSynth => "SPRAWL-84",
        }
    }

    pub fn palette(self) -> CyberPalette {
        match self {
            Self::NightCity2077 => CyberPalette {
                bg_rail: rgb(0x08, 0x09, 0x0E),
                bg_sidebar: rgb(0x0D, 0x0E, 0x15),
                bg_stage: rgb(0x09, 0x0B, 0x11),
                bg_card: rgb(0x11, 0x14, 0x1F),
                bg_elevated: rgb(0x18, 0x1D, 0x2E),
                bg_dock: rgb(0x0A, 0x0B, 0x11),
                border_subtle: rgb(0x1F, 0x28, 0x3D),
                border_accent: rgba(0xFF, 0x00, 0x7F, 0.62),
                accent_primary: rgb(0xFF, 0x00, 0x7F),
                accent_primary_soft: rgba(0xFF, 0x00, 0x7F, 0.15),
                accent_secondary: rgb(0x00, 0xF0, 0xFF),
                accent_secondary_soft: rgba(0x00, 0xF0, 0xFF, 0.14),
                nominal: rgb(0x00, 0xFF, 0x9D),
                nominal_soft: rgba(0x00, 0xFF, 0x9D, 0.14),
                warning: rgb(0xFF, 0xE6, 0x00),
                critical: rgb(0xFF, 0x00, 0x3C),
                text_primary: TEXT_PRIMARY,
                text_secondary: TEXT_SECONDARY,
                text_muted: TEXT_MUTED,
                on_accent_text: rgb(0xFF, 0xFF, 0xFF),
            },
            Self::ArasakaCorp => CyberPalette {
                bg_rail: rgb(0x09, 0x09, 0x0B),
                bg_sidebar: rgb(0x11, 0x11, 0x14),
                bg_stage: rgb(0x0C, 0x0C, 0x0F),
                bg_card: rgb(0x16, 0x16, 0x1A),
                bg_elevated: rgb(0x1F, 0x1F, 0x25),
                bg_dock: rgb(0x0E, 0x0E, 0x11),
                border_subtle: rgb(0x2B, 0x24, 0x28),
                border_accent: rgba(0xFF, 0x00, 0x33, 0.65),
                accent_primary: rgb(0xFF, 0x00, 0x33),
                accent_primary_soft: rgba(0xFF, 0x00, 0x33, 0.16),
                accent_secondary: rgb(0xF0, 0xF0, 0xF0),
                accent_secondary_soft: rgba(0xF0, 0xF0, 0xF0, 0.11),
                nominal: rgb(0x28, 0xD1, 0x7C),
                nominal_soft: rgba(0x28, 0xD1, 0x7C, 0.14),
                warning: rgb(0xFF, 0x55, 0x00),
                critical: rgb(0xFF, 0x00, 0x33),
                text_primary: rgb(0xF0, 0xF0, 0xF0),
                text_secondary: rgb(0xCF, 0xCF, 0xD4),
                text_muted: rgb(0x88, 0x88, 0x90),
                on_accent_text: rgb(0xFF, 0xFF, 0xFF),
            },
            Self::LucyNetrunner => CyberPalette {
                bg_rail: rgb(0x08, 0x06, 0x11),
                bg_sidebar: rgb(0x0F, 0x0C, 0x1C),
                bg_stage: rgb(0x0A, 0x08, 0x15),
                bg_card: rgb(0x14, 0x10, 0x24),
                bg_elevated: rgb(0x1D, 0x17, 0x33),
                bg_dock: rgb(0x0B, 0x08, 0x16),
                border_subtle: rgb(0x2B, 0x21, 0x44),
                border_accent: rgba(0xBA, 0x55, 0xD3, 0.62),
                accent_primary: rgb(0xBA, 0x55, 0xD3),
                accent_primary_soft: rgba(0xBA, 0x55, 0xD3, 0.17),
                accent_secondary: rgb(0xFF, 0x55, 0x00),
                accent_secondary_soft: rgba(0xFF, 0x55, 0x00, 0.14),
                nominal: rgb(0x00, 0xFF, 0xFF),
                nominal_soft: rgba(0x00, 0xFF, 0xFF, 0.13),
                warning: rgb(0xFF, 0xB6, 0xC1),
                critical: rgb(0xFF, 0x2E, 0x63),
                text_primary: rgb(0xF7, 0xF2, 0xFA),
                text_secondary: rgb(0xDD, 0xCE, 0xEB),
                text_muted: rgb(0x8E, 0x7D, 0xA8),
                on_accent_text: rgb(0xFF, 0xFF, 0xFF),
            },
            Self::DavidSandevistan => CyberPalette {
                bg_rail: rgb(0x05, 0x0B, 0x14),
                bg_sidebar: rgb(0x09, 0x12, 0x1F),
                bg_stage: rgb(0x07, 0x0E, 0x19),
                bg_card: rgb(0x0D, 0x17, 0x28),
                bg_elevated: rgb(0x14, 0x22, 0x38),
                bg_dock: rgb(0x07, 0x0E, 0x1A),
                border_subtle: rgb(0x1B, 0x31, 0x4D),
                border_accent: rgba(0xF6, 0xC0, 0x00, 0.62),
                accent_primary: rgb(0xF6, 0xC0, 0x00),
                accent_primary_soft: rgba(0xF6, 0xC0, 0x00, 0.16),
                accent_secondary: rgb(0x00, 0xA8, 0xFF),
                accent_secondary_soft: rgba(0x00, 0xA8, 0xFF, 0.15),
                nominal: rgb(0x00, 0xFF, 0x66),
                nominal_soft: rgba(0x00, 0xFF, 0x66, 0.14),
                warning: rgb(0xF6, 0xC0, 0x00),
                critical: rgb(0xFF, 0x33, 0x66),
                text_primary: rgb(0xEA, 0xEA, 0xEA),
                text_secondary: rgb(0xC6, 0xD4, 0xE5),
                text_muted: rgb(0x75, 0x8F, 0xAD),
                on_accent_text: rgb(0x08, 0x0C, 0x12),
            },
            Self::BladeRunner2049 => CyberPalette {
                bg_rail: rgb(0x09, 0x0A, 0x0B),
                bg_sidebar: rgb(0x11, 0x11, 0x10),
                bg_stage: rgb(0x0B, 0x0E, 0x10),
                bg_card: rgb(0x16, 0x14, 0x11),
                bg_elevated: rgb(0x22, 0x1D, 0x16),
                bg_dock: rgb(0x0E, 0x0D, 0x0B),
                border_subtle: rgb(0x36, 0x2B, 0x1C),
                border_accent: rgba(0xE8, 0x79, 0x2B, 0.62),
                accent_primary: rgb(0xE8, 0x79, 0x2B),
                accent_primary_soft: rgba(0xE8, 0x79, 0x2B, 0.16),
                accent_secondary: rgb(0xF4, 0xB2, 0x66),
                accent_secondary_soft: rgba(0xF4, 0xB2, 0x66, 0.14),
                nominal: rgb(0x8A, 0xC9, 0x9C),
                nominal_soft: rgba(0x8A, 0xC9, 0x9C, 0.14),
                warning: rgb(0xF4, 0xB2, 0x66),
                critical: rgb(0xC2, 0x51, 0x1C),
                text_primary: rgb(0xF4, 0xEE, 0xE6),
                text_secondary: rgb(0xD8, 0xC9, 0xB8),
                text_muted: rgb(0x95, 0x86, 0x73),
                on_accent_text: rgb(0x0D, 0x0A, 0x07),
            },
            Self::GhostInTheShell => CyberPalette {
                bg_rail: rgb(0x07, 0x0B, 0x0A),
                bg_sidebar: rgb(0x0D, 0x14, 0x13),
                bg_stage: rgb(0x0B, 0x10, 0x0F),
                bg_card: rgb(0x13, 0x1C, 0x1B),
                bg_elevated: rgb(0x1C, 0x27, 0x25),
                bg_dock: rgb(0x0C, 0x11, 0x10),
                border_subtle: rgb(0x2C, 0x36, 0x34),
                border_accent: rgba(0x45, 0xE3, 0xC6, 0.52),
                accent_primary: rgb(0x45, 0xE3, 0xC6),
                accent_primary_soft: rgba(0x45, 0xE3, 0xC6, 0.14),
                accent_secondary: rgb(0xAA, 0xAE, 0xAE),
                accent_secondary_soft: rgba(0xAA, 0xAE, 0xAE, 0.14),
                nominal: rgb(0x45, 0xE3, 0xC6),
                nominal_soft: rgba(0x45, 0xE3, 0xC6, 0.14),
                warning: rgb(0xE2, 0xB9, 0x3B),
                critical: rgb(0xEF, 0x44, 0x44),
                text_primary: rgb(0xF4, 0xF5, 0xF6),
                text_secondary: rgb(0xC8, 0xD0, 0xCE),
                text_muted: rgb(0x7D, 0x8C, 0x89),
                on_accent_text: rgb(0x08, 0x10, 0x0E),
            },
            Self::AkiraNeoTokyo => CyberPalette {
                bg_rail: rgb(0x0B, 0x0F, 0x1C),
                bg_sidebar: rgb(0x13, 0x18, 0x29),
                bg_stage: rgb(0x0E, 0x12, 0x20),
                bg_card: rgb(0x18, 0x1D, 0x2C),
                bg_elevated: rgb(0x21, 0x27, 0x3B),
                bg_dock: rgb(0x10, 0x14, 0x21),
                border_subtle: rgb(0x2E, 0x37, 0x4F),
                border_accent: rgba(0xE1, 0x21, 0x20, 0.65),
                accent_primary: rgb(0xE1, 0x21, 0x20),
                accent_primary_soft: rgba(0xE1, 0x21, 0x20, 0.16),
                accent_secondary: rgb(0xF4, 0xD0, 0x3F),
                accent_secondary_soft: rgba(0xF4, 0xD0, 0x3F, 0.14),
                nominal: rgb(0x38, 0xBD, 0xF8),
                nominal_soft: rgba(0x38, 0xBD, 0xF8, 0.14),
                warning: rgb(0xF4, 0xD0, 0x3F),
                critical: rgb(0xE1, 0x21, 0x20),
                text_primary: rgb(0xF3, 0xF4, 0xF6),
                text_secondary: rgb(0xD1, 0xD5, 0xDB),
                text_muted: rgb(0x88, 0x92, 0xA8),
                on_accent_text: rgb(0xFF, 0xFF, 0xFF),
            },
            Self::TheMatrix1999 => CyberPalette {
                bg_rail: rgb(0x03, 0x08, 0x04),
                bg_sidebar: rgb(0x06, 0x10, 0x07),
                bg_stage: rgb(0x04, 0x0B, 0x05),
                bg_card: rgb(0x09, 0x17, 0x0B),
                bg_elevated: rgb(0x0F, 0x24, 0x12),
                bg_dock: rgb(0x05, 0x0D, 0x06),
                border_subtle: rgb(0x14, 0x3B, 0x18),
                border_accent: rgba(0x00, 0xFF, 0x41, 0.58),
                accent_primary: rgb(0x00, 0xFF, 0x41),
                accent_primary_soft: rgba(0x00, 0xFF, 0x41, 0.14),
                accent_secondary: rgb(0x4A, 0xDE, 0x80),
                accent_secondary_soft: rgba(0x4A, 0xDE, 0x80, 0.14),
                nominal: rgb(0x00, 0xFF, 0x41),
                nominal_soft: rgba(0x00, 0xFF, 0x41, 0.14),
                warning: rgb(0xFF, 0xB0, 0x00),
                critical: rgb(0xFF, 0x3B, 0x5C),
                text_primary: rgb(0xE2, 0xFC, 0xE6),
                text_secondary: rgb(0xA6, 0xDF, 0xAF),
                text_muted: rgb(0x55, 0x9E, 0x61),
                on_accent_text: rgb(0x03, 0x0B, 0x04),
            },
            Self::DeusExRenaissance => CyberPalette {
                bg_rail: rgb(0x09, 0x08, 0x06),
                bg_sidebar: rgb(0x11, 0x0F, 0x0C),
                bg_stage: rgb(0x0D, 0x0C, 0x09),
                bg_card: rgb(0x17, 0x14, 0x0F),
                bg_elevated: rgb(0x24, 0x1E, 0x16),
                bg_dock: rgb(0x0F, 0x0E, 0x0B),
                border_subtle: rgb(0x3B, 0x31, 0x20),
                border_accent: rgba(0xFF, 0xBF, 0x00, 0.60),
                accent_primary: rgb(0xFF, 0xBF, 0x00),
                accent_primary_soft: rgba(0xFF, 0xBF, 0x00, 0.15),
                accent_secondary: rgb(0xB2, 0x8D, 0x3F),
                accent_secondary_soft: rgba(0xB2, 0x8D, 0x3F, 0.15),
                nominal: rgb(0x74, 0xC6, 0x7A),
                nominal_soft: rgba(0x74, 0xC6, 0x7A, 0.14),
                warning: rgb(0xFF, 0xBF, 0x00),
                critical: rgb(0xE0, 0x4F, 0x33),
                text_primary: rgb(0xF5, 0xF0, 0xE6),
                text_secondary: rgb(0xD6, 0xCA, 0xB4),
                text_muted: rgb(0x94, 0x84, 0x68),
                on_accent_text: rgb(0x11, 0x0E, 0x08),
            },
            Self::TronLegacyGrid => CyberPalette {
                bg_rail: rgb(0x05, 0x0A, 0x10),
                bg_sidebar: rgb(0x0A, 0x12, 0x1C),
                bg_stage: rgb(0x07, 0x0E, 0x17),
                bg_card: rgb(0x0C, 0x16, 0x22),
                bg_elevated: rgb(0x10, 0x29, 0x31),
                bg_dock: rgb(0x08, 0x10, 0x19),
                border_subtle: rgb(0x18, 0x3B, 0x4A),
                border_accent: rgba(0x00, 0xF7, 0xFF, 0.62),
                accent_primary: rgb(0x00, 0xF7, 0xFF),
                accent_primary_soft: rgba(0x00, 0xF7, 0xFF, 0.14),
                accent_secondary: rgb(0xDF, 0x74, 0x0C),
                accent_secondary_soft: rgba(0xDF, 0x74, 0x0C, 0.16),
                nominal: rgb(0x00, 0xF7, 0xFF),
                nominal_soft: rgba(0x00, 0xF7, 0xFF, 0.14),
                warning: rgb(0xDF, 0x74, 0x0C),
                critical: rgb(0xFF, 0x3B, 0x30),
                text_primary: rgb(0xFB, 0xFC, 0xFD),
                text_secondary: rgb(0xC9, 0xE2, 0xEB),
                text_muted: rgb(0x6D, 0x95, 0xA8),
                on_accent_text: rgb(0x05, 0x0B, 0x10),
            },
            Self::NeuromancerSynth => CyberPalette {
                bg_rail: rgb(0x0B, 0x05, 0x19),
                bg_sidebar: rgb(0x12, 0x07, 0x2B),
                bg_stage: rgb(0x0D, 0x06, 0x21),
                bg_card: rgb(0x17, 0x0C, 0x33),
                bg_elevated: rgb(0x23, 0x12, 0x4A),
                bg_dock: rgb(0x0F, 0x06, 0x24),
                border_subtle: rgb(0x3B, 0x1B, 0x6B),
                border_accent: rgba(0xF7, 0x06, 0xCF, 0.62),
                accent_primary: rgb(0xF7, 0x06, 0xCF),
                accent_primary_soft: rgba(0xF7, 0x06, 0xCF, 0.16),
                accent_secondary: rgb(0xFD, 0x5E, 0x53),
                accent_secondary_soft: rgba(0xFD, 0x5E, 0x53, 0.15),
                nominal: rgb(0x00, 0xF0, 0xFF),
                nominal_soft: rgba(0x00, 0xF0, 0xFF, 0.14),
                warning: rgb(0xFD, 0x5E, 0x53),
                critical: rgb(0xFF, 0x2A, 0x6D),
                text_primary: rgb(0xFA, 0xF5, 0xFF),
                text_secondary: rgb(0xDE, 0xCA, 0xF2),
                text_muted: rgb(0x98, 0x7A, 0xBD),
                on_accent_text: rgb(0xFF, 0xFF, 0xFF),
            },
        }
    }
}

/// Active color tokens shared across the pure-Rust shell and modular HUD widgets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CyberPalette {
    pub bg_rail: Color,
    pub bg_sidebar: Color,
    pub bg_stage: Color,
    pub bg_card: Color,
    pub bg_elevated: Color,
    pub bg_dock: Color,
    pub border_subtle: Color,
    pub border_accent: Color,
    pub accent_primary: Color,
    pub accent_primary_soft: Color,
    pub accent_secondary: Color,
    pub accent_secondary_soft: Color,
    pub nominal: Color,
    pub nominal_soft: Color,
    pub warning: Color,
    pub critical: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub on_accent_text: Color,
}

#[inline]
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[inline]
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::from_rgba(
        lerp_f32(a.r, b.r, t),
        lerp_f32(a.g, b.g, t),
        lerp_f32(a.b, b.b, t),
        lerp_f32(a.a, b.a, t),
    )
}

#[inline]
fn color_diff(a: Color, b: Color) -> f32 {
    (a.r - b.r)
        .abs()
        .max((a.g - b.g).abs())
        .max((a.b - b.b).abs())
        .max((a.a - b.a).abs())
}

#[inline]
pub fn linearize_channel(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 2.1 relative luminance in `[0.0, 1.0]`.
#[inline]
pub fn relative_luminance(c: Color) -> f32 {
    0.2126 * linearize_channel(c.r)
        + 0.7152 * linearize_channel(c.g)
        + 0.0722 * linearize_channel(c.b)
}

/// WCAG 2.1 contrast ratio in `[1.0, 21.0]`.
#[inline]
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Choose crisp obsidian dark or pure white foreground for maximum legibility on an accent fill.
#[inline]
pub fn best_on_accent(accent: Color) -> Color {
    let dark = rgb(0x06, 0x08, 0x0E);
    let light = rgb(0xFF, 0xFF, 0xFF);
    if contrast_ratio(dark, accent) >= contrast_ratio(light, accent) {
        dark
    } else {
        light
    }
}

/// Adaptively lift foreground lightness on dark surfaces until `min_ratio` is satisfied.
pub fn enforce_contrast_on_dark(fg: Color, bg: Color, min_ratio: f32) -> Color {
    let mut current = fg;
    for step in 1..=12 {
        if contrast_ratio(current, bg) >= min_ratio {
            return current;
        }
        let t = step as f32 / 12.0;
        current = lerp_color(fg, Color::WHITE, t);
    }
    Color::WHITE
}

impl CyberPalette {
    /// Enforce WCAG AA/AAA text & interactive contrast against dark surfaces without losing neon identity.
    pub fn ensure_wcag(mut self) -> Self {
        let worst_bg = self.bg_elevated;
        self.text_primary = enforce_contrast_on_dark(self.text_primary, worst_bg, 7.2);
        self.text_secondary = enforce_contrast_on_dark(self.text_secondary, worst_bg, 4.8);
        self.text_muted = enforce_contrast_on_dark(self.text_muted, self.bg_card, 3.6);
        self.on_accent_text = best_on_accent(self.accent_primary);
        self
    }

    pub fn wcag_primary_ratio(&self) -> f32 {
        contrast_ratio(self.text_primary, self.bg_card)
    }

    pub fn wcag_grade_label(&self) -> &'static str {
        let r = self.wcag_primary_ratio();
        if r >= 7.0 {
            "WCAG AAA"
        } else if r >= 4.5 {
            "WCAG AA"
        } else {
            "LOW"
        }
    }

    /// Smoothly interpolate every color token toward `target` for liquid theme morphing.
    pub fn lerp(&self, target: &CyberPalette, t: f32) -> CyberPalette {
        CyberPalette {
            bg_rail: lerp_color(self.bg_rail, target.bg_rail, t),
            bg_sidebar: lerp_color(self.bg_sidebar, target.bg_sidebar, t),
            bg_stage: lerp_color(self.bg_stage, target.bg_stage, t),
            bg_card: lerp_color(self.bg_card, target.bg_card, t),
            bg_elevated: lerp_color(self.bg_elevated, target.bg_elevated, t),
            bg_dock: lerp_color(self.bg_dock, target.bg_dock, t),
            border_subtle: lerp_color(self.border_subtle, target.border_subtle, t),
            border_accent: lerp_color(self.border_accent, target.border_accent, t),
            accent_primary: lerp_color(self.accent_primary, target.accent_primary, t),
            accent_primary_soft: lerp_color(
                self.accent_primary_soft,
                target.accent_primary_soft,
                t,
            ),
            accent_secondary: lerp_color(self.accent_secondary, target.accent_secondary, t),
            accent_secondary_soft: lerp_color(
                self.accent_secondary_soft,
                target.accent_secondary_soft,
                t,
            ),
            nominal: lerp_color(self.nominal, target.nominal, t),
            nominal_soft: lerp_color(self.nominal_soft, target.nominal_soft, t),
            warning: lerp_color(self.warning, target.warning, t),
            critical: lerp_color(self.critical, target.critical, t),
            text_primary: lerp_color(self.text_primary, target.text_primary, t),
            text_secondary: lerp_color(self.text_secondary, target.text_secondary, t),
            text_muted: lerp_color(self.text_muted, target.text_muted, t),
            on_accent_text: lerp_color(self.on_accent_text, target.on_accent_text, t),
        }
    }

    /// Maximum per-channel difference against another palette to detect animation completion.
    pub fn max_channel_diff(&self, other: &CyberPalette) -> f32 {
        color_diff(self.bg_stage, other.bg_stage)
            .max(color_diff(self.bg_card, other.bg_card))
            .max(color_diff(self.accent_primary, other.accent_primary))
            .max(color_diff(self.accent_secondary, other.accent_secondary))
            .max(color_diff(self.text_primary, other.text_primary))
    }

    pub fn iced_theme(&self) -> Theme {
        Theme::custom(
            "Lora Cyberpunk HUD",
            Palette {
                background: self.bg_stage,
                text: self.text_primary,
                primary: self.accent_primary,
                success: self.nominal,
                warning: self.warning,
                danger: self.critical,
            },
        )
    }
}

/// Construct the default Night City Cyberpunk theme with high-contrast dark surfaces.
pub fn lora_cyberpunk_theme() -> Theme {
    CyberThemePreset::NightCity2077.palette().iced_theme()
}

/// Style for Markdown blocks rendered directly inside `#chat`.
pub fn chat_markdown_style() -> markdown::Style {
    palette_markdown_style(CyberThemePreset::NightCity2077.palette())
}

/// Palette-driven Markdown styling reacting live to the active cyberpunk theme.
pub fn palette_markdown_style(pal: CyberPalette) -> markdown::Style {
    markdown::Style {
        font: iced::Font::DEFAULT,
        inline_code_highlight: markdown::Highlight {
            background: pal.bg_rail.into(),
            border: border::rounded(4),
        },
        inline_code_padding: iced::Padding::from([1, 4]),
        inline_code_color: pal.accent_secondary,
        inline_code_font: iced::Font::MONOSPACE,
        code_block_font: iced::Font::MONOSPACE,
        link_color: pal.accent_primary,
    }
}

pub fn surface_container(bg: Color) -> impl Fn(&Theme) -> container::Style {
    palette_surface_container(bg, TEXT_PRIMARY)
}

pub fn palette_surface_container(
    bg: Color,
    text_color: Color,
) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(text_color),
        ..container::Style::default()
    }
}

pub fn card_container(
    bg: Color,
    border_color: Color,
    radius: f32,
) -> impl Fn(&Theme) -> container::Style {
    palette_card_container(bg, TEXT_PRIMARY, border_color, radius)
}

pub fn palette_card_container(
    bg: Color,
    text_color: Color,
    border_color: Color,
    radius: f32,
) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(text_color),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: radius.into(),
        },
        ..container::Style::default()
    }
}

/// Minimalist cyberpunk card with optional crisp ambient edge highlight.
pub fn cyber_card(
    bg: Color,
    border_color: Color,
    glow_color: Option<Color>,
    radius: f32,
) -> impl Fn(&Theme) -> container::Style {
    palette_cyber_card(bg, TEXT_PRIMARY, border_color, glow_color, radius)
}

pub fn palette_cyber_card(
    bg: Color,
    text_color: Color,
    border_color: Color,
    glow_color: Option<Color>,
    radius: f32,
) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(text_color),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: radius.into(),
        },
        shadow: match glow_color {
            Some(c) => Shadow {
                color: Color {
                    a: if c.a < 0.99 { c.a } else { 0.14 },
                    ..c
                },
                offset: Vector::new(0.0, 2.0),
                blur_radius: 10.0,
            },
            None => Shadow::default(),
        },
        snap: true,
    }
}

pub fn pill_container(
    bg: Color,
    text_color: Color,
    border_color: Color,
) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(text_color),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 999.0.into(),
        },
        ..container::Style::default()
    }
}

pub fn modal_backdrop_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(MODAL_BACKDROP)),
        ..container::Style::default()
    }
}

pub fn modal_card_style(_theme: &Theme) -> container::Style {
    palette_modal_card(CyberThemePreset::NightCity2077.palette(), 8.0)(_theme)
}

pub fn palette_modal_card(pal: CyberPalette, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(pal.bg_card)),
        text_color: Some(pal.text_primary),
        border: Border {
            color: pal.warning,
            width: 1.5,
            radius: radius.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.78),
            offset: Vector::new(0.0, 14.0),
            blur_radius: 36.0,
        },
        snap: true,
    }
}

pub fn divider_rule(_theme: &Theme) -> rule::Style {
    rule::Style {
        color: BORDER_SUBTLE,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

pub fn palette_divider_rule(pal: CyberPalette) -> impl Fn(&Theme) -> rule::Style {
    move |_| rule::Style {
        color: pal.border_subtle,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

/// Guild server icon pill button in the far-left rail.
pub fn rail_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    palette_rail_button(active, LORA_MAGENTA, BLURPLE, BG_CARD, TEXT_PRIMARY)
}

/// Palette-driven rail button adapting live to the active cyberpunk preset.
pub fn palette_rail_button(
    active: bool,
    accent: Color,
    hover_accent: Color,
    idle_bg: Color,
    text_color: Color,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = if active {
            accent
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => hover_accent,
                _ => idle_bg,
            }
        };
        let radius =
            if active || matches!(status, button::Status::Hovered | button::Status::Pressed) {
                10.0
            } else {
                20.0
            };
        let fg = if active || matches!(status, button::Status::Hovered | button::Status::Pressed) {
            Color::from_rgb(0.05, 0.06, 0.09)
        } else {
            text_color
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: if active {
                    text_color
                } else {
                    Color {
                        a: 0.16,
                        ..text_color
                    }
                },
                width: 1.0,
                radius: radius.into(),
            },
            ..button::Style::default()
        }
    }
}

/// `#channel` row button in the left navigation sidebar.
pub fn channel_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    palette_channel_button(
        active,
        BG_SELECTED,
        BG_HOVER,
        BORDER_ACCENT,
        TEXT_PRIMARY,
        TEXT_SECONDARY,
        TEXT_MUTED,
    )
}

/// Palette-driven `#channel` row button.
pub fn palette_channel_button(
    active: bool,
    selected_bg: Color,
    hover_bg: Color,
    accent_border: Color,
    text_primary: Color,
    text_secondary: Color,
    text_muted: Color,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, fg) = if active {
            (selected_bg, text_primary)
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => (hover_bg, text_secondary),
                _ => (Color::TRANSPARENT, text_muted),
            }
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: if active {
                    accent_border
                } else {
                    Color::TRANSPARENT
                },
                width: if active { 1.0 } else { 0.0 },
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Parameterized solid action button.
pub fn solid_button(
    normal: Color,
    hover: Color,
    text_color: Color,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    solid_button_with_radius(normal, hover, text_color, 6.0)
}

pub fn solid_button_with_radius(
    normal: Color,
    hover: Color,
    text_color: Color,
    radius: f32,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match status {
            button::Status::Disabled => BG_HOVER,
            button::Status::Hovered | button::Status::Pressed => hover,
            button::Status::Active => normal,
        };
        let fg = if matches!(status, button::Status::Disabled) {
            TEXT_MUTED
        } else {
            text_color
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: radius.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Hairline-bordered cyber chip / HUD control button.
pub fn cyber_chip_button(
    active: bool,
    accent: Color,
    surface: Color,
    border: Color,
    text_primary: Color,
    text_muted: Color,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    cyber_chip_button_with_radius(
        active,
        accent,
        surface,
        border,
        text_primary,
        text_muted,
        6.0,
    )
}

pub fn cyber_chip_button_with_radius(
    active: bool,
    accent: Color,
    surface: Color,
    border: Color,
    text_primary: Color,
    text_muted: Color,
    radius: f32,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, brd, fg) = if active {
            (Color { a: 0.20, ..accent }, accent, text_primary)
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => (
                    Color { a: 0.11, ..accent },
                    Color { a: 0.72, ..accent },
                    text_primary,
                ),
                _ => (surface, border, text_muted),
            }
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: brd,
                width: 1.0,
                radius: radius.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn composer_input_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    cyber_input_style(BG_COMPOSER, BORDER_SUBTLE, LORA_MAGENTA, BG_SELECTED, 6.0)(_theme, status)
}

pub fn cyber_input_style(
    bg: Color,
    border_idle: Color,
    focus_accent: Color,
    selection_bg: Color,
    radius: f32,
) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    palette_input_style(
        bg,
        border_idle,
        focus_accent,
        selection_bg,
        TEXT_PRIMARY,
        TEXT_CHANNEL,
        TEXT_MUTED,
        radius,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn palette_input_style(
    bg: Color,
    border_idle: Color,
    focus_accent: Color,
    selection_bg: Color,
    text_color: Color,
    placeholder_color: Color,
    icon_color: Color,
    radius: f32,
) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| {
        let border_color = match status {
            text_input::Status::Focused { .. } => focus_accent,
            text_input::Status::Hovered => Color {
                a: 0.75,
                ..focus_accent
            },
            _ => border_idle,
        };
        text_input::Style {
            background: Background::Color(bg),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: radius.into(),
            },
            icon: icon_color,
            placeholder: placeholder_color,
            value: text_color,
            selection: selection_bg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcag_adaptive_contrast_enforced_across_presets_and_custom_hues() {
        for preset in CyberThemePreset::ALL {
            let pal = preset.palette().ensure_wcag();
            assert!(
                pal.wcag_primary_ratio() >= 7.0,
                "preset {:?} primary contrast too low: {:.2}",
                preset,
                pal.wcag_primary_ratio()
            );
            assert_eq!(pal.wcag_grade_label(), "WCAG AAA");
            assert!(
                contrast_ratio(pal.on_accent_text, pal.accent_primary) >= 4.5,
                "preset {:?} on_accent contrast too low",
                preset
            );
        }

        for depth in SurfaceDepth::ALL {
            for hue in (0_u16..360).step_by(37) {
                let spec = CustomThemeSpec {
                    enabled: true,
                    primary_hue: hue,
                    secondary_hue: (hue + 145) % 360,
                    surface_depth: depth,
                };
                let pal = spec.build_palette();
                assert!(
                    contrast_ratio(pal.text_primary, pal.bg_elevated) >= 7.0,
                    "custom hue {hue} depth {:?} failed AAA",
                    depth
                );
                assert!(
                    contrast_ratio(pal.text_secondary, pal.bg_elevated) >= 4.5,
                    "custom hue {hue} depth {:?} failed AA secondary",
                    depth
                );
                assert!(contrast_ratio(pal.on_accent_text, pal.accent_primary) >= 4.5);
            }
        }
    }
}
