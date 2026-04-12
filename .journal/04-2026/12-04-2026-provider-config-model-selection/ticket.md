# Provider Configuration & Model Selection

## Summary

Replace the flat `credentials` HashMap in `config.json` with a structured `providers` array. All providers use the OpenAI-compatible API format. Model selection is owned by Core (not agents). Agents retrieve provider info via gRPC and are bound to the active provider/model for the duration of a round. The Gemini-specific adapter is removed.

## Acceptance Criteria

### AC-1: Config file format
- **Given** a config.json in `%APPDATA%/scarllet/`
- **When** it is loaded by Core
- **Then** it contains:
  - `active_provider`: string (name of the selected provider, or empty)
  - `providers`: array of provider objects, each with:
    - `name`: string (unique identifier)
    - `api_key`: string
    - `api_url`: string (base URL for OpenAI-compatible endpoint)
    - `models`: array of strings (available model IDs)
    - `active_model`: string (the model ID to use for this provider)

### AC-2: Config file auto-creation
- **Given** no config.json exists
- **When** Core starts
- **Then** Core creates the file with `{ "active_provider": "", "providers": [] }`

### AC-3: Empty/missing fields auto-populated
- **Given** a config.json exists but is missing `active_provider` or `providers`
- **When** Core loads the file
- **Then** Core defaults `active_provider` to `""` and `providers` to `[]`

### AC-4: All providers use OpenAI-compatible format
- **Given** any configured provider
- **When** an agent makes an LLM call
- **Then** the request uses OpenAI-compatible chat completions to `{api_url}/chat/completions`
- **And** `gemini.rs` is deleted

### AC-5: gRPC endpoint to retrieve active provider info
- **Given** an agent that needs to make an LLM call
- **When** it calls `GetActiveProvider` on Core
- **Then** Core responds with `name`, `api_url`, `api_key`, `active_model`
- **And** if no provider is active, responds with `configured = false`

### AC-6: Agent binding per round
- **Given** an agent processing a task
- **When** the user changes the config mid-task
- **Then** the agent continues with the provider/model it fetched at task start
- **And** re-fetches before processing the next `AgentTask`

### AC-7: No model configuration in the agent
- **Given** the default agent
- **When** it processes a task
- **Then** it obtains all provider info from Core gRPC, not from env vars or hardcoded values

### AC-8: TUI message when no provider is configured
- **Given** a user sends a prompt
- **When** no provider is configured
- **Then** the TUI displays: "No provider configured. Edit config.json at `<path>` to add a provider."

### AC-9: LLM errors surfaced to user
- **Given** an LLM call fails (HTTP error, auth error, invalid model)
- **When** the provider returns an error
- **Then** the error propagates as `AgentFailure` and the TUI displays it

### AC-10: No validation on active_model
- **Given** a provider with any `active_model` string
- **When** Core serves the active provider info
- **Then** Core passes it as-is; errors are handled at the HTTP/agent level

## Edge Cases

| # | Scenario | Behavior |
|---|----------|----------|
| E-1 | `active_provider` references a name not in `providers` | Agent gets `configured = false`; TUI shows error |
| E-2 | Provider exists but `active_model` is empty | HTTP error surfaced to user |
| E-3 | Provider's `api_url` is unreachable | Network error as `AgentFailure` |
| E-4 | Provider's `api_key` is invalid | 401/403 surfaced as `AgentFailure` |
| E-5 | Config file has invalid JSON | Core logs error, falls back to empty default |
| E-6 | Multiple agents running, active provider changed between rounds | Each agent re-fetches independently |
| E-7 | `providers` is empty but `active_provider` is set | Treated as "not configured" |
