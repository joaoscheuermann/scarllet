use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Widget, Wrap};

use super::scroll_view::ScrollItem;
use crate::app::{ChatEntry, DisplayBlock, ToolCallData, ToolCallStatus};

/// Background color applied to the focused message in history.
const HIGHLIGHT_BG: Color = Color::Rgb(35, 35, 50);
/// Left padding inside each message bubble.
const PADDING_LEFT: u16 = 1;
/// Right padding inside each message bubble.
const PADDING_RIGHT: u16 = 1;

/// Pre-rendered widget for a single chat message in the scroll view.
///
/// Converts a `ChatEntry` into styled ratatui `Line`s and caches the
/// wrapped height so the scroll view can lay items out without re-measuring.
pub struct ChatMessageWidget<'a> {
    lines: Vec<Line<'a>>,
    focused: bool,
    cached_height: u16,
}

impl<'a> ChatMessageWidget<'a> {
    /// Builds the rendered lines for a chat entry and pre-computes the wrapped height.
    pub fn new(
        entry: &'a ChatEntry,
        focused: bool,
        tool_calls: &HashMap<String, ToolCallData>,
        tick: u64,
        width: u16,
    ) -> Self {
        let content_width = width.saturating_sub(PADDING_LEFT + PADDING_RIGHT);
        let lines = build_lines(entry, tool_calls, tick, content_width);
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

impl<'a> ScrollItem for ChatMessageWidget<'a> {
    /// Returns the pre-computed wrapped height.
    fn height(&self) -> u16 {
        self.cached_height
    }

    /// Paints the message lines into the buffer, applying focus highlight when selected.
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

/// Converts a `ChatEntry` into styled ratatui `Line`s for rendering.
///
/// Handles user messages, agent responses (with typewriter budget),
/// debug logs, and system notifications.
fn build_lines<'a>(
    entry: &'a ChatEntry,
    tool_calls: &HashMap<String, ToolCallData>,
    tick: u64,
    inner_width: u16,
) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();

    match entry {
        ChatEntry::User { text } => {
            lines.push(Line::from(Span::styled(
                "You: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            let md = super::markdown::render_markdown(text);
            for line in md.lines {
                lines.push(line);
            }
        }
        ChatEntry::Agent {
            name,
            task_id,
            blocks,
            visible_chars,
            done,
        } => {
            let id_short = &task_id[..8.min(task_id.len())];
            let label = format!("{name} ({id_short}): ");
            lines.push(Line::from(Span::styled(
                label,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::styled("", Style::default().fg(Color::DarkGray)));

            let mut chars_budget = *visible_chars;
            for (bi, blk) in blocks.iter().enumerate() {
                if bi > 0 {
                    lines.push(Line::raw(""));
                }
                match blk {
                    DisplayBlock::Thought(text) => {
                        if chars_budget == 0 {
                            break;
                        }
                        let char_count = text.chars().count();
                        let take = chars_budget.min(char_count);
                        let visible_end = byte_offset_for_chars(text, take);
                        let visible = &text[..visible_end];
                        chars_budget -= take;
                        let border = Span::styled("│ ", Style::default().fg(Color::DarkGray));
                        let content_w = inner_width.saturating_sub(2) as usize;
                        let md = super::markdown::render_markdown(visible);
                        let dimmed: Vec<Line> = md
                            .lines
                            .into_iter()
                            .map(|l| {
                                let spans: Vec<Span> =
                                    l.spans.into_iter().map(|s| s.dark_gray()).collect();
                                Line::from(spans)
                            })
                            .collect();
                        lines.extend(prepend_border(dimmed, border, content_w));
                    }
                    DisplayBlock::Text(text) => {
                        if chars_budget == 0 {
                            break;
                        }
                        let char_count = text.chars().count();
                        let take = chars_budget.min(char_count);
                        let visible_end = byte_offset_for_chars(text, take);
                        let visible = &text[..visible_end];
                        chars_budget -= take;
                        let md = super::markdown::render_markdown(visible);
                        for line in md.lines {
                            lines.push(line);
                        }
                    }
                    DisplayBlock::ToolCallRef(call_id) => {
                        if let Some(tc) = tool_calls.get(call_id) {
                            render_tool_call_lines(tc, &mut lines, inner_width);
                        }
                    }
                }
            }

            if !done {
                let dots = thinking_dots(tick);
                lines.push(Line::styled(
                    format!("Working (press ESC to stop) {dots}"),
                    Style::default().fg(Color::Yellow),
                ));
            }
        }
        ChatEntry::Debug {
            source,
            level,
            message,
            timestamp,
        } => {
            let level_style = match level.as_str() {
                "error" => Style::default().fg(Color::LightRed),
                "warn" => Style::default().fg(Color::Yellow),
                _ => Style::default().fg(Color::DarkGray),
            };
            let label = format!("{level} - {timestamp} [{source}]: ");
            lines.push(Line::from(vec![
                Span::styled(label, level_style),
                Span::styled(message.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
        }
        ChatEntry::System { text } => {
            lines.push(Line::styled(
                format!("System: {text}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    lines
}

/// Returns an animated dots string that cycles every few ticks.
fn thinking_dots(tick: u64) -> &'static str {
    match (tick / 3) % 4 {
        1 => ".",
        2 => "..",
        3 => "...",
        _ => "",
    }
}

/// Returns the byte index of the `max_chars`-th character in `s`.
fn byte_offset_for_chars(s: &str, max_chars: usize) -> usize {
    s.char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Appends bordered lines showing a tool call's name, result preview, and status.
fn render_tool_call_lines<'a>(tc: &ToolCallData, lines: &mut Vec<Line<'a>>, width: u16) {
    let border = Span::styled("│ ", Style::default().fg(Color::Rgb(100, 60, 140)));
    let content_w = width.saturating_sub(2) as usize;

    let title = format!("{} - {}", tc.tool_name, tc.arguments_preview);
    let title_style = Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::BOLD);
    let title_lines = vec![Line::from(Span::styled(title, title_style))];
    lines.extend(prepend_border(title_lines, border.clone(), content_w));

    if !tc.result.is_empty() {
        let first_line = tc.result.lines().next().unwrap_or("");
        let result_style = Style::default().fg(Color::DarkGray);
        let result_lines = vec![Line::from(Span::styled(
            first_line.to_string(),
            result_style,
        ))];
        lines.extend(prepend_border(result_lines, border.clone(), content_w));
    }

    let (status_text, status_style) = match tc.status {
        ToolCallStatus::Running => {
            let elapsed_secs = tc.started_at.elapsed().as_secs();
            (
                format!("running for {elapsed_secs}s..."),
                Style::default().fg(Color::Blue),
            )
        }
        ToolCallStatus::Done => {
            let secs = tc.duration_ms / 1000;
            (
                format!("ran for {secs}s."),
                Style::default().fg(Color::Rgb(0, 160, 0)),
            )
        }
        ToolCallStatus::Failed => {
            let secs = tc.duration_ms / 1000;
            (
                format!("failed after {secs}s."),
                Style::default().fg(Color::LightRed),
            )
        }
    };
    let status_lines = vec![Line::from(Span::styled(status_text, status_style))];
    lines.extend(prepend_border(status_lines, border, content_w));
}

/// Prepends a vertical border span to each line, wrapping long lines at `content_width`.
fn prepend_border<'a>(
    src_lines: Vec<Line<'a>>,
    border: Span<'a>,
    content_width: usize,
) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    for line in src_lines {
        let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let plain_chars: Vec<char> = plain.chars().collect();

        if plain_chars.is_empty() {
            out.push(Line::from(vec![border.clone()]));
            continue;
        }

        let style = if line.spans.len() == 1 {
            line.spans[0].style
        } else {
            Style::default()
        };

        if plain_chars.len() <= content_width {
            let mut spans = vec![border.clone()];
            spans.extend(line.spans);
            out.push(Line::from(spans));
        } else {
            for chunk in plain_chars.chunks(content_width) {
                let text: String = chunk.iter().collect();
                out.push(Line::from(vec![border.clone(), Span::styled(text, style)]));
            }
        }
    }
    out
}
