//! Modern AI Agent UX/CX Systems: Spring Physics, Morphing Status, Skeleton Shimmer,
//! CoT/Tool/Artifact Parsing, Multimodal Staging, and Ctrl+K Command Palette.

use crate::app::{ChannelTab, LoraDesktopApp, Message};
use crate::theme::{self, CyberThemePreset};
use iced::alignment::Alignment;
use iced::widget::{
    button, canvas, column, container, row, rule, scrollable, text, text_input, Column, Row,
};
use iced::{
    mouse, Color, Element, Font, Length, Padding, Point, Radians, Rectangle, Renderer, Size, Theme,
};
use std::f32::consts::PI;
use std::path::{Path, PathBuf};

pub const COMMAND_PALETTE_INPUT_ID: &str = "lora_cmd_palette_input";
pub const CHAT_SEARCH_INPUT_ID: &str = "lora_chat_search_input";
pub const CHAT_SCROLL_ID: &str = "lora_chat_feed_scroll";

/// Real spring damper state for modal, drawer & morphing element transitions.
#[derive(Debug, Clone, Copy)]
pub struct SpringFloat {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl SpringFloat {
    pub fn new(initial: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            value: initial,
            velocity: 0.0,
            target: initial,
            stiffness,
            damping,
        }
    }

    pub fn set_target(&mut self, target: f32, reduced_motion: bool) {
        self.target = target;
        if reduced_motion {
            self.value = target;
            self.velocity = 0.0;
        }
    }

    pub fn snap(&mut self, val: f32) {
        self.value = val;
        self.target = val;
        self.velocity = 0.0;
    }

    pub fn step(&mut self, dt: f32, reduced_motion: bool) -> bool {
        if reduced_motion {
            self.value = self.target;
            self.velocity = 0.0;
            return false;
        }
        let sub_dt = dt.clamp(0.002, 0.033);
        let force = -self.stiffness * (self.value - self.target);
        let accel = force - self.damping * self.velocity;
        self.velocity += accel * sub_dt;
        self.value += self.velocity * sub_dt;

        if (self.value - self.target).abs() < 0.004 && self.velocity.abs() < 0.01 {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }
}

/// Standard functional motion curves (ease-out enter, ease-in exit).
#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[inline]
pub fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}

/// Live agent execution phase rendered via the morphing status pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPhase {
    Idle,
    Thinking,
    Streaming,
    Paused,
    Verified,
    Error,
}

impl AgentPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "READY",
            Self::Thinking => "THINKING",
            Self::Streaming => "STREAMING",
            Self::Paused => "PAUSED",
            Self::Verified => "VERIFIED",
            Self::Error => "ATTENTION",
        }
    }
}

/// Morphing state visualizer without spinning spinners.
pub struct MorphingPhaseCanvas {
    pub phase: AgentPhase,
    pub clock: f32,
    pub color: Color,
    pub motion_enabled: bool,
}

impl canvas::Program<Message> for MorphingPhaseCanvas {
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
        let w = bounds.width.max(16.0);
        let h = bounds.height.max(12.0);
        let cy = h * 0.5;

