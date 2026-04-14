use scarllet_sdk::manifest::ModuleKind;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::registry::ModuleRegistry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for TaskStatus {
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

#[derive(Debug)]
pub struct TaskState {
    pub task_id: String,
    pub agent_name: String,
    pub status: TaskStatus,
    pub progress_log: Vec<String>,
    pub working_directory: String,
    pub snapshot_id: u64,
    pub pid: Option<u32>,
}

pub struct TaskManager {
    tasks: HashMap<String, TaskState>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    pub fn submit(
        &mut self,
        agent_name: String,
        working_directory: String,
        snapshot_id: u64,
    ) -> String {
        let task_id = Uuid::new_v4().to_string();
        let state = TaskState {
            task_id: task_id.clone(),
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

    pub fn get(&self, task_id: &str) -> Option<&TaskState> {
        self.tasks.get(task_id)
    }

    pub fn get_mut(&mut self, task_id: &str) -> Option<&mut TaskState> {
        self.tasks.get_mut(task_id)
    }

    pub fn set_status(&mut self, task_id: &str, status: TaskStatus) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = status;
        }
    }

    pub fn add_progress(&mut self, task_id: &str, message: String) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.progress_log.push(message);
        }
    }

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
}
