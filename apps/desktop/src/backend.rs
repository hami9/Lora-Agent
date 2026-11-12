//! In-process native desktop adapter connecting the `iced` UI thread to `LoraApplicationService`.

use lora_core::{
    compute_broker_permit_digest, execute_workflow_job_cancellable, now_epoch_ms,
    verify_agent_run_outcome, verify_broker_receipt, verify_desktop_snapshot, AdmissionPlan,
    AgentLimits, AgentRunOutcome, ApprovalDecision, ApprovalPromptPort, ApprovalRequest,
    BrokerExecutionReceipt, BrokerSingleUsePermit, BrokerVerb, DesktopDashboardViewModel,
    DesktopScreen, DesktopSnapshot, ExecutionMode, HostSnapshot, LoraApplicationService, LoraError,
    ModelManifestV1, PrivilegedBrokerGate, TrustedConsentDialogViewModel, TuningProfile,
    WorkflowExecutionReceipt, WorkflowJobConfig, WorkflowStepKind,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

/// Snapshot of live core state polled/refreshed by the native desktop shell.
#[derive(Debug, Clone)]
pub struct CoreTelemetry {
    pub dashboard: DesktopDashboardViewModel,
    pub host: Option<HostSnapshot>,
    pub models: Vec<ModelManifestV1>,
    pub recent_audits: Vec<(i64, String, String, String)>,
    pub workflow_receipts: Vec<WorkflowExecutionReceipt>,
    pub workflow_integrity: (usize, usize),
    pub extension_count: usize,
    pub tuning_receipts_count: usize,
}

/// Completed local agent/chat run summary ready for rendering in the `#chat` stream.
#[derive(Debug, Clone)]
pub struct CompletedRunSummary {
    pub run_id: String,
    pub plan: AdmissionPlan,
    pub outcome: AgentRunOutcome,
    pub outcome_verified: bool,
}

/// Asynchronous events pushed from background worker threads back to the `iced` UI loop.
#[derive(Debug, Clone)]
pub enum AppWorkerEvent {
    ApprovalRequested {
        request_id: u64,
        view_model: TrustedConsentDialogViewModel,
    },
    RunDelta {
        run_id: String,
        delta: String,
    },
    RunFinished {
        run_id: String,
        result: Result<CompletedRunSummary, String>,
    },
    ModelImported {
        result: Result<String, String>,
    },
    ModelDeleted {
        result: Result<String, String>,
    },
    WorkflowFinished {
        result: Result<WorkflowExecutionReceipt, String>,
    },
    BrokerAttested {
        result: Result<(BrokerExecutionReceipt, bool), String>,
    },
    SnapshotCaptured {
        result: Result<(DesktopSnapshot, bool), String>,
    },
}

/// Fail-closed default approval prompt used in headless CLI diagnostic modes (`--dump-*`, etc.).
pub struct DenyByDefaultDesktopPrompt;

impl ApprovalPromptPort for DenyByDefaultDesktopPrompt {
    fn prompt_user(&self, _request: &ApprovalRequest) -> Result<ApprovalDecision, LoraError> {
        Ok(ApprovalDecision::Deny)
    }
}

/// Thread-safe `ApprovalPromptPort` bridged directly to the native `iced` Trusted Consent Modal.
///
/// The background inference thread sends a [`TrustedConsentDialogViewModel`] over an `mpsc` channel
/// and blocks waiting for the user's explicit modal choice. If the UI channel disconnects or fails,
/// it fails closed with [`ApprovalDecision::Deny`].
pub struct GuiApprovalPromptPort {
    next_id: AtomicU64,
    event_tx: mpsc::Sender<AppWorkerEvent>,
    pending: Mutex<HashMap<u64, mpsc::SyncSender<ApprovalDecision>>>,
}

impl GuiApprovalPromptPort {
    pub fn new(event_tx: mpsc::Sender<AppWorkerEvent>) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            event_tx,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve a pending consent prompt from the GUI thread.
    pub fn resolve(&self, request_id: u64, decision: ApprovalDecision) -> bool {
        let sender = self
            .pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&request_id));
        if let Some(tx) = sender {
            tx.send(decision).is_ok()
        } else {
            false
        }
    }

    /// Deny all remaining pending prompts (e.g. when Stop is pressed or closing).
    pub fn deny_all_pending(&self) {
        if let Ok(mut map) = self.pending.lock() {
            for (_, tx) in map.drain() {
                let _ = tx.send(ApprovalDecision::Deny);
            }
        }
    }
}

