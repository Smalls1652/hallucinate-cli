//! Keyboard-driven picker overlays for the TUI.
//!
//! Provides centered, scrollable popups for selecting models, sessions, prompt
//! templates, and transcript messages. Supports text filtering, keyboard
//! navigation (↑↓, PgUp/PgDn, Home/End, Enter, Esc), and row selection markers.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::{alt_screen::MessageRole, utils::truncate_one_line};

/// State for the keyboard-driven model picker overlay.
#[derive(Debug, Clone)]
pub(crate) struct ModelPicker {
    /// Text used to limit models by display name.
    pub filter: String,

    /// Model indexes matching `filter`.
    pub filtered_indices: Vec<usize>,

    /// Index of the currently selected matching model.
    pub selected: usize,
}

/// State for the keyboard-driven session picker overlay.
#[derive(Debug, Clone)]
pub(crate) struct SessionPicker {
    /// Rows captured when the picker opened or was deliberately refreshed.
    pub rows: Vec<SessionPickerRow>,

    /// Text used to limit sessions by title.
    pub filter: String,

    /// Row indexes matching `filter`.
    pub filtered_indices: Vec<usize>,

    /// Index of the currently selected matching session.
    pub selected: usize,
}

/// Display-ready data for one session-picker row.
#[derive(Debug, Clone)]
pub(crate) struct SessionPickerRow {
    /// Full session ID used when the row is selected.
    pub id: String,

    /// Short session ID displayed in the picker.
    pub short_id: String,

    /// Last session activity time displayed in the picker.
    pub datetime: String,

    /// Human-readable session title.
    pub title: String,
}

/// State for the keyboard-driven prompt picker overlay.
#[derive(Debug, Clone)]
pub(crate) struct PromptPicker {
    /// Text used to limit prompt templates by name.
    pub filter: String,

    /// Template indexes matching `filter`.
    pub filtered_indices: Vec<usize>,

    /// Index of the currently selected matching template.
    pub selected: usize,
}

/// State for selecting a transcript message to copy.
#[derive(Debug, Clone)]
pub(crate) struct TranscriptMessagePicker {
    /// Copyable transcript messages captured when the picker opened.
    pub rows: Vec<TranscriptMessagePickerRow>,

    /// Index of the selected message.
    pub selected: usize,
}

/// Display-ready data for one copyable transcript message.
#[derive(Debug, Clone)]
pub(crate) struct TranscriptMessagePickerRow {
    /// Index in the complete transcript.
    pub message_index: usize,

    /// Role shown to distinguish user and assistant messages.
    pub role: MessageRole,

    /// First line of the message, used as a picker preview.
    pub preview: String,
}

/// Render a centered, scrollable picker popup.
pub fn render_picker<F>(
    frame: &mut ratatui::Frame,
    title: &str,
    filtered_indices: &[usize],
    selected: usize,
    mut render_row: F,
) where
    F: FnMut(usize, usize, u16) -> Line<'static>,
{
    let item_count = filtered_indices.len();
    let area = frame.area();
    let popup_width = area.width.saturating_sub(4).min(140);
    let popup_height = area
        .height
        .saturating_sub(4)
        .min((item_count.max(1) + 2).min(18) as u16);

    if popup_width <= 4 || popup_height <= 2 {
        return;
    }

    let popup_area = Rect::new(
        area.x + (area.width - popup_width) / 2,
        area.y + (area.height - popup_height) / 2,
        popup_width,
        popup_height,
    );

    let visible_rows = usize::from(popup_height.saturating_sub(2));
    let start = selected
        .saturating_sub(visible_rows / 2)
        .min(item_count.saturating_sub(visible_rows));
    let rows = (start..item_count)
        .take(visible_rows)
        .map(|index| render_row(filtered_indices[index], index, popup_width))
        .collect::<Vec<_>>();

    frame.render_widget(Clear, popup_area);
    frame.render_widget(
        Paragraph::new(rows).block(Block::default().borders(Borders::ALL).title(title)),
        popup_area,
    );
}

/// Prefix a picker row with its selected-state marker.
///
/// The marker is `> ` for the selected row and `  ` (two spaces) for others.
pub fn picker_row(
    index: usize,
    selected: usize,
    mut spans: Vec<ratatui::text::Span<'static>>,
) -> ratatui::text::Line<'static> {
    spans.insert(
        0,
        Span::styled(
            if index == selected { "> " } else { "  " },
            Style::default().fg(if index == selected {
                Color::Yellow
            } else {
                Color::DarkGray
            }),
        ),
    );

    Line::from(spans)
}

