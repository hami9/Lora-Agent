# Performance strategy

Measure the overhead of the product separately from model execution. Rust is valuable for memory-safe orchestration, predictable ownership and native distribution. When two applications invoke the same C/C++ kernels, their language labels do not explain differences in prefill or decode rate. Split time into queue, setup/tokenization, load, prefill, decode and delivery. Compare the private worker with the same engine revision and settings.

## Budgets to validate

The numbers below are *proposed engineering thresholds*, not results, guarantees or hardware minimums. Calibrate the thresholds on named reference devices before turning them into release gates. Measure with identical fixtures and report run dispersion. Treat failed or skipped samples as outcomes.

| Stage | Proposed gate | Decision if missed |
|---|---|---|
| Functional | 100% correct terminal outcomes on the deterministic cancel, kill and disconnect fixtures; one final event per request. No unbounded queue growth or retained worker after timeout | Block the release and inspect supervisor/lease lifecycle |
| Functional | For 30 load/generate/unload cycles, no monotonic private-memory growth beyond a measured allocator warm-up envelope | Compare core versus child peaks, repeat with child reap and inspect mappings |
| Competitive | Warm single-session decode throughput at least 90% of direct llama.cpp with identical settings; median first-token overhead within 10% or 100 ms, whichever is larger | Profile templating, IPC buffering, UI notification and measurement boundaries |
| Competitive | Core/CLI idle CPU below 1% of one logical CPU averaged over 60 seconds on reference hardware, with no loaded worker | Reduce polling, stop background sampling while idle, compare platform-counter conventions |
| Long term | Automatic plans avoid false-safe OOMs on the published qualification set and stay within 15% of the fastest eligible measured configuration on at least 80% of that set | Increase estimator reserve, narrow eligible tuples or leave manual tuning available |

## Instrumentation

Attach monotonic timestamps and a request ID to handshake, admission, first byte, first visible content, last token, cancellation and reaping. Keep only timings, count units and build/device metadata by default. Human-visible time to first token includes queue and renderer; token-generation metrics exclude UI delivery. Report process-tree resident/private memory, file-backed mappings and driver allocation peaks separately. On unified memory, avoid summing RAM and VRAM as independent physical use. Say unavailable where counters require special privileges or offer only coarse estimates.

Bound event buffering; coalesce small deltas for rendering while flushing the terminal event. Run tensor work outside Tokio's async executor, throttle hardware polling and suspend visualization when hidden. Prefer mmap and engine-owned buffers where supported; directly compare cold page faults, SSD versus network storage, and warm reuse. Capture native CPU flame graphs and GPU traces only on opt-in synthetic workloads. No private prompts or filesystem contents in public profiles.

## Regression policy

PR CI uses deterministic tests and simulated resource counters. Scheduled pinned hardware runs compare a candidate against the last passing build on the same machine and a native engine baseline. Recheck suspicious changes after clearing residual workers and normalizing power state. An amber result with large dispersion calls for repeated testing; an established regression requires an owner, reason and expiry before waiver. Driver or backend upgrades reset comparison strata and rerun unsafe-input and cancellation tests. [Reproduction protocol](BENCHMARK_PLAN.md)