        match self.phase {
            AgentPhase::Idle | AgentPhase::Verified => {
                let r = if self.motion_enabled {
                    3.2 + 0.6 * (self.clock * 1.8).sin()
                } else {
                    3.4
                };
                frame.fill(
                    &canvas::Path::circle(Point::new(w * 0.5, cy), r * 1.7),
                    Color {
                        a: 0.22,
                        ..self.color
                    },
                );
                frame.fill(
                    &canvas::Path::circle(Point::new(w * 0.5, cy), r),
                    self.color,
                );
            }
            AgentPhase::Thinking | AgentPhase::Streaming => {
                let speed = if self.phase == AgentPhase::Streaming {
                    5.2
                } else {
                    2.6
                };
                let bar_w = 2.6;
                let gap = 3.0;
                let start_x = (w - (4.0 * bar_w + 3.0 * gap)) * 0.5;
                for i in 0..4 {
                    let wave = if self.motion_enabled {
                        (self.clock * speed + i as f32 * 0.85).sin() * 0.5 + 0.5
                    } else {
                        0.65
                    };
                    let bh = (3.5 + wave * (h - 4.0)).clamp(3.0, h - 2.0);
                    let x = start_x + i as f32 * (bar_w + gap);
                    frame.fill_rectangle(
                        Point::new(x, cy - bh * 0.5),
                        Size::new(bar_w, bh),
                        self.color,
                    );
                }
            }
            AgentPhase::Paused | AgentPhase::Error => {
                let bar_w = 3.0;
                let bh = (h - 4.0).max(5.0);
                frame.fill_rectangle(
                    Point::new(w * 0.5 - 4.5, cy - bh * 0.5),
                    Size::new(bar_w, bh),
                    self.color,
                );
                frame.fill_rectangle(
                    Point::new(w * 0.5 + 1.5, cy - bh * 0.5),
                    Size::new(bar_w, bh),
                    self.color,
                );
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Diagonal cyberpunk skeleton shimmer rendered while awaiting first token (anti-spinner).
pub struct SkeletonShimmerCanvas {
    pub clock: f32,
    pub base: Color,
    pub shimmer: Color,
    pub motion_enabled: bool,
}

impl canvas::Program<Message> for SkeletonShimmerCanvas {
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
        let w = bounds.width.max(80.0);
        let widths = [w * 0.88, w * 0.66, w * 0.44];
        let sweep_t = if self.motion_enabled {
            (self.clock * 0.85) % 1.4 - 0.2
        } else {
            0.5
        };

        for (idx, line_w) in widths.iter().enumerate() {
            let y = idx as f32 * 14.0 + 2.0;
            frame.fill_rectangle(
                Point::new(0.0, y),
                Size::new(*line_w, 8.0),
                Color {
                    a: 0.48,
                    ..self.base
                },
            );
            if self.motion_enabled {
                let beam_x = (sweep_t * line_w).clamp(0.0, (*line_w - 36.0).max(0.0));
                let beam_w = 48.0_f32.min(*line_w);
                frame.fill_rectangle(
                    Point::new(beam_x, y),
                    Size::new(beam_w, 8.0),
                    Color {
                        a: 0.34,
                        ..self.shimmer
                    },
                );
            }
        }
        vec![frame.into_geometry()]
    }
}

/// Interactive angular Glitch-Cyber companion & mini-loader modeled closely on **Capri** fused with Lora's emblem.
/// Features jagged crimson shard aura, obsidian asymmetrical spikes, toxic-green V-notched mask,
/// concentric cobalt-blue / X-cross eyes, zigzag toothy grin, and crackling neon lightning shards.
pub struct LoraMiniBotCanvas {
    pub phase: AgentPhase,
    pub clock: f32,
    pub bounce: f32,
    pub palette: theme::CyberPalette,
    pub motion_enabled: bool,
    pub style: crate::dashboard::CapriStyleSpec,
}

impl canvas::Program<Message> for LoraMiniBotCanvas {
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
        let s = bounds.width.min(bounds.height).max(22.0);
        let cx = bounds.width * 0.5;
        let poke = self.bounce.clamp(-0.35, 1.35);
        let glitch_amp = (self.style.glitch_intensity_pct as f32 / 100.0).clamp(0.0, 1.5);

        let float_y = if self.motion_enabled {
            (self.clock * 2.8).sin() * (s * 0.036)
        } else {
            0.0
        };
        let jitter_x = if self.motion_enabled
            && (self.phase == AgentPhase::Streaming || self.phase == AgentPhase::Thinking)
        {
            (self.clock * 19.0).sin() * (s * 0.010 * glitch_amp)
        } else {
            0.0
        };
        let cy = bounds.height * 0.52 + float_y - poke * (s * 0.11);
        let hx = cx + jitter_x;

        let look_x = if self.motion_enabled && self.phase == AgentPhase::Thinking {
            (self.clock * 3.1).sin() * (s * 0.032)
        } else if self.motion_enabled {
            (self.clock * 1.2).sin() * (s * 0.014)
        } else {
            0.0
        };

        // Capri customizable trio + glitch violet trim + active theme fusion
        let (capri_crimson, capri_green, capri_blue, capri_violet) =
            self.style.resolve_colors(self.palette);
        let capri_black = Color::from_rgb(0.035, 0.035, 0.055);
        let theme_accent = self.palette.accent_primary;

        let glitch_wink = self.motion_enabled && glitch_amp > 0.1 && (self.clock % 4.1) > 3.65;
        let right_eye_x = match self.style.eye_mode {
            crate::dashboard::CapriEyeMode::Dynamic => {
                poke > 0.10
                    || glitch_wink
                    || matches!(
                        self.phase,
                        AgentPhase::Thinking | AgentPhase::Streaming | AgentPhase::Error
                    )
            }
            crate::dashboard::CapriEyeMode::DoubleBlue => false,
            crate::dashboard::CapriEyeMode::CrossGlitch | crate::dashboard::CapriEyeMode::Hypno => {
                true
            }
        };
        let toothy_grin = self.style.always_toothy_grin
            || poke > 0.10
            || matches!(self.phase, AgentPhase::Streaming | AgentPhase::Verified);

        // 1. Explosive jagged crimson glitch crown & energy fan behind head (Image #3 & #4)
        let aura_pulse = if self.motion_enabled {
            1.0 + 0.06 * glitch_amp * (self.clock * 4.2).sin() + poke.max(0.0) * 0.12
        } else {
            1.0
        };
        if self.style.show_aura_shards {
            let crown_aura = canvas::Path::new(|b| {
                b.move_to(Point::new(hx - s * 0.36 * aura_pulse, cy + s * 0.02));
                b.line_to(Point::new(
                    hx - s * 0.44 * aura_pulse,
                    cy - s * 0.24 * aura_pulse,
                ));
                b.line_to(Point::new(hx - s * 0.28, cy - s * 0.16));
                b.line_to(Point::new(
                    hx - s * 0.29 * aura_pulse,
                    cy - s * 0.43 * aura_pulse,
                ));
                b.line_to(Point::new(hx - s * 0.10, cy - s * 0.24));
                b.line_to(Point::new(
                    hx + s * 0.04 * aura_pulse,
                    cy - s * 0.46 * aura_pulse,
                ));
                b.line_to(Point::new(hx + s * 0.14, cy - s * 0.26));
                b.line_to(Point::new(
                    hx + s * 0.39 * aura_pulse,
                    cy - s * 0.43 * aura_pulse,
                ));
                b.line_to(Point::new(hx + s * 0.26, cy - s * 0.16));
                b.line_to(Point::new(
                    hx + s * 0.45 * aura_pulse,
                    cy - s * 0.18 * aura_pulse,
                ));
                b.line_to(Point::new(hx + s * 0.32, cy + s * 0.06));
                b.close();
            });
            frame.fill(
                &crown_aura,
                Color {
                    a: 0.90,
                    ..capri_crimson
                },
            );
            frame.stroke(
                &crown_aura,
                canvas::Stroke::default()
                    .with_color(Color {
                        a: 0.55,
                        ..capri_violet
                    })
                    .with_width((s * 0.020).max(0.9)),
            );

            // Crisp white/pink glitch stardust specks inside crimson aura
            let specks = [
                (-0.34, -0.26),
                (-0.20, -0.34),
                (-0.04, -0.36),
                (0.11, -0.33),
                (0.28, -0.31),
                (0.35, -0.14),
            ];
            let active_specks = if glitch_amp >= 0.65 {
                6
            } else if glitch_amp >= 0.25 {
                3
            } else {
                0
            };
            for (i, (sx_f, sy_f)) in specks.iter().take(active_specks).enumerate() {
                let tw = if self.motion_enabled {
                    ((self.clock * 5.0 + i as f32 * 1.4).sin() * 0.5 + 0.5).clamp(0.25, 1.0)
                } else {
                    0.85
                };
                let px = hx + s * sx_f * aura_pulse;
                let py = cy + s * sy_f * aura_pulse;
                let sz = (s * 0.025).max(1.2);
                frame.fill_rectangle(
                    Point::new(px, py),
                    Size::new(sz, sz),
                    Color {
                        a: tw,
                        ..Color::WHITE
                    },
                );
            }
        }

        // Lora emblem broken neon orbital slash arc around Capri (subtle Lora bridge!)
        let orbit_rot = if self.motion_enabled {
            self.clock * 1.4
        } else {
            0.2
        };
        let lora_arc = canvas::Path::new(|b| {
            b.arc(canvas::path::Arc {
                center: Point::new(cx, cy),
                radius: s * 0.45 + poke.max(0.0) * (s * 0.04),
                start_angle: Radians(PI * 0.92 + orbit_rot),
                end_angle: Radians(PI * 1.38 + orbit_rot),
            });
        });
        frame.stroke(
            &lora_arc,
            canvas::Stroke::default()
                .with_color(Color {
                    a: 0.72,
                    ..theme_accent
                })
                .with_width((s * 0.024).max(1.1))
                .with_line_cap(canvas::LineCap::Round),
        );

        // 2. Crackling jagged neon lightning shards (Green left, Crimson right - Image #3 & #4)
        if self.style.show_lightning {
            let bolt_shift = if self.motion_enabled {
                (self.clock * 11.0).sin() * (s * 0.018 * glitch_amp)
            } else {
                0.0
            };
            let left_bolt = canvas::Path::new(|b| {
                b.move_to(Point::new(hx - s * 0.28, cy + s * 0.14));
                b.line_to(Point::new(hx - s * 0.39 + bolt_shift, cy + s * 0.23));
                b.line_to(Point::new(hx - s * 0.31, cy + s * 0.27));
                b.line_to(Point::new(hx - s * 0.42 - bolt_shift, cy + s * 0.40));
            });
            frame.stroke(
                &left_bolt,
                canvas::Stroke::default()
                    .with_color(capri_green)
                    .with_width((s * 0.032).max(1.3))
                    .with_line_join(canvas::LineJoin::Miter),
            );
            let right_bolt = canvas::Path::new(|b| {
                b.move_to(Point::new(hx + s * 0.27, cy + s * 0.13));
                b.line_to(Point::new(hx + s * 0.39 - bolt_shift, cy + s * 0.22));
                b.line_to(Point::new(hx + s * 0.30, cy + s * 0.27));
                b.line_to(Point::new(hx + s * 0.42 + bolt_shift, cy + s * 0.40));
            });
            frame.stroke(
                &right_bolt,
                canvas::Stroke::default()
                    .with_color(capri_crimson)
                    .with_width((s * 0.032).max(1.3))
                    .with_line_join(canvas::LineJoin::Miter),
            );
        }

        // 3. Sharp Harlequin Cyber-Torso below chin (Green shoulder + Crimson rib shards)
        let torso = canvas::Path::new(|b| {
            b.move_to(Point::new(hx - s * 0.22, cy + s * 0.22));
            b.line_to(Point::new(hx - s * 0.10, cy + s * 0.18));
            b.line_to(Point::new(hx + s * 0.10, cy + s * 0.18));
            b.line_to(Point::new(hx + s * 0.22, cy + s * 0.23));
            b.line_to(Point::new(hx + s * 0.13, cy + s * 0.42));
            b.line_to(Point::new(hx - s * 0.13, cy + s * 0.42));
            b.close();
        });
        frame.fill(&torso, capri_black);
        frame.stroke(
            &torso,
            canvas::Stroke::default()
                .with_color(capri_violet)
                .with_width((s * 0.022).max(0.9)),
        );
        // Toxic-green left shoulder shard
        let left_shoulder = canvas::Path::new(|b| {
            b.move_to(Point::new(hx - s * 0.21, cy + s * 0.23));
            b.line_to(Point::new(hx - s * 0.10, cy + s * 0.20));
            b.line_to(Point::new(hx - s * 0.12, cy + s * 0.31));
            b.line_to(Point::new(hx - s * 0.20, cy + s * 0.30));
            b.close();
        });
        frame.fill(&left_shoulder, capri_green);
        // Crimson chest diamond core
        let chest_core = canvas::Path::new(|b| {
            b.move_to(Point::new(hx, cy + s * 0.23));
            b.line_to(Point::new(hx + s * 0.07, cy + s * 0.30));
            b.line_to(Point::new(hx, cy + s * 0.38));
            b.line_to(Point::new(hx - s * 0.07, cy + s * 0.30));
            b.close();
        });
        frame.fill(&chest_core, capri_crimson);

        // 4. Capri's sharp obsidian head polygon + 4 asymmetrical razor spikes
        let head = canvas::Path::new(|b| {
            // Sharp diamond face silhouette
            b.move_to(Point::new(hx, cy + s * 0.20)); // V-chin
            b.line_to(Point::new(hx - s * 0.28, cy + s * 0.05)); // left jaw
            b.line_to(Point::new(hx - s * 0.31, cy - s * 0.05)); // left cheekbone
            b.line_to(Point::new(hx - s * 0.29, cy - s * 0.34)); // tall left needle spike
            b.line_to(Point::new(hx - s * 0.23, cy - s * 0.11));
            b.line_to(Point::new(hx - s * 0.06, cy - s * 0.31)); // middle-left sharp ear spike
            b.line_to(Point::new(hx - s * 0.01, cy - s * 0.13));
            b.line_to(Point::new(hx + s * 0.32, cy - s * 0.41)); // huge sweeping right crown spike
            b.line_to(Point::new(hx + s * 0.19, cy - s * 0.12));
            b.line_to(Point::new(hx + s * 0.39, cy - s * 0.20)); // jagged upper-right spike
            b.line_to(Point::new(hx + s * 0.23, cy - s * 0.04));
            b.line_to(Point::new(hx + s * 0.28, cy + s * 0.05)); // right cheekbone
            b.close();
        });
        frame.fill(&head, capri_black);
        frame.stroke(
            &head,
            canvas::Stroke::default()
                .with_color(Color {
                    a: 0.78,
                    ..capri_violet
                })
                .with_width((s * 0.022).max(0.9))
                .with_line_join(canvas::LineJoin::Miter),
        );

        // Subtle Lora hairpin glow slash on the left ear spike
        frame.stroke(
            &canvas::Path::line(
                Point::new(hx - s * 0.23, cy - s * 0.08),
                Point::new(hx - s * 0.14, cy - s * 0.17),
            ),
            canvas::Stroke::default()
                .with_color(theme_accent)
                .with_width((s * 0.036).max(1.4))
                .with_line_cap(canvas::LineCap::Round),
        );

        // 5. Electric Toxic-Green Dual-Wing Eye Mask (#00FF7A) with sharp obsidian V-notch
        let left_mask = canvas::Path::new(|b| {
            b.move_to(Point::new(hx - s * 0.25, cy - s * 0.04));
            b.line_to(Point::new(hx - s * 0.07, cy - s * 0.15));
            b.line_to(Point::new(hx - s * 0.015, cy - s * 0.02));
            b.line_to(Point::new(hx - s * 0.20, cy + s * 0.04));
            b.close();
        });
        frame.fill(&left_mask, capri_green);

        let right_mask = canvas::Path::new(|b| {
            b.move_to(Point::new(hx + s * 0.015, cy - s * 0.02));
            b.line_to(Point::new(hx + s * 0.13, cy - s * 0.17));
            b.line_to(Point::new(hx + s * 0.26, cy - s * 0.06));
            b.line_to(Point::new(hx + s * 0.20, cy + s * 0.04));
            b.close();
        });
        frame.fill(&right_mask, capri_green);

        // 6. Hypnotic Blue Concentric Eye (`O`) & Capri Angular Glitch Cross (`X`)
        let left_eye_c = Point::new(hx - s * 0.115 + look_x, cy - s * 0.042);
        let right_eye_c = Point::new(hx + s * 0.125 + look_x, cy - s * 0.045);
        let eye_r = (s * 0.068).max(2.6);

        if self.phase == AgentPhase::Paused {
            for ec in [left_eye_c, right_eye_c] {
                frame.stroke(
                    &canvas::Path::line(
                        Point::new(ec.x - eye_r * 0.8, ec.y),
                        Point::new(ec.x + eye_r * 0.8, ec.y),
                    ),
                    canvas::Stroke::default()
                        .with_color(capri_black)
                        .with_width((s * 0.036).max(1.5)),
                );
            }
        } else {
            // Helper closure for hypnotic spiral/ring Capri eye
            let draw_hypnotic_eye = |frame: &mut canvas::Frame, c: Point, ring_color: Color| {
                // Sharp angular outer socket
                let sock = canvas::Path::new(|b| {
                    b.move_to(Point::new(c.x - eye_r * 1.18, c.y));
                    b.line_to(Point::new(c.x, c.y - eye_r * 1.05));
                    b.line_to(Point::new(c.x + eye_r * 1.15, c.y - eye_r * 0.15));
                    b.line_to(Point::new(c.x + eye_r * 0.85, c.y + eye_r * 0.92));
                    b.line_to(Point::new(c.x - eye_r * 0.92, c.y + eye_r * 0.85));
                    b.close();
                });
                frame.fill(&sock, capri_black);
                frame.fill(&canvas::Path::circle(c, eye_r * 0.72), ring_color);
                frame.fill(&canvas::Path::circle(c, eye_r * 0.44), capri_black);
                frame.fill(&canvas::Path::circle(c, eye_r * 0.25), Color::WHITE);
            };

            let draw_cross_eye = |frame: &mut canvas::Frame, c: Point, col: Color| {
                let k = eye_r * 0.95;
                let w = (s * 0.036).max(1.5);
                frame.stroke(
                    &canvas::Path::line(
                        Point::new(c.x - k, c.y - k * 0.55),
                        Point::new(c.x + k, c.y + k * 0.65),
                    ),
                    canvas::Stroke::default()
                        .with_color(col)
                        .with_width(w)
                        .with_line_cap(canvas::LineCap::Butt),
                );
                frame.stroke(
                    &canvas::Path::line(
                        Point::new(c.x + k * 0.65, c.y - k * 0.85),
                        Point::new(c.x - k * 0.55, c.y + k * 0.75),
                    ),
                    canvas::Stroke::default()
                        .with_color(col)
                        .with_width(w)
                        .with_line_cap(canvas::LineCap::Butt),
                );
            };

            if self.phase == AgentPhase::Error {
                draw_cross_eye(&mut frame, left_eye_c, capri_crimson);
                draw_cross_eye(&mut frame, right_eye_c, capri_crimson);
            } else {
                draw_hypnotic_eye(&mut frame, left_eye_c, capri_blue);
                if right_eye_x {
                    draw_cross_eye(&mut frame, right_eye_c, capri_black);
                } else {
                    draw_hypnotic_eye(&mut frame, right_eye_c, capri_blue);
                }
            }
        }

        // 7. Signature Zig-Zag Electric Mouth vs White Toothy Grin (Image #1 & #4!)
        if toothy_grin {
            let grin = canvas::Path::new(|b| {
                b.move_to(Point::new(hx - s * 0.12, cy + s * 0.065));
                b.line_to(Point::new(hx + s * 0.12, cy + s * 0.065));
                b.line_to(Point::new(hx + s * 0.05, cy + s * 0.145));
                b.line_to(Point::new(hx - s * 0.06, cy + s * 0.14));
                b.close();
            });
            frame.fill(&grin, Color::WHITE);
            // Jagged shark-tooth notch accents inside grin
            frame.stroke(
                &canvas::Path::new(|b| {
                    b.move_to(Point::new(hx - s * 0.08, cy + s * 0.065));
                    b.line_to(Point::new(hx - s * 0.04, cy + s * 0.105));
                    b.line_to(Point::new(hx, cy + s * 0.068));
                    b.line_to(Point::new(hx + s * 0.04, cy + s * 0.105));
                    b.line_to(Point::new(hx + s * 0.08, cy + s * 0.068));
                }),
                canvas::Stroke::default()
                    .with_color(capri_black)
                    .with_width((s * 0.018).max(0.8)),
            );
        } else {
            let zigzag_mouth = canvas::Path::new(|b| {
                b.move_to(Point::new(hx - s * 0.12, cy + s * 0.095));
                b.line_to(Point::new(hx - s * 0.06, cy + s * 0.075));
                b.line_to(Point::new(hx - s * 0.01, cy + s * 0.102));
                b.line_to(Point::new(hx + s * 0.04, cy + s * 0.078));
                b.line_to(Point::new(hx + s * 0.09, cy + s * 0.104));
                b.line_to(Point::new(hx + s * 0.13, cy + s * 0.082));
            });
            frame.stroke(
                &zigzag_mouth,
                canvas::Stroke::default()
                    .with_color(Color::WHITE)
                    .with_width((s * 0.022).max(1.0))
                    .with_line_join(canvas::LineJoin::Miter),
            );
        }

        vec![frame.into_geometry()]
    }
}

/// Interactive rotating micro-quips spoken by the Capri × Lora bot when poked.
pub fn next_bot_quip(
    poke_count: u64,
    phase: AgentPhase,
    project_name: &str,
    open_tasks: usize,
) -> String {
    if phase == AgentPhase::Thinking {
        return "Glitch-tracing thoughts in isolated RAM...".to_string();
    }
    if phase == AgentPhase::Streaming {
        return "High-voltage token stream locked in!".to_string();
    }
    if phase == AgentPhase::Paused {
        return "Volt-sleep mode: tokens buffered cleanly.".to_string();
    }
    if phase == AgentPhase::Error {
        return "Spark-alert! Review consent or error gate.".to_string();
    }
    let bank = [
        format!("Capri spark armed for {project_name}! Ready to hack."),
        format!("{open_tasks} open task(s) buzzing — poke Open Tasks to attack!"),
        "Zero-egress glitch shield locked · 100% local runtime.".to_string(),
        "Hyper-tip: Slam Ctrl+K anytime for instant warp navigation.".to_string(),
        "Drop .rs, images, audio, or .gguf shards straight onto me!".to_string(),
        "Prefer chill visuals? Flip Reduced Motion in Custom Studio.".to_string(),
    ];
    let idx = (poke_count as usize) % bank.len();
    bank[idx].clone()
}

/// Live token budget usage calculated uniformly across Chat, Code, Daily & Capri Widget.
#[derive(Debug, Clone, Copy)]
pub struct ContextBudgetUsage {
    pub history_tokens: usize,
    pub attached_tokens: usize,
    pub composer_tokens: usize,
    pub total_tokens: usize,
    pub max_tokens: usize,
    pub pct: f32,
}

pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        0
    } else {
        (chars / 4).max(1)
    }
}

