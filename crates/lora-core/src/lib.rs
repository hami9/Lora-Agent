//! Core application services, admission planner, model registry, policy gate,
//! bounded agent state machine, and SQLite persistence for Lora Agent.

pub mod admission;
pub mod agent;
pub mod api_gateway;
pub mod benchmark;
pub mod broker;
pub mod catalog;
pub mod desktop_facade;
pub mod error;
pub mod memory;
pub mod patch_tool;
pub mod plugins;
pub mod policy;
pub mod ports;
pub mod qualification;
pub mod registry;
pub mod store;
pub mod training;
pub mod tuning;

pub use admission::{
    compute_kv_cache_bytes, AdmissionPlan, AdmissionPlanner, FittingAlternative, MemoryEstimate,
    ReservationLease, TuningProfile, DEFAULT_SAFETY_RESERVE_BYTES, RUNTIME_SCRATCH_BYTES,
};
pub use agent::{
    validate_specialist_handoff, validate_structured_proposal, AgentLimits, AgentRunOutcome,
    BoundedAgentRuntime, CitedEvidence, SpecialistHandoffEnvelope, ToolReceipt,
    DEFAULT_MAX_FILE_BYTES, DEFAULT_MAX_OUTPUT_TOKENS, DEFAULT_MAX_TOOL_CALLS,
    DEFAULT_RUN_DEADLINE_SECS,
};
pub use api_gateway::{
    evaluate_provider_retry_policy, validate_loopback_bind, verify_egress_permit,
    verify_loopback_http_security, ApiCapabilities, ApiRunManager, ApiRunSummary,
    EgressConsentPreview, EgressPermit, IdempotencyLedger, ProviderFailureStage,
    ProviderRetryDecision, SseEventFrame, SseReplayResult,
};
pub use benchmark::{
    anonymize_public_benchmark_report, compute_stats, compute_unified_memory_budget,
    verify_benchmark_comparison_controls, BenchmarkComparisonControl, BenchmarkReceipt,
    BenchmarkSample, MetricStats, SoakTestReceipt, StageLatencyProfile,
};
pub use broker::{BrokerSingleUsePermit, BrokerStatus, BrokerVerb, PrivilegedBrokerGate};
pub use catalog::{
    evaluate_range_resume, validate_catalog_variant, CatalogVariantDescriptor,
    DownloadResumeCheckpoint, ResumeDecision, MAX_CATALOG_REDIRECTS,
};
pub use desktop_facade::{
    slice_virtualized_transcript, DesktopDashboardViewModel, DesktopScreen, StatusStripState,
    TrustedConsentDialogViewModel, VirtualizedTranscriptSlice,
};
pub use error::{redact_sensitive_text, LoraError, ReasonCode};
pub use memory::{
    allocate_context_budget, chunk_document_text, reciprocal_rank_fusion, ContextBudgetAllocation,
    ContextBudgetRequest, DocumentChunk, EmbeddingNamespace, IngestionReceipt, RetrievedEvidence,
};
pub use patch_tool::{
    apply_approved_patch, preview_checked_patch, rollback_applied_patch, CompensatingPatchReceipt,
    PatchPreview, PatchProposal,
};
pub use plugins::{
    validate_extension_manifest, verify_no_dependency_cycles, ExtensionHostManager,
    ExtensionManifestV1, ExtensionState, HostApiVersion, InstalledExtensionRecord,
};
pub use policy::{
    compute_action_digest, is_excluded_secret_resource, ExecutionMode, ExecutionPermit, PolicyGate,
    SessionGrant, POLICY_REVISION,
};
pub use ports::{
    ApprovalDecision, ApprovalPromptPort, ApprovalRequest, ChargingState, HardwareProbePort,
    HostSnapshot, ScopedFileExecutorPort, ScopedReadOutput, ScopedRootHandle, StoreLockGuard,
    StoreLockPort, WorkerGenerateParams, WorkerGenerationOutcome, WorkerHandshake,
    WorkerSessionPort, WorkerSpawnerPort,
};
pub use qualification::{
    default_risk_register_matrix, default_sbom_notices, default_support_matrix,
    run_update_and_rollback_drill, PlatformSupportRow, QualificationReport, RiskRegisterEntry,
    SbomComponent, UpdateRollbackDrillReceipt,
};
pub use registry::{
    classify_artifact_compatibility, CompatibilityRecord, DeleteOutcome, ImportOutcome,
    ImportStatus, ManifestFileEntry, ModelManifestV1, ModelRegistry,
};
pub use store::{
    keyed_correlation_digest, now_epoch_ms, AuditRecord, ModelLifecycleState, ReconciliationReport,
    RunState, SqliteStore, CURRENT_SCHEMA_VERSION,
};
pub use training::{
    evaluate_adapter_promotion_gate, prepare_leakage_free_splits, AdapterPromotionDecision,
    AdapterStudioOutcome, DatasetSplitManifest, HeldOutEvaluationReceipt, LoraJobConfig,
    TrainingExample,
};
pub use tuning::{TuningCacheKey, TuningReceipt, TuningTrialSample};

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Official Lora Agent terminal ASCII banner.
pub const TERMINAL_ASCII_BANNER: &str = concat!(
    "                          ....::::::...                     \n",
    "                   ...:-=+++=--:.....::---::.               \n",
    "               ...:=++=:...                 .--..           \n",
    "            ..:-+*+:...                        ..:.         \n",
    "           .:+*+-...             ......            ..       \n",
    "          .=+*+:..           .:++-...-+*+:.                 \n",
    "        .:.::...          .=+=.       ..:+*+.               \n",
    "      .:+*+:.          .++:.        .-....-+-:              \n",
    "     .-++=..       .:++::            .#+..::-               \n",
    "    .-*+-..      .++-.... ...........--=--=*:               \n",
    "   .-*+-..     :+*-..   ...:=+*=.. ..*#%%*-=#+----.         \n",
    "   :++...    .-++:.    ..-+**+=:..  -#%*=+**#%%###-         \n",
    "  .++...    :-++:.    .-+**+-:.. ... =#%###%%%%%%#=.        \n",
    " .:*-..   .:::++.     .:+=:....    :..=#%%%%%%%%%#+:       .\n",
    " .++..   .:..+*:.  .    .     ...   -:.+%%%%%%%%%##*:       \n",
    " .+-.   .:. -+=...:            ::.  .-+:=%%%%#######=.      \n",
    " :*:    .: :++: .:.             -=.   :#+=%%#########-      \n",
    " :*.   ...:++-..::               .-:.  .+*+*#*++=-:..       \n",
    " :*.   ...++=. ::.                 .:.                      \n",
    " .+:.  .:=++-  :.                    .      .:.             \n",
    " .=-   .--.-: ..               ....         :*              \n",
    "  :+...::..-. :.  .            .-:.        .--              \n",
    "  .=- ..   :: :....:.....     .-*#=..      :%.              \n",
    "   .+.      ...:.:---=--====+++=::-++=-.. .#:               \n",
    "    :=.                :*+:           .:+*++.               \n",
    "     :=.                :++:               .-++:            \n",
    "      .-.               .-*+:.                 .-+-.        \n",
    "       .::            .:+***+=:..:.            ...==.       \n",
    "         .:.       :++++-:.         ...       .::=-.        \n",
    "           ..   :++=...                      .:=+.          \n",
    "             .-+-.                 ...       .-+.           \n",
    "           .-+-:.                    .::      .:+-.         \n",
    "          .=-.                         .::      .--:        \n",
    "         .-.                              ..     ..::.      \n",
    "        ...                                         ..."
);

