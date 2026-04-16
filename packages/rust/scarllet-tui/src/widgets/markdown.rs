use std::ops::Range;

use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

/// Renders markdown to ratatui [`Text`] with GFM table support.
///
/// Non-table content delegates to [`tui_markdown::from_str`]; table
/// regions are parsed with `pulldown-cmark` and rendered as Unicode
/// box-drawn tables.
pub fn render_markdown(input: &str) -> Text<'static> {
    if input.is_empty() {
        return Text::default();
    }

    let segments = segment(input);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (i, seg) in segments.iter().enumerate() {
        if i > 0 && !lines.is_empty() {
            lines.push(Line::default());
        }
        match seg {
            Segment::Text(range) => {
                let slice = &input[range.clone()];
                if slice.trim().is_empty() {
                    continue;
                }
                let md = tui_markdown::from_str(slice);
                for line in md.lines {
                    lines.push(into_owned_line(line));
                }
            }
            Segment::Table(range) => {
                let slice = &input[range.clone()];
                lines.extend(render_table(slice));
            }
        }
    }

    Text::from(lines)
}

enum Segment {
    Text(Range<usize>),
    Table(Range<usize>),
}

/// Splits markdown source into contiguous table and non-table byte ranges.
fn segment(input: &str) -> Vec<Segment> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(input, opts);
    let mut table_ranges: Vec<Range<usize>> = Vec::new();
    let mut table_start: Option<usize> = None;

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Table(_)) => {
                table_start = Some(range.start);
            }
            Event::End(TagEnd::Table) => {
                if let Some(start) = table_start.take() {
                    table_ranges.push(start..range.end);
                }
            }
            _ => {}
        }
    }

    if table_ranges.is_empty() {
        return vec![Segment::Text(0..input.len())];
    }

    let mut segments = Vec::new();
    let mut cursor = 0;

    for tr in &table_ranges {
        if tr.start > cursor {
            segments.push(Segment::Text(cursor..tr.start));
        }
        segments.push(Segment::Table(tr.clone()));
        cursor = tr.end;
    }

    if cursor < input.len() {
        segments.push(Segment::Text(cursor..input.len()));
    }

    segments
}

/// Converts a borrowed [`Line`] into one that owns all its string data.
fn into_owned_line(line: Line<'_>) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect();
    let mut owned = Line::from(spans);
    owned.style = line.style;
    owned.alignment = line.alignment;
    owned
}

// ── Table rendering ─────────────────────────────────────────────────

struct TableData {
    alignments: Vec<Alignment>,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// Parses a markdown table slice into structured data.
fn parse_table(input: &str) -> TableData {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(input, opts);

    let mut alignments = Vec::new();
    let mut header: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_cell = String::new();
    let mut in_header = false;

    for event in parser {
        match event {
            Event::Start(Tag::Table(aligns)) => {
                alignments = aligns;
            }
            Event::Start(Tag::TableHead) => {
                in_header = true;
            }
            Event::End(TagEnd::TableHead) => {
                in_header = false;
            }
            Event::Start(Tag::TableRow) => {
                rows.push(Vec::new());
            }
            Event::Start(Tag::TableCell) => {
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                let cell = current_cell.trim().to_string();
                if in_header {
                    header.push(cell);
                } else if let Some(row) = rows.last_mut() {
                    row.push(cell);
                }
            }
            Event::Text(text) => current_cell.push_str(&text),
            Event::Code(code) => current_cell.push_str(&code),
            Event::SoftBreak | Event::HardBreak => current_cell.push(' '),
            _ => {}
        }
    }

    TableData {
        alignments,
        header,
        rows,
    }
}

/// Renders a GFM table slice as box-drawn [`Line`]s.
fn render_table(input: &str) -> Vec<Line<'static>> {
    let data = parse_table(input);
    if data.header.is_empty() {
        return Vec::new();
    }

    let col_count = data.header.len();

    let mut col_widths: Vec<usize> = data.header.iter().map(|h| h.chars().count()).collect();
    for row in &data.rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() {
                col_widths[i] = col_widths[i].max(cell.chars().count());
            }
        }
    }
    // Each column gets 1-char padding on each side inside the cell.
    let padded: Vec<usize> = col_widths.iter().map(|w| w + 2).collect();

    let border = Style::default().fg(Color::DarkGray);
    let header_style = Style::default().add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(horizontal_border('┌', '┬', '┐', &padded, border));
    lines.push(data_row(&data.header, &col_widths, &data.alignments, col_count, header_style, border));
    lines.push(horizontal_border('├', '┼', '┤', &padded, border));

    for row in &data.rows {
        lines.push(data_row(row, &col_widths, &data.alignments, col_count, Style::default(), border));
    }

    lines.push(horizontal_border('└', '┴', '┘', &padded, border));

    lines
}

