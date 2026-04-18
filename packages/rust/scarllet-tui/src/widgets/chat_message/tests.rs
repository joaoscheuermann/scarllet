use super::*;
use scarllet_proto::proto::{DebugPayload, TokenUsagePayload};

fn debug_node(id: &str, parent: &str, message: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Debug as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Debug(DebugPayload {
            source: "default".into(),
            level: "info".into(),
            message: message.into(),
        })),
    }
}

fn token_usage_node(id: &str, parent: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::TokenUsage as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::TokenUsage(TokenUsagePayload {
            total_tokens: 42,
            context_window: 128,
        })),
    }
}

fn thought_node(id: &str, parent: &str, content: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Thought as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Thought(ThoughtPayload {
            content: content.into(),
        })),
    }
}

fn tool_node(id: &str, parent: &str, name: &str, status: &str, result_json: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Tool as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Tool(ToolPayload {
            tool_name: name.into(),
            arguments_preview: String::new(),
            arguments_json: "{}".into(),
            status: status.into(),
            duration_ms: 12,
            result_json: result_json.into(),
        })),
    }
}

#[test]
fn is_inline_visible_hides_debug_when_flag_off() {
    let n = debug_node("d1", "a1", "hello");
    assert!(!is_inline_visible(&n, false));
    assert!(is_inline_visible(&n, true));
}

#[test]
fn is_inline_visible_always_hides_token_usage() {
    let n = token_usage_node("u1", "a1");
    assert!(!is_inline_visible(&n, false));
    assert!(!is_inline_visible(&n, true));
}

#[test]
fn is_inline_visible_keeps_thought_regardless_of_flag() {
    let n = thought_node("t1", "a1", "hi");
    assert!(is_inline_visible(&n, false));
    assert!(is_inline_visible(&n, true));
}

fn agent_node(id: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: None,
        kind: NodeKind::Agent as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Agent(AgentPayload {
            agent_module: "default".into(),
            agent_id: id.into(),
            status: "running".into(),
        })),
    }
}

// End-to-end render-filter check exercising `build_lines`: the Debug
// node only appears in the rendered line stream when `debug_enabled`
// is true. Effort 07 pins this contract.
#[test]
fn build_lines_filters_debug_based_on_flag() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let debug = debug_node("d1", "a1", "Using provider: openrouter");
    let descendants: Vec<&Node> = vec![&debug];

    let lines_off = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined_off: String = lines_off
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(
        !joined_off.contains("Using provider"),
        "Debug nodes must be hidden when debug_enabled=false; got: {joined_off:?}"
    );

    let lines_on = build_lines(&agent, &descendants, &expanded, true, None, 0);
    let joined_on: String = lines_on
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(
        joined_on.contains("Using provider"),
        "Debug nodes must render when debug_enabled=true; got: {joined_on:?}"
    );
}

#[test]
fn build_lines_always_filters_token_usage_from_body() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let usage = token_usage_node("u1", "a1");
    let descendants: Vec<&Node> = vec![&usage];

    for flag in [false, true] {
        let lines = build_lines(&agent, &descendants, &expanded, flag, None, 0);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(
            !joined.contains("TokenUsage"),
            "TokenUsage payload type must never render inline; got: {joined:?}"
        );
        // Sanity — the status-bar path should still see the node
        // (app-level), but the chat body line stream must not mention
        // the numeric counters either.
        assert!(
            !joined.contains("42"),
            "TokenUsage total must never render inline; got: {joined:?}"
        );
    }
}

/// `true` when any span (or the line itself) on `line` carries the
/// supplied fg color.
fn has_fg(line: &Line<'static>, color: Color) -> bool {
    if line.style.fg == Some(color) {
        return true;
    }
    line.spans.iter().any(|s| s.style.fg == Some(color))
}

