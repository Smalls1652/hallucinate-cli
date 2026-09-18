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
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::utils::truncate_one_line;

/// Title of the app, to show in the status bar, that is
/// determined based off whether the platform supports
/// emojis.
///
/// If it does, it returns "🤖 hallucinate "; however,
/// if it doesn't then it returns "hallucinate ".
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
        true => "🤖 hallucinate ",
        false => "hallucinate ",
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

    let session_id = session_manager
        .current_id()
        .map(|id| &id[..8.min(id.len())])
        .unwrap_or("none");
    let session_title = session_manager
        .current()
        .and_then(|session| session.title.as_deref())
        .filter(|title| !title.trim().is_empty());

    let token_count = format!("{token_count} tokens");

    let status_text = Line::from(vec![
        Span::styled(
            *APP_TITLE,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        if let Some(notice) = notice {
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
        if let Some(title) = session_title {
            Span::styled(
                truncate_one_line(title, 48),
                Style::default().fg(Color::White),
            )
        } else {
            Span::styled(session_id, Style::default().fg(Color::Green))
        },
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(token_count, Style::default().fg(Color::DarkGray)),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("model: {}/{}", model.provider, model.id),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            if show_thinking {
                "thinking: on"
            } else {
                "thinking: off"
            },
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let status = Paragraph::new(status_text);
    frame.render_widget(status, area);
}
