//! Engine-independent decode loop: UTF-8 assembly, tool-call detection, stop conditions.
//!
//! The native backend plugs llama.cpp in through [`TokenStream`]; tests use a scripted stream.

use crate::toolcall::{parse_tool_call_body, ToolStreamFilter, CLOSE_TAG, OPEN_TAG};
use crate::utf8::Utf8Accumulator;
use lora_protocol::{FinishReason, ToolProposalWire};
use std::time::Instant;

pub enum Piece {
    /// Raw bytes of one token (may be a partial UTF-8 sequence).
    Bytes(Vec<u8>),
    /// End-of-generation token sampled.
    Eog,
    /// The engine stopped because the abort flag was raised.
    Aborted,
}

pub trait TokenStream {
    fn next_piece(&mut self) -> Result<Piece, String>;
}

#[derive(Debug)]
pub struct LoopOutcome {
    pub finish_reason: FinishReason,
    pub tool_proposal: Option<ToolProposalWire>,
    pub completion_tokens: u32,
    pub decode_ms: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoopError {
    /// The engine reported a failure.
    Engine(String),
    /// The event sink could not write (host went away).
    SinkClosed,
}

/// Run decoding until a stop condition. `emit(text, token_count)` returns `false` if the sink broke.
pub fn run_decode_loop(
    src: &mut dyn TokenStream,
    max_tokens: u32,
    tool_mode: bool,
    aborted: &dyn Fn() -> bool,
    emit: &mut dyn FnMut(String, u32) -> bool,
) -> Result<LoopOutcome, LoopError> {
    let started = Instant::now();
    let mut acc = Utf8Accumulator::new();
    let mut filter = tool_mode.then(ToolStreamFilter::new);
    let mut completion = 0u32;
    let mut unreported = 0u32;
    let mut proposal = None;

    let mut send = |text: String, unreported: &mut u32| -> Result<(), LoopError> {
        if text.is_empty() {
            return Ok(());
        }
        let n = std::mem::take(unreported).max(1);
        if emit(text, n) {
            Ok(())
        } else {
            Err(LoopError::SinkClosed)
        }
    };

    let finish_reason = loop {
        if completion >= max_tokens {
            break FinishReason::MaxTokens;
        }
        if aborted() {
            break FinishReason::Cancelled;
        }
        match src.next_piece().map_err(LoopError::Engine)? {
            Piece::Aborted => break FinishReason::Cancelled,
            Piece::Eog => break FinishReason::Stop,
            Piece::Bytes(bytes) => {
                completion += 1;
                unreported += 1;
                let text = acc.push(&bytes);
                match filter.as_mut() {
                    None => send(text, &mut unreported)?,
                    Some(f) => {
                        let out = f.feed(&text);
                        send(out.emit, &mut unreported)?;
                        if let Some(body) = out.complete_body {
                            match parse_tool_call_body(&body) {
                                Ok(p) => {
                                    proposal = Some(p);
                                    break FinishReason::ToolCall;
                                }
                                Err(_) => {
                                    // Malformed or non-conforming call: show it as plain text.
                                    send(format!("{OPEN_TAG}{body}{CLOSE_TAG}"), &mut unreported)?;
                                    break FinishReason::Stop;
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    if finish_reason != FinishReason::Cancelled && finish_reason != FinishReason::ToolCall {
        let mut tail = acc.finish();
        if let Some(f) = filter.as_mut() {
            let out = f.feed(&tail);
            tail = out.emit;
            tail.push_str(&f.finish());
        }
        send(tail, &mut unreported)?;
    }

    Ok(LoopOutcome {
        finish_reason,
        tool_proposal: proposal,
        completion_tokens: completion,
        decode_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;

    struct Script(VecDeque<Piece>);
    impl TokenStream for Script {
        fn next_piece(&mut self) -> Result<Piece, String> {
            self.0.pop_front().ok_or_else(|| "script exhausted".into())
        }
    }
    fn bytes(s: &str) -> Piece {
        Piece::Bytes(s.as_bytes().to_vec())
    }

    fn run(
        pieces: Vec<Piece>,
        max: u32,
        tool: bool,
        abort_after: Option<u32>,
    ) -> (LoopOutcome, Vec<(String, u32)>) {
        let mut src = Script(pieces.into());
        let calls = Cell::new(0u32);
        let aborted = || {
            let c = calls.get();
            calls.set(c + 1);
            abort_after.is_some_and(|n| c >= n)
        };
        let mut deltas = Vec::new();
        let out = run_decode_loop(&mut src, max, tool, &aborted, &mut |t, n| {
            deltas.push((t, n));
            true
        })
        .unwrap();
        (out, deltas)
    }

    #[test]
    fn stops_on_eog() {
        let (o, d) = run(vec![bytes("Hel"), bytes("lo"), Piece::Eog], 10, false, None);
        assert_eq!(o.finish_reason, FinishReason::Stop);
        assert_eq!(o.completion_tokens, 2);
        assert_eq!(d, vec![("Hel".to_string(), 1), ("lo".to_string(), 1)]);
    }

    #[test]
    fn max_tokens_and_utf8_carry() {
        let euro = "€".as_bytes();
        let (o, d) = run(
            vec![
                Piece::Bytes(euro[..2].to_vec()),
                Piece::Bytes(euro[2..].to_vec()),
                bytes("x"),
            ],
            2,
            false,
            None,
        );
        assert_eq!(o.finish_reason, FinishReason::MaxTokens);
        // The first token produced no text, so its count rolls into the next delta.
        assert_eq!(d, vec![("€".to_string(), 2)]);
    }

    #[test]
    fn abort_flag_cancels() {
        let (o, d) = run(vec![bytes("a"), bytes("b"), bytes("c")], 10, false, Some(1));
        assert_eq!(o.finish_reason, FinishReason::Cancelled);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn engine_abort_cancels() {
        let (o, _) = run(vec![bytes("a"), Piece::Aborted], 10, false, None);
        assert_eq!(o.finish_reason, FinishReason::Cancelled);
    }

    #[test]
    fn tool_call_is_parsed_and_not_streamed() {
        let (o, d) = run(
            vec![
                bytes("<tool"),
                bytes("_call>{\"name\":\"read_file\",\"arguments\":"),
                bytes("{\"root_id\":\"ws1\",\"relative_path\":\"a.md\"}}"),
                bytes("</tool_call>"),
                bytes("never reached"),
            ],
            50,
            true,
            None,
        );
        assert_eq!(o.finish_reason, FinishReason::ToolCall);
        assert!(d.is_empty());
        let p = o.tool_proposal.unwrap();
        assert_eq!(p.arguments.root_id, "ws1");
        assert_eq!(o.completion_tokens, 4);
    }

    #[test]
    fn malformed_tool_call_finishes_with_stop_and_no_proposal() {
        let (o, d) = run(
            vec![bytes("<tool_call>{nope}</tool_call>"), bytes("later")],
            50,
            true,
            None,
        );
        assert_eq!(o.finish_reason, FinishReason::Stop);
        assert!(o.tool_proposal.is_none());
        assert_eq!(d.concat_text(), "<tool_call>{nope}</tool_call>");
    }

    #[test]
    fn tool_mode_prose_is_streamed_and_held_tag_prefix_flushed() {
        let (o, d) = run(
            vec![bytes("The file says hi <"), Piece::Eog],
            50,
            true,
            None,
        );
        assert_eq!(o.finish_reason, FinishReason::Stop);
        assert_eq!(d.concat_text(), "The file says hi <");
    }

    trait Concat {
        fn concat_text(&self) -> String;
    }
    impl Concat for Vec<(String, u32)> {
        fn concat_text(&self) -> String {
            self.iter().map(|(t, _)| t.as_str()).collect()
        }
    }
}
