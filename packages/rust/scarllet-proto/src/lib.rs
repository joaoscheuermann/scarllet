/// Generated gRPC/protobuf bindings for the Scarllet orchestrator protocol.
///
/// This module re-exports all types and service stubs produced by `tonic-prost-build`
/// from `proto/orchestrator.proto`, providing the wire format used between the
/// core daemon and its connected clients (TUI, agents, etc.).
pub mod proto {
    tonic::include_proto!("scarllet");
}

use proto::AgentBlock;

/// Concatenates the `text` blocks of an agent message into a single string.
///
/// `thought` and `tool_call_ref` blocks are intentionally dropped — this helper
/// is used to extract the user-facing assistant response for transcripts and
/// history entries, not the internal reasoning trace.
pub fn blocks_to_text(blocks: &[AgentBlock]) -> String {
    blocks
        .iter()
        .filter(|b| b.block_type == "text")
        .map(|b| b.content.as_str())
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(block_type: &str, content: &str) -> AgentBlock {
        AgentBlock {
            block_type: block_type.into(),
            content: content.into(),
        }
    }

    #[test]
    fn text_blocks_are_concatenated() {
        let blocks = vec![block("text", "Hello "), block("text", "world")];
        assert_eq!(blocks_to_text(&blocks), "Hello world");
    }

    #[test]
    fn non_text_blocks_are_ignored() {
        let blocks = vec![
            block("thought", "planning..."),
            block("text", "answer"),
            block("tool_call_ref", "call-123"),
        ];
        assert_eq!(blocks_to_text(&blocks), "answer");
    }

    #[test]
    fn empty_input_yields_empty_string() {
        assert_eq!(blocks_to_text(&[]), "");
    }
}
