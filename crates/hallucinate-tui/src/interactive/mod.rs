//! Interactive mode and the terminal application lifecycle.
//!
//! Input handling, command dispatch, session synchronization, and rendering
//! live in focused child modules so this module only owns shared state and
//! terminal setup.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context;
use hallucinate_agent::{
    agent::{
        runtime::Agent,
        types::{AgentCancellation, AgentEvent, AgentState},
    },
    config::{
        RuntimeOverrides,
        settings::{SessionTitleSettings, Settings},
    },
};
use hallucinate_session::manager::SessionManager;
use ratatui::{Terminal, backend::CrosstermBackend, crossterm, layout::Rect};

use crate::{
    alt_screen::{Message as TuiMessage, MessageRole, TranscriptRenderer},
    components::Editor,
};

mod commands;
mod input;
mod render;
mod session;

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
pub struct InteractiveTui {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,

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

    token_count_is_estimated: bool,

    event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,

    agent_completion_tx: tokio::sync::mpsc::UnboundedSender<AgentRunCompletion>,

    agent_completion_rx: tokio::sync::mpsc::UnboundedReceiver<AgentRunCompletion>,

    title_completion_tx: tokio::sync::mpsc::UnboundedSender<SessionTitleCompletion>,

    title_completion_rx: tokio::sync::mpsc::UnboundedReceiver<SessionTitleCompletion>,

    /// Settings used to create a fresh provider for each generated title.
    session_title_settings: SessionTitleSettings,

    /// Agent waiting for a before-response title request to complete.
    pending_title_agent: Option<Agent>,

    /// Whether a before-response session-title request is in progress.
    title_generation_in_progress: bool,

    /// Whether the current session is eligible to receive a generated title.
    title_generation_pending: bool,

    /// The transcript entry receiving content from the current assistant
    /// stream.
    streaming_message_index: Option<usize>,

    /// Maps streamed assistant content indices to tool-call positions in the
    /// current transcript entry. Content indices are stable for a provider
    /// response while the visible tool-call list contains tool blocks only.
    streaming_tool_call_indices: HashMap<usize, usize>,

    /// Whether reasoning blocks are visible in the transcript.
    show_thinking: bool,

    /// Whether completed tool results use their compact or full display.
    show_full_tool_output: bool,

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

    /// Runtime overrides are retained so switching providers uses the same
    /// endpoint/key precedence as the initial model.
    runtime_overrides: RuntimeOverrides,

    settings: Settings,

    model: hallucinate_agent::ai::types::Model,

    models: Vec<hallucinate_agent::ai::types::Model>,

    model_picker: Option<ModelPicker>,

    session_picker: Option<SessionPicker>,

    prompt_picker: Option<PromptPicker>,

    autocomplete_items: Vec<AutocompleteItem>,

    autocomplete_index: usize,

    /// Cached wrapped transcript lines. Scroll-only redraws reuse this layout
    /// rather than reparsing every historic Markdown message.
    transcript_layout_cache: render::TranscriptLayoutCache,

    render_state: RenderState,
}

/// A background operation which needs transcript feedback on completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentRunOperation {
    /// A manual `/compact` checkpoint request.
    Compact,
}

/// The result of an agent task, including ownership of the agent so the next
/// user message can reuse its conversation history.
struct AgentRunCompletion {
    /// The agent instance, which retains conversation history.
    agent: Agent,

    /// The result of the agent run — `Ok(())` on success, or an error string.
    result: Result<(), String>,

    /// Operation-specific transcript feedback, if this was not a normal turn.
    operation: Option<AgentRunOperation>,
}

/// The outcome of a background session-title request.
struct SessionTitleCompletion {
    /// The generated title, or an error suitable for a non-blocking notice.
    result: Result<String, String>,
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
#[derive(Debug, Clone)]
struct ModelPicker {
    /// Text used to limit models by display name.
    filter: String,

    /// Model indexes matching `filter`.
    filtered_indices: Vec<usize>,

    /// Index of the currently selected matching model.
    selected: usize,
}

/// State for the keyboard-driven session picker overlay.
#[derive(Debug, Clone)]
struct SessionPicker {
    /// Rows captured when the picker opened or was deliberately refreshed.
    rows: Vec<SessionPickerRow>,

    /// Text used to limit sessions by title.
    filter: String,

    /// Row indexes matching `filter`.
    filtered_indices: Vec<usize>,

    /// Index of the currently selected matching session.
    selected: usize,
}

/// Display-ready data for one session-picker row.
#[derive(Debug, Clone)]
struct SessionPickerRow {
    /// Full session ID used when the row is selected.
    id: String,

    /// Short session ID displayed in the picker.
    short_id: String,

    /// Last session activity time displayed in the picker.
    datetime: String,

