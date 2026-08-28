//! Interactive mode: Full terminal UI with transcript, editor, and tool
//! display.
//!
//! This mode wires together the TUI, Agent, and Session modules to provide
//! a complete interactive AI coding assistant experience.

use std::{
    io::{self, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context;
#[cfg(not(target_os = "macos"))]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    agent::{
        runtime::Agent,
        types::{AgentCancellation, AgentConfig, AgentEvent, AgentState},
    },
    ai::types::{AssistantContent, AssistantMessage, Message},
    cli::args::Cli,
    config::settings::Settings,
    session::manager::SessionManager,
    tui::{
        alt_screen::{
            Message as TuiMessage,
            MessageRole,
            ToolCall as TuiToolCall,
            TranscriptRenderer,
        },
        components::{Editor, markdown_to_lines},
        input::parse_event,
        keys::Key,
    },
};

/// Run interactive mode.
pub async fn run(
    cli: &Cli,
    settings: &Settings,
) -> Result<(), anyhow::Error> {
    // Ensure directories exist
    crate::config::paths::ensure_directories()?;

    // Load settings from disk if available
    let settings = Settings::load().unwrap_or_else(|_| settings.clone());

    // Resolve model
    let model = super::resolve_model(cli, &settings)?;

    // Create the provider belonging to the selected model. This matters when
    // the model came from a user-defined provider group.
    let provider = super::create_provider_for_model(cli, &settings, &model)?;

    // Create agent config
    let agent_config = AgentConfig {
        max_turns: settings.max_turns,
        max_context_tokens: model.context_window,
        enable_compaction: settings.enable_compaction,
        enable_tools: settings.enable_tools,
        temperature: settings.temperature,
        max_tokens: super::resolve_max_tokens(cli, &settings, &model),
        include_tool_definitions: true,
    };

    // Discover skills before creating the agent so their metadata can be
    // included in the system prompt. Invalid skill files do not prevent the
    // session from starting.
    let cwd = std::env::current_dir().context("Failed to determine working directory")?;
    let skills = crate::agent::skills::load_skills(&crate::agent::skills::SkillLoadOptions {
        cwd: cwd.clone(),
        skill_paths: cli.skills.clone(),
        include_defaults: !cli.no_skills,
    });
    for diagnostic in &skills.diagnostics {
        eprintln!(
            "Warning: could not load skill {}: {}",
            crate::config::paths::display_path(&diagnostic.path, &cwd),
            diagnostic.message
        );
    }

    let prompt_template_options = crate::agent::prompt_templates::PromptTemplateLoadOptions {
        cwd: cwd.clone(),
        prompt_paths: cli.prompt_templates.clone(),
        include_defaults: !cli.no_prompt_templates,
    };
    let prompt_templates =
        crate::agent::prompt_templates::load_prompt_templates(&prompt_template_options);
    for diagnostic in &prompt_templates.diagnostics {
        eprintln!(
            "Warning: could not load prompt template {}: {}",
            crate::config::paths::display_path(&diagnostic.path, &cwd),
            diagnostic.message
        );
    }

    // Create agent with all built-in tools.
    let loaded_skills = skills.skills;
    let mut agent = Agent::with_all_tools_and_image_resize_and_shell(
        provider,
        model.clone(),
        agent_config,
        settings.auto_resize_images,
        super::resolve_shell(cli, &settings),
    )
    .with_cwd(cwd.to_string_lossy().into_owned())
    .with_skills(loaded_skills.clone());

    // Apply prompt overrides. Provider credentials and endpoint are resolved
    // when `create_provider` constructs the provider.
    if let Some(system_prompt) = &cli.system_prompt {
        agent = agent.with_system_prompt(system_prompt.clone());
    }

    // Every interactive run has a concrete session before the UI starts, so
    // user and assistant messages can never be silently discarded. `--session`
    // restores validated history and the persisted cwd when it still exists.
    let mut session_manager = SessionManager::new();
    if let Some(session_id) = &cli.session {
        let session = session_manager.resume(session_id)?;
        let messages = session_manager
            .current_storage()
            .expect("resumed session has storage")
            .load_messages()?;
        agent.load_history(messages)?;
        agent.set_cwd(session.cwd);
    } else {
        session_manager.create(crate::session::manager::SessionCreateOptions {
            title: Some("Interactive session".to_string()),
            parent_session_id: None,
            cwd: Some(cwd.to_string_lossy().into_owned()),
        })?;
    }

    // Subscribe to agent events
    let event_rx = agent.subscribe();

    // Create TUI
    let mut models = super::available_models(&settings);
    if !models
        .iter()
        .any(|candidate| candidate.provider == model.provider && candidate.id == model.id)
    {
        models.push(model.clone());
        models.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    let mut tui = InteractiveTui::new(
        agent,
        event_rx,
        session_manager,
        prompt_templates.templates,
        prompt_template_options,
        loaded_skills,
        cli.clone(),
        settings.clone(),
        model,
        models,
    )?;

    // Run the TUI
    tui.run().await
}

const EPHEMERAL_NOTICE_DURATION: Duration = Duration::from_secs(4);

struct EphemeralNotice {
    message: String,
    expires_at: Instant,
}

/// A selection in the rendered chat area. Positions are terminal cell
/// coordinates, which means selection continues to work for wrapped and
/// styled Markdown without having to reconstruct the rendered text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChatSelection {
    anchor: (u16, u16),
    focus: (u16, u16),
}

impl ChatSelection {
    fn has_range(self) -> bool {
        self.anchor != self.focus
    }

