use crate::registry::ModuleRegistry;
use crate::sessions::TuiSessionRegistry;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use scarllet_proto::proto::core_event;
use scarllet_proto::proto::{CoreEvent, ProviderInfoEvent};
use scarllet_sdk::config::{self, ScarlletConfig};
use scarllet_sdk::manifest::ModuleManifest;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Returns local dirs (sibling to binary) first, then user dirs (%APPDATA%).
/// User dirs are scanned after local, so user modules override shipped defaults.
pub fn watched_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            dirs.push(bin_dir.join("commands"));
            dirs.push(bin_dir.join("tools"));
            dirs.push(bin_dir.join("agents"));
        }
    }

    let user_base = dirs::config_dir()
        .expect("could not determine OS config directory")
        .join("scarllet");
    dirs.push(user_base.join("commands"));
    dirs.push(user_base.join("tools"));
    dirs.push(user_base.join("agents"));

    dirs
}

pub fn ensure_dirs(dirs: &[PathBuf]) {
    for d in dirs {
        if let Err(e) = std::fs::create_dir_all(d) {
            warn!("Failed to create directory {}: {e}", d.display());
        }
    }
}

pub async fn run(registry: Arc<RwLock<ModuleRegistry>>, dirs: Vec<PathBuf>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<notify::Result<Event>>(256);

    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = tx.blocking_send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            warn!("Failed to create file watcher: {e}");
            return;
        }
    };

    for d in &dirs {
        if let Err(e) = watcher.watch(d, RecursiveMode::NonRecursive) {
            warn!("Failed to watch {}: {e}", d.display());
        } else {
            info!("Watching {}", d.display());
        }
    }

    // Initial scan of existing files
    for d in &dirs {
        if let Ok(entries) = std::fs::read_dir(d) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    handle_file_added(&registry, &path).await;
                }
            }
        }
    }

    while let Some(event) = rx.recv().await {
        let event = match event {
            Ok(e) => e,
            Err(e) => {
                debug!("Watcher error: {e}");
                continue;
            }
        };

        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                for path in &event.paths {
                    if path.is_file() {
                        handle_file_added(&registry, path).await;
                    }
                }
            }
            EventKind::Remove(_) => {
                for path in &event.paths {
                    handle_file_removed(&registry, path).await;
                }
            }
            _ => {}
        }
    }

    // Keep watcher alive
    drop(watcher);
}

async fn handle_file_added(registry: &Arc<RwLock<ModuleRegistry>>, path: &Path) {
    match probe_manifest(path).await {
        Some(manifest) => {
            info!("Registered {}: {}", manifest.kind_str(), manifest.name);
            registry.write().await.register(path.to_path_buf(), manifest);
        }
        None => {
            debug!("Ignoring {}", path.display());
        }
    }
}

async fn handle_file_removed(registry: &Arc<RwLock<ModuleRegistry>>, path: &Path) {
    if let Some(manifest) = registry.write().await.deregister(path) {
        info!("Deregistered {}: {}", manifest.kind_str(), manifest.name);
    }
}

async fn probe_manifest(path: &Path) -> Option<ModuleManifest> {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::process::Command::new(path)
            .arg("--manifest")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .await
    })
    .await;

    let output = match result {
        Ok(Ok(o)) if o.status.success() => o,
        _ => return None,
    };

    serde_json::from_slice(&output.stdout).ok()
}

pub async fn watch_config(
    config: Arc<RwLock<ScarlletConfig>>,
    session_registry: Arc<RwLock<TuiSessionRegistry>>,
) {
    let config_file = config::config_path();
    let Some(config_dir) = config_file.parent() else {
        warn!("Cannot determine config directory");
        return;
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel::<notify::Result<Event>>(64);

    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = tx.blocking_send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            warn!("Failed to create config watcher: {e}");
            return;
        }
    };

    if let Err(e) = watcher.watch(config_dir, RecursiveMode::NonRecursive) {
        warn!("Failed to watch config dir {}: {e}", config_dir.display());
        return;
    }

    info!("Watching config at {}", config_file.display());

    while let Some(event) = rx.recv().await {
        let event = match event {
            Ok(e) => e,
            Err(e) => {
                debug!("Config watcher error: {e}");
                continue;
            }
        };

        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
            continue;
        }

        let is_config = event.paths.iter().any(|p| p.ends_with("config.json"));
        if !is_config {
            continue;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;

        match config::load() {
            Ok(new_cfg) => {
                let provider_count = new_cfg.providers.len();
                let active = new_cfg.provider.clone();
                *config.write().await = new_cfg;
                info!(
                    "Config reloaded: {} provider(s), active='{active}'",
                    provider_count
                );

                let cfg = config.read().await;
                let event = match cfg.active_provider() {
                    Some(p) => CoreEvent {
                        payload: Some(core_event::Payload::ProviderInfo(ProviderInfoEvent {
                            provider_name: p.name.clone(),
                            model: p.model.clone(),
                            reasoning_effort: p
                                .reasoning_effort()
                                .unwrap_or_default()
                                .to_string(),
                        })),
                    },
                    None => CoreEvent {
                        payload: Some(core_event::Payload::ProviderInfo(ProviderInfoEvent {
                            provider_name: String::new(),
                            model: String::new(),
                            reasoning_effort: String::new(),
                        })),
                    },
                };
                drop(cfg);
                session_registry.read().await.broadcast(event);
            }
            Err(e) => {
                warn!("Failed to reload config: {e}");
            }
        }
    }

    drop(watcher);
}

trait ManifestExt {
    fn kind_str(&self) -> &'static str;
}

impl ManifestExt for ModuleManifest {
    fn kind_str(&self) -> &'static str {
        match self.kind {
            scarllet_sdk::manifest::ModuleKind::Command => "command",
            scarllet_sdk::manifest::ModuleKind::Tool => "tool",
            scarllet_sdk::manifest::ModuleKind::Agent => "agent",
        }
    }
}
