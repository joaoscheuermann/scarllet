//! Crate-level integration test for the bidi `AgentStream` handshake.
//!
//! Regression test for `bug-001-infinite-working-no-diffs`: the
//! `AgentStream` RPC used to deadlock because the server handler
//! awaited `incoming.message().await` before returning `Response`,
//! while the client awaited `client.agent_stream(...).await` before
//! sending `Register`. Both sides sat forever waiting for the other.
//!
//! This test spins up a real `OrchestratorService` on a loopback TCP
//! port, opens the bidi stream end-to-end, and asserts that the
//! matching `AgentInbound::Task` arrives within a short timeout —
//! which would fail against the pre-fix code because the handshake
//! never completes. Two tests are provided:
//!
//!  * [`direct_bidi_handshake_delivers_task`] — uses the generated
//!    `OrchestratorClient` directly (no SDK) and exercises the exact
//!    handshake path the default agent takes over the wire.
//!  * [`sdk_agent_session_connect_delivers_task`] — reuses
//!    `scarllet_sdk::agent::AgentSession::connect` against the same
//!    in-process server so the SDK's preemptive-register code path is
//!    also covered.

use std::sync::Arc;
use std::time::Duration;

use scarllet_core::agents::routing::PendingDispatch;
use scarllet_core::registry::ModuleRegistry;
use scarllet_core::service::OrchestratorService;
use scarllet_core::session::SessionRegistry;
use scarllet_proto::proto::agent_inbound;
use scarllet_proto::proto::agent_outbound;
use scarllet_proto::proto::orchestrator_client::OrchestratorClient;
use scarllet_proto::proto::orchestrator_server::OrchestratorServer;
use scarllet_proto::proto::{
    node, AgentOutbound, AgentPayload, AgentRegister, CreateSessionRequest, Node, NodeKind,
    QueuedPrompt,
};
use scarllet_sdk::config::ScarlletConfig;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, RwLock};
use tokio::time::timeout;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::transport::Channel;

/// Returns the hard wall-clock limit on every handshake step. Anything
/// longer than this means the deadlock regressed — the real handshake
/// on the loopback completes in tens of milliseconds on every
/// development machine we've measured.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

/// Spins up an `OrchestratorService` on a fresh loopback port and hands
/// back both a connected client plus a shared handle to the session
/// registry so the test can seed state directly (the real spawn path
/// would launch an agent process — we skip that and hand-stuff a
/// pending dispatch instead).
struct InProcServer {
    client: OrchestratorClient<Channel>,
    sessions: Arc<RwLock<SessionRegistry>>,
    _shutdown_tx: tokio::sync::oneshot::Sender<()>,
    bound_addr: String,
}

