//! Ratatui render pipeline.
//!
//! Walks the local node-graph mirror once per frame and paints the
//! chat pane, input box, and status bar. Rendering is read-only —
//! interactive state transitions happen in [`crate::events`].

use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

use crate::app::{App, Focus, Route, SessionStatus};

/// Top-level render dispatcher — draws the screen for the current route.
pub(crate) fn routes(frame: &mut Frame, app: &mut App) {
    match &app.route {
        Route::Connecting { tick } => draw_connecting(frame, *tick),
        Route::Chat => draw_chat(frame, app),
    }
}

/// Renders the "Connecting to agent core..." splash screen.
fn draw_connecting(frame: &mut Frame, tick: u64) {
    let dots = match (tick / 3) % 4 {
        1 => ".",
        2 => "..",
        3 => "...",
        _ => "",
    };
    let text = format!("Connecting to agent core{dots}");
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Center);

    let [_, center, _] = Layout::vertical([
        Constraint::Percentage(45),
        Constraint::Length(1),
        Constraint::Percentage(45),
    ])
    .areas(frame.area());

    frame.render_widget(paragraph, center);
}

/// Column width reserved for the `> ` input prompt prefix.
const INPUT_PREFIX_WIDTH: u16 = 2;

/// Calculates how many rows the input area needs based on wrapped line count.
fn compute_input_height(app: &crate::input::InputState, available_width: u16, max_height: u16) -> u16 {
    if available_width == 0 {
        return 1;
    }
    let input_col_width = available_width.saturating_sub(INPUT_PREFIX_WIDTH).max(1);
    let line_count = app.visual_lines(input_col_width).len() as u16;
    line_count.max(1).min(max_height)
}

