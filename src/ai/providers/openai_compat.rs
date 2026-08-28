//! OpenAI API-compatible provider.
//!
//! Supports any OpenAI-compatible endpoint including:
//! - OpenAI (api.openai.com)
//! - Azure OpenAI
//! - Local servers (Ollama, vLLM, LM Studio, llama.cpp, etc.)
//! - Proxy services (OpenRouter, etc.)

use std::{collections::HashMap, pin::Pin, time::Duration};

use futures::{Future, StreamExt};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};

use crate::ai::{
    provider::{Provider, ProviderError, ProviderResult},
    streaming::{
        AssistantMessageBuilder,
        AssistantMessageEventStream,
        create_assistant_message_event_stream,
    },
    types::{
        AssistantContent,
        Context,
        ImageContent,
        Message,
        Model,
        StopReason,
        StreamOptions,
        ToolResultMessage,
        Usage,
    },
};

/// Configuration for the OpenAI-compatible provider.
#[derive(Debug, Clone)]
pub struct OpenaiCompatConfig {
    /// Base URL for the API (e.g., "https://api.openai.com/v1").
    pub base_url: String,
    /// API key for authentication.
    pub api_key: String,
    /// Optional custom headers.
    pub headers: HashMap<String, String>,
    /// Request timeout in milliseconds.
    pub timeout_ms: u64,
    /// Number of retries for rate limits and temporary server failures.
    pub max_retries: u32,
}

impl Default for OpenaiCompatConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            headers: HashMap::new(),
            timeout_ms: 600_000, // 10 minutes
            max_retries: 3,
        }
    }
}

/// OpenAI API-compatible provider.
pub struct OpenaiCompatProvider {
    config: OpenaiCompatConfig,
    client: reqwest::Client,
}

/// Return whether an HTTP status represents a transient API failure.
fn should_retry_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(500_u64.saturating_mul(1_u64 << attempt.min(4)))
}

fn retry_after_delay(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(60)))
}

fn openai_image_part(image: &ImageContent) -> serde_json::Value {
    serde_json::json!({
        "type": "image_url",
        "image_url": {
            "url": format!("data:{};base64,{}", image.mime_type, image.data)
        }
    })
}

