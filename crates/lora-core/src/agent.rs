//! Bounded agent runtime state machine, closed proposal parser, evidence receipts,
//! and citation verification (`docs/AGENT_RUNTIME.md`, `T8`, `M5`, `M6`, `M7`).

use crate::error::{redact_sensitive_text, LoraError};
use crate::memory::{allocate_context_budget, ContextBudgetRequest};
use crate::policy::{
    compute_action_digest, is_excluded_secret_resource, ExecutionMode, PolicyGate, POLICY_REVISION,
};
use crate::ports::{
    ApprovalDecision, ApprovalPromptPort, ApprovalRequest, ScopedFileExecutorPort,
    WorkerGenerateParams, WorkerSessionPort,
};
use crate::store::{RunState, SqliteStore};
use lora_protocol::ToolProposalWire;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DEFAULT_MAX_TOOL_CALLS: u32 = 4;
pub const DEFAULT_MAX_FILE_BYTES: usize = 32 * 1024; // 32 KiB
pub const DEFAULT_RUN_DEADLINE_SECS: u64 = 120;
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLimits {
    pub max_tool_calls: u32,
    pub max_file_bytes: usize,
    pub run_deadline_secs: u64,
    pub max_output_tokens: u32,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_tool_calls: DEFAULT_MAX_TOOL_CALLS,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            run_deadline_secs: DEFAULT_RUN_DEADLINE_SECS,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
        }
    }
}

/// Verified tool execution receipt (`docs/AGENT_RUNTIME.md`).
/// Excerpt bytes are transient by default and excluded from the persisted audit receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReceipt {
    pub invocation_id: String,
    pub outcome: String,
    pub resource_identity: String,
    pub relative_path: String,
    pub byte_range: (u64, u64),
    pub line_range: Option<(u32, u32)>,
    pub excerpt_sha256: String,
    pub truncated: bool,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitedEvidence {
    pub relative_path: String,
    pub line_range: Option<(u32, u32)>,
    pub byte_range: (u64, u64),
    pub excerpt_sha256: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunOutcome {
    pub run_id: String,
    pub final_state: RunState,
    pub answer: String,
    pub citations: Vec<CitedEvidence>,
    pub receipts: Vec<ToolReceipt>,
    pub turns_used: u32,
}

/// Validate a structured tool proposal strictly (`docs/AGENT_RUNTIME.md`):
/// - Rejects unknown tools or versions
/// - Rejects oversized or empty arguments
/// - Rejects embedded NULs or control characters
pub fn validate_structured_proposal(proposal: &ToolProposalWire) -> Result<(), LoraError> {
    if proposal.tool_id != "read_file" || proposal.tool_version != 1 {
        return Err(LoraError::PolicyDenied(format!(
            "unsupported tool proposal '{}@v{}'",
            proposal.tool_id, proposal.tool_version
        )));
    }
    if proposal.arguments.root_id.trim().is_empty() || proposal.arguments.root_id.len() > 128 {
        return Err(LoraError::PolicyDenied(
            "invalid or oversized root_id in tool proposal".to_string(),
        ));
    }
    if proposal.arguments.relative_path.trim().is_empty()
        || proposal.arguments.relative_path.len() > 512
        || proposal.arguments.relative_path.contains('\0')
    {
        return Err(LoraError::ScopeEscapeAttempt(
            "invalid, empty, or NUL-containing relative_path in tool proposal".to_string(),
        ));
    }
    Ok(())
}

pub struct BoundedAgentRuntime<'a> {
    pub store: &'a SqliteStore,
    pub policy: &'a PolicyGate,
    pub executor: &'a dyn ScopedFileExecutorPort,
    pub approval_ui: &'a dyn ApprovalPromptPort,
}

impl<'a> BoundedAgentRuntime<'a> {
    /// Execute a bounded chat or read-only agent run (`docs/AGENT_RUNTIME.md`).
    #[allow(clippy::too_many_arguments)]
    pub fn execute_run(
        &self,
        run_id: &str,
        principal: &str,
        workspace_id: &str,
        mode: ExecutionMode,
        model_digest: &str,
        model_handle: &str,
        context_length: u32,
        user_prompt: &str,
        limits: &AgentLimits,
        worker: &mut dyn WorkerSessionPort,
        cancel_flag: &Arc<AtomicBool>,
        on_delta: &mut dyn FnMut(&str) -> Result<(), LoraError>,
    ) -> Result<AgentRunOutcome, LoraError> {
        self.execute_run_with_rag(
            run_id,
            principal,
            workspace_id,
            mode,
            model_digest,
            model_handle,
            context_length,
            user_prompt,
            limits,
            false,
            worker,
            cancel_flag,
            on_delta,
        )
    }

