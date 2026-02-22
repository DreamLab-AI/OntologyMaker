//! CLI entry point for the OpenPlanter coding agent.
//!
//! Port of Python `agent/__main__.py`. Parses command-line arguments, loads
//! credentials from multiple sources, applies persistent settings, and
//! launches either the TUI REPL, a plain REPL, or headless task execution.

mod builder;

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;

use op_core::config::provider_default_models;
use op_core::credentials::{
    credentials_from_env, discover_env_candidates, parse_env_file, CredentialBundle,
    CredentialStore, UserCredentialStore,
};
use op_core::settings::{normalize_reasoning_effort, PersistentSettings, SettingsStore};
use op_core::{AgentConfig, OpError};
use op_runtime::SessionStore;

use builder::{build_engine, fetch_models_for_provider, infer_provider_for_model, ModelInfo};

// ---------------------------------------------------------------------------
// Valid choices
// ---------------------------------------------------------------------------

const VALID_REASONING_FLAGS: &[&str] = &["low", "medium", "high", "none"];
const VALID_PROVIDERS: &[&str] = &[
    "auto",
    "openai",
    "anthropic",
    "openrouter",
    "cerebras",
    "ollama",
    "all",
];

// ---------------------------------------------------------------------------
// CLI arguments (matching Python's build_parser)
// ---------------------------------------------------------------------------

/// OpenPlanter coding agent with terminal UI.
#[derive(Parser, Debug)]
#[command(name = "openplanter-agent", about = "OpenPlanter coding agent with terminal UI.")]
struct Cli {
    /// Workspace root directory.
    #[arg(long, default_value = ".")]
    workspace: String,

    /// Model provider. Use 'all' only with --list-models.
    #[arg(long, value_parser = provider_value_parser)]
    provider: Option<String>,

    /// Model name (use 'newest' to auto-select latest from API).
    #[arg(long, short = 'm')]
    model: Option<String>,

    /// Per-run reasoning effort override.
    #[arg(long, value_parser = reasoning_value_parser)]
    reasoning_effort: Option<String>,

    /// Persist workspace default model in .openplanter/settings.json.
    #[arg(long)]
    default_model: Option<String>,

    /// Persist workspace default reasoning effort.
    #[arg(long, value_parser = reasoning_value_parser)]
    default_reasoning_effort: Option<String>,

    /// Persist workspace default model for OpenAI provider.
    #[arg(long)]
    default_model_openai: Option<String>,

    /// Persist workspace default model for Anthropic provider.
    #[arg(long)]
    default_model_anthropic: Option<String>,

    /// Persist workspace default model for OpenRouter provider.
    #[arg(long)]
    default_model_openrouter: Option<String>,

    /// Persist workspace default model for Cerebras provider.
    #[arg(long)]
    default_model_cerebras: Option<String>,

    /// Persist workspace default model for Ollama provider.
    #[arg(long)]
    default_model_ollama: Option<String>,

    /// Show persistent workspace defaults and exit (unless task/list action is also provided).
    #[arg(long)]
    show_settings: bool,

    /// Provider base URL override for this run.
    #[arg(long)]
    base_url: Option<String>,

    /// Legacy API key alias (maps to OpenAI).
    #[arg(long)]
    api_key: Option<String>,

    /// OpenAI API key override.
    #[arg(long)]
    openai_api_key: Option<String>,

    /// Anthropic API key override.
    #[arg(long)]
    anthropic_api_key: Option<String>,

    /// OpenRouter API key override.
    #[arg(long)]
    openrouter_api_key: Option<String>,

    /// Cerebras API key override.
    #[arg(long)]
    cerebras_api_key: Option<String>,

    /// Exa API key override.
    #[arg(long)]
    exa_api_key: Option<String>,

    /// Voyage API key override.
    #[arg(long)]
    voyage_api_key: Option<String>,

    /// Prompt to set/update provider API keys and persist them locally.
    #[arg(long)]
    configure_keys: bool,