pub fn compute_context_budget_usage(
    messages: &[crate::app::ChatMessageEntry],
    staged: &[StagedAttachment],
    composer: &str,
    requested_ctx: u32,
) -> ContextBudgetUsage {
    let history_tokens: usize = messages
        .iter()
        .map(|m| m.token_estimate.max(estimate_tokens(&m.raw_text)))
        .sum();
    let attached_tokens: usize = staged
        .iter()
        .map(|a| a.text_preview.as_deref().map(estimate_tokens).unwrap_or(64))
        .sum();
    let composer_tokens = estimate_tokens(composer);
    let total_tokens = history_tokens + attached_tokens + composer_tokens;
    let max_tokens = (requested_ctx as usize).max(512);
    let pct = ((total_tokens as f32 / max_tokens as f32) * 100.0).clamp(0.0, 100.0);
    ContextBudgetUsage {
        history_tokens,
        attached_tokens,
        composer_tokens,
        total_tokens,
        max_tokens,
        pct,
    }
}

pub fn is_dotenv_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    name == ".env" || name.starts_with(".env.") || name.ends_with(".env")
}

/// Clickable chibi Lora × Capri companion widget (reusable in sidebar, HUD, Chat/Code dock, and loaders).
pub fn view_mini_bot_avatar<'a>(
    app: &'a LoraDesktopApp,
    size_px: f32,
    phase_override: Option<AgentPhase>,
) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let phase = phase_override.unwrap_or_else(|| app.agent_phase());
    let c_rad = app.dashboard.control_radius();
    button(
        canvas(LoraMiniBotCanvas {
            phase,
            clock: app.dashboard.motion.phase,
            bounce: app.dashboard.motion.bot_bounce.value,
            palette: pal,
            motion_enabled: app.dashboard.prefs.cyber_motion,
            style: app.dashboard.prefs.capri,
        })
        .width(Length::Fixed(size_px))
        .height(Length::Fixed(size_px)),
    )
    .padding(2)
    .style(theme::cyber_chip_button_with_radius(
        false,
        pal.accent_primary,
        pal.bg_rail,
        pal.border_subtle,
        pal.text_primary,
        pal.text_primary,
        c_rad,
    ))
    .on_press(Message::PokeMiniBot)
    .into()
}

