use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// On-disk record written by the core daemon so other processes (TUI, agents)
/// can discover its PID and gRPC address without service-discovery overhead.
#[derive(Debug, Serialize, Deserialize)]
pub struct CoreLockfile {
    pub pid: u32,
    pub address: String,
    pub started_at: u64,
}

/// Returns the platform-standard path to the `core.lock` file.
pub fn path() -> PathBuf {
    let config_dir = dirs::config_dir().expect("could not determine OS config directory");
    config_dir.join("scarllet").join("core.lock")
}

/// Persists a lockfile recording the current process's PID, bound address,
/// and start timestamp so clients can connect to the running daemon.
pub fn write(addr: &SocketAddr) -> io::Result<()> {
    let lockfile = CoreLockfile {
        pid: std::process::id(),
        address: addr.to_string(),
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };

    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(&lockfile).map_err(io::Error::other)?;
    std::fs::write(&path, json)
}

/// Reads and deserializes the lockfile, returning `None` if the file does not exist.
pub fn read() -> io::Result<Option<CoreLockfile>> {
    let path = path();
    if !path.exists() {
        return Ok(None);
    }

    let contents = std::fs::read_to_string(&path)?;
    let lockfile: CoreLockfile =
        serde_json::from_str(&contents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(lockfile))
}

/// Deletes the lockfile on daemon shutdown (best-effort, errors are silently ignored).
pub fn remove() {
    let _ = std::fs::remove_file(path());
}

/// Checks whether a process with the given PID is still alive.
pub fn is_pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
            .unwrap_or(false)
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}