#[test]
fn build_lines_renders_top_level_error_as_banner() {
    let expanded = HashSet::new();
    let err = Node {
        id: "e1".into(),
        parent_id: None,
        kind: NodeKind::Error as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "core".into(),
            message: "default agent not registered".into(),
        })),
    };
    let lines = build_lines(&err, &[], &expanded, false, None, 0);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(joined.contains("default agent not registered"));
    assert!(joined.contains("⚠"));
    assert!(
        lines.iter().any(|l| has_fg(l, ERROR_FG)),
        "top-level Error banner must be rendered with ERROR_FG"
    );
    // Top-level (no parent) lanes start at column 0 — the first
    // character of the banner must not be an indent space.
    let banner = lines.first().expect("banner line present");
    let first_char = banner
        .spans
        .first()
        .and_then(|s| s.content.chars().next())
        .unwrap_or(' ');
    assert_ne!(first_char, ' ', "top-level Error must not be indented");
}

#[test]
fn build_lines_renders_agent_parented_error_indented() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let err = Node {
        id: "e1".into(),
        parent_id: Some("a1".into()),
        kind: NodeKind::Error as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "default".into(),
            message: "LLM error: bad api key".into(),
        })),
    };
    let descendants: Vec<&Node> = vec![&err];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(joined.contains("bad api key"));
    assert!(joined.contains("✗"));
    assert!(
        lines.iter().any(|l| has_fg(l, ERROR_FG)),
        "Agent-parented Error must be rendered with ERROR_FG"
    );
}

fn user_node(id: &str, text: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: None,
        kind: NodeKind::User as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::User(UserPayload {
            text: text.into(),
            working_directory: String::new(),
        })),
    }
}

fn result_node(id: &str, parent: &str, content: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Result as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Result(scarllet_proto::proto::ResultPayload {
            content: content.into(),
            finish_reason: "stop".into(),
        })),
    }
}

/// Joins every span content across `lines` into a single newline-separated
/// string so tests can pattern-match against the visible text regardless
/// of span granularity.
fn joined_text(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn user_text_passes_through_markdown_renderer() {
    let expanded = HashSet::new();
    let user = user_node("u1", "| A | B |\n|---|---|\n| 1 | 2 |");
    let lines = build_lines(&user, &[], &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains('┌') && joined.contains('└'),
        "user text with a GFM table must render box-drawing borders; got:\n{joined}"
    );
}

#[test]
fn result_content_passes_through_markdown_renderer() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let result =
        result_node("r1", "a1", "| A | B |\n|---|---|\n| x | y |\n\nafter table");
    let descendants: Vec<&Node> = vec![&result];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains('┌') && joined.contains('┐'),
        "agent Result containing a GFM table must render box-drawing top border; got:\n{joined}"
    );
    assert!(
        joined.contains("after table"),
        "text after a table must still render; got:\n{joined}"
    );
}

#[test]
fn thought_content_passes_through_markdown_renderer() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let thought = thought_node(
        "t1",
        "a1",
        "| H1 | H2 |\n|----|----|\n| v1 | v2 |",
    );
    let descendants: Vec<&Node> = vec![&thought];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains('┌') && joined.contains('└'),
        "thought content with a GFM table must render box-drawing borders; got:\n{joined}"
    );
    // Thought block uses dim tone only — no `thought:` prefix.
    assert!(
        !joined.contains("thought:"),
        "thought rendering must not carry a `thought:` prefix; got:\n{joined}"
    );
}

#[test]
fn tool_header_does_not_contain_wrench_emoji() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let tool = tool_node("t1", "a1", "tree", "done", "{}");
    let descendants: Vec<&Node> = vec![&tool];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        !joined.contains('🔧'),
        "tool header must not contain 🔧 emoji; got:\n{joined}"
    );
    assert!(
        joined.contains("tree (12ms) [done]"),
        "tool header must still render name / duration / status; got:\n{joined}"
    );
}

#[test]
fn spawn_sub_agent_header_does_not_contain_dna_emoji() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let spawn = Node {
        id: "sp1".into(),
        parent_id: Some("a1".into()),
        kind: NodeKind::Tool as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Tool(ToolPayload {
            tool_name: "spawn_sub_agent".into(),
            arguments_preview: String::new(),
            arguments_json: "{\"agent_module\":\"default\",\"prompt\":\"hi\"}".into(),
            status: "running".into(),
            duration_ms: 0,
            result_json: String::new(),
        })),
    };
    let descendants: Vec<&Node> = vec![&spawn];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        !joined.contains('🧬'),
        "spawn_sub_agent card must not contain 🧬 emoji; got:\n{joined}"
    );
    assert!(
        joined.contains("spawn_sub_agent('default')"),
        "spawn_sub_agent card must still label the module; got:\n{joined}"
    );
}

