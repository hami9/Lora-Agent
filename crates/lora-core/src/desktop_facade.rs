//! Phase 4 Desktop UX state projection and anti-spoofing approval view-model (`docs/DESKTOP_UX.md`, `R12`).
//!
//! Ensures the desktop shell (Tauri 2 + React/TS) consumes the exact same Rust application service
//! contracts and reason codes as the CLI, virtualizes long transcripts, and never renders untrusted
//! file contents inside the trusted approval prompt chrome.

use crate::error::redact_sensitive_text;
use crate::ports::{ApprovalRequest, HostSnapshot};
use crate::LoraApplicationService;
use serde::{Deserialize, Serialize};

/// Sidebar navigation screens defined in `docs/DESKTOP_UX.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopScreen {
    Dashboard,
    Chat,
    AgentWorkspace,
    ModelLibrary,
    Hardware,
    AdapterStudio,
    ExtensionManager,
    Settings,
}

/// Persistent status strip state displayed across all screens (`docs/DESKTOP_UX.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusStripState {
    pub execution_destination: String,
    pub active_model_alias: Option<String>,
    pub active_task_busy: bool,
    pub stop_command_reachable: bool,
    pub experimental_screens_enabled: bool,
}

/// Trusted approval modal view-model (`docs/DESKTOP_UX.md`):
/// "A trusted consent panel shows executable tool identity, relative resource, read limit,
/// model destination and lifetime. File text never renders inside the trusted prompt chrome."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedConsentDialogViewModel {
    pub run_id: String,
    pub tool_id: String,
    pub scope_root_id: String,
    pub target_relative_path: String,
    pub read_byte_limit: usize,
    pub destination_label: String,
    pub grant_lifetime_label: String,
    pub action_digest: String,
    pub default_button_focus: String,
}

impl TrustedConsentDialogViewModel {
    pub fn from_approval_request(req: &ApprovalRequest) -> Self {
        let escape_html = |s: &str| {
            redact_sensitive_text(s)
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        Self {
            run_id: escape_html(&req.run_id),
            tool_id: escape_html(&req.tool_id),
            scope_root_id: escape_html(&req.root_id),
            target_relative_path: escape_html(&req.relative_path),
            read_byte_limit: req.max_bytes,
            destination_label: "local_isolated_worker (no external transmission)".to_string(),
            grant_lifetime_label: "one-time or current session scope".to_string(),
            action_digest: req.approval_digest.clone(),
            // Accessibility (`docs/DESKTOP_UX.md`): "Dialogs trap and restore focus, default to the least-privileged choice"
            default_button_focus: "deny".to_string(),
        }
    }
}

/// Virtualized transcript window slice to keep DOM node count bounded on low-end machines (`R12`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualizedTranscriptSlice<T> {
    pub total_items: usize,
    pub start_index: usize,
    pub end_index: usize,
    pub visible_items: Vec<T>,
}

pub fn slice_virtualized_transcript<T: Clone>(
    items: &[T],
    scroll_offset: usize,
    viewport_capacity: usize,
) -> VirtualizedTranscriptSlice<T> {
    let total = items.len();
    let cap = viewport_capacity.max(1);
    let start = scroll_offset.min(total);
    let end = (start + cap).min(total);
    VirtualizedTranscriptSlice {
        total_items: total,
        start_index: start,
        end_index: end,
        visible_items: items[start..end].to_vec(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopDashboardViewModel {
    pub status_strip: StatusStripState,
    pub installed_model_count: usize,
    pub available_ram_mib: u64,
    pub total_ram_mib: u64,
    pub cpu_summary: String,
    pub unavailable_sensors: Vec<String>,
    pub recent_audit_count: usize,
}

impl LoraApplicationService {
    /// Build the Desktop Dashboard view-model from the live core service state (`docs/DESKTOP_UX.md`).
    pub fn build_desktop_dashboard(
        &self,
        experimental_enabled: bool,
    ) -> Result<DesktopDashboardViewModel, crate::error::LoraError> {
        let models = self.registry.list_models()?;
        let snap: HostSnapshot = self.hw_probe.capture_snapshot(&self.data_dir)?;
        let audits = self.store.list_audit_events(None)?;

        let active_model_alias = models.first().map(|m| m.display_alias.clone());
        let is_busy = self.planner.is_busy();

        Ok(DesktopDashboardViewModel {
            status_strip: StatusStripState {
                execution_destination: "local_cpu_worker".to_string(),
                active_model_alias,
                active_task_busy: is_busy,
                stop_command_reachable: true,
                experimental_screens_enabled: experimental_enabled,
            },
            installed_model_count: models.len(),
            available_ram_mib: snap.available_ram_bytes / (1024 * 1024),
            total_ram_mib: snap.total_ram_bytes / (1024 * 1024),
            cpu_summary: format!("{} ({} cores)", snap.cpu_brand, snap.physical_cores),
            unavailable_sensors: snap.unavailable_sensors,
            recent_audit_count: audits.len(),
        })
    }
}
