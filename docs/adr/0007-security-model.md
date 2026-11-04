# 0007 · Capabilities and approval

Status: proposed design baseline, 2026-11-04.

## Context

Local OS access can expose source, secrets or destructive commands. Prompt injection can enter through model files, attached documents, plugins and tool results.

## Decision

Default deny. Bind grants to the caller, run, capability, normalized resource, limits and expiry. Check the actual OS object at execution. Require fresh explicit approval for destructive or privileged effects. Keep audit metadata separate from content.

## Alternatives

Prompt-only controls cannot enforce OS rights. A blanket workspace grant is simpler but obscures irreversible actions. Running the model as admin makes a single failure systemic.

## Tradeoffs

Handle-based checks and OS-specific containment cost engineering time. Short-lived grants add interaction. Honest feature unavailability is preferable when the required guarantees cannot be established.

## Consequences

Model and plugin outputs are data. Egress and privilege have separate grants. Denial, expiry and revocation have fixtures. All front ends use the same policy service. Local same-user malware is a residual boundary limitation.

## Conditions for reconsideration

Reconsider the grant UX after measuring denial and abandonment. Relax interaction only if a narrower capability and adversarial test demonstrate equivalent protection.

[Detailed specification](../SECURITY_ARCHITECTURE.md)
