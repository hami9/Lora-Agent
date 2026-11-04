# API, CLI and SDK

The initial CLI embeds a local Rust application service. It needs no daemon, account or listener. Worker IPC is private to the host and backend. A later REST server, Tauri commands and public SDK consume the application service, so all three see the same model compatibility, authorization and cancellation decisions. Commands and payloads below specify proposed contracts rather than installed commands.

## CLI and service surface

| Example | Service responsibility |
| --- | --- |
| `lora hardware`, `lora doctor` | Inventory and redacted probe diagnostics. Report unavailable fields explicitly. |
| `lora models import ./model.gguf`, `lora models list` | Verify and register an offline artifact. Report digest, compatibility decision and diagnostics. |
| `lora chat --model <id>` | Open a local chat with no tool grant, stream deltas and accept Ctrl+C. |
| `lora agent run --model <id> --root <dir> "Summarize notes.txt"` | Show resource and grant lifetime, then execute the single scoped read tool under user authority. |
| `lora status`, `lora benchmark` | Inspect local state, or explicitly run the qualified benchmark profile when available. |
| `lora models install`, `lora plugins list` | Later catalog/download and extension clients. These must not cause network discovery at startup. |

Define typed service operations for inspect, import, validate plan, begin run, subscribe to run, cancel, approve and revoke. Inputs include the authenticated principal supplied by the trusted adapter. User strings cannot impersonate a principal. Use stable machine-readable error classes for invalid input, unsupported model/capability, incompatible protocol, insufficient resources, busy, denied, expired grant, cancelled, worker lost and storage failure. Human messages redact sensitive paths. Distinguish retryable pre-dispatch failures from ambiguous post-dispatch outcomes. Structured CLI output goes to stdout, diagnostics to stderr. Ctrl+C requests cancellation and waits for cleanup within a deadline.

## Private worker IPC

The supervisor creates private inherited stdio handles and sends a capped length-prefixed JSON handshake with supported protocol majors/minors and a per-launch nonce. Child echoes the nonce, chosen version, role, backend build identity and feature set. A wrong major, nonce or required feature ends startup. No global endpoint is discoverable. Parse frame length before allocation, reject malformed frames and bound buffered events. Only the bundled worker executable may receive the inherited channel. Separate diagnostics on stderr.

The host owns session and request IDs, the generation lease and deadlines. A child reports ordered deltas, terminal status and usage against those IDs. Late frames after cancellation or EOF cannot resurrect a run. Cancellation signals the worker, then terminates and reaps it after a deadline. The service frees the reservation only after confirmed cleanup. A stalled client cannot build an unbounded output queue. Backpressure or terminate with an explicit slow-consumer outcome. Preserve the distinction between a worker error, user stop and an incomplete run. See [inference engine](INFERENCE_ENGINE.md).

## Future local REST and streaming

Use an explicit enable action, bind only to numeric loopback by default, and generate a scoped bearer credential outside URL query strings. Validate Host and Origin, disable wildcard CORS and ambient-cookie authentication, cap request sizes/concurrency and require a valid token even on localhost. Remote service exposure requires a separate deployment design with TLS, authenticated identities, scoped authorization, rate limits and explicit configuration. A client token cannot approve its own escalation. Sensitive approval is reserved for a trusted same-user consent surface.

| Endpoint | Proposed contract |
| --- | --- |
| `GET /v1/capabilities` | Version range, enabled operations, model features and constraints for this principal. |
| `GET /v1/models` | Installed descriptors and compatibility summaries. No absolute paths or secrets. |
| `POST /v1/runs` | Body contains model ID, mode, input, workspace reference and finite limits. A scoped idempotency key and body digest return one run or conflict on changed input. Respond 202 with run ID and event location. |
| `GET /v1/runs/{id}` | Owner-only current state, last sequence and safe terminal metadata. |
| `GET /v1/runs/{id}/events` | Authenticated SSE with event ID, run ID, sequence, event kind and versioned payload. Use a header-capable client. |
| `POST /v1/runs/{id}/cancel` | Idempotent. Return terminal state or cancellation pending. A stream disconnect alone does not cancel a future REST run. |

Return an opaque request ID and safe fields such as `code`, `retryable` and `detail`. Use 400 for invalid schema, 401 for missing auth, 403 for denied capabilities, 404 for invisible resources, 409 for state/capability conflicts, 413 for size limits and 429 for exhausted admission limits. Reconnect with Last-Event-ID inside the finite replay window. Return a replay-gap response and current status when deltas have expired. Do not persist full text solely for replay. Only owners can resume subscriptions.

Publish OpenAPI schemas and compatibility fixtures alongside the server. Generate Rust/TypeScript clients after the API stabilizes. Within a major version, add optional fields and version event payloads. Unknown required capabilities fail negotiation. Test idempotency-key reuse, duplicate deltas, cancellation racing completion, reconnect after expiry, unauthorized cross-run reads, CLI/REST policy parity and a hostile browser origin.

## Optional providers

Define `Provider` with discovery, capability description, token budgeting, stream, cancel and typed error reporting. A local worker advertises capabilities at startup. OpenAI-compatible endpoints and other documented providers receive independent adapters, because structured output, authentication, usage and tool schemas differ. Custom endpoints require explicit registration, HTTPS verification, allowlisted origin and destination revalidation on redirects. Unknown features remain disabled until a provider/model pair passes a contract fixture.

Before any external transmission, show provider, model, destination origin, exact data classes, selected excerpts and cost estimate when a dated price is available. A change to body, destination or attachments invalidates the permit. Provider discovery and test-connection calls also require network authorization. Never send model prompts, memory, attachments or credentials as a fallback after a local failure. Keep the key in OS storage and inject it only at the egress gateway. Do not log headers, URLs with credentials or response bodies.

Honor provider rate limits and Retry-After. Retry bounded pre-send connection failures with jitter. After a response has started or a submitted request may have been billed, report the partial result and require explicit retry unless provider idempotency is verified. Cancel local streaming and attempt upstream cancellation where supported. Explain that a provider may continue processing or charging. Unsupported capability, provider refusal, expired key, exhausted quota, timeout and egress denial are distinct outcomes.
