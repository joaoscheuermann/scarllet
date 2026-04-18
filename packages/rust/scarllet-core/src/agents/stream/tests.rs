#![allow(clippy::unwrap_used)]

use super::*;

use crate::agents::AgentRecord;
use crate::session::{Session, SessionConfig};
use scarllet_proto::proto::{
    node, AgentPayload, Node, NodeKind, ResultPayload, ToolPayload,
};
use scarllet_sdk::config::ScarlletConfig;
use tokio::sync::{mpsc, oneshot};

fn make_session() -> Session {
    Session::new(
        "session-test".into(),
        SessionConfig::from_global(&ScarlletConfig::default()),
    )
}

fn make_agent_record(agent_id: &str, parent_id: &str) -> AgentRecord {
    let (tx, _rx) = mpsc::channel(4);
    AgentRecord {
        agent_id: agent_id.into(),
        agent_module: "default".into(),
        parent_id: parent_id.into(),
        pid: None,
        tx,
        agent_node_id: agent_id.into(),
    }
}

fn seed_agent_node(session: &mut Session, id: &str, parent: Option<&str>, status: &str) {
    session
        .nodes
        .create(Node {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            kind: NodeKind::Agent as i32,
            created_at: 0,
            updated_at: 0,
            payload: Some(node::Payload::Agent(AgentPayload {
                agent_module: "default".into(),
                agent_id: id.into(),
                status: status.into(),
            })),
        })
        .unwrap();
}

fn seed_tool_node(session: &mut Session, id: &str, parent: &str, name: &str) {
    session
        .nodes
        .create(Node {
            id: id.into(),
            parent_id: Some(parent.into()),
            kind: NodeKind::Tool as i32,
            created_at: 0,
            updated_at: 0,
            payload: Some(node::Payload::Tool(ToolPayload {
                tool_name: name.into(),
                arguments_preview: String::new(),
                arguments_json: String::new(),
                status: "running".into(),
                duration_ms: 0,
                result_json: String::new(),
            })),
        })
        .unwrap();
}

fn seed_result_under(session: &mut Session, parent: &str, content: &str) {
    let id = Uuid::new_v4().to_string();
    session
        .nodes
        .create(Node {
            id,
            parent_id: Some(parent.into()),
            kind: NodeKind::Result as i32,
            created_at: 0,
            updated_at: 0,
            payload: Some(node::Payload::Result(ResultPayload {
                content: content.into(),
                finish_reason: "stop".into(),
            })),
        })
        .unwrap();
}

fn count_top_level_errors(session: &Session) -> usize {
    session
        .nodes
        .all()
        .filter(|n| {
            n.parent_id.is_none()
                && matches!(
                    NodeKind::try_from(n.kind).unwrap_or(NodeKind::Unspecified),
                    NodeKind::Error
                )
        })
        .count()
}

fn agent_status(session: &Session, agent_id: &str) -> String {
    let node = session.nodes.get(agent_id).expect("agent node present");
    let Some(node::Payload::Agent(p)) = node.payload.as_ref() else {
        panic!("expected Agent payload");
    };
    p.status.clone()
}

#[test]
fn main_agent_turn_finished_marks_finished_and_requests_dispatch() {
    let mut session = make_session();
    let session_id = session.id.clone();
    seed_agent_node(&mut session, "main-a", None, "running");
    session
        .agents
        .register(&session_id, make_agent_record("main-a", &session_id));

    let outcome = process_turn_finished(&mut session, "main-a", "stop");
    assert!(outcome.dispatch_next, "main agent finish triggers re-dispatch");
    assert!(outcome.pids_to_kill.is_empty());
    assert!(session.agents.get("main-a").is_none(), "deregistered");
    assert_eq!(agent_status(&session, "main-a"), "finished");
}

#[test]
fn sub_agent_turn_finished_fires_waiter_without_dispatch() {
    let mut session = make_session();
    let session_id = session.id.clone();
    seed_agent_node(&mut session, "parent-a", None, "running");
    seed_tool_node(&mut session, "tool-1", "parent-a", "spawn_sub_agent");
    seed_agent_node(&mut session, "child-a", Some("tool-1"), "running");
    seed_result_under(&mut session, "child-a", "done");

    session
        .agents
        .register(&session_id, make_agent_record("parent-a", &session_id));
    session
        .agents
        .register(&session_id, make_agent_record("child-a", "parent-a"));

    let (tx, rx) = oneshot::channel::<Result<ResultPayload, String>>();
    session
        .agents
        .register_sub_agent_waiter("child-a".into(), tx);

    let outcome = process_turn_finished(&mut session, "child-a", "stop");
    assert!(!outcome.dispatch_next, "sub-agent finish must not dispatch");
    assert!(outcome.pids_to_kill.is_empty());
    assert!(session.agents.get("child-a").is_none());
    assert_eq!(agent_status(&session, "child-a"), "finished");

    let received = rx.blocking_recv().expect("waiter fired").expect("ok result");
    assert_eq!(received.content, "done");
    assert_eq!(received.finish_reason, "stop");
}

