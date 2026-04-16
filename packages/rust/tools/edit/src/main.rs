use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

/// A single text replacement: `old_text` → `new_text`.
#[derive(Deserialize)]
struct EditEntry {
    #[serde(alias = "oldText")]
    old_text: String,
    #[serde(alias = "newText")]
    new_text: String,
}

/// JSON input payload for the edit tool.
#[derive(Deserialize)]
struct EditInput {
    path: String,
    edits: Vec<EditEntry>,
}

/// JSON output payload returned to the agent with diff and status.
#[derive(Serialize)]
struct EditOutput {
    success: bool,
    diff: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_changed_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Prints the tool manifest JSON to stdout for Core auto-discovery.
fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "edit",
        "kind": "tool",
        "version": "0.1.0",
        "description": "Edit a file using exact text replacement. Each edit's oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit.",
        "timeout_ms": 30_000,
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute)"
                },
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "description": "Exact text to find. Must be unique in the original file."
                            },
                            "newText": {
                                "type": "string",
                                "description": "Replacement text."
                            }
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            },
            "required": ["path", "edits"]
        }
    });
    println!("{}", serde_json::to_string(&manifest).unwrap());
}

/// Normalizes all line endings to LF for consistent matching.
fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Detects whether the file uses CRLF or LF line endings.
fn detect_line_ending(content: &str) -> &'static str {
    let crlf_pos = content.find("\r\n");
    let lf_pos = content.find('\n');
    match (crlf_pos, lf_pos) {
        (Some(c), Some(l)) if c < l => "\r\n",
        _ => "\n",
    }
}

/// Converts LF-normalized text back to the original line ending style.
fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// Splits off the UTF-8 BOM prefix if present, returning `(bom, rest)`.
fn strip_bom(content: &str) -> (&str, &str) {
    if content.starts_with('\u{FEFF}') {
        ("\u{FEFF}", &content[3..])
    } else {
        ("", content)
    }
}

/// Strips trailing whitespace from each line for fuzzy matching.
fn normalize_for_fuzzy(text: &str) -> String {
    text.lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Produces a line-by-line unified diff with line numbers.
fn generate_diff(old: &str, new: &str) -> (String, Option<usize>) {
    let diff = TextDiff::from_lines(old, new);
    let mut output = Vec::new();
    let mut first_changed_line: Option<usize> = None;
    let max_line = old.lines().count().max(new.lines().count());
    let width = format!("{}", max_line).len();

    let mut old_line: usize = 1;
    let mut new_line: usize = 1;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                let text = change.value().trim_end_matches('\n');
                output.push(format!(" {:>width$} {}", old_line, text));
                old_line += 1;
                new_line += 1;
            }
            ChangeTag::Delete => {
                if first_changed_line.is_none() {
                    first_changed_line = Some(new_line);
                }
                let text = change.value().trim_end_matches('\n');
                output.push(format!("-{:>width$} {}", old_line, text));
                old_line += 1;
            }
            ChangeTag::Insert => {
                if first_changed_line.is_none() {
                    first_changed_line = Some(new_line);
                }
                let text = change.value().trim_end_matches('\n');
                output.push(format!("+{:>width$} {}", new_line, text));
                new_line += 1;
            }
        }
    }

    (output.join("\n"), first_changed_line)
}

/// An edit that has been located in the file content, ready for application.
struct MatchedEdit {
    edit_index: usize,
    match_index: usize,
    match_length: usize,
    new_text: String,
}

