/// Recursive Language Model Engine — core execution loop.
///
/// Matches Python's agent/engine.py:RLMEngine.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use op_core::{AgentConfig, ToolResult};
use op_model::LlmModel;
use op_tools::WorkspaceTools;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::condensation::{
    context_window_for_model, summarize_args, summarize_observation,
};
use crate::context::ExternalContext;
use crate::dispatch::dispatch_tool_call;
use crate::judge::judge_result;
use crate::prompts::build_system_prompt;

/// Callback types matching Python's EventCallback, StepCallback, ContentDeltaCallback.
pub type EventCallback = Arc<dyn Fn(&str) + Send + Sync>;
pub type StepCallback = Arc<dyn Fn(&serde_json::Value) + Send + Sync>;
pub type ContentDeltaCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;
pub type ModelFactory = Arc<dyn Fn(&str, Option<&str>) -> Box<dyn LlmModel> + Send + Sync>;

/// The Recursive Language Model Engine.
pub struct RLMEngine {
    pub model: Box<dyn LlmModel>,
    pub tools: Arc<WorkspaceTools>,
    pub config: AgentConfig,
    pub system_prompt: String,
    pub session_tokens: Mutex<HashMap<String, HashMap<String, u64>>>,
    pub model_factory: Option<ModelFactory>,
    model_cache: Mutex<HashMap<(String, Option<String>), Box<dyn LlmModel>>>,
    pub session_dir: Option<PathBuf>,
    pub session_id: Option<String>,
    shell_command_counts: Mutex<HashMap<(u32, String), u32>>,
    cancel: CancellationToken,
}

impl RLMEngine {
    pub fn new(
        model: Box<dyn LlmModel>,
        tools: WorkspaceTools,
        config: AgentConfig,
    ) -> Self {
        let system_prompt = build_system_prompt(
            config.recursive,
            config.acceptance_criteria,
            config.demo,
        );
        Self {
            model,
            tools: Arc::new(tools),
            config,
            system_prompt,
            session_tokens: Mutex::new(HashMap::new()),
            model_factory: None,
            model_cache: Mutex::new(HashMap::new()),
            session_dir: None,
            session_id: None,
            shell_command_counts: Mutex::new(HashMap::new()),
            cancel: CancellationToken::new(),
        }
    }

    /// Signal the engine to stop after the current model call or tool.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Simple solve — returns the result string.
    pub async fn solve(
        &self,
        objective: &str,
        on_event: Option<EventCallback>,
    ) -> String {
        let (result, _) = self
            .solve_with_context(objective, None, on_event, None, None)
            .await;
        result
    }

    /// Full solve with external context and callbacks.
    pub async fn solve_with_context(
        &self,
        objective: &str,
        context: Option<ExternalContext>,
        on_event: Option<EventCallback>,
        on_step: Option<StepCallback>,
        on_content_delta: Option<ContentDeltaCallback>,
    ) -> (String, ExternalContext) {
        if objective.trim().is_empty() {
            return (
                "No objective provided.".to_string(),
                context.unwrap_or_default(),
            );
        }

        self.shell_command_counts.lock().clear();
        let mut active_context = context.unwrap_or_default();

        let deadline = if self.config.max_solve_seconds > 0 {
            Some(Instant::now() + std::time::Duration::from_secs(self.config.max_solve_seconds))
        } else {
            None
        };

        let result = self
            .solve_recursive(
                objective.trim(),
                0,
                &mut active_context,
                on_event,
                on_step,
                on_content_delta,
                deadline,
                None,
            )
            .await;

        self.tools.cleanup_bg_jobs().await;
        (result, active_context)
    }

