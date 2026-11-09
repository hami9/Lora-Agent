//! Adapter Studio (`docs/TRAINING_SYSTEM.md`, Phase 8):
//! Dataset source-grouped splitting (leakage prevention), plain LoRA job configuration,
//! memory/disk cost estimation, checkpointing, and held-out evaluation promotion gate.

use crate::error::LoraError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Domain separation prefix for dataset split manifest attestation (`docs/TRAINING_SYSTEM.md`, `R11`).
pub const ADAPTER_SPLIT_DOMAIN_V1: &[u8] = b"lora.adapter.split.v1\0";

/// Domain separation prefix for held-out evaluation receipt attestation (`docs/TRAINING_SYSTEM.md`).
pub const ADAPTER_EVAL_DOMAIN_V1: &[u8] = b"lora.adapter.eval.v1\0";

/// Domain separation prefix for adapter artifact cryptographic identifier (`docs/TRAINING_SYSTEM.md`).
pub const ADAPTER_ARTIFACT_DOMAIN_V1: &[u8] = b"lora.adapter.artifact.v1\0";

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Single training example in a consented dataset snapshot (`docs/TRAINING_SYSTEM.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrainingExample {
    pub example_id: String,
    /// Source document or conversation ID used for group-level split before chunking to prevent leakage.
    pub source_group_id: String,
    pub prompt: String,
    pub target_completion: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetSplitManifest {
    pub dataset_sha256: String,
    pub license: String,
    pub train_examples: Vec<TrainingExample>,
    pub validation_examples: Vec<TrainingExample>,
    pub test_examples: Vec<TrainingExample>,
    pub train_groups: BTreeSet<String>,
    pub validation_groups: BTreeSet<String>,
    pub test_groups: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
}

/// Compute a domain-separated canonical SHA-256 digest over a DatasetSplitManifest (`lora.adapter.split.v1`).
pub fn compute_dataset_split_digest(manifest: &DatasetSplitManifest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ADAPTER_SPLIT_DOMAIN_V1);
    update_len_prefixed(&mut hasher, manifest.dataset_sha256.as_bytes());
    update_len_prefixed(&mut hasher, manifest.license.as_bytes());
    hasher.update((manifest.train_examples.len() as u64).to_le_bytes());
    hasher.update((manifest.validation_examples.len() as u64).to_le_bytes());
    hasher.update((manifest.test_examples.len() as u64).to_le_bytes());
    for g in &manifest.train_groups {
        update_len_prefixed(&mut hasher, g.as_bytes());
    }
    for g in &manifest.validation_groups {
        update_len_prefixed(&mut hasher, g.as_bytes());
    }
    for g in &manifest.test_groups {
        update_len_prefixed(&mut hasher, g.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Verify that a DatasetSplitManifest has explicit licensing, valid examples, non-overlapping groups,
/// and matching cryptographic attestation digest.
pub fn verify_dataset_split_manifest(manifest: &DatasetSplitManifest) -> Result<(), LoraError> {
    if manifest.license.trim().is_empty() || manifest.license.eq_ignore_ascii_case("unknown") {
        return Err(LoraError::PolicyDenied(
            "training dataset requires an explicit license declaration".to_string(),
        ));
    }
    if manifest.train_groups.is_empty()
        || manifest.validation_groups.is_empty()
        || manifest.test_groups.is_empty()
    {
        return Err(LoraError::MalformedArtifact(
            "split manifest must contain non-empty train, validation, and test groups".to_string(),
        ));
    }
    if !manifest
        .train_groups
        .is_disjoint(&manifest.validation_groups)
        || !manifest.train_groups.is_disjoint(&manifest.test_groups)
        || !manifest
            .validation_groups
            .is_disjoint(&manifest.test_groups)
    {
        return Err(LoraError::MalformedArtifact(
            "leakage detected: overlapping groups between dataset splits".to_string(),
        ));
    }
    for ex in &manifest.train_examples {
        if !manifest.train_groups.contains(&ex.source_group_id) {
            return Err(LoraError::MalformedArtifact(format!(
                "train example '{}' group '{}' not in train_groups",
                ex.example_id, ex.source_group_id
            )));
        }
        if ex.prompt.trim().is_empty() || ex.target_completion.trim().is_empty() {
            return Err(LoraError::MalformedArtifact(format!(
                "train example '{}' has empty prompt or completion",
                ex.example_id
            )));
        }
    }
    for ex in &manifest.validation_examples {
        if !manifest.validation_groups.contains(&ex.source_group_id) {
            return Err(LoraError::MalformedArtifact(format!(
                "validation example '{}' group '{}' not in validation_groups",
                ex.example_id, ex.source_group_id
            )));
        }
        if ex.prompt.trim().is_empty() || ex.target_completion.trim().is_empty() {
            return Err(LoraError::MalformedArtifact(format!(
                "validation example '{}' has empty prompt or completion",
                ex.example_id
            )));
        }
    }
    for ex in &manifest.test_examples {
        if !manifest.test_groups.contains(&ex.source_group_id) {
            return Err(LoraError::MalformedArtifact(format!(
                "test example '{}' group '{}' not in test_groups",
                ex.example_id, ex.source_group_id
            )));
        }
        if ex.prompt.trim().is_empty() || ex.target_completion.trim().is_empty() {
            return Err(LoraError::MalformedArtifact(format!(
                "test example '{}' has empty prompt or completion",
                ex.example_id
            )));
        }
    }

    if let Some(expected_digest) = &manifest.manifest_sha256 {
        let computed = compute_dataset_split_digest(manifest);
        if computed != *expected_digest {
            return Err(LoraError::PolicyDenied(format!(
                "dataset split manifest digest mismatch: expected {expected_digest}, got {computed}"
            )));
        }
    }
    Ok(())
}

/// Validate and split a dataset by `source_group_id` before chunking so no source conversation/document
/// spans across train, validation, and held-out test splits (`docs/TRAINING_SYSTEM.md`, `R11`).
pub fn prepare_leakage_free_splits(
    examples: &[TrainingExample],
    license: &str,
) -> Result<DatasetSplitManifest, LoraError> {
    if license.trim().is_empty() || license.eq_ignore_ascii_case("unknown") {
        return Err(LoraError::PolicyDenied(
            "training dataset requires an explicit license declaration".to_string(),
        ));
    }
    if examples.len() < 3 {
        return Err(LoraError::MalformedArtifact(
            "training dataset requires at least 3 examples across distinct source groups"
                .to_string(),
        ));
    }

    let mut groups: BTreeSet<String> = BTreeSet::new();
    for ex in examples {
        if ex.prompt.trim().is_empty() || ex.target_completion.trim().is_empty() {
            return Err(LoraError::MalformedArtifact(format!(
                "training example '{}' has empty prompt or completion",
                ex.example_id
            )));
        }
        groups.insert(ex.source_group_id.clone());
    }

    if groups.len() < 3 {
        return Err(LoraError::MalformedArtifact(
            "dataset must contain at least 3 distinct source_group_id values to form non-overlapping train/val/test splits"
                .to_string(),
        ));
    }

    let ordered_groups: Vec<String> = groups.into_iter().collect();
    let total_g = ordered_groups.len();
    let test_count = (total_g / 5).max(1);
    let val_count = (total_g / 5).max(1);
    let train_count = total_g - test_count - val_count;

    let train_groups: BTreeSet<String> = ordered_groups[..train_count].iter().cloned().collect();
    let validation_groups: BTreeSet<String> = ordered_groups[train_count..train_count + val_count]
        .iter()
        .cloned()
        .collect();
    let test_groups: BTreeSet<String> = ordered_groups[train_count + val_count..]
        .iter()
        .cloned()
        .collect();

    // Double-check zero overlap between splits
    if !train_groups.is_disjoint(&validation_groups)
        || !train_groups.is_disjoint(&test_groups)
        || !validation_groups.is_disjoint(&test_groups)
    {
        return Err(LoraError::MalformedArtifact(
            "split group overlap detected".to_string(),
        ));
    }

    let mut train_examples = Vec::new();
    let mut validation_examples = Vec::new();
    let mut test_examples = Vec::new();

    for ex in examples {
        if train_groups.contains(&ex.source_group_id) {
            train_examples.push(ex.clone());
        } else if validation_groups.contains(&ex.source_group_id) {
            validation_examples.push(ex.clone());
        } else {
            test_examples.push(ex.clone());
        }
    }

    let canonical_json = serde_json::to_string(examples).unwrap_or_default();
    let dataset_sha256 = hex::encode(Sha256::digest(canonical_json.as_bytes()));

    let mut manifest = DatasetSplitManifest {
        dataset_sha256,
        license: license.to_string(),
        train_examples,
        validation_examples,
        test_examples,
        train_groups,
        validation_groups,
        test_groups,
        manifest_sha256: None,
    };
    manifest.manifest_sha256 = Some(compute_dataset_split_digest(&manifest));
    Ok(manifest)
}

/// Immutable plain-LoRA training job configuration (`docs/TRAINING_SYSTEM.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoraJobConfig {
    pub job_id: String,
    pub base_model_sha256: String,
    pub tokenizer_template_sha256: String,
    pub dataset_sha256: String,
    pub rank: u32,
    pub alpha: u32,
    pub target_modules: Vec<String>,
    pub epochs: u32,
    pub context_length: u32,
    pub microbatch_size: u32,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldOutEvaluationReceipt {
    pub base_task_score_pct: u32,
    pub adapter_task_score_pct: u32,
    pub general_regression_pct: u32,
    pub schema_valid_pct: u32,
    pub target_runtime_load_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_sha256: Option<String>,
}

/// Compute a domain-separated canonical SHA-256 digest over a HeldOutEvaluationReceipt (`lora.adapter.eval.v1`).
pub fn compute_held_out_evaluation_digest(eval: &HeldOutEvaluationReceipt) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ADAPTER_EVAL_DOMAIN_V1);
    hasher.update(eval.base_task_score_pct.to_le_bytes());
    hasher.update(eval.adapter_task_score_pct.to_le_bytes());
    hasher.update(eval.general_regression_pct.to_le_bytes());
    hasher.update(eval.schema_valid_pct.to_le_bytes());
    hasher.update([if eval.target_runtime_load_verified {
        1u8
    } else {
        0u8
    }]);
    hex::encode(hasher.finalize())
}

