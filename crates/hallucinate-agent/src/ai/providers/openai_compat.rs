//! OpenAI API-compatible provider.
//!
//! Supports both the Chat Completions and Responses wire formats. Any
//! OpenAI-compatible endpoint can be used, including:
//! - OpenAI (api.openai.com)
//! - Azure OpenAI
//! - Local servers (Ollama, vLLM, LM Studio, llama.cpp, etc.)
//! - Proxy services (OpenRouter, etc.)

use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    time::Duration,
};

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
        Api,
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
    /// Return the default configuration with OpenAI's base URL and sensible
    /// defaults.
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
    /// Provider configuration (base URL, API key, timeout, etc.).
    config: OpenaiCompatConfig,
    /// Validated custom and content-type headers shared by all requests.
    ///
    /// Header validation happens once at provider construction. Authorization
    /// remains per request because command-line options may override the key.
    static_headers: Result<HeaderMap, String>,
    /// HTTP client for making API requests.
    client: reqwest::Client,
}

/// Check whether an HTTP status code indicates a transient failure worth
/// retrying.
fn should_retry_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

/// Check whether a reqwest error is caused by a timeout or connection failure.
fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

/// Compute an exponential-backoff delay with jitter, capped at 8 seconds.
fn retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(500_u64.saturating_mul(1_u64 << attempt.min(4)))
}

/// Parse the `Retry-After` header and return a delay capped at 60 seconds.
fn retry_after_delay(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(60)))
}

/// Convert an image content item into an OpenAI `image_url` JSON object.
fn openai_image_part(image: &ImageContent) -> serde_json::Value {
    serde_json::json!({
        "type": "image_url",
        "image_url": {
            "url": format!("data:{};base64,{}", image.mime_type, image.data)
        }
    })
}

/// Split a combined tool-call ID into its call ID and optional item ID parts.
fn split_response_tool_call_id(id: &str) -> (&str, Option<&str>) {
    id.split_once('|')
        .map_or((id, None), |(call_id, item_id)| (call_id, Some(item_id)))
}

/// Convert a tool result message into OpenAI chat-format JSON messages.
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
        "tool_call_id": split_response_tool_call_id(&result.tool_call_id).0,
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

