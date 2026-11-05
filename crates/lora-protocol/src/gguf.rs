//! Safe, bounded GGUF v2/v3 header and tensor table parser plus deterministic fixture builder.
//!
//! The parser reads only the header region from any [`Read`] source, so a multi-gigabyte model
//! file is never loaded into memory. Every metadata value type is understood; values the app does
//! not use (tokenizer vocabularies, merges, scores, ...) are walked and skipped without being
//! stored. Counts, string lengths, array lengths, array nesting, total header size and all tensor
//! offsets are bounded and overflow-checked to defend against malformed or truncated files
//! (`M2` acceptance test and `R04` risk mitigation).

use crate::messages::ProbedModelMetadata;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;
use thiserror::Error;

pub const GGUF_MAGIC: &[u8; 4] = b"GGUF";
/// Tensor-info entries in one file. Large MoE models stay well below this.
pub const MAX_GGUF_TENSORS: u64 = 65_536;
/// Top-level metadata key/value pairs. Real files have a few dozen.
pub const MAX_GGUF_KV_PAIRS: u64 = 4_096;
/// One string (key, value, array element or tensor name). Chat templates are the largest seen.
pub const MAX_GGUF_STRING_BYTES: u64 = 1024 * 1024;
/// Elements in one metadata array (vocabularies and merge lists are a few hundred thousand).
pub const MAX_GGUF_ARRAY_LEN: u64 = 16 * 1024 * 1024;
/// Nesting depth for arrays of arrays (llama.cpp itself writes none).
pub const MAX_GGUF_ARRAY_DEPTH: u32 = 4;
/// Total bytes of header (fixed fields + metadata + tensor table) the parser will walk.
pub const MAX_GGUF_HEADER_BYTES: u64 = 64 * 1024 * 1024;
/// Dimensions per tensor (`GGML_MAX_DIMS`).
pub const MAX_GGUF_TENSOR_DIMS: u32 = 4;
pub const DEFAULT_ALIGNMENT: u64 = 32;

/// Size of the fixed GGUF preamble: magic, version, tensor count, KV count.
const GGUF_PREAMBLE_BYTES: u64 = 24;
/// Buffer used when reading a file header; small reads are coalesced by it.
const HEADER_READ_BUFFER_BYTES: usize = 64 * 1024;
/// Chunk size used by [`sha256_reader`] and [`sha256_file`].
pub const HASH_CHUNK_BYTES: usize = 1024 * 1024;