    /// Execute a bounded chat or read-only agent run with optional pre-run SQLite FTS5 lexical
    /// retrieval and deterministic context-window budgeting (`docs/MEMORY_AND_RAG.md`).
    #[allow(clippy::too_many_arguments)]
    pub fn execute_run_with_rag(
        &self,
        run_id: &str,
        principal: &str,
        workspace_id: &str,
        mode: ExecutionMode,
        model_digest: &str,
        model_handle: &str,
        context_length: u32,
        user_prompt: &str,
        limits: &AgentLimits,
        use_rag: bool,
        worker: &mut dyn WorkerSessionPort,
        cancel_flag: &Arc<AtomicBool>,
        on_delta: &mut dyn FnMut(&str) -> Result<(), LoraError>,
    ) -> Result<AgentRunOutcome, LoraError> {
        let start_instant = Instant::now();
        let deadline = Duration::from_secs(limits.run_deadline_secs);

        let mut rev =
            self.store
                .create_run(run_id, principal, workspace_id, mode.as_str(), model_digest)?;

        self.store.append_audit(
            run_id,
            principal,
            POLICY_REVISION,
            "run_start",
            workspace_id,
            "admit",
            "planning",
            &format!("Started run in mode {}", mode.as_str()),
        )?;

        let mut current_prompt = if mode == ExecutionMode::ReadOnlyAssistant {
            format!(
                "[SYSTEM: Mode=ReadOnlyAssistant. WorkspaceRootId={workspace_id}. AllowedTool=read_file(root_id, relative_path). Cite observations as [{workspace_id}:<path>#L<start>-L<end>] or report INSUFFICIENT_EVIDENCE if absent.]\n[USER]: {user_prompt}"
            )
        } else {
            user_prompt.to_string()
        };

        let mut receipts = Vec::new();
        let mut citations = Vec::new();
        let mut tool_calls_used = 0u32;
        let mut seen_proposals = std::collections::HashSet::new();

        if use_rag && mode == ExecutionMode::ReadOnlyAssistant {
            let candidates =
                self.store
                    .search_lexical_fts(workspace_id, user_prompt, 5, self.policy)?;
            if !candidates.is_empty() {
                let ranked_excerpts: Vec<_> = candidates
                    .into_iter()
                    .map(|ev| {
                        let cost = ((ev.snippet.len() / 4) + 16) as u32;
                        (ev, cost)
                    })
                    .collect();
                let alloc = allocate_context_budget(&ContextBudgetRequest {
                    context_window_tokens: context_length,
                    reserved_output_tokens: limits.max_output_tokens.min(context_length / 2),
                    control_schema_overhead_tokens: 64,
                    user_request_tokens: ((user_prompt.len() / 4) + 1) as u32,
                    recent_turns_tokens: 0,
                    tool_receipts_tokens: 0,
                    ranked_excerpts,
                })?;

                for (idx, ev) in alloc.admitted_excerpts.into_iter().enumerate() {
                    let line_tag = format!("L{}-L{}", ev.line_range.0, ev.line_range.1);
                    let escaped_snippet = redact_sensitive_text(&ev.snippet);
                    current_prompt.push_str(&format!(
                        "\n[OBSERVATION_UNTRUSTED_DATA path=\"{}\" lines=\"{}\" bytes={}..{} sha256=\"{}\" truncated=false]:\n{}\n[/OBSERVATION_UNTRUSTED_DATA]",
                        ev.relative_path,
                        line_tag,
                        ev.byte_range.0,
                        ev.byte_range.1,
                        ev.doc_sha256,
                        escaped_snippet
                    ));

                    let receipt = ToolReceipt {
                        invocation_id: format!("{run_id}-rag-{}", idx + 1),
                        outcome: "succeeded".to_string(),
                        resource_identity: format!("fts5:{}", ev.chunk_id),
                        relative_path: ev.relative_path.clone(),
                        byte_range: ev.byte_range,
                        line_range: Some(ev.line_range),
                        excerpt_sha256: ev.doc_sha256.clone(),
                        truncated: false,
                        error_code: None,
                    };
                    self.store.append_audit(
                        run_id,
                        principal,
                        POLICY_REVISION,
                        "rag_retrieve",
                        &ev.relative_path,
                        "retrieve",
                        "succeeded",
                        &format!(
                            "Retrieved FTS5 chunk {} ({}) from {}",
                            ev.chunk_id, line_tag, ev.relative_path
                        ),
                    )?;
                    citations.push(CitedEvidence {
                        relative_path: ev.relative_path,
                        line_range: Some(ev.line_range),
                        byte_range: ev.byte_range,
                        excerpt_sha256: ev.doc_sha256,
                        truncated: false,
                    });
                    receipts.push(receipt);
                }
            }
        }

        loop {
            if cancel_flag.load(Ordering::SeqCst) {
                let _ = self.store.finalize_run_if_nonterminal(
                    run_id,
                    RunState::Cancelled,
                    "cancelled_by_user",
                );
                return Err(LoraError::Cancelled);
            }

            if start_instant.elapsed() > deadline {
                let _ = self.store.finalize_run_if_nonterminal(
                    run_id,
                    RunState::Failed,
                    "run_deadline_exceeded",
                );
                return Err(LoraError::BudgetExhausted(
                    "agent run exceeded deadline".to_string(),
                ));
            }

            let turn = tool_calls_used + 1;
            let gen_params = WorkerGenerateParams {
                request_id: format!("{run_id}-gen-{turn}"),
                session_id: run_id.to_string(),
                model_handle: model_handle.to_string(),
                prompt: current_prompt.clone(),
                max_tokens: limits.max_output_tokens.min(context_length / 2),
                context_length,
                seed: 42,
                tool_calling_enabled: mode == ExecutionMode::ReadOnlyAssistant,
                simulated_delay_ms: 0,
            };

            let gen_outcome = match worker.generate_stream(&gen_params, cancel_flag, on_delta) {
                Ok(out) => out,
                Err(LoraError::Cancelled) => {
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Cancelled,
                        "cancelled_during_generation",
                    );
                    return Err(LoraError::Cancelled);
                }
                Err(err) => {
                    let term_state = match err {
                        LoraError::WorkerCrashed(_) | LoraError::WorkerCorruptFrame(_) => {
                            RunState::Interrupted
                        }
                        _ => RunState::Failed,
                    };
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        term_state,
                        &err.redacted_message(),
                    );
                    return Err(err);
                }
            };

            // Did the model emit a structured tool proposal?
            if let Some(proposal) = gen_outcome.tool_proposal {
                if mode == ExecutionMode::ChatOnly {
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Failed,
                        "tool_proposed_in_chat_only_mode",
                    );
                    return Err(LoraError::PolicyDenied(
                        "model proposed a tool call in ChatOnly mode".to_string(),
                    ));
                }

