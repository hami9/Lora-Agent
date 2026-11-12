# Technology stack

Selections below are design commitments for the first implementation. Shipping pins belong in the first build's lockfile, upstream revision manifest and SBOM. Research citations establish upstream behavior. Lora still has to qualify its own binaries.

| Surface | Initial selection | Why it earns the dependency / cost |
| --- | --- | --- |
| Application and CLI | Stable Rust, Cargo workspace, clap, serde, tracing | Typed commands and ownership simplify review of handles, cancellation and tool arguments. One binary can host the core. Evaluate distribution size and idle memory on the smallest target. |
| I/O scheduling | Tokio with a bounded channel per stream and explicit deadlines | Coordinate pipes, signals and cancellation. Run filesystem hashing and blocking SQLite work outside the reactor. Avoid one native thread per token or holding a transaction across an await. [Tokio guide](https://tokio.rs/tokio/topics/bridging) |
| Text inference | Pinned llama.cpp CPU build, small worker shim, C API confined to that binary | Gives the project an upstream GGUF path and native kernels with a replaceable adapter. Compare to the same revision's runner before optimizing IPC. [Upstream](https://github.com/ggml-org/llama.cpp) |
| Private transport | Versioned, length-prefixed UTF-8 JSON on inherited pipes, bounded token chunks | Debuggable and testable across target OSes, independent of native ABI. Model weights stay on disk. Binary serialization/shared memory must show a measured benefit. |
| Registry, tasks and audit | SQLite through rusqlite, one writer, content-addressed local model files | Transactions protect related metadata. Metadata remains portable and queryable. Start with local storage, busy handling, recovery tests and migration backups. [Binding](https://docs.rs/rusqlite/latest/rusqlite/) · [SQLite journaling](https://www.sqlite.org/wal.html) |
| Device discovery | sysinfo for basic host data, small OS adapters and direct worker capability probes | Distinguishes a detected adapter from a usable backend. Vendor libraries load optionally. OS-specific uncertainty travels with the plan. [Hardware specification](HARDWARE_INTELLIGENCE.md) |
| Tests and distribution | Cargo tests with a fake worker plus opt-in real-weight qualification; GitHub Actions for documentation now | Hosted runners can prove protocols and packaging. Physical reference machines are required for driver, energy and thermal claims. [CI/CD](CI_CD.md) |

## Later selections

The desktop client (`lora-desktop`) uses **pure-Rust `iced 0.14`** (`wgpu` + `tiny-skia` software fallback, native Markdown progressive streaming, and `cosmic-text` RTL/Unicode shaping) in a Discord-inspired multi-pane desktop layout. Application policy, hardware admission, and `ApprovalPromptPort` trusted consent dialogs run directly in-process over `LoraApplicationService` with zero WebView, Electron, or HTTP loopback UI overhead.

Use an opt-in HTTP adapter (candidate axum) and SSE for external clients, an OpenAPI document for generated types, and native OS credential stores for remote keys. MCP integration comes before a general extension marketplace. A Wasmtime host with explicit imports is the preferred programmable extension path after policy maturity. A dedicated trainer is a separately installed component. Each choice has an integration and removal gate. [API](API_AND_SDK.md) · [Extensions](PLUGIN_SYSTEM.md) · [Training](TRAINING_SYSTEM.md)

## Licensing decision

Recommend Apache-2.0 for original project code, subject to owner adoption. Its explicit patent grant and distribution terms suit an integration-heavy project. MIT has shorter notice requirements. Dual MIT/Apache-2.0 gives recipients a choice at the cost of managing both texts and contribution terms. Do not label the repository or a build with a project license until the owner adopts it. Review attribution, bundled libraries, model weights, adapters, fonts, artwork and accelerator redistributables separately for each distribution. An SPDX row for an engine does not license a model. Keep an approved notices manifest in each future binary release. [Apache-2.0 text](https://www.apache.org/licenses/LICENSE-2.0) · [MIT text](https://opensource.org/license/mit)

## Decision records

1. [Rust and native code](adr/0001-language-strategy.md)
2. [Inference backends](adr/0002-inference-backends.md)
3. [Process isolation](adr/0003-process-isolation.md)
4. [Desktop framework](adr/0004-desktop-framework.md)
5. [Storage](adr/0005-storage.md)
6. [Agent runtime](adr/0006-agent-runtime.md)
7. [Security model](adr/0007-security-model.md)
8. [Extension runtime](adr/0008-extension-runtime.md)
9. [Platform integration](adr/0009-platform-integration.md)
10. [Training backend](adr/0010-training-backend.md)
