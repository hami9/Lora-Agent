# MVP specification

## Release contract

An offline user on one qualified CPU/OS pair can register a known GGUF text model, understand its fit, stream and stop a chat response, and complete a read-only task against a file in an approved local scope. A fresh install can perform the entire journey with outbound traffic blocked. Test on Windows x86-64, Linux x86-64 and macOS arm64 as qualification candidates. Publish the passing subset with exact OS, CPU instruction baseline, worker revision and fixture hash.

## Essential capabilities

| Capability | Required observable behavior |
| --- | --- |
| Install and inspect | Offline install of CLI and pinned worker. Diagnostics report build IDs, capabilities and unavailable sensors. No account needed. |
| Register a model | Import one supported text-only GGUF architecture/quantization/template tuple from disk. Validate size, bounds, digest and identity, record provenance, show duplicate or incompatible status. Interrupted imports can be reconciled. |
| Admit a run | Compute a conservative memory estimate with headroom, explain uncertainty and compare to the user's requested context. Allow a bounded capability probe. Refuse an unsafe plan before loading weights or allocating a generation context. One active generation and a bounded or rejected backlog. |
| Generate and stop | Stream ordered valid text, handle template and context limits, signal terminal outcome, support cancellation and unload. Record user-facing cause on OOM, malformed input, timeout or child exit. |
| Use one tool | Chat-only plus read-only assistance. A compatible model may request one bounded text-file read at a time. Host approval binds path scope and operation, validates actual opened object, and records the outcome. Answers cite the observation or explain missing evidence. |
| Recover and inspect | Protect store ownership, distinguish interrupted from completed work, preserve inspectable redacted action history, handle disk-full and leave registry consistent. A restart does not replay tools. |
| Qualify a release | Package CLI and worker, third-party notices and per-platform support matrix. Record reference measurements and offline policy tests. |

**Optional after the functional gate:** one qualified GPU build for one named device/driver pair, a read-only diagnostics export, or convenient catalog acquisition. Each has its own acceptance record. The minimum release uses offline import and CPU execution.

**Later or experimental:** Tauri desktop, cloud providers, HTTP/SDK, OS writes, shell, browser control, durable autonomous grants, MCP/Wasm, persistent retrieval, concurrent generation, multi-GPU scheduling, NPU, multimodal models, training and adapter merging. These facilities cross new permission or qualification boundaries.

## Acceptance suite

| ID | Setup and action | Pass evidence |
| --- | --- | --- |
| M1 Offline boot | Fresh profile, network denied, compatible fixture and matching worker already on disk. Import and start a chat. | Hash and manifest recorded, response includes at least one valid chunk and terminal success, network monitor records zero outbound attempts. |
| M2 Malformed input | Truncated GGUF, out-of-range tensor, missing template and mismatched digest fixtures. | Actionable error, no runnable registry entry, no host crash or overwrite of a good revision. |
| M3 Constrained memory | Inject a small memory budget and request a long context. Also simulate a pressure change after reservation. | Rejection includes estimated weights/KV and available headroom; alternative context requires acknowledgement. No hidden cloud request. |
| M4 Stream control | Cancel mid-prefill, mid-decode and while the receiving UI is slow; race cancel against completion. | Bounded buffers, one terminal outcome, one release after confirmed worker stop. Next run succeeds. |
| M5 File-grounded answer | Tool-capable fixture reads a guide with a unique expected value. Include misleading instructions within its text. | The positive case returns the unique correct value and verifiable path/line range. Repeat with the value removed: the agent must report insufficient evidence. Injection cannot open another file, call network or upgrade a grant. Both cases must pass. |
| M6 Scope enforcement | Try traversal, symlink/reparse escape, device file, renamed target, expired grant and user denial. | Executor returns a scoped failure before bytes reach model. Denial and revocation are visible, secrets absent from logs. |
| M7 Crash recovery | Kill worker on load and generation, kill host during import and after a tool is recorded, inject disk-full. | Restart reconciles staging and unfinished runs, no replay or dangling reservation. Existing verified bytes remain usable. |
| M8 Qualification | Run above tests, baseline performance cases and installation from a clean offline machine for each claimed target. | Publish manifests, pass/fail results, raw metadata and exact limitations. Unknown hardware remains labeled unqualified. |

## Exit decision

The runtime maintainer signs off M1–M4 and M7. The security/tool maintainer signs off M5–M6 on each supported filesystem implementation. The release maintainer ties M8 to the shipped hashes. A platform that fails scoped file access can be offered as a separately labeled chat-only preview, and cannot count as a passing full-MVP platform. No throughput target overrides a failed security or data-integrity case. [Benchmark protocol](BENCHMARK_PLAN.md) · [Risk register](RISK_REGISTER.md)