    /// Maximum recursion depth.
    #[arg(long)]
    max_depth: Option<u32>,

    /// Maximum steps per recursive call.
    #[arg(long)]
    max_steps: Option<u32>,

    /// Shell command timeout seconds.
    #[arg(long)]
    timeout: Option<u64>,

    /// Disable interactive UI/prompts; intended for CI/non-TTY execution.
    #[arg(long)]
    headless: bool,

    /// Use plain REPL instead of Rich REPL (no colors, no spinner).
    #[arg(long)]
    no_tui: bool,

    /// Single objective to run and exit.
    #[arg(long, short = 't')]
    task: Option<String>,

    /// Session id to use. If omitted, a new id is generated unless --resume is used.
    #[arg(long)]
    session_id: Option<String>,

    /// Resume an existing session (with --session-id or latest session).
    #[arg(long)]
    resume: bool,

    /// List known sessions in .openplanter and exit.
    #[arg(long)]
    list_sessions: bool,

    /// Fetch and list available provider models from newest to oldest via API.
    #[arg(long)]
    list_models: bool,

    /// Enable recursive mode with subtask delegation (default: flat agent).
    #[arg(long)]
    recursive: bool,

    /// Enable acceptance criteria: subtask/execute results are judged by a lightweight model.
    #[arg(long)]
    acceptance_criteria: bool,

    /// Censor entity names and workspace path segments in output (UI-only).
    #[arg(long)]
    demo: bool,
}

// ---------------------------------------------------------------------------
// Custom value parsers for clap
// ---------------------------------------------------------------------------

fn provider_value_parser(s: &str) -> Result<String, String> {
    let lower = s.to_lowercase();
    if VALID_PROVIDERS.contains(&lower.as_str()) {
        Ok(lower)
    } else {
        Err(format!(
            "Invalid provider '{}'. Valid choices: {}",
            s,
            VALID_PROVIDERS.join(", ")
        ))
    }
}

fn reasoning_value_parser(s: &str) -> Result<String, String> {
    let lower = s.to_lowercase();
    if VALID_REASONING_FLAGS.contains(&lower.as_str()) {
        Ok(lower)
    } else {
        Err(format!(
            "Invalid reasoning effort '{}'. Valid choices: {}",
            s,
            VALID_REASONING_FLAGS.join(", ")
        ))
    }
}

// ---------------------------------------------------------------------------
// Timestamp formatting
// ---------------------------------------------------------------------------

