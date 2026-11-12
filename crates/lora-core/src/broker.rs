//! Privileged Broker single-use permit, allowlisted administrative verbs, replay defense,
//! and disconnection reconciliation (`docs/OS_INTEGRATION.md` — "Privileged broker design").

use crate::error::LoraError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// Domain separation prefix for privileged broker single-use permit digests (`docs/OS_INTEGRATION.md`, `R09`).
pub const BROKER_PERMIT_DOMAIN_V1: &[u8] = b"lora.broker.permit.v1\0";

/// Domain separation prefix for privileged broker execution receipt digests (`docs/OS_INTEGRATION.md`, `R09`).
pub const BROKER_RECEIPT_DOMAIN_V1: &[u8] = b"lora.broker.receipt.v1\0";

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn encode_verb_canonical(hasher: &mut Sha256, verb: &BrokerVerb) {
    match verb {
        BrokerVerb::QueryServiceStatus { service_name } => {
            update_len_prefixed(hasher, b"query_service_status");
            update_len_prefixed(hasher, service_name.as_bytes());
        }
        BrokerVerb::RestartAllowlistedService { service_name } => {
            update_len_prefixed(hasher, b"restart_allowlisted_service");
            update_len_prefixed(hasher, service_name.as_bytes());
        }
    }
}

/// Allowlisted typed administrative verbs; arbitrary commands, scripts, or kernel writes are prohibited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerVerb {
    QueryServiceStatus { service_name: String },
    RestartAllowlistedService { service_name: String },
}

/// Actionable broker status codes (`docs/OS_INTEGRATION.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerStatus {
    Completed,
    Unsupported,
    PermissionRequired,
    ScopeChanged,
    ResourceBusy,
    UnknownOutcomeNeedsReconciliation,
}

impl BrokerStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Unsupported => "unsupported",
            Self::PermissionRequired => "permission_required",
            Self::ScopeChanged => "scope_changed",
            Self::ResourceBusy => "resource_busy",
            Self::UnknownOutcomeNeedsReconciliation => "unknown_outcome_needs_reconciliation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerSingleUsePermit {
    pub permit_id: String,
    pub session_id: String,
    pub challenge_nonce: String,
    pub verb: BrokerVerb,
    pub request_digest: String,
    pub expires_at_ms: u64,
}

/// Compute a domain-separated, length-prefixed SHA-256 permit digest (`lora.broker.permit.v1`)
/// over session ID, challenge nonce, and canonical administrative verb.
pub fn compute_broker_permit_digest(
    session_id: &str,
    challenge_nonce: &str,
    verb: &BrokerVerb,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(BROKER_PERMIT_DOMAIN_V1);
    update_len_prefixed(&mut hasher, session_id.as_bytes());
    update_len_prefixed(&mut hasher, challenge_nonce.as_bytes());
    encode_verb_canonical(&mut hasher, verb);
    hex::encode(hasher.finalize())
}

/// Verify that a `BrokerSingleUsePermit` has non-empty identifiers and matches its canonical
/// domain-separated request digest (`lora.broker.permit.v1`).
pub fn verify_broker_permit_digest(permit: &BrokerSingleUsePermit) -> Result<(), LoraError> {
    if permit.permit_id.trim().is_empty()
        || permit.session_id.trim().is_empty()
        || permit.challenge_nonce.trim().is_empty()
    {
        return Err(LoraError::MalformedArtifact(
            "broker permit requires non-empty permit_id, session_id, and challenge_nonce"
                .to_string(),
        ));
    }
    let expected =
        compute_broker_permit_digest(&permit.session_id, &permit.challenge_nonce, &permit.verb);
    if permit.request_digest != expected {
        return Err(LoraError::PolicyDenied(format!(
            "broker permit request_digest mismatch: expected {expected}, got {}",
            permit.request_digest
        )));
    }
    Ok(())
}

impl BrokerSingleUsePermit {
    pub fn compute_request_digest(
        session_id: &str,
        challenge_nonce: &str,
        verb: &BrokerVerb,
    ) -> String {
        compute_broker_permit_digest(session_id, challenge_nonce, verb)
    }
}

/// Attested execution receipt emitted for a privileged broker permit evaluation
/// (`docs/OS_INTEGRATION.md`, `R09`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerExecutionReceipt {
    pub schema_version: u32,
    pub receipt_id: String,
    pub permit_id: String,
    pub session_id: String,
    pub challenge_nonce: String,
    pub verb: BrokerVerb,
    pub request_digest: String,
    pub status: BrokerStatus,
    pub executed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_sha256: Option<String>,
}

