//! Model Context Protocol (MCP) tool integration.
//!
//! Configured servers use either local stdio or the MCP streamable HTTP
//! transport. Tools are discovered with `tools/list` and given unique,
//! configurable names before the agent sees them.

use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use futures::Future;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

use crate::agent::tools::types::{Tool, ToolResult};

/// Authentication for a remote MCP server.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum McpAuthSettings {
    /// Send no authentication header.
    #[default]
    NoAuth,

    /// Add an API key to every remote MCP request.
    ApiKey {
        /// API key value sent to the remote server.
        api_key: String,
        /// HTTP header receiving the API key. Defaults to `Authorization`.
        #[serde(default = "default_api_key_header")]
        header: String,
        /// Optional text prepended to the API key. Defaults to `Bearer `.
        #[serde(default = "default_api_key_prefix")]
        prefix: String,
    },
}

/// Return the default HTTP header for API-key authentication.
fn default_api_key_header() -> String {
    "Authorization".to_string()
}

/// Return the default value prefix for API-key authentication.
fn default_api_key_prefix() -> String {
    "Bearer ".to_string()
}

/// Configuration for one MCP server.
///
/// Set `command` for a local stdio server or `url` for a remote streamable
/// HTTP server.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct McpServerSettings {
    /// Executable used to start a local stdio server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Arguments passed to `command` for a local server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// Environment variables added to a local server process.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// Streamable HTTP endpoint for a remote MCP server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Authentication sent to a remote MCP server. Defaults to no auth.
    #[serde(default, skip_serializing_if = "mcp_auth_is_no_auth")]
    pub auth: McpAuthSettings,

    /// Optional prefix for discovered tools.
    ///
    /// When omitted or empty, the remote tool name is used directly after
    /// sanitization. Configure a prefix to disambiguate tools deliberately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_prefix: Option<String>,
}

/// Parsed tool definition from an MCP server's `tools/list` response.
#[derive(Debug, serde::Deserialize)]
struct McpToolDefinition {
    /// Unique tool name.
    name: String,

    /// Human-readable tool description.
    #[serde(default)]
    description: String,

    /// JSON Schema describing accepted input parameters.
    #[serde(default, rename = "inputSchema")]
    input_schema: Value,
}

/// A connected local MCP stdio server.
struct StdioConnection {
    /// The spawned MCP server process.
    child: Child,

    /// Pipe handle for writing JSON-RPC requests.
    stdin: ChildStdin,

    /// Line-buffered reader for JSON-RPC responses.
    stdout: Lines<BufReader<ChildStdout>>,
}

impl Drop for StdioConnection {
    fn drop(&mut self) {
        // Stopping the runtime command also stops a `docker run -i` or similar
        // container session. This prevents MCP containers surviving after the
        // agent exits; use `--rm` to remove their stopped containers as well.
        let _ = self.child.start_kill();
    }
}

/// A connected remote MCP HTTP server.
struct HttpConnection {
    /// HTTP client for sending requests.
    client: reqwest::Client,

    /// Target MCP server URL.
    url: String,

    /// Authentication configuration for the server.
    auth: McpAuthSettings,

    /// Active session ID, set by the server.
    session_id: Mutex<Option<HeaderValue>>,
}

/// Transport layer for an MCP server connection.
enum McpTransport {
    /// Local stdio-based server.
    Stdio(Mutex<StdioConnection>),

    /// Remote HTTP-based server.
    Http(HttpConnection),
}

/// Client for a single MCP server.
struct McpClient {
    /// Transport used to communicate with the server.
    transport: McpTransport,

    /// Monotonically increasing request ID counter.
    next_request_id: AtomicU64,
}

impl fmt::Debug for McpClient {
    fn fmt(
        &self,
        formatter: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        formatter.debug_struct("McpClient").finish_non_exhaustive()
    }
}

