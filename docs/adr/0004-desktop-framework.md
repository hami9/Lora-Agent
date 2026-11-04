# 0004 · Desktop client

Status: proposed design baseline, 2026-11-04.

## Context

The project needs accessible model selection and transparent permission prompts on several OSes. The first release must still run on a machine with limited memory.

## Decision

After CLI qualification, use Tauri 2, React and TypeScript with local packaged UI assets. Expose narrow application commands to the renderer. Trusted prompts bind the request identity and never render executable model markup.

## Alternatives

Electron gives more uniform rendering and adds a bundled browser runtime. Native per-OS UI gives native integration at the cost of three implementations. An immediate-mode Rust toolkit merits a spike if webview costs exceed budget.

## Tradeoffs

Webview differences require OS-specific testing. Local assets and scoped command handlers limit the web surface. The shared core keeps the CLI viable for headless or low-memory use.

## Consequences

Measure cold start, idle RSS, transcript growth, accessibility and approval spoofing on named devices. Keep credentials and native file handles out of renderer state.

## Conditions for reconsideration

Revisit when the slowest reference device misses agreed UX budgets after profiling, required assistive technology fails, or a target lacks an acceptable webview distribution path.

[Detailed specification](../DESKTOP_UX.md)
