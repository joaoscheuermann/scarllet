use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

const DEFAULT_LIMIT: usize = 1000;
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

#[derive(Deserialize)]
struct FindInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize)]
struct FindOutput {
    results: Vec<String>,
    total: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

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

fn matches_glob(path: &str, pattern: &str) -> bool {
    let glob_pattern = glob::Pattern::new(pattern);
    match glob_pattern {
        Ok(p) => p.matches(path),
        Err(_) => false,
    }
}

fn to_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

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

    let walker = WalkBuilder::new(&search_path)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != "node_modules" && name != ".git"
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

        let is_match = matches_glob(&relative_str, &input.pattern)
            || matches_glob(&file_name, &input.pattern);

        if !is_match {
            continue;
        }

        let line_bytes = relative_str.len() + 1;
        if total_bytes + line_bytes > MAX_OUTPUT_BYTES {
            truncated = true;
            break;
        }

        total_bytes += line_bytes;
        results.push(relative_str);

        if results.len() >= effective_limit {
            truncated = true;
            break;
        }
    }

    let total = results.len();
    FindOutput {
        results,
        total,
        truncated,
        error: None,
    }
}

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