/// Boots a full `OrchestratorService` on a random loopback TCP port,
/// returns an [`InProcServer`] carrying the connected client, the
/// shared session registry, and a shutdown guard that stops the
/// server when dropped.
async fn boot_in_proc_server() -> InProcServer {
    let registry = Arc::new(RwLock::new(ModuleRegistry::new()));
    let sessions = Arc::new(RwLock::new(SessionRegistry::new()));
    let config = Arc::new(RwLock::new(ScarlletConfig::default()));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let bound_addr = listener.local_addr().expect("resolve bound addr");
    let bound_addr_str = bound_addr.to_string();

    let service = OrchestratorService {
        registry,
        config,
        sessions: Arc::clone(&sessions),
        bound_addr: bound_addr_str.clone(),
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let incoming = TcpListenerStream::new(listener);

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(OrchestratorServer::new(service))
            .serve_with_incoming_shutdown(incoming, async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    let endpoint = format!("http://{bound_addr_str}");
    let client = loop {
        match OrchestratorClient::connect(endpoint.clone()).await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    };

    InProcServer {
        client,
        sessions,
        _shutdown_tx: shutdown_tx,
        bound_addr: bound_addr_str,
    }
}

/// Seeds the `Agent` node + `pending_dispatch` entry that `handle_register`
/// expects, bypassing the real `routing::try_dispatch_main` path (which
/// would launch an agent process). Returns the generated agent_id.
async fn seed_pending_dispatch(
    sessions: &Arc<RwLock<SessionRegistry>>,
    session_id: &str,
    prompt_text: &str,
) -> String {
    let handle = {
        let reg = sessions.read().await;
        reg.get(session_id).expect("session exists")
    };
    let mut session = handle.write().await;
    let agent_id = uuid::Uuid::new_v4().to_string();

    let agent_node = Node {
        id: agent_id.clone(),
        parent_id: None,
        kind: NodeKind::Agent as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Agent(AgentPayload {
            agent_module: "default".into(),
            agent_id: agent_id.clone(),
            status: "running".into(),
        })),
    };
    session
        .nodes
        .create(agent_node)
        .expect("seed agent node invariants hold");

    session.pending_dispatch.insert(
        agent_id.clone(),
        PendingDispatch {
            prompt: QueuedPrompt {
                prompt_id: uuid::Uuid::new_v4().to_string(),
                text: prompt_text.into(),
                working_directory: String::new(),
                user_node_id: format!("user-{agent_id}"),
            },
            pid: None,
        },
    );

    agent_id
}

/// Exercises the full `AgentStream` handshake using the generated gRPC
/// client directly. The test fails against the pre-fix code because the
/// handler + SDK both wait forever; post-fix the timeout is not reached
/// and the `AgentInbound::Task` arrives quickly.
#[tokio::test]
async fn direct_bidi_handshake_delivers_task() {
    let mut server = boot_in_proc_server().await;

    let session_id = timeout(
        HANDSHAKE_TIMEOUT,
        server.client.create_session(CreateSessionRequest {}),
    )
    .await
    .expect("CreateSession did not complete within 2s")
    .expect("CreateSession rpc succeeds")
    .into_inner()
    .session_id;

    let agent_id = seed_pending_dispatch(&server.sessions, &session_id, "hello from test").await;

    let (out_tx, out_rx) = mpsc::channel::<AgentOutbound>(64);
    out_tx
        .send(AgentOutbound {
            payload: Some(agent_outbound::Payload::Register(AgentRegister {
                session_id: session_id.clone(),
                agent_id: agent_id.clone(),
                agent_module: "default".into(),
                parent_id: session_id.clone(),
            })),
        })
        .await
        .expect("push register onto outbound channel");

    let outgoing = ReceiverStream::new(out_rx);

    let response = timeout(HANDSHAKE_TIMEOUT, server.client.agent_stream(outgoing))
        .await
        .expect(
            "AgentStream RPC did not return Response within 2s — bidi handshake deadlock regressed",
        )
        .expect("AgentStream rpc succeeds");

    let mut inbound = response.into_inner();

    let first = timeout(HANDSHAKE_TIMEOUT, inbound.message())
        .await
        .expect(
            "no AgentInbound message within 2s — register -> task handshake deadlock regressed",
        )
        .expect("inbound stream yields Ok");

    let Some(msg) = first else {
        panic!("inbound stream closed before AgentTask arrived");
    };
    let payload = msg.payload.expect("inbound payload is present");
    let agent_inbound::Payload::Task(task) = payload else {
        panic!("expected AgentInbound::Task, got: {:?}", payload);
    };

    assert_eq!(task.session_id, session_id, "session_id round-trips");
    assert_eq!(task.agent_id, agent_id, "agent_id round-trips");
    assert_eq!(task.prompt, "hello from test", "prompt round-trips");

    drop(out_tx);
}

/// Additional coverage for the SDK path: the test drives
/// `scarllet_sdk::agent::AgentSession::connect` — the function the
/// default agent binary calls at startup — against the same in-process
/// orchestrator. Validates that the SDK's preemptive-register send
/// (defense-in-depth fix in the bug-001 plan) unblocks the handshake
/// even when combined with the server-side spawn-and-return fix.
///
/// We set the `SCARLLET_*` env vars this test owns and then restore
/// them on exit to keep the test binary's env reasonably clean.
#[tokio::test]
async fn sdk_agent_session_connect_delivers_task() {
    let server = boot_in_proc_server().await;

    let mut create_client = server.client.clone();
    let session_id = timeout(
        HANDSHAKE_TIMEOUT,
        create_client.create_session(CreateSessionRequest {}),
    )
    .await
    .expect("CreateSession did not complete within 2s")
    .expect("CreateSession rpc succeeds")
    .into_inner()
    .session_id;

    let agent_id = seed_pending_dispatch(
        &server.sessions,
        &session_id,
        "hello from sdk connect test",
    )
    .await;

    // Scope env-var set/restore so we don't interfere with other tests
    // in the same binary (they don't touch these vars, but the guard
    // keeps this behaviour future-proof).
    //
    // SAFETY: `std::env::set_var` / `remove_var` are marked unsafe on
    // nightly (cargo clippy with MSRV >= 1.82), but on the stable
    // toolchain used by this workspace they are safe fns. Keep the
    // calls conservative — set once, remove on drop, nothing between.
    let _guard = EnvGuard::new(&[
        ("SCARLLET_CORE_ADDR", &server.bound_addr),
        ("SCARLLET_SESSION_ID", &session_id),
        ("SCARLLET_AGENT_ID", &agent_id),
        ("SCARLLET_PARENT_ID", &session_id),
        ("SCARLLET_AGENT_MODULE", "default"),
    ]);

    let mut session = timeout(
        HANDSHAKE_TIMEOUT,
        scarllet_sdk::agent::AgentSession::connect(),
    )
    .await
    .expect("AgentSession::connect did not complete within 2s — handshake deadlock regressed")
    .expect("AgentSession::connect succeeds");

    let task = timeout(HANDSHAKE_TIMEOUT, session.next_task())
        .await
        .expect("AgentSession::next_task did not complete within 2s")
        .expect("inbound stream yielded an AgentTask");

    assert_eq!(task.session_id, session_id);
    assert_eq!(task.agent_id, agent_id);
    assert_eq!(task.prompt, "hello from sdk connect test");
}

/// Explicitly exercises the **server-side** fix: opens the bidi RPC
/// without any message pre-queued, then pushes `Register` only after
/// `client.agent_stream(...).await` has resolved. Against the pre-fix
/// server handler (which awaited `incoming.message()` before returning
/// `Response`) this sequence deadlocks because tonic's client waits
/// for response headers before it can flush any DATA — and the server
/// doesn't send response headers until the handler returns.
///
/// Passes post-fix because the handler now spawns the per-stream task
/// and returns `Response` immediately, letting tonic flush headers
/// back to the client so the subsequent `out_tx.send(...)` is picked
/// up by the spawned task's first `incoming.message().await`.
#[tokio::test]
async fn server_handler_returns_response_before_first_message() {
    let mut server = boot_in_proc_server().await;

    let session_id = timeout(
        HANDSHAKE_TIMEOUT,
        server.client.create_session(CreateSessionRequest {}),
    )
    .await
    .expect("CreateSession did not complete within 2s")
    .expect("CreateSession rpc succeeds")
    .into_inner()
    .session_id;

    let agent_id = seed_pending_dispatch(
        &server.sessions,
        &session_id,
        "server-fix verification prompt",
    )
    .await;

    let (out_tx, out_rx) = mpsc::channel::<AgentOutbound>(64);
    let outgoing = ReceiverStream::new(out_rx);

    // Deliberately call `agent_stream` with an EMPTY outbound channel.
    // The server must return `Response` before any client message
    // arrives — otherwise this await never resolves (deadlock).
    let response = timeout(HANDSHAKE_TIMEOUT, server.client.agent_stream(outgoing))
        .await
        .expect(
            "AgentStream RPC did not return Response within 2s with an empty outbound stream \
             — server-side bidi handshake deadlock regressed",
        )
        .expect("AgentStream rpc succeeds");

    let mut inbound = response.into_inner();

    // Only NOW push the register frame. tonic's HTTP/2 client already
    // has the response headers; it can flush DATA right away.
    out_tx
        .send(AgentOutbound {
            payload: Some(agent_outbound::Payload::Register(AgentRegister {
                session_id: session_id.clone(),
                agent_id: agent_id.clone(),
                agent_module: "default".into(),
                parent_id: session_id.clone(),
            })),
        })
        .await
        .expect("push register onto outbound channel");

    let first = timeout(HANDSHAKE_TIMEOUT, inbound.message())
        .await
        .expect("no AgentInbound within 2s after Register — handshake regressed")
        .expect("inbound stream yields Ok");

    let msg = first.expect("inbound stream closed before AgentTask arrived");
    let agent_inbound::Payload::Task(task) = msg.payload.expect("inbound payload present") else {
        panic!("expected AgentInbound::Task");
    };
    assert_eq!(task.agent_id, agent_id);
    assert_eq!(task.prompt, "server-fix verification prompt");

    drop(out_tx);
}

/// Restores every env var listed in `keys` on drop. Used so the SDK
/// test's `std::env::set_var` calls don't leak into the rest of the
/// test binary's process state.
struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn new(entries: &[(&str, &str)]) -> Self {
        let mut saved = Vec::with_capacity(entries.len());
        for (k, v) in entries {
            saved.push(((*k).to_string(), std::env::var(k).ok()));
            std::env::set_var(k, v);
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.saved {
            match prev {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}
