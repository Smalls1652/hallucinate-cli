//! Presentation of streamed assistant text and tool lifecycle events.

use std::{collections::HashMap, io::Write};

use hallucinate_agent::{agent::types::AgentEvent, ai::types::AssistantContent};
use hallucinate_tui::components::markdown_to_ansi;
use serde_json::Value;

/// Render completed Markdown blocks on terminals and preserve raw redirected
/// output.
pub(super) struct StreamOutput<'a, O: Write, D: Write> {
    output: &'a mut O,
    diagnostics: &'a mut D,
    render_markdown: bool,
    color_tools: bool,
    label_tool_calls: bool,
    pending_markdown: String,
    tool_arguments: HashMap<String, Value>,
}

impl<'a, O: Write, D: Write> StreamOutput<'a, O, D> {
    pub(super) fn new(
        output: &'a mut O,
        diagnostics: &'a mut D,
        render_markdown: bool,
        color_tools: bool,
        label_tool_calls: bool,
    ) -> Self {
        Self {
            output,
            diagnostics,
            render_markdown,
            color_tools,
            label_tool_calls,
            pending_markdown: String::new(),
            tool_arguments: HashMap::new(),
        }
    }

    pub(super) fn handle(
        &mut self,
        event: AgentEvent,
    ) -> std::io::Result<()> {
        match event {
            AgentEvent::AssistantTextDelta { delta } => {
                if self.render_markdown {
                    self.pending_markdown.push_str(&delta);
                    self.flush_complete_blocks()?;
                } else {
                    self.output.write_all(delta.as_bytes())?;
                    self.output.flush()?;
                }
            },

            AgentEvent::AssistantMessageCompleted { message, .. } => {
                self.flush_markdown()?;

                // The completed message is authoritative even if a provider
                // omitted a ToolCallDetected event.
                for content in &message.content {
                    if let AssistantContent::ToolCall(call) = content {
                        self.tool_arguments
                            .insert(call.id.clone(), call.arguments.clone());
                    }
                }
            },

            AgentEvent::ToolCallDetected {
                tool_call_id,
                arguments,
                ..
            } => {
                self.tool_arguments.insert(tool_call_id, arguments);
            },

            AgentEvent::ToolExecutionCompleted {
                tool_call_id,
                tool_name,
                is_error,
                ..
            } => self.render_tool(&tool_call_id, &tool_name, !is_error)?,

            AgentEvent::ToolExecutionFailed {
                tool_call_id,
                tool_name,
                ..
            } => self.render_tool(&tool_call_id, &tool_name, false)?,

            _ => {},
        }
        Ok(())
    }

    fn render_tool(
        &mut self,
        tool_call_id: &str,
        tool_name: &str,
        succeeded: bool,
    ) -> std::io::Result<()> {
        let arguments = self.tool_arguments.remove(tool_call_id);
        let name = safe_label(tool_name);

        let label = arguments
            .as_ref()
            .filter(|_| self.label_tool_calls)
            .map(|arguments| {
                arguments
                    .get(hallucinate_agent::ai::types::TOOL_CALL_LABEL)
                    .and_then(Value::as_str)
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or("Tool call")
            });

        if self.color_tools {
            write!(self.diagnostics, "\x1b[1;36m[{name}]\x1b[0m")?;
        } else {
            write!(self.diagnostics, "[{name}]")?;
        }

        if let Some(label) = label {
            write!(self.diagnostics, " {}", safe_input(label))?;
        }

        writeln!(self.diagnostics)?;

        if let Some(arguments) = arguments {
            let input = if self.label_tool_calls {
                if tool_name == "shell" {
                    arguments
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                } else {
                    String::new()
                }
            } else {
                tool_input(tool_name, &arguments)
            };

            for line in input.lines() {
                if self.color_tools {
                    writeln!(self.diagnostics, "  \x1b[2m{}\x1b[0m", safe_input(line))?;
                } else {
                    writeln!(self.diagnostics, "  {}", safe_input(line))?;
                }
            }
        }

        let result = if succeeded { "✅" } else { "❌" };
        if self.color_tools {
            let color = if succeeded { "32" } else { "31" };

            writeln!(
                self.diagnostics,
                "  \x1b[1mTool result:\x1b[0m \x1b[{color}m{result}\x1b[0m\n"
            )?;
        } else {
            writeln!(self.diagnostics, "  Tool result: {result}\n")?;
        }

        self.diagnostics.flush()
    }

