use async_trait::async_trait;
use op_core::{Conversation, ModelTurn, OpResult, ToolResult};
use serde_json::json;

use crate::traits::LlmModel;

/// Fallback model that echoes a static message when no API keys are configured.
///
/// This model never makes tool calls and always returns the same note text.
pub struct EchoFallbackModel {
    pub note: String,
}

impl Default for EchoFallbackModel {
    fn default() -> Self {
        Self {
            note: "No provider API keys configured. Set OpenAI/Anthropic/OpenRouter keys \
                   or use --provider ollama for a local model."
                .to_string(),
        }
    }
}

impl EchoFallbackModel {
    /// Create a new EchoFallbackModel with a custom note.
    pub fn new(note: String) -> Self {
        Self { note }
    }
}

#[async_trait]
impl LlmModel for EchoFallbackModel {
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
        Ok(ModelTurn {
            text: Some(self.note.clone()),
            stop_reason: "end_turn".to_string(),
            ..Default::default()
        })
    }

    fn append_assistant_turn(&self, _conversation: &mut Conversation, _turn: &ModelTurn) {
        // No-op for echo model
    }

    fn append_tool_results(
        &self,
        _conversation: &mut Conversation,
        _results: &[ToolResult],
    ) {
        // No-op for echo model
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

    #[test]
    fn test_default_note() {
        let model = EchoFallbackModel::default();
        assert!(model.note.contains("No provider API keys configured"));
    }

    #[test]
    fn test_custom_note() {
        let model = EchoFallbackModel::new("custom note".to_string());
        assert_eq!(model.note, "custom note");
    }

    #[test]
    fn test_create_conversation() {
        let model = EchoFallbackModel::default();
        let conv = model.create_conversation("system", "hello");
        assert_eq!(conv.provider_messages.len(), 1);
        assert_eq!(conv.system_prompt, "system");
    }

    #[tokio::test]
    async fn test_complete() {
        let model = EchoFallbackModel::new("test note".to_string());
        let conv = model.create_conversation("sys", "hi");
        let turn = model.complete(&conv).await.unwrap();
        assert_eq!(turn.text.as_deref(), Some("test note"));
        assert_eq!(turn.stop_reason, "end_turn");
        assert!(turn.tool_calls.is_empty());
        assert_eq!(turn.input_tokens, 0);
        assert_eq!(turn.output_tokens, 0);
    }

    #[test]
    fn test_condense_returns_zero() {
        let model = EchoFallbackModel::default();
        let mut conv = model.create_conversation("sys", "hi");
        assert_eq!(model.condense_conversation(&mut conv, 4), 0);
    }
}