/// Compute a domain-separated SHA-256 attestation digest (`lora.broker.receipt.v1`)
/// over a `BrokerExecutionReceipt`.
pub fn compute_broker_receipt_digest(receipt: &BrokerExecutionReceipt) -> String {
    let mut hasher = Sha256::new();
    hasher.update(BROKER_RECEIPT_DOMAIN_V1);
    hasher.update(receipt.schema_version.to_le_bytes());
    update_len_prefixed(&mut hasher, receipt.receipt_id.as_bytes());
    update_len_prefixed(&mut hasher, receipt.permit_id.as_bytes());
    update_len_prefixed(&mut hasher, receipt.session_id.as_bytes());
    update_len_prefixed(&mut hasher, receipt.challenge_nonce.as_bytes());
    encode_verb_canonical(&mut hasher, &receipt.verb);
    update_len_prefixed(&mut hasher, receipt.request_digest.as_bytes());
    update_len_prefixed(&mut hasher, receipt.status.as_str().as_bytes());
    hasher.update(receipt.executed_at_ms.to_le_bytes());
    hex::encode(hasher.finalize())
}

/// Verify self-contained cryptographic integrity and permit consistency of a `BrokerExecutionReceipt`.
pub fn verify_broker_receipt(receipt: &BrokerExecutionReceipt) -> Result<(), LoraError> {
    if receipt.schema_version != 1 {
        return Err(LoraError::MalformedArtifact(format!(
            "unsupported broker receipt schema_version {}",
            receipt.schema_version
        )));
    }
    if receipt.receipt_id.trim().is_empty()
        || receipt.permit_id.trim().is_empty()
        || receipt.session_id.trim().is_empty()
        || receipt.challenge_nonce.trim().is_empty()
    {
        return Err(LoraError::MalformedArtifact(
            "broker execution receipt requires non-empty receipt_id, permit_id, session_id, and challenge_nonce"
                .to_string(),
        ));
    }
    let expected_request =
        compute_broker_permit_digest(&receipt.session_id, &receipt.challenge_nonce, &receipt.verb);
    if receipt.request_digest != expected_request {
        return Err(LoraError::PolicyDenied(format!(
            "broker execution receipt bound request_digest mismatch: expected {expected_request}, got {}",
            receipt.request_digest
        )));
    }
    let expected_receipt = compute_broker_receipt_digest(receipt);
    match &receipt.receipt_sha256 {
        Some(actual) if actual == &expected_receipt => Ok(()),
        Some(actual) => Err(LoraError::PolicyDenied(format!(
            "broker execution receipt digest mismatch: expected {expected_receipt}, got {actual}"
        ))),
        None => Err(LoraError::MalformedArtifact(
            "broker execution receipt missing cryptographic digest".to_string(),
        )),
    }
}

/// Evaluates single-use broker permits, enforces service allowlists, prevents replay,
/// and blocks automatic retry after an indeterminate disconnection until resource reconciliation (`docs/OS_INTEGRATION.md`).
pub struct PrivilegedBrokerGate {
    allowlisted_services: HashSet<String>,
    consumed_nonces: HashSet<String>,
    indeterminate_resources: HashMap<String, String>,
}

impl PrivilegedBrokerGate {
    pub fn new(allowlisted_services: Vec<String>) -> Self {
        Self {
            allowlisted_services: allowlisted_services.into_iter().collect(),
            consumed_nonces: HashSet::new(),
            indeterminate_resources: HashMap::new(),
        }
    }

    pub fn allow_service(&mut self, service: impl Into<String>) {
        self.allowlisted_services.insert(service.into());
    }

    pub fn validate_and_consume_permit(
        &mut self,
        permit: &BrokerSingleUsePermit,
        current_time_ms: u64,
    ) -> Result<BrokerStatus, LoraError> {
        if current_time_ms > permit.expires_at_ms {
            return Err(LoraError::GrantRevokedOrExpired);
        }
        if self.consumed_nonces.contains(&permit.challenge_nonce) {
            return Err(LoraError::PolicyDenied(format!(
                "broker replay detected: single-use permit nonce '{}' was already consumed",
                permit.challenge_nonce
            )));
        }

        verify_broker_permit_digest(permit)?;

        let service_name = match &permit.verb {
            BrokerVerb::QueryServiceStatus { service_name } => service_name,
            BrokerVerb::RestartAllowlistedService { service_name } => service_name,
        };

        if !self.allowlisted_services.contains(service_name) {
            return Err(LoraError::PolicyDenied(format!(
                "service '{service_name}' is not in the broker allowlist"
            )));
        }

        if self.indeterminate_resources.contains_key(service_name) {
            return Ok(BrokerStatus::UnknownOutcomeNeedsReconciliation);
        }

        self.consumed_nonces.insert(permit.challenge_nonce.clone());
        Ok(BrokerStatus::Completed)
    }

