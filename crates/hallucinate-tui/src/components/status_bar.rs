//! Status bar component for the TUI.
//!
//! Renders a single-line status bar above the transcript showing the agent
//! state, session title/ID, token count, model provider, and thinking
//! indicator. Supports short-lived ephemeral notices that auto-expire.

use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use hallucinate_agent::agent::types::AgentState;
use ratatui::{
    layout::Rect,
    style::Stylize,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::utils::truncate_one_line;

/// Title of the app, to show in the status bar, that is
/// determined based off whether the platform supports
/// emojis.
///
/// If it does, it returns "🤖 hallucinate"; however,
/// if it doesn't then it returns "hallucinate".
const APP_TITLE: LazyLock<&str> = LazyLock::new(|| {
    let supports_emojis = cfg_select! {
        target_os = "macos" => true,
        target_os = "linux" => {
            if let Ok(lang) = std::env::var("LANG")
                .or_else(|_| std::env::var("LC_ALL"))
                .or_else(|_| std::env::var("LC_CTYPE"))
            {
                lang.to_lowercase().replace("-", "").contains("utf8")
            } else {
                false
            }
        },
        target_os = "windows" => std::env::var("WT_SESSION").is_ok(),
        _ => false,
    };

    match supports_emojis {
        true => "🤖 hallucinate",
        false => "hallucinate",
    }
});

pub(crate) const EPHEMERAL_NOTICE_DURATION: Duration = Duration::from_secs(4);

/// A short-lived UI notice with an expiration time.
pub(crate) struct EphemeralNotice {
    /// The notice text to display.
    pub message: String,

    /// When the notice should expire and be cleared.
    pub expires_at: Instant,
}

/// Render the session state and model information above the transcript.
pub(crate) fn render_status_bar(
    frame: &mut ratatui::Frame,
    area: Rect,
    agent_state: &AgentState,
    session_manager: &hallucinate_session::SessionManager,
    token_count: u64,
    model: &hallucinate_agent::ai::types::Model,
    show_thinking: bool,
    title_generation_in_progress: bool,
    notice: Option<&EphemeralNotice>,
) {
    let state_str = if title_generation_in_progress {
        "generating title...".to_string()
    } else {
        match agent_state {
            AgentState::Idle => "idle".to_string(),

            AgentState::Streaming => "streaming...".to_string(),

            AgentState::ToolExecuting => "executing tools...".to_string(),

            AgentState::TurnComplete => "idle".to_string(),

            AgentState::Error(_) => "error — see transcript".to_string(),

            AgentState::Aborted => "aborted".to_string(),
        }
    };

    let status_text = StatusBarBuilder::new()
        .insert_blank_space()
        .with_notice(notice)
        .with_state(&state_str)
        .insert_blank_space()
        .with_session_title(session_manager)
        .insert_separator()
        .with_token_count(token_count)
        .insert_separator()
        .with_model(model)
        .insert_separator()
        .with_thinking(show_thinking)
        .build();

    let status = Paragraph::new(status_text);
    frame.render_widget(status, area);
}

/// Builder for constructing the status bar line.
struct StatusBarBuilder<'spans> {
    /// The spans to render in the status bar.
    spans: Vec<Span<'spans>>,
}

impl<'spans> StatusBarBuilder<'spans> {
    /// Create a new builder with the app title as the initial span.
    pub fn new() -> Self {
        Self {
            spans: vec![APP_TITLE.cyan().bold()],
        }
    }

    /// Consume the builder and produce the final `Line`.
    pub fn build(self) -> Line<'spans> {
        Line::from(self.spans)
    }

    /// Insert a single blank space between spans.
    pub fn insert_blank_space(mut self) -> Self {
        self.spans.push(Span::raw(" "));

        self
    }

    /// Insert a separator (` | `) between spans.
    pub fn insert_separator(mut self) -> Self {
        self.spans.push(" | ".dark_gray());

        self
    }

    /// Add an ephemeral notice, if provided.
    pub fn with_notice(
        mut self,
        notice: Option<&EphemeralNotice>,
    ) -> Self {
        if let Some(notice) = notice {
            let notice_content = format!("! {} |", truncate_one_line(&notice.message, 80));

            self.spans.push(notice_content.light_yellow());
            self = self.insert_blank_space();
        }

        self
    }

    /// Add the agent state indicator.
    pub fn with_state(
        mut self,
        state: &str,
    ) -> Self {
        let state = state.to_string();

        self.spans.push("[".dark_gray());
        self.spans.push(state.yellow());
        self.spans.push("]".dark_gray());

        self
    }

    /// Add the session title or ID.
    pub fn with_session_title(
        mut self,
        session_manager: &hallucinate_session::SessionManager,
    ) -> Self {
        let session_display = if let Some(title) = session_manager
            .current()
            .and_then(|session| session.title.as_deref())
            .filter(|title| !title.trim().is_empty())
        {
            truncate_one_line(title, 48).white()
        } else {
            session_manager
                .current_id()
                .map(|id| &id[..8.min(id.len())])
                .unwrap_or("none")
                .to_string()
                .green()
        };

        self.spans.push("session:".dark_gray());
        self.spans.push(session_display);

        self
    }

    /// Add the token count.
    pub fn with_token_count(
        mut self,
        token_count: u64,
    ) -> Self {
        let token_count = format!("{} tokens", token_count);

        self.spans.push(token_count.dark_gray());

        self
    }

    /// Add the model provider and ID.
    pub fn with_model(
        mut self,
        model: &hallucinate_agent::ai::types::Model,
    ) -> Self {
        let model = format!("model: {}/{}", model.provider, model.id);

        self.spans.push(model.cyan());

        self
    }

    /// Add the thinking indicator.
    pub fn with_thinking(
        mut self,
        show_thinking: bool,
    ) -> Self {
        let thinking_display = match show_thinking {
            true => "thinking: on",
            false => "thinking: off",
        };

        self.spans.push(thinking_display.dark_gray());

        self
    }
}