/// Compact dockable Capri Companion + Token/Model/Trust Widget strip for Chat & Code views.
pub fn view_capri_chat_widget<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let c_rad = app.dashboard.control_radius();
    let rad = app.dashboard.card_radius();
    let capri = &app.dashboard.prefs.capri;
    let phase = app.agent_phase();
    let usage = compute_context_budget_usage(
        &app.messages,
        &app.staged_attachments,
        &app.composer_input,
        app.requested_context,
    );
    let model_text = match app.access_mode {
        crate::workspace_db::ExecutionAccessMode::Local => {
            if let Some(m) = &app.selected_model {
                format!("LOCAL GGUF · {m}")
            } else {
                "LOCAL · NO GGUF LOADED".to_string()
            }
        }
        crate::workspace_db::ExecutionAccessMode::Api => {
            format!("API GATEWAY · {}", app.api_config.model_name)
        }
    };
    let scale = (capri.widget_scale_px as f32).clamp(36.0, 62.0);

    container(
        row![
            view_mini_bot_avatar(app, scale, None),
            column![
                row![
                    text(format!("CAPRI × LORA · {}", phase.label()))
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(pal.accent_primary),
                    container(text(model_text).size(9).font(Font::MONOSPACE))
                        .padding(Padding::from([1, 6]))
                        .style(theme::pill_container(
                            pal.bg_rail,
                            pal.accent_secondary,
                            pal.border_subtle
                        )),
                    text(format!(
                        "CTX ~{}/{} ({:.0}%)",
                        usage.total_tokens, usage.max_tokens, usage.pct
                    ))
                    .size(10)
                    .font(Font::MONOSPACE)
                    .color(if usage.pct > 82.0 {
                        pal.warning
                    } else {
                        pal.nominal
                    }),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                text(app.bot_quip.as_str())
                    .size(11)
                    .color(pal.text_secondary),
            ]
            .spacing(3)
            .width(Length::Fill),
            button(
                text(format!("Aura: {}", capri.aura_preset.label()))
                    .size(10)
                    .font(Font::MONOSPACE)
            )
            .padding(Padding::from([4, 9]))
            .style(theme::cyber_chip_button_with_radius(
                false,
                pal.accent_primary,
                pal.bg_elevated,
                pal.border_subtle,
                pal.text_primary,
                pal.accent_primary,
                c_rad,
            ))
            .on_press(Message::CapriAuraPresetChanged(capri.aura_preset.next())),
            button(text("Customize").size(10).font(Font::MONOSPACE))
                .padding(Padding::from([4, 9]))
                .style(theme::cyber_chip_button_with_radius(
                    false,
                    pal.accent_secondary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_secondary,
                    c_rad,
                ))
                .on_press(Message::SelectChannel(ChannelTab::Settings)),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([7, 12]))
    .width(Length::Fill)
    .style(theme::palette_card_container(
        pal.bg_card,
        pal.text_primary,
        pal.border_subtle,
        rad,
    ))
    .into()
}

/// Extracted code/artifact block from assistant markdown responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedArtifact {
    pub title: String,
    pub lang: String,
    pub content: String,
}

