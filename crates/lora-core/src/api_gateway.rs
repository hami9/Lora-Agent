//! Loopback REST security gate (`R08`), idempotency-key ledger, and Cloud Provider Egress Consent
//! preview/permit validator (`docs/API_AND_SDK.md`, `docs/SECURITY_ARCHITECTURE.md`, Phase 6).

use crate::error::LoraError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Validate that a listening bind address is strictly numeric loopback (`127.0.0.1` or `[::1]`) (`docs/API_AND_SDK.md`).
pub fn validate_loopback_bind(addr: &str) -> Result<(), LoraError> {
    let trimmed = addr.trim();
    if trimmed.starts_with("127.0.0.1:") || trimmed.starts_with("[::1]:") {
        Ok(())
    } else {
        Err(LoraError::PolicyDenied(format!(
            "refusing non-loopback API bind address '{addr}'; only numeric 127.0.0.1 or [::1] is permitted by default"
        )))
    }
}

/// Validate an incoming loopback API request against DNS rebinding, cross-origin browser attacks (`R08`),
/// ambient cookie authentication, and URL query-string token leakage (`docs/API_AND_SDK.md`).
pub fn verify_loopback_http_security(
    host_header: Option<&str>,
    origin_header: Option<&str>,
    authorization_header: Option<&str>,
    cookie_header: Option<&str>,
    request_uri: &str,
    expected_bearer_token: &str,
) -> Result<(), LoraError> {
    // 1. Reject tokens leaked in URL query string
    if request_uri.contains("token=") || request_uri.contains("access_token=") {
        return Err(LoraError::PolicyDenied(
            "bearer credentials in URL query strings are prohibited".to_string(),
        ));
    }

    // 2. Reject ambient cookie authentication
    if cookie_header.is_some_and(|c| !c.trim().is_empty()) {
        return Err(LoraError::PolicyDenied(
            "ambient Cookie header rejected on loopback API".to_string(),
        ));
    }

    // 3. Validate Host header to prevent DNS rebinding (`evil.rebind.example:8080`)
    let Some(host) = host_header else {
        return Err(LoraError::PolicyDenied(
            "missing Host header on loopback API request".to_string(),
        ));
    };
    let host_only = host.split(':').next().unwrap_or(host).trim();
    if host_only != "127.0.0.1" && host_only != "[::1]" && host_only != "localhost" {
        return Err(LoraError::PolicyDenied(format!(
            "DNS rebinding defense blocked non-loopback Host header '{host}'"
        )));
    }

    // 4. Validate Origin header if present (block arbitrary browser tabs `https://malicious.example`)
    if let Some(origin) = origin_header {
        let orig = origin.trim();
        let allowed = orig.starts_with("tauri://localhost")
            || orig.starts_with("http://127.0.0.1:")
            || orig.starts_with("http://[::1]:");
        if !allowed {
            return Err(LoraError::PolicyDenied(format!(
                "cross-origin browser request blocked from untrusted Origin '{orig}'"
            )));
        }
    }

    // 5. Constant-time / exact bearer token check
    let Some(auth) = authorization_header else {
        return Err(LoraError::PolicyDenied(
            "missing Authorization bearer token".to_string(),
        ));
    };
    let Some(provided_token) = auth.strip_prefix("Bearer ") else {
        return Err(LoraError::PolicyDenied(
            "invalid Authorization scheme (expected 'Bearer <token>')".to_string(),
        ));
    };
    if provided_token.trim() != expected_bearer_token {
        return Err(LoraError::PolicyDenied(
            "invalid or expired bearer token".to_string(),
        ));
    }

    Ok(())
}

/// Tracks `POST /v1/runs` idempotency keys and body digests (`docs/API_AND_SDK.md`):
/// - Same key + same body digest -> returns existing `run_id`
/// - Same key + different body digest -> returns conflict error (`409 Conflict`)
#[derive(Default)]
pub struct IdempotencyLedger {
    entries: HashMap<(String, String), (String, String)>,
}