    /// Human-readable session title.
    title: String,
}

/// State for the keyboard-driven prompt picker overlay.
#[derive(Debug, Clone)]
struct PromptPicker {
    /// Text used to limit prompt templates by name.
    filter: String,

    /// Template indexes matching `filter`.
    filtered_indices: Vec<usize>,

    /// Index of the currently selected matching template.
    selected: usize,
}

/// UI regions which need another terminal draw.
#[derive(Debug, Clone, Copy)]
struct RenderState {
    transcript: bool,

    status: bool,

    editor: bool,

    overlays: bool,

    selection: bool,

    terminal_size: bool,
}

impl RenderState {
    fn all() -> Self {
        Self {
            transcript: true,
            status: true,
            editor: true,
            overlays: true,
            selection: true,
            terminal_size: true,
        }
    }

    fn any(self) -> bool {
        self.transcript
            || self.status
            || self.editor
            || self.overlays
            || self.selection
            || self.terminal_size
    }

    fn clear(&mut self) {
        *self = Self {
            transcript: false,
            status: false,
            editor: false,
            overlays: false,
            selection: false,
            terminal_size: false,
        };
    }

    fn mark_all(&mut self) {
        *self = Self::all();
    }
}

impl InteractiveTui {
    /// Create a new `InteractiveTui`, setting up the terminal, raw mode, and
    /// alternate screen.
    pub fn new(
        mut agent: Agent,
        event_rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
        session_manager: SessionManager,
        prompt_templates: Vec<hallucinate_agent::agent::prompt_templates::PromptTemplate>,
        prompt_template_options: hallucinate_agent::agent::prompt_templates::PromptTemplateLoadOptions,
        skills: Vec<hallucinate_agent::agent::skills::Skill>,
        runtime_overrides: RuntimeOverrides,
        show_thinking_override: bool,
        settings: Settings,
        model: hallucinate_agent::ai::types::Model,
        models: Vec<hallucinate_agent::ai::types::Model>,
        session_title_settings: SessionTitleSettings,
        title_generation_pending: bool,
    ) -> Result<Self, anyhow::Error> {
        // Use a raw stdout writer that we own
        let stdout = std::io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("Failed to create terminal")?;

        crossterm::terminal::enable_raw_mode().context("Failed to enable raw mode")?;

        if let Err(error) = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::cursor::Hide,
            crossterm::event::EnableBracketedPaste
        ) {
            let _ = crossterm::terminal::disable_raw_mode();

            return Err(error).context("Failed to enter alternate screen");
        }
        if let Err(error) = input::enable_mouse_capture() {
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

        // Initialize the immutable context once so the status-bar estimate
        // and the first provider turn share the same prompt-file snapshot.
        agent.reload_system_prompt();

        let agent_state = agent.state().clone();

        let token_count = agent.estimate_tokens();

        let show_thinking =
            hallucinate_agent::config::resolve_show_thinking(&settings, show_thinking_override);

        let (agent_completion_tx, agent_completion_rx) = tokio::sync::mpsc::unbounded_channel();

        let (title_completion_tx, title_completion_rx) = tokio::sync::mpsc::unbounded_channel();

        let mut tui = Self {
            terminal,
            transcript: {
                let messages = agent.messages();

                let mut transcript = session::transcript_from_messages(&messages);
                transcript.set_show_thinking(show_thinking);
                transcript
            },
            editor: Editor::new("").placeholder("Type a message..."),
            transcript_area: Rect::default(),
            chat_selection: None,
            selected_text: String::new(),
            copy_selection_on_render: false,
            agent: Some(agent),
            agent_state,
            token_count,
            token_count_is_estimated: true,
            event_rx,
            agent_completion_tx,
            agent_completion_rx,
            title_completion_tx,
            title_completion_rx,
            session_title_settings,
            pending_title_agent: None,
            title_generation_in_progress: false,
            title_generation_pending,
            streaming_message_index: None,
            streaming_tool_call_indices: HashMap::new(),
            show_thinking,
            show_full_tool_output: false,
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
            runtime_overrides,
            settings,
            model,
            models,
            model_picker: None,
            session_picker: None,
            prompt_picker: None,
            autocomplete_items: Vec::new(),
            autocomplete_index: 0,
            transcript_layout_cache: render::TranscriptLayoutCache::default(),
            render_state: RenderState::all(),
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
        self.render_state.transcript = true;
        self.render_state.status = true;
    }

    /// Run the main TUI event loop. Terminal cleanup is performed even when
    /// rendering or input handling returns an error.
    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        self.running = true;

        let result = self.run_loop();
        let cleanup_result = self.session_manager.discard_empty_current();
        self.exit_raw_mode()?;

        result?;
        cleanup_result.map(|_| ())
    }

