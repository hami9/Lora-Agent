//! Real inference through the pinned llama.cpp, via the C++ shim in `native/`.
//!
//! Only compiled with `--features native-llama-cpp`.

use crate::cancel::CancelState;
use crate::generation::{run_decode_loop, LoopError, Piece, TokenStream};
use crate::{GenerateArgs, LoadArgs, LoadedModel};
use lora_protocol::{
    write_worker_event, FinishReason, GenerationUsage, WorkerErrorCode, WorkerEvent,
};
use std::ffi::{c_char, CString};
use std::io::Write;
use std::sync::atomic::AtomicI32;
use std::sync::{Arc, Once};
use std::time::Instant;

mod ffi {
    use std::ffi::c_char;

    #[repr(C)]
    pub struct LoraLlama {
        _private: [u8; 0],
    }

    extern "C" {
        pub fn lora_llama_backend_init();
        pub fn lora_llama_load(
            path: *const c_char,
            n_ctx: u32,
            n_threads: i32,
            abort_flag: *const i32,
            err: *mut c_char,
            err_len: usize,
        ) -> *mut LoraLlama;
        pub fn lora_llama_free(h: *mut LoraLlama);
        pub fn lora_llama_model_size(h: *const LoraLlama) -> u64;
        pub fn lora_llama_n_ctx_train(h: *const LoraLlama) -> i32;
        pub fn lora_llama_kv_bytes_per_token(h: *const LoraLlama) -> u64;
        pub fn lora_llama_n_ctx(h: *const LoraLlama) -> u32;
        pub fn lora_llama_apply_template(
            h: *const LoraLlama,
            system: *const c_char,
            user: *const c_char,
            buf: *mut c_char,
            cap: i32,
        ) -> i32;
        pub fn lora_llama_tokenize(
            h: *const LoraLlama,
            text: *const c_char,
            text_len: i32,
            tokens: *mut i32,
            cap: i32,
        ) -> i32;
        pub fn lora_llama_begin(
            h: *mut LoraLlama,
            tokens: *const i32,
            n_tokens: i32,
            seed: u64,
        ) -> i32;
        pub fn lora_llama_next(
            h: *mut LoraLlama,
            is_eog: *mut i32,
            piece: *mut c_char,
            piece_cap: i32,
            piece_len: *mut i32,
        ) -> i32;
    }
}

const RC_OK: i32 = 0;
const RC_ABORTED: i32 = 1;
const RC_BUFFER_TOO_SMALL: i32 = 2;

static BACKEND_INIT: Once = Once::new();

/// Safe owner of one loaded model + context.
pub struct NativeModel {
    raw: *mut ffi::LoraLlama,
    /// Kept alive because the shim reads it from the abort callback.
    _abort: Arc<AtomicI32>,
}

impl NativeModel {
    fn load(
        path: &str,
        n_ctx: u32,
        n_threads: i32,
        abort: Arc<AtomicI32>,
    ) -> Result<Self, (WorkerErrorCode, String)> {
        BACKEND_INIT.call_once(|| unsafe { ffi::lora_llama_backend_init() });
        let cpath = CString::new(path).map_err(|_| {
            (
                WorkerErrorCode::IncompatibleInput,
                "artifact path contains NUL".to_string(),
            )
        })?;
        let mut err = vec![0u8; 512];
        let raw = unsafe {
            ffi::lora_llama_load(
                cpath.as_ptr(),
                n_ctx,
                n_threads,
                abort.as_ptr().cast_const(),
                err.as_mut_ptr().cast::<c_char>(),
                err.len(),
            )
        };
        if raw.is_null() {
            let end = err.iter().position(|&b| b == 0).unwrap_or(err.len());
            let msg = String::from_utf8_lossy(&err[..end]).into_owned();
            let code = if msg.starts_with("context:") {
                WorkerErrorCode::InsufficientResources
            } else {
                WorkerErrorCode::CorruptArtifact
            };
            return Err((code, msg));
        }
        Ok(Self { raw, _abort: abort })
    }

