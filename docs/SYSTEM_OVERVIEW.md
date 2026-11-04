# System overview

## A session from the user's seat

Mina is offline, has a GGUF text model on disk and wants to know which configuration option her project's guide recommends. On first import she sees the model's identity, byte size, digest, template and provenance. Unknown provenance is shown explicitly. The release allowlist identifies the fixture and backend build used for qualification. A familiar filename grants no trust.

The hardware panel or CLI reports available memory, sample age, intended context and any unsupported feature. If the requested context cannot fit, Lora shows a smaller configuration with its effect on the task. Mina chooses whether to change it. The runtime reserves memory and opens the worker. Chat starts without a network account, discovery service or background model download.

Mina selects read-only assistance, chooses the project scope and asks about its guide. When the model proposes a read, the host resolves the candidate relative to that scope and displays the file and byte limit. The executor returns a bounded excerpt with the relative path and line range. The final answer points back to the observation. If the file contains instructions to upload a credential, the bytes remain untrusted text. A missing grant, unreadable file or incapable model produces an actionable result.

A stop request ends generation and releases the active reservation after the worker has actually stopped. A crashed worker produces an interrupted outcome. Mina can retry from a fresh turn and inspect what the previous task accomplished.

## Users and boundaries

| User | First useful outcome | Later outcome and dependency |
| --- | --- | --- |
| Offline laptop owner | Check local fit, import once, stream and stop a small text model | Battery-aware profiles after comparable device measurements |
| Developer | Reproduce a session through the CLI, identify which model/build ran, diagnose a rejection | Authenticated SDK, repository tools and reversible edits after policy qualification |
| Local model author | Register an existing model and distinguish compatibility failure from memory exhaustion | Evaluate and export a compatible adapter after a training worker passes its own gates |
| Maintainer | Reproduce a reported failure from redacted metadata and run the documentation checks | Publish independently qualified binaries, worker backends and extension API versions |

## Operational states

An installation starts with no model. The registry can be populated offline. A registered revision can be incompatible with a particular worker or host and remains inspectable. Admission creates a temporary execution plan. Loading materializes native state in one worker. A chat or file question then consumes an application run. Worker death invalidates every native handle from that worker epoch. Registering and loading have different lifetimes, so unloading never deletes model bytes.

A fresh MVP user sees local modes only. Later features appear when their prerequisites are present. For example, a cloud endpoint requires explicit setup and per-scope egress consent, and Adapter Studio requires a qualified base/trainer/device tuple. The desktop client reports the same reason codes as the CLI.

## What to measure first

Track first successful offline answer, failed admissions, time to first token and observed peak memory on named machines. For agent tasks, measure correct answers with verifiable file references alongside denied out-of-scope reads. Keep a record of abstentions and failed tasks. Product claims need representative tests, including the smallest supported device. See the [benchmark plan](BENCHMARK_PLAN.md) and [MVP acceptance suite](MVP_SPECIFICATION.md).
