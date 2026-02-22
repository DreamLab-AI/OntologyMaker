# AFD: Architecture-First Design — OpenPlanter Rust Port

**Version:** 1.0
**Date:** 2026-02-22

---

## 1. Crate Topology (Cargo Workspace)

```
openplanter/                         # Workspace root
├── Cargo.toml                       # [workspace] definition
├── crates/
│   ├── op-core/                     # Core domain types, errors, config
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs            # AgentConfig
│   │       ├── credentials.rs       # CredentialBundle, CredentialStore
│   │       ├── settings.rs          # PersistentSettings, SettingsStore
│   │       ├── error.rs             # ModelError, ToolError, SessionError, PatchError
│   │       └── types.rs             # ToolCall, ToolResult, ModelTurn, Conversation, ImageData
│   │
│   ├── op-model/                    # LLM abstraction and provider implementations
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── traits.rs            # BaseModel trait
│   │       ├── sse.rs               # SSE streaming (http_stream_sse, read_sse_events)
│   │       ├── http.rs              # HTTP JSON helper (http_json)
│   │       ├── openai.rs            # OpenAICompatibleModel
│   │       ├── anthropic.rs         # AnthropicModel
│   │       ├── echo.rs              # EchoFallbackModel
│   │       ├── scripted.rs          # ScriptedModel (testing)
│   │       ├── listing.rs           # list_openai_models, list_anthropic_models, etc.
│   │       └── accumulator.rs       # Stream accumulation (OpenAI + Anthropic)
│   │
│   ├── op-tools/                    # Workspace tool implementations
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── workspace.rs         # WorkspaceTools struct
│   │       ├── file_ops.rs          # list_files, read_file, write_file, edit_file
│   │       ├── search.rs            # search_files, repo_map, symbol extraction
│   │       ├── shell.rs             # run_shell, run_shell_bg, check/kill_shell_bg
│   │       ├── web.rs               # web_search, fetch_url (Exa API)
│   │       ├── patch.rs             # apply_patch, hashline_edit
│   │       ├── policy.rs            # Shell policy checks, write conflict detection
│   │       └── defs.rs              # TOOL_DEFINITIONS, to_openai_tools, to_anthropic_tools
│   │
│   ├── op-engine/                   # Recursive LLM engine
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── engine.rs            # RLMEngine
│   │       ├── context.rs           # ExternalContext
│   │       ├── dispatch.rs          # Tool call dispatch (_apply_tool_call)
│   │       ├── condensation.rs      # Context window condensation
│   │       ├── judge.rs             # Acceptance criteria judging
│   │       └── prompts.rs           # System prompt construction
│   │
│   ├── op-runtime/                  # Session persistence and lifecycle
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── session_store.rs     # SessionStore
│   │       ├── session_runtime.rs   # SessionRuntime
│   │       ├── replay_log.rs        # ReplayLogger
│   │       ├── patching.rs          # Codex-style patch parse + apply
│   │       └── wiki.rs              # Wiki seeding
│   │
│   ├── op-tui/                      # Terminal user interface
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── app.rs               # Main TUI application state
│   │       ├── repl.rs              # REPL input handling, line editing
│   │       ├── activity.rs          # Activity display (spinner, streaming)
│   │       ├── splash.rs            # ASCII art splash screen
│   │       ├── render.rs            # Step rendering, markdown, syntax highlight
│   │       ├── commands.rs          # Slash command dispatch
│   │       ├── demo.rs              # Demo censoring (DemoCensor, DemoRenderHook)
│   │       └── theme.rs             # Color theme and styling
│   │
│   ├── op-scripts/                  # Data fetcher scripts (binary crate)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs              # Subcommand dispatch
│   │       ├── fetch_fec.rs
│   │       ├── fetch_census_acs.rs
│   │       ├── fetch_epa_echo.rs
│   │       ├── fetch_fdic.rs
│   │       ├── fetch_icij_leaks.rs
│   │       ├── fetch_osha.rs
│   │       ├── fetch_ofac_sdn.rs
│   │       ├── fetch_propublica_990.rs
│   │       ├── fetch_sam_gov.rs
│   │       ├── fetch_sec_edgar.rs
│   │       ├── fetch_senate_lobbying.rs
│   │       ├── fetch_usaspending.rs
│   │       ├── entity_resolution.rs
│   │       ├── cross_link_analysis.rs
│   │       ├── build_findings_json.rs
│   │       └── timing_analysis.rs
│   │
│   └── op-cli/                      # Main binary crate (entry point)
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs              # CLI entry point (clap)
│           └── builder.rs           # Engine/model factory construction
```

## 2. Dependency Graph

```
op-cli ──→ op-tui ──→ op-engine ──→ op-model ──→ op-core
  │           │           │             │
  │           │           ├──→ op-tools ─┤
  │           │           │              │
  │           ├──→ op-runtime ──→ op-core
  │           │
  └──→ op-core

op-scripts ──→ op-core (standalone binary)
```

**Key invariant:** No circular dependencies. Each crate depends only on crates below it in the hierarchy.

## 3. Module-to-Crate Mapping

