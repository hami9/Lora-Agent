# Benchmark plan

This is an experiment design. No measured performance results or validated hardware support claims accompany the architecture. Store machine-readable receipts and summaries under an explicit user-selected results directory; public reports use synthetic prompts and redact usernames, hostnames, paths, serial numbers and private model IDs.

## Fixture and controls

1. Pin a redistributable small dense text model, quantized weight SHA-256, template, tokenizer, tool grammar if present, fixture prompts, model license and output count. Include a larger boundary artifact that approaches the machine's RAM limit. Publish hashes, permitted acquisition steps and exact tokenized inputs. Reject comparisons where prompt token IDs or effective context differ.
2. Capture OS/driver, BIOS or firmware where relevant, available/installed memory, GPU type and budget, power mode, battery/AC, ambient throttling indications, storage, runtime source/build flags, app revision, ISA, thread affinity, batch/microbatch, offload layers, KV type, memory mapping, sampling settings and seed.
3. First compare the same llama.cpp revision via native benchmark and via the private worker. Then compare end-to-end request paths with upstream server, Ollama local API and mistral.rs on that same checkpoint/quant, using explicit options. Verify Ollama's imported digest and template and verify mistral.rs supports the artifact. A different quantization, tokenizer or model makes the result a separate scenario. Publish incompatibility or inability to match settings as its own outcome. [llama-bench](https://github.com/ggml-org/llama.cpp/blob/c6a0ff11a0fe16ac1a2eb569190934593a2b3670/tools/llama-bench/README.md) · [mistral.rs GGUF](https://docs.mistralrs.dev/reference/gguf-support/)
4. Run a short 128-token input and a long 2,048-token input when they fit, with a fixed 128-token decode workload. For synthetic throughput suppress early stop consistently if all compared engines support it. Keep a separate realistic EOS-enabled chat fixture. Run one cold app start, distinguish first model load with OS cache unknown from documented cold-cache procedures, warm twice, then collect at least 10 measured repetitions per cell. For reported tail latency collect at least 100 samples or omit p95. Interleave candidates in seeded random order and record raw samples and failed runs.
5. Run CPU-only on Windows/Linux x86-64 and Apple Silicon macOS candidates. Test accelerated variants separately on named real hardware, including a low-memory refusal case. Headless CI may perform deterministic protocol tests only. Publish no GPU performance numbers from a CPU-only host.

## Measurements

| Metric | Definition |
|---|---|
| Startup and load | Process launch to ready handshake; separate admission-to-loaded time and first-use lazy faults |
| Time to first token | Request submission to first nonempty text delta visible at consumer. Report cold and warm, with queue time and prompt length |
| Prefill and decode | Actual prompt tokens processed per prefill second and generated tokens per decode second. Exclude loading; declare whether first token is counted. Report engine counters and end-to-end separately |
| RAM, VRAM, energy | Sample parent and worker trees and driver counters at a fixed recorded interval; include engine allocation high-water marks for sub-sample peaks. Report available counter units, attribution limits and idle-subtracted joules per generated token where possible |
| Concurrency | One request, then two and four contenders. Record queue delay, busy rejections, aggregate throughput, per-request latency and fairness. The MVP's single active generation must remain observable |
| Agent success | Frozen task fixtures with inspectable filesystem/audit postconditions. Separate correct completion, correct refusal, unexpected approval, premature claim and unauthorized side effect. Report distributions across seeds |
| Reliability | Cancel during prefill and decode, slow reader, worker kill, allocation failure, thermal pressure, depleted disk and repeated reload. Count time to terminal, released reservations and subsequent successful runs |

## Receipts and decisions

Each receipt includes case ID, artifact digests, configuration snapshot, monotonic event samples, effective token counts, memory source/units, exit reason and runner/environment versions. Save raw lines plus aggregation method. Compare paired medians, ranges and bootstrap intervals over repeated paired runs. Never replace a crash with an omitted run. Disclose missing sensors, throttling, cache state or sample-size limits. Report quantization quality on fixed tasks whenever representations differ.

| Prototype decision | Evidence required | Default until qualified |
|---|---|---|
| Private worker transport | Compare event latency, CPU and memory for per-token vs coalesced frames; kill/restart during load and generation | Framed pipes, bounded queue, one active generation |
| GPU qualification | Exact driver/build tuple passes CPU-vs-GPU output checks, safe load, cancellation, repeated unload and memory plan | CPU only |
| Add mistral.rs | Document a model coverage gap or matched improvement with correct outputs; measure bundle and upgrade cost | Keep one backend |
| HIE automatic tuning | Compare safe defaults with exhaustive bounded grid on representative memory/power profiles, including held-out machines | Conservative profile with explainable refusal |
| Concurrent scheduling/KV reuse | Measure p95 interactive latency, peak KV overhead and cross-session isolation under mixed load | Serial generation, no cross-user prefix reuse |
| Optional trainer | Measure held-out tasks, regression, peak RAM and exported adapter conformance against the exact base | Experimental job only; no general support claim |
| Desktop and packaging | Compare cold launch, idle RSS/CPU and installed size of packaged desktop against CLI on the lowest target | Reuse one core and lazy-load worker and panels |

The first qualification report must state hardware availability and recreate both successful and failing cases using only the recorded configuration. Thresholds for performance regressions and resource-safe admission are proposed in the [performance strategy](PERFORMANCE_STRATEGY.md).
