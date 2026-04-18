use super::*;

fn response(configured: bool, name: &str, model: &str, effort: &str) -> ActiveProviderResponse {
    ActiveProviderResponse {
        configured,
        provider_name: name.into(),
        provider_type: "openai".into(),
        api_url: String::new(),
        api_key: String::new(),
        model: model.into(),
        reasoning_effort: effort.into(),
    }
}

#[test]
fn provider_info_from_wire_returns_none_when_not_configured() {
    let info = ProviderInfo::from_wire(response(false, "", "", ""));
    assert!(info.is_none());
}

#[test]
fn provider_info_from_wire_populates_display_fields() {
    let info = ProviderInfo::from_wire(response(true, "openrouter", "gpt-4o", "medium"))
        .expect("configured provider yields Some");
    assert_eq!(info.name, "openrouter");
    assert_eq!(info.model, "gpt-4o");
    assert_eq!(info.reasoning_effort, "medium");
}

#[test]
fn display_label_joins_name_model_effort_with_middle_dot() {
    let info = ProviderInfo {
        name: "openrouter".into(),
        model: "gpt-4o".into(),
        reasoning_effort: "medium".into(),
    };
    assert_eq!(
        info.display_label().as_deref(),
        Some("openrouter · gpt-4o · medium")
    );
}

#[test]
fn display_label_omits_empty_effort() {
    let info = ProviderInfo {
        name: "openrouter".into(),
        model: "gpt-4o".into(),
        reasoning_effort: String::new(),
    };
    assert_eq!(info.display_label().as_deref(), Some("openrouter · gpt-4o"));
}

#[test]
fn display_label_returns_none_when_all_core_fields_empty() {
    let info = ProviderInfo::default();
    assert!(info.display_label().is_none());
}

use scarllet_proto::proto::{
    AgentPayload, DebugPayload, ErrorPayload, ResultPayload, ThoughtPayload,
};

fn app_with_debug(debug_enabled: bool) -> App {
    let (tx, _rx) = mpsc::channel::<CoreCommand>(16);
    App::new(tx, PathBuf::from("."), debug_enabled)
}

fn top_level_agent(id: &str, status: &str) -> Node {
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

fn thought_child(id: &str, parent: &str, content: &str) -> Node {
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

fn result_child(id: &str, parent: &str, content: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Result as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Result(ResultPayload {
            content: content.into(),
            finish_reason: "stop".into(),
        })),
    }
}

fn debug_child(id: &str, parent: &str, message: &str) -> Node {
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

fn error_child(id: &str, parent: &str, message: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Error as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "default".into(),
            message: message.into(),
        })),
    }
}

#[test]
fn visible_subtree_chars_sums_thought_and_result_only() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "running"));
    app.insert_node(thought_child("t1", "a1", "hello")); // 5
    app.insert_node(result_child("r1", "a1", "world!")); // 6
    assert_eq!(app.visible_subtree_chars("a1"), 11);
}

#[test]
fn visible_subtree_chars_skips_debug_when_flag_off() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "running"));
    app.insert_node(thought_child("t1", "a1", "hi")); // 2
    app.insert_node(debug_child("d1", "a1", "debug message")); // not counted
    assert_eq!(app.visible_subtree_chars("a1"), 2);
}

#[test]
fn visible_subtree_chars_counts_debug_when_flag_on() {
    let mut app = app_with_debug(true);
    app.insert_node(top_level_agent("a1", "running"));
    app.insert_node(thought_child("t1", "a1", "hi")); // 2
    app.insert_node(debug_child("d1", "a1", "abc")); // 3
    assert_eq!(app.visible_subtree_chars("a1"), 5);
}

#[test]
fn visible_subtree_chars_counts_error_messages() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "failed"));
    app.insert_node(error_child("e1", "a1", "boom")); // 4
    assert_eq!(app.visible_subtree_chars("a1"), 4);
}

#[test]
fn visible_subtree_chars_handles_non_ascii() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "running"));
    app.insert_node(thought_child("t1", "a1", "héllo✨"));
    // "héllo✨" is 6 chars, not 9 bytes.
    assert_eq!(app.visible_subtree_chars("a1"), 6);
}

// Pins the literal reveal rate so a future refactor doesn't
// silently drift from the ~600 chars/sec cadence the pre-refactor
// animation targeted. 30 chars/tick × 20 Hz (50 ms tick) = 600 /s.
#[test]
fn typewriter_chars_per_tick_is_pinned_to_thirty() {
    assert_eq!(TYPEWRITER_CHARS_PER_TICK, 30);
}

#[test]
fn advance_typewriter_grows_by_per_tick_while_running() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "running"));
    // 100 chars of content — enough to take multiple ticks.
    let content = "x".repeat(100);
    app.insert_node(thought_child("t1", "a1", &content));

    app.advance_typewriter();
    assert_eq!(
        app.reveal_for("a1").visible_chars,
        TYPEWRITER_CHARS_PER_TICK
    );
    app.advance_typewriter();
    assert_eq!(
        app.reveal_for("a1").visible_chars,
        TYPEWRITER_CHARS_PER_TICK * 2
    );
}

#[test]
fn advance_typewriter_caps_at_total_while_running() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "running"));
    app.insert_node(thought_child("t1", "a1", "ab")); // 2 chars total
    for _ in 0..4 {
        app.advance_typewriter();
    }
    assert_eq!(app.reveal_for("a1").visible_chars, 2);
}

#[test]
fn advance_typewriter_snaps_to_total_when_finished() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "running"));
    let content = "y".repeat(100);
    app.insert_node(thought_child("t1", "a1", &content));
    app.advance_typewriter();
    assert_eq!(
        app.reveal_for("a1").visible_chars,
        TYPEWRITER_CHARS_PER_TICK
    );

    // Flip the Agent status via the patch path.
    app.apply_node_patch(
        "a1",
        NodePatch {
            agent_status: Some("finished".into()),
            ..Default::default()
        },
        0,
    );
    app.advance_typewriter();
    assert_eq!(app.reveal_for("a1").visible_chars, 100);
}

#[test]
fn advance_typewriter_snaps_to_total_when_failed() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "failed"));
    app.insert_node(result_child("r1", "a1", "partial result"));
    app.advance_typewriter();
    assert_eq!(
        app.reveal_for("a1").visible_chars,
        "partial result".chars().count()
    );
}

#[test]
fn reset_with_clears_reveal_state() {
    let mut app = app_with_debug(false);
    app.insert_node(top_level_agent("a1", "running"));
    app.insert_node(thought_child("t1", "a1", "abcdef"));
    app.advance_typewriter();
    assert!(app.reveal_for("a1").visible_chars > 0);

    app.reset_with(
        "new-session".into(),
        SessionStatus::Running,
        Vec::new(),
        0,
        Vec::new(),
        None,
    );
    assert_eq!(app.reveal_for("a1").visible_chars, 0);
}
