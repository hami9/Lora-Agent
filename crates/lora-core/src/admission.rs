//! Hardware Intelligence Engine (HIE) memory estimator, admission planner, and reservation lease
//! (`docs/HARDWARE_INTELLIGENCE.md`, `T5`, `M3`).

use crate::error::LoraError;
use crate::ports::{HardwareProbePort, HostSnapshot};
use lora_protocol::ProbedModelMetadata;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Default safety headroom reserved on the host RAM pool (128 MiB).
pub const DEFAULT_SAFETY_RESERVE_BYTES: u64 = 128 * 1024 * 1024;
/// Fixed runtime/scratch/graph buffer overhead estimate (32 MiB).
pub const RUNTIME_SCRATCH_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TuningProfile {
    Automatic,
    Balanced,
    MaximumPerformance,
    LowMemory,
    BatterySaver,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEstimate {
    pub weights_bytes: u64,
    pub kv_cache_bytes: u64,
    pub runtime_scratch_bytes: u64,
    pub safety_reserve_bytes: u64,
    pub total_required_bytes: u64,
    pub observed_available_bytes: u64,
    pub usable_budget_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionPlan {
    pub plan_id: String,
    pub model_digest: String,
    pub requested_context: u32,
    pub recommended_threads: u32,
    pub backend: String,
    pub profile: TuningProfile,
    pub estimate: MemoryEstimate,
    pub explanation: String,
    pub snapshot_timestamp_ms: u64,
}

/// First-order uncompressed KV cache estimate (`docs/HARDWARE_INTELLIGENCE.md`):
/// `layers * context * sequences * kv_heads * (key_dim * key_bytes + value_dim * value_bytes)`
pub fn compute_kv_cache_bytes(
    meta: &ProbedModelMetadata,
    context_length: u32,
    sequences: u32,
) -> u64 {
    let layers = u64::from(meta.block_count.max(1));
    let ctx = u64::from(context_length.max(1));
    let seq = u64::from(sequences.max(1));
    let kv_heads = u64::from(meta.kv_head_count.max(1));
    let elem_bytes = u64::from(meta.kv_elem_bytes.max(2));
    let k_bytes = u64::from(meta.key_dim.max(64)).saturating_mul(elem_bytes);
    let v_bytes = u64::from(meta.value_dim.max(64)).saturating_mul(elem_bytes);
    let per_head = k_bytes.saturating_add(v_bytes);

    layers
        .saturating_mul(ctx)
        .saturating_mul(seq)
        .saturating_mul(kv_heads)
        .saturating_mul(per_head)
}

#[derive(Default)]
struct ReservationState {
    active_plan_id: Option<String>,
    reserved_bytes: u64,
}

#[derive(Clone)]
pub struct AdmissionPlanner {
    state: Arc<Mutex<ReservationState>>,
    safety_reserve_bytes: u64,
}

/// RAII lease representing the single active generation reservation in the MVP (`T5`).
pub struct ReservationLease {
    state: Arc<Mutex<ReservationState>>,
    pub plan: AdmissionPlan,
    released: bool,
}

impl ReservationLease {
    /// Recheck host memory pressure immediately before materializing model weights in the worker (`M3`).
    pub fn verify_pressure_before_load(
        &self,
        probe: &dyn HardwareProbePort,
        storage_dir: &Path,
    ) -> Result<(), LoraError> {
        let fresh = probe.capture_snapshot(storage_dir)?;
        let required_with_reserve = self.plan.estimate.total_required_bytes;
        if fresh.available_ram_bytes < required_with_reserve {
            return Err(LoraError::PressureChangedAfterReservation {
                details: format!(
                    "available RAM dropped from {} B at reservation to {} B before load (plan requires {} B including safety reserve)",
                    self.plan.estimate.observed_available_bytes,
                    fresh.available_ram_bytes,
                    required_with_reserve
                ),
            });
        }
        Ok(())
    }

    pub fn release(&mut self) {
        if !self.released {
            let mut guard = self.state.lock().expect("reservation mutex poisoned");
            if guard.active_plan_id.as_deref() == Some(&self.plan.plan_id) {
                guard.active_plan_id = None;
                guard.reserved_bytes = 0;
            }
            self.released = true;
        }
    }
}

impl Drop for ReservationLease {
    fn drop(&mut self) {
        self.release();
    }
}

impl AdmissionPlanner {
    pub fn new(safety_reserve_bytes: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(ReservationState::default())),
            safety_reserve_bytes,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.state
            .lock()
            .expect("reservation mutex poisoned")
            .active_plan_id
            .is_some()
    }

    /// Evaluate model metadata + requested context against a fresh hardware snapshot and acquire
    /// the single active reservation lease if feasible (`M3`, `T5`).
    /// Never silently shrinks an explicit context request.
    pub fn evaluate_and_reserve(
        &self,
        model_digest: &str,
        meta: &ProbedModelMetadata,
        requested_context: u32,
        profile: TuningProfile,
        snapshot: &HostSnapshot,
    ) -> Result<ReservationLease, LoraError> {
        if requested_context > meta.context_length {
            return Err(LoraError::IncompatibleModel(format!(
                "requested context {} exceeds model maximum trained context {}",
                requested_context, meta.context_length
            )));
        }

        let mut guard = self.state.lock().expect("reservation mutex poisoned");
        if guard.active_plan_id.is_some() {
            return Err(LoraError::ActiveGenerationBusy);
        }

        let weights_bytes = meta.total_tensor_bytes.max(meta.file_size_bytes);
        let kv_cache_bytes = compute_kv_cache_bytes(meta, requested_context, 1);
        let runtime_scratch_bytes = RUNTIME_SCRATCH_BYTES;
        let safety_reserve_bytes = self.safety_reserve_bytes;

        let workload_bytes = weights_bytes
            .saturating_add(kv_cache_bytes)
            .saturating_add(runtime_scratch_bytes);
        let total_required_bytes = workload_bytes.saturating_add(safety_reserve_bytes);

        let usable_budget_bytes = snapshot
            .available_ram_bytes
            .saturating_sub(safety_reserve_bytes)
            .saturating_sub(guard.reserved_bytes);

        if workload_bytes > usable_budget_bytes {
            // Compute a smaller context alternative if weights + scratch fit inside usable budget
            let base_without_kv = weights_bytes.saturating_add(runtime_scratch_bytes);
            let suggested_context = if usable_budget_bytes > base_without_kv {
                let remaining_for_kv = usable_budget_bytes - base_without_kv;
                let per_token_kv = compute_kv_cache_bytes(meta, 1, 1).max(1);
                let max_fitting_tokens = (remaining_for_kv / per_token_kv) as u32;
                let rounded = (max_fitting_tokens / 256) * 256;
                if rounded >= 256 && rounded < requested_context {
                    Some(rounded)
                } else {
                    None
                }
            } else {
                None
            };

            let suggestion_msg = match suggested_context {
                Some(ctx) => {
                    format!("; re-run with '--context {ctx}' to acknowledge reduced context")
                }
                None => "; model weights exceed available RAM budget even at minimal context"
                    .to_string(),
            };

            return Err(LoraError::InsufficientMemory {
                explanation: format!(
                    "plan requires {} MiB (weights: {} MiB, KV @ ctx {}: {} MiB, scratch: {} MiB) + {} MiB safety reserve, but only {} MiB usable headroom is available{}",
                    workload_bytes / (1024 * 1024),
                    weights_bytes / (1024 * 1024),
                    requested_context,
                    kv_cache_bytes / (1024 * 1024),
                    runtime_scratch_bytes / (1024 * 1024),
                    safety_reserve_bytes / (1024 * 1024),
                    usable_budget_bytes / (1024 * 1024),
                    suggestion_msg
                ),
                estimated_weights_bytes: weights_bytes,
                estimated_kv_bytes: kv_cache_bytes,
                available_headroom_bytes: usable_budget_bytes,
                suggested_context_length: suggested_context,
            });
        }

        let recommended_threads = match profile {
            TuningProfile::BatterySaver | TuningProfile::LowMemory => {
                (snapshot.physical_cores / 2).max(1)
            }
            _ => snapshot.physical_cores.clamp(1, 8),
        };

        let plan_id = format!(
            "plan-{}-{}",
            &model_digest[..8.min(model_digest.len())],
            snapshot.sample_timestamp_ms
        );
        let explanation = format!(
            "Admitted on CPU ({} threads, profile {:?}): weights={} B, KV(ctx={})={} B, scratch={} B, usable_budget={} B (safety_reserve={} B)",
            recommended_threads,
            profile,
            weights_bytes,
            requested_context,
            kv_cache_bytes,
            runtime_scratch_bytes,
            usable_budget_bytes,
            safety_reserve_bytes
        );

        let plan = AdmissionPlan {
            plan_id: plan_id.clone(),
            model_digest: model_digest.to_string(),
            requested_context,
            recommended_threads,
            backend: "cpu".to_string(),
            profile,
            estimate: MemoryEstimate {
                weights_bytes,
                kv_cache_bytes,
                runtime_scratch_bytes,
                safety_reserve_bytes,
                total_required_bytes,
                observed_available_bytes: snapshot.available_ram_bytes,
                usable_budget_bytes,
            },
            explanation,
            snapshot_timestamp_ms: snapshot.sample_timestamp_ms,
        };

        guard.active_plan_id = Some(plan_id);
        guard.reserved_bytes = workload_bytes;

        Ok(ReservationLease {
            state: Arc::clone(&self.state),
            plan,
            released: false,
        })
    }

    /// Rank locally installed models that fit inside the current usable RAM budget at `requested_context` (`docs/INFERENCE_ENGINE.md`).
    pub fn rank_fitting_alternatives(
        &self,
        candidates: &[(String, ProbedModelMetadata)],
        requested_context: u32,
        snapshot: &HostSnapshot,
    ) -> Vec<FittingAlternative> {
        let usable_budget = snapshot
            .available_ram_bytes
            .saturating_sub(self.safety_reserve_bytes);
        let mut fitting = Vec::new();
        for (alias, meta) in candidates {
            let ctx = requested_context.min(meta.context_length);
            let weights = meta.total_tensor_bytes.max(meta.file_size_bytes);
            let kv = compute_kv_cache_bytes(meta, ctx, 1);
            let workload = weights
                .saturating_add(kv)
                .saturating_add(RUNTIME_SCRATCH_BYTES);
            if workload <= usable_budget {
                fitting.push(FittingAlternative {
                    display_alias: alias.clone(),
                    fitting_context: ctx,
                    estimated_workload_bytes: workload,
                    headroom_remaining_bytes: usable_budget - workload,
                });
            }
        }
        // Rank by largest fitting context first, then by lowest memory footprint
        fitting.sort_by(|a, b| {
            b.fitting_context
                .cmp(&a.fitting_context)
                .then_with(|| a.estimated_workload_bytes.cmp(&b.estimated_workload_bytes))
        });
        fitting
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FittingAlternative {
    pub display_alias: String,
    pub fitting_context: u32,
    pub estimated_workload_bytes: u64,
    pub headroom_remaining_bytes: u64,
}
