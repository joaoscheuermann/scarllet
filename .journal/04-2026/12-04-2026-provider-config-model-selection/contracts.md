# Contracts — gRPC Changes

## Summary of changes to `orchestrator.proto`

| Change | Type | Rationale |
|--------|------|-----------|
| Add `GetActiveProvider` RPC | New | Agents call this to get the current provider name, URL, key, and model |
| Remove `GetCredentials` RPC | Breaking | Superseded by `GetActiveProvider` |
| Remove `SetCredential` RPC | Breaking | Config editing is manual for now; no runtime mutation path needed |
| Remove `CredentialQuery`, `CredentialResponse`, `SetCredentialRequest`, `SetCredentialResponse` | Breaking | Orphaned messages from removed RPCs |

## New RPC

```protobuf
service Orchestrator {
  // ... existing RPCs ...
  rpc GetActiveProvider(ActiveProviderQuery) returns (ActiveProviderResponse);
}
```

## New messages

```protobuf
message ActiveProviderQuery {}

message ActiveProviderResponse {
  bool configured = 1;
  string provider_name = 2;
  string api_url = 3;
  string api_key = 4;
  string model = 5;
}
```

### Field semantics

| Field | Type | Description |
|-------|------|-------------|
| `configured` | bool | `true` if an active provider was resolved; `false` if providers list is empty, `active_provider` is empty, or the named provider is not found |
| `provider_name` | string | Name of the active provider (empty if not configured) |
| `api_url` | string | Base URL for OpenAI-compatible API (empty if not configured) |
| `api_key` | string | Bearer token (empty if not configured) |
| `model` | string | The `active_model` from the active provider (empty if not configured) |

### Behavior contract

1. Core reads its in-memory `ScarlletConfig` (loaded from `config.json` at startup).
2. If `active_provider` is empty, or no provider in the `providers` array matches the name → return `configured = false`, all other fields empty.
3. If a matching provider is found → return `configured = true` with the provider's `name`, `api_url`, `api_key`, and `active_model`.
4. Core does NOT validate `active_model` against the provider's `models` list.

## Removed messages

```protobuf
// REMOVED — replaced by ActiveProviderQuery / ActiveProviderResponse
// message CredentialQuery { string provider = 1; }
// message CredentialResponse { string provider = 1; string api_key = 2; bool found = 3; }
// message SetCredentialRequest { string provider = 1; string api_key = 2; }
// message SetCredentialResponse { bool success = 1; }
```

## Full updated service definition

```protobuf
service Orchestrator {
  rpc Ping(PingRequest) returns (PingResponse);
  rpc ListCommands(ListCommandsRequest) returns (ListCommandsResponse);
  rpc GetToolRegistry(ToolRegistryQuery) returns (ToolRegistryResponse);
  rpc GetActiveProvider(ActiveProviderQuery) returns (ActiveProviderResponse);
  rpc InvokeTool(ToolInvocation) returns (ToolResult);
  rpc SubmitTask(TaskSubmission) returns (TaskReceipt);
  rpc CancelTask(CancelRequest) returns (CancelResponse);
  rpc ReportProgress(ProgressReport) returns (Ack);
  rpc GetAgentStatus(AgentStatusQuery) returns (AgentStatusResponse);
  rpc AttachTui(stream TuiMessage) returns (stream CoreEvent);
  rpc AgentStream(stream AgentMessage) returns (stream AgentTask);
}
```

## Agent-side call pattern

```rust
// At the start of each round (before processing a new AgentTask):
let resp = client.get_active_provider(ActiveProviderQuery {}).await?;
if !resp.configured {
    // Send AgentFailure: "No provider configured"
    return;
}
// Use resp.api_url, resp.api_key, resp.model for the LLM call
// Remain bound to these values for the entire round
```
