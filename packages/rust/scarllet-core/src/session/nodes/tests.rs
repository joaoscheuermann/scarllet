use super::*;
use scarllet_proto::proto::{
    node, AgentPayload, ErrorPayload, ResultPayload, ThoughtPayload, ToolPayload, UserPayload,
};

fn user_node(id: &str, text: &str) -> Node {
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

fn agent_node(id: &str, parent: Option<&str>) -> Node {
    Node {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        kind: NodeKind::Agent as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Agent(AgentPayload {
            agent_module: "default".into(),
            agent_id: id.into(),
            status: "running".into(),
        })),
    }
}

fn result_node(id: &str, parent: Option<&str>) -> Node {
    Node {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        kind: NodeKind::Result as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Result(ResultPayload {
            content: "ok".into(),
            finish_reason: "stop".into(),
        })),
    }
}

fn thought_node(id: &str, parent: &str, content: &str) -> Node {
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

fn empty_result_node(id: &str, parent: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Result as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Result(ResultPayload {
            content: String::new(),
            finish_reason: String::new(),
        })),
    }
}

fn tool_node(id: &str, parent: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Tool as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Tool(ToolPayload {
            tool_name: "noop".into(),
            arguments_preview: String::new(),
            arguments_json: String::new(),
            status: "pending".into(),
            duration_ms: 0,
            result_json: String::new(),
        })),
    }
}

fn error_node(id: &str, parent: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Error as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "test".into(),
            message: "initial".into(),
        })),
    }
}

fn seed_agent(store: &mut NodeStore) -> &'static str {
    store.create(agent_node("a1", None)).unwrap();
    "a1"
}

#[test]
fn create_top_level_user_succeeds() {
    let mut store = NodeStore::new();
    store.create(user_node("u1", "hi")).expect("happy path");
    assert_eq!(store.len(), 1);
    assert_eq!(store.order, vec!["u1"]);
    assert!(store.children_of.is_empty());
}

#[test]
fn create_result_under_agent_succeeds() {
    let mut store = NodeStore::new();
    store.create(agent_node("a1", None)).unwrap();
    store
        .create(result_node("r1", Some("a1")))
        .expect("Result under Agent is allowed");

    assert_eq!(store.children_of.get("a1").unwrap(), &vec!["r1".to_string()]);
}

#[test]
fn create_result_top_level_is_rejected() {
    let mut store = NodeStore::new();
    let err = store.create(result_node("r1", None)).unwrap_err();
    assert_eq!(err, InvariantError::ParentRequired(NodeKind::Result));
}

#[test]
fn create_result_with_unknown_parent_is_rejected() {
    let mut store = NodeStore::new();
    let err = store.create(result_node("r1", Some("missing"))).unwrap_err();
    assert_eq!(err, InvariantError::UnknownParent("missing".into()));
}

#[test]
fn create_result_under_user_is_rejected() {
    let mut store = NodeStore::new();
    store.create(user_node("u1", "hi")).unwrap();
    let err = store.create(result_node("r1", Some("u1"))).unwrap_err();
    assert!(matches!(
        err,
        InvariantError::InvalidParentKind {
            child: NodeKind::Result,
            expected_parent: NodeKind::Agent,
            actual_parent: NodeKind::User,
        }
    ));
}

#[test]
fn create_user_with_parent_is_rejected() {
    let mut store = NodeStore::new();
    store.create(agent_node("a1", None)).unwrap();
    let err = store.create(user_node_with_parent("u1", "hi", "a1")).unwrap_err();
    assert_eq!(err, InvariantError::TopLevelNotAllowed(NodeKind::User));
}

#[test]
fn create_duplicate_id_is_rejected() {
    let mut store = NodeStore::new();
    store.create(agent_node("a1", None)).unwrap();
    let err = store.create(agent_node("a1", None)).unwrap_err();
    assert_eq!(err, InvariantError::DuplicateId("a1".into()));
}

