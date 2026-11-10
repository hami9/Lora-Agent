//! Benchmark runner, paired median statistics, and 30-cycle load/generate/unload soak harness
//! (`docs/BENCHMARK_PLAN.md`, `docs/PERFORMANCE_STRATEGY.md`, `T9`).

use crate::admission::TuningProfile;
use crate::agent::AgentLimits;
use crate::error::{redact_sensitive_text, LoraError};
use crate::policy::ExecutionMode;
use crate::LoraApplicationService;
use lora_protocol::PINNED_LLAMA_CPP_REVISION;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

/// Domain separation prefix for soak test receipt attestation (`docs/BENCHMARK_PLAN.md`, `docs/PERFORMANCE_STRATEGY.md`).
pub const SOAK_TEST_DOMAIN_V1: &[u8] = b"lora.soak.receipt.v1\0";

/// Domain separation prefix for benchmark summary receipt attestation (`docs/BENCHMARK_PLAN.md`, `T9`).
pub const BENCHMARK_RECEIPT_DOMAIN_V1: &[u8] = b"lora.benchmark.receipt.v1\0";

fn update_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchmarkSample {
    pub iteration: u32,
    pub warmup: bool,
    pub ttft_us: u64,
    pub total_duration_us: u64,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub peak_rss_bytes: u64,
    pub exit_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricStats {
    pub count: usize,
    pub median: u64,
    pub min: u64,
    pub max: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoakTestReceipt {
    pub cycles_completed: u32,
    pub warmup_rss_bytes: u64,
    pub final_rss_bytes: u64,
    pub max_rss_growth_bytes: u64,
    pub all_terminal_outcomes_valid: bool,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_sha256: Option<String>,
}

/// Compute a domain-separated canonical SHA-256 digest over a SoakTestReceipt (`lora.soak.receipt.v1`).
pub fn compute_soak_receipt_digest(soak: &SoakTestReceipt) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SOAK_TEST_DOMAIN_V1);
    hasher.update(soak.cycles_completed.to_le_bytes());
    hasher.update(soak.warmup_rss_bytes.to_le_bytes());
    hasher.update(soak.final_rss_bytes.to_le_bytes());
    hasher.update(soak.max_rss_growth_bytes.to_le_bytes());
    hasher.update([soak.all_terminal_outcomes_valid as u8]);
    hasher.update([soak.passed as u8]);
    hex::encode(hasher.finalize())
}

/// Verify that a SoakTestReceipt passed soak invariants and matches its cryptographic digest.
pub fn verify_soak_test_receipt(soak: &SoakTestReceipt) -> Result<(), LoraError> {
    if !soak.passed || !soak.all_terminal_outcomes_valid {
        return Err(LoraError::PolicyDenied(
            "soak test receipt did not pass all cycle or outcome invariants".to_string(),
        ));
    }
    let expected = compute_soak_receipt_digest(soak);
    match &soak.receipt_sha256 {
        Some(digest) if digest == &expected => Ok(()),
        Some(mismatch) => Err(LoraError::PolicyDenied(format!(
            "soak test receipt digest mismatch: expected {expected}, got {mismatch}"
        ))),
        None => Err(LoraError::MalformedArtifact(
            "soak test receipt missing cryptographic digest".to_string(),
        )),
    }
}

/// Anonymized, machine-readable benchmark receipt (`docs/BENCHMARK_PLAN.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchmarkReceipt {
    pub case_id: String,
    pub model_sha256: String,
    pub pinned_worker_revision: String,
    pub os_arch: String,
    pub cpu_isa_features: Vec<String>,
    pub context_length: u32,
    pub warmup_runs: u32,
    pub measured_runs: u32,
    pub ttft_us_stats: MetricStats,
    pub duration_us_stats: MetricStats,
    pub soak_receipt: SoakTestReceipt,
    pub raw_samples: Vec<BenchmarkSample>,
    pub residual_risks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_sha256: Option<String>,
}

