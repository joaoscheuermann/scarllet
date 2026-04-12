# API Contracts — Default Chat Agent

## 1. Proto — AgentStream RPC (additive to `orchestrator.proto`)

```protobuf
// Added to the Orchestrator service — existing RPCs unchanged
service Orchestrator {
  // ... all existing RPCs ...
  rpc AgentStream(stream AgentMessage) returns (stream AgentTask);
}

// ── Agent → Core (upstream) ──

message AgentMessage {
  oneof payload {
    AgentRegister register = 1;
    AgentProgress progress = 2;
    AgentResult result = 3;
    AgentFailure failure = 4;
  }
}

message AgentRegister {
  string agent_name = 1;
}

message AgentProgress {
  string task_id = 1;
  string content = 2;
}

message AgentResult {
  string task_id = 1;
  string content = 2;
}

message AgentFailure {
  string task_id = 1;
  string error = 2;
}

// ── Core → Agent (downstream) ──

message AgentTask {
  string task_id = 1;
  string prompt = 2;
  string working_directory = 3;
}
```

### Sequence of operations

```
Agent                          Core
  │                              │
  │── AgentRegister ───────────→ │  (agent identifies itself)
  │                              │  Core registers stream sender in AgentRegistry
  │                              │
  │                              │  ... user sends prompt in TUI ...
  │                              │
  │  ◄──────────── AgentTask ────│  (Core pushes task)
  │                              │
  │── AgentProgress ───────────→ │  (status: thinking + content)
  │                              │  Core broadcasts AgentThinkingEvent to TUI
  │                              │
  │── AgentResult ─────────────→ │  (final response)
  │                              │  Core broadcasts AgentResponseEvent to TUI
  │                              │
  │  ◄──────────── AgentTask ────│  (next prompt)
  │   ...                        │
```

### Error flow

```
Agent                          Core
  │                              │
  │── AgentFailure ────────────→ │  (error message)
  │                              │  Core broadcasts AgentErrorEvent to TUI
  │                              │  Agent stays alive for next prompt
```

### Stream close

```
Agent crashes or exits           Core
  │                              │
  │ ─── stream drops ──────────→ │  Core deregisters agent from AgentRegistry
                                 │  If task was in-progress: broadcast AgentErrorEvent
```

## 2. Gemini HTTP API (external, consumed by GeminiProvider)

### Request — POST `https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`

Query parameter: `key={API_KEY}`

```json
{
  "systemInstruction": {
    "parts": [{ "text": "You are a helpful assistant." }]
  },
  "contents": [
    { "role": "user", "parts": [{ "text": "Hello" }] },
    { "role": "model", "parts": [{ "text": "Hi there!" }] },
    { "role": "user", "parts": [{ "text": "What is Rust?" }] }
  ]
}
```

### Response

```json
{
  "candidates": [
    {
      "content": {
        "parts": [{ "text": "Rust is a systems programming language..." }],
        "role": "model"
      },
      "finishReason": "STOP"
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 25,
    "candidatesTokenCount": 150,
    "totalTokenCount": 175
  }
}
```

### Translation rules (ChatRequest → Gemini format)

| Our type | Gemini field |
|----------|-------------|
| `ChatMessage { role: System, content }` | `systemInstruction.parts[0].text` |
| `ChatMessage { role: User, content }` | `contents[].role = "user"`, `parts[0].text = content` |
| `ChatMessage { role: Assistant, content }` | `contents[].role = "model"`, `parts[0].text = content` |
| `ChatRequest.model` | URL path: `/models/{model}:generateContent` |
| `ChatRequest.temperature` | `generationConfig.temperature` |
| `ChatRequest.max_tokens` | `generationConfig.maxOutputTokens` |

### Translation rules (Gemini response → ChatResponse)

| Gemini field | Our type |
|-------------|----------|
| `candidates[0].content.parts[0].text` | `ChatResponse.message.content` |
| `candidates[0].content.role` ("model") | `ChatResponse.message.role = Assistant` |
| `candidates[0].finishReason` | `ChatResponse.finish_reason` (lowercase) |
| `usageMetadata.promptTokenCount` | `Usage.prompt_tokens` |
| `usageMetadata.candidatesTokenCount` | `Usage.completion_tokens` |
| `usageMetadata.totalTokenCount` | `Usage.total_tokens` |

### Error mapping

| HTTP status | LlmError variant |
|-------------|-----------------|
| 401, 403 | `Unauthorized` |
| 429 | `RateLimited { retry_after }` |
| 500+ | `ServerError { status, body }` |
| Network failure | `NetworkError(message)` |
| Empty candidates | `InvalidResponse("No candidates")` |

## 3. Core internal — AgentRegistry interface

```rust
pub struct AgentRegistry {
    // Maps agent_name → channel sender for pushing AgentTask
    agents: HashMap<String, mpsc::Sender<AgentTask>>,
}

impl AgentRegistry {
    pub fn register(&mut self, name: String, sender: mpsc::Sender<AgentTask>);
    pub fn deregister(&mut self, name: &str);
    pub fn get(&self, name: &str) -> Option<&mpsc::Sender<AgentTask>>;
    pub fn is_running(&self, name: &str) -> bool;
}
```

Not a proto contract — internal Core type. Tracks which agent names have an active bidirectional stream.