fn format_ts(ts: i64) -> String {
    if ts <= 0 {
        return "unknown".to_string();
    }
    match Utc.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(dt) => dt.to_rfc3339(),
        _ => "unknown".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Provider resolution
// ---------------------------------------------------------------------------

/// Resolve the provider string. If "auto", pick the first provider for which
/// an API key exists.
fn resolve_provider(requested: &str, creds: &CredentialBundle) -> String {
    let requested = requested.trim().to_lowercase();
    match requested.as_str() {
        "openai" | "anthropic" | "openrouter" | "cerebras" | "ollama" => requested,
        "all" => "all".to_string(),
        _ => {
            // "auto" or anything else — pick first available provider.
            if creds.openai_api_key.is_some() {
                "openai".to_string()
            } else if creds.anthropic_api_key.is_some() {
                "anthropic".to_string()
            } else if creds.openrouter_api_key.is_some() {
                "openrouter".to_string()
            } else if creds.cerebras_api_key.is_some() {
                "cerebras".to_string()
            } else {
                "openai".to_string()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Model listing
// ---------------------------------------------------------------------------

async fn print_models(cfg: &AgentConfig, requested_provider: &str) -> i32 {
    let providers: Vec<&str> = match requested_provider {
        "all" | "auto" => vec!["openai", "anthropic", "openrouter", "cerebras", "ollama"],
        other => vec![other],
    };

    let mut printed_any = false;
    for provider in providers {
        match fetch_models_for_provider(cfg, provider).await {
            Ok(models) => {
                println!("{}: {} models", provider, models.len());
                for m in &models {
                    println!("  {} | {}", m.id, format_ts(m.created_ts));
                }
                printed_any = true;
            }
            Err(e) => {
                println!("{}: skipped ({})", provider, e);
            }
        }
    }

    if !printed_any {
        println!("No models could be listed. Configure at least one provider API key.");
        return 1;
    }
    0
}

// ---------------------------------------------------------------------------
// Credential loading (multi-level: args -> env -> .env -> ~/.openplanter -> .openplanter)
// ---------------------------------------------------------------------------

fn load_credentials(cfg: &AgentConfig, args: &Cli, allow_prompt: bool) -> CredentialBundle {
    // 1. User-global store (~/.openplanter/credentials.json)
    let user_store = UserCredentialStore::new();
    let user_creds = user_store.load().unwrap_or_default();

    let mut creds = user_creds.clone();

    // 2. Workspace-local store (.openplanter/credentials.json)
    let store = CredentialStore::new(&cfg.workspace, &cfg.session_root_dir);
    let stored = store.load().unwrap_or_default();
    apply_override(&mut creds.openai_api_key, &stored.openai_api_key);
    apply_override(&mut creds.anthropic_api_key, &stored.anthropic_api_key);
    apply_override(&mut creds.openrouter_api_key, &stored.openrouter_api_key);
    apply_override(&mut creds.cerebras_api_key, &stored.cerebras_api_key);
    apply_override(&mut creds.exa_api_key, &stored.exa_api_key);
    apply_override(&mut creds.voyage_api_key, &stored.voyage_api_key);

    // 3. Environment variables
    let env_creds = credentials_from_env();
    apply_override(&mut creds.openai_api_key, &env_creds.openai_api_key);
    apply_override(&mut creds.anthropic_api_key, &env_creds.anthropic_api_key);
    apply_override(&mut creds.openrouter_api_key, &env_creds.openrouter_api_key);
    apply_override(&mut creds.cerebras_api_key, &env_creds.cerebras_api_key);
    apply_override(&mut creds.exa_api_key, &env_creds.exa_api_key);
    apply_override(&mut creds.voyage_api_key, &env_creds.voyage_api_key);

    // 4. .env files (workspace + parent directories) — fill missing only
    for env_path in discover_env_candidates(&cfg.workspace) {
        if let Ok(file_creds) = parse_env_file(&env_path) {
            creds.merge_missing(&file_creds);
        }
    }

    // 5. CLI argument overrides (highest priority)
    if let Some(ref key) = args.api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            creds.openai_api_key = Some(trimmed.to_string());
        }
    }
    if let Some(ref key) = args.openai_api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            creds.openai_api_key = Some(trimmed.to_string());
        }
    }
    if let Some(ref key) = args.anthropic_api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            creds.anthropic_api_key = Some(trimmed.to_string());
        }
    }
    if let Some(ref key) = args.openrouter_api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            creds.openrouter_api_key = Some(trimmed.to_string());
        }
    }
    if let Some(ref key) = args.cerebras_api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            creds.cerebras_api_key = Some(trimmed.to_string());
        }
    }
    if let Some(ref key) = args.exa_api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            creds.exa_api_key = Some(trimmed.to_string());
        }
    }
    if let Some(ref key) = args.voyage_api_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            creds.voyage_api_key = Some(trimmed.to_string());
        }
    }

    // 6. Interactive prompt (if allowed)
    if allow_prompt && args.configure_keys {
        prompt_for_credentials_interactive(&mut creds);
    } else if args.configure_keys {
        println!("Headless/non-interactive mode: skipping interactive key prompt.");
    }

    if !creds.has_any() {
        println!(
            "No API keys are configured. \
             Set keys with --configure-keys, env vars, or .env files."
        );
    }

    // Persist updated credentials
    if creds.openai_api_key != user_creds.openai_api_key
        || creds.anthropic_api_key != user_creds.anthropic_api_key
        || creds.openrouter_api_key != user_creds.openrouter_api_key
        || creds.cerebras_api_key != user_creds.cerebras_api_key
        || creds.exa_api_key != user_creds.exa_api_key
        || creds.voyage_api_key != user_creds.voyage_api_key
    {
        let _ = user_store.save(&creds);
    }
    if stored.has_any()
        && (creds.openai_api_key != stored.openai_api_key
            || creds.anthropic_api_key != stored.anthropic_api_key
            || creds.openrouter_api_key != stored.openrouter_api_key
            || creds.cerebras_api_key != stored.cerebras_api_key
            || creds.exa_api_key != stored.exa_api_key
            || creds.voyage_api_key != stored.voyage_api_key)
    {
        let _ = store.save(&creds);
    }

    creds
}