/// Extract token usage from a chat-completions SSE chunk.
fn parse_usage(chunk: &serde_json::Value) -> Option<Usage> {
    let usage = chunk.get("usage").or_else(|| {
        chunk
            .get("response")
            .and_then(|response| response.get("usage"))
    })?;

    Some(Usage {
        input: usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        output: usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        cache_read: usage
            .get("prompt_tokens_details")
            .or_else(|| usage.get("input_tokens_details"))
            .and_then(|value| value.get("cached_tokens"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        cache_write: 0,
        cache_write_1h: None,
        reasoning: usage
            .get("completion_tokens_details")
            .or_else(|| usage.get("output_tokens_details"))
            .and_then(|value| value.get("reasoning_tokens"))
            .and_then(|value| value.as_u64()),
        total_tokens: usage
            .get("total_tokens")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        cost: crate::ai::types::Cost::default(),
    })
}

/// Extract token usage from a Responses API SSE chunk, subtracting cached
/// tokens from input.
fn parse_responses_usage(chunk: &serde_json::Value) -> Option<Usage> {
    let mut usage = parse_usage(chunk)?;
    usage.input = usage.input.saturating_sub(usage.cache_read);
    Some(usage)
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

/// Streaming state for the OpenAI Responses API SSE decoder.
#[derive(Default)]
struct ResponsesStreamState {
    /// Maps item IDs to their tool-call index for deduplication.
    tool_indices: HashMap<String, usize>,
    /// Maps tool-call index to the OpenAI call ID.
    tool_call_ids: HashMap<usize, String>,
    /// Maps tool-call index to the OpenAI item ID.
    tool_item_ids: HashMap<usize, String>,
    /// Next available tool-call index when deduplication doesn't match.
    next_tool_index: usize,
    /// Indices of tool calls whose arguments have already been emitted.
    emitted_tool_arguments: HashSet<usize>,
    /// Whether the response has produced any tool call output.
    has_tool_calls: bool,
    /// Whether the response has produced any text output.
    has_text_output: bool,
    /// Whether the response has produced any reasoning/thinking output.
    has_thinking_output: bool,
}

impl ResponsesStreamState {
    /// Resolve the tool-call index for a function call, deduplicating by item
    /// ID.
    fn tool_index(
        &mut self,
        output_index: Option<usize>,
        item_id: Option<&str>,
    ) -> usize {
        if let Some(item_id) = item_id
            && let Some(index) = self.tool_indices.get(item_id)
        {
            return *index;
        }

        let index = output_index.unwrap_or_else(|| {
            let index = self.next_tool_index;
            self.next_tool_index = self.next_tool_index.saturating_add(1);
            index
        });
        self.next_tool_index = self.next_tool_index.max(index.saturating_add(1));

        if let Some(item_id) = item_id.filter(|item_id| !item_id.is_empty()) {
            self.tool_indices.insert(item_id.to_string(), index);
        }

        index
    }

    /// Register the call ID and item ID for a tool-call index and return the
    /// combined ID.
    fn set_tool_identity(
        &mut self,
        index: usize,
        item_id: Option<&str>,
        call_id: Option<&str>,
    ) -> Option<String> {
        if let Some(item_id) = item_id.filter(|item_id| !item_id.is_empty()) {
            self.tool_item_ids.insert(index, item_id.to_string());
        }
        if let Some(call_id) = call_id.filter(|call_id| !call_id.is_empty()) {
            self.tool_call_ids.insert(index, call_id.to_string());
        }

        match (
            self.tool_call_ids.get(&index),
            self.tool_item_ids.get(&index),
        ) {
            (Some(call_id), Some(item_id)) => Some(format!("{call_id}|{item_id}")),
            (Some(call_id), None) => Some(call_id.clone()),
            (None, Some(item_id)) => Some(item_id.clone()),
            (None, None) => None,
        }
    }
}

/// Result of processing a single SSE event in the Responses API stream.
#[derive(Debug, PartialEq, Eq)]
enum ResponsesSseResult {
    Continue,
    Done,
    Failed(String),
}

/// Extract a numeric index field from a JSON value.
fn response_index(
    value: &serde_json::Value,
    field: &str,
) -> Option<usize> {
    value
        .get(field)
        .and_then(|value| value.as_u64())
        .map(|value| value as usize)
}

/// Compute a flat content index from output and content indices.
fn response_content_index(
    output_index: Option<usize>,
    content_index: Option<usize>,
) -> usize {
    match (output_index, content_index) {
        (Some(output_index), Some(content_index)) => output_index
            .saturating_mul(1_000_000)
            .saturating_add(content_index),
        (Some(output_index), None) => output_index,
        (None, Some(content_index)) => content_index,
        (None, None) => 0,
    }
}

/// Compute the flat content index for an SSE event.
fn event_content_index(event: &serde_json::Value) -> usize {
    response_content_index(
        response_index(event, "output_index"),
        response_index(event, "content_index"),
    )
}

/// Return the `response` object if present, otherwise the value itself.
fn response_object(value: &serde_json::Value) -> &serde_json::Value {
    match value.get("response") {
        Some(response) if response.is_object() => response,
        _ => value,
    }
}

/// Apply response-level metadata (id, model) from an SSE event to the builder.
fn apply_response_metadata(
    event: &serde_json::Value,
    builder: &mut AssistantMessageBuilder,
) {
    let response = response_object(event);

    if let Some(id) = response.get("id").and_then(|value| value.as_str()) {
        builder.message.response_id = Some(id.to_string());
    }

    if let Some(model) = response.get("model").and_then(|value| value.as_str()) {
        builder.message.response_model = Some(model.to_string());
    }
}

/// Extract a human-readable error message from a Responses API error event.
fn response_stream_error(event: &serde_json::Value) -> String {
    let response = response_object(event);
    let error = event.get("error").or_else(|| response.get("error"));

    let message = error
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .or_else(|| error.and_then(|error| error.as_str()))
        .unwrap_or("unknown Responses API error");

    format!("OpenAI Responses API stream error: {message}")
}

/// Process a Responses API output item (function call or message) and append to
/// the builder.
async fn process_response_output_item(
    item: &serde_json::Value,
    output_index: Option<usize>,
    builder: &mut AssistantMessageBuilder,
    state: &mut ResponsesStreamState,
) {
    let Some(item_type) = item.get("type").and_then(|value| value.as_str()) else {
        return;
    };

    if item_type == "function_call" {
        let item_id = item.get("id").and_then(|value| value.as_str());
        let index = state.tool_index(output_index, item_id);
        let call_id = item.get("call_id").and_then(|value| value.as_str());
        let tool_call_id = state.set_tool_identity(index, item_id, call_id);
        let name = item.get("name").and_then(|value| value.as_str());

        state.has_tool_calls = true;
        builder
            .append_tool_call(index, tool_call_id.as_deref(), name, None)
            .await;

        if let Some(arguments) = item
            .get("arguments")
            .and_then(|value| value.as_str())
            .filter(|arguments| !arguments.is_empty())
            && !state.emitted_tool_arguments.contains(&index)
        {
            builder
                .append_tool_call(index, None, None, Some(arguments))
                .await;
            state.emitted_tool_arguments.insert(index);
        }

        return;
    }

    if item_type == "message" && !state.has_text_output {
        let Some(content) = item.get("content").and_then(|value| value.as_array()) else {
            return;
        };

        for (content_index, part) in content.iter().enumerate() {
            let part_type = part.get("type").and_then(|value| value.as_str());

            let Some(text) = part
                .get("text")
                .or_else(|| part.get("refusal"))
                .and_then(|value| value.as_str())
                .filter(|text| !text.is_empty())
            else {
                continue;
            };

            if matches!(
                part_type,
                Some("output_text") | Some("text") | Some("refusal")
            ) {
                state.has_text_output = true;
                builder
                    .append_text(
                        response_content_index(output_index, Some(content_index)),
                        text,
                    )
                    .await;
            }
        }
    }
}

/// Process a single Responses API SSE event and update the builder state.
async fn process_responses_event(
    event: &serde_json::Value,
    builder: &mut AssistantMessageBuilder,
    state: &mut ResponsesStreamState,
    terminal_reason: &mut Option<StopReason>,
) -> Result<ResponsesSseResult, ProviderError> {
    apply_response_metadata(event, builder);

    if let Some(usage) = parse_responses_usage(event) {
        builder.set_usage(usage);
    }

    let Some(event_type) = event.get("type").and_then(|value| value.as_str()) else {
        return Ok(ResponsesSseResult::Continue);
    };

    match event_type {
        "response.output_text.delta" => {
            if let Some(delta) = event
                .get("delta")
                .and_then(|value| value.as_str())
                .filter(|delta| !delta.is_empty())
            {
                state.has_text_output = true;
                builder.append_text(event_content_index(event), delta).await;
            }
        },
        "response.output_text.done" => {
            if !state.has_text_output
                && let Some(text) = event
                    .get("text")
                    .and_then(|value| value.as_str())
                    .filter(|text| !text.is_empty())
            {
                state.has_text_output = true;
                builder.append_text(event_content_index(event), text).await;
            }
        },
        "response.reasoning_summary_text.delta"
        | "response.reasoning_text.delta"
        | "response.reasoning_summary.delta" => {
            if let Some(delta) = event
                .get("delta")
                .and_then(|value| value.as_str())
                .filter(|delta| !delta.is_empty())
            {
                state.has_thinking_output = true;
                builder
                    .append_thinking(event_content_index(event), delta)
                    .await;
            }
        },
        "response.reasoning_summary_text.done" => {
            if !state.has_thinking_output
                && let Some(text) = event
                    .get("text")
                    .and_then(|value| value.as_str())
                    .filter(|text| !text.is_empty())
            {
                state.has_thinking_output = true;
                builder
                    .append_thinking(event_content_index(event), text)
                    .await;
            }
        },
        "response.refusal.delta" => {
            if let Some(delta) = event
                .get("delta")
                .and_then(|value| value.as_str())
                .filter(|delta| !delta.is_empty())
            {
                state.has_text_output = true;
                builder.append_text(event_content_index(event), delta).await;
            }
        },
        "response.output_item.added" | "response.output_item.done" => {
            if let Some(item) = event.get("item") {
                process_response_output_item(
                    item,
                    response_index(event, "output_index"),
                    builder,
                    state,
                )
                .await;
            }
        },
        "response.function_call_arguments.delta" => {
            let item_id = event.get("item_id").and_then(|value| value.as_str());
            let index = state.tool_index(response_index(event, "output_index"), item_id);
            let call_id = event.get("call_id").and_then(|value| value.as_str());
            let name = event.get("name").and_then(|value| value.as_str());
            let tool_call_id = state.set_tool_identity(index, item_id, call_id);

            state.has_tool_calls = true;
            builder
                .append_tool_call(index, tool_call_id.as_deref(), name, None)
                .await;

            if let Some(delta) = event
                .get("delta")
                .and_then(|value| value.as_str())
                .filter(|delta| !delta.is_empty())
            {
                builder
                    .append_tool_call(index, None, None, Some(delta))
                    .await;
                state.emitted_tool_arguments.insert(index);
            }
        },
        "response.function_call_arguments.done" => {
            let item_id = event.get("item_id").and_then(|value| value.as_str());
            let index = state.tool_index(response_index(event, "output_index"), item_id);
            let call_id = event.get("call_id").and_then(|value| value.as_str());
            let name = event.get("name").and_then(|value| value.as_str());
            let tool_call_id = state.set_tool_identity(index, item_id, call_id);

            state.has_tool_calls = true;
            builder
                .append_tool_call(index, tool_call_id.as_deref(), name, None)
                .await;

            if let Some(arguments) = event
                .get("arguments")
                .and_then(|value| value.as_str())
                .filter(|arguments| !arguments.is_empty())
                && !state.emitted_tool_arguments.contains(&index)
            {
                builder
                    .append_tool_call(index, None, None, Some(arguments))
                    .await;
                state.emitted_tool_arguments.insert(index);
            }
        },
        "response.completed" => {
            // Normally all output items have already arrived through their
            // item events. Processing the completed response as a fallback
            // also supports compatible servers that omit those events.
            let response = response_object(event);
            if let Some(output) = response.get("output").and_then(|value| value.as_array()) {
                for (output_index, item) in output.iter().enumerate() {
                    process_response_output_item(item, Some(output_index), builder, state).await;
                }
            }

            match response.get("status").and_then(|value| value.as_str()) {
                Some("incomplete") => *terminal_reason = Some(StopReason::Length),
                Some("failed") | Some("cancelled") => {
                    return Ok(ResponsesSseResult::Failed(response_stream_error(event)));
                },
                _ => {
                    *terminal_reason = Some(if state.has_tool_calls {
                        StopReason::ToolUse
                    } else {
                        StopReason::Stop
                    });
                },
            }
            return Ok(ResponsesSseResult::Done);
        },
        "response.incomplete" => {
            *terminal_reason = Some(StopReason::Length);
            return Ok(ResponsesSseResult::Done);
        },
        "response.failed" | "response.error" | "error" => {
            return Ok(ResponsesSseResult::Failed(response_stream_error(event)));
        },
        _ => {},
    }

    Ok(ResponsesSseResult::Continue)
}

/// Parse and process a single SSE line from the Responses API stream.
async fn process_responses_sse_line(
    line: &[u8],
    builder: &mut AssistantMessageBuilder,
    state: &mut ResponsesStreamState,
    terminal_reason: &mut Option<StopReason>,
) -> Result<ResponsesSseResult, ProviderError> {
    let line = std::str::from_utf8(line)
        .map_err(|_| ProviderError::Stream("SSE stream contained invalid UTF-8".into()))?;
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);

    let Some(data) = line.strip_prefix("data:") else {
        // `event:` and SSE comments carry no JSON payload. The event type is
        // also present in each Responses API data object, so these fields are
        // not needed by the decoder.
        return Ok(ResponsesSseResult::Continue);
    };

    let data = data.trim_start();
    if data == "[DONE]" || data.is_empty() {
        return Ok(if data == "[DONE]" {
            ResponsesSseResult::Done
        } else {
            ResponsesSseResult::Continue
        });
    }

    let value = serde_json::from_str::<serde_json::Value>(data)
        .map_err(|error| ProviderError::Stream(format!("Malformed SSE JSON: {error}")))?;

    process_responses_event(&value, builder, state, terminal_reason).await
}

/// Conservative limit for a single unframed SSE record.
///
/// SSE records contain JSON metadata and deltas, not arbitrary tool output, so
/// allowing a peer to retain more than one MiB without a newline is unsafe.
const MAX_SSE_RECORD_BYTES: usize = 1_048_576;

/// Cursor-based line buffer for fragmented SSE records.
///
/// Consumed records advance a cursor instead of draining the unread tail. The
/// tail is compacted only before the next network append, avoiding a copy and
/// allocation for every SSE line.
#[derive(Default)]
struct SseLineBuffer {
    bytes: Vec<u8>,
    cursor: usize,
    record_count: u64,
    max_buffered_bytes: usize,
}

impl SseLineBuffer {
    fn extend(
        &mut self,
        bytes: &[u8],
    ) {
        if self.cursor == self.bytes.len() {
            self.bytes.clear();
            self.cursor = 0;
        } else if self.cursor > 0 {
            self.bytes.copy_within(self.cursor.., 0);
            self.bytes.truncate(self.bytes.len() - self.cursor);
            self.cursor = 0;
        }

        self.bytes.extend_from_slice(bytes);
        self.max_buffered_bytes = self.max_buffered_bytes.max(self.bytes.len());
    }

    fn next_line(&mut self) -> Result<Option<&[u8]>, ProviderError> {
        let unread = &self.bytes[self.cursor..];
        let Some(newline) = unread.iter().position(|byte| *byte == b'\n') else {
            if unread.len() > MAX_SSE_RECORD_BYTES {
                return Err(ProviderError::Stream(format!(
                    "SSE record exceeded the {} byte limit",
                    MAX_SSE_RECORD_BYTES
                )));
            }
            return Ok(None);
        };

        if newline > MAX_SSE_RECORD_BYTES {
            return Err(ProviderError::Stream(format!(
                "SSE record exceeded the {} byte limit",
                MAX_SSE_RECORD_BYTES
            )));
        }

        let start = self.cursor;
        self.cursor += newline + 1;
        self.record_count += 1;

        Ok(Some(&self.bytes[start..self.cursor]))
    }

    fn remaining(&self) -> &[u8] {
        &self.bytes[self.cursor..]
    }

    fn ensure_remaining_within_limit(&self) -> Result<(), ProviderError> {
        if self.remaining().len() > MAX_SSE_RECORD_BYTES {
            return Err(ProviderError::Stream(format!(
                "SSE record exceeded the {} byte limit",
                MAX_SSE_RECORD_BYTES
            )));
        }

        Ok(())
    }
}

/// Feed byte fragments into the Responses API SSE decoder with buffering.
async fn process_responses_sse_bytes(
    bytes: &[u8],
    buffer: &mut SseLineBuffer,
    builder: &mut AssistantMessageBuilder,
    state: &mut ResponsesStreamState,
    terminal_reason: &mut Option<StopReason>,
) -> Result<ResponsesSseResult, ProviderError> {
    buffer.extend(bytes);

    while let Some(line) = buffer.next_line()? {
        let result = process_responses_sse_line(line, builder, state, terminal_reason).await?;

        if result != ResponsesSseResult::Continue {
            return Ok(result);
        }
    }

    Ok(ResponsesSseResult::Continue)
}

/// Parse and process a single SSE line from a chat-completions stream.
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
    buffer: &mut SseLineBuffer,
    builder: &mut AssistantMessageBuilder,
    terminal_reason: &mut Option<StopReason>,
    done_seen: &mut bool,
) -> Result<(), ProviderError> {
    if *done_seen {
        return Ok(());
    }

    buffer.extend(bytes);

    while let Some(line) = buffer.next_line()? {
        if process_sse_line(line, builder, terminal_reason).await? {
            *done_seen = true;
            break;
        }
    }

    Ok(())
}

/// Consume a chat-completions SSE stream and return the terminal reason and any
/// failure.
async fn consume_chat_stream(
    response: reqwest::Response,
    builder: &mut AssistantMessageBuilder,
    cancellation: Option<&crate::ai::types::RequestCancellation>,
) -> (Option<StopReason>, Option<String>) {
    let mut reader = response.bytes_stream();
    let mut buffer = SseLineBuffer::default();
    let mut bytes_received = 0_u64;
    let mut terminal_reason = None;
    let mut failure = None;
    let mut done_seen = false;

    while !done_seen {
        let chunk = match tokio::select! {
            chunk = reader.next() => chunk,
            _ = wait_for_cancellation(cancellation) => {
                return (None, Some("provider request was cancelled".to_string()));
            },
        } {
            Some(chunk) => chunk,
            None => break,
        };

        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                failure = Some(format!("SSE transport error: {error}"));
                break;
            },
        };
        bytes_received += chunk.len() as u64;

        if let Err(error) = process_sse_bytes(
            &chunk,
            &mut buffer,
            builder,
            &mut terminal_reason,
            &mut done_seen,
        )
        .await
        {
            failure = Some(error.to_string());
            break;
        }
    }

    // A final SSE field may not be newline-terminated. Process it so a valid
    // response is not lost, while malformed trailing bytes still become a
    // structured stream error.
    if failure.is_none()
        && !done_seen
        && !buffer.remaining().is_empty()
        && let Err(error) = buffer.ensure_remaining_within_limit()
    {
        failure = Some(error.to_string());
    }

    if failure.is_none()
        && !done_seen
        && !buffer.remaining().is_empty()
        && let Err(error) =
            process_sse_line(buffer.remaining(), builder, &mut terminal_reason).await
    {
        failure = Some(error.to_string());
    }

    if failure.is_none() && terminal_reason.is_none() {
        failure = Some("SSE stream ended without a terminal completion chunk".to_string());
    }

    tracing::info!(
        sse_event_count = buffer.record_count,
        bytes_received,
        max_buffered_sse_bytes = buffer.max_buffered_bytes,
        "consumed chat-completions SSE stream"
    );

    (terminal_reason, failure)
}