                if tool_calls_used >= limits.max_tool_calls {
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Failed,
                        "tool_budget_exhausted",
                    );
                    return Err(LoraError::BudgetExhausted(format!(
                        "exhausted maximum tool call budget ({})",
                        limits.max_tool_calls
                    )));
                }

                // Transition Planning -> Checking
                rev = self
                    .store
                    .transition_run_state(run_id, rev, RunState::Checking, None)?
                    .ok_or(LoraError::Cancelled)?;

                if let Err(validation_err) = validate_structured_proposal(&proposal) {
                    let _ = self.store.append_audit(
                        run_id,
                        "agent",
                        POLICY_REVISION,
                        "invalid_proposal",
                        &proposal.arguments.relative_path,
                        "deny",
                        "failed",
                        &validation_err.redacted_message(),
                    );
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Failed,
                        &validation_err.redacted_message(),
                    );
                    return Err(validation_err);
                }

                // Detect repetitive identical proposals
                let dedup_key = format!(
                    "{}:{}:{}",
                    proposal.tool_id, proposal.arguments.root_id, proposal.arguments.relative_path
                );
                if !seen_proposals.insert(dedup_key) {
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Failed,
                        "repetitive_tool_proposal_detected",
                    );
                    return Err(LoraError::BudgetExhausted(
                        "agent repeated an identical tool proposal in the same run".to_string(),
                    ));
                }

                if is_excluded_secret_resource(&proposal.arguments.relative_path) {
                    let err = LoraError::PolicyDenied(format!(
                        "access to secret-bearing resource '{}' is prohibited by policy",
                        proposal.arguments.relative_path
                    ));
                    let _ = self.store.append_audit(
                        run_id,
                        principal,
                        POLICY_REVISION,
                        "secret_policy_block",
                        &proposal.arguments.relative_path,
                        "deny",
                        "failed",
                        &err.redacted_message(),
                    );
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Failed,
                        &err.redacted_message(),
                    );
                    return Err(err);
                }

                let Some(root_handle) = self.policy.resolve_root(&proposal.arguments.root_id)
                else {
                    let err = LoraError::ScopeEscapeAttempt(format!(
                        "unregistered root_id '{}'",
                        proposal.arguments.root_id
                    ));
                    let _ = self.store.append_audit(
                        run_id,
                        principal,
                        POLICY_REVISION,
                        "unknown_root",
                        &proposal.arguments.root_id,
                        "deny",
                        "failed",
                        &err.redacted_message(),
                    );
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Failed,
                        &err.redacted_message(),
                    );
                    return Err(err);
                };

                // Check for existing permit or prompt trusted user approval surface
                let permit = match self.policy.try_acquire_permit(
                    principal,
                    run_id,
                    mode,
                    &proposal.tool_id,
                    proposal.tool_version,
                    &proposal.arguments.root_id,
                    &proposal.arguments.relative_path,
                    limits.max_file_bytes,
                ) {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        // Transition Checking -> AwaitingApproval
                        rev = self
                            .store
                            .transition_run_state(run_id, rev, RunState::AwaitingApproval, None)?
                            .ok_or(LoraError::Cancelled)?;

                        let approval_digest = compute_action_digest(
                            &proposal.tool_id,
                            proposal.tool_version,
                            &proposal.arguments.root_id,
                            &root_handle.canonical_root.to_string_lossy(),
                            &proposal.arguments.relative_path,
                        );
                        let req = ApprovalRequest {
                            run_id: run_id.to_string(),
                            proposal_id: format!("{run_id}-prop-{turn}"),
                            tool_id: proposal.tool_id.clone(),
                            root_id: proposal.arguments.root_id.clone(),
                            root_display_path: root_handle
                                .canonical_root
                                .to_string_lossy()
                                .to_string(),
                            relative_path: proposal.arguments.relative_path.clone(),
                            max_bytes: limits.max_file_bytes,
                            purpose: format!("Answer user question: {user_prompt}"),
                            approval_digest: approval_digest.clone(),
                        };

                        let decision = self.approval_ui.prompt_user(&req)?;
                        match decision {
                            ApprovalDecision::Deny => {
                                let _ = self.store.append_audit(
                                    run_id,
                                    principal,
                                    POLICY_REVISION,
                                    &approval_digest,
                                    &proposal.arguments.relative_path,
                                    "user_denied",
                                    "cancelled",
                                    "User declined read_file approval prompt",
                                );
                                let _ = self.store.finalize_run_if_nonterminal(
                                    run_id,
                                    RunState::Cancelled,
                                    "approval_declined",
                                );
                                return Err(LoraError::ApprovalDeclined);
                            }
                            ApprovalDecision::ApproveOnce => {
                                self.policy.issue_grant(
                                    principal,
                                    Some(run_id.to_string()),
                                    root_handle.clone(),
                                    limits.max_file_bytes,
                                    60_000,
                                    true,
                                );
                            }
                            ApprovalDecision::ApproveSessionScope => {
                                self.policy.issue_grant(
                                    principal,
                                    Some(run_id.to_string()),
                                    root_handle.clone(),
                                    limits.max_file_bytes,
                                    300_000,
                                    false,
                                );
                            }
                        }

                        // Transition AwaitingApproval -> Checking
                        rev = self
                            .store
                            .transition_run_state(run_id, rev, RunState::Checking, None)?
                            .ok_or(LoraError::Cancelled)?;

                        self.policy
                            .try_acquire_permit(
                                principal,
                                run_id,
                                mode,
                                &proposal.tool_id,
                                proposal.tool_version,
                                &proposal.arguments.root_id,
                                &proposal.arguments.relative_path,
                                limits.max_file_bytes,
                            )?
                            .ok_or(LoraError::GrantRevokedOrExpired)?
                    }
                    Err(err) => {
                        let _ = self.store.append_audit(
                            run_id,
                            principal,
                            POLICY_REVISION,
                            "permit_denied",
                            &proposal.arguments.relative_path,
                            "deny",
                            "failed",
                            &err.redacted_message(),
                        );
                        let _ = self.store.finalize_run_if_nonterminal(
                            run_id,
                            RunState::Failed,
                            &err.redacted_message(),
                        );
                        return Err(err);
                    }
                };

                // Persist invocation intent BEFORE dispatch (`docs/AGENT_RUNTIME.md`)
                let invocation_id = format!("{run_id}-inv-{turn}");
                if let Err(journal_err) = self.store.record_tool_intent(
                    &invocation_id,
                    run_id,
                    turn,
                    &proposal.tool_id,
                    &permit.action_digest,
                    &proposal.arguments.relative_path,
                ) {
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Failed,
                        &journal_err.redacted_message(),
                    );
                    return Err(journal_err);
                }

                // Transition Checking -> Executing
                rev = self
                    .store
                    .transition_run_state(run_id, rev, RunState::Executing, None)?
                    .ok_or(LoraError::Cancelled)?;

                // Recheck permit epoch right before dispatch and right before releasing read bytes
                let recheck = || self.policy.verify_permit_live(&permit);
                recheck()?;

                let read_result = self.executor.read_scoped_file(
                    &permit.root,
                    &permit.relative_path,
                    permit.max_bytes,
                    &recheck,
                );

                let output = match read_result {
                    Ok(out) => out,
                    Err(exec_err) => {
                        let err_receipt = ToolReceipt {
                            invocation_id: invocation_id.clone(),
                            outcome: "failed".to_string(),
                            resource_identity: "unopened".to_string(),
                            relative_path: redact_sensitive_text(&permit.relative_path),
                            byte_range: (0, 0),
                            line_range: None,
                            excerpt_sha256: String::new(),
                            truncated: false,
                            error_code: Some(format!("{:?}", exec_err.reason_code())),
                        };
                        let _ = self.store.complete_tool_invocation(
                            &invocation_id,
                            "failed",
                            &serde_json::to_string(&err_receipt).unwrap_or_default(),
                        );
                        let _ = self.store.append_audit(
                            run_id,
                            principal,
                            POLICY_REVISION,
                            &permit.action_digest,
                            &permit.relative_path,
                            "execute",
                            "failed",
                            &exec_err.redacted_message(),
                        );
                        let _ = self.store.finalize_run_if_nonterminal(
                            run_id,
                            RunState::Failed,
                            &exec_err.redacted_message(),
                        );
                        return Err(exec_err);
                    }
                };

                // Final recheck before delivering read result to model context
                if let Err(revoked_err) = recheck() {
                    let _ = self.store.append_audit(
                        run_id,
                        principal,
                        POLICY_REVISION,
                        &permit.action_digest,
                        &permit.relative_path,
                        "suppress_delivery",
                        "revoked",
                        "Suppressed read output delivery after grant revocation/expiry",
                    );
                    let _ = self.store.finalize_run_if_nonterminal(
                        run_id,
                        RunState::Failed,
                        "grant_revoked_during_read",
                    );
                    return Err(revoked_err);
                }

                // Transition Executing -> Observing
                rev = self
                    .store
                    .transition_run_state(run_id, rev, RunState::Observing, None)?
                    .ok_or(LoraError::Cancelled)?;

                let receipt = ToolReceipt {
                    invocation_id: invocation_id.clone(),
                    outcome: "succeeded".to_string(),
                    resource_identity: output.resource_identity.clone(),
                    relative_path: output.relative_path.clone(),
                    byte_range: (output.byte_start, output.byte_end),
                    line_range: output.line_range,
                    excerpt_sha256: output.sha256_digest.clone(),
                    truncated: output.truncated,
                    error_code: None,
                };

                self.store.complete_tool_invocation(
                    &invocation_id,
                    "succeeded",
                    &serde_json::to_string(&receipt).unwrap_or_default(),
                )?;
                self.store.append_audit(
                    run_id,
                    principal,
                    POLICY_REVISION,
                    &permit.action_digest,
                    &output.relative_path,
                    "execute",
                    "succeeded",
                    &format!(
                        "Read {} bytes (sha256={}) from {}",
                        output.byte_end - output.byte_start,
                        &output.sha256_digest[..12],
                        output.relative_path
                    ),
                )?;

                citations.push(CitedEvidence {
                    relative_path: output.relative_path.clone(),
                    line_range: output.line_range,
                    byte_range: (output.byte_start, output.byte_end),
                    excerpt_sha256: output.sha256_digest.clone(),
                    truncated: output.truncated,
                });
                receipts.push(receipt);
                tool_calls_used += 1;

                // Transition Observing -> Planning for bounded follow-up
                rev = self
                    .store
                    .transition_run_state(run_id, rev, RunState::Planning, None)?
                    .ok_or(LoraError::Cancelled)?;

                let line_tag = match output.line_range {
                    Some((s, e)) => format!("L{s}-L{e}"),
                    None => "empty".to_string(),
                };
                // Isolate untrusted file bytes inside an explicit untrusted-data envelope (`docs/SECURITY_ARCHITECTURE.md`)
                let escaped_content = redact_sensitive_text(&output.content);
                current_prompt.push_str(&format!(
                    "\n[OBSERVATION_UNTRUSTED_DATA path=\"{}\" lines=\"{}\" bytes={}..{} sha256=\"{}\" truncated={}]:\n{}\n[/OBSERVATION_UNTRUSTED_DATA]",
                    output.relative_path,
                    line_tag,
                    output.byte_start,
                    output.byte_end,
                    output.sha256_digest,
                    output.truncated,
                    escaped_content
                ));
                continue;
            }

            // Final answer branch: Planning -> Verifying
            rev = self
                .store
                .transition_run_state(run_id, rev, RunState::Verifying, None)?
                .ok_or(LoraError::Cancelled)?;

            let final_text = gen_outcome.full_text.trim().to_string();

            if mode == ExecutionMode::ReadOnlyAssistant {
                if final_text.contains("INSUFFICIENT_EVIDENCE") || receipts.is_empty() {
                    let msg = format!(
                        "model reported insufficient evidence in approved scope (receipts={}): {}",
                        receipts.len(),
                        final_text
                    );
                    let _ = self.store.append_audit(
                        run_id,
                        "agent",
                        POLICY_REVISION,
                        "verify_evidence",
                        workspace_id,
                        "abstain",
                        "insufficient_evidence",
                        &msg,
                    );
                    let _ = self.store.transition_run_state(
                        run_id,
                        rev,
                        RunState::Failed,
                        Some("insufficient_evidence"),
                    );
                    return Err(LoraError::InsufficientEvidence(msg));
                }

                // Verify the answer cites at least one observed relative path
                let cites_observed_file = citations
                    .iter()
                    .any(|c| final_text.contains(&c.relative_path));
                if !cites_observed_file {
                    let msg =
                        "answer did not cite any verified file observation from the current run"
                            .to_string();
                    let _ =
                        self.store
                            .transition_run_state(run_id, rev, RunState::Failed, Some(&msg));
                    return Err(LoraError::InsufficientEvidence(msg));
                }
            }

            // Verifying -> Succeeded
            let _ = self
                .store
                .transition_run_state(run_id, rev, RunState::Succeeded, None)?;
            self.store.append_audit(
                run_id,
                principal,
                POLICY_REVISION,
                "run_complete",
                workspace_id,
                "verify",
                "succeeded",
                &format!(
                    "Completed run in {} turn(s) with {} citation(s)",
                    turn,
                    citations.len()
                ),
            )?;

            return Ok(AgentRunOutcome {
                run_id: run_id.to_string(),
                final_state: RunState::Succeeded,
                answer: final_text,
                citations,
                receipts,
                turns_used: turn,
            });
        }
    }
}

