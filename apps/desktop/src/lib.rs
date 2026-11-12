//! Pure-Rust native Discord-style desktop client and headless diagnostics (`docs/DESKTOP_UX.md`, `ADR-0004`).

pub mod app;
pub mod backend;
pub mod theme;

use lora_core::{ApprovalRequest, TrustedConsentDialogViewModel};
use std::path::Path;

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
