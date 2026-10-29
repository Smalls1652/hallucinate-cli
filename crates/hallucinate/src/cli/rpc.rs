//! RPC mode: newline-delimited JSON-RPC 2.0 over stdin/stdout.
//!
//! `chat` is deliberately non-streaming: the response is written after the
//! agent reaches a terminal answer. Session and lifecycle events are persisted
//! even though they are not sent as live notifications.

use std::io::{self, BufRead, Write};

use anyhow::Result;
use clap::{ArgAction, Args};
use hallucinate_agent::{
    agent::runtime::Agent,
    ai::types::{Message, ReasoningEffort},
    config::settings::Settings,
};
use hallucinate_session::manager::{SessionCreateOptions, SessionManager};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Arguments for RPC mode.
#[derive(Args, Debug, Clone)]
pub struct RpcArgs {
    /// Model to use as a configured model ID or "provider/model" pair.
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// API key for the provider. Overrides environment variables and stored
    /// credentials. Environment fallback also accepts OPENAI_API_KEY.
    #[arg(long = "api-key", env = "HALLUCINATE_API_KEY")]
    pub api_key: Option<String>,

    /// Base URL for the API (for OpenAI-compatible providers).
    #[arg(long = "base-url")]
    pub base_url: Option<String>,

    /// Shell executable for the shell tool. Overrides the settings file and
    /// SHELL environment variable.
    #[arg(long = "shell")]
    pub shell: Option<String>,

    /// Maximum number of tokens in the response.
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u32>,

    /// Temperature for sampling (0.0 - 2.0).
    #[arg(long = "temperature")]
    pub temperature: Option<f64>,

    /// Reasoning effort for models that support it.
    #[arg(long = "reasoning-effort", value_parser = parse_reasoning_effort)]
    pub reasoning_effort: Option<ReasoningEffort>,

    /// System prompt to use.
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,

    /// Attach an image to a print-mode prompt. May be repeated.
    #[arg(long = "image", action = ArgAction::Append)]
    pub images: Vec<std::path::PathBuf>,

    /// Load a skill Markdown file or a directory of skills. May be repeated.
    #[arg(long = "skill", action = ArgAction::Append)]
    pub skills: Vec<std::path::PathBuf>,

    /// Disable automatic discovery of user and project skills.
    #[arg(long = "no-skills")]
    pub no_skills: bool,
}

impl From<&RpcArgs> for hallucinate_agent::config::RuntimeOverrides {
    fn from(args: &RpcArgs) -> Self {
        let shell = match args.shell {
            Some(ref value) => Some(
                value
                    .clone()
                    .split(" ")
                    .map(|item| item.to_string())
                    .collect::<Vec<String>>(),
            ),
            None => None,
        };

        Self {
            model: args.model.clone(),
            api_key: args.api_key.clone(),
            base_url: args.base_url.clone(),
            shell: shell,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            reasoning_effort: args.reasoning_effort,
            system_prompt: args.system_prompt.clone(),
        }
    }
}

/// Parse a CLI reasoning-effort value with a user-facing validation error.
fn parse_reasoning_effort(value: &str) -> Result<ReasoningEffort, String> {
    value.parse()
}

