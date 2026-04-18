//! TUI application state mirror.
//!
//! Holds the local node-graph mirror, input buffer, focus / scroll
//! state, and the outbound command channel used to issue `SendPrompt`,
//! `StopSession`, etc. All mutations happen through either
//! [`App::reset_with`] (on `Attached` / Ctrl-N) or the per-diff
//! apply helpers exercised from `events::handle_session_diff`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use scarllet_proto::proto::{node, ActiveProviderResponse, Node, NodeKind, NodePatch};
use tokio::sync::mpsc;

/// Minimum interval between environment refreshes (cwd, git info).
pub(crate) const ENV_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Number of visible characters the typewriter reveals per tick while
/// an Agent node is still `running`. Combined with the 50 ms streaming
/// tick this paces revealed text at ~600 chars/sec — matching the
/// pre-refactor animation. When the Agent transitions to `finished` or
/// `failed`, the renderer snaps to the full content so no backlog is
/// left dangling on screen.
pub(crate) const TYPEWRITER_CHARS_PER_TICK: usize = 30;

/// Lifecycle status mirroring `scarllet::SessionStatus` in the proto.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionStatus {
    Running,
    Paused,
}

impl SessionStatus {
    /// Parses the canonical wire string (`RUNNING` / `PAUSED`).
    pub(crate) fn from_wire(s: &str) -> Self {
        if s.eq_ignore_ascii_case("PAUSED") {
            Self::Paused
        } else {
            Self::Running
        }
    }
}

/// Which pane currently owns keyboard focus.
#[derive(Clone, PartialEq)]
pub(crate) enum Focus {
    Input,
    History,
}

/// Compact summary of a connected agent.
///
/// Stores the minimal set the TUI actually renders (`agent_id` keys the
/// `connected_agents` map; `is_streaming` gates the "streaming" status
/// bar indicator). Additional fields from the wire `AgentSummary` are
/// dropped on ingest since no current view consumes them.
#[derive(Clone)]
pub(crate) struct AgentSummary {
    pub agent_id: String,
}

/// Provider snapshot surfaced in the status bar (AC-9.1 / AC-9.2 — the
/// core takes this snapshot at session create and does not mutate it
/// during the session's lifetime). Populated from the `Attached` diff's
/// [`ActiveProviderResponse`], cleared on `Destroyed`, refreshed on
/// Ctrl-N.
#[derive(Clone, Default)]
pub(crate) struct ProviderInfo {
    pub name: String,
    pub model: String,
    pub reasoning_effort: String,
}

impl ProviderInfo {
    /// Lifts an `ActiveProviderResponse` wire type into the TUI's local
    /// snapshot. Returns `None` when the provider was not configured so
    /// the status bar can omit the segment entirely rather than
    /// rendering an empty slot. `provider_type` from the wire response
    /// is intentionally dropped — the TUI renders the human-facing
    /// `provider_name` and not the adapter kind.
    pub(crate) fn from_wire(resp: ActiveProviderResponse) -> Option<Self> {
        if !resp.configured {
            return None;
        }
        Some(Self {
            name: resp.provider_name,
            model: resp.model,
            reasoning_effort: resp.reasoning_effort,
        })
    }

    /// Builds the display string rendered in the status bar.
    ///
    /// Mirrors the pre-refactor format: provider name followed by a
    /// `  ·  ` separator and the model. If a reasoning effort is set it
    /// is appended after the model with another `·`. Returns `None`
    /// when every candidate field is empty so the render layer can skip
    /// the segment entirely.
    pub(crate) fn display_label(&self) -> Option<String> {
        let name = self.name.trim();
        let model = self.model.trim();
        let effort = self.reasoning_effort.trim();
        if name.is_empty() && model.is_empty() {
            return None;
        }
        let mut parts: Vec<&str> = Vec::new();
        if !name.is_empty() {
            parts.push(name);
        }
        if !model.is_empty() {
            parts.push(model);
        }
        if !effort.is_empty() {
            parts.push(effort);
        }
        Some(parts.join(" · "))
    }
}

/// Outbound commands the connection task issues to core.
///
/// The connection task receives these via an `mpsc::Receiver` and translates
/// each into the corresponding unary or stream RPC.
#[derive(Clone)]
pub(crate) enum CoreCommand {
    /// Submit a prompt for the current session.
    SendPrompt { text: String, cwd: String },
    /// Stop the current session (effort 06 fleshes this out).
    StopSession,
    /// Destroy the current session and attach to a fresh one (Ctrl-N).
    DestroyAndRecreate,
}

