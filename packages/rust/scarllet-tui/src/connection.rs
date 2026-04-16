use std::io;
use std::time::Duration;

use scarllet_proto::proto::orchestrator_client::OrchestratorClient;
use scarllet_proto::proto::*;
use scarllet_sdk::lockfile;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

/// Establishes a bidirectional gRPC stream with the Core orchestrator.
///
/// Resolves the Core address (spawning Core if needed), opens the
/// `AttachTui` stream, and forwards incoming `CoreEvent`s to the TUI
/// event channel until the stream closes.
pub(crate) async fn connect_and_stream(
    event_tx: mpsc::Sender<CoreEvent>,
    message_rx: mpsc::Receiver<TuiMessage>,
) {
    let address = find_core_address().await;

    let Some(channel) = connect_to_core(&address).await else {
        return;
    };

    let mut client = OrchestratorClient::new(channel)
        .max_decoding_message_size(64 * 1024 * 1024)
        .max_encoding_message_size(64 * 1024 * 1024);
    let outgoing = ReceiverStream::new(message_rx);

    let Ok(response) = client.attach_tui(outgoing).await else {
        return;
    };

    let mut incoming = response.into_inner();
    while let Ok(Some(event)) = incoming.message().await {
        if event_tx.send(event).await.is_err() {
            return;
        }
    }
}

/// Attempts to connect to the Core gRPC endpoint with retries.
async fn connect_to_core(address: &str) -> Option<tonic::transport::Channel> {
    for _ in 0..10 {
        let endpoint = format!("http://{address}");
        if let Ok(ep) = tonic::transport::Endpoint::from_shared(endpoint) {
            if let Ok(Ok(channel)) =
                tokio::time::timeout(Duration::from_secs(3), ep.connect()).await
            {
                return Some(channel);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

/// Reads the lockfile for a running Core address, spawning a new Core process if absent.
async fn find_core_address() -> String {
    loop {
        if let Ok(Some(lock)) = lockfile::read() {
            if lockfile::is_pid_alive(lock.pid) {
                return lock.address;
            }
            lockfile::remove();
        }

        let _ = spawn_core();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Ok(Some(lock)) = lockfile::read() {
                if lockfile::is_pid_alive(lock.pid) {
                    return lock.address;
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Launches the Core binary as a detached background process.
fn spawn_core() -> io::Result<()> {
    let self_path = std::env::current_exe()?;
    let dir = self_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot determine binary dir"))?;
    let mut core_path = dir.join("core");
    if cfg!(windows) {
        core_path.set_extension("exe");
    }
    if !core_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Core binary not found at {}", core_path.display()),
        ));
    }
    std::process::Command::new(&core_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}
