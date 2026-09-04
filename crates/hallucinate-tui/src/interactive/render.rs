//! Ratatui rendering for the interactive transcript and overlays.

use std::{borrow::Cow, path::Path};

use hallucinate_agent::agent::types::AgentState;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::InteractiveTui;
use crate::{
    alt_screen::{MessageRole, ToolCall, TranscriptRenderer},
    components::markdown_to_lines,
};

/// Cached visual transcript layout for a particular content width and display
/// configuration. It intentionally excludes scrolling so viewport redraws can
/// reuse the complete layout without reparsing historic Markdown or cloning
/// every visual line.
const COMPACT_TOOL_RESULT_WIDTH: usize = 100;

#[derive(Default)]
pub(super) struct TranscriptLayoutCache {
    width: u16,
    show_thinking: bool,
    show_timestamps: bool,
    show_full_tool_output: bool,
    streaming_message_index: Option<usize>,
    transcript_revision: u64,
    lines: Vec<ratatui::text::Line<'static>>,
    selection_line_starts: Vec<Option<usize>>,
}

/// Render only the visible rows from a cached, already-wrapped transcript.
///
/// `Paragraph` owns its text, which would require cloning every cached line on
/// a scroll-only redraw. This widget borrows the cache and draws only the
/// viewport, making scroll work proportional to the terminal height.
struct TranscriptViewport<'a> {
    lines: &'a [ratatui::text::Line<'static>],
    scroll: usize,
}

impl ratatui::widgets::Widget for TranscriptViewport<'_> {
    fn render(
        self,
        area: Rect,
        buffer: &mut ratatui::buffer::Buffer,
    ) {
        for (row, line) in self
            .lines
            .iter()
            .skip(self.scroll)
            .take(usize::from(area.height))
            .enumerate()
        {
            let row_area = Rect::new(area.x, area.y.saturating_add(row as u16), area.width, 1);
            ratatui::widgets::Widget::render(line, row_area, buffer);
        }
    }
}

impl TranscriptLayoutCache {
    /// Return the number of cached visual transcript rows.
    pub(super) fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn matches(
        &self,
        transcript: &TranscriptRenderer,
        width: u16,
        show_full_tool_output: bool,
        streaming_message_index: Option<usize>,
    ) -> bool {
        self.width == width
            && self.show_thinking == transcript.show_thinking()
            && self.show_timestamps == transcript.show_timestamps()
            && self.show_full_tool_output == show_full_tool_output
            && self.streaming_message_index == streaming_message_index
            && self.transcript_revision == transcript.layout_revision()
    }

    fn replace(
        &mut self,
        transcript: &TranscriptRenderer,
        width: u16,
        show_full_tool_output: bool,
        streaming_message_index: Option<usize>,
        lines: Vec<ratatui::text::Line<'static>>,
        selection_line_starts: Vec<Option<usize>>,
    ) {
        self.width = width;
        self.show_thinking = transcript.show_thinking();
        self.show_timestamps = transcript.show_timestamps();
        self.show_full_tool_output = show_full_tool_output;
        self.streaming_message_index = streaming_message_index;
        self.transcript_revision = transcript.layout_revision();
        self.lines = lines;
        self.selection_line_starts = selection_line_starts;
    }
}

