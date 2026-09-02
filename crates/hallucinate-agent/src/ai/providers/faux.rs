//! Faux (fake) provider for testing.
//!
//! Simulates LLM responses without making real API calls. Useful for
//! unit testing and development.

use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};

use futures::Future;

use crate::ai::{
    provider::{Provider, ProviderResult},
    streaming::{AssistantMessageEvent, create_assistant_message_event_stream},
    types::{
        Api, AssistantContent, AssistantMessage, Context, Model, StopReason, StreamOptions,
        TextContent, ToolCall,
    },
};

/// A faux response that can be queued for the provider.
#[derive(Debug, Clone)]
pub enum FauxResponse {
    /// A text response.
    Text(String, StopReason),
    /// A tool call response.
    ToolCall(String, String, serde_json::Value, StopReason),
    /// An error response.
    Error(String),
}

/// State for the faux provider.
#[derive(Debug, Default)]
pub struct FauxProviderState {
    /// Number of times the provider has been called.
    pub call_count: std::sync::atomic::AtomicU32,
    /// Queued responses to return in order.
    pub responses: Arc<Mutex<Vec<FauxResponse>>>,
}

/// Faux provider for testing.
pub struct FauxProvider {
    /// The model configured for this provider.
    configured_model: Model,
    /// Shared state holding call count and response queue.
    state: FauxProviderState,
}

impl FauxProvider {
    /// Create a new faux provider with default settings.
    pub fn new() -> Self {
        Self::with_model(Model::new(
            "faux-1",
            "Faux Model",
            Api::OpenaiCompletions,
            "faux",
            "http://localhost:0",
            false,
            128000,
            16384,
        ))
    }

    /// Create a faux provider with a specific model.
    pub fn with_model(model: Model) -> Self {
        Self {
            configured_model: model,
            state: FauxProviderState::default(),
        }
    }

    /// Set the response queue.
    pub fn set_responses(
        &self,
        responses: Vec<FauxResponse>,
    ) {
        let mut state = self.state.responses.lock().unwrap();
        *state = responses;
    }

    /// Get the number of pending responses.
    pub fn pending_response_count(&self) -> usize {
        self.state.responses.lock().unwrap().len()
    }

