//! Provider-neutral tool definitions for the OpenPlanter agent.
//!
//! Single source of truth for tool schemas. Converter helpers produce the
//! provider-specific shapes expected by OpenAI and Anthropic APIs.
//!
//! Ports Python `tool_defs.py`.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::LazyLock;

/// All tool definitions, matching the Python `TOOL_DEFINITIONS` list exactly.
pub static TOOL_DEFINITIONS: LazyLock<Vec<Value>> = LazyLock::new(|| {
    vec![
        json!({
            "name": "list_files",
            "description": "List files in the workspace directory. Optionally filter with a glob pattern.",
            "parameters": {
                "type": "object",
                "properties": {
                    "glob": {
                        "type": "string",
                        "description": "Optional glob pattern to filter files."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "search_files",
            "description": "Search file contents in the workspace for a text or regex query.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Text or regex to search for."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional glob pattern to restrict which files are searched."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "repo_map",
            "description": "Build a lightweight map of source files and symbols to speed up code navigation.",
            "parameters": {
                "type": "object",
                "properties": {
                    "glob": {
                        "type": "string",
                        "description": "Optional glob pattern to limit which files are scanned."
                    },
                    "max_files": {
                        "type": "integer",
                        "description": "Maximum number of files to scan (1-500, default 200)."
                    }
                },
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "web_search",
            "description": "Search the web using the Exa API. Returns URLs, titles, and optional page text.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Web search query string."
                    },
                    "num_results": {
                        "type": "integer",
                        "description": "Number of results to return (1-20, default 10)."
                    },
                    "include_text": {
                        "type": "boolean",
                        "description": "Whether to include page text in results."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "fetch_url",
            "description": "Fetch and return the text content of one or more URLs.",
            "parameters": {
                "type": "object",
                "properties": {
                    "urls": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of URLs to fetch."
                    }
                },
                "required": ["urls"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "read_file",
            "description": "Read the contents of a file in the workspace. Lines are numbered LINE:HASH|content by default for use with hashline_edit. Set hashline=false for plain N|content.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path within the workspace."
                    },
                    "hashline": {
                        "type": "boolean",
                        "description": "Prefix each line with LINE:HASH| format for content verification. Default true."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "read_image",
            "description": "Read an image file and return it for visual analysis. Supports PNG, JPEG, GIF, WebP.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative or absolute path to the image file within the workspace."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "write_file",
            "description": "Create or overwrite a file in the workspace with the given content.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path for the file."
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file content to write."
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "apply_patch",
            "description": "Apply a Codex-style patch to one or more files. Use the *** Begin Patch / *** End Patch format with Update File, Add File, and Delete File operations.",
            "parameters": {
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "The full patch block in Codex patch format."
                    }
                },
                "required": ["patch"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "edit_file",
            "description": "Replace a specific text span in a file. Provide the exact old text to find and the new text to replace it with. The old text must appear exactly once in the file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file to edit."
                    },
                    "old_text": {
                        "type": "string",
                        "description": "The exact text to find and replace."
                    },
                    "new_text": {
                        "type": "string",
                        "description": "The replacement text."
                    }
                },
                "required": ["path", "old_text", "new_text"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "hashline_edit",
            "description": "Edit a file using hash-anchored line references from read_file(hashline=true). Operations: set_line (replace one line), replace_lines (replace a range), insert_after (insert new lines after an anchor).",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file."
                    },
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "set_line": {
                                    "type": "string",
                                    "description": "Anchor 'N:HH' for single-line replace."
                                },
                                "replace_lines": {
                                    "type": "object",
                                    "description": "Range with 'start' and 'end' anchors.",
                                    "properties": {
                                        "start": { "type": "string" },
                                        "end": { "type": "string" }
                                    },
                                    "required": ["start", "end"],
                                    "additionalProperties": false
                                },
                                "insert_after": {
                                    "type": "string",
                                    "description": "Anchor 'N:HH' to insert after."
                                },
                                "content": {
                                    "type": "string",
                                    "description": "New content for the operation."
                                }
                            },
                            "required": [],
                            "additionalProperties": false
                        },
                        "description": "Edit operations: set_line, replace_lines, or insert_after."
                    }
                },
                "required": ["path", "edits"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "run_shell",
            "description": "Execute a shell command from the workspace root and return its output.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds for this command (default: agent default, max: 600)."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "run_shell_bg",
            "description": "Start a shell command in the background. Returns a job ID to check or kill later.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to run in the background."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "check_shell_bg",
            "description": "Check the status and output of a background job started with run_shell_bg.",
            "parameters": {
                "type": "object",
                "properties": {
                    "job_id": {
                        "type": "integer",
                        "description": "The job ID returned by run_shell_bg."
                    }
                },
                "required": ["job_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "kill_shell_bg",
            "description": "Kill a background job started with run_shell_bg.",
            "parameters": {
                "type": "object",
                "properties": {
                    "job_id": {
                        "type": "integer",
                        "description": "The job ID returned by run_shell_bg."
                    }
                },
                "required": ["job_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "think",
            "description": "Record an internal planning thought. Use this to reason about the task before acting.",
            "parameters": {
                "type": "object",
                "properties": {
                    "note": {
                        "type": "string",
                        "description": "Your planning thought or reasoning note."
                    }
                },
                "required": ["note"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "subtask",
            "description": "Spawn a recursive sub-agent to solve a smaller sub-problem. The result is returned as an observation.",
            "parameters": {
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "Clear objective for the sub-agent to accomplish."
                    },
                    "model": {
                        "type": "string",
                        "description": "Optional model for subtask (e.g. 'claude-sonnet-4-5-20250929', 'claude-haiku-4-5-20251001')."
                    },
                    "reasoning_effort": {
                        "type": "string",
                        "enum": ["xhigh", "high", "medium", "low"],
                        "description": "Optional reasoning effort for the subtask model. For OpenAI codex models, this controls the delegation level."
                    },
                    "acceptance_criteria": {
                        "type": "string",
                        "description": "Acceptance criteria for judging the subtask result. A lightweight judge evaluates the result against these criteria and appends PASS/FAIL to your observation. Be specific and verifiable."
                    }
                },
                "required": ["objective", "acceptance_criteria"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "execute",
            "description": "Hand an atomic sub-problem to a leaf executor agent with full tool access. Use this when the sub-problem requires no further decomposition and can be solved directly (e.g. write a file, run tests, apply a patch). The executor has no subtask or execute tools — it must solve the objective in one pass.",
            "parameters": {
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "Clear, specific objective for the executor to accomplish."
                    },
                    "acceptance_criteria": {
                        "type": "string",
                        "description": "Acceptance criteria for judging the executor result. A lightweight judge evaluates the result against these criteria and appends PASS/FAIL to your observation. Be specific and verifiable."
                    }
                },
                "required": ["objective", "acceptance_criteria"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_artifacts",
            "description": "List artifacts from previous subagent runs. Returns ID, objective, and result summary for each.",
            "parameters": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "read_artifact",
            "description": "Read a previous subagent's conversation log artifact. Returns JSONL records of the subagent's full conversation.",
            "parameters": {
                "type": "object",
                "properties": {
                    "artifact_id": {
                        "type": "string",
                        "description": "Artifact ID from list_artifacts."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Start line (0-indexed). Default 0."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max lines to return. Default 100."
                    }
                },
                "required": ["artifact_id"],
                "additionalProperties": false
            }
        }),
    ]
});

static ARTIFACT_TOOLS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ["list_artifacts", "read_artifact"].into_iter().collect());

static DELEGATION_TOOLS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    ["subtask", "execute", "list_artifacts", "read_artifact"]
        .into_iter()
        .collect()
});

/// Strip `acceptance_criteria` property from subtask/execute schemas.
fn strip_acceptance_criteria(defs: &[Value]) -> Vec<Value> {
    defs.iter()
        .map(|d| {
            let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name == "subtask" || name == "execute" {
                let mut d = d.clone();
                if let Some(params) = d.get_mut("parameters") {
                    if let Some(props) = params.get_mut("properties") {
                        if let Some(obj) = props.as_object_mut() {
                            obj.remove("acceptance_criteria");
                        }
                    }
                    if let Some(req) = params.get_mut("required") {
                        if let Some(arr) = req.as_array_mut() {
                            arr.retain(|v| v.as_str() != Some("acceptance_criteria"));
                        }
                    }
                }
                d
            } else {
                d.clone()
            }
        })
        .collect()
}

/// Return tool definitions based on mode.
///
/// - `include_subtask=true` (normal recursive) -> everything except execute, artifact tools.
/// - `include_subtask=false` (flat / executor) -> no subtask, no execute, no artifact tools.
/// - `include_artifacts=true` -> add list_artifacts + read_artifact.
/// - `include_acceptance_criteria=false` -> strip acceptance_criteria from schemas.
pub fn get_tool_definitions(
    include_subtask: bool,
    include_artifacts: bool,
    include_acceptance_criteria: bool,
) -> Vec<Value> {
    let mut defs: Vec<Value> = if include_subtask {
        TOOL_DEFINITIONS
            .iter()
            .filter(|d| {
                let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
                name != "execute" && !ARTIFACT_TOOLS.contains(name)
            })
            .cloned()
            .collect()
    } else {
        TOOL_DEFINITIONS
            .iter()
            .filter(|d| {
                let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
                !DELEGATION_TOOLS.contains(name)
            })
            .cloned()
            .collect()
    };

    if include_artifacts {
        let artifact_defs: Vec<Value> = TOOL_DEFINITIONS
            .iter()
            .filter(|d| {
                let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
                ARTIFACT_TOOLS.contains(name)
            })
            .cloned()
            .collect();
        defs.extend(artifact_defs);
    }

    if !include_acceptance_criteria {
        defs = strip_acceptance_criteria(&defs);
    }

    defs
}

/// Recursively enforce OpenAI strict-mode constraints on a schema in-place.
fn strict_fixup(schema: &mut Value) {
    let schema_type = schema
        .get("type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match schema_type.as_deref() {
        Some("object") => {
            let all_keys: Vec<String> = schema
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();

            let required: HashSet<String> = schema
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            // First, recursively fix up child properties
            for key in &all_keys {
                if let Some(prop) = schema
                    .get_mut("properties")
                    .and_then(|v| v.get_mut(key.as_str()))
                {
                    if prop.is_object() {
                        strict_fixup(prop);
                    }
                }
            }

            // Then make optional properties nullable
            for key in &all_keys {
                if !required.contains(key) {
                    if let Some(props) = schema.get_mut("properties").and_then(|v| v.as_object_mut())
                    {
                        if let Some(prop) = props.get(key).cloned() {
                            if let Some(type_val) = prop.get("type").cloned() {
                                let desc = prop.get("description").cloned();

                                // Build anyOf with the original type schema and null
                                let mut original_schema = serde_json::Map::new();
                                original_schema.insert("type".to_string(), type_val);

                                // Copy other properties from original to the first anyOf option
                                if let Some(obj) = prop.as_object() {
                                    for (k, v) in obj {
                                        if k != "type" && k != "description" {
                                            original_schema.insert(k.clone(), v.clone());
                                        }
                                    }
                                }

                                let null_schema = json!({"type": "null"});
                                let mut new_prop = serde_json::Map::new();
                                new_prop.insert(
                                    "anyOf".to_string(),
                                    json!([Value::Object(original_schema), null_schema]),
                                );
                                if let Some(d) = desc {
                                    new_prop.insert("description".to_string(), d);
                                }

                                props.insert(key.clone(), Value::Object(new_prop));
                            }
                        }
                    }
                }
            }

            // Set required to all keys
            schema["required"] = json!(all_keys);
            schema["additionalProperties"] = json!(false);
        }
        Some("array") => {
            if let Some(items) = schema.get_mut("items") {
                if items.is_object() {
                    strict_fixup(items);
                }
            }
        }
        _ => {}
    }
}

/// For OpenAI strict mode: make all properties required, make optional ones nullable.
fn make_strict_parameters(params: &Value) -> Value {
    let mut out = params.clone();
    strict_fixup(&mut out);
    out
}

/// Convert provider-neutral definitions to OpenAI tools array format.
pub fn to_openai_tools(defs: Option<&[Value]>, strict: bool) -> Vec<Value> {
    let definitions = defs.unwrap_or(&TOOL_DEFINITIONS);
    definitions
        .iter()
        .map(|d| {
            let parameters = if strict {
                make_strict_parameters(d.get("parameters").unwrap_or(&json!({})))
            } else {
                d.get("parameters").cloned().unwrap_or(json!({}))
            };
            let mut function = json!({
                "name": d.get("name").unwrap_or(&json!("")),
                "description": d.get("description").unwrap_or(&json!("")),
                "parameters": parameters,
            });
            if strict {
                function["strict"] = json!(true);
            }
            json!({
                "type": "function",
                "function": function,
            })
        })
        .collect()
}

/// Convert provider-neutral definitions to Anthropic tools array format.
pub fn to_anthropic_tools(defs: Option<&[Value]>) -> Vec<Value> {
    let definitions = defs.unwrap_or(&TOOL_DEFINITIONS);
    definitions
        .iter()
        .map(|d| {
            json!({
                "name": d.get("name").unwrap_or(&json!("")),
                "description": d.get("description").unwrap_or(&json!("")),
                "input_schema": d.get("parameters").unwrap_or(&json!({})),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions_count() {
        // Python has exactly 20 definitions
        assert_eq!(TOOL_DEFINITIONS.len(), 20);
    }

    #[test]
    fn test_tool_definitions_names() {
        let names: Vec<&str> = TOOL_DEFINITIONS
            .iter()
            .filter_map(|d| d.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"list_files"));
        assert!(names.contains(&"search_files"));
        assert!(names.contains(&"repo_map"));
        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"fetch_url"));
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"read_image"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"apply_patch"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"hashline_edit"));
        assert!(names.contains(&"run_shell"));
        assert!(names.contains(&"run_shell_bg"));
        assert!(names.contains(&"check_shell_bg"));
        assert!(names.contains(&"kill_shell_bg"));
        assert!(names.contains(&"think"));
        assert!(names.contains(&"subtask"));
        assert!(names.contains(&"execute"));
        assert!(names.contains(&"list_artifacts"));
        assert!(names.contains(&"read_artifact"));
    }

    #[test]
    fn test_get_tool_definitions_with_subtask() {
        let defs = get_tool_definitions(true, false, true);
        let names: Vec<&str> = defs
            .iter()
            .filter_map(|d| d.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"subtask"));
        assert!(!names.contains(&"execute"));
        assert!(!names.contains(&"list_artifacts"));
        assert!(!names.contains(&"read_artifact"));
    }

    #[test]
    fn test_get_tool_definitions_without_subtask() {
        let defs = get_tool_definitions(false, false, true);
        let names: Vec<&str> = defs
            .iter()
            .filter_map(|d| d.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(!names.contains(&"subtask"));
        assert!(!names.contains(&"execute"));
        assert!(!names.contains(&"list_artifacts"));
        assert!(!names.contains(&"read_artifact"));
    }

    #[test]
    fn test_get_tool_definitions_with_artifacts() {
        let defs = get_tool_definitions(true, true, true);
        let names: Vec<&str> = defs
            .iter()
            .filter_map(|d| d.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"list_artifacts"));
        assert!(names.contains(&"read_artifact"));
    }

    #[test]
    fn test_strip_acceptance_criteria() {
        let defs = get_tool_definitions(true, false, false);
        for d in &defs {
            let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name == "subtask" {
                let props = d
                    .get("parameters")
                    .and_then(|v| v.get("properties"))
                    .and_then(|v| v.as_object())
                    .unwrap();
                assert!(
                    !props.contains_key("acceptance_criteria"),
                    "subtask should not have acceptance_criteria"
                );
                let req = d
                    .get("parameters")
                    .and_then(|v| v.get("required"))
                    .and_then(|v| v.as_array())
                    .unwrap();
                assert!(!req
                    .iter()
                    .any(|v| v.as_str() == Some("acceptance_criteria")));
            }
        }
    }

    #[test]
    fn test_to_openai_tools_shape() {
        let tools = to_openai_tools(None, false);
        assert!(!tools.is_empty());
        for tool in &tools {
            assert_eq!(tool.get("type").and_then(|v| v.as_str()), Some("function"));
            let func = tool.get("function").unwrap();
            assert!(func.get("name").is_some());
            assert!(func.get("description").is_some());
            assert!(func.get("parameters").is_some());
        }
    }

    #[test]
    fn test_to_openai_tools_strict() {
        let tools = to_openai_tools(None, true);
        for tool in &tools {
            let func = tool.get("function").unwrap();
            assert_eq!(func.get("strict").and_then(|v| v.as_bool()), Some(true));
        }
    }

    #[test]
    fn test_to_openai_tools_strict_parameters() {
        let defs = vec![json!({
            "name": "test",
            "description": "test tool",
            "parameters": {
                "type": "object",
                "properties": {
                    "required_field": {
                        "type": "string",
                        "description": "This is required."
                    },
                    "optional_field": {
                        "type": "integer",
                        "description": "This is optional."
                    }
                },
                "required": ["required_field"],
                "additionalProperties": false
            }
        })];
        let tools = to_openai_tools(Some(&defs), true);
        let func = tools[0].get("function").unwrap();
        let params = func.get("parameters").unwrap();

        // All keys should be in required
        let required = params
            .get("required")
            .and_then(|v| v.as_array())
            .unwrap();
        let req_names: Vec<&str> = required
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(req_names.contains(&"required_field"));
        assert!(req_names.contains(&"optional_field"));

        // Optional field should have anyOf with null
        let opt = params
            .get("properties")
            .and_then(|v| v.get("optional_field"))
            .unwrap();
        assert!(opt.get("anyOf").is_some());
        let any_of = opt.get("anyOf").unwrap().as_array().unwrap();
        assert_eq!(any_of.len(), 2);
        assert_eq!(
            any_of[1].get("type").and_then(|v| v.as_str()),
            Some("null")
        );
    }

    #[test]
    fn test_to_anthropic_tools_shape() {
        let tools = to_anthropic_tools(None);
        assert!(!tools.is_empty());
        for tool in &tools {
            assert!(tool.get("name").is_some());
            assert!(tool.get("description").is_some());
            assert!(tool.get("input_schema").is_some());
            // Anthropic format should NOT have "type": "function" wrapper
            assert!(tool.get("type").is_none());
        }
    }

    #[test]
    fn test_to_anthropic_tools_custom_defs() {
        let custom = vec![json!({
            "name": "custom_tool",
            "description": "A custom tool",
            "parameters": {
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }
        })];
        let tools = to_anthropic_tools(Some(&custom));
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("name").and_then(|v| v.as_str()),
            Some("custom_tool")
        );
    }

    #[test]
    fn test_strict_fixup_nested_array() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" }
                        },
                        "required": [],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["items"],
            "additionalProperties": false
        });
        strict_fixup(&mut schema);
        // The nested object in array items should also be fixed up
        let nested = schema
            .get("properties")
            .and_then(|v| v.get("items"))
            .and_then(|v| v.get("items"))
            .unwrap();
        let nested_req = nested.get("required").and_then(|v| v.as_array()).unwrap();
        assert!(nested_req
            .iter()
            .any(|v| v.as_str() == Some("name")));
    }
}