    fn model_size(&self) -> u64 {
        unsafe { ffi::lora_llama_model_size(self.raw) }
    }

    fn n_ctx_train(&self) -> i32 {
        unsafe { ffi::lora_llama_n_ctx_train(self.raw) }
    }

    fn kv_bytes_per_token(&self) -> u64 {
        unsafe { ffi::lora_llama_kv_bytes_per_token(self.raw) }
    }

    fn n_ctx(&self) -> u32 {
        unsafe { ffi::lora_llama_n_ctx(self.raw) }
    }

    fn apply_template(&self, system: Option<&str>, user: &str) -> Result<String, String> {
        let user_c = CString::new(user).map_err(|_| "prompt contains NUL".to_string())?;
        let sys_c = match system {
            Some(s) => Some(CString::new(s).map_err(|_| "system prompt contains NUL".to_string())?),
            None => None,
        };
        let sys_ptr = sys_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
        let mut cap = (user.len() + system.map_or(0, str::len)) * 2 + 1024;
        for _ in 0..2 {
            let mut buf = vec![0u8; cap];
            let n = unsafe {
                ffi::lora_llama_apply_template(
                    self.raw,
                    sys_ptr,
                    user_c.as_ptr(),
                    buf.as_mut_ptr().cast::<c_char>(),
                    cap as i32,
                )
            };
            if n < 0 {
                return Err("chat template could not be applied".to_string());
            }
            if n as usize <= cap {
                buf.truncate(n as usize);
                return String::from_utf8(buf).map_err(|e| e.to_string());
            }
            cap = n as usize + 1;
        }
        Err("chat template size did not converge".to_string())
    }

    fn tokenize(&self, text: &str) -> Result<Vec<i32>, String> {
        let mut cap = text.len() + 16;
        for _ in 0..2 {
            let mut toks = vec![0i32; cap];
            let n = unsafe {
                ffi::lora_llama_tokenize(
                    self.raw,
                    text.as_ptr().cast::<c_char>(),
                    text.len() as i32,
                    toks.as_mut_ptr(),
                    cap as i32,
                )
            };
            if n == i32::MIN {
                return Err("tokenization overflow".to_string());
            }
            if n >= 0 {
                toks.truncate(n as usize);
                return Ok(toks);
            }
            cap = n.unsigned_abs() as usize;
        }
        Err("tokenization size did not converge".to_string())
    }

    /// 0 = ok, 1 = aborted, other = error.
    fn begin(&mut self, tokens: &[i32], seed: u64) -> i32 {
        unsafe { ffi::lora_llama_begin(self.raw, tokens.as_ptr(), tokens.len() as i32, seed) }
    }
}

impl Drop for NativeModel {
    fn drop(&mut self) {
        unsafe { ffi::lora_llama_free(self.raw) };
    }
}

struct NativeStream<'a> {
    model: &'a mut NativeModel,
    buf: Vec<u8>,
}

impl NativeStream<'_> {
    fn call_next(&mut self, eog: &mut i32, len: &mut i32) -> i32 {
        unsafe {
            ffi::lora_llama_next(
                self.model.raw,
                eog,
                self.buf.as_mut_ptr().cast::<c_char>(),
                self.buf.len() as i32,
                len,
            )
        }
    }
}

impl TokenStream for NativeStream<'_> {
    fn next_piece(&mut self) -> Result<Piece, String> {
        let mut eog = 0i32;
        let mut len = 0i32;
        let mut rc = self.call_next(&mut eog, &mut len);
        if rc == RC_BUFFER_TOO_SMALL {
            self.buf.resize(len.max(1) as usize, 0);
            rc = self.call_next(&mut eog, &mut len);
        }
        match rc {
            RC_OK if eog != 0 => Ok(Piece::Eog),
            RC_OK => Ok(Piece::Bytes(self.buf[..len as usize].to_vec())),
            RC_ABORTED => Ok(Piece::Aborted),
            _ => Err(format!("llama.cpp decode/sample failed (code {rc})")),
        }
    }
}

