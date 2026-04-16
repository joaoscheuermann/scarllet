use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use scarllet_proto::proto::TuiMessage;

/// Characters revealed per tick during the typewriter animation.
pub(crate) const TYPEWRITER_CHARS_PER_TICK: usize = 30;

/// Minimum interval between environment refreshes (cwd, git info).
pub(crate) const ENV_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Which pane currently owns keyboard focus.
#[derive(Clone, PartialEq)]
pub(crate) enum Focus {
    Input,
    History,
}

/// Lifecycle state of a single tool invocation.
#[derive(Clone, PartialEq)]
pub(crate) enum ToolCallStatus {
    Running,
    Done,
    Failed,
}

/// Runtime data for a tool call tracked by the TUI.
///
/// Stored in `App::tool_calls` keyed by the call ID so the renderer can
/// display progress, duration, and results inline.
pub(crate) struct ToolCallData {
    #[allow(dead_code)]
    pub(crate) call_id: String,
    pub(crate) tool_name: String,
    pub(crate) arguments_preview: String,
    pub(crate) status: ToolCallStatus,
    pub(crate) started_at: Instant,
    pub(crate) duration_ms: u64,
    pub(crate) result: String,
}

/// A renderable content block within an agent message.
///
/// Agent responses are composed of interleaved thoughts, text, and
/// tool-call references that the chat widget renders differently.
pub(crate) enum DisplayBlock {
    /// Internal reasoning shown in a dimmed side-bar.
    Thought(String),
    /// Visible markdown text.
    Text(String),
    /// Reference to a tool call ID rendered as an inline card.
    ToolCallRef(String),
}

/// A single entry in the chat history.
///
/// Each variant carries the data needed for its visual representation
/// in the chat scroll view.
pub(crate) enum ChatEntry {
    /// Message typed by the user.
    User {
        text: String,
    },
    /// Streamed or completed response from an agent.
    Agent {
        name: String,
        task_id: String,
        blocks: Vec<DisplayBlock>,
        visible_chars: usize,
        done: bool,
    },
    /// Core debug log forwarded for on-screen display.
    Debug {
        source: String,
        level: String,
        message: String,
        timestamp: String,
    },
    /// System-level notification (connection status, errors).
    System {
        text: String,
    },
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
/// Holds the chat history, tool-call registry, input state, and all
/// transient UI state needed to draw a frame.
pub(crate) struct App {
    pub(crate) route: Route,
    pub(crate) messages: Vec<ChatEntry>,
    pub(crate) tool_calls: HashMap<String, ToolCallData>,
    pub(crate) input_state: crate::input::InputState,
    pub(crate) input_locked: bool,
    pub(crate) focus: Focus,
    pub(crate) wrap_width: u16,
    pub(crate) scroll_view_state: crate::widgets::ScrollViewState,
    pub(crate) focused_message_idx: Option<usize>,
    pub(crate) history_viewport_height: u16,
    pub(crate) tick: u64,
    pub(crate) stream_closed: bool,
    pub(crate) message_tx: mpsc::Sender<TuiMessage>,
    pub(crate) provider_name: String,
    pub(crate) model: String,
    pub(crate) reasoning_effort: String,
    pub(crate) cwd: PathBuf,
    pub(crate) cwd_display: String,
    pub(crate) git_info: Option<crate::git_info::GitInfo>,
    pub(crate) last_env_refresh: Instant,
    pub(crate) debug_enabled: bool,
    pub(crate) total_tokens: u32,
    pub(crate) context_window: u32,
}

/// Counts the total character length of all text and thought blocks.
pub(crate) fn total_block_chars(blocks: &[DisplayBlock]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            DisplayBlock::Thought(t) | DisplayBlock::Text(t) => t.chars().count(),
            DisplayBlock::ToolCallRef(_) => 0,
        })
        .sum()
}

impl App {
    /// Initializes application state with the given Core message channel,
    /// working directory, and debug flag.
    pub(crate) fn new(
        message_tx: mpsc::Sender<TuiMessage>,
        cwd: PathBuf,
        debug_enabled: bool,
    ) -> Self {
        let cwd_display = crate::git_info::abbreviate_home(&cwd);
        let git = crate::git_info::read_git_info(&cwd);
        Self {
            route: Route::Connecting { tick: 0 },
            messages: Vec::new(),
            tool_calls: HashMap::new(),
            input_state: crate::input::InputState::new(),
            input_locked: false,
            focus: Focus::Input,
            wrap_width: 80,
            scroll_view_state: crate::widgets::ScrollViewState::new(),
            focused_message_idx: None,
            history_viewport_height: 0,
            tick: 0,
            stream_closed: false,
            message_tx,
            provider_name: String::new(),
            model: String::new(),
            reasoning_effort: String::new(),
            cwd,
            cwd_display,
            git_info: git,
            last_env_refresh: Instant::now(),
            debug_enabled,
            total_tokens: 0,
            context_window: 0,
        }
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
    /// progresses the typewriter animation on all streaming agent messages.
    pub(crate) fn advance_tick(&mut self) {
        self.tick += 1;
        self.refresh_env();
        if let Route::Connecting { ref mut tick } = self.route {
            *tick += 1;
        }
        for entry in &mut self.messages {
            if let ChatEntry::Agent {
                blocks,
                visible_chars,
                done,
                ..
            } = entry
            {
                let total = total_block_chars(blocks);
                if *visible_chars < total {
                    *visible_chars = (*visible_chars + TYPEWRITER_CHARS_PER_TICK).min(total);
                }
                if *done {
                    *visible_chars = total;
                }
            }
        }
    }

    /// Returns true when the input field accepts edits (correct route, focused, unlocked).
    pub(crate) fn is_input_editable(&self) -> bool {
        self.focus == Focus::Input && !self.input_locked && matches!(self.route, Route::Chat)
    }

    /// Returns true when any agent response is still streaming or a tool call is running.
    pub(crate) fn is_streaming(&self) -> bool {
        self.messages
            .iter()
            .any(|e| matches!(e, ChatEntry::Agent { done: false, .. }))
            || self
                .tool_calls
                .values()
                .any(|tc| tc.status == ToolCallStatus::Running)
    }

    /// Appends a chat entry to the message history.
    pub(crate) fn push_message(&mut self, entry: ChatEntry) {
        self.messages.push(entry);
    }
}
