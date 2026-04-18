use super::*;

#[test]
fn truncate_preview_passes_through_short_strings() {
    assert_eq!(truncate_preview("abc", 10), "abc");
}

#[test]
fn truncate_preview_appends_ellipsis_when_too_long() {
    let out = truncate_preview("abcdefghij", 5);
    assert_eq!(out, "abcde…");
}

#[test]
fn inject_working_directory_adds_when_missing() {
    let out = inject_working_directory(r#"{"path":"."}"#, "/tmp/x");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["path"], ".");
    assert_eq!(v["working_directory"], "/tmp/x");
}

#[test]
fn inject_working_directory_preserves_existing() {
    let raw = r#"{"path":".","working_directory":"/explicit"}"#;
    let out = inject_working_directory(raw, "/tmp/x");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["working_directory"], "/explicit");
}

#[test]
fn inject_working_directory_passes_through_non_object() {
    assert_eq!(inject_working_directory("[]", "/x"), "[]");
    assert_eq!(inject_working_directory("not-json", "/x"), "not-json");
}

#[test]
fn accumulate_tool_call_deltas_concats_arguments() {
    let mut acc: Vec<AccumulatedCall> = Vec::new();
    accumulate_tool_call_deltas(
        &mut acc,
        &[ToolCallDelta {
            index: 0,
            id: Some("call_1".into()),
            function_name: Some("tree".into()),
            function_arguments: Some("{\"path\"".into()),
            thought_signature: None,
        }],
    );
    accumulate_tool_call_deltas(
        &mut acc,
        &[ToolCallDelta {
            index: 0,
            id: None,
            function_name: None,
            function_arguments: Some(":\".\"}".into()),
            thought_signature: None,
        }],
    );

    let calls = finalize_tool_calls(acc);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].function.name, "tree");
    assert_eq!(calls[0].function.arguments, r#"{"path":"."}"#);
}

#[test]
fn history_entry_to_chat_message_parses_tool_calls_json() {
    let entry = HistoryEntry {
        role: "assistant".into(),
        content: String::new(),
        tool_call_id: None,
        tool_calls_json: Some(
            r#"[{"id":"t1","type":"function","function":{"name":"tree","arguments":"{}"}}]"#
                .into(),
        ),
    };
    let msg = history_entry_to_chat_message(&entry);
    let calls = msg.tool_calls.expect("tool_calls populated");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "t1");
    assert_eq!(calls[0].function.name, "tree");
}

#[test]
fn history_entry_to_chat_message_propagates_tool_call_id() {
    let entry = HistoryEntry {
        role: "tool".into(),
        content: "result".into(),
        tool_call_id: Some("t1".into()),
        tool_calls_json: None,
    };
    let msg = history_entry_to_chat_message(&entry);
    assert!(matches!(msg.role, Role::Tool));
    assert_eq!(msg.tool_call_id.as_deref(), Some("t1"));
    assert_eq!(msg.content, "result");
}