#[test]
fn tool_result_preview_truncates_wide_lines_with_ellipsis() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let long_line: String = "x".repeat(TOOL_RESULT_PREVIEW_WIDTH + 40);
    let tool = tool_node("t1", "a1", "grep", "done", &long_line);
    let descendants: Vec<&Node> = vec![&tool];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains('…'),
        "over-wide tool result line must be truncated with an ellipsis; got:\n{joined}"
    );
    let ellipsis_count = joined.matches('…').count();
    let long_occurrences = joined.matches(&"x".repeat(TOOL_RESULT_PREVIEW_WIDTH + 40)).count();
    assert_eq!(
        long_occurrences, 0,
        "full over-wide line must not survive verbatim; got:\n{joined}"
    );
    assert!(
        ellipsis_count >= 1,
        "expected at least one ellipsis marker; got count {ellipsis_count}"
    );
}

#[test]
fn tool_result_body_is_dim_dark_gray() {
    let expanded = HashSet::new();
    let agent = agent_node("a1");
    let tool = tool_node("t1", "a1", "tree", "done", "line one\nline two");
    let descendants: Vec<&Node> = vec![&tool];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let body_dark = lines.iter().any(|l| {
        let text: String = l.spans.iter().map(|s| s.content.to_string()).collect();
        if !text.contains("line one") {
            return false;
        }
        // Accept either a line-level DarkGray fg OR every span explicitly tagged.
        l.style.fg == Some(Color::DarkGray)
            || l.spans.iter().all(|s| s.style.fg == Some(Color::DarkGray))
    });
    assert!(
        body_dark,
        "tool result body must render in dim DarkGray; lines: {:?}",
        lines
    );
}

// ---- Typewriter tests ----------------------------------------

fn agent_node_with_status(id: &str, status: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: None,
        kind: NodeKind::Agent as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Agent(AgentPayload {
            agent_module: "default".into(),
            agent_id: id.into(),
            status: status.into(),
        })),
    }
}

#[test]
fn build_lines_banner_present_while_running() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "running");
    let thought = thought_node("t1", "a1", "hi there");
    let descendants: Vec<&Node> = vec![&thought];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains("Working (press ESC to stop)"),
        "running Agent must show Working banner; got:\n{joined}"
    );
}

#[test]
fn build_lines_banner_absent_when_finished() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "finished");
    let result = result_node("r1", "a1", "all done.");
    let descendants: Vec<&Node> = vec![&result];
    let lines = build_lines(&agent, &descendants, &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        !joined.contains("Working (press ESC to stop)"),
        "finished Agent must NOT show Working banner; got:\n{joined}"
    );
}

#[test]
fn build_lines_banner_absent_when_failed() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "failed");
    let lines = build_lines(&agent, &[], &expanded, false, None, 0);
    let joined = joined_text(&lines);
    assert!(
        !joined.contains("Working (press ESC to stop)"),
        "failed Agent must NOT show Working banner; got:\n{joined}"
    );
}

#[test]
fn build_lines_banner_dots_cycle_with_tick() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "running");
    let bare = |tick: u64| -> String {
        let lines = build_lines(&agent, &[], &expanded, false, None, tick);
        joined_text(&lines)
    };
    // tick/3 % 4 == 0 at ticks 0,1,2 → "" (no trailing dots).
    assert!(
        bare(0).contains("Working (press ESC to stop)")
            && !bare(0).contains("Working (press ESC to stop) ."),
        "tick 0 must show bare banner; got: {:?}",
        bare(0)
    );
    // tick=3 → "." suffix.
    assert!(
        bare(3).contains("Working (press ESC to stop) ."),
        "tick 3 must carry single-dot banner; got: {:?}",
        bare(3)
    );
    // tick=6 → ".." suffix.
    assert!(
        bare(6).contains("Working (press ESC to stop) .."),
        "tick 6 must carry two-dot banner; got: {:?}",
        bare(6)
    );
    // tick=9 → "..." suffix.
    assert!(
        banner_contains_three_dots(&bare(9)),
        "tick 9 must carry three-dot banner; got: {:?}",
        bare(9)
    );
}

