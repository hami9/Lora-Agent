# Development roadmap

Use exit evidence to schedule work. The primary dependency chain is a qualified native fixture → supervised generation → conservative admission → a verified read-only task → an offline install. Future UI, training and extension work have independent qualification gates. No delivery dates are assumed.

## First ten engineering tasks

These are implementation tickets for the next phase. They do not describe code already in the repository.

| ID | Objective and owned components | Depends on | Completion criteria and validation |
| --- | --- | --- | --- |
| T1 | Select one redistributable small text GGUF or a reproducible locally supplied fixture. Record base, template, quant, SHA-256, upstream revision, compiler flags, license and three target machines. | Architecture review | Native runner produces a recorded offline baseline and a structured read-tool proposal on representative prompts. If no model qualifies, gate agent release and continue chat development. |
| T2 | Bootstrap Cargo workspace, config resolution, per-user data paths, redacted structured errors and protocol DTO fixtures. | Design contracts | Three target OS builds in CI, clean profile starts with no network, stale/newer protocol majors rejected in unit fixtures. |
| T3 | Build registry, single writer, migrations, staging and atomic verified imports. Separate bytes from metadata. | T2 | Duplicate import, truncated model, disk-full and interrupted promotion tests preserve registry and existing files. |
| T4 | Implement worker shim and host supervisor with inherited pipes, framed messages, watchdog, process-tree cleanup and explicit capability handshake. | T1, T2 | Fake and real workers load an allowlisted fixture. Fault-injected corrupt frame, hung startup, incompatible revision and child exit become typed terminal errors. |
| T5 | Implement HIE snapshot, estimate and single reservation with conservative CPU baseline. | T1, T2 | Golden model header cases, stale/missing telemetry, over-budget admission and simultaneous requests pass. Validate estimated vs observed peak on the smallest target. |
| T6 | Deliver CLI chat vertical slice including import discovery, plan explanation, load, streaming, Ctrl-C and unload. | T3–T5 | M1–M4 pass, incl. slow consumer, cancellation race and second invocation under store lock. |
| T7 | Build scoped read executor and trusted approval UI on reference platforms. Bind actual opened file, budget, purpose and run. | T2 | Test denial, revoke, traversal, symlink/reparse substitution, pseudo-files, size cap and log redaction on each OS. |
| T8 | Add bounded agent state machine, structured proposal parser, evidence references and task journal. | T3, T6, T7 | M5–M7 pass on adversarial fixtures. No tool replay on restart. Score cited answers separately from abstentions. |
| T9 | Run native-vs-host benchmarks, failure-recovery soak and offline egress checks with fixed build/model inputs. | T6, T8 | Publish raw anonymized measurements, comparison methodology and residual risks; model/driver combinations have measured or unknown status. |
| T10 | Produce qualified offline packages, SBOM/notices, integrity data and support matrix. Exercise update and rollback from preceding test version. | T1–T9 | Clean-machine M8 sign-off for each advertised tuple, signed candidate where required, documentation agrees with shipped features. |

## Phases and release gates

For each phase the evidence in the last column is its definition of done. Proposed parallel lanes are below the table.

| Phase | Prerequisites | Scope, engineering work and deliverable | Primary risk / fallback | Acceptance evidence |
| --- | --- | --- | --- | --- |
| 0 Research and architecture | Product brief | Compare engines, write ADRs/contracts, select benchmark fixture and threat model. Deliver this docs package, then run T1 experiments. | Upstream example omits the needed template / change fixture | Review architecture and contested choices. Record license, native baseline, tool-call result and rule for updating the worker pin. |
| 1 Foundation | Reviewed boundaries | Workspace, OS facade, configuration, journal, redaction, build matrix. T2 and first part of T3. | Paths/locks differ across OSes / support fewer platforms first | Fresh and migrated profiles open consistently. Parallel open, permission failure and read-only-volume cases pass. |
| 2 Local runtime | Phases 0–1 | T3, T4 and T6 native path. Registry, native shim, streaming CLI and cancellation. | Native ABI drift / keep last verified pin | Golden protocol conformance and real-model import → stream → stop → reload on a named CPU. |
| 3 Hardware intelligence | Model metadata + worker capability shape | T5 then integration in T6. Choose CPU limits and expose a request-specific fit explanation. Start GPU experiments outside required build. | Unknown free VRAM / refuse or use conservative CPU plan | Boundary tests for unknown budget, OOM and stale sensor sample, paired estimated/observed peak traces. |
| 4 Desktop client | Stable application facade and MVP security model | Tauri spike, model library, chat, Stop and native approval surface. Package independently of CLI. | Webview differences / keep CLI first | Keyboard-only offline journey, permission spoofing tests and startup/idle budget on oldest supported device. |
| 5 Agent runtime | Phase 1 policy types, phase 2 generation, phase 3 limits | T7 and T8 establish the MVP read loop. After qualification, add previewable workspace writes in a separate milestone. | Weak tool calling / disable agent mode for that model | M5–M7 and a verifiable local answer; all escaping paths denied. A mutation release separately tests undo and uncertain completion. |
| 6 Extensions and memory | Phase 5 policy + revocation | Add opt-in MCP discovery, scoped ingestion, FTS and provenance before a Wasm host or vector index. Add opt-in provider/API adapters under their own consent gate. | Uncontained native server or egress / restrict to trusted installs or keep disabled | Revoke mid-job, delete/rebuild index and trace one retrieval to approved content. Provider test intercepts payload and matches the consent preview. |
| 7 Advanced hardware | Phases 2–3, T9 baselines | Qualify one accelerator first, then trial auto-tuning, multi-device placement and cache variants. | Driver regressions / quarantine tuple and retain CPU | Repeatable improvement within measured memory/thermal limits. Regression and cancellation pass for each named driver. |
| 8 Adapter Studio | Registry + admission + separate trainer proof | Add narrowly scoped data preparation, plain LoRA, checkpoints and held-out evaluation. Broader QLoRA and merge paths need separate qualification. | Adapter ABI or quality regression / export experimental result without promoting | Matched base/model digest, reproducible run recipe, held-out comparison, and verified target-runtime load before installation. |
| 9 Production hardening | Each candidate feature has functional tests | T9–T10 for the MVP, repeated per later feature. Sign packages, check updates/rollback, verify migration recovery and vulnerability response. | Packaging and signing cost / narrow published target set | Fresh offline install plus full release checklist, restored backup and published limitations tied to exact artifact hashes. |

## Implementation order and parallel work

Run T1 and T2 in parallel once the contract is approved. T3, T4, T5 and T7 can proceed against the same fixtures on separate branches. T6 joins registry, inference and admission. T8 joins T6 and T7. T9 and T10 close the first release. This takes the minimum pieces of phases 0, 1, 2, 3, 5 and 9. GUI design and accelerator experiments can proceed alongside it, without becoming MVP dependencies. Begin indexing, plugins or training only after the relevant permission and lifecycle interfaces have stable conformance cases.

Track budgets in the [benchmark plan](BENCHMARK_PLAN.md), use the [MVP acceptance suite](MVP_SPECIFICATION.md) for release gates, and update the [risk register](RISK_REGISTER.md) when an experiment invalidates an assumption.