/// Cleanly separated Chain-of-Thought trace vs visible answer vs artifacts.
#[derive(Debug, Clone, Default)]
pub struct ParsedAgentOutput {
    pub thinking: Option<String>,
    pub thinking_in_progress: bool,
    pub answer: String,
    pub artifacts: Vec<ExtractedArtifact>,
}

/// Separate `<think>...</think>` Chain of Thought and fenced code artifacts from raw text.
pub fn parse_structured_response(raw: &str) -> ParsedAgentOutput {
    let mut thinking_parts = Vec::new();
    let mut thinking_in_progress = false;
    let mut clean = String::with_capacity(raw.len());
    let mut rest = raw;

    while let Some(start) = rest.find("<think>") {
        clean.push_str(&rest[..start]);
        let after_start = &rest[start + 7..];
        if let Some(end) = after_start.find("</think>") {
            let inner = after_start[..end].trim();
            if !inner.is_empty() {
                thinking_parts.push(inner.to_string());
            }
            rest = &after_start[end + 8..];
        } else {
            let inner = after_start.trim();
            if !inner.is_empty() {
                thinking_parts.push(inner.to_string());
            }
            thinking_in_progress = true;
            rest = "";
            break;
        }
    }
    clean.push_str(rest);

    let mut artifacts = Vec::new();
    let mut lines = clean.lines().enumerate().peekable();
    while let Some((_, line)) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(lang_tag) = trimmed.strip_prefix("```") {
            let lang = lang_tag.trim().to_string();
            let mut body = String::new();
            let mut closed = false;
            for (_, inner) in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    closed = true;
                    break;
                }
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(inner);
            }
            if !body.trim().is_empty() {
                let idx = artifacts.len() + 1;
                let lang_clean = if lang.is_empty() {
                    "text".to_string()
                } else {
                    lang
                };
                artifacts.push(ExtractedArtifact {
                    title: format!("Artifact #{idx} ({lang_clean})"),
                    lang: lang_clean,
                    content: if closed {
                        body
                    } else {
                        format!("{body}\n// streaming...")
                    },
                });
            }
        }
    }

    let thinking = (!thinking_parts.is_empty()).then(|| thinking_parts.join("\n\n"));
    ParsedAgentOutput {
        thinking,
        thinking_in_progress,
        answer: clean.trim().to_string(),
        artifacts,
    }
}

