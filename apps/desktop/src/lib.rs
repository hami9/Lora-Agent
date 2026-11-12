//! Pure-Rust native multi-pane desktop client and headless diagnostics (`docs/DESKTOP_UX.md`, `ADR-0004`).

pub mod agent_views;
pub mod app;
pub mod backend;
pub mod dashboard;
pub mod theme;
pub mod ux_studio;
pub mod workspace_db;

use lora_core::{ApprovalRequest, TrustedConsentDialogViewModel};
use std::path::Path;

/// Square pixel dimension of the embedded Lora Agent desktop icon.
pub const LORA_ICON_SIZE: u32 = 128;

/// Pre-rasterized 128x128 RGBA8888 pixels for the Lora Agent app icon (zero-codec embed).
pub const LORA_ICON_RGBA: &[u8] = include_bytes!("../assets/lora-icon-128.rgba");

/// Load the native OS window and taskbar icon from embedded RGBA pixels.
pub fn load_window_icon() -> Option<iced::window::Icon> {
    iced::window::icon::from_rgba(LORA_ICON_RGBA.to_vec(), LORA_ICON_SIZE, LORA_ICON_SIZE).ok()
}

/// Load the embedded Lora app logo handle for in-app chrome rendering.
pub fn load_logo_handle() -> iced::widget::image::Handle {
    iced::widget::image::Handle::from_rgba(LORA_ICON_SIZE, LORA_ICON_SIZE, LORA_ICON_RGBA.to_vec())
}

/// Construct a deterministic, HTML/script-escaped consent dialog preview for diagnostics and tests.
pub fn build_consent_preview(data_dir: &Path, rel_path: String) -> TrustedConsentDialogViewModel {
    let req = ApprovalRequest {
        run_id: "run-desktop-preview".to_string(),
        proposal_id: "prop-desktop-1".to_string(),
        tool_id: "read_file".to_string(),
        root_id: "ws-desktop".to_string(),
        root_display_path: data_dir.to_string_lossy().to_string(),
        relative_path: rel_path,
        max_bytes: 32768,
        purpose: "Read file in approved workspace scope".to_string(),
        approval_digest: "sha256:preview-action-digest".to_string(),
    };
    TrustedConsentDialogViewModel::from_approval_request(&req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_lora_icon_has_expected_rgba_size_and_loads() {
        assert_eq!(
            LORA_ICON_RGBA.len(),
            (LORA_ICON_SIZE * LORA_ICON_SIZE * 4) as usize
        );
        assert!(load_window_icon().is_some());
        let _handle = load_logo_handle();
    }
}
