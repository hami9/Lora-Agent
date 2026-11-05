//! Backend selection (`LORA_WORKER_BACKEND`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    Deterministic,
    Native,
    /// Native was requested but this binary was built without `native-llama-cpp`.
    NativeUnavailable,
}

/// `env` is the raw value of `LORA_WORKER_BACKEND`; `native_compiled` tells whether the binary
/// was built with the `native-llama-cpp` feature.
pub fn select_backend(env: Option<&str>, native_compiled: bool) -> BackendChoice {
    match env.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("deterministic") => BackendChoice::Deterministic,
        Some("native") if native_compiled => BackendChoice::Native,
        Some("native") => BackendChoice::NativeUnavailable,
        _ if native_compiled => BackendChoice::Native,
        _ => BackendChoice::Deterministic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_compiled_feature() {
        assert_eq!(select_backend(None, false), BackendChoice::Deterministic);
        assert_eq!(select_backend(None, true), BackendChoice::Native);
        assert_eq!(select_backend(Some(""), true), BackendChoice::Native);
        assert_eq!(
            select_backend(Some("bogus"), false),
            BackendChoice::Deterministic
        );
    }

    #[test]
    fn env_override() {
        assert_eq!(
            select_backend(Some("deterministic"), true),
            BackendChoice::Deterministic
        );
        assert_eq!(
            select_backend(Some(" Native "), true),
            BackendChoice::Native
        );
        assert_eq!(
            select_backend(Some("native"), false),
            BackendChoice::NativeUnavailable
        );
    }
}
