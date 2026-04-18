use super::*;

#[test]
fn missing_env_var_is_reported() {
    // Use an env var name that should not exist in the test runner's environment.
    let err = read_env("DEFINITELY_NOT_SET_IN_TESTS_42").unwrap_err();
    assert!(matches!(err, AgentSdkError::MissingEnv(_)));
}

#[test]
fn agent_sdk_error_displays_helpfully() {
    let err = AgentSdkError::ChannelClosed;
    assert_eq!(err.to_string(), "agent stream channel closed");
}

#[test]
fn tool_status_wire_strings_are_canonical() {
    assert_eq!(ToolStatus::Pending.as_wire(), "pending");
    assert_eq!(ToolStatus::Running.as_wire(), "running");
    assert_eq!(ToolStatus::Done.as_wire(), "done");
    assert_eq!(ToolStatus::Failed.as_wire(), "failed");
    assert_eq!(format!("{}", ToolStatus::Done), "done");
}