    pub(super) fn finish(&mut self) -> std::io::Result<()> {
        self.flush_markdown()
    }

    fn flush_complete_blocks(&mut self) -> std::io::Result<()> {
        if let Some(end) = complete_block_end(&self.pending_markdown) {
            let remainder = self.pending_markdown.split_off(end);
            let complete = std::mem::replace(&mut self.pending_markdown, remainder);
            self.write_markdown(&complete)?;

            // A blank line separates blocks that may be parsed independently.
            self.output.write_all(b"\n")?;
            self.output.flush()?;
        }

        Ok(())
    }

    fn flush_markdown(&mut self) -> std::io::Result<()> {
        if !self.pending_markdown.is_empty() {
            let markdown = std::mem::take(&mut self.pending_markdown);
            self.write_markdown(&markdown)?;
        }

        Ok(())
    }

    fn write_markdown(
        &mut self,
        markdown: &str,
    ) -> std::io::Result<()> {
        self.output
            .write_all(markdown_to_ansi(markdown).as_bytes())?;

        self.output.flush()
    }
}

/// Find blank-line block boundaries while keeping fenced code blocks intact.
fn complete_block_end(markdown: &str) -> Option<usize> {
    let mut fence: Option<(char, usize)> = None;
    let mut offset = 0;
    let mut complete = None;

    for line in markdown.split_inclusive('\n') {
        if !line.ends_with('\n') {
            break;
        }
        offset += line.len();

        let text = line.trim_end_matches(['\r', '\n']);
        let trimmed = text.trim_start_matches(' ');
        if text.len() - trimmed.len() <= 3 {
            if let Some((marker, length)) = fence {
                let count = trimmed
                    .chars()
                    .take_while(|character| *character == marker)
                    .count();

                if count >= length && trimmed[count..].trim().is_empty() {
                    fence = None;
                }
            } else if let Some(marker @ ('`' | '~')) = trimmed.chars().next() {
                let count = trimmed
                    .chars()
                    .take_while(|character| *character == marker)
                    .count();

                if count >= 3 {
                    fence = Some((marker, count));
                }
            }
        }

        if fence.is_none() && text.trim().is_empty() {
            complete = Some(offset);
        }
    }

    complete
}

/// Display shell commands directly while retaining any additional arguments.
fn tool_input(
    tool_name: &str,
    arguments: &Value,
) -> String {
    if tool_name == "shell"
        && let Some(object) = arguments.as_object()
        && let Some(command) = object.get("command").and_then(Value::as_str)
    {
        let mut input = command.to_string();
        let mut extra = object.clone();
        extra.remove("command");

        if !extra.is_empty() {
            input.push('\n');
            input.push_str(&serde_json::to_string_pretty(&extra).unwrap_or_default());
        }

        return input;
    }

    serde_json::to_string_pretty(arguments).unwrap_or_default()
}

/// Escape control characters from untrusted tool names and inputs.
fn safe_label(value: &str) -> String {
    value.chars().flat_map(char::escape_default).collect()
}

