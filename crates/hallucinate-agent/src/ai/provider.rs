//! Provider trait for LLM API implementations.
//!
//! Defines the interface that all providers must implement for streaming
//! completions.

use std::pin::Pin;

use futures::Future;

use crate::ai::{
    streaming::AssistantMessageEventStream,
    types::{Context, Model, StreamOptions},
};

/// Result type for provider operations.
pub type ProviderResult<T> = Result<T, ProviderError>;

/// Error type for provider operations.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// API error from the LLM provider with status code.
    #[error("API error: {0} (status {1})")]
    Api(String, u16),

    /// HTTP error from the underlying client.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization/deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Invalid provider configuration.
    #[error("Provider configuration error: {0}")]
    Configuration(String),

    /// Error while reading or processing the response stream.
    #[error("Stream error: {0}")]
    Stream(String),
}

/// Trait that all LLM providers must implement for streaming completions.
/// Providers handle communication with the configured LLM API.
pub trait Provider: Send + Sync {
    /// Stream a completion for the given context.
    ///
    /// Returns a stream of
    /// [`AssistantMessageEvent`](crate::ai::streaming::AssistantMessageEvent)
    /// that will emit events as the model generates content, terminating with
    /// either a `done` or `error` event.
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> Pin<Box<dyn Future<Output = ProviderResult<AssistantMessageEventStream>> + Send + '_>>;
}
