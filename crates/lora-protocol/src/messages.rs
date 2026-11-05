//! Wire messages, version negotiation, and closed tool schemas for the private worker protocol.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::frame::{read_frame_str, write_frame_str, FrameError};
use std::io::{Read, Write};

pub const SUPPORTED_PROTOCOL_MAJOR: u16 = 1;
pub const CURRENT_PROTOCOL_MINOR: u16 = 0;

/// Pinned upstream llama.cpp revision recorded in the architecture baseline (`docs/INFERENCE_ENGINE.md`).
pub const PINNED_LLAMA_CPP_REVISION: &str = "c6a0ff11a0fe16ac1a2eb569190934593a2b3670";

/// Golden hello payload from `docs/INFERENCE_ENGINE.md`.
pub const GOLDEN_HELLO_JSON: &str = r#"{"type":"hello","versions":[{"major":1,"minor":0}],"nonce":"afcb26831b679a7621eabc53e375cb3a"}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const V1_0: Self = Self { major: 1, minor: 0 };
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("framing error: {0}")]
    Frame(#[from] FrameError),

    #[error("invalid message schema: {0}")]
    InvalidSchema(String),

    #[error("incompatible protocol major versions {offered:?}; host requires major {required}")]
    IncompatibleMajor { offered: Vec<u16>, required: u16 },

    #[error("invalid or mismatched 128-bit launch nonce")]
    InvalidNonce,

    #[error("message '{message_type}' is invalid in worker state '{state}'")]
    InvalidStateTransition { message_type: String, state: String },
}

/// Validate that a nonce is exactly 32 lowercase hexadecimal characters (128 bits).
pub fn validate_nonce(nonce: &str) -> Result<(), ProtocolError> {
    if nonce.len() != 32
        || !nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ProtocolError::InvalidNonce);
    }
    Ok(())
}

/// Negotiate the highest compatible minor version for `SUPPORTED_PROTOCOL_MAJOR`.
/// Rejects empty lists or lists containing only stale/newer major versions.
#[allow(clippy::unnecessary_min_or_max)]
pub fn negotiate_version(offered: &[ProtocolVersion]) -> Result<ProtocolVersion, ProtocolError> {
    if offered.is_empty() {
        return Err(ProtocolError::IncompatibleMajor {
            offered: vec![],
            required: SUPPORTED_PROTOCOL_MAJOR,
        });
    }
    let best = offered
        .iter()
        .copied()
        .filter(|v| v.major == SUPPORTED_PROTOCOL_MAJOR)
        .max_by_key(|v| v.minor);

    match best {
        Some(v) => Ok(ProtocolVersion {
            major: SUPPORTED_PROTOCOL_MAJOR,
            minor: v.minor.min(CURRENT_PROTOCOL_MINOR),
        }),
        None => Err(ProtocolError::IncompatibleMajor {
            offered: offered.iter().map(|v| v.major).collect(),
            required: SUPPORTED_PROTOCOL_MAJOR,
        }),
    }
}

/// Capabilities reported by the worker during the initial handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCapabilities {
    pub supported_formats: Vec<String>,
    pub supported_architectures: Vec<String>,
    pub supported_quantizations: Vec<String>,
    pub max_context_length: u32,
    pub supports_structured_tools: bool,
    pub compute_backends: Vec<String>,
    pub cpu_isa_features: Vec<String>,
}

impl Default for WorkerCapabilities {
    fn default() -> Self {
        Self {
            supported_formats: vec!["gguf".to_string()],
            supported_architectures: vec![
                "llama".to_string(),
                "qwen2".to_string(),
                "mistral".to_string(),
            ],
            supported_quantizations: vec![
                "Q4_K_M".to_string(),
                "Q4_0".to_string(),
                "Q8_0".to_string(),
                "F16".to_string(),
            ],
            max_context_length: 32_768,
            supports_structured_tools: true,
            compute_backends: vec!["cpu".to_string()],
            cpu_isa_features: vec!["x86_64-baseline".to_string(), "avx2".to_string()],
        }
    }
}

/// Model metadata observed by probing a verified GGUF file in the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbedModelMetadata {
    pub format: String,
    pub architecture: String,
    pub model_name: String,
    pub quantization: String,
    pub license: String,
    pub context_length: u32,
    pub block_count: u32,
    pub head_count: u32,
    pub kv_head_count: u32,
    pub embedding_length: u32,
    pub key_dim: u32,
    pub value_dim: u32,
    pub kv_elem_bytes: u32,
    pub has_chat_template: bool,
    pub template_digest: String,
    pub supports_tool_calling: bool,
    pub tensor_count: u64,
    pub total_tensor_bytes: u64,
    pub file_size_bytes: u64,
}

