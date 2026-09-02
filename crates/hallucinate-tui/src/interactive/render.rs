//! Ratatui rendering for the interactive transcript and overlays.

use hallucinate_agent::agent::types::AgentState;
use ratatui::layout::Rect;
#[cfg(test)]
use ratatui::{style::Color, text::Line};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{InteractiveTui, short_session_id};
use crate::{
    alt_screen::{MessageRole, TranscriptRenderer},
    components::markdown_to_lines,
};

/// Cached visual transcript layout for a particular content width and display
/// configuration. It intentionally excludes scrolling: `Paragraph::scroll`
/// can move this layout without reparsing Markdown or rewrapping cards.
///
/// A very large transcript is rendered correctly but is not retained twice:
/// the cache is dropped once it exceeds this visual-line budget.
const TRANSCRIPT_LAYOUT_CACHE_LINE_BUDGET: usize = 20_000;

#[derive(Default)]
pub(super) struct TranscriptLayoutCache {
    width: u16,
    show_thinking: bool,
    show_timestamps: bool,
    streaming_message_index: Option<usize>,
    message_revisions: Vec<u64>,
    lines: Vec<ratatui::text::Line<'static>>,
    selection_line_starts: Vec<Option<usize>>,
}

impl TranscriptLayoutCache {
    fn matches(
        &self,
        transcript: &TranscriptRenderer,
        width: u16,
        streaming_message_index: Option<usize>,
    ) -> bool {
        self.width == width
            && self.show_thinking == transcript.show_thinking()
            && self.show_timestamps == transcript.show_timestamps()
            && self.streaming_message_index == streaming_message_index
            && self.message_revisions.len() == transcript.messages().len()
            && self
                .message_revisions
                .iter()
                .enumerate()
                .all(|(index, revision)| transcript.message_revision(index) == Some(*revision))
    }

    fn replace(
        &mut self,
        transcript: &TranscriptRenderer,
        width: u16,
        streaming_message_index: Option<usize>,
        lines: Vec<ratatui::text::Line<'static>>,
        selection_line_starts: Vec<Option<usize>>,
    ) {
        if lines.len() > TRANSCRIPT_LAYOUT_CACHE_LINE_BUDGET {
            *self = Self::default();
            return;
        }

        self.width = width;
        self.show_thinking = transcript.show_thinking();
        self.show_timestamps = transcript.show_timestamps();
        self.streaming_message_index = streaming_message_index;
        self.message_revisions = (0..transcript.messages().len())
            .filter_map(|index| transcript.message_revision(index))
            .collect();
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

            let transcript_inner_area = transcript_block.inner(chunks[1]);
            let transcript_width = transcript_inner_area.width;

            let cache_matches = self.transcript_layout_cache.matches(
                &self.transcript,
                transcript_width,
                self.streaming_message_index,
            );

            let (lines, selection_line_starts) = if cache_matches {
                (
                    self.transcript_layout_cache.lines.clone(),
                    self.transcript_layout_cache.selection_line_starts.clone(),
                )
            } else {
                let mut selection_line_starts = Vec::new();
                let messages = self.transcript.messages();

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

                        let role_style =
                            Style::default().fg(role_color).add_modifier(Modifier::BOLD);
                        let mut lines = Vec::new();
                        let mut line_starts = Vec::new();

                        if starts_card {
                            lines.push(Line::from(vec![
                                Span::styled("╭─ ", role_color),
                                Span::styled(icon, role_style),
                                Span::styled(format!(" {}", role_label), role_style),
                            ]));
                            line_starts.push(None);

                            if self.transcript.show_timestamps() {
                                lines[0].spans.push(Span::styled("  · ", muted));
                                lines[0].spans.push(Span::styled(
                                    msg.timestamp.format("%H:%M:%S").to_string(),
                                    muted.add_modifier(Modifier::ITALIC),
                                ));
                            }
                        }

                        if self.show_thinking && !msg.thinking.is_empty() {
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
                                for line in
                                    wrap_line_to_width(line, transcript_width.saturating_sub(2))
                                {
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
                                for line in
                                    wrap_line_to_width(line, transcript_width.saturating_sub(2))
                                {
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
                            let readable = tool_call.human_readable_with_cwd(&self.project_cwd);

                            append_prefixed_lines(
                                &mut lines,
                                &readable,
                                "├─ ⚙ ",
                                "│  ",
                                Style::default().fg(Color::LightBlue),
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
                                    &truncate_width(result, 100),
                                    "│  ",
                                    "│  ",
                                    Style::default().fg(Color::LightBlue),
                                    result_style,
                                );

                                line_starts.extend(vec![None; lines.len() - previous_len]);
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

                self.transcript_layout_cache.replace(
                    &self.transcript,
                    transcript_width,
                    self.streaming_message_index,
                    lines.clone(),
                    selection_line_starts.clone(),
                );

                (lines, selection_line_starts)
            };

            debug_assert_eq!(lines.len(), selection_line_starts.len());

            // A Line is deliberately a single visual line in ratatui: an
            // embedded newline in a message would otherwise not create a new
            // row. The helpers above expand those newlines and pre-wrap
            // message content, while Paragraph's wrapping keeps any bounded
            // tool output inside the viewport. A little horizontal breathing
            // room makes the transcript read like a conversation rather than
            // a log file.
            // Retain the cached lines for selection-mask generation only
            // during an active drag. Ordinary frames move the owned layout
            // directly into the paragraph without a second line tree.
            let active_selection = selection.filter(|selection| selection.has_range());
            let selection_lines = active_selection.is_some().then(|| lines.clone());
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
            if let Some(selection) = active_selection {
                let ((start_x, start_y), (end_x, end_y)) = selection.bounds();
                let area = chunks[1];

                let selection_style = Style::default().fg(Color::Black).bg(Color::White);

                // The duplicate marker tree is needed only while a real drag
                // selection is active; ordinary transcript frames retain one
                // cached layout.
                let selection_mask_lines = selection_lines
                    .as_ref()
                    .expect("active selection retains transcript lines")
                    .iter()
                    .zip(selection_line_starts.iter().copied())
                    .map(|(line, start)| selection_mask_line(line, start))
                    .collect::<Vec<_>>();
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

                let popup_width = area.width.saturating_sub(4).min(140);

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

            // Session picker overlay. Rendered after the model picker so the
            // model picker takes visual precedence when both could appear.
            if let Some(picker) = self.session_picker.as_ref() {
                let area = frame.area();

                let popup_width = area.width.saturating_sub(4).min(140);

                {
                    let popup_height = area
                        .height
                        .saturating_sub(4)
                        .min((picker.rows.len() + 2).min(18) as u16);

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
                            .min(picker.rows.len().saturating_sub(visible_rows));

                        let rows = picker
                            .rows
                            .iter()
                            .enumerate()
                            .skip(start)
                            .take(visible_rows)
                            .map(|(index, session)| {
                                let marker = if index == picker.selected { "> " } else { "  " };

                                let metadata = format!(
                                    "{} · {} · {}",
                                    session.modified, session.display_path, session.title
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
                                        format!("{} — ", short_session_id(&session.id)),
                                        Style::default().fg(Color::Cyan),
                                    ),
                                    Span::styled(metadata, Style::default().fg(Color::DarkGray)),
                                ])
                            })
                            .collect::<Vec<_>>();

                        frame.render_widget(Clear, popup_area);

                        frame.render_widget(
                            Paragraph::new(rows).block(
                                Block::default()
                                    .borders(Borders::ALL)
                                    .title(" Select session · ↑↓ PgUp/PgDn Home/End Enter Esc "),
                            ),
                            popup_area,
                        );
                    }
                }
            }

            // Prompt template picker overlay. Rendered after the session picker
            // so it takes visual precedence when open.
            if let Some(picker) = self.prompt_picker {
                let area = frame.area();

                let popup_width = area.width.saturating_sub(4).min(140);

                let popup_height = area
                    .height
                    .saturating_sub(4)
                    .min((self.prompt_templates.len() + 2).min(18) as u16);

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
                        .min(self.prompt_templates.len().saturating_sub(visible_rows));

                    let rows = self
                        .prompt_templates
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(visible_rows)
                        .map(|(index, template)| {
                            let marker = if index == picker.selected { "> " } else { "  " };

                            let hint = template
                                .argument_hint
                                .as_deref()
                                .map(|hint| format!(" {hint}"))
                                .unwrap_or_default();

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
                                    format!("/{}{}", template.name, hint),
                                    Style::default().fg(Color::Cyan),
                                ),
                                Span::styled(format!(" - "), Style::default()),
                                Span::styled(
                                    template.description.clone(),
                                    Style::default().fg(Color::DarkGray),
                                ),
                            ])
                        })
                        .collect::<Vec<_>>();

