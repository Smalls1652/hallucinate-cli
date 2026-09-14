//! Slash commands and picker state transitions for interactive mode.

use std::path::PathBuf;

use hallucinate_agent::ai::types::Message;
use hallucinate_session::manager::SessionManager;
use ratatui::crossterm;

use super::{
    AgentRunCompletion,
    AgentRunOperation,
    InteractiveTui,
    ModelPicker,
    PromptPicker,
    SessionPicker,
    SessionPickerRow,
    TranscriptMessagePicker,
    TranscriptMessagePickerRow,
    session::transcript_from_messages,
    short_session_id,
};
use crate::{
    alt_screen::TranscriptRenderer,
    keys::Key,
    slash_commands::{BUILTIN_COMMANDS, is_builtin_command, parse},
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
        let token_count = storage
            .load_latest_context_token_count()?
            .unwrap_or_default();

        let agent = self.agent.as_mut().expect("agent checked above");
        agent.load_history(messages.clone())?;

        if let Some((summary, archived_message_count)) = checkpoint {
            agent.restore_compaction_checkpoint(summary, archived_message_count)?;
        }

        self.project_cwd = PathBuf::from(&session.cwd);
        self.token_count = token_count;
        agent.set_cwd(session.cwd);
        agent.reload_system_prompt();

        self.transcript = transcript_from_messages(&messages);
        self.transcript.set_show_thinking(self.show_thinking);
        self.transcript_layout_cache = Default::default();

        self.streaming_message_index = None;
        self.streaming_tool_call_indices.clear();
        self.title_generation_pending = false;

        self.message_history = messages
            .iter()
            .filter_map(|message| match message {
                Message::User(message) => Some(message.content.clone()),
                _ => None,
            })
            .collect();

        self.history_index = self.message_history.len();

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
                let mut providers = self.settings.providers_with_stored_api_keys()?;
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

        if !is_builtin_command(&parsed.name) {
            return Ok(false);
        }

        let command = parsed.name.to_ascii_lowercase();
        let arguments = parsed.arguments;

        match command.as_str() {
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

            "new" => {
                if !self.ensure_idle("creating a session") {
                    return Ok(true);
                }

                let title = command_arguments(&arguments);
                let title_generation_pending = title.is_none();

                self.create_empty_session(title, title_generation_pending)?;
                self.show_notice("Created a new session.");
            },

            "resume" => {
                let Some(session_id) = command_arguments(&arguments)
                    .and_then(|value| value.split_whitespace().next().map(str::to_owned))
                else {
                    self.open_session_picker();

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
                    self.create_empty_session(Some("Interactive session".to_string()), true)?;
                }

                self.show_notice(format!(
                    "Deleted session {}.",
                    short_session_id(&session_id)
                ));
            },

            "clear-sessions" => self.open_clear_sessions_confirmation(&arguments)?,

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

            "session" => {
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

            "copy-message" => self.open_transcript_message_picker(&arguments),

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

            "stop" => {
                if !self.cancel_running_response() {
                    self.show_notice("There is no response running.");
                }
            },

            "quit" => {
                if let Some(cancellation) = &self.agent_cancellation {
                    cancellation.cancel();
                }

                self.running = false;
            },

            _ => return Ok(false),
        }

        Ok(true)
    }

    /// Request confirmation before deleting every saved session for the
    /// process's current directory.
    fn open_clear_sessions_confirmation(
        &mut self,
        arguments: &str,
    ) -> Result<(), anyhow::Error> {
        if !arguments.trim().is_empty() {
            self.show_notice("Usage: /clear-sessions");
            return Ok(());
        }

        if !self.ensure_idle("clearing sessions") {
            return Ok(());
        }

        let cwd = std::env::current_dir()?;
        let session_ids = SessionManager::list_for_directory(&cwd)?
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();

        if session_ids.is_empty() {
            self.show_notice("No saved sessions found for the current directory.");
            return Ok(());
        }

        self.clear_sessions_confirmation = Some(super::ClearSessionsConfirmation { session_ids });
        self.render_state.overlays = true;

        Ok(())
    }

    /// Confirm or cancel the pending deletion of sessions for the current
    /// directory.
    pub(super) fn handle_clear_sessions_confirmation(
        &mut self,
        confirmed: bool,
    ) -> Result<(), anyhow::Error> {
        let Some(confirmation) = self.clear_sessions_confirmation.take() else {
            return Ok(());
        };

        if !confirmed {
            self.show_notice("Clearing sessions cancelled.");
            return Ok(());
        }

        let deleting_current = self.session_manager.current_id().is_some_and(|current_id| {
            confirmation
                .session_ids
                .iter()
                .any(|session_id| session_id == current_id)
        });

        for session_id in &confirmation.session_ids {
            SessionManager::delete_session(session_id)?;
        }

        if deleting_current {
            self.create_empty_session(Some("Interactive session".to_string()), true)?;
        }

        let session_count = confirmation.session_ids.len();
        self.show_notice(format!(
            "Deleted {session_count} saved session{} for the current directory.",
            if session_count == 1 { "" } else { "s" }
        ));

        Ok(())
    }

    /// Open the model picker with the current model selected.
    pub fn open_model_picker(&mut self) {
        if self.models.is_empty() {
            self.show_notice("No models are configured.");

            return;
        }

        let filtered_indices = filtered_model_indices(&self.models, "");
        if filtered_indices.is_empty() {
            self.show_notice("No visible models are configured.");

            return;
        }

        let selected = filtered_indices
            .iter()
            .position(|index| {
                let model = &self.models[*index];
                model.provider == self.model.provider && model.id == self.model.id
            })
            .unwrap_or(0);

        self.model_picker = Some(ModelPicker {
            filter: String::new(),
            filtered_indices,
            selected,
        });
    }

    /// Open the session picker with the current session selected.
    pub fn open_session_picker(&mut self) {
        let sessions = match std::env::current_dir()
            .map_err(anyhow::Error::from)
            .and_then(|cwd| SessionManager::list_for_directory(&cwd))
        {
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
                    short_id: short_session_id(&session.id).to_string(),
                    id: session.id.clone(),
                    datetime: session.modified_str(),
                    title,
                }
            })
            .collect::<Vec<_>>();

        let current_id = self.session_manager.current_id();

        let selected = rows
            .iter()
            .position(|row| current_id == Some(row.id.as_str()))
            .unwrap_or(0);

        self.session_picker = Some(SessionPicker {
            filter: String::new(),
            filtered_indices: (0..rows.len()).collect(),
            rows,
            selected,
        });
    }

    /// Open a picker for copying the complete source text of a transcript
    /// message.
    fn open_transcript_message_picker(
        &mut self,
        arguments: &str,
    ) {
        if !arguments.trim().is_empty() {
            self.show_notice("Usage: /copy-message");
            return;
        }

        let rows = self
            .transcript
            .messages()
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                matches!(
                    message.role,
                    crate::alt_screen::MessageRole::User
                        | crate::alt_screen::MessageRole::Assistant
                ) && !message.content.is_empty()
            })
            .map(|(message_index, message)| TranscriptMessagePickerRow {
                message_index,
                role: message.role,
                preview: message
                    .content
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            })
            .collect::<Vec<_>>();

        if rows.is_empty() {
            self.show_notice("There are no messages to copy.");
            return;
        }

        self.transcript_message_picker = Some(TranscriptMessagePicker { rows, selected: 0 });
    }

    /// Handle a key while the transcript message picker owns input focus.
    pub(super) fn handle_transcript_message_picker_key(
        &mut self,
        key: Key,
    ) {
        let Some(picker) = self.transcript_message_picker.as_mut() else {
            return;
        };

        match_picker_navigation(key, &mut picker.selected, picker.rows.len());

        match key.code {
            crossterm::event::KeyCode::Esc => self.transcript_message_picker = None,

            crossterm::event::KeyCode::Enter => {
                let content = picker
                    .rows
                    .get(picker.selected)
                    .and_then(|row| self.transcript.messages().get(row.message_index))
                    .map(|message| message.content.clone());
                self.transcript_message_picker = None;

                if let Some(content) = content {
                    self.copy_transcript_message(&content);
                }
            },

            _ => {},
        }
    }

    /// Open the prompt template picker with all loaded templates.
    pub fn open_prompt_picker(&mut self) {
        if self.prompt_templates.is_empty() {
            self.show_notice("No prompt templates found.");

            return;
        }

        self.prompt_picker = Some(PromptPicker {
            filter: String::new(),
            filtered_indices: (0..self.prompt_templates.len()).collect(),
            selected: 0,
        });
    }

    /// Handle a key while the session picker owns the input focus.
    pub(super) fn handle_session_picker_key(
        &mut self,
        key: Key,
    ) -> Result<(), anyhow::Error> {
        if key.matches('c') && key.ctrl {
            // Ctrl+C remains available to interrupt an active response.
            if let Some(cancellation) = &self.agent_cancellation {
                cancellation.cancel();
            }

            return Ok(());
        }

        if handle_picker_filter_key(key, self.session_picker.as_mut(), |picker| {
            picker.filtered_indices = filtered_session_indices(&picker.rows, &picker.filter);
        }) {
            return Ok(());
        }

        let Some(picker) = self.session_picker.as_mut() else {
            return Ok(());
        };

        match_picker_navigation(key, &mut picker.selected, picker.filtered_indices.len());

        match key.code {
            crossterm::event::KeyCode::Esc => self.session_picker = None,

            crossterm::event::KeyCode::Enter => {
                let session_id = picker
                    .filtered_indices
                    .get(picker.selected)
                    .and_then(|index| picker.rows.get(*index))
                    .map(|row| row.id.clone());

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
        if key.matches('c') && key.ctrl {
            // Ctrl+C remains available to interrupt an active response.
            if let Some(cancellation) = &self.agent_cancellation {
                cancellation.cancel();
            }

            return Ok(());
        }

        if handle_picker_filter_key(key, self.prompt_picker.as_mut(), |picker| {
            picker.filtered_indices =
                filtered_prompt_indices(&self.prompt_templates, &picker.filter);
        }) {
            return Ok(());
        }

        let Some(picker) = self.prompt_picker.as_mut() else {
            return Ok(());
        };
        match_picker_navigation(key, &mut picker.selected, picker.filtered_indices.len());

        match key.code {
            crossterm::event::KeyCode::Esc => self.prompt_picker = None,

            crossterm::event::KeyCode::Enter => {
                let template = picker
                    .filtered_indices
                    .get(picker.selected)
                    .and_then(|index| self.prompt_templates.get(*index))
                    .cloned();
                self.prompt_picker = None;

                if let Some(template) = template {
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
        if key.matches('c') && key.ctrl {
            // Ctrl+C remains available to interrupt an active response.
            if let Some(cancellation) = &self.agent_cancellation {
                cancellation.cancel();
            }

            return Ok(());
        }

        if handle_picker_filter_key(key, self.model_picker.as_mut(), |picker| {
            picker.filtered_indices = filtered_model_indices(&self.models, &picker.filter);
        }) {
            return Ok(());
        }

        let Some(picker) = self.model_picker.as_mut() else {
            return Ok(());
        };
        match_picker_navigation(key, &mut picker.selected, picker.filtered_indices.len());

        match key.code {
            crossterm::event::KeyCode::Esc => self.model_picker = None,

            crossterm::event::KeyCode::Enter => {
                let model = picker
                    .filtered_indices
                    .get(picker.selected)
                    .and_then(|index| self.models.get(*index))
                    .cloned();
                self.model_picker = None;

                if let Some(model) = model {
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

        self.show_notice(format!("Switched model to {}/{}", model.provider, model.id));

        Ok(())
    }

    /// Toggle or explicitly set the visibility of model reasoning.
    pub fn handle_thinking_command(
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
    pub fn create_empty_session(
        &mut self,
        title: Option<String>,
        title_generation_pending: bool,
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
        self.token_count = 0;

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

        self.title_generation_pending =
            title_generation_pending && self.session_title_settings.enabled;

        self.message_history.clear();

        self.history_index = 0;

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

/// Update a picker filter and reset its selection when text input changes.
fn handle_picker_filter_key<T>(
    key: Key,
    picker: Option<&mut T>,
    update_indices: impl FnOnce(&mut T),
) -> bool
where
    T: PickerFilter,
{
    let Some(picker) = picker else {
        return false;
    };

    let changed = match key.code {
        crossterm::event::KeyCode::Char(character) if !key.ctrl && !key.alt => {
            picker.filter_mut().push(character);
            true
        },

        crossterm::event::KeyCode::Backspace if !key.ctrl && !key.alt => {
            picker.filter_mut().pop().is_some()
        },

        _ => false,
    };

    if changed {
        update_indices(picker);
        *picker.selected_mut() = 0;
    }

    changed
}

/// Trait for picker types that support filtering and selection.
trait PickerFilter {
    /// Return a mutable reference to the filter string.
    fn filter_mut(&mut self) -> &mut String;

    /// Return a mutable reference to the selected index.
    fn selected_mut(&mut self) -> &mut usize;
}

impl PickerFilter for ModelPicker {
    fn filter_mut(&mut self) -> &mut String {
        &mut self.filter
    }

    fn selected_mut(&mut self) -> &mut usize {
        &mut self.selected
    }
}
impl PickerFilter for SessionPicker {
    fn filter_mut(&mut self) -> &mut String {
        &mut self.filter
    }

    fn selected_mut(&mut self) -> &mut usize {
        &mut self.selected
    }
}
impl PickerFilter for PromptPicker {
    fn filter_mut(&mut self) -> &mut String {
        &mut self.filter
    }

    fn selected_mut(&mut self) -> &mut usize {
        &mut self.selected
    }
}

/// Navigate picker selection using arrow keys, page keys, and home/end.
fn match_picker_navigation(
    key: Key,
    selected: &mut usize,
    item_count: usize,
) {
    if item_count == 0 {
        return;
    }

    match key.code {
        crossterm::event::KeyCode::Up => *selected = selected.saturating_sub(1),

        crossterm::event::KeyCode::Down => *selected = (*selected + 1).min(item_count - 1),

        crossterm::event::KeyCode::PageUp => *selected = selected.saturating_sub(8),

        crossterm::event::KeyCode::PageDown => *selected = (*selected + 8).min(item_count - 1),

        crossterm::event::KeyCode::Home => *selected = 0,

        crossterm::event::KeyCode::End => *selected = item_count - 1,

        _ => {},
    }
}

/// Return whether a filter string matches a value (case-insensitive).
fn matches_picker_filter(
    value: &str,
    filter: &str,
) -> bool {
    value.to_lowercase().contains(&filter.to_lowercase())
}

/// Return indices of models matching a filter string.
fn filtered_model_indices(
    models: &[hallucinate_agent::ai::types::Model],
    filter: &str,
) -> Vec<usize> {
    models
        .iter()
        .enumerate()
        .filter_map(|(index, model)| {
            (!model.hidden && matches_picker_filter(&model.name, filter)).then_some(index)
        })
        .collect()
}

/// Return indices of session picker rows matching a filter string.
fn filtered_session_indices(
    rows: &[SessionPickerRow],
    filter: &str,
) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| matches_picker_filter(&row.title, filter).then_some(index))
        .collect()
}

/// Return indices of prompt templates matching a filter string.
fn filtered_prompt_indices(
    templates: &[hallucinate_agent::agent::prompt_templates::PromptTemplate],
    filter: &str,
) -> Vec<usize> {
    templates
        .iter()
        .enumerate()
        .filter_map(|(index, template)| {
            matches_picker_filter(&template.name, filter).then_some(index)
        })
        .collect()
}