impl McpClient {
    /// Initialize the MCP server connection and exchange handshake messages.
    async fn start(settings: &McpServerSettings) -> Result<Self, anyhow::Error> {
        let transport = match (&settings.command, &settings.url) {
            (Some(_), Some(_)) => {
                anyhow::bail!("MCP server must configure either command or url, not both")
            },

            (None, None) => anyhow::bail!("MCP server must configure command or url"),

            (Some(command), None) => {
                McpTransport::Stdio(Mutex::new(start_stdio(command, settings).await?))
            },

            (None, Some(url)) => {
                let url = reqwest::Url::parse(url)
                    .map_err(|error| anyhow::anyhow!("invalid MCP server URL '{url}': {error}"))?;

                if !matches!(url.scheme(), "http" | "https") {
                    anyhow::bail!("MCP server URL must use http or https");
                }

                McpTransport::Http(HttpConnection {
                    client: reqwest::Client::new(),
                    url: url.into(),
                    auth: settings.auth.clone(),
                    session_id: Mutex::new(None),
                })
            },
        };

        let client = Self {
            transport,
            next_request_id: AtomicU64::new(1),
        };

        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "hallucinate", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;

        client
            .notify("notifications/initialized", json!({}))
            .await?;

        Ok(client)
    }

    /// Send a notification (fire-and-forget) to the MCP server.
    async fn notify(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(), anyhow::Error> {
        self.send(
            json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Send a request to the MCP server and return the result.
    async fn request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, anyhow::Error> {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);

        let response = self
            .send(
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
                Some(id),
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("MCP {method} response has no result"))?;

        if let Some(error) = response.get("error") {
            anyhow::bail!("MCP {method} failed: {error}");
        }

        response
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("MCP {method} response has no result"))
    }

    /// Send a JSON-RPC message to the server and optionally wait for a
    /// response.
    async fn send(
        &self,
        request: Value,
        expected_id: Option<u64>,
    ) -> Result<Option<Value>, anyhow::Error> {
        match &self.transport {
            McpTransport::Stdio(connection) => {
                let mut connection = connection.lock().await;

                write_jsonrpc_message(&mut connection.stdin, &request).await?;

                let Some(expected_id) = expected_id else {
                    return Ok(None);
                };

                while let Some(line) = connection.stdout.next_line().await? {
                    let response = parse_jsonrpc_response(&line)?;

                    if response.get("id") == Some(&json!(expected_id)) {
                        return Ok(Some(response));
                    }
                }

                match connection.child.try_wait().map_err(|error| {
                    anyhow::anyhow!("failed to inspect MCP server process: {error}")
                })? {
                    Some(status) => {
                        anyhow::bail!("MCP server exited while handling request: {status}")
                    },

                    None => anyhow::bail!(
                        "MCP server closed stdout while handling request but is still running"
                    ),
                }
            },
            McpTransport::Http(connection) => {
                send_http_request(connection, request, expected_id).await
            },
        }
    }
}

/// Start a local MCP stdio server and return a connection handle.
async fn start_stdio(
    command: &str,

    settings: &McpServerSettings,
) -> Result<StdioConnection, anyhow::Error> {
    if command.trim().is_empty() {
        anyhow::bail!("MCP server command cannot be empty");
    }

    let mut process = Command::new(command);
    process.args(&settings.args).envs(&settings.env);
    // An MCP stdio server's stdout is its JSON-RPC transport, so it must
    // always be captured rather than inherited by the user's terminal. Server
    // diagnostics must not disrupt the TUI or RPC output either.
    process.stdin(std::process::Stdio::piped());
    process.stdout(std::process::Stdio::piped());
    process.stderr(std::process::Stdio::null());

    let mut child = process
        .spawn()
        .map_err(|error| anyhow::anyhow!("failed to start MCP server '{command}': {error}"))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP server stdin was not piped"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP server stdout was not piped"))?;

    // Retain the child as well as its pipe handles. This lets the client reap
    // local servers and report their exit status, including a container
    // runtime which fails to start its configured image.
    Ok(StdioConnection {
        child,
        stdin,
        stdout: BufReader::new(stdout).lines(),
    })
}