fn banner_contains_three_dots(s: &str) -> bool {
    s.contains("Working (press ESC to stop) ...")
}

#[test]
fn build_lines_truncates_thought_when_budget_smaller_than_content() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "running");
    let thought = thought_node("t1", "a1", "abcdefghij"); // 10 chars
    let descendants: Vec<&Node> = vec![&thought];
    let lines = build_lines(&agent, &descendants, &expanded, false, Some(3), 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains("abc"),
        "first 3 chars of thought must render; got:\n{joined}"
    );
    assert!(
        !joined.contains("abcd"),
        "char #4 must NOT appear at budget=3; got:\n{joined}"
    );
}

#[test]
fn build_lines_truncates_result_when_budget_smaller_than_content() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "running");
    let result = result_node("r1", "a1", "0123456789");
    let descendants: Vec<&Node> = vec![&result];
    let lines = build_lines(&agent, &descendants, &expanded, false, Some(5), 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains("01234"),
        "first 5 chars of result must render; got:\n{joined}"
    );
    assert!(
        !joined.contains("012345"),
        "char #6 must NOT appear at budget=5; got:\n{joined}"
    );
}

#[test]
fn build_lines_renders_full_content_when_budget_exceeds_total() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "running");
    let thought = thought_node("t1", "a1", "tiny");
    let descendants: Vec<&Node> = vec![&thought];
    let lines = build_lines(&agent, &descendants, &expanded, false, Some(100), 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains("tiny"),
        "oversized budget must render full content; got:\n{joined}"
    );
}

#[test]
fn build_lines_stops_emitting_after_budget_hits_zero() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "running");
    let t1 = thought_node("t1", "a1", "aaaaa"); // 5 chars
    let t2 = thought_node("t2", "a1", "bbbbb"); // 5 chars
    let descendants: Vec<&Node> = vec![&t1, &t2];
    // Budget 5 exactly covers t1; t2 must not appear at all.
    let lines = build_lines(&agent, &descendants, &expanded, false, Some(5), 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains("aaaaa"),
        "t1 must fully render; got:\n{joined}"
    );
    assert!(
        !joined.contains("bbbbb"),
        "t2 must NOT render at budget=5 with 5-char t1 preceding; got:\n{joined}"
    );
}

#[test]
fn build_lines_zero_budget_hides_content_but_keeps_banner() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "running");
    let thought = thought_node("t1", "a1", "secret-text");
    let descendants: Vec<&Node> = vec![&thought];
    let lines = build_lines(&agent, &descendants, &expanded, false, Some(0), 0);
    let joined = joined_text(&lines);
    assert!(
        !joined.contains("secret-text"),
        "content must be hidden at budget=0; got:\n{joined}"
    );
    assert!(
        joined.contains("Working (press ESC to stop)"),
        "banner must persist at budget=0 while running; got:\n{joined}"
    );
}

#[test]
fn build_lines_slices_content_on_raw_chars_not_bytes() {
    let expanded = HashSet::new();
    let agent = agent_node_with_status("a1", "running");
    // Multi-byte chars — each UTF-8 scalar should count as 1 toward
    // the budget. Slicing on bytes would split a codepoint and panic
    // at the markdown boundary, or leak an ASCII-bias short read.
    let thought = thought_node("t1", "a1", "héllo✨world");
    let descendants: Vec<&Node> = vec![&thought];
    let lines = build_lines(&agent, &descendants, &expanded, false, Some(6), 0);
    let joined = joined_text(&lines);
    assert!(
        joined.contains("héllo✨"),
        "budget=6 must reveal 6 chars including multi-byte; got:\n{joined}"
    );
    assert!(
        !joined.contains("world"),
        "characters beyond budget must NOT render; got:\n{joined}"
    );
}
