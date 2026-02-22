/// Context window management and condensation heuristics.
use std::collections::HashMap;

/// Known model context window sizes (tokens).
pub static MODEL_CONTEXT_WINDOWS: &[(&str, usize)] = &[
    ("claude-opus-4-6", 200_000),
    ("claude-sonnet-4-5-20250929", 200_000),
    ("claude-haiku-4-5-20251001", 200_000),
    ("gpt-4o", 128_000),
    ("gpt-4.1", 1_000_000),
    ("gpt-5-turbo-16k", 16_000),
];

pub const DEFAULT_CONTEXT_WINDOW: usize = 128_000;
pub const CONDENSATION_THRESHOLD: f64 = 0.75;

/// Get the context window size for a model name.
pub fn context_window_for_model(model_name: &str) -> usize {
    for &(name, size) in MODEL_CONTEXT_WINDOWS {
        if model_name == name {
            return size;
        }
    }
    DEFAULT_CONTEXT_WINDOW
}

/// Check if we should trigger condensation based on input tokens used.
pub fn should_condense(model_name: &str, input_tokens: u64) -> bool {
    let window = context_window_for_model(model_name) as f64;
    (input_tokens as f64) > CONDENSATION_THRESHOLD * window
}

/// Determine the model tier for delegation control.
/// Lower number = higher capability.
///
/// Anthropic chain: opus → 1, sonnet → 2, haiku → 3
/// OpenAI codex chain (by reasoning effort): xhigh → 1, high → 2, medium → 3, low → 4
/// Unknown → 2
pub fn model_tier(model_name: &str, reasoning_effort: Option<&str>) -> u32 {
    let lower = model_name.to_lowercase();
    if lower.contains("opus") {
        return 1;
    }
    if lower.contains("sonnet") {
        return 2;
    }
    if lower.contains("haiku") {
        return 3;
    }
    if lower.starts_with("gpt-5") && lower.contains("codex") {
        let effort_map: HashMap<&str, u32> =
            [("xhigh", 1), ("high", 2), ("medium", 3), ("low", 4)]
                .into_iter()
                .collect();
        if let Some(effort) = reasoning_effort {
            return *effort_map.get(effort.to_lowercase().as_str()).unwrap_or(&2);
        }
        return 2;
    }
    2
}

/// Return (model_name, reasoning_effort) for the lowest-tier executor.
/// Anthropic models → haiku. Unknown → no downgrade.
pub fn lowest_tier_model(model_name: &str) -> (&'static str, Option<&'static str>) {
    let lower = model_name.to_lowercase();
    if lower.contains("claude") {
        return ("claude-haiku-4-5-20251001", None);
    }
    // For non-Claude models, we can't downgrade — return a static reference.
    // The caller should handle this case.
    ("claude-haiku-4-5-20251001", None)
}

/// Summarize tool call arguments for display (one-line).
pub fn summarize_args(args: &serde_json::Value, max_len: usize) -> String {
    let obj = match args.as_object() {
        Some(o) => o,
        None => return args.to_string(),
    };
    let mut parts = Vec::new();
    for (k, v) in obj {
        let s = v.to_string();
        let display = if s.len() > 60 {
            format!("{}...", &s[..57])
        } else {
            s
        };
        parts.push(format!("{}={}", k, display));
    }
    let joined = parts.join(", ");
    if joined.len() > max_len {
        format!("{}...", &joined[..max_len.saturating_sub(3)])
    } else {
        joined
    }
}

/// First line or truncated preview of an observation.
pub fn summarize_observation(text: &str, max_len: usize) -> String {
    let first = text.split('\n').next().unwrap_or("").trim();
    let first = if first.len() > max_len {
        format!("{}...", &first[..max_len.saturating_sub(3)])
    } else {
        first.to_string()
    };
    let lines = text.chars().filter(|&c| c == '\n').count() + 1;
    let chars = text.len();
    if lines > 1 {
        format!("{} ({} lines, {} chars)", first, lines, chars)
    } else {
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_window_known_model() {
        assert_eq!(context_window_for_model("claude-opus-4-6"), 200_000);
        assert_eq!(context_window_for_model("gpt-4o"), 128_000);
    }

    #[test]
    fn test_context_window_unknown_model() {
        assert_eq!(
            context_window_for_model("unknown-model"),
            DEFAULT_CONTEXT_WINDOW
        );
    }

    #[test]
    fn test_should_condense() {
        // 75% of 200k = 150k
        assert!(!should_condense("claude-opus-4-6", 100_000));
        assert!(should_condense("claude-opus-4-6", 160_000));
    }

    #[test]
    fn test_model_tier() {
        assert_eq!(model_tier("claude-opus-4-6", None), 1);
        assert_eq!(model_tier("claude-sonnet-4-5", None), 2);
        assert_eq!(model_tier("claude-haiku-4-5", None), 3);
        assert_eq!(model_tier("unknown-model", None), 2);
    }

    #[test]
    fn test_lowest_tier_model() {
        let (name, effort) = lowest_tier_model("claude-opus-4-6");
        assert_eq!(name, "claude-haiku-4-5-20251001");
        assert!(effort.is_none());
    }

    #[test]
    fn test_summarize_args() {
        let args = serde_json::json!({"path": "/tmp/test.txt", "content": "hello"});
        let s = summarize_args(&args, 120);
        assert!(s.contains("path"));
        assert!(s.contains("content"));
    }

    #[test]
    fn test_summarize_observation_single_line() {
        let s = summarize_observation("short line", 200);
        assert_eq!(s, "short line");
    }

    #[test]
    fn test_summarize_observation_multiline() {
        let s = summarize_observation("first line\nsecond line\nthird", 200);
        assert!(s.contains("first line"));
        assert!(s.contains("3 lines"));
    }

    #[test]
    fn test_summarize_observation_truncation() {
        let long = "a".repeat(300);
        let s = summarize_observation(&long, 100);
        assert!(s.ends_with("..."));
        assert!(s.len() <= 110);
    }
}
