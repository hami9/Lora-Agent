# Desktop UX

The proposed desktop client follows the CLI and uses the same Rust application services. A user should be able to tell where a prompt runs, which model is active and whether a pending action will read, change or transmit data. The pictured black and magenta identity is a branding reference. Body text, focus outlines and alerts need independent contrast in light and dark themes.

## Shell and navigation

Use Tauri 2, React and TypeScript for the first desktop prototype. Native command handlers validate every request and expose a narrow service surface. Restrict capabilities by window, package UI assets locally, disallow remote navigation and avoid exposing shell or generic filesystem plugins. Tauri's access controls complement the application's per-run grant checks. Validate system webview availability, packaging and idle costs on reference devices before committing a support claim. [Tauri capabilities](https://v2.tauri.app/security/capabilities/) · [Tauri distribution](https://v2.tauri.app/distribute/)

Sidebar groups are **Work** (Dashboard, Chat, Agent Workspace), **Library** (Models, Hardware), and **Manage** (Settings and enabled extensions or training). A narrow window collapses the sidebar into a labeled menu. A persistent status strip shows local/remote destination, selected model, current task and Stop. Hide unavailable experimental screens until enabled. Keep diagnostic details behind an expandable panel.

| Screen | Main content and action | Empty, failure or restricted state |
| --- | --- | --- |
| Dashboard | Active model, task, RAM/VRAM headroom, sampled CPU/GPU and recent local activity. Resume a run or import a model. | With no model, show offline import. Missing sensors show “Unavailable” with source and last refresh. |
| Chat | Local model selector, mode, attachments with per-run scope, streaming text, history preference, Stop and Retry. | Mark partial output on cancellation. Retry creates a new run and displays its destination. Disable tools on unsupported templates. |
| Agent Workspace | Ordered plan proposals, verified tool receipts, approvals, bounded output, progress, result and history. | Show the concrete denied scope or failed precondition. Interrupted effects remain visibly unconfirmed. |
| Model Library | Installed vs catalog models, source, license, digest, context fit, variant size, progress, import and delete. | A failed validation explains the requirement and preserves the existing installed model. Catalog is an explicit network feature. |
| Hardware | Detected CPU/GPU, RAM/VRAM, probe confidence, candidate plans, active profile and benchmark provenance. | Unknown memory budget is shown as unknown. Distinguish projected from measured behavior. |
| Adapter Studio | Future dataset consent, split, training profile, progress/checkpoints, held-out evaluation and promotion. | Unsupported device/base combination shows required qualification. Failed training keeps previous adapters active. |
| Extension Manager | Future installed digest, publisher, permission delta, compatibility, disable/revoke and update history. | Quarantine failures. Offer rollback only to a previously verified digest. |
| Settings | General, inference, hardware, grants, external providers, data retention, diagnostics and theme. | Private local mode works without account setup. Disabled external access lists what enabling it would disclose. |

## Primary journeys

**First local conversation.** Open Models → Import file → inspect digest, source and compatibility → accept a feasible execution plan → Chat. If the model exceeds available memory, keep the conversation local and offer a reduced context or a smaller model. Any external-provider choice starts a separate consent journey.

**Read a project document.** Select Read-only Assistant, choose a root, ask about a named file, and inspect the request. A trusted consent panel shows executable tool identity, relative resource, read limit, model destination and lifetime. File text never renders inside the trusted prompt chrome. The answer cites the file read. Revoke in Settings to invalidate further access.

**Recover from interruption.** Stop remains reachable throughout generation. On a worker crash, retain partial output and safe receipts, show whether a tool ran, and offer a new run after the supervisor has released resources. Restoring a UI window never replays an effect.

**Escalate to cloud, later.** Show the endpoint origin, model, exact categories and amount of prompt/file content proposed for disclosure, estimated cost or “unknown”, and a preview. Default to no file excerpts. Change any recipient or attached content and request consent again.

## Accessibility and low-end operation

Set semantic landmarks and real control labels. Keyboard users can navigate every screen, cancel a run, inspect a receipt and reject a request without a pointer. Dialogs trap and restore focus, default to the least-privileged choice, and describe errors in text. Do not announce token-by-token updates to screen readers. Use a throttled summary and a user-controlled transcript. Respect system scale, contrast, reduced motion and RTL text. Validate against WCAG 2.2 AA criteria and test keyboard-only use, NVDA on Windows and VoiceOver on macOS. [WCAG 2.2](https://www.w3.org/TR/WCAG22/)

Virtualize long transcripts, coalesce stream deltas on animation frames, sample telemetry at a low rate, pause hidden charts and avoid chart animations on battery. Measure cold open, idle RAM/CPU and input responsiveness during generation with the same model and prompt as the CLI. Release the desktop client only after the first-time, denied-grant, missing-webview, out-of-memory, crash and stop journeys pass on each advertised platform.

## Desktop View-Model Cryptographic Attestation

The desktop projection layer produces cryptographically signed state snapshots (`DesktopSnapshot`) under domain separator `lora.desktop.snapshot.v1\0` (`DESKTOP_SNAPSHOT_DOMAIN_V1`).

Each snapshot binds:
- Active screen navigation destination
- Complete status strip state and reachability invariants
- Hardware memory budgets (installed models, available/total RAM, CPU summary)
- Session identifier and anti-spoofing challenge token
- Audit journal event counter

Verification routines (`verify_desktop_snapshot`) confirm state integrity and detect any UI state tampering before client render. Supported in CLI (`lora status --verify-desktop <PATH>`, `lora-desktop --verify-snapshot <PATH>`), loopback bridge (`GET /api/desktop-snapshot`), and REST API (`GET /v1/desktop/snapshot`, `POST /v1/desktop/verify`).