/// Multimodal attachment types accepted via Drag & Drop or Clipboard staging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Audio,
    CodeDoc,
    GgufModel,
    ClipboardText,
}

impl AttachmentKind {
    pub fn badge(self) -> &'static str {
        match self {
            Self::Image => "IMG",
            Self::Audio => "AUDIO",
            Self::CodeDoc => "FILE",
            Self::GgufModel => "GGUF",
            Self::ClipboardText => "CLIP",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAttachment {
    pub kind: AttachmentKind,
    pub label: String,
    pub path: Option<PathBuf>,
    pub text_preview: Option<String>,
    pub bytes: u64,
}

pub fn classify_dropped_path(path: &Path) -> StagedAttachment {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let kind = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" => AttachmentKind::Image,
        "wav" | "mp3" | "ogg" | "flac" | "m4a" => AttachmentKind::Audio,
        "gguf" => AttachmentKind::GgufModel,
        _ => AttachmentKind::CodeDoc,
    };
    let label = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment")
        .to_string();
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let text_preview = if kind == AttachmentKind::CodeDoc && bytes > 0 && bytes <= 16_384 {
        std::fs::read_to_string(path).ok().map(|s| {
            if s.chars().count() > 600 {
                let short: String = s.chars().take(600).collect();
                format!("{short}...")
            } else {
                s
            }
        })
    } else {
        None
    };

    StagedAttachment {
        kind,
        label,
        path: Some(path.to_path_buf()),
        text_preview,
        bytes,
    }
}

pub fn stage_clipboard_text(raw: &str) -> Option<StagedAttachment> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.len() as u64;
    let preview = if trimmed.chars().count() > 800 {
        let short: String = trimmed.chars().take(800).collect();
        format!("{short}...")
    } else {
        trimmed.to_string()
    };
    Some(StagedAttachment {
        kind: AttachmentKind::ClipboardText,
        label: "clipboard-snippet.txt".to_string(),
        path: None,
        text_preview: Some(preview),
        bytes,
    })
}

/// Zero-friction onboarding templates displayed on fresh sessions.
pub struct OperatorTemplate {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub read_only_recommended: bool,
    pub prompt: &'static str,
}

pub const OPERATOR_TEMPLATES: [OperatorTemplate; 4] = [
    OperatorTemplate {
        title: "Workspace Audit",
        subtitle: "Map files, crates & entrypoints",
        read_only_recommended: true,
        prompt: "Inspect this project workspace structure, summarize core modules, and list high-priority checks.",
    },
    OperatorTemplate {
        title: "Security Posture",
        subtitle: "Verify air-gap & consent gates",
        read_only_recommended: false,
        prompt: "Explain active air-gapped policy boundaries, SHA-256 attestations, and fail-closed consent controls.",
    },
    OperatorTemplate {
        title: "llama.cpp Tuning",
        subtitle: "Optimize layers, KV & context",
        read_only_recommended: false,
        prompt: "Recommend optimal GPU offload, KV quantization, and context window settings for my host RAM.",
    },
    OperatorTemplate {
        title: "Rust Defect Scan",
        subtitle: "Look for edge cases & tests",
        read_only_recommended: true,
        prompt: "Review recent code paths in the workspace and highlight potential panic or concurrency risks.",
    },
];

