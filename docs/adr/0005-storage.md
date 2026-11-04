# 0005 · Durable local state

Status: proposed design baseline, 2026-11-04.

## Context

Tasks, grants, model revisions and audit records need consistent transitions. Weights may be multiple gigabytes and need independent deduplication, verification and deletion.

## Decision

Use a single local SQLite writer and immutable SHA-256-addressed model files. Stage and validate bytes on the destination volume, publish them atomically, then transactionally publish the registry reference. Reconcile abandoned files after a crash.

## Alternatives

Loose JSON files simplify inspection but complicate multi-row transitions and recovery. Storing weights inside SQLite inflates backups. A separate service database increases idle dependencies.

## Tradeoffs

An atomic file rename and a DB commit do not form one transaction. Ordering permits a harmless orphan, while readers see only committed verified references. An in-use object cannot be collected.

## Consequences

Migrations version the schema, create a coherent backup and reject unsupported newer versions. Copying only the main DB during a WAL transaction is unsafe. Use SQLite backup or a properly quiesced store. Keep secrets in OS credential storage.

## Conditions for reconsideration

Revisit if sustained contention survives a dedicated writer and bounded queues, or if multiple independent devices must share a catalog. Do not put a WAL database on a network filesystem.

[Detailed specification](../MODEL_MANAGEMENT.md)