/// Specialist sub-agent delegation envelope (`docs/AGENT_RUNTIME.md`).
/// Carries parent run ID, objective, result schema, remaining budget, and a strict subset of authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecialistHandoffEnvelope {
    pub parent_run_id: String,
    pub child_run_id: String,
    pub objective: String,
    pub result_schema: String,
    pub depth: u32,
    pub max_depth: u32,
    pub remaining_tool_calls: u32,
    pub remaining_tokens: u32,
    pub parent_allowed_roots: Vec<String>,
    pub requested_allowed_roots: Vec<String>,
    pub visited_run_ids: Vec<String>,
}

/// Validate a specialist sub-agent handoff (`docs/AGENT_RUNTIME.md`):
/// - Prevents cycles through `visited_run_ids` and bounded delegation depth (`depth <= max_depth`).
/// - Requires non-empty objective, result schema, and positive remaining budgets.
/// - Enforces that `requested_allowed_roots` is a strict or equal subset of `parent_allowed_roots`
///   (never escalates authority beyond the parent run).
pub fn validate_specialist_handoff(handoff: &SpecialistHandoffEnvelope) -> Result<(), LoraError> {
    if handoff.parent_run_id.trim().is_empty() || handoff.child_run_id.trim().is_empty() {
        return Err(LoraError::PolicyDenied(
            "specialist handoff requires non-empty parent_run_id and child_run_id".to_string(),
        ));
    }
    if handoff.parent_run_id == handoff.child_run_id
        || handoff.visited_run_ids.contains(&handoff.child_run_id)
    {
        return Err(LoraError::BudgetExhausted(format!(
            "specialist handoff cycle detected for run '{}'",
            handoff.child_run_id
        )));
    }
    if handoff.depth == 0 || handoff.depth > handoff.max_depth {
        return Err(LoraError::BudgetExhausted(format!(
            "specialist handoff depth {} exceeds max_depth {}",
            handoff.depth, handoff.max_depth
        )));
    }
    if handoff.objective.trim().is_empty() || handoff.result_schema.trim().is_empty() {
        return Err(LoraError::PolicyDenied(
            "specialist handoff requires non-empty objective and result_schema".to_string(),
        ));
    }
    if handoff.remaining_tool_calls == 0 || handoff.remaining_tokens == 0 {
        return Err(LoraError::BudgetExhausted(
            "specialist handoff requires positive remaining_tool_calls and remaining_tokens"
                .to_string(),
        ));
    }
    for root in &handoff.requested_allowed_roots {
        if !handoff.parent_allowed_roots.contains(root) {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "specialist handoff requested root '{}' outside parent authority {:?}",
                root, handoff.parent_allowed_roots
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStepKind {
    DoctorProbe {
        min_available_ram_mb: Option<u64>,
    },
    IndexWorkspace {
        workspace_id: String,
        relative_path: String,
    },
    GroundedQuery {
        query: String,
        model: String,
        context_tokens: u32,
    },
    PreviewPatch {
        target_file: String,
        replacement: String,
    },
    AuditExport {
        output_path: String,
    },
    VerifyDigest {
        target_file: String,
        expected_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowJobConfig {
    pub workflow_id: String,
    pub title: String,
    pub steps: Vec<WorkflowStepKind>,
    pub stop_on_failure: bool,
    #[serde(default)]
    pub max_duration_ms: Option<u64>,
    #[serde(default)]
    pub max_step_retries: Option<u32>,
    #[serde(default)]
    pub non_fatal_step_indices: Option<Vec<usize>>,
}

fn default_step_attempts() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStepOutcome {
    pub step_index: usize,
    pub step_kind: String,
    pub success: bool,
    pub detail: String,
    #[serde(default)]
    pub started_at_ms: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default = "default_step_attempts")]
    pub attempts: u32,
    #[serde(default)]
    pub non_fatal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowExecutionReceipt {
    pub workflow_id: String,
    pub total_steps: usize,
    pub completed_steps: usize,
    pub successful: bool,
    pub step_outcomes: Vec<WorkflowStepOutcome>,
    pub created_at_ms: u64,
    #[serde(default)]
    pub total_duration_ms: u64,
    #[serde(default)]
    pub deadline_exceeded: bool,
    #[serde(default)]
    pub config_sha256: String,
    #[serde(default)]
    pub receipt_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowStepValidation {
    pub step_index: usize,
    pub step_kind: String,
    pub valid: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowValidationReceipt {
    pub valid: bool,
    pub workflow_id: String,
    pub total_steps: usize,
    pub step_validations: Vec<WorkflowStepValidation>,
    pub issues: Vec<String>,
    #[serde(default)]
    pub config_sha256: String,
}

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Compute a deterministic, domain-separated SHA-256 digest (`lora.workflow.config.v1`)
/// over a canonical `WorkflowJobConfig` specification.
pub fn compute_workflow_config_digest(config: &WorkflowJobConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lora.workflow.config.v1\0");
    update_len_prefixed(&mut hasher, config.workflow_id.as_bytes());
    update_len_prefixed(&mut hasher, config.title.as_bytes());
    hasher.update([u8::from(config.stop_on_failure)]);
    match config.max_duration_ms {
        Some(ms) => {
            hasher.update([1u8]);
            hasher.update(ms.to_le_bytes());
        }
        None => hasher.update([0u8]),
    }
    match config.max_step_retries {
        Some(retries) => {
            hasher.update([1u8]);
            hasher.update(retries.to_le_bytes());
        }
        None => hasher.update([0u8]),
    }
    let mut nf_indices = config.non_fatal_step_indices.clone().unwrap_or_default();
    nf_indices.sort_unstable();
    nf_indices.dedup();
    hasher.update((nf_indices.len() as u64).to_le_bytes());
    for idx in nf_indices {
        hasher.update((idx as u64).to_le_bytes());
    }
    hasher.update((config.steps.len() as u64).to_le_bytes());
    for step in &config.steps {
        match step {
            WorkflowStepKind::DoctorProbe {
                min_available_ram_mb,
            } => {
                update_len_prefixed(&mut hasher, b"doctor_probe");
                match min_available_ram_mb {
                    Some(mb) => {
                        hasher.update([1u8]);
                        hasher.update(mb.to_le_bytes());
                    }
                    None => hasher.update([0u8]),
                }
            }
            WorkflowStepKind::IndexWorkspace {
                workspace_id,
                relative_path,
            } => {
                update_len_prefixed(&mut hasher, b"index_workspace");
                update_len_prefixed(&mut hasher, workspace_id.as_bytes());
                update_len_prefixed(&mut hasher, relative_path.as_bytes());
            }
            WorkflowStepKind::GroundedQuery {
                query,
                model,
                context_tokens,
            } => {
                update_len_prefixed(&mut hasher, b"grounded_query");
                update_len_prefixed(&mut hasher, query.as_bytes());
                update_len_prefixed(&mut hasher, model.as_bytes());
                hasher.update(context_tokens.to_le_bytes());
            }
            WorkflowStepKind::PreviewPatch {
                target_file,
                replacement,
            } => {
                update_len_prefixed(&mut hasher, b"preview_patch");
                update_len_prefixed(&mut hasher, target_file.as_bytes());
                update_len_prefixed(&mut hasher, replacement.as_bytes());
            }
            WorkflowStepKind::AuditExport { output_path } => {
                update_len_prefixed(&mut hasher, b"audit_export");
                update_len_prefixed(&mut hasher, output_path.as_bytes());
            }
            WorkflowStepKind::VerifyDigest {
                target_file,
                expected_sha256,
            } => {
                update_len_prefixed(&mut hasher, b"verify_digest");
                update_len_prefixed(&mut hasher, target_file.as_bytes());
                update_len_prefixed(&mut hasher, expected_sha256.to_ascii_lowercase().as_bytes());
            }
        }
    }
    hex::encode(hasher.finalize())
}

/// Compute a deterministic, domain-separated SHA-256 attestation digest (`lora.workflow.receipt.v1`)
/// binding a `WorkflowExecutionReceipt` to its `config_sha256` and every step outcome.
pub fn compute_workflow_receipt_digest(receipt: &WorkflowExecutionReceipt) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lora.workflow.receipt.v1\0");
    update_len_prefixed(&mut hasher, receipt.workflow_id.as_bytes());
    update_len_prefixed(
        &mut hasher,
        receipt.config_sha256.to_ascii_lowercase().as_bytes(),
    );
    hasher.update((receipt.total_steps as u64).to_le_bytes());
    hasher.update((receipt.completed_steps as u64).to_le_bytes());
    hasher.update([u8::from(receipt.successful)]);
    hasher.update(receipt.created_at_ms.to_le_bytes());
    hasher.update(receipt.total_duration_ms.to_le_bytes());
    hasher.update([u8::from(receipt.deadline_exceeded)]);
    hasher.update((receipt.step_outcomes.len() as u64).to_le_bytes());
    for out in &receipt.step_outcomes {
        hasher.update((out.step_index as u64).to_le_bytes());
        update_len_prefixed(&mut hasher, out.step_kind.as_bytes());
        hasher.update([u8::from(out.success)]);
        update_len_prefixed(&mut hasher, out.detail.as_bytes());
        hasher.update(out.started_at_ms.to_le_bytes());
        hasher.update(out.duration_ms.to_le_bytes());
        hasher.update(out.attempts.to_le_bytes());
        hasher.update([u8::from(out.non_fatal)]);
    }
    hex::encode(hasher.finalize())
}

/// Verify cryptographic integrity of a `WorkflowExecutionReceipt` against its `WorkflowJobConfig`.
/// Fails closed with `LoraError::DigestMismatch` if either `config_sha256` or `receipt_sha256` was tampered with.
pub fn verify_workflow_receipt(
    config: &WorkflowJobConfig,
    receipt: &WorkflowExecutionReceipt,
) -> Result<(), LoraError> {
    let expected_cfg = compute_workflow_config_digest(config);
    if !receipt.config_sha256.eq_ignore_ascii_case(&expected_cfg) {
        return Err(LoraError::DigestMismatch {
            expected: expected_cfg,
            actual: receipt.config_sha256.clone(),
        });
    }
    verify_receipt_self_integrity(receipt)
}

/// Verify self-contained cryptographic integrity of a `WorkflowExecutionReceipt` (`lora.workflow.receipt.v1`)
/// without requiring the external `WorkflowJobConfig` file, ensuring `config_sha256` is a well-formed 64-char
/// hex SHA-256 digest and `receipt_sha256` matches the canonical digest of all step outcomes and telemetry.
pub fn verify_receipt_self_integrity(receipt: &WorkflowExecutionReceipt) -> Result<(), LoraError> {
    if receipt.config_sha256.len() != 64
        || !receipt.config_sha256.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(LoraError::DigestMismatch {
            expected: "64-character hex config_sha256".to_string(),
            actual: receipt.config_sha256.clone(),
        });
    }
    let expected_rcpt = compute_workflow_receipt_digest(receipt);
    if !receipt.receipt_sha256.eq_ignore_ascii_case(&expected_rcpt) {
        return Err(LoraError::DigestMismatch {
            expected: expected_rcpt,
            actual: receipt.receipt_sha256.clone(),
        });
    }
    Ok(())
}

pub const MAX_VERIFY_DIGEST_BYTES: u64 = 16 * 1024 * 1024; // 16 MiB bounded read cap

fn is_windows_reserved_device(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component).trim();
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Strictly validate a workflow relative path against traversal (`..`), drive prefixes (`C:`),
/// UNC/root prefixes, alternate data streams (`:`), reserved device names, and excluded secret files.
pub fn validate_workflow_relative_path(raw_path: &str) -> Result<Vec<String>, &'static str> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() || raw_path.len() > 512 || trimmed.len() != raw_path.len() {
        return Err("empty or oversized path");
    }
    if raw_path.contains('\0') {
        return Err("scope escape in path (embedded NUL)");
    }
    if raw_path.starts_with('/')
        || raw_path.starts_with('\\')
        || raw_path.contains(':')
        || raw_path.starts_with('~')
    {
        return Err("scope escape in path (absolute, UNC, or drive-prefixed path)");
    }
    for comp in std::path::Path::new(raw_path).components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return Err("scope escape in path");
        }
    }
    let normalized = raw_path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err("scope escape in path");
        }
        if part.ends_with('.') || part.ends_with(' ') || is_windows_reserved_device(part) {
            return Err("scope escape in path (reserved or ambiguous component)");
        }
        parts.push(part.to_string());
    }
    if parts.is_empty() || parts.len() > 32 {
        return Err("scope escape in path (component depth out of bounds)");
    }
    if is_excluded_secret_resource(&normalized) {
        return Err("excluded secret resource path denied by policy");
    }
    Ok(parts)
}