/// Lays out and draws the chat screen: history, input, and status bar.
fn draw_chat(frame: &mut Frame, app: &mut App) {
    let total_height = frame.area().height;
    let max_input_lines = (total_height as u32 * 30 / 100).max(1) as u16;
    let input_inner_width = frame.area().width.saturating_sub(2 + 1);
    let input_lines = compute_input_height(&app.input_state, input_inner_width, max_input_lines);

    let paused = app.session_status == SessionStatus::Paused;
    let paused_hint_rows: u16 = if paused { 1 } else { 0 };

    let [history_area, _, hint_area, input_area, _, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(paused_hint_rows),
        Constraint::Length(input_lines),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_history(frame, app, history_area);
    if paused {
        draw_paused_hint(frame, hint_area);
    }
    draw_input(frame, app, input_area);
    draw_status_bar(frame, app, status_area);
}

/// Renders the "Press Esc to resume" banner shown above the input while
/// the session is in `SessionStatus::Paused`. Typing remains functional
/// (B.4: prompts enqueue under Paused but do not dispatch until the user
/// presses Esc to recover).
fn draw_paused_hint(frame: &mut Frame, area: ratatui::layout::Rect) {
    let hint = Paragraph::new(Line::styled(
        "  Press Esc to resume",
        Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(hint, area);
}

/// Renders the scrollable chat message history with auto-follow and scrollbar.
fn draw_history(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let block = Block::default();
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.history_viewport_height = inner.height;

    let top_level: Vec<scarllet_proto::proto::Node> = app.top_level_nodes().cloned().collect();
    if top_level.is_empty() {
        let welcome = Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                "Welcome to Scarllet. Type a message to begin.",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(welcome, inner);
        return;
    }

    use crate::widgets::ScrollItem;

    let descendants_per_node: Vec<Vec<scarllet_proto::proto::Node>> = top_level
        .iter()
        .map(|n| app.descendants_of(&n.id).into_iter().cloned().collect())
        .collect();
    let expanded_tools = app.expanded_tools.clone();

    let debug_enabled = app.debug_enabled;
    let tick = app.tick;
    // Snapshot the reveal map up front so we don't borrow `app` again
    // inside the widget builder loop.
    let reveal_budgets: Vec<Option<usize>> = top_level
        .iter()
        .map(|n| {
            let is_agent = n.kind == scarllet_proto::proto::NodeKind::Agent as i32;
            if is_agent {
                Some(app.reveal_for(&n.id).visible_chars)
            } else {
                None
            }
        })
        .collect();
    let msg_widgets: Vec<crate::widgets::ChatMessageWidget> = top_level
        .iter()
        .zip(descendants_per_node.iter())
        .zip(reveal_budgets.iter())
        .enumerate()
        .map(|(i, ((node, descendants), budget))| {
            let descendant_refs: Vec<&scarllet_proto::proto::Node> = descendants.iter().collect();
            crate::widgets::ChatMessageWidget::new(
                node,
                &descendant_refs,
                &expanded_tools,
                app.focused_message_idx == Some(i),
                inner.width,
                debug_enabled,
                *budget,
                tick,
            )
        })
        .collect();

    if let Some(idx) = app.focused_message_idx {
        let gap = 1u16;
        let mut y = 0u16;
        for (i, w) in msg_widgets.iter().enumerate() {
            if i > 0 {
                y = y.saturating_add(gap);
            }
            if i == idx {
                app.scroll_view_state
                    .ensure_visible(y, w.height(), inner.height);
                break;
            }
            y = y.saturating_add(w.height());
        }
    }

    let items: Vec<&dyn crate::widgets::ScrollItem> = msg_widgets
        .iter()
        .map(|w| w as &dyn crate::widgets::ScrollItem)
        .collect();

    crate::widgets::ScrollView::render(
        inner,
        frame.buffer_mut(),
        &mut app.scroll_view_state,
        &items,
        1,
    );

    if app.scroll_view_state.content_height > inner.height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("┃")
            .track_symbol(Some("│"))
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(Color::DarkGray))
            .track_style(Style::default().fg(Color::Rgb(40, 40, 40)));

        let max_offset = app
            .scroll_view_state
            .content_height
            .saturating_sub(inner.height) as usize;
        let position = app.scroll_view_state.offset_y as usize;
        let mut scrollbar_state = ScrollbarState::new(max_offset).position(position);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

/// Renders the multi-line input editor with cursor, selection highlights, and scrollbar.
fn draw_input(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let block = Block::default().padding(Padding::left(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [prefix_area, input_col] =
        Layout::horizontal([Constraint::Length(INPUT_PREFIX_WIDTH), Constraint::Min(0)])
            .areas(inner);

    if app.input_locked {
        let paragraph = Paragraph::new(Line::styled(
            "Waiting for agent...",
            Style::default().fg(Color::DarkGray),
        ));
        frame.render_widget(paragraph, input_col);
        return;
    }

    let prefix = Paragraph::new(Line::styled("> ", Style::default().fg(Color::White)));
    frame.render_widget(prefix, prefix_area);

    let w = input_col.width.max(1);
    app.wrap_width = w;

    let visual_lines = app.input_state.visual_lines(w);
    let visible_rows = input_col.height;

    let (_, cursor_row) = app.input_state.cursor_visual_position(w);
    if cursor_row < app.input_state.vertical_scroll {
        app.input_state.vertical_scroll = cursor_row;
    }
    if cursor_row >= app.input_state.vertical_scroll + visible_rows {
        app.input_state.vertical_scroll = cursor_row - visible_rows + 1;
    }

    let start = app.input_state.vertical_scroll as usize;
    let end = (start + visible_rows as usize).min(visual_lines.len());

    let mut display_lines: Vec<Line> = Vec::new();
    let sel_range = app.input_state.selection_range();

    for vl in &visual_lines[start..end] {
        let line_text = &app.input_state.text()[vl.byte_start..vl.byte_end];

        if let Some((sel_start, sel_end)) = sel_range {
            let mut spans = Vec::new();

            for (g_idx, g) in
                unicode_segmentation::UnicodeSegmentation::grapheme_indices(line_text, true)
            {
                let absolute_byte = vl.byte_start + g_idx;
                let is_selected = absolute_byte >= sel_start && absolute_byte < sel_end;
                let style = if is_selected {
                    Style::default().bg(Color::Rgb(60, 60, 80))
                } else {
                    Style::default()
                };
                spans.push(Span::styled(g.to_string(), style));
            }
            display_lines.push(Line::from(spans));
        } else {
            display_lines.push(Line::raw(line_text.to_string()));
        }
    }

    let paragraph = Paragraph::new(display_lines);
    frame.render_widget(paragraph, input_col);

    if app.focus == Focus::Input {
        let (cursor_col, cursor_row) = app.input_state.cursor_visual_position(w);
        if cursor_row >= app.input_state.vertical_scroll
            && cursor_row < app.input_state.vertical_scroll + visible_rows
        {
            let screen_row = cursor_row - app.input_state.vertical_scroll;
            frame.set_cursor_position((input_col.x + cursor_col, input_col.y + screen_row));
        }
    }

    if visual_lines.len() as u16 > visible_rows {
        let scrollbar = ratatui::widgets::Scrollbar::default()
            .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        let mut state = ratatui::widgets::ScrollbarState::default()
            .content_length(visual_lines.len().saturating_sub(visible_rows as usize))
            .position(app.input_state.vertical_scroll as usize);
        frame.render_stateful_widget(scrollbar, input_col, &mut state);
    }
}

/// Formats the git branch and short SHA for the status bar.
fn format_git_segment(info: &crate::git_info::GitInfo) -> String {
    if info.detached {
        return format!("detached - {}", info.short_sha);
    }
    if let Some(ref branch) = info.branch {
        return format!("{branch} ({}/HEAD)", info.short_sha);
    }
    info.short_sha.clone()
}

/// Picks the lifecycle label + color shown in the status bar.
///
/// Priority (per effort 06):
/// - `PAUSED` (red) overrides everything else while the session is paused.
/// - Otherwise, `READY` when no agent is running and the queue is empty.
/// - `THINKING` when a main agent is streaming and the queue is empty.
/// - `THINKING +N queued` when a turn is active with queued follow-ups.
/// - `+N queued` when nothing is running but prompts have been enqueued
///   (transient — normally the next turn dispatches immediately).
fn lifecycle_segment(app: &App) -> (String, Style) {
    if app.session_status == SessionStatus::Paused {
        return (
            "PAUSED".to_string(),
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        );
    }
    let streaming = app.is_streaming();
    let queued = app.queue_len;
    let label = match (streaming, queued) {
        (false, 0) => "READY".to_string(),
        (true, 0) => "THINKING".to_string(),
        (true, n) => format!("THINKING  +{n} queued"),
        (false, n) => format!("+{n} queued"),
    };
    let style = if streaming {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    (label, style)
}

/// Renders the bottom status bar with cwd, git info, lifecycle label,
/// optional token counter, provider snapshot, and session id.
///
/// Segment priority on narrow terminals (drop-first → kept-longest):
/// `provider` → `tokens` → `session` → `lifecycle`. Lifecycle stays even
/// at tiny widths so a PAUSED indicator never disappears.
fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let muted = Style::default().fg(Color::DarkGray);
    let sep = "  │  ";
    let width = area.width as usize;

    let left_cwd = format!(" {}", app.cwd_display);
    let left_git = app.git_info.as_ref().map(format_git_segment);

    let session_segment = match app.session_id.as_ref() {
        Some(id) => {
            let short = &id[..8.min(id.len())];
            format!("session {short}")
        }
        None => "no session".to_string(),
    };

    let (lifecycle_text, lifecycle_style) = lifecycle_segment(app);
    let token_segment = app
        .latest_token_usage()
        .map(|(total, window)| format!("tokens: {total}/{window}"));
    let provider_segment = app
        .provider_info
        .as_ref()
        .and_then(|p| p.display_label());

    let left_git_str = left_git
        .as_ref()
        .map(|g| format!("{sep}{g}"))
        .unwrap_or_default();
    let left_full = format!("{left_cwd}{left_git_str}");

    let gap = 2usize;
    let layout = pick_right_layout(
        &left_full,
        &left_cwd,
        provider_segment.as_deref(),
        token_segment.as_deref(),
        &lifecycle_text,
        &session_segment,
        sep,
        width,
        gap,
    );

    let left_len = layout.left_out.chars().count();
    let mut spans: Vec<Span> = vec![Span::styled(layout.left_out.clone(), muted)];
    let right_len = layout.right_len();

    if layout.has_right() && left_len + right_len <= width {
        let padding = width.saturating_sub(left_len + right_len);
        spans.push(Span::raw(" ".repeat(padding)));
        if layout.show_tokens {
            if let Some(tokens) = token_segment.as_ref() {
                spans.push(Span::styled(tokens.clone(), muted));
                spans.push(Span::styled(sep.to_string(), muted));
            }
        }
        if layout.show_provider {
            if let Some(provider) = provider_segment.as_ref() {
                spans.push(Span::styled(provider.clone(), muted));
                spans.push(Span::styled(sep.to_string(), muted));
            }
        }
        spans.push(Span::styled(lifecycle_text.clone(), lifecycle_style));
        if layout.show_session {
            spans.push(Span::styled(format!("{sep}{session_segment}"), muted));
        }
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}

/// Resolved layout decision from [`pick_right_layout`]. Each flag gates
/// exactly one right-hand segment; `left_out` is the already-truncated
/// left slab, and `right_plain_len` is the ready-to-measure right slab
/// length (in chars) used to compute the centre padding.
struct StatusBarLayout {
    left_out: String,
    show_provider: bool,
    show_tokens: bool,
    show_session: bool,
    has_lifecycle: bool,
    right_plain_len: usize,
}

impl StatusBarLayout {
    fn has_right(&self) -> bool {
        self.has_lifecycle
    }

    fn right_len(&self) -> usize {
        self.right_plain_len
    }
}

/// Pure layout decision for [`draw_status_bar`]. Tests can pin the
/// degradation cascade without instantiating a `Frame`. The algorithm
/// starts from the richest layout and strips segments in ascending
/// priority — provider first, then tokens, then session — always keeping
/// the lifecycle label so `PAUSED` / `THINKING` remain visible at every
/// width.
#[allow(clippy::too_many_arguments)]
fn pick_right_layout(
    left_full: &str,
    left_cwd: &str,
    provider: Option<&str>,
    tokens: Option<&str>,
    lifecycle: &str,
    session: &str,
    sep: &str,
    width: usize,
    gap: usize,
) -> StatusBarLayout {
    let compose = |show_provider: bool, show_tokens: bool, show_session: bool| -> String {
        let mut parts: Vec<String> = Vec::new();
        if show_tokens {
            if let Some(t) = tokens {
                parts.push(t.to_string());
            }
        }
        if show_provider {
            if let Some(p) = provider {
                parts.push(p.to_string());
            }
        }
        parts.push(lifecycle.to_string());
        if show_session {
            parts.push(session.to_string());
        }
        parts.join(sep)
    };

    let left_len = left_full.chars().count();
    let left_cwd_len = left_cwd.chars().count();

    let variants: [(bool, bool, bool); 4] = [
        (true, true, true),
        (false, true, true),
        (false, false, true),
        (false, false, false),
    ];

    for (show_provider, show_tokens, show_session) in variants {
        let want_provider = show_provider && provider.is_some();
        let want_tokens = show_tokens && tokens.is_some();
        let right = compose(want_provider, want_tokens, show_session);
        let right_len = right.chars().count();
        if left_len + gap + right_len <= width {
            return StatusBarLayout {
                left_out: left_full.to_string(),
                show_provider: want_provider,
                show_tokens: want_tokens,
                show_session,
                has_lifecycle: true,
                right_plain_len: right_len,
            };
        }
    }

    // Right column doesn't fit alongside the full left — fall back to
    // showing lifecycle only (possibly stripping the left column too).
    if lifecycle.chars().count() + gap <= width {
        return StatusBarLayout {
            left_out: String::new(),
            show_provider: false,
            show_tokens: false,
            show_session: false,
            has_lifecycle: true,
            right_plain_len: lifecycle.chars().count(),
        };
    }

    // Nothing on the right fits; render whatever slice of the left we can.
    if left_len <= width {
        return StatusBarLayout {
            left_out: left_full.to_string(),
            show_provider: false,
            show_tokens: false,
            show_session: false,
            has_lifecycle: false,
            right_plain_len: 0,
        };
    }
    if left_cwd_len <= width {
        return StatusBarLayout {
            left_out: left_cwd.to_string(),
            show_provider: false,
            show_tokens: false,
            show_session: false,
            has_lifecycle: false,
            right_plain_len: 0,
        };
    }
    let max = width.saturating_sub(1);
    let truncated: String = left_cwd.chars().take(max).collect();
    StatusBarLayout {
        left_out: format!("{truncated}…"),
        show_provider: false,
        show_tokens: false,
        show_session: false,
        has_lifecycle: false,
        right_plain_len: 0,
    }
}

#[cfg(test)]
mod tests;
