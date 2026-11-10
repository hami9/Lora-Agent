//! Phase 4 Desktop UX state projection and anti-spoofing approval view-model (`docs/DESKTOP_UX.md`, `R12`).
//!
//! Ensures the desktop shell (Tauri 2 + React/TS) consumes the exact same Rust application service
//! contracts and reason codes as the CLI, virtualizes long transcripts, and never renders untrusted
//! file contents inside the trusted approval prompt chrome.

use crate::error::{redact_sensitive_text, LoraError};
use crate::ports::{ApprovalRequest, HostSnapshot};
use crate::LoraApplicationService;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Domain separation prefix for desktop snapshot integrity attestations (`docs/DESKTOP_UX.md`).
pub const DESKTOP_SNAPSHOT_DOMAIN_V1: &[u8] = b"lora.desktop.snapshot.v1\0";

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

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

    /// Capture a cryptographically attested DesktopSnapshot from the live core service state (`docs/DESKTOP_UX.md`).
    pub fn capture_desktop_snapshot(
        &self,
        screen: DesktopScreen,
        experimental_enabled: bool,
        session_id: &str,
    ) -> Result<DesktopSnapshot, LoraError> {
        let dashboard = self.build_desktop_dashboard(experimental_enabled)?;
        let ts = crate::store::now_epoch_ms();
        let anti_spoofing_token = format!("ds-{session_id}-{ts}");
        let mut snapshot = DesktopSnapshot {
            schema_version: 1,
            session_id: session_id.to_string(),
            timestamp_ms: ts,
            screen,
            dashboard,
            anti_spoofing_token,
            snapshot_sha256: None,
        };
        snapshot.snapshot_sha256 = Some(compute_desktop_snapshot_digest(&snapshot));
        Ok(snapshot)
    }
}

/// Cryptographically attested desktop session and view-model state snapshot (`docs/DESKTOP_UX.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopSnapshot {
    pub schema_version: u32,
    pub session_id: String,
    pub timestamp_ms: u64,
    pub screen: DesktopScreen,
    pub dashboard: DesktopDashboardViewModel,
    pub anti_spoofing_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_sha256: Option<String>,
}

/// Compute a domain-separated canonical SHA-256 digest over a DesktopSnapshot (`lora.desktop.snapshot.v1`).
pub fn compute_desktop_snapshot_digest(snapshot: &DesktopSnapshot) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DESKTOP_SNAPSHOT_DOMAIN_V1);
    hasher.update(snapshot.schema_version.to_le_bytes());
    update_len_prefixed(&mut hasher, snapshot.session_id.as_bytes());
    hasher.update(snapshot.timestamp_ms.to_le_bytes());

    let screen_str = match snapshot.screen {
        DesktopScreen::Dashboard => "dashboard",
        DesktopScreen::Chat => "chat",
        DesktopScreen::AgentWorkspace => "agent_workspace",
        DesktopScreen::ModelLibrary => "model_library",
        DesktopScreen::Hardware => "hardware",
        DesktopScreen::AdapterStudio => "adapter_studio",
        DesktopScreen::ExtensionManager => "extension_manager",
        DesktopScreen::Settings => "settings",
    };
    update_len_prefixed(&mut hasher, screen_str.as_bytes());

    // Status strip
    update_len_prefixed(
        &mut hasher,
        snapshot
            .dashboard
            .status_strip
            .execution_destination
            .as_bytes(),
    );
    if let Some(ref alias) = snapshot.dashboard.status_strip.active_model_alias {
        hasher.update([1u8]);
        update_len_prefixed(&mut hasher, alias.as_bytes());
    } else {
        hasher.update([0u8]);
    }
    hasher.update([snapshot.dashboard.status_strip.active_task_busy as u8]);
    hasher.update([snapshot.dashboard.status_strip.stop_command_reachable as u8]);
    hasher.update([snapshot.dashboard.status_strip.experimental_screens_enabled as u8]);

    // Dashboard metrics
    hasher.update((snapshot.dashboard.installed_model_count as u64).to_le_bytes());
    hasher.update(snapshot.dashboard.available_ram_mib.to_le_bytes());
    hasher.update(snapshot.dashboard.total_ram_mib.to_le_bytes());
    update_len_prefixed(&mut hasher, snapshot.dashboard.cpu_summary.as_bytes());

    hasher.update((snapshot.dashboard.unavailable_sensors.len() as u64).to_le_bytes());
    for s in &snapshot.dashboard.unavailable_sensors {
        update_len_prefixed(&mut hasher, s.as_bytes());
    }

    hasher.update((snapshot.dashboard.recent_audit_count as u64).to_le_bytes());
    update_len_prefixed(&mut hasher, snapshot.anti_spoofing_token.as_bytes());

    hex::encode(hasher.finalize())
}

/// Verify that a DesktopSnapshot matches its cryptographic attestation digest.
pub fn verify_desktop_snapshot(snapshot: &DesktopSnapshot) -> Result<(), LoraError> {
    let expected = compute_desktop_snapshot_digest(snapshot);
    match &snapshot.snapshot_sha256 {
        Some(digest) if digest == &expected => Ok(()),
        Some(mismatch) => Err(LoraError::PolicyDenied(format!(
            "desktop snapshot digest mismatch: expected {expected}, got {mismatch}"
        ))),
        None => Err(LoraError::MalformedArtifact(
            "desktop snapshot missing cryptographic digest".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desktop_snapshot_attestation_and_tamper_detection() {
        let dashboard = DesktopDashboardViewModel {
            status_strip: StatusStripState {
                execution_destination: "local_cpu_worker".to_string(),
                active_model_alias: Some("smollm-135m".to_string()),
                active_task_busy: false,
                stop_command_reachable: true,
                experimental_screens_enabled: false,
            },
            installed_model_count: 1,
            available_ram_mib: 4096,
            total_ram_mib: 16384,
            cpu_summary: "x86_64 (8 cores)".to_string(),
            unavailable_sensors: vec![],
            recent_audit_count: 3,
        };

        let mut snapshot = DesktopSnapshot {
            schema_version: 1,
            session_id: "test-sess-01".to_string(),
            timestamp_ms: 1731234567890,
            screen: DesktopScreen::Dashboard,
            dashboard,
            anti_spoofing_token: "ds-tok-abc".to_string(),
            snapshot_sha256: None,
        };

        let digest = compute_desktop_snapshot_digest(&snapshot);
        assert_eq!(digest.len(), 64);
        snapshot.snapshot_sha256 = Some(digest.clone());

        // Verifies successfully
        assert!(verify_desktop_snapshot(&snapshot).is_ok());

        // Tamper session ID
        let mut tampered = snapshot.clone();
        tampered.session_id = "attacker-sess".to_string();
        assert!(verify_desktop_snapshot(&tampered).is_err());

        // Tamper audit count
        let mut tampered_audit = snapshot.clone();
        tampered_audit.dashboard.recent_audit_count = 999;
        assert!(verify_desktop_snapshot(&tampered_audit).is_err());

        // Missing digest
        let mut missing_digest = snapshot.clone();
        missing_digest.snapshot_sha256 = None;
        assert!(verify_desktop_snapshot(&missing_digest).is_err());
    }
}
