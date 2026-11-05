//! Bounded agent runtime state machine, closed proposal parser, evidence receipts,
//! and citation verification (`docs/AGENT_RUNTIME.md`, `T8`, `M5`, `M6`, `M7`).

use crate::error::{redact_sensitive_text, LoraError};
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
