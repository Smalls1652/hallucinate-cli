//! Interactive mode and the terminal application lifecycle.
//!
//! Input handling, command dispatch, session synchronization, and rendering
//! live in focused child modules so this module only owns shared state and
//! terminal setup.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context;
use hallucinate_agent::{
    agent::{
        runtime::Agent,
        types::{AgentCancellation, AgentConfig, AgentEvent, AgentState},
    },
    config::settings::Settings,
};
use hallucinate_tui::{
    alt_screen::{Message as TuiMessage, MessageRole, TranscriptRenderer},
    components::Editor,
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

use crate::{cli::args::Cli, session::manager::SessionManager};

mod commands;
mod input;
mod render;
mod session;

pub(crate) use session::session_event;

/// Run interactive mode.
pub async fn run(
    cli: &Cli,
    settings: &Settings,
) -> Result<(), anyhow::Error> {
    // Ensure directories exist
    hallucinate_agent::config::paths::ensure_directories()?;

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

    let skills = hallucinate_agent::agent::skills::load_skills(
        &hallucinate_agent::agent::skills::SkillLoadOptions {
            cwd: cwd.clone(),
            skill_paths: cli.skills.clone(),
            include_defaults: !cli.no_skills,
        },
    );

    for diagnostic in &skills.diagnostics {
        eprintln!(
            "Warning: could not load skill {}: {}",
            hallucinate_agent::config::paths::display_path(&diagnostic.path, &cwd),
            diagnostic.message
        );
    }

    let prompt_template_options =
        hallucinate_agent::agent::prompt_templates::PromptTemplateLoadOptions {
            cwd: cwd.clone(),
            prompt_paths: cli.prompt_templates.clone(),
            include_defaults: !cli.no_prompt_templates,
        };

    let prompt_templates =
        hallucinate_agent::agent::prompt_templates::load_prompt_templates(&prompt_template_options);

    for diagnostic in &prompt_templates.diagnostics {
        eprintln!(
            "Warning: could not load prompt template {}: {}",
            hallucinate_agent::config::paths::display_path(&diagnostic.path, &cwd),
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

/// A short-lived UI notice with an expiration time.
struct EphemeralNotice {
    /// The notice text to display.
    message: String,
    /// When the notice should expire and be cleared.
    expires_at: Instant,
}

/// A selection in the rendered chat area. Positions are terminal cell
/// coordinates, which means selection continues to work for wrapped and
/// styled Markdown without having to reconstruct the rendered text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChatSelection {
    /// The starting point of the selection.
    anchor: (u16, u16),
    /// The current cursor/focus point of the selection.
    focus: (u16, u16),
}

impl ChatSelection {
    /// Return `true` if the anchor and focus differ, indicating a non-empty
    /// selection range.
    fn has_range(self) -> bool {
        self.anchor != self.focus
    }

    /// Return the selection bounds as a `(top_left, bottom_right)` pair,
    /// regardless of which point is the anchor or focus.
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
    prompt_templates: Vec<hallucinate_agent::agent::prompt_templates::PromptTemplate>,
    prompt_template_options: hallucinate_agent::agent::prompt_templates::PromptTemplateLoadOptions,
    skills: Vec<hallucinate_agent::agent::skills::Skill>,
    /// CLI overrides are retained so switching providers uses the same
    /// endpoint/key precedence as the initial model.
    cli: Cli,
    settings: Settings,
    model: hallucinate_agent::ai::types::Model,
    models: Vec<hallucinate_agent::ai::types::Model>,
    model_picker: Option<ModelPicker>,
    session_picker: Option<SessionPicker>,
    autocomplete_items: Vec<AutocompleteItem>,
    autocomplete_index: usize,
}

/// The result of an agent task, including ownership of the agent so the next
/// user message can reuse its conversation history.
struct AgentRunCompletion {
    /// The agent instance, which retains conversation history.
    agent: Agent,
    /// The result of the agent run — `Ok(())` on success, or an error string.
    result: Result<(), String>,
}

/// A completion shown for a slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AutocompleteItem {
    /// The completion text value.
    value: String,
    /// A human-readable description of the completion.
    description: String,
}

/// State for the keyboard-driven model picker overlay.
#[derive(Debug, Clone, Copy)]
struct ModelPicker {
    /// Index of the currently selected model.
    selected: usize,
}

/// State for the keyboard-driven session picker overlay.
#[derive(Debug, Clone, Copy)]
struct SessionPicker {
    /// Index of the currently selected session.
    selected: usize,
}

impl InteractiveTui {
    /// Create a new `InteractiveTui`, setting up the terminal, raw mode, and
    /// alternate screen.
    fn new(
        agent: Agent,
        event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        session_manager: SessionManager,
        prompt_templates: Vec<hallucinate_agent::agent::prompt_templates::PromptTemplate>,
        prompt_template_options: hallucinate_agent::agent::prompt_templates::PromptTemplateLoadOptions,
        skills: Vec<hallucinate_agent::agent::skills::Skill>,
        cli: Cli,
        settings: Settings,
        model: hallucinate_agent::ai::types::Model,
        models: Vec<hallucinate_agent::ai::types::Model>,
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
        if let Err(error) = input::enable_mouse_capture() {
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
                let mut transcript = session::transcript_from_messages(agent.messages());
                transcript.set_show_thinking(show_thinking);
                transcript
            },
            editor: Editor::new("").placeholder(
                "Type a message..."
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
            session_picker: None,
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

    /// Run the event loop until `running` is set to `false` or an error
    /// occurs.
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

    /// Add a system message describing loaded context files, skills, and
    /// prompt templates to the transcript when a new session starts.
    fn add_new_session_message(&mut self) {
        let mut message = String::new();

        let cwd = self
            .agent
            .as_ref()
            .and_then(|agent| agent.cwd())
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok());

        if let Some(cwd) = cwd {
            let files = hallucinate_agent::agent::system_prompt::prompt_file_paths(&cwd);

            let context_paths = [
                files
                    .system_prompt
                    .map(|path| hallucinate_agent::config::paths::display_path(path, &cwd)),
                files
                    .agents
                    .map(|path| hallucinate_agent::config::paths::display_path(path, &cwd)),
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
}

/// Truncate a session ID to at most 8 characters for display.
fn short_session_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// Append a formatted section to a message string, showing the section name
/// as a heading and the items as a fenced code block.
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