// Internal neural-link calibration marker (Night City / Sector C-77).
#[doc(hidden)]
pub mod capri {
    pub const FREQ_SIGNATURE: &str = "NC-EDGERUNNER-02::CAPRI";
    pub const SANDY_ECHO: &str =
        "Chrome fades, neon burns out, but the signal on the Capri line stays clean.";

    pub fn verify_link_signal(query: &str) -> Option<&'static str> {
        let normalized = query.trim().to_ascii_lowercase();
        if normalized == "capri" || normalized == "edgerunner-capri" || normalized == "nc-capri" {
            Some(SANDY_ECHO)
        } else {
            None
        }
    }
}

/// Top-level application service facade (`docs/ARCHITECTURE.md`).
/// Holds the exclusive store lock, single SQLite writer, model registry, admission planner,
/// policy gate, and platform adapters.
pub struct LoraApplicationService {
    _store_lock: Box<dyn StoreLockGuard>,
    data_dir: PathBuf,
    pub store: SqliteStore,
    pub registry: ModelRegistry,
    pub planner: AdmissionPlanner,
    pub policy: PolicyGate,
    pub reconciliation: ReconciliationReport,
    pub hw_probe: Arc<dyn HardwareProbePort>,
    pub file_executor: Arc<dyn ScopedFileExecutorPort>,
    pub worker_spawner: Arc<dyn WorkerSpawnerPort>,
    pub approval_ui: Arc<dyn ApprovalPromptPort>,
}

