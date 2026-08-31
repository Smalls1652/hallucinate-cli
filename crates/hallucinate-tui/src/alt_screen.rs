//! Transcript data used by the purpose-built interactive UI.

use std::path::Path;

/// A renderer-independent transcript model. The interactive mode owns the
/// terminal event loop and uses this type for message storage and viewport
/// state.
pub struct TranscriptRenderer {
    /// Stored messages in the transcript.
    messages: Vec<Message>,
    /// Whether to automatically follow new messages.
    follow_mode: bool,
    /// Number of lines to remain above the newest content while unfollowed.
    scroll_offset: usize,
    /// Whether to display timestamps on messages.
    show_timestamps: bool,
    /// Whether to display thinking blocks.
    show_thinking: bool,
}

/// A message in the transcript.
#[derive(Debug, Clone)]
pub struct Message {
    /// The role of the message sender.
    pub role: MessageRole,
    /// User-visible answer text.
    pub content: String,
    /// Model reasoning kept separately so it can be toggled without changing
    /// the answer or the persisted assistant message.
    pub thinking: String,
    /// Tool calls made by the assistant.
    pub tool_calls: Vec<ToolCall>,
    /// When this message was created.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// The role of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    /// A user-submitted message.
    User,
    /// An assistant-generated response.
    Assistant,
    /// A tool call result.
    Tool,
    /// A system-level instruction.
    System,
}

/// A tool call in a message.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Unique identifier for this tool call.
    pub id: String,
    /// The name of the tool being called.
    pub name: String,
    /// JSON-encoded arguments passed to the tool.
    pub arguments: String,
    /// Optional result returned by the tool execution.
    pub result: Option<String>,
}

impl ToolCall {
    /// Return a human-readable representation of the tool call.
    ///
    /// Instead of showing raw JSON arguments, this formats the call based on
    /// the tool name so users can quickly understand what action was taken.
    #[allow(dead_code)]
    pub fn human_readable(&self) -> String {
        let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());

        self.human_readable_with_cwd(&cwd)
    }

    /// Return a human-readable representation using `cwd` as the project
    /// directory for path shortening.
    pub fn human_readable_with_cwd(
        &self,
        cwd: &Path,
    ) -> String {
        let args = serde_json::from_str::<serde_json::Value>(&self.arguments).ok();
        let Some(args) = args.as_ref() else {
            return self.arguments.clone();
        };

        match self.name.as_str() {
            "bash" => {
                // Show the command directly
                if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                    format!("bash {cmd}")
                } else {
                    self.arguments.clone()
                }
            },

            "read" | "write" | "edit" => {
                // Show tool name + path
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    format!("{} {}", self.name, display_tool_path(path, cwd))
                } else {
                    self.arguments.clone()
                }
            },

            "find" => {
                // Show tool name + pattern (and optional path)
                if let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) {
                    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                        format!("find {} {}", pattern, display_tool_path(path, cwd))
                    } else {
                        format!("find {}", pattern)
                    }
                } else {
                    self.arguments.clone()
                }
            },

            "grep" => {
                // Show grep pattern + optional path
                if let Some(pattern) = args.get("pattern").and_then(|v| v.as_str()) {
                    if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                        format!("grep {} {}", pattern, display_tool_path(path, cwd))
                    } else {
                        format!("grep {}", pattern)
                    }
                } else {
                    self.arguments.clone()
                }
            },

            "ls" => {
                // Show ls + optional path
                if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                    format!("ls {}", display_tool_path(path, cwd))
                } else {
                    "ls .".to_string()
                }
            },

            _ => self.arguments.clone(),
        }
    }
}

/// Format a tool path for display, preserving trailing slashes or backslashes.
fn display_tool_path(
    path: &str,
    cwd: &Path,
) -> String {
    let displayed = hallucinate_agent::config::paths::display_path(path, cwd);

    if (path.ends_with('/') || path.ends_with('\\'))
        && displayed != "."
        && !displayed.ends_with('/')
        && !displayed.ends_with('\\')
    {
        format!("{displayed}{}", &path[path.len() - 1..])
    } else {
        displayed
    }
}

