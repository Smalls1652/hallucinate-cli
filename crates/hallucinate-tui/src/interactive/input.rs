//! Keyboard, mouse, editor, and response-task interaction.

use std::{
    io::{self, Write},
    time::Duration,
};

#[cfg(not(target_os = "macos"))]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hallucinate_agent::{ai::types::Message, config::settings::SessionTitleGenerationMode};
use ratatui::crossterm;

use super::{AgentRunCompletion, AutocompleteItem, ChatSelection, InteractiveTui};
use crate::{
    alt_screen::{Message as TuiMessage, MessageRole},
    input::parse_event,
    keys::Key,
};

impl InteractiveTui {
    /// Clear the current chat selection state.
    fn clear_chat_selection(&mut self) {
        self.chat_selection = None;
        self.selected_text.clear();
    }

    /// Return the largest scroll offset represented by the rendered viewport.
    fn transcript_scroll_limit(&self) -> usize {
        self.transcript_layout_cache
            .line_count()
            .saturating_sub(usize::from(self.transcript_area.height))
    }

    /// Map a mouse column/row to a transcript coordinate, or `None` if the
    /// point lies outside the transcript area.
    fn chat_point(
        &self,
        column: u16,
        row: u16,
    ) -> Option<(u16, u16)> {
        let area = self.transcript_area;
        let right = area.x.saturating_add(area.width);
        let bottom = area.y.saturating_add(area.height);

        if column >= area.x && column < right && row >= area.y && row < bottom {
            Some((column, row))
        } else {
            None
        }
    }

    /// Copy the currently selected transcript text to the host clipboard.
    pub(super) fn copy_chat_selection(&mut self) {
        if self.selected_text.is_empty() {
            self.show_notice("Nothing selected");

            return;
        }

        let result = copy_to_clipboard(&self.selected_text);
        if result.is_ok() {
            self.show_notice("Copied selection to clipboard");
        } else {
            self.show_notice("Could not copy selection");
        }
    }

    /// Refresh slash-command completions for the current editor contents.
    pub(super) fn refresh_autocomplete(&mut self) {
        let input = self.editor.content().trim_start();

        let Some(prefix) = input.strip_prefix('/') else {
            self.autocomplete_items.clear();
            self.autocomplete_index = 0;

            return;
        };

        // Only complete the command name. Arguments have their own syntax and
        // should continue to be edited normally.
        if prefix.chars().any(char::is_whitespace) {
            self.autocomplete_items.clear();
            self.autocomplete_index = 0;

            return;
        }

        let prefix = prefix.to_ascii_lowercase();
        let mut items = Vec::new();

        for command in crate::slash_commands::BUILTIN_COMMANDS {
            if command.name.starts_with("skill:") {
                continue;
            }

            if command.name.starts_with(&prefix) {
                items.push(AutocompleteItem {
                    value: command.name.to_string(),
                    description: command.description.to_string(),
                });
            }
        }
        for template in &self.prompt_templates {
            if template.name.to_ascii_lowercase().starts_with(&prefix)
                && !items.iter().any(|item| item.value == template.name)
            {
                items.push(AutocompleteItem {
                    value: template.name.clone(),
                    description: template.description.clone(),
                });
            }
        }
        for skill in &self.skills {
            let value = format!("skill:{}", skill.name);

            if value.starts_with(&prefix) && !items.iter().any(|item| item.value == value) {
                items.push(AutocompleteItem {
                    value,
                    description: skill.description.clone(),
                });
            }
        }

        items.sort_by(|left, right| left.value.cmp(&right.value));

        self.autocomplete_items = items;
        self.autocomplete_index = self
            .autocomplete_index
            .min(self.autocomplete_items.len().saturating_sub(1));
    }

    /// Insert the selected completion into the editor.
    fn accept_autocomplete(&mut self) {
        let Some(item) = self.autocomplete_items.get(self.autocomplete_index) else {
            return;
        };

        self.editor.set_content(format!("/{}", item.value));

        self.refresh_autocomplete();
    }

    /// Toggle whether completed tool results display their full available
    /// output or a compact one-line preview.
    fn toggle_full_tool_output(&mut self) {
        self.show_full_tool_output = !self.show_full_tool_output;

        self.clear_chat_selection();

        self.render_state.transcript = true;

        self.show_notice(if self.show_full_tool_output {
            "Showing full tool output."
        } else {
            "Showing compact tool output."
        });
    }