// GGUF metadata value type ids (`gguf_type` in ggml/include/gguf.h).
const T_U8: u32 = 0;
const T_I8: u32 = 1;
const T_U16: u32 = 2;
const T_I16: u32 = 3;
const T_U32: u32 = 4;
const T_I32: u32 = 5;
const T_F32: u32 = 6;
const T_BOOL: u32 = 7;
const T_STRING: u32 = 8;
const T_ARRAY: u32 = 9;
const T_U64: u32 = 10;
const T_I64: u32 = 11;
const T_F64: u32 = 12;

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

    #[error("metadata array length {0} exceeds safety bound {MAX_GGUF_ARRAY_LEN}")]
    ArrayTooLong(u64),

    #[error("metadata arrays nested deeper than {MAX_GGUF_ARRAY_DEPTH} levels")]
    ArrayTooDeep,

    #[error("GGUF header is larger than the {MAX_GGUF_HEADER_BYTES}-byte safety bound")]
    HeaderTooLarge,

    #[error("unexpected EOF while parsing GGUF section '{0}'")]
    UnexpectedEof(&'static str),

    #[error("I/O error while reading GGUF header: {0}")]
    Io(String),

    #[error("invalid UTF-8 in GGUF metadata or tensor name")]
    InvalidUtf8,

    #[error("unsupported or corrupt GGUF metadata value type {0}")]
    InvalidValueType(u32),

    #[error("duplicate GGUF metadata key '{0}'")]
    DuplicateKey(String),

    #[error("general.alignment {0} is not a non-zero power of two")]
    InvalidAlignment(u64),

    #[error("missing required GGUF metadata key '{0}'")]
    MissingRequiredKey(&'static str),

    #[error("unsupported model architecture '{0}'")]
    UnsupportedArchitecture(String),

    #[error("unsupported or multimodal GGUF artifact (projector/vision tensors are not qualified for MVP text worker)")]
    UnsupportedModality,

    #[error("missing tokenizer.chat_template required for chat/agent qualification")]
    MissingChatTemplate,

    #[error("tensor '{name}' has unknown ggml type {ggml_type}")]
    InvalidTensorType { name: String, ggml_type: u32 },

    #[error("tensor '{name}' has an invalid shape: {reason}")]
    InvalidTensorShape { name: String, reason: &'static str },

    #[error("tensor '{name}' offset/size ({offset} + {byte_len}) is out of range for file size {file_size}")]
    TensorOutOfBounds {
        name: String,
        offset: u64,
        byte_len: u64,
        file_size: u64,
    },

    #[error("tensor '{name}' offset {offset} is not a multiple of alignment {alignment}")]
    MisalignedTensor {
        name: String,
        offset: u64,
        alignment: u64,
    },
}

/// A stored metadata value. Integers are widened; arrays are validated and skipped, keeping only
/// their element type and length.
#[derive(Debug, Clone, PartialEq)]
pub enum GgufValue {
    UInt(u64),
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Array { elem_type: u32, len: u64 },
}

impl GgufValue {
    /// The value as a `u32` when it is a non-negative integer that fits.
    pub fn as_u32(&self) -> Option<u32> {
        match self {
            GgufValue::UInt(v) => u32::try_from(*v).ok(),
            GgufValue::Int(v) => u32::try_from(*v).ok(),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            GgufValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            GgufValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedGguf {
    pub version: u32,
    pub metadata: ProbedModelMetadata,
    /// Every top-level string metadata value.
    pub kv_strings: BTreeMap<String, String>,
}

/// Byte size of one element block for each ggml tensor type: `(block_elems, block_bytes)`.
///
/// Mirrors `type_traits` in ggml/src/ggml.c of the pinned llama.cpp. Removed ids return `None`.
pub fn ggml_type_block(ggml_type: u32) -> Option<(u64, u64)> {
    const QK_K: u64 = 256;
    Some(match ggml_type {
        0 => (1, 4),       // F32
        1 => (1, 2),       // F16
        2 => (32, 18),     // Q4_0
        3 => (32, 20),     // Q4_1
        6 => (32, 22),     // Q5_0
        7 => (32, 24),     // Q5_1
        8 => (32, 34),     // Q8_0
        9 => (32, 36),     // Q8_1
        10 => (QK_K, 84),  // Q2_K
        11 => (QK_K, 110), // Q3_K
        12 => (QK_K, 144), // Q4_K
        13 => (QK_K, 176), // Q5_K
        14 => (QK_K, 210), // Q6_K
        15 => (QK_K, 292), // Q8_K
        16 => (QK_K, 66),  // IQ2_XXS
        17 => (QK_K, 74),  // IQ2_XS
        18 => (QK_K, 98),  // IQ3_XXS
        19 => (QK_K, 50),  // IQ1_S
        20 => (32, 18),    // IQ4_NL
        21 => (QK_K, 110), // IQ3_S
        22 => (QK_K, 82),  // IQ2_S
        23 => (QK_K, 136), // IQ4_XS
        24 => (1, 1),      // I8
        25 => (1, 2),      // I16
        26 => (1, 4),      // I32
        27 => (1, 8),      // I64
        28 => (1, 8),      // F64
        29 => (QK_K, 56),  // IQ1_M
        30 => (1, 2),      // BF16
        34 => (QK_K, 54),  // TQ1_0
        35 => (QK_K, 66),  // TQ2_0
        39 => (32, 17),    // MXFP4
        40 => (64, 36),    // NVFP4
        41 => (128, 18),   // Q1_0
        42 => (64, 18),    // Q2_0
        43 => (1, 1),      // F8_E4M3
        _ => return None,
    })
}

/// Human-readable name for `general.file_type` (`llama_ftype`), used when a file has no
/// `general.quantization` string.
fn file_type_name(file_type: u32) -> Option<&'static str> {
    Some(match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4_MOE",
        _ => return None,
    })
}

/// Fixed byte width of a scalar metadata type, or `None` for strings/arrays/unknown ids.
fn scalar_width(value_type: u32) -> Option<u64> {
    match value_type {
        T_U8 | T_I8 | T_BOOL => Some(1),
        T_U16 | T_I16 => Some(2),
        T_U32 | T_I32 | T_F32 => Some(4),
        T_U64 | T_I64 | T_F64 => Some(8),
        _ => None,
    }
}

/// Sequential reader over the header region that tracks its position and refuses to go past the
/// file size or [`MAX_GGUF_HEADER_BYTES`] before touching the underlying reader.
struct HeaderReader<R> {
    inner: R,
    pos: u64,
    file_size: u64,
}

impl<R: Read> HeaderReader<R> {
    fn reserve(&self, len: u64, section: &'static str) -> Result<(), GgufError> {
        let end = self.pos.checked_add(len).ok_or(GgufError::HeaderTooLarge)?;
        if end > self.file_size {
            return Err(GgufError::UnexpectedEof(section));
        }
        if end > MAX_GGUF_HEADER_BYTES {
            return Err(GgufError::HeaderTooLarge);
        }
        Ok(())
    }

    fn read_into(&mut self, buf: &mut [u8], section: &'static str) -> Result<(), GgufError> {
        self.reserve(buf.len() as u64, section)?;
        self.inner
            .read_exact(buf)
            .map_err(|e| io_error(e, section))?;
        self.pos += buf.len() as u64;
        Ok(())
    }

    fn read_array<const N: usize>(&mut self, section: &'static str) -> Result<[u8; N], GgufError> {
        let mut buf = [0u8; N];
        self.read_into(&mut buf, section)?;
        Ok(buf)
    }

    fn read_u32(&mut self, section: &'static str) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.read_array(section)?))
    }

    fn read_u64(&mut self, section: &'static str) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.read_array(section)?))
    }

    /// Discard `len` bytes without buffering them.
    fn skip(&mut self, len: u64, section: &'static str) -> Result<(), GgufError> {
        self.reserve(len, section)?;
        let copied = io::copy(&mut (&mut self.inner).take(len), &mut io::sink())
            .map_err(|e| io_error(e, section))?;
        if copied != len {
            return Err(GgufError::UnexpectedEof(section));
        }
        self.pos += len;
        Ok(())
    }

    fn read_string_len(&mut self, section: &'static str) -> Result<u64, GgufError> {
        let len = self.read_u64(section)?;
        if len > MAX_GGUF_STRING_BYTES {
            return Err(GgufError::StringTooLong(len));
        }
        Ok(len)
    }

    fn read_string(&mut self, section: &'static str) -> Result<String, GgufError> {
        let len = self.read_string_len(section)?;
        // Checked against the file size before allocating.
        self.reserve(len, section)?;
        let mut raw = vec![0u8; len as usize];
        self.read_into(&mut raw, section)?;
        String::from_utf8(raw).map_err(|_| GgufError::InvalidUtf8)
    }

    fn skip_string(&mut self, section: &'static str) -> Result<(), GgufError> {
        let len = self.read_string_len(section)?;
        self.skip(len, section)
    }

    /// Read one metadata value of `value_type`. Arrays are walked and skipped.
    fn read_value(&mut self, value_type: u32) -> Result<GgufValue, GgufError> {
        const S: &str = "kv.value";
        Ok(match value_type {
            T_U8 => GgufValue::UInt(u64::from(self.read_array::<1>(S)?[0])),
            T_I8 => GgufValue::Int(i64::from(i8::from_le_bytes(self.read_array(S)?))),
            T_U16 => GgufValue::UInt(u64::from(u16::from_le_bytes(self.read_array(S)?))),
            T_I16 => GgufValue::Int(i64::from(i16::from_le_bytes(self.read_array(S)?))),
            T_U32 => GgufValue::UInt(u64::from(self.read_u32(S)?)),
            T_I32 => GgufValue::Int(i64::from(i32::from_le_bytes(self.read_array(S)?))),
            T_F32 => GgufValue::Float(f64::from(f32::from_le_bytes(self.read_array(S)?))),
            T_BOOL => GgufValue::Bool(self.read_array::<1>(S)?[0] != 0),
            T_STRING => GgufValue::String(self.read_string(S)?),
            T_U64 => GgufValue::UInt(self.read_u64(S)?),
            T_I64 => GgufValue::Int(i64::from_le_bytes(self.read_array(S)?)),
            T_F64 => GgufValue::Float(f64::from_le_bytes(self.read_array(S)?)),
            T_ARRAY => {
                let (elem_type, len) = self.skip_array(1)?;
                GgufValue::Array { elem_type, len }
            }
            other => return Err(GgufError::InvalidValueType(other)),
        })
    }

    /// Read an array header (element type + length) and skip its body. `depth` is this array's
    /// nesting level, starting at 1.
    fn skip_array(&mut self, depth: u32) -> Result<(u32, u64), GgufError> {
        const S: &str = "kv.array";
        if depth > MAX_GGUF_ARRAY_DEPTH {
            return Err(GgufError::ArrayTooDeep);
        }
        let elem_type = self.read_u32(S)?;
        let len = self.read_u64(S)?;
        if len > MAX_GGUF_ARRAY_LEN {
            return Err(GgufError::ArrayTooLong(len));
        }
        if let Some(width) = scalar_width(elem_type) {
            let body = len.checked_mul(width).ok_or(GgufError::HeaderTooLarge)?;
            self.skip(body, S)?;
        } else {
            match elem_type {
                T_STRING => {
                    for _ in 0..len {
                        self.skip_string(S)?;
                    }
                }
                T_ARRAY => {
                    for _ in 0..len {
                        self.skip_array(depth + 1)?;
                    }
                }
                other => return Err(GgufError::InvalidValueType(other)),
            }
        }
        Ok((elem_type, len))
    }
}

fn io_error(e: io::Error, section: &'static str) -> GgufError {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        GgufError::UnexpectedEof(section)
    } else {
        GgufError::Io(e.to_string())
    }
}

/// Parse and validate a GGUF file held in memory. Convenience wrapper over [`parse_gguf_reader`].
pub fn parse_gguf_bytes(bytes: &[u8]) -> Result<ParsedGguf, GgufError> {
    parse_gguf_reader(bytes, bytes.len() as u64)
}

/// Open `path` and parse its GGUF header. Only the header region is read.
pub fn parse_gguf_file(path: &Path) -> Result<ParsedGguf, GgufError> {
    let file = File::open(path).map_err(|e| GgufError::Io(e.to_string()))?;
    let file_size = file
        .metadata()
        .map_err(|e| GgufError::Io(e.to_string()))?
        .len();
    parse_gguf_reader(
        BufReader::with_capacity(HEADER_READ_BUFFER_BYTES, file),
        file_size,
    )
}

/// Parse and validate a GGUF header from `reader`, positioned at the start of a file of
/// `file_size` bytes. Reads only the header and tensor table; tensor data is bounds-checked
/// against `file_size` but never read.
pub fn parse_gguf_reader<R: Read>(reader: R, file_size: u64) -> Result<ParsedGguf, GgufError> {
    if file_size < GGUF_PREAMBLE_BYTES {
        return Err(GgufError::TruncatedHeader);
    }
    let mut cur = HeaderReader {
        inner: reader,
        pos: 0,
        file_size,
    };

    let magic = cur.read_array::<4>("magic")?;
    if &magic != GGUF_MAGIC {
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

    let mut kv: BTreeMap<String, GgufValue> = BTreeMap::new();
    for _ in 0..kv_count {
        let key = cur.read_string("kv.key")?;
        let value_type = cur.read_u32("kv.type")?;
        let value = cur.read_value(value_type)?;
        if kv.contains_key(&key) {
            return Err(GgufError::DuplicateKey(key));
        }
        kv.insert(key, value);
    }

    let kv_strings: BTreeMap<String, String> = kv
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    let get_u32 = |key: &str| kv.get(key).and_then(GgufValue::as_u32);

    let alignment = match kv.get("general.alignment") {
        None => DEFAULT_ALIGNMENT,
        Some(v) => match v.as_u32() {
            Some(a) if a.is_power_of_two() => u64::from(a),
            Some(a) => return Err(GgufError::InvalidAlignment(u64::from(a))),
            None => return Err(GgufError::InvalidAlignment(0)),
        },
    };

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

    let template_digest = hex::encode(Sha256::digest(chat_template.as_bytes()));

    let model_name = kv_strings
        .get("general.name")
        .cloned()
        .unwrap_or_else(|| architecture.clone());
    let quantization = kv_strings
        .get("general.quantization")
        .cloned()
        .or_else(|| {
            get_u32("general.file_type")
                .and_then(file_type_name)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());
    let license = kv_strings
        .get("general.license")
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());

    // A stored 0 is treated as missing so the defaults below apply. Per-layer arrays (e.g. a
    // `head_count_kv` array) are not read and fall back to the defaults too.
    let arch_u32 = |suffix: &str| get_u32(&format!("{architecture}.{suffix}")).filter(|v| *v > 0);
    let context_length = arch_u32("context_length").unwrap_or(4096);
    let block_count = arch_u32("block_count").unwrap_or(24);
    let head_count = arch_u32("attention.head_count").unwrap_or(16);
    // llama.cpp defaults the KV head count to the head count (no grouped-query attention).
    let kv_head_count = arch_u32("attention.head_count_kv").unwrap_or(head_count);
    let embedding_length = arch_u32("embedding_length").unwrap_or(2048);

    let default_head_dim = embedding_length.checked_div(head_count).unwrap_or(128);
    let key_dim = arch_u32("attention.key_length").unwrap_or(default_head_dim);
    let value_dim = arch_u32("attention.value_length").unwrap_or(default_head_dim);

    let supports_tool_calling = kv
        .get("lora.supports_tools")
        .and_then(GgufValue::as_bool)
        .unwrap_or_else(|| chat_template.contains("tool") || chat_template.contains("read_file"));

    // Tensor info entries: name, n_dims (u32), dims (u64 * n_dims), ggml_type (u32), offset (u64).
    let mut tensor_entries = Vec::with_capacity(tensor_count.min(4096) as usize);
    for _ in 0..tensor_count {
        let name = cur.read_string("tensor.name")?;
        let shape_err = |reason| GgufError::InvalidTensorShape {
            name: name.clone(),
            reason,
        };
        let n_dims = cur.read_u32("tensor.n_dims")?;
        if n_dims > MAX_GGUF_TENSOR_DIMS {
            return Err(shape_err("too many dimensions"));
        }
        let mut elements: u64 = 1;
        let mut row_len: u64 = 1;
        for i in 0..n_dims {
            let dim = cur.read_u64("tensor.dim")?;
            if dim > i64::MAX as u64 {
                return Err(shape_err("dimension exceeds i64::MAX"));
            }
            if i == 0 {
                row_len = dim;
            }
            elements = elements
                .checked_mul(dim)
                .ok_or_else(|| shape_err("element count overflows"))?;
        }
        let ggml_type = cur.read_u32("tensor.type")?;
        let offset = cur.read_u64("tensor.offset")?;
        let (block_elems, block_bytes) =
            ggml_type_block(ggml_type).ok_or_else(|| GgufError::InvalidTensorType {
                name: name.clone(),
                ggml_type,
            })?;
        if !row_len.is_multiple_of(block_elems) {
            return Err(shape_err(
                "row length is not a multiple of the type block size",
            ));
        }
        let byte_len = (elements / block_elems)
            .checked_mul(block_bytes)
            .ok_or_else(|| shape_err("byte size overflows"))?;
        tensor_entries.push((name, offset, byte_len));
    }

    // Tensor data starts at the next multiple of `alignment` after the header.
    let data_start = cur
        .pos
        .div_ceil(alignment)
        .checked_mul(alignment)
        .ok_or(GgufError::HeaderTooLarge)?;

    let mut total_tensor_bytes = 0u64;
    for (name, offset, byte_len) in tensor_entries {
        let in_bounds = data_start
            .checked_add(offset)
            .and_then(|start| start.checked_add(byte_len))
            .is_some_and(|end| end <= file_size);
        if !in_bounds {
            return Err(GgufError::TensorOutOfBounds {
                name,
                offset,
                byte_len,
                file_size,
            });
        }
        if !offset.is_multiple_of(alignment) {
            return Err(GgufError::MisalignedTensor {
                name,
                offset,
                alignment,
            });
        }
        total_tensor_bytes = total_tensor_bytes.saturating_add(byte_len);
    }

    // Synthetic test fixtures declare a simulated weight footprint via `lora.simulated_weight_mb`.
    if let Some(sim_mb) = get_u32("lora.simulated_weight_mb") {
        total_tensor_bytes = u64::from(sim_mb) * 1024 * 1024;
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

/// SHA-256 of everything `reader` yields, read in [`HASH_CHUNK_BYTES`] chunks. Returns the hex
/// digest and the number of bytes hashed.
pub fn sha256_reader<R: Read>(mut reader: R) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_CHUNK_BYTES];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), total))
}

