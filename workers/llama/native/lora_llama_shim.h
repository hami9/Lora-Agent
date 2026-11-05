/* Flat C API over the pinned llama.cpp C API.
 *
 * Only opaque handles, scalars and plain pointers cross this boundary, so the Rust side never
 * mirrors llama.cpp struct layouts. Everything returns 0 on success unless documented.
 */
#ifndef LORA_LLAMA_SHIM_H
#define LORA_LLAMA_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct lora_llama lora_llama;

/* Result codes for lora_llama_begin / lora_llama_next. */
#define LORA_LLAMA_OK 0
#define LORA_LLAMA_ABORTED 1
#define LORA_LLAMA_BUFFER_TOO_SMALL 2 /* next(): *piece_len holds the needed size; call again */
#define LORA_LLAMA_ERROR (-1)

/* Initialise the ggml/llama backend once and route llama.cpp logs to stderr only when the env var
 * LORA_WORKER_LLAMA_LOG=1 (stdout is the protocol channel and must never receive log output). */
void lora_llama_backend_init(void);

/* Load a GGUF model and create a context. `abort_flag` points to an int32_t owned by the caller that
 * must outlive the returned handle; a non-zero value aborts the running decode. On failure returns
 * NULL and writes a message prefixed "model:" or "context:" into `err`. */
lora_llama * lora_llama_load(const char * path, uint32_t n_ctx, int32_t n_threads,
                             const int32_t * abort_flag, char * err, size_t err_len);
void lora_llama_free(lora_llama * h);

uint64_t lora_llama_model_size(const lora_llama * h);
int32_t lora_llama_n_ctx_train(const lora_llama * h);
uint32_t lora_llama_n_ctx(const lora_llama * h);
/* Approximate KV-cache bytes per token (f16), derived from the model hyperparameters. */
uint64_t lora_llama_kv_bytes_per_token(const lora_llama * h);

/* Apply the model's own chat template (system may be NULL) with the assistant turn opened.
 * Returns the number of bytes needed (may exceed `cap`; call again with a bigger buffer), or < 0. */
int32_t lora_llama_apply_template(const lora_llama * h, const char * system, const char * user,
                                  char * buf, int32_t cap);

/* Tokenize (special tokens parsed). Returns the token count; if it exceeds `cap` returns the
 * negated required count; INT32_MIN on overflow. */
int32_t lora_llama_tokenize(const lora_llama * h, const char * text, int32_t text_len,
                            int32_t * tokens, int32_t cap);

/* Reset the context memory, build the sampler chain and decode the prompt in n_batch chunks. */
int32_t lora_llama_begin(lora_llama * h, const int32_t * tokens, int32_t n_tokens, uint64_t seed);

/* Sample the next token. On success sets *is_eog and writes the raw piece bytes (possibly a
 * partial UTF-8 sequence) into `piece` / *piece_len. The previously sampled token is decoded lazily
 * at the start of the call, so the last token of a generation is never decoded needlessly. */
int32_t lora_llama_next(lora_llama * h, int32_t * is_eog, char * piece, int32_t piece_cap,
                        int32_t * piece_len);

#ifdef __cplusplus
}
#endif

#endif
