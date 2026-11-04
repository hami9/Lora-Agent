# 0009 · Platform qualification

Status: proposed design baseline, 2026-11-04.

## Context

Windows reparse points, Linux namespaces and macOS privacy prompts demand different implementations. A successful cross-compile says little about runtime access controls.

## Decision

Put native OS adapters behind typed capability ports with unsupported and permission-denied outcomes. Qualify Windows/Linux x86-64 and macOS arm64 CPU candidates separately. GPU support requires a separate backend/device/driver record.

## Alternatives

Per-OS products duplicate policy and UX. A universal shell abstraction is expedient but hard to scope. WSL and emulation are separate environments with separate path and device semantics.

## Tradeoffs

Selective feature availability complicates support communication but lets the team ship a defensible support matrix. GPU monitoring can fail independently of inference.

## Consequences

Feature detection is runtime and per capability. Keep desktop automation and future privileged broker opt-in. Require handle-race tests and fresh install evidence before naming a target supported.

## Conditions for reconsideration

Expand to ARM Windows/Linux, Intel macOS or other Unix only when a maintainer supplies a continuous test machine, package and evidence for the same gates.

[Detailed specification](../PLATFORM_SUPPORT.md)