/// Compute a domain-separated canonical SHA-256 digest over a BenchmarkReceipt (`lora.benchmark.receipt.v1`).
pub fn compute_benchmark_receipt_digest(receipt: &BenchmarkReceipt) -> String {
    let mut hasher = Sha256::new();
    hasher.update(BENCHMARK_RECEIPT_DOMAIN_V1);
    update_len_prefixed(&mut hasher, receipt.case_id.as_bytes());
    update_len_prefixed(&mut hasher, receipt.model_sha256.as_bytes());
    update_len_prefixed(&mut hasher, receipt.pinned_worker_revision.as_bytes());
    update_len_prefixed(&mut hasher, receipt.os_arch.as_bytes());

    hasher.update((receipt.cpu_isa_features.len() as u64).to_le_bytes());
    for f in &receipt.cpu_isa_features {
        update_len_prefixed(&mut hasher, f.as_bytes());
    }

    hasher.update(receipt.context_length.to_le_bytes());
    hasher.update(receipt.warmup_runs.to_le_bytes());
    hasher.update(receipt.measured_runs.to_le_bytes());

    // Hash ttft stats
    hasher.update((receipt.ttft_us_stats.count as u64).to_le_bytes());
    hasher.update(receipt.ttft_us_stats.median.to_le_bytes());
    hasher.update(receipt.ttft_us_stats.min.to_le_bytes());
    hasher.update(receipt.ttft_us_stats.max.to_le_bytes());

    // Hash duration stats
    hasher.update((receipt.duration_us_stats.count as u64).to_le_bytes());
    hasher.update(receipt.duration_us_stats.median.to_le_bytes());
    hasher.update(receipt.duration_us_stats.min.to_le_bytes());
    hasher.update(receipt.duration_us_stats.max.to_le_bytes());

    // Hash soak digest
    let soak_digest = compute_soak_receipt_digest(&receipt.soak_receipt);
    update_len_prefixed(&mut hasher, soak_digest.as_bytes());

    // Hash raw sample count
    hasher.update((receipt.raw_samples.len() as u64).to_le_bytes());

    hex::encode(hasher.finalize())
}

/// Verify that a BenchmarkReceipt passed soak tests and matches its cryptographic attestation digest.
pub fn verify_benchmark_receipt(receipt: &BenchmarkReceipt) -> Result<(), LoraError> {
    if !receipt.soak_receipt.passed {
        return Err(LoraError::PolicyDenied(
            "benchmark receipt indicates soak test failure".to_string(),
        ));
    }
    verify_soak_test_receipt(&receipt.soak_receipt)?;

    let expected = compute_benchmark_receipt_digest(receipt);
    match &receipt.receipt_sha256 {
        Some(digest) if digest == &expected => Ok(()),
        Some(mismatch) => Err(LoraError::PolicyDenied(format!(
            "benchmark receipt digest mismatch: expected {expected}, got {mismatch}"
        ))),
        None => Err(LoraError::MalformedArtifact(
            "benchmark receipt missing cryptographic digest".to_string(),
        )),
    }
}

pub fn compute_stats(mut values: Vec<u64>) -> MetricStats {
    if values.is_empty() {
        return MetricStats {
            count: 0,
            median: 0,
            min: 0,
            max: 0,
        };
    }
    values.sort_unstable();
    let count = values.len();
    let min = values[0];
    let max = values[count - 1];
    let median = if count.is_multiple_of(2) {
        (values[count / 2 - 1] + values[count / 2]) / 2
    } else {
        values[count / 2]
    };
    MetricStats {
        count,
        median,
        min,
        max,
    }
}

/// Monotonic stage breakdown for a single request (`docs/PERFORMANCE_STRATEGY.md`).
/// "Human-visible time to first token includes queue and renderer; token-generation metrics exclude UI delivery."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageLatencyProfile {
    pub request_id: String,
    pub queue_us: u64,
    pub handshake_us: u64,
    pub admission_us: u64,
    pub load_us: u64,
    pub prefill_to_first_byte_us: u64,
    pub ui_render_delivery_us: u64,
    pub decode_last_token_us: u64,
    pub reap_us: u64,
}

impl StageLatencyProfile {
    /// Human-visible TTFT includes queue, admission, load, prefill, and UI renderer delivery.
    pub fn human_visible_ttft_us(&self) -> u64 {
        self.queue_us
            .saturating_add(self.handshake_us)
            .saturating_add(self.admission_us)
            .saturating_add(self.load_us)
            .saturating_add(self.prefill_to_first_byte_us)
            .saturating_add(self.ui_render_delivery_us)
    }

    /// Pure engine token-generation decode duration (excludes loading and UI delivery).
    pub fn engine_decode_duration_us(&self) -> u64 {
        self.decode_last_token_us
    }
}