    /// Evaluate and consume a single-use permit, emitting an attested `BrokerExecutionReceipt`.
    pub fn execute_and_attest_permit(
        &mut self,
        permit: &BrokerSingleUsePermit,
        current_time_ms: u64,
    ) -> Result<BrokerExecutionReceipt, LoraError> {
        let status = self.validate_and_consume_permit(permit, current_time_ms)?;
        let mut receipt = BrokerExecutionReceipt {
            schema_version: 1,
            receipt_id: format!("brk-rcpt-{}", permit.permit_id),
            permit_id: permit.permit_id.clone(),
            session_id: permit.session_id.clone(),
            challenge_nonce: permit.challenge_nonce.clone(),
            verb: permit.verb.clone(),
            request_digest: permit.request_digest.clone(),
            status,
            executed_at_ms: current_time_ms,
            receipt_sha256: None,
        };
        receipt.receipt_sha256 = Some(compute_broker_receipt_digest(&receipt));
        Ok(receipt)
    }

    /// Record an interrupted/disconnected broker operation so it is never blindly repeated
    /// before explicit reconciliation by resource identity (`docs/OS_INTEGRATION.md`).
    pub fn mark_indeterminate_disconnection(&mut self, service_name: &str, permit_id: &str) {
        self.indeterminate_resources
            .insert(service_name.to_string(), permit_id.to_string());
    }

    pub fn reconcile_resource(&mut self, service_name: &str) {
        self.indeterminate_resources.remove(service_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broker_permit_domain_separation_and_collision_resistance() {
        let verb = BrokerVerb::RestartAllowlistedService {
            service_name: "lora-local-service".to_string(),
        };
        // Previously session="a", nonce="b:c" collided with session="a:b", nonce="c"
        let d1 = compute_broker_permit_digest("a", "b:c", &verb);
        let d2 = compute_broker_permit_digest("a:b", "c", &verb);
        assert_ne!(d1, d2);
        assert_eq!(d1.len(), 64);

        let verb_query = BrokerVerb::QueryServiceStatus {
            service_name: "lora-local-service".to_string(),
        };
        let d3 = compute_broker_permit_digest("a", "b:c", &verb_query);
        assert_ne!(d1, d3);
        assert_eq!(BROKER_PERMIT_DOMAIN_V1, b"lora.broker.permit.v1\0");
        assert_eq!(BROKER_RECEIPT_DOMAIN_V1, b"lora.broker.receipt.v1\0");
    }

    #[test]
    fn test_broker_execution_receipt_attestation_and_tamper_detection() {
        let mut gate = PrivilegedBrokerGate::new(vec!["lora-local-service".to_string()]);
        let verb = BrokerVerb::RestartAllowlistedService {
            service_name: "lora-local-service".to_string(),
        };
        let permit = BrokerSingleUsePermit {
            permit_id: "p-attest-1".to_string(),
            session_id: "sess-attest".to_string(),
            challenge_nonce: "nonce-attest-01".to_string(),
            verb: verb.clone(),
            request_digest: BrokerSingleUsePermit::compute_request_digest(
                "sess-attest",
                "nonce-attest-01",
                &verb,
            ),
            expires_at_ms: 100_000,
        };
        assert!(verify_broker_permit_digest(&permit).is_ok());

        let receipt = gate.execute_and_attest_permit(&permit, 50_000).unwrap();
        assert_eq!(receipt.status, BrokerStatus::Completed);
        assert!(verify_broker_receipt(&receipt).is_ok());

        // Tampered status fails closed
        let mut bad_status = receipt.clone();
        bad_status.status = BrokerStatus::PermissionRequired;
        assert!(verify_broker_receipt(&bad_status).is_err());

        // Tampered verb / service name fails closed
        let mut bad_verb = receipt.clone();
        bad_verb.verb = BrokerVerb::RestartAllowlistedService {
            service_name: "evil-service".to_string(),
        };
        assert!(verify_broker_receipt(&bad_verb).is_err());

        // Tampered timestamp fails closed
        let mut bad_time = receipt.clone();
        bad_time.executed_at_ms = 99_999;
        assert!(verify_broker_receipt(&bad_time).is_err());

        // Missing digest fails closed
        let mut missing_digest = receipt.clone();
        missing_digest.receipt_sha256 = None;
        assert!(verify_broker_receipt(&missing_digest).is_err());
    }
}