                    frame.render_widget(Clear, popup_area);

                    frame.render_widget(
                        Paragraph::new(rows).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" Select prompt · ↑↓ PgUp/PgDn Home/End Enter Esc "),
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

/// Append text that may contain newlines, using the supplied visual prefix on
/// continuation lines (useful for tool calls and results).
fn append_prefixed_lines<'a>(
    lines: &mut Vec<ratatui::text::Line<'a>>,
    text: &str,
    prefix: &'a str,
    continuation_prefix: &'a str,
    prefix_style: ratatui::style::Style,
    text_style: ratatui::style::Style,
) {
    for (index, line) in text.split('\n').enumerate() {
        let line = line.trim_end();

        let prefix_span = if index == 0 {
            ratatui::text::Span::styled(prefix, prefix_style)
        } else {
            ratatui::text::Span::styled(continuation_prefix, prefix_style)
        };

        let line_span = ratatui::text::Span::styled(line.to_string(), text_style);

        lines.push(ratatui::text::Line::from(vec![prefix_span, line_span]));
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
            None,
            vec![Line::from("cached")],
            vec![Some(0)],
        );

        transcript.scroll_up(3);

        assert!(cache.matches(&transcript, 80, None));
    }

    #[test]
    fn cache_invalidates_when_a_message_revision_changes() {
        let mut transcript = TranscriptRenderer::new();
        transcript.add_message(Message {
            role: MessageRole::Assistant,
            content: String::new(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });
        let mut cache = TranscriptLayoutCache::default();
        cache.replace(&transcript, 80, None, Vec::new(), Vec::new());

        transcript.append_to_message(0, "delta");

        assert!(!cache.matches(&transcript, 80, None));
    }

    #[test]
    fn autocomplete_description_truncation_stays_on_one_line() {
        let truncated = truncate_one_line("first line\nsecond line", 12);

        assert_eq!(truncated, "first line …");
        assert!(!truncated.contains('\n'));
    }
}