    /// Handle user input.
    pub(super) fn handle_input(
        &mut self,
        timeout: Duration,
    ) -> Result<bool, anyhow::Error> {
        // Polling is event-driven from the renderer's perspective: this may
        // block for a full notice lifetime while idle, and only uses a short
        // bound while an agent stream needs draining.
        if crossterm::event::poll(timeout)? {
            let event = crossterm::event::read()?;
            let input_event = parse_event(event);

            let is_transcript_scroll = self.model_picker.is_none()
                && self.session_picker.is_none()
                && self.prompt_picker.is_none()
                && self.clear_sessions_confirmation.is_none()
                && matches!(
                    &input_event,
                    crate::input::InputEvent::Mouse(mouse)
                        if matches!(
                            mouse.kind,
                            crossterm::event::MouseEventKind::ScrollUp
                                | crossterm::event::MouseEventKind::ScrollDown
                        )
                );

            // Trackpad momentum can queue a large number of wheel events at
            // a transcript boundary. Those no-op events must be drained
            // without scheduling full terminal redraws, or the visible
            // viewport appears to lock until the queue empties.
            if !is_transcript_scroll {
                self.render_state.mark_all();
            }

            // Esc cancels the active agent task instead of being passed to the
            // editor. Keep the editor contents intact so a draft can be sent
            // after the response has stopped; when idle, Esc retains its
            // normal editor-cancel behavior below.
            if matches!(
                &input_event,
                crate::input::InputEvent::Key(Key {
                    code: crossterm::event::KeyCode::Esc,
                    ..
                })
            ) && self.agent_cancellation.is_some()
            {
                self.cancel_running_response();

                return Ok(false);
            }

            let cancel_edit = matches!(
                &input_event,
                crate::input::InputEvent::Key(Key {
                    code: crossterm::event::KeyCode::Esc,
                    ..
                })
            );

            match &input_event {
                crate::input::InputEvent::Resize(width, height) => {
                    // Ratatui recalculates its viewport on the next draw; read
                    // both dimensions so resize events are not discarded.
                    let _ = (width, height);
                    self.clear_chat_selection();
                    self.terminal.autoresize()?;

                    return Ok(false);
                },

                crate::input::InputEvent::Mouse(mouse) => {
                    if let Some(picker) = self.model_picker.as_mut() {
                        match mouse.kind {
                            crossterm::event::MouseEventKind::ScrollUp => {
                                picker.selected = picker.selected.saturating_sub(3);
                            },

                            crossterm::event::MouseEventKind::ScrollDown => {
                                picker.selected = (picker.selected + 3)
                                    .min(picker.filtered_indices.len().saturating_sub(1));
                            },

                            _ => {},
                        }

                        return Ok(false);
                    }

                    if let Some(picker) = self.session_picker.as_mut() {
                        match mouse.kind {
                            crossterm::event::MouseEventKind::ScrollUp => {
                                picker.selected = picker.selected.saturating_sub(3);
                            },

                            crossterm::event::MouseEventKind::ScrollDown => {
                                picker.selected = (picker.selected + 3)
                                    .min(picker.filtered_indices.len().saturating_sub(1));
                            },

                            _ => {},
                        }

                        return Ok(false);
                    }

                    if let Some(picker) = self.prompt_picker.as_mut() {
                        match mouse.kind {
                            crossterm::event::MouseEventKind::ScrollUp => {
                                picker.selected = picker.selected.saturating_sub(3);
                            },

                            crossterm::event::MouseEventKind::ScrollDown => {
                                picker.selected = (picker.selected + 3)
                                    .min(picker.filtered_indices.len().saturating_sub(1));
                            },

                            _ => {},
                        }

                        return Ok(false);
                    }

                    use crossterm::event::{MouseButton, MouseEventKind};

                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if let Some(point) = self.chat_point(mouse.column, mouse.row) {
                                self.chat_selection = Some(ChatSelection {
                                    anchor: point,
                                    focus: point,
                                });

                                self.selected_text.clear();
                            } else {
                                self.clear_chat_selection();
                            }
                        },

                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Some(point) = self.chat_point(mouse.column, mouse.row)
                                && let Some(selection) = self.chat_selection.as_mut()
                            {
                                selection.focus = point;
                            }
                        },

                        MouseEventKind::Up(MouseButton::Left) => {
                            if let Some(point) = self.chat_point(mouse.column, mouse.row)
                                && let Some(selection) = self.chat_selection.as_mut()
                            {
                                selection.focus = point;
                            }

                            if self
                                .chat_selection
                                .is_some_and(|selection| !selection.has_range())
                            {
                                self.clear_chat_selection();
                            } else {
                                // Terminal.app and some other macOS terminals
                                // consume Cmd+C instead of forwarding it once
                                // mouse reporting is enabled. Copy on release
                                // so the constrained application selection
                                // still reaches the host clipboard.
                                #[cfg(target_os = "macos")]
                                {
                                    self.copy_selection_on_render = true;
                                }
                            }
                        },

                        MouseEventKind::ScrollUp => {
                            let selection_cleared = self.chat_selection.is_some();
                            self.clear_chat_selection();

                            let scroll_limit = self.transcript_scroll_limit();
                            let scroll_changed = self.transcript.scroll_up(3, scroll_limit);

                            if selection_cleared || scroll_changed {
                                self.render_state.transcript = true;
                                self.render_state.selection = true;
                            }
                        },

                        MouseEventKind::ScrollDown => {
                            let selection_cleared = self.chat_selection.is_some();
                            self.clear_chat_selection();
                            let scroll_changed = self.transcript.scroll_down(3);

                            if selection_cleared || scroll_changed {
                                self.render_state.transcript = true;
                                self.render_state.selection = true;
                            }
                        },

                        _ => {},
                    }

                    return Ok(false);
                },