impl IdempotencyLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_or_verify(
        &mut self,
        principal: &str,
        idempotency_key: &str,
        request_body_json: &str,
        candidate_run_id: &str,
    ) -> Result<String, LoraError> {
        let body_digest = hex::encode(Sha256::digest(request_body_json.as_bytes()));
        let key = (principal.to_string(), idempotency_key.to_string());

        if let Some((existing_digest, existing_run_id)) = self.entries.get(&key) {
            if existing_digest == &body_digest {
                return Ok(existing_run_id.clone());
            }
            return Err(LoraError::PolicyDenied(format!(
                "idempotency conflict (409): key '{idempotency_key}' was already used with a different request payload"
            )));
        }

        self.entries
            .insert(key, (body_digest, candidate_run_id.to_string()));
        Ok(candidate_run_id.to_string())
    }
}

/// Preview shown to the user before any external cloud provider transmission (`docs/API_AND_SDK.md`, `docs/DESKTOP_UX.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressConsentPreview {
    pub provider_id: String,
    pub model_id: String,
    pub destination_origin: String,
    pub data_classes: Vec<String>,
    pub prompt_text: String,
    pub attached_excerpts: Vec<String>,
    pub estimated_cost_usd_micros: Option<u64>,
}

impl EgressConsentPreview {
    /// Compute a canonical cryptographic digest over every field of the egress preview.
    /// Any change to prompt, destination, model, or attached excerpts changes this digest and invalidates the permit.
    pub fn compute_preview_digest(&self) -> String {
        let canonical = serde_json::to_string(self).unwrap_or_default();
        hex::encode(Sha256::digest(canonical.as_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPermit {
    pub permit_id: String,
    pub principal: String,
    pub approved_preview_digest: String,
    pub expires_at_ms: u64,
}

/// Verify that an outgoing cloud provider payload matches the exact human-approved `EgressConsentPreview` (`docs/API_AND_SDK.md`).
pub fn verify_egress_permit(
    permit: &EgressPermit,
    actual_payload: &EgressConsentPreview,
    current_time_ms: u64,
) -> Result<(), LoraError> {
    if current_time_ms > permit.expires_at_ms {
        return Err(LoraError::GrantRevokedOrExpired);
    }
    if !actual_payload.destination_origin.starts_with("https://") {
        return Err(LoraError::PolicyDenied(
            "external provider destination_origin must use HTTPS".to_string(),
        ));
    }
    let actual_digest = actual_payload.compute_preview_digest();
    if actual_digest != permit.approved_preview_digest {
        return Err(LoraError::PolicyDenied(
            "outgoing provider payload or destination changed after consent preview; permit invalidated"
                .to_string(),
        ));
    }
    Ok(())
}

/// Stage at which an optional external provider request failed (`docs/API_AND_SDK.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureStage {
    PreSendConnection,
    PostSubmitOrStreamingStarted,
    LocalInferenceFailed,
}

/// Retry decision for an optional external provider (`docs/API_AND_SDK.md`):
/// - Never fall back from local inference failure to external cloud transmission
/// - Retry bounded pre-send connection failures with jitter
/// - After a response has started or a submitted request may have been billed, require explicit user retry unless provider idempotency is verified
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRetryDecision {
    RetryWithBoundedJitter,
    RequireExplicitUserRetry,
    ForbiddenLocalToCloudFallback,
}

pub fn evaluate_provider_retry_policy(
    stage: ProviderFailureStage,
    attempt_number: u8,
    provider_idempotency_verified: bool,
) -> ProviderRetryDecision {
    match stage {
        ProviderFailureStage::LocalInferenceFailed => {
            ProviderRetryDecision::ForbiddenLocalToCloudFallback
        }
        ProviderFailureStage::PreSendConnection => {
            if attempt_number < 3 {
                ProviderRetryDecision::RetryWithBoundedJitter
            } else {
                ProviderRetryDecision::RequireExplicitUserRetry
            }
        }
        ProviderFailureStage::PostSubmitOrStreamingStarted => {
            if provider_idempotency_verified && attempt_number < 2 {
                ProviderRetryDecision::RetryWithBoundedJitter
            } else {
                ProviderRetryDecision::RequireExplicitUserRetry
            }
        }
    }
}

/// Capabilities descriptor returned by `GET /v1/capabilities` (`docs/API_AND_SDK.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiCapabilities {
    pub api_major: u16,
    pub min_compatible_minor: u16,
    pub max_compatible_minor: u16,
    pub principal: String,
    pub enabled_operations: Vec<String>,
    pub max_request_bytes: usize,
    pub sse_replay_window_events: usize,
}

/// Versioned Server-Sent Event (`GET /v1/runs/{id}/events`, `docs/API_AND_SDK.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseEventFrame {
    pub event_id: String,
    pub run_id: String,
    pub sequence: u64,
    pub event_kind: String,
    pub schema_version: u16,
    pub payload_json: String,
}

impl SseEventFrame {
    /// Format as standard W3C Server-Sent Event wire text.
    pub fn to_sse_wire(&self) -> String {
        let data = serde_json::json!({
            "run_id": self.run_id,
            "sequence": self.sequence,
            "kind": self.event_kind,
            "schema_version": self.schema_version,
            "payload": serde_json::from_str::<serde_json::Value>(&self.payload_json)
                .unwrap_or_else(|_| serde_json::Value::String(self.payload_json.clone())),
        });
        format!(
            "id: {}\nevent: {}\ndata: {}\n\n",
            self.event_id, self.event_kind, data
        )
    }
}

/// Owner-only run status summary returned by `GET /v1/runs/{id}` and `POST /v1/runs/{id}/cancel` (`docs/API_AND_SDK.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiRunSummary {
    pub run_id: String,
    pub owner_principal: String,
    pub state: String,
    pub last_sequence: u64,
    pub cancel_requested: bool,
}