/// Floating Ctrl+K Command Palette modal overlay.
pub fn view_command_palette<'a>(app: &'a LoraDesktopApp) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();
    let query = app.command_palette_query.trim().to_ascii_lowercase();

    let mut commands: Vec<(&'static str, &'static str, Message)> = vec![
        (
            "NAV",
            "Go to Command HUD",
            Message::SelectChannel(ChannelTab::Dashboard),
        ),
        (
            "NAV",
            "Go to Agent Chat",
            Message::SelectChannel(ChannelTab::Chat),
        ),
        (
            "NAV",
            "Open Code, Trusted Folders & .env",
            Message::SelectChannel(ChannelTab::CodeWorkspace),
        ),
        (
            "NAV",
            "Open Daily, Tasks & Completed Log",
            Message::SelectChannel(ChannelTab::WorkflowPipeline),
        ),
        (
            "NAV",
            "Open Projects & Workspaces",
            Message::SelectChannel(ChannelTab::AgentWorkspace),
        ),
        (
            "NAV",
            "Open llama.cpp Control Panel",
            Message::SelectChannel(ChannelTab::Hardware),
        ),
        (
            "NAV",
            "Open Model Registry (GGUF)",
            Message::SelectChannel(ChannelTab::ModelLibrary),
        ),
        (
            "NAV",
            "Open Infinite Custom Studio & Capri Lab",
            Message::SelectChannel(ChannelTab::Settings),
        ),
        (
            "NAV",
            "Open Audit Log",
            Message::SelectChannel(ChannelTab::AuditJournal),
        ),
        (
            "NAV",
            "Open Security Broker",
            Message::SelectChannel(ChannelTab::BrokerGate),
        ),
        (
            "AGENT",
            "Stop Active Inference Run",
            Message::StopActiveTask,
        ),
        ("AGENT", "Pause / Resume Streaming", Message::TogglePauseRun),
        (
            "AGENT",
            "Toggle Artifacts & Live Preview",
            Message::ToggleArtifactPanel,
        ),
        ("AGENT", "Toggle Telemetry Radar", Message::ToggleInspector),
        (
            "AGENT",
            "Clear Project Chat (Reversible)",
            Message::ClearProjectChat,
        ),
        (
            "AGENT",
            "Undo Last Reversible Action",
            Message::UndoLastAction,
        ),
        (
            "AGENT",
            "Poke / Interact with Lora Mini-Bot",
            Message::PokeMiniBot,
        ),
        (
            "UI",
            "Toggle Animated Lora Mini-Bot",
            Message::ToggleMiniBot(!app.dashboard.prefs.show_mini_bot),
        ),
        (
            "UI",
            "Toggle Far-Left Icon Rail",
            Message::ToggleIconRail(!app.dashboard.prefs.show_icon_rail),
        ),
        (
            "UI",
            "Toggle Reduced Motion Mode",
            Message::ToggleCyberMotion(!app.dashboard.prefs.cyber_motion),
        ),
        (
            "THEME",
            "Theme: Night City 2077",
            Message::SelectCyberTheme(CyberThemePreset::NightCity2077),
        ),
        (
            "THEME",
            "Theme: Blade Runner 2049",
            Message::SelectCyberTheme(CyberThemePreset::BladeRunner2049),
        ),
        (
            "THEME",
            "Theme: The Matrix Grid",
            Message::SelectCyberTheme(CyberThemePreset::TheMatrix1999),
        ),
        (
            "THEME",
            "Theme: Ghost in the Shell",
            Message::SelectCyberTheme(CyberThemePreset::GhostInTheShell),
        ),
        (
            "THEME",
            "Theme: Tron Legacy Grid",
            Message::SelectCyberTheme(CyberThemePreset::TronLegacyGrid),
        ),
    ];

    let mut filtered_rows = Column::new().spacing(4);
    let mut matches_found = 0usize;

    for (cat, label, msg) in commands.drain(..) {
        if !query.is_empty()
            && !label.to_ascii_lowercase().contains(&query)
            && !cat.to_ascii_lowercase().contains(&query)
        {
            continue;
        }
        matches_found += 1;
        filtered_rows = filtered_rows.push(palette_cmd_row(cat, label, msg, pal, c_rad));
    }

    for proj in &app.projects_cache {
        let lbl = format!("Switch Project -> {} [{}]", proj.name, proj.accent_tag);
        if !query.is_empty()
            && !lbl.to_ascii_lowercase().contains(&query)
            && !"project".contains(&query)
        {
            continue;
        }
        matches_found += 1;
        filtered_rows = filtered_rows.push(
            button(
                row![
                    container(text("PROJ").size(9).font(Font::MONOSPACE))
                        .padding(Padding::from([2, 6]))
                        .style(theme::pill_container(
                            pal.accent_secondary_soft,
                            pal.accent_secondary,
                            pal.border_subtle,
                        )),
                    text(lbl)
                        .size(12)
                        .color(pal.text_primary)
                        .width(Length::Fill),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([9, 12]))
            .width(Length::Fill)
            .style(theme::palette_channel_button(
                proj.id == app.active_project_id,
                pal.bg_elevated,
                pal.bg_card,
                pal.border_accent,
                pal.text_primary,
                pal.text_secondary,
                pal.text_muted,
            ))
            .on_press(Message::RunCommandPaletteAction(Box::new(
                Message::SelectWorkspaceProject(proj.id),
            ))),
        );
    }

    if matches_found == 0 {
        filtered_rows = filtered_rows.push(
            container(
                text("No matching command. Try 'llama', 'theme', 'tasks', or 'undo'.")
                    .size(12)
                    .color(pal.text_muted),
            )
            .padding(14),
        );
    }

    let modal_card = container(
        column![
            row![
                text("COMMAND PALETTE")
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.accent_secondary)
                    .width(Length::Fill),
                container(text("Ctrl+K · Esc").size(10).font(Font::MONOSPACE))
                    .padding(Padding::from([2, 7]))
                    .style(theme::pill_container(
                        pal.bg_elevated,
                        pal.text_muted,
                        pal.border_subtle,
                    )),
                button(text("Close").size(11))
                    .padding(Padding::from([4, 9]))
                    .style(theme::solid_button_with_radius(
                        pal.bg_elevated,
                        pal.accent_primary,
                        pal.text_primary,
                        c_rad,
                    ))
                    .on_press(Message::CloseCommandPalette),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            text_input(
                "Type a command, destination, project, or theme...",
                &app.command_palette_query,
            )
            .id(iced::widget::Id::new(COMMAND_PALETTE_INPUT_ID))
            .on_input(Message::CommandPaletteQueryChanged)
            .padding(Padding::from([10, 14]))
            .style(theme::palette_input_style(
                pal.bg_elevated,
                pal.accent_primary,
                pal.accent_secondary,
                pal.accent_primary_soft,
                pal.text_primary,
                pal.text_muted,
                pal.text_muted,
                c_rad,
            ))
            .width(Length::Fill),
            rule::horizontal(1).style(theme::palette_divider_rule(pal)),
            scrollable(filtered_rows).height(Length::Fixed(320.0)),
        ]
        .spacing(10),
    )
    .padding(16)
    .width(Length::Fixed(610.0))
    .style(theme::palette_modal_card(pal, rad.max(8.0)));

    container(
        container(modal_card)
            .padding(Padding::from([64, 24]))
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Top)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(iced::Background::Color(Color::from_rgba(
            0.02, 0.03, 0.06, 0.78,
        ))),
        ..Default::default()
    })
    .into()
}

