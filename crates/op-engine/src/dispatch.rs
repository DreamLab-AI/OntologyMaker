/// Tool call dispatch logic — routes tool calls to WorkspaceTools methods.
use op_core::ToolCall;
use op_tools::WorkspaceTools;

/// Result of dispatching a tool call: (is_final_answer, observation_text).
pub type DispatchResult = (bool, String);

/// Dispatch a tool call to the appropriate WorkspaceTools method.
///
/// Returns (is_final, observation). `is_final` is always false for regular tools.
/// The engine handles subtask/execute separately.
pub async fn dispatch_tool_call(
    tools: &WorkspaceTools,
    tool_call: &ToolCall,
) -> DispatchResult {
    let name = tool_call.name.as_str();
    let args = &tool_call.arguments;

    match name {
        "think" => {
            let note = args
                .get("note")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            (false, format!("Thought noted: {}", note))
        }

        "list_files" => {
            let glob = args.get("glob").and_then(|v| v.as_str());
            let result = tools.list_files(glob);
            (false, result)
        }

        "search_files" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if query.is_empty() {
                return (false, "search_files requires non-empty query".to_string());
            }
            let glob = args.get("glob").and_then(|v| v.as_str());
            let result = tools.search_files(query, glob);
            (false, result)
        }

        "repo_map" => {
            let glob = args.get("glob").and_then(|v| v.as_str());
            let max_files = args
                .get("max_files")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            let result = tools.repo_map(glob, max_files);
            (false, result)
        }

        "web_search" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if query.is_empty() {
                return (false, "web_search requires non-empty query".to_string());
            }
            let num_results = args
                .get("num_results")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let include_text = args
                .get("include_text")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let result = tools.web_search(query, num_results, include_text).await;
            (false, result)
        }

        "fetch_url" => {
            let urls = args
                .get("urls")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if urls.is_empty() {
                return (false, "fetch_url requires a list of URL strings".to_string());
            }
            let result = tools.fetch_url(&urls).await;
            (false, result)
        }

        "read_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if path.is_empty() {
                return (false, "read_file requires path".to_string());
            }
            let hashline = args
                .get("hashline")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let result = tools.read_file(path, hashline);
            (false, result)
        }

        "read_image" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if path.is_empty() {
                return (false, "read_image requires path".to_string());
            }
            let result = tools.read_image(path);
            // The image data is handled separately by the engine via pending_image.
            (false, result.0)
        }

        "write_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if path.is_empty() {
                return (false, "write_file requires path".to_string());
            }
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let result = tools.write_file(path, content);
            (false, result)
        }

        "apply_patch" => {
            let patch = args
                .get("patch")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if patch.is_empty() {
                return (false, "apply_patch requires non-empty patch".to_string());
            }
            let result = tools.apply_patch(patch);
            (false, result)
        }

        "edit_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if path.is_empty() {
                return (false, "edit_file requires path".to_string());
            }
            let old_text = args
                .get("old_text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_text = args
                .get("new_text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if old_text.is_empty() {
                return (false, "edit_file requires old_text".to_string());
            }
            let result = tools.edit_file(path, old_text, new_text);
            (false, result)
        }

        "hashline_edit" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if path.is_empty() {
                return (false, "hashline_edit requires path".to_string());
            }
            let edits = args
                .get("edits")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let result = tools.hashline_edit_json(path, &edits);
            (false, result)
        }

        "run_shell" => {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if command.is_empty() {
                return (false, "run_shell requires command".to_string());
            }
            let timeout = args
                .get("timeout")
                .and_then(|v| v.as_u64());
            let result = tools.run_shell(command, timeout).await;
            (false, result)
        }

        "run_shell_bg" => {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if command.is_empty() {
                return (false, "run_shell_bg requires command".to_string());
            }
            let result = tools.run_shell_bg(command).await;
            (false, result)
        }

        "check_shell_bg" => {
            let job_id = args.get("job_id").and_then(|v| v.as_u64());
            match job_id {
                Some(id) => {
                    let result = tools.check_shell_bg(id).await;
                    (false, result)
                }
                None => (false, "check_shell_bg requires job_id".to_string()),
            }
        }

        "kill_shell_bg" => {
            let job_id = args.get("job_id").and_then(|v| v.as_u64());
            match job_id {
                Some(id) => {
                    let result = tools.kill_shell_bg(id).await;
                    (false, result)
                }
                None => (false, "kill_shell_bg requires job_id".to_string()),
            }
        }

        "list_artifacts" | "read_artifact" => {
            // These are handled by the engine directly, not tools
            (false, format!("Tool {} is handled by the engine directly", name))
        }

        "subtask" | "execute" => {
            // These are handled by the engine's recursive solver, not here
            (false, format!("Tool {} is handled by the engine's recursive solver", name))
        }

        _ => (false, format!("Unknown action type: {}", name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn test_think_dispatch() {
        let ws = WorkspaceTools::new(Path::new("/tmp"));
        let tc = ToolCall {
            id: "tc_1".into(),
            name: "think".into(),
            arguments: serde_json::json!({"note": "testing"}),
        };
        let (is_final, result) = dispatch_tool_call(&ws, &tc).await;
        assert!(!is_final);
        assert!(result.contains("Thought noted"));
        assert!(result.contains("testing"));
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let ws = WorkspaceTools::new(Path::new("/tmp"));
        let tc = ToolCall {
            id: "tc_2".into(),
            name: "nonexistent".into(),
            arguments: serde_json::json!({}),
        };
        let (_, result) = dispatch_tool_call(&ws, &tc).await;
        assert!(result.contains("Unknown action type"));
    }
}