#[test]
fn update_unknown_id_is_rejected() {
    let mut store = NodeStore::new();
    let err = store
        .update("nonexistent", NodePatch::default(), 1)
        .unwrap_err();
    assert_eq!(err, InvariantError::UnknownNode("nonexistent".into()));
}

#[test]
fn update_thought_content_appends_chunks() {
    let mut store = NodeStore::new();
    let agent_id = seed_agent(&mut store);
    store.create(thought_node("t1", agent_id, "Hel")).unwrap();

    store
        .update(
            "t1",
            NodePatch {
                thought_content: Some("lo, ".into()),
                ..Default::default()
            },
            10,
        )
        .unwrap();
    store
        .update(
            "t1",
            NodePatch {
                thought_content: Some("world!".into()),
                ..Default::default()
            },
            20,
        )
        .unwrap();

    let node = store.get("t1").unwrap();
    let Some(node::Payload::Thought(t)) = node.payload.as_ref() else {
        panic!("expected Thought payload");
    };
    assert_eq!(t.content, "Hello, world!");
    assert_eq!(node.updated_at, 20);
}

#[test]
fn update_result_content_appends_and_finish_reason_replaces() {
    let mut store = NodeStore::new();
    let agent_id = seed_agent(&mut store);
    store.create(empty_result_node("r1", agent_id)).unwrap();

    store
        .update(
            "r1",
            NodePatch {
                result_content: Some("Hello".into()),
                result_finish_reason: Some("length".into()),
                ..Default::default()
            },
            5,
        )
        .unwrap();
    store
        .update(
            "r1",
            NodePatch {
                result_content: Some(" world".into()),
                result_finish_reason: Some("stop".into()),
                ..Default::default()
            },
            7,
        )
        .unwrap();

    let node = store.get("r1").unwrap();
    let Some(node::Payload::Result(r)) = node.payload.as_ref() else {
        panic!("expected Result payload");
    };
    assert_eq!(r.content, "Hello world", "content must append");
    assert_eq!(r.finish_reason, "stop", "finish_reason must replace");
    assert_eq!(node.updated_at, 7);
}

#[test]
fn update_tool_fields_replace() {
    let mut store = NodeStore::new();
    let agent_id = seed_agent(&mut store);
    store.create(tool_node("tool1", agent_id)).unwrap();

    store
        .update(
            "tool1",
            NodePatch {
                tool_status: Some("running".into()),
                tool_duration_ms: Some(123),
                tool_result_json: Some(r#"{"k":1}"#.into()),
                ..Default::default()
            },
            3,
        )
        .unwrap();
    store
        .update(
            "tool1",
            NodePatch {
                tool_status: Some("done".into()),
                tool_duration_ms: Some(456),
                tool_result_json: Some(r#"{"k":2}"#.into()),
                ..Default::default()
            },
            4,
        )
        .unwrap();

    let node = store.get("tool1").unwrap();
    let Some(node::Payload::Tool(t)) = node.payload.as_ref() else {
        panic!("expected Tool payload");
    };
    assert_eq!(t.status, "done");
    assert_eq!(t.duration_ms, 456);
    assert_eq!(t.result_json, r#"{"k":2}"#);
    assert_eq!(node.updated_at, 4);
}

#[test]
fn update_agent_status_replaces() {
    let mut store = NodeStore::new();
    let agent_id = seed_agent(&mut store);

    store
        .update(
            agent_id,
            NodePatch {
                agent_status: Some("finished".into()),
                ..Default::default()
            },
            42,
        )
        .unwrap();

    let node = store.get(agent_id).unwrap();
    let Some(node::Payload::Agent(a)) = node.payload.as_ref() else {
        panic!("expected Agent payload");
    };
    assert_eq!(a.status, "finished");
    assert_eq!(node.updated_at, 42);
}

#[test]
fn update_error_message_replaces() {
    let mut store = NodeStore::new();
    let agent_id = seed_agent(&mut store);
    store.create(error_node("e1", agent_id)).unwrap();

    store
        .update(
            "e1",
            NodePatch {
                error_message: Some("revised".into()),
                ..Default::default()
            },
            9,
        )
        .unwrap();

    let node = store.get("e1").unwrap();
    let Some(node::Payload::Error(e)) = node.payload.as_ref() else {
        panic!("expected Error payload");
    };
    assert_eq!(e.message, "revised");
}

#[test]
fn update_does_not_touch_unrelated_fields() {
    let mut store = NodeStore::new();
    let agent_id = seed_agent(&mut store);
    store
        .create(thought_node("t1", agent_id, "original"))
        .unwrap();

    store
        .update(
            "t1",
            NodePatch {
                // No thought_content set: payload must be unchanged.
                agent_status: Some("ignored".into()),
                ..Default::default()
            },
            1,
        )
        .unwrap();

    let node = store.get("t1").unwrap();
    let Some(node::Payload::Thought(t)) = node.payload.as_ref() else {
        panic!("expected Thought payload");
    };
    assert_eq!(t.content, "original");
}

fn user_node_with_parent(id: &str, text: &str, parent: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::User as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::User(UserPayload {
            text: text.into(),
            working_directory: String::new(),
        })),
    }
}

fn top_level_error_node(id: &str, message: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: None,
        kind: NodeKind::Error as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "core".into(),
            message: message.into(),
        })),
    }
}

