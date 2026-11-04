# 0006 · Bounded agent loop

Status: proposed design baseline, 2026-11-04.

## Context

Model output, user intent and completed effects have different trust levels. A tool request can be malformed, repeated or emitted just before cancellation.

## Decision

Use one deterministic state machine around a model proposing typed operations. Host-side validation, approval, bounded execution and observed evidence decide whether a run can finish. MVP exposes only one scoped read tool.

## Alternatives

A generic agent framework shortens provider integration but may obscure effect ordering. Multiple planners or verifier agents can improve some tasks while multiplying budgets and uncertain outcomes.

## Tradeoffs

A small custom loop makes recovery inspectable. It limits flexibility until new tools gain schemas, failure modes and fixtures. Empirical task success still depends on a qualified model.

## Consequences

Persist step IDs, transitions and grant references. Treat a recovered nonterminal side effect as uncertain. Never retry an effect based only on a model instruction. Enforce turn, time, token and byte limits.

## Conditions for reconsideration

Adopt library pieces when they preserve authorization and idempotency tests. Add specialized agents only if ground-truth task success improves at a measured cost and handoffs cannot widen grants.

[Detailed specification](../AGENT_RUNTIME.md)