/// Top-level navigation state of the TUI.
pub(crate) enum Route {
    /// Waiting for the gRPC connection to the Core to be established.
    Connecting { tick: u64 },
    /// Interactive chat session is active.
    Chat,
}

/// Central application state shared across event handling and rendering.
///
/// Holds a thin mirror of the per-session node graph plus transient UI
/// state needed to draw a frame.
pub(crate) struct App {
    pub(crate) route: Route,
    /// Stable id of the currently attached session (None until first `Attached`).
    pub(crate) session_id: Option<String>,
    /// Lifecycle state mirrored from core.
    pub(crate) session_status: SessionStatus,
    /// Per-session node graph mirror keyed by node id.
    pub(crate) nodes: HashMap<String, Node>,
    /// Insertion order for stable rendering.
    pub(crate) node_order: Vec<String>,
    /// Length of the most recent queue snapshot from `QueueChanged`.
    /// Rendered as a count in the status bar; the individual prompt
    /// payloads are not surfaced in any current view.
    pub(crate) queue_len: usize,
    /// Connected agents keyed by agent_id.
    pub(crate) connected_agents: HashMap<String, AgentSummary>,
    /// Provider / model snapshot taken at session-create time. `None`
    /// until the first `Attached` diff; cleared on `Destroyed`.
    pub(crate) provider_info: Option<ProviderInfo>,
    pub(crate) input_state: crate::input::InputState,
    pub(crate) input_locked: bool,
    pub(crate) focus: Focus,
    pub(crate) wrap_width: u16,
    pub(crate) scroll_view_state: crate::widgets::ScrollViewState,
    pub(crate) focused_message_idx: Option<usize>,
    pub(crate) history_viewport_height: u16,
    pub(crate) tick: u64,
    pub(crate) stream_closed: bool,
    pub(crate) command_tx: mpsc::Sender<CoreCommand>,
    pub(crate) cwd: PathBuf,
    pub(crate) cwd_display: String,
    pub(crate) git_info: Option<crate::git_info::GitInfo>,
    pub(crate) last_env_refresh: Instant,
    /// `true` when `SCARLLET_DEBUG=true` at startup. Consumed by the
    /// render path to decide whether to show `Debug` nodes inline under
    /// their owning Agent (AC-6.2).
    pub(crate) debug_enabled: bool,
    /// Node ids of `spawn_sub_agent` Tool nodes the user has toggled into
    /// the "expanded" view (full nested subtree). Absence means the node
    /// renders in its default truncated / summary form per AC-11.5.
    pub(crate) expanded_tools: HashSet<String>,
    /// Per-top-level-Agent typewriter reveal state, keyed by the Agent
    /// node id. [`advance_tick`] is the only writer; the renderer reads
    /// the snapshot into a per-frame `chars_budget` and slices visible
    /// content (Thought / Result / Tool / Debug) up to that budget.
    pub(crate) reveal: HashMap<String, AgentReveal>,
}

/// Typewriter reveal state for one Agent top-level node.
///
/// Stores the number of visible characters currently unveiled across
/// the Agent's subtree in creation order. Held on [`App`] keyed by the
/// Agent node's id and advanced exclusively by
/// [`App::advance_tick`]. When the Agent's `status` is `running` the
/// counter grows by [`TYPEWRITER_CHARS_PER_TICK`] per tick (capped at
/// the total visible char count); when it transitions to `finished` or
/// `failed`, the renderer snaps the counter to the total so any
/// backlog flushes in one frame.
#[derive(Clone, Copy, Default)]
pub(crate) struct AgentReveal {
    /// Characters already revealed across the subtree: Thought content
    /// plus Result content plus Tool preview / header text plus Debug
    /// text when `debug_enabled`. `TokenUsage` never contributes.
    pub(crate) visible_chars: usize,
}

