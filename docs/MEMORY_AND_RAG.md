# Memory and retrieval

A user asking a model to read a file authorizes that read. Durable indexing, later reuse, and external transmission each require their own choices. The MVP needs local run metadata and transient context. Long-term memory, embeddings and background indexing are later features.

## Ownership and retention

| Class | Keys and contents | Lifetime and controls |
| --- | --- | --- |
| Working context | Run ID, token budget, pending tool observations and evidence references | In memory until termination. Keep tool bytes transient unless the user explicitly saves content. |
| Conversation | Workspace, conversation ID, ordered user/assistant text, model/template revision | Opt-in saved history, inspect/export/delete by conversation. Never promote a message to reusable memory implicitly. |
| Task journal | Run ID, action receipts, safe resource aliases and outcome | Host-owned SQLite tables with configurable retention. A receipt need not store file contents. |
| Reusable memory | Explicit user preference or approved fact, project scope, origin, revision, expiry | Review, edit, pin, expire and forget independently from chat history. |
| Derived retrieval data | Source identity, version/content digest, chunk offsets, parser revision and embedding namespace | Source grant controls indexing and retrieval. Revocation hides entries immediately and schedules physical cleanup. |

One host coordinates SQLite writes, enforces foreign keys and stores content-addressed model blobs separately. Commit source revisions and their indexing jobs atomically. A background worker obtains a lease, builds a new generation, then atomically flips the active generation only if the source revision and grant still match. Crashes leave inactive generations collectible. Avoid storing raw tool outputs in ordinary logs.

## Index and query

1. User selects a local source scope, sees estimated disk use and excluded paths, and approves indexing. Exclude credential stores, hidden sensitive paths, binary files and unsupported document types by default. Handle the same link/race hazards as file tools.
2. Parse supported types in a bounded helper. Store section/line/byte ranges and document digest. A starting experiment is 256–512 tokenizer tokens per chunk with a small overlap, preserving headings and code blocks. Tune with a labeled corpus.
3. Start with SQLite FTS5 for indexed lexical search. Benchmark exact cosine scanning of normalized local embedding blobs on small catalogs and pinned sqlite-vec on larger catalogs. The extension advertises a virtual-table nearest-neighbor interface and alpha builds. Keep runtime extension loading disabled. Qualify a statically linked build and migration/rebuild path before adoption. [SQLite FTS5](https://sqlite.org/fts5.html) · [sqlite-vec](https://alexgarcia.xyz/sqlite-vec/)
4. Define the embedding namespace by model digest, tokenizer, dimension, normalization, chunker and parser revisions. Never compare incompatible vectors. Reindex changed namespaces separately with idle-priority leases. Embedding jobs yield to interactive generation.
5. Filter by workspace, active revision and current ACL before ranking. Combine lexical and vector candidates using reciprocal-rank fusion. Fetch top evidence with source and offsets, revalidate read grants and version at dispatch. A reranker runs only if the benchmark shows improved relevance within the latency budget. Stale or deleted sources must be invisible even during cleanup.

## Context budgeting

Reserve output tokens and control/schema overhead first, using the selected worker's tokenizer and template. Partition remaining input into current request, recent turns, tool receipts and retrieved excerpts. Drop lower-ranked excerpts before truncating the user's current request. If the request alone exceeds the budget, report the required and available context. Summaries retain citations and source ranges, are labeled model-derived, and cannot grant tool authority. Quoted evidence is delimited, attributed and treated as untrusted input. Prompt assembly respects the provider choice and external-data grant in [API and SDK](API_AND_SDK.md).

## Forget and verify

Deletion immediately tombstones a source or conversation and invalidates retrieval caches within a transaction. Cancel in-flight jobs, queue idempotent cleanup of derived chunks, vectors, lexical entries, summaries and saved excerpts, then mark complete. If related content was separately exported, report that copy as beyond the managed store. WAL, backups and virtual-table segments can retain historical bytes. Apply a checkpoint and qualified per-index rebuild/secure-delete recipe when offering deeper local cleanup, and describe limits of SSDs, snapshots and copies. [SQLite WAL](https://www.sqlite.org/wal.html) · [SQLite secure deletion](https://www.sqlite.org/pragma.html#pragma_secure_delete)

Gate promotion on retrieval recall and citation correctness over a fixed corpus, cold and warm query latency, peak RSS and index size. Exercise two projects with identical file names, source replacement mid-index, permission revocation between search and model dispatch, deletion while offline, interrupted cleanup, migration rollback and wrong embedding dimensions. A deletion test must query each managed store and derived index, including a fresh process after restart.