impl ApprovalPromptPort for GuiApprovalPromptPort {
    fn prompt_user(&self, request: &ApprovalRequest) -> Result<ApprovalDecision, LoraError> {
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let view_model = TrustedConsentDialogViewModel::from_approval_request(request);

        {
            let mut map = self
                .pending
                .lock()
                .map_err(|_| LoraError::PolicyDenied("approval map lock poisoned".to_string()))?;
            map.insert(request_id, reply_tx);
        }

        if self
            .event_tx
            .send(AppWorkerEvent::ApprovalRequested {
                request_id,
                view_model,
            })
            .is_err()
        {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&request_id);
            }
            return Ok(ApprovalDecision::Deny);
        }

        Ok(reply_rx.recv().unwrap_or(ApprovalDecision::Deny))
    }
}

/// Query fresh state directly from `LoraApplicationService` without HTTP serialization overhead.
pub fn collect_core_telemetry(
    service: &LoraApplicationService,
    experimental_enabled: bool,
) -> Result<CoreTelemetry, LoraError> {
    let dashboard = service.build_desktop_dashboard(experimental_enabled)?;
    let host = service.hw_probe.capture_snapshot(service.data_dir()).ok();
    let models = service.registry.list_models().unwrap_or_default();
    let recent_audits = service
        .store
        .list_audit_events(None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .take(16)
        .map(|a| (a.seq, a.outcome, a.resource_alias, a.summary))
        .collect();
    let workflow_receipts = service
        .store
        .list_workflow_receipts(Some(12))
        .unwrap_or_default();
    let workflow_integrity = service
        .store
        .verify_stored_workflow_receipts(Some(20))
        .unwrap_or((0, 0));
    let extension_count = service
        .store
        .load_extension_host()
        .map(|h| h.list().len())
        .unwrap_or(0);
    let tuning_receipts_count = service
        .store
        .list_tuning_receipts()
        .map(|t| t.len())
        .unwrap_or(0);

    Ok(CoreTelemetry {
        dashboard,
        host,
        models,
        recent_audits,
        workflow_receipts,
        workflow_integrity,
        extension_count,
        tuning_receipts_count,
    })
}

/// Spawn a non-blocking local model inference + optional RAG / scoped tool run on a dedicated OS thread.
#[allow(clippy::too_many_arguments)]
pub fn spawn_chat_run(
    service: Arc<LoraApplicationService>,
    event_tx: mpsc::Sender<AppWorkerEvent>,
    run_id: String,
    model_id_or_alias: String,
    requested_context: u32,
    profile: TuningProfile,
    mode: ExecutionMode,
    workspace_root: Option<PathBuf>,
    prompt: String,
    use_rag: bool,
    cancel_flag: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let tx_delta = event_tx.clone();
        let run_id_delta = run_id.clone();
        let limits = AgentLimits::default();
        let mut on_delta = |chunk: &str| -> Result<(), LoraError> {
            let _ = tx_delta.send(AppWorkerEvent::RunDelta {
                run_id: run_id_delta.clone(),
                delta: chunk.to_string(),
            });
            Ok(())
        };

        let res = service.run_rag_task(
            &run_id,
            "desktop-local-user",
            &model_id_or_alias,
            requested_context,
            profile,
            mode,
            workspace_root.as_deref(),
            &prompt,
            &limits,
            use_rag,
            &cancel_flag,
            &mut on_delta,
        );

        let mapped = match res {
            Ok((plan, outcome)) => {
                let outcome_verified = verify_agent_run_outcome(&outcome).is_ok();
                Ok(CompletedRunSummary {
                    run_id: run_id.clone(),
                    plan,
                    outcome,
                    outcome_verified,
                })
            }
            Err(e) => Err(format!("[{:?}] {}", e.reason_code(), e)),
        };

        let _ = event_tx.send(AppWorkerEvent::RunFinished {
            run_id,
            result: mapped,
        });
    });
}

/// Spawn local GGUF model import with SHA-256 verification and content-addressed staging.
pub fn spawn_model_import(
    service: Arc<LoraApplicationService>,
    event_tx: mpsc::Sender<AppWorkerEvent>,
    source_path: PathBuf,
    alias: Option<String>,
) {
    thread::spawn(move || {
        let res = service
            .import_model(&source_path, alias.as_deref(), None)
            .map(|out| {
                let sha = out
                    .manifest
                    .files
                    .first()
                    .map(|f| f.sha256.chars().take(12).collect::<String>())
                    .unwrap_or_default();
                format!(
                    "{:?}: '{}' (sha256:{}...)",
                    out.status, out.manifest.display_alias, sha
                )
            })
            .map_err(|e| format!("[{:?}] {}", e.reason_code(), e));
        let _ = event_tx.send(AppWorkerEvent::ModelImported { result: res });
    });
}

