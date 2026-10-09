//! Errors for the tool system.

/// Errors related to MCP (Model Context Protocol) server configuration and
/// execution.
#[derive(thiserror::Error, Debug)]
pub enum McpError {
    /// MCP tool arguments for the tool must be a JSON object.
    #[error("MCP tool arguments for '{0}' must be a JSON object")]
    InvalidToolArgsType(String),

    /// MCP server must configure either command or url, not both.
    #[error("MCP server must configure either command or url, not both")]
    BothParamsProvided,

    /// MCP server must configure command or url.
    #[error("MCP server must configure command or url")]
    MissingParams,

    /// MCP server command cannot be empty.
    #[error("MCP server command cannot be empty")]
    EmptyCommand,

    /// Failed to start MCP server.
    #[error("failed to start MCP server '{command}': {error}")]
    FailedToStartServer { command: String, error: String },

    /// Invalid MCP server URL.
    #[error("invalid MCP server URL '{url}': {error}")]
    InvalidUrl { url: String, error: String },

    /// MCP server URL must use http or https.
    #[error("MCP server URL must use http or https")]
    InvalidHttpProtocol,

    /// MCP API key cannot be empty.
    #[error("MCP API key cannot be empty")]
    EmptyApiKey,

    /// Invalid MCP API-key value.
    #[error("invalid MCP API-key value: {0}")]
    InvalidApiKeyValue(reqwest::header::InvalidHeaderValue),

    /// Invalid MCP API-key header.
    #[error("invalid MCP API-key header '{header}': {error}")]
    InvalidApiKeyHeader {
        header: String,
        error: reqwest::header::InvalidHeaderName,
    },

