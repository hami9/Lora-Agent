# Third-party notices for `workers/llama`

This directory contains the isolated worker shim and build pin for `llama.cpp`.

## Upstream engine attribution

- **Component:** `ggml-org/llama.cpp` (includes the `ggml` tensor library)
- **Pinned tag / revision:** `b12125` / `7d516e25ead4653169236b3f90d13f7bd8629724`
- **Location:** git submodule at `workers/llama/vendor/llama.cpp`; compiled only with the `native-llama-cpp` Cargo feature
- **SPDX license expression:** `MIT` (copyright (c) 2023-2026 The ggml authors; full text in `vendor/llama.cpp/LICENSE`)
- **Bundled components:** third-party sources shipped inside the llama.cpp tree carry their own notices under `vendor/llama.cpp/licenses/`. The static library build in this worker disables the examples, tools, server and `common` library, so those sources are not linked.
- **Note:** An SPDX identifier for the inference engine does not license any model weights, tokenizers, or adapters. Model weights must be reviewed or supplied locally under their own terms.