/// Applies all edits to the file: validates uniqueness, checks for overlaps, writes, and diffs.
fn execute(input: EditInput) -> EditOutput {
    let file_path = PathBuf::from(&input.path);

    if !file_path.exists() {
        return EditOutput {
            success: false,
            diff: String::new(),
            first_changed_line: None,
            error: Some(format!("File not found: {}", input.path)),
        };
    }

    if input.edits.is_empty() {
        return EditOutput {
            success: false,
            diff: String::new(),
            first_changed_line: None,
            error: Some("edits must contain at least one replacement.".into()),
        };
    }

    let raw_content = match fs::read_to_string(&file_path) {
        Ok(c) => c,
        Err(e) => {
            return EditOutput {
                success: false,
                diff: String::new(),
                first_changed_line: None,
                error: Some(format!("Failed to read file: {e}")),
            };
        }
    };

    let (bom, content) = strip_bom(&raw_content);
    let original_ending = detect_line_ending(content);
    let normalized = normalize_to_lf(content);

    // Normalize edits: convert line endings and strip trailing whitespace for fuzzy matching
    let edits_normalized: Vec<(String, String)> = input
        .edits
        .iter()
        .map(|e| {
            let old_lf = normalize_to_lf(&e.old_text);
            let new_lf = normalize_to_lf(&e.new_text);
            (normalize_for_fuzzy(&old_lf), normalize_for_fuzzy(&new_lf))
        })
        .collect();

    for (i, (old_text, _)) in edits_normalized.iter().enumerate() {
        if old_text.is_empty() {
            let msg = if edits_normalized.len() == 1 {
                format!("oldText must not be empty in {}.", input.path)
            } else {
                format!("edits[{}].oldText must not be empty in {}.", i, input.path)
            };
            return EditOutput {
                success: false,
                diff: String::new(),
                first_changed_line: None,
                error: Some(msg),
            };
        }
    }

    // Check if any edit requires fuzzy matching (exact match fails due to whitespace differences)
    let any_fuzzy = edits_normalized
        .iter()
        .any(|(old_text, _)| normalized.find(old_text.as_str()).is_none());

    // For fuzzy matching, we need consistent index mapping between content and patterns
    let base_content = if any_fuzzy {
        normalize_for_fuzzy(&normalized)
    } else {
        normalized.clone()
    };

    let mut matched_edits: Vec<MatchedEdit> = Vec::new();

    for (i, (old_text, new_text)) in edits_normalized.iter().enumerate() {
        // Find the edit in the base content (already fuzzy-normalized if needed)
        let idx = match base_content.find(old_text.as_str()) {
            Some(idx) => idx,
            None => {
                let msg = if edits_normalized.len() == 1 {
                    format!(
                        "Could not find the exact text in {}. The old text must match exactly including all whitespace and newlines.",
                        input.path
                    )
                } else {
                    format!(
                        "Could not find edits[{}] in {}. The oldText must match exactly including all whitespace and newlines.",
                        i, input.path
                    )
                };
                return EditOutput {
                    success: false,
                    diff: String::new(),
                    first_changed_line: None,
                    error: Some(msg),
                };
            }
        };

        // Check for duplicate occurrences
        let occurrences = base_content.matches(old_text.as_str()).count();
        if occurrences > 1 {
            let msg = if edits_normalized.len() == 1 {
                format!(
                    "Found {} occurrences of the text in {}. The text must be unique. Please provide more context.",
                    occurrences, input.path
                )
            } else {
                format!(
                    "Found {} occurrences of edits[{}] in {}. Each oldText must be unique.",
                    occurrences, i, input.path
                )
            };
            return EditOutput {
                success: false,
                diff: String::new(),
                first_changed_line: None,
                error: Some(msg),
            };
        }

        matched_edits.push(MatchedEdit {
            edit_index: i,
            match_index: idx,
            match_length: old_text.len(),
            new_text: new_text.clone(),
        });
    }

    matched_edits.sort_by_key(|e| e.match_index);

    for i in 1..matched_edits.len() {
        let prev = &matched_edits[i - 1];
        let curr = &matched_edits[i];
        if prev.match_index + prev.match_length > curr.match_index {
            return EditOutput {
                success: false,
                diff: String::new(),
                first_changed_line: None,
                error: Some(format!(
                    "edits[{}] and edits[{}] overlap in {}. Merge them into one edit.",
                    prev.edit_index, curr.edit_index, input.path
                )),
            };
        }
    }

    let mut new_content = base_content.clone();
    for edit in matched_edits.iter().rev() {
        let end = edit.match_index + edit.match_length;
        new_content = format!(
            "{}{}{}",
            &new_content[..edit.match_index],
            edit.new_text,
            &new_content[end..]
        );
    }

    if base_content == new_content {
        return EditOutput {
            success: false,
            diff: String::new(),
            first_changed_line: None,
            error: Some(format!("No changes made to {}. The replacement produced identical content.", input.path)),
        };
    }

    let final_content = format!(
        "{}{}",
        bom,
        restore_line_endings(&new_content, original_ending)
    );

    if let Err(e) = fs::write(&file_path, &final_content) {
        return EditOutput {
            success: false,
            diff: String::new(),
            first_changed_line: None,
            error: Some(format!("Failed to write file: {e}")),
        };
    }

    let (diff, first_changed_line) = generate_diff(&base_content, &new_content);

    EditOutput {
        success: true,
        diff,
        first_changed_line,
        error: None,
    }
}

/// Entry point — reads edit instructions from stdin, applies them, and prints JSON output.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--manifest") {
        print_manifest();
        return;
    }

    let mut stdin_buf = String::new();
    if std::io::stdin().read_to_string(&mut stdin_buf).is_err() {
        let output = EditOutput {
            success: false,
            diff: String::new(),
            first_changed_line: None,
            error: Some("Failed to read stdin".into()),
        };
        println!("{}", serde_json::to_string(&output).unwrap());
        return;
    }

    let input: EditInput = match serde_json::from_str(&stdin_buf) {
        Ok(i) => i,
        Err(e) => {
            let output = EditOutput {
                success: false,
                diff: String::new(),
                first_changed_line: None,
                error: Some(format!("Invalid input JSON: {e}")),
            };
            println!("{}", serde_json::to_string(&output).unwrap());
            return;
        }
    };

    let output = execute(input);
    println!("{}", serde_json::to_string(&output).unwrap());
}