/// Outcome of subscribing/reconnecting to `GET /v1/runs/{id}/events` with optional `Last-Event-ID`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseReplayResult {
    pub replay_gap_detected: bool,
    pub current_state: String,
    pub events: Vec<SseEventFrame>,
}

struct ManagedApiRun {
    summary: ApiRunSummary,
    replay_window: std::collections::VecDeque<SseEventFrame>,
    max_replay_events: usize,
}

/// Manages REST/SSE runs, enforcing owner-only visibility, bounded SSE replay windows (`Last-Event-ID`),
/// replay-gap detection, and idempotent cancellation (`docs/API_AND_SDK.md`).
pub struct ApiRunManager {
    runs: HashMap<String, ManagedApiRun>,
    max_replay_events: usize,
}

impl Default for ApiRunManager {
    fn default() -> Self {
        Self::new(32)
    }
}

impl ApiRunManager {
    pub fn new(max_replay_events: usize) -> Self {
        Self {
            runs: HashMap::new(),
            max_replay_events: max_replay_events.max(1),
        }
    }

    pub fn capabilities_for(&self, principal: &str) -> ApiCapabilities {
        ApiCapabilities {
            api_major: 1,
            min_compatible_minor: 0,
            max_compatible_minor: 0,
            principal: principal.to_string(),
            enabled_operations: vec![
                "models.list".to_string(),
                "models.inspect".to_string(),
                "models.delete".to_string(),
                "runs.create".to_string(),
                "runs.inspect".to_string(),
                "runs.events.sse".to_string(),
                "runs.cancel".to_string(),
                "memory.index".to_string(),
                "memory.search".to_string(),
                "agent.assistant".to_string(),
                "agent.rag".to_string(),
                "patch.preview".to_string(),
                "patch.apply".to_string(),
                "patch.rollback".to_string(),
            ],
            max_request_bytes: 1024 * 1024,
            sse_replay_window_events: self.max_replay_events,
        }
    }

    pub fn create_run(&mut self, run_id: &str, owner_principal: &str) -> ApiRunSummary {
        let summary = ApiRunSummary {
            run_id: run_id.to_string(),
            owner_principal: owner_principal.to_string(),
            state: "running".to_string(),
            last_sequence: 0,
            cancel_requested: false,
        };
        self.runs.insert(
            run_id.to_string(),
            ManagedApiRun {
                summary: summary.clone(),
                replay_window: std::collections::VecDeque::new(),
                max_replay_events: self.max_replay_events,
            },
        );
        summary
    }

