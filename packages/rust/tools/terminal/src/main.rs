use serde::{Deserialize, Serialize};
use std::io::Read;
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Deserialize)]
struct TerminalInput {
    command: String,
    #[serde(default)]
    working_directory: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Serialize)]
struct TerminalOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "terminal",
        "kind": "tool",
        "version": "0.1.0",
        "description": "Executes shell commands on the host machine. Input: {\"command\": \"<shell command>\", \"working_directory\": \"<path>\"}. Returns stdout, stderr, and exit_code.",
        "timeout_ms": 600_000,
        "input_schema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "working_directory": {
                    "type": "string",
                    "description": "Working directory for the command"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Command timeout in milliseconds (default: 120000)"
                }
            },
            "required": ["command"]
        }
    });
    println!("{}", serde_json::to_string(&manifest).unwrap());
}

fn shell_program() -> &'static str {
    if cfg!(windows) {
        "cmd.exe"
    } else {
        "sh"
    }
}

fn shell_flag() -> &'static str {
    if cfg!(windows) {
        "/C"
    } else {
        "-c"
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--manifest") {
        print_manifest();
        return;
    }

    let mut stdin_buf = String::new();
    if std::io::stdin().read_to_string(&mut stdin_buf).is_err() {
        let output = TerminalOutput {
            stdout: String::new(),
            stderr: "Failed to read stdin".into(),
            exit_code: -1,
        };
        println!("{}", serde_json::to_string(&output).unwrap());
        return;
    }

    let input: TerminalInput = match serde_json::from_str(&stdin_buf) {
        Ok(i) => i,
        Err(e) => {
            let output = TerminalOutput {
                stdout: String::new(),
                stderr: format!("Invalid input JSON: {e}"),
                exit_code: -1,
            };
            println!("{}", serde_json::to_string(&output).unwrap());
            return;
        }
    };

    let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));

    let mut cmd = tokio::process::Command::new(shell_program());
    cmd.arg(shell_flag())
        .arg(&input.command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(ref wd) = input.working_directory {
        if !wd.is_empty() {
            cmd.current_dir(wd);
        }
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let output = TerminalOutput {
                stdout: String::new(),
                stderr: format!("Failed to spawn command: {e}"),
                exit_code: -1,
            };
            println!("{}", serde_json::to_string(&output).unwrap());
            return;
        }
    };

    let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

    let output = match result {
        Ok(Ok(proc_output)) => {
            let code = proc_output.status.code().unwrap_or(-1);
            TerminalOutput {
                stdout: String::from_utf8_lossy(&proc_output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&proc_output.stderr).into_owned(),
                exit_code: code,
            }
        }
        Ok(Err(e)) => TerminalOutput {
            stdout: String::new(),
            stderr: format!("Command execution error: {e}"),
            exit_code: -1,
        },
        Err(_) => TerminalOutput {
            stdout: String::new(),
            stderr: format!(
                "Command timed out after {}ms",
                timeout.as_millis()
            ),
            exit_code: -1,
        },
    };

    println!("{}", serde_json::to_string(&output).unwrap());
}
