use super::*;
use crate::agents::AgentRecord;
use crate::session::{Session, SessionConfig};
use scarllet_proto::proto::{node, NodeKind, QueuedPrompt};
use scarllet_sdk::config::ScarlletConfig;
use scarllet_sdk::manifest::{ModuleKind, ModuleManifest};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

fn config_with_default_agent(name: &str) -> ScarlletConfig {
    ScarlletConfig {
        default_agent: name.to_string(),
        ..Default::default()
    }
}

fn empty_registry() -> Arc<RwLock<ModuleRegistry>> {
    Arc::new(RwLock::new(ModuleRegistry::new()))
}

fn registry_with_agent(name: &str, path: &str) -> Arc<RwLock<ModuleRegistry>> {
    let mut reg = ModuleRegistry::new();
    reg.register(
        PathBuf::from(path),
        ModuleManifest {
            name: name.into(),
            kind: ModuleKind::Agent,
            version: "0.0.0".into(),
            description: "test".into(),
            input_schema: None,
            timeout_ms: None,
            capabilities: vec![],
            aliases: vec![],
        },
    );
    Arc::new(RwLock::new(reg))
}

fn make_session(default_agent: &str) -> Session {
    Session::new(
        "session-test".into(),
        SessionConfig::from_global(&config_with_default_agent(default_agent)),
    )
}

fn enqueue(session: &mut Session, prompt_id: &str, text: &str) {
    session.queue.push_back(QueuedPrompt {
        prompt_id: prompt_id.into(),
        text: text.into(),
        working_directory: String::new(),
        user_node_id: format!("user-{prompt_id}"),
    });
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

/// Counts how many times the spawn callback fires across one
/// `try_dispatch_main_with` invocation; used by the short-circuit
/// tests below.
fn never_spawn() -> impl FnOnce(SpawnArgs<'_>) -> Option<u32> {
    |_| panic!("spawn_fn must not be invoked")
}

#[tokio::test]
async fn short_circuits_when_paused() {
    let mut session = make_session("default");
    session.status = SessionStatus::Paused;
    enqueue(&mut session, "p1", "hello");
    let registry = empty_registry();

    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", never_spawn()).await;

    assert_eq!(session.queue.len(), 1, "queue must be untouched while paused");
    assert!(session.pending_dispatch.is_empty());
    assert!(session.nodes.is_empty());
}

#[tokio::test]
async fn short_circuits_when_main_already_running() {
    let mut session = make_session("default");
    let session_id = session.id.clone();
    session
        .agents
        .register(&session_id, make_agent_record("running-1", &session_id));
    enqueue(&mut session, "p1", "hello");
    let registry = empty_registry();

    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", never_spawn()).await;

    assert_eq!(
        session.queue.len(),
        1,
        "queue must be untouched while a main is running"
    );
    assert!(session.pending_dispatch.is_empty());
}

#[tokio::test]
async fn short_circuits_when_queue_empty() {
    let mut session = make_session("default");
    let registry = empty_registry();

    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", never_spawn()).await;

    assert!(session.queue.is_empty());
    assert!(session.pending_dispatch.is_empty());
    assert!(session.nodes.is_empty());
}

#[tokio::test]
async fn missing_default_agent_pops_queue_and_emits_error() {
    let mut session = make_session("default");
    enqueue(&mut session, "p1", "hello");
    let registry = empty_registry();

    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", never_spawn()).await;

    assert!(session.queue.is_empty(), "missing module must drain its queue item");
    assert!(session.pending_dispatch.is_empty());
    assert_eq!(count_top_level_errors(&session), 1);

    let err = session
        .nodes
        .all()
        .find(|n| {
            n.parent_id.is_none()
                && matches!(
                    NodeKind::try_from(n.kind).unwrap_or(NodeKind::Unspecified),
                    NodeKind::Error
                )
        })
        .expect("error node was created");
    let Some(node::Payload::Error(payload)) = err.payload.as_ref() else {
        panic!("expected Error payload");
    };
    assert!(
        payload.message.contains("agent module 'default' is not registered"),
        "got message: {}",
        payload.message
    );
}

#[tokio::test]
async fn empty_default_agent_pops_queue_and_emits_specific_error() {
    let mut session = make_session("");
    enqueue(&mut session, "p1", "hello");
    let registry = empty_registry();

    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", never_spawn()).await;

    assert!(session.queue.is_empty());
    assert_eq!(count_top_level_errors(&session), 1);

    let err = session
        .nodes
        .all()
        .find(|n| {
            n.parent_id.is_none()
                && matches!(
                    NodeKind::try_from(n.kind).unwrap_or(NodeKind::Unspecified),
                    NodeKind::Error
                )
        })
        .unwrap();
    let Some(node::Payload::Error(payload)) = err.payload.as_ref() else {
        panic!("expected Error payload");
    };
    assert_eq!(payload.message, "default_agent not configured");
}

#[tokio::test]
async fn happy_path_pops_queue_creates_agent_node_and_pending_dispatch() {
    let mut session = make_session("default");
    enqueue(&mut session, "p1", "hello");
    let registry = registry_with_agent("default", "/fake/path/default");

    let spawn_calls = Arc::new(AtomicU32::new(0));
    let calls = Arc::clone(&spawn_calls);

    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", move |_args| {
        calls.fetch_add(1, Ordering::SeqCst);
        Some(42)
    })
    .await;

    assert_eq!(
        spawn_calls.load(Ordering::SeqCst),
        1,
        "spawn called exactly once"
    );
    assert!(session.queue.is_empty());

    let agent_nodes: Vec<_> = session
        .nodes
        .all()
        .filter(|n| {
            matches!(
                NodeKind::try_from(n.kind).unwrap_or(NodeKind::Unspecified),
                NodeKind::Agent
            )
        })
        .collect();
    assert_eq!(agent_nodes.len(), 1, "exactly one Agent node created");
    let agent_id = agent_nodes[0].id.clone();
    assert!(
        session.pending_dispatch.contains_key(&agent_id),
        "pending dispatch keyed by agent id"
    );
    let pending = &session.pending_dispatch[&agent_id];
    assert_eq!(pending.prompt.text, "hello");
}

