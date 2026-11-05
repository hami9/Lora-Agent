#include "lora_llama_shim.h"

#include "llama.h"

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

struct lora_llama {
    llama_model *   model   = nullptr;
    llama_context * ctx     = nullptr;
    llama_sampler * sampler = nullptr;
    const llama_vocab * vocab = nullptr;
    const std::atomic<int32_t> * abort_flag = nullptr;
    uint32_t n_ctx   = 0;
    uint32_t n_batch = 512;
    llama_token pending = -1;
    llama_token retry_tok = -1; // sampled token whose piece did not fit the caller buffer
};

static void set_err(char * err, size_t err_len, const char * msg) {
    if (err && err_len > 0) {
        std::snprintf(err, err_len, "%s", msg);
    }
}

static void log_to_stderr(enum ggml_log_level, const char * text, void *) {
    std::fputs(text, stderr);
}

static void log_discard(enum ggml_log_level, const char *, void *) {}

static bool abort_cb(void * data) {
    auto * flag = static_cast<const std::atomic<int32_t> *>(data);
    return flag != nullptr && flag->load(std::memory_order_relaxed) != 0;
}

extern "C" void lora_llama_backend_init(void) {
    const char * v = std::getenv("LORA_WORKER_LLAMA_LOG");
    const bool verbose = v != nullptr && std::strcmp(v, "1") == 0;
    llama_log_set(verbose ? log_to_stderr : log_discard, nullptr);
    llama_backend_init();
}

extern "C" lora_llama * lora_llama_load(const char * path, uint32_t n_ctx, int32_t n_threads,
                                        const int32_t * abort_flag, char * err, size_t err_len) {
    llama_model_params mp = llama_model_default_params();
    mp.n_gpu_layers = 0;
    llama_model * model = llama_model_load_from_file(path, mp);
    if (!model) {
        set_err(err, err_len, "model: llama_model_load_from_file failed");
        return nullptr;
    }

    auto * h = new lora_llama();
    h->model = model;
    h->vocab = llama_model_get_vocab(model);
    h->abort_flag = reinterpret_cast<const std::atomic<int32_t> *>(abort_flag);
    h->n_ctx = n_ctx;
    h->n_batch = std::min<uint32_t>(512, n_ctx);

    llama_context_params cp = llama_context_default_params();
    cp.n_ctx = n_ctx;
    cp.n_batch = h->n_batch;
    cp.n_ubatch = h->n_batch;
    cp.n_threads = n_threads;
    cp.n_threads_batch = n_threads;
    cp.no_perf = true;
    cp.abort_callback = abort_cb;
    cp.abort_callback_data = const_cast<std::atomic<int32_t> *>(h->abort_flag);
    h->ctx = llama_init_from_model(model, cp);
    if (!h->ctx) {
        set_err(err, err_len, "context: llama_init_from_model failed");
        llama_model_free(model);
        delete h;
        return nullptr;
    }
    return h;
}

extern "C" void lora_llama_free(lora_llama * h) {
    if (!h) return;
    if (h->sampler) llama_sampler_free(h->sampler);
    if (h->ctx) llama_free(h->ctx);
    if (h->model) llama_model_free(h->model);
    delete h;
}

extern "C" uint64_t lora_llama_model_size(const lora_llama * h) { return llama_model_size(h->model); }
extern "C" int32_t lora_llama_n_ctx_train(const lora_llama * h) { return llama_model_n_ctx_train(h->model); }
extern "C" uint32_t lora_llama_n_ctx(const lora_llama * h) { return llama_n_ctx(h->ctx); }

extern "C" int32_t lora_llama_apply_template(const lora_llama * h, const char * system,
                                             const char * user, char * buf, int32_t cap) {
    // Models without an embedded template fall back to the built-in ChatML layout.
    const char * tmpl = llama_model_chat_template(h->model, nullptr);
    if (!tmpl) tmpl = "chatml";
    llama_chat_message msgs[2];
    size_t n = 0;
    if (system && system[0]) {
        msgs[n++] = {"system", system};
    }
    msgs[n++] = {"user", user};
    int32_t r = llama_chat_apply_template(tmpl, msgs, n, true, buf, cap);
    if (r < 0 && std::strcmp(tmpl, "chatml") != 0) {
        // Embedded template not understood by the built-in matcher: retry with ChatML.
        r = llama_chat_apply_template("chatml", msgs, n, true, buf, cap);
    }
    return r;
}

