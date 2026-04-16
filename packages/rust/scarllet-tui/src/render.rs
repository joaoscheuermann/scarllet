use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;

use crate::app::{App, Focus, Route};

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
    let max_input_lines = (total_height as u32 * 30 / 100).max(1) as u16; // 30% of screen as per spec
    let input_inner_width = frame.area().width.saturating_sub(2 + 1);
    let input_lines = compute_input_height(&app.input_state, input_inner_width, max_input_lines);

    let [history_area, _, input_area, _, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(input_lines),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_history(frame, app, history_area);
    draw_input(frame, app, input_area);
    draw_status_bar(frame, app, status_area);
}

/// Renders the scrollable chat message history with auto-follow and scrollbar.
fn draw_history(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let block = Block::default();

    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.history_viewport_height = inner.height;

    if app.messages.is_empty() {
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
    let msg_widgets: Vec<crate::widgets::ChatMessageWidget> = app
        .messages
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            crate::widgets::ChatMessageWidget::new(
                entry,
                app.focused_message_idx == Some(i),
                &app.tool_calls,
                app.tick,
                inner.width,
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

    for (_, vl) in visual_lines[start..end].iter().enumerate() {
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

/// Formats a number with `k` suffix for thousands (e.g. `12.5k`).
fn format_compact(n: u32) -> String {
    if n < 1_000 {
        return format!("{n}");
    }
    let k = n as f64 / 1_000.0;
    let decimal = (k.fract() * 10.0).round() as u32;
    if decimal == 0 {
        format!("{}k", k as u32)
    } else {
        format!("{:.1}k", k)
    }
}

/// Builds the `"used / limit tokens (pct%)"` string for the status bar.
fn format_token_budget(total: u32, window: u32) -> String {
    if window == 0 {
        return String::new();
    }
    let pct = (total as f64 / window as f64 * 100.0).round() as u32;
    format!(
        "{} / {} tokens ({}%)",
        format_compact(total),
        format_compact(window),
        pct
    )
}

/// Returns a color style that reflects context-window pressure (green → yellow → red).
fn token_budget_style(total: u32, window: u32) -> Style {
    if window == 0 {
        return Style::default().fg(Color::DarkGray);
    }
    let pct = (total as f64 / window as f64 * 100.0).round() as u32;
    if pct >= 95 {
        Style::default().fg(Color::LightRed)
    } else if pct >= 75 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Renders the bottom status bar with cwd, git info, token budget, and provider/model.
fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let style = Style::default().fg(Color::DarkGray);
    let sep = "  │  ";
    let width = area.width as usize;

    let left_cwd = format!(" {}", app.cwd_display);
    let left_git = app.git_info.as_ref().map(format_git_segment);

    let right_provider: Option<String> = if app.provider_name.is_empty() {
        None
    } else {
        Some(app.provider_name.clone())
    };
    let right_model: Option<String> = if app.model.is_empty() {
        None
    } else if app.reasoning_effort.is_empty() {
        Some(app.model.clone())
    } else {
        Some(format!("{} · {}", app.model, app.reasoning_effort))
    };

    let mut right_parts: Vec<String> = Vec::new();
    if let Some(ref prov) = right_provider {
        right_parts.push(prov.clone());
        if let Some(ref model) = right_model {
            right_parts.push(sep.to_string());
            right_parts.push(model.clone());
        }
    }

    let right_str: String = right_parts.concat();
    let left_git_str = left_git
        .as_ref()
        .map(|g| format!("{sep}{g}"))
        .unwrap_or_default();
    let left_full = format!("{left_cwd}{left_git_str}");

    let center_str = format_token_budget(app.total_tokens, app.context_window);
    let center_style = token_budget_style(app.total_tokens, app.context_window);

    let gap = 2usize;

    let (left_out, right_out) =
        if !right_str.is_empty() && left_full.len() + gap + right_str.len() <= width {
            (left_full, right_str)
        } else if right_provider.is_some()
            && left_full.len() + gap + right_provider.as_ref().unwrap().len() <= width
        {
            (left_full, right_provider.unwrap())
        } else if left_full.len() <= width {
            (left_full, String::new())
        } else if left_cwd.len() <= width {
            (left_cwd.clone(), String::new())
        } else {
            let max = width.saturating_sub(1);
            let truncated: String = left_cwd.chars().take(max).collect();
            (format!("{truncated}…"), String::new())
        };

    let left_len = left_out.chars().count();
    let right_len = right_out.chars().count();
    let center_len = center_str.chars().count();

    let mut spans: Vec<Span> = vec![Span::styled(left_out, style)];

    let fits_all_three =
        !center_str.is_empty() && left_len + gap + center_len + gap + right_len <= width;

    if fits_all_three && !right_out.is_empty() {
        let total_content = left_len + center_len + right_len;
        let remaining = width.saturating_sub(total_content);
        let left_pad = remaining / 2;
        let right_pad = remaining - left_pad;

        spans.push(Span::raw(" ".repeat(left_pad)));
        spans.push(Span::styled(center_str, center_style));
        spans.push(Span::raw(" ".repeat(right_pad)));
        spans.push(Span::styled(right_out, style));
    } else if fits_all_three {
        let remaining = width.saturating_sub(left_len + center_len);
        let left_pad = remaining / 2;

        spans.push(Span::raw(" ".repeat(left_pad)));
        spans.push(Span::styled(center_str, center_style));
    } else if !center_str.is_empty() && left_len + gap + center_len <= width && right_out.is_empty()
    {
        let remaining = width.saturating_sub(left_len + center_len);
        let left_pad = remaining / 2;

        spans.push(Span::raw(" ".repeat(left_pad)));
        spans.push(Span::styled(center_str, center_style));
    } else if !right_out.is_empty() && left_len + right_len < width {
        let padding = width.saturating_sub(left_len + right_len);
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(right_out, style));
    }

    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}
