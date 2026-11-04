# 0001 · Rust and native code

Status: proposed design baseline, 2026-11-04.

## Context

The application must coordinate I/O, permissions and multiple user-facing surfaces. Most model computation already runs in native kernels. Language choice alone cannot predict decode speed.

## Decision

Use Rust for the CLI, application state, policy, model registry and supervisor. Keep llama.cpp and its backend-specific kernels behind a small native boundary. Isolate unsafe FFI and document allocator, lifetime and thread ownership for every handle.

## Alternatives

An all-C++ host can reuse more native types but expands the memory-safety review surface. A Python host is attractive for training integration and quick experiments but adds a runtime to baseline packaging. A Rust tensor-only stack would take on model and kernel validation earlier.

## Tradeoffs

Two compilers and a pinned native ABI increase build work. Rust's ownership model is most valuable around long-lived sessions, handles, policy and cancellation. Reference-counted buffers or language changes need profiling evidence.

## Consequences

Build manifests include both toolchains. Future Python trainers are optional separate processes. Benchmarks compare applications using identical kernels and model settings.

## Conditions for reconsideration

Revisit if a maintained Rust backend meets the full reference matrix with lower packaging cost, or measured host overhead dominates an important workload.

[Detailed specification](../TECH_STACK.md)