fn safe_input(value: &str) -> String {
    let mut escaped = String::new();

    for character in value.chars() {
        if character.is_control() {
            escaped.extend(character.escape_default());
        } else {
            escaped.push(character);
        }
    }

    escaped
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn displays_concurrent_tool_calls_without_outputs_or_reasoning() {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let mut display = StreamOutput::new(&mut output, &mut diagnostics, false, false, false);

        for (id, command) in [("a", "first"), ("b", "second")] {
            display
                .handle(AgentEvent::ToolCallDetected {
                    tool_call_id: id.into(),
                    tool_name: "shell".into(),
                    arguments: json!({"command": command}),
                })
                .unwrap();

            display
                .handle(AgentEvent::ToolExecutionStarted {
                    tool_call_id: id.into(),
                    tool_name: "shell".into(),
                })
                .unwrap();
        }

        display
            .handle(AgentEvent::ToolExecutionOutput {
                tool_call_id: "a".into(),
                tool_name: "shell".into(),
                output: "secret-output".into(),
            })
            .unwrap();

        display
            .handle(AgentEvent::AssistantThinkingDelta {
                delta: "secret-thinking".into(),
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolExecutionFailed {
                tool_call_id: "b".into(),
                tool_name: "shell".into(),
                error: "secret-error-output".into(),
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolExecutionCompleted {
                tool_call_id: "a".into(),
                tool_name: "shell".into(),
                result: "secret-result".into(),
                is_error: false,
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolExecutionCompleted {
                tool_call_id: "c".into(),
                tool_name: "read".into(),
                result: "secret-result".into(),
                is_error: true,
            })
            .unwrap();

        display.finish().unwrap();

        assert!(output.is_empty());

        let diagnostics = String::from_utf8(diagnostics).unwrap();
        assert!(diagnostics.contains("[shell]\n  first\n  Tool result: ✅\n\n"));
        assert!(diagnostics.contains("[shell]\n  second\n  Tool result: ❌\n\n"));
        assert!(diagnostics.contains("[read]\n  Tool result: ❌\n\n"));
        assert!(!diagnostics.contains("[tool"));
        assert!(!diagnostics.contains("secret"));
    }

    #[test]
    fn labeled_tool_calls_hide_arguments_except_shell_command() {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let mut display = StreamOutput::new(&mut output, &mut diagnostics, false, false, true);

        for (id, name, args) in [
            (
                "1",
                "mcp_search",
                json!({"__hallucinate_tool_call_label": "Search issues", "query": "secret"}),
            ),
            (
                "2",
                "shell",
                json!({"__hallucinate_tool_call_label": "Run tests", "command": "cargo test", "timeout": 123}),
            ),
            ("3", "read", json!({"path": "private"})),
        ] {
            display
                .handle(AgentEvent::ToolCallDetected {
                    tool_call_id: id.into(),
                    tool_name: name.into(),
                    arguments: args,
                })
                .unwrap();

            display
                .handle(AgentEvent::ToolExecutionCompleted {
                    tool_call_id: id.into(),
                    tool_name: name.into(),
                    result: String::new(),
                    is_error: false,
                })
                .unwrap();
        }

        let diagnostics = String::from_utf8(diagnostics).unwrap();
        assert!(diagnostics.contains("[mcp_search] Search issues\n  Tool result: ✅\n\n"));
        assert!(diagnostics.contains("[shell] Run tests\n  cargo test\n  Tool result: ✅\n\n"));
        assert!(diagnostics.contains("[read] Tool call\n  Tool result: ✅\n\n"));
        assert!(!diagnostics.contains("secret"));
        assert!(!diagnostics.contains("123"));
        assert!(!diagnostics.contains("private"));
    }

    #[test]
    fn colored_labels_escape_control_characters_on_the_header_line() {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let mut display = StreamOutput::new(&mut output, &mut diagnostics, false, true, true);

        display
            .handle(AgentEvent::ToolCallDetected {
                tool_call_id: "one".into(),
                tool_name: "shell".into(),
                arguments: json!({"__hallucinate_tool_call_label": "Echo\n\u{1b}[31mhello", "command": "echo hello"}),
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolExecutionCompleted {
                tool_call_id: "one".into(),
                tool_name: "shell".into(),
                result: String::new(),
                is_error: false,
            })
            .unwrap();

        let diagnostics = String::from_utf8(diagnostics).unwrap();
        assert!(diagnostics.starts_with(
            "\x1b[1;36m[shell]\x1b[0m Echo\\n\\u{1b}[31mhello\n  \x1b[2mecho hello\x1b[0m\n"
        ));
    }

    #[test]
    fn tool_inputs_are_readable_and_color_is_optional() {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let mut display = StreamOutput::new(&mut output, &mut diagnostics, false, true, false);

        display
            .handle(AgentEvent::ToolCallDetected {
                tool_call_id: "one".into(),
                tool_name: "shell".into(),
                arguments: json!({"command": "echo hello\nprintf world\u{1b}[31m", "timeout": 5}),
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolExecutionCompleted {
                tool_call_id: "one".into(),
                tool_name: "shell".into(),
                result: "private output".into(),
                is_error: false,
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolCallDetected {
                tool_call_id: "two".into(),
                tool_name: "read".into(),
                arguments: json!({"path": "a.txt"}),
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolExecutionFailed {
                tool_call_id: "two".into(),
                tool_name: "read".into(),
                error: "private error".into(),
            })
            .unwrap();

        let diagnostics = String::from_utf8(diagnostics).unwrap();
        assert!(diagnostics.contains("\x1b[1;36m[shell]\x1b[0m"));
        assert!(diagnostics.contains("echo hello"));
        assert!(diagnostics.contains("printf world\\u{1b}[31m"));
        assert!(diagnostics.contains("\"timeout\": 5"));
        assert!(diagnostics.contains("\"path\": \"a.txt\""));
        assert!(diagnostics.contains("\x1b[32m✅"));
        assert!(diagnostics.contains("\x1b[31m❌"));
        assert!(!diagnostics.contains("private"));
        assert!(!diagnostics.contains("[tool"));
        assert!(diagnostics.ends_with("\x1b[0m\n\n"));
    }

    #[test]
    fn separates_tool_blocks_from_surrounding_terminal_responses() {
        use std::{cell::RefCell, rc::Rc};

        struct SharedOutput(Rc<RefCell<Vec<u8>>>);
        impl Write for SharedOutput {
            fn write(
                &mut self,
                bytes: &[u8],
            ) -> std::io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let shared = Rc::new(RefCell::new(Vec::new()));

        let mut output = SharedOutput(shared.clone());
        let mut diagnostics = SharedOutput(shared.clone());
        let mut display = StreamOutput::new(&mut output, &mut diagnostics, true, false, false);

        display
            .handle(AgentEvent::AssistantTextDelta {
                delta: "Before".into(),
            })
            .unwrap();

        display
            .handle(AgentEvent::AssistantMessageCompleted {
                turn_number: 1,
                message: Box::new(hallucinate_agent::ai::types::AssistantMessage::new(
                    vec![],
                    hallucinate_agent::ai::types::Api::OpenaiCompletions,
                    "test",
                    "test",
                    hallucinate_agent::ai::types::StopReason::ToolUse,
                )),
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolCallDetected {
                tool_call_id: "1".into(),
                tool_name: "shell".into(),
                arguments: json!({"command": "echo hello"}),
            })
            .unwrap();

        display
            .handle(AgentEvent::ToolExecutionCompleted {
                tool_call_id: "1".into(),
                tool_name: "shell".into(),
                result: "hidden".into(),
                is_error: false,
            })
            .unwrap();

        display
            .handle(AgentEvent::AssistantTextDelta {
                delta: "After".into(),
            })
            .unwrap();

        display.finish().unwrap();

        let result = String::from_utf8(shared.borrow().clone()).unwrap();
        assert_eq!(
            result,
            "Before\n[shell]\n  echo hello\n  Tool result: ✅\n\nAfter\n"
        );
    }

    #[test]
    fn renders_split_markdown_blocks_and_flushes_partial_response() {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let mut display = StreamOutput::new(&mut output, &mut diagnostics, true, false, false);

        display
            .handle(AgentEvent::AssistantTextDelta {
                delta: "**bo".into(),
            })
            .unwrap();

        display
            .handle(AgentEvent::AssistantTextDelta {
                delta: "ld**\n\n```rust\nlet x = 1;\n\n".into(),
            })
            .unwrap();

        assert!(display.pending_markdown.starts_with("```rust"));

        display
            .handle(AgentEvent::AssistantTextDelta {
                delta: "let y = 2;\n```\n\n- one\n- **two**".into(),
            })
            .unwrap();

        display.finish().unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("\x1b[1m"));
        assert!(rendered.contains("bold"));
        assert!(!rendered.contains("**"));
        assert!(!rendered.contains("```"));
        assert!(rendered.contains("let x = 1;"));
        assert!(rendered.contains("let y = 2;"));
        assert!(rendered.contains("• "));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn redirected_output_is_immediate_and_preserves_markdown() {
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        let mut display = StreamOutput::new(&mut output, &mut diagnostics, false, false, false);

        display
            .handle(AgentEvent::AssistantTextDelta {
                delta: "**unfinished".into(),
            })
            .unwrap();

        assert!(display.pending_markdown.is_empty());

        display.finish().unwrap();

        assert_eq!(output, b"**unfinished");
    }
}
