//! Bidi `AgentStream` handler.
//!
//! Decodes the per-frame `AgentOutbound` payloads (register / create
//! node / update node / turn_finished / failure) and applies the
//! corresponding mutation to the owning session, broadcasting the
//! matching diff after each accepted mutation.

use std::sync::Arc;

use scarllet_proto::proto::{
    agent_inbound, agent_outbound, node, AgentInbound, AgentOutbound, AgentRegister, AgentTask,
    CancelNow, CreateNode, ErrorPayload, Node, NodeKind, ResultPayload, UpdateNode,
};
use tokio::sync::{mpsc, RwLock};
use tonic::{Status, Streaming};
use tracing::{info, warn};
use uuid::Uuid;

use crate::registry::ModuleRegistry;
use crate::session::{diff, Session, SessionStatus};

use super::{routing, AgentRecord};

/// Grace period between sending `CancelNow` and force-killing the child
/// process during the AC-8.4 invariant cascade. Short because this path
/// fires on a protocol violation that we want to clear quickly.
pub(crate) const AC_8_4_KILL_GRACE_MS: u64 = 500;

/// Grace period between sending `CancelNow` and force-killing the child
/// process during the session-wide `StopSession` / `DestroySession`
/// cascade. Longer than [`AC_8_4_KILL_GRACE_MS`] so well-behaved SDKs
/// have time to finish their in-flight LLM / tool calls and close the
/// stream on their own before the OS kill arrives (effort 06).
pub(crate) const CASCADE_KILL_GRACE_MS: u64 = 2000;

/// Error message recorded on per-agent `Error` nodes and passed to
/// `sub_agent_waiters` when `cascade_cancel` tears agents down. The exact
/// string is asserted by the TUI + the unit tests, so changing it is a
/// user-visible event.
pub(crate) const CANCEL_REASON: &str = "cancelled by user";

/// Shared dependencies the bidi `AgentStream` handler needs to validate
/// register messages and trigger re-dispatch of the queue when a turn ends.
pub struct StreamDeps {
    pub session: Arc<RwLock<Session>>,
    pub registry: Arc<RwLock<ModuleRegistry>>,
    pub core_addr: String,
}

/// Drives the bidirectional `AgentStream` for one connected agent process,
/// using a `Register` message that was peeked off the stream by the gRPC
/// service layer. Subsequent messages are read from `incoming` until the
/// stream closes.
pub async fn run_with_register(
    register: AgentRegister,
    mut incoming: Streaming<AgentOutbound>,
    outgoing_tx: mpsc::Sender<Result<AgentInbound, Status>>,
    deps: StreamDeps,
) {
    let agent_id = match handle_register(&deps, &outgoing_tx, register).await {
        Ok(id) => id,
        Err(status) => {
            let _ = outgoing_tx.send(Err(status)).await;
            return;
        }
    };

    drive_main_loop(&deps, &mut incoming, &agent_id).await;

    handle_disconnect(&deps, &agent_id).await;
}

/// Validates the register, inserts the [`AgentRecord`], and immediately
/// pushes the queued [`AgentTask`] back to the agent. Returns the agent's
/// id on success or a `Status` describing why the register was rejected
/// so the caller can push it to the client as a clean gRPC error.
async fn handle_register(
    deps: &StreamDeps,
    outgoing_tx: &mpsc::Sender<Result<AgentInbound, Status>>,
    register: AgentRegister,
) -> Result<String, Status> {
    let agent_id = register.agent_id.clone();

    let mut session = deps.session.write().await;

    if register.session_id != session.id {
        warn!(
            "AgentRegister session_id '{}' does not match session '{}'; refusing",
            register.session_id, session.id
        );
        return Err(Status::failed_precondition(format!(
            "AgentRegister session_id '{}' does not match owning session '{}'",
            register.session_id, session.id
        )));
    }

    let Some(pending) = session.pending_dispatch.remove(&agent_id) else {
        warn!(
            "AgentRegister for unknown agent_id '{agent_id}' (no pending dispatch); refusing"
        );
        return Err(Status::failed_precondition(format!(
            "no pending dispatch for agent_id '{agent_id}'"
        )));
    };

    let agent_node_id = agent_id.clone();
    let record = AgentRecord {
        agent_id: agent_id.clone(),
        agent_module: register.agent_module.clone(),
        parent_id: register.parent_id.clone(),
        pid: pending.pid,
        tx: outgoing_tx.clone(),
        agent_node_id: agent_node_id.clone(),
    };

    let session_id = session.id.clone();
    session.agents.register(&session_id, record);
    session.broadcast(diff::agent_registered(
        agent_id.clone(),
        register.agent_module.clone(),
        register.parent_id.clone(),
        agent_node_id.clone(),
    ));

    let task = AgentInbound {
        payload: Some(agent_inbound::Payload::Task(AgentTask {
            session_id,
            agent_id: agent_id.clone(),
            parent_id: register.parent_id,
            prompt: pending.prompt.text,
            working_directory: pending.prompt.working_directory,
        })),
    };
    drop(session);

    if outgoing_tx.send(Ok(task)).await.is_err() {
        warn!("AgentStream receiver dropped before AgentTask could be delivered");
        return Err(Status::cancelled(
            "AgentStream receiver dropped before AgentTask could be delivered",
        ));
    }

    info!("Agent '{agent_id}' registered");
    Ok(agent_id)
}

