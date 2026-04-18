use super::*;
use scarllet_proto::proto::{
    node, AgentPayload, Node, NodeKind, ResultPayload, ThoughtPayload, ToolPayload,
    UserPayload,
};

fn user(id: &str, text: &str) -> Node {
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

fn agent(id: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: None,
        kind: NodeKind::Agent as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Agent(AgentPayload {
            agent_module: "default".into(),
            agent_id: id.into(),
            status: "finished".into(),
        })),
    }
}

fn result_under(id: &str, parent: &str, content: &str) -> Node {
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

fn thought_under(id: &str, parent: &str, content: &str) -> Node {
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

fn tool_under(
    id: &str,
    parent: &str,
    name: &str,
    args: &str,
    status: &str,
    result: &str,
) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Tool as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Tool(ToolPayload {
            tool_name: name.into(),
            arguments_preview: args.into(),
            arguments_json: args.into(),
            status: status.into(),
            duration_ms: 12,
            result_json: result.into(),
        })),
    }
}

#[test]
fn empty_store_yields_empty_history() {
    let store = NodeStore::new();
    let history = conversation_history(&store);
    assert!(history.is_empty());
}

#[test]
fn user_then_agent_with_result_yields_two_entries() {
    let mut store = NodeStore::new();
    store.create(user("u1", "What is 2 + 2?")).unwrap();
    store.create(agent("a1")).unwrap();
    store
        .create(thought_under("t1", "a1", "thinking..."))
        .unwrap();
    store.create(result_under("r1", "a1", "4")).unwrap();

    let history = conversation_history(&store);
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content, "What is 2 + 2?");
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].content, "4");
}

#[test]
fn multi_turn_history_is_chronological() {
    let mut store = NodeStore::new();
    store.create(user("u1", "What is 2 + 2?")).unwrap();
    store.create(agent("a1")).unwrap();
    store.create(result_under("r1", "a1", "4")).unwrap();
    store.create(user("u2", "and times 3?")).unwrap();
    store.create(agent("a2")).unwrap();
    store.create(result_under("r2", "a2", "12")).unwrap();

    let history = conversation_history(&store);
    let roles: Vec<&str> = history.iter().map(|h| h.role.as_str()).collect();
    let contents: Vec<&str> = history.iter().map(|h| h.content.as_str()).collect();
    assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
    assert_eq!(contents, vec!["What is 2 + 2?", "4", "and times 3?", "12"]);
}

#[test]
fn agent_without_result_is_skipped() {
    let mut store = NodeStore::new();
    store.create(user("u1", "ping")).unwrap();
    store.create(agent("a1")).unwrap();

    let history = conversation_history(&store);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "user");
}

#[test]
fn nested_payloads_are_not_emitted_in_history() {
    let mut store = NodeStore::new();
    store.create(agent("a1")).unwrap();
    store
        .create(thought_under("t1", "a1", "halfway"))
        .unwrap();

    let history = conversation_history(&store);
    assert!(history.is_empty(), "thought-only agents must not surface");
}

#[test]
fn tool_call_emits_assistant_call_then_tool_result() {
    let mut store = NodeStore::new();
    store.create(user("u1", "list files")).unwrap();
    store.create(agent("a1")).unwrap();
    store
        .create(tool_under(
            "tool1",
            "a1",
            "tree",
            r#"{"path":"."}"#,
            "done",
            r#"{"entries":["a.txt","b.txt"]}"#,
        ))
        .unwrap();
    store
        .create(result_under("r1", "a1", "Here are the files."))
        .unwrap();

    let history = conversation_history(&store);
    assert_eq!(history.len(), 4, "user + assistant_call + tool + assistant");

    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].content, "list files");

    assert_eq!(history[1].role, "assistant");
    assert!(history[1].content.is_empty(), "assistant tool-call carries empty content");
    let calls_json = history[1]
        .tool_calls_json
        .as_ref()
        .expect("tool_calls_json present on assistant tool-call entry");
    let parsed: serde_json::Value = serde_json::from_str(calls_json).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], "tool1");
    assert_eq!(arr[0]["type"], "function");
    assert_eq!(arr[0]["function"]["name"], "tree");
    assert_eq!(arr[0]["function"]["arguments"], r#"{"path":"."}"#);

    assert_eq!(history[2].role, "tool");
    assert_eq!(history[2].content, r#"{"entries":["a.txt","b.txt"]}"#);
    assert_eq!(history[2].tool_call_id.as_deref(), Some("tool1"));

    assert_eq!(history[3].role, "assistant");
    assert_eq!(history[3].content, "Here are the files.");
    assert!(history[3].tool_calls_json.is_none());
}

#[test]
fn pending_tool_is_skipped_in_history() {
    let mut store = NodeStore::new();
    store.create(user("u1", "list files")).unwrap();
    store.create(agent("a1")).unwrap();
    store
        .create(tool_under("tool1", "a1", "tree", "{}", "running", ""))
        .unwrap();

    let history = conversation_history(&store);
    assert_eq!(history.len(), 1, "running tool must not surface");
    assert_eq!(history[0].role, "user");
}

#[test]
fn failed_tool_still_emits_history_pair() {
    let mut store = NodeStore::new();
    store.create(user("u1", "do thing")).unwrap();
    store.create(agent("a1")).unwrap();
    store
        .create(tool_under(
            "tool1",
            "a1",
            "grep",
            r#"{"pattern":"x"}"#,
            "failed",
            "Error: timeout",
        ))
        .unwrap();
    store
        .create(result_under("r1", "a1", "I could not run grep."))
        .unwrap();

    let history = conversation_history(&store);
    assert_eq!(history.len(), 4);
    assert_eq!(history[1].role, "assistant");
    assert!(history[1].tool_calls_json.is_some());
    assert_eq!(history[2].role, "tool");
    assert_eq!(history[2].content, "Error: timeout");
    assert_eq!(history[3].role, "assistant");
    assert_eq!(history[3].content, "I could not run grep.");
}

#[test]
fn multiple_tool_calls_preserve_creation_order() {
    let mut store = NodeStore::new();
    store.create(user("u1", "look around")).unwrap();
    store.create(agent("a1")).unwrap();
    store
        .create(tool_under(
            "first", "a1", "tree", r#"{"path":"."}"#, "done", "tree-out",
        ))
        .unwrap();
    store
        .create(tool_under(
            "second", "a1", "grep", r#"{"pattern":"main"}"#, "done", "grep-out",
        ))
        .unwrap();
    store
        .create(result_under("r1", "a1", "Done."))
        .unwrap();

    let history = conversation_history(&store);
    let roles: Vec<&str> = history.iter().map(|h| h.role.as_str()).collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "tool", "assistant", "tool", "assistant"]
    );
    assert_eq!(history[2].tool_call_id.as_deref(), Some("first"));
    assert_eq!(history[4].tool_call_id.as_deref(), Some("second"));
}
