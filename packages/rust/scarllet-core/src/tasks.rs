use scarllet_sdk::manifest::ModuleKind;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::registry::ModuleRegistry;

/// Lifecycle states a task transitions through from creation to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for TaskStatus {
    /// Renders the status as a lowercase string matching the proto contract.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Full mutable snapshot of a single task, including its process ID and progress log.
///
/// The task ID lives in the owning [`TaskManager`] HashMap key — the struct itself
/// carries only per-task mutable state.
#[derive(Debug)]
pub struct TaskState {
    pub agent_name: String,
    pub status: TaskStatus,
    pub progress_log: Vec<String>,
    pub working_directory: String,
    pub snapshot_id: u64,
    pub pid: Option<u32>,
}

/// In-memory store for all tasks submitted during this core session.
pub struct TaskManager {
    tasks: HashMap<String, TaskState>,
}

impl TaskManager {
    /// Initialises an empty task store.
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Creates a task in `Pending` state and returns its generated UUID.
    pub fn submit(
        &mut self,
        agent_name: String,
        working_directory: String,
        snapshot_id: u64,
    ) -> String {
        let task_id = Uuid::new_v4().to_string();
        let state = TaskState {
            agent_name,
            status: TaskStatus::Pending,
            progress_log: vec![],
            working_directory,
            snapshot_id,
            pid: None,
        };
        self.tasks.insert(task_id.clone(), state);
        task_id
    }

    /// Retrieves an immutable reference to a task by ID.
    pub fn get(&self, task_id: &str) -> Option<&TaskState> {
        self.tasks.get(task_id)
    }

    /// Retrieves a mutable reference to a task by ID.
    pub fn get_mut(&mut self, task_id: &str) -> Option<&mut TaskState> {
        self.tasks.get_mut(task_id)
    }

    /// Transitions a task to the given status. No-op if the task ID is unknown.
    pub fn set_status(&mut self, task_id: &str, status: TaskStatus) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = status;
        }
    }

    /// Appends a line to the task's progress log.
    pub fn add_progress(&mut self, task_id: &str, message: String) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.progress_log.push(message);
        }
    }

    /// Returns the agent name recorded for `task_id`, or the empty string when
    /// the task is unknown. This matches the semantics callers previously
    /// implemented inline with `get(..).map(..).unwrap_or_default()`.
    pub fn agent_name_for(&self, task_id: &str) -> String {
        self.get(task_id)
            .map(|t| t.agent_name.clone())
            .unwrap_or_default()
    }

    /// Transitions a task to [`TaskStatus::Completed`]. No-op on unknown IDs.
    pub fn mark_completed(&mut self, task_id: &str) {
        self.set_status(task_id, TaskStatus::Completed);
    }

    /// Transitions a task to [`TaskStatus::Failed`] and appends `reason` to its
    /// progress log. No-op on unknown IDs.
    pub fn mark_failed(&mut self, task_id: &str, reason: &str) {
        self.set_status(task_id, TaskStatus::Failed);
        self.add_progress(task_id, reason.into());
    }

    /// Returns IDs of all pending or running tasks regardless of agent.
    pub fn all_active_task_ids(&self) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|(_, t)| matches!(t.status, TaskStatus::Pending | TaskStatus::Running))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Returns IDs of all pending or running tasks assigned to the named agent.
    pub fn active_tasks_for_agent(&self, agent_name: &str) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|(_, t)| {
                t.agent_name == agent_name
                    && matches!(t.status, TaskStatus::Pending | TaskStatus::Running)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// Spawns the agent binary as a child process and waits for it to exit.
///
/// Sets `SCARLLET_CORE_ADDR`, `SCARLLET_TASK_ID`, and `SCARLLET_SNAPSHOT_ID`
/// in the child environment so the agent can connect back to the core.
/// Updates the task status based on the process exit code.
pub async fn spawn_agent(
    registry: &Arc<RwLock<ModuleRegistry>>,
    task_manager: &Arc<RwLock<TaskManager>>,
    task_id: &str,
    core_addr: &str,
) {
    let (agent_name_owned, working_dir, snapshot_id) = {
        let tm = task_manager.read().await;
        match tm.get(task_id) {
            Some(t) => (
                t.agent_name.clone(),
                t.working_directory.clone(),
                t.snapshot_id,
            ),
            None => return,
        }
    };

    let agent_path = {
        let reg = registry.read().await;
        let found = reg
            .by_kind(ModuleKind::Agent)
            .into_iter()
            .find(|(_, m)| m.name == agent_name_owned)
            .map(|(p, _)| p.clone());
        match found {
            Some(p) => p,
            None => {
                let mut tm = task_manager.write().await;
                tm.set_status(task_id, TaskStatus::Failed);
                tm.add_progress(
                    task_id,
                    format!("Agent '{agent_name_owned}' not found"),
                );
                return;
            }
        }
    };

    let agent_name = agent_name_owned;

    {
        let mut tm = task_manager.write().await;
        tm.set_status(task_id, TaskStatus::Running);
        tm.add_progress(task_id, format!("Starting agent '{agent_name}'"));
    }

    let child = tokio::process::Command::new(&agent_path)
        .env("SCARLLET_CORE_ADDR", core_addr)
        .env("SCARLLET_TASK_ID", task_id)
        .env("SCARLLET_SNAPSHOT_ID", snapshot_id.to_string())
        .current_dir(&working_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to spawn agent '{agent_name}': {e}");
            let mut tm = task_manager.write().await;
            tm.set_status(task_id, TaskStatus::Failed);
            tm.add_progress(task_id, format!("Spawn failed: {e}"));
            return;
        }
    };

    if let Some(pid) = child.id() {
        let mut tm = task_manager.write().await;
        if let Some(task) = tm.get_mut(task_id) {
            task.pid = Some(pid);
        }
    }

    info!("Agent '{agent_name}' spawned for task {task_id}");

    let status = child.wait().await;
    let final_status = match status {
        Ok(s) if s.success() => TaskStatus::Completed,
        _ => TaskStatus::Failed,
    };

    let mut tm = task_manager.write().await;
    if let Some(task) = tm.get(task_id) {
        if task.status == TaskStatus::Cancelled {
            return;
        }
    }
    tm.set_status(task_id, final_status.clone());
    tm.add_progress(task_id, format!("Agent exited with status: {final_status}"));
    info!("Task {task_id} finished: {final_status}");
}

