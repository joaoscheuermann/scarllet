use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

/// JSON input payload for the write tool.
#[derive(Deserialize)]
struct WriteInput {
    path: String,
    content: String,
}

/// JSON output payload returned to the agent.
#[derive(Serialize)]
struct WriteOutput {
    success: bool,
    bytes_written: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Prints the tool manifest JSON to stdout for Core auto-discovery.
fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "write",
        "kind": "tool",
        "version": "0.1.0",
        "description": "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.",
        "timeout_ms": 30_000,
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write (relative or absolute)"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        }
    });
    println!("{}", serde_json::to_string(&manifest).unwrap());
}

/// Writes content to the file, creating parent directories if needed.
fn execute(input: WriteInput) -> WriteOutput {
    let file_path = PathBuf::from(&input.path);

    if let Some(parent) = file_path.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                return WriteOutput {
                    success: false,
                    bytes_written: 0,
                    error: Some(format!("Failed to create parent directories: {e}")),
                };
            }
        }
    }

    let bytes = input.content.len();

    match fs::write(&file_path, &input.content) {
        Ok(()) => WriteOutput {
            success: true,
            bytes_written: bytes,
            error: None,
        },
        Err(e) => WriteOutput {
            success: false,
            bytes_written: 0,
            error: Some(format!("Failed to write file: {e}")),
        },
    }
}

/// Entry point — reads file content from stdin, writes it, and prints JSON output.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--manifest") {
        print_manifest();
        return;
    }

    let mut stdin_buf = String::new();
    if std::io::stdin().read_to_string(&mut stdin_buf).is_err() {
        let output = WriteOutput {
            success: false,
            bytes_written: 0,
            error: Some("Failed to read stdin".into()),
        };
        println!("{}", serde_json::to_string(&output).unwrap());
        return;
    }

    let input: WriteInput = match serde_json::from_str(&stdin_buf) {
        Ok(i) => i,
        Err(e) => {
            let output = WriteOutput {
                success: false,
                bytes_written: 0,
                error: Some(format!("Invalid input JSON: {e}")),
            };
            println!("{}", serde_json::to_string(&output).unwrap());
            return;
        }
    };

    let output = execute(input);
    println!("{}", serde_json::to_string(&output).unwrap());
}
