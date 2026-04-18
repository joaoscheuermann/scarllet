use super::*;

fn prompt(id: &str, text: &str) -> QueuedPrompt {
    QueuedPrompt {
        prompt_id: id.into(),
        text: text.into(),
        working_directory: String::new(),
        user_node_id: String::new(),
    }
}

#[test]
fn push_and_pop_preserve_fifo_order() {
    let mut q = SessionQueue::new();
    q.push_back(prompt("a", "first"));
    q.push_back(prompt("b", "second"));
    assert_eq!(q.len(), 2);
    assert_eq!(q.pop_front().unwrap().prompt_id, "a");
    assert_eq!(q.pop_front().unwrap().prompt_id, "b");
    assert!(q.is_empty());
}

#[test]
fn snapshot_clones_without_draining() {
    let mut q = SessionQueue::new();
    q.push_back(prompt("only", "hello"));
    let snap = q.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(q.len(), 1);
}

#[test]
fn snapshot_preserves_fifo_order_after_mixed_push_pop() {
    let mut q = SessionQueue::new();
    q.push_back(prompt("a", "1"));
    q.push_back(prompt("b", "2"));
    q.pop_front();
    q.push_back(prompt("c", "3"));
    q.push_back(prompt("d", "4"));

    let ids: Vec<String> = q.snapshot().into_iter().map(|p| p.prompt_id).collect();
    assert_eq!(ids, vec!["b", "c", "d"]);
}

#[test]
fn clear_empties_the_queue_without_returning() {
    let mut q = SessionQueue::new();
    q.push_back(prompt("a", "1"));
    q.push_back(prompt("b", "2"));
    q.clear();
    assert!(q.is_empty());
    assert_eq!(q.snapshot().len(), 0);
}

#[test]
fn pop_front_on_empty_returns_none() {
    let mut q = SessionQueue::new();
    assert!(q.pop_front().is_none());
}
