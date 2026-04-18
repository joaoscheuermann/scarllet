//! gRPC connection task.
//!
//! Dials the core orchestrator, performs the `AttachSession` handshake,
//! and shuttles diffs / outbound commands between the TUI event loop
//! and the gRPC stream until either side closes.

use std::io;
use std::time::Duration;

use scarllet_proto::proto::orchestrator_client::OrchestratorClient;
use scarllet_proto::proto::*;
use scarllet_sdk::lockfile;
use tokio::sync::mpsc;

use crate::app::CoreCommand;

/// Establishes the gRPC connection, calls `AttachSession` to hydrate, and
/// shuttles diffs / commands between the TUI and the core orchestrator.
///
/// The function does not return until either side of the channels closes.
/// When `requested_session` is `Some(id)`, the TUI first attempts to
/// attach to that id; if core returns `NOT_FOUND` it falls back to the
/// auto-create path and surfaces a status message explaining the fallback
/// (AC-11.1 + effort 07 `--session` contract).
pub(crate) async fn connect_and_stream(
    diff_tx: mpsc::Sender<SessionDiff>,
    mut command_rx: mpsc::Receiver<CoreCommand>,
    requested_session: Option<String>,
) {
    let address = find_core_address().await;

    let Some(channel) = connect_to_core(&address).await else {
        return;
    };

    let mut client = OrchestratorClient::new(channel)
        .max_decoding_message_size(64 * 1024 * 1024)
        .max_encoding_message_size(64 * 1024 * 1024);

    let mut state = match initial_attach(&mut client, &diff_tx, requested_session).await {
        Some(s) => s,
        None => return,
    };

    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                let Some(cmd) = cmd else { return; };
                handle_command(&mut client, &mut state, &diff_tx, cmd).await;
            }
            diff = state.incoming.message() => {
                let Some(diff) = diff.ok().flatten() else { return; };
                if diff_tx.send(diff).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Holds the active `AttachSession` stream plus the bound session id.
struct AttachedState {
    incoming: tonic::Streaming<SessionDiff>,
    session_id: String,
}

/// Handles the first `AttachSession` call. When `requested_session` is
/// supplied, tries to attach to it; on `NOT_FOUND`, retries with
/// auto-create and then surfaces a synthetic top-level `Error` diff
/// explaining the fallback. The notice is queued **after** the initial
/// `Attached` diff so the TUI's `reset_with` does not wipe it.
async fn initial_attach(
    client: &mut OrchestratorClient<tonic::transport::Channel>,
    diff_tx: &mpsc::Sender<SessionDiff>,
    requested_session: Option<String>,
) -> Option<AttachedState> {
    let mut fallback_from: Option<String> = None;
    if let Some(id) = requested_session {
        match attach_with_id(client, diff_tx, Some(id.clone())).await {
            AttachResult::Attached(state) => return Some(*state),
            AttachResult::NotFound => {
                fallback_from = Some(id);
            }
            AttachResult::Failed => return None,
        }
    }
    let state = match attach_with_id(client, diff_tx, None).await {
        AttachResult::Attached(state) => *state,
        _ => return None,
    };
    if let Some(requested_id) = fallback_from {
        emit_fallback_notice(diff_tx, &requested_id).await;
    }
    Some(state)
}

/// Outcome of one `AttachSession` RPC dial. `Attached` is boxed so the
/// enum stays compact — the active stream owned by `AttachedState` is
/// much larger than the `NotFound` / `Failed` variants.
enum AttachResult {
    Attached(Box<AttachedState>),
    NotFound,
    Failed,
}

/// Opens a fresh `AttachSession` stream (no session id → core auto-creates)
/// and forwards the first `Attached` diff to the TUI.
async fn attach_with_id(
    client: &mut OrchestratorClient<tonic::transport::Channel>,
    diff_tx: &mpsc::Sender<SessionDiff>,
    session_id: Option<String>,
) -> AttachResult {
    let response = match client
        .attach_session(AttachSessionRequest { session_id })
        .await
    {
        Ok(r) => r,
        Err(status) if status.code() == tonic::Code::NotFound => {
            return AttachResult::NotFound;
        }
        Err(e) => {
            tracing::error!("AttachSession failed: {e}");
            return AttachResult::Failed;
        }
    };

    let mut incoming = response.into_inner();
    let Some(first) = incoming.message().await.ok().flatten() else {
        return AttachResult::Failed;
    };
    let Some(bound_id) = extract_session_id(&first) else {
        return AttachResult::Failed;
    };
    let _ = diff_tx.send(first).await;
    AttachResult::Attached(Box::new(AttachedState {
        incoming,
        session_id: bound_id,
    }))
}

/// Pushes a synthetic top-level `Error` diff into the TUI's event queue
/// so the user can see that the requested `--session <id>` did not exist
/// and that the TUI fell back to auto-creating a new session. The error
/// is created client-side — core never sees this node — because it is
/// purely a local status message, not a session-level error.
async fn emit_fallback_notice(diff_tx: &mpsc::Sender<SessionDiff>, requested_id: &str) {
    let node = Node {
        id: format!("tui-fallback-{requested_id}"),
        parent_id: None,
        kind: NodeKind::Error as i32,
        created_at: 0,
        updated_at: 0,
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "tui".into(),
            message: format!("session {requested_id} not found; started a new one"),
        })),
    };
    let _ = diff_tx
        .send(SessionDiff {
            payload: Some(session_diff::Payload::NodeCreated(NodeCreated {
                node: Some(node),
            })),
        })
        .await;
}