async fn wait_for_cancellation(cancellation: Option<&crate::ai::types::RequestCancellation>) {
    match cancellation {
        Some(cancellation) => cancellation.cancelled().await,
        None => std::future::pending().await,
    }
}

/// Consume a Responses API SSE stream and return the terminal reason and any
/// failure.
async fn consume_responses_stream(
    response: reqwest::Response,
    builder: &mut AssistantMessageBuilder,
    cancellation: Option<&crate::ai::types::RequestCancellation>,
) -> (Option<StopReason>, Option<String>) {
    let mut reader = response.bytes_stream();
    let mut buffer = SseLineBuffer::default();
    let mut bytes_received = 0_u64;
    let mut state = ResponsesStreamState::default();
    let mut terminal_reason = None;
    let mut failure = None;
    let mut done_seen = false;

    while !done_seen {
        let chunk = match tokio::select! {
            chunk = reader.next() => chunk,
            _ = wait_for_cancellation(cancellation) => {
                return (None, Some("provider request was cancelled".to_string()));
            },
        } {
            Some(chunk) => chunk,
            None => break,
        };

        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                failure = Some(format!("SSE transport error: {error}"));
                break;
            },
        };

        bytes_received += chunk.len() as u64;

        match process_responses_sse_bytes(
            &chunk,
            &mut buffer,
            builder,
            &mut state,
            &mut terminal_reason,
        )
        .await
        {
            Ok(ResponsesSseResult::Continue) => {},
            Ok(ResponsesSseResult::Done) => done_seen = true,
            Ok(ResponsesSseResult::Failed(error)) => {
                failure = Some(error);
                done_seen = true;
            },
            Err(error) => {
                failure = Some(error.to_string());
                break;
            },
        }
    }

    if failure.is_none()
        && !done_seen
        && !buffer.remaining().is_empty()
        && let Err(error) = buffer.ensure_remaining_within_limit()
    {
        failure = Some(error.to_string());
    }

    if failure.is_none() && !done_seen && !buffer.remaining().is_empty() {
        match process_responses_sse_line(
            buffer.remaining(),
            builder,
            &mut state,
            &mut terminal_reason,
        )
        .await
        {
            Ok(ResponsesSseResult::Continue | ResponsesSseResult::Done) => {},
            Ok(ResponsesSseResult::Failed(error)) => failure = Some(error),
            Err(error) => failure = Some(error.to_string()),
        }
    }

    if failure.is_none() && terminal_reason.is_none() {
        failure = Some("SSE stream ended without a terminal Responses API event".to_string());
    }

    tracing::info!(
        sse_event_count = buffer.record_count,
        bytes_received,
        max_buffered_sse_bytes = buffer.max_buffered_bytes,
        "consumed Responses API SSE stream"
    );

    (terminal_reason, failure)
}