    /// Run the event loop until `running` is set to `false` or an error
    /// occurs.
    fn run_loop(&mut self) -> Result<(), anyhow::Error> {
        let mut next_stream_render = None;

        while self.running {
            let received_delta = self.process_events();

            let completed_run = self.process_completed_runs();
            let completed_title = self.process_completed_titles();

            if received_delta {
                self.render_state.transcript = true;
                self.render_state.status = true;

                next_stream_render
                    .get_or_insert_with(|| Instant::now() + Duration::from_millis(16));
            }

            if completed_run || completed_title {
                self.render_state.mark_all();
            }

            self.clear_expired_notice();

            let stream_render_due =
                next_stream_render.is_some_and(|deadline| Instant::now() >= deadline);

            if self.render_state.any() && (!received_delta || stream_render_due) {
                // Clear before drawing so work performed by rendering itself
                // (for example the macOS selection-copy fallback) can request
                // the follow-up status redraw it needs.
                self.render_state.clear();
                self.render()?;

                next_stream_render = None;

                if self.render_state.any() {
                    continue;
                }
            }

            // Crossterm input cannot be selected with the Tokio agent channels.
            // Poll only while a response is active (which keeps stream events
            // responsive); when idle this blocks until a notice must expire.
            let timeout = self.input_timeout(next_stream_render);

            if self.handle_input(timeout)? {
                break;
            }
        }

        Ok(())
    }

    fn input_timeout(
        &self,
        next_stream_render: Option<Instant>,
    ) -> Duration {
        let now = Instant::now();

        let notice_timeout = self
            .notice
            .as_ref()
            .map(|notice| notice.expires_at.saturating_duration_since(now));

        let stream_timeout =
            next_stream_render.map(|deadline| deadline.saturating_duration_since(now));

        input_poll_timeout(
            self.agent_cancellation.is_some(),
            notice_timeout,
            stream_timeout,
        )
    }

    /// Restore the terminal after leaving interactive mode.
    fn exit_raw_mode(&mut self) -> Result<(), anyhow::Error> {
        let mut cleanup_error = crossterm::terminal::disable_raw_mode().err();

        if let Err(error) = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show,
        ) {
            cleanup_error.get_or_insert(error);
        }

        if let Some(error) = cleanup_error {
            return Err(error).context("Failed to restore terminal state");
        }

        Ok(())
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

    /// Add an error message to the transcript once. The agent normally emits
    /// an error event before its background task completes, while some
    /// operations only report their failure through task completion.
    fn add_error_message(
        &mut self,
        content: impl Into<String>,
    ) {
        let content = content.into();

        if self
            .transcript
            .messages()
            .last()
            .is_some_and(|message| message.role == MessageRole::Error && message.content == content)
        {
            return;
        }

        self.transcript.add_message(TuiMessage {
            role: MessageRole::Error,
            content,
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });

        self.render_state.transcript = true;
    }

    /// Add a compaction checkpoint as a synthetic tool result. The regular
    /// full-tool-output toggle controls whether the complete checkpoint is
    /// expanded or rendered as a compact preview.
    fn add_compaction_result(
        &mut self,
        summary: &str,
    ) {
        self.transcript.add_message(TuiMessage {
            role: MessageRole::Assistant,
            content: String::new(),
            thinking: String::new(),
            tool_calls: vec![crate::alt_screen::ToolCall {
                id: "compaction-summary".to_string(),
                name: "compact".to_string(),
                arguments: serde_json::json!({}),
                result: Some(summary.to_string()),
            }],
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

        self.render_state.status = true;
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
            self.render_state.status = true;
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

/// Select how long crossterm may block before the next UI-loop iteration.
///
/// A scheduled stream redraw must wake the loop even after the agent task has
/// completed. The final text delta and completion can arrive in the same loop
/// iteration; otherwise clearing the cancellation handle would let input poll
/// block until unrelated terminal input occurs.
fn input_poll_timeout(
    response_active: bool,
    notice_timeout: Option<Duration>,
    stream_timeout: Option<Duration>,
) -> Duration {
    let scheduled_timeout = [notice_timeout, stream_timeout].into_iter().flatten().min();

    if response_active {
        scheduled_timeout
            .unwrap_or(Duration::from_millis(16))
            .min(Duration::from_millis(16))
    } else {
        scheduled_timeout.unwrap_or(Duration::from_secs(60 * 60))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_response_keeps_the_scheduled_stream_redraw() {
        let timeout = input_poll_timeout(false, None, Some(Duration::from_millis(16)));

        assert_eq!(timeout, Duration::from_millis(16));
    }

    #[test]
    fn active_response_polls_often_without_a_scheduled_redraw() {
        let timeout = input_poll_timeout(true, None, None);

        assert_eq!(timeout, Duration::from_millis(16));
    }
}