/// Pulls the session id out of an `Attached` diff for bookkeeping.
fn extract_session_id(diff: &SessionDiff) -> Option<String> {
    match diff.payload.as_ref()? {
        session_diff::Payload::Attached(att) => att.state.as_ref().map(|s| s.session_id.clone()),
        _ => None,
    }
}

/// Translates one [`CoreCommand`] into the matching RPC.
async fn handle_command(
    client: &mut OrchestratorClient<tonic::transport::Channel>,
    state: &mut AttachedState,
    diff_tx: &mpsc::Sender<SessionDiff>,
    cmd: CoreCommand,
) {
    match cmd {
        CoreCommand::SendPrompt { text, cwd } => {
            if let Err(e) = client
                .send_prompt(SendPromptRequest {
                    session_id: state.session_id.clone(),
                    text,
                    working_directory: cwd,
                })
                .await
            {
                tracing::warn!("SendPrompt RPC failed: {e}");
            }
        }
        CoreCommand::StopSession => {
            if let Err(e) = client
                .stop_session(StopSessionRequest {
                    session_id: state.session_id.clone(),
                })
                .await
            {
                tracing::warn!("StopSession RPC failed: {e}");
            }
        }
        CoreCommand::DestroyAndRecreate => {
            if let Err(e) = client
                .destroy_session(DestroySessionRequest {
                    session_id: state.session_id.clone(),
                })
                .await
            {
                tracing::warn!("DestroySession RPC failed: {e}");
            }
            if let AttachResult::Attached(new_state) = attach_with_id(client, diff_tx, None).await {
                *state = *new_state;
            }
        }
    }
}

/// Attempts to connect to the Core gRPC endpoint with retries.
async fn connect_to_core(address: &str) -> Option<tonic::transport::Channel> {
    for _ in 0..10 {
        let endpoint = format!("http://{address}");
        if let Ok(ep) = tonic::transport::Endpoint::from_shared(endpoint) {
            if let Ok(Ok(channel)) =
                tokio::time::timeout(Duration::from_secs(3), ep.connect()).await
            {
                return Some(channel);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

/// Reads the lockfile for a running Core address, spawning a new Core process if absent.
async fn find_core_address() -> String {
    loop {
        if let Ok(Some(lock)) = lockfile::read() {
            if lockfile::is_pid_alive(lock.pid) {
                return lock.address;
            }
            lockfile::remove();
        }

        let _ = spawn_core();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(Some(lock)) = lockfile::read() {
                if lockfile::is_pid_alive(lock.pid) {
                    return lock.address;
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Launches the Core binary as a detached background process.
fn spawn_core() -> io::Result<()> {
    let self_path = std::env::current_exe()?;
    let dir = self_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine binary dir"))?;
    let mut core_path = dir.join("core");
    if cfg!(windows) {
        core_path.set_extension("exe");
    }
    if !core_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Core binary not found at {}", core_path.display()),
        ));
    }
    std::process::Command::new(&core_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}