/// Send an HTTP request to a remote MCP server and return the response.
async fn send_http_request(
    connection: &HttpConnection,
    request: Value,
    expected_id: Option<u64>,
) -> Result<Option<Value>, anyhow::Error> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );

    if let Some(session_id) = connection.session_id.lock().await.clone() {
        headers.insert("mcp-session-id", session_id);
    }

    add_auth_header(&mut headers, &connection.auth)?;

    let response = connection
        .client
        .post(&connection.url)
        .headers(headers)
        .json(&request)
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("MCP HTTP request failed: {error}"))?
        .error_for_status()
        .map_err(|error| anyhow::anyhow!("MCP HTTP server returned an error: {error}"))?;

    if let Some(session_id) = response.headers().get("mcp-session-id") {
        *connection.session_id.lock().await = Some(session_id.clone());
    }

    if expected_id.is_none() || response.status() == reqwest::StatusCode::ACCEPTED {
        return Ok(None);
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();

    let body = response.text().await?;

    let response = if content_type.starts_with("text/event-stream") {
        parse_sse_response(&body, expected_id.expect("requests have an expected ID"))?
    } else {
        parse_jsonrpc_response(&body)?
    };

    Ok(Some(response))
}

/// Return whether authentication is the default no-auth configuration.
fn mcp_auth_is_no_auth(auth: &McpAuthSettings) -> bool {
    matches!(auth, McpAuthSettings::NoAuth)
}

/// Add configured remote-server authentication to HTTP request headers.
fn add_auth_header(
    headers: &mut HeaderMap,
    auth: &McpAuthSettings,
) -> Result<(), anyhow::Error> {
    let McpAuthSettings::ApiKey {
        api_key,
        header,
        prefix,
    } = auth
    else {
        return Ok(());
    };

    if api_key.trim().is_empty() {
        anyhow::bail!("MCP API key cannot be empty");
    }

    let name = reqwest::header::HeaderName::from_bytes(header.as_bytes())
        .map_err(|error| anyhow::anyhow!("invalid MCP API-key header '{header}': {error}"))?;
    let value = HeaderValue::from_str(&format!("{prefix}{api_key}"))
        .map_err(|error| anyhow::anyhow!("invalid MCP API-key value: {error}"))?;

    headers.insert(name, value);

    Ok(())
}

/// Parse a JSON-RPC response from a string body.
fn parse_jsonrpc_response(body: &str) -> Result<Value, anyhow::Error> {
    serde_json::from_str(body)
        .map_err(|error| anyhow::anyhow!("MCP server returned invalid JSON-RPC: {error}"))
}

/// Parse a Server-Sent Events body and return the JSON-RPC response with the
/// expected ID.
fn parse_sse_response(
    body: &str,
    expected_id: u64,
) -> Result<Value, anyhow::Error> {
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };

        let response = parse_jsonrpc_response(data.trim())?;

        if response.get("id") == Some(&json!(expected_id)) {
            return Ok(response);
        }
    }

    anyhow::bail!("MCP HTTP event stream ended without the request response")
}

/// Write a JSON-RPC message to the server's stdin.
async fn write_jsonrpc_message(
    stdin: &mut ChildStdin,
    message: &Value,
) -> Result<(), anyhow::Error> {
    let message = serde_json::to_string(message)?;

    stdin.write_all(message.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;

    Ok(())
}

/// A tool supplied by a configured MCP server.
pub struct McpTool {
    /// Qualified tool name presented to the agent.
    name: String,

    /// Name of the MCP server providing this tool.
    server_name: String,

    /// Remote tool name as reported by the MCP server.
    remote_name: String,

    /// Human-readable tool description.
    description: String,

    /// JSON Schema describing accepted input parameters.
    parameters: Value,

    /// Shared client handle for communicating with the MCP server.
    client: Arc<McpClient>,
}

impl fmt::Debug for McpTool {
    fn fmt(
        &self,
        formatter: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        formatter
            .debug_struct("McpTool")
            .field("name", &self.name)
            .field("server_name", &self.server_name)
            .field("remote_name", &self.remote_name)
            .finish()
    }
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn execute<'a>(
        &'a self,
        arguments: &'a Value,
        _cwd: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<ToolResult, anyhow::Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let result = self
                .client
                .request(
                    "tools/call",
                    json!({
                        "name": self.remote_name,
                        "arguments": arguments,
                    }),
                )
                .await?;

            let is_error = result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let content = format_mcp_content(result.get("content"));

            Ok(if is_error {
                ToolResult::error("", &self.name, content)
            } else {
                ToolResult::success("", &self.name, content)
            })
        })
    }
}