/// Loops over `AgentOutbound` messages until the stream closes.
async fn drive_main_loop(
    deps: &StreamDeps,
    incoming: &mut Streaming<AgentOutbound>,
    agent_id: &str,
) {
    while let Ok(Some(msg)) = incoming.message().await {
        let Some(payload) = msg.payload else {
            continue;
        };
        match payload {
            agent_outbound::Payload::Register(_) => {
                warn!("AgentStream sent duplicate Register; ignoring");
            }
            agent_outbound::Payload::CreateNode(create) => {
                handle_create_node(deps, agent_id, create).await;
            }
            agent_outbound::Payload::UpdateNode(update) => {
                handle_update_node(deps, agent_id, update).await;
            }
            agent_outbound::Payload::TurnFinished(turn) => {
                handle_turn_finished(deps, agent_id, &turn.finish_reason).await;
                return;
            }
            agent_outbound::Payload::Failure(f) => {
                handle_failure(deps, agent_id, &f.message).await;
                return;
            }
        }
    }
}

/// Validates and inserts a node sent by the agent.
///
/// Effort 01 only needs `Result` + `Error` nodes parented onto the agent's
/// own `Agent` node — `Agent` nodes themselves are created by core. Other
/// node kinds will land in efforts 02 / 03 / 06 / 07.
async fn handle_create_node(deps: &StreamDeps, agent_id: &str, create: CreateNode) {
    let Some(node) = create.node else {
        warn!("CreateNode payload missing node; ignoring");
        return;
    };

    let kind = NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified);
    if matches!(kind, NodeKind::Agent) {
        warn!("Agents are forbidden from creating Agent nodes; rejecting CreateNode");
        return;
    }

    let mut session = deps.session.write().await;

    let agent_node_id = match session.agents.get(agent_id) {
        Some(rec) => rec.agent_node_id.clone(),
        None => {
            warn!("CreateNode from unknown agent '{agent_id}'; rejecting");
            return;
        }
    };

    if !node_parent_belongs_to_agent(&node, &agent_node_id) {
        warn!(
            "CreateNode parent_id does not target the agent's Agent node; rejecting (kind {:?})",
            kind
        );
        return;
    }

    let stamped = stamp_node_ids_and_times(node, &agent_node_id);
    let stored = match session.nodes.create(stamped) {
        Ok(stored) => stored.clone(),
        Err(err) => {
            warn!("CreateNode rejected by NodeStore invariants: {err:?}");
            return;
        }
    };
    session.broadcast(diff::node_created(stored));
}

/// Validates and applies an [`UpdateNode`] patch sent by the agent.
///
/// Rejects updates that target an unknown node, that the agent does not
/// own (subtree root must equal the agent's `agent_node_id`), or that try
/// to mutate the `Agent` node itself — Agent payloads are core-managed.
async fn handle_update_node(deps: &StreamDeps, agent_id: &str, update: UpdateNode) {
    let UpdateNode { node_id, patch } = update;
    if node_id.is_empty() {
        warn!("UpdateNode missing node_id; rejecting");
        return;
    }
    let Some(patch) = patch else {
        warn!("UpdateNode missing patch; rejecting");
        return;
    };

    let mut session = deps.session.write().await;

    let agent_node_id = match session.agents.get(agent_id) {
        Some(rec) => rec.agent_node_id.clone(),
        None => {
            warn!("UpdateNode from unknown agent '{agent_id}'; rejecting");
            return;
        }
    };

    let Some(target) = session.nodes.get(&node_id) else {
        warn!("UpdateNode targets unknown node '{node_id}'; rejecting");
        return;
    };
    let target_kind = NodeKind::try_from(target.kind).unwrap_or(NodeKind::Unspecified);
    if matches!(target_kind, NodeKind::Agent) {
        warn!(
            "UpdateNode targeting Agent node '{node_id}' rejected — Agent payloads are core-managed",
        );
        return;
    }

    if !node_owned_by_agent(&session, &node_id, &agent_node_id) {
        warn!(
            "UpdateNode targeting '{node_id}' rejected — node not owned by agent '{agent_id}'",
        );
        return;
    }

    let updated_at = now_secs();
    if let Err(err) = session.nodes.update(&node_id, patch.clone(), updated_at) {
        warn!("UpdateNode rejected by NodeStore invariants: {err:?}");
        return;
    }
    diff::broadcast_node_updated(&mut session, node_id, patch, updated_at);
}

