//! Shared cancellation state between the stdin reader thread and the generation loop.
//!
//! The reader thread sees `Cancel` frames while the main thread is busy decoding, so it flips the
//! abort flag directly. The flag is an `AtomicI32` so the C++ shim can poll it from llama.cpp's
//! abort callback.

use lora_protocol::HostRequest;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub struct CancelState {
    flag: Arc<AtomicI32>,
    current: Mutex<Option<String>>,
}

impl CancelState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The abort flag handed to the native shim.
    #[cfg_attr(not(feature = "native-llama-cpp"), allow(dead_code))]
    pub fn flag(&self) -> Arc<AtomicI32> {
        Arc::clone(&self.flag)
    }

    pub fn is_aborted(&self) -> bool {
        self.flag.load(Ordering::Relaxed) != 0
    }

    /// Reader-side hook, called for every parsed request before it is forwarded.
    /// Returns `true` when the request was a `Cancel` for the in-flight generation and has been
    /// fully handled here (it must not be forwarded).
    pub fn intercept(&self, req: &HostRequest) -> bool {
        match req {
            HostRequest::Generate { request_id, .. } => {
                let mut cur = self.current.lock().unwrap();
                if cur.is_none() {
                    *cur = Some(request_id.clone());
                }
                false
            }
            HostRequest::Cancel {
                target_request_id, ..
            } => {
                let cur = self.current.lock().unwrap();
                if cur.as_deref() == Some(target_request_id.as_str()) {
                    self.flag.store(1, Ordering::Relaxed);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Generation-side: mark `request_id` as in flight (no-op if the reader already did).
    pub fn begin(&self, request_id: &str) {
        let mut cur = self.current.lock().unwrap();
        if cur.is_none() {
            *cur = Some(request_id.to_string());
        }
    }

    /// Raise the abort flag unconditionally (host pipe closed or corrupt).
    pub fn abort(&self) {
        self.flag.store(1, Ordering::Relaxed);
    }

    /// Generation-side: clear in-flight state and the abort flag.
    pub fn end(&self) {
        let mut cur = self.current.lock().unwrap();
        *cur = None;
        self.flag.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen(id: &str) -> HostRequest {
        HostRequest::Generate {
            request_id: id.into(),
            session_id: "s".into(),
            model_handle: "h".into(),
            prompt: "p".into(),
            max_tokens: 1,
            context_length: 1,
            seed: 0,
            tool_calling_enabled: false,
            simulated_delay_ms: 0,
        }
    }
    fn cancel(target: &str) -> HostRequest {
        HostRequest::Cancel {
            request_id: "c".into(),
            target_request_id: target.into(),
        }
    }

    #[test]
    fn matching_cancel_sets_flag_and_is_consumed() {
        let st = CancelState::new();
        assert!(!st.intercept(&gen("g1")));
        assert!(st.intercept(&cancel("g1")));
        assert!(st.is_aborted());
        st.end();
        assert!(!st.is_aborted());
    }

    #[test]
    fn abort_sets_flag_until_end() {
        let st = CancelState::new();
        st.abort();
        assert!(st.is_aborted());
        st.end();
        assert!(!st.is_aborted());
    }

    #[test]
    fn non_matching_or_idle_cancel_is_forwarded() {
        let st = CancelState::new();
        assert!(!st.intercept(&cancel("g1")));
        st.intercept(&gen("g1"));
        assert!(!st.intercept(&cancel("other")));
        assert!(!st.is_aborted());
    }
}
