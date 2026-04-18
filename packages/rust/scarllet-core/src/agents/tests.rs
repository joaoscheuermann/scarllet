use super::*;
use scarllet_proto::proto::{node, AgentPayload, Node, ToolPayload};

fn record(id: &str, parent: &str) -> (AgentRecord, mpsc::Receiver<Result<AgentInbound, Status>>) {
    let (tx, rx) = mpsc::channel(8);
    let rec = AgentRecord {
        agent_id: id.into(),
        agent_module: "default".into(),
        parent_id: parent.into(),
        pid: None,
        tx,
        agent_node_id: id.into(),
    };
    (rec, rx)
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

fn tool_node(id: &str, parent: &str, tool_name: &str) -> Node {
    Node {
        id: id.into(),
        parent_id: Some(parent.into()),
        kind: NodeKind::Tool as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Tool(ToolPayload {
            tool_name: tool_name.into(),
            arguments_preview: String::new(),
            arguments_json: String::new(),
            status: "running".into(),
            duration_ms: 0,
            result_json: String::new(),
        })),
    }
}

#[test]
fn register_main_agent_marks_main_slot() {
    let mut reg = AgentRegistry::new();
    let (rec, _rx) = record("a1", "session-1");
    reg.register("session-1", rec);
    assert!(reg.has_main());
    assert!(reg.get("a1").is_some());
}

#[test]
fn register_sub_agent_does_not_mark_main_slot() {
    let mut reg = AgentRegistry::new();
    let (rec, _rx) = record("sub", "parent-agent");
    reg.register("session-1", rec);
    assert!(!reg.has_main());
}

#[test]
fn deregister_clears_main_slot() {
    let mut reg = AgentRegistry::new();
    let (rec, _rx) = record("a1", "session-1");
    reg.register("session-1", rec);
    reg.deregister("a1");
    assert!(!reg.has_main());
    assert!(reg.get("a1").is_none());
}

#[test]
fn sub_agent_waiter_roundtrip() {
    let mut reg = AgentRegistry::new();
    let (tx, _rx) = oneshot::channel::<Result<ResultPayload, String>>();
    assert!(!reg.has_sub_agent_waiter("child-1"));
    reg.register_sub_agent_waiter("child-1".into(), tx);
    assert!(reg.has_sub_agent_waiter("child-1"));
    assert!(reg.take_sub_agent_waiter("child-1").is_some());
    assert!(!reg.has_sub_agent_waiter("child-1"));
}

#[test]
fn any_descendant_running_returns_false_when_no_subs() {
    let mut store = NodeStore::new();
    store.create(agent_node("parent-a", None)).unwrap();

    let mut reg = AgentRegistry::new();
    let (rec, _rx) = record("parent-a", "session-1");
    reg.register("session-1", rec);

    assert!(!reg.any_descendant_running("parent-a", &store));
}

#[test]
fn any_descendant_running_finds_registered_sub_agent() {
    let mut store = NodeStore::new();
    store.create(agent_node("parent-a", None)).unwrap();
    store
        .create(tool_node("tool-1", "parent-a", "spawn_sub_agent"))
        .unwrap();
    store
        .create(agent_node("child-a", Some("tool-1")))
        .unwrap();

    let mut reg = AgentRegistry::new();
    let (parent_rec, _prx) = record("parent-a", "session-1");
    reg.register("session-1", parent_rec);
    let (child_rec, _crx) = record("child-a", "parent-a");
    reg.register("session-1", child_rec);

    assert!(reg.any_descendant_running("parent-a", &store));
}

#[test]
fn any_descendant_running_ignores_finished_sub_agents() {
    let mut store = NodeStore::new();
    store.create(agent_node("parent-a", None)).unwrap();
    store
        .create(tool_node("tool-1", "parent-a", "spawn_sub_agent"))
        .unwrap();
    store
        .create(agent_node("child-a", Some("tool-1")))
        .unwrap();

    let mut reg = AgentRegistry::new();
    let (parent_rec, _prx) = record("parent-a", "session-1");
    reg.register("session-1", parent_rec);
    // child-a exists as a Node but is no longer registered: "finished".
    assert!(!reg.any_descendant_running("parent-a", &store));
}

#[test]
fn descendant_agent_ids_includes_only_registered_descendants() {
    let mut store = NodeStore::new();
    store.create(agent_node("parent-a", None)).unwrap();
    store
        .create(tool_node("tool-1", "parent-a", "spawn_sub_agent"))
        .unwrap();
    store
        .create(agent_node("child-a", Some("tool-1")))
        .unwrap();
    store
        .create(tool_node("tool-2", "child-a", "spawn_sub_agent"))
        .unwrap();
    store
        .create(agent_node("grandchild-a", Some("tool-2")))
        .unwrap();

    let mut reg = AgentRegistry::new();
    let (p, _p_rx) = record("parent-a", "session-1");
    reg.register("session-1", p);
    let (c, _c_rx) = record("child-a", "parent-a");
    reg.register("session-1", c);
    // grandchild-a finished + deregistered already; should be excluded.

    let descendants = reg.descendant_agent_ids("parent-a", &store);
    assert_eq!(descendants, vec!["child-a".to_string()]);
}