fn palette_cmd_row<'a>(
    category: &'static str,
    label: &'static str,
    target_msg: Message,
    pal: theme::CyberPalette,
    c_rad: f32,
) -> Element<'a, Message> {
    let _ = c_rad;
    button(
        row![
            container(text(category).size(9).font(Font::MONOSPACE))
                .padding(Padding::from([2, 6]))
                .style(theme::pill_container(
                    pal.bg_elevated,
                    pal.accent_primary,
                    pal.border_subtle,
                )),
            text(label)
                .size(12)
                .color(pal.text_primary)
                .width(Length::Fill),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([9, 12]))
    .width(Length::Fill)
    .style(theme::palette_channel_button(
        false,
        pal.bg_elevated,
        pal.bg_card,
        pal.border_accent,
        pal.text_primary,
        pal.text_secondary,
        pal.text_muted,
    ))
    .on_press(Message::RunCommandPaletteAction(Box::new(target_msg)))
    .into()
}

/// Modular right-side Artifacts, Diagrams & Live Code Preview drawer in `Agent Chat`.
pub fn view_artifact_drawer<'a>(
    app: &'a LoraDesktopApp,
    artifacts: &[ExtractedArtifact],
    width_px: f32,
) -> Element<'a, Message> {
    let pal = app.dashboard.palette();
    let rad = app.dashboard.card_radius();
    let c_rad = app.dashboard.control_radius();

    let mut header = row![
        text("ARTIFACTS & PREVIEW")
            .size(11)
            .font(Font::MONOSPACE)
            .color(pal.accent_secondary)
            .width(Length::Fill),
        button(text("Close").size(10))
            .padding(Padding::from([3, 8]))
            .style(theme::solid_button_with_radius(
                pal.bg_elevated,
                pal.accent_primary,
                pal.text_primary,
                c_rad,
            ))
            .on_press(Message::ToggleArtifactPanel),
    ]
    .align_y(Alignment::Center);

    let _ = &mut header;

    let mut col = Column::new().spacing(10).padding(12);
    col = col.push(header);
    col = col.push(rule::horizontal(1).style(theme::palette_divider_rule(pal)));

    if artifacts.is_empty() {
        col = col.push(
            container(
                column![
                    text("No fenced artifacts in current session yet.")
                        .size(12)
                        .color(pal.text_secondary),
                    text("When the model outputs code blocks, diffs, or diagrams, inspect and copy them cleanly here.")
                        .size(11)
                        .color(pal.text_muted),
                ]
                .spacing(6),
            )
            .padding(12)
            .width(Length::Fill)
            .style(theme::palette_card_container(
                pal.bg_card,
                pal.text_primary,
                pal.border_subtle,
                rad,
            )),
        );
    } else {
        let selected_idx = app.selected_artifact_idx.min(artifacts.len() - 1);
        let mut tabs = Row::new().spacing(6);
        for (idx, art) in artifacts.iter().enumerate() {
            tabs = tabs.push(
                button(
                    text(format!("#{} {}", idx + 1, art.lang))
                        .size(10)
                        .font(Font::MONOSPACE),
                )
                .padding(Padding::from([4, 8]))
                .style(theme::cyber_chip_button_with_radius(
                    idx == selected_idx,
                    pal.accent_primary,
                    pal.bg_elevated,
                    pal.border_subtle,
                    pal.text_primary,
                    pal.text_muted,
                    c_rad,
                ))
                .on_press(Message::SelectArtifactTab(idx)),
            );
        }
        col = col.push(scrollable(tabs));

        let art = &artifacts[selected_idx];
        let line_count = art.content.lines().count();
        col = col.push(
            row![
                text(format!("{} · {line_count} lines", art.title))
                    .size(11)
                    .font(Font::MONOSPACE)
                    .color(pal.text_secondary)
                    .width(Length::Fill),
                button(text("Copy Code").size(10))
                    .padding(Padding::from([4, 9]))
                    .style(theme::solid_button_with_radius(
                        pal.accent_secondary,
                        pal.accent_primary,
                        pal.on_accent_text,
                        c_rad,
                    ))
                    .on_press(Message::CopyTextToClipboard(art.content.clone())),
            ]
            .align_y(Alignment::Center),
        );

        col = col.push(
            container(
                scrollable(
                    text(art.content.clone())
                        .size(11)
                        .font(Font::MONOSPACE)
                        .color(pal.text_primary),
                )
                .height(Length::Fill),
            )
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::palette_card_container(
                pal.bg_rail,
                pal.text_primary,
                pal.border_subtle,
                rad,
            )),
        );
    }

    container(col)
        .width(Length::Fixed(width_px))
        .height(Length::Fill)
        .style(theme::palette_surface_container(
            pal.bg_sidebar,
            pal.text_primary,
        ))
        .into()
}

/// Optional non-blocking native Windows toast notification when long runs finish.
pub fn notify_desktop_completion(enabled: bool, title: &str, body: &str) {
    if !enabled || cfg!(test) {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        let title_clean = title.replace('\'', "");
        let body_clean: String = body.chars().take(120).collect::<String>().replace('\'', "");
        std::thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let script = format!(
                "Add-Type -AssemblyName System.Windows.Forms; \
                 $n = New-Object System.Windows.Forms.NotifyIcon; \
                 $n.Icon = [System.Drawing.SystemIcons]::Information; \
                 $n.Visible = $true; \
                 $n.ShowBalloonTip(3000, '{title_clean}', '{body_clean}', [System.Windows.Forms.ToolTipIcon]::Info); \
                 Start-Sleep -Seconds 3; $n.Dispose()"
            );
            let _ = std::process::Command::new("powershell")
                .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        });
    }
}
