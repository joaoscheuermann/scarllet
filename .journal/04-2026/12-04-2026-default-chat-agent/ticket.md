---
status: planning
created: 2026-04-12 15:19
slug: default-chat-agent
---

## Prompt

I want to create a default agent, that will answer whatever question I have, just to experiment with the TUI. This agent should consume our LLM api, this LLM api should consume the Gemini API.

Providers should be configurable in config.json. The agent should maintain conversation history. Agents should be long-lived processes that receive prompts via a bidirectional gRPC stream from Core (agent connects to Core as client).

## Research

(empty)

## Architecture

(empty)
