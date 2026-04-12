# Architecture Plan — Default Chat Agent + Gemini Provider

## System Boundaries

```
┌─────────────────────────────────────────────────────────────┐
│                        TUI (existing)                       │
│  AttachTui stream ←──────────────────→ Core                 │
└─────────────────────────────────────────────────────────────┘
                              │
                   ┌──────────┴──────────┐
                   │   Core Orchestrator  │
                   │                      │
                   │  ┌────────────────┐  │
                   │  │ AgentRegistry  │  │  ← tracks running agent streams
                   │  └────────────────┘  │
                   │  ┌────────────────┐  │
                   │  │ TuiSessions    │  │  ← broadcasts CoreEvents (existing)
                   │  └────────────────┘  │
                   └──────────┬──────────┘
                              │
                    AgentStream RPC
                    (bidi gRPC stream)
                              │
                   ┌──────────┴──────────┐
                   │   Chat Agent Binary  │
                   │                      │
                   │  ┌────────────────┐  │
                   │  │ ConversationHx │  │  ← in-memory message history
                   │  └────────────────┘  │
                   │  ┌────────────────┐  │
                   │  │ LlmClient      │──┼──→ GetCredentials RPC (Core)
                   │  └────────────────┘  │
                   │          │           │
                   │  ┌───────┴────────┐  │
                   │  │ GeminiProvider  │──┼──→ generativelanguage.googleapis.com
                   │  └────────────────┘  │
                   └──────────────────────┘
```

## Component Interactions

### Prompt flow (happy path)

1. User types in TUI → `PromptMessage` sent via `AttachTui` stream to Core
2. Core's `route_prompt` checks if agent "chat" has a live stream
   - **Not running**: Core spawns the agent binary. Agent starts, connects to Core, opens `AgentStream`. Core sends `AgentTask` with the prompt.
   - **Already running**: Core sends `AgentTask` directly through the existing stream.
3. Core broadcasts `AgentStartedEvent` to TUI sessions
4. Agent receives `AgentTask`, appends user message to conversation history
5. Agent sends `AgentProgress { status: "thinking" }` back through the stream
6. Core receives it, maps to `AgentThinkingEvent`, broadcasts to TUI
7. Agent calls `LlmClient::chat()` with full conversation history
8. `LlmClient` resolves Gemini API key via `GetCredentials` unary RPC
9. `GeminiProvider` translates `ChatRequest` → Gemini `generateContent`, sends HTTP request
10. Gemini responds → `GeminiProvider` normalizes to `ChatResponse`
11. Agent appends assistant response to conversation history
12. Agent sends `AgentResult { content }` back through the stream
13. Core maps to `AgentResponseEvent`, broadcasts to TUI
14. TUI renders the response, unlocks input

### Agent lifecycle

```
spawn binary ──→ agent starts ──→ connects to Core ──→ opens AgentStream
                                                              │
                                                    sends AgentRegister
                                                              │
                                              Core registers stream sender
                                                              │
                                              ┌───── receives AgentTask ◄──── Core routes prompt
                                              │               │
                                              │    process + call LLM
                                              │               │
                                              │    sends AgentProgress/AgentResult
                                              │               │
                                              └───────────────┘  (loop for each prompt)
                                                              │
                                              stream closes ──→ Core deregisters agent
```

## Crate Impact Map

| Crate | Change | New? |
|-------|--------|------|
| `scarllet-proto` | Add `AgentStream` RPC + 5 new message types | No |
| `scarllet-sdk` | No changes needed (config model sufficient as-is) | No |
| `scarllet-llm` | Add `GeminiProvider` adapter module | No |
| `scarllet-core` | Add `AgentRegistry` for live agent streams, implement `AgentStream` RPC, update `route_prompt` | No |
| `scarllet-chat-agent` | New agent binary crate | **Yes** |

## Implementation Order

1. **Proto**: Add `AgentStream` RPC and messages (foundation for everything else)
2. **LLM — Gemini adapter**: Add `GeminiProvider`, update `LlmClient` routing (independent of Core changes)
3. **Core — AgentRegistry + AgentStream**: Implement the RPC, track live agent streams, bridge events to TUI sessions
4. **Core — route_prompt update**: Route to live agent stream instead of spawning per-prompt
5. **Chat agent binary**: New crate that connects, receives tasks, calls LLM, maintains history
6. **Integration verification**: End-to-end test with Core + TUI + agent + Gemini

## Verification Plan

1. `npx nx run-many -t build` — all crates compile
2. `npx nx run-many -t test` — all tests pass
3. Start Core → place chat agent binary in agents dir → Core discovers it
4. Start TUI → type a prompt → see "thinking..." → see Gemini response
5. Send a second prompt → agent reuses stream → response includes conversation context
6. Kill agent process → TUI shows error → next prompt spawns new agent

## Principles Applied

| Principle | Application |
|-----------|-------------|
| **SRP** | GeminiProvider owns Gemini-specific translation only. AgentRegistry owns live-stream tracking. Chat agent owns conversation + LLM calls. |
| **OCP** | `LlmProvider` trait is the extension seam — adding Gemini doesn't modify OpenAI code. `AgentStream` is additive to the proto (existing RPCs unchanged). |
| **DIP** | Agent depends on proto contracts (AgentStream), not Core internals. LlmClient depends on LlmProvider trait, not vendor APIs. |
| **ISP** | AgentMessage uses oneof — each variant carries only its fields. AgentTask is a focused payload (task_id, prompt, working_dir). |
| **KISS** | One new crate (chat-agent). Config model unchanged. No generic "agent framework" — just a binary that opens a stream. |
| **DRY** | Proto is single source of truth for agent-core contract. LlmProvider trait shared across all providers. |

## Risks

| Risk | Mitigation |
|------|-----------|
| Agent crash loses conversation history | Acceptable for MVP. Future: persist history to disk. |
| Gemini API format changes | Adapter is isolated in one module; easy to update. |
| Long-lived agent holds memory indefinitely | MVP accepts this. Future: prune history or cap token count. |
| Race between agent spawn and first task delivery | Core waits for AgentRegister before sending tasks. |