/// Hex SHA-256 of the file at `path`, streamed in fixed-size chunks.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    sha256_reader(File::open(path)?).map(|(digest, _)| digest)
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
    /// Elements of the single Q8_0 tensor; must be a multiple of 32.
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
    buf.extend_from_slice(&T_STRING.to_le_bytes());
    push_string(buf, val);
}

fn push_kv_u32(buf: &mut Vec<u8>, key: &str, val: u32) {
    push_string(buf, key);
    buf.extend_from_slice(&T_U32.to_le_bytes());
    buf.extend_from_slice(&val.to_le_bytes());
}

fn push_kv_bool(buf: &mut Vec<u8>, key: &str, val: bool) {
    push_string(buf, key);
    buf.extend_from_slice(&T_BOOL.to_le_bytes());
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

    // Tensor 0: "token_embd.weight", 1D, spec.tensor_elements, type 8 (Q8_0: 34 bytes per 32)
    const Q8_0: u32 = 8;
    push_string(&mut out, "token_embd.weight");
    out.extend_from_slice(&1u32.to_le_bytes()); // n_dims = 1
    out.extend_from_slice(&spec.tensor_elements.to_le_bytes());
    out.extend_from_slice(&Q8_0.to_le_bytes());
    let offset = spec.tensor_offset_override.unwrap_or(0);
    out.extend_from_slice(&offset.to_le_bytes());

    let alignment = DEFAULT_ALIGNMENT as usize;
    if !out.len().is_multiple_of(alignment) {
        let pad = alignment - (out.len() % alignment);
        out.resize(out.len() + pad, 0u8);
    }

    // Append the tensor data bytes
    let (block_elems, block_bytes) = ggml_type_block(Q8_0).expect("Q8_0 is a known type");
    let data_len = spec.tensor_elements / block_elems * block_bytes;
    out.resize(out.len() + data_len as usize, 0x5Au8);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal GGUF writer for tests: arbitrary metadata, tensors and raw corruption.
    #[derive(Default)]
    struct Builder {
        version: u32,
        kv_count: Option<u64>,
        kv: Vec<u8>,
        n_kv: u64,
        tensors: Vec<u8>,
        n_tensors: u64,
        data_len: u64,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                version: 3,
                ..Self::default()
            }
        }

        /// Valid qwen2-style header with chat template, ready for extra keys.
        fn base() -> Self {
            Self::new()
                .string("general.architecture", "qwen2")
                .string("tokenizer.chat_template", "{{ messages }}")
        }

        fn raw(mut self, key: &str, value_type: u32, payload: &[u8]) -> Self {
            push_string(&mut self.kv, key);
            self.kv.extend_from_slice(&value_type.to_le_bytes());
            self.kv.extend_from_slice(payload);
            self.n_kv += 1;
            self
        }

        fn string(self, key: &str, val: &str) -> Self {
            let mut p = Vec::new();
            push_string(&mut p, val);
            self.raw(key, T_STRING, &p)
        }

        fn u32(self, key: &str, val: u32) -> Self {
            self.raw(key, T_U32, &val.to_le_bytes())
        }

        /// Array of fixed-width scalars given as already-encoded element bytes.
        fn array(self, key: &str, elem_type: u32, len: u64, body: &[u8]) -> Self {
            let mut p = Vec::new();
            p.extend_from_slice(&elem_type.to_le_bytes());
            p.extend_from_slice(&len.to_le_bytes());
            p.extend_from_slice(body);
            self.raw(key, T_ARRAY, &p)
        }

        fn tensor(mut self, name: &str, dims: &[u64], ggml_type: u32, offset: u64) -> Self {
            push_string(&mut self.tensors, name);
            self.tensors
                .extend_from_slice(&(dims.len() as u32).to_le_bytes());
            for d in dims {
                self.tensors.extend_from_slice(&d.to_le_bytes());
            }
            self.tensors.extend_from_slice(&ggml_type.to_le_bytes());
            self.tensors.extend_from_slice(&offset.to_le_bytes());
            self.n_tensors += 1;
            self
        }

        fn data(mut self, len: u64) -> Self {
            self.data_len = len;
            self
        }

        fn build(self) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(GGUF_MAGIC);
            out.extend_from_slice(&self.version.to_le_bytes());
            out.extend_from_slice(&self.n_tensors.to_le_bytes());
            out.extend_from_slice(&self.kv_count.unwrap_or(self.n_kv).to_le_bytes());
            out.extend_from_slice(&self.kv);
            out.extend_from_slice(&self.tensors);
            let aligned = out.len().div_ceil(32) * 32;
            out.resize(aligned + self.data_len as usize, 0);
            out
        }
    }

    fn parse(bytes: &[u8]) -> Result<ParsedGguf, GgufError> {
        parse_gguf_bytes(bytes)
    }

    fn le_strings(items: &[&str]) -> Vec<u8> {
        let mut b = Vec::new();
        for s in items {
            push_string(&mut b, s);
        }
        b
    }

    #[test]
    fn default_fixture_parses() {
        let parsed = parse(&build_synthetic_gguf(&SyntheticGgufSpec::default())).unwrap();
        assert_eq!(parsed.version, 3);
        assert_eq!(parsed.metadata.architecture, "llama");
        assert_eq!(parsed.metadata.key_dim, 128);
        assert_eq!(parsed.metadata.kv_head_count, 4);
        assert_eq!(parsed.metadata.total_tensor_bytes, 64 * 1024 * 1024);
    }

    #[test]
    fn every_scalar_value_type_is_accepted() {
        let bytes = Builder::base()
            .raw("t.u8", T_U8, &[7])
            .raw("t.i8", T_I8, &(-7i8).to_le_bytes())
            .raw("t.u16", T_U16, &700u16.to_le_bytes())
            .raw("t.i16", T_I16, &(-700i16).to_le_bytes())
            .raw("t.u32", T_U32, &70_000u32.to_le_bytes())
            .raw("t.i32", T_I32, &(-70_000i32).to_le_bytes())
            .raw("t.f32", T_F32, &1.5f32.to_le_bytes())
            .raw("t.bool", T_BOOL, &[1])
            .raw("t.u64", T_U64, &(u64::MAX).to_le_bytes())
            .raw("t.i64", T_I64, &(i64::MIN).to_le_bytes())
            .raw("t.f64", T_F64, &(-2.25f64).to_le_bytes())
            .string("t.string", "hello")
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.kv_strings.get("t.string").unwrap(), "hello");
        assert_eq!(parsed.metadata.architecture, "qwen2");
    }

    #[test]
    fn integer_metadata_of_any_width_is_used() {
        let bytes = Builder::base()
            .raw("qwen2.context_length", T_U64, &32_768u64.to_le_bytes())
            .raw("qwen2.block_count", T_I32, &24i32.to_le_bytes())
            .raw("qwen2.attention.head_count", T_U16, &14u16.to_le_bytes())
            .raw("qwen2.attention.head_count_kv", T_U8, &[2])
            .u32("qwen2.embedding_length", 896)
            .build();
        let m = parse(&bytes).unwrap().metadata;
        assert_eq!(m.context_length, 32_768);
        assert_eq!(m.block_count, 24);
        assert_eq!(m.head_count, 14);
        assert_eq!(m.kv_head_count, 2);
        assert_eq!(m.key_dim, 64);
        assert_eq!(m.value_dim, 64);
    }

    #[test]
    fn explicit_key_and_value_lengths_override_head_dim() {
        let bytes = Builder::base()
            .u32("qwen2.attention.head_count", 16)
            .u32("qwen2.embedding_length", 2048)
            .u32("qwen2.attention.key_length", 192)
            .u32("qwen2.attention.value_length", 96)
            .build();
        let m = parse(&bytes).unwrap().metadata;
        assert_eq!(m.key_dim, 192);
        assert_eq!(m.value_dim, 96);
        // No head_count_kv: defaults to head_count like llama.cpp.
        assert_eq!(m.kv_head_count, 16);
    }

    #[test]
    fn quantization_falls_back_to_file_type() {
        let bytes = Builder::base().u32("general.file_type", 15).build();
        assert_eq!(parse(&bytes).unwrap().metadata.quantization, "Q4_K_M");
        let bytes = Builder::base().build();
        assert_eq!(parse(&bytes).unwrap().metadata.quantization, "unknown");
    }

    #[test]
    fn arrays_of_every_kind_are_skipped() {
        let mut nested = Vec::new();
        for inner in [&[1u32, 2][..], &[3u32][..]] {
            nested.extend_from_slice(&T_U32.to_le_bytes());
            nested.extend_from_slice(&(inner.len() as u64).to_le_bytes());
            for v in inner {
                nested.extend_from_slice(&v.to_le_bytes());
            }
        }
        let floats: Vec<u8> = [0.5f32, -1.0, 3.25]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let bytes = Builder::base()
            .array(
                "tokenizer.ggml.tokens",
                T_STRING,
                3,
                &le_strings(&["<s>", "hello", "world"]),
            )
            .array("tokenizer.ggml.scores", T_F32, 3, &floats)
            .array("tokenizer.ggml.token_type", T_I32, 1, &1i32.to_le_bytes())
            .array("t.bools", T_BOOL, 2, &[0, 1])
            .array("t.f64", T_F64, 1, &1.0f64.to_le_bytes())
            .array("t.empty", T_U64, 0, &[])
            .array("t.nested", T_ARRAY, 2, &nested)
            .string("general.name", "after-arrays")
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.metadata.model_name, "after-arrays");
        assert!(!parsed.kv_strings.contains_key("tokenizer.ggml.tokens"));
    }

    #[test]
    fn large_vocabulary_array_parses_without_storing_it() {
        let tokens: Vec<String> = (0..200_000).map(|i| format!("tok{i}")).collect();
        let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
        let bytes = Builder::base()
            .array(
                "tokenizer.ggml.tokens",
                T_STRING,
                200_000,
                &le_strings(&refs),
            )
            .build();
        assert!(parse(&bytes).is_ok());
    }

    #[test]
    fn tensors_use_real_block_sizes() {
        // Q4_K: 144 bytes per 256 elements; F32: 4 bytes per element.
        let q4k = 896 * 256 / 256 * 144;
        let bytes = Builder::base()
            .tensor("blk.0.attn_q.weight", &[256, 896], 12, 0)
            .tensor("output_norm.weight", &[896], 0, q4k)
            .data(q4k + 896 * 4)
            .build();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.metadata.tensor_count, 2);
        assert_eq!(parsed.metadata.total_tensor_bytes, q4k + 896 * 4);

        // One byte short of the last tensor.
        let short = Builder::base().tensor("t", &[256], 12, 0).data(143).build();
        assert!(matches!(
            parse(&short),
            Err(GgufError::TensorOutOfBounds { .. })
        ));

        // Ending exactly at the end of the file is fine.
        let exact = Builder::base().tensor("t", &[256], 12, 0).data(144).build();
        assert!(parse(&exact).is_ok());
    }

    #[test]
    fn signed_alignment_and_zero_head_counts() {
        let bytes = Builder::base()
            .raw("general.alignment", T_I32, &64i32.to_le_bytes())
            .u32("qwen2.attention.head_count", 0)
            .u32("qwen2.attention.head_count_kv", 0)
            .build();
        let m = parse(&bytes).unwrap().metadata;
        assert_eq!(m.head_count, 16);
        assert_eq!(m.kv_head_count, 16);

        let negative = Builder::base()
            .raw("general.alignment", T_I32, &(-32i32).to_le_bytes())
            .build();
        assert_eq!(parse(&negative), Err(GgufError::InvalidAlignment(0)));
    }

    #[test]
    fn honours_general_alignment() {
        let bytes = Builder::base()
            .u32("general.alignment", 64)
            .tensor("t", &[32], 0, 64)
            .data(64 + 128 + 64)
            .build();
        assert!(parse(&bytes).is_ok());

        let misaligned = Builder::base()
            .u32("general.alignment", 64)
            .tensor("t", &[32], 0, 32)
            .data(256)
            .build();
        assert!(matches!(
            parse(&misaligned),
            Err(GgufError::MisalignedTensor { .. })
        ));

        let bad = Builder::base().u32("general.alignment", 48).build();
        assert_eq!(parse(&bad), Err(GgufError::InvalidAlignment(48)));
    }

    #[test]
    fn rejects_bad_tensor_tables() {
        let unknown_type = Builder::base().tensor("t", &[32], 4, 0).data(64).build();
        assert!(matches!(
            parse(&unknown_type),
            Err(GgufError::InvalidTensorType { ggml_type: 4, .. })
        ));

        let ragged_row = Builder::base()
            .tensor("t", &[100], 12, 0)
            .data(1024)
            .build();
        assert!(matches!(
            parse(&ragged_row),
            Err(GgufError::InvalidTensorShape { .. })
        ));

        let too_many_dims = Builder::base()
            .tensor("t", &[1, 1, 1, 1, 1], 0, 0)
            .data(64)
            .build();
        assert!(matches!(
            parse(&too_many_dims),
            Err(GgufError::InvalidTensorShape { .. })
        ));

        let overflow = Builder::base()
            .tensor("t", &[1 << 40, 1 << 40], 0, 0)
            .build();
        assert!(matches!(
            parse(&overflow),
            Err(GgufError::InvalidTensorShape { .. })
        ));

        let huge_offset = Builder::base()
            .tensor("t", &[32], 0, u64::MAX - 31)
            .data(128)
            .build();
        assert!(matches!(
            parse(&huge_offset),
            Err(GgufError::TensorOutOfBounds { .. })
        ));
    }

    #[test]
    fn rejects_malformed_metadata() {
        let bad_type = Builder::base().raw("x", 13, &[0; 8]).build();
        assert_eq!(parse(&bad_type), Err(GgufError::InvalidValueType(13)));

        let bad_elem = Builder::base().array("x", 99, 0, &[]).build();
        assert_eq!(parse(&bad_elem), Err(GgufError::InvalidValueType(99)));

        let dup = Builder::base().u32("x", 1).u32("x", 2).build();
        assert_eq!(parse(&dup), Err(GgufError::DuplicateKey("x".into())));

        let bad_utf8 = Builder::base().raw("x", T_STRING, &{
            let mut p = 2u64.to_le_bytes().to_vec();
            p.extend_from_slice(&[0xff, 0xfe]);
            p
        });
        assert_eq!(parse(&bad_utf8.build()), Err(GgufError::InvalidUtf8));

        let mut bad_magic = Builder::base().build();
        bad_magic[0] = b'X';
        assert_eq!(parse(&bad_magic), Err(GgufError::InvalidMagic));

        let mut v1 = Builder::base();
        v1.version = 1;
        assert_eq!(parse(&v1.build()), Err(GgufError::UnsupportedVersion(1)));

        assert_eq!(parse(&[0u8; 10]), Err(GgufError::TruncatedHeader));
    }

    #[test]
    fn rejects_oversized_inputs() {
        let long_string = Builder::base()
            .raw("x", T_STRING, &(MAX_GGUF_STRING_BYTES + 1).to_le_bytes())
            .build();
        assert_eq!(
            parse(&long_string),
            Err(GgufError::StringTooLong(MAX_GGUF_STRING_BYTES + 1))
        );

        let long_array = Builder::base()
            .array("x", T_U8, MAX_GGUF_ARRAY_LEN + 1, &[])
            .build();
        assert_eq!(
            parse(&long_array),
            Err(GgufError::ArrayTooLong(MAX_GGUF_ARRAY_LEN + 1))
        );

        // Array body claims more bytes than the file holds.
        let short_array = Builder::base().array("x", T_U64, 1000, &[0; 16]).build();
        assert!(matches!(
            parse(&short_array),
            Err(GgufError::UnexpectedEof(_))
        ));

        // An absurd length is refused before any width multiplication.
        let overflow_array = Builder::base().array("x", T_U64, u64::MAX / 4, &[]).build();
        assert!(matches!(
            parse(&overflow_array),
            Err(GgufError::ArrayTooLong(_))
        ));

        // Nesting deeper than the bound.
        let mut deep = Vec::new();
        for _ in 0..MAX_GGUF_ARRAY_DEPTH {
            deep.extend_from_slice(&T_ARRAY.to_le_bytes());
            deep.extend_from_slice(&1u64.to_le_bytes());
        }
        deep.extend_from_slice(&T_U8.to_le_bytes());
        deep.extend_from_slice(&0u64.to_le_bytes());
        let too_deep = Builder::base().raw("x", T_ARRAY, &deep).build();
        assert_eq!(parse(&too_deep), Err(GgufError::ArrayTooDeep));

        let mut many_kv = Builder::base();
        many_kv.kv_count = Some(MAX_GGUF_KV_PAIRS + 1);
        assert_eq!(
            parse(&many_kv.build()),
            Err(GgufError::ExcessiveKvCount(MAX_GGUF_KV_PAIRS + 1))
        );

        let mut many_tensors = Builder::base().build();
        many_tensors[8..16].copy_from_slice(&(MAX_GGUF_TENSORS + 1).to_le_bytes());
        assert_eq!(
            parse(&many_tensors),
            Err(GgufError::ExcessiveTensorCount(MAX_GGUF_TENSORS + 1))
        );
    }

    #[test]
    fn header_size_cap_is_enforced_without_reading_the_body() {
        // A reader that claims a huge file and yields endless zeros: an array whose body is
        // larger than the header cap (16 Mi x 8 bytes) must be refused before it is read.
        let header = Builder::base()
            .array("x", T_U64, MAX_GGUF_ARRAY_LEN, &[])
            .build();
        let reader = header.as_slice().chain(io::repeat(0));
        assert_eq!(
            parse_gguf_reader(reader, u64::MAX),
            Err(GgufError::HeaderTooLarge)
        );
    }

    #[test]
    fn truncated_files_fail_cleanly_at_every_length() {
        let full = Builder::base()
            .array(
                "tokenizer.ggml.tokens",
                T_STRING,
                2,
                &le_strings(&["a", "b"]),
            )
            .tensor("t", &[32], 0, 0)
            .data(128)
            .build();
        assert!(parse(&full).is_ok());
        let header_len = full.len() - 128;
        for cut in 0..header_len {
            assert!(parse(&full[..cut]).is_err(), "cut at {cut} must fail");
        }
    }

    /// `LORA_TEST_GGUF=/path/model.gguf cargo test -p lora-protocol -- --ignored real_gguf`
    #[test]
    #[ignore = "needs a real GGUF file in LORA_TEST_GGUF"]
    fn real_gguf_header_parses() {
        let path = std::env::var("LORA_TEST_GGUF").expect("set LORA_TEST_GGUF");
        let parsed = parse_gguf_file(Path::new(&path)).expect("real GGUF must parse");
        let m = &parsed.metadata;
        println!("{m:#?}");
        assert!(m.tensor_count > 0);
        assert!(m.total_tensor_bytes > 0 && m.total_tensor_bytes < m.file_size_bytes);
        assert!(m.key_dim > 0 && m.value_dim > 0 && m.kv_head_count > 0);
    }

    #[test]
    fn file_parser_and_hasher_stream_from_disk() {
        let dir = std::env::temp_dir().join(format!("lora-gguf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fixture.gguf");
        let bytes = build_synthetic_gguf(&SyntheticGgufSpec::default());
        std::fs::write(&path, &bytes).unwrap();

        let from_file = parse_gguf_file(&path).unwrap();
        assert_eq!(from_file, parse(&bytes).unwrap());
        assert_eq!(
            sha256_file(&path).unwrap(),
            hex::encode(Sha256::digest(&bytes))
        );

        // Multi-chunk input hashes the same as a one-shot digest.
        let big = vec![0xA5u8; HASH_CHUNK_BYTES * 2 + 17];
        let (digest, len) = sha256_reader(big.as_slice()).unwrap();
        assert_eq!(len, big.len() as u64);
        assert_eq!(digest, hex::encode(Sha256::digest(&big)));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
