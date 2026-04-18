//! Chat message widget.
//!
//! Renders one top-level chat entry — a `User` node, a session-level
//! `Error`, or an `Agent` node plus its entire subtree of `Thought` /
//! `Tool` / `Result` / `Debug` descendants. Implements AC-11.5
//! sub-agent collapsing and AC-6.2 debug-node filtering.
//!
//! Streaming `Agent` nodes reveal character-by-character through a
//! typewriter budget (`chars_budget`) passed down by the caller —
//! paced at `crate::app::TYPEWRITER_CHARS_PER_TICK` per 50 ms tick by
//! [`crate::app::App::advance_tick`] so long responses unveil at ~600
//! chars/sec instead of dumping the whole block at once. When the
//! Agent's status flips to `finished` / `failed` the budget is snapped
//! to the total by `advance_tick`, flushing any remaining backlog in
//! one frame.

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Widget, Wrap};

use scarllet_proto::proto::{
    node, AgentPayload, DebugPayload, ErrorPayload, Node, NodeKind, ResultPayload, ThoughtPayload,
    ToolPayload, UserPayload,
};

use super::scroll_view::ScrollItem;

/// Background color applied to the focused message in history.
const HIGHLIGHT_BG: Color = Color::Rgb(35, 35, 50);
/// Left padding inside each message bubble.
const PADDING_LEFT: u16 = 1;
/// Right padding inside each message bubble.
const PADDING_RIGHT: u16 = 1;

/// Pre-rendered widget for a single top-level node in the scroll view.
///
/// Owns its rendered [`Line`] list so the scroll view can measure and
/// paint each message without re-running the flatten pass per frame.
pub struct ChatMessageWidget {
    lines: Vec<Line<'static>>,
    focused: bool,
    cached_height: u16,
}

impl ChatMessageWidget {
    /// Builds the rendered lines for a top-level node and every node in
    /// its subtree (flat `descendants` list, creation order) then
    /// pre-computes the wrapped height.
    ///
    /// `debug_enabled` gates whether `Debug` nodes in the subtree render
    /// inline (AC-6.2). `TokenUsage` nodes never render inline — the
    /// status bar surfaces the latest one separately.
    ///
    /// `chars_budget` is the typewriter reveal cap owned by the
    /// caller (`App::reveal`) — `None` means "render everything"
    /// (used for non-Agent top-level nodes like `User` or session-level
    /// `Error`), `Some(n)` caps visible streaming content at `n` chars
    /// so the block unveils smoothly. `tick` drives the `Working…`
    /// banner's animated ellipsis while the Agent is still running.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        top_level: &Node,
        descendants: &[&Node],
        expanded_tools: &HashSet<String>,
        focused: bool,
        width: u16,
        debug_enabled: bool,
        chars_budget: Option<usize>,
        tick: u64,
    ) -> Self {
        let content_width = width.saturating_sub(PADDING_LEFT + PADDING_RIGHT);
        let lines = build_lines(
            top_level,
            descendants,
            expanded_tools,
            debug_enabled,
            chars_budget,
            tick,
        );
        let height = Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .line_count(content_width.max(1)) as u16;
        Self {
            lines,
            focused,
            cached_height: height,
        }
    }
}

impl ScrollItem for ChatMessageWidget {
    fn height(&self) -> u16 {
        self.cached_height
    }

    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if self.focused {
            buf.set_style(area, Style::default().bg(HIGHLIGHT_BG));
        }
        let pad_block = Block::default().padding(Padding::new(PADDING_LEFT, PADDING_RIGHT, 0, 0));
        let inner = pad_block.inner(area);
        let paragraph = Paragraph::new(self.lines.clone()).wrap(Wrap { trim: false });
        Widget::render(paragraph, inner, buf);
    }
}

/// Returns the immediate children of `parent_id` drawn from the flat
/// descendants list (preserves creation order).
fn immediate_children<'a>(descendants: &[&'a Node], parent_id: &str) -> Vec<&'a Node> {
    descendants
        .iter()
        .copied()
        .filter(|n| n.parent_id.as_deref() == Some(parent_id))
        .collect()
}

/// Foreground color used for every Error-node rendering (both top-level
/// banners and Agent-parented indented errors). Matches the palette used
/// by `lifecycle_segment` for `PAUSED` in effort 06 so the visual
/// language stays consistent.
const ERROR_FG: Color = Color::LightRed;