/// Override `target` with `source` if `source` is `Some`.
fn apply_override(target: &mut Option<String>, source: &Option<String>) {
    if source.is_some() {
        *target = source.clone();
    }
}

/// Interactive credential prompt. Reads from stdin for each key that is empty.
fn prompt_for_credentials_interactive(creds: &mut CredentialBundle) {
    let keys: &[(&str, fn(&mut CredentialBundle) -> &mut Option<String>)] = &[
        ("OpenAI API key", |c| &mut c.openai_api_key),
        ("Anthropic API key", |c| &mut c.anthropic_api_key),
        ("OpenRouter API key", |c| &mut c.openrouter_api_key),
        ("Cerebras API key", |c| &mut c.cerebras_api_key),
        ("Exa API key", |c| &mut c.exa_api_key),
        ("Voyage API key", |c| &mut c.voyage_api_key),
    ];

    for (label, accessor) in keys {
        let current = accessor(creds).as_deref().unwrap_or("");
        let display = if current.is_empty() {
            "(not set)".to_string()
        } else {
            let visible = std::cmp::min(4, current.len());
            format!("{}...", &current[..visible])
        };
        print!("{} [{}]: ", label, display);
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            let trimmed = input.trim();
            if !trimmed.is_empty() {
                *accessor(creds) = Some(trimmed.to_string());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime overrides
// ---------------------------------------------------------------------------

fn apply_runtime_overrides(cfg: &mut AgentConfig, args: &Cli, creds: &CredentialBundle) {
    if let Some(max_depth) = args.max_depth {
        cfg.max_depth = max_depth;
    }
    if let Some(max_steps) = args.max_steps {
        cfg.max_steps_per_call = max_steps;
    }
    if let Some(timeout) = args.timeout {
        cfg.command_timeout_sec = timeout;
    }

    if let Some(ref provider) = args.provider {
        cfg.provider = provider.clone();
    }
    cfg.provider = resolve_provider(&cfg.provider, creds);

    cfg.openai_api_key = creds.openai_api_key.clone();
    cfg.anthropic_api_key = creds.anthropic_api_key.clone();
    cfg.openrouter_api_key = creds.openrouter_api_key.clone();
    cfg.cerebras_api_key = creds.cerebras_api_key.clone();
    cfg.exa_api_key = creds.exa_api_key.clone();
    cfg.voyage_api_key = creds.voyage_api_key.clone();
    cfg.api_key = cfg.openai_api_key.clone();

    if let Some(ref base_url) = args.base_url {
        match cfg.provider.as_str() {
            "openai" => cfg.openai_base_url = base_url.clone(),
            "anthropic" => cfg.anthropic_base_url = base_url.clone(),
            "openrouter" => cfg.openrouter_base_url = base_url.clone(),
            "cerebras" => cfg.cerebras_base_url = base_url.clone(),
            "ollama" => cfg.ollama_base_url = base_url.clone(),
            _ => {}
        }
        cfg.base_url = base_url.clone();
    }

    if let Some(ref model) = args.model {
        cfg.model = model.clone();
    }
    if let Some(ref effort) = args.reasoning_effort {
        cfg.reasoning_effort = if effort == "none" {
            None
        } else {
            Some(effort.clone())
        };
    }
    if args.recursive {
        cfg.recursive = true;
    }
    if args.acceptance_criteria {
        cfg.acceptance_criteria = true;
    }
    if args.demo {
        cfg.demo = true;
    }
}

// ---------------------------------------------------------------------------
// Persistent settings
// ---------------------------------------------------------------------------

fn apply_persistent_settings(
    cfg: &mut AgentConfig,
    args: &Cli,
    store: &SettingsStore,
) -> PersistentSettings {
    let mut settings = store.load().unwrap_or_default();
    let mut changed = false;

    if let Some(ref val) = args.default_model {
        let trimmed = val.trim();
        settings.default_model = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        changed = true;
    }
    if let Some(ref val) = args.default_reasoning_effort {
        if val == "none" {
            settings.default_reasoning_effort = None;
        } else {
            settings.default_reasoning_effort = normalize_reasoning_effort(Some(val.as_str()));
        }
        changed = true;
    }
    if let Some(ref val) = args.default_model_openai {
        let trimmed = val.trim();
        settings.default_model_openai = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        changed = true;
    }
    if let Some(ref val) = args.default_model_anthropic {
        let trimmed = val.trim();
        settings.default_model_anthropic = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        changed = true;
    }
    if let Some(ref val) = args.default_model_openrouter {
        let trimmed = val.trim();
        settings.default_model_openrouter = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        changed = true;
    }
    if let Some(ref val) = args.default_model_cerebras {
        let trimmed = val.trim();
        settings.default_model_cerebras = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        changed = true;
    }
    if let Some(ref val) = args.default_model_ollama {
        let trimmed = val.trim();
        settings.default_model_ollama = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        changed = true;
    }

    if changed {
        let _ = store.save(&settings);
        settings = settings.normalized();
        println!("Saved persistent defaults to .openplanter/settings.json");
    }

    // Apply default model from settings if not overridden by CLI or env.
    if args.model.is_none()
        && std::env::var("OPENPLANTER_MODEL").is_err()
        && settings.default_model.is_some()
    {
        cfg.model = settings.default_model.clone().unwrap();
    }
    if args.reasoning_effort.is_none()
        && std::env::var("OPENPLANTER_REASONING_EFFORT").is_err()
        && settings.default_reasoning_effort.is_some()
    {
        cfg.reasoning_effort = settings.default_reasoning_effort.clone();
    }

    settings
}

fn print_settings(settings: &PersistentSettings) {
    println!("Persistent settings:");
    println!(
        "  default_model: {}",
        settings.default_model.as_deref().unwrap_or("(unset)")
    );
    println!(
        "  default_reasoning_effort: {}",
        settings
            .default_reasoning_effort
            .as_deref()
            .unwrap_or("(unset)")
    );
    println!(
        "  default_model_openai: {}",
        settings
            .default_model_openai
            .as_deref()
            .unwrap_or("(unset)")
    );
    println!(
        "  default_model_anthropic: {}",
        settings
            .default_model_anthropic
            .as_deref()
            .unwrap_or("(unset)")
    );
    println!(
        "  default_model_openrouter: {}",
        settings
            .default_model_openrouter
            .as_deref()
            .unwrap_or("(unset)")
    );
    println!(
        "  default_model_cerebras: {}",
        settings
            .default_model_cerebras
            .as_deref()
            .unwrap_or("(unset)")
    );
    println!(
        "  default_model_ollama: {}",
        settings
            .default_model_ollama
            .as_deref()
            .unwrap_or("(unset)")
    );
}

// ---------------------------------------------------------------------------
// Non-interactive command detection
// ---------------------------------------------------------------------------

fn has_non_interactive_command(args: &Cli) -> bool {
    args.task.is_some()
        || args.list_models
        || args.list_sessions
        || args.show_settings
        || args.configure_keys
        || args.default_model.is_some()
        || args.default_reasoning_effort.is_some()
        || args.default_model_openai.is_some()
        || args.default_model_anthropic.is_some()
        || args.default_model_openrouter.is_some()
        || args.default_model_cerebras.is_some()
        || args.default_model_ollama.is_some()
}

// ---------------------------------------------------------------------------
// TTY detection
// ---------------------------------------------------------------------------

fn is_tty() -> bool {
    atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout)
}

// ---------------------------------------------------------------------------
// Startup info display
// ---------------------------------------------------------------------------

fn print_startup(info: &[(String, String)]) {
    for (key, val) in info {
        println!("{:>10}  {}", key, val);
    }
    println!();
}

// ---------------------------------------------------------------------------
// Plain REPL
// ---------------------------------------------------------------------------

async fn run_plain_repl(engine: &op_engine::RLMEngine) {
    println!("OpenPlanter Agent (plain mode). Type /quit to exit.");
    loop {
        print!("you> ");
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        match std::io::stdin().read_line(&mut input) {
            Ok(0) => {
                // EOF
                println!();
                break;
            }
            Ok(_) => {}
            Err(_) => {
                println!();
                break;
            }
        }

        let objective = input.trim();
        if objective.is_empty() {
            continue;
        }
        if objective == "/quit" || objective == "/exit" {
            break;
        }
        if objective == "/clear" {
            continue;
        }

        let on_event: Option<op_engine::engine::EventCallback> =
            Some(Arc::new(|msg: &str| {
                println!("trace> {}", clip_event(msg));
            }));

        let result = engine.solve(objective, on_event).await;
        println!("agent> {}", result);
    }
}

/// Truncate an event string to a reasonable display length.
fn clip_event(ev: &str) -> String {
    if ev.len() <= 300 {
        ev.to_string()
    } else {
        format!("{}...", &ev[..300])
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize tracing (respects RUST_LOG env).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let mut args = Cli::parse();

    // TTY detection — force no-tui if headless or non-TTY.
    let non_tty = !is_tty();
    if args.headless || non_tty {
        args.no_tui = true;
    }

    // Load configuration from environment.
    let mut cfg = AgentConfig::from_env(Path::new(&args.workspace));

    let settings_store = SettingsStore::new(&cfg.workspace, &cfg.session_root_dir);
    let settings = apply_persistent_settings(&mut cfg, &args, &settings_store);

    // --list-sessions: print and exit.
    if args.list_sessions {
        match SessionStore::new(&cfg.workspace, &cfg.session_root_dir) {
            Ok(store) => {
                match store.list_sessions(200) {
                    Ok(sessions) => {
                        if sessions.is_empty() {
                            println!("No sessions found.");
                        } else {
                            for sess in &sessions {
                                let created = sess.created_at.as_deref().unwrap_or("unknown");
                                let updated = sess.updated_at.as_deref().unwrap_or("unknown");
                                println!(
                                    "{} | created={} | updated={}",
                                    sess.session_id, created, updated
                                );
                            }
                        }
                    }
                    Err(e) => {
                        println!("Error listing sessions: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("Error opening session store: {}", e);
            }
        }
        return ExitCode::SUCCESS;
    }

    // --show-settings: print, and exit unless --task or --list-models also given.
    if args.show_settings {
        print_settings(&settings);
        if args.task.is_none() && !args.list_models {
            return ExitCode::SUCCESS;
        }
    }

    // Headless guard: require a non-interactive command.
    if (args.headless || non_tty) && !has_non_interactive_command(&args) {
        println!(
            "Headless/non-interactive mode requires --task or a non-interactive command \
             (e.g., --list-models, --show-settings)."
        );
        return ExitCode::from(2);
    }

    // Load credentials from all sources.
    let creds = load_credentials(&cfg, &args, !(args.headless || non_tty));

    // Apply runtime overrides from CLI args.
    apply_runtime_overrides(&mut cfg, &args, &creds);

    // If model is still empty, try provider-specific default from settings.
    if cfg.model.trim().is_empty() {
        if let Some(provider_model) = settings.default_model_for_provider(&cfg.provider) {
            cfg.model = provider_model.to_string();
        }
    }

    // --configure-keys only (no task/list/settings): done.
    if args.configure_keys && args.task.is_none() && !args.list_models && !args.show_settings {
        println!("Credential configuration step complete.");
        return ExitCode::SUCCESS;
    }

    // --list-models
    if args.list_models {
        let requested_provider = args
            .provider
            .as_deref()
            .unwrap_or("auto")
            .trim()
            .to_lowercase();
        let rc = print_models(&cfg, &requested_provider).await;
        if rc != 0 {
            return ExitCode::from(rc as u8);
        }
        return ExitCode::SUCCESS;
    }

    // Provider "all" is only valid with --list-models.
    if cfg.provider == "all" {
        println!("Provider 'all' is only valid with --list-models.");
        return ExitCode::from(2);
    }

    // Model-provider cross-check: auto-switch provider if the model clearly
    // belongs to a different provider and we have a key for it.
    let model_for_check = cfg.model.trim().to_string();
    if !model_for_check.is_empty() && cfg.provider != "openrouter" {
        if let Some(inferred) = infer_provider_for_model(&model_for_check) {
            if inferred != cfg.provider {
                let has_key = match inferred {
                    "openai" => cfg.openai_api_key.is_some(),
                    "anthropic" => cfg.anthropic_api_key.is_some(),
                    "openrouter" => cfg.openrouter_api_key.is_some(),
                    "cerebras" => cfg.cerebras_api_key.is_some(),
                    "ollama" => true,
                    _ => false,
                };
                if has_key {
                    cfg.provider = inferred.to_string();
                } else {
                    println!(
                        "Model '{}' requires provider '{}' but no API key is configured for it.",
                        model_for_check, inferred
                    );
                    return ExitCode::from(1);
                }
            }
        }
    }

    // Build engine.
    let engine = build_engine(&cfg).await;

    // Get model display name from engine.
    let model_name = cfg.model.clone();

    // Open or resume session.
    let session_store = match SessionStore::new(&cfg.workspace, &cfg.session_root_dir) {
        Ok(s) => s,
        Err(e) => {
            println!("Session error: {}", e);
            return ExitCode::from(1);
        }
    };

    let (session_id, _session_state, _created_new) = match session_store.open_session(
        args.session_id.as_deref(),
        args.resume,
    ) {
        Ok(v) => v,
        Err(e) => {
            println!("Session error: {}", e);
            return ExitCode::SUCCESS;
        }
    };

    // Build startup info.
    let mut startup_info: Vec<(String, String)> = Vec::new();
    startup_info.push(("Provider".to_string(), cfg.provider.clone()));
    startup_info.push(("Model".to_string(), model_name.clone()));
    if let Some(ref effort) = cfg.reasoning_effort {
        startup_info.push(("Reasoning".to_string(), effort.clone()));
    }
    startup_info.push((
        "Mode".to_string(),
        if cfg.recursive {
            "recursive".to_string()
        } else {
            "flat".to_string()
        },
    ));
    startup_info.push(("Workspace".to_string(), cfg.workspace.display().to_string()));
    startup_info.push(("Session".to_string(), session_id.clone()));

    // --task: headless task mode.
    if let Some(ref task) = args.task {
        print_startup(&startup_info);

        let on_event: Option<op_engine::engine::EventCallback> =
            Some(Arc::new(|msg: &str| {
                println!("trace> {}", clip_event(msg));
            }));

        let result = engine.solve(task, on_event).await;
        println!("{}", result);
        return ExitCode::SUCCESS;
    }

    // --no-tui: plain REPL.
    if args.no_tui {
        if !atty::is(atty::Stream::Stdin) {
            println!("No interactive stdin available; use --task for headless execution.");
            return ExitCode::from(2);
        }
        print_startup(&startup_info);
        run_plain_repl(&engine).await;
        return ExitCode::SUCCESS;
    }

    // Full TUI mode — attempt to launch, fall back to plain REPL.
    // TUI module is currently a stub; for now, fall back to plain REPL.
    print_startup(&startup_info);
    run_plain_repl(&engine).await;
    ExitCode::SUCCESS
}
