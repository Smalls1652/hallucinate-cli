//! Slash commands and picker state transitions for interactive mode.

use std::path::PathBuf;

use hallucinate_agent::ai::types::Message;
use hallucinate_session::manager::SessionManager;

use super::{
    AgentRunCompletion,
    AgentRunOperation,
    InteractiveTui,
    ModelPicker,
    PromptPicker,
    SessionPicker,
    SessionPickerRow,
    session::transcript_from_messages,
    short_session_id,
};
use crate::{
    alt_screen::TranscriptRenderer,
    keys::Key,
    slash_commands::{BUILTIN_COMMANDS, parse, resolve_builtin_command},
};

impl InteractiveTui {
    /// Load a selected session into both the agent and the visible transcript.
    fn load_selected_session(
        &mut self,
        session_id: &str,
    ) -> Result<(), anyhow::Error> {
        if self.agent.is_none() {
            self.show_notice("A response is still running; wait before changing sessions.");
            return Ok(());
        }

        let session = self.session_manager.resume(session_id)?;
        let storage = self
            .session_manager
            .current_storage()
            .expect("resumed session has storage");
        let messages = storage.load_messages()?;
        let checkpoint = storage.load_compaction_checkpoint()?;

        let agent = self.agent.as_mut().expect("agent checked above");
        agent.load_history(messages.clone())?;
        if let Some((summary, archived_message_count)) = checkpoint {
            agent.restore_compaction_checkpoint(summary, archived_message_count)?;
        }

        self.project_cwd = PathBuf::from(&session.cwd);
        agent.set_cwd(session.cwd);
        agent.reload_system_prompt();

        self.transcript = transcript_from_messages(&messages);
        self.transcript.set_show_thinking(self.show_thinking);
        self.transcript_layout_cache = Default::default();

        self.streaming_message_index = None;
        self.streaming_tool_call_indices.clear();

        self.message_history = messages
            .iter()
            .filter_map(|message| match message {
                Message::User(message) => Some(message.content.clone()),
                _ => None,
            })
            .collect();

        self.history_index = self.message_history.len();
        self.token_count = agent.estimate_tokens();

        self.show_notice(format!("Resumed session {}.", short_session_id(session_id)));

        Ok(())
    }

    /// Save, remove, or list provider API keys from the interactive session.
    /// Keys are never included in the confirmation message or transcript.
    fn handle_auth_command(
        &mut self,
        arguments: &str,
    ) -> Result<(), anyhow::Error> {
        let arguments = hallucinate_agent::agent::prompt_templates::parse_command_args(arguments);
        let Some(operation) = arguments.first().map(String::as_str) else {
            self.show_notice("Usage: /auth <list|set|remove> [provider] [api-key]");
            return Ok(());
        };

        match operation {
            "list" => {
                let mut providers: Vec<_> = self.settings.credential_store.provider_ids().collect();
                providers.sort_unstable();

                if providers.is_empty() {
                    self.add_system_message("No saved provider credentials.");
                } else {
                    self.add_system_message(format!("Saved credentials: {}", providers.join(", ")));
                }
            },

            "set" => {
                if !self.ensure_idle("saving credentials") {
                    return Ok(());
                }

                if arguments.len() != 3 {
                    self.show_notice("Usage: /auth set <provider> <api-key>");
                    return Ok(());
                }

                let provider = &arguments[1];
                self.settings.save_api_key(provider, &arguments[2])?;

                if provider == &self.model.provider {
                    let model = self.model.clone();
                    let new_provider = hallucinate_agent::config::create_provider(
                        &self.settings,
                        &self.runtime_overrides,
                        &model,
                    )?;

                    self.agent
                        .as_mut()
                        .expect("credentials require an idle agent")
                        .set_model(
                            new_provider,
                            model,
                            hallucinate_agent::config::agent_config(
                                &self.settings,
                                &self.model,
                                &self.runtime_overrides,
                            )
                            .max_tokens,
                        );
                }

                self.show_notice(format!("Saved credentials for provider '{}'.", provider));
            },

            "remove" => {
                if !self.ensure_idle("removing credentials") {
                    return Ok(());
                }

                if arguments.len() != 2 {
                    self.show_notice("Usage: /auth remove <provider>");
                    return Ok(());
                }

                let provider = &arguments[1];

                if self.settings.remove_api_key(provider)? {
                    self.show_notice(format!(
                        "Removed saved credentials for provider '{}'.",
                        provider
                    ));
                } else {
                    self.show_notice(format!(
                        "No saved credentials found for provider '{}'.",
                        provider
                    ));
                }
            },

            _ => self.show_notice("Usage: /auth <list|set|remove> [provider] [api-key]"),
        }
        Ok(())
    }