    fn bounds(self) -> ((u16, u16), (u16, u16)) {
        if (self.anchor.1, self.anchor.0) <= (self.focus.1, self.focus.0) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

/// The interactive TUI application.
struct InteractiveTui {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    keyboard_enhancement_enabled: bool,
    transcript: TranscriptRenderer,
    editor: Editor,
    /// The last transcript rectangle rendered on screen. Mouse coordinates are
    /// relative to the terminal, so retaining this avoids duplicating the
    /// layout calculation in the input path.
    transcript_area: Rect,
    /// The currently selected range in the visible chat transcript.
    chat_selection: Option<ChatSelection>,
    /// Text from the last rendered selection, used by Ctrl+C/Cmd+C.
    selected_text: String,
    /// macOS terminals may consume Cmd+C before it reaches the application.
    /// A completed mouse selection is copied on the next render as a fallback.
    copy_selection_on_render: bool,
    /// The agent is moved into a background task while it produces a response.
    /// It is returned through `agent_completion_rx` when the task finishes.
    agent: Option<Agent>,
    agent_state: AgentState,
    token_count: u64,
    event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    agent_completion_tx: tokio::sync::mpsc::UnboundedSender<AgentRunCompletion>,
    agent_completion_rx: tokio::sync::mpsc::UnboundedReceiver<AgentRunCompletion>,
    /// The transcript entry receiving content from the current assistant
    /// stream.
    streaming_message_index: Option<usize>,
    /// Whether reasoning blocks are visible in the transcript.
    show_thinking: bool,
    /// Project directory used to shorten paths in the transcript.
    project_cwd: PathBuf,
    /// The latest short-lived UI notice. Notices are deliberately not part of
    /// the transcript or session history.
    notice: Option<EphemeralNotice>,
    /// Cancellation handle for the background agent task, if one is running.
    agent_cancellation: Option<AgentCancellation>,
    session_manager: SessionManager,
    running: bool,
    message_history: Vec<String>,
    history_index: usize,
    prompt_templates: Vec<crate::agent::prompt_templates::PromptTemplate>,
    prompt_template_options: crate::agent::prompt_templates::PromptTemplateLoadOptions,
    skills: Vec<crate::agent::skills::Skill>,
    /// CLI overrides are retained so switching providers uses the same
    /// endpoint/key precedence as the initial model.
    cli: Cli,
    settings: Settings,
    model: crate::ai::types::Model,
    models: Vec<crate::ai::types::Model>,
    model_picker: Option<ModelPicker>,
    autocomplete_items: Vec<AutocompleteItem>,
    autocomplete_index: usize,
}

/// The result of an agent task, including ownership of the agent so the next
/// user message can reuse its conversation history.
struct AgentRunCompletion {
    agent: Agent,
    result: Result<(), String>,
}

/// A completion shown for a slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AutocompleteItem {
    value: String,
    description: String,
}

/// State for the keyboard-driven model picker overlay.
#[derive(Debug, Clone, Copy)]
struct ModelPicker {
    selected: usize,
}

impl InteractiveTui {
    fn new(
        agent: Agent,
        event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        session_manager: SessionManager,
        prompt_templates: Vec<crate::agent::prompt_templates::PromptTemplate>,
        prompt_template_options: crate::agent::prompt_templates::PromptTemplateLoadOptions,
        skills: Vec<crate::agent::skills::Skill>,
        cli: Cli,
        settings: Settings,
        model: crate::ai::types::Model,
        models: Vec<crate::ai::types::Model>,
    ) -> Result<Self, anyhow::Error> {
        // Use a raw stdout writer that we own
        let stdout = std::io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("Failed to create terminal")?;

        // Enter raw mode separately. Keyboard enhancement makes modified
        // keys such as Shift+Enter distinguishable from plain Enter in
        // terminals which support the Kitty keyboard protocol.
        crossterm::terminal::enable_raw_mode().context("Failed to enable raw mode")?;
        let keyboard_enhancement_enabled = matches!(
            crossterm::terminal::supports_keyboard_enhancement(),
            Ok(true)
        );
        if keyboard_enhancement_enabled
            && let Err(error) = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::PushKeyboardEnhancementFlags(
                    crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                        | crossterm::event::KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                        | crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            )
        {
            let _ = crossterm::terminal::disable_raw_mode();
            return Err(error).context("Failed to enable keyboard enhancements");
        }
        if let Err(error) = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide,
            crossterm::event::EnableBracketedPaste
        ) {
            if keyboard_enhancement_enabled {
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::event::PopKeyboardEnhancementFlags
                );
            }
            let _ = crossterm::terminal::disable_raw_mode();
            return Err(error).context("Failed to enter alternate screen");
        }
        if let Err(error) = enable_mouse_capture() {
            if keyboard_enhancement_enabled {
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::event::PopKeyboardEnhancementFlags
                );
            }
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::DisableBracketedPaste,
                crossterm::terminal::LeaveAlternateScreen
            );
            let _ = crossterm::terminal::disable_raw_mode();
            return Err(error).context("Failed to enable mouse capture");
        }

        let project_cwd = agent
            .cwd()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let agent_state = agent.state().clone();
        let token_count = agent.estimate_tokens();
        let show_thinking = super::resolve_show_thinking(&cli, &settings);
        let (agent_completion_tx, agent_completion_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut tui = Self {
            terminal,
            keyboard_enhancement_enabled,
            transcript: {
                let mut transcript = transcript_from_messages(agent.messages());
                transcript.set_show_thinking(show_thinking);
                transcript
            },
            editor: Editor::new("").placeholder(
                "Type a message... (Shift+Enter/Ctrl+J newline, Enter send, drag chat to select, Ctrl/Cmd+C copy)"
            ),
            transcript_area: Rect::default(),
            chat_selection: None,
            selected_text: String::new(),
            copy_selection_on_render: false,
            agent: Some(agent),
            agent_state,
            token_count,
            event_rx,
            agent_completion_tx,
            agent_completion_rx,
            streaming_message_index: None,
            show_thinking,
            project_cwd,
            notice: None,
            agent_cancellation: None,
            session_manager,
            running: true,
            message_history: Vec::new(),
            history_index: 0,
            prompt_templates,
            prompt_template_options,
            skills,
            cli,
            settings,
            model,
            models,
            model_picker: None,
            autocomplete_items: Vec::new(),
            autocomplete_index: 0
        };

        tui.add_new_session_message();

        Ok(tui)
    }

    /// Set whether reasoning blocks are visible without changing conversation
    /// history or the provider request.
    fn set_show_thinking(
        &mut self,
        show: bool,
    ) {
        self.show_thinking = show;
        self.transcript.set_show_thinking(show);
    }

    /// Run the main TUI event loop. Terminal cleanup is performed even when
    /// rendering or input handling returns an error.
    async fn run(&mut self) -> Result<(), anyhow::Error> {
        self.running = true;
        let result = self.run_loop();
        self.exit_raw_mode();
        result
    }

    fn run_loop(&mut self) -> Result<(), anyhow::Error> {
        while self.running {
            self.process_events();
            self.process_completed_runs();
            self.render()?;
            if self.handle_input()? {
                break;
            }
        }
        Ok(())
    }

    /// Exit raw mode and restore terminal.
    fn exit_raw_mode(&mut self) {
        if self.keyboard_enhancement_enabled {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::PopKeyboardEnhancementFlags
            );
        }
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen,
        );
        let _ = crossterm::terminal::disable_raw_mode();
    }

    /// Process any pending agent events.
    fn process_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            if let Some((event_type, data)) = session_event(&event)
                && let Err(error) = self.session_manager.save_event(event_type, data)
            {
                eprintln!("Warning: could not persist session event: {error}");
            }
            match &event {
                AgentEvent::StateChanged { new_state, .. } => {
                    self.agent_state = new_state.clone();
                    match new_state {
                        AgentState::Streaming => {
                            // Text and reasoning are displayed as their delta
                            // events arrive.
                        },
                        AgentState::ToolExecuting => {
                            // Tool is being executed
                        },
                        AgentState::TurnComplete => {
                            // Response complete, ready for next input
                        },
                        AgentState::Error(msg) => {
                            self.show_notice(format!("❌ Error: {}", msg));
                        },
                        AgentState::Aborted => {
                            self.show_notice("Agent aborted.");
                        },
                        _ => {},
                    }
                },
                AgentEvent::AssistantTextDelta { delta, .. } => {
                    let index = match self.streaming_message_index {
                        Some(index) => index,
                        None => {
                            self.transcript.add_message(TuiMessage {
                                role: MessageRole::Assistant,
                                content: String::new(),
                                thinking: String::new(),
                                tool_calls: Vec::new(),
                                timestamp: chrono::Utc::now(),
                            });
                            let index = self.transcript.messages().len() - 1;
                            self.streaming_message_index = Some(index);
                            index
                        },
                    };
                    self.transcript.append_to_message(index, delta);
                },
                AgentEvent::AssistantThinkingDelta { delta } => {
                    let index = match self.streaming_message_index {
                        Some(index) => index,
                        None => {
                            self.transcript.add_message(TuiMessage {
                                role: MessageRole::Assistant,
                                content: String::new(),
                                thinking: String::new(),
                                tool_calls: Vec::new(),
                                timestamp: chrono::Utc::now(),
                            });
                            let index = self.transcript.messages().len() - 1;
                            self.streaming_message_index = Some(index);
                            index
                        },
                    };
                    self.transcript.append_thinking_to_message(index, delta);
                },
                AgentEvent::AssistantMessageCompleted { message, .. } => {
                    let (content, thinking, tool_calls) = display_assistant_message(message);
                    if let Some(index) = self.streaming_message_index.take() {
                        self.transcript
                            .complete_message(index, content, thinking, tool_calls);
                    } else {
                        self.transcript.add_message(TuiMessage {
                            role: MessageRole::Assistant,
                            content,
                            thinking,
                            tool_calls,
                            timestamp: chrono::Utc::now(),
                        });
                    }
                },
                AgentEvent::ToolCallDetected { .. } | AgentEvent::ToolExecutionStarted { .. } => {},
                AgentEvent::ToolExecutionCompleted {
                    tool_call_id,
                    result,
                    is_error,
                    ..
                } => {
                    self.transcript
                        .complete_tool_call(tool_call_id, result.clone(), *is_error);
                },
                AgentEvent::ToolExecutionFailed {
                    tool_call_id,
                    error,
                    ..
                } => {
                    self.transcript
                        .complete_tool_call(tool_call_id, error.clone(), true);
                },
                AgentEvent::TurnCompleted { .. } => {
                    self.session_manager.touch();
                },
                _ => {},
            }
        }
    }

    fn add_new_session_message(&mut self) {
        let mut message = String::new();

        let cwd = self
            .agent
            .as_ref()
            .and_then(|agent| agent.cwd())
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok());
        if let Some(cwd) = cwd {
            let files = crate::agent::system_prompt::prompt_file_paths(&cwd);
            let context_paths = [
                files
                    .system_prompt
                    .map(|path| crate::config::paths::display_path(path, &cwd)),
                files
                    .agents
                    .map(|path| crate::config::paths::display_path(path, &cwd)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
            if !context_paths.is_empty() {
                push_loaded_system_items_string(&mut message, "Context", &context_paths);
            }
        }

        if !self.skills.is_empty() {
            let skill_names = self
                .skills
                .iter()
                .map(|item| item.name.clone())
                .collect::<Vec<String>>()
                .join(", ");

            push_loaded_system_items_string(&mut message, "Skills", &skill_names);
        }

        if !self.prompt_templates.is_empty() {
            let prompt_template_names = self
                .prompt_templates
                .iter()
                .map(|item| format!("/{}", item.name))
                .collect::<Vec<String>>()
                .join(", ");

            push_loaded_system_items_string(&mut message, "Prompts", &prompt_template_names);
        }

        if !message.is_empty() {
            self.transcript.add_message(TuiMessage {
                role: MessageRole::System,
                content: message,
                thinking: String::new(),
                tool_calls: Vec::new(),
                timestamp: chrono::Utc::now(),
            });
        }
    }

    /// Add a system message to the transcript. This is used for command
    /// output such as help and lists, where the user needs to be able to read
    /// multiple lines after the command finishes.
    fn add_system_message(
        &mut self,
        content: impl Into<String>,
    ) {
        self.transcript.add_message(TuiMessage {
            role: MessageRole::System,
            content: content.into(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });
    }

    /// Add multiple lines of command output as one transcript entry. This
    /// keeps list-style output together instead of rendering one Notice card
    /// for every row.
    fn add_system_messages(
        &mut self,
        messages: impl IntoIterator<Item = String>,
    ) {
        let content = messages.into_iter().collect::<Vec<_>>().join("\n");
        if !content.is_empty() {
            self.add_system_message(content);
        }
    }

    fn clear_chat_selection(&mut self) {
        self.chat_selection = None;
        self.selected_text.clear();
    }

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

    fn copy_chat_selection(&mut self) {
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

    /// Show a short-lived notice in the status bar. Notices are UI feedback,
    /// not conversation or command output, so they are not added to the
    /// transcript or session history.
    fn show_notice(
        &mut self,
        content: impl Into<String>,
    ) {
        self.notice = Some(EphemeralNotice {
            message: content.into(),
            expires_at: Instant::now() + EPHEMERAL_NOTICE_DURATION,
        });
    }

    /// Remove a notice after its display window has elapsed. This runs before
    /// every draw, so the status bar clears even when no input or agent event
    /// arrives.
    fn clear_expired_notice(&mut self) {
        if self
            .notice
            .as_ref()
            .is_some_and(|notice| Instant::now() >= notice.expires_at)
        {
            self.notice = None;
        }
    }

    /// Request cancellation of the response currently running in the
    /// background task. The task remains responsible for returning the agent
    /// to the UI once its cancellation point is reached.
    fn cancel_running_response(&mut self) -> bool {
        let Some(cancellation) = &self.agent_cancellation else {
            return false;
        };
        cancellation.cancel();
        self.show_notice("Cancelling the running response.");
        true
    }

    /// Refresh slash-command completions for the current editor contents.
    fn refresh_autocomplete(&mut self) {
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
        for command in crate::modes::commands::BUILTIN_COMMANDS {
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
        let messages = self
            .session_manager
            .current_storage()
            .expect("resumed session has storage")
            .load_messages()?;
        let agent = self.agent.as_mut().expect("agent checked above");
        agent.load_history(messages.clone())?;
        self.project_cwd = PathBuf::from(&session.cwd);
        agent.set_cwd(session.cwd);
        self.transcript = transcript_from_messages(&messages);
        self.transcript.set_show_thinking(self.show_thinking);
        self.streaming_message_index = None;
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
        let arguments = crate::agent::prompt_templates::parse_command_args(arguments);
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
                    let new_provider =
                        crate::modes::create_provider_for_model(&self.cli, &self.settings, &model)?;
                    self.agent
                        .as_mut()
                        .expect("credentials require an idle agent")
                        .set_model(
                            new_provider,
                            model,
                            crate::modes::resolve_max_tokens(
                                &self.cli,
                                &self.settings,
                                &self.model,
                            ),
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
    fn handle_command(
        &mut self,
        content: &str,
    ) -> Result<bool, anyhow::Error> {
        let Some(parsed) = crate::modes::commands::parse(content) else {
            return Ok(false);
        };
        let Some(command) = crate::modes::commands::resolve_builtin_command(&parsed.name) else {
            return Ok(false);
        };
        let arguments = parsed.arguments;

        match command {
            "auth" => self.handle_auth_command(&arguments)?,
            "help" => {
                let mut lines = vec!["Built-in commands:".to_string()];
                lines.extend(
                    crate::modes::commands::BUILTIN_COMMANDS
                        .iter()
                        .map(|command| format!("{} — {}", command.usage, command.description)),
                );
                if !self.prompt_templates.is_empty() {
                    lines.push(
                        "Prompt templates: type /<name> [arguments]. Use /templates to list them."
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
            "sessions" | "list" => {
                let sessions = SessionManager::list()?;
                if sessions.is_empty() {
                    self.add_system_message("No sessions found.");
                } else {
                    let current = self.session_manager.current_id().map(str::to_owned);
                    let mut lines = Vec::with_capacity(sessions.len());
                    for session in sessions {
                        let marker = if current.as_deref() == Some(session.id.as_str()) {
                            '*'
                        } else {
                            ' '
                        };
                        let title = crate::session::storage::SessionStorage::new(&session.id)
                            .load_metadata()?
                            .and_then(|metadata| metadata.0)
                            .unwrap_or_else(|| "untitled".to_string());
                        lines.push(format!(
                            "{} {} — {} — {} ({})",
                            marker,
                            short_session_id(&session.id),
                            title,
                            session.modified_str(),
                            crate::config::paths::display_path(&session.path, &self.project_cwd)
                        ));
                    }
                    self.add_system_messages(lines);
                }
            },
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
                        crate::config::paths::display_path(&session.cwd, &self.project_cwd),
                        session.parent_session_id.as_deref().unwrap_or("none")
                    ));
                }
            },
            "templates" | "prompts" => {
                if self.prompt_templates.is_empty() {
                    self.add_system_message("No prompt templates found.");
                } else {
                    let mut lines = vec!["Prompt templates:".to_string()];
                    lines.extend(self.prompt_templates.iter().map(|template| {
                        let hint = template
                            .argument_hint
                            .as_deref()
                            .map(|hint| format!(" {hint}"))
                            .unwrap_or_default();
                        format!("/{}{} — {}", template.name, hint, template.description)
                    }));
                    self.add_system_messages(lines);
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
                let arguments = crate::agent::prompt_templates::parse_command_args(&arguments);
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
                let agent = self.agent.as_mut().expect("agent is idle");
                agent.compact()?;
                self.token_count = agent.estimate_tokens();
                self.show_notice("Compacted the current context.");
            },
            "reload" => {
                let result = crate::agent::prompt_templates::load_prompt_templates(
                    &self.prompt_template_options,
                );
                let diagnostics =
                    result
                        .diagnostics
                        .iter()
                        .map(|diagnostic| {
                            format!(
                                "Warning: could not load prompt template {}: {}",
                                crate::config::paths::display_path(
                                    &diagnostic.path,
                                    &self.project_cwd,
                                ),
                                diagnostic.message
                            )
                        })
                        .collect::<Vec<_>>();
                self.add_system_messages(diagnostics);
                self.prompt_templates = result.templates;
                self.show_notice(format!(
                    "Reloaded {} prompt template{}.",
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

    /// Handle a key while the model picker owns the input focus.
    fn handle_model_picker_key(
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
        model: crate::ai::types::Model,
    ) -> Result<(), anyhow::Error> {
        if !self.ensure_idle("switching models") {
            return Ok(());
        }
        let provider = crate::modes::create_provider_for_model(&self.cli, &self.settings, &model)?;
        let max_tokens = crate::modes::resolve_max_tokens(&self.cli, &self.settings, &model);
        self.agent
            .as_mut()
            .expect("model changes require an idle agent")
            .set_model(provider, model.clone(), max_tokens);
        self.model = model.clone();
        self.token_count = self
            .agent
            .as_ref()
            .expect("agent was not moved")
            .estimate_tokens();
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
            .create(crate::session::manager::SessionCreateOptions {
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
        self.streaming_message_index = None;
        self.message_history.clear();
        self.history_index = 0;
        self.token_count = agent.estimate_tokens();

        self.add_new_session_message();

        Ok(())
    }

    /// Handle user input.
    fn handle_input(&mut self) -> Result<bool, anyhow::Error> {
        // Check for quit
        if crossterm::event::poll(std::time::Duration::from_millis(100))? {
            let event = crossterm::event::read()?;
            let input_event = parse_event(event);

            // Esc cancels the active agent task instead of being passed to the
            // editor. Keep the editor contents intact so a draft can be sent
            // after the response has stopped; when idle, Esc retains its
            // normal editor-cancel behavior below.
            if matches!(
                &input_event,
                crate::tui::input::InputEvent::Key(Key {
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
                crate::tui::input::InputEvent::Key(Key {
                    code: crossterm::event::KeyCode::Esc,
                    ..
                })
            );

            match &input_event {
                crate::tui::input::InputEvent::Resize(width, height) => {
                    // Ratatui recalculates its viewport on the next draw; read
                    // both dimensions so resize events are not discarded.
                    let _ = (width, height);
                    self.clear_chat_selection();
                    self.terminal.autoresize()?;
                    return Ok(false);
                },
                crate::tui::input::InputEvent::Mouse(mouse) => {
                    if let Some(picker) = self.model_picker.as_mut() {
                        match mouse.kind {
                            crossterm::event::MouseEventKind::ScrollUp => {
                                picker.selected = picker.selected.saturating_sub(3);
                            },
                            crossterm::event::MouseEventKind::ScrollDown => {
                                picker.selected = (picker.selected + 3).min(self.models.len() - 1);
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
                            self.clear_chat_selection();
                            self.transcript.scroll_up(3);
                        },
                        MouseEventKind::ScrollDown => {
                            self.clear_chat_selection();
                            self.transcript.scroll_down(3);
                        },
                        _ => {},
                    }
                    return Ok(false);
                },
                _ => {},
            }

            // The model picker owns keyboard input while it is open. This
            // prevents arrows and Enter from moving/submitting the editor.
            if self.model_picker.is_some() {
                if let crate::tui::input::InputEvent::Key(key) = input_event {
                    self.handle_model_picker_key(key)?;
                }
                return Ok(false);
            }

            if let crate::tui::input::InputEvent::Key(key) = input_event {
                // Tab accepts the selected slash-command completion. Up and
                // Down select suggestions while the popup is visible; when
                // it is hidden they retain their transcript-scroll behavior.
                if !key.ctrl
                    && !key.alt
                    && matches!(key.code, crossterm::event::KeyCode::Tab)
                    && !self.autocomplete_items.is_empty()
                {
                    self.accept_autocomplete();
                    return Ok(false);
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
                    if key.ctrl {
                        if let Some(cancellation) = &self.agent_cancellation {
                            cancellation.cancel();
                        }
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
                    self.transcript.scroll_up(3);
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
    fn send_message(
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
        let content = match crate::agent::skills::expand_skill_command(content, &self.skills) {
            Ok(content) => content,
            Err(error) => {
                self.show_notice(format!("Skill expansion failed: {error}"));
                return Ok(());
            },
        };
        let content = crate::agent::prompt_templates::expand_prompt_template(
            &content,
            &self.prompt_templates,
        );

        // A cancelled/failed stream may have left a partial assistant card;
        // start the next response in a fresh card.
        self.streaming_message_index = None;

        // Add user message to transcript
        self.transcript.add_message(TuiMessage {
            role: MessageRole::User,
            content: content.clone(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });

        // Save to session
        self.session_manager
            .save_message(&Message::User(crate::ai::types::UserMessage::new(&content)))?;

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
        self.token_count = agent.estimate_tokens();

        self.agent_cancellation = Some(agent.cancellation());
        let completion_tx = self.agent_completion_tx.clone();
        tokio::spawn(async move {
            let result = agent
                .run()
                .await
                .map(|_| ())
                .map_err(|error| error.to_string());
            let _ = completion_tx.send(AgentRunCompletion { agent, result });
        });

        Ok(())
    }

    /// Restore completed agents and display their responses without blocking
    /// terminal input or rendering.
    fn process_completed_runs(&mut self) {
        while let Ok(completion) = self.agent_completion_rx.try_recv() {
            self.token_count = completion.agent.estimate_tokens();
            self.agent_cancellation = None;
            self.agent = Some(completion.agent);

            let agent = self.agent.as_ref().expect("completed agent is restored");
            self.agent_state = agent.state().clone();

            // The user message is written before the background task starts.
            // Persist every assistant and tool message which is not already in
            // the archive, including messages produced after compaction. A
            // count-based index is not safe here because `/compact` replaces
            // the in-memory history while deliberately retaining the complete
            // archived session.
            let mut persisted_messages = self
                .session_manager
                .current_storage()
                .and_then(|storage| match storage.load_messages() {
                    Ok(messages) => Some(messages),
                    Err(error) => {
                        eprintln!("Warning: could not inspect persisted messages: {error}");
                        None
                    },
                })
                .unwrap_or_default();
            for message in agent.messages() {
                if matches!(message, Message::User(_)) {
                    continue;
                }
                let already_persisted = persisted_messages.iter().any(|persisted| {
                    serde_json::to_string(persisted).ok() == serde_json::to_string(message).ok()
                });
                if already_persisted {
                    continue;
                }
                if let Err(error) = self.session_manager.save_message(message) {
                    eprintln!("Warning: could not persist session message: {error}");
                } else {
                    persisted_messages.push(message.clone());
                }
            }
            if let Err(error) = completion.result
                && !matches!(self.agent_state, AgentState::Aborted | AgentState::Error(_))
            {
                // The matching Error/Aborted event has already been consumed
                // by process_events; the status bar is the single visible
                // error presentation here.
                self.agent_state = AgentState::Error(error);
            }
        }
    }

    /// Render the current state.
    fn render(&mut self) -> Result<(), anyhow::Error> {
        self.clear_expired_notice();
        let selection = self.chat_selection;
        let mut rendered_transcript_area = Rect::default();
        let mut rendered_selection_text = String::new();
        self.terminal.draw(|frame| {
            use ratatui::{
                layout::{Constraint, Layout},
                style::{Color, Modifier, Style},
                text::{Line, Span},
                widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap},
            };

            let autocomplete_height = if self.autocomplete_items.is_empty() {
                0
            } else {
                // Five rows are enough to make completion useful without
                // hiding the conversation on a small terminal. Two extra
                // rows are reserved for the popup border.
                (self.autocomplete_items.len().min(5) + 2) as u16
            };
            let mut constraints = vec![
                Constraint::Length(1), // Status bar
                Constraint::Min(1),    // Transcript
            ];
            if autocomplete_height > 0 {
                constraints.push(Constraint::Length(autocomplete_height));
            }
            let editor_height = self.editor.desired_height(frame.area().width).min(
                frame
                    .area()
                    .height
                    .saturating_sub(2 + autocomplete_height)
                    .max(3),
            );
            constraints.push(Constraint::Length(editor_height)); // Editor area
            let chunks = Layout::default()
                .constraints(constraints)
                .split(frame.area());
            let editor_chunk = chunks.len() - 1;

            // Status bar
            let state_str = match &self.agent_state {
                AgentState::Idle => "idle".to_string(),
                AgentState::Streaming => "streaming...".to_string(),
                AgentState::ToolExecuting => "executing tools...".to_string(),
                AgentState::TurnComplete => "response complete".to_string(),
                AgentState::Error(message) => format!("error: {}", message),
                AgentState::Aborted => "aborted".to_string(),
            };

            let session_id = self
                .session_manager
                .current_id()
                .map(|id| &id[..8.min(id.len())])
                .unwrap_or("none");

            let token_count = self.token_count;

            let status_text = Line::from(vec![
                Span::styled(
                    "hallucinate ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ),
                if let Some(notice) = self.notice.as_ref() {
                    Span::styled(
                        format!("! {}  |  ", truncate_one_line(&notice.message, 80)),
                        Style::default().fg(Color::LightYellow),
                    )
                } else {
                    Span::styled("", Style::default())
                },
                Span::styled("[", Style::default().fg(Color::DarkGray)),
                Span::styled(state_str, Style::default().fg(Color::Yellow)),
                Span::styled("] ", Style::default().fg(Color::DarkGray)),
                Span::styled("session:", Style::default().fg(Color::DarkGray)),
                Span::styled(session_id, Style::default().fg(Color::Green)),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} tokens", token_count),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("model: {}/{}", self.model.provider, self.model.id),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if self.show_thinking {
                        "thinking: on"
                    } else {
                        "thinking: off"
                    },
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            let status = Paragraph::new(status_text);
            frame.render_widget(status, chunks[0]);

            // Transcript
            //
            // Messages use a light-weight chat-card treatment instead of a
            // stream of bracketed labels. The rounded header and accent rail
            // give each turn a clear visual boundary while leaving the
            // Markdown renderer in charge of the actual message content.
            //
            // Paragraph's word wrapper does not repeat a line's prefix on
            // continuation rows. Wrap message content before adding the
            // decorative rail so long paragraphs keep the rail aligned and
            // do not appear to spill out of their message card.
            let transcript_block = Block::default()
                .borders(Borders::NONE)
                .padding(Padding::horizontal(1));
            let transcript_inner_area = transcript_block.inner(chunks[1]);
            let transcript_width = transcript_inner_area.width;
            let mut selection_line_starts = Vec::new();
            let lines: Vec<Line<'static>> = self
                .transcript
                .messages()
                .iter()
                .enumerate()
                .flat_map(|(message_index, msg)| {
                    let (role_color, icon, role_label) = match msg.role {
                        MessageRole::User => (Color::LightGreen, "›", "You"),
                        MessageRole::Assistant => (Color::LightCyan, "✦", "Assistant"),
                        MessageRole::Tool => (Color::LightMagenta, "⚙", "Tool"),
                        MessageRole::System => (Color::DarkGray, "·", "Notice"),
                    };
                    let muted = Style::default().fg(Color::DarkGray);
                    let role_style = Style::default().fg(role_color).add_modifier(Modifier::BOLD);
                    let mut lines = vec![Line::from(vec![
                        Span::styled("╭─ ", muted),
                        Span::styled(icon, role_style),
                        Span::styled(format!(" {}", role_label), role_style),
                    ])];
                    let mut line_starts = vec![None];

                    if self.transcript.show_timestamps() {
                        lines[0].spans.push(Span::styled("  · ", muted));
                        lines[0].spans.push(Span::styled(
                            msg.timestamp.format("%H:%M:%S").to_string(),
                            muted.add_modifier(Modifier::ITALIC),
                        ));
                    }

                    if self.show_thinking && !msg.thinking.is_empty() {
                        // Reasoning is deliberately styled as a secondary
                        // surface: muted text plus a subtle background makes
                        // it visually distinct from the answer while keeping
                        // it readable during long streams.
                        let thinking_style = Style::default()
                            .fg(Color::Gray)
                            .bg(Color::Rgb(45, 45, 45))
                            .add_modifier(Modifier::DIM);
                        lines.push(Line::from(vec![
                            Span::styled("│ ", thinking_style),
                            Span::styled(
                                "💭 Thinking",
                                thinking_style.add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                        line_starts.push(None);
                        let mut thinking = markdown_to_lines(&msg.thinking);
                        while thinking.last().is_some_and(line_is_empty) {
                            thinking.pop();
                        }
                        for line in thinking {
                            for line in wrap_line_to_width(line, transcript_width.saturating_sub(2))
                            {
                                lines.push(muted_thinking_line(line, thinking_style));
                                line_starts.push(None);
                            }
                        }
                    }

                    let mut body = markdown_to_lines(&msg.content);
                    while body.last().is_some_and(line_is_empty) {
                        body.pop();
                    }
                    if body.is_empty() {
                        let mut empty_line = vec![Span::styled("│ ", muted_for(role_color))];
                        if self.streaming_message_index == Some(message_index) {
                            empty_line.push(Span::styled(
                                "thinking…",
                                muted.add_modifier(Modifier::ITALIC),
                            ));
                        }
                        lines.push(Line::from(empty_line));
                        line_starts.push(None);
                    } else {
                        let selectable_start =
                            matches!(msg.role, MessageRole::User | MessageRole::Assistant)
                                .then_some(2);
                        for line in body {
                            for line in wrap_line_to_width(line, transcript_width.saturating_sub(2))
                            {
                                lines.push(prefix_message_line(line, role_color));
                                // The first two cells are the decorative rail;
                                // only user and assistant message bodies are
                                // selectable.
                                line_starts.push(selectable_start);
                            }
                        }
                    }

                    for tool_call in &msg.tool_calls {
                        let previous_len = lines.len();
                        let readable = tool_call.human_readable_with_cwd(&self.project_cwd);
                        append_prefixed_lines(
                            &mut lines,
                            &format!("├─ {}", &readable),
                            "   ",
                            Style::default().fg(Color::LightBlue),
                        );
                        line_starts.extend(vec![None; lines.len() - previous_len]);
                        if let Some(result) = &tool_call.result {
                            let result_style = if result.starts_with("❌") {
                                Style::default().fg(Color::LightRed)
                            } else {
                                Style::default().fg(Color::LightGreen)
                            };
                            let previous_len = lines.len();
                            append_prefixed_lines(
                                &mut lines,
                                &format!("│  {}", truncate_width(result, 100)),
                                "   ",
                                result_style,
                            );
                            line_starts.extend(vec![None; lines.len() - previous_len]);
                        }
                    }
                    lines.push(Line::from(Span::styled("╰─", muted_for(role_color))));
                    line_starts.push(None);
                    lines.push(Line::from(""));
                    line_starts.push(None);
                    selection_line_starts.extend(line_starts);
                    lines
                })
                .collect();
            debug_assert_eq!(lines.len(), selection_line_starts.len());
            let selection_mask_lines = lines
                .iter()
                .zip(selection_line_starts.iter().copied())
                .map(|(line, start)| selection_mask_line(line, start))
                .collect::<Vec<_>>();

            // A Line is deliberately a single visual line in ratatui: an
            // embedded newline in a message would otherwise not create a new
            // row. The helpers above expand those newlines and pre-wrap
            // message content, while Paragraph's wrapping keeps any bounded
            // tool output inside the viewport. A little horizontal breathing
            // room makes the transcript read like a conversation rather than
            // a log file.
            let transcript = Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(transcript_block);
            let scroll = self.transcript.scroll_position(
                transcript.line_count(transcript_width),
                chunks[1].height as usize,
            );
            let transcript = transcript.scroll((scroll, 0));

            frame.render_widget(transcript, chunks[1]);
            rendered_transcript_area = chunks[1];

            // Mouse capture normally prevents the terminal emulator from
            // selecting text itself. Paint the selection into ratatui's
            // buffer so drag selection works consistently across terminals.
            // Reading the same cells also gives Ctrl+C/Cmd+C the exact text
            // the user can see, including wrapped lines and Markdown styling.
            if let Some(selection) = selection.filter(|selection| selection.has_range()) {
                let ((start_x, start_y), (end_x, end_y)) = selection.bounds();
                let area = chunks[1];
                let selection_style = Style::default().fg(Color::Black).bg(Color::White);
                let mut mask_buffer = ratatui::buffer::Buffer::empty(area);
                let mask = Paragraph::new(selection_mask_lines)
                    .wrap(Wrap { trim: false })
                    .scroll((scroll, 0));
                mask.render(transcript_inner_area, &mut mask_buffer);

                let buffer = frame.buffer_mut();
                let mut selected_rows = Vec::new();
                for row in start_y..=end_y {
                    let from = if row == start_y { start_x } else { area.x };
                    let to = if row == end_y {
                        end_x
                    } else {
                        area.x.saturating_add(area.width).saturating_sub(1)
                    };
                    let mut selected_row = String::new();
                    let mut row_has_selectable_text = false;
                    for column in from..=to {
                        let selectable = mask_buffer
                            .cell((column, row))
                            .is_some_and(|cell| cell.fg == SELECTION_MARKER);
                        if selectable {
                            row_has_selectable_text = true;
                            if let Some(cell) = buffer.cell_mut((column, row)) {
                                selected_row.push_str(cell.symbol());
                                cell.set_style(selection_style);
                            }
                        }
                    }
                    if row_has_selectable_text {
                        selected_rows.push(selected_row.trim_end().to_string());
                    }
                }
                rendered_selection_text = selected_rows.join("\n");
            }

            if autocomplete_height > 0 {
                let start = self.autocomplete_index.saturating_sub(4);
                let popup_inner_width = usize::from(chunks[2].width.saturating_sub(2));
                let suggestions = self
                    .autocomplete_items
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(5)
                    .map(|(index, item)| {
                        let marker = if index == self.autocomplete_index {
                            '>'
                        } else {
                            ' '
                        };
                        let command = format!("{marker} /{}", item.value);
                        let command_width = command
                            .chars()
                            .map(|ch| ch.width().unwrap_or(0))
                            .sum::<usize>();
                        let description_width =
                            popup_inner_width.saturating_sub(command_width + " — ".width());
                        let description = truncate_one_line(&item.description, description_width);
                        Line::from(vec![
                            Span::styled(command, Style::default().fg(Color::Cyan)),
                            Span::styled(
                                format!(" — {description}"),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ])
                    })
                    .collect::<Vec<_>>();
                let suggestions = Paragraph::new(suggestions)
                    .block(Block::default().borders(Borders::ALL).title(" Commands "));
                frame.render_widget(suggestions, chunks[2]);
            }

            // Editor
            self.editor.render(frame, chunks[editor_chunk]);

            // Model picker overlay. It is rendered last so it remains visible
            // above the transcript, autocomplete popup, and editor.
            if let Some(picker) = self.model_picker {
                let area = frame.area();
                let popup_width = area.width.saturating_sub(4).min(90);
                let popup_height = area
                    .height
                    .saturating_sub(4)
                    .min((self.models.len() + 2).min(18) as u16);
                if popup_width > 4 && popup_height > 2 {
                    let popup_area = ratatui::layout::Rect::new(
                        area.x + (area.width - popup_width) / 2,
                        area.y + (area.height - popup_height) / 2,
                        popup_width,
                        popup_height,
                    );
                    let visible_rows = usize::from(popup_height.saturating_sub(2));
                    let start = picker
                        .selected
                        .saturating_sub(visible_rows / 2)
                        .min(self.models.len().saturating_sub(visible_rows));
                    let rows = self
                        .models
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(visible_rows)
                        .map(|(index, model)| {
                            let marker = if index == picker.selected { "> " } else { "  " };
                            let metadata = format!(
                                "context {} · max {} · {}{}",
                                model.context_window,
                                model.max_tokens,
                                model.input.join(","),
                                if model.reasoning { " · reasoning" } else { "" }
                            );
                            Line::from(vec![
                                Span::styled(
                                    marker,
                                    Style::default().fg(if index == picker.selected {
                                        Color::Yellow
                                    } else {
                                        Color::DarkGray
                                    }),
                                ),
                                Span::styled(
                                    format!("{}/{} — {} ", model.provider, model.id, model.name),
                                    Style::default().fg(Color::Cyan),
                                ),
                                Span::styled(metadata, Style::default().fg(Color::DarkGray)),
                            ])
                        })
                        .collect::<Vec<_>>();
                    frame.render_widget(Clear, popup_area);
                    // Keep each model on one terminal row. Wrapping made
                    // the index-based scroll window disagree with the number
                    // of visual rows, so the selected model could disappear
                    // below the popup.
                    frame.render_widget(
                        Paragraph::new(rows).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" Select model · ↑↓ PgUp/PgDn Home/End Enter Esc "),
                        ),
                        popup_area,
                    );
                }
            }
        })?;
        self.transcript_area = rendered_transcript_area;
        self.selected_text = rendered_selection_text;
        if self.copy_selection_on_render {
            self.copy_selection_on_render = false;
            self.copy_chat_selection();
        }

        Ok(())
    }
}

/// Enable mouse reporting so the application can constrain selections to
/// message text instead of allowing the terminal to select UI decorations.
fn enable_mouse_capture() -> io::Result<()> {
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

/// Parse a command's arguments as shell-like words and use them as a
/// human-readable value (for example, `/new "fix auth"`).
fn command_arguments(arguments: &str) -> Option<String> {
    let arguments = crate::agent::prompt_templates::parse_command_args(arguments);
    (!arguments.is_empty()).then(|| arguments.join(" "))
}

/// Resolve a complete session ID or an unambiguous ID prefix.
fn resolve_session_id(input: &str) -> Result<String, anyhow::Error> {
    let input = input.trim();
    if input.is_empty() {
        return Err(anyhow::anyhow!("session id cannot be empty"));
    }
    if crate::session::storage::SessionStorage::new(input).exists() {
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

/// Convert lifecycle events into durable session entries. Streaming text and
/// thinking deltas are intentionally omitted: the completed assistant message
/// is saved as a message entry, while these events preserve operational
/// history.
pub(crate) fn session_event(event: &AgentEvent) -> Option<(&'static str, serde_json::Value)> {
    match event {
        AgentEvent::StateChanged {
            new_state,
            old_state,
        } => Some((
            "state_changed",
            serde_json::json!({ "old_state": old_state.to_string(), "new_state": new_state.to_string() }),
        )),
        AgentEvent::TurnStarted { turn_number } => Some((
            "turn_started",
            serde_json::json!({ "turn_number": turn_number }),
        )),
        AgentEvent::ToolCallDetected {
            tool_call_id,
            tool_name,
            arguments,
        } => Some((
            "tool_call_detected",
            serde_json::json!({ "tool_call_id": tool_call_id, "tool_name": tool_name, "arguments": arguments }),
        )),
        AgentEvent::ToolExecutionStarted {
            tool_call_id,
            tool_name,
        } => Some((
            "tool_execution_started",
            serde_json::json!({ "tool_call_id": tool_call_id, "tool_name": tool_name }),
        )),
        AgentEvent::ToolExecutionCompleted {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => Some((
            "tool_execution_completed",
            serde_json::json!({ "tool_call_id": tool_call_id, "tool_name": tool_name, "result": result, "is_error": is_error }),
        )),
        AgentEvent::ToolExecutionFailed {
            tool_call_id,
            tool_name,
            error,
        } => Some((
            "tool_execution_failed",
            serde_json::json!({ "tool_call_id": tool_call_id, "tool_name": tool_name, "error": error }),
        )),
        AgentEvent::TurnCompleted {
            turn_number,
            stop_reason,
        } => Some((
            "turn_completed",
            serde_json::json!({ "turn_number": turn_number, "stop_reason": stop_reason.to_string() }),
        )),
        AgentEvent::Error { message } => Some(("error", serde_json::json!({ "message": message }))),
        AgentEvent::CompactionPerformed {
            tokens_before,
            tokens_after,
            summaries_added,
        } => Some((
            "compaction",
            serde_json::json!({ "tokens_before": tokens_before, "tokens_after": tokens_after, "summaries_added": summaries_added }),
        )),
        AgentEvent::Aborted => Some(("aborted", serde_json::json!({}))),
        AgentEvent::AssistantTextDelta { .. }
        | AgentEvent::AssistantThinkingDelta { .. }
        | AgentEvent::AssistantMessageCompleted { .. } => None,
    }
}

/// Whether a rendered Markdown line has no visible content.
fn line_is_empty(line: &ratatui::text::Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.is_empty())
}

const SELECTION_MARKER: ratatui::style::Color = ratatui::style::Color::Rgb(1, 2, 3);
const NON_SELECTION_MARKER: ratatui::style::Color = ratatui::style::Color::Rgb(4, 5, 6);

/// Make a copy of a rendered line for the selection mask. Keeping the original
/// symbols means ratatui applies exactly the same word wrapping to the mask as
/// it applies to the visible transcript; only the foreground color differs.
fn selection_mask_line(
    line: &ratatui::text::Line<'static>,
    selectable_start: Option<usize>,
) -> ratatui::text::Line<'static> {
    let mut column = 0;
    let spans = line
        .spans
        .iter()
        .map(|span| {
            let selectable = selectable_start.is_some_and(|start| column >= start);
            column += span.content.width();
            ratatui::text::Span::styled(
                span.content.clone(),
                ratatui::style::Style::default().fg(if selectable {
                    SELECTION_MARKER
                } else {
                    NON_SELECTION_MARKER
                }),
            )
        })
        .collect();
    ratatui::text::Line {
        style: line.style,
        alignment: line.alignment,
        spans,
    }
}

/// Wrap a styled Markdown line to the available content width.
///
/// Ratatui's `Paragraph` wrapper is intentionally used here so word breaking,
/// whitespace, and Unicode graphemes behave exactly as they do in the final
/// widget. Rendering into a temporary buffer lets us turn the wrapped visual
/// rows back into `Line`s without losing inline styles.
fn wrap_line_to_width(
    line: ratatui::text::Line<'static>,
    width: u16,
) -> Vec<ratatui::text::Line<'static>> {
    if width == 0 {
        return vec![ratatui::text::Line::from("")];
    }

    let line_style = line.style;
    let alignment = line.alignment;
    let paragraph =
        ratatui::widgets::Paragraph::new(line).wrap(ratatui::widgets::Wrap { trim: false });
    let line_count = paragraph.line_count(width).max(1);
    let height = u16::try_from(line_count).unwrap_or(u16::MAX);
    let area = ratatui::layout::Rect::new(0, 0, width, height);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Widget::render(paragraph, area, &mut buffer);

    (0..height)
        .map(|row| {
            let mut spans = Vec::new();
            let mut column = 0;
            while column < width {
                let cell = buffer
                    .cell((column, row))
                    .expect("temporary wrapping buffer contains every cell");
                let symbol = cell.symbol();
                let symbol_width = UnicodeWidthStr::width(symbol) as u16;
                if symbol_width == 0 {
                    column += 1;
                    continue;
                }
                spans.push(ratatui::text::Span::styled(
                    symbol.to_string(),
                    cell.style(),
                ));
                // A wide grapheme occupies more than one buffer cell. Skip
                // its continuation cells so it is not emitted twice when the
                // reconstructed line is rendered again.
                column = column.saturating_add(symbol_width).min(width);
            }
            while spans
                .last()
                .is_some_and(|span| span.content.as_ref() == " ")
            {
                spans.pop();
            }
            ratatui::text::Line {
                style: line_style,
                alignment,
                spans,
            }
        })
        .collect()
}

/// Prefix a Markdown line with the transcript's accent rail while preserving
/// all of the inline styles produced by the Markdown renderer.
fn prefix_message_line(
    line: ratatui::text::Line<'static>,
    accent: ratatui::style::Color,
) -> ratatui::text::Line<'static> {
    let mut spans = vec![ratatui::text::Span::styled(
        "│ ",
        ratatui::style::Style::default().fg(accent),
    )];
    spans.extend(line.spans);
    ratatui::text::Line::from(spans)
}

/// Render a reasoning line on the muted thinking surface. Markdown emphasis
/// is intentionally flattened here so the entire block stays visually quiet.
fn muted_thinking_line(
    line: ratatui::text::Line<'static>,
    style: ratatui::style::Style,
) -> ratatui::text::Line<'static> {
    let mut spans = vec![ratatui::text::Span::styled("│ ", style)];
    if line.spans.is_empty() {
        spans.push(ratatui::text::Span::styled(" ", style));
    } else {
        spans.extend(
            line.spans
                .into_iter()
                .map(|span| ratatui::text::Span::styled(span.content.into_owned(), style)),
        );
    }
    ratatui::text::Line::from(spans)
}

/// Use a dim version of a message's accent for structural card elements.
fn muted_for(accent: ratatui::style::Color) -> ratatui::style::Style {
    ratatui::style::Style::default()
        .fg(accent)
        .add_modifier(ratatui::style::Modifier::DIM)
}

/// Append text that may contain newlines, using the supplied visual prefix on
/// continuation lines (useful for tool calls and results).
fn append_prefixed_lines<'a>(
    lines: &mut Vec<ratatui::text::Line<'a>>,
    text: &str,
    continuation_prefix: &str,
    style: ratatui::style::Style,
) {
    for (index, line) in text.split('\n').enumerate() {
        let line = line.trim_end_matches('\r');
        let content = if index == 0 {
            line.to_string()
        } else {
            format!("{continuation_prefix}{line}")
        };
        lines.push(ratatui::text::Line::from(ratatui::text::Span::styled(
            content, style,
        )));
    }
}

/// Rebuild the visible transcript from persisted conversation history.
fn transcript_from_messages(messages: &[Message]) -> TranscriptRenderer {
    let mut transcript = TranscriptRenderer::new();
    for message in messages {
        match message {
            Message::User(message) => transcript.add_message(TuiMessage {
                role: MessageRole::User,
                content: message.content.clone(),
                thinking: String::new(),
                tool_calls: Vec::new(),
                timestamp: timestamp_from_millis(message.timestamp),
            }),
            Message::Assistant(message) => {
                let (content, thinking, tool_calls) = display_assistant_message(message);
                transcript.add_message(TuiMessage {
                    role: MessageRole::Assistant,
                    content,
                    thinking,
                    tool_calls,
                    timestamp: timestamp_from_millis(message.timestamp),
                });
            },
            Message::ToolResult(message) => {
                let content = message
                    .content
                    .iter()
                    .map(|item| item.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !transcript.complete_tool_call(
                    &message.tool_call_id,
                    content.clone(),
                    message.is_error,
                ) {
                    transcript.add_message(TuiMessage {
                        role: MessageRole::Tool,
                        content: format!("{}: {}", message.tool_name, content),
                        thinking: String::new(),
                        tool_calls: Vec::new(),
                        timestamp: timestamp_from_millis(message.timestamp),
                    });
                }
            },
        }
    }
    transcript
}

/// Convert an agent response into the transcript representation.
fn display_assistant_message(message: &AssistantMessage) -> (String, String, Vec<TuiToolCall>) {
    let tool_calls = message
        .content
        .iter()
        .filter_map(|content| {
            if let AssistantContent::ToolCall(tool_call) = content {
                Some(TuiToolCall {
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    arguments: tool_call.arguments.to_string(),
                    result: None,
                })
            } else {
                None
            }
        })
        .collect();

    let text = message
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    let thinking = message
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Thinking(thinking) => Some(thinking.thinking.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    (text, thinking, tool_calls)
}

fn timestamp_from_millis(timestamp: u64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(timestamp as i64).unwrap_or_else(chrono::Utc::now)
}

fn short_session_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// Truncate a string to one display line and a maximum display width.
fn truncate_one_line(
    s: &str,
    max_width: usize,
) -> String {
    let single_line = s.replace(['\r', '\n'], " ");
    truncate_width(&single_line, max_width)
}

/// Truncate a string to a maximum display width.
fn truncate_width(
    s: &str,
    max_width: usize,
) -> String {
    let total_width: usize = s.chars().map(|ch| ch.width().unwrap_or(0)).sum();
    if total_width <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }

    let mut result = String::with_capacity(max_width);
    let mut current_width = 0;

    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if current_width + ch_width > max_width - 1 {
            break;
        }
        current_width += ch_width;
        result.push(ch);
    }

    result.push('…');
    result
}

fn push_loaded_system_items_string(
    message: &mut String,
    section_name: &str,
    section_items: &str,
) {
    let section_name = format!("## {}", section_name);

    message.push_str(&section_name);
    message.push_str("\n\n");
    message.push_str("```\n");
    message.push_str(section_items);
    message.push_str("\n```\n");
    message.push_str("\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{Api, StopReason, ToolCall, ToolResultMessage};

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

    #[test]
    fn selection_mask_excludes_the_message_rail() {
        let line = prefix_message_line(
            ratatui::text::Line::from("answer"),
            ratatui::style::Color::LightCyan,
        );
        let mask = selection_mask_line(&line, Some(2));
        assert_eq!(mask.spans[0].style.fg, Some(NON_SELECTION_MARKER));
        assert_eq!(mask.spans[1].style.fg, Some(SELECTION_MARKER));
    }

    #[test]
    fn long_message_rows_keep_their_rail_when_wrapped() {
        let rows = wrap_line_to_width(ratatui::text::Line::from("one two three four five"), 8)
            .into_iter()
            .map(|line| prefix_message_line(line, ratatui::style::Color::LightCyan))
            .collect::<Vec<_>>();

        assert!(rows.len() > 1);
        for row in rows {
            assert!(row.width() <= 10);
            assert!(String::from(row).starts_with("│ "));
        }
    }

    #[test]
    fn non_message_lines_are_not_selectable() {
        let line = ratatui::text::Line::from("tool output");
        let mask = selection_mask_line(&line, None);
        assert_eq!(mask.spans[0].style.fg, Some(NON_SELECTION_MARKER));
    }

    #[test]
    fn persisted_tool_results_reconstruct_the_original_transcript_call() {
        let messages = vec![
            Message::User(crate::ai::types::UserMessage::new("run it")),
            Message::Assistant(AssistantMessage::new(
                vec![AssistantContent::ToolCall(ToolCall::new(
                    "call-1",
                    "bash",
                    serde_json::json!({ "command": "printf ok" }),
                ))],
                Api::OpenaiCompletions,
                "openai",
                "test",
                StopReason::ToolUse,
            )),
            Message::ToolResult(ToolResultMessage::new("call-1", "bash", "ok", false)),
        ];

        let transcript = transcript_from_messages(&messages);
        assert_eq!(transcript.messages().len(), 2);
        assert_eq!(transcript.messages()[0].role, MessageRole::User);
        assert_eq!(
            transcript.messages()[1].tool_calls[0].result.as_deref(),
            Some("✅ ok")
        );
    }

    #[test]
    fn markdown_display_preserves_source_line_breaks() {
        let rendered = markdown_to_lines("first\r\nsecond")
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        assert_eq!(rendered, vec!["first", "second"]);
    }

    #[test]
    fn width_truncation_does_not_split_wide_unicode() {
        let truncated = truncate_width("abc😀def", 5);
        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn autocomplete_description_truncation_stays_on_one_line() {
        let truncated = truncate_one_line("first line\nsecond line", 12);
        assert_eq!(truncated, "first line …");
        assert!(!truncated.contains('\n'));
    }

    #[test]
    fn assistant_display_keeps_thinking_separate_from_answer_text() {
        let message = AssistantMessage::new(
            vec![
                AssistantContent::Thinking(crate::ai::types::ThinkingContent::new("plan")),
                AssistantContent::Text(crate::ai::types::TextContent::new("answer")),
            ],
            Api::OpenaiCompletions,
            "test",
            "model",
            StopReason::Stop,
        );
        let (content, thinking, tools) = display_assistant_message(&message);
        assert_eq!(content, "answer");
        assert_eq!(thinking, "plan");
        assert!(tools.is_empty());
    }
}
