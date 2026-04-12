---
status: planning
created: 2026-04-12 18:25
slug: project-reorg-release
---

## Prompt

Reorganize the workspace: agents go under packages/rust/agents/<name>, rename scarllet-chat-agent to agents/default. Create a release build target that compiles --release and copies all executables to release/ at the project root (core.exe, tui.exe, agents/, tools/, commands/). Core should watch both local dirs (sibling to its binary) and %APPDATA% dirs, with user overrides winning. TUI should only launch Core if core.exe is a sibling in the same folder.

## Research

(empty)

## Architecture

(empty)
