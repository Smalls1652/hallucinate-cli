use std::collections::HashMap;

#[cfg(feature = "schemas")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agent::tools::BuiltInTool;

/// The default time, in seconds, that a shell command can run.
pub const DEFAULT_SHELL_COMMAND_TIMEOUT: u64 = 60;

/// The default maximum amount of results to return.
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The default maximum amount of files to consider.
pub const DEFAULT_MAX_FILES: usize = 10_000;

/// The default maximum amount of directory entries to walk.
pub const DEFAULT_MAX_DIRECTORY_ENTRIES: usize = 20_000;

/// The default maximum bytes scanned from one grep candidate.
pub const DEFAULT_MAX_FILE_SCAN_BYTES: usize = 1_000_000;

/// The default maximum bytes scanned by one grep request.
pub const DEFAULT_MAX_SEARCH_BYTES: usize = 20 * (1024 * 1024);

/// The default maximum accepted line length while scanning grep candidates.
pub const DEFAULT_MAX_LINE_BYTES: usize = 64 * 1024;

/// The default maximum number of bytes read from/written to an ordinary text
/// file.
pub const DEFAULT_MAX_TEXT_FILE_BYTES: usize = 1_000_000;

/// The default maximum source-image bytes accepted before decoding.
pub const DEFAULT_MAX_IMAGE_SOURCE_BYTES: usize = 20 * (1024 * 1024);

/// The default maximum bytes included in a returned tool result.
pub const DEFAULT_MAX_TOOL_RESULT_BYTES: usize = 100_000;

/// Configuration for the built-in shell tool.
#[cfg_attr(not(feature = "schemas"), derive(Debug, Clone, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct ShellToolConfig {
    /// The shell executable and arguments to use (e.g., `["/bin/bash", "-c"]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<Vec<String>>,

    /// Maximum execution timeout in seconds.
    pub timeout: u64,

    /// Maximum bytes included in a returned tool result.
    pub max_tool_result_bytes: usize,
}

impl Default for ShellToolConfig {
    fn default() -> Self {
        Self {
            shell: None,
            timeout: DEFAULT_SHELL_COMMAND_TIMEOUT,
            max_tool_result_bytes: DEFAULT_MAX_TOOL_RESULT_BYTES,
        }
    }
}

/// Configuration for the built-in edit tool.
#[cfg_attr(not(feature = "schemas"), derive(Debug, Clone, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct EditToolConfig {}

impl Default for EditToolConfig {
    fn default() -> Self {
        Self {}
    }
}

/// Configuration for the built-in find tool.
#[cfg_attr(not(feature = "schemas"), derive(Debug, Clone, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct FindToolConfig {
    /// The maximum amount of results to return.
    pub max_results: usize,

    /// The maximum amount of files to consider.
    pub max_files: usize,

    /// The maximum amount of directory entries to walk.
    pub max_directory_entries: usize,
}

impl Default for FindToolConfig {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            max_files: DEFAULT_MAX_FILES,
            max_directory_entries: DEFAULT_MAX_DIRECTORY_ENTRIES,
        }
    }
}

/// Configuration for the built-in grep tool.
#[cfg_attr(not(feature = "schemas"), derive(Debug, Clone, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct GrepToolConfig {
    /// The maximum amount of results to return.
    pub max_results: usize,

    /// The maximum amount of files to consider.
    pub max_files: usize,

    /// The maximum amount of directory entries to walk.
    pub max_directory_entries: usize,

    /// Maximum bytes scanned from one grep candidate.
    pub max_file_scan_bytes: usize,

    /// Maximum bytes scanned by one grep request.
    pub max_search_bytes: usize,

    /// Maximum accepted line length while scanning grep candidates.
    pub max_line_bytes: usize,

    /// Maximum bytes included in a returned tool result.
    pub max_tool_result_bytes: usize,
}

impl Default for GrepToolConfig {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            max_files: DEFAULT_MAX_FILES,
            max_directory_entries: DEFAULT_MAX_DIRECTORY_ENTRIES,
            max_file_scan_bytes: DEFAULT_MAX_FILE_SCAN_BYTES,
            max_search_bytes: DEFAULT_MAX_SEARCH_BYTES,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            max_tool_result_bytes: DEFAULT_MAX_TOOL_RESULT_BYTES,
        }
    }
}

/// Configuration for the built-in ls tool.
#[cfg_attr(not(feature = "schemas"), derive(Debug, Clone, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct LsToolConfig {
    /// The maximum amount of results to return.
    pub max_results: usize,

    /// The maximum amount of files to consider.
    pub max_files: usize,

    /// The maximum amount of directory entries to walk.
    pub max_directory_entries: usize,
}

