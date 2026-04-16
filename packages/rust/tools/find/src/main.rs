use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Maximum number of results before truncation.
const DEFAULT_LIMIT: usize = 1000;
/// Maximum total output size in bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
/// Directories and entries to filter out from file walking.
const FILTERED_ENTRIES: &[&str] = &["node_modules", ".git"];

/// JSON input payload for the find tool.
#[derive(Deserialize)]
struct FindInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// JSON output payload returned to the agent.
#[derive(Serialize)]
struct FindOutput {
    results: Vec<String>,
    total: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Prints the tool manifest JSON to stdout for Core auto-discovery.
fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "find",
        "kind": "tool",
        "version": "0.1.0",
        "description": "Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to 1000 results or 50KB (whichever is hit first).",
        "timeout_ms": 30_000,
        "input_schema": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 1000)"
                }
            },
            "required": ["pattern"]
        }
    });
    println!("{}", serde_json::to_string(&manifest).unwrap());
}

/// Tests whether a path matches the given glob pattern.
fn matches_glob(path: &str, pattern: &str) -> Result<bool, String> {
    let glob_pattern = glob::Pattern::new(pattern)
        .map_err(|e| format!("Invalid glob pattern '{}': {}", pattern, e))?;
    Ok(glob_pattern.matches(path))
}

/// Converts a path to forward-slash format for cross-platform consistency.
fn to_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Walks the directory tree and collects paths matching the glob pattern.
fn execute(input: FindInput) -> FindOutput {
    let search_dir = input.path.as_deref().unwrap_or(".");
    let search_path = PathBuf::from(search_dir);

    if !search_path.exists() {
        return FindOutput {
            results: vec![],
            total: 0,
            truncated: false,
            error: Some(format!("Path not found: {search_dir}")),
        };
    }

    let effective_limit = input.limit.unwrap_or(DEFAULT_LIMIT);
    let mut results: Vec<String> = Vec::new();
    let mut total_bytes: usize = 0;
    let mut truncated = false;
    let mut total_matched: usize = 0;

    let walker = WalkBuilder::new(&search_path)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !FILTERED_ENTRIES.contains(&name.as_ref())
        })
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().map_or(true, |ft| ft.is_dir()) {
            continue;
        }

        let full_path = entry.path();
        let relative = match full_path.strip_prefix(&search_path) {
            Ok(r) => r,
            Err(_) => full_path,
        };

        let relative_str = to_posix(relative);
        let file_name = relative
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();

        let is_match = match matches_glob(&relative_str, &input.pattern) {
            Ok(m) => m || matches_glob(&file_name, &input.pattern).unwrap_or(false),
            Err(e) => {
                return FindOutput {
                    results: vec![],
                    total: 0,
                    truncated: false,
                    error: Some(e),
                };
            }
        };

        if !is_match {
            continue;
        }

        total_matched += 1;

        let line_bytes = relative_str.len() + 1;
        if total_bytes + line_bytes > MAX_OUTPUT_BYTES {
            truncated = true;
            break;
        }

        if results.len() >= effective_limit {
            truncated = true;
            break;
        }

        total_bytes += line_bytes;
        results.push(relative_str);
    }

    let total = if truncated { total_matched } else { results.len() };
    FindOutput {
        results,
        total,
        truncated,
        error: None,
    }
}

/// Entry point — reads a glob pattern from stdin, finds matching files, and prints JSON output.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--manifest") {
        print_manifest();
        return;
    }

    let mut stdin_buf = String::new();
    if std::io::stdin().read_to_string(&mut stdin_buf).is_err() {
        let output = FindOutput {
            results: vec![],
            total: 0,
            truncated: false,
            error: Some("Failed to read stdin".into()),
        };
        println!("{}", serde_json::to_string(&output).unwrap());
        return;
    }

    let input: FindInput = match serde_json::from_str(&stdin_buf) {
        Ok(i) => i,
        Err(e) => {
            let output = FindOutput {
                results: vec![],
                total: 0,
                truncated: false,
                error: Some(format!("Invalid input JSON: {e}")),
            };
            println!("{}", serde_json::to_string(&output).unwrap());
            return;
        }
    };

    let output = execute(input);
    println!("{}", serde_json::to_string(&output).unwrap());
}