/// Render the active model picker overlay.
pub(crate) fn render_model_picker(
    frame: &mut ratatui::Frame,
    picker: Option<&ModelPicker>,
    models: &[hallucinate_agent::ai::types::Model],
) {
    let Some(picker) = picker else {
        return;
    };

    render_picker(
        frame,
        &format!(
            " Select model · filter: {} · ↑↓ PgUp/PgDn Home/End Enter Esc ",
            picker.filter
        ),
        &picker.filtered_indices,
        picker.selected,
        |model_index, selected_index, _| {
            let model = &models[model_index];
            let metadata = format!(
                "context {} · max {} · {}{}",
                model.context_window,
                model.max_tokens,
                model.input.join(", "),
                if model.reasoning { " · reasoning" } else { "" }
            );

            picker_row(
                selected_index,
                picker.selected,
                vec![
                    Span::styled(
                        model.name.to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " - ",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        format!("{}/{}", model.provider, model.id),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        format!(" ({})", metadata),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    ),
                ],
            )
        },
    );
}

/// Render the active session picker overlay.
pub(crate) fn render_session_picker(
    frame: &mut ratatui::Frame,
    picker: Option<&SessionPicker>,
) {
    let Some(picker) = picker else {
        return;
    };

    render_picker(
        frame,
        &format!(
            " Select session · filter: {} · ↑↓ PgUp/PgDn Home/End Enter Esc ",
            picker.filter
        ),
        &picker.filtered_indices,
        picker.selected,
        |session_index, selected_index, popup_width| {
            let session = &picker.rows[session_index];
            let title =
                truncate_one_line(&session.title, usize::from(popup_width.saturating_sub(34)));

            picker_row(
                selected_index,
                picker.selected,
                vec![
                    Span::styled(
                        format!("{:<8}  ", session.short_id),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{:<16}  ", session.datetime),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(title, Style::default().fg(Color::White)),
                ],
            )
        },
    );
}

/// Render the active prompt picker overlay.
pub(crate) fn render_prompt_picker(
    frame: &mut ratatui::Frame,
    picker: Option<&PromptPicker>,
    prompt_templates: &[hallucinate_agent::agent::prompt_templates::PromptTemplate],
) {
    let Some(picker) = picker else {
        return;
    };

    render_picker(
        frame,
        &format!(
            " Select prompt · filter: {} · ↑↓ PgUp/PgDn Home/End Enter Esc ",
            picker.filter
        ),
        &picker.filtered_indices,
        picker.selected,
        |template_index, selected_index, _| {
            let template = &prompt_templates[template_index];
            let hint = template
                .argument_hint
                .as_deref()
                .map(|hint| format!(" {hint}"))
                .unwrap_or_default();

            picker_row(
                selected_index,
                picker.selected,
                vec![
                    Span::styled(
                        format!("/{}", template.name),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        hint,
                        Style::default()
                            .add_modifier(Modifier::DIM)
                            .add_modifier(Modifier::ITALIC),
                    ),
                    Span::styled(
                        " - ",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    ),
                    Span::styled(
                        template.description.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ],
            )
        },
    );
}

/// Render the active transcript-message picker overlay.
pub(crate) fn render_transcript_message_picker(
    frame: &mut ratatui::Frame,
    picker: Option<&TranscriptMessagePicker>,
) {
    let Some(picker) = picker else {
        return;
    };

    render_picker(
        frame,
        " Select message to copy · ↑↓ PgUp/PgDn Home/End Enter Esc ",
        &(0..picker.rows.len()).collect::<Vec<_>>(),
        picker.selected,
        |row_index, selected_index, popup_width| {
            let row = &picker.rows[row_index];
            let role = match row.role {
                MessageRole::User => "You",
                MessageRole::Assistant => "Assistant",
                _ => unreachable!("copy picker contains only user or assistant messages"),
            };
            let preview =
                truncate_one_line(&row.preview, usize::from(popup_width.saturating_sub(16)));

            picker_row(
                selected_index,
                picker.selected,
                vec![
                    Span::styled(
                        format!("{role:<9}"),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(preview, Style::default().fg(Color::White)),
                ],
            )
        },
    );
}