fn process_rss_bytes() -> u64 {
    use sysinfo::{get_current_pid, ProcessesToUpdate, System};
    let Ok(pid) = get_current_pid() else { return 0 };
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map_or(0, |p| p.memory())
}

const TOOL_SYSTEM_PROMPT: &str = "You are a careful local assistant. You may call exactly one tool \
when it is needed to answer. The only tool is read_file, which reads a text file from a workspace \
root. To call it, reply with only this text and nothing else: \
<tool_call>{\"name\":\"read_file\",\"arguments\":{\"root_id\":\"...\",\"relative_path\":\"...\"}}</tool_call> \
Use the WorkspaceRootId given in the request for root_id. If no tool is needed, answer normally.";

pub struct NativeBackend {
    model: Option<(String, u32, NativeModel)>,
    cancel: Arc<CancelState>,
}

impl NativeBackend {
    pub fn new(cancel: Arc<CancelState>) -> Self {
        Self {
            model: None,
            cancel,
        }
    }

    pub fn load(&mut self, args: &LoadArgs) -> Result<LoadedModel, (WorkerErrorCode, String)> {
        // Free the previous model first so two models never coexist in memory.
        self.model = None;
        if args.context_length == 0 {
            return Err((
                WorkerErrorCode::IncompatibleInput,
                "context_length must be greater than zero".to_string(),
            ));
        }
        let threads = if args.threads == 0 {
            // Half the logical CPUs: using all of them (SMT/hybrid cores) measured ~50x slower here.
            (std::thread::available_parallelism().map_or(4, |n| n.get()) / 2).max(1)
        } else {
            args.threads as usize
        } as i32;
        let model = NativeModel::load(
            &args.artifact_path,
            args.context_length,
            threads,
            self.cancel.flag(),
        )?;
        if model.n_ctx() < args.context_length {
            return Err((
                WorkerErrorCode::InsufficientResources,
                format!(
                    "llama.cpp provided context {} smaller than requested {}",
                    model.n_ctx(),
                    args.context_length
                ),
            ));
        }
        if i64::from(model.n_ctx_train()) < i64::from(args.context_length) {
            return Err((
                WorkerErrorCode::OverContextLimit,
                format!(
                    "context {} exceeds trained limit {}",
                    args.context_length,
                    model.n_ctx_train()
                ),
            ));
        }
        let kv_bytes_per_token = match &args.meta {
            Some(m) => {
                u64::from(m.block_count)
                    * u64::from(m.kv_head_count)
                    * u64::from(m.key_dim + m.value_dim)
                    * u64::from(m.kv_elem_bytes)
            }
            // Strict parser could not read the file: ask llama.cpp (f16 K and V cache).
            None => model.kv_bytes_per_token(),
        };
        let handle = format!("worker-mdl-{}", args.artifact_id);
        let resident = model.model_size();
        self.model = Some((handle.clone(), args.context_length, model));
        Ok(LoadedModel {
            handle,
            resident_bytes: resident,
            kv_bytes_per_token,
        })
    }

    pub fn unload(&mut self) {
        self.model = None;
    }

    pub fn rss_bytes(&self) -> u64 {
        process_rss_bytes()
    }

    pub fn generate<W: Write>(&mut self, writer: &mut W, g: GenerateArgs) -> Result<(), ()> {
        self.cancel.begin(&g.request_id);
        let res = self.generate_inner(writer, g);
        self.cancel.end();
        res
    }