impl OpenaiCompatProvider {
    /// Create a new OpenAI-compatible provider with the given configuration.
    pub fn new(config: OpenaiCompatConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .expect("Failed to create HTTP client");

        let static_headers = Self::build_static_headers(&config);

        Self {
            config,
            static_headers,
            client,
        }
    }

    /// Validate headers that are independent of the request API key.
    fn build_static_headers(config: &OpenaiCompatConfig) -> Result<HeaderMap, String> {
        let mut headers = HeaderMap::new();

        for (key, value) in &config.headers {
            let name = HeaderName::from_bytes(key.as_bytes())
                .map_err(|error| format!("invalid header name {key:?}: {error}"))?;
            let value = HeaderValue::from_str(value)
                .map_err(|error| format!("invalid value for header {key:?}: {error}"))?;
            headers.insert(name, value);
        }
        // Authentication and content type are controlled by the provider so a
        // custom header cannot shadow the per-request credentials.
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        Ok(headers)
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
        let mut headers = self
            .static_headers
            .as_ref()
            .map_err(|error| ProviderError::Configuration(error.clone()))?
            .clone();
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
                },
                Message::Assistant(a) => {
                    let mut text_parts: Vec<String> = Vec::new();
                    let mut has_tool_calls = false;

                    for block in &a.content {
                        match block {
                            AssistantContent::Text(t) => {
                                if !t.text.is_empty() {
                                    text_parts.push(t.text.clone());
                                }
                            },
                            AssistantContent::ToolCall(_tc) => {
                                has_tool_calls = true;
                            },
                            AssistantContent::Thinking(tc) => {
                                text_parts.push(format!("<thinking>\n{}\n</thinking>", tc.thinking));
                            },
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
                                        "id": split_response_tool_call_id(&tc.id).0,
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
                },
                Message::ToolResult(_) => unreachable!("tool results return above")
                };

                vec![message]
            })
            .collect()
    }

    /// Convert messages to the OpenAI Responses input format.
    fn convert_responses_input(
        &self,
        model: &Model,
        messages: &[Message],
    ) -> Vec<serde_json::Value> {
        let mut input = Vec::new();

        for (message_index, message) in messages.iter().enumerate() {
            match message {
                Message::User(user) => {
                    let mut content = vec![serde_json::json!({
                        "type": "input_text",
                        "text": user.content
                    })];

                    content.extend(user.images.iter().map(|image| {
                        serde_json::json!({
                            "type": "input_image",
                            "detail": "auto",
                            "image_url": format!(
                                "data:{};base64,{}",
                                image.mime_type,
                                image.data
                            )
                        })
                    }));

                    input.push(serde_json::json!({
                        "role": "user",
                        "content": content
                    }));
                },
                Message::Assistant(assistant) => {
                    for content in &assistant.content {
                        match content {
                            AssistantContent::Thinking(thinking) => {
                                if let Some(signature) = &thinking.thinking_signature
                                    && let Ok(reasoning) = serde_json::from_str(signature)
                                {
                                    input.push(reasoning);
                                }
                            },
                            AssistantContent::Text(text) if !text.text.is_empty() => {
                                input.push(serde_json::json!({
                                    "type": "message",
                                    "role": "assistant",
                                    "content": [{
                                        "type": "output_text",
                                        "text": text.text,
                                        "annotations": []
                                    }],
                                    "status": "completed",
                                    "id": format!("msg_{message_index}")
                                }));
                            },
                            AssistantContent::ToolCall(tool_call) => {
                                let (call_id, item_id) = split_response_tool_call_id(&tool_call.id);
                                let mut function_call = serde_json::json!({
                                    "type": "function_call",
                                    "call_id": call_id,
                                    "name": tool_call.name,
                                    "arguments": tool_call.arguments.to_string()
                                });
                                if let Some(item_id) = item_id {
                                    function_call["id"] = serde_json::json!(item_id);
                                }
                                input.push(function_call);
                            },
                            AssistantContent::Text(_) => {},
                        }
                    }
                },
                Message::ToolResult(result) => {
                    let text = result
                        .content
                        .iter()
                        .map(|item| item.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let (call_id, _) = split_response_tool_call_id(&result.tool_call_id);
                    let output = if result.images.is_empty()
                        || !model.input.iter().any(|input| input == "image")
                    {
                        serde_json::json!(if text.is_empty() {
                            if result.images.is_empty() {
                                "(no tool output)"
                            } else {
                                "(see attached image)"
                            }
                        } else {
                            &text
                        })
                    } else {
                        let mut content = if text.is_empty() {
                            Vec::new()
                        } else {
                            vec![serde_json::json!({
                                "type": "input_text",
                                "text": text
                            })]
                        };
                        content.extend(result.images.iter().map(|image| {
                            serde_json::json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!(
                                    "data:{};base64,{}",
                                    image.mime_type,
                                    image.data
                                )
                            })
                        }));
                        serde_json::Value::Array(content)
                    };

                    input.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output
                    }));
                },
            }
        }

        input
    }

    /// Convert tools to OpenAI Chat Completions function format.
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

    /// Build the Chat Completions request body.
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

        if let Some(include_usage) = options.include_usage {
            let mut stream_opts = serde_json::Map::new();
            stream_opts.insert("include_usage".into(), serde_json::json!(include_usage));

            body.insert("stream_options".into(), stream_opts.into());
        }

        serde_json::Value::Object(body)
    }

    /// Build a non-streaming Chat Completions request body.
    fn build_completion_request_body(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> serde_json::Value {
        let mut body = self.build_request_body(model, context, options);
        body["stream"] = serde_json::json!(false);
        body
    }

    /// Convert tools to the flat function format used by the Responses API.
    fn convert_responses_tools(
        &self,
        tools: &[crate::ai::types::Tool],
    ) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                    "strict": false
                })
            })
            .collect()
    }

    /// Build the OpenAI Responses request body.
    fn build_responses_request_body(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> serde_json::Value {
        let mut body = serde_json::Map::new();

        body.insert("model".into(), model.id.clone().into());

        let mut input = Vec::new();
        if let Some(system_prompt) = &context.system_prompt {
            let role = if model.reasoning {
                "developer"
            } else {
                "system"
            };
            input.push(serde_json::json!({
                "role": role,
                "content": system_prompt
            }));
        }
        input.extend(self.convert_responses_input(model, &context.messages));
        body.insert("input".into(), input.into());

        if let Some(tools) = &context.tools
            && !tools.is_empty()
        {
            body.insert("tools".into(), self.convert_responses_tools(tools).into());

            if let Some(tool_choice) = &options.tool_choice {
                let value = match tool_choice {
                    crate::ai::types::ToolChoice::Auto => "auto",
                    crate::ai::types::ToolChoice::None => "none",
                };
                body.insert("tool_choice".into(), value.into());
            }
        }

        if let Some(temp) = options.temperature {
            body.insert("temperature".into(), temp.into());
        }
        if let Some(max_tokens) = options.max_tokens {
            body.insert("max_output_tokens".into(), max_tokens.into());
        }

        body.insert("stream".into(), true.into());
        body.insert("store".into(), false.into());

        serde_json::Value::Object(body)
    }
}

