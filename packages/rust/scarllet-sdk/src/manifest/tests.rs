use super::*;

#[test]
fn parse_tool_manifest() {
    let json = r#"{
        "name": "echo-tool",
        "kind": "tool",
        "version": "0.1.0",
        "description": "Echoes input back",
        "timeout_ms": 5000
    }"#;
    let m: ModuleManifest = serde_json::from_str(json).unwrap();
    assert_eq!(m.name, "echo-tool");
    assert_eq!(m.kind, ModuleKind::Tool);
    assert_eq!(m.timeout_ms, Some(5000));
    assert!(m.input_schema.is_none());
    assert!(m.aliases.is_empty());
}

#[test]
fn parse_command_manifest() {
    let json = r#"{
        "name": "setup",
        "kind": "command",
        "version": "0.1.0",
        "description": "Configure credentials",
        "aliases": ["/setup", "/config"]
    }"#;
    let m: ModuleManifest = serde_json::from_str(json).unwrap();
    assert_eq!(m.kind, ModuleKind::Command);
    assert_eq!(m.aliases, vec!["/setup", "/config"]);
}
