//! Session runtime: wraps an engine + store, handles bootstrap, solve, and persistence.
//!
//! Port of `agent/runtime.py` `SessionRuntime`.
//!
//! Since `op-engine` is not yet built, the engine dependency is abstracted behind
//! the [`Solvable`] trait. The real `RLMEngine` will implement this trait later.

use std::path::PathBuf;

use op_core::{AgentConfig, OpResult};
use serde_json::{json, Value};
use tracing::warn;

use crate::replay_log::ReplayLogger;
use crate::session_store::{SessionState, SessionStore};
use crate::wiki::seed_wiki;

// ---------------------------------------------------------------------------
// Abstractions for the engine layer (not yet built)
// ---------------------------------------------------------------------------

/// External context carried across solve calls (observations accumulated by the agent).
#[derive(Debug, Clone, Default)]
pub struct ExternalContext {
    pub observations: Vec<String>,
}

/// Result of a single solve call.
#[derive(Debug)]
pub struct SolveResult {
    pub answer: String,
    pub updated_context: ExternalContext,
}

/// Callback types matching the Python signatures.
pub type EventCallback = Box<dyn FnMut(&str) + Send>;
pub type StepCallback = Box<dyn FnMut(&Value) + Send>;
pub type ContentDeltaCallback = Box<dyn FnMut(&str) + Send>;

/// Trait abstracting the engine's solve capability.
///
/// The real `RLMEngine` will implement this once `op-engine` is built.
pub trait Solvable: Send {
    /// Set the session directory on the engine (for file operations).
    fn set_session_dir(&mut self, dir: PathBuf);

    /// Set the session id on the engine.
    fn set_session_id(&mut self, id: String);

    /// Solve an objective within the given context.
    fn solve_with_context(
        &mut self,
        objective: &str,
        context: &ExternalContext,
        on_event: Option<EventCallback>,
        on_step: Option<StepCallback>,
        on_content_delta: Option<ContentDeltaCallback>,
        replay_logger: &mut ReplayLogger,
    ) -> OpResult<SolveResult>;
}

// ---------------------------------------------------------------------------
// SessionRuntime
// ---------------------------------------------------------------------------

/// High-level runtime that orchestrates session lifecycle around an engine.
pub struct SessionRuntime<E: Solvable> {
    pub engine: E,
    pub store: SessionStore,
    pub session_id: String,
    pub context: ExternalContext,
    pub max_persisted_observations: usize,
}

impl<E: Solvable> SessionRuntime<E> {
    /// Bootstrap a new (or resumed) session runtime.
    ///
    /// This is the primary constructor, matching Python's `SessionRuntime.bootstrap`.
    pub fn bootstrap(
        mut engine: E,
        config: &AgentConfig,
        session_id: Option<&str>,
        resume: bool,
    ) -> OpResult<Self> {
        let store = SessionStore::new(&config.workspace, &config.session_root_dir)?;

        // Seed wiki (non-fatal).
        seed_wiki(&config.workspace, &config.session_root_dir);

        let (sid, state, created_new) = store.open_session(session_id, resume)?;

        // Restore persisted observations.
        let persisted: Vec<String> = state.external_observations;
        let max_obs = config.max_persisted_observations.max(1);
        let start = if persisted.len() > max_obs {
            persisted.len() - max_obs
        } else {
            0
        };
        let context = ExternalContext {
            observations: persisted[start..].to_vec(),
        };

        // Inform the engine of the session location.
        engine.set_session_dir(store.session_dir(&sid));
        engine.set_session_id(sid.clone());

        let mut runtime = Self {
            engine,
            store,
            session_id: sid.clone(),
            context,
            max_persisted_observations: max_obs,
        };

        // Record session_started event (non-fatal).
        if let Err(e) = runtime.store.append_event(
            &sid,
            "session_started",
            &json!({"resume": resume, "created_new": created_new}),
        ) {
            warn!("failed to write session_started event: {}", e);
        }

        // Persist initial state (non-fatal).
        if let Err(e) = runtime.persist_state() {
            warn!("failed to persist initial state: {}", e);
        }

        Ok(runtime)
    }

