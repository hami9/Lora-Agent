//! Builds the pinned llama.cpp (git submodule) and the C++ shim, but only for
//! `--features native-llama-cpp`. Default builds do nothing here.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REVISION: &str = "7d516e25ead4653169236b3f90d13f7bd8629724";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../.gitmodules");
    println!("cargo:rerun-if-changed=native/lora_llama_shim.cpp");
    println!("cargo:rerun-if-changed=native/lora_llama_shim.h");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NATIVE_LLAMA_CPP");
    println!("cargo:rerun-if-env-changed=LORA_LLAMA_CPP_DIR");

    if env::var_os("CARGO_FEATURE_NATIVE_LLAMA_CPP").is_none() {
        return;
    }
    build_native();
}

/// Fail if the submodule checkout is not the pinned revision; warn if that cannot be determined.
fn check_revision(llama_dir: &Path) {
    let out = Command::new("git")
        .arg("-C")
        .arg(llama_dir)
        .args(["rev-parse", "HEAD"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let actual = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if actual != REVISION {
                panic!(
                    "llama.cpp submodule is at {actual} but this worker pins {REVISION}.\n\
                     Run: git submodule update --init --depth 1 workers/llama/vendor/llama.cpp"
                );
            }
        }
        _ => println!(
            "cargo:warning=could not verify the llama.cpp revision (git unavailable or not a checkout); expected {REVISION}"
        ),
    }
}

fn build_native() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let llama_dir = env::var_os("LORA_LLAMA_CPP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("vendor").join("llama.cpp"));
    if !llama_dir.join("CMakeLists.txt").is_file() || !llama_dir.join("include/llama.h").is_file() {
        panic!(
            "llama.cpp submodule is missing at {}.\n\
             Run: git submodule update --init --depth 1 workers/llama/vendor/llama.cpp\n\
             (on Windows first: git config core.longpaths true)",
            llama_dir.display()
        );
    }
    check_revision(&llama_dir);
    for p in ["CMakeLists.txt", "src", "ggml", "include"] {
        println!("cargo:rerun-if-changed=vendor/llama.cpp/{p}");
    }

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env_abi = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let x86_64 = arch == "x86_64";

    let mut cfg = cmake::Config::new(&llama_dir);
    cfg.profile("Release")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("LLAMA_BUILD_TESTS", "OFF")
        .define("LLAMA_BUILD_TOOLS", "OFF")
        .define("LLAMA_BUILD_EXAMPLES", "OFF")
        .define("LLAMA_BUILD_SERVER", "OFF")
        .define("LLAMA_BUILD_COMMON", "OFF")
        .define("LLAMA_BUILD_APP", "OFF")
        .define("LLAMA_OPENSSL", "OFF")
        .define("GGML_NATIVE", "OFF")
        .define("GGML_OPENMP", "OFF")
        .define("GGML_METAL", "OFF")
        .define("GGML_AVX2", if x86_64 { "ON" } else { "OFF" })
        .define("GGML_FMA", if x86_64 { "ON" } else { "OFF" })
        .define("GGML_F16C", if x86_64 { "ON" } else { "OFF" })
        .define("GGML_BMI2", if x86_64 { "ON" } else { "OFF" })
        // keep the install tree small and avoid pulling cmake package metadata
        .define("LLAMA_BUILD_NUMBER", "12125")
        .define("LLAMA_BUILD_COMMIT", &REVISION[..7]); // verified by check_revision
    let dst = cfg.build();

    // Shim first: it depends on the llama/ggml archives linked after it.
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .file(manifest.join("native/lora_llama_shim.cpp"))
        .include(manifest.join("native"))
        .include(llama_dir.join("include"))
        .include(llama_dir.join("ggml/include"))
        .warnings(true);
    build.compile("lora_llama_shim");

    // Link search paths: <dst>/lib (and lib64 on some distros).
    let mut libs: Vec<(String, u8)> = Vec::new();
    for sub in ["lib", "lib64"] {
        let dir = dst.join(sub);
        if !dir.is_dir() {
            continue;
        }
        println!("cargo:rustc-link-search=native={}", dir.display());
        collect_libs(&dir, &mut libs);
    }
    if libs.is_empty() {
        panic!(
            "no llama/ggml static libraries found under {}",
            dst.display()
        );
    }
    // Static link order matters on GNU ld: dependents first.
    libs.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    libs.dedup();
    for (name, _) in &libs {
        println!("cargo:rustc-link-lib=static={name}");
    }

    // C++ runtime and platform libraries for the static ggml/llama archives.
    match (os.as_str(), env_abi.as_str()) {
        ("windows", "msvc") => {}
        ("macos", _) | ("ios", _) => {
            println!("cargo:rustc-link-lib=c++");
            println!("cargo:rustc-link-lib=framework=Accelerate");
            println!("cargo:rustc-link-lib=framework=Foundation");
        }
        ("windows", _) => println!("cargo:rustc-link-lib=stdc++"),
        _ => {
            println!("cargo:rustc-link-lib=stdc++");
            println!("cargo:rustc-link-lib=m");
            println!("cargo:rustc-link-lib=pthread");
            println!("cargo:rustc-link-lib=dl");
        }
    }

    // Build options actually compiled in, reported honestly in HelloAck.
    let mut opts = vec!["cpu".to_string()];
    if x86_64 {
        for isa in ["avx2", "fma", "f16c", "bmi2"] {
            opts.push(isa.to_string());
        }
    }
    println!(
        "cargo:rustc-env=LORA_LLAMA_BUILD_OPTIONS={}",
        opts.join(",")
    );
}

/// Collect static library names (without prefix/extension) whose file name starts with
/// `llama` or `ggml`, with a rank that yields a dependency-correct link order.
fn collect_libs(dir: &Path, out: &mut Vec<(String, u8)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if ext != "a" && ext != "lib" {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let name = stem.strip_prefix("lib").unwrap_or(stem);
        // On MSVC the stem has no `lib` prefix, but "libllama" never occurs upstream.
        let rank = match name {
            "llama" => 0,
            "ggml" => 1,
            "ggml-cpu" => 2,
            "ggml-base" => 9,
            n if n.starts_with("ggml") => 5,
            _ => continue,
        };
        out.push((name.to_string(), rank));
    }
}