/// Builds the rendered lines for a top-level node + its full subtree.
///
/// `debug_enabled` decides whether `Debug` child nodes render inline or
/// are silently filtered out (AC-6.2). `TokenUsage` child nodes are
/// always filtered here; the status bar surfaces them separately.
///
/// `chars_budget` caps the visible streaming content across the Agent's
/// subtree — see module docs. `tick` drives the animated `Working …`
/// banner.
fn build_lines(
    top_level: &Node,
    descendants: &[&Node],
    expanded_tools: &HashSet<String>,
    debug_enabled: bool,
    chars_budget: Option<usize>,
    tick: u64,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let kind = NodeKind::try_from(top_level.kind).unwrap_or(NodeKind::Unspecified);

    match (kind, top_level.payload.as_ref()) {
        (NodeKind::User, Some(node::Payload::User(UserPayload { text, .. }))) => {
            lines.push(Line::from(Span::styled(
                "You: ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            for line in super::markdown::render_markdown(text).lines {
                lines.push(line);
            }
        }
        (NodeKind::Agent, Some(node::Payload::Agent(AgentPayload { agent_module, agent_id, status }))) => {
            let id_short = &agent_id[..8.min(agent_id.len())];
            let label = format!("{agent_module} ({id_short}): ");
            lines.push(Line::from(Span::styled(
                label,
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            )));
            let children = immediate_children(descendants, &top_level.id);
            let visible_children: Vec<&Node> = children
                .into_iter()
                .filter(|c| is_inline_visible(c, debug_enabled))
                .collect();
            let is_running = status == "running";
            // Clone budget into a mutable counter the per-child append
            // helpers decrement. `None` means "unbounded" — render
            // every block in full (used for fallback rendering such as
            // the snapshot case where the reveal map is missing).
            let mut budget = chars_budget;
            for child in &visible_children {
                if matches!(budget, Some(0)) {
                    break;
                }
                append_child_lines(
                    child,
                    descendants,
                    expanded_tools,
                    &mut lines,
                    0,
                    debug_enabled,
                    &mut budget,
                );
            }
            if is_running {
                // While the agent is still streaming, always show the
                // `Working…` banner. Matches the pre-refactor behaviour
                // — animated ellipsis cycling every 3 ticks so the user
                // sees a live "I'm still here" signal even when the
                // typewriter budget is saturated mid-stream.
                let dots = thinking_dots(tick);
                lines.push(Line::styled(
                    format!("Working (press ESC to stop){dots}"),
                    Style::default().fg(Color::Yellow),
                ));
            }
        }
        // Top-level session-level Error (AC-3.3). Rendered at the same
        // lane as User / Agent (no indent), bold so it stands out as a
        // banner above the conversation.
        (NodeKind::Error, Some(node::Payload::Error(ErrorPayload { message, source }))) => {
            lines.push(Line::styled(
                format!("⚠ Error ({source}): {message}"),
                Style::default().fg(ERROR_FG).add_modifier(Modifier::BOLD),
            ));
        }
        _ => {
            // Unrecognised top-level kind; render nothing.
        }
    }

    lines
}

/// Animated ellipsis for the `Working (press ESC to stop)` banner.
/// Cycles `"" → " ." → " .." → " ..."` every 3 ticks so the indicator
/// visibly "breathes" while the agent streams. Leading space is part
/// of the return value so an empty dots string doesn't leave a trailing
/// space on the banner line.
fn thinking_dots(tick: u64) -> &'static str {
    match (tick / 3) % 4 {
        1 => " .",
        2 => " ..",
        3 => " ...",
        _ => "",
    }
}

/// Byte index of the `max_chars`-th character in `s`, used to slice
/// raw content to the typewriter budget BEFORE handing it to the
/// markdown renderer. Slicing after markdown rendering would leave
/// half-formed tags / partial code fences visible; slicing raw chars
/// first keeps the structure well-formed at every frame.
fn byte_offset_for_chars(s: &str, max_chars: usize) -> usize {
    s.char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Takes up to `budget` characters from `text`, returning the `(bytes,
/// chars_taken)` pair and decrementing `budget` in place. When
/// `budget` is `None` (unbounded), returns the whole string.
fn take_chars<'a>(text: &'a str, budget: &mut Option<usize>) -> (&'a str, usize) {
    let total = text.chars().count();
    let take = match budget {
        Some(remaining) => (*remaining).min(total),
        None => total,
    };
    if let Some(remaining) = budget.as_mut() {
        *remaining -= take;
    }
    let end = byte_offset_for_chars(text, take);
    (&text[..end], take)
}

/// `true` when `node` should appear inline in the chat body given the
/// current `debug_enabled` flag. `Debug` nodes are hidden unless the flag
/// is on; `TokenUsage` nodes are always hidden (status bar surfaces the
/// latest). This is a pure function so the effort 07 render-filter test
/// can pin the contract.
pub fn is_inline_visible(node: &Node, debug_enabled: bool) -> bool {
    match NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified) {
        NodeKind::Debug => debug_enabled,
        NodeKind::TokenUsage => false,
        _ => true,
    }
}