/// Builds a horizontal border line (top, separator, or bottom).
fn horizontal_border(
    left: char,
    mid: char,
    right: char,
    padded_widths: &[usize],
    style: Style,
) -> Line<'static> {
    let mut s = String::new();
    for (i, &w) in padded_widths.iter().enumerate() {
        s.push(if i == 0 { left } else { mid });
        for _ in 0..w {
            s.push('─');
        }
    }
    s.push(right);
    Line::styled(s, style)
}

/// Builds a data row with styled cells and border characters.
fn data_row(
    cells: &[String],
    col_widths: &[usize],
    alignments: &[Alignment],
    col_count: usize,
    cell_style: Style,
    border_style: Style,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    for i in 0..col_count {
        let text = cells.get(i).map(String::as_str).unwrap_or("");
        let width = col_widths.get(i).copied().unwrap_or(0);
        let align = alignments.get(i).copied().unwrap_or(Alignment::None);
        let padded = align_cell(text, width, align);

        spans.push(Span::styled("│", border_style));
        spans.push(Span::styled(format!(" {padded} "), cell_style));
    }
    spans.push(Span::styled("│", border_style));

    Line::from(spans)
}

/// Pads cell content to `width` according to column alignment.
fn align_cell(text: &str, width: usize, alignment: Alignment) -> String {
    let text_len = text.chars().count();
    if text_len >= width {
        return text.to_string();
    }

    let pad = width - text_len;
    match alignment {
        Alignment::Right => format!("{}{}", " ".repeat(pad), text),
        Alignment::Center => {
            let left = pad / 2;
            let right = pad - left;
            format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
        }
        _ => format!("{}{}", text, " ".repeat(pad)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flattens a Text into a Vec of plain strings for easy assertion.
    fn to_plain_lines(text: &Text) -> Vec<String> {
        text.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn table_only() {
        let input = "| Name | Age |\n|------|-----|\n| Alice | 30 |\n| Bob | 25 |\n";
        let text = render_markdown(input);
        let plain = to_plain_lines(&text);

        assert!(plain[0].starts_with('┌'), "first line is top border");
        assert!(plain[0].contains('┬'), "top border has column separator");
        assert!(plain[0].ends_with('┐'), "top border ends correctly");

        assert!(plain[1].starts_with('│'), "header row starts with │");
        assert!(plain[1].contains("Name"), "header contains Name");
        assert!(plain[1].contains("Age"), "header contains Age");

        assert!(plain[2].starts_with('├'), "separator starts with ├");

        assert!(plain[3].contains("Alice"), "body row 1 has Alice");
        assert!(plain[4].contains("Bob"), "body row 2 has Bob");

        assert!(plain[5].starts_with('└'), "bottom border starts with └");
    }

    #[test]
    fn text_only_passthrough() {
        let input = "Hello **world**";
        let our_text = render_markdown(input);
        let tui_text = tui_markdown::from_str(input);

        let our_plain = to_plain_lines(&our_text);
        let tui_plain: Vec<String> = tui_text
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert_eq!(our_plain, tui_plain);
    }

    #[test]
    fn mixed_text_and_table() {
        let input = "Intro paragraph\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nOutro paragraph\n";
        let text = render_markdown(input);
        let plain = to_plain_lines(&text);
        let joined = plain.join("\n");

        assert!(joined.contains("Intro"), "has intro text");
        assert!(joined.contains('┌'), "has table top border");
        assert!(joined.contains('└'), "has table bottom border");
        assert!(joined.contains("Outro"), "has outro text");

        let intro_pos = joined.find("Intro").unwrap();
        let table_pos = joined.find('┌').unwrap();
        let outro_pos = joined.find("Outro").unwrap();
        assert!(intro_pos < table_pos, "intro before table");
        assert!(table_pos < outro_pos, "table before outro");
    }

    #[test]
    fn column_alignment() {
        let input = "| Left | Center | Right |\n|:-----|:------:|------:|\n| a | b | c |\n";
        let text = render_markdown(input);
        let plain = to_plain_lines(&text);

        // Body row with alignment applied
        let body = &plain[3];
        assert!(body.contains("a "), "left-aligned pads right");
        assert!(body.contains(" c"), "right-aligned pads left");
    }

    #[test]
    fn single_column() {
        let input = "| Solo |\n|------|\n| val |\n";
        let text = render_markdown(input);
        let plain = to_plain_lines(&text);

        assert!(!plain[0].contains('┬'), "single column has no ┬");
        assert!(plain[0].starts_with('┌'));
        assert!(plain[0].ends_with('┐'));
        assert!(plain[1].contains("Solo"));
    }

    #[test]
    fn empty_cells() {
        let input = "| A | B |\n|---|---|\n|   |   |\n";
        let text = render_markdown(input);
        let plain = to_plain_lines(&text);

        assert!(plain[3].contains('│'), "empty-cell row still has borders");
        assert_eq!(plain.len(), 5, "header + separator + 1 body + 2 borders = 5 lines");
    }

    #[test]
    fn empty_input() {
        let text = render_markdown("");
        assert!(text.lines.is_empty());
    }
}
