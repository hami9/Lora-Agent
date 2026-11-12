# 0004 · Desktop client

Status: adopted pure-Rust native GUI baseline (`iced 0.14`), updated 2026-11-12.

## Context

The project needs an ultra-fast, low-memory, accessible desktop client with transparent trusted permission prompts on Linux, Windows, and macOS without shipping an embedded browser runtime or depending on inconsistent system WebViews.

## Decision

Implement `lora-desktop` directly in **100% pure Rust using `iced 0.14`** (`wgpu` GPU renderer with pure-Rust `tiny-skia` CPU fallback, native progressive `markdown` streaming, and `cosmic-text` RTL/Unicode shaping) using a multi-pane cyberpunk workspace layout. Bind `LoraApplicationService` and `GuiApprovalPromptPort` directly in-process. Trusted prompts render non-spoofable fields from `TrustedConsentDialogViewModel`, default safely to `Deny`, and never execute or parse untrusted HTML/CSS/JavaScript.

## Alternatives

Tauri 2 / system WebView introduces cross-OS WebView inconsistencies, HTML/JS serialization bridges, and CSP/origin maintenance overhead. Electron bundles Chromium and exceeds low-end memory budgets. Native per-OS toolkits duplicate UI code three times.

## Tradeoffs

All UI layout and state live in a single strongly-typed Elm-architecture Rust crate sharing exact types with `lora-core`. Binary packaging avoids system WebView installation requirements and eliminates XSS/CSP attack surfaces completely. The headless diagnostic CLI flags preserve zero-display CI testability.

## Consequences

Verify headless diagnostic output, Trusted Consent default-deny transitions, live streaming Markdown updates, and `DesktopSnapshot` verification in workspace tests (`apps/desktop/tests/desktop_journeys.rs`). Keep credentials and native file handles isolated in `lora-core` / `lora-platform` behind explicit user approval.

[Detailed specification](../DESKTOP_UX.md)