impl App {
    /// Initializes application state with the given outbound command channel,
    /// working directory, and debug flag.
    pub(crate) fn new(
        command_tx: mpsc::Sender<CoreCommand>,
        cwd: PathBuf,
        debug_enabled: bool,
    ) -> Self {
        let cwd_display = crate::git_info::abbreviate_home(&cwd);
        let git = crate::git_info::read_git_info(&cwd);
        Self {
            route: Route::Connecting { tick: 0 },
            session_id: None,
            session_status: SessionStatus::Running,
            nodes: HashMap::new(),
            node_order: Vec::new(),
            queue_len: 0,
            connected_agents: HashMap::new(),
            provider_info: None,
            input_state: crate::input::InputState::new(),
            input_locked: false,
            focus: Focus::Input,
            wrap_width: 80,
            scroll_view_state: crate::widgets::ScrollViewState::new(),
            focused_message_idx: None,
            history_viewport_height: 0,
            tick: 0,
            stream_closed: false,
            command_tx,
            cwd,
            cwd_display,
            git_info: git,
            last_env_refresh: Instant::now(),
            debug_enabled,
            expanded_tools: HashSet::new(),
            reveal: HashMap::new(),
        }
    }

    /// Replaces the in-memory mirror with a fresh hydration snapshot. Used
    /// by `Attached` and Ctrl-N (which destroys + re-attaches). The
    /// provider snapshot is taken at session-create on the core side and
    /// rides along in the same `Attached` diff (AC-9.1 / AC-9.2), so the
    /// status bar field is refreshed in lockstep with every other
    /// hydration field.
    pub(crate) fn reset_with(
        &mut self,
        session_id: String,
        status: SessionStatus,
        nodes: Vec<Node>,
        queue_len: usize,
        agents: Vec<AgentSummary>,
        provider_info: Option<ProviderInfo>,
    ) {
        self.session_id = Some(session_id);
        self.session_status = status;
        self.nodes.clear();
        self.node_order.clear();
        for n in nodes {
            self.node_order.push(n.id.clone());
            self.nodes.insert(n.id.clone(), n);
        }
        self.queue_len = queue_len;
        self.connected_agents = agents
            .into_iter()
            .map(|a| (a.agent_id.clone(), a))
            .collect();
        self.provider_info = provider_info;
        self.scroll_view_state = crate::widgets::ScrollViewState::new();
        self.focused_message_idx = None;
        self.input_locked = false;
        self.expanded_tools.clear();
        self.reveal.clear();
    }

    /// Inserts a freshly created node into the mirror, preserving order.
    pub(crate) fn insert_node(&mut self, node: Node) {
        if !self.nodes.contains_key(&node.id) {
            self.node_order.push(node.id.clone());
        }
        self.nodes.insert(node.id.clone(), node);
    }

    /// Applies a partial-patch update to the local node mirror using the
    /// same merge rules as core's `NodeStore::update`: `thought_content`
    /// and `result_content` APPEND to the existing payload string;
    /// everything else REPLACES the matching field. Silently ignores
    /// updates targeting unknown ids (a stale diff after Ctrl-N, etc.).
    pub(crate) fn apply_node_patch(&mut self, id: &str, patch: NodePatch, updated_at: u64) {
        let Some(node) = self.nodes.get_mut(id) else {
            return;
        };
        node.updated_at = updated_at;
        let Some(payload) = node.payload.as_mut() else {
            return;
        };
        match payload {
            node::Payload::Agent(agent) => {
                if let Some(status) = patch.agent_status {
                    agent.status = status;
                }
            }
            node::Payload::Thought(thought) => {
                if let Some(chunk) = patch.thought_content {
                    thought.content.push_str(&chunk);
                }
            }
            node::Payload::Tool(tool) => {
                if let Some(status) = patch.tool_status {
                    tool.status = status;
                }
                if let Some(duration) = patch.tool_duration_ms {
                    tool.duration_ms = duration;
                }
                if let Some(result_json) = patch.tool_result_json {
                    tool.result_json = result_json;
                }
            }
            node::Payload::Result(result) => {
                if let Some(chunk) = patch.result_content {
                    result.content.push_str(&chunk);
                }
                if let Some(reason) = patch.result_finish_reason {
                    result.finish_reason = reason;
                }
            }
            node::Payload::Error(error) => {
                if let Some(message) = patch.error_message {
                    error.message = message;
                }
            }
            node::Payload::TokenUsage(usage) => {
                if let Some(total) = patch.token_total {
                    usage.total_tokens = total;
                }
                if let Some(window) = patch.token_window {
                    usage.context_window = window;
                }
            }
            node::Payload::User(_) | node::Payload::Debug(_) => {}
        }
    }

