//! CPU feature gate for the native backend.
//!
//! The native x86_64 build is compiled for x86-64-v3 (AVX2, FMA, F16C, BMI2). Running it on an
//! older CPU would crash with an illegal instruction, so the worker checks before touching
//! llama.cpp and refuses to load instead.

/// x86_64 features the native build requires.
pub const REQUIRED_X86_FEATURES: [&str; 4] = ["avx2", "fma", "f16c", "bmi2"];

/// Names from `required` for which `has` returns false.
pub fn missing_features(has: &dyn Fn(&str) -> bool, required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter(|f| !has(f))
        .map(|f| (*f).to_string())
        .collect()
}

/// Features the native build needs but this CPU lacks (always empty off x86_64).
pub fn missing_native_cpu_features() -> Vec<String> {
    #[cfg(target_arch = "x86_64")]
    {
        let has = |name: &str| match name {
            "avx2" => std::is_x86_feature_detected!("avx2"),
            "fma" => std::is_x86_feature_detected!("fma"),
            "f16c" => std::is_x86_feature_detected!("f16c"),
            "bmi2" => std::is_x86_feature_detected!("bmi2"),
            _ => false,
        };
        missing_features(&has, &REQUIRED_X86_FEATURES)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_missing_features() {
        let has = |n: &str| n == "avx2" || n == "fma";
        assert_eq!(
            missing_features(&has, &REQUIRED_X86_FEATURES),
            vec!["f16c".to_string(), "bmi2".to_string()]
        );
        assert!(missing_features(&|_| true, &REQUIRED_X86_FEATURES).is_empty());
    }
}