                _ => {},
            }

            // The model picker owns keyboard input while it is open. This
            // prevents arrows and Enter from moving/submitting the editor.
            if self.model_picker.is_some()
                || self.session_picker.is_some()
                || self.prompt_picker.is_some()
                || self.clear_sessions_confirmation.is_some()
            {
                if let crate::input::InputEvent::Key(key) = input_event {
                    if self.clear_sessions_confirmation.is_some() {
                        match key.code {
                            crossterm::event::KeyCode::Enter
                            | crossterm::event::KeyCode::Char('y')
                            | crossterm::event::KeyCode::Char('Y') => {
                                self.handle_clear_sessions_confirmation(true)?;
                            },

                            crossterm::event::KeyCode::Esc
                            | crossterm::event::KeyCode::Char('n')
                            | crossterm::event::KeyCode::Char('N') => {
                                self.handle_clear_sessions_confirmation(false)?;
                            },

                            _ => {},
                        }
                    } else if self.model_picker.is_some() {
                        self.handle_model_picker_key(key)?;
                    } else if self.session_picker.is_some() {
                        self.handle_session_picker_key(key)?;
                    } else {
                        self.handle_prompt_picker_key(key)?;
                    }
                }

                return Ok(false);
            }

            // Handle key events.
            if let crate::input::InputEvent::Key(key) = input_event {
                // Tab accepts the selected slash-command completion. Enter
                // accepts it and continues through the normal submit path so
                // the selected command is run immediately. Shift+Enter and
                // Alt+Enter remain available for inserting a newline.
                if !key.ctrl
                    && !key.alt
                    && matches!(key.code, crossterm::event::KeyCode::Tab)
                    && !self.autocomplete_items.is_empty()
                {
                    self.accept_autocomplete();

                    return Ok(false);
                }

                if !key.shift
                    && !key.alt
                    && matches!(key.code, crossterm::event::KeyCode::Enter)
                    && !self.autocomplete_items.is_empty()
                {
                    self.accept_autocomplete();
                }

                if !self.autocomplete_items.is_empty()
                    && matches!(key.code, crossterm::event::KeyCode::Up)
                {
                    self.autocomplete_index = self.autocomplete_index.saturating_sub(1);

                    return Ok(false);
                }

                if !self.autocomplete_items.is_empty()
                    && matches!(key.code, crossterm::event::KeyCode::Down)
                {
                    self.autocomplete_index = (self.autocomplete_index + 1)
                        .min(self.autocomplete_items.len().saturating_sub(1));

                    return Ok(false);
                }

                // Toggle tool output. (Alt + O)
                if key.alt && (key.matches('o') || key.matches('O')) {
                    self.toggle_full_tool_output();

                    return Ok(false);
                }

                // Create a new session. (Shift + Alt + N)
                if key.shift && key.alt && (key.matches('n') || key.matches('N')) {
                    self.create_empty_session(None, false)?;

                    return Ok(false);
                }

                // Show prompt picker. (Alt + P)
                if key.alt && (key.matches('p') || key.matches('P')) {
                    self.open_prompt_picker();

                    return Ok(false);
                }

                // Show session picker. (Alt + R)
                if key.alt && (key.matches('r') || key.matches('R')) {
                    self.open_session_picker();

                    return Ok(false);
                }

                // Show model picker. (Alt + M)
                if key.alt && (key.matches('m') || key.matches('M')) {
                    self.open_model_picker();

                    return Ok(false);
                }

                // Toggle thinking output. (Alt + T)
                if key.alt && (key.matches('t') || key.matches('T')) {
                    self.set_show_thinking(!self.show_thinking);

                    return Ok(false);
                }

                // Do not reserve printable characters for global
                // shortcuts: doing so made it impossible to type words
                // containing `q`, `/`, `j`, or `k` into the editor.
                if (key.ctrl || key.super_key) && (key.matches('c') || key.matches('C')) {
                    if self.chat_selection.is_some_and(ChatSelection::has_range) {
                        self.copy_chat_selection();

                        return Ok(false);
                    }

                    // Ctrl+C can interrupt an active response, but never
                    // quits the application. Cmd+C is copy-only on macOS.
                    if key.ctrl
                        && let Some(cancellation) = &self.agent_cancellation
                    {
                        cancellation.cancel();
                    }

                    return Ok(false);
                }

                if self.chat_selection.is_some() {
                    self.clear_chat_selection();
                }

                if key.alt && key.matches('k') && !self.message_history.is_empty() {
                    self.history_index = self
                        .history_index
                        .saturating_sub(1)
                        .min(self.message_history.len());

                    if self.history_index < self.message_history.len() {
                        self.editor
                            .set_content(self.message_history[self.history_index].clone());

                        self.refresh_autocomplete();
                    }

                    return Ok(false);
                }

                if matches!(key.code, crossterm::event::KeyCode::Up) && !self.editor.is_multiline()
                {
                    self.clear_chat_selection();
                    let scroll_limit = self.transcript_scroll_limit();
                    self.transcript.scroll_up(3, scroll_limit);

                    return Ok(false);
                }

                if matches!(key.code, crossterm::event::KeyCode::Down)
                    && !self.editor.is_multiline()
                {
                    self.clear_chat_selection();
                    self.transcript.scroll_down(3);

                    return Ok(false);
                }

                if key.ctrl && matches!(key.code, crossterm::event::KeyCode::End) {
                    self.clear_chat_selection();
                    self.transcript.follow();

                    return Ok(false);
                }

                if key.alt && key.matches('j') && !self.message_history.is_empty() {
                    self.history_index = (self.history_index + 1).min(self.message_history.len());

                    if self.history_index < self.message_history.len() {
                        self.editor
                            .set_content(self.message_history[self.history_index].clone());

                        self.refresh_autocomplete();
                    }

                    return Ok(false);
                }
            }

            // Handle editor input
            if self.editor.handle_event(input_event) {
                // Editor submitted or cancelled
                let content = self.editor.content().to_string();
                if !cancel_edit && !content.is_empty() {
                    self.send_message(&content)?;
                }

                self.editor.set_content("");
                self.refresh_autocomplete();
            } else {
                self.refresh_autocomplete();
            }
        }

        Ok(false)
    }

    /// Send a user message and process the agent response.
    pub(super) fn send_message(
        &mut self,
        content: &str,
    ) -> Result<(), anyhow::Error> {
        if content.trim_start().starts_with('/') {
            match self.handle_command(content) {
                Ok(true) => return Ok(()),

                Ok(false) => {},

                Err(error) => {
                    self.show_notice(format!("Command failed: {error}"));

                    return Ok(());
                },
            }
        }

        if self.agent.is_none() {
            self.show_notice("A response is still running; use /stop or wait for it to finish.");

            return Ok(());
        }

        let content =
            match hallucinate_agent::agent::skills::expand_skill_command(content, &self.skills) {
                Ok(content) => content,

                Err(error) => {
                    self.show_notice(format!("Skill expansion failed: {error}"));

                    return Ok(());
                },
            };

        let content = hallucinate_agent::agent::prompt_templates::expand_prompt_template(
            &content,
            &self.prompt_templates,
        );

        // A cancelled/failed stream may have left a partial assistant card;
        // start the next response in a fresh card.
        self.streaming_message_index = None;
        self.streaming_tool_call_indices.clear();

        // Add user message to transcript
        self.transcript.add_message(TuiMessage {
            role: MessageRole::User,
            content: content.clone(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });

        // Save to session
        self.session_manager.save_message(&Message::User(
            hallucinate_agent::ai::types::UserMessage::new(&content),
        ))?;
        let should_generate_title = self.title_generation_pending
            && self
                .session_manager
                .current()
                .is_some_and(|session| session.message_count == 1);

        // Save to history for navigation
        self.message_history.push(content.clone());
        self.history_index = self.message_history.len();

        // Move the agent into a task instead of blocking the TUI task. Agent
        // events continue to update the status and tool display while the
        // provider is streaming a response.
        let mut agent = self
            .agent
            .take()
            .expect("an agent must be available before sending a message");
        agent.add_user_message(&content);

        self.agent_cancellation = Some(agent.cancellation());

        if should_generate_title {
            self.title_generation_pending = false;
            let title_completion_tx = self.title_completion_tx.clone();

            let title_content = content.clone();
            let title_settings = self.session_title_settings.clone();

            let settings = self.settings.clone();
            let runtime_overrides = self.runtime_overrides.clone();

            let conversation_model = self.model.clone();

            let generate_title = async move {
                let title_model = resolve_title_model(
                    &title_settings,
                    &settings,
                    &runtime_overrides,
                    &conversation_model,
                )?;

                let provider = hallucinate_agent::config::create_provider(
                    &settings,
                    &runtime_overrides,
                    &title_model,
                )?;

                hallucinate_agent::ai::title::generate_session_title(
                    provider.as_ref(),
                    &title_model,
                    &title_content,
                )
                .await
            };

            if self.session_title_settings.generation_mode == SessionTitleGenerationMode::Before {
                self.pending_title_agent = Some(agent);
                self.title_generation_in_progress = true;
                self.render_state.status = true;

                tokio::spawn(async move {
                    let result = generate_title.await.map_err(|error| error.to_string());

                    let _ = title_completion_tx.send(super::SessionTitleCompletion { result });
                });

                return Ok(());
            }

            tokio::spawn(async move {
                let result = generate_title.await.map_err(|error| error.to_string());

                let _ = title_completion_tx.send(super::SessionTitleCompletion { result });
            });
        }

        let completion_tx = self.agent_completion_tx.clone();
        tokio::spawn(async move {
            let result = agent
                .run()
                .await
                .map(|_| ())
                .map_err(|error| error.to_string());

            let _ = completion_tx.send(AgentRunCompletion {
                agent,
                result,
                operation: None,
            });
        });

        Ok(())
    }
}