/// Appends the lines for one Agent's child node. Indentation is
/// parameterised so sub-agent subtrees render nested under their spawn
/// Tool node (AC-11.5). `debug_enabled` controls whether Debug nodes
/// encountered in the subtree render inline (AC-6.2).
///
/// `budget` is the shared typewriter cap decremented by content-bearing
/// blocks (Thought / Result / Error / Debug). Tool headers and nested
/// sub-agent header lines do not consume budget — they render instantly
/// once reached (matches the pre-refactor `ToolCallRef` treatment). A
/// value of `Some(0)` at entry signals "stop before emitting any more
/// content-bearing blocks"; the function returns early in that case.
fn append_child_lines(
    child: &Node,
    descendants: &[&Node],
    expanded_tools: &HashSet<String>,
    lines: &mut Vec<Line<'static>>,
    depth: u16,
    debug_enabled: bool,
    budget: &mut Option<usize>,
) {
    let indent = "  ".repeat(depth as usize);
    let kind = NodeKind::try_from(child.kind).unwrap_or(NodeKind::Unspecified);
    match (kind, child.payload.as_ref()) {
        (NodeKind::Thought, Some(node::Payload::Thought(ThoughtPayload { content }))) => {
            if matches!(budget, Some(0)) {
                return;
            }
            let (visible, _) = take_chars(content, budget);
            append_thought_lines(visible, lines, &indent);
        }
        (NodeKind::Tool, Some(node::Payload::Tool(payload))) => {
            if payload.tool_name == "spawn_sub_agent" {
                append_spawn_sub_agent_lines(
                    child,
                    payload,
                    descendants,
                    expanded_tools,
                    lines,
                    depth,
                    debug_enabled,
                    budget,
                );
            } else {
                append_tool_lines(payload, lines, &indent);
            }
        }
        (NodeKind::Result, Some(node::Payload::Result(ResultPayload { content, .. }))) => {
            if matches!(budget, Some(0)) {
                return;
            }
            let (visible, _) = take_chars(content, budget);
            append_result_lines(visible, lines, &indent);
        }
        // Per-turn (Agent-parented) Error node (AC-3.4). Indented under
        // the Agent card so it's visually attached to the failing turn,
        // red foreground consistent with the top-level banner.
        (NodeKind::Error, Some(node::Payload::Error(ErrorPayload { message, .. }))) => {
            if matches!(budget, Some(0)) {
                return;
            }
            let (visible, _) = take_chars(message, budget);
            lines.push(Line::styled(
                format!("{indent}✗ Error: {visible}"),
                Style::default().fg(ERROR_FG).add_modifier(Modifier::BOLD),
            ));
        }
        (NodeKind::Debug, Some(node::Payload::Debug(DebugPayload { level, message, .. }))) => {
            if !debug_enabled {
                return;
            }
            if matches!(budget, Some(0)) {
                return;
            }
            let (visible, _) = take_chars(message, budget);
            lines.push(Line::styled(
                format!("{indent}[debug {level}] {visible}"),
                Style::default().fg(Color::Magenta),
            ));
        }
        (NodeKind::Agent, Some(node::Payload::Agent(AgentPayload { agent_module, agent_id, status }))) => {
            // A nested Agent node means a sub-agent running under some
            // Tool. Render its header + its own children recursively
            // sharing the same budget so the reveal paces smoothly
            // across the whole subtree.
            let id_short = &agent_id[..8.min(agent_id.len())];
            lines.push(Line::styled(
                format!("{indent}↳ sub-agent {agent_module} ({id_short}) [{status}]"),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
            let sub_children = immediate_children(descendants, &child.id);
            for sc in sub_children {
                if !is_inline_visible(sc, debug_enabled) {
                    continue;
                }
                if matches!(budget, Some(0)) {
                    break;
                }
                append_child_lines(
                    sc,
                    descendants,
                    expanded_tools,
                    lines,
                    depth + 1,
                    debug_enabled,
                    budget,
                );
            }
        }
        _ => {
            // TokenUsage (and any future kinds) are filtered at the
            // `is_inline_visible` gate in `build_lines`; fall-through
            // here handles unknown payloads defensively.
        }
    }
}

/// Maximum lines pulled from the sub-agent subtree for the truncated
/// running view (AC-11.5: last N lines + spinner while the sub-agent is
/// still streaming).
const SPAWN_SUB_AGENT_PREVIEW_LINES: usize = 3;

/// Renders a `spawn_sub_agent` Tool node per AC-11.5:
/// - `pending` / `running`: compact card + last N lines of the subtree +
///   a spinner prompt.
/// - `done` / `failed` (not expanded): single summary line with the
///   result excerpt from `tool_result_json.content`.
/// - Any status with `expand` toggled on: full nested subtree.
///
/// `budget` is the shared typewriter cap; the expanded subtree path
/// propagates it so streaming sub-agent content still reveals smoothly.
/// The collapsed preview path does not consume budget — the preview
/// lines themselves are a lossy snapshot of already-revealed content.
#[allow(clippy::too_many_arguments)]
fn append_spawn_sub_agent_lines(
    tool_node: &Node,
    payload: &ToolPayload,
    descendants: &[&Node],
    expanded_tools: &HashSet<String>,
    lines: &mut Vec<Line<'static>>,
    depth: u16,
    debug_enabled: bool,
    budget: &mut Option<usize>,
) {
    let indent = "  ".repeat(depth as usize);
    let module = spawn_sub_agent_module_from_arguments(&payload.arguments_json);
    let is_terminal = matches!(payload.status.as_str(), "done" | "failed");
    let is_expanded = expanded_tools.contains(&tool_node.id);
    let status_color = tool_status_color(&payload.status);
    let marker = if is_expanded { "[-]" } else { "[+]" };

    if !is_terminal {
        lines.push(Line::styled(
            format!(
                "{indent}{marker} spawn_sub_agent('{module}') [{}]",
                payload.status
            ),
            Style::default().fg(status_color).add_modifier(Modifier::BOLD),
        ));
        if is_expanded {
            append_spawn_sub_agent_subtree(
                tool_node,
                descendants,
                expanded_tools,
                lines,
                depth + 1,
                debug_enabled,
                budget,
            );
        } else {
            let preview = collect_subtree_preview_lines(tool_node, descendants, SPAWN_SUB_AGENT_PREVIEW_LINES);
            for line in preview {
                lines.push(Line::styled(
                    format!("{indent}  … {line}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::styled(
                format!("{indent}  spinner · press Enter to expand"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            ));
        }
        return;
    }

    // Terminal status: `done` or `failed`.
    let summary = result_summary_from_tool(payload);
    lines.push(Line::styled(
        format!(
            "{indent}{marker} spawn_sub_agent('{module}') [{status} in {ms}ms] → {summary}",
            status = payload.status,
            ms = payload.duration_ms,
        ),
        Style::default().fg(status_color).add_modifier(Modifier::BOLD),
    ));

    if is_expanded {
        append_spawn_sub_agent_subtree(
            tool_node,
            descendants,
            expanded_tools,
            lines,
            depth + 1,
            debug_enabled,
            budget,
        );
    }
}

/// Renders the full nested subtree of a `spawn_sub_agent` Tool node when
/// the user has toggled expand for it. Delegates to `append_child_lines`
/// for each immediate child of the Tool (typically a single nested Agent
/// node), propagating the `debug_enabled` flag so nested Debug nodes
/// honour the same filter.
fn append_spawn_sub_agent_subtree(
    tool_node: &Node,
    descendants: &[&Node],
    expanded_tools: &HashSet<String>,
    lines: &mut Vec<Line<'static>>,
    depth: u16,
    debug_enabled: bool,
    budget: &mut Option<usize>,
) {
    for child in immediate_children(descendants, &tool_node.id) {
        if !is_inline_visible(child, debug_enabled) {
            continue;
        }
        if matches!(budget, Some(0)) {
            break;
        }
        append_child_lines(
            child,
            descendants,
            expanded_tools,
            lines,
            depth,
            debug_enabled,
            budget,
        );
    }
}

/// Gathers up to `max` lines from every Thought / Result / Tool-header in
/// the sub-agent's subtree and returns only the final `max` (so the user
/// sees the latest activity while the sub-agent streams).
fn collect_subtree_preview_lines(
    tool_node: &Node,
    descendants: &[&Node],
    max: usize,
) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    collect_subtree_preview_recursive(tool_node, descendants, &mut all);
    if all.len() <= max {
        return all;
    }
    all.split_off(all.len() - max)
}

/// Depth-first walk that appends one preview string per meaningful node
/// in the subtree. Used by [`collect_subtree_preview_lines`].
fn collect_subtree_preview_recursive(
    parent: &Node,
    descendants: &[&Node],
    out: &mut Vec<String>,
) {
    for child in immediate_children(descendants, &parent.id) {
        let kind = NodeKind::try_from(child.kind).unwrap_or(NodeKind::Unspecified);
        match (kind, child.payload.as_ref()) {
            (NodeKind::Thought, Some(node::Payload::Thought(ThoughtPayload { content }))) => {
                if let Some(last) = content.lines().last() {
                    if !last.trim().is_empty() {
                        out.push(last.to_string());
                    }
                }
            }
            (NodeKind::Tool, Some(node::Payload::Tool(payload))) => {
                out.push(format!("tool: {} [{}]", payload.tool_name, payload.status));
            }
            (NodeKind::Result, Some(node::Payload::Result(ResultPayload { content, .. }))) => {
                if let Some(first) = content.lines().next() {
                    if !first.trim().is_empty() {
                        out.push(format!("result: {first}"));
                    }
                }
            }
            (NodeKind::Agent, Some(node::Payload::Agent(AgentPayload { agent_module, .. }))) => {
                out.push(format!("sub-agent {agent_module} streaming"));
            }
            _ => {}
        }
        collect_subtree_preview_recursive(child, descendants, out);
    }
}

/// Extracts the `agent_module` value from the Tool node's
/// `arguments_json` so the header line can label the sub-agent concisely.
/// Falls back to `"?"` if the JSON is malformed or missing the key.
fn spawn_sub_agent_module_from_arguments(arguments_json: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(arguments_json) else {
        return "?".to_string();
    };
    parsed
        .get("agent_module")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string()
}

/// Builds the "→ content excerpt" summary used on the terminal header
/// when the sub-agent finished. Prefers `tool_result_json.content`;
/// falls back to raw JSON if the payload cannot be parsed.
fn result_summary_from_tool(payload: &ToolPayload) -> String {
    const EXCERPT_MAX: usize = 80;
    if payload.result_json.is_empty() {
        return "(no result)".to_string();
    }
    let content = match serde_json::from_str::<serde_json::Value>(&payload.result_json) {
        Ok(v) => v
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        Err(_) => payload.result_json.clone(),
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return "(empty)".to_string();
    }
    let first_line = trimmed.lines().next().unwrap_or_default();
    truncate(first_line, EXCERPT_MAX)
}

/// Truncates `s` to at most `max` chars, appending `…` when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

/// Maximum number of lines from `result_json` shown inline under a Tool
/// node. Truncated output ends with a `… (+N more lines)` line so the
/// user knows there is more behind the surface.
const TOOL_RESULT_PREVIEW_LINES: usize = 8;

/// Max characters rendered per result-preview line before horizontal
/// truncation (`…` appended). Long JSON strings / one-liners then wrap
/// into the footer-truncation indicator rather than overflowing the
/// chat pane width.
const TOOL_RESULT_PREVIEW_WIDTH: usize = 120;

/// Renders a (non-spawn_sub_agent) `Tool` payload as a header line +
/// truncated arguments preview + first [`TOOL_RESULT_PREVIEW_LINES`] of
/// result JSON. The result body is rendered in dim dark-gray so it
/// reads as a muted aside under the coloured header.
fn append_tool_lines(payload: &ToolPayload, lines: &mut Vec<Line<'static>>, indent: &str) {
    let status_color = tool_status_color(&payload.status);
    let header = format!(
        "{indent}{name} ({duration}ms) [{status}]",
        name = payload.tool_name,
        duration = payload.duration_ms,
        status = payload.status,
    );
    lines.push(Line::styled(
        header,
        Style::default().fg(status_color).add_modifier(Modifier::BOLD),
    ));

    if !payload.arguments_preview.is_empty() {
        lines.push(Line::styled(
            format!("{indent}    args: {}", payload.arguments_preview),
            Style::default().fg(Color::DarkGray),
        ));
    }

    if payload.result_json.is_empty() {
        return;
    }
    let dim = Style::default().fg(Color::DarkGray);
    let result_lines: Vec<&str> = payload.result_json.lines().collect();
    let take = result_lines.len().min(TOOL_RESULT_PREVIEW_LINES);
    for line in &result_lines[..take] {
        lines.push(Line::styled(
            format!("{indent}    {}", truncate_line(line, TOOL_RESULT_PREVIEW_WIDTH)),
            dim,
        ));
    }
    let remaining = result_lines.len().saturating_sub(take);
    if remaining > 0 {
        lines.push(Line::styled(
            format!("{indent}    … (+{remaining} more lines)"),
            dim,
        ));
    }
}

/// Truncates `line` to at most `max_chars` characters, appending `…`
/// when truncation happened. Uses `chars()` (not `len()`) so multi-byte
/// UTF-8 is handled safely.
fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let truncated: String = line.chars().take(max_chars).collect();
    format!("{truncated}…")
}

/// Picks the foreground color for a Tool node header based on its
/// lifecycle status string.
fn tool_status_color(status: &str) -> Color {
    match status {
        "running" | "pending" => Color::Yellow,
        "done" => Color::Green,
        "failed" => Color::LightRed,
        _ => Color::Gray,
    }
}

/// Renders a `Thought` payload as a dimmed block under its owning Agent
/// node. Content flows through the GFM-capable markdown renderer so
/// bold / lists / inline code / tables appear correctly in thought
/// streams. Every resulting line is forced to dark-gray so the block
/// reads as a muted aside. No prefix is added — the dim tone alone
/// conveys "this is a thought, not the final response".
fn append_thought_lines(content: &str, lines: &mut Vec<Line<'static>>, indent: &str) {
    let base_style = Style::default().fg(Color::DarkGray);
    if content.is_empty() {
        lines.push(Line::raw(indent.to_string()));
        return;
    }

    let rendered = super::markdown::render_markdown(content);
    for line in rendered.lines {
        lines.push(dim_line_with_indent(line, indent, base_style));
    }
}

/// Forces a dark-gray fg on every span of `line`, prepends a raw indent
/// span, and returns the composed line. Preserves span-level modifiers
/// (bold / inline code) so markdown structure stays visible inside the
/// muted block.
fn dim_line_with_indent(
    line: Line<'static>,
    indent: &str,
    base_style: Style,
) -> Line<'static> {
    let header = Span::styled(indent.to_string(), base_style);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
    spans.push(header);
    for mut span in line.spans {
        span.style = span.style.fg(Color::DarkGray);
        spans.push(span);
    }
    let mut out = Line::from(spans);
    out.style = base_style;
    out
}

/// Renders a `Result` payload's body through the markdown pipeline,
/// indenting each rendered line under its owning Agent. Empty content
/// keeps the blank indent slot so the typewriter has a landing row.
fn append_result_lines(content: &str, lines: &mut Vec<Line<'static>>, indent: &str) {
    if content.is_empty() {
        lines.push(Line::raw(indent.to_string()));
        return;
    }

    let rendered = super::markdown::render_markdown(content);
    for line in rendered.lines {
        lines.push(prepend_indent(line, indent));
    }
}

/// Prepends a raw-string indent span to `line`, preserving the
/// original span styling. Returns the composed line unchanged if
/// `indent` is empty.
fn prepend_indent(line: Line<'static>, indent: &str) -> Line<'static> {
    if indent.is_empty() {
        return line;
    }
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(indent.to_string()));
    spans.extend(line.spans);
    let mut out = Line::from(spans);
    out.style = line.style;
    out.alignment = line.alignment;
    out
}

#[cfg(test)]
mod tests;
