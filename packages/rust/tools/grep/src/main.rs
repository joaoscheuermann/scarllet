use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const DEFAULT_LIMIT: usize = 100;
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_LINE_LENGTH: usize = 500;

#[derive(Deserialize)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default, rename = "ignoreCase")]
    ignore_case: Option<bool>,
    #[serde(default)]
    literal: Option<bool>,
    #[serde(default)]
    context: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize)]
struct GrepMatch {
    file: String,
    line: usize,
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context_before: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context_after: Vec<String>,
}

#[derive(Serialize)]
struct GrepOutput {
    matches: Vec<GrepMatch>,
    total: usize,
    truncated: bool,
    lines_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "grep",
        "kind": "tool",
        "version": "0.1.0",
        "description": "Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to 100 matches or 50KB (whichever is hit first). Long lines are truncated to 500 chars.",
        "timeout_ms": 30_000,
        "input_schema": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regex or literal string)"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (default: current directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'"
                },
                "ignoreCase": {
                    "type": "boolean",
                    "description": "Case-insensitive search (default: false)"
                },
                "literal": {
                    "type": "boolean",
                    "description": "Treat pattern as literal string instead of regex (default: false)"
                },
                "context": {
                    "type": "integer",
                    "description": "Number of lines to show before and after each match (default: 0)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of matches to return (default: 100)"
                }
            },
            "required": ["pattern"]
        }
    });
    println!("{}", serde_json::to_string(&manifest).unwrap());
}

fn truncate_line(line: &str) -> (String, bool) {
    if line.len() <= MAX_LINE_LENGTH {
        (line.to_string(), false)
    } else {
        (
            format!("{}... [truncated]", &line[..MAX_LINE_LENGTH]),
            true,
        )
    }
}

fn to_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn execute(input: GrepInput) -> GrepOutput {
    let search_dir = input.path.as_deref().unwrap_or(".");
    let search_path = PathBuf::from(search_dir);

    if !search_path.exists() {
        return GrepOutput {
            matches: vec![],
            total: 0,
            truncated: false,
            lines_truncated: false,
            error: Some(format!("Path not found: {search_dir}")),
        };
    }

    let case_insensitive = input.ignore_case.unwrap_or(false);
    let literal = input.literal.unwrap_or(false);
    let context_lines = input.context.unwrap_or(0);
    let effective_limit = input.limit.unwrap_or(DEFAULT_LIMIT).max(1);

    let regex_pattern = if literal {
        regex::escape(&input.pattern)
    } else {
        input.pattern.clone()
    };

    let re = match RegexBuilder::new(&regex_pattern)
        .case_insensitive(case_insensitive)
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            return GrepOutput {
                matches: vec![],
                total: 0,
                truncated: false,
                lines_truncated: false,
                error: Some(format!("Invalid regex pattern: {e}")),
            };
        }
    };

    let glob_pattern: Option<glob::Pattern> = input
        .glob
        .as_deref()
        .and_then(|g| glob::Pattern::new(g).ok());

    let is_file = search_path.is_file();
    let mut results: Vec<GrepMatch> = Vec::new();
    let mut total_bytes: usize = 0;
    let mut truncated = false;
    let mut lines_truncated = false;

    let files: Vec<PathBuf> = if is_file {
        vec![search_path.clone()]
    } else {
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

        walker
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map_or(false, |ft| ft.is_file()))
            .map(|e| e.into_path())
            .collect()
    };

    'outer: for file_path in &files {
        if let Some(ref gp) = glob_pattern {
            let name = file_path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            let relative = file_path
                .strip_prefix(&search_path)
                .map(|r| to_posix(r))
                .unwrap_or_else(|_| to_posix(file_path));
            if !gp.matches(&name) && !gp.matches(&relative) {
                continue;
            }
        }

        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let all_lines: Vec<&str> = content.lines().collect();
        let relative = if is_file {
            file_path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            file_path
                .strip_prefix(&search_path)
                .map(|r| to_posix(r))
                .unwrap_or_else(|_| to_posix(file_path))
        };

        for (line_idx, line) in all_lines.iter().enumerate() {
            if !re.is_match(line) {
                continue;
            }

            let line_number = line_idx + 1;
            let (truncated_line, was_truncated) = truncate_line(line);
            if was_truncated {
                lines_truncated = true;
            }

            let context_before: Vec<String> = if context_lines > 0 {
                let start = line_idx.saturating_sub(context_lines);
                (start..line_idx)
                    .map(|i| {
                        let (t, w) = truncate_line(all_lines[i]);
                        if w {
                            lines_truncated = true;
                        }
                        t
                    })
                    .collect()
            } else {
                vec![]
            };

            let context_after: Vec<String> = if context_lines > 0 {
                let end = (line_idx + 1 + context_lines).min(all_lines.len());
                ((line_idx + 1)..end)
                    .map(|i| {
                        let (t, w) = truncate_line(all_lines[i]);
                        if w {
                            lines_truncated = true;
                        }
                        t
                    })
                    .collect()
            } else {
                vec![]
            };

            let entry_estimate = relative.len() + truncated_line.len() + 20;
            if total_bytes + entry_estimate > MAX_OUTPUT_BYTES {
                truncated = true;
                break 'outer;
            }
            total_bytes += entry_estimate;

            results.push(GrepMatch {
                file: relative.clone(),
                line: line_number,
                text: truncated_line,
                context_before,
                context_after,
            });

            if results.len() >= effective_limit {
                truncated = true;
                break 'outer;
            }
        }
    }

    let total = results.len();
    GrepOutput {
        matches: results,
        total,
        truncated,
        lines_truncated,
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
        let output = GrepOutput {
            matches: vec![],
            total: 0,
            truncated: false,
            lines_truncated: false,
            error: Some("Failed to read stdin".into()),
        };
        println!("{}", serde_json::to_string(&output).unwrap());
        return;
    }

    let input: GrepInput = match serde_json::from_str(&stdin_buf) {
        Ok(i) => i,
        Err(e) => {
            let output = GrepOutput {
                matches: vec![],
                total: 0,
                truncated: false,
                lines_truncated: false,
                error: Some(format!("Invalid input JSON: {e}")),
            };
            println!("{}", serde_json::to_string(&output).unwrap());
            return;
        }
    };

    let output = execute(input);
    println!("{}", serde_json::to_string(&output).unwrap());
}
