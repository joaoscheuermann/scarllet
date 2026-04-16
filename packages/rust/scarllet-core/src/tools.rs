use crate::registry::ModuleRegistry;
use scarllet_sdk::manifest::ModuleKind;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Outcome of a tool invocation, carrying either JSON output or an error.
pub struct ToolResult {
    pub success: bool,
    pub output_json: String,
    pub error_message: String,
    pub duration_ms: u64,
}

/// Runs a registered tool binary by piping `input_json` to its stdin.
///
/// Validates the snapshot ID to prevent stale-registry invocations, enforces
/// the manifest-declared timeout, and returns structured success or error
/// information.
pub async fn invoke(
    registry: &Arc<RwLock<ModuleRegistry>>,
    tool_name: &str,
    input_json: &str,
    snapshot_id: u64,
) -> ToolResult {
    let reg = registry.read().await;

    if reg.version() < snapshot_id {
        return ToolResult {
            success: false,
            output_json: String::new(),
            error_message: "Invalid snapshot ID".into(),
            duration_ms: 0,
        };
    }

    let tool_entry = reg
        .by_kind(ModuleKind::Tool)
        .into_iter()
        .find(|(_, m)| m.name == tool_name);

    let (path, manifest) = match tool_entry {
        Some((p, m)) => (p.clone(), m.clone()),
        None => {
            return ToolResult {
                success: false,
                output_json: String::new(),
                error_message: format!("Tool '{tool_name}' not found"),
                duration_ms: 0,
            };
        }
    };

    let timeout_ms = manifest.timeout_ms.unwrap_or(30000);
    drop(reg);

    let start = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);

    let result = tokio::time::timeout(timeout, async {
        let mut child = match tokio::process::Command::new(&path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    success: false,
                    output_json: String::new(),
                    error_message: format!("Failed to spawn tool: {e}"),
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(input_json.as_bytes()).await;
            drop(stdin);
        }

        match child.wait_with_output().await {
            Ok(output) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return ToolResult {
                        success: false,
                        output_json: String::new(),
                        error_message: format!("Tool exited with {}: {stderr}", output.status),
                        duration_ms,
                    };
                }
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                if serde_json::from_str::<serde_json::Value>(&stdout).is_err() {
                    debug!("Tool output is not valid JSON");
                }
                ToolResult {
                    success: true,
                    output_json: stdout,
                    error_message: String::new(),
                    duration_ms,
                }
            }
            Err(e) => ToolResult {
                success: false,
                output_json: String::new(),
                error_message: format!("Failed to read tool output: {e}"),
                duration_ms: start.elapsed().as_millis() as u64,
            },
        }
    })
    .await;

    match result {
        Ok(r) => r,
        Err(_) => {
            warn!("Tool '{tool_name}' exceeded timeout of {timeout_ms}ms — killing");
            ToolResult {
                success: false,
                output_json: String::new(),
                error_message: format!("Tool exceeded timeout of {timeout_ms}ms"),
                duration_ms: timeout_ms,
            }
        }
    }
}