#[test]
fn parent_turn_finished_with_running_descendant_triggers_ac_8_4_cascade() {
    let mut session = make_session();
    let session_id = session.id.clone();
    seed_agent_node(&mut session, "parent-a", None, "running");
    seed_tool_node(&mut session, "tool-1", "parent-a", "spawn_sub_agent");
    seed_agent_node(&mut session, "child-a", Some("tool-1"), "running");

    session
        .agents
        .register(&session_id, make_agent_record("parent-a", &session_id));
    session
        .agents
        .register(&session_id, make_agent_record("child-a", "parent-a"));

    let (tx, rx) = oneshot::channel::<Result<ResultPayload, String>>();
    session
        .agents
        .register_sub_agent_waiter("child-a".into(), tx);

    let outcome = process_turn_finished(&mut session, "parent-a", "stop");

    assert!(!outcome.dispatch_next, "reject TurnFinished");
    assert_eq!(
        count_top_level_errors(&session),
        1,
        "one top-level invariant error emitted"
    );
    assert_eq!(agent_status(&session, "parent-a"), "failed");
    assert_eq!(agent_status(&session, "child-a"), "failed");
    assert!(session.agents.get("parent-a").is_none());
    assert!(session.agents.get("child-a").is_none());

    let fired = rx
        .blocking_recv()
        .expect("waiter fired with error")
        .expect_err("invariant-violation error");
    assert!(fired.contains("invariant violation"), "got: {fired}");
}

#[test]
fn parent_turn_finished_without_descendants_still_finishes_normally() {
    let mut session = make_session();
    let session_id = session.id.clone();
    seed_agent_node(&mut session, "parent-a", None, "running");
    seed_tool_node(&mut session, "tool-1", "parent-a", "spawn_sub_agent");
    seed_agent_node(&mut session, "child-a", Some("tool-1"), "finished");

    session
        .agents
        .register(&session_id, make_agent_record("parent-a", &session_id));
    // child-a is NOT registered — it already finished.

    let outcome = process_turn_finished(&mut session, "parent-a", "stop");
    assert!(outcome.dispatch_next);
    assert_eq!(count_top_level_errors(&session), 0);
    assert_eq!(agent_status(&session, "parent-a"), "finished");
}

// -----------------------------------------------------------------
// Effort 06 tests
// -----------------------------------------------------------------

use scarllet_proto::proto::{session_diff, SessionDiff};

/// Pushes a fresh subscriber onto `session.subscribers` and returns
/// its receiver so tests can poll the broadcast stream.
fn attach_subscriber(
    session: &mut Session,
) -> tokio::sync::mpsc::Receiver<Result<SessionDiff, tonic::Status>> {
    let (tx, rx) = mpsc::channel::<Result<SessionDiff, tonic::Status>>(64);
    session.subscribers.push(tx);
    rx
}

/// Drains every `SessionDiff` already produced by the synchronous
/// mutation under test out of `rx`. Stops on empty without blocking.
fn drain(rx: &mut tokio::sync::mpsc::Receiver<Result<SessionDiff, tonic::Status>>) -> Vec<SessionDiff> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let Ok(diff) = msg {
            out.push(diff);
        }
    }
    out
}

/// Creates a pair of `AgentRecord`s (parent + child sub-agent) and a
/// matching set of `Agent` / `Tool` nodes + a parent PID channel.
fn seed_parent_and_child(session: &mut Session) {
    let session_id = session.id.clone();
    seed_agent_node(session, "parent-a", None, "running");
    seed_tool_node(session, "tool-1", "parent-a", "spawn_sub_agent");
    seed_agent_node(session, "child-a", Some("tool-1"), "running");

    let (parent_tx, _parent_rx) = mpsc::channel(8);
    let parent_rec = AgentRecord {
        agent_id: "parent-a".into(),
        agent_module: "default".into(),
        parent_id: session_id.clone(),
        pid: Some(1111),
        tx: parent_tx,
        agent_node_id: "parent-a".into(),
    };
    session.agents.register(&session_id, parent_rec);

    let (child_tx, _child_rx) = mpsc::channel(8);
    let child_rec = AgentRecord {
        agent_id: "child-a".into(),
        agent_module: "default".into(),
        parent_id: "parent-a".into(),
        pid: Some(2222),
        tx: child_tx,
        agent_node_id: "child-a".into(),
    };
    session.agents.register(&session_id, child_rec);
}

#[test]
fn cascade_cancel_deregisters_sub_agent_before_main() {
    let mut session = make_session();
    seed_parent_and_child(&mut session);

    let sorted = agent_ids_leaves_first(&session);
    let parent_idx = sorted.iter().position(|id| id == "parent-a").unwrap();
    let child_idx = sorted.iter().position(|id| id == "child-a").unwrap();
    assert!(
        child_idx < parent_idx,
        "sub-agent must come before main agent in cascade order (got {sorted:?})"
    );
}