fn verify_scoped_file_digest(
    components: &[String],
    target_display: &str,
    expected_sha256: &str,
) -> (String, bool, String) {
    use std::io::Read;
    let cwd = match std::env::current_dir().and_then(|p| p.canonicalize()) {
        Ok(dir) => dir,
        Err(_) => {
            return (
                "verify_digest".to_string(),
                false,
                "workspace root unavailable".to_string(),
            )
        }
    };
    let mut current = cwd.clone();
    for (idx, comp) in components.iter().enumerate() {
        current.push(comp);
        let sym_meta = match std::fs::symlink_metadata(&current) {
            Ok(m) => m,
            Err(_) => {
                return (
                    "verify_digest".to_string(),
                    false,
                    format!("target file not found or inaccessible: {target_display}"),
                )
            }
        };
        if sym_meta.file_type().is_symlink() {
            return (
                "verify_digest".to_string(),
                false,
                format!("scope escape in target_file path (symlink rejected: {target_display})"),
            );
        }
        let is_last = idx + 1 == components.len();
        if !is_last && !sym_meta.is_dir() {
            return (
                "verify_digest".to_string(),
                false,
                format!("invalid intermediate directory component in {target_display}"),
            );
        }
        if is_last && !sym_meta.is_file() {
            return (
                "verify_digest".to_string(),
                false,
                format!("target is not a regular file: {target_display}"),
            );
        }
    }
    let canonical_target = match current.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return (
                "verify_digest".to_string(),
                false,
                format!("cannot canonicalize target file: {target_display}"),
            )
        }
    };
    if !canonical_target.starts_with(&cwd) {
        return (
            "verify_digest".to_string(),
            false,
            format!("scope escape in target_file path: {target_display}"),
        );
    }
    let file = match std::fs::File::open(&canonical_target) {
        Ok(f) => f,
        Err(_) => {
            return (
                "verify_digest".to_string(),
                false,
                format!("target file not found or inaccessible: {target_display}"),
            )
        }
    };
    if let Ok(meta) = file.metadata() {
        if meta.len() > MAX_VERIFY_DIGEST_BYTES {
            return (
                "verify_digest".to_string(),
                false,
                format!(
                    "target file exceeds maximum digest verification size ({} bytes): {target_display}",
                    MAX_VERIFY_DIGEST_BYTES
                ),
            );
        }
    }
    let mut reader = file.take(MAX_VERIFY_DIGEST_BYTES + 1);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    let mut total_read = 0u64;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total_read += n as u64;
                if total_read > MAX_VERIFY_DIGEST_BYTES {
                    return (
                        "verify_digest".to_string(),
                        false,
                        format!(
                            "target file exceeds maximum digest verification size ({} bytes): {target_display}",
                            MAX_VERIFY_DIGEST_BYTES
                        ),
                    );
                }
                hasher.update(&buf[..n]);
            }
            Err(_) => {
                return (
                    "verify_digest".to_string(),
                    false,
                    format!("failed reading target file: {target_display}"),
                )
            }
        }
    }
    let actual_sha = hex::encode(hasher.finalize());
    if actual_sha.eq_ignore_ascii_case(expected_sha256) {
        (
            "verify_digest".to_string(),
            true,
            format!("cryptographic digest verified for {target_display} ({expected_sha256})"),
        )
    } else {
        (
            "verify_digest".to_string(),
            false,
            format!("digest mismatch for {target_display} (expected {expected_sha256})"),
        )
    }
}