    fn generate_inner<W: Write>(&mut self, writer: &mut W, g: GenerateArgs) -> Result<(), ()> {
        let request_id = g.request_id.clone();
        let err = |writer: &mut W, code, msg: String| -> Result<(), ()> {
            // Like the other backend, a failed error write is not fatal on its own.
            let _ = write_worker_event(
                writer,
                &WorkerEvent::Error {
                    request_id: Some(request_id.clone()),
                    code,
                    message: msg,
                    retryable: false,
                },
            );
            Ok(())
        };

        let Some((loaded_h, loaded_ctx, model)) = self.model.as_mut() else {
            return err(
                writer,
                WorkerErrorCode::InvalidState,
                "no model loaded in worker".to_string(),
            );
        };
        if loaded_h != &g.model_handle {
            return err(
                writer,
                WorkerErrorCode::InvalidState,
                "stale or unknown model_handle".to_string(),
            );
        }
        if g.context_length > *loaded_ctx {
            return err(
                writer,
                WorkerErrorCode::OverContextLimit,
                "requested context exceeds the loaded context window".to_string(),
            );
        }

        let system = g.tool_calling_enabled.then_some(TOOL_SYSTEM_PROMPT);
        let formatted = match model.apply_template(system, &g.prompt) {
            Ok(s) => s,
            Err(e) => return err(writer, WorkerErrorCode::InternalError, e),
        };
        let tokens = match model.tokenize(&formatted) {
            Ok(t) => t,
            Err(e) => return err(writer, WorkerErrorCode::InternalError, e),
        };
        let prompt_tokens = tokens.len() as u64;
        if prompt_tokens + u64::from(g.max_tokens) > u64::from(g.context_length) {
            return err(
                writer,
                WorkerErrorCode::OverContextLimit,
                format!(
                    "prompt ({prompt_tokens} tokens) plus max_tokens ({}) exceeds context window {}",
                    g.max_tokens, g.context_length
                ),
            );
        }

        let prefill_start = Instant::now();
        let rc = model.begin(&tokens, g.seed);
        let prefill_ms = prefill_start.elapsed().as_millis() as u64;

        let mut usage = GenerationUsage {
            prompt_tokens: prompt_tokens as u32,
            completion_tokens: 0,
            prefill_ms,
            decode_ms: 0,
            peak_rss_bytes: 0,
        };
        let mut seq = 0u64;
        let (finish_reason, tool_proposal) = if rc == RC_ABORTED {
            (FinishReason::Cancelled, None)
        } else if rc != RC_OK {
            return err(
                writer,
                WorkerErrorCode::InternalError,
                format!("llama.cpp prompt decode failed (code {rc})"),
            );
        } else {
            let cancel = Arc::clone(&self.cancel);
            let mut stream = NativeStream {
                model,
                buf: vec![0u8; 256],
            };
            let outcome = run_decode_loop(
                &mut stream,
                g.max_tokens,
                g.tool_calling_enabled,
                &|| cancel.is_aborted(),
                &mut |text, token_count| {
                    seq += 1;
                    write_worker_event(
                        writer,
                        &WorkerEvent::GenerationDelta {
                            request_id: g.request_id.clone(),
                            sequence: seq,
                            text_delta: text,
                            token_count,
                        },
                    )
                    .is_ok()
                },
            );
            match outcome {
                Ok(o) => {
                    usage.completion_tokens = o.completion_tokens;
                    usage.decode_ms = o.decode_ms;
                    (o.finish_reason, o.tool_proposal)
                }
                Err(LoopError::SinkClosed) => return Err(()),
                Err(LoopError::Engine(m)) => {
                    return err(writer, WorkerErrorCode::InternalError, m);
                }
            }
        };

        usage.peak_rss_bytes = process_rss_bytes();
        seq += 1;
        write_worker_event(
            writer,
            &WorkerEvent::GenerationComplete {
                request_id: g.request_id,
                sequence: seq,
                finish_reason,
                usage,
                tool_proposal,
            },
        )
        .map_err(|_| ())
    }
}