#[tokio::test]
async fn two_enqueued_prompts_dispatch_in_order_under_successive_turn_finished() {
    use std::sync::Mutex;

    let mut session = make_session("default");
    let session_id = session.id.clone();
    enqueue(&mut session, "p1", "first");
    enqueue(&mut session, "p2", "second");
    let registry = registry_with_agent("default", "/fake/path/default");

    let spawn_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // First dispatch — pulls "first" off the queue.
    let log1 = Arc::clone(&spawn_log);
    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", move |args| {
        log1.lock()
            .unwrap()
            .push(format!("{}|{}", args.agent_id, args.agent_module));
        Some(1)
    })
    .await;

    let first_agent_id = session
        .pending_dispatch
        .keys()
        .next()
        .cloned()
        .expect("first dispatch enqueued a pending entry");
    assert_eq!(session.queue.len(), 1, "second prompt remains queued");

    // Promote the pending dispatch to a registered agent (mirrors
    // what `agents::stream::handle_register` does in production).
    let _pending = session.pending_dispatch.remove(&first_agent_id).unwrap();
    session
        .agents
        .register(&session_id, make_agent_record(&first_agent_id, &session_id));

    // Second invocation must short-circuit because a main is running.
    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", |_| {
        panic!("must not spawn while main is running");
    })
    .await;
    assert_eq!(session.queue.len(), 1, "queue still has the second prompt");

    // Simulate TurnFinished: deregister the running agent, then re-call
    // try_dispatch_main, which should drain the second prompt.
    session.agents.deregister(&first_agent_id);

    let log2 = Arc::clone(&spawn_log);
    try_dispatch_main_with(&mut session, &registry, "127.0.0.1:0", move |args| {
        log2.lock()
            .unwrap()
            .push(format!("{}|{}", args.agent_id, args.agent_module));
        Some(2)
    })
    .await;

    assert!(session.queue.is_empty(), "second prompt now dispatched");
    let second_agent_id = session
        .pending_dispatch
        .keys()
        .next()
        .cloned()
        .expect("second dispatch enqueued a pending entry");
    assert_ne!(first_agent_id, second_agent_id);

    let entries = spawn_log.lock().unwrap().clone();
    assert_eq!(entries.len(), 2, "spawn_fn was invoked once per dispatch");
    let agent_ids: Vec<&str> = entries.iter().map(|s| s.split('|').next().unwrap()).collect();
    assert_eq!(agent_ids[0], first_agent_id);
    assert_eq!(agent_ids[1], second_agent_id);

    let second_pending = &session.pending_dispatch[&second_agent_id];
    assert_eq!(second_pending.prompt.text, "second");
}