impl Default for LsToolConfig {
    fn default() -> Self {
        Self {
            max_results: DEFAULT_MAX_RESULTS,
            max_files: DEFAULT_MAX_FILES,
            max_directory_entries: DEFAULT_MAX_DIRECTORY_ENTRIES,
        }
    }
}

/// Configuration for the built-in read tool.
#[cfg_attr(not(feature = "schemas"), derive(Debug, Clone, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct ReadToolConfig {
    /// Whether to automatically resize images when reading files.
    pub auto_resize_images: bool,

    /// Maximum number of bytes read from an ordinary text file.
    pub max_text_file_bytes: usize,

    /// Maximum source-image bytes accepted before decoding.
    pub max_image_source_bytes: usize,
}

impl Default for ReadToolConfig {
    fn default() -> Self {
        Self {
            auto_resize_images: true,
            max_text_file_bytes: DEFAULT_MAX_TEXT_FILE_BYTES,
            max_image_source_bytes: DEFAULT_MAX_IMAGE_SOURCE_BYTES,
        }
    }
}

/// Configuration for the built-in write tool.
#[cfg_attr(not(feature = "schemas"), derive(Debug, Clone, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct WriteToolConfig {
    /// Maximum number of bytes written to an ordinary text file.
    pub max_text_file_bytes: usize,

    /// Maximum bytes included in a returned tool result.
    pub max_tool_result_bytes: usize,
}

impl Default for WriteToolConfig {
    fn default() -> Self {
        Self {
            max_text_file_bytes: DEFAULT_MAX_TEXT_FILE_BYTES,
            max_tool_result_bytes: DEFAULT_MAX_TOOL_RESULT_BYTES,
        }
    }
}

/// Settings for the built-in tools.
#[cfg_attr(
    not(feature = "schemas"),
    derive(Debug, Clone, Default, Serialize, Deserialize)
)]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct BuiltInToolSettings {
    /// Settings for the shell tool.
    pub shell: ShellToolConfig,

    /// Settings for the edit tool.
    pub edit: EditToolConfig,

    /// Settings for the find tool.
    pub find: FindToolConfig,

    /// Settings for the grep tool.
    pub grep: GrepToolConfig,

    /// Settings for the ls tool.
    pub ls: LsToolConfig,

    /// Settings for the read tool.
    pub read: ReadToolConfig,

    /// Settings for the write tool.
    pub write: WriteToolConfig,
}

impl BuiltInToolSettings {
    /// Create a [`Vec<BuiltInTool>`] for built-in tools.
    pub fn to_tool_list(&self) -> Vec<BuiltInTool> {
        let mut tool_list = vec![];

        tool_list.push(BuiltInTool::Shell(self.shell.clone()));
        tool_list.push(BuiltInTool::Edit(self.edit.clone()));
        tool_list.push(BuiltInTool::Find(self.find.clone()));
        tool_list.push(BuiltInTool::Grep(self.grep.clone()));
        tool_list.push(BuiltInTool::Ls(self.ls.clone()));
        tool_list.push(BuiltInTool::Read(self.read.clone()));
        tool_list.push(BuiltInTool::Write(self.write.clone()));

        tool_list
    }
}

/// Settings for managing tools.
#[cfg_attr(not(feature = "schemas"), derive(Debug, Clone, Serialize, Deserialize))]
#[cfg_attr(
    feature = "schemas",
    derive(Debug, Clone, Serialize, Deserialize, JsonSchema)
)]
#[serde(default)]
pub struct ToolSettings {
    /// Enable tool use.
    pub enable: bool,

    /// Enable tools by name. (Cannot be used in combination with
    /// `disable_tools`)
    pub enable_tools: Vec<String>,

    /// Disable tools by name. (Cannot be used in combination with
    /// `enable_tools`)
    pub disable_tools: Vec<String>,

    /// Config settings for built-in tools.
    pub builtin: BuiltInToolSettings,

    /// MCP servers whose stdio tools are exposed to the agent. Each map key
    /// identifies a server and becomes part of its qualified tool names.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub mcp_servers: HashMap<String, crate::agent::tools::mcp::McpServerSettings>,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            enable: true,
            enable_tools: vec![],
            disable_tools: vec![],
            builtin: BuiltInToolSettings::default(),
            mcp_servers: HashMap::new(),
        }
    }
}