    /// Get the call count.
    pub fn call_count(&self) -> u32 {
        self.state
            .call_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for FauxProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for FauxProvider {
    fn stream(
        &self,
        model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> Pin<
        Box<
            dyn Future<Output = ProviderResult<crate::ai::streaming::AssistantMessageEventStream>>
                + Send
                + '_,
        >,
    > {
        let responses = self.state.responses.clone();
        let call_count = &self.state.call_count;

        let response = {
            let mut r = responses.lock().unwrap();
            call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            r.pop()
        };

        let model_api = model.api.clone();
        let model_id = if model.id.is_empty() {
            self.configured_model.id.clone()
        } else {
            model.id.clone()
        };

        Box::pin(async move {
            let stream = create_assistant_message_event_stream();

            // Push all events immediately (synchronously) before returning
            let _ = stream.push(AssistantMessageEvent::Start).await;

            match response {
                Some(FauxResponse::Text(text, stop_reason)) => {
                    let content = vec![AssistantContent::Text(TextContent::new(text.clone()))];
                    let stop = stop_reason.clone();

                    let _ = stream
                        .push(AssistantMessageEvent::TextStart { content_index: 0 })
                        .await;

                    let _ = stream
                        .push(AssistantMessageEvent::TextDelta {
                            content_index: 0,
                            delta: text.clone(),
                        })
                        .await;

                    let _ = stream
                        .push(AssistantMessageEvent::TextEnd {
                            content_index: 0,
                            content: text,
                        })
                        .await;

                    let _ = stream
                        .push(AssistantMessageEvent::Done {
                            reason: stop,
                            message: AssistantMessage::new(
                                content.clone(),
                                model_api.clone(),
                                "faux",
                                &model_id,
                                stop_reason.clone(),
                            ),
                        })
                        .await;

                    stream
                        .final_message
                        .lock()
                        .unwrap()
                        .replace(AssistantMessage::new(
                            content,
                            model_api,
                            "faux",
                            &model_id,
                            stop_reason,
                        ));
                },
                Some(FauxResponse::ToolCall(id, name, args, stop_reason)) => {
                    let tool_call = ToolCall::new(id, name, args);
                    let content = vec![AssistantContent::ToolCall(tool_call.clone())];
                    let stop = stop_reason.clone();

                    let _ = stream
                        .push(AssistantMessageEvent::ToolcallStart { content_index: 0 })
                        .await;

                    let _ = stream
                        .push(AssistantMessageEvent::ToolcallEnd {
                            content_index: 0,
                            tool_call,
                        })
                        .await;

                    let _ = stream
                        .push(AssistantMessageEvent::Done {
                            reason: stop,
                            message: AssistantMessage::new(
                                content.clone(),
                                model_api.clone(),
                                "faux",
                                &model_id,
                                stop_reason.clone(),
                            ),
                        })
                        .await;

                    stream
                        .final_message
                        .lock()
                        .unwrap()
                        .replace(AssistantMessage::new(
                            content,
                            model_api,
                            "faux",
                            &model_id,
                            stop_reason,
                        ));
                },

                Some(FauxResponse::Error(msg)) => {
                    let error_content = Vec::new();
                    let error_msg = AssistantMessage {
                        content: error_content.clone(),
                        api: model_api.clone(),
                        provider: "faux".to_string(),
                        model: model_id.clone(),
                        response_model: None,
                        response_id: None,
                        usage: crate::ai::types::Usage::default(),
                        stop_reason: StopReason::Error,
                        error_message: Some(msg),
                        raw_stop_reason: None,
                        end_turn: None,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };

                    let _ = stream
                        .push(AssistantMessageEvent::Error {
                            reason: StopReason::Error,
                            error: error_msg.clone(),
                        })
                        .await;

                    stream.final_message.lock().unwrap().replace(error_msg);
                },
                None => {
                    let error_content = Vec::new();
                    let error_msg = AssistantMessage {
                        content: error_content.clone(),
                        api: model_api.clone(),
                        provider: "faux".to_string(),
                        model: model_id.clone(),
                        response_model: None,
                        response_id: None,
                        usage: crate::ai::types::Usage::default(),
                        stop_reason: StopReason::Error,
                        error_message: Some("No more faux responses queued".to_string()),
                        raw_stop_reason: None,
                        end_turn: None,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };

                    let _ = stream
                        .push(AssistantMessageEvent::Error {
                            reason: StopReason::Error,
                            error: error_msg.clone(),
                        })
                        .await;

                    stream.final_message.lock().unwrap().replace(error_msg);
                },
            }

            Ok(stream)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_faux_text_response() {
        let provider = FauxProvider::new();

        provider.set_responses(vec![FauxResponse::Text(
            "Hello, world!".to_string(),
            StopReason::Stop,
        )]);

        let context = Context::new(vec![crate::ai::types::Message::User(
            crate::ai::types::UserMessage::new("Hi"),
        )]);

        let stream = provider
            .stream(&provider.configured_model, &context, &StreamOptions::new())
            .await
            .unwrap();

        // Check that final message was set
        let final_msg = stream.final_message.lock().unwrap();
        assert!(final_msg.is_some());

        let msg = final_msg.as_ref().unwrap();

        assert_eq!(msg.stop_reason, StopReason::Stop);
        assert!(!msg.content.is_empty());
    }

    #[tokio::test]
    async fn test_faux_tool_call_response() {
        let provider = FauxProvider::new();

        provider.set_responses(vec![FauxResponse::ToolCall(
            "call_123".to_string(),
            "bash".to_string(),
            serde_json::json!({"command": "echo hello"}),
            StopReason::ToolUse,
        )]);

        let context = Context::new(vec![crate::ai::types::Message::User(
            crate::ai::types::UserMessage::new("Run echo hello"),
        )]);

        let stream = provider
            .stream(&provider.configured_model, &context, &StreamOptions::new())
            .await
            .unwrap();

        let final_msg = stream.final_message.lock().unwrap();
        assert!(final_msg.is_some());

        let msg = final_msg.as_ref().unwrap();
        assert_eq!(msg.stop_reason, StopReason::ToolUse);
    }

    #[tokio::test]
    async fn test_faux_error_response() {
        let provider = FauxProvider::new();
        provider.set_responses(vec![FauxResponse::Error(
            "Something went wrong".to_string(),
        )]);

        let context = Context::default();

        let stream = provider
            .stream(&provider.configured_model, &context, &StreamOptions::new())
            .await
            .unwrap();

        let final_msg = stream.final_message.lock().unwrap();
        assert!(final_msg.is_some());

        let msg = final_msg.as_ref().unwrap();
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.is_some());
    }

    #[tokio::test]
    async fn test_faux_empty_queue() {
        let provider = FauxProvider::new();
        // No responses set

        let context = Context::default();

        let stream = provider
            .stream(&provider.configured_model, &context, &StreamOptions::new())
            .await
            .unwrap();

        let final_msg = stream.final_message.lock().unwrap();
        assert!(final_msg.is_some());

        let msg = final_msg.as_ref().unwrap();
        assert_eq!(msg.stop_reason, StopReason::Error);
    }

    #[test]
    fn test_faux_provider_creation() {
        let provider = FauxProvider::new();
        assert_eq!(provider.call_count(), 0);
        assert_eq!(provider.pending_response_count(), 0);
    }
}
