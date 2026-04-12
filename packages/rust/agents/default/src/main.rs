use scarllet_llm::types::{ChatMessage, ChatRequest, Role};
use scarllet_llm::LlmClient;
use scarllet_proto::proto::agent_message;
use scarllet_proto::proto::orchestrator_client::OrchestratorClient;
use scarllet_proto::proto::*;
use tokio_stream::wrappers::ReceiverStream;
use tracing::info;

const SYSTEM_PROMPT: &str = "You are a helpful assistant.";

fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "default",
        "kind": "agent",
        "version": "0.1.0",
        "description": "Default chat agent — answers questions using an LLM"
    });
    println!("{}", serde_json::to_string(&manifest).unwrap());
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--manifest") {
        print_manifest();
        return Ok(());
    }

    tracing_subscriber::fmt::init();

    let core_addr =
        std::env::var("SCARLLET_CORE_ADDR").unwrap_or_else(|_| "127.0.0.1:50051".to_string());

    info!("Default agent starting, connecting to Core at {core_addr}");

    let endpoint = format!("http://{core_addr}");
    let mut client = OrchestratorClient::connect(endpoint).await?;

    let (msg_tx, msg_rx) = tokio::sync::mpsc::channel::<AgentMessage>(64);
    let outgoing = ReceiverStream::new(msg_rx);

    let response = client.agent_stream(outgoing).await?;
    let mut task_stream = response.into_inner();

    let register = AgentMessage {
        payload: Some(agent_message::Payload::Register(AgentRegister {
            agent_name: "default".into(),
        })),
    };
    msg_tx.send(register).await?;
    info!("Registered as 'default' agent");

    let mut history: Vec<ChatMessage> = Vec::new();

    while let Some(task) = task_stream.message().await? {
        info!(
            "Received task {}: {}",
            task.task_id,
            &task.prompt[..task.prompt.len().min(50)]
        );

        let provider_resp = client
            .get_active_provider(ActiveProviderQuery {})
            .await
            .map_err(|e| format!("GetActiveProvider RPC failed: {e}"))?
            .into_inner();

        if !provider_resp.configured {
            let failure = AgentMessage {
                payload: Some(agent_message::Payload::Failure(AgentFailure {
                    task_id: task.task_id,
                    error: "No provider configured.".into(),
                })),
            };
            let _ = msg_tx.send(failure).await;
            continue;
        }

        let llm = LlmClient::new(provider_resp.api_url, provider_resp.api_key);

        history.push(ChatMessage {
            role: Role::User,
            content: task.prompt.clone(),
        });

        let progress = AgentMessage {
            payload: Some(agent_message::Payload::Progress(AgentProgressMsg {
                task_id: task.task_id.clone(),
                content: String::new(),
            })),
        };
        let _ = msg_tx.send(progress).await;

        let mut messages = vec![ChatMessage {
            role: Role::System,
            content: SYSTEM_PROMPT.to_string(),
        }];
        messages.extend(history.clone());

        let request = ChatRequest {
            model: provider_resp.model,
            messages,
            temperature: None,
            max_tokens: None,
        };

        match llm.chat(request).await {
            Ok(resp) => {
                history.push(ChatMessage {
                    role: Role::Assistant,
                    content: resp.message.content.clone(),
                });

                let result = AgentMessage {
                    payload: Some(agent_message::Payload::Result(AgentResultMsg {
                        task_id: task.task_id,
                        content: resp.message.content,
                    })),
                };
                let _ = msg_tx.send(result).await;
            }
            Err(e) => {
                let failure = AgentMessage {
                    payload: Some(agent_message::Payload::Failure(AgentFailure {
                        task_id: task.task_id,
                        error: e.to_string(),
                    })),
                };
                let _ = msg_tx.send(failure).await;
            }
        }
    }

    info!("Task stream closed, agent exiting");
    Ok(())
}