    /// Solve an objective, persisting events and state along the way.
    pub fn solve(
        &mut self,
        objective: &str,
        on_event: Option<EventCallback>,
        on_step: Option<StepCallback>,
        on_content_delta: Option<ContentDeltaCallback>,
    ) -> OpResult<String> {
        let objective = objective.trim();
        if objective.is_empty() {
            return Ok("No objective provided.".to_string());
        }

        // Log objective event (non-fatal).
        let _ = self.store.append_event(
            &self.session_id,
            "objective",
            &json!({"text": objective}),
        );

        // We need to move the user callbacks into the wrappers.
        let mut user_on_event = on_event;
        let mut user_on_step = on_step;

        let wrapped_on_event: EventCallback = {
            Box::new(move |msg: &str| {
                // Forward to user callback.
                if let Some(ref mut cb) = user_on_event {
                    cb(msg);
                }
            })
        };

        let wrapped_on_step: StepCallback = {
            Box::new(move |step_event: &Value| {
                // Forward to user callback.
                if let Some(ref mut cb) = user_on_step {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(step_event)));
                }
            })
        };

        // Create replay logger.
        let replay_path = self.store.session_dir(&self.session_id).join("replay.jsonl");
        let mut replay_logger = ReplayLogger::new(replay_path);

        let result = self.engine.solve_with_context(
            objective,
            &self.context,
            Some(wrapped_on_event),
            Some(wrapped_on_step),
            on_content_delta,
            &mut replay_logger,
        )?;

        self.context = result.updated_context;

        // Log result event (non-fatal).
        let _ = self.store.append_event(
            &self.session_id,
            "result",
            &json!({"text": result.answer}),
        );

        // Persist state (non-fatal).
        if let Err(e) = self.persist_state() {
            warn!("failed to persist state after solve: {}", e);
        }

        Ok(result.answer)
    }

    /// Persist the current context (observations) to state.json.
    fn persist_state(&mut self) -> OpResult<()> {
        // Trim observations if needed.
        let obs = &mut self.context.observations;
        if obs.len() > self.max_persisted_observations {
            let start = obs.len() - self.max_persisted_observations;
            *obs = obs[start..].to_vec();
        }

        let state = SessionState {
            session_id: self.session_id.clone(),
            external_observations: self.context.observations.clone(),
            saved_at: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        };
        self.store.save_state(&self.session_id, &state)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Mock engine for testing.
    struct MockEngine {
        session_dir: Option<PathBuf>,
        session_id: Option<String>,
        solve_response: String,
        solve_observations: Vec<String>,
    }

    impl MockEngine {
        fn new(response: &str) -> Self {
            Self {
                session_dir: None,
                session_id: None,
                solve_response: response.to_string(),
                solve_observations: vec!["test observation".to_string()],
            }
        }
    }

    impl Solvable for MockEngine {
        fn set_session_dir(&mut self, dir: PathBuf) {
            self.session_dir = Some(dir);
        }

        fn set_session_id(&mut self, id: String) {
            self.session_id = Some(id);
        }

        fn solve_with_context(
            &mut self,
            _objective: &str,
            context: &ExternalContext,
            _on_event: Option<EventCallback>,
            _on_step: Option<StepCallback>,
            _on_content_delta: Option<ContentDeltaCallback>,
            _replay_logger: &mut ReplayLogger,
        ) -> OpResult<SolveResult> {
            let mut obs = context.observations.clone();
            obs.extend(self.solve_observations.clone());
            Ok(SolveResult {
                answer: self.solve_response.clone(),
                updated_context: ExternalContext { observations: obs },
            })
        }
    }

    fn temp_workspace(name: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("op_runtime_sr_test")
            .join(name)
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn test_config(ws: &Path) -> AgentConfig {
        AgentConfig {
            workspace: ws.to_path_buf(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            reasoning_effort: None,
            base_url: String::new(),
            api_key: None,
            openai_base_url: String::new(),
            anthropic_base_url: String::new(),
            openrouter_base_url: String::new(),
            cerebras_base_url: String::new(),
            ollama_base_url: String::new(),
            exa_base_url: String::new(),
            openai_api_key: None,
            anthropic_api_key: None,
            openrouter_api_key: None,
            cerebras_api_key: None,
            exa_api_key: None,
            voyage_api_key: None,
            max_depth: 4,
            max_steps_per_call: 100,
            max_observation_chars: 6000,
            command_timeout_sec: 45,
            shell: "/bin/sh".to_string(),
            max_files_listed: 400,
            max_file_chars: 20000,
            max_search_hits: 200,
            max_shell_output_chars: 16000,
            session_root_dir: ".openplanter".to_string(),
            max_persisted_observations: 400,
            max_solve_seconds: 0,
            recursive: true,
            min_subtask_depth: 0,
            acceptance_criteria: true,
            max_plan_chars: 40000,
            demo: false,
        }
    }

    #[test]
    fn test_bootstrap_new_session() {
        let ws = temp_workspace("bootstrap_new");
        let config = test_config(&ws);
        let engine = MockEngine::new("test answer");

        let runtime = SessionRuntime::bootstrap(engine, &config, None, false).unwrap();

        assert!(!runtime.session_id.is_empty());
        assert!(runtime.context.observations.is_empty());
        assert!(ws
            .join(".openplanter")
            .join("sessions")
            .join(&runtime.session_id)
            .exists());

        // Engine should have been informed of session dir.
        assert!(runtime.engine.session_dir.is_some());
        assert_eq!(
            runtime.engine.session_id.as_deref(),
            Some(runtime.session_id.as_str())
        );

        // Events file should contain session_started.
        let events_path = ws
            .join(".openplanter")
            .join("sessions")
            .join(&runtime.session_id)
            .join("events.jsonl");
        let events_content = fs::read_to_string(&events_path).unwrap();
        assert!(events_content.contains("session_started"));

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_bootstrap_resume() {
        let ws = temp_workspace("bootstrap_resume");
        let config = test_config(&ws);

        // Create a first session.
        let engine1 = MockEngine::new("answer1");
        let runtime1 = SessionRuntime::bootstrap(engine1, &config, None, false).unwrap();
        let first_sid = runtime1.session_id.clone();
        drop(runtime1);

        // Resume it.
        let engine2 = MockEngine::new("answer2");
        let runtime2 = SessionRuntime::bootstrap(engine2, &config, None, true).unwrap();
        assert_eq!(runtime2.session_id, first_sid);

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_bootstrap_resume_no_sessions_error() {
        let ws = temp_workspace("bootstrap_resume_err");
        let config = test_config(&ws);
        let engine = MockEngine::new("x");

        let result = SessionRuntime::bootstrap(engine, &config, None, true);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_solve_empty_objective() {
        let ws = temp_workspace("solve_empty");
        let config = test_config(&ws);
        let engine = MockEngine::new("should not appear");

        let mut runtime = SessionRuntime::bootstrap(engine, &config, None, false).unwrap();
        let result = runtime.solve("  ", None, None, None).unwrap();
        assert_eq!(result, "No objective provided.");

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_solve_basic() {
        let ws = temp_workspace("solve_basic");
        let config = test_config(&ws);
        let engine = MockEngine::new("42");

        let mut runtime = SessionRuntime::bootstrap(engine, &config, None, false).unwrap();
        let result = runtime.solve("What is the answer?", None, None, None).unwrap();
        assert_eq!(result, "42");

        // Context should have accumulated an observation.
        assert!(!runtime.context.observations.is_empty());

        // State should be persisted.
        let state = runtime.store.load_state(&runtime.session_id).unwrap();
        assert!(!state.external_observations.is_empty());

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_solve_persists_events() {
        let ws = temp_workspace("solve_events");
        let config = test_config(&ws);
        let engine = MockEngine::new("done");

        let mut runtime = SessionRuntime::bootstrap(engine, &config, None, false).unwrap();
        let sid = runtime.session_id.clone();
        runtime.solve("do something", None, None, None).unwrap();

        let events_path = ws
            .join(".openplanter")
            .join("sessions")
            .join(&sid)
            .join("events.jsonl");
        let content = fs::read_to_string(&events_path).unwrap();
        assert!(content.contains("session_started"));
        assert!(content.contains("objective"));
        assert!(content.contains("result"));

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_observation_trimming() {
        let ws = temp_workspace("obs_trim");
        let mut config = test_config(&ws);
        config.max_persisted_observations = 3;

        let mut engine = MockEngine::new("ok");
        engine.solve_observations = vec!["obs_new".to_string()];

        let mut runtime = SessionRuntime::bootstrap(engine, &config, None, false).unwrap();

        // Simulate having accumulated many observations.
        runtime.context.observations = vec![
            "obs1".into(),
            "obs2".into(),
            "obs3".into(),
            "obs4".into(),
        ];

        // Solve will add one more (from MockEngine).
        runtime.solve("go", None, None, None).unwrap();

        // Should be trimmed to max_persisted_observations (3).
        assert!(runtime.context.observations.len() <= 3);

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_wiki_seeding_during_bootstrap() {
        let ws = temp_workspace("wiki_seed");
        let wiki_dir = ws.join("wiki");
        fs::create_dir_all(&wiki_dir).unwrap();
        fs::write(wiki_dir.join("test.md"), "# Test").unwrap();

        let config = test_config(&ws);
        let engine = MockEngine::new("ok");

        let _runtime = SessionRuntime::bootstrap(engine, &config, None, false).unwrap();

        assert!(ws.join(".openplanter").join("wiki").join("test.md").exists());

        let _ = fs::remove_dir_all(&ws);
    }
}