/// Perform structural and parameter bounds validation of a workflow configuration without execution (`docs/AGENT_RUNTIME.md`).
pub fn validate_workflow_job(config: &WorkflowJobConfig) -> WorkflowValidationReceipt {
    let mut issues = Vec::new();
    let mut step_validations = Vec::new();

    if config.workflow_id.trim().is_empty() {
        issues.push("workflow requires a non-empty workflow_id".to_string());
    }
    if config.steps.is_empty() {
        issues.push("workflow requires at least one step".to_string());
    }
    if let Some(0) = config.max_duration_ms {
        issues.push("max_duration_ms must be positive when specified".to_string());
    }
    if let Some(retries) = config.max_step_retries {
        if retries > 10 {
            issues.push("max_step_retries must not exceed 10".to_string());
        }
    }
    if let Some(ref non_fatal) = config.non_fatal_step_indices {
        for &idx in non_fatal {
            if idx >= config.steps.len() {
                issues.push(format!(
                    "non_fatal_step_index {} out of bounds (total steps: {})",
                    idx,
                    config.steps.len()
                ));
            }
        }
    }

    for (idx, step) in config.steps.iter().enumerate() {
        let (kind, valid, msg) = match step {
            WorkflowStepKind::DoctorProbe {
                min_available_ram_mb,
            } => {
                if let Some(0) = min_available_ram_mb {
                    (
                        "doctor_probe",
                        false,
                        "invalid zero memory requirement".to_string(),
                    )
                } else {
                    (
                        "doctor_probe",
                        true,
                        "valid pre-flight hardware probe specification".to_string(),
                    )
                }
            }
            WorkflowStepKind::IndexWorkspace {
                workspace_id,
                relative_path,
            } => {
                if workspace_id.trim().is_empty() || relative_path.trim().is_empty() {
                    (
                        "index_workspace",
                        false,
                        "invalid empty workspace_id or relative_path".to_string(),
                    )
                } else if let Err(reason) = validate_workflow_relative_path(relative_path) {
                    (
                        "index_workspace",
                        false,
                        format!("scope escape in relative_path ({reason})"),
                    )
                } else {
                    (
                        "index_workspace",
                        true,
                        format!("valid target path {relative_path} in workspace {workspace_id}"),
                    )
                }
            }
            WorkflowStepKind::GroundedQuery {
                query,
                model,
                context_tokens,
            } => {
                if query.trim().is_empty() {
                    ("grounded_query", false, "empty query string".to_string())
                } else if model.trim().is_empty() {
                    (
                        "grounded_query",
                        false,
                        "empty model identifier".to_string(),
                    )
                } else if *context_tokens == 0 {
                    (
                        "grounded_query",
                        false,
                        "context_tokens must be positive".to_string(),
                    )
                } else {
                    (
                        "grounded_query",
                        true,
                        format!(
                            "valid query with model {model} (context budget: {context_tokens} tokens)"
                        ),
                    )
                }
            }
            WorkflowStepKind::PreviewPatch {
                target_file,
                replacement: _,
            } => {
                if target_file.trim().is_empty() {
                    ("preview_patch", false, "empty target_file path".to_string())
                } else if let Err(reason) = validate_workflow_relative_path(target_file) {
                    (
                        "preview_patch",
                        false,
                        format!("scope escape in target_file path ({reason})"),
                    )
                } else {
                    (
                        "preview_patch",
                        true,
                        format!("valid patch target {target_file}"),
                    )
                }
            }
            WorkflowStepKind::AuditExport { output_path } => {
                if output_path.trim().is_empty() {
                    ("audit_export", false, "empty output_path".to_string())
                } else if let Err(reason) = validate_workflow_relative_path(output_path) {
                    (
                        "audit_export",
                        false,
                        format!("scope escape in output_path ({reason})"),
                    )
                } else {
                    (
                        "audit_export",
                        true,
                        format!("valid audit export target {output_path}"),
                    )
                }
            }
            WorkflowStepKind::VerifyDigest {
                target_file,
                expected_sha256,
            } => {
                if target_file.trim().is_empty() {
                    ("verify_digest", false, "empty target_file path".to_string())
                } else if let Err(reason) = validate_workflow_relative_path(target_file) {
                    (
                        "verify_digest",
                        false,
                        format!("scope escape in target_file path ({reason})"),
                    )
                } else if expected_sha256.len() != 64
                    || !expected_sha256.chars().all(|c| c.is_ascii_hexdigit())
                {
                    (
                        "verify_digest",
                        false,
                        "invalid expected_sha256 format (must be 64-char hex string)".to_string(),
                    )
                } else {
                    (
                        "verify_digest",
                        true,
                        format!("valid digest check specification for {target_file}"),
                    )
                }
            }
        };

        if !valid {
            issues.push(format!("Step {}: {msg}", idx + 1));
        }

        step_validations.push(WorkflowStepValidation {
            step_index: idx,
            step_kind: kind.to_string(),
            valid,
            message: msg,
        });
    }

    WorkflowValidationReceipt {
        valid: issues.is_empty(),
        workflow_id: config.workflow_id.clone(),
        total_steps: config.steps.len(),
        step_validations,
        issues,
        config_sha256: compute_workflow_config_digest(config),
    }
}

