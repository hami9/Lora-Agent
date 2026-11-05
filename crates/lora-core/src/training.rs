//! Adapter Studio (`docs/TRAINING_SYSTEM.md`, Phase 8):
//! Dataset source-grouped splitting (leakage prevention), plain LoRA job configuration,
//! memory/disk cost estimation, checkpointing, and held-out evaluation promotion gate.

use crate::error::LoraError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

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

    Ok(DatasetSplitManifest {
        dataset_sha256,
        license: license.to_string(),
        train_examples,
        validation_examples,
        test_examples,
        train_groups,
        validation_groups,
        test_groups,
    })
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterPromotionDecision {
    PromoteExperimental,
    RejectKeepReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterStudioOutcome {
    pub job_id: String,
    pub adapter_sha256: String,
    pub base_model_sha256: String,
    pub decision: AdapterPromotionDecision,
    pub evaluation: HeldOutEvaluationReceipt,
    pub reason: String,
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

    let mut hasher = Sha256::new();
    hasher.update(config.job_id.as_bytes());
    hasher.update(config.base_model_sha256.as_bytes());
    hasher.update(config.dataset_sha256.as_bytes());
    let adapter_sha256 = hex::encode(hasher.finalize());

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

    Ok(AdapterStudioOutcome {
        job_id: config.job_id.clone(),
        adapter_sha256,
        base_model_sha256: config.base_model_sha256.clone(),
        decision,
        evaluation: eval,
        reason,
    })
}
