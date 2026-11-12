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
#[derive(Debug, Clone, Copy)]
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

impl CyberPalette {
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

/// Style for Markdown blocks rendered directly inside the native `#chat` message stream.
pub fn chat_markdown_style() -> markdown::Style {
    markdown::Style {
        font: iced::Font::DEFAULT,
        inline_code_highlight: markdown::Highlight {
            background: BG_RAIL.into(),
            border: border::rounded(4),
        },
        inline_code_padding: iced::Padding::from([1, 4]),
        inline_code_color: LORA_MAGENTA,
        inline_code_font: iced::Font::MONOSPACE,
        code_block_font: iced::Font::MONOSPACE,
        link_color: LORA_MAGENTA,
    }
}

pub fn surface_container(bg: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(TEXT_PRIMARY),
        ..container::Style::default()
    }
}

pub fn card_container(
    bg: Color,
    border_color: Color,
    radius: f32,
) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: radius.into(),
        },
        ..container::Style::default()
    }
}

/// Minimalist cyberpunk card with optional subtle neon ambient edge glow.
pub fn cyber_card(
    bg: Color,
    border_color: Color,
    glow_color: Option<Color>,
    radius: f32,
) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: radius.into(),
        },
        shadow: match glow_color {
            Some(c) => Shadow {
                color: Color { a: 0.18, ..c },
                offset: Vector::new(0.0, 2.0),
                blur_radius: 14.0,
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
    container::Style {
        background: Some(Background::Color(BG_CARD)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: AMBER,
            width: 1.5,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.72),
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
                color: if active { TEXT_PRIMARY } else { BORDER_SUBTLE },
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
            icon: TEXT_MUTED,
            placeholder: TEXT_CHANNEL,
            value: TEXT_PRIMARY,
            selection: selection_bg,
        }
    }
}
