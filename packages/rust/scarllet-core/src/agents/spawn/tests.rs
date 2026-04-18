#![allow(clippy::unwrap_used)]

use super::*;

use crate::agents::AgentRecord;
use crate::session::{Session, SessionRegistry};
use scarllet_proto::proto::{node, AgentPayload, ResultPayload, ToolPayload};
use scarllet_sdk::config::ScarlletConfig;
use scarllet_sdk::manifest::{ModuleKind, ModuleManifest};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

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

fn make_session_in_registry() -> (Arc<RwLock<SessionRegistry>>, String) {
    let mut sessions = SessionRegistry::new();
    let id = sessions.create_session(&ScarlletConfig::default());
    (Arc::new(RwLock::new(sessions)), id)
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

fn seed_parent_with_spawn_tool(
    session: &mut Session,
    parent_agent_id: &str,
    module: &str,
    prompt: &str,
) -> String {
    session
        .nodes
        .create(Node {
            id: parent_agent_id.into(),
            parent_id: None,
            kind: NodeKind::Agent as i32,
            created_at: 0,
            updated_at: 0,
            payload: Some(node::Payload::Agent(AgentPayload {
                agent_module: "default".into(),
                agent_id: parent_agent_id.into(),
                status: "running".into(),
            })),
        })
        .unwrap();

    let tool_id = Uuid::new_v4().to_string();
    let args_json = serde_json::json!({
        "agent_module": module,
        "prompt": prompt,
    })
    .to_string();
    session
        .nodes
        .create(Node {
            id: tool_id.clone(),
            parent_id: Some(parent_agent_id.into()),
            kind: NodeKind::Tool as i32,
            created_at: 0,
            updated_at: 0,
            payload: Some(node::Payload::Tool(ToolPayload {
                tool_name: crate::tools::SPAWN_SUB_AGENT_TOOL.into(),
                arguments_preview: String::new(),
                arguments_json: args_json,
                status: "pending".into(),
                duration_ms: 0,
                result_json: String::new(),
            })),
        })
        .unwrap();

    let session_id = session.id.clone();
    session
        .agents
        .register(&session_id, make_agent_record(parent_agent_id, &session_id));

    tool_id
}

#[tokio::test]
async fn handle_spawn_sub_agent_rejects_missing_module() {
    let (sessions, session_id) = make_session_in_registry();
    let registry = empty_registry();

    // Seed a parent agent / tool so the early-return is exclusively the missing-module branch.
    {
        let handle = sessions.read().await.get(&session_id).unwrap();
        let mut session = handle.write().await;
        seed_parent_with_spawn_tool(&mut session, "parent-1", "missing", "hi");
    }

    let input = serde_json::json!({ "agent_module": "missing", "prompt": "hi" }).to_string();
    let result = handle_spawn_sub_agent_with(
        &sessions,
        &registry,
        "127.0.0.1:0",
        &session_id,
        "parent-1",
        &input,
        |_| panic!("spawn_fn must not fire when module is missing"),
    )
    .await;

    assert!(!result.success);
    assert!(
        result
            .error_message
            .contains("agent module 'missing' not registered"),
        "got: {}",
        result.error_message
    );
}

#[tokio::test]
async fn handle_spawn_sub_agent_rejects_malformed_input_json() {
    let (sessions, session_id) = make_session_in_registry();
    let registry = empty_registry();

    let result = handle_spawn_sub_agent_with(
        &sessions,
        &registry,
        "127.0.0.1:0",
        &session_id,
        "parent-1",
        "not-json",
        |_| panic!("spawn_fn must not fire on parse error"),
    )
    .await;

    assert!(!result.success);
    assert!(
        result
            .error_message
            .starts_with("invalid spawn_sub_agent args"),
        "got: {}",
        result.error_message
    );
}

#[tokio::test]
async fn handle_spawn_sub_agent_happy_path_returns_result_content() {
    let (sessions, session_id) = make_session_in_registry();
    let registry = registry_with_agent("default", "/fake/path/default");

    // Seed parent agent + Tool node before calling.
    let parent_agent_id = "parent-a";
    let prompt = "summarise this repo";
    let module = "default";
    {
        let handle = sessions.read().await.get(&session_id).unwrap();
        let mut session = handle.write().await;
        seed_parent_with_spawn_tool(&mut session, parent_agent_id, module, prompt);
    }

    let spawn_calls = Arc::new(AtomicU32::new(0));
    let spawn_calls_clone = Arc::clone(&spawn_calls);

    let input = serde_json::json!({ "agent_module": module, "prompt": prompt }).to_string();

    let sessions_for_fire = Arc::clone(&sessions);
    let session_id_for_fire = session_id.clone();
    // Concurrently fire the waiter once the spawn callback has run.
    let fire_task = tokio::spawn(async move {
        // Poll until the waiter is installed.
        loop {
            let handle = sessions_for_fire
                .read()
                .await
                .get(&session_id_for_fire)
                .unwrap();
            let mut sess = handle.write().await;
            let child_id = sess
                .pending_dispatch
                .keys()
                .next()
                .cloned();
            if let Some(id) = child_id {
                if let Some(tx) = sess.agents.take_sub_agent_waiter(&id) {
                    let _ = tx.send(Ok(ResultPayload {
                        content: "done".into(),
                        finish_reason: "stop".into(),
                    }));
                    return;
                }
            }
            drop(sess);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });

    let result = handle_spawn_sub_agent_with(
        &sessions,
        &registry,
        "127.0.0.1:0",
        &session_id,
        parent_agent_id,
        &input,
        move |args| {
            spawn_calls_clone.fetch_add(1, Ordering::SeqCst);
            assert_eq!(args.agent_module, module);
            assert_eq!(args.parent_agent_id, parent_agent_id);
            Some(9999)
        },
    )
    .await;

    let _ = fire_task.await;

    assert_eq!(spawn_calls.load(Ordering::SeqCst), 1);
    assert!(result.success, "got error: {}", result.error_message);
    let parsed: serde_json::Value = serde_json::from_str(&result.output_json).unwrap();
    assert_eq!(parsed["content"], "done");
    assert_eq!(parsed["finish_reason"], "stop");
}

#[tokio::test]
async fn handle_spawn_sub_agent_maps_waiter_error_to_tool_failure() {
    let (sessions, session_id) = make_session_in_registry();
    let registry = registry_with_agent("default", "/fake/path/default");

    let parent_agent_id = "parent-b";
    let prompt = "run this";
    let module = "default";
    {
        let handle = sessions.read().await.get(&session_id).unwrap();
        let mut session = handle.write().await;
        seed_parent_with_spawn_tool(&mut session, parent_agent_id, module, prompt);
    }

    let sessions_for_fire = Arc::clone(&sessions);
    let session_id_for_fire = session_id.clone();
    let fire_task = tokio::spawn(async move {
        loop {
            let handle = sessions_for_fire
                .read()
                .await
                .get(&session_id_for_fire)
                .unwrap();
            let mut sess = handle.write().await;
            let child_id = sess.pending_dispatch.keys().next().cloned();
            if let Some(id) = child_id {
                if let Some(tx) = sess.agents.take_sub_agent_waiter(&id) {
                    let _ = tx.send(Err("sub-agent crashed".into()));
                    return;
                }
            }
            drop(sess);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });

    let input = serde_json::json!({ "agent_module": module, "prompt": prompt }).to_string();
    let result = handle_spawn_sub_agent_with(
        &sessions,
        &registry,
        "127.0.0.1:0",
        &session_id,
        parent_agent_id,
        &input,
        |_| Some(1234),
    )
    .await;

    let _ = fire_task.await;

    assert!(!result.success);
    assert_eq!(result.error_message, "sub-agent crashed");
}

#[tokio::test]
async fn handle_spawn_sub_agent_oneshot_drop_maps_to_unexpected_termination() {
    let (sessions, session_id) = make_session_in_registry();
    let registry = registry_with_agent("default", "/fake/path/default");

    let parent_agent_id = "parent-c";
    let prompt = "x";
    let module = "default";
    {
        let handle = sessions.read().await.get(&session_id).unwrap();
        let mut session = handle.write().await;
        seed_parent_with_spawn_tool(&mut session, parent_agent_id, module, prompt);
    }

    let sessions_for_drop = Arc::clone(&sessions);
    let session_id_for_drop = session_id.clone();
    let drop_task = tokio::spawn(async move {
        loop {
            let handle = sessions_for_drop
                .read()
                .await
                .get(&session_id_for_drop)
                .unwrap();
            let mut sess = handle.write().await;
            let child_id = sess.pending_dispatch.keys().next().cloned();
            if let Some(id) = child_id {
                if let Some(tx) = sess.agents.take_sub_agent_waiter(&id) {
                    drop(tx); // sender dropped without sending → RecvError
                    return;
                }
            }
            drop(sess);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });

    let input = serde_json::json!({ "agent_module": module, "prompt": prompt }).to_string();
    let result = handle_spawn_sub_agent_with(
        &sessions,
        &registry,
        "127.0.0.1:0",
        &session_id,
        parent_agent_id,
        &input,
        |_| None,
    )
    .await;

    let _ = drop_task.await;
    assert!(!result.success);
    assert_eq!(result.error_message, "sub-agent terminated unexpectedly");
}