/// Walks the parent chain of `node_id` and returns `true` when the chain
/// terminates on `agent_node_id` (either by matching directly or by being
/// itself the agent node).
fn node_owned_by_agent(session: &Session, node_id: &str, agent_node_id: &str) -> bool {
    if node_id == agent_node_id {
        return true;
    }
    let mut current = node_id;
    loop {
        let Some(node) = session.nodes.get(current) else {
            return false;
        };
        match node.parent_id.as_deref() {
            Some(parent) if parent == agent_node_id => return true,
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// Returns `true` if the node's parent_id is either the agent's own Agent
/// node id, or `None` (top-level) for kinds the parent rules allow that.
fn node_parent_belongs_to_agent(node: &Node, agent_node_id: &str) -> bool {
    match node.parent_id.as_deref() {
        Some(parent) => parent == agent_node_id,
        None => matches!(
            NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified),
            NodeKind::Error
        ),
    }
}

/// Fills in `id` / `created_at` / `updated_at` / `parent_id` for nodes the
/// agent submitted with blank or partially-blank values.
fn stamp_node_ids_and_times(mut node: Node, agent_node_id: &str) -> Node {
    if node.id.is_empty() {
        node.id = Uuid::new_v4().to_string();
    }
    let now = now_secs();
    if node.created_at == 0 {
        node.created_at = now;
    }
    if node.updated_at == 0 {
        node.updated_at = now;
    }
    if node.parent_id.is_none() {
        let kind = NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified);
        if !matches!(kind, NodeKind::Error) {
            node.parent_id = Some(agent_node_id.to_string());
        }
    }
    node
}

/// Outcome carried out of the synchronous [`process_turn_finished`] core
/// so the async wrapper knows whether to re-run queue dispatch (main
/// agent only) and which PIDs need the post-grace kill (AC-8.4 cascade).
#[derive(Debug, Default)]
pub(crate) struct TurnFinishedOutcome {
    /// Main agent finished cleanly — drain next queued prompt.
    pub dispatch_next: bool,
    /// PIDs the invariant-violation path wants force-killed after grace.
    pub pids_to_kill: Vec<u32>,
}

/// Handles `TurnFinished`: branches on whether this agent is a registered
/// sub-agent (fire its waiter) or a main agent (enforce AC-8.4 invariant
/// + deregister + re-dispatch).
async fn handle_turn_finished(deps: &StreamDeps, agent_id: &str, finish_reason: &str) {
    let outcome = {
        let mut session = deps.session.write().await;
        process_turn_finished(&mut session, agent_id, finish_reason)
    };

    for pid in outcome.pids_to_kill {
        schedule_pid_kill(pid, AC_8_4_KILL_GRACE_MS);
    }

    if outcome.dispatch_next {
        let mut session = deps.session.write().await;
        routing::try_dispatch_main(&mut session, &deps.registry, &deps.core_addr).await;
    }
}

/// Synchronous core of [`handle_turn_finished`]. Exposed to unit tests so
/// the AC-8.4 invariant can be exercised without plumbing a gRPC stream.
pub(crate) fn process_turn_finished(
    session: &mut Session,
    agent_id: &str,
    _finish_reason: &str,
) -> TurnFinishedOutcome {
    if session.agents.has_sub_agent_waiter(agent_id) {
        finish_sub_agent(session, agent_id);
        return TurnFinishedOutcome::default();
    }

    if session.agents.any_descendant_running(agent_id, &session.nodes) {
        let pids = enforce_ac_8_4_invariant(session, agent_id);
        return TurnFinishedOutcome {
            dispatch_next: false,
            pids_to_kill: pids,
        };
    }

    mark_agent_status(session, agent_id, "finished");
    if session.agents.deregister(agent_id).is_some() {
        session.broadcast(diff::agent_unregistered(agent_id.to_string()));
    }
    TurnFinishedOutcome {
        dispatch_next: true,
        pids_to_kill: Vec::new(),
    }
}

/// Completes a sub-agent's `TurnFinished`: marks the Agent node finished,
/// finds the Result node in the sub-agent's subtree, fires the waiter with
/// the [`ResultPayload`], and deregisters the agent.
fn finish_sub_agent(session: &mut Session, agent_id: &str) {
    let agent_node_id = match session.agents.get(agent_id) {
        Some(rec) => rec.agent_node_id.clone(),
        None => agent_id.to_string(),
    };
    let result_payload = find_latest_result_payload(&session.nodes, &agent_node_id);

    mark_agent_status(session, agent_id, "finished");

    if let Some(tx) = session.agents.take_sub_agent_waiter(agent_id) {
        let send_result = match result_payload {
            Some(payload) => Ok(payload),
            None => Err("sub-agent finished without emitting a Result node".to_string()),
        };
        let _ = tx.send(send_result);
    }

    if session.agents.deregister(agent_id).is_some() {
        session.broadcast(diff::agent_unregistered(agent_id.to_string()));
    }
}

/// Finds the most-recently-created `Result` node parented to
/// `agent_node_id` and returns its [`ResultPayload`] clone.
fn find_latest_result_payload(
    nodes: &crate::session::nodes::NodeStore,
    agent_node_id: &str,
) -> Option<ResultPayload> {
    let mut found: Option<ResultPayload> = None;
    for node in nodes.all() {
        if node.parent_id.as_deref() != Some(agent_node_id) {
            continue;
        }
        let kind = NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified);
        if !matches!(kind, NodeKind::Result) {
            continue;
        }
        if let Some(node::Payload::Result(payload)) = node.payload.as_ref() {
            found = Some(payload.clone());
        }
    }
    found
}

