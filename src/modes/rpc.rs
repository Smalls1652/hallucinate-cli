//! RPC mode: newline-delimited JSON-RPC 2.0 over stdin/stdout.
//!
//! `chat` is deliberately non-streaming: the response is written after the
//! agent reaches a terminal answer. Session and lifecycle events are persisted
//! even though they are not sent as live notifications.

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    agent::{runtime::Agent, types::AgentConfig},
    ai::types::Message,
    cli::args::Cli,
    config::settings::Settings,
    session::manager::{SessionCreateOptions, SessionManager},
};

pub async fn run(
    cli: &Cli,
    settings: &Settings,
) -> Result<(), anyhow::Error> {
    crate::config::paths::ensure_directories()?;
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
        let response = handle_request(request, cli, settings).await;
        // JSON-RPC notifications are processed but do not receive a response.
        if !is_notification {
            writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

async fn handle_request(
    request: RpcRequest,
    cli: &Cli,
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
        "chat" => chat(request.params.as_ref(), id, cli, settings).await,
        _ => RpcResponse::error(-32601, "Method not found", id, method),
    }
}

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

async fn chat(
    params: Option<&Value>,
    id: Value,
    cli: &Cli,
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
        Ok(value) => value.unwrap_or_else(|| super::resolve_show_thinking(cli, settings)),
        Err(error) => return RpcResponse::error(-32602, "Invalid params", id, error),
    };

    let model = match super::resolve_model(cli, settings) {
        Ok(model) => model,
        Err(error) => {
            return RpcResponse::error(-32603, "Model configuration failed", id, error.to_string());
        },
    };
    let provider = match super::create_provider_for_model(cli, settings, &model) {
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

    let history = match manager
        .current_storage()
        .expect("a created or resumed session has storage")
        .load_messages()
    {
        Ok(history) => history,
        Err(error) => {
            return RpcResponse::error(-32603, "Could not load session", id, error.to_string());
        },
    };
    let history_len = history.len();
    let mut agent = Agent::with_all_tools_and_image_resize_and_shell(
        provider,
        model.clone(),
        AgentConfig {
            max_turns: settings.max_turns,
            max_context_tokens: model.context_window,
            enable_compaction: settings.enable_compaction,
            enable_tools: settings.enable_tools,
            temperature: settings.temperature,
            max_tokens: super::resolve_max_tokens(cli, settings, &model),
            include_tool_definitions: true,
        },
        settings.auto_resize_images,
        super::resolve_shell(cli, settings),
    )
    .with_cwd(session.cwd.clone());
    if let Some(system_prompt) = &cli.system_prompt {
        agent = agent.with_system_prompt(system_prompt.clone());
    }
    if let Err(error) = agent.load_history(history) {
        return RpcResponse::error(-32603, "Invalid session history", id, error.to_string());
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
        if let Some((event_type, data)) = super::interactive::session_event(&event)
            && let Err(error) = manager.save_event(event_type, data)
        {
            return RpcResponse::error(
                -32603,
                "Could not persist session event",
                id,
                error.to_string(),
            );
        }
    }

    // Persist any messages produced before a provider/tool failure too. The
    // user message was already saved, hence the history_len + 1 offset.
    for message in agent.messages().iter().skip(history_len + 1) {
        if let Err(error) = manager.save_message(message) {
            return RpcResponse::error(-32603, "Could not persist response", id, error.to_string());
        }
    }
    if let Err(error) = run_result {
        return RpcResponse::error(-32603, "Chat failed", id, error.to_string());
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

fn object_params(params: Option<&Value>) -> Result<Option<&Map<String, Value>>, String> {
    match params {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(params)) => Ok(Some(params)),
        Some(_) => Err("params must be a JSON object".to_string()),
    }
}

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

fn assistant_output(
    message: &crate::ai::types::AssistantMessage,
    show_thinking: bool,
) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            crate::ai::types::AssistantContent::Text(text) => Some(text.text.as_str()),
            crate::ai::types::AssistantContent::Thinking(thinking) if show_thinking => {
                Some(thinking.thinking.as_str())
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn valid_id(id: &Value) -> bool {
    id.is_null() || id.is_string() || id.is_number()
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
    id: Value,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
    data: Option<Value>,
}

impl RpcResponse {
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

    #[test]
    fn rpc_response_success_has_a_jsonrpc_id() {
        let response = RpcResponse::result(json!(1), json!({}));
        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, json!(1));
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn rejects_wrong_jsonrpc_version() {
        let response = handle_request(
            RpcRequest {
                jsonrpc: Some("1.0".to_string()),
                method: Some("ping".to_string()),
                params: None,
                id: Some(json!(1)),
            },
            &Cli::parse_from(["hallucinate"]),
            &Settings::default(),
        )
        .await;
        assert_eq!(response.error.unwrap().code, -32600);
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
            "--model",
            "test-model",
            "--base-url",
            server.url().as_str(),
            "--api-key",
            "test-key",
        ])
        .unwrap();
        let response = chat(
            Some(&json!({ "message": "say hello" })),
            json!(7),
            &cli,
            &Settings::default(),
        )
        .await;
        let result = response.result.unwrap();
        assert_eq!(result["message"], "hello");
        let session_id = result["session_id"].as_str().unwrap().to_string();
        let mut manager = SessionManager::new();
        manager.resume(&session_id).unwrap();
        let messages = manager.current_storage().unwrap().load_messages().unwrap();
        assert!(matches!(&messages[0], Message::User(message) if message.content == "say hello"));
        assert!(
            matches!(&messages[1], Message::Assistant(message) if message.content[0].text_content() == Some("hello"))
        );
        SessionManager::delete_session(&session_id).unwrap();
    }
}