/// Convert a non-streaming Chat Completions response into an assistant message.
fn parse_completion_response(
    response: &serde_json::Value,
    model: &Model,
) -> Result<crate::ai::types::AssistantMessage, ProviderError> {
    let choice = response
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| ProviderError::Stream("completion response contained no choices".into()))?;
    let message = choice
        .get("message")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| ProviderError::Stream("completion choice contained no message".into()))?;

    let mut content = Vec::new();
    for field in ["reasoning_content", "reasoning", "thinking"] {
        if let Some(thinking) = message
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
        {
            content.push(AssistantContent::Thinking(
                crate::ai::types::ThinkingContent::new(thinking),
            ));
        }
    }
    if let Some(text) = message
        .get("content")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
    {
        content.push(AssistantContent::Text(crate::ai::types::TextContent::new(
            text,
        )));
    }
    if let Some(tool_calls) = message
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        for tool_call in tool_calls {
            let id = tool_call
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| ProviderError::Stream("tool call is missing its ID".into()))?;
            let function = tool_call
                .get("function")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| ProviderError::Stream("tool call is missing its function".into()))?;
            let name = function
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| ProviderError::Stream("tool call is missing its name".into()))?;
            let arguments = function
                .get("arguments")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    ProviderError::Stream("tool call is missing its arguments".into())
                })?;
            let arguments = serde_json::from_str(arguments).map_err(|error| {
                ProviderError::Stream(format!("tool call arguments are invalid JSON: {error}"))
            })?;

            content.push(AssistantContent::ToolCall(crate::ai::types::ToolCall::new(
                id, name, arguments,
            )));
        }
    }

    let stop_reason = match choice
        .get("finish_reason")
        .and_then(serde_json::Value::as_str)
    {
        Some("stop") | None => StopReason::Stop,
        Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
        Some("length") => StopReason::Length,
        Some(other) => {
            return Err(ProviderError::Stream(format!(
                "unsupported completion finish reason: {other}"
            )));
        },
    };
    let mut message = crate::ai::types::AssistantMessage::new(
        content,
        Api::OpenaiCompletions,
        &model.provider,
        &model.id,
        stop_reason,
    );
    message.response_id = response
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    message.response_model = response
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    if let Some(usage) = parse_usage(response) {
        message.usage = usage;
    }

    Ok(message)
}

