//! Model Context Protocol (MCP) tool integration.
//!
//! Configured servers use rmcp's local stdio or streamable HTTP transports.
//! Tools are discovered with `tools/list` and given unique, configurable names
//! before the agent sees them.

use std::{collections::HashMap, fmt, process::Stdio, sync::Arc};

use rmcp::{
    RoleClient,
    ServiceExt,
    model::{CallToolRequestParams, ContentBlock, JsonObject, Tool as RmcpTool},
    service::RunningService,
    transport::{
        ConfigureCommandExt,
        StreamableHttpClientTransport,
        TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::tools::{
    error::McpError,
    types::{AsyncToolExecutionResult, Tool, ToolResult},
};

#[cfg_attr(
    not(feature = "schemas"),
    derive(Debug, Clone, Default, Serialize, Deserialize)
)]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)
)]
#[serde(tag = "type", rename_all = "kebab-case")]
/// Authentication for a remote MCP server.
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
#[cfg_attr(
    not(feature = "schemas"),
    derive(Debug, Clone, Default, Serialize, Deserialize)
)]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)
)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_prefix: Option<String>,
}

/// Return whether authentication is the default no-auth configuration.
fn mcp_auth_is_no_auth(auth: &McpAuthSettings) -> bool {
    matches!(auth, McpAuthSettings::NoAuth)
}

/// A connected MCP server managed by rmcp.
enum McpClient {
    /// A local server using rmcp's child-process transport.
    Stdio(RunningService<RoleClient, ()>),

    /// A remote server using rmcp's streamable HTTP transport.
    Http(RunningService<RoleClient, ()>),
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
    /// Connect to and initialize the configured MCP server.
    async fn start(settings: &McpServerSettings) -> Result<Self, McpError> {
        match (&settings.command, &settings.url) {
            (Some(_), Some(_)) => return Err(McpError::BothParamsProvided),
            (None, None) => return Err(McpError::MissingParams),
            (Some(command), None) => Self::start_stdio(command, settings).await,
            (None, Some(url)) => Self::start_http(url, &settings.auth).await,
        }
    }

    /// Start a local MCP server through rmcp's child-process transport.
    async fn start_stdio(
        command: &str,
        settings: &McpServerSettings,
    ) -> Result<Self, McpError> {
        if command.trim().is_empty() {
            return Err(McpError::EmptyCommand);
        }

        let process_command = command.to_string();
        let command = tokio::process::Command::new(command).configure(|process| {
            process.args(&settings.args).envs(&settings.env);
        });

        let transport = TokioChildProcess::builder(command)
            // MCP stdout is its protocol transport; server diagnostics must not
            // be emitted into the application's terminal output.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| McpError::FailedToStartServer {
                command: process_command.to_string(),
                error: error.to_string(),
            })?
            .0;

        let service = ().serve(transport).await?;

        Ok(Self::Stdio(service))
    }

    /// Start a remote MCP server through rmcp's streamable HTTP transport.
    async fn start_http(
        url: &str,
        auth: &McpAuthSettings,
    ) -> Result<Self, McpError> {
        let url = reqwest::Url::parse(url).map_err(|error| McpError::InvalidUrl {
            url: url.to_string(),
            error: error.to_string(),
        })?;

        if !matches!(url.scheme(), "http" | "https") {
            return Err(McpError::InvalidHttpProtocol);
        }

        let mut config = StreamableHttpClientTransportConfig::with_uri(url.to_string());

        if let McpAuthSettings::ApiKey {
            api_key,
            header,
            prefix,
        } = auth
        {
            if api_key.trim().is_empty() {
                return Err(McpError::EmptyApiKey);
            }

            let value = reqwest::header::HeaderValue::from_str(&format!("{prefix}{api_key}"))
                .map_err(|error| McpError::InvalidApiKeyValue(error))?;

            let name =
                reqwest::header::HeaderName::from_bytes(header.as_bytes()).map_err(|error| {
                    McpError::InvalidApiKeyHeader {
                        header: header.clone(),
                        error,
                    }
                })?;

            config.custom_headers.insert(name, value);
        }

        let service = ().serve(StreamableHttpClientTransport::from_config(config)).await?;

        Ok(Self::Http(service))
    }

    /// List the remote server's tools.
    async fn list_tools(&self) -> Result<Vec<RmcpTool>, McpError> {
        match self {
            Self::Stdio(service) | Self::Http(service) => Ok(service.list_all_tools().await?),
        }
    }

    /// Call one remote tool.
    async fn call_tool(
        &self,
        remote_name: String,
        arguments: JsonObject,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        let request = CallToolRequestParams::new(remote_name).with_arguments(arguments);

        match self {
            Self::Stdio(service) | Self::Http(service) => Ok(service.call_tool(request).await?),
        }
    }
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
    ) -> AsyncToolExecutionResult<'a> {
        Box::pin(async move {
            let arguments = arguments
                .as_object()
                .cloned()
                .ok_or_else(|| McpError::InvalidToolArgsType(self.server_name.clone()))?;

            let result = self
                .client
                .call_tool(self.remote_name.clone(), arguments)
                .await?;

            let content = format_mcp_content(&result.content);

            Ok(if result.is_error.unwrap_or(false) {
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
) -> Result<Vec<Arc<dyn Tool + Send + Sync>>, McpError> {
    let mut servers = servers.iter().collect::<Vec<_>>();
    servers.sort_unstable_by(|left, right| left.0.cmp(right.0));

    let mut tools: Vec<Arc<dyn Tool + Send + Sync>> = Vec::new();
    for (server_name, settings) in servers {
        let client = Arc::new(McpClient::start(settings).await?);

        let definitions = client.list_tools().await?;

        for definition in definitions {
            if definition.name.trim().is_empty() {
                return Err(McpError::EmptyToolName);
            }

            let name = qualified_tool_name(settings.tool_prefix.as_deref(), &definition.name);

            if tools.iter().any(|tool| tool.name() == name) {
                return Err(McpError::ToolNameCollision(name));
            }

            tools.push(Arc::new(McpTool {
                name,
                server_name: server_name.clone(),
                remote_name: definition.name.into_owned(),
                description: definition.description.map(Into::into).unwrap_or_default(),
                parameters: Value::Object((*definition.input_schema).clone()),
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
fn format_mcp_content(content: &[ContentBlock]) -> String {
    content
        .iter()
        .map(|item| match item.as_text() {
            Some(text) => text.text.clone(),
            None => serde_json::to_string(item).unwrap_or_else(|_| format!("{item:?}")),
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
}