/// Cascade-cancels the parent agent and every descendant sub-agent when
/// the parent tried to emit `TurnFinished` while descendants were still
/// running (AC-8.4). Returns the PIDs the caller should force-kill after
/// the grace period.
fn enforce_ac_8_4_invariant(session: &mut Session, parent_agent_id: &str) -> Vec<u32> {
    let descendants = session
        .agents
        .descendant_agent_ids(parent_agent_id, &session.nodes);

    let invariant_message = format!(
        "invariant violation: agent '{parent_agent_id}' tried to finish with running sub-agents"
    );
    emit_top_level_error(session, &invariant_message);

    let mut pids: Vec<u32> = Vec::new();
    // Sub-agents first: their waiters need to fire with the same cancel error
    // so the parent's InvokeTool call unblocks while we're still tearing things down.
    let mut cancel_targets: Vec<String> = descendants.clone();
    cancel_targets.push(parent_agent_id.to_string());

    for agent_id in &cancel_targets {
        if let Some(rec) = session.agents.get(agent_id) {
            let _ = rec.tx.try_send(Ok(AgentInbound {
                payload: Some(agent_inbound::Payload::Cancel(CancelNow {})),
            }));
            if let Some(pid) = rec.pid {
                pids.push(pid);
            }
        }
    }

    // Fire any registered sub-agent waiters so parked InvokeTool calls return.
    for agent_id in &descendants {
        if let Some(tx) = session.agents.take_sub_agent_waiter(agent_id) {
            let _ = tx.send(Err(invariant_message.clone()));
        }
    }

    // Mark every affected Agent node `failed` + deregister.
    for agent_id in &cancel_targets {
        mark_agent_status(session, agent_id, "failed");
        if session.agents.deregister(agent_id).is_some() {
            session.broadcast(diff::agent_unregistered(agent_id.clone()));
        }
    }

    pids
}

/// Emits a top-level `Error` node (no `parent_id`) and broadcasts it.
fn emit_top_level_error(session: &mut Session, message: &str) {
    let id = Uuid::new_v4().to_string();
    let now = now_secs();
    let node = Node {
        id,
        parent_id: None,
        kind: NodeKind::Error as i32,
        created_at: now,
        updated_at: now,
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "core".into(),
            message: message.into(),
        })),
    };
    match session.nodes.create(node) {
        Ok(stored) => {
            let cloned = stored.clone();
            session.broadcast(diff::node_created(cloned));
        }
        Err(err) => {
            warn!("Failed to emit top-level error node: {err:?}");
        }
    }
}