    /// Iterates top-level nodes (no parent_id) in creation order.
    pub(crate) fn top_level_nodes(&self) -> impl Iterator<Item = &Node> {
        self.node_order
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|n| n.parent_id.is_none())
    }

    /// Returns every transitive descendant of `parent_id` (immediate
    /// children, grandchildren, …) in creation order. Used to build the
    /// nested sub-agent subtree view (AC-11.5) without duplicating the
    /// BFS in the widget layer.
    pub(crate) fn descendants_of(&self, parent_id: &str) -> Vec<&Node> {
        let mut set: HashSet<&str> = HashSet::new();
        set.insert(parent_id);
        let mut changed = true;
        while changed {
            changed = false;
            for id in &self.node_order {
                let Some(node) = self.nodes.get(id) else {
                    continue;
                };
                let Some(parent) = node.parent_id.as_deref() else {
                    continue;
                };
                if set.contains(parent) && !set.contains(id.as_str()) {
                    set.insert(id.as_str());
                    changed = true;
                }
            }
        }
        self.node_order
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .filter(|n| n.id != parent_id && set.contains(n.id.as_str()))
            .collect()
    }

    /// Toggles the expanded state of every `spawn_sub_agent` Tool node in
    /// the subtree of the top-level node at `idx`. Returns `true` if at
    /// least one toggle happened. Used by the history-focus Enter binding
    /// to provide AC-11.5's expand affordance.
    pub(crate) fn toggle_spawn_sub_agent_expand(&mut self, idx: usize) -> bool {
        let top_levels: Vec<String> = self.top_level_nodes().map(|n| n.id.clone()).collect();
        let Some(top_id) = top_levels.get(idx) else {
            return false;
        };
        let candidate_ids: Vec<String> = self
            .descendants_of(top_id)
            .into_iter()
            .filter(|n| {
                matches!(
                    NodeKind::try_from(n.kind).unwrap_or(NodeKind::Unspecified),
                    NodeKind::Tool
                ) && is_spawn_sub_agent_tool(n)
            })
            .map(|n| n.id.clone())
            .collect();
        if candidate_ids.is_empty() {
            return false;
        }
        let all_expanded = candidate_ids
            .iter()
            .all(|id| self.expanded_tools.contains(id));
        if all_expanded {
            for id in &candidate_ids {
                self.expanded_tools.remove(id);
            }
        } else {
            for id in candidate_ids {
                self.expanded_tools.insert(id);
            }
        }
        true
    }

    /// Re-reads the working directory and git info if enough time has elapsed.
    pub(crate) fn refresh_env(&mut self) {
        if self.last_env_refresh.elapsed() < ENV_REFRESH_INTERVAL {
            return;
        }
        self.last_env_refresh = Instant::now();
        self.cwd = std::env::current_dir().unwrap_or_default();
        self.cwd_display = crate::git_info::abbreviate_home(&self.cwd);
        self.git_info = crate::git_info::read_git_info(&self.cwd);
    }

    /// Advances the global tick counter, refreshes environment, and
    /// paces the per-Agent typewriter reveal.
    ///
    /// Walks every top-level `Agent` node and updates its
    /// [`AgentReveal`] entry:
    ///
    /// - `running` → `visible_chars = min(visible_chars + TYPEWRITER_CHARS_PER_TICK, total)`
    ///   where `total` is the sum of visible content chars across the
    ///   Agent's subtree (Thought / Result content + Debug messages
    ///   when `debug_enabled` + Error messages, recursing into nested
    ///   sub-agents). `TokenUsage` and Tool headers never contribute.
    /// - `finished` / `failed` → `visible_chars = total` (snap to end
    ///   so any backlog flushes in one frame).
    ///
    /// Entries for Agent nodes no longer present (e.g. after `reset_with`)
    /// are left in the map but become inert; they're pruned en-masse by
    /// `reset_with` / `Destroyed` to keep the map from growing boundlessly
    /// across sessions.
    pub(crate) fn advance_tick(&mut self) {
        self.tick += 1;
        self.refresh_env();
        if let Route::Connecting { ref mut tick } = self.route {
            *tick += 1;
        }
        self.advance_typewriter();
    }

    /// Shared implementation used by [`advance_tick`] and unit tests —
    /// walks every top-level Agent node and updates its reveal counter.
    pub(crate) fn advance_typewriter(&mut self) {
        let agent_ids: Vec<String> = self
            .top_level_nodes()
            .filter(|n| {
                matches!(
                    NodeKind::try_from(n.kind).unwrap_or(NodeKind::Unspecified),
                    NodeKind::Agent
                )
            })
            .map(|n| n.id.clone())
            .collect();

        for agent_id in agent_ids {
            let status = self
                .nodes
                .get(&agent_id)
                .and_then(|n| match n.payload.as_ref() {
                    Some(node::Payload::Agent(a)) => Some(a.status.as_str()),
                    _ => None,
                })
                .unwrap_or("running")
                .to_string();
            let total = self.visible_subtree_chars(&agent_id);
            let reveal = self.reveal.entry(agent_id).or_default();
            if matches!(status.as_str(), "finished" | "failed") {
                reveal.visible_chars = total;
                continue;
            }
            reveal.visible_chars = reveal
                .visible_chars
                .saturating_add(TYPEWRITER_CHARS_PER_TICK)
                .min(total);
        }
    }

    /// Sums the number of "revealable" content characters across the
    /// subtree rooted at `agent_id`, including nested sub-agent content.
    ///
    /// Must match exactly what the renderer will consume from
    /// `chars_budget` so the typewriter unveils smoothly: Thought +
    /// Result content, Error messages (at any depth), Debug messages
    /// when `debug_enabled`, and recurses into nested Agent nodes (sub-
    /// agents under `spawn_sub_agent` Tool nodes). `TokenUsage` and
    /// Tool headers contribute 0 — tools render instantly regardless
    /// of budget (matches pre-refactor behaviour).
    pub(crate) fn visible_subtree_chars(&self, agent_id: &str) -> usize {
        let mut total = 0usize;
        for node in self.descendants_of(agent_id) {
            total = total.saturating_add(content_chars_for(node, self.debug_enabled));
        }
        total
    }

    /// Returns the reveal counter for `agent_id`, defaulting to zero
    /// when no entry exists yet (e.g. the very first frame after the
    /// Agent node arrived but before `advance_tick` ran once).
    pub(crate) fn reveal_for(&self, agent_id: &str) -> AgentReveal {
        self.reveal.get(agent_id).copied().unwrap_or_default()
    }

    /// Returns true when the input field accepts edits (correct route, focused, unlocked).
    pub(crate) fn is_input_editable(&self) -> bool {
        self.focus == Focus::Input && !self.input_locked && matches!(self.route, Route::Chat)
    }

    /// Returns true when an agent is connected and processing a turn.
    pub(crate) fn is_streaming(&self) -> bool {
        !self.connected_agents.is_empty()
    }

    /// Returns the (total, window) tuple from the most recently-created
    /// `TokenUsage` node across the whole session, or `None` when no
    /// usage node has been observed yet. `TokenUsage` nodes do not
    /// render in the chat body (per effort 07) — the status bar uses
    /// this helper to surface the latest counter instead.
    pub(crate) fn latest_token_usage(&self) -> Option<(u32, u32)> {
        for id in self.node_order.iter().rev() {
            let Some(node) = self.nodes.get(id) else {
                continue;
            };
            if NodeKind::try_from(node.kind).unwrap_or(NodeKind::Unspecified)
                != NodeKind::TokenUsage
            {
                continue;
            }
            let Some(node::Payload::TokenUsage(u)) = node.payload.as_ref() else {
                continue;
            };
            return Some((u.total_tokens, u.context_window));
        }
        None
    }
}