impl Provider for OpenaiCompatProvider {
    /// Create a non-streaming Chat Completions response for a one-shot task.
    fn complete(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> Pin<Box<dyn Future<Output = ProviderResult<crate::ai::types::AssistantMessage>> + Send + '_>>
    {
        let base_url = self.config.base_url.clone();
        let api_key = options
            .api_key
            .as_deref()
            .unwrap_or(&self.config.api_key)
            .to_owned();
        let body = self.build_completion_request_body(model, context, options);
        let client = self.client.clone();
        let max_retries = self.config.max_retries;
        let cancellation = options.cancellation.clone();
        let model = model.clone();

        Box::pin(async move {
            let headers = self.request_headers(&api_key)?;
            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
            let mut attempt = 0;

            let response = loop {
                match tokio::select! {
                    response = client.post(&url).headers(headers.clone()).json(&body).send() => response,
                    _ = wait_for_cancellation(cancellation.as_ref()) => {
                        return Err(ProviderError::Stream("provider request was cancelled".to_string()));
                    },
                } {
                    Ok(response) if response.status().is_success() => break response,
                    Ok(response) => {
                        let status = response.status().as_u16();
                        let retry_after = retry_after_delay(&response);
                        let error_text = response.text().await.unwrap_or_default();
                        if should_retry_status(status) && attempt < max_retries {
                            let delay = retry_after.unwrap_or_else(|| retry_delay(attempt));
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {},
                                _ = wait_for_cancellation(cancellation.as_ref()) => {
                                    return Err(ProviderError::Stream("provider request was cancelled".to_string()));
                                },
                            }
                            attempt += 1;
                        } else {
                            return Err(ProviderError::Api(error_text, status));
                        }
                    },
                    Err(error) if is_retryable_transport_error(&error) && attempt < max_retries => {
                        let delay = retry_delay(attempt);
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {},
                            _ = wait_for_cancellation(cancellation.as_ref()) => {
                                return Err(ProviderError::Stream("provider request was cancelled".to_string()));
                            },
                        }
                        attempt += 1;
                    },
                    Err(error) => return Err(ProviderError::Http(error)),
                }
            };

            let completion = tokio::select! {
                completion = response.json::<serde_json::Value>() => completion?,
                _ = wait_for_cancellation(cancellation.as_ref()) => {
                    return Err(ProviderError::Stream("provider request was cancelled".to_string()));
                },
            };
            parse_completion_response(&completion, &model)
        })
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> Pin<Box<dyn Future<Output = ProviderResult<AssistantMessageEventStream>> + Send + '_>>
    {
        let request_build_started_at = std::time::Instant::now();
        let base_url = self.config.base_url.clone();
        let api_key = options
            .api_key
            .as_deref()
            .unwrap_or(&self.config.api_key)
            .to_owned();
        let (endpoint, body) = match &model.api {
            Api::OpenaiCompletions => (
                "chat/completions",
                self.build_request_body(model, context, options),
            ),
            Api::OpenaiResponses => (
                "responses",
                self.build_responses_request_body(model, context, options),
            ),
        };
        tracing::debug!(
            elapsed_ms = request_build_started_at.elapsed().as_millis(),
            message_count = context.messages.len(),
            tool_count = context.tools.as_ref().map_or(0, |tools| tools.len()),
            "built OpenAI-compatible request JSON"
        );

        let model_id = model.id.clone();
        let model_api = model.api.clone();
        let model_provider = model.provider.clone();
        let client = self.client.clone();
        let max_retries = self.config.max_retries;
        let cancellation = options.cancellation.clone();

        Box::pin(async move {
            let headers = self.request_headers(&api_key)?;
            let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
            let mut attempt = 0;

            let response = loop {
                match tokio::select! {
                    response = client
                        .post(&url)
                        .headers(headers.clone())
                        .json(&body)
                        .send() => response,
                    _ = wait_for_cancellation(cancellation.as_ref()) => {
                        return Err(ProviderError::Stream("provider request was cancelled".to_string()));
                    },
                } {
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

                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {},
                                _ = wait_for_cancellation(cancellation.as_ref()) => {
                                    return Err(ProviderError::Stream("provider request was cancelled".to_string()));
                                },
                            }
                            attempt += 1;
                        } else {
                            return Err(ProviderError::Api(error_text, status));
                        }
                    },
                    Err(error) if is_retryable_transport_error(&error) && attempt < max_retries => {
                        let delay = retry_delay(attempt);

                        tracing::warn!(attempt = attempt + 1, max_retries, error = %error, delay_ms = delay.as_millis(), "retrying transient transport failure");

                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {},
                            _ = wait_for_cancellation(cancellation.as_ref()) => {
                                return Err(ProviderError::Stream("provider request was cancelled".to_string()));
                            },
                        }
                        attempt += 1;
                    },
                    Err(error) => return Err(ProviderError::Http(error)),
                }
            };

            tracing::debug!(retry_count = attempt, "OpenAI-compatible request accepted");

            let stream = create_assistant_message_event_stream();
            let producer_stream = stream.clone();
            tokio::spawn(async move {
                let mut builder =
                    AssistantMessageBuilder::new(producer_stream, model_api.clone(), model_id);

                // The shared builder uses a generic provider label. Preserve
                // the configured provider in persisted assistant messages so
                // model switching remains visible in session history.
                builder.message.provider = model_provider;
                builder.emit_start().await;

                let (terminal_reason, failure) = match model_api {
                    Api::OpenaiCompletions => {
                        consume_chat_stream(response, &mut builder, cancellation.as_ref()).await
                    },
                    Api::OpenaiResponses => {
                        consume_responses_stream(response, &mut builder, cancellation.as_ref())
                            .await
                    },
                };

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
                                .fail("SSE stream ended without a terminal completion event")
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
    async fn non_streaming_completion_returns_a_complete_message() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "model": "test",
                "stream": false,
            })))
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "id": "chatcmpl_1",
                    "model": "test",
                    "choices": [{
                        "message": { "role": "assistant", "content": "one shot" },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 3,
                        "completion_tokens": 2,
                        "total_tokens": 5
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let provider = OpenaiCompatProvider::new(OpenaiCompatConfig {
            base_url: server.url(),
            api_key: "test".to_string(),
            ..Default::default()
        });
        let model = Model::new(
            "test",
            "Test",
            Api::OpenaiCompletions,
            "test-provider",
            server.url(),
            false,
            4096,
            100,
        );

        let message = provider
            .complete(&model, &Context::default(), &StreamOptions::new())
            .await
            .unwrap();

        assert_eq!(message.content[0].text_content(), Some("one shot"));
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.provider, "test-provider");
        assert_eq!(message.response_id.as_deref(), Some("chatcmpl_1"));
        assert_eq!(message.usage.total_tokens, 5);
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

        let mut buffer = SseLineBuffer::default();
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

    #[test]
    fn builds_responses_request_body() {
        let provider = OpenaiCompatProvider::new(OpenaiCompatConfig {
            api_key: "test-key".to_string(),
            ..Default::default()
        });
        let context = Context::new(vec![Message::User(UserMessage::new("Hello"))])
            .with_system_prompt("Be concise")
            .with_tools(vec![crate::ai::types::Tool::new(
                "lookup",
                "Look something up",
                serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } }
                }),
            )]);
        let mut options = StreamOptions::new()
            .with_temperature(0.2)
            .with_max_tokens(42);
        options.tool_choice = Some(crate::ai::types::ToolChoice::Auto);

        let body = provider.build_responses_request_body(
            &Model::new(
                "gpt-5",
                "GPT-5",
                Api::OpenaiResponses,
                "openai",
                "http://localhost/v1",
                true,
                128_000,
                16_384,
            ),
            &context,
            &options,
        );

        assert_eq!(body["model"], "gpt-5");
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body["input"][0]["content"], "Be concise");
        assert_eq!(body["input"][1]["role"], "user");
        assert_eq!(body["input"][1]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][1]["content"][0]["text"], "Hello");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["max_output_tokens"], 42);
        assert_eq!(body["stream"], true);
    }

    #[tokio::test]
    async fn streamed_responses_accumulate_text_and_usage() {
        let mut server = mockito::Server::new_async().await;
        let body = [
            serde_json::json!({
                "type": "response.created",
                "response": { "id": "resp_1", "model": "gpt-5" }
            }),
            serde_json::json!({
                "type": "response.output_text.delta",
                "output_index": 0,
                "content_index": 0,
                "delta": "Hello"
            }),
            serde_json::json!({
                "type": "response.output_text.delta",
                "output_index": 0,
                "content_index": 0,
                "delta": " world"
            }),
            serde_json::json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "model": "gpt-5",
                    "usage": {
                        "input_tokens": 3,
                        "output_tokens": 2,
                        "input_tokens_details": { "cached_tokens": 1 },
                        "output_tokens_details": { "reasoning_tokens": 0 },
                        "total_tokens": 5
                    }
                }
            }),
        ]
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();

        let _mock = server
            .mock("POST", "/responses")
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
            "gpt-5",
            "GPT-5",
            Api::OpenaiResponses,
            "openai",
            server.url(),
            true,
            128_000,
            16_384,
        );

        let mut stream = provider
            .stream(&model, &Context::default(), &StreamOptions::new())
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

        assert_eq!(message.content[0].text_content(), Some("Hello world"));
        assert_eq!(message.api, Api::OpenaiResponses);
        assert_eq!(message.response_id.as_deref(), Some("resp_1"));
        assert_eq!(message.response_model.as_deref(), Some("gpt-5"));
        assert_eq!(message.usage.input, 2);
        assert_eq!(message.usage.output, 2);
        assert_eq!(message.usage.cache_read, 1);
        assert_eq!(message.usage.total_tokens, 5);
    }

    #[tokio::test]
    async fn streamed_responses_accumulate_function_call_arguments() {
        let stream = crate::ai::streaming::create_assistant_message_event_stream();
        let mut builder =
            AssistantMessageBuilder::new(stream.clone(), Api::OpenaiResponses, "gpt-5");
        let mut state = ResponsesStreamState::default();
        let mut terminal_reason = None;

        for event in [
            serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": "fc_item_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": ""
                }
            }),
            serde_json::json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 0,
                "item_id": "fc_item_1",
                "delta": r#"{"query":"#
            }),
            serde_json::json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 0,
                "item_id": "fc_item_1",
                "delta": r#""rust"}"#
            }),
            serde_json::json!({
                "type": "response.completed",
                "response": { "output": [] }
            }),
        ] {
            assert_eq!(
                process_responses_event(&event, &mut builder, &mut state, &mut terminal_reason,)
                    .await
                    .unwrap(),
                if event["type"] == "response.completed" {
                    ResponsesSseResult::Done
                } else {
                    ResponsesSseResult::Continue
                }
            );
        }

        builder
            .finish(terminal_reason.clone().unwrap())
            .await
            .unwrap();
        let events: Vec<_> = stream.collect().await;
        let tool_call = events
            .into_iter()
            .find_map(|event| match event {
                AssistantMessageEventStreamEvent::ToolcallEnd { tool_call, .. } => Some(tool_call),
                _ => None,
            })
            .unwrap();

        assert_eq!(tool_call.id, "call_1|fc_item_1");
        assert_eq!(tool_call.name, "lookup");
        assert_eq!(tool_call.arguments, serde_json::json!({"query": "rust"}));
        assert_eq!(terminal_reason, Some(StopReason::ToolUse));
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

    #[tokio::test]
    async fn responses_sse_accepts_fragmented_crlf_and_final_record() {
        let stream = crate::ai::streaming::create_assistant_message_event_stream();
        let mut builder = AssistantMessageBuilder::new(stream, Api::OpenaiResponses, "test");
        let mut state = ResponsesStreamState::default();
        let mut terminal_reason = None;
        let mut buffer = SseLineBuffer::default();
        let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\r\n\r\ndata: {\"type\":\"response.completed\",\"response\":{}}".to_string();

        for fragment in body.as_bytes().chunks(5) {
            assert_eq!(
                process_responses_sse_bytes(
                    fragment,
                    &mut buffer,
                    &mut builder,
                    &mut state,
                    &mut terminal_reason,
                )
                .await
                .unwrap(),
                ResponsesSseResult::Continue
            );
        }

        assert_eq!(
            process_responses_sse_line(
                buffer.remaining(),
                &mut builder,
                &mut state,
                &mut terminal_reason,
            )
            .await
            .unwrap(),
            ResponsesSseResult::Done
        );
        builder.finish(terminal_reason.unwrap()).await.unwrap();
    }

    #[tokio::test]
    async fn oversized_unframed_records_fail_for_both_sse_formats() {
        let oversized = vec![b'x'; MAX_SSE_RECORD_BYTES + 1];
        let mut chat_buffer = SseLineBuffer::default();
        let mut responses_buffer = SseLineBuffer::default();
        let chat_stream = crate::ai::streaming::create_assistant_message_event_stream();
        let responses_stream = crate::ai::streaming::create_assistant_message_event_stream();
        let mut chat_builder =
            AssistantMessageBuilder::new(chat_stream, Api::OpenaiCompletions, "test");
        let mut responses_builder =
            AssistantMessageBuilder::new(responses_stream, Api::OpenaiResponses, "test");
        let mut terminal_reason = None;
        let mut responses_state = ResponsesStreamState::default();
        let mut done_seen = false;

        assert!(
            process_sse_bytes(
                &oversized,
                &mut chat_buffer,
                &mut chat_builder,
                &mut terminal_reason,
                &mut done_seen,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeded")
        );
        assert!(
            process_responses_sse_bytes(
                &oversized,
                &mut responses_buffer,
                &mut responses_builder,
                &mut responses_state,
                &mut terminal_reason,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeded")
        );
    }
}