| Python Module | Rust Crate | Notes |
|---------------|-----------|-------|
| `agent/config.py` | `op-core` | `config.rs` |
| `agent/credentials.py` | `op-core` | `credentials.rs` |
| `agent/settings.py` | `op-core` | `settings.rs` |
| `agent/model.py` (types) | `op-core` | `types.rs` |
| `agent/model.py` (impls) | `op-model` | Split across 6 files |
| `agent/tool_defs.py` | `op-tools` | `defs.rs` |
| `agent/tools.py` | `op-tools` | Split across 7 files |
| `agent/engine.py` | `op-engine` | Split across 5 files |
| `agent/prompts.py` | `op-engine` | `prompts.rs` |
| `agent/runtime.py` | `op-runtime` | Split across 3 files |
| `agent/replay_log.py` | `op-runtime` | `replay_log.rs` |
| `agent/patching.py` | `op-runtime` | `patching.rs` |
| `agent/tui.py` | `op-tui` | Split across 7 files |
| `agent/demo.py` | `op-tui` | `demo.rs` |
| `agent/__main__.py` | `op-cli` | `main.rs` + `builder.rs` |
| `agent/builder.py` | `op-cli` | `builder.rs` |
| `scripts/*.py` | `op-scripts` | One file per fetcher |

## 4. Cross-Cutting Concerns

### 4.1 Error Strategy
```rust
// op-core/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("Model error: {0}")]
    Model(String),
    #[error("Tool error: {0}")]
    Tool(String),
    #[error("Session error: {0}")]
    Session(String),
    #[error("Patch error: {0}")]
    Patch(String),
    #[error("Config error: {0}")]
    Config(String),
    #[error("IO error: {source}")]
    Io { #[from] source: std::io::Error },
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("JSON error: {source}")]
    Json { #[from] source: serde_json::Error },
}

pub type OpResult<T> = Result<T, OpError>;
```

### 4.2 Async Strategy
- **Tokio multi-thread runtime** for the main binary
- `op-model`: All LLM calls are `async fn` (SSE streaming naturally async)
- `op-tools`: Shell execution via `tokio::process::Command`
- `op-engine`: `tokio::spawn` for parallel subtask/execute tools
- `op-tui`: `crossterm` event loop on dedicated thread, engine on Tokio
- **Cancellation**: `tokio_util::sync::CancellationToken` replaces `threading.Event`

### 4.3 Concurrency Primitives
| Python | Rust |
|--------|------|
| `threading.Lock` | `tokio::sync::Mutex` or `parking_lot::Mutex` |
| `threading.Event` | `CancellationToken` |
| `threading.local` | `tokio::task_local!` |
| `ThreadPoolExecutor` | `tokio::spawn` + `JoinSet` |
| `subprocess.Popen` | `tokio::process::Command` |

### 4.4 Configuration Loading
```rust
// Same environment variable precedence
// OPENPLANTER_* prefix → fallback bare name → default
impl AgentConfig {
    pub fn from_env(workspace: &Path) -> OpResult<Self> { ... }
}
```

### 4.5 Serialization
- All persisted types derive `serde::Serialize` + `serde::Deserialize`
- JSON output uses `serde_json::to_string_pretty` for human-readable files
- JSONL uses one `serde_json::to_string` per line

### 4.6 Feature Flags (Cargo)
```toml
[features]
default = ["tui", "scripts"]
tui = ["dep:ratatui", "dep:crossterm"]
scripts = ["dep:csv"]
```

## 5. Binary Outputs

| Binary | Crate | Purpose |
|--------|-------|---------|
| `openplanter-agent` | `op-cli` | Main agent binary |
| `openplanter-scripts` | `op-scripts` | Data fetcher CLI |

## 6. Build Configuration

```toml
# Workspace Cargo.toml
[workspace]
resolver = "2"
members = [
    "crates/op-core",
    "crates/op-model",
    "crates/op-tools",
    "crates/op-engine",
    "crates/op-runtime",
    "crates/op-tui",
    "crates/op-scripts",
    "crates/op-cli",
]

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["stream", "json"] }
clap = { version = "4", features = ["derive"] }
ratatui = "0.29"
crossterm = "0.28"
thiserror = "2"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
chrono = { version = "0.4", features = ["serde"] }
regex = "1"
base64 = "0.22"
crc32fast = "1"
rand = "0.8"
uuid = { version = "1", features = ["v4"] }
csv = "1"

[profile.release]
lto = true
codegen-units = 1
strip = true
```

## 7. Testing Strategy

| Level | Location | Tool |
|-------|----------|------|
| Unit tests | `src/*.rs` inline `#[cfg(test)]` | `cargo test` |
| Integration tests | `crates/*/tests/` | `cargo test` |
| Cross-compat | `tests/cross_compat/` | Compare Rust ↔ Python session files |
| Property tests | Selected modules | `proptest` crate |

## 8. Docker

```dockerfile
# Multi-stage build
FROM rust:1.82-alpine AS builder
RUN apk add musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM alpine:3.20
RUN apk add --no-cache ripgrep
COPY --from=builder /src/target/x86_64-unknown-linux-musl/release/openplanter-agent /usr/local/bin/
ENTRYPOINT ["openplanter-agent"]
```