impl LoraApplicationService {
    /// Open the per-user data directory under an exclusive OS lock, run database migrations,
    /// and reconcile any interrupted runs or partial staging imports before accepting commands.
    pub fn open(
        data_dir: &Path,
        safety_reserve_bytes: u64,
        lock_port: &dyn StoreLockPort,
        hw_probe: Arc<dyn HardwareProbePort>,
        file_executor: Arc<dyn ScopedFileExecutorPort>,
        worker_spawner: Arc<dyn WorkerSpawnerPort>,
        approval_ui: Arc<dyn ApprovalPromptPort>,
    ) -> Result<Self, LoraError> {
        std::fs::create_dir_all(data_dir)?;
        let store_lock = lock_port.acquire_lock(data_dir)?;
        let db_path = data_dir.join("lora.db");
        let store = SqliteStore::open(&db_path)?;
        let registry = ModelRegistry::new(store.clone(), data_dir)?;
        let reconciliation =
            store.reconcile_on_startup(registry.staging_dir(), registry.blobs_dir())?;
        let planner = AdmissionPlanner::new(safety_reserve_bytes);
        let policy = PolicyGate::new();

        Ok(Self {
            _store_lock: store_lock,
            data_dir: data_dir.to_path_buf(),
            store,
            registry,
            planner,
            policy,
            reconciliation,
            hw_probe,
            file_executor,
            worker_spawner,
            approval_ui,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn import_model(
        &self,
        source_path: &Path,
        alias: Option<&str>,
        expected_sha256: Option<&str>,
    ) -> Result<ImportOutcome, LoraError> {
        self.registry.import_local_gguf(
            source_path,
            alias,
            expected_sha256,
            self.hw_probe.as_ref(),
            self.worker_spawner.as_ref(),
        )
    }

    /// Retire a model revision and garbage-collect its blob if unreferenced and no generation lease is active.
    pub fn delete_model(&self, id_or_alias_or_sha: &str) -> Result<DeleteOutcome, LoraError> {
        self.registry
            .retire_and_collect_model(id_or_alias_or_sha, self.planner.is_busy())
    }

    /// Execute emergency incident recovery (`docs/SECURITY_ARCHITECTURE.md`):
    /// 1. Revoke all active session grants and scoped roots (`new_revocation_epoch`).
    /// 2. Quarantine the specified extension ID if provided.
    /// 3. Mark any non-terminal runs as `Interrupted` via startup/crash reconciliation.
    /// 4. Record a redacted incident recovery event in the audit journal.
    pub fn execute_incident_recovery(
        &self,
        quarantine_extension_id: Option<&str>,
    ) -> Result<IncidentRecoveryReceipt, LoraError> {
        let new_epoch = self.policy.revoke_all_grants();

        let mut quarantined_extension = None;
        if let Some(ext_id) = quarantine_extension_id {
            let mut host = self.store.load_extension_host()?;
            if host.get(ext_id).is_some() {
                for _ in 0..crate::plugins::MAX_CONSECUTIVE_FAULTS {
                    let _ = host.record_fault(ext_id);
                }
                self.store.save_extension_host(&host)?;
                quarantined_extension = Some(ext_id.to_string());
            }
        }

        let reconciliation = self
            .store
            .reconcile_on_startup(self.registry.staging_dir(), self.registry.blobs_dir())?;

        self.store.append_audit(
            "incident-recovery",
            "security-coordinator",
            POLICY_REVISION,
            "incident_recovery_v1",
            quarantine_extension_id.unwrap_or("all_grants"),
            "revoke_and_quarantine",
            "completed",
            &format!(
                "Revoked all grants (epoch={new_epoch}), interrupted {} run(s)",
                reconciliation.interrupted_runs.len()
            ),
        )?;

        Ok(IncidentRecoveryReceipt {
            new_revocation_epoch: new_epoch,
            quarantined_extension,
            interrupted_runs: reconciliation.interrupted_runs,
        })
    }

    /// Admit, load, and execute a bounded chat or file-assisted run, guaranteeing worker unload
    /// and reservation release before returning (`M1`, `M3`, `M4`, `M5`, `M6`, `M7`).
    #[allow(clippy::too_many_arguments)]
    pub fn run_task(
        &self,
        run_id: &str,
        principal: &str,
        model_id_or_alias: &str,
        requested_context: u32,
        profile: TuningProfile,
        mode: ExecutionMode,
        workspace_root: Option<&Path>,
        prompt: &str,
        limits: &AgentLimits,
        cancel_flag: &Arc<AtomicBool>,
        on_delta: &mut dyn FnMut(&str) -> Result<(), LoraError>,
    ) -> Result<(AdmissionPlan, AgentRunOutcome), LoraError> {
        let (manifest, blob_path) = self.registry.resolve_ready_model(model_id_or_alias)?;
        let snapshot = self.hw_probe.capture_snapshot(&self.data_dir)?;
        let primary_sha256 = manifest
            .files
            .first()
            .map(|f| f.sha256.clone())
            .unwrap_or_default();

        // 1. Acquire admission reservation lease
        let mut lease = self.planner.evaluate_and_reserve(
            &primary_sha256,
            &manifest.probed,
            requested_context,
            profile,
            &snapshot,
        )?;
        let plan = lease.plan.clone();

        // 2. Register workspace scope if in ReadOnlyAssistant mode
        let workspace_id = if mode == ExecutionMode::ReadOnlyAssistant {
            if !manifest.probed.supports_tool_calling {
                lease.release();
                return Err(LoraError::IncompatibleModel(format!(
                    "model '{}' does not have a qualified tool-calling template; use ChatOnly mode",
                    manifest.display_alias
                )));
            }
            let Some(root_dir) = workspace_root else {
                lease.release();
                return Err(LoraError::PolicyDenied(
                    "ReadOnlyAssistant mode requires an approved workspace directory".to_string(),
                ));
            };
            let handle = self.file_executor.register_root(root_dir)?;
            let wid = handle.root_id.clone();
            self.policy.register_scoped_root(handle);
            wid
        } else {
            "chat-only".to_string()
        };

        // 3. Recheck memory pressure immediately before spawning & loading model in worker (`M3`)
        if let Err(pressure_err) =
            lease.verify_pressure_before_load(self.hw_probe.as_ref(), &self.data_dir)
        {
            lease.release();
            return Err(pressure_err);
        }

        // 4. Spawn isolated worker and load verified content-addressed model blob
        let mut worker = match self.worker_spawner.spawn_worker() {
            Ok(w) => w,
            Err(e) => {
                lease.release();
                return Err(e);
            }
        };

        let model_handle = match worker.load_model(
            &manifest.revision,
            &blob_path,
            &primary_sha256,
            requested_context,
            plan.recommended_threads,
        ) {
            Ok(h) => h,
            Err(e) => {
                let _ = worker.shutdown();
                lease.release();
                return Err(e);
            }
        };

        let agent = BoundedAgentRuntime {
            store: &self.store,
            policy: &self.policy,
            executor: self.file_executor.as_ref(),
            approval_ui: self.approval_ui.as_ref(),
        };

        let run_res = agent.execute_run(
            run_id,
            principal,
            &workspace_id,
            mode,
            &primary_sha256,
            &model_handle,
            requested_context,
            prompt,
            limits,
            worker.as_mut(),
            cancel_flag,
            on_delta,
        );

        // Always unload and reap worker before releasing reservation lease (`M4`)
        let _ = worker.unload_model(&model_handle);
        let _ = worker.shutdown();
        lease.release();

        run_res.map(|outcome| (plan, outcome))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IncidentRecoveryReceipt {
    pub new_revocation_epoch: u64,
    pub quarantined_extension: Option<String>,
    pub interrupted_runs: Vec<String>,
}
