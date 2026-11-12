//! Discord-inspired Obsidian Dark + Lora Magenta native `iced` design tokens and widget styles.

use iced::border::{self, Border};
use iced::theme::Palette;
use iced::widget::{button, container, markdown, rule, text_input};
use iced::{Background, Color, Shadow, Theme, Vector};

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a)
}

pub const BG_RAIL: Color = rgb(0x1E, 0x1F, 0x22);
pub const BG_SIDEBAR: Color = rgb(0x2B, 0x2D, 0x31);
pub const BG_STAGE: Color = rgb(0x31, 0x33, 0x38);
pub const BG_CARD: Color = rgb(0x26, 0x28, 0x2D);
pub const BG_COMPOSER: Color = rgb(0x38, 0x3A, 0x40);
pub const BG_HOVER: Color = rgb(0x3F, 0x41, 0x47);
pub const BG_SELECTED: Color = rgb(0x40, 0x42, 0x49);
pub const BG_DOCK: Color = rgb(0x23, 0x24, 0x28);

pub const BORDER_SUBTLE: Color = rgb(0x1F, 0x20, 0x24);
pub const BORDER_ACCENT: Color = rgba(0xC8, 0x4B, 0xFA, 0.55);

pub const TEXT_PRIMARY: Color = rgb(0xF2, 0xF3, 0xF5);
pub const TEXT_SECONDARY: Color = rgb(0xDB, 0xDE, 0xE1);
pub const TEXT_MUTED: Color = rgb(0x94, 0x9B, 0xA4);
pub const TEXT_CHANNEL: Color = rgb(0x80, 0x84, 0x8E);

pub const BLURPLE: Color = rgb(0x58, 0x65, 0xF2);
pub const BLURPLE_HOVER: Color = rgb(0x47, 0x52, 0xC4);
pub const LORA_MAGENTA: Color = rgb(0xC8, 0x4B, 0xFA);
pub const LORA_MAGENTA_SOFT: Color = rgba(0xC8, 0x4B, 0xFA, 0.16);
pub const GREEN: Color = rgb(0x23, 0xA5, 0x5A);
pub const GREEN_SOFT: Color = rgba(0x23, 0xA5, 0x5A, 0.18);
pub const AMBER: Color = rgb(0xF0, 0xB2, 0x32);
pub const AMBER_SOFT: Color = rgba(0xF0, 0xB2, 0x32, 0.16);
pub const RED: Color = rgb(0xDA, 0x37, 0x3C);
pub const RED_HOVER: Color = rgb(0xA1, 0x28, 0x2C);
pub const RED_SOFT: Color = rgba(0xDA, 0x37, 0x3C, 0.16);

pub const MODAL_BACKDROP: Color = rgba(0x0B, 0x0C, 0x10, 0.80);

/// Construct the custom Lora Obsidian Dark theme compatible with Discord layout colors and WCAG contrast.
pub fn lora_discord_theme() -> Theme {
    Theme::custom(
        "Lora Discord Obsidian",
        Palette {
            background: BG_STAGE,
            text: TEXT_PRIMARY,
            primary: BLURPLE,
            success: GREEN,
            warning: AMBER,
            danger: RED,
        },
    )
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
        background: Some(Background::Color(BG_SIDEBAR)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: AMBER,
            width: 1.5,
            radius: 12.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.65),
            offset: Vector::new(0.0, 12.0),
            blur_radius: 32.0,
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

/// Guild server icon pill button in the far-left Discord rail.
pub fn rail_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = if active {
            LORA_MAGENTA
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => BLURPLE,
                _ => BG_STAGE,
            }
        };
        let radius =
            if active || matches!(status, button::Status::Hovered | button::Status::Pressed) {
                14.0
            } else {
                24.0
            };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: TEXT_PRIMARY,
            border: Border {
                color: if active {
                    TEXT_PRIMARY
                } else {
                    Color::TRANSPARENT
                },
                width: if active { 1.0 } else { 0.0 },
                radius: radius.into(),
            },
            ..button::Style::default()
        }
    }
}

/// Discord `#channel` row button in the left channel list sidebar.
pub fn channel_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, fg) = if active {
            (BG_SELECTED, TEXT_PRIMARY)
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => (BG_HOVER, TEXT_SECONDARY),
                _ => (Color::TRANSPARENT, TEXT_MUTED),
            }
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: if active {
                    BORDER_ACCENT
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

/// Parameterized solid action button (Blurple primary, Red danger/Stop, Green success, Subtle secondary).
pub fn solid_button(
    normal: Color,
    hover: Color,
    text_color: Color,
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
                radius: 6.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn composer_input_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused { .. } => LORA_MAGENTA,
        text_input::Status::Hovered => BLURPLE,
        _ => BORDER_SUBTLE,
    };
    text_input::Style {
        background: Background::Color(BG_COMPOSER),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 8.0.into(),
        },
        icon: TEXT_MUTED,
        placeholder: TEXT_CHANNEL,
        value: TEXT_PRIMARY,
        selection: BLURPLE,
    }
}
