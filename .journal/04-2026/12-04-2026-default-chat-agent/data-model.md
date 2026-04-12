# Data Model — Default Chat Agent

## 1. Config (existing, no changes needed)

File: `%APPDATA%/scarllet/config.json`

```json
{
  "credentials": {
    "gemini": { "api_key": "AIza..." },
    "openai": { "api_key": "sk-..." }
  }
}
```

Rust types (existing in `scarllet-sdk/src/config.rs`):

```rust
pub struct ScarlletConfig {
    pub credentials: HashMap<String, ProviderCredential>,
}

pub struct ProviderCredential {
    pub api_key: String,
}
```

No changes to the config model. The provider name (e.g., `"gemini"`) is the map key. The model is selected at runtime by the agent in the `ChatRequest.model` field — not stored in config.

## 2. Conversation History (in-memory, chat agent)

The chat agent maintains conversation history as a `Vec<ChatMessage>` in its process memory.

```rust
struct Conversation {
    system_prompt: String,
    history: Vec<ChatMessage>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `system_prompt` | `String` | Prepended as a System message on every LLM call. Default: `"You are a helpful assistant."` |
| `history` | `Vec<ChatMessage>` | Ordered list of User/Assistant messages from the session |

### Lifecycle

- Created when agent starts
- Appended on each user prompt (User) and LLM response (Assistant)
- Lost when agent process exits or crashes
- Not persisted to disk (MVP)

### LLM call construction

Each call to `LlmClient::chat()` builds a `ChatRequest` containing:
1. `ChatMessage { role: System, content: system_prompt }`
2. All entries from `history` (full conversation context)

## 3. Agent Manifest (stdout JSON on `--manifest`)

```json
{
  "name": "chat",
  "kind": "agent",
  "version": "0.1.0",
  "description": "Default chat agent — answers questions using Gemini"
}
```

Matches existing `ModuleManifest` schema in `scarllet-sdk/src/manifest.rs`. No new fields needed.

## 4. AgentRegistry (Core in-memory state)

```rust
struct LiveAgent {
    sender: mpsc::Sender<AgentTask>,
}

struct AgentRegistry {
    agents: HashMap<String, LiveAgent>,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `agents` | `HashMap<String, LiveAgent>` | Maps agent name → channel for pushing tasks |
| `sender` | `mpsc::Sender<AgentTask>` | Core writes `AgentTask` here; agent reads from the other end via the gRPC stream |

### Lifecycle

- Entry created when agent sends `AgentRegister` through `AgentStream`
- Entry removed when the stream closes (agent exits, crashes, or disconnects)
- At most one entry per agent name (second registration replaces the first)

## 5. Proto Message Types (new)

| Message | Direction | Fields |
|---------|-----------|--------|
| `AgentRegister` | Agent → Core | `agent_name: string` |
| `AgentProgress` | Agent → Core | `task_id: string`, `content: string` |
| `AgentResult` | Agent → Core | `task_id: string`, `content: string` |
| `AgentFailure` | Agent → Core | `task_id: string`, `error: string` |
| `AgentTask` | Core → Agent | `task_id: string`, `prompt: string`, `working_directory: string` |

All defined in `orchestrator.proto`. See `contracts.md` for full protobuf definitions.

## 6. Gemini Provider Internal Types (scarllet-llm)

```rust
// Request to Gemini API
struct GeminiRequest {
    system_instruction: Option<GeminiContent>,
    contents: Vec<GeminiContent>,
    generation_config: Option<GeminiGenerationConfig>,
}

struct GeminiContent {
    role: Option<String>,       // "user" or "model"; omitted for systemInstruction
    parts: Vec<GeminiPart>,
}

struct GeminiPart {
    text: String,
}

struct GeminiGenerationConfig {
    temperature: Option<f32>,
    max_output_tokens: Option<u32>,
}

// Response from Gemini API
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

struct GeminiCandidate {
    content: GeminiContent,
    finish_reason: Option<String>,
}

struct GeminiUsageMetadata {
    prompt_token_count: u32,
    candidates_token_count: u32,
    total_token_count: u32,
}
```

These are private to the `gemini` module in `scarllet-llm`. They serialize/deserialize the Gemini HTTP API format and are never exposed outside the module.
