//! Narrow ports and domain value types connecting `lora-core` to OS, hardware, worker, and UI adapters.

use crate::error::LoraError;
use lora_protocol::{
    FinishReason, GenerationUsage, ProbedModelMetadata, ToolProposalWire, WorkerCapabilities,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Exclusive OS-owned store lock guard released automatically on drop or process exit.
pub trait StoreLockGuard: Send + Sync {}

/// Port for acquiring an exclusive per-user-data-directory application lock.
pub trait StoreLockPort: Send + Sync {
    fn acquire_lock(&self, data_dir: &Path) -> Result<Box<dyn StoreLockGuard>, LoraError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargingState {
    AcOnline,
    OnBattery,
    Unknown,
}

/// Fresh host hardware and OS budget observation (`docs/HARDWARE_INTELLIGENCE.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSnapshot {
    pub os_name: String,
    pub cpu_arch: String,
    pub cpu_brand: String,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub cpu_isa_features: Vec<String>,
    pub total_ram_bytes: u64,
    pub available_ram_bytes: u64,
    pub free_disk_bytes: u64,
    pub charging_state: ChargingState,
    pub sample_timestamp_ms: u64,
    pub unavailable_sensors: Vec<String>,
}

/// Port for querying host memory, CPU topology, disk space, and sensor availability.
pub trait HardwareProbePort: Send + Sync {
    fn capture_snapshot(&self, storage_dir: &Path) -> Result<HostSnapshot, LoraError>;
}

/// Handle representing a validated, user-selected directory scope (`docs/OS_INTEGRATION.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedRootHandle {
    pub root_id: String,
    pub canonical_root: PathBuf,
    pub device_or_volume_id: u64,
}

/// Output from a bounded, handle-verified read-only file operation (`docs/AGENT_RUNTIME.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedReadOutput {
    pub root_id: String,
    pub relative_path: String,
    pub resource_identity: String,
    pub content: String,
    pub byte_start: u64,
    pub byte_end: u64,
    /// Inclusive 1-based line range (`None` when the file is empty).
    pub line_range: Option<(u32, u32)>,
    pub sha256_digest: String,
    pub truncated: bool,
}

/// Port for executing handle-anchored, reparse/symlink-rejecting read-only file operations.
pub trait ScopedFileExecutorPort: Send + Sync {
    fn register_root(&self, root_dir: &Path) -> Result<ScopedRootHandle, LoraError>;

    fn read_scoped_file(
        &self,
        root: &ScopedRootHandle,
        relative_path: &str,
        max_bytes: usize,
        recheck_grant: &dyn Fn() -> Result<(), LoraError>,
    ) -> Result<ScopedReadOutput, LoraError>;
}

/// Request presented to a trusted human approval surface before executing a scoped read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub run_id: String,
    pub proposal_id: String,
    pub tool_id: String,
    pub root_id: String,
    pub root_display_path: String,
    pub relative_path: String,
    pub max_bytes: usize,
    pub purpose: String,
    pub approval_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    ApproveOnce,
    ApproveSessionScope,
    Deny,
}

/// Trusted consent prompt port. Never callable by model output.
pub trait ApprovalPromptPort: Send + Sync {
    fn prompt_user(&self, request: &ApprovalRequest) -> Result<ApprovalDecision, LoraError>;
}

/// Handshake and build metadata returned by a verified worker process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerHandshake {
    pub engine_name: String,
    pub engine_revision: String,
    pub build_options: Vec<String>,
    pub capabilities: WorkerCapabilities,
}

/// Stream outcome returned by a worker generation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerGenerationOutcome {
    pub full_text: String,
    pub finish_reason: FinishReason,
    pub usage: GenerationUsage,
    pub tool_proposal: Option<ToolProposalWire>,
}

/// Request parameters for a single worker generation call.
#[derive(Debug, Clone)]
pub struct WorkerGenerateParams {
    pub request_id: String,
    pub session_id: String,
    pub model_handle: String,
    pub prompt: String,
    pub max_tokens: u32,
    pub context_length: u32,
    pub seed: u64,
    pub tool_calling_enabled: bool,
    pub simulated_delay_ms: u64,
}

/// Active supervised worker session over inherited pipes.
pub trait WorkerSessionPort: Send {
    fn handshake(&self) -> &WorkerHandshake;

    fn probe_artifact(
        &mut self,
        artifact_id: &str,
        artifact_path: &Path,
        expected_sha256: &str,
    ) -> Result<ProbedModelMetadata, LoraError>;

    fn load_model(
        &mut self,
        artifact_id: &str,
        artifact_path: &Path,
        expected_sha256: &str,
        context_length: u32,
        threads: u32,
    ) -> Result<String, LoraError>;

    fn generate_stream(
        &mut self,
        params: &WorkerGenerateParams,
        cancel_flag: &Arc<AtomicBool>,
        on_delta: &mut dyn FnMut(&str) -> Result<(), LoraError>,
    ) -> Result<WorkerGenerationOutcome, LoraError>;

    fn unload_model(&mut self, model_handle: &str) -> Result<(), LoraError>;

    fn shutdown(&mut self) -> Result<(), LoraError>;
}

/// Factory port for spawning a fresh, isolated worker session.
pub trait WorkerSpawnerPort: Send + Sync {
    fn spawn_worker(&self) -> Result<Box<dyn WorkerSessionPort>, LoraError>;
}
