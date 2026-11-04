# 0002 · One qualified backend

Status: proposed design baseline, 2026-11-04.

## Context

An engine's advertised model list is larger than the set Lora can reproduce across machines. Supporting two engines doubles many fixture, template and packaging combinations.

## Decision

Start with a pinned llama.cpp CPU worker, one qualified GGUF text architecture/quantization/template tuple and a backend-independent generation contract. Advertise only capabilities confirmed by the worker and fixture manifest.

## Alternatives

mistral.rs is the second candidate for an otherwise unsupported architecture or a demonstrable gain. Ollama is a later external provider where the user already operates it. Candle and Burn need substantially more integration before serving.

## Tradeoffs

An external service reduces early integration work but owns its updates, lifecycle and memory. Owning the worker makes cost and failure explicit at the price of packaging native code.

## Consequences

Record engine revision, flags, kernel path and model SHA with each benchmark. Withhold tool mode when the template cannot produce valid proposals. Reject a format or feature mismatch with a reason.

## Conditions for reconsideration

Add a second worker after an identified user workload fails on the chosen backend and a matched benchmark or conformance test justifies the extra release matrix.

[Detailed specification](../INFERENCE_ENGINE.md)
