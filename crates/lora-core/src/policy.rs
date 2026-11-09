//! Capability grants, policy evaluation, secret-path exclusion, and revocation epochs
//! (`docs/SECURITY_ARCHITECTURE.md`, `ADR-0007`, `M6`).

use crate::error::LoraError;
use crate::ports::ScopedRootHandle;
use crate::store::now_epoch_ms;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const POLICY_REVISION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    ChatOnly,
    ReadOnlyAssistant,
}

impl ExecutionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ChatOnly => "chat_only",
            Self::ReadOnlyAssistant => "read_only_assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGrant {
    pub grant_id: String,
    pub subject: String,
    pub run_id: Option<String>,
    pub tool_id: String,
    pub tool_version: u32,
    pub operation: String,
    pub root: ScopedRootHandle,
    pub mode: ExecutionMode,
    pub max_bytes: usize,
    pub expires_at_ms: u64,
    pub revocation_epoch: u64,
    pub one_time: bool,
    pub consumed: bool,
}

/// Domain separation prefix for ExecutionPermit action digests (`docs/SECURITY_ARCHITECTURE.md`).
pub const ACTION_DIGEST_DOMAIN_V1: &[u8] = b"lora.action.digest.v1\0";

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Short-lived execution permit passed only to the trusted tool executor (`docs/AGENT_RUNTIME.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPermit {
    pub grant_id: String,
    pub principal: String,
    pub run_id: String,
    pub tool_id: String,
    pub tool_version: u32,
    pub action_digest: String,
    pub root: ScopedRootHandle,
    pub relative_path: String,
    pub max_bytes: usize,
    pub issued_epoch: u64,
    pub expires_at_ms: u64,
}

/// Compute a domain-separated canonical SHA-256 digest over typed tool arguments and resolved scope identity (`lora.action.digest.v1`).
pub fn compute_action_digest(
    tool_id: &str,
    tool_version: u32,
    root_id: &str,
    canonical_root: &str,
    relative_path: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ACTION_DIGEST_DOMAIN_V1);
    update_len_prefixed(&mut hasher, tool_id.as_bytes());
    hasher.update(tool_version.to_le_bytes());
    update_len_prefixed(&mut hasher, root_id.as_bytes());
    update_len_prefixed(&mut hasher, canonical_root.as_bytes());
    update_len_prefixed(&mut hasher, relative_path.as_bytes());
    hex::encode(hasher.finalize())
}

/// Reject secret-bearing filenames excluded by default policy (`docs/SECURITY_ARCHITECTURE.md`).
pub fn is_excluded_secret_resource(relative_path: &str) -> bool {
    let lower = relative_path.replace('\\', "/").to_ascii_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);
    filename == ".env"
        || filename.starts_with(".env.")
        || filename.ends_with(".pem")
        || filename.ends_with(".key")
        || filename.ends_with(".pfx")
        || filename == "id_rsa"
        || filename == "id_ed25519"
        || filename.contains("credentials")
        || filename.contains("secret")
}

#[derive(Clone)]
pub struct PolicyGate {
    roots: Arc<Mutex<HashMap<String, ScopedRootHandle>>>,
    grants: Arc<Mutex<HashMap<String, SessionGrant>>>,
    revocation_epoch: Arc<AtomicU64>,
}

impl Default for PolicyGate {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyGate {
    pub fn new() -> Self {
        Self {
            roots: Arc::new(Mutex::new(HashMap::new())),
            grants: Arc::new(Mutex::new(HashMap::new())),
            revocation_epoch: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn current_epoch(&self) -> u64 {
        self.revocation_epoch.load(Ordering::SeqCst)
    }

    /// Register a trusted directory handle under its opaque `root_id`.
    pub fn register_scoped_root(&self, root: ScopedRootHandle) {
        self.roots
            .lock()
            .expect("roots mutex poisoned")
            .insert(root.root_id.clone(), root);
    }

    pub fn resolve_root(&self, root_id: &str) -> Option<ScopedRootHandle> {
        self.roots
            .lock()
            .expect("roots mutex poisoned")
            .get(root_id)
            .cloned()
    }

    /// Issue a trusted human-approved grant. Untrusted model strings can never call this method.
    pub fn issue_grant(
        &self,
        subject: &str,
        run_id: Option<String>,
        root: ScopedRootHandle,
        max_bytes: usize,
        ttl_ms: u64,
        one_time: bool,
    ) -> SessionGrant {
        let epoch = self.current_epoch();
        let now = now_epoch_ms();
        let grant_id = format!("grant-{}-{}-{}", root.root_id, epoch, now);
        let grant = SessionGrant {
            grant_id: grant_id.clone(),
            subject: subject.to_string(),
            run_id,
            tool_id: "read_file".to_string(),
            tool_version: 1,
            operation: "read".to_string(),
            root: root.clone(),
            mode: ExecutionMode::ReadOnlyAssistant,
            max_bytes,
            expires_at_ms: now.saturating_add(ttl_ms),
            revocation_epoch: epoch,
            one_time,
            consumed: false,
        };
        self.register_scoped_root(root);
        self.grants
            .lock()
            .expect("grants mutex poisoned")
            .insert(grant_id, grant.clone());
        grant
    }

    /// Revoke all grants or a specific grant and advance the global revocation epoch (`M6`).
    pub fn revoke_all_grants(&self) -> u64 {
        self.grants.lock().expect("grants mutex poisoned").clear();
        self.roots.lock().expect("roots mutex poisoned").clear();
        self.revocation_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn revoke_grant(&self, grant_id: &str) -> u64 {
        self.grants
            .lock()
            .expect("grants mutex poisoned")
            .remove(grant_id);
        self.revocation_epoch.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Check if an active, unrevoked, non-expired grant covers this `read_file` proposal,
    /// consuming one-time grants atomically before dispatch.
    #[allow(clippy::too_many_arguments)]
    pub fn try_acquire_permit(
        &self,
        principal: &str,
        run_id: &str,
        mode: ExecutionMode,
        tool_id: &str,
        tool_version: u32,
        root_id: &str,
        relative_path: &str,
        max_bytes: usize,
    ) -> Result<Option<ExecutionPermit>, LoraError> {
        if mode != ExecutionMode::ReadOnlyAssistant {
            return Err(LoraError::PolicyDenied(
                "tools are disabled in ChatOnly mode".to_string(),
            ));
        }

        if tool_id != "read_file" || tool_version != 1 {
            return Err(LoraError::PolicyDenied(format!(
                "unrecognized or unauthorized tool '{tool_id}@v{tool_version}' (MVP permits only 'read_file@v1')"
            )));
        }

        if is_excluded_secret_resource(relative_path) {
            return Err(LoraError::PolicyDenied(format!(
                "resource '{relative_path}' is excluded by secret-protection policy"
            )));
        }

        let Some(root) = self.resolve_root(root_id) else {
            return Err(LoraError::ScopeEscapeAttempt(format!(
                "unknown or unapproved root_id '{root_id}'"
            )));
        };

        let now = now_epoch_ms();
        let current_epoch = self.current_epoch();
        let mut grants = self.grants.lock().expect("grants mutex poisoned");

        // Find matching grant
        let matching_id = grants
            .iter()
            .find(|(_, g)| {
                g.subject == principal
                    && g.root.root_id == root_id
                    && g.tool_id == tool_id
                    && g.tool_version == tool_version
                    && (g.run_id.is_none() || g.run_id.as_deref() == Some(run_id))
            })
            .map(|(id, _)| id.clone());

        let Some(gid) = matching_id else {
            return Ok(None);
        };

        let grant = grants.get_mut(&gid).expect("grant exists");
        if grant.consumed || now > grant.expires_at_ms || grant.revocation_epoch != current_epoch {
            grants.remove(&gid);
            return Err(LoraError::GrantRevokedOrExpired);
        }

        if grant.one_time {
            grant.consumed = true;
        }

        let effective_max_bytes = max_bytes.min(grant.max_bytes);
        let action_digest = compute_action_digest(
            tool_id,
            tool_version,
            root_id,
            &root.canonical_root.to_string_lossy(),
            relative_path,
        );

        Ok(Some(ExecutionPermit {
            grant_id: gid,
            principal: principal.to_string(),
            run_id: run_id.to_string(),
            tool_id: tool_id.to_string(),
            tool_version,
            action_digest,
            root,
            relative_path: relative_path.to_string(),
            max_bytes: effective_max_bytes,
            issued_epoch: current_epoch,
            expires_at_ms: grant.expires_at_ms,
        }))
    }

    /// List all currently registered session grants sorted by grant ID.
    pub fn list_grants(&self) -> Vec<SessionGrant> {
        let grants = self.grants.lock().expect("grants mutex poisoned");
        let mut list: Vec<SessionGrant> = grants.values().cloned().collect();
        list.sort_by(|a, b| a.grant_id.cmp(&b.grant_id));
        list
    }

    /// Lookup a specific session grant by ID.
    pub fn get_grant(&self, grant_id: &str) -> Option<SessionGrant> {
        self.grants
            .lock()
            .expect("grants mutex poisoned")
            .get(grant_id)
            .cloned()
    }

    /// Recheck permit validity at dispatch and right before delivering read bytes (`docs/AGENT_RUNTIME.md`).
    /// Strictly verifies revocation epoch, expiration, issuing grant binding, and cryptographic action digest.
    pub fn verify_permit_live(&self, permit: &ExecutionPermit) -> Result<(), LoraError> {
        let now = now_epoch_ms();
        if now > permit.expires_at_ms || self.current_epoch() != permit.issued_epoch {
            return Err(LoraError::GrantRevokedOrExpired);
        }

        // Verify cryptographic integrity of the permit's action digest
        let expected_digest = compute_action_digest(
            &permit.tool_id,
            permit.tool_version,
            &permit.root.root_id,
            &permit.root.canonical_root.to_string_lossy(),
            &permit.relative_path,
        );
        if permit.action_digest != expected_digest {
            return Err(LoraError::PolicyDenied(format!(
                "execution permit action digest mismatch (expected {expected_digest}, got {})",
                permit.action_digest
            )));
        }

        let grants = self.grants.lock().expect("grants mutex poisoned");
        let Some(grant) = grants.get(&permit.grant_id) else {
            return Err(LoraError::GrantRevokedOrExpired);
        };

        // Fail-closed verification against issuing grant binding
        if grant.subject != permit.principal
            || grant.tool_id != permit.tool_id
            || grant.tool_version != permit.tool_version
            || grant.root.root_id != permit.root.root_id
            || (grant.run_id.is_some() && grant.run_id.as_deref() != Some(&permit.run_id))
        {
            return Err(LoraError::PolicyDenied(
                "execution permit fields do not match issuing grant binding".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mock_root(root_id: &str) -> ScopedRootHandle {
        ScopedRootHandle {
            root_id: root_id.to_string(),
            canonical_root: PathBuf::from(if cfg!(windows) {
                r"C:\safe\workspace"
            } else {
                "/safe/workspace"
            }),
            device_or_volume_id: 1,
        }
    }

    #[test]
    fn test_compute_action_digest_domain_separated() {
        let d1 = compute_action_digest("read_file", 1, "root_1", "/safe/workspace", "doc.txt");
        let d2 = compute_action_digest("read_file", 1, "root_1", "/safe/workspace", "doc.txt");
        assert_eq!(d1, d2);
        assert_eq!(d1.len(), 64);

        // Different relative path produces distinct digest
        let d3 = compute_action_digest("read_file", 1, "root_1", "/safe/workspace", "other.txt");
        assert_ne!(d1, d3);

        // Length prefixing prevents delimiter collision ambiguity
        let da = compute_action_digest("read_file", 1, "root", "a:b", "c");
        let db = compute_action_digest("read_file", 1, "root:a", "b", "c");
        assert_ne!(da, db);
    }

    #[test]
    fn test_secret_resource_exclusion() {
        assert!(is_excluded_secret_resource(".env"));
        assert!(is_excluded_secret_resource(".env.production"));
        assert!(is_excluded_secret_resource("config/server.key"));
        assert!(is_excluded_secret_resource("certs/cert.pem"));
        assert!(is_excluded_secret_resource("id_rsa"));
        assert!(is_excluded_secret_resource("id_ed25519"));
        assert!(is_excluded_secret_resource("aws_credentials.json"));
        assert!(!is_excluded_secret_resource("README.md"));
        assert!(!is_excluded_secret_resource("src/main.rs"));
    }

    #[test]
    fn test_permit_acquisition_and_live_verification() {
        let gate = PolicyGate::new();
        let root = mock_root("workspace_01");

        let grant = gate.issue_grant(
            "operator",
            Some("run-101".to_string()),
            root.clone(),
            64 * 1024,
            60_000,
            false,
        );
        assert_eq!(gate.list_grants().len(), 1);
        assert_eq!(gate.get_grant(&grant.grant_id).unwrap().subject, "operator");

        let permit = gate
            .try_acquire_permit(
                "operator",
                "run-101",
                ExecutionMode::ReadOnlyAssistant,
                "read_file",
                1,
                &root.root_id,
                "src/lib.rs",
                32 * 1024,
            )
            .expect("should acquire permit")
            .expect("permit must be present");

        assert_eq!(permit.max_bytes, 32 * 1024);
        assert_eq!(permit.tool_id, "read_file");
        assert_eq!(permit.tool_version, 1);
        assert!(gate.verify_permit_live(&permit).is_ok());

        // Tampering relative_path fails cryptographic verification
        let mut tampered = permit.clone();
        tampered.relative_path = "src/tampered.rs".to_string();
        assert!(matches!(
            gate.verify_permit_live(&tampered),
            Err(LoraError::PolicyDenied(_))
        ));

        // Tampering action digest fails verification
        let mut tampered_digest = permit.clone();
        tampered_digest.action_digest = "0".repeat(64);
        assert!(matches!(
            gate.verify_permit_live(&tampered_digest),
            Err(LoraError::PolicyDenied(_))
        ));

        // Tampering principal fails grant binding check
        let mut tampered_principal = permit.clone();
        tampered_principal.principal = "attacker".to_string();
        assert!(matches!(
            gate.verify_permit_live(&tampered_principal),
            Err(LoraError::PolicyDenied(_))
        ));

        // Advancing epoch invalidates permit
        gate.revoke_all_grants();
        assert_eq!(gate.list_grants().len(), 0);
        assert!(matches!(
            gate.verify_permit_live(&permit),
            Err(LoraError::GrantRevokedOrExpired)
        ));
    }
}