fn tool_result_messages(result: &ToolResultMessage) -> Vec<serde_json::Value> {
    let text = result
        .content
        .iter()
        .map(|item| item.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let tool_content = if text.is_empty() && !result.images.is_empty() {
        "(see attached image)"
    } else if text.is_empty() {
        "(no tool output)"
    } else {
        &text
    };
    let mut messages = vec![serde_json::json!({
        "role": "tool",
        "tool_call_id": result.tool_call_id,
        "content": tool_content
    })];
    if !result.images.is_empty() {
        let mut content = vec![serde_json::json!({
            "type": "text",
            "text": "Attached image(s) from tool result:"
        })];
        content.extend(result.images.iter().map(openai_image_part));
        messages.push(serde_json::json!({
            "role": "user",
            "content": content
        }));
    }
    messages
}

fn parse_usage(chunk: &serde_json::Value) -> Option<Usage> {
    let usage = chunk.get("usage")?;
    Some(Usage {
        input: usage
            .get("prompt_tokens")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        output: usage
            .get("completion_tokens")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        cache_read: usage
            .get("prompt_tokens_details")
            .and_then(|value| value.get("cached_tokens"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: usage
            .get("total_tokens")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        cost: crate::ai::types::Cost::default(),
    })
}

/// Apply an OpenAI chat-completions chunk to the accumulated message.
/// Usage-only chunks are valid and are deliberately handled before choices.
async fn process_chunk(
    chunk: &serde_json::Value,
    builder: &mut AssistantMessageBuilder,
) -> Result<Option<StopReason>, ProviderError> {
    if let Some(usage) = parse_usage(chunk) {
        builder.set_usage(usage);
    }
    let Some(choices) = chunk.get("choices").and_then(|value| value.as_array()) else {
        return Ok(None);
    };

    for choice in choices {
        let Some(delta) = choice.get("delta").and_then(|value| value.as_object()) else {
            // A terminal choice is permitted to omit delta.
            if choice.get("finish_reason").is_none() {
                return Err(ProviderError::Stream(
                    "Missing delta in non-terminal choice".into(),
                ));
            }
            continue;
        };
        if let Some(text) = delta
            .get("content")
            .and_then(|value| value.as_str())
            .filter(|text| !text.is_empty())
        {
            builder.append_text(0, text).await;
        }
        // Several OpenAI-compatible servers use one of these names for
        // reasoning deltas. Treat them as a separate block so reasoning is not
        // accidentally included in the user-visible answer text.
        for field in ["reasoning_content", "reasoning", "thinking"] {
            if let Some(thinking) = delta
                .get(field)
                .and_then(|value| value.as_str())
                .filter(|text| !text.is_empty())
            {
                builder.append_thinking(0, thinking).await;
            }
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|value| value.as_array()) {
            for tool_call in tool_calls {
                let index = tool_call
                    .get("index")
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| ProviderError::Stream("Tool call is missing its index".into()))?
                    as usize;
                let function = tool_call
                    .get("function")
                    .and_then(|value| value.as_object());
                // Continuation chunks are allowed to contain only an argument
                // fragment. An absent function object is therefore different
                // from a malformed function value.
                let invalid_function = tool_call.get("function").is_some() && function.is_none();
                if invalid_function {
                    return Err(ProviderError::Stream(
                        "Tool call function must be an object".into(),
                    ));
                }
                builder
                    .append_tool_call(
                        index,
                        tool_call.get("id").and_then(|value| value.as_str()),
                        function
                            .and_then(|value| value.get("name"))
                            .and_then(|value| value.as_str()),
                        function
                            .and_then(|value| value.get("arguments"))
                            .and_then(|value| value.as_str()),
                    )
                    .await;
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|value| value.as_str()) {
            return match reason {
                "stop" => Ok(Some(StopReason::Stop)),
                "tool_calls" | "function_call" => Ok(Some(StopReason::ToolUse)),
                "length" => Ok(Some(StopReason::Length)),
                other => Err(ProviderError::Stream(format!(
                    "Unsupported finish reason: {other}"
                ))),
            };
        }
    }
    Ok(None)
}

async fn process_sse_line(
    line: &[u8],
    builder: &mut AssistantMessageBuilder,
    terminal_reason: &mut Option<StopReason>,
) -> Result<bool, ProviderError> {
    let line = std::str::from_utf8(line)
        .map_err(|_| ProviderError::Stream("SSE stream contained invalid UTF-8".into()))?;
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    let Some(data) = line.strip_prefix("data:") else {
        // Comments and other SSE fields are not part of the chat-completion
        // payload. They are legal and can be ignored.
        return Ok(false);
    };
    let data = data.trim_start();
    if data == "[DONE]" {
        return Ok(true);
    }
    if data.is_empty() {
        return Ok(false);
    }
    let value = serde_json::from_str::<serde_json::Value>(data)
        .map_err(|error| ProviderError::Stream(format!("Malformed SSE JSON: {error}")))?;
    if let Some(reason) = process_chunk(&value, builder).await? {
        *terminal_reason = Some(reason);
    }
    Ok(false)
}

/// Feed an arbitrary HTTP byte fragment into the newline-delimited SSE
/// decoder. Keeping buffering here makes the production path and fragmented
/// stream tests exercise exactly the same boundary handling.
async fn process_sse_bytes(
    bytes: &[u8],
    buffer: &mut Vec<u8>,
    builder: &mut AssistantMessageBuilder,
    terminal_reason: &mut Option<StopReason>,
    done_seen: &mut bool,
) -> Result<(), ProviderError> {
    if *done_seen {
        return Ok(());
    }
    buffer.extend_from_slice(bytes);
    while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = buffer.drain(..=newline).collect();
        if process_sse_line(&line, builder, terminal_reason).await? {
            *done_seen = true;
            break;
        }
    }
    Ok(())
}

impl OpenaiCompatProvider {
    /// Create a new OpenAI-compatible provider with the given configuration.
    pub fn new(config: OpenaiCompatConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .expect("Failed to create HTTP client");

        Self { config, client }
    }

    /// Build the authorization header value.
    fn auth_header(api_key: &str) -> Result<HeaderValue, ProviderError> {
        HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|error| {
            ProviderError::Configuration(format!("invalid API key header value: {error}"))
        })
    }

    /// Build headers for the request. The per-request key takes precedence over
    /// the provider default so CLI overrides are honored by every mode.
    fn request_headers(
        &self,
        api_key: &str,
    ) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        for (key, value) in &self.config.headers {
            let name = HeaderName::from_bytes(key.as_bytes()).map_err(|error| {
                ProviderError::Configuration(format!("invalid header name {key:?}: {error}"))
            })?;
            let value = HeaderValue::from_str(value).map_err(|error| {
                ProviderError::Configuration(format!("invalid value for header {key:?}: {error}"))
            })?;
            headers.insert(name, value);
        }
        // Authentication and content type are controlled by the provider so a
        // per-request API-key override cannot be shadowed by a custom header.
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(AUTHORIZATION, Self::auth_header(api_key)?);

        Ok(headers)
    }

    /// Convert messages to OpenAI chat format.
    fn convert_messages(
        &self,
        messages: &[Message],
    ) -> Vec<serde_json::Value> {
        messages
            .iter()
            .flat_map(|msg| {
                if let Message::ToolResult(result) = msg {
                    return tool_result_messages(result);
                }
                let message = match msg {
                Message::User(u) => {
                    if u.images.is_empty() {
                        serde_json::json!({
                            "role": "user",
                            "content": u.content
                        })
                    } else {
                        let mut content = vec![serde_json::json!({
                            "type": "text",
                            "text": u.content
                        })];
                        content.extend(u.images.iter().map(|image| {
                            serde_json::json!({
                                "type": "image_url",
                                "image_url": {
                                    "url": format!(
                                        "data:{};base64,{}",
                                        image.mime_type,
                                        image.data
                                    )
                                }
                            })
                        }));
                        serde_json::json!({
                            "role": "user",
                            "content": content
                        })
                    }
                }
                Message::Assistant(a) => {
                    let mut text_parts: Vec<String> = Vec::new();
                    let mut has_tool_calls = false;
                    for block in &a.content {
                        match block {
                            AssistantContent::Text(t) => {
                                if !t.text.is_empty() {
                                    text_parts.push(t.text.clone());
                                }
                            }
                            AssistantContent::ToolCall(_tc) => {
                                has_tool_calls = true;
                            }
                            AssistantContent::Thinking(tc) => {
                                text_parts.push(format!("<thinking>\n{}\n</thinking>", tc.thinking));
                            }
                        }
                    }

                    if has_tool_calls {
                        // Build tool_calls array
                        let tool_calls: Vec<serde_json::Value> = a
                            .content
                            .iter()
                            .filter_map(|block| {
                                if let AssistantContent::ToolCall(tc) = block {
                                    Some(serde_json::json!({
                                        "id": tc.id,
                                        "type": "function",
                                        "function": {
                                            "name": tc.name,
                                            "arguments": tc.arguments.to_string()
                                        }
                                    }))
                                } else {
                                    None
                                }
                            })
                            .collect();

                        serde_json::json!({
                            "role": "assistant",
                            "content": if text_parts.is_empty() { None } else { Some(text_parts.join("\n")) },
                            "tool_calls": tool_calls
                        })
                    } else if text_parts.is_empty() {
                        serde_json::json!({
                            "role": "assistant",
                            "content": ""
                        })
                    } else {
                        serde_json::json!({
                            "role": "assistant",
                            "content": text_parts.join("\n")
                        })
                    }
                }
                Message::ToolResult(_) => unreachable!("tool results return above")
                };
                vec![message]
            })
            .collect()
    }

    /// Convert tools to OpenAI function format.
    fn convert_tools(
        &self,
        tools: &[crate::ai::types::Tool],
    ) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters
                    }
                })
            })
            .collect()
    }

    /// Build the chat completions request body.
    fn build_request_body(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> serde_json::Value {
        let mut body = serde_json::Map::new();

        // Model
        body.insert("model".into(), model.id.clone().into());

        // Messages
        let mut messages = Vec::new();

        // System prompt
        if let Some(system_prompt) = &context.system_prompt {
            messages.push(serde_json::json!({
                "role": "system",
                "content": system_prompt
            }));
        }

        // User messages
        messages.extend(self.convert_messages(&context.messages));

        body.insert("messages".into(), messages.into());

        // Tools
        if let Some(tools) = &context.tools
            && !tools.is_empty()
        {
            body.insert("tools".into(), self.convert_tools(tools).into());
            if let Some(tool_choice) = &options.tool_choice {
                match tool_choice {
                    crate::ai::types::ToolChoice::Auto => {
                        body.insert("tool_choice".into(), "auto".into());
                    },
                    crate::ai::types::ToolChoice::None => {
                        body.insert("tool_choice".into(), "none".into());
                    },
                }
            }
        }

        // Sampling options
        if let Some(temp) = options.temperature {
            body.insert("temperature".into(), temp.into());
        }
        if let Some(max_tokens) = options.max_tokens {
            body.insert("max_tokens".into(), max_tokens.into());
        }

        // Streaming
        body.insert("stream".into(), true.into());

        serde_json::Value::Object(body)
    }
}

