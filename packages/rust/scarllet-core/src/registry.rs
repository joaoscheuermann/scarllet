use scarllet_sdk::manifest::{ModuleKind, ModuleManifest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct ModuleRegistry {
    modules: HashMap<PathBuf, ModuleManifest>,
    version: u64,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            version: 0,
        }
    }

    pub fn register(&mut self, path: PathBuf, manifest: ModuleManifest) {
        self.modules.insert(path, manifest);
        self.version += 1;
    }

    pub fn deregister(&mut self, path: &Path) -> Option<ModuleManifest> {
        let removed = self.modules.remove(path);
        if removed.is_some() {
            self.version += 1;
        }
        removed
    }

    pub fn by_kind(&self, kind: ModuleKind) -> Vec<(&PathBuf, &ModuleManifest)> {
        self.modules
            .iter()
            .filter(|(_, m)| m.kind == kind)
            .collect()
    }

    pub fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_manifest(name: &str) -> ModuleManifest {
        ModuleManifest {
            name: name.to_string(),
            kind: ModuleKind::Tool,
            version: "0.1.0".into(),
            description: "test".into(),
            input_schema: None,
            timeout_ms: Some(5000),
            capabilities: vec![],
            aliases: vec![],
        }
    }

    #[test]
    fn register_and_query() {
        let mut reg = ModuleRegistry::new();
        assert_eq!(reg.version(), 0);

        reg.register(PathBuf::from("/tmp/echo"), tool_manifest("echo"));
        assert_eq!(reg.version(), 1);
        assert_eq!(reg.by_kind(ModuleKind::Tool).len(), 1);
        assert_eq!(reg.by_kind(ModuleKind::Command).len(), 0);
    }

    #[test]
    fn deregister() {
        let mut reg = ModuleRegistry::new();
        let path = PathBuf::from("/tmp/echo");
        reg.register(path.clone(), tool_manifest("echo"));
        assert_eq!(reg.by_kind(ModuleKind::Tool).len(), 1);

        reg.deregister(&path);
        assert_eq!(reg.by_kind(ModuleKind::Tool).len(), 0);
        assert_eq!(reg.version(), 2);
    }

    #[test]
    fn deregister_missing_noop() {
        let mut reg = ModuleRegistry::new();
        reg.deregister(Path::new("/nonexistent"));
        assert_eq!(reg.version(), 0);
    }
}
