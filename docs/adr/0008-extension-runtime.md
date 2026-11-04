# 0008 · Extension isolation

Status: proposed design baseline, 2026-11-04.

## Context

Users may want connectors, model-specific tools and custom workflows. Third-party executables can otherwise bypass a permission prompt using ambient OS rights.

## Decision

Defer extensions until the core policy is tested. First add configured MCP servers with least authority. For packaged programmable tools prefer Wasmtime with versioned imports, explicit capabilities, quotas and no ambient host access. Gate native providers separately.

## Alternatives

A native dynamic plugin ABI has low call overhead but couples memory safety and build versions. Subprocess tools support existing integrations but need real OS restrictions. Declarative skills support guidance without executable authority.

## Tradeoffs

Wasm host imports become privileged API surface. MCP server discovery describes tools but does not attest safety. Signing only proves a package identity and integrity.

## Consequences

Pin plugin digest and permissions, review new scopes on upgrade, revoke capabilities and terminate inflight instances on disable. Test malicious output, fuel/time exhaustion, stale grants and dependency cycles.

## Conditions for reconsideration

Allow a native host only for a concrete capability unavailable via Wasm/MCP and after independent platform isolation and release gates pass.

[Detailed specification](../PLUGIN_SYSTEM.md)
