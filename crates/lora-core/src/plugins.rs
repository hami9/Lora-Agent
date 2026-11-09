//! Extension and plugin manifest verification, dependency cycle detection,
//! and lifecycle state machine (`docs/PLUGIN_SYSTEM.md`, Phase 6).

use crate::error::LoraError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};

pub const SUPPORTED_EXTENSION_HOST_MAJOR: u16 = 1;
pub const MAX_CONSECUTIVE_FAULTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState {
    Staged,
    Verified,
    Rejected,
    Disabled,
    Active,
    Quarantined,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostApiVersion {
    pub major: u16,
    pub minimum_minor: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifestV1 {
    pub manifest_version: u16,
    pub id: String,
    pub version: String,
    pub kind: String,
    pub host_api: HostApiVersion,
    pub entry: String,
    pub exports: Vec<String>,
    pub requests: Vec<String>,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledExtensionRecord {
    pub manifest: ExtensionManifestV1,
    pub package_sha256: String,
    pub state: ExtensionState,
    pub approved_capabilities: BTreeSet<String>,
    pub consecutive_faults: u32,
    pub previous_compatible_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
}

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Compute a deterministic, domain-separated SHA-256 digest (`lora.extension.manifest.v1`)
/// over an `ExtensionManifestV1`.
pub fn compute_extension_manifest_digest(manifest: &ExtensionManifestV1) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lora.extension.manifest.v1\0");
    hasher.update(manifest.manifest_version.to_le_bytes());
    update_len_prefixed(&mut hasher, manifest.id.as_bytes());
    update_len_prefixed(&mut hasher, manifest.version.as_bytes());
    update_len_prefixed(&mut hasher, manifest.kind.as_bytes());
    hasher.update(manifest.host_api.major.to_le_bytes());
    hasher.update(manifest.host_api.minimum_minor.to_le_bytes());
    update_len_prefixed(&mut hasher, manifest.entry.as_bytes());
    hasher.update((manifest.exports.len() as u64).to_le_bytes());
    for exp in &manifest.exports {
        update_len_prefixed(&mut hasher, exp.as_bytes());
    }
    hasher.update((manifest.requests.len() as u64).to_le_bytes());
    for req in &manifest.requests {
        update_len_prefixed(&mut hasher, req.as_bytes());
    }
    hasher.update((manifest.dependencies.len() as u64).to_le_bytes());
    for dep in &manifest.dependencies {
        update_len_prefixed(&mut hasher, dep.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Validate an extension manifest against `docs/PLUGIN_SYSTEM.md` security rules:
/// - `manifest_version == 1`
/// - `host_api.major == 1`
/// - `kind` in `["wasm-component", "mcp-stdio", "declarative-skill"]`
/// - Entry path must be relative with no `..`, `/`, `\`, or `:` traversal
/// - Rejects ambient host capabilities (`ambient.fs`, `ambient.net`, `ambient.shell`, `ambient.env`)
pub fn validate_extension_manifest(manifest: &ExtensionManifestV1) -> Result<(), LoraError> {
    if manifest.manifest_version != 1 {
        return Err(LoraError::IncompatibleModel(format!(
            "unsupported extension manifest_version {}",
            manifest.manifest_version
        )));
    }
    if manifest.host_api.major != SUPPORTED_EXTENSION_HOST_MAJOR {
        return Err(LoraError::IncompatibleModel(format!(
            "unsupported extension host_api.major {} (required {})",
            manifest.host_api.major, SUPPORTED_EXTENSION_HOST_MAJOR
        )));
    }
    let trimmed_id = manifest.id.trim();
    if trimmed_id.is_empty() || manifest.id.len() > 128 || trimmed_id != manifest.id {
        return Err(LoraError::MalformedArtifact(
            "extension id must be 1..=128 non-padded characters".to_string(),
        ));
    }
    if !manifest
        .id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(LoraError::MalformedArtifact(format!(
            "invalid characters in extension id '{}' (allowed: [a-zA-Z0-9._-])",
            manifest.id
        )));
    }
    if !matches!(
        manifest.kind.as_str(),
        "wasm-component" | "mcp-stdio" | "declarative-skill"
    ) {
        return Err(LoraError::IncompatibleModel(format!(
            "unsupported extension kind '{}'",
            manifest.kind
        )));
    }
    crate::agent::validate_workflow_relative_path(&manifest.entry).map_err(|reason| {
        LoraError::ScopeEscapeAttempt(format!(
            "unsafe extension entry path '{}': {reason}",
            manifest.entry
        ))
    })?;
    for req in &manifest.requests {
        if req.starts_with("ambient.") || req == "shell.exec" || req == "host.credentials" {
            return Err(LoraError::PolicyDenied(format!(
                "extension requested prohibited ambient capability '{req}'"
            )));
        }
    }
    Ok(())
}

/// Detect dependency cycles across a set of extension manifests (`docs/PLUGIN_SYSTEM.md`).
pub fn verify_no_dependency_cycles(manifests: &[ExtensionManifestV1]) -> Result<(), LoraError> {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for m in manifests {
        adj.insert(
            m.id.as_str(),
            m.dependencies.iter().map(|s| s.as_str()).collect(),
        );
    }

    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();

    fn dfs<'a>(
        node: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        visiting: &mut HashSet<&'a str>,
        visited: &mut HashSet<&'a str>,
    ) -> bool {
        if visiting.contains(node) {
            return true; // cycle detected
        }
        if visited.contains(node) {
            return false;
        }
        visiting.insert(node);
        if let Some(neighbors) = adj.get(node) {
            for &next in neighbors {
                if dfs(next, adj, visiting, visited) {
                    return true;
                }
            }
        }
        visiting.remove(node);
        visited.insert(node);
        false
    }

    for m in manifests {
        if dfs(m.id.as_str(), &adj, &mut visiting, &mut visited) {
            return Err(LoraError::MalformedArtifact(format!(
                "dependency cycle detected involving extension '{}'",
                m.id
            )));
        }
    }
    Ok(())
}

/// Manages extension staging, verification, capability negotiation, fault quarantine, and rollback.
#[derive(Default)]
pub struct ExtensionHostManager {
    records: HashMap<String, InstalledExtensionRecord>,
}

impl ExtensionHostManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage and verify a candidate extension package against its outer expected SHA-256 digest.
    /// Follows `[*] -> Staged -> Verified -> Disabled` (or `Staged -> Rejected`).
    pub fn stage_and_verify(
        &mut self,
        manifest: ExtensionManifestV1,
        actual_sha256: &str,
        expected_outer_sha256: &str,
    ) -> Result<ExtensionState, LoraError> {
        if !actual_sha256.eq_ignore_ascii_case(expected_outer_sha256) {
            return Err(LoraError::DigestMismatch {
                expected: expected_outer_sha256.to_string(),
                actual: actual_sha256.to_string(),
            });
        }

        validate_extension_manifest(&manifest)?;

        // Check for dependency cycles with existing installed extensions
        let mut all_manifests: Vec<ExtensionManifestV1> =
            self.records.values().map(|r| r.manifest.clone()).collect();
        all_manifests.retain(|m| m.id != manifest.id);
        all_manifests.push(manifest.clone());
        verify_no_dependency_cycles(&all_manifests)?;

        let prev_digest = self
            .records
            .get(&manifest.id)
            .map(|old| old.package_sha256.clone());

        let manifest_digest = compute_extension_manifest_digest(&manifest);
        let id = manifest.id.clone();
        self.records.insert(
            id,
            InstalledExtensionRecord {
                manifest,
                package_sha256: actual_sha256.to_string(),
                state: ExtensionState::Disabled,
                approved_capabilities: BTreeSet::new(),
                consecutive_faults: 0,
                previous_compatible_digest: prev_digest,
                manifest_sha256: Some(manifest_digest),
            },
        );

        Ok(ExtensionState::Disabled)
    }

    /// Compute the permission delta (new capabilities requested by an update that were not previously granted).
    pub fn permission_diff(&self, id: &str) -> Result<Vec<String>, LoraError> {
        let rec = self.records.get(id).ok_or_else(|| {
            LoraError::IncompatibleModel(format!("extension '{id}' is not installed"))
        })?;
        let mut added = Vec::new();
        for req in &rec.manifest.requests {
            if !rec.approved_capabilities.contains(req) {
                added.push(req.clone());
            }
        }
        Ok(added)
    }

    /// Authorize requested capabilities and transition `Disabled -> Active`.
    /// Quarantined extensions require explicit `review_and_unquarantine` before activation.
    pub fn activate(
        &mut self,
        id: &str,
        granted_capabilities: &[String],
    ) -> Result<ExtensionState, LoraError> {
        let rec = self.records.get_mut(id).ok_or_else(|| {
            LoraError::IncompatibleModel(format!("extension '{id}' is not installed"))
        })?;

        if rec.state == ExtensionState::Quarantined {
            return Err(LoraError::PolicyDenied(format!(
                "extension '{id}' is Quarantined after repeated faults and requires explicit review before activation"
            )));
        }

        let granted_set: BTreeSet<String> = granted_capabilities.iter().cloned().collect();
        for req in &rec.manifest.requests {
            if !granted_set.contains(req) {
                return Err(LoraError::PolicyDenied(format!(
                    "extension '{id}' requires ungranted capability '{req}'"
                )));
            }
        }

        rec.approved_capabilities = granted_set;
        rec.state = ExtensionState::Active;
        Ok(rec.state)
    }

    /// Record a runtime fault; transitions `Active -> Quarantined` when consecutive faults reach `MAX_CONSECUTIVE_FAULTS`.
    pub fn record_fault(&mut self, id: &str) -> Result<ExtensionState, LoraError> {
        let rec = self.records.get_mut(id).ok_or_else(|| {
            LoraError::IncompatibleModel(format!("extension '{id}' is not installed"))
        })?;
        rec.consecutive_faults += 1;
        if rec.consecutive_faults >= MAX_CONSECUTIVE_FAULTS {
            rec.state = ExtensionState::Quarantined;
            rec.approved_capabilities.clear();
        }
        Ok(rec.state)
    }

    /// Revoke permissions and transition `Active -> Disabled`.
    pub fn disable_and_revoke(&mut self, id: &str) -> Result<ExtensionState, LoraError> {
        let rec = self.records.get_mut(id).ok_or_else(|| {
            LoraError::IncompatibleModel(format!("extension '{id}' is not installed"))
        })?;
        rec.approved_capabilities.clear();
        rec.state = ExtensionState::Disabled;
        Ok(rec.state)
    }

    /// Roll back to the retained previous compatible digest (`docs/PLUGIN_SYSTEM.md`).
    pub fn rollback(&mut self, id: &str) -> Result<String, LoraError> {
        let rec = self.records.get_mut(id).ok_or_else(|| {
            LoraError::IncompatibleModel(format!("extension '{id}' is not installed"))
        })?;
        let Some(prev) = rec.previous_compatible_digest.take() else {
            return Err(LoraError::IncompatibleModel(format!(
                "no previous compatible digest available for rollback of '{id}'"
            )));
        };
        rec.package_sha256 = prev.clone();
        rec.state = ExtensionState::Disabled;
        rec.consecutive_faults = 0;
        rec.approved_capabilities.clear();
        Ok(prev)
    }

    pub fn get(&self, id: &str) -> Option<&InstalledExtensionRecord> {
        self.records.get(id)
    }

    pub fn list(&self) -> Vec<InstalledExtensionRecord> {
        let mut out: Vec<InstalledExtensionRecord> = self.records.values().cloned().collect();
        out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        out
    }
}

impl crate::store::SqliteStore {
    fn ensure_extensions_table(&self) -> Result<(), LoraError> {
        let conn = self.conn_lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS installed_extensions (
                extension_id TEXT PRIMARY KEY,
                record_json TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    pub fn load_extension_host(&self) -> Result<ExtensionHostManager, LoraError> {
        self.ensure_extensions_table()?;
        let conn = self.conn_lock();
        let mut stmt =
            conn.prepare("SELECT record_json FROM installed_extensions ORDER BY extension_id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut host = ExtensionHostManager::new();
        for r in rows {
            let json = r?;
            if let Ok(rec) = serde_json::from_str::<InstalledExtensionRecord>(&json) {
                host.records.insert(rec.manifest.id.clone(), rec);
            }
        }
        Ok(host)
    }

    pub fn save_extension_host(&self, host: &ExtensionHostManager) -> Result<(), LoraError> {
        self.ensure_extensions_table()?;
        let conn = self.conn_lock();
        conn.execute("DELETE FROM installed_extensions", [])?;
        for rec in host.records.values() {
            let json =
                serde_json::to_string(rec).map_err(|e| LoraError::Database(e.to_string()))?;
            conn.execute(
                "INSERT INTO installed_extensions (extension_id, record_json) VALUES (?1, ?2)",
                rusqlite::params![rec.manifest.id, json],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_extension_manifest_path_and_id_containment() {
        let mut manifest = ExtensionManifestV1 {
            manifest_version: 1,
            id: "org.example.good".to_string(),
            version: "1.0.0".to_string(),
            kind: "wasm-component".to_string(),
            host_api: HostApiVersion {
                major: 1,
                minimum_minor: 0,
            },
            entry: "component.wasm".to_string(),
            exports: vec!["tool.wordcount.v1".to_string()],
            requests: vec!["input.text".to_string()],
            dependencies: vec![],
        };
        assert!(validate_extension_manifest(&manifest).is_ok());

        // Reserved Windows device name in entry fails closed
        manifest.entry = "CON".to_string();
        assert!(matches!(
            validate_extension_manifest(&manifest),
            Err(LoraError::ScopeEscapeAttempt(_))
        ));

        // Directory traversal in entry fails closed
        manifest.entry = "../component.wasm".to_string();
        assert!(matches!(
            validate_extension_manifest(&manifest),
            Err(LoraError::ScopeEscapeAttempt(_))
        ));

        // Invalid characters in ID fails closed
        manifest.entry = "component.wasm".to_string();
        manifest.id = "bad id with spaces".to_string();
        assert!(matches!(
            validate_extension_manifest(&manifest),
            Err(LoraError::MalformedArtifact(_))
        ));
    }

    #[test]
    fn test_compute_extension_manifest_digest_deterministic() {
        let manifest = ExtensionManifestV1 {
            manifest_version: 1,
            id: "org.example.test".to_string(),
            version: "1.0.0".to_string(),
            kind: "wasm-component".to_string(),
            host_api: HostApiVersion {
                major: 1,
                minimum_minor: 0,
            },
            entry: "component.wasm".to_string(),
            exports: vec!["tool.test.v1".to_string()],
            requests: vec![],
            dependencies: vec![],
        };
        let d1 = compute_extension_manifest_digest(&manifest);
        let d2 = compute_extension_manifest_digest(&manifest);
        assert_eq!(d1, d2);
        assert_eq!(d1.len(), 64);
    }
}