/// Run the RPC mode server, reading JSON-RPC requests from stdin and writing
/// responses to stdout.
pub async fn rpc_command(args: &RpcArgs) -> Result<()> {
    let settings = &Settings::load()?;

    hallucinate_agent::config::paths::ensure_directories()?;

    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let value = match serde_json::from_str::<Value>(&line) {
            Ok(value) => value,

            Err(error) => {
                let response =
                    RpcResponse::error(-32700, "Parse error", Value::Null, error.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
                stdout.flush()?;

                continue;
            },
        };

        let request = match serde_json::from_value::<RpcRequest>(value) {
            Ok(request) => request,

            Err(error) => {
                let response =
                    RpcResponse::error(-32600, "Invalid Request", Value::Null, error.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
                stdout.flush()?;

                continue;
            },
        };

        // Only a structurally valid request without an id is a notification.
        // Invalid request objects still require an Invalid Request response,
        // even when their decoded representation has no id.
        let is_notification = request.id.is_none()
            && request.jsonrpc.as_deref() == Some("2.0")
            && request.method.is_some();

        let response = handle_request(request, args, settings).await;

        // JSON-RPC notifications are processed but do not receive a response.
        if !is_notification {
            writeln!(stdout, "{}", serde_json::to_string(&response)?)?;

            stdout.flush()?;
        }
    }

    Ok(())
}

/// Dispatch a single RPC request to the appropriate handler based on method.
async fn handle_request(
    request: RpcRequest,
    args: &RpcArgs,
    settings: &Settings,
) -> RpcResponse {
    let id = request.id.clone().unwrap_or(Value::Null);

    if !valid_id(&id) {
        return RpcResponse::error(
            -32600,
            "Invalid Request",
            Value::Null,
            "id must be a string, number, or null",
        );
    }

    if request.jsonrpc.as_deref() != Some("2.0") || request.method.is_none() {
        return RpcResponse::error(
            -32600,
            "Invalid Request",
            id,
            "jsonrpc must be '2.0' and method is required",
        );
    }

    let method = request.method.as_deref().unwrap_or_default();
    match method {
        "ping" => RpcResponse::result(id, json!({ "pong": true })),

        "list_sessions" => list_sessions(id),

        "create_session" => create_session(request.params.as_ref(), id),

        "chat" => chat(request.params.as_ref(), id, args, settings).await,

        _ => RpcResponse::error(-32601, "Method not found", id, method),
    }
}

/// List all sessions and return them as an RPC result.
fn list_sessions(id: Value) -> RpcResponse {
    match SessionManager::list() {
        Ok(sessions) => RpcResponse::result(
            id,
            json!({
                "sessions": sessions.into_iter().map(|session| json!({
                    "id": session.id,
                    "modified": session.modified.map(|time| time.to_rfc3339()),
                    "path": session.path
                })).collect::<Vec<_>>()
            }),
        ),

        Err(error) => RpcResponse::error(-32603, "Could not list sessions", id, error.to_string()),
    }
}

/// Create a new session from the given parameters and return it as an RPC
/// result.
fn create_session(
    params: Option<&Value>,
    id: Value,
) -> RpcResponse {
    let params = match object_params(params) {
        Ok(params) => params,
        Err(error) => return RpcResponse::error(-32602, "Invalid params", id, error),
    };

    let title = match optional_string(params, "title") {
        Ok(title) => title,
        Err(error) => return RpcResponse::error(-32602, "Invalid params", id, error),
    };

    let cwd = match optional_string(params, "cwd") {
        Ok(cwd) => cwd,
        Err(error) => return RpcResponse::error(-32602, "Invalid params", id, error),
    };

    let mut manager = SessionManager::new();
    match manager.create(SessionCreateOptions {
        title,
        parent_session_id: None,
        cwd,
    }) {
        Ok(session) => {
            RpcResponse::result(id, json!({ "session_id": session.id, "cwd": session.cwd }))
        },

        Err(error) => RpcResponse::error(-32603, "Could not create session", id, error.to_string()),
    }
}

/// Handle a chat request: create or resume a session, run the agent, and return
/// the result.
async fn chat(
    params: Option<&Value>,
    id: Value,
    args: &RpcArgs,
    settings: &Settings,
) -> RpcResponse {
    let params = match object_params(params) {
        Ok(Some(params)) => params,

        Ok(None) => {
            return RpcResponse::error(
                -32602,
                "Invalid params",
                id,
                "chat requires an object of parameters",
            );
        },

        Err(error) => return RpcResponse::error(-32602, "Invalid params", id, error),
    };

    let Some(prompt) = params
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
    else {
        return RpcResponse::error(
            -32602,
            "Invalid params",
            id,
            "message must be a non-empty string",
        );
    };

    let show_thinking = match optional_bool(Some(params), "thinking") {
        Ok(value) => value
            .unwrap_or_else(|| hallucinate_agent::config::resolve_show_thinking(settings, false)),

        Err(error) => return RpcResponse::error(-32602, "Invalid params", id, error),
    };

    let runtime_overrides = hallucinate_agent::config::RuntimeOverrides::from(args);

    let model = match hallucinate_agent::config::resolve_model(
        settings,
        runtime_overrides.model.as_ref(),
        runtime_overrides.base_url.as_ref(),
    ) {
        Ok(model) => model,

        Err(error) => {
            return RpcResponse::error(-32603, "Model configuration failed", id, error.to_string());
        },
    };

    let provider =
        match hallucinate_agent::config::create_provider(settings, &runtime_overrides, &model) {
            Ok(provider) => provider,

            Err(error) => {
                return RpcResponse::error(
                    -32603,
                    "Provider configuration failed",
                    id,
                    error.to_string(),
                );
            },
        };

    let session_id = match optional_string(Some(params), "session_id") {
        Ok(session_id) => session_id,

        Err(error) => return RpcResponse::error(-32602, "Invalid params", id, error),
    };

    let requested_cwd = match optional_string(Some(params), "cwd") {
        Ok(cwd) => cwd,

        Err(error) => return RpcResponse::error(-32602, "Invalid params", id, error),
    };

    let mut manager = SessionManager::new();

    let session = match session_id.as_deref() {
        Some(session_id) => match manager.resume(session_id) {
            Ok(session) => session,

            Err(error) => {
                return RpcResponse::error(-32602, "Invalid session", id, error.to_string());
            },
        },

        None => match manager.create(SessionCreateOptions {
            title: Some("RPC session".to_string()),
            parent_session_id: None,
            cwd: requested_cwd,
        }) {
            Ok(session) => session,

            Err(error) => {
                return RpcResponse::error(
                    -32603,
                    "Could not create session",
                    id,
                    error.to_string(),
                );
            },
        },
    };

    let storage = manager
        .current_storage()
        .expect("a created or resumed session has storage");

    let history = match storage.load_messages() {
        Ok(history) => history,

        Err(error) => {
            return RpcResponse::error(-32603, "Could not load session", id, error.to_string());
        },
    };

    let checkpoint = match storage.load_compaction_checkpoint() {
        Ok(checkpoint) => checkpoint,

        Err(error) => {
            return RpcResponse::error(
                -32603,
                "Could not load compaction checkpoint",
                id,
                error.to_string(),
            );
        },
    };

    let mut agent = Agent::new(
        provider,
        model.clone(),
        hallucinate_agent::config::agent_config(settings, &model, &runtime_overrides),
    )
    .with_cwd(session.cwd.clone());

    // Load built-in tools.
    agent.add_builtin_tools(settings.builtin_tools.to_tool_list());

    // Load MCP servers.
    if let Err(error) = agent.add_mcp_servers(&settings.mcp_servers).await {
        return RpcResponse::error(
            -32603,
            "MCP server configuration failed",
            id,
            error.to_string(),
        );
    }

    // Load in any script tools the user has defined.
    let script_tools = hallucinate_agent::agent::tools::script_tool::load_script_tools(
        &session.cwd,
        hallucinate_agent::config::resolve_shell_timeout(settings, None),
    );

    if let Some(diagnostic) = script_tools.diagnostics.first() {
        return RpcResponse::error(
            -32603,
            "Script tool configuration failed",
            id,
            format!("{}: {}", diagnostic.path.display(), diagnostic.message),
        );
    }

    for tool in script_tools.tools {
        if let Err(error) = agent.add_tool_checked(std::sync::Arc::new(tool)) {
            return RpcResponse::error(
                -32603,
                "Script tool configuration failed",
                id,
                error.to_string(),
            );
        }
    }

    if let Some(system_prompt) = &args.system_prompt {
        agent = agent.with_system_prompt(system_prompt.clone());
    }

    if let Err(error) = agent.load_history(history) {
        return RpcResponse::error(-32603, "Invalid session history", id, error.to_string());
    }

    if let Some((summary, archived_message_count)) = checkpoint
        && let Err(error) = agent.restore_compaction_checkpoint(summary, archived_message_count)
    {
        return RpcResponse::error(
            -32603,
            "Invalid compaction checkpoint",
            id,
            error.to_string(),
        );
    }

    let mut event_rx = agent.subscribe();

    agent.add_user_message(prompt);

    if let Some(message) = agent.messages().last()
        && let Err(error) = manager.save_message(message)
    {
        return RpcResponse::error(-32603, "Could not persist message", id, error.to_string());
    }

    let run_result = agent.run().await;

    // The agent owns the event sequence; drain it after completion so provider,
    // tool, compaction, and terminal-state diagnostics are durable in RPC mode.
    while let Ok(event) = event_rx.try_recv() {
        if let Err(error) = manager.save_agent_event(&event) {
            return RpcResponse::error(
                -32603,
                "Could not persist session event",
                id,
                error.to_string(),
            );
        }
    }

    // Persist any messages produced before a provider/tool failure too. User
    // messages are saved before the run. Compare complete records instead of
    // using an index because compaction prepends an internal checkpoint to the
    // provider-visible history.
    let persisted_messages = match storage.load_messages() {
        Ok(messages) => messages,

        Err(error) => {
            return RpcResponse::error(-32603, "Could not inspect session", id, error.to_string());
        },
    };
    for message in agent.messages() {
        if matches!(&message, Message::User(_)) {
            continue;
        }

        let already_persisted = persisted_messages.iter().any(|persisted| {
            serde_json::to_string(persisted).ok() == serde_json::to_string(&message).ok()
        });

        if already_persisted {
            continue;
        }

        if let Err(error) = manager.save_message(&message) {
            return RpcResponse::error(-32603, "Could not persist response", id, error.to_string());
        }
    }

    if let Err(error) = run_result {
        return RpcResponse::error(-32603, "Chat failed", id, error.to_string());
    }

    if let Some(summary) = agent.compaction_summary()
        && let Err(error) =
            manager.save_compaction_checkpoint(summary, agent.compaction_archived_message_count())
    {
        return RpcResponse::error(
            -32603,
            "Could not persist compaction checkpoint",
            id,
            error.to_string(),
        );
    }

    let answer = agent
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(message) => Some(assistant_output(message, show_thinking)),
            _ => None,
        })
        .unwrap_or_default();

    RpcResponse::result(id, json!({ "session_id": session.id, "message": answer }))
}

