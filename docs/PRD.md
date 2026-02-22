# PRD: OpenPlanter Python-to-Native-Rust Conversion

**Version:** 1.0
**Date:** 2026-02-22
**Status:** Active

---

## 1. Overview

Complete rewrite of the OpenPlanter Agent ("OntologyMaker") from Python 3.12 to native Rust. The system is a recursive language-model investigation agent with a terminal UI that ingests public datasets, resolves entities across them, and surfaces non-obvious connections. The Rust port must preserve 100% behavioral parity with the Python original while gaining Rust's advantages: zero-cost abstractions, memory safety without GC, fearless concurrency, and single-binary deployment.

## 2. Motivation

| Concern | Python Status | Rust Target |
|---------|--------------|-------------|
| Startup time | ~1.2s (interpreter + imports) | <50ms (native binary) |
| Memory footprint | ~60MB baseline (runtime + rich) | <10MB baseline |
| Distribution | Requires Python 3.10+, pip, virtualenv | Single static binary |
| Concurrency | GIL-limited threading, no true parallelism | Tokio async + Rayon data parallelism |
| Type safety | Runtime duck typing, Protocol hints | Compile-time trait enforcement |
| Dependency supply chain | PyPI (rich, prompt_toolkit, pyfiglet) | Cargo with vendored deps |

## 3. Scope

### 3.1 In Scope

- **All 16 agent modules** (`__main__`, `engine`, `runtime`, `model`, `builder`, `tools`, `tool_defs`, `prompts`, `config`, `credentials`, `tui`, `demo`, `patching`, `replay_log`, `settings`, `__init__`)
- **All 13 data scripts** (fetch_fec, fetch_census_acs, fetch_epa_echo, fetch_fdic, fetch_icij_leaks, fetch_osha, fetch_ofac_sdn, fetch_propublica_990, fetch_sam_gov, fetch_sec_edgar, fetch_senate_lobbying, fetch_usaspending, entity_resolution, cross_link_analysis, build_findings_json, timing_analysis)
- **All 43 test files** → Rust `#[cfg(test)]` modules + integration tests
- **CLI interface** (argument parsing, REPL, headless mode)
- **Docker/container support** (multi-stage build with musl for static linking)
- **Session persistence** (JSON, JSONL)
- **Wiki seeding** (embedded or runtime file copying)

### 3.2 Out of Scope

- Changing the LLM API protocols (OpenAI, Anthropic, Exa stay as-is)
- Adding new data sources not in the Python original
- Changing the `.openplanter/` directory structure or file formats
- GUI or web frontend

## 4. Behavioral Parity Requirements

### 4.1 CLI Interface
- Identical argument names and semantics as `build_parser()` in `__main__.py`
- Same environment variable names (`OPENPLANTER_*` prefix, fallback to bare names)
- Same exit codes (0 success, 2 validation error)
- Same `openplanter-agent` binary name

### 4.2 LLM Provider Support
- **OpenAI-compatible** (OpenAI, OpenRouter, Cerebras, Ollama) via SSE streaming
- **Anthropic** native API with SSE streaming, thinking blocks, adaptive thinking
- **Provider auto-detection** from model name (same regex patterns)
- **Conversation condensation** at 75% context window threshold
- **Token tracking** per model per session
- **Reasoning effort** parameter support (low/medium/high/xhigh)

### 4.3 Tool System
- Same 20+ tool definitions with identical JSON schemas
- Same provider-neutral → OpenAI/Anthropic conversion logic
- Same workspace path validation (no escape outside root)
- Same shell policy enforcement (no heredoc, no interactive programs)
- Same parallel write group tracking with mutex-based conflict detection
- Same background job management
- Same ripgrep integration (or native Rust grep)
- Same Exa API integration

### 4.4 Engine
- Same recursive delegation with depth limiting
- Same parallel tool execution for `subtask`/`execute` via thread pool
- Same plan injection from `*.plan.md` files
- Same budget warnings at 25%/50% step thresholds
- Same acceptance criteria judging with lightweight model
- Same model tier downgrade logic

### 4.5 Session Persistence
- **Identical file formats**: `metadata.json`, `state.json`, `events.jsonl`, artifacts
- **Identical directory structure**: `.openplanter/sessions/{id}/`
- **Cross-compatible**: A Rust session must be resumable by Python and vice versa
- Same session ID format: `YYYYMMDD-HHMMSS-{3 hex bytes}`