/// Retire a model revision and collect unreferenced content-addressed blobs off-thread.
pub fn spawn_model_delete(
    service: Arc<LoraApplicationService>,
    event_tx: mpsc::Sender<AppWorkerEvent>,
    id_or_alias: String,
) {
    thread::spawn(move || {
        let res = service
            .delete_model(&id_or_alias)
            .map(|d| {
                format!(
                    "Retired '{}' (gc={})",
                    d.display_alias, d.blob_garbage_collected
                )
            })
            .map_err(|e| format!("[{:?}] {}", e.reason_code(), e));
        let _ = event_tx.send(AppWorkerEvent::ModelDeleted { result: res });
    });
}

/// Execute and persist an air-gapped deterministic workflow pipeline with receipt attestation.
pub fn spawn_demo_workflow(
    service: Arc<LoraApplicationService>,
    event_tx: mpsc::Sender<AppWorkerEvent>,
    workflow_id: String,
    cancel_flag: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let cfg = WorkflowJobConfig {
            workflow_id,
            title: "Native Desktop Verification Pipeline".to_string(),
            steps: vec![
                WorkflowStepKind::DoctorProbe {
                    min_available_ram_mb: Some(256),
                },
                WorkflowStepKind::IndexWorkspace {
                    workspace_id: "desktop-ws".to_string(),
                    relative_path: "docs/DESKTOP_UX.md".to_string(),
                },
                WorkflowStepKind::PreviewPatch {
                    target_file: "notes/desktop-check.md".to_string(),
                    replacement: "Native pure-Rust desktop pipeline check".to_string(),
                },
            ],
            stop_on_failure: true,
            max_duration_ms: Some(15_000),
            max_step_retries: Some(1),
            non_fatal_step_indices: Some(vec![2]),
        };

        let res = execute_workflow_job_cancellable(&cfg, Some(&cancel_flag))
            .inspect(|receipt| {
                let _ = service.store.record_workflow_receipt(receipt);
            })
            .map_err(|e| format!("[{:?}] {}", e.reason_code(), e));

        let _ = event_tx.send(AppWorkerEvent::WorkflowFinished { result: res });
    });
}

/// Execute a single-use privileged broker permit and verify the returned cryptographic receipt.
pub fn spawn_broker_attestation(
    event_tx: mpsc::Sender<AppWorkerEvent>,
    service_name: String,
    restart: bool,
    session_id: String,
) {
    thread::spawn(move || {
        let nonce = format!("brk-{}", now_epoch_ms());
        let verb = if restart {
            BrokerVerb::RestartAllowlistedService {
                service_name: service_name.clone(),
            }
        } else {
            BrokerVerb::QueryServiceStatus {
                service_name: service_name.clone(),
            }
        };
        let now = now_epoch_ms();
        let digest = compute_broker_permit_digest(&session_id, &nonce, &verb);
        let permit = BrokerSingleUsePermit {
            permit_id: format!("perm-{now}"),
            session_id,
            challenge_nonce: nonce,
            verb,
            request_digest: digest,
            expires_at_ms: now.saturating_add(60_000),
        };
        let mut gate = PrivilegedBrokerGate::new(vec![
            "lora-local-service".to_string(),
            "lora-worker".to_string(),
            "lora-daemon".to_string(),
        ]);
        let res = gate
            .execute_and_attest_permit(&permit, now)
            .map(|receipt| {
                let verified = verify_broker_receipt(&receipt).is_ok();
                (receipt, verified)
            })
            .map_err(|e| format!("[{:?}] {}", e.reason_code(), e));
        let _ = event_tx.send(AppWorkerEvent::BrokerAttested { result: res });
    });
}

/// Capture and self-verify an attested [`DesktopSnapshot`].
pub fn spawn_desktop_snapshot(
    service: Arc<LoraApplicationService>,
    event_tx: mpsc::Sender<AppWorkerEvent>,
    screen: DesktopScreen,
    experimental_enabled: bool,
    session_id: String,
) {
    thread::spawn(move || {
        let res = service
            .capture_desktop_snapshot(screen, experimental_enabled, &session_id)
            .map(|snap| {
                let verified = verify_desktop_snapshot(&snap).is_ok();
                (snap, verified)
            })
            .map_err(|e| format!("[{:?}] {}", e.reason_code(), e));
        let _ = event_tx.send(AppWorkerEvent::SnapshotCaptured { result: res });
    });
}