/// Start configured servers and discover their tools.
pub async fn load_mcp_tools(
    servers: &HashMap<String, McpServerSettings>
) -> Result<Vec<Arc<dyn Tool + Send + Sync>>, anyhow::Error> {
    let mut servers = servers.iter().collect::<Vec<_>>();
    servers.sort_unstable_by(|left, right| left.0.cmp(right.0));

    let mut tools: Vec<Arc<dyn Tool + Send + Sync>> = Vec::new();

    for (server_name, settings) in servers {
        let client = Arc::new(McpClient::start(settings).await.map_err(|error| {
            anyhow::anyhow!("failed to initialize MCP server '{server_name}': {error}")
        })?);

        let result = client
            .request("tools/list", json!({}))
            .await
            .map_err(|error| {
                anyhow::anyhow!("failed to list tools from MCP server '{server_name}': {error}")
            })?;

        let definitions: Vec<McpToolDefinition> =
            serde_json::from_value(result.get("tools").cloned().unwrap_or_else(|| json!([])))
                .map_err(|error| {
                    anyhow::anyhow!("MCP server '{server_name}' returned invalid tools: {error}")
                })?;

        for definition in definitions {
            if definition.name.trim().is_empty() {
                anyhow::bail!("MCP server '{server_name}' returned a tool with an empty name");
            }

            let name = qualified_tool_name(settings.tool_prefix.as_deref(), &definition.name);

            if tools.iter().any(|tool| tool.name() == name) {
                anyhow::bail!("MCP server tool name collision: {name}");
            }

            tools.push(Arc::new(McpTool {
                name,
                server_name: server_name.clone(),
                remote_name: definition.name,
                description: definition.description,
                parameters: definition.input_schema,
                client: client.clone(),
            }));
        }
    }

    Ok(tools)
}

/// Build a qualified tool name by sanitizing and optionally prefixing the
/// remote name.
fn qualified_tool_name(
    prefix: Option<&str>,
    tool_name: &str,
) -> String {
    let tool_name = sanitize_name(tool_name);

    let Some(prefix) = prefix.filter(|prefix| !prefix.is_empty()) else {
        return tool_name;
    };

    format!(
        "{}_{}",
        sanitize_name(prefix).trim_end_matches('_'),
        tool_name
    )
}

/// Sanitize a name by replacing non-alphanumeric characters with underscores.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Format MCP tool content into a single string.
fn format_mcp_content(content: Option<&Value>) -> String {
    let Some(Value::Array(items)) = content else {
        return String::new();
    };

    items
        .iter()
        .map(|item| match item.get("type").and_then(Value::as_str) {
            Some("text") => item
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => serde_json::to_string(item).unwrap_or_else(|_| item.to_string()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_unprefixed_sanitized_mcp_tool_names_by_default() {
        assert_eq!(qualified_tool_name(None, "get/file"), "get_file");
        assert_eq!(qualified_tool_name(Some(""), "get/file"), "get_file");
    }

    #[test]
    fn qualifies_and_sanitizes_mcp_tool_names_when_configured() {
        assert_eq!(
            qualified_tool_name(Some("tools-"), "get/file"),
            "tools_get_file"
        );
    }

    #[test]
    fn parses_sse_jsonrpc_responses() {
        let response = parse_sse_response(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{}}\n",
            7,
        )
        .unwrap();
        assert_eq!(response["id"], 7);
    }

    #[test]
    fn adds_default_bearer_api_key_authentication() {
        let mut headers = HeaderMap::new();
        add_auth_header(
            &mut headers,
            &McpAuthSettings::ApiKey {
                api_key: "secret".to_string(),
                header: default_api_key_header(),
                prefix: default_api_key_prefix(),
            },
        )
        .unwrap();

        assert_eq!(
            headers.get(reqwest::header::AUTHORIZATION).unwrap(),
            "Bearer secret"
        );
    }

    #[test]
    fn supports_custom_api_key_headers() {
        let mut headers = HeaderMap::new();
        add_auth_header(
            &mut headers,
            &McpAuthSettings::ApiKey {
                api_key: "secret".to_string(),
                header: "X-API-Key".to_string(),
                prefix: String::new(),
            },
        )
        .unwrap();

        assert_eq!(headers.get("x-api-key").unwrap(), "secret");
    }
}