impl TranscriptRenderer {
    /// Create a transcript. Timestamps are opt-in through the environment so
    /// the default display remains compact.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            follow_mode: true,
            scroll_offset: 0,
            show_timestamps: std::env::var_os("HALLUCINATE_SHOW_TIMESTAMPS").is_some(),
            show_thinking: false,
        }
    }

    /// Add a message to the transcript.
    pub fn add_message(
        &mut self,
        message: Message,
    ) {
        self.messages.push(message);
    }

    /// Scroll toward older messages and leave follow mode.
    pub fn scroll_up(
        &mut self,
        lines: usize,
    ) {
        self.follow_mode = false;
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
    }

    /// Scroll toward newer messages. Reaching the bottom restores follow mode.
    pub fn scroll_down(
        &mut self,
        lines: usize,
    ) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);

        if self.scroll_offset == 0 {
            self.follow_mode = true;
        }
    }

    /// Compute the vertical scroll position for the given viewport.
    pub fn scroll_position(
        &self,
        total_lines: usize,
        viewport_lines: usize,
    ) -> u16 {
        let bottom = total_lines.saturating_sub(viewport_lines);

        let position = if self.follow_mode {
            bottom
        } else {
            bottom.saturating_sub(self.scroll_offset)
        };

        position.min(u16::MAX as usize) as u16
    }

    /// Whether timestamps are enabled for display.
    pub fn show_timestamps(&self) -> bool {
        self.show_timestamps
    }

    /// Toggle whether thinking blocks are displayed.
    pub fn set_show_thinking(
        &mut self,
        show: bool,
    ) {
        self.show_thinking = show;
    }

    /// Enter follow mode and reset the scroll offset to the bottom.
    pub fn follow(&mut self) {
        self.follow_mode = true;
        self.scroll_offset = 0;
    }

    /// Append text to an existing message's content by index.
    pub fn append_to_message(
        &mut self,
        index: usize,
        content: &str,
    ) {
        if let Some(message) = self.messages.get_mut(index) {
            message.content.push_str(content);
        }
    }

    /// Append reasoning text to an existing message's thinking block by index.
    pub fn append_thinking_to_message(
        &mut self,
        index: usize,
        thinking: &str,
    ) {
        if let Some(message) = self.messages.get_mut(index) {
            message.thinking.push_str(thinking);
        }
    }

    /// Finalize a message with its full content, thinking block, and tool calls.
    pub fn complete_message(
        &mut self,
        index: usize,
        content: String,
        thinking: String,
        tool_calls: Vec<ToolCall>,
    ) {
        if let Some(message) = self.messages.get_mut(index) {
            message.content = content;
            message.thinking = thinking;
            message.tool_calls = tool_calls;
        }
    }

    /// Attach a tool result to the original assistant tool call. This keeps
    /// tool progress beside the request that caused it instead of adding a
    /// second unrelated transcript entry.
    pub fn complete_tool_call(
        &mut self,
        tool_call_id: &str,
        result: String,
        is_error: bool,
    ) -> bool {
        for message in self.messages.iter_mut().rev() {
            if let Some(call) = message
                .tool_calls
                .iter_mut()
                .find(|call| call.id == tool_call_id)
            {
                let prefix = if is_error { "❌" } else { "✅" };
                call.result = Some(format!("{prefix} {result}"));

                return true;
            }
        }

        false
    }

    /// Access the stored messages.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}

impl Default for TranscriptRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
/// Unit tests for transcript rendering and tool call formatting.
mod tests {
    use super::*;

    #[test]
    fn bash_tool_shows_command_readable() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command": "echo hello world"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "bash echo hello world");
    }

    #[test]
    fn bash_tool_with_complex_command() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command": "grep -r 'foo' src/ | wc -l"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "bash grep -r 'foo' src/ | wc -l");
    }

    #[test]
    fn read_tool_shows_path() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "read".to_string(),
            arguments: r#"{"path": "/path/to/file.rs"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "read /path/to/file.rs");
    }

    #[test]
    fn write_tool_shows_path() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "write".to_string(),
            arguments: r#"{"path": "/path/to/file.rs", "content": "hello"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "write /path/to/file.rs");
    }

    #[test]
    fn edit_tool_shows_path() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "edit".to_string(),
            arguments: r#"{"path": "/path/to/file.rs", "old_text": "foo", "new_text": "bar"}"#
                .to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "edit /path/to/file.rs");
    }

    #[test]
    fn find_tool_shows_pattern() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "find".to_string(),
            arguments: r#"{"pattern": "**/*.rs"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "find **/*.rs");
    }

    #[test]
    fn find_tool_with_path() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "find".to_string(),
            arguments: r#"{"pattern": "*.toml", "path": "src"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "find *.toml src");
    }

    #[test]
    fn grep_tool_shows_pattern() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "grep".to_string(),
            arguments: r#"{"pattern": "hello"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "grep hello");
    }

    #[test]
    fn grep_tool_with_path() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "grep".to_string(),
            arguments: r#"{"pattern": "foo", "path": "src/"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "grep foo src/");
    }

    #[test]
    fn ls_tool_shows_path() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "ls".to_string(),
            arguments: r#"{"path": "/some/dir"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "ls /some/dir");
    }

    #[test]
    fn ls_tool_defaults_to_current_dir() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "ls".to_string(),
            arguments: r#"{}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "ls .");
    }

    #[test]
    fn unknown_tool_falls_back_to_raw_json() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "custom_tool".to_string(),
            arguments: r#"{"foo": "bar"}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), r#"{"foo": "bar"}"#);
    }

    #[test]
    fn invalid_json_falls_back_to_raw_string() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            arguments: "not valid json".to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), "not valid json");
    }

    #[test]
    fn bash_tool_missing_command_falls_back() {
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"timeout_ms": 5000}"#.to_string(),
            result: None,
        };

        assert_eq!(call.human_readable(), r#"{"timeout_ms": 5000}"#);
    }

    #[test]
    fn tool_results_update_the_original_call() {
        let mut transcript = TranscriptRenderer::new();
        transcript.add_message(Message {
            role: MessageRole::Assistant,
            content: String::new(),
            thinking: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: None,
            }],
            timestamp: chrono::Utc::now(),
        });

        assert!(transcript.complete_tool_call("call-1", "ok".to_string(), false));
        assert_eq!(
            transcript.messages()[0].tool_calls[0].result.as_deref(),
            Some("✅ ok")
        );
        assert!(!transcript.complete_tool_call("missing", "nope".to_string(), true));
    }

    #[test]
    fn thinking_can_be_collected_while_hidden_and_shown_later() {
        let mut transcript = TranscriptRenderer::new();
        transcript.add_message(Message {
            role: MessageRole::Assistant,
            content: String::new(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });
        transcript.append_thinking_to_message(0, "first");
        transcript.set_show_thinking(true);
        transcript.append_thinking_to_message(0, " second");

        assert_eq!(transcript.messages()[0].thinking, "first second");
    }

    #[test]
    fn scroll_position_clamps_to_the_available_history() {
        let mut transcript = TranscriptRenderer::new();
        transcript.scroll_up(100);

        assert_eq!(transcript.scroll_position(20, 5), 0);
        transcript.scroll_down(100);
    }
}
