/// Acceptance criteria judging using a lightweight model.
use op_model::LlmModel;

use crate::condensation::lowest_tier_model;

/// Evaluate a subtask/execute result against acceptance criteria using a cheap judge model.
///
/// Returns a verdict string starting with "PASS:" or "FAIL:".
pub async fn judge_result(
    objective: &str,
    acceptance_criteria: &str,
    result: &str,
    judge_model: &dyn LlmModel,
) -> String {
    let truncated = if result.len() > 4000 {
        &result[..4000]
    } else {
        result
    };

    let prompt = format!(
        "You are a judge evaluating whether a task result meets acceptance criteria.\n\n\
         Objective: {}\n\n\
         Acceptance criteria: {}\n\n\
         Result:\n{}\n\n\
         Respond with exactly one line starting with PASS: or FAIL: followed by a brief explanation.",
        objective, acceptance_criteria, truncated
    );

    let mut conversation = judge_model.create_conversation("You are a concise evaluator.", &prompt);
    match judge_model.complete(&mut conversation).await {
        Ok(turn) => {
            let verdict = turn.text.unwrap_or_default().trim().to_string();
            if verdict.is_empty() {
                "PASS\n(judge returned empty response)".to_string()
            } else {
                verdict
            }
        }
        Err(e) => format!("PASS\n(judge error: {})", e),
    }
}

/// Get the model name to use as a judge (lowest-tier model).
pub fn judge_model_name(current_model_name: &str) -> (&'static str, Option<&'static str>) {
    lowest_tier_model(current_model_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_judge_model_name_claude() {
        let (name, effort) = judge_model_name("claude-opus-4-6");
        assert_eq!(name, "claude-haiku-4-5-20251001");
        assert!(effort.is_none());
    }

    #[test]
    fn test_judge_model_name_unknown() {
        let (name, _effort) = judge_model_name("some-random-model");
        // Should still return haiku as default
        assert_eq!(name, "claude-haiku-4-5-20251001");
    }
}