/// Extract the JSON object from params, returning None if params is
/// null/absent.
fn object_params(params: Option<&Value>) -> Result<Option<&Map<String, Value>>, String> {
    match params {
        None | Some(Value::Null) => Ok(None),

        Some(Value::Object(params)) => Ok(Some(params)),

        Some(_) => Err("params must be a JSON object".to_string()),
    }
}

/// Extract an optional string field from the params object.
fn optional_string(
    params: Option<&Map<String, Value>>,
    name: &str,
) -> Result<Option<String>, String> {
    match params.and_then(|params| params.get(name)) {
        None | Some(Value::Null) => Ok(None),

        Some(Value::String(value)) => Ok(Some(value.clone())),

        Some(_) => Err(format!("{name} must be a string or null")),
    }
}

/// Extract an optional boolean field from the params object.
fn optional_bool(
    params: Option<&Map<String, Value>>,
    name: &str,
) -> Result<Option<bool>, String> {
    match params.and_then(|params| params.get(name)) {
        None | Some(Value::Null) => Ok(None),

        Some(Value::Bool(value)) => Ok(Some(*value)),

        Some(_) => Err(format!("{name} must be a boolean or null")),
    }
}

/// Extract the text content from an assistant message, optionally including
/// thinking blocks.
fn assistant_output(
    message: &hallucinate_agent::ai::types::AssistantMessage,
    show_thinking: bool,
) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            hallucinate_agent::ai::types::AssistantContent::Text(text) => Some(text.text.as_str()),

            hallucinate_agent::ai::types::AssistantContent::Thinking(thinking) if show_thinking => {
                Some(thinking.thinking.as_str())
            },

            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check whether an id value is valid (null, string, or number).