/// Kills the agent process for a running task and marks it cancelled.
///
/// On Unix, sends SIGTERM then SIGKILL after 2 seconds. On Windows, uses
/// `taskkill /F`. Returns `false` if the task is not in `Running` state.
pub async fn cancel_task(task_manager: &Arc<RwLock<TaskManager>>, task_id: &str) -> bool {
    let mut tm = task_manager.write().await;
    let task = match tm.get(task_id) {
        Some(t) if t.status == TaskStatus::Running => t,
        _ => return false,
    };

    if let Some(pid) = task.pid {
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .output()
                .await;
        }
    }

    tm.set_status(task_id, TaskStatus::Cancelled);
    tm.add_progress(task_id, "Cancelled by user".into());
    info!("Task {task_id} cancelled");
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_and_lifecycle() {
        let mut tm = TaskManager::new();
        let id = tm.submit("test-agent".into(), "/tmp".into(), 1);
        assert_eq!(tm.get(&id).unwrap().status, TaskStatus::Pending);

        tm.set_status(&id, TaskStatus::Running);
        assert_eq!(tm.get(&id).unwrap().status, TaskStatus::Running);

        tm.add_progress(&id, "step 1".into());
        assert_eq!(tm.get(&id).unwrap().progress_log.len(), 1);

        tm.set_status(&id, TaskStatus::Completed);
        assert_eq!(tm.get(&id).unwrap().status, TaskStatus::Completed);
    }

    #[test]
    fn agent_name_for_is_empty_when_missing() {
        let tm = TaskManager::new();
        assert_eq!(tm.agent_name_for("unknown"), "");
    }

    #[test]
    fn agent_name_for_returns_recorded_name() {
        let mut tm = TaskManager::new();
        let id = tm.submit("default".into(), "/tmp".into(), 1);
        assert_eq!(tm.agent_name_for(&id), "default");
    }

    #[test]
    fn mark_completed_and_failed_transition_status_and_log() {
        let mut tm = TaskManager::new();
        let ok_id = tm.submit("a".into(), "/tmp".into(), 1);
        let fail_id = tm.submit("b".into(), "/tmp".into(), 1);

        tm.mark_completed(&ok_id);
        tm.mark_failed(&fail_id, "boom");

        assert_eq!(tm.get(&ok_id).unwrap().status, TaskStatus::Completed);
        let failed = tm.get(&fail_id).unwrap();
        assert_eq!(failed.status, TaskStatus::Failed);
        assert_eq!(failed.progress_log, vec!["boom".to_string()]);
    }

    #[test]
    fn all_active_task_ids_returns_pending_and_running() {
        let mut tm = TaskManager::new();
        let pending = tm.submit("a".into(), "/tmp".into(), 1);
        let running = tm.submit("b".into(), "/tmp".into(), 1);
        let done = tm.submit("c".into(), "/tmp".into(), 1);

        tm.set_status(&running, TaskStatus::Running);
        tm.set_status(&done, TaskStatus::Completed);

        let mut active = tm.all_active_task_ids();
        active.sort();
        let mut expected = vec![pending, running];
        expected.sort();
        assert_eq!(active, expected);
    }
}