extern "C" int32_t lora_llama_tokenize(const lora_llama * h, const char * text, int32_t text_len,
                                       int32_t * tokens, int32_t cap) {
    // The chat template emits special tokens as text (parse_special=true), so add_special=false.
    // BOS is then added only if the vocab wants it and the text did not already produce it.
    const bool want_bos = llama_vocab_get_add_bos(h->vocab);
    if (!want_bos) {
        return llama_tokenize(h->vocab, text, text_len, tokens, cap, false, true);
    }
    // Reserve slot 0 for a possible BOS.
    const int32_t cap1 = cap > 0 ? cap - 1 : 0;
    const int32_t n = llama_tokenize(h->vocab, text, text_len, cap > 0 ? tokens + 1 : nullptr, cap1, false, true);
    if (n == INT32_MIN) return n;
    if (n < 0) return n - 1; // needed count, plus the reserved BOS slot
    if (cap <= 0) return -(n + 1);
    const llama_token bos = llama_vocab_bos(h->vocab);
    if (bos >= 0 && (n == 0 || tokens[1] != bos)) {
        tokens[0] = bos;
        return n + 1;
    }
    std::memmove(tokens, tokens + 1, static_cast<size_t>(n) * sizeof(int32_t));
    return n;
}

extern "C" int32_t lora_llama_begin(lora_llama * h, const int32_t * tokens, int32_t n_tokens,
                                    uint64_t seed) {
    if (n_tokens <= 0) return LORA_LLAMA_ERROR;
    if (h->sampler) {
        llama_sampler_free(h->sampler);
        h->sampler = nullptr;
    }
    h->pending = -1;
    h->retry_tok = -1;
    llama_memory_clear(llama_get_memory(h->ctx), true);

    llama_sampler_chain_params sp = llama_sampler_chain_default_params();
    sp.no_perf = true;
    h->sampler = llama_sampler_chain_init(sp);
    llama_sampler_chain_add(h->sampler, llama_sampler_init_top_k(40));
    llama_sampler_chain_add(h->sampler, llama_sampler_init_top_p(0.9f, 1));
    llama_sampler_chain_add(h->sampler, llama_sampler_init_temp(0.6f));
    llama_sampler_chain_add(h->sampler, llama_sampler_init_dist(static_cast<uint32_t>(seed)));

    std::vector<llama_token> buf(tokens, tokens + n_tokens);
    int32_t done = 0;
    while (done < n_tokens) {
        const int32_t n = std::min<int32_t>(static_cast<int32_t>(h->n_batch), n_tokens - done);
        const int32_t rc = llama_decode(h->ctx, llama_batch_get_one(buf.data() + done, n));
        if (rc == 2) return LORA_LLAMA_ABORTED;
        if (rc != 0) return LORA_LLAMA_ERROR;
        done += n;
    }
    return LORA_LLAMA_OK;
}

extern "C" int32_t lora_llama_next(lora_llama * h, int32_t * is_eog, char * piece, int32_t piece_cap,
                                   int32_t * piece_len) {
    *is_eog = 0;
    *piece_len = 0;
    llama_token tok;
    if (h->retry_tok >= 0) {
        tok = h->retry_tok;
        h->retry_tok = -1;
    } else {
        if (h->pending >= 0) {
            llama_token t = h->pending;
            h->pending = -1;
            const int32_t rc = llama_decode(h->ctx, llama_batch_get_one(&t, 1));
            if (rc == 2) return LORA_LLAMA_ABORTED;
            if (rc != 0) return LORA_LLAMA_ERROR;
        }
        tok = llama_sampler_sample(h->sampler, h->ctx, -1);
        if (llama_vocab_is_eog(h->vocab, tok)) {
            *is_eog = 1;
            return LORA_LLAMA_OK;
        }
    }
    const int32_t n = llama_token_to_piece(h->vocab, tok, piece, piece_cap, 0, false);
    if (n < 0) {
        // Buffer too small: report the needed size; calling next again reuses this token.
        h->retry_tok = tok;
        *piece_len = -n;
        return LORA_LLAMA_BUFFER_TOO_SMALL;
    }
    *piece_len = n;
    h->pending = tok;
    return LORA_LLAMA_OK;
}

extern "C" uint64_t lora_llama_kv_bytes_per_token(const lora_llama * h) {
    // K and V per layer: n_head_kv * head_dim elements each, f16 (2 bytes) by default.
    const int64_t n_head = llama_model_n_head(h->model);
    const int64_t head_dim = n_head > 0 ? llama_model_n_embd(h->model) / n_head : 0;
    return static_cast<uint64_t>(llama_model_n_layer(h->model)) *
           static_cast<uint64_t>(llama_model_n_head_kv(h->model)) *
           static_cast<uint64_t>(head_dim) * 2 /*K+V*/ * 2 /*f16*/;
}