fn debug_node(id: &str, parent: Option<&str>) -> Node {
    use scarllet_proto::proto::DebugPayload;
    Node {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        kind: NodeKind::Debug as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Debug(DebugPayload {
            source: "test".into(),
            level: "info".into(),
            message: "hello".into(),
        })),
    }
}

fn token_usage_node(id: &str, parent: Option<&str>) -> Node {
    use scarllet_proto::proto::TokenUsagePayload;
    Node {
        id: id.into(),
        parent_id: parent.map(str::to_string),
        kind: NodeKind::TokenUsage as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::TokenUsage(TokenUsagePayload {
            total_tokens: 10,
            context_window: 1000,
        })),
    }
}

#[test]
fn create_top_level_error_succeeds() {
    let mut store = NodeStore::new();
    store
        .create(top_level_error_node("e1", "missing default agent"))
        .expect("Error may be top-level (session-level error)");
    assert_eq!(store.len(), 1);
    assert_eq!(store.order, vec!["e1"]);
    let Some(node::Payload::Error(e)) = store.get("e1").unwrap().payload.as_ref() else {
        panic!("expected Error payload");
    };
    assert_eq!(e.message, "missing default agent");
}

#[test]
fn create_agent_parented_error_succeeds() {
    let mut store = NodeStore::new();
    let agent_id = seed_agent(&mut store);
    store
        .create(error_node("e1", agent_id))
        .expect("Error under Agent is allowed (per-turn error)");
    assert_eq!(store.children_of.get(agent_id).unwrap(), &vec!["e1".to_string()]);
}

#[test]
fn create_debug_without_parent_is_rejected() {
    let mut store = NodeStore::new();
    let err = store.create(debug_node("d1", None)).unwrap_err();
    assert_eq!(err, InvariantError::ParentRequired(NodeKind::Debug));
}

#[test]
fn create_token_usage_without_parent_is_rejected() {
    let mut store = NodeStore::new();
    let err = store.create(token_usage_node("u1", None)).unwrap_err();
    assert_eq!(err, InvariantError::ParentRequired(NodeKind::TokenUsage));
}

#[test]
fn create_thought_under_non_agent_is_rejected() {
    let mut store = NodeStore::new();
    store.create(user_node("u1", "hi")).unwrap();
    let err = store.create(thought_node("t1", "u1", "hi")).unwrap_err();
    assert!(matches!(
        err,
        InvariantError::InvalidParentKind {
            child: NodeKind::Thought,
            expected_parent: NodeKind::Agent,
            actual_parent: NodeKind::User,
        }
    ));
}