/// Resolve the model used for a session-title request.
fn resolve_title_model(
    title_settings: &hallucinate_agent::config::settings::SessionTitleSettings,
    settings: &hallucinate_agent::config::settings::Settings,
    runtime_overrides: &hallucinate_agent::config::RuntimeOverrides,
    conversation_model: &hallucinate_agent::ai::types::Model,
) -> Result<hallucinate_agent::ai::types::Model, anyhow::Error> {
    let Some(selector) = &title_settings.model else {
        return Ok(conversation_model.clone());
    };

    hallucinate_agent::config::resolve_model_selector(
        selector,
        settings,
        runtime_overrides.base_url.as_ref(),
    )
    .ok_or_else(|| anyhow::anyhow!("No title-generation model matches '{selector}'"))
}

/// Enable mouse reporting so the application can constrain selections to
/// message text instead of allowing the terminal to select UI decorations.
pub(super) fn enable_mouse_capture() -> io::Result<()> {
    crossterm::execute!(io::stdout(), crossterm::event::EnableMouseCapture)
}

/// Copy text to the host clipboard. macOS's native `pbcopy` path is used
/// because some macOS terminal emulators reserve Cmd+C instead of forwarding
/// it to applications using the terminal clipboard protocol.
fn copy_to_clipboard(text: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};

        let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "pbcopy stdin unavailable"))?;
        stdin.write_all(text.as_bytes())?;
        drop(stdin);

        let status = child.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("pbcopy failed"))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        // OSC 52 is supported by most modern terminal emulators and lets a
        // captured mouse selection behave like a normal terminal selection.
        let encoded = BASE64.encode(text.as_bytes());

        let mut stdout = io::stdout();
        stdout.write_all(format!("\x1b]52;c;{encoded}\x07").as_bytes())?;
        stdout.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_selection_orders_points_for_forward_and_reverse_drags() {
        let forward = ChatSelection {
            anchor: (4, 2),
            focus: (8, 5),
        };
        assert_eq!(forward.bounds(), ((4, 2), (8, 5)));
        assert!(forward.has_range());

        let reverse = ChatSelection {
            anchor: (8, 5),
            focus: (4, 2),
        };
        assert_eq!(reverse.bounds(), ((4, 2), (8, 5)));
    }

    #[test]
    fn a_click_without_drag_is_not_a_text_selection() {
        let selection = ChatSelection {
            anchor: (4, 2),
            focus: (4, 2),
        };
        assert!(!selection.has_range());
    }
}