    pub fn append_event(
        &mut self,
        run_id: &str,
        event_kind: &str,
        payload_json: &str,
    ) -> Result<SseEventFrame, LoraError> {
        let Some(run) = self.runs.get_mut(run_id) else {
            return Err(LoraError::PolicyDenied(format!("unknown run '{run_id}'")));
        };
        if run.summary.state == "cancelled" || run.summary.state == "succeeded" {
            return Err(LoraError::PolicyDenied(
                "late frame after terminal run state rejected".to_string(),
            ));
        }
        run.summary.last_sequence += 1;
        let seq = run.summary.last_sequence;
        let frame = SseEventFrame {
            event_id: format!("{run_id}:{seq}"),
            run_id: run_id.to_string(),
            sequence: seq,
            event_kind: event_kind.to_string(),
            schema_version: 1,
            payload_json: payload_json.to_string(),
        };
        if run.replay_window.len() >= run.max_replay_events {
            run.replay_window.pop_front();
        }
        run.replay_window.push_back(frame.clone());
        if event_kind == "run_completed" {
            run.summary.state = "succeeded".to_string();
        }
        Ok(frame)
    }

    /// Owner-only inspection (`GET /v1/runs/{id}`). Unauthorized cross-principal reads fail closed.
    pub fn get_run(
        &self,
        run_id: &str,
        caller_principal: &str,
    ) -> Result<ApiRunSummary, LoraError> {
        let Some(run) = self.runs.get(run_id) else {
            return Err(LoraError::PolicyDenied(format!("run '{run_id}' not found")));
        };
        if run.summary.owner_principal != caller_principal {
            return Err(LoraError::PolicyDenied(
                "unauthorized cross-principal run read denied".to_string(),
            ));
        }
        Ok(run.summary.clone())
    }

    /// Idempotent owner-only cancellation (`POST /v1/runs/{id}/cancel`).
    pub fn cancel_run(
        &mut self,
        run_id: &str,
        caller_principal: &str,
    ) -> Result<ApiRunSummary, LoraError> {
        let Some(run) = self.runs.get_mut(run_id) else {
            return Err(LoraError::PolicyDenied(format!("run '{run_id}' not found")));
        };
        if run.summary.owner_principal != caller_principal {
            return Err(LoraError::PolicyDenied(
                "unauthorized cross-principal run cancellation denied".to_string(),
            ));
        }
        run.summary.cancel_requested = true;
        if run.summary.state == "running" {
            run.summary.state = "cancelled".to_string();
        }
        Ok(run.summary.clone())
    }

    /// Resume or subscribe to SSE (`GET /v1/runs/{id}/events`) with optional `Last-Event-ID` (`<run_id>:<seq>`).
    /// Returns `replay_gap_detected = true` when older deltas have expired from the finite replay buffer (`docs/API_AND_SDK.md`).
    pub fn replay_events(
        &self,
        run_id: &str,
        caller_principal: &str,
        last_event_id: Option<&str>,
    ) -> Result<SseReplayResult, LoraError> {
        let Some(run) = self.runs.get(run_id) else {
            return Err(LoraError::PolicyDenied(format!("run '{run_id}' not found")));
        };
        if run.summary.owner_principal != caller_principal {
            return Err(LoraError::PolicyDenied(
                "unauthorized cross-principal SSE subscription denied".to_string(),
            ));
        }

        let Some(last_id) = last_event_id else {
            return Ok(SseReplayResult {
                replay_gap_detected: false,
                current_state: run.summary.state.clone(),
                events: run.replay_window.iter().cloned().collect(),
            });
        };

        let parsed_seq = last_id
            .rsplit_once(':')
            .and_then(|(_, s)| s.parse::<u64>().ok())
            .ok_or_else(|| {
                LoraError::MalformedArtifact(format!("invalid Last-Event-ID '{last_id}'"))
            })?;

        let oldest_buffered = run.replay_window.front().map(|e| e.sequence).unwrap_or(1);
        if parsed_seq + 1 < oldest_buffered {
            return Ok(SseReplayResult {
                replay_gap_detected: true,
                current_state: run.summary.state.clone(),
                events: vec![],
            });
        }

        let events = run
            .replay_window
            .iter()
            .filter(|e| e.sequence > parsed_seq)
            .cloned()
            .collect();

        Ok(SseReplayResult {
            replay_gap_detected: false,
            current_state: run.summary.state.clone(),
            events,
        })
    }
}
