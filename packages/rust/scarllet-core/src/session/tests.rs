use super::*;

fn empty_config() -> ScarlletConfig {
    ScarlletConfig::default()
}

fn make_session() -> Session {
    Session::new(
        "session-test".into(),
        SessionConfig::from_global(&empty_config()),
    )
}

#[test]
fn set_status_transition_returns_true_and_updates_state() {
    let mut s = make_session();
    assert_eq!(s.status, SessionStatus::Running);
    let changed = s.set_status(SessionStatus::Paused);
    assert!(changed, "transition must return true");
    assert_eq!(s.status, SessionStatus::Paused);
}

#[test]
fn set_status_idempotent_calls_return_false_and_do_not_mutate() {
    let mut s = make_session();
    // First transition: Running -> Paused.
    assert!(s.set_status(SessionStatus::Paused));
    let activity_after_first = s.last_activity;
    // Second call with the same value must be a no-op.
    let changed_again = s.set_status(SessionStatus::Paused);
    assert!(!changed_again, "idempotent call must return false");
    assert_eq!(s.status, SessionStatus::Paused);
    assert_eq!(
        s.last_activity, activity_after_first,
        "idempotent call must not bump last_activity"
    );
}

#[test]
fn set_status_back_to_running_returns_true_again() {
    let mut s = make_session();
    assert!(s.set_status(SessionStatus::Paused));
    assert!(
        s.set_status(SessionStatus::Running),
        "the reverse transition also counts as a transition"
    );
    assert_eq!(s.status, SessionStatus::Running);
}

#[tokio::test]
async fn create_session_returns_unique_id_and_inserts() {
    let mut reg = SessionRegistry::new();
    let id_a = reg.create_session(&empty_config());
    let id_b = reg.create_session(&empty_config());

    assert_ne!(id_a, id_b);
    assert_eq!(reg.len(), 2);
    assert!(reg.get(&id_a).is_some());
    assert!(reg.get(&id_b).is_some());
}

#[tokio::test]
async fn create_session_initialises_default_state() {
    let mut reg = SessionRegistry::new();
    let id = reg.create_session(&empty_config());
    let session = reg.get(&id).expect("just created");
    let s = session.read().await;
    assert_eq!(s.id, id);
    assert!(matches!(s.status, SessionStatus::Running));
    assert!(s.queue.is_empty());
    assert!(s.nodes.is_empty());
    assert_eq!(s.subscribers.len(), 0);
    assert_eq!(s.config.default_agent, "default");
    assert!(s.config.provider.is_none(), "no providers configured");
}

#[tokio::test]
async fn destroy_session_removes_and_returns_handle() {
    let mut reg = SessionRegistry::new();
    let id = reg.create_session(&empty_config());
    let removed = reg.destroy_session(&id);
    assert!(removed.is_some(), "destroy returns the dropped handle");
    assert!(reg.get(&id).is_none(), "session no longer registered");
    assert!(reg.is_empty());
}

#[tokio::test]
async fn destroy_unknown_session_returns_none() {
    let mut reg = SessionRegistry::new();
    let removed = reg.destroy_session("never-created");
    assert!(removed.is_none());
}