/// Schedules a fire-and-forget task that waits `grace_ms` and then
/// force-kills `pid` via the OS-specific kill command. Best-effort — if
/// the child already exited cleanly on `CancelNow` the kill is a no-op.
///
/// `grace_ms` is a parameter rather than a constant so the AC-8.4
/// invariant cascade (short grace) and the `StopSession` cascade (longer
/// grace, per effort 06's `CANCEL_GRACE = 2 s`) can reuse the same
/// kill primitive.
pub(crate) fn schedule_pid_kill(pid: u32, grace_ms: u64) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(grace_ms)).await;
        kill_pid_best_effort(pid);
    });
}

/// Cross-platform "kill this PID" helper used by the cascade paths.
/// Uses `taskkill` on Windows and `kill` on Unix; failures are logged and
/// swallowed because the cascade is a last-resort safety net.
pub(crate) fn kill_pid_best_effort(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Err(e) = status {
            warn!("taskkill /F /PID {pid} failed: {e}");
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let status = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Err(e) = status {
            warn!("kill -KILL {pid} failed: {e}");
        }
    }
}

/// Handles a recoverable `AgentFailure` — emits an Error node parented on
/// the agent's Agent node and deregisters. For sub-agents the waiter is
/// fired with `Err(message)` so the parent's `InvokeTool` call unblocks
/// with a structured failure. For main agents the session transitions to
/// `Paused` (AC-3.4) so `try_dispatch_main` short-circuits until the
/// user recovers via `StopSession`.
async fn handle_failure(deps: &StreamDeps, agent_id: &str, message: &str) {
    let mut session = deps.session.write().await;
    apply_agent_termination(&mut session, agent_id, message);
}

/// Cleanup when the bidi stream closes without a clean `TurnFinished`.
/// Same lifecycle as [`handle_failure`] but with a canonical disconnect
/// message so TUIs can distinguish the two failure modes.
async fn handle_disconnect(deps: &StreamDeps, agent_id: &str) {
    let mut session = deps.session.write().await;
    if session.agents.get(agent_id).is_none() {
        // Register never succeeded or a prior handler already cleaned up.
        return;
    }
    apply_agent_termination(
        &mut session,
        agent_id,
        "agent disconnected unexpectedly",
    );
}

/// Shared cleanup for both `AgentFailure` and disconnect-before-
/// `TurnFinished`. Emits the `Error` node + agent-node status flip,
/// fires any sub-agent waiter, deregisters the record, and — for main
/// agents — transitions the session to `Paused` (AC-3.4).
///
/// Does **not** re-run `try_dispatch_main`: when the session goes to
/// `Paused` the routing layer already short-circuits on status, and the
/// sub-agent path leaves the parent's turn in charge of continuing.
fn apply_agent_termination(session: &mut Session, agent_id: &str, message: &str) {
    let is_sub = session.agents.has_sub_agent_waiter(agent_id);
    let Some(rec) = session.agents.get(agent_id) else {
        warn!("AgentFailure/disconnect from unknown agent '{agent_id}'");
        return;
    };
    let agent_node_id = rec.agent_node_id.clone();

    emit_error_under_agent(session, &agent_node_id, message);
    mark_agent_status(session, agent_id, "failed");

    if is_sub {
        if let Some(tx) = session.agents.take_sub_agent_waiter(agent_id) {
            let _ = tx.send(Err(message.to_string()));
        }
    }

    if session.agents.deregister(agent_id).is_some() {
        session.broadcast(diff::agent_unregistered(agent_id.to_string()));
    }

    if !is_sub && session.set_status(SessionStatus::Paused) {
        diff::broadcast_status_changed(session);
    }
}

/// Patches the Agent node's `status` field and broadcasts the matching
/// `NodeUpdated` diff. Silently no-ops when the agent record has already
/// been cleared or the Agent node is missing (defensive; both should hold
/// in normal operation).
fn mark_agent_status(session: &mut Session, agent_id: &str, status: &str) {
    let agent_node_id = match session.agents.get(agent_id) {
        Some(rec) => rec.agent_node_id.clone(),
        None => return,
    };
    let patch = scarllet_proto::proto::NodePatch {
        agent_status: Some(status.to_string()),
        ..Default::default()
    };
    let updated_at = now_secs();
    if session
        .nodes
        .update(&agent_node_id, patch.clone(), updated_at)
        .is_ok()
    {
        diff::broadcast_node_updated(session, agent_node_id, patch, updated_at);
    }
}