    fn emit(&self, msg: &str, on_event: &Option<EventCallback>) {
        if let Some(ref cb) = on_event {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(msg)));
        }
    }

    fn clip_observation(&self, text: &str) -> String {
        let max = self.config.max_observation_chars;
        if text.len() <= max {
            text.to_string()
        } else {
            format!(
                "{}\n...[truncated {} chars]...",
                &text[..max],
                text.len() - max
            )
        }
    }

    fn runtime_policy_check(&self, name: &str, args: &serde_json::Value, depth: u32) -> Option<String> {
        if name != "run_shell" {
            return None;
        }
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if command.is_empty() {
            return None;
        }
        let key = (depth, command);
        let count = {
            let mut counts = self.shell_command_counts.lock();
            let c = counts.entry(key).or_insert(0);
            *c += 1;
            *c
        };
        if count <= 2 {
            None
        } else {
            Some(
                "Blocked by runtime policy: identical run_shell command repeated more than twice \
                 at the same depth. Change strategy instead of retrying the same command."
                    .to_string(),
            )
        }
    }

    /// Main recursive solve loop.
    async fn solve_recursive(
        &self,
        objective: &str,
        depth: u32,
        context: &mut ExternalContext,
        on_event: Option<EventCallback>,
        on_step: Option<StepCallback>,
        _on_content_delta: Option<ContentDeltaCallback>,
        deadline: Option<Instant>,
        model_override: Option<&dyn LlmModel>,
    ) -> String {
        let model: &dyn LlmModel = model_override.unwrap_or(self.model.as_ref());

        self.emit(
            &format!("[depth {}] objective: {}", depth, objective),
            &on_event,
        );

        let now_iso = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        // Build initial message
        let mut initial_msg = serde_json::json!({
            "timestamp": now_iso,
            "objective": objective,
            "max_steps_per_call": self.config.max_steps_per_call,
            "workspace": self.config.workspace.to_string_lossy(),
            "external_context_summary": context.summary(12, 8000),
        });

        if self.config.recursive {
            let repl_hint = if depth == 0 {
                "Begin REPL cycle 1: start with a broad READ of the workspace."
            } else {
                "Begin REPL cycle 1: parent has surveyed — READ only what this objective requires, then act."
            };
            initial_msg["depth"] = serde_json::json!(depth);
            initial_msg["max_depth"] = serde_json::json!(self.config.max_depth);
            initial_msg["repl_hint"] = serde_json::json!(repl_hint);
        }

        if let Some(ref dir) = self.session_dir {
            initial_msg["session_dir"] = serde_json::json!(dir.to_string_lossy());
        }
        if let Some(ref id) = self.session_id {
            initial_msg["session_id"] = serde_json::json!(id);
        }

        let initial_message = serde_json::to_string(&initial_msg).unwrap_or_default();
        let mut conversation = model.create_conversation(&self.system_prompt, &initial_message);

        for step in 1..=self.config.max_steps_per_call {
            if self.cancel.is_cancelled() {
                self.emit(
                    &format!("[d{}] cancelled by user", depth),
                    &on_event,
                );
                return "Task cancelled.".to_string();
            }

            if let Some(dl) = deadline {
                if Instant::now() > dl {
                    self.emit(
                        &format!("[d{}] wall-clock limit reached", depth),
                        &on_event,
                    );
                    return "Time limit exceeded. Try a more focused objective.".to_string();
                }
            }

            self.emit(
                &format!("[d{}/s{}] calling model...", depth, step),
                &on_event,
            );

            let t0 = Instant::now();
            let turn = match model.complete(&mut conversation).await {
                Ok(t) => t,
                Err(e) => {
                    self.emit(
                        &format!("[d{}/s{}] model error: {}", depth, step, e),
                        &on_event,
                    );
                    return format!("Model error at depth {}, step {}: {}", depth, step, e);
                }
            };
            let elapsed = t0.elapsed().as_secs_f64();

            // Track token usage
            if turn.input_tokens > 0 || turn.output_tokens > 0 {
                let model_name = "unknown".to_string(); // Would need model.model() accessor
                let mut tokens = self.session_tokens.lock();
                let bucket = tokens
                    .entry(model_name)
                    .or_insert_with(|| {
                        let mut m = HashMap::new();
                        m.insert("input".to_string(), 0u64);
                        m.insert("output".to_string(), 0u64);
                        m
                    });
                *bucket.entry("input".to_string()).or_insert(0) += turn.input_tokens;
                *bucket.entry("output".to_string()).or_insert(0) += turn.output_tokens;
            }

            model.append_assistant_turn(&mut conversation, &turn);

            // Emit step event
            if let Some(ref cb) = on_step {
                let step_event = serde_json::json!({
                    "depth": depth,
                    "step": step,
                    "objective": objective,
                    "action": {"name": "_model_turn"},
                    "observation": "",
                    "model_text": turn.text.as_deref().unwrap_or(""),
                    "tool_call_names": turn.tool_calls.iter().map(|tc| tc.name.as_str()).collect::<Vec<_>>(),
                    "input_tokens": turn.input_tokens,
                    "output_tokens": turn.output_tokens,
                    "elapsed_sec": (elapsed * 100.0).round() / 100.0,
                    "is_final": false,
                });
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(&step_event)));
            }

            // No tool calls + text = final answer
            if turn.tool_calls.is_empty() {
                if let Some(ref text) = turn.text {
                    let preview = if text.len() > 200 {
                        format!("{}...", &text[..200])
                    } else {
                        text.clone()
                    };
                    self.emit(
                        &format!(
                            "[d{}/s{}] final answer ({} chars, {:.1}s): {}",
                            depth,
                            step,
                            text.len(),
                            elapsed,
                            preview
                        ),
                        &on_event,
                    );

                    if let Some(ref cb) = on_step {
                        let final_event = serde_json::json!({
                            "depth": depth,
                            "step": step,
                            "objective": objective,
                            "action": {"name": "final", "arguments": {"text": text}},
                            "observation": text,
                            "is_final": true,
                        });
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            cb(&final_event)
                        }));
                    }

                    return text.clone();
                }

                // No tool calls and no text — nudge
                self.emit(
                    &format!(
                        "[d{}/s{}] empty model response ({:.1}s), nudging...",
                        depth, step, elapsed
                    ),
                    &on_event,
                );
                let nudge = ToolResult::ok(
                    "empty".to_string(),
                    "system".to_string(),
                    "No tool calls and no text in response. Please use a tool or provide a final answer."
                        .to_string(),
                );
                model.append_tool_results(&mut conversation, &[nudge]);
                continue;
            }

            // Log tool calls
            let tc_names: Vec<&str> = turn.tool_calls.iter().map(|tc| tc.name.as_str()).collect();
            self.emit(
                &format!(
                    "[d{}/s{}] model returned {} tool call(s) ({:.1}s): {}",
                    depth,
                    step,
                    turn.tool_calls.len(),
                    elapsed,
                    tc_names.join(", ")
                ),
                &on_event,
            );

            // Execute tool calls sequentially (subtask/execute handled by engine)
            let mut results: Vec<ToolResult> = Vec::new();
            let mut final_answer: Option<String> = None;

            for tc in &turn.tool_calls {
                if self.cancel.is_cancelled() {
                    results.push(ToolResult::ok(
                        tc.id.clone(),
                        tc.name.clone(),
                        "Task cancelled.".to_string(),
                    ));
                    break;
                }

                // Policy check
                if let Some(policy_err) =
                    self.runtime_policy_check(&tc.name, &tc.arguments, depth)
                {
                    results.push(ToolResult::ok(
                        tc.id.clone(),
                        tc.name.clone(),
                        policy_err,
                    ));
                    continue;
                }

                let arg_summary = summarize_args(&tc.arguments, 120);
                self.emit(
                    &format!("[d{}/s{}] {}({})", depth, step, tc.name, arg_summary),
                    &on_event,
                );

                let t1 = Instant::now();

                // Handle subtask/execute specially
                if tc.name == "subtask" || tc.name == "execute" {
                    let obj = tc
                        .arguments
                        .get("objective")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    if obj.is_empty() {
                        results.push(ToolResult::ok(
                            tc.id.clone(),
                            tc.name.clone(),
                            format!("{} requires objective", tc.name),
                        ));
                        continue;
                    }
                    if depth >= self.config.max_depth {
                        results.push(ToolResult::ok(
                            tc.id.clone(),
                            tc.name.clone(),
                            "Max recursion depth reached.".to_string(),
                        ));
                        continue;
                    }

                    let criteria = tc
                        .arguments
                        .get("acceptance_criteria")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();

                    if self.config.acceptance_criteria && criteria.is_empty() {
                        results.push(ToolResult::ok(
                            tc.id.clone(),
                            tc.name.clone(),
                            format!(
                                "{} requires acceptance_criteria when acceptance criteria mode is enabled.",
                                tc.name
                            ),
                        ));
                        continue;
                    }

                    self.emit(
                        &format!("[d{}] >> entering {}: {}", depth, tc.name, obj),
                        &on_event,
                    );

                    // Recursive call
                    let sub_result = Box::pin(self.solve_recursive(
                        obj,
                        depth + 1,
                        context,
                        on_event.clone(),
                        on_step.clone(),
                        None,
                        deadline,
                        None,
                    ))
                    .await;

                    let mut observation =
                        format!("{} result for '{}':\n{}", tc.name.as_str(), obj, sub_result);

                    // Judge if acceptance criteria provided
                    if !criteria.is_empty() && self.config.acceptance_criteria {
                        let verdict =
                            judge_result(obj, &criteria, &sub_result, self.model.as_ref()).await;
                        let tag = if verdict.starts_with("PASS") {
                            "PASS"
                        } else {
                            "FAIL"
                        };
                        observation +=
                            &format!("\n\n[ACCEPTANCE CRITERIA: {}]\n{}", tag, verdict);
                    }

                    let observation = self.clip_observation(&observation);
                    results.push(ToolResult::ok(
                        tc.id.clone(),
                        tc.name.clone(),
                        observation,
                    ));
                    continue;
                }

                // Regular tool dispatch
                let (is_final, observation) =
                    dispatch_tool_call(&self.tools, tc).await;
                let observation = self.clip_observation(&observation);
                let tool_elapsed = t1.elapsed().as_secs_f64();

                let obs_summary = summarize_observation(&observation, 200);
                self.emit(
                    &format!(
                        "[d{}/s{}]   -> {} ({:.1}s)",
                        depth, step, obs_summary, tool_elapsed
                    ),
                    &on_event,
                );

                if let Some(ref cb) = on_step {
                    let tool_event = serde_json::json!({
                        "depth": depth,
                        "step": step,
                        "objective": objective,
                        "action": {"name": tc.name, "arguments": tc.arguments},
                        "observation": observation,
                        "elapsed_sec": (tool_elapsed * 100.0).round() / 100.0,
                        "is_final": is_final,
                    });
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        cb(&tool_event)
                    }));
                }

                results.push(ToolResult::ok(
                    tc.id.clone(),
                    tc.name.clone(),
                    observation.clone(),
                ));

                if is_final {
                    final_answer = Some(observation);
                    break;
                }
            }

            // Add budget warnings
            if final_answer.is_none() && !results.is_empty() {
                let budget_total = self.config.max_steps_per_call;
                let remaining = budget_total - step;

                // Add timestamp and budget tags to first result
                let now_tag = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
                let model_name = "unknown";
                let ctx_window = context_window_for_model(model_name);
                let budget_tag = format!("[Step {}/{}]", step, budget_total);
                let ctx_tag = format!("[Context {}/{} tokens]", turn.input_tokens, ctx_window);

                if let Some(first) = results.first_mut() {
                    first.content = format!(
                        "[{}] {} {} {}",
                        now_tag, budget_tag, ctx_tag, first.content
                    );
                }

                if remaining > 0 && remaining <= budget_total / 4 {
                    if let Some(last) = results.last_mut() {
                        last.content += &format!(
                            "\n\n** BUDGET CRITICAL: {} of {} steps remain. \
                             Stop exploring/surveying. Write your output files NOW with your best answer. \
                             A partial result beats no result.",
                            remaining, budget_total
                        );
                    }
                } else if remaining <= budget_total / 2 {
                    if let Some(last) = results.last_mut() {
                        last.content += &format!(
                            "\n\n** BUDGET WARNING: {} of {} steps remain. \
                             Focus on completing the task directly. Do not write exploration scripts.",
                            remaining, budget_total
                        );
                    }
                }
            }

            // Plan injection
            if let Some(ref session_dir) = self.session_dir {
                if !results.is_empty() && final_answer.is_none() {
                    if let Ok(mut entries) = std::fs::read_dir(session_dir) {
                        let mut plan_files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
                        while let Some(Ok(entry)) = entries.next() {
                            let path = entry.path();
                            if path.extension().and_then(|e| e.to_str()) == Some("md")
                                && path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|n| n.ends_with(".plan.md"))
                                    .unwrap_or(false)
                            {
                                if let Ok(meta) = path.metadata() {
                                    if let Ok(mtime) = meta.modified() {
                                        plan_files.push((path, mtime));
                                    }
                                }
                            }
                        }
                        plan_files.sort_by(|a, b| b.1.cmp(&a.1));
                        if let Some((plan_path, _)) = plan_files.first() {
                            if let Ok(plan_text) = std::fs::read_to_string(plan_path) {
                                if !plan_text.trim().is_empty() {
                                    let max_pc = self.config.max_plan_chars;
                                    let plan_display = if plan_text.len() > max_pc {
                                        format!("{}\n...[plan truncated]...", &plan_text[..max_pc])
                                    } else {
                                        plan_text
                                    };
                                    let plan_name = plan_path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("plan.md");
                                    let plan_block = format!(
                                        "\n[SESSION PLAN file={}]\n{}\n[/SESSION PLAN]\n",
                                        plan_name, plan_display
                                    );
                                    if let Some(last) = results.last_mut() {
                                        last.content += &plan_block;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            model.append_tool_results(&mut conversation, &results);

            if let Some(answer) = final_answer {
                self.emit(
                    &format!("[d{}] completed in {} step(s)", depth, step),
                    &on_event,
                );
                return answer;
            }

            // Add observations to context
            for r in &results {
                context.add(format!("[depth {} step {}]\n{}", depth, step, r.content));
            }
        }

        format!(
            "Step budget exhausted at depth {} for objective: {}\n\
             Please try with a more specific task, higher step budget, or deeper recursion.",
            depth, objective
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clip_observation_short() {
        let config = AgentConfig::from_env(std::path::Path::new("/tmp"));
        let tools = WorkspaceTools::new(std::path::Path::new("/tmp"));
        let engine = RLMEngine::new(
            Box::new(op_model::EchoFallbackModel::default()),
            tools,
            config,
        );
        let clipped = engine.clip_observation("short text");
        assert_eq!(clipped, "short text");
    }

    #[test]
    fn test_clip_observation_long() {
        let mut config = AgentConfig::from_env(std::path::Path::new("/tmp"));
        config.max_observation_chars = 20;
        let tools = WorkspaceTools::new(std::path::Path::new("/tmp"));
        let engine = RLMEngine::new(
            Box::new(op_model::EchoFallbackModel::default()),
            tools,
            config,
        );
        let long = "a".repeat(100);
        let clipped = engine.clip_observation(&long);
        assert!(clipped.contains("truncated"));
        assert!(clipped.len() < 100);
    }

    #[test]
    fn test_runtime_policy_check_allows_first_two() {
        let config = AgentConfig::from_env(std::path::Path::new("/tmp"));
        let tools = WorkspaceTools::new(std::path::Path::new("/tmp"));
        let engine = RLMEngine::new(
            Box::new(op_model::EchoFallbackModel::default()),
            tools,
            config,
        );
        let args = serde_json::json!({"command": "ls -la"});
        assert!(engine.runtime_policy_check("run_shell", &args, 0).is_none());
        assert!(engine.runtime_policy_check("run_shell", &args, 0).is_none());
        assert!(engine
            .runtime_policy_check("run_shell", &args, 0)
            .is_some());
    }

    #[test]
    fn test_runtime_policy_check_ignores_non_shell() {
        let config = AgentConfig::from_env(std::path::Path::new("/tmp"));
        let tools = WorkspaceTools::new(std::path::Path::new("/tmp"));
        let engine = RLMEngine::new(
            Box::new(op_model::EchoFallbackModel::default()),
            tools,
            config,
        );
        let args = serde_json::json!({"path": "/tmp/test"});
        assert!(engine
            .runtime_policy_check("read_file", &args, 0)
            .is_none());
    }
}
