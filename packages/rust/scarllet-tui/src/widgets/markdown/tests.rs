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
    assert_eq!(
        plain.len(),
        5,
        "header + separator + 1 body + 2 borders = 5 lines"
    );
}

#[test]
fn empty_input() {
    let text = render_markdown("");
    assert!(text.lines.is_empty());
}

/// Acceptance criterion: a GFM table must emit the four characteristic
/// Unicode box-drawing borders (top `┌…┐`, header separator `├…┤`,
/// column separator `┬` / `┼` / `┴`, bottom `└…┘`). This is the
/// single most distinctive visual guarantee restored by this module.
#[test]
fn gfm_table_emits_box_drawing_characters() {
    let input = "| Col A | Col B |\n|-------|-------|\n| foo   | bar   |\n";
    let text = render_markdown(input);
    let joined: String = to_plain_lines(&text).join("\n");

    for glyph in ['┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '│', '─'] {
        assert!(
            joined.contains(glyph),
            "rendered table must contain {glyph:?}; got:\n{joined}"
        );
    }
}
