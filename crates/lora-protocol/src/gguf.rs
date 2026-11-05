//! Safe, bounded GGUF v2/v3 header and tensor table parser plus deterministic fixture builder.
//!
//! Enforces strict counts, string bounds, and tensor offset validation to defend against
//! malformed or truncated GGUF files (`M2` acceptance test and `R04` risk mitigation).

use crate::messages::ProbedModelMetadata;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

pub const GGUF_MAGIC: &[u8; 4] = b"GGUF";
pub const MAX_GGUF_TENSORS: u64 = 16_384;
pub const MAX_GGUF_KV_PAIRS: u64 = 4_096;
pub const MAX_GGUF_STRING_BYTES: usize = 65_536;
pub const DEFAULT_ALIGNMENT: u64 = 32;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GgufError {
    #[error("file is truncated or smaller than the 24-byte GGUF header")]
    TruncatedHeader,

    #[error("invalid file magic (expected 'GGUF')")]
    InvalidMagic,

    #[error("unsupported GGUF format version {0} (supported: 2, 3)")]
    UnsupportedVersion(u32),

    #[error("header tensor count {0} exceeds safety bound {MAX_GGUF_TENSORS}")]
    ExcessiveTensorCount(u64),

    #[error("header metadata KV count {0} exceeds safety bound {MAX_GGUF_KV_PAIRS}")]
    ExcessiveKvCount(u64),

    #[error("metadata string length {0} exceeds safety bound {MAX_GGUF_STRING_BYTES}")]
    StringTooLong(u64),

    #[error("unexpected EOF while parsing GGUF section '{0}'")]
    UnexpectedEof(&'static str),

    #[error("invalid UTF-8 in GGUF metadata or tensor name")]
    InvalidUtf8,

    #[error("unsupported or corrupt GGUF metadata value type {0}")]
    InvalidValueType(u32),

    #[error("missing required GGUF metadata key '{0}'")]
    MissingRequiredKey(&'static str),

    #[error("unsupported model architecture '{0}'")]
    UnsupportedArchitecture(String),

    #[error("unsupported or multimodal GGUF artifact (projector/vision tensors are not qualified for MVP text worker)")]
    UnsupportedModality,

    #[error("missing tokenizer.chat_template required for chat/agent qualification")]
    MissingChatTemplate,

    #[error("tensor '{name}' offset/size ({offset} + {byte_len}) is out of range for file size {file_size}")]
    TensorOutOfBounds {
        name: String,
        offset: u64,
        byte_len: u64,
        file_size: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GgufValue {
    UInt32(u32),
    UInt64(u64),
    String(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGguf {
    pub version: u32,
    pub metadata: ProbedModelMetadata,
    pub kv_strings: BTreeMap<String, String>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn read_exact(&mut self, len: usize, section: &'static str) -> Result<&'a [u8], GgufError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(GgufError::UnexpectedEof(section))?;
        if end > self.bytes.len() {
            return Err(GgufError::UnexpectedEof(section));
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u32(&mut self, section: &'static str) -> Result<u32, GgufError> {
        let b = self.read_exact(4, section)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self, section: &'static str) -> Result<u64, GgufError> {
        let b = self.read_exact(8, section)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_string(&mut self, section: &'static str) -> Result<String, GgufError> {
        let len = self.read_u64(section)?;
        if len > MAX_GGUF_STRING_BYTES as u64 {
            return Err(GgufError::StringTooLong(len));
        }
        let raw = self.read_exact(len as usize, section)?;
        std::str::from_utf8(raw)
            .map(|s| s.to_string())
            .map_err(|_| GgufError::InvalidUtf8)
    }
}

/// Parse and validate a GGUF file byte slice with strict structural bounds.
pub fn parse_gguf_bytes(bytes: &[u8]) -> Result<ParsedGguf, GgufError> {
    if bytes.len() < 24 {
        return Err(GgufError::TruncatedHeader);
    }
    let mut cur = Cursor::new(bytes);
    let magic = cur.read_exact(4, "magic")?;
    if magic != GGUF_MAGIC {
        return Err(GgufError::InvalidMagic);
    }

    let version = cur.read_u32("version")?;
    if version != 2 && version != 3 {
        return Err(GgufError::UnsupportedVersion(version));
    }

    let tensor_count = cur.read_u64("tensor_count")?;
    if tensor_count > MAX_GGUF_TENSORS {
        return Err(GgufError::ExcessiveTensorCount(tensor_count));
    }

    let kv_count = cur.read_u64("kv_count")?;
    if kv_count > MAX_GGUF_KV_PAIRS {
        return Err(GgufError::ExcessiveKvCount(kv_count));
    }

    let mut kv_strings = BTreeMap::new();
    let mut kv_u32 = BTreeMap::new();
    let mut kv_bool = BTreeMap::new();

    for _ in 0..kv_count {
        let key = cur.read_string("kv.key")?;
        let val_type = cur.read_u32("kv.type")?;
        match val_type {
            // UINT32 = 4
            4 => {
                let v = cur.read_u32("kv.u32")?;
                kv_u32.insert(key, v);
            }
            // BOOL = 7
            7 => {
                let b = cur.read_exact(1, "kv.bool")?[0];
                kv_bool.insert(key, b != 0);
            }
            // STRING = 8
            8 => {
                let s = cur.read_string("kv.string")?;
                kv_strings.insert(key, s);
            }
            // UINT64 = 10
            10 => {
                let v = cur.read_u64("kv.u64")?;
                if let Ok(v32) = u32::try_from(v) {
                    kv_u32.insert(key, v32);
                }
            }
            other => return Err(GgufError::InvalidValueType(other)),
        }
    }

    let architecture = kv_strings
        .get("general.architecture")
        .cloned()
        .ok_or(GgufError::MissingRequiredKey("general.architecture"))?;

    if architecture == "clip"
        || architecture == "mllama"
        || kv_strings.contains_key("clip.projector_type")
    {
        return Err(GgufError::UnsupportedModality);
    }

    if !matches!(architecture.as_str(), "llama" | "qwen2" | "mistral") {
        return Err(GgufError::UnsupportedArchitecture(architecture));
    }

    let chat_template = kv_strings
        .get("tokenizer.chat_template")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .ok_or(GgufError::MissingChatTemplate)?;

    let mut template_hasher = Sha256::new();
    template_hasher.update(chat_template.as_bytes());
    let template_digest = hex::encode(template_hasher.finalize());

    let model_name = kv_strings
        .get("general.name")
        .cloned()
        .unwrap_or_else(|| format!("{architecture}-fixture"));
    let quantization = kv_strings
        .get("general.quantization")
        .cloned()
        .unwrap_or_else(|| "Q4_K_M".to_string());
    let license = kv_strings
        .get("general.license")
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());

    let context_length = kv_u32
        .get(&format!("{architecture}.context_length"))
        .copied()
        .unwrap_or(4096);
    let block_count = kv_u32
        .get(&format!("{architecture}.block_count"))
        .copied()
        .unwrap_or(24);
    let head_count = kv_u32
        .get(&format!("{architecture}.attention.head_count"))
        .copied()
        .unwrap_or(16);
    let kv_head_count = kv_u32
        .get(&format!("{architecture}.attention.head_count_kv"))
        .copied()
        .unwrap_or(4);
    let embedding_length = kv_u32
        .get(&format!("{architecture}.embedding_length"))
        .copied()
        .unwrap_or(2048);

    let key_dim = embedding_length.checked_div(head_count).unwrap_or(128);
    let value_dim = key_dim;

    let supports_tool_calling = kv_bool
        .get("lora.supports_tools")
        .copied()
        .unwrap_or_else(|| chat_template.contains("tool") || chat_template.contains("read_file"));

    // Parse tensor info entries: name, n_dims (u32), dims (u64 * n_dims), ggml_type (u32), offset (u64)
    let mut tensor_entries = Vec::with_capacity(tensor_count as usize);
    for _ in 0..tensor_count {
        let name = cur.read_string("tensor.name")?;
        let n_dims = cur.read_u32("tensor.n_dims")?;
        if n_dims == 0 || n_dims > 8 {
            return Err(GgufError::UnexpectedEof("tensor.n_dims"));
        }
        let mut elements: u64 = 1;
        for _ in 0..n_dims {
            let dim = cur.read_u64("tensor.dim")?;
            elements = elements
                .checked_mul(dim)
                .ok_or(GgufError::UnexpectedEof("tensor.dim_overflow"))?;
        }
        let ggml_type = cur.read_u32("tensor.type")?;
        let offset = cur.read_u64("tensor.offset")?;
        // Approximate byte size based on ggml_type (0 = F32 (4B), 1 = F16 (2B), 8 = Q8_0 (1B), default = 1B)
        let bytes_per_elem: u64 = match ggml_type {
            0 => 4,
            1 => 2,
            _ => 1,
        };
        let byte_len = elements
            .checked_mul(bytes_per_elem)
            .ok_or(GgufError::UnexpectedEof("tensor.size_overflow"))?;
        tensor_entries.push((name, offset, byte_len));
    }

    let alignment = DEFAULT_ALIGNMENT as usize;
    let data_start = if cur.pos.is_multiple_of(alignment) {
        cur.pos
    } else {
        cur.pos + (alignment - (cur.pos % alignment))
    };

    let file_size = bytes.len() as u64;
    let mut total_tensor_bytes = 0u64;

    for (name, offset, byte_len) in tensor_entries {
        let abs_start =
            (data_start as u64)
                .checked_add(offset)
                .ok_or(GgufError::TensorOutOfBounds {
                    name: name.clone(),
                    offset,
                    byte_len,
                    file_size,
                })?;
        let abs_end = abs_start
            .checked_add(byte_len)
            .ok_or(GgufError::TensorOutOfBounds {
                name: name.clone(),
                offset,
                byte_len,
                file_size,
            })?;
        if abs_end > file_size {
            return Err(GgufError::TensorOutOfBounds {
                name,
                offset,
                byte_len,
                file_size,
            });
        }
        total_tensor_bytes = total_tensor_bytes.saturating_add(byte_len);
    }

    // Allow synthetic test fixtures to declare simulated weight footprint via `lora.simulated_weight_bytes`
    if let Some(sim_mb) = kv_u32.get("lora.simulated_weight_mb") {
        total_tensor_bytes = (*sim_mb as u64) * 1024 * 1024;
    }

    Ok(ParsedGguf {
        version,
        metadata: ProbedModelMetadata {
            format: "gguf".to_string(),
            architecture,
            model_name,
            quantization,
            license,
            context_length,
            block_count,
            head_count,
            kv_head_count,
            embedding_length,
            key_dim,
            value_dim,
            kv_elem_bytes: 2,
            has_chat_template: true,
            template_digest,
            supports_tool_calling,
            tensor_count,
            total_tensor_bytes,
            file_size_bytes: file_size,
        },
        kv_strings,
    })
}

/// Specification for generating deterministic GGUF v3 fixture files for offline testing.
#[derive(Debug, Clone)]
pub struct SyntheticGgufSpec {
    pub architecture: String,
    pub model_name: String,
    pub quantization: String,
    pub license: String,
    pub chat_template: Option<String>,
    pub context_length: u32,
    pub block_count: u32,
    pub head_count: u32,
    pub kv_head_count: u32,
    pub embedding_length: u32,
    pub simulated_weight_mb: u32,
    pub supports_tools: bool,
    pub tensor_offset_override: Option<u64>,
    pub tensor_elements: u64,
}

impl Default for SyntheticGgufSpec {
    fn default() -> Self {
        Self {
            architecture: "llama".to_string(),
            model_name: "lora-fixture-llama-1b-q4_k_m".to_string(),
            quantization: "Q4_K_M".to_string(),
            license: "Apache-2.0".to_string(),
            chat_template: Some(
                "{% for message in messages %}<|{{ message.role }}|>\n{{ message.content }}\n{% endfor %}<|assistant|>\n<!-- tool: read_file -->"
                    .to_string(),
            ),
            context_length: 8192,
            block_count: 16,
            head_count: 16,
            kv_head_count: 4,
            embedding_length: 2048,
            simulated_weight_mb: 64,
            supports_tools: true,
            tensor_offset_override: None,
            tensor_elements: 256,
        }
    }
}

fn push_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u64).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

fn push_kv_string(buf: &mut Vec<u8>, key: &str, val: &str) {
    push_string(buf, key);
    buf.extend_from_slice(&8u32.to_le_bytes());
    push_string(buf, val);
}

fn push_kv_u32(buf: &mut Vec<u8>, key: &str, val: u32) {
    push_string(buf, key);
    buf.extend_from_slice(&4u32.to_le_bytes());
    buf.extend_from_slice(&val.to_le_bytes());
}

fn push_kv_bool(buf: &mut Vec<u8>, key: &str, val: bool) {
    push_string(buf, key);
    buf.extend_from_slice(&7u32.to_le_bytes());
    buf.push(u8::from(val));
}

/// Build a valid (or intentionally malformed) GGUF v3 binary payload from `spec`.
pub fn build_synthetic_gguf(spec: &SyntheticGgufSpec) -> Vec<u8> {
    let mut kv_buf = Vec::new();
    let mut kv_count = 0u64;

    push_kv_string(&mut kv_buf, "general.architecture", &spec.architecture);
    kv_count += 1;
    push_kv_string(&mut kv_buf, "general.name", &spec.model_name);
    kv_count += 1;
    push_kv_string(&mut kv_buf, "general.quantization", &spec.quantization);
    kv_count += 1;
    push_kv_string(&mut kv_buf, "general.license", &spec.license);
    kv_count += 1;

    if let Some(tpl) = &spec.chat_template {
        push_kv_string(&mut kv_buf, "tokenizer.chat_template", tpl);
        kv_count += 1;
    }

    let arch = &spec.architecture;
    push_kv_u32(
        &mut kv_buf,
        &format!("{arch}.context_length"),
        spec.context_length,
    );
    kv_count += 1;
    push_kv_u32(
        &mut kv_buf,
        &format!("{arch}.block_count"),
        spec.block_count,
    );
    kv_count += 1;
    push_kv_u32(
        &mut kv_buf,
        &format!("{arch}.attention.head_count"),
        spec.head_count,
    );
    kv_count += 1;
    push_kv_u32(
        &mut kv_buf,
        &format!("{arch}.attention.head_count_kv"),
        spec.kv_head_count,
    );
    kv_count += 1;
    push_kv_u32(
        &mut kv_buf,
        &format!("{arch}.embedding_length"),
        spec.embedding_length,
    );
    kv_count += 1;
    push_kv_u32(
        &mut kv_buf,
        "lora.simulated_weight_mb",
        spec.simulated_weight_mb,
    );
    kv_count += 1;
    push_kv_bool(&mut kv_buf, "lora.supports_tools", spec.supports_tools);
    kv_count += 1;

    let mut out = Vec::new();
    out.extend_from_slice(GGUF_MAGIC);
    out.extend_from_slice(&3u32.to_le_bytes()); // version 3
    out.extend_from_slice(&1u64.to_le_bytes()); // 1 tensor
    out.extend_from_slice(&kv_count.to_le_bytes());
    out.extend_from_slice(&kv_buf);

    // Tensor 0: "token_embd.weight", 1D, spec.tensor_elements, type 8 (Q8_0 = 1 byte/elem)
    push_string(&mut out, "token_embd.weight");
    out.extend_from_slice(&1u32.to_le_bytes()); // n_dims = 1
    out.extend_from_slice(&spec.tensor_elements.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes()); // ggml_type = 8
    let offset = spec.tensor_offset_override.unwrap_or(0);
    out.extend_from_slice(&offset.to_le_bytes());

    let alignment = DEFAULT_ALIGNMENT as usize;
    if !out.len().is_multiple_of(alignment) {
        let pad = alignment - (out.len() % alignment);
        out.resize(out.len() + pad, 0u8);
    }

    // Append actual tensor data bytes
    out.resize(out.len() + spec.tensor_elements as usize, 0x5Au8);
    out
}