### 4.6 TUI
- Same visual layout (splash screen, activity spinner, step rendering)
- Same slash commands (`/quit`, `/exit`, `/help`, `/status`, `/clear`, `/model`, `/reasoning`)
- Same model aliases (opus, sonnet, haiku, gpt5, o3, cerebras, etc.)
- Same streaming delta display (thinking, text)
- Same Ctrl+C cancellation
- Same plain REPL fallback for non-TTY

### 4.7 Data Scripts
- Same API endpoints and pagination logic
- Same output CSV/JSON formats
- Same entity resolution fuzzy matching behavior
- Same cross-link analysis pay-to-play detection

## 5. Rust Technology Stack

| Layer | Crate | Replaces |
|-------|-------|----------|
| Async runtime | `tokio` (multi-thread) | Python threading |
| HTTP client | `reqwest` (with SSE via `eventsource-stream` or manual) | `urllib.request` |
| JSON | `serde` + `serde_json` | `json` stdlib |
| CLI parsing | `clap` (derive) | `argparse` |
| TUI | `ratatui` + `crossterm` | `rich` + `prompt_toolkit` |
| Fuzzy matching | `nucleo` or `fuzzy-matcher` | `rapidfuzz` |
| Regex | `regex` | `re` |
| File watching | `notify` | N/A (new capability) |
| CSV | `csv` | Python `csv` |
| CRC32 | `crc32fast` | Python `binascii.crc32` |
| Base64 | `base64` | Python `base64` |
| UUID/random | `uuid`, `rand` | Python `secrets` |
| Date/time | `chrono` | Python `datetime` |
| Path handling | `std::path` | `pathlib` |
| Subprocess | `tokio::process` | `subprocess` |
| Logging | `tracing` + `tracing-subscriber` | Print statements |
| Error handling | `thiserror` + `anyhow` | Custom exceptions |
| Testing | Built-in `#[test]` + `tokio::test` | `pytest` |
| ASCII art | `figlet-rs` or embedded | `pyfiglet` |
| Concurrency | `tokio::sync`, `parking_lot` | `threading.Lock` |

## 6. Non-Functional Requirements

| Metric | Target |
|--------|--------|
| Compilation | `cargo build --release` < 120s |
| Binary size | < 15MB (static musl) |
| Cold start | < 50ms to first prompt |
| Memory (idle) | < 10MB RSS |
| Memory (active session) | < 50MB RSS typical |
| CI/CD | GitHub Actions with `cross` for linux/mac/windows |
| MSRV | Rust 1.82+ (edition 2024) |

## 7. Migration Strategy

### Phase 1: Foundation (AFD + DDD + Scaffolding)
- Create architecture and domain design documents
- Scaffold Cargo workspace with crate boundaries
- Establish error types, config, and core domain types

### Phase 2: Model Layer
- Implement `BaseModel` trait and provider implementations
- SSE streaming with `reqwest` + manual line parsing
- Token counting and conversation management

### Phase 3: Tool Layer
- Workspace tools (file I/O, shell, search)
- Tool definition schemas and provider conversion
- Exa API client

### Phase 4: Engine Layer
- Recursive LLM engine with async task spawning
- Context condensation
- Acceptance criteria judging

### Phase 5: Runtime Layer
- Session persistence (JSON/JSONL)
- Replay logging
- Patch parsing and application

### Phase 6: TUI Layer
- ratatui-based terminal UI
- REPL with line editing
- Streaming display

### Phase 7: CLI + Builder
- clap-based argument parsing
- Credential management
- Provider/model factory

### Phase 8: Data Scripts
- HTTP data fetchers as subcommands or separate binary
- Entity resolution
- Cross-link analysis

### Phase 9: Testing + Validation
- Port all 43 test files
- Integration tests
- Cross-compatibility validation with Python sessions

## 8. Success Criteria

1. `cargo test` passes all ported tests
2. `cargo build --release` produces a working single binary
3. Binary can resume a session created by Python version
4. All 20+ tools produce identical output for identical inputs
5. All 5 LLM providers work (OpenAI, Anthropic, OpenRouter, Cerebras, Ollama)
6. TUI is visually comparable to Python Rich version
7. All 13 data fetchers produce identical CSV/JSON output
8. Entity resolution produces identical match results

## 9. Risks

| Risk | Mitigation |
|------|-----------|
| SSE streaming complexity | Manual line-by-line parser matching Python's `_read_sse_events` |
| Rich TUI feature gap | ratatui has equivalent primitives; accept minor visual differences |
| Fuzzy patching accuracy | Port exact `_find_subsequence` logic with whitespace normalization |
| Python AST symbol extraction | Use `tree-sitter` for multi-language symbol extraction |
| Compilation time | Workspace crate splitting reduces incremental builds |
