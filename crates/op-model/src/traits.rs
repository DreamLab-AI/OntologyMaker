use async_trait::async_trait;
use op_core::{Conversation, ModelTurn, OpResult, ToolResult};

/// Callback type for streaming content deltas from the model.
///
/// The first argument is the delta kind (`"text"` or `"thinking"`),
/// the second is the content fragment.
pub type ContentDeltaCb = Box<dyn Fn(&str, &str) + Send + Sync>;

/// Trait implemented by every LLM backend (OpenAI, Anthropic, echo, scripted, etc.).
#[async_trait]
pub trait LlmModel: Send + Sync {
    /// Create a new conversation with the given system prompt and initial user message.
    fn create_conversation(
        &self,
        system_prompt: &str,
        initial_user_message: &str,
    ) -> Conversation;

    /// Send the current conversation to the model and get a response.
    async fn complete(&self, conversation: &Conversation) -> OpResult<ModelTurn>;

    /// Append an assistant turn (from a previous `complete` call) to the conversation.
    fn append_assistant_turn(&self, conversation: &mut Conversation, turn: &ModelTurn);

    /// Append tool results to the conversation so the model can see them on the next call.
    fn append_tool_results(
        &self,
        conversation: &mut Conversation,
        results: &[ToolResult],
    );

    /// Replace old tool result contents with a short placeholder to save context.
    /// Returns the number of messages condensed.
    fn condense_conversation(
        &self,
        conversation: &mut Conversation,
        keep_recent_turns: usize,
    ) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify the trait is object-safe (can be used as dyn LlmModel)
    fn _assert_object_safe(_model: &dyn LlmModel) {}

    #[test]
    fn test_content_delta_cb_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ContentDeltaCb>();
    }
}