#[tokio::test]
async fn cascade_cancel_marks_every_agent_failed_and_clears_the_registry() {
    let mut session = make_session();
    seed_parent_and_child(&mut session);

    // Register a waiter for the child to check the err-propagation
    // branch fires.
    let (tx, rx) = oneshot::channel::<Result<ResultPayload, String>>();
    session
        .agents
        .register_sub_agent_waiter("child-a".into(), tx);

    cascade_cancel(&mut session, CANCEL_REASON);

    // Both Agent nodes end up patched to "failed".
    assert_eq!(agent_status(&session, "parent-a"), "failed");
    assert_eq!(agent_status(&session, "child-a"), "failed");

    // Both records are gone from the registry.
    assert!(session.agents.get("parent-a").is_none());
    assert!(session.agents.get("child-a").is_none());

    // Waiter received the cancel error.
    let got = rx
        .await
        .expect("waiter fired")
        .expect_err("expected Err variant");
    assert_eq!(got, CANCEL_REASON);

    // Error child nodes exist under each Agent node.
    let count_errors_under = |parent: &str| -> usize {
        session
            .nodes
            .all()
            .filter(|n| {
                n.parent_id.as_deref() == Some(parent)
                    && matches!(
                        NodeKind::try_from(n.kind).unwrap_or(NodeKind::Unspecified),
                        NodeKind::Error
                    )
            })
            .count()
    };
    assert_eq!(count_errors_under("parent-a"), 1, "per-turn Error under parent");
    assert_eq!(count_errors_under("child-a"), 1, "per-turn Error under child");

    // `status` is **not** touched by cascade_cancel — callers own the
    // Running↔Paused transition.
    assert_eq!(session.status, SessionStatus::Running);
}

#[tokio::test]
async fn apply_agent_termination_flips_session_to_paused_and_broadcasts_once() {
    let mut session = make_session();
    let session_id = session.id.clone();
    seed_agent_node(&mut session, "main-a", None, "running");
    session
        .agents
        .register(&session_id, make_agent_record("main-a", &session_id));

    let mut rx = attach_subscriber(&mut session);

    apply_agent_termination(&mut session, "main-a", "agent disconnected unexpectedly");

    assert_eq!(session.status, SessionStatus::Paused);
    assert_eq!(agent_status(&session, "main-a"), "failed");
    assert!(session.agents.get("main-a").is_none(), "deregistered");

    let diffs = drain(&mut rx);
    let status_changes: Vec<_> = diffs
        .iter()
        .filter_map(|d| match d.payload.as_ref() {
            Some(session_diff::Payload::StatusChanged(sc)) => Some(sc.status.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        status_changes,
        vec!["PAUSED".to_string()],
        "exactly one StatusChanged=PAUSED broadcast"
    );
}

#[tokio::test]
async fn apply_agent_termination_called_twice_does_not_double_broadcast_paused() {
    let mut session = make_session();
    let session_id = session.id.clone();
    seed_agent_node(&mut session, "main-a", None, "running");
    session
        .agents
        .register(&session_id, make_agent_record("main-a", &session_id));

    let mut rx = attach_subscriber(&mut session);

    apply_agent_termination(&mut session, "main-a", "first");
    // Second call is a no-op (agent already deregistered); but even if
    // a caller retried with a stray id, `set_status(Paused)` is
    // idempotent.
    apply_agent_termination(&mut session, "ghost", "second");

    let diffs = drain(&mut rx);
    let status_changes = diffs
        .iter()
        .filter(|d| matches!(d.payload.as_ref(), Some(session_diff::Payload::StatusChanged(_))))
        .count();
    assert_eq!(status_changes, 1, "Paused broadcast must not repeat");
}

#[tokio::test]
async fn apply_agent_termination_for_sub_agent_does_not_flip_session_status() {
    let mut session = make_session();
    seed_parent_and_child(&mut session);

    // Mark child as a sub-agent via waiter registration.
    let (tx, _rx) = oneshot::channel::<Result<ResultPayload, String>>();
    session
        .agents
        .register_sub_agent_waiter("child-a".into(), tx);

    let mut diff_rx = attach_subscriber(&mut session);

    apply_agent_termination(&mut session, "child-a", "sub-agent died");

    // Session remains Running — only main-agent termination flips to Paused.
    assert_eq!(session.status, SessionStatus::Running);
    let diffs = drain(&mut diff_rx);
    let status_broadcasts = diffs
        .iter()
        .filter(|d| matches!(d.payload.as_ref(), Some(session_diff::Payload::StatusChanged(_))))
        .count();
    assert_eq!(
        status_broadcasts, 0,
        "sub-agent termination must not emit StatusChanged"
    );
}
