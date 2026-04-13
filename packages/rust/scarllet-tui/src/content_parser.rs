const OPEN_TAG: &str = "<thought>";
const CLOSE_TAG: &str = "</thought>";

#[derive(Debug, PartialEq)]
pub enum ContentBlock<'a> {
    Thought(&'a str),
    Response(&'a str),
}

pub fn parse_blocks(raw: &str) -> Vec<ContentBlock<'_>> {
    let mut blocks = Vec::new();
    let mut cursor = 0;

    while cursor < raw.len() {
        let haystack = &raw[cursor..];

        let Some(open_rel) = haystack.find(OPEN_TAG) else {
            let tail = haystack.trim();
            if !tail.is_empty() {
                blocks.push(ContentBlock::Response(tail));
            }
            break;
        };

        if open_rel > 0 {
            let before = haystack[..open_rel].trim();
            if !before.is_empty() {
                blocks.push(ContentBlock::Response(before));
            }
        }

        let content_start = cursor + open_rel + OPEN_TAG.len();

        let Some(close_rel) = raw[content_start..].find(CLOSE_TAG) else {
            let inner = raw[content_start..].trim();
            if !inner.is_empty() {
                blocks.push(ContentBlock::Thought(inner));
            }
            break;
        };

        let inner = raw[content_start..content_start + close_rel].trim();
        if !inner.is_empty() {
            blocks.push(ContentBlock::Thought(inner));
        }

        cursor = content_start + close_rel + CLOSE_TAG.len();
    }

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tags_single_response() {
        let blocks = parse_blocks("Hello **world**");
        assert_eq!(blocks, vec![ContentBlock::Response("Hello **world**")]);
    }

    #[test]
    fn single_thought_block() {
        let blocks = parse_blocks("<thought>thinking hard</thought>");
        assert_eq!(blocks, vec![ContentBlock::Thought("thinking hard")]);
    }

    #[test]
    fn thought_then_response() {
        let blocks = parse_blocks("<thought>hmm</thought>Here is the answer.");
        assert_eq!(
            blocks,
            vec![
                ContentBlock::Thought("hmm"),
                ContentBlock::Response("Here is the answer."),
            ]
        );
    }

    #[test]
    fn multiple_thoughts_then_response() {
        let input = "<thought>step 1</thought><thought>step 2</thought>Final answer.";
        let blocks = parse_blocks(input);
        assert_eq!(
            blocks,
            vec![
                ContentBlock::Thought("step 1"),
                ContentBlock::Thought("step 2"),
                ContentBlock::Response("Final answer."),
            ]
        );
    }

    #[test]
    fn interleaved_blocks() {
        let input = "Preamble<thought>think</thought>Middle<thought>more</thought>End";
        let blocks = parse_blocks(input);
        assert_eq!(
            blocks,
            vec![
                ContentBlock::Response("Preamble"),
                ContentBlock::Thought("think"),
                ContentBlock::Response("Middle"),
                ContentBlock::Thought("more"),
                ContentBlock::Response("End"),
            ]
        );
    }

    #[test]
    fn unclosed_thought_streaming() {
        let blocks = parse_blocks("<thought>partial reasoning");
        assert_eq!(blocks, vec![ContentBlock::Thought("partial reasoning")]);
    }

    #[test]
    fn unclosed_thought_after_complete_block() {
        let input = "<thought>done</thought><thought>still going";
        let blocks = parse_blocks(input);
        assert_eq!(
            blocks,
            vec![
                ContentBlock::Thought("done"),
                ContentBlock::Thought("still going"),
            ]
        );
    }

    #[test]
    fn empty_thought_tags_skipped() {
        let blocks = parse_blocks("<thought></thought>Answer");
        assert_eq!(blocks, vec![ContentBlock::Response("Answer")]);
    }

    #[test]
    fn whitespace_only_thought_skipped() {
        let blocks = parse_blocks("<thought>   </thought>Answer");
        assert_eq!(blocks, vec![ContentBlock::Response("Answer")]);
    }

    #[test]
    fn nested_angle_brackets_preserved() {
        let input = "<thought>Use the <div> tag here</thought>Done";
        let blocks = parse_blocks(input);
        assert_eq!(
            blocks,
            vec![
                ContentBlock::Thought("Use the <div> tag here"),
                ContentBlock::Response("Done"),
            ]
        );
    }

    #[test]
    fn empty_input() {
        let blocks = parse_blocks("");
        assert!(blocks.is_empty());
    }

    #[test]
    fn only_whitespace() {
        let blocks = parse_blocks("   \n  ");
        assert!(blocks.is_empty());
    }

    #[test]
    fn partial_closing_tag_mid_stream() {
        let blocks = parse_blocks("<thought>reasoning</tho");
        assert_eq!(blocks, vec![ContentBlock::Thought("reasoning</tho")]);
    }

    #[test]
    fn thought_with_markdown() {
        let input = "<thought>Consider **bold** and `code`</thought>";
        let blocks = parse_blocks(input);
        assert_eq!(
            blocks,
            vec![ContentBlock::Thought("Consider **bold** and `code`")]
        );
    }
}