/// Session-wide cascade used by `StopSession` and `DestroySession`.
///
/// Walks every registered agent in reverse topological order (sub-agents
/// first, main agent last), sends `CancelNow`, schedules a
/// [`CASCADE_KILL_GRACE_MS`] PID kill as a safety net, fires any waiting
/// `sub_agent_waiters` with `Err(reason)` so parent `InvokeTool` calls
/// unblock, patches each Agent node's `status` to `"failed"`, creates an
/// `Error` child node parented to the Agent node carrying `reason`, and
/// deregisters the record (with an `AgentUnregistered` broadcast).
///
/// Does **not** touch `session.queue` or `session.status` — callers are
/// responsible for those (see `session_rpc::stop_session` +
/// `session_rpc::destroy_session_inner`).
pub(crate) fn cascade_cancel(session: &mut Session, reason: &str) {
    let sorted_ids = agent_ids_leaves_first(session);

    for agent_id in &sorted_ids {
        let Some(rec) = session.agents.get(agent_id) else {
            continue;
        };
        let _ = rec.tx.try_send(Ok(AgentInbound {
            payload: Some(agent_inbound::Payload::Cancel(CancelNow {})),
        }));
        if let Some(pid) = rec.pid {
            schedule_pid_kill(pid, CASCADE_KILL_GRACE_MS);
        }
    }

    for agent_id in &sorted_ids {
        if let Some(tx) = session.agents.take_sub_agent_waiter(agent_id) {
            let _ = tx.send(Err(reason.to_string()));
        }
    }

    for agent_id in &sorted_ids {
        let agent_node_id = match session.agents.get(agent_id) {
            Some(rec) => rec.agent_node_id.clone(),
            None => continue,
        };
        mark_agent_status(session, agent_id, "failed");
        emit_error_under_agent(session, &agent_node_id, reason);
        if session.agents.deregister(agent_id).is_some() {
            session.broadcast(diff::agent_unregistered(agent_id.to_string()));
        }
    }
}

/// Topologically sorts every registered agent id by its parent-chain
/// depth, leaves (deepest sub-agents) first, main agent (depth = 0) last.
///
/// Depth is derived from `AgentRecord.parent_id`: agents whose parent is
/// the owning session have depth 0; each subsequent hop up through the
/// registry adds one. Unknown parents (ids that are not themselves
/// registered) terminate the walk — so sub-agents whose parent has
/// already been deregistered still compute a finite depth.
fn agent_ids_leaves_first(session: &Session) -> Vec<String> {
    let mut with_depth: Vec<(String, usize)> = session
        .agents
        .iter_records()
        .map(|rec| (rec.agent_id.clone(), depth_of(session, &rec.agent_id)))
        .collect();
    // Deeper first; stable within a depth band so iteration order is
    // deterministic for the tests and TUI logs.
    with_depth.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    with_depth.into_iter().map(|(id, _)| id).collect()
}

/// Counts the number of registered-agent hops from `agent_id` back up
/// toward the session root. Used by [`agent_ids_leaves_first`] to pick
/// a safe reverse-topological order.
fn depth_of(session: &Session, agent_id: &str) -> usize {
    let mut depth = 0usize;
    let mut current = agent_id.to_string();
    let mut seen = std::collections::HashSet::<String>::new();
    while let Some(rec) = session.agents.get(&current) {
        if !seen.insert(current.clone()) {
            break; // defensive: cycle (should never happen, but don't loop forever).
        }
        if rec.parent_id == session.id {
            return depth;
        }
        current = rec.parent_id.clone();
        depth += 1;
    }
    depth
}

/// Inserts an `Error` node parented on `agent_node_id` and broadcasts it.
fn emit_error_under_agent(session: &mut Session, agent_node_id: &str, message: &str) {
    let id = Uuid::new_v4().to_string();
    let now = now_secs();
    let node = Node {
        id,
        parent_id: Some(agent_node_id.to_string()),
        kind: NodeKind::Error as i32,
        created_at: now,
        updated_at: now,
        payload: Some(node::Payload::Error(ErrorPayload {
            source: "core".into(),
            message: message.into(),
        })),
    };
    let stored = match session.nodes.create(node) {
        Ok(stored) => stored.clone(),
        Err(err) => {
            warn!("Failed to emit error node under '{agent_node_id}': {err:?}");
            return;
        }
    };
    session.broadcast(diff::node_created(stored));
}

/// Returns the current Unix-epoch second count, saturating on errors.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
