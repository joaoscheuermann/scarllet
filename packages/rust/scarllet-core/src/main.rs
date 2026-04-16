mod agents;
mod events;
mod registry;
mod routing;
mod service;
mod sessions;
mod tasks;
mod tools;
mod watcher;

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Instant;

use agents::AgentRegistry;
use clap::Parser;
use registry::ModuleRegistry;
use scarllet_proto::proto::orchestrator_server::OrchestratorServer;
use scarllet_sdk::config;
use scarllet_sdk::lockfile;
use service::OrchestratorService;
use sessions::TuiSessionRegistry;
use std::sync::Arc;
use tasks::TaskManager;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_stream::wrappers::TcpListenerStream;
use tracing::info;

/// CLI argument definition for the core orchestrator binary.
#[derive(Parser)]
#[command(name = "scarllet-core", about = "Scarllet Core Orchestrator")]
struct Cli {}

/// Bootstraps the gRPC server, module watcher, and config watcher, then
/// blocks until a Ctrl-C signal triggers graceful shutdown.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _cli = Cli::parse();
    tracing_subscriber::fmt::init();

    let started_at = Instant::now();
    let registry = Arc::new(RwLock::new(ModuleRegistry::new()));
    let task_manager = Arc::new(RwLock::new(TaskManager::new()));
    let session_registry = Arc::new(RwLock::new(TuiSessionRegistry::new()));
    let agent_registry = Arc::new(RwLock::new(AgentRegistry::new()));

    let cfg = config::load().unwrap_or_default();
    info!("Loaded {} provider(s) from config", cfg.providers.len());
    let config = Arc::new(RwLock::new(cfg));

    let dirs = watcher::watched_dirs();
    watcher::ensure_dirs(&dirs);

    let watcher_registry = Arc::clone(&registry);
    tokio::spawn(async move {
        watcher::run(watcher_registry, dirs).await;
    });

    let watcher_config = Arc::clone(&config);
    let watcher_sessions = Arc::clone(&session_registry);
    tokio::spawn(async move {
        watcher::watch_config(watcher_config, watcher_sessions).await;
    });

    let addr: SocketAddr = "127.0.0.1:0".parse()?;
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    let bound_addr_str = bound_addr.to_string();

    info!("Listening on {}", bound_addr);

    lockfile::write(&bound_addr)?;

    let service = OrchestratorService {
        started_at,
        registry,
        config,
        task_manager,
        session_registry,
        agent_registry,
        bound_addr: bound_addr_str,
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = Mutex::new(Some(shutdown_tx));
    ctrlc::set_handler(move || {
        if let Some(tx) = shutdown_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
    })?;

    let incoming = TcpListenerStream::new(listener);

    tonic::transport::Server::builder()
        .add_service(
            OrchestratorServer::new(service)
                .max_decoding_message_size(64 * 1024 * 1024)
                .max_encoding_message_size(64 * 1024 * 1024),
        )
        .serve_with_incoming_shutdown(incoming, async {
            let _ = shutdown_rx.await;
            info!("Shutdown signal received");
        })
        .await?;

    lockfile::remove();
    info!("Core stopped");
    println!("Core stopped");

    Ok(())
}