/// `true` if `node` is a `Tool` node whose `tool_name` is the synthetic
/// `spawn_sub_agent` entry. Used for AC-11.5 rendering and the expand
/// keybinding.
pub(crate) fn is_spawn_sub_agent_tool(node: &Node) -> bool {
    let Some(node::Payload::Tool(payload)) = node.payload.as_ref() else {
        return false;
    };
    payload.tool_name == "spawn_sub_agent"
}

/// Number of revealable characters this node contributes to the
/// typewriter budget. Kept in sync with the consumption logic in
/// `widgets::chat_message::build_lines` so the reveal counter matches
/// the renderer frame-for-frame.
///
/// `Thought` / `Result` count their raw `content.chars().count()`;
/// `Error` counts the message text; `Debug` counts the message only
/// when `debug_enabled`. Everything else (User, Agent-header-itself,
/// Tool headers, TokenUsage) contributes zero.
pub(crate) fn content_chars_for(node: &Node, debug_enabled: bool) -> usize {
    match node.payload.as_ref() {
        Some(node::Payload::Thought(t)) => t.content.chars().count(),
        Some(node::Payload::Result(r)) => r.content.chars().count(),
        Some(node::Payload::Error(e)) => e.message.chars().count(),
        Some(node::Payload::Debug(d)) if debug_enabled => d.message.chars().count(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