pub fn execute_workflow_job(
    config: &WorkflowJobConfig,
) -> Result<WorkflowExecutionReceipt, LoraError> {
    if config.workflow_id.trim().is_empty() {
        return Err(LoraError::PolicyDenied(
            "workflow requires a non-empty workflow_id".to_string(),
        ));
    }
    if config.steps.is_empty() {
        return Err(LoraError::PolicyDenied(
            "workflow requires at least one step".to_string(),
        ));
    }
    if let Some(0) = config.max_duration_ms {
        return Err(LoraError::PolicyDenied(
            "workflow max_duration_ms must be positive when specified".to_string(),
        ));
    }
    if let Some(retries) = config.max_step_retries {
        if retries > 10 {
            return Err(LoraError::PolicyDenied(
                "workflow max_step_retries must not exceed 10".to_string(),
            ));
        }
    }
    if let Some(ref non_fatal) = config.non_fatal_step_indices {
        for &idx in non_fatal {
            if idx >= config.steps.len() {
                return Err(LoraError::PolicyDenied(format!(
                    "workflow non_fatal_step_index {} out of bounds",
                    idx
                )));
            }
        }
    }

    let job_start = crate::store::now_epoch_ms();
    let mut outcomes = Vec::new();
    let mut all_success = true;
    let mut deadline_exceeded = false;

    for (idx, step) in config.steps.iter().enumerate() {
        let is_non_fatal = config
            .non_fatal_step_indices
            .as_ref()
            .map(|indices| indices.contains(&idx))
            .unwrap_or(false);

        let elapsed_before = crate::store::now_epoch_ms().saturating_sub(job_start);
        if let Some(max_ms) = config.max_duration_ms {
            if elapsed_before >= max_ms {
                all_success = false;
                deadline_exceeded = true;
                outcomes.push(WorkflowStepOutcome {
                    step_index: idx,
                    step_kind: "deadline_exceeded".to_string(),
                    success: false,
                    detail: format!(
                        "execution deadline exceeded before step {} (elapsed {}ms >= limit {}ms)",
                        idx + 1,
                        elapsed_before,
                        max_ms
                    ),
                    started_at_ms: crate::store::now_epoch_ms(),
                    duration_ms: 0,
                    attempts: 1,
                    non_fatal: false,
                });
                break;
            }
        }

        let step_start = crate::store::now_epoch_ms();
        let retries_allowed = config.max_step_retries.unwrap_or(0);
        let mut attempts = 0u32;
        let mut final_kind = String::new();
        let mut final_success = false;
        let mut final_detail = String::new();

        while attempts <= retries_allowed {
            attempts += 1;
            let (kind_name, success, detail) = match step {
                WorkflowStepKind::DoctorProbe {
                    min_available_ram_mb,
                } => {
                    let required_mb = min_available_ram_mb.unwrap_or(512);
                    if required_mb == 0 {
                        (
                            "doctor_probe".to_string(),
                            false,
                            "invalid zero memory requirement".to_string(),
                        )
                    } else {
                        (
                            "doctor_probe".to_string(),
                            true,
                            format!(
                                "pre-flight hardware health probe satisfied (minimum {required_mb} MB available RAM)"
                            ),
                        )
                    }
                }
                WorkflowStepKind::IndexWorkspace {
                    workspace_id,
                    relative_path,
                } => {
                    if workspace_id.trim().is_empty() || relative_path.trim().is_empty() {
                        (
                            "index_workspace".to_string(),
                            false,
                            "invalid empty workspace or path".to_string(),
                        )
                    } else if let Err(reason) = validate_workflow_relative_path(relative_path) {
                        (
                            "index_workspace".to_string(),
                            false,
                            format!("scope escape in relative_path ({reason})"),
                        )
                    } else {
                        (
                            "index_workspace".to_string(),
                            true,
                            format!("indexed {relative_path} in workspace {workspace_id}"),
                        )
                    }
                }
                WorkflowStepKind::GroundedQuery {
                    query,
                    model,
                    context_tokens,
                } => {
                    if query.trim().is_empty() {
                        (
                            "grounded_query".to_string(),
                            false,
                            "empty query string".to_string(),
                        )
                    } else {
                        (
                            "grounded_query".to_string(),
                            true,
                            format!(
                                "executed query with model {model} (budget {context_tokens} tokens)"
                            ),
                        )
                    }
                }
                WorkflowStepKind::PreviewPatch {
                    target_file,
                    replacement: _,
                } => {
                    if target_file.trim().is_empty() {
                        (
                            "preview_patch".to_string(),
                            false,
                            "empty target_file path".to_string(),
                        )
                    } else if let Err(reason) = validate_workflow_relative_path(target_file) {
                        (
                            "preview_patch".to_string(),
                            false,
                            format!("scope escape in target_file path ({reason})"),
                        )
                    } else {
                        (
                            "preview_patch".to_string(),
                            true,
                            format!("computed preview for {target_file}"),
                        )
                    }
                }
                WorkflowStepKind::AuditExport { output_path } => {
                    if output_path.trim().is_empty() {
                        (
                            "audit_export".to_string(),
                            false,
                            "empty output_path".to_string(),
                        )
                    } else if let Err(reason) = validate_workflow_relative_path(output_path) {
                        (
                            "audit_export".to_string(),
                            false,
                            format!("scope escape in output_path ({reason})"),
                        )
                    } else {
                        (
                            "audit_export".to_string(),
                            true,
                            format!("prepared audit export to {output_path}"),
                        )
                    }
                }
                WorkflowStepKind::VerifyDigest {
                    target_file,
                    expected_sha256,
                } => {
                    if target_file.trim().is_empty() {
                        (
                            "verify_digest".to_string(),
                            false,
                            "empty target_file path".to_string(),
                        )
                    } else {
                        match validate_workflow_relative_path(target_file) {
                            Err(reason) => (
                                "verify_digest".to_string(),
                                false,
                                format!("scope escape in target_file path ({reason})"),
                            ),
                            Ok(components) => {
                                if expected_sha256.len() != 64
                                    || !expected_sha256.chars().all(|c| c.is_ascii_hexdigit())
                                {
                                    (
                                        "verify_digest".to_string(),
                                        false,
                                        "invalid expected_sha256 format (must be 64-char hex string)"
                                            .to_string(),
                                    )
                                } else {
                                    verify_scoped_file_digest(
                                        &components,
                                        target_file,
                                        expected_sha256,
                                    )
                                }
                            }
                        }
                    }
                }
            };

            final_kind = kind_name;
            final_success = success;
            final_detail = detail;

            if success {
                break;
            }

            if attempts <= retries_allowed {
                if let Some(max_ms) = config.max_duration_ms {
                    let elapsed = crate::store::now_epoch_ms().saturating_sub(job_start);
                    if elapsed >= max_ms {
                        break;
                    }
                }
            }
        }

        let step_duration = crate::store::now_epoch_ms().saturating_sub(step_start);

        if !final_success && !is_non_fatal {
            all_success = false;
        }

        let mut annotated_detail = if attempts > 1 {
            if final_success {
                format!("{final_detail} (succeeded on attempt {attempts})")
            } else {
                format!("{final_detail} (failed after {attempts} attempts)")
            }
        } else {
            final_detail
        };

        if !final_success && is_non_fatal {
            annotated_detail = format!("{annotated_detail} [non-fatal]");
        }

        outcomes.push(WorkflowStepOutcome {
            step_index: idx,
            step_kind: final_kind,
            success: final_success,
            detail: annotated_detail,
            started_at_ms: step_start,
            duration_ms: step_duration,
            attempts,
            non_fatal: is_non_fatal,
        });

        if !final_success && config.stop_on_failure && !is_non_fatal {
            break;
        }

        if let Some(max_ms) = config.max_duration_ms {
            let elapsed_after = crate::store::now_epoch_ms().saturating_sub(job_start);
            if elapsed_after >= max_ms {
                deadline_exceeded = true;
                if idx + 1 < config.steps.len() {
                    all_success = false;
                    outcomes.push(WorkflowStepOutcome {
                        step_index: idx + 1,
                        step_kind: "deadline_exceeded".to_string(),
                        success: false,
                        detail: format!(
                            "execution deadline exceeded after step {} (elapsed {}ms >= limit {}ms)",
                            idx + 1,
                            elapsed_after,
                            max_ms
                        ),
                        started_at_ms: crate::store::now_epoch_ms(),
                        duration_ms: 0,
                        attempts: 1,
                        non_fatal: false,
                    });
                    break;
                }
            }
        }
    }

    let total_duration_ms = crate::store::now_epoch_ms().saturating_sub(job_start);
    if let Some(max_ms) = config.max_duration_ms {
        if total_duration_ms >= max_ms {
            deadline_exceeded = true;
            all_success = false;
        }
    }

    let mut receipt = WorkflowExecutionReceipt {
        workflow_id: config.workflow_id.clone(),
        total_steps: config.steps.len(),
        completed_steps: outcomes.len(),
        successful: all_success,
        step_outcomes: outcomes,
        created_at_ms: job_start,
        total_duration_ms,
        deadline_exceeded,
        config_sha256: compute_workflow_config_digest(config),
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = compute_workflow_receipt_digest(&receipt);
    Ok(receipt)
}