    /// MCP client initialization error.
    #[error("MCP client initialization error: {0}")]
    InitError(#[from] rmcp::service::ClientInitializeError),

    /// MCP service error.
    #[error("MCP service error: {0}")]
    ServiceError(#[from] rmcp::ServiceError),

    /// Empty tool name returned by MCP server.
    #[error("empty tool name returned by MCP servcer")]
    EmptyToolName,

    /// MCP server tool name collision.
    #[error("MCP server tool name collision: {0}")]
    ToolNameCollision(String),
}

/// Errors internal to the tool system (file I/O, image processing, limits,
/// etc.).
#[derive(thiserror::Error, Debug)]
pub enum InternalToolError {
    /// Unknown tool requested.
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    /// The blocking task failed.
    #[error("{0} blocking task failed: {1}")]
    BlockingTaskFailed(String, #[source] tokio::task::JoinError),

    /// Failed to read a file.
    #[error("failed to read file {0}: {1}")]
    FailedToReadFile(String, std::io::Error),

    /// A generic I/O error.
    #[error("I/O error: {0}")]
    IOError(#[from] std::io::Error),

    /// Template expansion failed.
    #[error("template expansion error: {0}")]
    TemplateExpansionError(#[from] minijinja::Error),

    /// A tool limits error.
    #[error("{0}")]
    LimitsError(#[from] ToolLimitsError),

    /// An image processing error.
    #[error("{0}")]
    Image(#[from] ImageError),

    /// An MCP error.
    #[error("{0}")]
    Mcp(#[from] McpError),

    /// Failed to open a file.
    #[error("failed to open {0}")]
    FailedToOpen(String, #[source] std::io::Error),

    /// Failed to inspect a file.
    #[error("failed to inspect {0}")]
    FailedToInspect(String, #[source] std::io::Error),

    /// The file is not valid UTF-8 text or a supported image.
    #[error("file is not valid UTF-8 text or a supported image")]
    InvalidFile,

    /// A required field is missing.
    #[error("missing required field: {0}")]
    MissingRequiredField(String),

    /// Failed to determine the current working directory.
    #[error("failed to determine the current directory: {0}")]
    FailedToGetCurrentDirectory(#[source] std::io::Error),

    /// Field edits must be an array.
    #[error("Field edits must be an array")]
    FieldEditsNotAnArray,

    /// Field edits must contain at least one edit.
    #[error("Field edits must contain at least one edit")]
    FieldEditsMissingEdits,

    /// A required field is missing from a specific edit.
    #[error("Missing required field: edits[{0}].{1}")]
    EditsMissingRequiredField(usize, EditsField),

    /// The glob pattern is invalid.
    #[error("Invalid glob pattern '{0}': {1}")]
    GlobInvalidPattern(String, #[source] glob::PatternError),

    /// The regex pattern is invalid.
    #[error("Invalid regex pattern '{0}': {1}")]
    RegexInvalidPattern(String, #[source] regex::Error),

    /// The file pattern is invalid.
    #[error("Invalid file_pattern: {0}")]
    InvalidFilePattern(#[source] glob::PatternError),

    /// Tool arguments must be a JSON object.
    #[error("tool arguments must be an object")]
    InvalidScriptToolArgsType,

    /// A parameter value does not satisfy its declared constraints.
    #[error("parameter '{0}' does not satisfy its constraints")]
    InvalidScriptToolParamConstraint(String),

    /// A parameter value has an unexpected type.
    #[error("parameter '{0}' must be a {1}")]
    InvalidScriptToolParamType(String, String),
}

/// A field name within a field edit operation.
#[derive(Debug)]
pub enum EditsField {
    /// The `old_text` field.
    OldText,

    /// The `new_text` field.
    NewText,
}

impl std::fmt::Display for EditsField {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let value = match self {
            EditsField::OldText => "old_text",
            EditsField::NewText => "new_text",
        };

        write!(f, "{value}")
    }
}

/// Errors related to tool output size limits.
#[derive(thiserror::Error, Debug)]
pub enum ToolLimitsError {
    /// The value must be a non-negative integer.
    #[error("{0} must be a non-negative integer")]
    NonNegativeInteger(String),

    /// The value is too large for this platform.
    #[error("{0} is too large for this platform")]
    UnboundedIntegerTooLarge(String),

    /// The value must not exceed the specified maximum.
    #[error("{0} must not exceed {1}")]
    UnboundedIntegerExceedsMax(String, usize),
}

/// Errors related to image processing.
#[derive(thiserror::Error, Debug)]
pub enum ImageError {
    /// Failed to inspect an image file.
    #[error("failed to inspect image file {0}")]
    FailedToInspect(String),

    /// An image file exceeds the source byte limit.
    #[error(
        "image file {display_path} is {source_bytes} bytes (maximum: {max_source_bytes} bytes)"
    )]
    FileTooLarge {
        /// The display path of the image file.
        display_path: String,
        /// The number of source bytes in the image file.
        source_bytes: u64,
        /// The maximum allowed source bytes.
        max_source_bytes: usize,
    },

    /// Failed to read an image file.
    #[error("failed to read image file {0}")]
    FailedToRead(String),

    /// An image file grew beyond its source-byte limit during processing.
    #[error("image file {0} grew beyond its source-byte limit")]
    ConvertedImageLarger(String),

    /// Failed to process an image file.
    #[error("failed to process image file {0}")]
    FailedToProcess(String),

    /// The image data is unsupported or invalid.
    #[error("unsupported or invalid image data")]
    InvalidData,

    /// The image format is unsupported.
    #[error("unsupported image format")]
    UnsupportedFormat,

    /// Failed to decode the image.
    #[error("failed to decode image")]
    DecodeFailure,

    /// Image dimensions must be non-zero.
    #[error("image dimensions must be non-zero")]
    NonZeroDimensions,

    /// The image could not be resized below the specified byte limit.
    #[error("image could not be resized below the {0} byte limit")]
    StillAboveMaxBytes(usize),

    /// Failed to encode the resized image as PNG.
    #[error("failed to encode resized image as PNG")]
    FailedToEncodeToPNG,
}

/// Errors related to script tool definition parsing and validation.
#[derive(thiserror::Error, Debug)]
pub enum ScriptToolDefinitionError {
    /// A generic I/O error.
    #[error("I/O error: {0}")]
    IOError(#[from] std::io::Error),

    /// A TOML deserialization error.
    #[error("TOML load error: {0}")]
    TomlLoadError(#[from] toml::de::Error),

    /// The tool ID contains invalid characters (must be letters, numbers,
    /// underscores, or hyphens).
    #[error("tool ID must contain only letters, numbers, underscores, or hyphens")]
    InvalidToolName,

    /// The command has no executable.
    #[error("command must contain a non-empty executable")]
    EmptyToolExecutable,

    /// The description is empty.
    #[error("description cannot be empty")]
    EmptyDescription,

    /// The shell has no executable configured.
    #[error("shell must contain a non-empty executable when configured")]
    EmptyShell,

    /// The timeout is not greater than zero.
    #[error("timeout must be greater than zero when configured")]
    InvalidTimeout,

    /// A parameter name contains invalid characters.
    #[error("parameter '{0}' must contain only letters, numbers, or underscores")]
    InvalidParameterName(String),

    /// A parameter has an empty description.
    #[error("parameter '{0}' has an empty description")]
    EmptyParameterDescription(String),

    /// A parameter has an unsupported input type.
    #[error("parameter '{0}' has unsupported input_type '{1}'")]
    InvalidParameterInputType(String, String),

    /// A parameter's constraint enum is empty.
    #[error("parameter '{0}' constraint enum cannot be empty")]
    EmptyParameterConstraintEnum(String),

    /// A parameter's constraint enum values don't match the input type.
    #[error("parameter '{0}' constraint enum values must match input_type '{1}'")]
    InvalidParameterConstraintEnumInputType(String, String),

    /// Minimum and maximum are used with a non-numeric input type.
    #[error("parameter '{0}' minimum and maximum require a numeric input_type")]
    InvalidParameterConstraintNumericUse(String),

    /// Minimum and maximum values are not finite numbers.
    #[error("parameter '{0}' minimum and maximum must be finite numbers")]
    InvalidParameterConstraintNumericValue(String),

    /// The minimum value exceeds the maximum value.
    #[error("parameter '{0}' minimum cannot exceed maximum")]
    ParameterConstraintMinNumericValueExceedsMax(String),
}
