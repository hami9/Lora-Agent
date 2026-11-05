//! Wire protocol, length-prefixed framing, version negotiation, and bounded GGUF parser
//! for Lora Agent's isolated worker boundary.
//!
//! Kept independent of Tokio, SQLite, and front-end types (`docs/REPOSITORY_STRUCTURE.md`).

pub mod frame;
pub mod gguf;
pub mod messages;

pub use frame::{encode_frame_str, read_frame_str, write_frame_str, FrameError, MAX_FRAME_BYTES};
pub use gguf::{
    build_synthetic_gguf, parse_gguf_bytes, parse_gguf_file, parse_gguf_reader, sha256_file,
    sha256_reader, GgufError, ParsedGguf, SyntheticGgufSpec,
};
pub use messages::{
    negotiate_version, read_host_request, read_worker_event, validate_nonce, write_host_request,
    write_worker_event, FinishReason, GenerationUsage, HostRequest, ProbedModelMetadata,
    ProtocolError, ProtocolVersion, ReadFileArgs, ToolProposalWire, WorkerCapabilities,
    WorkerErrorCode, WorkerEvent, CURRENT_PROTOCOL_MINOR, GOLDEN_HELLO_JSON,
    PINNED_LLAMA_CPP_REVISION, SUPPORTED_PROTOCOL_MAJOR,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn golden_hello_matches_spec() {
        let parsed: HostRequest =
            serde_json::from_str(GOLDEN_HELLO_JSON).expect("golden hello must parse");
        match parsed {
            HostRequest::Hello { versions, nonce } => {
                assert_eq!(versions, vec![ProtocolVersion { major: 1, minor: 0 }]);
                assert_eq!(nonce, "afcb26831b679a7621eabc53e375cb3a");
                validate_nonce(&nonce).expect("golden nonce is valid 128-bit hex");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn empty_json_object_frame_is_accepted_by_framing_and_rejected_by_schema() {
        // From `docs/INFERENCE_ENGINE.md`:
        // "A test payload of `{}` is encoded as `00 00 00 02 7B 7D`: framing accepts it and
        // message-schema validation rejects it."
        let wire: [u8; 6] = [0x00, 0x00, 0x00, 0x02, 0x7B, 0x7D];
        let mut cursor = Cursor::new(&wire[..]);
        let raw = read_frame_str(&mut cursor)
            .expect("framing must accept 2-byte `{}`")
            .expect("not EOF");
        assert_eq!(raw, "{}");

        let mut cursor2 = Cursor::new(&wire[..]);
        let err = read_host_request(&mut cursor2).expect_err("schema must reject `{}`");
        assert!(matches!(err, ProtocolError::InvalidSchema(_)));
    }

    #[test]
    fn rejects_zero_and_oversized_frame_lengths() {
        let zero_len = [0x00, 0x00, 0x00, 0x00];
        let mut cur = Cursor::new(&zero_len[..]);
        assert_eq!(read_frame_str(&mut cur), Err(FrameError::InvalidLength(0)));

        let too_large = (MAX_FRAME_BYTES + 1).to_be_bytes();
        let mut cur2 = Cursor::new(&too_large[..]);
        assert_eq!(
            read_frame_str(&mut cur2),
            Err(FrameError::InvalidLength(MAX_FRAME_BYTES + 1))
        );
    }

    #[test]
    fn distinguishes_clean_eof_from_truncated_frame() {
        let empty: [u8; 0] = [];
        let mut cur = Cursor::new(&empty[..]);
        assert_eq!(read_frame_str(&mut cur), Ok(None));

        let partial_prefix = [0x00, 0x00];
        let mut cur2 = Cursor::new(&partial_prefix[..]);
        assert_eq!(read_frame_str(&mut cur2), Err(FrameError::TruncatedFrame));

        let partial_payload = [0x00, 0x00, 0x00, 0x05, b'a', b'b'];
        let mut cur3 = Cursor::new(&partial_payload[..]);
        assert_eq!(read_frame_str(&mut cur3), Err(FrameError::TruncatedFrame));
    }

    #[test]
    fn rejects_stale_and_newer_protocol_majors() {
        assert_eq!(
            negotiate_version(&[ProtocolVersion { major: 0, minor: 9 }]),
            Err(ProtocolError::IncompatibleMajor {
                offered: vec![0],
                required: 1
            })
        );
        assert_eq!(
            negotiate_version(&[ProtocolVersion { major: 2, minor: 0 }]),
            Err(ProtocolError::IncompatibleMajor {
                offered: vec![2],
                required: 1
            })
        );
        assert_eq!(
            negotiate_version(&[
                ProtocolVersion { major: 2, minor: 0 },
                ProtocolVersion { major: 1, minor: 3 }
            ]),
            Ok(ProtocolVersion { major: 1, minor: 0 })
        );
    }

    #[test]
    fn closed_tool_proposal_rejects_unknown_fields() {
        let valid = r#"{"tool_id":"read_file","tool_version":1,"arguments":{"root_id":"root-1","relative_path":"docs/guide.md"}}"#;
        let parsed: ToolProposalWire = serde_json::from_str(valid).expect("valid tool proposal");
        assert_eq!(parsed.tool_id, "read_file");

        let forged_extra_field = r#"{"tool_id":"read_file","tool_version":1,"arguments":{"root_id":"root-1","relative_path":"docs/guide.md","bypass_policy":true}}"#;
        assert!(serde_json::from_str::<ToolProposalWire>(forged_extra_field).is_err());
    }

    #[test]
    fn gguf_parser_validates_bounds_and_rejects_malformed_fixtures() {
        let good_bytes = build_synthetic_gguf(&SyntheticGgufSpec::default());
        let parsed = parse_gguf_bytes(&good_bytes).expect("default synthetic GGUF must parse");
        assert_eq!(parsed.metadata.architecture, "llama");
        assert!(parsed.metadata.has_chat_template);
        assert!(parsed.metadata.supports_tool_calling);

        // Truncated GGUF
        let truncated = &good_bytes[..64];
        assert!(parse_gguf_bytes(truncated).is_err());

        // Out-of-range tensor offset
        let bad_offset = build_synthetic_gguf(&SyntheticGgufSpec {
            tensor_offset_override: Some(999_999),
            ..SyntheticGgufSpec::default()
        });
        assert!(matches!(
            parse_gguf_bytes(&bad_offset),
            Err(GgufError::TensorOutOfBounds { .. })
        ));

        // Missing chat template
        let no_template = build_synthetic_gguf(&SyntheticGgufSpec {
            chat_template: None,
            ..SyntheticGgufSpec::default()
        });
        assert_eq!(
            parse_gguf_bytes(&no_template),
            Err(GgufError::MissingChatTemplate)
        );
    }
}
