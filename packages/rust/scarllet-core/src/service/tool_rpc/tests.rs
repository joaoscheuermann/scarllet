use super::*;
use crate::registry::ModuleRegistry;
use crate::session::SessionRegistry;
use scarllet_sdk::config::ScarlletConfig;
use scarllet_sdk::manifest::ModuleManifest;
use std::sync::Arc;
use tokio::sync::RwLock;

fn build_service() -> OrchestratorService {
    OrchestratorService {
        registry: Arc::new(RwLock::new(ModuleRegistry::new())),
        config: Arc::new(RwLock::new(ScarlletConfig::default())),
        sessions: Arc::new(RwLock::new(SessionRegistry::new())),
        bound_addr: "127.0.0.1:0".to_string(),
    }
}

fn tool_manifest(name: &str) -> ModuleManifest {
    ModuleManifest {
        name: name.to_string(),
        kind: ModuleKind::Tool,
        version: "0.1.0".into(),
        description: format!("test tool {name}"),
        input_schema: Some(serde_json::json!({"type": "object"})),
        timeout_ms: Some(1000),
        capabilities: vec![],
        aliases: vec![],
    }
}

#[tokio::test]
async fn invoke_tool_rejects_unknown_session() {
    let svc = build_service();
    let req = Request::new(InvokeToolRequest {
        session_id: "missing-session".into(),
        agent_id: "irrelevant".into(),
        tool_name: "tree".into(),
        input_json: "{}".into(),
    });
    let err = invoke_tool(&svc, req).await.expect_err("unknown session");
    assert_eq!(err.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn invoke_tool_rejects_unknown_agent_in_session() {
    let svc = build_service();
    let session_id = {
        let mut sessions = svc.sessions.write().await;
        sessions.create_session(&ScarlletConfig::default())
    };
    let req = Request::new(InvokeToolRequest {
        session_id,
        agent_id: "ghost-agent".into(),
        tool_name: "tree".into(),
        input_json: "{}".into(),
    });
    let err = invoke_tool(&svc, req).await.expect_err("unknown agent");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
}

#[tokio::test]
async fn get_tool_registry_includes_external_and_spawn_sub_agent() {
    let svc = build_service();
    {
        let mut reg = svc.registry.write().await;
        reg.register(std::path::PathBuf::from("/tmp/tree"), tool_manifest("tree"));
        reg.register(std::path::PathBuf::from("/tmp/grep"), tool_manifest("grep"));
    }
    let session_id = {
        let mut sessions = svc.sessions.write().await;
        sessions.create_session(&ScarlletConfig::default())
    };

    let resp = get_tool_registry(&svc, Request::new(GetToolRegistryRequest { session_id }))
        .await
        .expect("registry call succeeds");
    let tools = resp.into_inner().tools;
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"tree"));
    assert!(names.contains(&"grep"));
    assert!(
        names.contains(&"spawn_sub_agent"),
        "synthetic spawn_sub_agent must always be advertised"
    );
    let spawn = tools
        .iter()
        .find(|t| t.name == "spawn_sub_agent")
        .expect("spawn_sub_agent present");
    assert!(!spawn.description.is_empty());
    assert!(!spawn.input_schema_json.is_empty());
}

#[tokio::test]
async fn get_tool_registry_rejects_unknown_session() {
    let svc = build_service();
    let err = get_tool_registry(
        &svc,
        Request::new(GetToolRegistryRequest {
            session_id: "missing".into(),
        }),
    )
    .await
    .expect_err("unknown session");
    assert_eq!(err.code(), tonic::Code::NotFound);
}