impl Provider for OpenaiCompatProvider {
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> Pin<Box<dyn Future<Output = ProviderResult<AssistantMessageEventStream>> + Send + '_>>
    {
        let base_url = self.config.base_url.clone();
        let api_key = options
            .api_key
            .as_deref()
            .unwrap_or(&self.config.api_key)
            .to_owned();
        let body = self.build_request_body(model, context, options);
        let model_id = model.id.clone();
        let model_api = model.api.clone();
        let model_provider = model.provider.clone();
        let client = self.client.clone();
        let max_retries = self.config.max_retries;

        Box::pin(async move {
            let headers = self.request_headers(&api_key)?;
            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let mut attempt = 0;
            let response = loop {
                match client
                    .post(&url)
                    .headers(headers.clone())
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => break response,
                    Ok(response) => {
                        let status = response.status().as_u16();
                        let retry_after = retry_after_delay(&response);
                        let error_text = response.text().await.unwrap_or_default();
                        if should_retry_status(status) && attempt < max_retries {
                            let delay = retry_after.unwrap_or_else(|| retry_delay(attempt));
                            tracing::warn!(
                                attempt = attempt + 1,
                                max_retries,
                                status,
                                delay_ms = delay.as_millis(),
                                "retrying transient API failure"
                            );
                            tokio::time::sleep(delay).await;
                            attempt += 1;
                        } else {
                            return Err(ProviderError::Api(error_text, status));
                        }
                    },
                    Err(error) if is_retryable_transport_error(&error) && attempt < max_retries => {
                        let delay = retry_delay(attempt);
                        tracing::warn!(attempt = attempt + 1, max_retries, error = %error, delay_ms = delay.as_millis(), "retrying transient transport failure");
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                    },
                    Err(error) => return Err(ProviderError::Http(error)),
                }
            };

            let stream = create_assistant_message_event_stream();
            let producer_stream = stream.clone();
            tokio::spawn(async move {
                let mut builder =
                    AssistantMessageBuilder::new(producer_stream, model_api, model_id);
                // The shared builder uses a generic provider label. Preserve
                // the configured provider in persisted assistant messages so
                // model switching remains visible in session history.
                builder.message.provider = model_provider;
                builder.emit_start().await;
                let mut reader = response.bytes_stream();
                let mut buffer = Vec::new();
                let mut terminal_reason = None;
                let mut failure = None;

                let mut done_seen = false;

                while !done_seen {
                    let Some(chunk) = reader.next().await else {
                        break;
                    };
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            failure = Some(format!("SSE transport error: {error}"));
                            break;
                        },
                    };
                    if let Err(error) = process_sse_bytes(
                        &chunk,
                        &mut buffer,
                        &mut builder,
                        &mut terminal_reason,
                        &mut done_seen,
                    )
                    .await
                    {
                        failure = Some(error.to_string());
                        break;
                    }
                    if failure.is_some() {
                        break;
                    }
                }

                // A final SSE field may not be newline-terminated. Process it
                // so a valid response is not lost, while malformed trailing
                // bytes still become a structured stream error.
                if failure.is_none()
                    && !done_seen
                    && !buffer.is_empty()
                    && let Err(error) =
                        process_sse_line(&buffer, &mut builder, &mut terminal_reason).await
                {
                    failure = Some(error.to_string());
                }

                match failure {
                    Some(error) => builder.fail(error).await,
                    None => match terminal_reason {
                        Some(reason) => {
                            if let Err(error) = builder.finish(reason).await {
                                builder.fail(error).await;
                            }
                        },
                        None => {
                            builder
                                .fail("SSE stream ended without a terminal completion chunk")
                                .await
                        },
                    },
                }
            });
            Ok(stream)
        })
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;
    use crate::ai::{
        streaming::AssistantMessageEvent as AssistantMessageEventStreamEvent,
        types::{ImageContent, UserMessage},
    };

    #[test]
    fn provider_headers_cannot_override_authentication() {
        let mut headers = HashMap::new();
        headers.insert("authorization".to_string(), "Bearer wrong".to_string());
        headers.insert("content-type".to_string(), "text/plain".to_string());
        let provider = OpenaiCompatProvider::new(OpenaiCompatConfig {
            api_key: "default".to_string(),
            headers,
            ..Default::default()
        });
        let request_headers = provider.request_headers("override").unwrap();
        assert_eq!(
            request_headers
                .get(AUTHORIZATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer override"
        );
        assert_eq!(
            request_headers.get(CONTENT_TYPE).unwrap().to_str().unwrap(),
            "application/json"
        );
    }

    #[test]
    fn invalid_provider_headers_return_configuration_errors() {
        let mut headers = HashMap::new();
        headers.insert("bad header".to_string(), "value".to_string());
        let provider = OpenaiCompatProvider::new(OpenaiCompatConfig {
            headers,
            ..Default::default()
        });
        assert!(matches!(
            provider.request_headers("key"),
            Err(ProviderError::Configuration(_))
        ));
    }

    #[test]
    fn retries_only_transient_status_codes() {
        assert!(should_retry_status(429));
        assert!(should_retry_status(503));
        assert!(!should_retry_status(400));
        assert!(!should_retry_status(401));
    }

    #[test]
    fn retry_backoff_is_bounded_and_increasing() {
        assert_eq!(retry_delay(0), Duration::from_millis(500));
        assert_eq!(retry_delay(3), Duration::from_secs(4));
        assert_eq!(retry_delay(100), Duration::from_secs(8));
    }

    #[test]
    fn encodes_user_images_as_openai_image_urls() {
        let provider = OpenaiCompatProvider::new(OpenaiCompatConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        });
        let messages = provider.convert_messages(&[Message::User(UserMessage::with_images(
            "Describe this",
            vec![ImageContent::new("aGVsbG8=", "image/png")],
        ))]);
        assert_eq!(messages[0]["content"][0]["type"], "text");
        assert_eq!(messages[0]["content"][1]["type"], "image_url");
        assert_eq!(
            messages[0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
    }

    #[test]
    fn emits_tool_images_as_a_following_user_message() {
        let provider = OpenaiCompatProvider::new(OpenaiCompatConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        });
        let mut result = ToolResultMessage::new("call-1", "read", "Read image", false);
        result
            .images
            .push(ImageContent::new("aGVsbG8=", "image/png"));
        let messages = provider.convert_messages(&[Message::ToolResult(result)]);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["content"], "Read image");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(
            messages[1]["content"][1]["image_url"]["url"],
            "data:image/png;base64,aGVsbG8="
        );
    }

    #[tokio::test]
    async fn streamed_sse_accumulates_text_and_usage() {
        let mut server = mockito::Server::new_async().await;
        let chunks = [
            serde_json::json!({"choices":[{"delta":{"content":"hel"},"finish_reason":null}]}),
            serde_json::json!({"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}),
            serde_json::json!({"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
        ];
        let body = chunks
            .into_iter()
            .map(|chunk| format!("data: {chunk}\n\n"))
            .collect::<String>()
            + "data: [DONE]\n\n";
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let provider = OpenaiCompatProvider::new(OpenaiCompatConfig {
            base_url: server.url(),
            api_key: "test".to_string(),
            ..Default::default()
        });
        let model = Model::new(
            "test",
            "test",
            crate::ai::types::Api::OpenaiCompletions,
            "test",
            server.url(),
            false,
            4096,
            100,
        );
        let mut stream = provider
            .stream(&model, &Context::new(Vec::new()), &StreamOptions::new())
            .await
            .unwrap();
        let events: Vec<_> = stream.by_ref().collect().await;
        let message = events
            .into_iter()
            .find_map(|event| match event {
                AssistantMessageEventStreamEvent::Done { message, .. } => Some(message),
                _ => None,
            })
            .unwrap();
        assert_eq!(message.content[0].text_content(), Some("hello"));
        assert_eq!(message.usage.total_tokens, 5);
    }

    #[tokio::test]
    async fn fragmented_http_bytes_are_reassembled_as_sse_lines() {
        let stream = crate::ai::streaming::create_assistant_message_event_stream();
        let mut builder = AssistantMessageBuilder::new(
            stream.clone(),
            crate::ai::types::Api::OpenaiCompletions,
            "test",
        );
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({
                "choices": [{ "delta": { "content": "hello" }, "finish_reason": null }]
            }),
            serde_json::json!({
                "choices": [{ "delta": {}, "finish_reason": "stop" }]
            })
        );
        let mut buffer = Vec::new();
        let mut terminal_reason = None;
        let mut done_seen = false;
        for fragment in body.as_bytes().chunks(3) {
            process_sse_bytes(
                fragment,
                &mut buffer,
                &mut builder,
                &mut terminal_reason,
                &mut done_seen,
            )
            .await
            .unwrap();
        }
        assert!(done_seen);
        assert_eq!(terminal_reason, Some(StopReason::Stop));
        builder.finish(StopReason::Stop).await.unwrap();
        let events: Vec<_> = stream.collect().await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AssistantMessageEventStreamEvent::Done { message, .. }
                    if message.content[0].text_content() == Some("hello")
            )
        }));
    }

    #[tokio::test]
    async fn malformed_sse_and_tool_chunks_are_reported() {
        let stream = crate::ai::streaming::create_assistant_message_event_stream();
        let mut builder =
            AssistantMessageBuilder::new(stream, crate::ai::types::Api::OpenaiCompletions, "test");
        let mut terminal_reason = None;
        let error = process_sse_line(b"data: {not-json}\n", &mut builder, &mut terminal_reason)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Malformed SSE JSON"));

        let error = process_chunk(
            &serde_json::json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{ "id": "call-1", "function": { "arguments": "{}" } }]
                    }
                }]
            }),
            &mut builder,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("missing its index"));
    }

    #[tokio::test]
    async fn done_without_a_terminal_completion_is_a_stream_error() {
        let stream = crate::ai::streaming::create_assistant_message_event_stream();
        let mut builder = AssistantMessageBuilder::new(
            stream.clone(),
            crate::ai::types::Api::OpenaiCompletions,
            "test",
        );
        let mut terminal_reason = None;
        assert!(
            process_sse_line(b"data: [DONE]\n", &mut builder, &mut terminal_reason)
                .await
                .unwrap()
        );
        assert!(terminal_reason.is_none());
        builder
            .fail("SSE stream ended without a terminal completion chunk")
            .await;
        let events: Vec<_> = stream.collect().await;
        assert!(events.iter().any(|event| {
            matches!(event, AssistantMessageEventStreamEvent::Error { error, .. }
                if error.error_message.as_deref() == Some("SSE stream ended without a terminal completion chunk"))
        }));
    }

    #[tokio::test]
    async fn accepts_a_final_non_newline_terminated_done_marker() {
        let mut server = mockito::Server::new_async().await;
        let body = format!(
            "data: {}\n\ndata: [DONE]",
            serde_json::json!({
                "choices": [{ "delta": { "content": "complete" }, "finish_reason": "stop" }]
            })
        );
        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let provider = OpenaiCompatProvider::new(OpenaiCompatConfig {
            base_url: server.url(),
            api_key: "test".to_string(),
            ..Default::default()
        });
        let model = Model::new(
            "test",
            "test",
            crate::ai::types::Api::OpenaiCompletions,
            "test",
            server.url(),
            false,
            4096,
            100,
        );
        let mut stream = provider
            .stream(&model, &Context::default(), &StreamOptions::new())
            .await
            .unwrap();
        let events: Vec<_> = stream.by_ref().collect().await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AssistantMessageEventStreamEvent::Done { .. }))
        );
    }

    #[tokio::test]
    async fn streamed_reasoning_is_kept_in_a_thinking_block() {
        let stream = crate::ai::streaming::create_assistant_message_event_stream();
        let mut builder = AssistantMessageBuilder::new(
            stream.clone(),
            crate::ai::types::Api::OpenaiCompletions,
            "test",
        );
        process_chunk(
            &serde_json::json!({
                "choices": [{ "delta": { "reasoning_content": "considering" }, "finish_reason": null }]
            }),
            &mut builder
        )
        .await
        .unwrap();
        process_chunk(
            &serde_json::json!({
                "choices": [{ "delta": { "content": "answer" }, "finish_reason": "stop" }]
            }),
            &mut builder,
        )
        .await
        .unwrap();
        builder.finish(StopReason::Stop).await.unwrap();
        let mut stream = stream;
        let events: Vec<_> = stream.by_ref().collect().await;
        let message = events
            .into_iter()
            .find_map(|event| match event {
                AssistantMessageEventStreamEvent::Done { message, .. } => Some(message),
                _ => None,
            })
            .unwrap();
        assert!(message.content.iter().any(|content| matches!(
            content,
            AssistantContent::Thinking(thinking) if thinking.thinking == "considering"
        )));
    }

    #[tokio::test]
    async fn streamed_tool_calls_are_completed_with_json_arguments() {
        let stream = crate::ai::streaming::create_assistant_message_event_stream();
        let mut builder = AssistantMessageBuilder::new(
            stream.clone(),
            crate::ai::types::Api::OpenaiCompletions,
            "test",
        );
        process_chunk(&serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"first","arguments":r#"{"a":"#}},{"index":1,"id":"call_b","function":{"name":"second","arguments":r#"{"b":"#}}]},"finish_reason":null}]}), &mut builder).await.unwrap();
        let finish = process_chunk(&serde_json::json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}},{"index":1,"function":{"arguments":"2}"}}]},"finish_reason":"tool_calls"}]}), &mut builder).await.unwrap();
        builder.finish(finish.unwrap()).await.unwrap();
        let mut stream = stream;
        let events: Vec<_> = stream.by_ref().collect().await;
        let tools: Vec<_> = events
            .into_iter()
            .filter_map(|event| match event {
                AssistantMessageEventStreamEvent::ToolcallEnd { tool_call, .. } => Some(tool_call),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].arguments, serde_json::json!({"a": 1}));
        assert_eq!(tools[1].arguments, serde_json::json!({"b": 2}));
    }
}