impl InteractiveTui {
    /// Render the current state.
    pub(super) fn render(&mut self) -> Result<(), anyhow::Error> {
        self.clear_expired_notice();

        let selection = self.chat_selection;
        let mut rendered_transcript_area = Rect::default();
        let mut rendered_selection_text = String::new();

        self.terminal.draw(|frame| {
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

            render_status_bar(
                frame,
                chunks[0],
                &self.agent_state,
                &self.session_manager,
                self.token_count,
                &self.model,
                self.show_thinking,
                self.notice.as_ref(),
            );

            let rendered_selection = render_transcript(
                frame,
                chunks[1],
                selection,
                &mut self.transcript_layout_cache,
                &self.transcript,
                self.show_full_tool_output,
                self.show_thinking,
                self.streaming_message_index,
                &self.project_cwd,
            );
            rendered_transcript_area = chunks[1];
            rendered_selection_text = rendered_selection;
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

            render_model_picker(frame, self.model_picker.as_ref(), &self.models);

            render_session_picker(frame, self.session_picker.as_ref());

            render_prompt_picker(frame, self.prompt_picker.as_ref(), &self.prompt_templates);
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

/// Render the cached transcript and return any active drag-selection text.
fn render_transcript(
    frame: &mut ratatui::Frame,
    area: Rect,
    selection: Option<super::ChatSelection>,
    transcript_layout_cache: &mut TranscriptLayoutCache,
    transcript: &TranscriptRenderer,
    show_full_tool_output: bool,
    show_thinking: bool,
    streaming_message_index: Option<usize>,
    project_cwd: &Path,
) -> String {
    use ratatui::{
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Padding, Paragraph, Widget},
    };

    // Transcript
    //
    // Messages use a light-weight chat-card treatment instead of a
    // stream of bracketed labels. The rounded header and accent rail
    // give each speaker a clear visual boundary while leaving the
    // Markdown renderer in charge of the actual message content.
    // Consecutive assistant turns deliberately share a card.
    //
    // Paragraph's word wrapper does not repeat a line's prefix on
    // continuation rows. Wrap message content before adding the
    // decorative rail so long paragraphs keep the rail aligned and
    // do not appear to spill out of their message card.
    let transcript_block = Block::default()
        .borders(Borders::NONE)
        .padding(Padding::horizontal(1));

    let transcript_inner_area = transcript_block.inner(area);
    let transcript_width = transcript_inner_area.width;

    let cache_matches = transcript_layout_cache.matches(
        transcript,
        transcript_width,
        show_full_tool_output,
        streaming_message_index,
    );

    if !cache_matches {
        let mut selection_line_starts = Vec::new();
        let messages = transcript.messages();

        let lines: Vec<Line<'static>> = messages
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
                let starts_card = starts_transcript_card(
                    message_index
                        .checked_sub(1)
                        .and_then(|index| messages.get(index))
                        .map(|message| message.role),
                    msg.role,
                );
                let ends_card = ends_transcript_card(
                    msg.role,
                    messages.get(message_index + 1).map(|message| message.role),
                );

                let role_style = Style::default().fg(role_color).add_modifier(Modifier::BOLD);
                let mut lines = Vec::new();
                let mut line_starts = Vec::new();

                if starts_card {
                    lines.push(Line::from(vec![
                        Span::styled("╭─ ", role_color),
                        Span::styled(icon, role_style),
                        Span::styled(format!(" {}", role_label), role_style),
                    ]));
                    line_starts.push(None);

                    if transcript.show_timestamps() {
                        lines[0].spans.push(Span::styled("  · ", muted));
                        lines[0].spans.push(Span::styled(
                            msg.timestamp.format("%H:%M:%S").to_string(),
                            muted.add_modifier(Modifier::ITALIC),
                        ));
                    }
                }

                if show_thinking && !msg.thinking.is_empty() {
                    // Reasoning is deliberately styled as a secondary
                    // surface: muted text plus a subtle background
                    // makes it visually
                    // distinct from the answer while keeping
                    // it readable during long streams.
                    let thinking_style = Style::default()
                        .fg(Color::Gray)
                        .bg(Color::Rgb(45, 45, 45))
                        .add_modifier(Modifier::DIM);

                    lines.push(Line::from(vec![
                        Span::styled("│ ", thinking_style),
                        Span::styled("💭 Thinking", thinking_style.add_modifier(Modifier::ITALIC)),
                    ]));
                    line_starts.push(None);

                    let mut thinking = markdown_to_lines(&msg.thinking);
                    while thinking.last().is_some_and(line_is_empty) {
                        thinking.pop();
                    }

                    for line in thinking {
                        for line in wrap_line_to_width(line, transcript_width.saturating_sub(2)) {
                            lines.push(muted_thinking_line(line, thinking_style));
                            line_starts.push(None);
                        }
                    }

                    lines.push(Line::from(vec![Span::styled("│ ", role_color)]));
                    line_starts.push(None);
                }

                let mut body = markdown_to_lines(&msg.content);
                while body.last().is_some_and(line_is_empty) {
                    body.pop();
                }

                if body.is_empty() {
                    let mut empty_line = vec![Span::styled("│ ", role_color)];

                    if streaming_message_index == Some(message_index) {
                        empty_line.push(Span::styled(
                            "thinking…",
                            muted.add_modifier(Modifier::ITALIC),
                        ));
                    }

                    lines.push(Line::from(empty_line));
                    line_starts.push(None);
                } else {
                    let selectable_start =
                        matches!(msg.role, MessageRole::User | MessageRole::Assistant).then_some(2);

                    for line in body {
                        for line in wrap_line_to_width(line, transcript_width.saturating_sub(2)) {
                            lines.push(prefix_message_line(line, role_color));

                            // The first two cells are the decorative
                            // rail;
                            // only user and assistant message bodies
                            // are
                            // selectable.
                            line_starts.push(selectable_start);
                        }
                    }
                }

                // If the last body line left an empty card rail,
                // remove it.
                if lines.last().is_some_and(line_is_empty_card_rail) {
                    lines.pop();
                    line_starts.pop();
                }

                for tool_call in &msg.tool_calls {
                    // If the previous line is not an empty card rail,
                    // push an empty card rail. This may seem
                    // counter-intuitive,
                    // considering the last thing done is removing an
                    // empty card rail from
                    // a body; however, this is genuinely
                    // a tough one to keep track of.
                    if !lines.last().is_some_and(line_is_empty_card_rail) {
                        lines.push(Line::from(Span::styled("│ ", role_color)));
                        line_starts.push(None);
                    }

                    let previous_len = lines.len();
                    let readable = streamed_tool_call_summary(tool_call, project_cwd);

                    append_prefixed_lines(
                        &mut lines,
                        &readable,
                        "├─ ⚙ ",
                        "│  ",
                        Style::default().fg(Color::LightBlue),
                        Style::default().fg(Color::LightBlue),
                        transcript_width,
                    );

                    line_starts.extend(vec![None; lines.len() - previous_len]);
                    if let Some(result) = &tool_call.result {
                        let previous_len = lines.len();
                        let is_error = result.starts_with("❌");
                        let result = tool_result_for_display(result, show_full_tool_output);
                        let result = tool_result_content(&result);

                        append_tool_result_box(&mut lines, result, is_error, transcript_width);

                        line_starts.extend(vec![None; lines.len() - previous_len]);
                    } else if streaming_message_index == Some(message_index)
                        && tool_call.arguments.is_string()
                    {
                        lines.push(Line::from(vec![
                            Span::styled("│  ", Style::default().fg(Color::LightBlue)),
                            Span::styled(
                                "✍ Writing…",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                        line_starts.push(None);

                        if tool_call.name == "write"
                            && let Some(content) = streamed_json_string_prefix(
                                tool_call.arguments.as_str().unwrap_or_default(),
                                "content",
                            )
                        {
                            let previous_len = lines.len();
                            let content = tool_result_for_display(&content, show_full_tool_output);
                            append_tool_result_box(&mut lines, &content, false, transcript_width);
                            line_starts.extend(vec![None; lines.len() - previous_len]);
                        }
                    } else if transcript.is_tool_call_running(&tool_call.id) {
                        lines.push(Line::from(vec![
                            Span::styled("│  ", Style::default().fg(Color::LightBlue)),
                            Span::styled(
                                "⏳ Running…",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                        line_starts.push(None);
                    }

                    lines.push(Line::from(Span::styled(
                        "│ ",
                        Style::default().fg(Color::LightBlue),
                    )));
                    line_starts.push(None);
                }

                if ends_card {
                    lines.push(Line::from(Span::styled("╰─", role_color)));
                    line_starts.push(None);
                    lines.push(Line::from(""));
                    line_starts.push(None);
                } else {
                    // Consecutive assistant turns share this card;
                    // leave a quiet in-card
                    // divider between their content.
                    lines.push(Line::from(Span::styled("│ ", role_color)));
                    line_starts.push(None);
                }

                // If the last line is an empty card rail,
                // remove it.
                if lines.last().is_some_and(line_is_empty_card_rail) {
                    lines.pop();
                    line_starts.pop();
                }

                selection_line_starts.extend(line_starts);

                lines
            })
            .collect();

        transcript_layout_cache.replace(
            transcript,
            transcript_width,
            show_full_tool_output,
            streaming_message_index,
            lines,
            selection_line_starts,
        );
    }

    let lines = &transcript_layout_cache.lines;
    let selection_line_starts = &transcript_layout_cache.selection_line_starts;
    debug_assert_eq!(lines.len(), selection_line_starts.len());

    // A Line is deliberately a single visual line in ratatui: an
    // embedded newline in a message would otherwise not create a row.
    // The helpers above expand and pre-wrap every message and tool
    // row, allowing this renderer to borrow just the visible cache
    // entries rather than cloning the full transcript into a
    // `Paragraph` for every scroll event.
    let active_selection = selection.filter(|selection| selection.has_range());
    let scroll = transcript.scroll_position(lines.len(), usize::from(transcript_inner_area.height));
    frame.render_widget(
        TranscriptViewport {
            lines,
            scroll: usize::from(scroll),
        },
        transcript_inner_area,
    );

    // Mouse capture normally prevents the terminal emulator from
    // selecting text itself. Paint the selection into ratatui's
    // buffer so drag selection works consistently across terminals.
    // Reading the same cells also gives Ctrl+C/Cmd+C the exact text
    // the user can see, including wrapped lines and Markdown styling.
    if let Some(selection) = active_selection {
        let ((start_x, start_y), (end_x, end_y)) = selection.bounds();
        let area = area;

        let selection_style = Style::default().fg(Color::Black).bg(Color::White);

        // A marker tree is needed only while a real drag selection
        // is active. Build it for visible rows only, rather than
        // cloning every historic transcript line.
        let selection_mask_lines = lines
            .iter()
            .skip(usize::from(scroll))
            .zip(
                selection_line_starts
                    .iter()
                    .skip(usize::from(scroll))
                    .copied(),
            )
            .take(usize::from(transcript_inner_area.height))
            .map(|(line, start)| selection_mask_line(line, start))
            .collect::<Vec<_>>();
        let mut mask_buffer = ratatui::buffer::Buffer::empty(area);
        let mask = Paragraph::new(selection_mask_lines);
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
        return selected_rows.join("\n");
    }
    String::new()
}

/// Render the active model picker overlay.
fn render_model_picker(
    frame: &mut ratatui::Frame,
    picker: Option<&super::ModelPicker>,
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
                model.input.join(","),
                if model.reasoning { " · reasoning" } else { "" }
            );

            picker_row(
                selected_index,
                picker.selected,
                vec![
                    Span::styled(
                        format!("{}", model.name),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" - {}/{} ", model.provider, model.id),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(metadata, Style::default().fg(Color::DarkGray)),
                ],
            )
        },
    );
}

/// Render the active session picker overlay.
fn render_session_picker(
    frame: &mut ratatui::Frame,
    picker: Option<&super::SessionPicker>,
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
fn render_prompt_picker(
    frame: &mut ratatui::Frame,
    picker: Option<&super::PromptPicker>,
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
                        format!("/{}{}", template.name, hint),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(" - ", Style::default()),
                    Span::styled(
                        template.description.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ],
            )
        },
    );
}

/// Render a centered, scrollable picker popup.
fn render_picker<F>(
    frame: &mut ratatui::Frame,
    title: &str,
    filtered_indices: &[usize],
    selected: usize,
    mut render_row: F,
) where
    F: FnMut(usize, usize, u16) -> ratatui::text::Line<'static>,
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
fn picker_row(
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

/// Render the session state and model information above the transcript.
fn render_status_bar(
    frame: &mut ratatui::Frame,
    area: Rect,
    agent_state: &AgentState,
    session_manager: &hallucinate_session::SessionManager,
    token_count: u64,
    model: &hallucinate_agent::ai::types::Model,
    show_thinking: bool,
    notice: Option<&super::EphemeralNotice>,
) {
    let state_str = match agent_state {
        AgentState::Idle => "idle".to_string(),
        AgentState::Streaming => "streaming...".to_string(),
        AgentState::ToolExecuting => "executing tools...".to_string(),
        AgentState::TurnComplete => "response complete".to_string(),
        AgentState::Error(message) => format!("error: {}", message),
        AgentState::Aborted => "aborted".to_string(),
    };

    let session_id = session_manager
        .current_id()
        .map(|id| &id[..8.min(id.len())])
        .unwrap_or("none");
    let session_title = session_manager
        .current()
        .and_then(|session| session.title.as_deref())
        .filter(|title| !title.trim().is_empty());

    let token_count = token_count;

    let status_text = Line::from(vec![
        Span::styled(
            "hallucinate ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(ratatui::style::Modifier::BOLD),
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
        Span::styled(
            format!("{} tokens", token_count),
            Style::default().fg(Color::DarkGray),
        ),
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

/// Return a concise tool-call summary while the provider streams raw JSON.
///
/// Write and edit payloads commonly contain an entire file, so displaying the
/// incomplete JSON would flood the transcript. Instead, show only an argument
/// that identifies the operation and wait for the complete call to use the
/// normal tool-specific summary.
fn streamed_tool_call_summary(
    tool_call: &ToolCall,
    cwd: &Path,
) -> String {
    let tool_name = if tool_call.name.is_empty() {
        "tool call"
    } else {
        tool_call.name.as_str()
    };

    if tool_name == "compact" {
        return "Context checkpoint".to_string();
    }

    let Some(arguments) = tool_call.arguments.as_str() else {
        return tool_call.human_readable_with_cwd(cwd);
    };

    let summary = match tool_name {
        "bash" => partial_json_string_value(arguments, "command")
            .map(|command| format!("bash {}", truncate_one_line(&command, 100))),
        "read" | "write" | "edit" | "ls" => {
            partial_json_string_value(arguments, "path").map(|path| {
                format!(
                    "{tool_name} {}",
                    hallucinate_agent::config::paths::display_path(&path, cwd)
                )
            })
        },
        "find" | "grep" => partial_json_string_value(arguments, "pattern")
            .map(|pattern| format!("{tool_name} {}", truncate_one_line(&pattern, 100))),
        "compact" => Some("Context checkpoint".to_string()),
        _ => None,
    };

    summary.unwrap_or_else(|| format!("{tool_name} …"))
}

/// Extract a complete JSON string value for `field` from an incomplete object.
///
/// Tool-call arguments arrive as JSON fragments. A field that closed before a
/// later, large field (for example, `path` before write `content`) is still a
/// valid standalone JSON string and can be decoded without accepting or
/// displaying the unfinished payload.
fn partial_json_string_value(
    arguments: &str,
    field: &str,
) -> Option<String> {
    let field_key = format!(r#""{field}""#);
    let field_start = arguments.find(&field_key)? + field_key.len();
    let value = arguments[field_start..].trim_start();
    let value = value.strip_prefix(':')?.trim_start();
    if !value.starts_with('"') {
        return None;
    }

    let mut escaped = false;
    for (index, character) in value[1..].char_indices() {
        if character == '"' && !escaped {
            let encoded_value = &value[..=index + 1];

            return serde_json::from_str(encoded_value).ok();
        }

        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }

    None
}

/// Return an incomplete JSON string field's content as it is streamed.
///
/// The provider has not necessarily emitted the closing quote yet. Completing
/// the fragment locally lets serde decode ordinary escapes without requiring a
/// fully valid tool-call payload; retain the raw fragment if it ends
/// mid-escape.
fn streamed_json_string_prefix(
    arguments: &str,
    field: &str,
) -> Option<String> {
    let field_key = format!(r#""{field}""#);
    let value_start = arguments.find(&field_key)? + field_key.len();
    let value = arguments[value_start..].trim_start();
    let value = value.strip_prefix(':')?.trim_start();
    let encoded_value = value.strip_prefix('"')?;
    let mut escaped = false;
    let mut end = encoded_value.len();
    for (index, character) in encoded_value.char_indices() {
        if character == '"' && !escaped {
            end = index;
            break;
        }

        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    let encoded_value = &encoded_value[..end];

    serde_json::from_str(&format!(r#""{encoded_value}""#))
        .ok()
        .or_else(|| Some(encoded_value.to_string()))
}

/// Return whether `role` begins a new visual transcript card.
///
/// Tool-result messages are attached to their originating assistant message,
/// so adjacent assistant messages represent consecutive agent turns and share
/// one card.
fn starts_transcript_card(
    previous_role: Option<MessageRole>,
    role: MessageRole,
) -> bool {
    !matches!(
        (previous_role, role),
        (Some(MessageRole::Assistant), MessageRole::Assistant)
    )
}

/// Return whether `role` closes its visual transcript card.
fn ends_transcript_card(
    role: MessageRole,
    next_role: Option<MessageRole>,
) -> bool {
    !matches!(
        (role, next_role),
        (MessageRole::Assistant, Some(MessageRole::Assistant))
    )
}

/// Return whether a [`Line`] contains no non-empty spans.
fn line_is_empty(line: &ratatui::text::Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.is_empty())
}

/// Return whether a [`Line`] consists solely of the card-rail marker `"│ "`.
fn line_is_empty_card_rail(line: &ratatui::text::Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content == "│ ")
}

/// Foreground color marker for selectable text in the transcript.
const SELECTION_MARKER: ratatui::style::Color = ratatui::style::Color::Rgb(1, 2, 3);
/// Foreground color marker for non-selectable text in the transcript.
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

/// Append a half-width, colour-coded surface for a completed tool result.
///
/// The assistant card rail remains visible on the left, with a small gap before
/// the result box. This keeps tool output associated with its call without
/// competing with the full width of conversational messages.
fn append_tool_result_box(
    lines: &mut Vec<ratatui::text::Line<'static>>,
    result: &str,
    is_error: bool,
    width: u16,
) {
    if width == 0 {
        return;
    }

    let background = if is_error {
        Color::Rgb(80, 20, 20)
    } else {
        Color::Rgb(15, 65, 30)
    };
    let style = Style::default().fg(Color::DarkGray).bg(background);
    let rail_style = Style::default().fg(Color::LightBlue);
    let box_width = usize::from((width / 2).max(1));
    // Keep the output clear of the coloured box edges without overflowing very
    // narrow terminal layouts.
    let horizontal_padding = usize::from(box_width >= 3);
    let content_width = box_width.saturating_sub(horizontal_padding * 2).max(1);

    lines.push(ratatui::text::Line::from(vec![
        Span::styled("│ ", rail_style),
        Span::styled(" ".repeat(box_width), style),
    ]));

    for physical_line in result.split('\n') {
        for line in wrap_line_to_width(
            ratatui::text::Line::from(Span::styled(physical_line.trim_end().to_string(), style)),
            u16::try_from(content_width).unwrap_or(u16::MAX),
        ) {
            let content = String::from(line);
            let remaining_width = content_width.saturating_sub(content.width());
            lines.push(ratatui::text::Line::from(vec![
                Span::styled("│ ", rail_style),
                Span::styled(
                    format!(
                        "{:horizontal_padding$}{content}{:remaining_width$}{:horizontal_padding$}",
                        "", "", "",
                    ),
                    style,
                ),
            ]));
        }
    }
}

/// Remove the internal success or failure marker from persisted tool output.
///
/// Status is represented by the result box colour rather than a repeated emoji
/// in the transcript.
fn tool_result_content(result: &str) -> &str {
    result
        .strip_prefix("✅ ")
        .or_else(|| result.strip_prefix("❌ "))
        .unwrap_or(result)
}

/// Append text that may contain newlines, using the supplied visual prefix on
/// continuation lines (useful for tool calls and results).
fn append_prefixed_lines(
    lines: &mut Vec<ratatui::text::Line<'static>>,
    text: &str,
    prefix: &'static str,
    continuation_prefix: &'static str,
    prefix_style: ratatui::style::Style,
    text_style: ratatui::style::Style,
    width: u16,
) {
    let prefix_width = prefix.width().max(continuation_prefix.width());
    let prefix_width = u16::try_from(prefix_width).unwrap_or(u16::MAX);
    let content_width = width.saturating_sub(prefix_width);

    for (index, physical_line) in text.split('\n').enumerate() {
        let initial_prefix = if index == 0 {
            prefix
        } else {
            continuation_prefix
        };
        let wrapped_lines = wrap_line_to_width(
            ratatui::text::Line::from(ratatui::text::Span::styled(
                physical_line.trim_end().to_string(),
                text_style,
            )),
            content_width,
        );

        for (wrapped_index, line) in wrapped_lines.into_iter().enumerate() {
            let line_prefix = if wrapped_index == 0 {
                initial_prefix
            } else {
                continuation_prefix
            };
            let mut spans = vec![ratatui::text::Span::styled(line_prefix, prefix_style)];
            spans.extend(line.spans);
            lines.push(ratatui::text::Line::from(spans));
        }
    }
}

/// Return a tool result in its selected transcript display mode.
///
/// Full output remains subject to the tool's execution-time result limit; this
/// setting only controls the TUI's compact display.
fn tool_result_for_display<'a>(
    result: &'a str,
    show_full_tool_output: bool,
) -> Cow<'a, str> {
    if show_full_tool_output {
        Cow::Borrowed(result)
    } else {
        Cow::Owned(truncate_width(result, COMPACT_TOOL_RESULT_WIDTH))
    }
}

/// Truncate a string to one display line and a maximum display width.
///
/// Carriage returns and newlines are replaced with spaces so the result fits
/// on a single terminal row.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alt_screen::Message;

    #[test]
    fn consecutive_assistant_turns_share_a_transcript_card() {
        assert!(!starts_transcript_card(
            Some(MessageRole::Assistant),
            MessageRole::Assistant,
        ));
        assert!(!ends_transcript_card(
            MessageRole::Assistant,
            Some(MessageRole::Assistant),
        ));
        assert!(ends_transcript_card(MessageRole::Assistant, None));
    }

    #[test]
    fn selection_mask_excludes_the_message_rail() {
        let line = prefix_message_line(Line::from("answer"), Color::LightCyan);
        let mask = selection_mask_line(&line, Some(2));

        assert_eq!(mask.spans[0].style.fg, Some(NON_SELECTION_MARKER));
        assert_eq!(mask.spans[1].style.fg, Some(SELECTION_MARKER));
    }

    #[test]
    fn tool_output_display_can_switch_between_compact_and_full() {
        let result = format!("{} tail", "x".repeat(COMPACT_TOOL_RESULT_WIDTH));

        assert_eq!(
            tool_result_for_display(&result, false),
            format!("{}…", "x".repeat(COMPACT_TOOL_RESULT_WIDTH - 1)),
        );
        assert_eq!(tool_result_for_display(&result, true), result);
    }

    #[test]
    fn tool_result_boxes_are_half_width_with_padding_from_the_rail() {
        let mut success_lines = Vec::new();
        append_tool_result_box(&mut success_lines, "done", false, 20);

        assert_eq!(success_lines.len(), 2);
        assert_eq!(success_lines[0].width(), 12);
        assert_eq!(success_lines[0].spans[0].content, "│ ");
        assert_eq!(success_lines[0].spans[0].style.bg, None);
        assert_eq!(
            success_lines[0].spans[1].style.bg,
            Some(Color::Rgb(15, 65, 30)),
        );
        assert_eq!(success_lines[0].spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(success_lines[0].spans[1].content, "          ");
        assert_eq!(success_lines[1].width(), 12);
        assert_eq!(success_lines[1].spans[0].content, "│ ");
        assert_eq!(success_lines[1].spans[0].style.bg, None);
        assert_eq!(
            success_lines[1].spans[1].style.bg,
            Some(Color::Rgb(15, 65, 30)),
        );
        assert_eq!(success_lines[1].spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(success_lines[1].spans[1].content, " done     ");

        let mut error_lines = Vec::new();
        append_tool_result_box(&mut error_lines, "failed", true, 20);

        assert_eq!(error_lines.len(), 2);
        assert_eq!(error_lines[0].width(), 12);
        assert_eq!(
            error_lines[0].spans[1].style.bg,
            Some(Color::Rgb(80, 20, 20)),
        );
        assert_eq!(error_lines[0].spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(error_lines[0].spans[1].content, "          ");
        assert_eq!(error_lines[1].spans[1].style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn tool_result_content_removes_the_status_emoji() {
        assert_eq!(tool_result_content("✅ completed"), "completed");
        assert_eq!(tool_result_content("❌ failed"), "failed");
        assert_eq!(tool_result_content("unmarked output"), "unmarked output");
    }

    #[test]
    fn long_message_rows_keep_their_rail_when_wrapped() {
        let rows = wrap_line_to_width(Line::from("one two three four five"), 8)
            .into_iter()
            .map(|line| prefix_message_line(line, Color::LightCyan))
            .collect::<Vec<_>>();

        assert!(rows.len() > 1);

        for row in rows {
            assert!(row.width() <= 10);
            assert!(String::from(row).starts_with("│ "));
        }
    }

    #[test]
    fn non_message_lines_are_not_selectable() {
        let line = Line::from("tool output");
        let mask = selection_mask_line(&line, None);

        assert_eq!(mask.spans[0].style.fg, Some(NON_SELECTION_MARKER));
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
    fn compact_tool_call_has_a_context_checkpoint_label() {
        let tool_call = ToolCall {
            id: "compaction-summary".to_string(),
            name: "compact".to_string(),
            arguments: serde_json::json!({}),
            result: None,
        };

        assert_eq!(
            streamed_tool_call_summary(&tool_call, Path::new(".")),
            "Context checkpoint"
        );
    }

    #[test]
    fn streamed_write_content_is_available_before_the_tool_executes() {
        assert_eq!(
            streamed_json_string_prefix(
                r#"{"path":"plans/example.md","content":"first line\nsecond line"#,
                "content",
            ),
            Some("first line\nsecond line".to_string())
        );
    }

    #[test]
    fn streamed_write_content_with_many_backslashes_is_linear() {
        let arguments = format!(r#"{{"content":"{}"}}"#, "\\".repeat(20_000));

        let started_at = std::time::Instant::now();
        let content = streamed_json_string_prefix(&arguments, "content");

        assert!(started_at.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(content, Some("\\".repeat(10_000)));
    }

    #[test]
    fn streamed_write_call_shows_its_path_without_file_content() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "write".to_string(),
            arguments: serde_json::Value::String(
                r#"{"path":"plans/example.md","content":"a very large payload"#.to_string(),
            ),
            result: None,
        };

        assert_eq!(
            streamed_tool_call_summary(&tool_call, Path::new(".")),
            "write plans/example.md"
        );
    }

    #[test]
    fn streamed_tool_without_an_identifying_argument_uses_a_short_placeholder() {
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "write".to_string(),
            arguments: serde_json::Value::String(r#"{"content":"large"#.to_string()),
            result: None,
        };

        assert_eq!(
            streamed_tool_call_summary(&tool_call, Path::new(".")),
            "write …"
        );
    }

    #[test]
    fn width_truncation_does_not_split_wide_unicode() {
        let truncated = truncate_width("abc😀def", 5);

        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn cache_remains_valid_while_only_scrolling() {
        let mut transcript = TranscriptRenderer::new();
        transcript.add_message(Message {
            role: MessageRole::User,
            content: "cached".to_string(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });
        let mut cache = TranscriptLayoutCache::default();
        cache.replace(
            &transcript,
            80,
            false,
            None,
            vec![Line::from("cached")],
            vec![Some(0)],
        );

        assert!(transcript.scroll_up(3, 3));

        assert!(cache.matches(&transcript, 80, false, None));
        assert!(!cache.matches(&transcript, 80, true, None));
    }

    #[test]
    fn cache_invalidates_when_the_transcript_layout_changes() {
        let mut transcript = TranscriptRenderer::new();
        transcript.add_message(Message {
            role: MessageRole::Assistant,
            content: String::new(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });
        let mut cache = TranscriptLayoutCache::default();
        cache.replace(&transcript, 80, false, None, Vec::new(), Vec::new());

        transcript.append_to_message(0, "delta");

        assert!(!cache.matches(&transcript, 80, false, None));
    }

    #[test]
    fn autocomplete_description_truncation_stays_on_one_line() {
        let truncated = truncate_one_line("first line\nsecond line", 12);

        assert_eq!(truncated, "first line …");
        assert!(!truncated.contains('\n'));
    }

    #[test]
    fn cached_viewport_renders_only_the_scrolled_visible_rows() {
        let lines = vec![
            Line::from("first"),
            Line::from("second"),
            Line::from("third"),
        ];
        let area = Rect::new(0, 0, 10, 2);
        let mut buffer = ratatui::buffer::Buffer::empty(area);

        ratatui::widgets::Widget::render(
            TranscriptViewport {
                lines: &lines,
                scroll: 1,
            },
            area,
            &mut buffer,
        );

        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "s");
        assert_eq!(buffer.cell((0, 1)).unwrap().symbol(), "t");
    }

    #[test]
    fn prefixed_tool_rows_wrap_without_losing_the_continuation_rail() {
        let mut lines = Vec::new();
        append_prefixed_lines(
            &mut lines,
            "one two three",
            "├─ ",
            "│  ",
            ratatui::style::Style::default(),
            ratatui::style::Style::default(),
            8,
        );

        assert!(lines.len() > 1);
        assert_eq!(lines[0].spans[0].content, "├─ ");
        assert_eq!(lines[1].spans[0].content, "│  ");
        assert!(lines.iter().all(|line| line.width() <= 8));
    }
}
