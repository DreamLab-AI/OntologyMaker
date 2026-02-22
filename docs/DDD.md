# DDD: Domain-Driven Design — OpenPlanter Rust Port

**Version:** 1.0
**Date:** 2026-02-22

---

## 1. Strategic Design: Bounded Contexts

```
┌─────────────────────────────────────────────────────────────────┐
│                    OpenPlanter System                             │
│                                                                   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │
│  │ Configuration │  │  Credential  │  │    Settings           │   │
│  │   Context     │  │   Context    │  │    Context            │   │
│  │              │  │              │  │                      │   │
│  │ AgentConfig  │  │ CredBundle   │  │ PersistentSettings   │   │
│  │ Provider     │  │ CredStore    │  │ SettingsStore        │   │
│  │ defaults     │  │ UserStore    │  │                      │   │
│  └──────┬───────┘  └──────┬───────┘  └──────────┬───────────┘   │
│         │                  │                      │               │
│         └──────────┬───────┴──────────────────────┘               │
│                    │                                              │
│  ┌─────────────────▼──────────────────────────────────────────┐  │
│  │              Model (LLM) Context                            │  │
│  │                                                              │  │
│  │  BaseModel trait                                             │  │
│  │  ├── OpenAICompatibleModel                                   │  │
│  │  ├── AnthropicModel                                          │  │
│  │  ├── ScriptedModel                                           │  │
│  │  └── EchoFallbackModel                                       │  │
│  │                                                              │  │
│  │  Value Objects: ToolCall, ToolResult, ModelTurn, Conversation │  │
│  │  Services: SSE streaming, HTTP transport, stream accumulation │  │
│  └─────────────────┬──────────────────────────────────────────┘  │
│                    │                                              │
│  ┌─────────────────▼──────────────────────────────────────────┐  │
│  │              Tool Execution Context                          │  │
│  │                                                              │  │
│  │  Aggregate Root: WorkspaceTools                              │  │
│  │  ├── FileOps (list, read, write, edit)                       │  │
│  │  ├── ShellOps (run, bg, check, kill)                         │  │
│  │  ├── SearchOps (grep, repo_map, symbols)                     │  │
│  │  ├── WebOps (search, fetch)                                  │  │
│  │  ├── PatchOps (apply_patch, hashline_edit)                   │  │
│  │  └── PolicyEnforcement (heredoc, interactive, write locks)   │  │
│  │                                                              │  │
│  │  Value Objects: ToolDefinition, BackgroundJob                │  │
│  │  Domain Events: ToolExecuted, PolicyViolation, WriteConflict │  │
│  └─────────────────┬──────────────────────────────────────────┘  │
│                    │                                              │
│  ┌─────────────────▼──────────────────────────────────────────┐  │
│  │              Engine (Orchestration) Context                  │  │
│  │                                                              │  │
│  │  Aggregate Root: RLMEngine                                   │  │
│  │  ├── Recursive solve loop                                    │  │
│  │  ├── Tool dispatch                                           │  │
│  │  ├── Context condensation                                    │  │
│  │  ├── Acceptance criteria judging                             │  │
│  │  ├── Plan injection                                          │  │
│  │  └── Budget tracking                                         │  │
│  │                                                              │  │
│  │  Entity: ExternalContext (observations list)                  │  │
│  │  Domain Events: StepCompleted, BudgetWarning, Condensed      │  │
│  └─────────────────┬──────────────────────────────────────────┘  │
│                    │                                              │
│  ┌─────────────────▼──────────────────────────────────────────┐  │
│  │              Session (Persistence) Context                  │  │
│  │                                                              │  │
│  │  Aggregate Root: SessionRuntime                              │  │
│  │  ├── SessionStore (CRUD for sessions)                        │  │
│  │  ├── ReplayLogger (JSONL delta encoding)                     │  │
│  │  └── WikiSeeder (baseline wiki copy)                         │  │
│  │                                                              │  │
│  │  Entities: Session (id, metadata, state, events, artifacts)  │  │
│  │  Value Objects: SessionId, Event, Artifact                   │  │
│  │  Domain Events: SessionCreated, SessionResumed, EventLogged  │  │
│  └─────────────────┬──────────────────────────────────────────┘  │
│                    │                                              │
│  ┌─────────────────▼──────────────────────────────────────────┐  │
│  │              Presentation (TUI) Context                      │  │
│  │                                                              │  │
│  │  Aggregate Root: TuiApp                                      │  │
│  │  ├── RichREPL (input handling, history)                      │  │
│  │  ├── ActivityDisplay (spinner, streaming, tool status)       │  │
│  │  ├── SplashScreen (ASCII art)                                │  │
│  │  ├── StepRenderer (rules, trees, token counts)               │  │
│  │  ├── SlashCommandDispatcher                                  │  │
│  │  └── DemoCensor (path/entity censoring)                      │  │
│  │                                                              │  │
│  │  Value Objects: ChatContext, StepState, ToolCallRecord        │  │
│  │  Domain Events: UserInput, AgentResponse, DisplayUpdate      │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │              Data Ingestion Context                          │  │
│  │                                                              │  │
│  │  Services:                                                   │  │
│  │  ├── FecFetcher, CensusAcsFetcher, EpaEchoFetcher, ...     │  │
│  │  ├── EntityResolver (fuzzy matching, name normalization)     │  │
│  │  ├── CrossLinkAnalyzer (pay-to-play detection)              │  │
│  │  └── FindingsBuilder (structured JSON output)                │  │
│  │                                                              │  │
│  │  Value Objects: DataRecord, EntityMatch, Finding, Provenance │  │
│  └────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## 2. Tactical Design: Aggregates, Entities, Value Objects

### 2.1 Configuration Context (`op-core`)

**Value Objects** (immutable, compared by value):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub workspace: PathBuf,
    pub provider: String,             // "auto", "openai", "anthropic", etc.
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub openai_base_url: String,
    pub anthropic_base_url: String,
    pub openrouter_base_url: String,
    pub cerebras_base_url: String,
    pub ollama_base_url: String,
    pub exa_base_url: String,
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub cerebras_api_key: Option<String>,
    pub exa_api_key: Option<String>,
    pub voyage_api_key: Option<String>,
    pub max_depth: u32,
    pub max_steps_per_call: u32,
    pub max_observation_chars: usize,
    pub command_timeout_sec: u64,
    pub shell: String,
    pub max_files_listed: usize,
    pub max_file_chars: usize,
    pub max_search_hits: usize,
    pub max_shell_output_chars: usize,
    pub session_root_dir: String,
    pub max_persisted_observations: usize,
    pub max_solve_seconds: u64,
    pub recursive: bool,
    pub min_subtask_depth: u32,
    pub acceptance_criteria: bool,
    pub max_plan_chars: usize,
    pub demo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialBundle {
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub cerebras_api_key: Option<String>,
    pub exa_api_key: Option<String>,
    pub voyage_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentSettings {
    pub default_model: Option<String>,
    pub default_reasoning_effort: Option<String>,
    pub default_model_openai: Option<String>,
    pub default_model_anthropic: Option<String>,
    pub default_model_openrouter: Option<String>,
    pub default_model_cerebras: Option<String>,
    pub default_model_ollama: Option<String>,
}
```

