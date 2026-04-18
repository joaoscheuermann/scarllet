use super::*;

const SEP: &str = "  │  ";
const GAP: usize = 2;

#[test]
fn layout_shows_provider_when_terminal_is_wide_enough() {
    let layout = pick_right_layout(
        " ~/scarllet",
        " ~/scarllet",
        Some("openrouter · gpt-4o"),
        Some("tokens: 1234/8192"),
        "THINKING",
        "session 12345678",
        SEP,
        200,
        GAP,
    );
    assert!(layout.show_provider, "wide terminal must keep provider");
    assert!(layout.show_tokens);
    assert!(layout.show_session);
}

#[test]
fn layout_drops_provider_first_when_terminal_is_narrow() {
    // Width chosen so provider+tokens+session+lifecycle does NOT fit
    // but tokens+session+lifecycle DOES.
    let left = " ~/scarllet";
    let tokens = "tokens: 1234/8192";
    let provider = "openrouter · gpt-4o";
    let lifecycle = "THINKING";
    let session = "session 12345678";
    let without_provider = format!("{tokens}{SEP}{lifecycle}{SEP}{session}");
    let with_provider =
        format!("{tokens}{SEP}{provider}{SEP}{lifecycle}{SEP}{session}");
    let width = left.chars().count() + GAP + without_provider.chars().count() + 1;
    assert!(
        left.chars().count() + GAP + with_provider.chars().count() > width,
        "test setup: full layout must not fit at width={width}"
    );
    let layout = pick_right_layout(
        left, left, Some(provider), Some(tokens), lifecycle, session, SEP, width, GAP,
    );
    assert!(
        !layout.show_provider,
        "provider must be dropped first on narrow terminals"
    );
    assert!(layout.show_tokens, "tokens survive if provider fits out");
    assert!(layout.show_session);
}

#[test]
fn layout_keeps_lifecycle_even_at_tiny_widths() {
    let layout = pick_right_layout(
        " /",
        " /",
        Some("openrouter · gpt-4o"),
        Some("tokens: 1/1"),
        "PAUSED",
        "session abcdefgh",
        SEP,
        "PAUSED".len() + GAP + 2,
        GAP,
    );
    assert!(layout.has_lifecycle, "lifecycle must survive");
    assert!(!layout.show_provider);
    assert!(!layout.show_tokens);
    assert!(!layout.show_session);
}

#[test]
fn layout_omits_provider_segment_when_none() {
    let layout = pick_right_layout(
        " ~/scarllet",
        " ~/scarllet",
        None,
        Some("tokens: 10/100"),
        "READY",
        "session 12345678",
        SEP,
        200,
        GAP,
    );
    assert!(
        !layout.show_provider,
        "provider never surfaces when provider info is absent"
    );
    assert!(layout.show_tokens);
}
