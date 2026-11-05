# lora-llama-worker

Isolated inference worker. It speaks length-prefixed v1 JSON over stdin/stdout (see
`docs/INFERENCE_ENGINE.md`) and has two backends:

- **deterministic** (default build): no model is run; replies are synthesized offline. Used by the
  acceptance suites.
- **native** (`--features native-llama-cpp`): runs a real GGUF model through the pinned llama.cpp
  (tag `b12125`, submodule at `vendor/llama.cpp`) via a small C++ shim in `native/`. CPU only.

## Building the native backend

Requirements: Rust, CMake, and a C++17 compiler (MSVC Build Tools on Windows). No libclang or
bindgen is needed.

```sh
# Windows only, llama.cpp has very long paths:
git config core.longpaths true
git submodule update --init --depth 1 workers/llama/vendor/llama.cpp

cargo build -p lora-llama-worker --features native-llama-cpp --release
```

Building without the submodule fails with a message pointing at the command above. The first
build compiles llama.cpp (a few minutes); later builds reuse it.

On x86_64 the native build is compiled for x86-64-v3 and requires a CPU with AVX2, FMA, F16C and
BMI2 (Haswell/Zen or newer). There is no runtime CPU detection; older CPUs will crash on an illegal
instruction.

## Smoke tests (real model)

```sh
LORA_TEST_GGUF=/path/to/qwen2.5-0.5b-instruct-q4_k_m.gguf \
  cargo test -p lora-llama-worker --features native-llama-cpp --release -- --ignored --nocapture
```

They spawn the worker, run the Hello and LoadModel handshake, then check a short answer, cancel
latency and tool-call mode. Model files are never stored in the repository.

## Environment variables

| Variable | Meaning |
| --- | --- |
| `LORA_WORKER_BACKEND` | `deterministic` forces the fake backend even in a native build; `native` requires the feature (otherwise `LoadModel` fails with `unsupported_capability`). Unset: native if compiled in, else deterministic. |
| `LORA_WORKER_LLAMA_LOG` | `1` sends llama.cpp logs to stderr. Off by default; stdout is the protocol channel and never receives logs. |
| `LORA_WORKER_FAULT` | Test-only fault injection (`crash_on_boot`, `crash_on_load`, `crash_mid_decode`, `corrupt_on_boot`, `corrupt_on_load`). |
| `LORA_TEST_GGUF` | Path of the GGUF used by `tests/native_smoke.rs`. |
| `LORA_TEST_THREADS` | Optional thread count for the smoke test (default: worker picks half the logical CPUs). |

## Behaviour notes

- A `Cancel` for the in-flight native generation is handled by a reader thread that raises an abort
  flag polled by llama.cpp, so cancellation interrupts prefill and decode. The terminal event is
  `GenerationComplete` with `finish_reason: cancelled`.
- The host prompt is wrapped with the model's own chat template. With `tool_calling_enabled` a system
  message asks for `<tool_call>{...}</tool_call>`. A valid call becomes a tool proposal and is not streamed;
  a malformed or non-conforming call finishes with `stop` and is shown as plain text.