/// Verify bounds and cryptographic integrity of a HeldOutEvaluationReceipt.
pub fn verify_held_out_evaluation_receipt(
    eval: &HeldOutEvaluationReceipt,
) -> Result<(), LoraError> {
    if eval.base_task_score_pct > 100
        || eval.adapter_task_score_pct > 100
        || eval.general_regression_pct > 100
        || eval.schema_valid_pct > 100
    {
        return Err(LoraError::MalformedArtifact(
            "evaluation metric score percentage exceeds 100%".to_string(),
        ));
    }
    if let Some(expected_digest) = &eval.receipt_sha256 {
        let computed = compute_held_out_evaluation_digest(eval);
        if computed != *expected_digest {
            return Err(LoraError::PolicyDenied(format!(
                "held-out evaluation receipt digest mismatch: expected {expected_digest}, got {computed}"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterPromotionDecision {
    PromoteExperimental,
    RejectKeepReceipt,
}

impl AdapterPromotionDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PromoteExperimental => "promote_experimental",
            Self::RejectKeepReceipt => "reject_keep_receipt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterStudioOutcome {
    pub job_id: String,
    pub adapter_sha256: String,
    pub base_model_sha256: String,
    pub decision: AdapterPromotionDecision,
    pub evaluation: HeldOutEvaluationReceipt,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_sha256: Option<String>,
}

/// Compute a domain-separated canonical SHA-256 digest over an adapter artifact (`lora.adapter.artifact.v1`).
pub fn compute_adapter_artifact_digest(
    job_id: &str,
    base_model_sha256: &str,
    dataset_sha256: &str,
    rank: u32,
    alpha: u32,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ADAPTER_ARTIFACT_DOMAIN_V1);
    update_len_prefixed(&mut hasher, job_id.as_bytes());
    update_len_prefixed(&mut hasher, base_model_sha256.as_bytes());
    update_len_prefixed(&mut hasher, dataset_sha256.as_bytes());
    hasher.update(rank.to_le_bytes());
    hasher.update(alpha.to_le_bytes());
    hex::encode(hasher.finalize())
}

/// Compute a domain-separated canonical SHA-256 digest over an AdapterStudioOutcome (`lora.adapter.eval.v1`).
pub fn compute_adapter_outcome_digest(outcome: &AdapterStudioOutcome) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ADAPTER_EVAL_DOMAIN_V1);
    update_len_prefixed(&mut hasher, outcome.job_id.as_bytes());
    update_len_prefixed(&mut hasher, outcome.adapter_sha256.as_bytes());
    update_len_prefixed(&mut hasher, outcome.base_model_sha256.as_bytes());
    update_len_prefixed(&mut hasher, outcome.decision.as_str().as_bytes());
    update_len_prefixed(&mut hasher, outcome.reason.as_bytes());
    if let Some(ref eval_digest) = outcome.evaluation.receipt_sha256 {
        update_len_prefixed(&mut hasher, eval_digest.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Verify cryptographic integrity and promotion gate decision consistency of an AdapterStudioOutcome.
pub fn verify_adapter_studio_outcome(outcome: &AdapterStudioOutcome) -> Result<(), LoraError> {
    verify_held_out_evaluation_receipt(&outcome.evaluation)?;
    if let Some(expected_digest) = &outcome.outcome_sha256 {
        let computed = compute_adapter_outcome_digest(outcome);
        if computed != *expected_digest {
            return Err(LoraError::PolicyDenied(format!(
                "adapter studio outcome digest mismatch: expected {expected_digest}, got {computed}"
            )));
        }
    }
    Ok(())
}

/// Evaluate whether a trained LoRA adapter passes the promotion gate (`docs/TRAINING_SYSTEM.md`):
/// - Base model digest must match the target serving model digest
/// - Target runtime load check must succeed
/// - Held-out task score must improve over the untuned base model
/// - General task regression must not exceed 2%
pub fn evaluate_adapter_promotion_gate(
    config: &LoraJobConfig,
    installed_base_sha256: &str,
    eval: HeldOutEvaluationReceipt,
) -> Result<AdapterStudioOutcome, LoraError> {
    if config.base_model_sha256 != installed_base_sha256 {
        return Err(LoraError::IncompatibleModel(format!(
            "adapter base digest {} does not match installed base model {}",
            config.base_model_sha256, installed_base_sha256
        )));
    }
    if config.rank == 0 || config.rank > 128 || config.target_modules.is_empty() {
        return Err(LoraError::MalformedArtifact(
            "invalid LoRA rank or empty target_modules".to_string(),
        ));
    }

    let adapter_sha256 = compute_adapter_artifact_digest(
        &config.job_id,
        &config.base_model_sha256,
        &config.dataset_sha256,
        config.rank,
        config.alpha,
    );

    let (decision, reason) = if !eval.target_runtime_load_verified {
        (
            AdapterPromotionDecision::RejectKeepReceipt,
            "target inference worker failed to load exported adapter".to_string(),
        )
    } else if eval.adapter_task_score_pct <= eval.base_task_score_pct {
        (
            AdapterPromotionDecision::RejectKeepReceipt,
            format!(
                "held-out task score did not improve over base ({}% vs {}%)",
                eval.adapter_task_score_pct, eval.base_task_score_pct
            ),
        )
    } else if eval.general_regression_pct > 2 || eval.schema_valid_pct < 100 {
        (
            AdapterPromotionDecision::RejectKeepReceipt,
            format!(
                "adapter exceeded regression tolerance (regression={}%, schema_valid={}%)",
                eval.general_regression_pct, eval.schema_valid_pct
            ),
        )
    } else {
        (
            AdapterPromotionDecision::PromoteExperimental,
            format!(
                "passed held-out gate (+{}% improvement, 0 runtime load errors)",
                eval.adapter_task_score_pct - eval.base_task_score_pct
            ),
        )
    };

    let mut eval = eval;
    eval.receipt_sha256 = Some(compute_held_out_evaluation_digest(&eval));

    let mut outcome = AdapterStudioOutcome {
        job_id: config.job_id.clone(),
        adapter_sha256,
        base_model_sha256: config.base_model_sha256.clone(),
        decision,
        evaluation: eval,
        reason,
        outcome_sha256: None,
    };
    outcome.outcome_sha256 = Some(compute_adapter_outcome_digest(&outcome));
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_examples() -> Vec<TrainingExample> {
        vec![
            TrainingExample {
                example_id: "ex_1".to_string(),
                source_group_id: "grp_A".to_string(),
                prompt: "Summarize doc A".to_string(),
                target_completion: "Summary A".to_string(),
            },
            TrainingExample {
                example_id: "ex_2".to_string(),
                source_group_id: "grp_B".to_string(),
                prompt: "Summarize doc B".to_string(),
                target_completion: "Summary B".to_string(),
            },
            TrainingExample {
                example_id: "ex_3".to_string(),
                source_group_id: "grp_C".to_string(),
                prompt: "Summarize doc C".to_string(),
                target_completion: "Summary C".to_string(),
            },
            TrainingExample {
                example_id: "ex_4".to_string(),
                source_group_id: "grp_A".to_string(),
                prompt: "Explain doc A details".to_string(),
                target_completion: "Details A".to_string(),
            },
        ]
    }

    #[test]
    fn test_split_manifest_attestation_and_leakage_verification() {
        let examples = sample_examples();
        let manifest = prepare_leakage_free_splits(&examples, "Apache-2.0")
            .expect("should prepare valid split");

        assert!(manifest.manifest_sha256.is_some());
        assert!(verify_dataset_split_manifest(&manifest).is_ok());

        // Tampering with manifest_sha256 fails verification
        let mut tampered = manifest.clone();
        tampered.manifest_sha256 = Some("0".repeat(64));
        assert!(matches!(
            verify_dataset_split_manifest(&tampered),
            Err(LoraError::PolicyDenied(_))
        ));

        // Group overlap leakage fails verification
        let mut leaked = manifest.clone();
        leaked.validation_groups.insert("grp_A".to_string());
        assert!(matches!(
            verify_dataset_split_manifest(&leaked),
            Err(LoraError::MalformedArtifact(_))
        ));

        // Empty license fails
        let mut no_license = manifest.clone();
        no_license.license = "".to_string();
        assert!(matches!(
            verify_dataset_split_manifest(&no_license),
            Err(LoraError::PolicyDenied(_))
        ));
    }

    #[test]
    fn test_held_out_evaluation_receipt_and_outcome_attestation() {
        let eval = HeldOutEvaluationReceipt {
            base_task_score_pct: 65,
            adapter_task_score_pct: 82,
            general_regression_pct: 1,
            schema_valid_pct: 100,
            target_runtime_load_verified: true,
            receipt_sha256: None,
        };
        let eval_digest = compute_held_out_evaluation_digest(&eval);
        assert_eq!(eval_digest.len(), 64);

        let mut signed_eval = eval.clone();
        signed_eval.receipt_sha256 = Some(eval_digest);
        assert!(verify_held_out_evaluation_receipt(&signed_eval).is_ok());

        let job = LoraJobConfig {
            job_id: "job-001".to_string(),
            base_model_sha256: "base_sha".to_string(),
            tokenizer_template_sha256: "tok_sha".to_string(),
            dataset_sha256: "data_sha".to_string(),
            rank: 16,
            alpha: 32,
            target_modules: vec!["q_proj".to_string(), "v_proj".to_string()],
            epochs: 3,
            context_length: 2048,
            microbatch_size: 4,
            seed: 42,
        };

        let outcome = evaluate_adapter_promotion_gate(&job, "base_sha", eval)
            .expect("should evaluate promotion gate");
        assert_eq!(
            outcome.decision,
            AdapterPromotionDecision::PromoteExperimental
        );
        assert!(outcome.outcome_sha256.is_some());
        assert!(verify_adapter_studio_outcome(&outcome).is_ok());

        // Tampering outcome_sha256 fails verification
        let mut bad_outcome = outcome.clone();
        bad_outcome.outcome_sha256 = Some("1".repeat(64));
        assert!(matches!(
            verify_adapter_studio_outcome(&bad_outcome),
            Err(LoraError::PolicyDenied(_))
        ));
    }
}
