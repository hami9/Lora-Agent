//! Privileged Broker single-use permit, allowlisted administrative verbs, replay defense,
//! and disconnection reconciliation (`docs/OS_INTEGRATION.md` — "Privileged broker design").

use crate::error::LoraError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerSingleUsePermit {
    pub permit_id: String,
    pub session_id: String,
    pub challenge_nonce: String,
    pub verb: BrokerVerb,
    pub request_digest: String,
    pub expires_at_ms: u64,
}

impl BrokerSingleUsePermit {
    pub fn compute_request_digest(
        session_id: &str,
        challenge_nonce: &str,
        verb: &BrokerVerb,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(session_id.as_bytes());
        hasher.update(b":");
        hasher.update(challenge_nonce.as_bytes());
        hasher.update(b":");
        let verb_json = serde_json::to_string(verb).unwrap_or_default();
        hasher.update(verb_json.as_bytes());
        hex::encode(hasher.finalize())
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

        let expected_digest = BrokerSingleUsePermit::compute_request_digest(
            &permit.session_id,
            &permit.challenge_nonce,
            &permit.verb,
        );
        if permit.request_digest != expected_digest {
            return Err(LoraError::PolicyDenied(
                "broker permit request_digest mismatch".to_string(),
            ));
        }

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