fn valid_id(id: &Value) -> bool {
    id.is_null() || id.is_string() || id.is_number()
}

/// A parsed JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
struct RpcRequest {
    /// JSON-RPC version string.
    #[serde(default)]
    jsonrpc: Option<String>,

    /// RPC method name.
    #[serde(default)]
    method: Option<String>,

    /// Method parameters.
    #[serde(default)]
    params: Option<Value>,

    /// Request identifier.
    #[serde(default)]
    id: Option<Value>,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
struct RpcResponse {
    /// JSON-RPC version string.
    jsonrpc: &'static str,

    /// The result value, if the request succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,

    /// The error details, if the request failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,

    /// Request identifier, echoed back from the request.
    id: Value,
}

/// A JSON-RPC 2.0 error response.
#[derive(Debug, Serialize)]
struct RpcError {
    /// Error code.
    code: i32,

    /// Human-readable error message.
    message: String,

    /// Optional additional error data.
    data: Option<Value>,
}

impl RpcResponse {
    /// Construct a successful RPC response with the given id and result.
    fn result(
        id: Value,
        result: Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0",
            result: Some(result),
            error: None,
            id,
        }
    }

    /// Construct an error RPC response with the given code, message, id, and
    /// detail.
    fn error(
        code: i32,
        message: impl Into<String>,
        id: Value,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: Some(json!({ "detail": detail.into() })),
            }),
            id,
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli::{Cli, CliCommands};

    #[test]
    fn rpc_response_success_has_a_jsonrpc_id() {
        let response = RpcResponse::result(json!(1), json!({}));

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, json!(1));
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn rejects_wrong_jsonrpc_version() {
        let cli = Cli::try_parse_from(["hallucinate", "rpc"]);

        let cli = match cli {
            Ok(value) => value,

            Err(error) => panic!("Failed to parse CLI: {}", error),
        };

        if let Some(CliCommands::Rpc(args)) = &cli.command {
            let response = handle_request(
                RpcRequest {
                    jsonrpc: Some("1.0".to_string()),
                    method: Some("ping".to_string()),
                    params: None,
                    id: Some(json!(1)),
                },
                args,
                &Settings::default(),
            )
            .await;

            assert_eq!(response.error.unwrap().code, -32600);
        } else {
            panic!("Test command was not `hallucinate rpc`")
        }
    }

    #[test]
    fn validates_request_ids_and_parameter_shape() {
        assert!(valid_id(&json!(null)));
        assert!(valid_id(&json!("request")));
        assert!(!valid_id(&json!(true)));
        assert!(object_params(Some(&json!({}))).unwrap().is_some());
        assert!(object_params(Some(&json!([]))).is_err());
    }

    #[tokio::test]
    async fn chat_creates_and_persists_a_session() {
        let mut server = mockito::Server::new_async().await;

        let body = format!(
            "data: {}\n\ndata: [DONE]\n",
            json!({
                "choices": [{
                    "delta": { "content": "hello" },
                    "finish_reason": "stop"
                }]
            })
        );

        let _mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let cli = Cli::try_parse_from([
            "hallucinate",
            "rpc",
            "--model",
            "test-model",
            "--base-url",
            server.url().as_str(),
            "--api-key",
            "test-key",
        ]);

        let cli = match cli {
            Ok(value) => value,

            Err(error) => panic!("Failed to parse CLI: {}", error),
        };

        let mut settings = Settings::default();
        settings
            .providers
            .insert("openai".to_string(), Default::default());

        if let Some(CliCommands::Rpc(args)) = &cli.command {
            let response = chat(
                Some(&json!({ "message": "say hello" })),
                json!(7),
                args,
                &settings,
            )
            .await;

            let result = response.result.unwrap();
            assert_eq!(result["message"], "hello");

            let session_id = result["session_id"].as_str().unwrap().to_string();

            let mut manager = SessionManager::new();
            manager.resume(&session_id).unwrap();

            let messages = manager.current_storage().unwrap().load_messages().unwrap();
            assert!(
                matches!(&messages[0], Message::User(message) if message.content == "say hello")
            );
            assert!(
                matches!(&messages[1], Message::Assistant(message) if message.content[0].text_content() == Some("hello"))
            );

            SessionManager::delete_session(&session_id).unwrap();
        } else {
            panic!("Test command was not `hallucinate rpc`")
        }
    }
}