**Services:**
```rust
pub struct CredentialStore { workspace: PathBuf, session_root_dir: String }
pub struct UserCredentialStore { credentials_path: PathBuf }
pub struct SettingsStore { workspace: PathBuf, session_root_dir: String }
```

### 2.2 Model Context (`op-model`)

**Trait (Protocol equivalent):**
```rust
#[async_trait]
pub trait LlmModel: Send + Sync {
    fn create_conversation(&self, system_prompt: &str, initial_user_message: &str) -> Conversation;
    async fn complete(&self, conversation: &mut Conversation) -> OpResult<ModelTurn>;
    fn append_assistant_turn(&self, conversation: &mut Conversation, turn: &ModelTurn);
    fn append_tool_results(&self, conversation: &mut Conversation, results: &[ToolResult]);
    fn condense_conversation(&self, conversation: &mut Conversation, keep_recent: usize) -> usize;
}
```

**Value Objects:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ImageData {
    pub base64_data: String,
    pub media_type: String,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    pub image: Option<ImageData>,
}

#[derive(Debug, Clone)]
pub struct ModelTurn {
    pub tool_calls: Vec<ToolCall>,
    pub text: Option<String>,
    pub stop_reason: String,
    pub raw_response: serde_json::Value,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug)]
pub struct Conversation {
    pub(crate) provider_messages: Vec<serde_json::Value>,
    pub system_prompt: String,
    pub turn_count: u32,
    pub stop_sequences: Vec<String>,
}
```

**Implementations:**
```rust
pub struct OpenAiModel { /* fields matching Python dataclass */ }
pub struct AnthropicModel { /* fields matching Python dataclass */ }
pub struct ScriptedModel { /* for testing */ }
pub struct EchoFallbackModel { /* fallback */ }
```

### 2.3 Tool Execution Context (`op-tools`)

**Aggregate Root:**
```rust
pub struct WorkspaceTools {
    root: PathBuf,
    shell: String,
    command_timeout: Duration,
    max_shell_output_chars: usize,
    max_file_chars: usize,
    max_files_listed: usize,
    max_search_hits: usize,
    exa_api_key: Option<String>,
    exa_base_url: String,
    bg_jobs: Mutex<HashMap<u32, BackgroundJob>>,
    bg_next_id: AtomicU32,
    files_read: Mutex<HashSet<PathBuf>>,
    parallel_write_claims: Mutex<HashMap<String, HashSet<PathBuf>>>,
}
```

**Value Objects:**
```rust
pub struct BackgroundJob {
    pub child: tokio::process::Child,
    pub output_path: PathBuf,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
```

**Domain Events:**
```rust
pub enum ToolEvent {
    Executed { name: String, duration: Duration },
    PolicyViolation { name: String, reason: String },
    WriteConflict { path: PathBuf, group_id: String },
}
```

### 2.4 Engine Context (`op-engine`)

**Aggregate Root:**
```rust
pub struct RLMEngine {
    model: Box<dyn LlmModel>,
    tools: WorkspaceTools,
    config: AgentConfig,
    system_prompt: String,
    session_tokens: Mutex<HashMap<String, TokenUsage>>,
    model_factory: Option<ModelFactory>,
    model_cache: Mutex<HashMap<(String, Option<String>), Box<dyn LlmModel>>>,
    cancel: CancellationToken,
    shell_command_counts: Mutex<HashMap<(u32, String), u32>>,
    session_dir: Option<PathBuf>,
    session_id: Option<String>,
}
```

**Entity:**
```rust
pub struct ExternalContext {
    observations: Vec<String>,
}
```

**Callbacks:**
```rust
pub type EventCallback = Box<dyn Fn(&str) + Send + Sync>;
pub type StepCallback = Box<dyn Fn(&serde_json::Value) + Send + Sync>;
pub type ContentDeltaCallback = Box<dyn Fn(&str, &str) + Send + Sync>;
pub type ModelFactory = Box<dyn Fn(&str, Option<&str>) -> Box<dyn LlmModel> + Send + Sync>;
```

### 2.5 Session Context (`op-runtime`)

**Aggregate Root:**
```rust
pub struct SessionRuntime {
    engine: RLMEngine,
    store: SessionStore,
    session_id: String,
    context: ExternalContext,
    max_persisted_observations: usize,
}
```

**Entity:**
```rust
pub struct SessionStore {
    workspace: PathBuf,
    session_root_dir: String,
    root: PathBuf,
    sessions: PathBuf,
}
```

**Value Objects:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub event_type: String,
    pub timestamp: String,
    pub payload: serde_json::Value,
}

pub struct ReplayLogger {
    path: PathBuf,
    conversation_id: String,
    seq: u32,
    last_msg_count: usize,
}
```

### 2.6 Presentation Context (`op-tui`)

**Aggregate Root:**
```rust
pub struct TuiApp {
    runtime: SessionRuntime,
    config: AgentConfig,
    settings_store: SettingsStore,
    activity: ActivityDisplay,
    input_history: Vec<String>,
}
```

**Value Objects:**
```rust
pub struct ChatContext {
    pub runtime: SessionRuntime,
    pub cfg: AgentConfig,
    pub settings_store: SettingsStore,
}

pub struct StepState {
    tool_calls: Vec<ToolCallRecord>,
    text_parts: Vec<String>,
    token_count: TokenUsage,
}

pub struct ToolCallRecord {
    name: String,
    key_arg: String,
    duration: Duration,
    observation_preview: String,
}
```

### 2.7 Data Ingestion Context (`op-scripts`)

**Services (one per data source):**
```rust
pub trait DataFetcher {
    fn name(&self) -> &str;
    async fn fetch(&self, config: &FetchConfig) -> OpResult<Vec<DataRecord>>;
}

pub struct FecFetcher;
pub struct CensusAcsFetcher;
pub struct EpaEchoFetcher;
pub struct FdicFetcher;
pub struct IcijLeaksFetcher;
pub struct OshaFetcher;
pub struct OfacSdnFetcher;
pub struct PropublicaFetcher;
pub struct SamGovFetcher;
pub struct SecEdgarFetcher;
pub struct SenateLobbyingFetcher;
pub struct UsaSpendingFetcher;
```

**Value Objects:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataRecord {
    pub source: String,
    pub fields: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct EntityMatch {
    pub entity_a: String,
    pub entity_b: String,
    pub confidence: f64,
    pub source_a: String,
    pub source_b: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub title: String,
    pub entities: Vec<String>,
    pub connections: Vec<EntityMatch>,
    pub confidence: f64,
    pub provenance: Vec<String>,
}
```

## 3. Context Mapping (Integration Patterns)

| Upstream | Downstream | Pattern |
|----------|-----------|---------|
| Configuration → Model | Model reads config | **Shared Kernel** (op-core types) |
| Configuration → Tools | Tools reads config | **Shared Kernel** |
| Model → Engine | Engine owns model via trait | **Conformist** (Engine adapts to Model trait) |
| Tools → Engine | Engine owns tools | **Conformist** |
| Engine → Runtime | Runtime wraps engine | **Anti-Corruption Layer** |
| Runtime → TUI | TUI drives runtime | **Customer-Supplier** |
| Configuration → CLI | CLI constructs config | **Open Host Service** |

## 4. Domain Events Flow

```
User Input
    │
    ▼
TUI::UserInput ──→ SessionRuntime::solve()
                        │
                        ▼
                   RLMEngine::_solve_recursive()
                        │
                        ├── ModelTurn received
                        │   └── Engine::StepCompleted
                        │
                        ├── ToolCall dispatched
                        │   ├── Tool::Executed
                        │   ├── Tool::PolicyViolation
                        │   └── Tool::WriteConflict
                        │
                        ├── Context condensed
                        │   └── Engine::Condensed
                        │
                        ├── Budget warning
                        │   └── Engine::BudgetWarning
                        │
                        └── Final answer
                            └── Session::EventLogged
                                    │
                                    ▼
                               TUI::AgentResponse
```

## 5. Invariants

1. **Path Safety**: No tool operation may access paths outside `workspace.root`
2. **Write Exclusivity**: No two parallel write groups may claim the same file path
3. **Session Consistency**: `metadata.json` timestamps monotonically increase
4. **Token Accounting**: Every model call's input/output tokens are tracked
5. **Cancellation Propagation**: `cancel` signal stops all in-flight tool executions
6. **Context Window**: Condensation triggers before exceeding 75% of model's window
7. **Recursion Depth**: `depth` never exceeds `config.max_depth`
8. **Shell Policy**: Heredoc syntax and interactive programs are always rejected
9. **Credential Security**: Credential files written with mode 0o600
10. **Event Ordering**: JSONL events are strictly append-only, timestamp-ordered
