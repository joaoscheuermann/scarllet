---
status: done
order: 5
created: 2026-04-16 09:59
title: "DIP: Make hidden dependencies explicit in gemini, config, TUI"
---

## Description

Fix three DIP violations where functions hide their dependencies by reading from the environment or constructing internal clients. Make these explicit via parameters or stored fields.

## Objective

After this effort, `GeminiProvider` reuses a stored HTTP client and accepts an optional base URL, `config_path` has a testable override variant, and `App::new` receives its environment dependencies as parameters. All crates compile and tests pass.

## Implementation Details

### 5a. GeminiProvider — store HTTP client + injectable base URL

In `packages/rust/scarllet-llm/src/gemini.rs`:

1. Add fields to `GeminiProvider`:
   ```rust
   pub struct GeminiProvider {
       api_key: String,
       http: reqwest::Client,
       api_base_url: String,
   }
   ```
2. Update `GeminiProvider::new`:
   ```rust
   pub fn new(api_key: String) -> Self {
       let http = reqwest::Client::builder()
           .timeout(std::time::Duration::from_secs(120))
           .connect_timeout(std::time::Duration::from_secs(10))
           .build()
           .unwrap_or_default();
       Self {
           api_key,
           http,
           api_base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
       }
   }
   ```
3. Add `pub fn with_base_url(mut self, url: String) -> Self { self.api_base_url = url; self }`.
4. In `get_context_window`, replace `reqwest::Client::new()` with `&self.http` and use `self.api_base_url` instead of the hardcoded URL.

### 5b. config_path — testable override

In `packages/rust/scarllet-sdk/src/config.rs`:

1. Add:
   ```rust
   pub fn config_path_in(base: &Path) -> PathBuf {
       base.join("scarllet").join("config.json")
   }
   ```
2. Refactor `config_path()` to call `config_path_in(&dirs::config_dir().expect(...))`.
3. Existing call sites continue using `config_path()` (no changes needed).

### 5c. App::new — explicit parameters

In `packages/rust/scarllet-tui/src/app.rs` (after Effort 3 split):

1. Change `App::new` signature to:
   ```rust
   pub(crate) fn new(message_tx: mpsc::Sender<TuiMessage>, cwd: PathBuf, debug_enabled: bool) -> Self
   ```
2. Remove the `std::env::current_dir()` and `std::env::var("SCARLLET_DEBUG")` calls from inside `new`.
3. In `main.rs`, read environment once and pass values:
   ```rust
   let cwd = std::env::current_dir().unwrap_or_default();
   let debug = std::env::var("SCARLLET_DEBUG").map(|v| v == "true").unwrap_or(false);
   let mut app = App::new(message_tx, cwd, debug);
   ```

## Verification Criteria

1. `npx nx run scarllet-llm:build` succeeds.
2. `npx nx run scarllet-llm:test` passes.
3. `npx nx run scarllet-sdk:build` succeeds.
4. `npx nx run scarllet-sdk:test` passes.
5. `npx nx run scarllet-tui:build` succeeds.
6. Start core + TUI with a Gemini provider configured — verify context window is fetched and displayed in status bar. Verify with an OpenAI-compatible provider too.
7. Start TUI with `SCARLLET_DEBUG=true` — verify debug messages appear, confirming the explicit parameter works.

## Done

- `GeminiProvider` stores `http: reqwest::Client` and `api_base_url: String`.
- `get_context_window` uses the stored client and base URL.
- `config_path_in(base)` exists for testable config path resolution.
- `App::new` takes `cwd` and `debug_enabled` as parameters.
- All three crates compile and pass tests.
