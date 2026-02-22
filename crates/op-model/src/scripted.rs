use async_trait::async_trait;
use op_core::{Conversation, ModelTurn, OpError, OpResult, ToolResult};
use serde_json::json;
use std::sync::Mutex;

use crate::traits::LlmModel;

/// Model that returns pre-scripted `ModelTurn` responses for testing.
///
/// Each call to `complete` pops the first turn from the list.
/// When the list is exhausted, it returns an error.
pub struct ScriptedModel {
    /// The remaining scripted turns, wrapped in a Mutex for interior mutability
    /// since `complete` takes `&self`.
    scripted_turns: Mutex<Vec<ModelTurn>>,
}

impl ScriptedModel {
    /// Create a new ScriptedModel with the given list of turns.
    pub fn new(turns: Vec<ModelTurn>) -> Self {
        Self {
            scripted_turns: Mutex::new(turns),
        }
    }

    /// Returns the number of remaining scripted turns.
    pub fn remaining(&self) -> usize {
        self.scripted_turns.lock().unwrap().len()
    }
}

#[async_trait]
impl LlmModel for ScriptedModel {
    fn create_conversation(
        &self,
        system_prompt: &str,
        initial_user_message: &str,
    ) -> Conversation {
        let messages = vec![json!({"role": "user", "content": initial_user_message})];
        let mut conv = Conversation::new(system_prompt.to_string());
        conv.provider_messages = messages;
        conv
    }

    async fn complete(&self, _conversation: &Conversation) -> OpResult<ModelTurn> {
        let mut turns = self.scripted_turns.lock().unwrap();
        if turns.is_empty() {
            return Err(OpError::model(
                "ScriptedModel exhausted; no responses left.",
            ));
        }
        Ok(turns.remove(0))
    }

    fn append_assistant_turn(&self, _conversation: &mut Conversation, _turn: &ModelTurn) {
        // No-op for scripted model
    }

    fn append_tool_results(
        &self,
        _conversation: &mut Conversation,
        _results: &[ToolResult],
    ) {
        // No-op for scripted model
    }

    fn condense_conversation(
        &self,
        _conversation: &mut Conversation,
        _keep_recent_turns: usize,
    ) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use op_core::ToolCall;

    #[test]
    fn test_scripted_model_remaining() {
        let model = ScriptedModel::new(vec![
            ModelTurn {
                text: Some("first".to_string()),
                ..Default::default()
            },
            ModelTurn {
                text: Some("second".to_string()),
                ..Default::default()
            },
        ]);
        assert_eq!(model.remaining(), 2);
    }

    #[tokio::test]
    async fn test_scripted_model_complete() {
        let model = ScriptedModel::new(vec![
            ModelTurn {
                text: Some("first".to_string()),
                stop_reason: "end_turn".to_string(),
                ..Default::default()
            },
            ModelTurn {
                text: Some("second".to_string()),
                stop_reason: "end_turn".to_string(),
                ..Default::default()
            },
        ]);

        let conv = model.create_conversation("sys", "hi");

        let turn1 = model.complete(&conv).await.unwrap();
        assert_eq!(turn1.text.as_deref(), Some("first"));
        assert_eq!(model.remaining(), 1);

        let turn2 = model.complete(&conv).await.unwrap();
        assert_eq!(turn2.text.as_deref(), Some("second"));
        assert_eq!(model.remaining(), 0);
    }

    #[tokio::test]
    async fn test_scripted_model_exhausted() {
        let model = ScriptedModel::new(vec![]);
        let conv = model.create_conversation("sys", "hi");

        let result = model.complete(&conv).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exhausted"));
    }

    #[tokio::test]
    async fn test_scripted_model_with_tool_calls() {
        let model = ScriptedModel::new(vec![ModelTurn {
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: json!({"path": "test.txt"}),
            }],
            stop_reason: "tool_calls".to_string(),
            ..Default::default()
        }]);

        let conv = model.create_conversation("sys", "hi");
        let turn = model.complete(&conv).await.unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "read_file");
    }

    #[test]
    fn test_condense_returns_zero() {
        let model = ScriptedModel::new(vec![]);
        let mut conv = model.create_conversation("sys", "hi");
        assert_eq!(model.condense_conversation(&mut conv, 4), 0);
    }

    #[test]
    fn test_create_conversation() {
        let model = ScriptedModel::new(vec![]);
        let conv = model.create_conversation("system prompt", "hello user");
        assert_eq!(conv.provider_messages.len(), 1);
        assert_eq!(conv.system_prompt, "system prompt");
        assert_eq!(
            conv.provider_messages[0]["content"].as_str().unwrap(),
            "hello user"
        );
    }
}