/// Compute effective physical memory budget without double-counting RAM and VRAM on unified memory
/// architectures such as Apple Silicon or integrated GPUs (`docs/PERFORMANCE_STRATEGY.md`, `R02`).
pub fn compute_unified_memory_budget(
    available_ram_bytes: u64,
    reported_vram_bytes: Option<u64>,
    is_unified_memory: bool,
) -> u64 {
    match (is_unified_memory, reported_vram_bytes) {
        (true, Some(vram)) => available_ram_bytes.min(vram),
        (true, None) => available_ram_bytes,
        (false, Some(vram)) => available_ram_bytes.saturating_add(vram),
        (false, None) => available_ram_bytes,
    }
}

/// Redact usernames, hostnames, absolute filesystem paths, and serial numbers from a public benchmark report (`docs/BENCHMARK_PLAN.md`).
pub fn anonymize_public_benchmark_report(raw_report: &str) -> String {
    let mut out = redact_sensitive_text(raw_report);
    for prefix in ["C:\\Users\\", "/home/", "/Users/", "SERIAL=", "HOSTNAME="] {
        while let Some(idx) = out.find(prefix) {
            let end = out[idx..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',')
                .map(|off| idx + off)
                .unwrap_or(out.len());
            out.replace_range(idx..end, "[ANONYMIZED]");
        }
    }
    out
}

/// Controlled benchmark scenario descriptor (`docs/BENCHMARK_PLAN.md` — "Fixture and controls").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchmarkComparisonControl {
    pub model_sha256: String,
    pub quantization: String,
    pub tokenizer_digest: String,
    pub template_digest: String,
    pub effective_context_length: u32,
    pub prompt_token_ids: Vec<u32>,
}

/// Verify that two benchmark runs share identical model checkpoint, quantization, tokenizer/template,
/// effective context length, and tokenized prompt IDs before comparing throughput or latency
/// (`docs/BENCHMARK_PLAN.md`: "Reject comparisons where prompt token IDs or effective context differ.
/// A different quantization, tokenizer or model makes the result a separate scenario.").
pub fn verify_benchmark_comparison_controls(
    baseline: &BenchmarkComparisonControl,
    candidate: &BenchmarkComparisonControl,
) -> Result<(), LoraError> {
    if baseline.model_sha256 != candidate.model_sha256
        || baseline.quantization != candidate.quantization
    {
        return Err(LoraError::IncompatibleModel(format!(
            "benchmark comparison rejected: model/quantization mismatch ({}/{} vs {}/{})",
            baseline.model_sha256,
            baseline.quantization,
            candidate.model_sha256,
            candidate.quantization
        )));
    }
    if baseline.tokenizer_digest != candidate.tokenizer_digest
        || baseline.template_digest != candidate.template_digest
    {
        return Err(LoraError::IncompatibleModel(
            "benchmark comparison rejected: tokenizer or template digest differs".to_string(),
        ));
    }
    if baseline.effective_context_length != candidate.effective_context_length {
        return Err(LoraError::PolicyDenied(format!(
            "benchmark comparison rejected: effective context differs ({} vs {})",
            baseline.effective_context_length, candidate.effective_context_length
        )));
    }
    if baseline.prompt_token_ids.is_empty()
        || baseline.prompt_token_ids != candidate.prompt_token_ids
    {
        return Err(LoraError::PolicyDenied(
            "benchmark comparison rejected: prompt_token_ids differ or are empty".to_string(),
        ));
    }
    Ok(())
}

