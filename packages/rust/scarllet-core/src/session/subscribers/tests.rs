use super::*;

#[tokio::test]
async fn broadcast_delivers_to_all_subscribers() {
    let mut set = SubscriberSet::<u32>::new();
    let (tx1, mut rx1) = mpsc::channel(8);
    let (tx2, mut rx2) = mpsc::channel(8);
    set.push(tx1);
    set.push(tx2);

    set.broadcast(42);

    assert_eq!(rx1.recv().await.unwrap().unwrap(), 42);
    assert_eq!(rx2.recv().await.unwrap().unwrap(), 42);
    assert_eq!(set.len(), 2);
}

#[tokio::test]
async fn broadcast_prunes_closed_senders() {
    let mut set = SubscriberSet::<u32>::new();
    let (tx_alive, mut rx_alive) = mpsc::channel(8);
    let (tx_dead, rx_dead) = mpsc::channel::<Result<u32, Status>>(8);
    drop(rx_dead);

    set.push(tx_alive);
    set.push(tx_dead);

    set.broadcast(7);

    assert_eq!(set.len(), 1);
    assert_eq!(rx_alive.recv().await.unwrap().unwrap(), 7);
}

// Effort 07: two subscribers attached to the same `SubscriberSet` must
// receive an identical ordered sequence of broadcasts (backs the
// multi-TUI claim that both clients see the same diff stream).
#[tokio::test]
async fn multiple_subscribers_receive_identical_ordered_stream() {
    let mut set = SubscriberSet::<u32>::new();
    let (tx_a, mut rx_a) = mpsc::channel(8);
    let (tx_b, mut rx_b) = mpsc::channel(8);
    set.push(tx_a);
    set.push(tx_b);

    for v in [1, 2, 3, 42] {
        set.broadcast(v);
    }

    for expected in [1u32, 2, 3, 42] {
        assert_eq!(rx_a.recv().await.unwrap().unwrap(), expected);
        assert_eq!(rx_b.recv().await.unwrap().unwrap(), expected);
    }
}

// Effort 07 / AC-2.5: the destroy-on-last-detach rule is gated on
// `subscribers.len() == 0`. A single subscriber dropping while another
// is still attached must leave the set non-empty so destruction is
// NOT triggered. (The actual `destroy_session_inner` trigger lives in
// the service layer; this test pins the precondition it reads.)
#[tokio::test]
async fn single_subscriber_drop_does_not_empty_the_set() {
    let mut set = SubscriberSet::<u32>::new();
    let (tx_alive, mut rx_alive) = mpsc::channel(8);
    let (tx_drop, rx_drop) = mpsc::channel::<Result<u32, Status>>(8);
    set.push(tx_alive);
    set.push(tx_drop);
    assert_eq!(set.len(), 2);

    drop(rx_drop);

    // Dropping the receiver alone does not prune — broadcast is what
    // actually drives the retain pass. After the broadcast, one
    // subscriber remains (the "last one").
    set.broadcast(9);
    assert_eq!(set.len(), 1, "one subscriber remains; set is NOT empty");
    assert_eq!(rx_alive.recv().await.unwrap().unwrap(), 9);

    // Now the last one drops. The next broadcast must fully empty the
    // set — this is the condition the service layer gates destruction
    // on.
    drop(rx_alive);
    set.broadcast(10);
    assert!(set.is_empty(), "set empty once the last receiver is gone");
}