/// Closed-schema arguments for the MVP `read_file` tool (`docs/AGENT_RUNTIME.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadFileArgs {
    pub root_id: String,
    pub relative_path: String,
}

/// Closed-schema structured tool proposal emitted by the model/worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolProposalWire {
    pub tool_id: String,
    pub tool_version: u32,
    pub arguments: ReadFileArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    MaxTokens,
    ToolCall,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GenerationUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub prefill_ms: u64,
    pub decode_ms: u64,
    pub peak_rss_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerErrorCode {
    IncompatibleInput,
    InsufficientResources,
    InvalidWireVersion,
    CorruptArtifact,
    Timeout,
    Cancelled,
    UnsupportedCapability,
    OverContextLimit,
    InvalidState,
    InternalError,
}

/// Requests sent from the Core host to the isolated native worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostRequest {
    Hello {
        versions: Vec<ProtocolVersion>,
        nonce: String,
    },
    ProbeArtifact {
        request_id: String,
        artifact_id: String,
        artifact_path: String,
        expected_sha256: String,
    },
    LoadModel {
        request_id: String,
        artifact_id: String,
        artifact_path: String,
        expected_sha256: String,
        context_length: u32,
        threads: u32,
    },
    UnloadModel {
        request_id: String,
        model_handle: String,
    },
    Generate {
        request_id: String,
        session_id: String,
        model_handle: String,
        prompt: String,
        max_tokens: u32,
        context_length: u32,
        seed: u64,
        tool_calling_enabled: bool,
        #[serde(default)]
        simulated_delay_ms: u64,
    },
    Cancel {
        request_id: String,
        target_request_id: String,
    },
    Ping {
        request_id: String,
    },
    Shutdown {
        request_id: String,
    },
}

/// Replies and stream events sent from the isolated native worker to the Core host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerEvent {
    HelloAck {
        selected_version: ProtocolVersion,
        nonce: String,
        engine_name: String,
        engine_revision: String,
        build_options: Vec<String>,
        capabilities: WorkerCapabilities,
    },
    ProbeResult {
        request_id: String,
        metadata: ProbedModelMetadata,
    },
    ModelLoaded {
        request_id: String,
        model_handle: String,
        artifact_id: String,
        resident_bytes: u64,
        kv_bytes_per_token: u64,
    },
    ModelUnloaded {
        request_id: String,
        model_handle: String,
    },
    GenerationDelta {
        request_id: String,
        sequence: u64,
        text_delta: String,
        token_count: u32,
    },
    GenerationComplete {
        request_id: String,
        sequence: u64,
        finish_reason: FinishReason,
        usage: GenerationUsage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_proposal: Option<ToolProposalWire>,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        code: WorkerErrorCode,
        message: String,
        retryable: bool,
    },
    Pong {
        request_id: String,
        rss_bytes: u64,
    },
    ShutdownAck {
        request_id: String,
    },
}

pub fn write_host_request<W: Write>(
    writer: &mut W,
    req: &HostRequest,
) -> Result<(), ProtocolError> {
    let json =
        serde_json::to_string(req).map_err(|e| ProtocolError::InvalidSchema(e.to_string()))?;
    write_frame_str(writer, &json)?;
    Ok(())
}

pub fn read_host_request<R: Read>(reader: &mut R) -> Result<Option<HostRequest>, ProtocolError> {
    let Some(raw) = read_frame_str(reader)? else {
        return Ok(None);
    };
    let req: HostRequest =
        serde_json::from_str(&raw).map_err(|e| ProtocolError::InvalidSchema(e.to_string()))?;
    Ok(Some(req))
}

pub fn write_worker_event<W: Write>(writer: &mut W, ev: &WorkerEvent) -> Result<(), ProtocolError> {
    let json =
        serde_json::to_string(ev).map_err(|e| ProtocolError::InvalidSchema(e.to_string()))?;
    write_frame_str(writer, &json)?;
    Ok(())
}

pub fn read_worker_event<R: Read>(reader: &mut R) -> Result<Option<WorkerEvent>, ProtocolError> {
    let Some(raw) = read_frame_str(reader)? else {
        return Ok(None);
    };
    let ev: WorkerEvent =
        serde_json::from_str(&raw).map_err(|e| ProtocolError::InvalidSchema(e.to_string()))?;
    Ok(Some(ev))
}
