//! Process-wide module manifest registry.
//!
//! Populated by [`crate::watcher`] on start-up and kept in sync with
//! filesystem events afterwards. Readers iterate by [`ModuleKind`] to
//! find agent / tool / command manifests.

use scarllet_sdk::manifest::{ModuleKind, ModuleManifest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Stores discovered module manifests (commands, tools, agents) keyed by filesystem path.
///
/// A monotonically increasing `version` is bumped on every mutation so callers
/// can detect stale snapshots.
pub struct ModuleRegistry {
    modules: HashMap<PathBuf, ModuleManifest>,
    version: u64,
}

impl Default for ModuleRegistry {
    /// Empty registry; identical to [`ModuleRegistry::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleRegistry {
    /// Initialises an empty registry at version 0.
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            version: 0,
        }
    }

    /// Inserts or replaces a module at the given path, bumping the version.
    pub fn register(&mut self, path: PathBuf, manifest: ModuleManifest) {
        self.modules.insert(path, manifest);
        self.version += 1;
    }

    /// Removes a module by path, returning the manifest if it existed.
    pub fn deregister(&mut self, path: &Path) -> Option<ModuleManifest> {
        let removed = self.modules.remove(path);
        if removed.is_some() {
            self.version += 1;
        }
        removed
    }

    /// Returns all modules matching the given kind (command, tool, or agent).
    pub fn by_kind(&self, kind: ModuleKind) -> Vec<(&PathBuf, &ModuleManifest)> {
        self.modules
            .iter()
            .filter(|(_, m)| m.kind == kind)
            .collect()
    }

    /// Returns the current snapshot version counter. Test-only accessor —
    /// the counter is bumped internally on every mutation so production
    /// code does not need to read it; it exists purely so tests can assert
    /// that register / deregister bumped the version.
    #[cfg(test)]
    pub fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod tests;
