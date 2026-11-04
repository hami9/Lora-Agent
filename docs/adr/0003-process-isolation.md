# 0003 · Native worker isolation

Status: proposed design baseline, 2026-11-04.

## Context

A native model parser or device driver can crash or hang. Loading it directly into the host would also jeopardize grants, audit state and the user's session.

## Decision

Supervise a version-matched worker owned by the current host. Use private inherited anonymous pipes, bounded frames and a per-launch nonce. Keep protocol stdout separate from diagnostic stderr. Apply OS restrictions and verify containment for supported artifact classes.

## Alternatives

In-process FFI saves a transport hop but shares all failure state. An upstream HTTP server eases integration but adds local network exposure and versioned server semantics. Shared memory introduces ownership and reclamation complexity.

## Tradeoffs

Extra launch time, resident pages and serialization must be measured. Weight bytes and KV buffers stay outside the JSON stream. Process separation alone does not constrain a malicious child with the user's OS rights.

## Consequences

Supervisor owns child reaping and resource release. Reject invalid frames, nonce or major-version mismatch. Kill/reap on timeout after cooperative cancellation. A crash never resumes the previous native session.

## Conditions for reconsideration

Consider binary framing for payloads after trace evidence of IPC bottlenecks. Consider broader file acceptance only when OS isolation and adversarial parser tests pass on that platform.

[Detailed specification](../INFERENCE_ENGINE.md)
