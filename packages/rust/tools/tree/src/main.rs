use glob::Pattern;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};

const HIDDEN_EXCEPTIONS: &[&str] = &[".agents"];

#[derive(Deserialize)]
struct TreeInput {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
}

fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "tree",
        "kind": "tool",
        "version": "0.1.0",
        "description": "Display directory structure as a tree. Returns a plain-text hierarchical listing with box-drawing connectors. Directories are listed first, then files, both sorted alphabetically. Respects .gitignore and excludes hidden files.",
        "timeout_ms": 30_000,
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Root directory to display (default: current directory)"
                },
                "exclude": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Glob patterns to exclude from the tree"
                }
            }
        }
    });
    println!("{}", serde_json::to_string(&manifest).unwrap());
}

fn is_excluded(path: &Path, exclude_patterns: &[Pattern]) -> bool {
    if exclude_patterns.is_empty() {
        return false;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let path_str = path.to_string_lossy().replace('\\', "/");
    exclude_patterns
        .iter()
        .any(|p| p.matches(&name) || p.matches(&path_str))
}

/// Check gitignore rules from most specific (last) to least specific (first).
/// Whitelisted entries (negation patterns like `!keep.log`) override ignores.
fn is_gitignored(path: &Path, is_dir: bool, gitignores: &[Gitignore]) -> bool {
    for gi in gitignores.iter().rev() {
        match gi.matched(path, is_dir) {
            ignore::Match::Ignore(_) => return true,
            ignore::Match::Whitelist(_) => return false,
            ignore::Match::None => continue,
        }
    }
    false
}

/// Load `.gitignore` from `dir` if present, push onto the stack, return whether one was added.
fn maybe_push_gitignore(dir: &Path, stack: &mut Vec<Gitignore>) -> bool {
    let gitignore_path = dir.join(".gitignore");
    if !gitignore_path.is_file() {
        return false;
    }
    let mut builder = GitignoreBuilder::new(dir);
    builder.add(gitignore_path);
    match builder.build() {
        Ok(gi) => {
            stack.push(gi);
            true
        }
        Err(_) => false,
    }
}

fn case_insensitive_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
}

/// Read, filter, and sort directory entries.
/// Skips hidden files, symlinks, gitignored entries, and user-excluded entries.
/// Returns dirs-first then files, each group case-insensitive alphabetical.
fn filtered_sorted_entries(
    dir: &Path,
    gitignores: &[Gitignore],
    exclude_patterns: &[Pattern],
) -> std::io::Result<Vec<(PathBuf, bool)>> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        if name.starts_with('.') && !HIDDEN_EXCEPTIONS.contains(&name.as_ref()) {
            continue;
        }

        let is_symlink = std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            continue;
        }

        let entry_is_dir = path.is_dir();

        if is_gitignored(&path, entry_is_dir, gitignores) {
            continue;
        }
        if is_excluded(&path, exclude_patterns) {
            continue;
        }

        if entry_is_dir {
            dirs.push(path);
        } else {
            files.push(path);
        }
    }

    dirs.sort_by(|a, b| case_insensitive_name(a).cmp(&case_insensitive_name(b)));
    files.sort_by(|a, b| case_insensitive_name(a).cmp(&case_insensitive_name(b)));

    let mut result = Vec::with_capacity(dirs.len() + files.len());
    result.extend(dirs.into_iter().map(|d| (d, true)));
    result.extend(files.into_iter().map(|f| (f, false)));
    Ok(result)
}

fn build_tree(
    dir: &Path,
    prefix: &str,
    gitignores: &mut Vec<Gitignore>,
    exclude_patterns: &[Pattern],
    output: &mut String,
) {
    let added = maybe_push_gitignore(dir, gitignores);

    let entries = match filtered_sorted_entries(dir, gitignores, exclude_patterns) {
        Ok(e) => e,
        Err(_) => {
            if added {
                gitignores.pop();
            }
            return;
        }
    };

    for (i, (path, is_dir)) in entries.iter().enumerate() {
        let is_last = i == entries.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        if *is_dir {
            output.push_str(&format!("{prefix}{connector}{name}/"));
            let mark = output.len();
            output.push('\n');

            let child_prefix = if is_last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}│   ")
            };
            build_tree(path, &child_prefix, gitignores, exclude_patterns, output);

            if output.len() == mark + 1 {
                output.truncate(mark);
                output.push_str(" (empty)\n");
            }
        } else {
            output.push_str(&format!("{prefix}{connector}{name}\n"));
        }
    }

    if added {
        gitignores.pop();
    }
}

fn execute(input: TreeInput) -> String {
    let raw_path = input.path.unwrap_or_default();
    let dir = if raw_path.is_empty() { "." } else { &raw_path };
    let root = PathBuf::from(dir);

    if !root.exists() {
        return format!("Error: path not found: {dir}");
    }
    if !root.is_dir() {
        return format!("Error: path is not a directory: {dir}");
    }

    let exclude_patterns: Vec<Pattern> = input
        .exclude
        .unwrap_or_default()
        .iter()
        .filter_map(|p| Pattern::new(p).ok())
        .collect();

    let root_name = root.file_name().unwrap_or_default().to_string_lossy();
    if exclude_patterns.iter().any(|p| p.matches(&root_name)) {
        return format!("Error: exclude pattern matches the root directory: {root_name}");
    }

    let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
    let root_display = canonical
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .to_string();

    let mut gitignores: Vec<Gitignore> = Vec::new();
    let mut output = format!("{root_display}\n");
    build_tree(&root, "", &mut gitignores, &exclude_patterns, &mut output);
    output
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--manifest") {
        print_manifest();
        return;
    }

    let mut stdin_buf = String::new();
    if std::io::stdin().read_to_string(&mut stdin_buf).is_err() {
        println!("Error: failed to read stdin");
        return;
    }

    let input: TreeInput = match serde_json::from_str(&stdin_buf) {
        Ok(i) => i,
        Err(e) => {
            println!("Error: invalid input JSON: {e}");
            return;
        }
    };

    let output = execute(input);
    print!("{output}");
}
