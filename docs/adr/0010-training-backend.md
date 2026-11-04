# 0010 · Adapter training

Status: proposed design baseline, 2026-11-04.

## Context

The ability to run quantized inference does not imply enough memory, matching kernels or serialization for adapter training. A useful adapter must load on the intended inference runtime and improve a held-out task.

## Decision

Keep training outside the MVP. First prototype plain LoRA on a pinned small Qwen3 checkpoint with an optional PMetal worker on Apple Silicon. Publish this as a candidate until data, licensing, memory, save/resume and target inference evaluation pass.

## Alternatives

Compare MLX LM on the same checkpoint. Consider an isolated Python trainer for a proven workflow, or Burn/Candle when custom training semantics justify implementing and validating them. Pretraining and full tuning have different resource envelopes.

## Tradeoffs

A single candidate narrows portability but makes compatibility testable. Conversion may change tensor names, dtype, tokenizer or quality. A successful training loss curve does not establish export compatibility.

## Consequences

Record base, tokenizer, dataset, split, parameters, worker and adapter digests. Never overwrite the base. Publish the adapter only after a held-out comparison, leak check and successful load by the target worker.

## Conditions for reconsideration

Enable QLoRA, another accelerator or a merge pipeline only after separate memory and quality experiments. Choose a different trainer if the offline export/import round trip fails.

[Detailed specification](../TRAINING_SYSTEM.md)