    /// Handle a built-in command. Unknown slash-prefixed input remains a
    /// normal prompt so providers can still be used with their own slash
    /// command conventions. Prompt templates are handled after this method.
    pub(super) fn handle_command(
        &mut self,
        content: &str,
    ) -> Result<bool, anyhow::Error> {
        let Some(parsed) = parse(content) else {
            return Ok(false);
        };

        let Some(command) = resolve_builtin_command(&parsed.name) else {
            return Ok(false);
        };

        let arguments = parsed.arguments;

        match command {
            "auth" => self.handle_auth_command(&arguments)?,

            "help" => {
                let mut lines = vec!["Built-in commands:".to_string()];
                lines.extend(
                    BUILTIN_COMMANDS
                        .iter()
                        .map(|command| format!("{} — {}", command.usage, command.description)),
                );

                if !self.prompt_templates.is_empty() {
                    lines.push(
                        "Prompt templates: type /<name> [arguments]. Use /prompts to list them."
                            .to_string(),
                    );
                }

                self.add_system_messages(lines);
            },

            "new" | "clear" => {
                if !self.ensure_idle("creating a session") {
                    return Ok(true);
                }

                let title = command_arguments(&arguments)
                    .or_else(|| Some("Interactive session".to_string()));

                self.create_empty_session(title)?;
                self.show_notice("Created a new session.");
            },

            "sessions" | "list" => self.open_session_picker(),

            "resume" => {
                let Some(session_id) = command_arguments(&arguments)
                    .and_then(|value| value.split_whitespace().next().map(str::to_owned))
                else {
                    self.show_notice("Usage: /resume <session-id>");
                    return Ok(true);
                };

                if !self.ensure_idle("resuming a session") {
                    return Ok(true);
                }

                let session_id = resolve_session_id(&session_id)?;
                self.load_selected_session(&session_id)?;
            },

            "fork" => {
                if !self.ensure_idle("forking a session") {
                    return Ok(true);
                }

                let Some(parent_id) = self.session_manager.current_id().map(str::to_owned) else {
                    self.show_notice("There is no current session to fork.");

                    return Ok(true);
                };

                let session = self.session_manager.fork(&parent_id)?;
                self.load_selected_session(&session.id)?;
                self.show_notice(format!("Forked session {}.", short_session_id(&session.id)));
            },

            "delete" => {
                if !self.ensure_idle("deleting a session") {
                    return Ok(true);
                }

                let session_id = command_arguments(&arguments)
                    .and_then(|value| value.split_whitespace().next().map(str::to_owned))
                    .or_else(|| self.session_manager.current_id().map(str::to_owned));

                let Some(session_id) = session_id else {
                    self.show_notice("There is no current session to delete.");
                    return Ok(true);
                };

                let session_id = resolve_session_id(&session_id)?;
                let deleting_current =
                    self.session_manager.current_id() == Some(session_id.as_str());

                SessionManager::delete_session(&session_id)?;
                if deleting_current {
                    self.create_empty_session(Some("Interactive session".to_string()))?;
                }

                self.show_notice(format!(
                    "Deleted session {}.",
                    short_session_id(&session_id)
                ));
            },

            "name" => {
                if !self.ensure_idle("renaming a session") {
                    return Ok(true);
                }

                let title = command_arguments(&arguments);
                self.session_manager.rename_current(title.clone())?;

                if let Some(title) = title {
                    self.show_notice(format!("Session renamed to {title}."));
                } else {
                    self.show_notice("Session name cleared.");
                }
            },

            "session" | "status" => {
                if let Some(session) = self.session_manager.current() {
                    self.add_system_message(format!(
                        "Session {} — {} — {} messages — {} tokens — cwd: {} — parent: {}",
                        short_session_id(&session.id),
                        session.title.as_deref().unwrap_or("untitled"),
                        session.message_count,
                        self.token_count,
                        hallucinate_agent::config::paths::display_path(
                            &session.cwd,
                            &self.project_cwd
                        ),
                        session.parent_session_id.as_deref().unwrap_or("none")
                    ));
                }
            },

            "prompts" => {
                if self.prompt_templates.is_empty() {
                    self.add_system_message("No prompt templates found.");
                } else {
                    self.open_prompt_picker();
                }
            },

            "skills" => {
                if self.skills.is_empty() {
                    self.add_system_message("No skills loaded.");
                } else {
                    let mut lines = vec!["Loaded skills:".to_string()];
                    lines.extend(
                        self.skills
                            .iter()
                            .map(|skill| format!("/skill:{} — {}", skill.name, skill.description)),
                    );

                    self.add_system_messages(lines);
                }
            },

            "model" => {
                let arguments =
                    hallucinate_agent::agent::prompt_templates::parse_command_args(&arguments);
                if arguments.is_empty() {
                    self.open_model_picker();
                } else {
                    self.switch_model(&arguments[0])?;
                }
            },

            "thinking" => self.handle_thinking_command(&arguments),

            "compact" => {
                if !self.ensure_idle("compacting context") {
                    return Ok(true);
                }

                let mut agent = self.agent.take().expect("agent is idle");
                self.agent_cancellation = Some(agent.cancellation());
                let completion_tx = self.agent_completion_tx.clone();
                tokio::spawn(async move {
                    let result = agent.compact().await.map_err(|error| error.to_string());
                    let _ = completion_tx.send(AgentRunCompletion {
                        agent,
                        result,
                        operation: Some(AgentRunOperation::Compact),
                    });
                });

                self.add_system_message("Compacting the current context...");
                self.show_notice("Compacting the current context...");
            },

            "reload" => {
                let result = hallucinate_agent::agent::prompt_templates::load_prompt_templates(
                    &self.prompt_template_options,
                );

                let diagnostics = result
                    .diagnostics
                    .iter()
                    .map(|diagnostic| {
                        format!(
                            "Warning: could not load prompt template {}: {}",
                            hallucinate_agent::config::paths::display_path(
                                &diagnostic.path,
                                &self.project_cwd,
                            ),
                            diagnostic.message
                        )
                    })
                    .collect::<Vec<_>>();

                self.add_system_messages(diagnostics);

                self.prompt_templates = result.templates;

                if let Some(agent) = self.agent.as_mut() {
                    agent.reload_system_prompt();
                    self.token_count = agent.estimate_tokens();
                }

                self.show_notice(format!(
                    "Reloaded {} prompt template{} and system prompt files.",
                    self.prompt_templates.len(),
                    if self.prompt_templates.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
            },

            "stop" | "cancel" => {
                if !self.cancel_running_response() {
                    self.show_notice("There is no response running.");
                }
            },

            "quit" | "exit" => {
                if let Some(cancellation) = &self.agent_cancellation {
                    cancellation.cancel();
                }

                self.running = false;
            },

            _ => return Ok(false),
        }

        Ok(true)
    }

    /// Open the model picker with the current model selected.
    fn open_model_picker(&mut self) {
        if self.models.is_empty() {
            self.show_notice("No models are configured.");
            return;
        }

        let selected = self
            .models
            .iter()
            .position(|model| model.provider == self.model.provider && model.id == self.model.id)
            .unwrap_or(0);

        self.model_picker = Some(ModelPicker { selected });
    }

    /// Open the session picker with the current session selected.
    fn open_session_picker(&mut self) {
        let sessions = match SessionManager::list() {
            Ok(sessions) => sessions,
            Err(error) => {
                self.show_notice(format!("Failed to list sessions: {error}"));
                return;
            },
        };

        if sessions.is_empty() {
            self.show_notice("No sessions found.");
            return;
        }

        let rows = sessions
            .iter()
            .map(|session| {
                let title = hallucinate_session::storage::SessionStorage::new(&session.id)
                    .load_metadata()
                    .ok()
                    .flatten()
                    .and_then(|metadata| metadata.title)
                    .unwrap_or_else(|| "untitled".to_string());

                SessionPickerRow {
                    id: session.id.clone(),
                    modified: session.modified_str(),
                    display_path: hallucinate_agent::config::paths::display_path(
                        &session.path,
                        &self.project_cwd,
                    ),
                    title,
                }
            })
            .collect::<Vec<_>>();
        let current_id = self.session_manager.current_id();
        let selected = rows
            .iter()
            .position(|row| current_id == Some(row.id.as_str()))
            .unwrap_or(0);

        self.session_picker = Some(SessionPicker { rows, selected });
    }

    /// Open the prompt template picker with all loaded templates.
    fn open_prompt_picker(&mut self) {
        if self.prompt_templates.is_empty() {
            self.show_notice("No prompt templates found.");
            return;
        }

        self.prompt_picker = Some(PromptPicker { selected: 0 });
    }

    /// Handle a key while the session picker owns the input focus.
    pub(super) fn handle_session_picker_key(
        &mut self,
        key: Key,
    ) -> Result<(), anyhow::Error> {
        let Some(picker) = self.session_picker.as_mut() else {
            return Ok(());
        };

        let session_count = picker.rows.len();

        match key.code {
            crossterm::event::KeyCode::Char('c') if key.ctrl => {
                // Ctrl+C is intentionally not a quit shortcut. It can still
                // interrupt an active response, matching the Esc/stop path.
                if let Some(cancellation) = &self.agent_cancellation {
                    cancellation.cancel();
                }
            },

            crossterm::event::KeyCode::Up => {
                picker.selected = picker.selected.saturating_sub(1);
            },

            crossterm::event::KeyCode::Down => {
                picker.selected = (picker.selected + 1).min(session_count - 1);
            },

            crossterm::event::KeyCode::PageUp => {
                picker.selected = picker.selected.saturating_sub(8);
            },

            crossterm::event::KeyCode::PageDown => {
                picker.selected = (picker.selected + 8).min(session_count - 1);
            },

            crossterm::event::KeyCode::Home => {
                picker.selected = 0;
            },

            crossterm::event::KeyCode::End => {
                picker.selected = session_count - 1;
            },

            crossterm::event::KeyCode::Esc => {
                self.session_picker = None;
            },

            crossterm::event::KeyCode::Enter => {
                let session_id = picker.rows.get(picker.selected).map(|row| row.id.clone());
                self.session_picker = None;

                if let Some(session_id) = session_id {
                    if !self.ensure_idle("resuming a session") {
                        return Ok(());
                    }

                    if let Err(error) = self.load_selected_session(&session_id) {
                        self.show_notice(format!("Failed to resume session: {error}"));
                    }
                }
            },

            _ => {},
        }

        Ok(())
    }

    /// Handle a key while the prompt picker owns the input focus.
    pub(super) fn handle_prompt_picker_key(
        &mut self,
        key: Key,
    ) -> Result<(), anyhow::Error> {
        let Some(picker) = self.prompt_picker.as_mut() else {
            return Ok(());
        };

        match key.code {
            crossterm::event::KeyCode::Char('c') if key.ctrl => {
                // Ctrl+C is intentionally not a quit shortcut. It can still
                // interrupt an active response, matching the Esc/stop path.
                if let Some(cancellation) = &self.agent_cancellation {
                    cancellation.cancel();
                }
            },

            crossterm::event::KeyCode::Up => {
                picker.selected = picker.selected.saturating_sub(1);
            },

            crossterm::event::KeyCode::Down => {
                picker.selected = (picker.selected + 1).min(self.prompt_templates.len() - 1);
            },

            crossterm::event::KeyCode::PageUp => {
                picker.selected = picker.selected.saturating_sub(8);
            },

            crossterm::event::KeyCode::PageDown => {
                picker.selected = (picker.selected + 8).min(self.prompt_templates.len() - 1);
            },

            crossterm::event::KeyCode::Home => {
                picker.selected = 0;
            },

            crossterm::event::KeyCode::End => {
                picker.selected = self.prompt_templates.len() - 1;
            },

            crossterm::event::KeyCode::Esc => {
                self.prompt_picker = None;
            },

            crossterm::event::KeyCode::Enter => {
                let selected = picker.selected;
                self.prompt_picker = None;

                if let Some(template) = self.prompt_templates.get(selected).cloned() {
                    self.editor.set_content(format!("/{} ", template.name));
                    self.refresh_autocomplete();
                }
            },

            _ => {},
        }

        Ok(())
    }

    /// Handle a key while the model picker owns the input focus.
    pub(super) fn handle_model_picker_key(
        &mut self,
        key: Key,
    ) -> Result<(), anyhow::Error> {
        let Some(picker) = self.model_picker.as_mut() else {
            return Ok(());
        };

        match key.code {
            crossterm::event::KeyCode::Char('c') if key.ctrl => {
                // Ctrl+C is intentionally not a quit shortcut. It can still
                // interrupt an active response, matching the Esc/stop path.
                if let Some(cancellation) = &self.agent_cancellation {
                    cancellation.cancel();
                }
            },

            crossterm::event::KeyCode::Up => {
                picker.selected = picker.selected.saturating_sub(1);
            },

            crossterm::event::KeyCode::Down => {
                picker.selected = (picker.selected + 1).min(self.models.len() - 1);
            },

            crossterm::event::KeyCode::PageUp => {
                picker.selected = picker.selected.saturating_sub(8);
            },

            crossterm::event::KeyCode::PageDown => {
                picker.selected = (picker.selected + 8).min(self.models.len() - 1);
            },

            crossterm::event::KeyCode::Home => {
                picker.selected = 0;
            },

            crossterm::event::KeyCode::End => {
                picker.selected = self.models.len() - 1;
            },

            crossterm::event::KeyCode::Esc => {
                self.model_picker = None;
            },

            crossterm::event::KeyCode::Enter => {
                let selected = picker.selected;
                self.model_picker = None;

                if let Some(model) = self.models.get(selected).cloned() {
                    self.switch_to_model(model)?;
                }
            },

            _ => {},
        }

        Ok(())
    }

    /// Switch an idle agent to a model from the configured model catalog.
    fn switch_model(
        &mut self,
        selector: &str,
    ) -> Result<(), anyhow::Error> {
        let model = if let Some((provider, model_id)) = selector.split_once('/') {
            self.models
                .iter()
                .find(|model| model.provider == provider && model.id == model_id)
                .cloned()
        } else {
            let mut matches = self.models.iter().filter(|model| model.id == selector);
            let model = matches.next().cloned();

            if matches.next().is_some() {
                self.show_notice(format!(
                    "Model '{}' is ambiguous; use provider/model.",
                    selector
                ));

                return Ok(());
            }
            model
        };

        let Some(model) = model else {
            self.show_notice(format!(
                "Model '{}' was not found. Use /model to list configured models.",
                selector
            ));

            return Ok(());
        };

        self.switch_to_model(model)
    }

    /// Install a selected model and the provider serving it.
    fn switch_to_model(
        &mut self,
        model: hallucinate_agent::ai::types::Model,
    ) -> Result<(), anyhow::Error> {
        if !self.ensure_idle("switching models") {
            return Ok(());
        }

        let provider = hallucinate_agent::config::create_provider(
            &self.settings,
            &self.runtime_overrides,
            &model,
        )?;
        let max_tokens = hallucinate_agent::config::agent_config(
            &self.settings,
            &model,
            &self.runtime_overrides,
        )
        .max_tokens;

        let agent = self
            .agent
            .as_mut()
            .expect("model changes require an idle agent");
        agent.set_model(provider, model.clone(), max_tokens);
        agent.reload_system_prompt();

        self.model = model.clone();
        self.token_count = agent.estimate_tokens();

        self.show_notice(format!(
            "Switched to {}/{} (context: {}, max output: {}).",
            model.provider, model.id, model.context_window, model.max_tokens
        ));

        Ok(())
    }

    /// Toggle or explicitly set the visibility of model reasoning.
    fn handle_thinking_command(
        &mut self,
        arguments: &str,
    ) {
        let argument = arguments.trim().to_ascii_lowercase();

        let show = match argument.as_str() {
            "" => !self.show_thinking,
            "on" | "true" | "yes" => true,
            "off" | "false" | "no" => false,
            _ => {
                self.show_notice("Usage: /thinking [on|off]");
                return;
            },
        };

        self.set_show_thinking(show);

        self.show_notice(if show {
            "Thinking output enabled."
        } else {
            "Thinking output disabled."
        });
    }

    /// Return whether a command which changes the active context may run.
    fn ensure_idle(
        &mut self,
        operation: &str,
    ) -> bool {
        if self.agent.is_none() {
            self.show_notice(format!(
                "A response is still running; wait before {operation}."
            ));

            false
        } else {
            true
        }
    }

    /// Create a session and reset the in-memory agent/transcript to match it.
    fn create_empty_session(
        &mut self,
        title: Option<String>,
    ) -> Result<(), anyhow::Error> {
        let cwd = std::env::current_dir()?;
        let cwd_string = cwd.to_string_lossy().into_owned();

        self.session_manager
            .create(hallucinate_session::manager::SessionCreateOptions {
                title,
                parent_session_id: None,
                cwd: Some(cwd_string.clone()),
            })?;

        self.project_cwd = cwd.clone();

        let agent = self
            .agent
            .as_mut()
            .expect("session changes require an idle agent");
        agent.load_history(Vec::new())?;
        agent.set_cwd(cwd_string);

        self.transcript = TranscriptRenderer::new();
        self.transcript.set_show_thinking(self.show_thinking);
        self.transcript_layout_cache = Default::default();

        self.streaming_message_index = None;
        self.streaming_tool_call_indices.clear();
        self.message_history.clear();
        self.history_index = 0;
        self.token_count = agent.estimate_tokens();

        self.add_new_session_message();

        Ok(())
    }
}

/// Parse a command's arguments as shell-like words and use them as a
/// human-readable value (for example, `/new "fix auth"`).
fn command_arguments(arguments: &str) -> Option<String> {
    let arguments = hallucinate_agent::agent::prompt_templates::parse_command_args(arguments);

    (!arguments.is_empty()).then(|| arguments.join(" "))
}

/// Resolve a complete session ID or an unambiguous ID prefix.
fn resolve_session_id(input: &str) -> Result<String, anyhow::Error> {
    let input = input.trim();

    if input.is_empty() {
        return Err(anyhow::anyhow!("session id cannot be empty"));
    }

    if hallucinate_session::storage::SessionStorage::new(input).exists() {
        return Ok(input.to_string());
    }

    let matches: Vec<_> = SessionManager::list()?
        .into_iter()
        .filter(|session| session.id.starts_with(input))
        .map(|session| session.id)
        .collect();

    match matches.as_slice() {
        [session_id] => Ok(session_id.clone()),
        [] => Err(anyhow::anyhow!("Session '{}' not found", input)),
        _ => Err(anyhow::anyhow!("session id '{}' is ambiguous", input)),
    }
}
