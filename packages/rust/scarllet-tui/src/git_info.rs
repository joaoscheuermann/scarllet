use std::fs;
use std::path::{Path, PathBuf};

pub struct GitInfo {
    pub branch: Option<String>,
    pub short_sha: String,
    pub detached: bool,
}

pub fn read_git_info(cwd: &Path) -> Option<GitInfo> {
    let git_dir = find_git_dir(cwd)?;
    let head_contents = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head_contents.trim();

    if let Some(ref_path) = head.strip_prefix("ref: ") {
        let branch = ref_path
            .strip_prefix("refs/heads/")
            .unwrap_or(ref_path)
            .to_string();
        let sha = fs::read_to_string(git_dir.join(ref_path))
            .ok()
            .map(|s| s.trim().to_string())
            .or_else(|| resolve_packed_ref(&git_dir, ref_path))?;
        let short = abbreviate_sha(&sha);
        return Some(GitInfo {
            branch: Some(branch),
            short_sha: short,
            detached: false,
        });
    }

    if head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(GitInfo {
            branch: None,
            short_sha: abbreviate_sha(head),
            detached: true,
        });
    }

    None
}

fn find_git_dir(mut dir: &Path) -> Option<PathBuf> {
    loop {
        let candidate = dir.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            let content = fs::read_to_string(&candidate).ok()?;
            let gitdir = content.trim().strip_prefix("gitdir: ")?;
            let resolved = if Path::new(gitdir).is_absolute() {
                PathBuf::from(gitdir)
            } else {
                dir.join(gitdir)
            };
            return Some(resolved);
        }
        dir = dir.parent()?;
    }
}

fn resolve_packed_ref(git_dir: &Path, ref_path: &str) -> Option<String> {
    let packed = fs::read_to_string(git_dir.join("packed-refs")).ok()?;
    for line in packed.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let sha = parts.next()?;
        let name = parts.next()?;
        if name == ref_path {
            return Some(sha.to_string());
        }
    }
    None
}

fn abbreviate_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

pub fn abbreviate_home(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            let mut display = String::from("~");
            let remainder = rest.to_string_lossy();
            if !remainder.is_empty() {
                display.push(std::path::MAIN_SEPARATOR);
                display.push_str(&remainder);
            }
            return display;
        }
    }
    path.to_string_lossy().into_owned()
}