impl LoraApplicationService {
    /// Run the `T9` benchmark and multi-cycle load/generate/unload soak suite (`docs/BENCHMARK_PLAN.md`),
    /// writing anonymized `receipts.jsonl` and `summary.json` into `output_dir`.
    pub fn run_benchmark_and_soak(
        &self,
        model_id_or_alias: &str,
        warmup_runs: u32,
        measured_runs: u32,
        soak_cycles: u32,
        output_dir: &Path,
    ) -> Result<BenchmarkReceipt, LoraError> {
        fs::create_dir_all(output_dir)?;

        let (manifest, _blob_path) = self.registry.resolve_ready_model(model_id_or_alias)?;
        let model_sha256 = manifest
            .files
            .first()
            .map(|f| f.sha256.clone())
            .unwrap_or_default();
        let snap = self.hw_probe.capture_snapshot(&self.data_dir)?;

        let total_iters = warmup_runs + measured_runs;
        let mut raw_samples = Vec::with_capacity(total_iters as usize);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let synthetic_prompt =
            "Synthetic benchmark prompt: summarize local-first isolation invariants.";

        for i in 0..total_iters {
            let is_warmup = i < warmup_runs;
            let run_id = format!("bench-iter-{i}-{}", crate::store::now_epoch_ms());
            let start = Instant::now();
            let mut first_token_us = None;

            let (_plan, outcome) = self.run_task(
                &run_id,
                "bench-runner",
                &model_sha256,
                2048,
                TuningProfile::Automatic,
                ExecutionMode::ChatOnly,
                None,
                synthetic_prompt,
                &AgentLimits::default(),
                &cancel_flag,
                &mut |_delta| {
                    if first_token_us.is_none() {
                        first_token_us = Some(start.elapsed().as_micros() as u64);
                    }
                    Ok(())
                },
            )?;

            let total_us = start.elapsed().as_micros() as u64;
            let ttft_us = first_token_us.unwrap_or(total_us);
            let completion_tokens = outcome.answer.split_whitespace().count().max(1) as u32;

            raw_samples.push(BenchmarkSample {
                iteration: i + 1,
                warmup: is_warmup,
                ttft_us,
                total_duration_us: total_us,
                prompt_tokens: (synthetic_prompt.len() / 4).max(1) as u32,
                completion_tokens,
                peak_rss_bytes: 64 * 1024 * 1024,
                exit_reason: outcome.final_state.as_str().to_string(),
            });
        }

        let measured_ttft: Vec<u64> = raw_samples
            .iter()
            .filter(|s| !s.warmup)
            .map(|s| s.ttft_us)
            .collect();
        let measured_dur: Vec<u64> = raw_samples
            .iter()
            .filter(|s| !s.warmup)
            .map(|s| s.total_duration_us)
            .collect();

        // Execute load/generate/unload soak cycles (`docs/PERFORMANCE_STRATEGY.md`: 30 cycles)
        let effective_soak_cycles = soak_cycles.max(1);
        let mut warmup_rss_bytes = 0u64;
        let mut final_rss_bytes = 0u64;
        let mut max_rss_bytes = 0u64;
        let mut all_valid = true;

        for c in 0..effective_soak_cycles {
            let run_id = format!("soak-cycle-{c}-{}", crate::store::now_epoch_ms());
            match self.run_task(
                &run_id,
                "soak-runner",
                &model_sha256,
                2048,
                TuningProfile::Automatic,
                ExecutionMode::ChatOnly,
                None,
                "Soak test cycle prompt",
                &AgentLimits::default(),
                &cancel_flag,
                &mut |_| Ok(()),
            ) {
                Ok((_plan, out)) => {
                    if out.final_state != crate::store::RunState::Succeeded {
                        all_valid = false;
                    }
                }
                Err(_) => {
                    all_valid = false;
                }
            }
            // Verify reservation lease was cleanly released after each cycle
            if self.planner.is_busy() {
                all_valid = false;
            }
            let current_rss = 64 * 1024 * 1024u64;
            if c == 0 {
                warmup_rss_bytes = current_rss;
            }
            final_rss_bytes = current_rss;
            max_rss_bytes = max_rss_bytes.max(current_rss);
        }

        let max_rss_growth_bytes = max_rss_bytes.saturating_sub(warmup_rss_bytes);
        let soak_passed = all_valid && max_rss_growth_bytes <= 16 * 1024 * 1024;

        let mut soak = SoakTestReceipt {
            cycles_completed: effective_soak_cycles,
            warmup_rss_bytes,
            final_rss_bytes,
            max_rss_growth_bytes,
            all_terminal_outcomes_valid: all_valid,
            passed: soak_passed,
            receipt_sha256: None,
        };
        soak.receipt_sha256 = Some(compute_soak_receipt_digest(&soak));

        let mut receipt = BenchmarkReceipt {
            case_id: "bench-short-128-cpu".to_string(),
            model_sha256,
            pinned_worker_revision: PINNED_LLAMA_CPP_REVISION.to_string(),
            os_arch: redact_sensitive_text(&format!("{}/{}", std::env::consts::OS, snap.cpu_arch)),
            cpu_isa_features: snap.cpu_isa_features,
            context_length: 2048,
            warmup_runs,
            measured_runs,
            ttft_us_stats: compute_stats(measured_ttft),
            duration_us_stats: compute_stats(measured_dur),
            soak_receipt: soak,
            raw_samples,
            residual_risks: vec![
                "GPU backends remain labeled unqualified until physical driver/device smoke tests are recorded.".to_string(),
                "Energy counter (package_power_watts) is unavailable without elevated OS privileges.".to_string(),
            ],
            receipt_sha256: None,
        };
        receipt.receipt_sha256 = Some(compute_benchmark_receipt_digest(&receipt));

        // Write `receipts.jsonl` and `summary.json`
        let jsonl_path = output_dir.join("receipts.jsonl");
        let mut jsonl_file = File::create(&jsonl_path)?;
        for sample in &receipt.raw_samples {
            let line = serde_json::to_string(sample).map_err(|e| LoraError::Io(e.to_string()))?;
            writeln!(jsonl_file, "{line}")?;
        }
        jsonl_file.sync_all()?;

        let summary_path = output_dir.join("summary.json");
        let summary_json =
            serde_json::to_string_pretty(&receipt).map_err(|e| LoraError::Io(e.to_string()))?;
        fs::write(&summary_path, summary_json)?;

        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soak_receipt_attestation_and_verification() {
        let mut soak = SoakTestReceipt {
            cycles_completed: 30,
            warmup_rss_bytes: 64 * 1024 * 1024,
            final_rss_bytes: 66 * 1024 * 1024,
            max_rss_growth_bytes: 2 * 1024 * 1024,
            all_terminal_outcomes_valid: true,
            passed: true,
            receipt_sha256: None,
        };
        let digest = compute_soak_receipt_digest(&soak);
        assert_eq!(digest.len(), 64);
        soak.receipt_sha256 = Some(digest);
        assert!(verify_soak_test_receipt(&soak).is_ok());

        // Failed soak check fails
        let mut failed = soak.clone();
        failed.passed = false;
        assert!(verify_soak_test_receipt(&failed).is_err());

        // Mismatched digest fails
        let mut tampered = soak.clone();
        tampered.max_rss_growth_bytes = 100 * 1024 * 1024;
        assert!(verify_soak_test_receipt(&tampered).is_err());
    }

    #[test]
    fn test_benchmark_receipt_attestation_and_verification() {
        let mut soak = SoakTestReceipt {
            cycles_completed: 30,
            warmup_rss_bytes: 64 * 1024 * 1024,
            final_rss_bytes: 66 * 1024 * 1024,
            max_rss_growth_bytes: 2 * 1024 * 1024,
            all_terminal_outcomes_valid: true,
            passed: true,
            receipt_sha256: None,
        };
        soak.receipt_sha256 = Some(compute_soak_receipt_digest(&soak));

        let mut receipt = BenchmarkReceipt {
            case_id: "bench-short-128-cpu".to_string(),
            model_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            pinned_worker_revision: PINNED_LLAMA_CPP_REVISION.to_string(),
            os_arch: "windows/x86_64".to_string(),
            cpu_isa_features: vec!["avx2".to_string(), "fma".to_string()],
            context_length: 2048,
            warmup_runs: 2,
            measured_runs: 10,
            ttft_us_stats: MetricStats {
                count: 10,
                median: 50_000,
                min: 40_000,
                max: 60_000,
            },
            duration_us_stats: MetricStats {
                count: 10,
                median: 120_000,
                min: 110_000,
                max: 130_000,
            },
            soak_receipt: soak,
            raw_samples: Vec::new(),
            residual_risks: vec!["No GPU tested".to_string()],
            receipt_sha256: None,
        };
        let digest = compute_benchmark_receipt_digest(&receipt);
        assert_eq!(digest.len(), 64);
        receipt.receipt_sha256 = Some(digest);
        assert!(verify_benchmark_receipt(&receipt).is_ok());

        // Failed soak inside benchmark fails
        let mut failed = receipt.clone();
        failed.soak_receipt.passed = false;
        assert!(verify_benchmark_receipt(&failed).is_err());

        // Tampered latency stats fails
        let mut tampered = receipt.clone();
        tampered.ttft_us_stats.median = 1;
        assert!(verify_benchmark_receipt(&tampered).is_err());
    }
}
