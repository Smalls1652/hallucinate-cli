//! Ratatui rendering for the interactive transcript and overlays.

use std::path::Path;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::InteractiveTui;
use crate::{
    alt_screen::{MessageRole, ToolCall, TranscriptRenderer},
    components::{
        markdown::markdown_to_clickable_lines,
        markdown_to_text,
        picker_overlays::{
            render_model_picker,
            render_prompt_picker,
            render_session_picker,
            render_transcript_message_picker,
        },
        status_bar::render_status_bar,
        thinking::append_thinking_box,
        tool_results::{append_tool_result_box, tool_result_content, tool_result_for_display},
    },
    utils::{append_prefixed_lines, line_is_empty, truncate_one_line, wrap_line_to_width},
};

#[derive(Default)]
pub(super) struct TranscriptLayoutCache {
    /// Cached visible width in columns.
    width: u16,

    /// Whether the thinking/reasoning surface is shown.
    show_thinking: bool,

    /// Whether timestamps are displayed on messages.
    show_timestamps: bool,

    /// Whether full (untruncated) tool output is shown.
    show_full_tool_output: bool,

    /// Index of the currently streaming message, if any.
    streaming_message_index: Option<usize>,

    /// Layout revision counter for cache invalidation.
    transcript_revision: u64,

    /// Cached visual transcript lines.
    lines: Vec<Line<'static>>,

    /// Link destinations referenced by markers in cached visual lines.
    links: Vec<String>,

    /// Column offset where each line becomes selectable, or `None`.
    selection_line_starts: Vec<Option<usize>>,

    /// Source message for each selectable visual row. This identifies message
    /// content rows while rendering a drag selection.
    selection_message_indices: Vec<Option<usize>>,
}

/// Render only the visible rows from a cached, already-wrapped transcript.
///
/// `Paragraph` owns its text, which would require cloning every cached line on
/// a scroll-only redraw. This widget borrows the cache and draws only the
/// viewport, making scroll work proportional to the terminal height.
struct TranscriptViewport<'a> {
    /// Cached visual lines to render.
    lines: &'a [Line<'static>],

    /// Number of rows to skip from the top (scroll offset).
    scroll: usize,
}

impl ratatui::widgets::Widget for TranscriptViewport<'_> {
    fn render(
        self,
        area: Rect,
        buffer: &mut Buffer,
    ) {
        // Unlike Paragraph, this lightweight viewport does not render trailing
        // spaces for every row. Clear its complete area first so shorter rows,
        // blank rows, and rows which disappear after a scroll cannot leave
        // cells from the preceding frame on screen.
        ratatui::widgets::Clear.render(area, buffer);

        for (row, line) in self
            .lines
            .iter()
            .skip(self.scroll)
            .take(usize::from(area.height))
            .enumerate()
        {
            let row_area = Rect::new(area.x, area.y.saturating_add(row as u16), area.width, 1);

            let mut visible = line.clone();

            for span in &mut visible.spans {
                span.style.underline_color = None;
            }

            ratatui::widgets::Widget::render(visible, row_area, buffer);
        }
    }
}

impl TranscriptLayoutCache {
    /// Return the number of cached visual transcript rows.
    pub(super) fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Find the link under a screen coordinate in the current scrolled view.
    pub(super) fn link_at(
        &self,
        column: u16,
        row: u16,
        area: Rect,
        scroll: usize,
    ) -> Option<&str> {
        let inner = Block::default().padding(Padding::horizontal(1)).inner(area);

        if column < inner.x || column >= inner.right() || row < inner.y || row >= inner.bottom() {
            return None;
        }

        let line = self.lines.get(scroll + usize::from(row - inner.y))?;

        let mut x = 0;

        for span in &line.spans {
            let end = x + span.content.width();

            if (x..end).contains(&usize::from(column - inner.x)) {
                return link_index(span.style)
                    .and_then(|index| self.links.get(index).map(String::as_str));
            }

            x = end;
        }

        None
    }

    /// Return whether the cache matches the given transcript configuration.
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

    /// Replace the cached layout with a new one for the given transcript.
    fn replace(
        &mut self,
        transcript: &TranscriptRenderer,
        width: u16,
        show_full_tool_output: bool,
        streaming_message_index: Option<usize>,
        lines: Vec<Line<'static>>,
        links: Vec<String>,
        selection_line_starts: Vec<Option<usize>>,
        selection_message_indices: Vec<Option<usize>>,
    ) {
        self.width = width;
        self.show_thinking = transcript.show_thinking();
        self.show_timestamps = transcript.show_timestamps();
        self.show_full_tool_output = show_full_tool_output;
        self.streaming_message_index = streaming_message_index;
        self.transcript_revision = transcript.layout_revision();
        self.lines = lines;
        self.links = links;
        self.selection_line_starts = selection_line_starts;
        self.selection_message_indices = selection_message_indices;
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
                self.title_generation_in_progress,
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

            render_transcript_message_picker(frame, self.transcript_message_picker.as_ref());

            render_clear_sessions_confirmation(frame, self.clear_sessions_confirmation.as_ref());
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
///
/// The transcript is laid out into visual rows using cached Markdown rendering.
/// If the cache is stale it is rebuilt, then a lightweight viewport widget
/// renders only the visible rows.
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
        let mut links = Vec::new();
        let mut selection_line_starts = Vec::new();
        let mut selection_message_indices = Vec::new();
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

                    MessageRole::Error => (Color::LightRed, "✗", "Error"),
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
                let mut message_indices = Vec::new();

                if starts_card {
                    lines.push(Line::from(vec![
                        Span::styled("╭─ ", role_color),
                        Span::styled(icon, role_style),
                        Span::styled(format!(" {}", role_label), role_style),
                    ]));
                    line_starts.push(None);
                    message_indices.push(None);

                    if transcript.show_timestamps() {
                        lines[0].spans.push(Span::styled("  · ", muted));
                        lines[0].spans.push(Span::styled(
                            msg.timestamp.format("%H:%M:%S").to_string(),
                            muted.add_modifier(Modifier::ITALIC),
                        ));
                    }
                }

                if show_thinking && !msg.thinking.is_empty() {
                    let previous_len = lines.len();
                    append_thinking_box(&mut lines, &msg.thinking, transcript_width);
                    line_starts.extend(vec![None; lines.len() - previous_len]);
                    message_indices.extend(vec![None; lines.len() - previous_len]);

                    // Keep the response visually separated from its reasoning
                    // surface.
                    lines.push(Line::from(Span::styled("│ ", role_color)));
                    line_starts.push(None);
                    message_indices.push(None);
                }

                let (mut body, message_links) = markdown_to_clickable_lines(
                    &msg.content,
                    usize::from(transcript_width.saturating_sub(2)),
                );
                let link_offset = links.len();

                for line in &mut body {
                    for span in &mut line.spans {
                        if let Some(index) = link_index(span.style) {
                            span.style = link_marker(span.style, index + link_offset);
                        }
                    }
                }

                links.extend(message_links);

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
                    message_indices.push(None);
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
                            message_indices.push(selectable_start.map(|_| message_index));
                        }
                    }
                }

                // If the last body line left an empty card rail,
                // remove it.
                if lines.last().is_some_and(line_is_empty_card_rail) {
                    lines.pop();
                    line_starts.pop();
                    message_indices.pop();
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
                        message_indices.push(None);
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
                    message_indices.extend(vec![None; lines.len() - previous_len]);
                    if let Some(result) = &tool_call.result {
                        let previous_len = lines.len();
                        let is_error = result.starts_with("❌");

                        let result = tool_result_for_display(result, show_full_tool_output);
                        let result = tool_result_content(&result);

                        let is_running = transcript.is_tool_call_running(&tool_call.id);
                        append_tool_result_box(
                            &mut lines,
                            result,
                            is_error,
                            is_running,
                            transcript_width,
                        );

                        line_starts.extend(vec![None; lines.len() - previous_len]);
                        message_indices.extend(vec![None; lines.len() - previous_len]);
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
                        message_indices.push(None);

                        if tool_call.name == "write"
                            && let Some(content) = streamed_json_string_prefix(
                                tool_call.arguments.as_str().unwrap_or_default(),
                                "content",
                            )
                        {
                            let previous_len = lines.len();
                            let content = tool_result_for_display(&content, show_full_tool_output);
                            append_tool_result_box(
                                &mut lines,
                                &content,
                                false,
                                true,
                                transcript_width,
                            );
                            line_starts.extend(vec![None; lines.len() - previous_len]);
                            message_indices.extend(vec![None; lines.len() - previous_len]);
                        }
                    } else if transcript.is_tool_call_running(&tool_call.id) {
                        let previous_len = lines.len();
                        append_tool_result_box(
                            &mut lines,
                            "⏳ Running…",
                            false,
                            true,
                            transcript_width,
                        );
                        line_starts.extend(vec![None; lines.len() - previous_len]);
                        message_indices.extend(vec![None; lines.len() - previous_len]);
                    }

                    lines.push(Line::from(Span::styled(
                        "│ ",
                        Style::default().fg(Color::LightBlue),
                    )));
                    line_starts.push(None);
                    message_indices.push(None);
                }

                if ends_card {
                    lines.push(Line::from(Span::styled("╰─", role_color)));
                    line_starts.push(None);
                    message_indices.push(None);

                    lines.push(Line::from(""));
                    line_starts.push(None);
                    message_indices.push(None);
                } else {
                    // Consecutive assistant turns share this card;
                    // leave a quiet in-card
                    // divider between their content.
                    lines.push(Line::from(Span::styled("│ ", role_color)));
                    line_starts.push(None);
                    message_indices.push(None);
                }

                // If the last line is an empty card rail,
                // remove it.
                if lines.last().is_some_and(line_is_empty_card_rail) {
                    lines.pop();
                    line_starts.pop();
                    message_indices.pop();
                }

                selection_line_starts.extend(line_starts);
                selection_message_indices.extend(message_indices);

                lines
            })
            .collect();

        transcript_layout_cache.replace(
            transcript,
            transcript_width,
            show_full_tool_output,
            streaming_message_index,
            lines,
            links,
            selection_line_starts,
            selection_message_indices,
        );
    }

    let lines = &transcript_layout_cache.lines;
    let selection_line_starts = &transcript_layout_cache.selection_line_starts;
    let selection_message_indices = &transcript_layout_cache.selection_message_indices;
    debug_assert_eq!(lines.len(), selection_line_starts.len());
    debug_assert_eq!(lines.len(), selection_message_indices.len());

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

        let mut mask_buffer = Buffer::empty(area);
        let mask = Paragraph::new(selection_mask_lines);
        mask.render(transcript_inner_area, &mut mask_buffer);

        let buffer = frame.buffer_mut();

        for row in start_y..=end_y {
            let from = if row == start_y { start_x } else { area.x };

            let to = if row == end_y {
                end_x
            } else {
                area.x.saturating_add(area.width).saturating_sub(1)
            };

            for column in from..=to {
                let selectable = mask_buffer
                    .cell((column, row))
                    .is_some_and(|cell| cell.fg == SELECTION_MARKER);

                if selectable {
                    if let Some(cell) = buffer.cell_mut((column, row)) {
                        cell.set_style(selection_style);
                    }
                }
            }
        }
        return selected_transcript_text(
            transcript,
            lines,
            selection_line_starts,
            selection_message_indices,
            selection,
            usize::from(scroll),
            transcript_inner_area,
        );
    }

    String::new()
}

/// Return precisely the visible text covered by a visual selection.
///
/// Markdown source does not have a one-to-one column mapping to its rendered
/// form: syntax such as `**` is omitted from the displayed cells. Extracting
/// text from the cached display rows therefore preserves the user's exact
/// partial selection, including selections from wrapped lines.
fn selected_transcript_text(
    transcript: &TranscriptRenderer,
    lines: &[Line<'static>],
    selection_line_starts: &[Option<usize>],
    selection_message_indices: &[Option<usize>],
    selection: super::ChatSelection,
    scroll: usize,
    transcript_area: Rect,
) -> String {
    let ((start_x, start_y), (end_x, end_y)) = selection.bounds();
    let mut selected_lines = Vec::new();

    for (visible_index, ((line, selectable_start), message_index)) in lines
        .iter()
        .skip(scroll)
        .zip(selection_line_starts.iter().skip(scroll))
        .zip(selection_message_indices.iter().skip(scroll))
        .take(usize::from(transcript_area.height))
        .enumerate()
    {
        let screen_row = transcript_area
            .y
            .saturating_add(u16::try_from(visible_index).unwrap_or(u16::MAX));

        if !(start_y..=end_y).contains(&screen_row) {
            continue;
        }

        let Some(selectable_start) = selectable_start else {
            continue;
        };

        let from = if screen_row == start_y {
            start_x
        } else {
            transcript_area.x
        };

        let through = if screen_row == end_y {
            end_x
        } else {
            transcript_area
                .x
                .saturating_add(transcript_area.width)
                .saturating_sub(1)
        };

        let from = usize::from(from.saturating_sub(transcript_area.x));
        let through = usize::from(through.saturating_sub(transcript_area.x));
        if through < *selectable_start || from > through {
            continue;
        }

        let selected_text = selected_line_text(line, from.max(*selectable_start), through);
        let rendered_line = selected_line_text(line, *selectable_start, usize::MAX);
        let source_markdown = message_index
            .and_then(|index| transcript.messages().get(index))
            .and_then(|message| {
                source_markdown_fragment(&message.content, &rendered_line, &selected_text)
            })
            .unwrap_or(selected_text);

        selected_lines.push(source_markdown);
    }

    selected_lines.join("\n")
}

/// Map a selection from one rendered Markdown row back to its source text.
///
/// A whole rendered source row copies its unmodified Markdown, retaining block
/// markers such as headings and list prefixes. A partial row is sliced from
/// that raw source row, so only the selected text is copied.
fn source_markdown_fragment(
    markdown: &str,
    rendered_line: &str,
    selected_text: &str,
) -> Option<String> {
    let source_line = markdown.lines().find(|source_line| {
        markdown_to_text(source_line).trim_end_matches('\n') == rendered_line
    })?;

    if selected_text == rendered_line {
        return Some(source_line.to_string());
    }

    let start = source_line.find(selected_text)?;
    let end = start + selected_text.len();

    Some(source_line[start..end].to_string())
}

/// Return characters in `line` whose terminal cells intersect the range.
fn selected_line_text(
    line: &Line<'_>,
    from: usize,
    through: usize,
) -> String {
    let mut text = String::new();
    let mut column = 0usize;

    for span in &line.spans {
        for character in span.content.chars() {
            let width = character.width().unwrap_or(0);
            let character_end = column.saturating_add(width.saturating_sub(1));

            if width == 0 {
                if column > from && column <= through {
                    text.push(character);
                }
            } else if column <= through && character_end >= from {
                text.push(character);
            }

            column = column.saturating_add(width);
        }
    }

    text
}

/// Render a modal warning before deleting all saved sessions in a directory.
fn render_clear_sessions_confirmation(
    frame: &mut ratatui::Frame,
    confirmation: Option<&super::ClearSessionsConfirmation>,
) {
    let Some(confirmation) = confirmation else {
        return;
    };

    let area = frame.area();
    let popup_width = area.width.saturating_sub(4).min(72);
    let popup_height = 7;

    if popup_width <= 4 || area.height < popup_height + 2 {
        return;
    }

    let popup_area = Rect::new(
        area.x + (area.width - popup_width) / 2,
        area.y + (area.height - popup_height) / 2,
        popup_width,
        popup_height,
    );
    let session_count = confirmation.session_ids.len();
    let message = format!(
        "Delete {session_count} saved session{} for this directory?\n\nEnter or Y: delete · Esc or N: cancel",
        if session_count == 1 { "" } else { "s" }
    );

    frame.render_widget(Clear, popup_area);
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().fg(Color::LightRed))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Clear saved sessions "),
            ),
        popup_area,
    );
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
        "shell" => partial_json_string_value(arguments, "command")
            .map(|command| format!("shell {}", truncate_one_line(&command, 100))),

        "read" | "write" | "edit" | "ls" => {
            partial_json_string_value(arguments, "path").map(|path| {
                format!(
                    "{tool_name} {}",
                    hallucinate_agent::config::display_path(&path, cwd)
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

/// Return whether a [`Line`] consists solely of the card-rail marker `"│ "`.
fn line_is_empty_card_rail(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content == "│ ")
}

/// Decode the temporary link marker carried through Markdown wrapping.
fn link_index(style: Style) -> Option<usize> {
    let Some(Color::Rgb(red, green, blue)) = style.underline_color else {
        return None;
    };

    let id = (usize::from(red) << 16) | (usize::from(green) << 8) | usize::from(blue);

    id.checked_sub(1)
}

fn link_marker(
    mut style: Style,
    index: usize,
) -> Style {
    let id = index + 1;

    style.underline_color = Some(Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8));

    style
}

/// Foreground color marker for selectable text in the transcript.
const SELECTION_MARKER: Color = Color::Rgb(1, 2, 3);

/// Foreground color marker for non-selectable text in the transcript.
const NON_SELECTION_MARKER: Color = Color::Rgb(4, 5, 6);

/// Make a copy of a rendered line for the selection mask. Keeping the original
/// symbols means ratatui applies exactly the same word wrapping to the mask as
/// it applies to the visible transcript; only the foreground color differs.
fn selection_mask_line(
    line: &Line<'static>,
    selectable_start: Option<usize>,
) -> Line<'static> {
    let mut column = 0usize;

    let spans = line
        .spans
        .iter()
        .map(|span| {
            let selectable = selectable_start.is_some_and(|start| column >= start);

            column += span.content.width();

            Span::styled(
                span.content.clone(),
                Style::default().fg(if selectable {
                    SELECTION_MARKER
                } else {
                    NON_SELECTION_MARKER
                }),
            )
        })
        .collect();

    Line {
        style: line.style,
        alignment: line.alignment,
        spans,
    }
}

/// Prefix a Markdown line with the transcript's accent rail while preserving
/// all of the inline styles produced by the Markdown renderer.
fn prefix_message_line(
    line: Line<'static>,
    accent: Color,
) -> Line<'static> {
    let mut spans = vec![Span::styled("│ ", Style::default().fg(accent))];
    spans.extend(line.spans);

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alt_screen::Message;

    #[test]
    fn link_hit_testing_respects_wrapping_padding_and_scroll() {
        let (body, links) = markdown_to_clickable_lines("[documentation](https://example.com)", 12);

        let mut rows = Vec::new();

        for line in body {
            rows.extend(
                wrap_line_to_width(line, 12)
                    .into_iter()
                    .map(|line| prefix_message_line(line, Color::LightCyan)),
            );
        }
        let mut cache = TranscriptLayoutCache::default();
        cache.lines = rows;
        cache.links = links;

        let area = Rect::new(0, 1, 16, 2);

        assert_eq!(cache.link_at(3, 1, area, 0), Some("https://example.com"));
        assert_eq!(cache.link_at(1, 1, area, 0), None);
        assert_eq!(cache.link_at(0, 1, area, 0), None);
        assert_eq!(cache.link_at(3, 1, area, 1), Some("https://example.com"));
        assert_eq!(cache.link_at(3, 0, area, 0), None);
    }

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
    fn copies_only_the_selected_rendered_text() {
        let lines = vec![
            prefix_message_line(Line::from("first line"), Color::LightCyan),
            prefix_message_line(Line::from("second line"), Color::LightCyan),
        ];

        let mut transcript = TranscriptRenderer::new();
        transcript.add_message(Message {
            role: MessageRole::Assistant,
            content: "first line\nsecond line".to_string(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            timestamp: chrono::Utc::now(),
        });

        let selected = selected_transcript_text(
            &transcript,
            &lines,
            &[Some(2), Some(2)],
            &[Some(0), Some(0)],
            super::super::ChatSelection {
                anchor: (4, 1),
                focus: (6, 2),
            },
            0,
            Rect::new(0, 1, 80, 4),
        );

        assert_eq!(selected, "rst line\nsecon");
    }

    #[test]
    fn selected_text_uses_the_corresponding_source_markdown_line() {
        assert_eq!(
            source_markdown_fragment("- **bold**", "• bold", "• bold"),
            Some("- **bold**".to_string())
        );
        assert_eq!(
            source_markdown_fragment("- **bold**", "• bold", "bold"),
            Some("bold".to_string())
        );
    }
    #[test]
    fn selected_text_preserves_wide_characters() {
        let line = prefix_message_line(Line::from("a界b"), Color::LightCyan);

        assert_eq!(selected_line_text(&line, 3, 4), "界");
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
            Vec::new(),
            vec![Some(0)],
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
        cache.replace(
            &transcript,
            80,
            false,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );

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

        let mut buffer = Buffer::empty(area);

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
    fn cached_viewport_clears_cells_not_covered_by_shorter_rows() {
        let lines = vec![Line::from("short")];

        let area = Rect::new(0, 0, 10, 2);

        let mut buffer = Buffer::empty(area);
        buffer.cell_mut((8, 0)).unwrap().set_symbol("x");
        buffer.cell_mut((0, 1)).unwrap().set_symbol("y");

        ratatui::widgets::Widget::render(
            TranscriptViewport {
                lines: &lines,
                scroll: 0,
            },
            area,
            &mut buffer,
        );

        assert_eq!(buffer.cell((8, 0)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((0, 1)).unwrap().symbol(), " ");
    }

    #[test]
    fn prefixed_tool_rows_wrap_without_losing_the_continuation_rail() {
        let mut lines = Vec::new();
        append_prefixed_lines(
            &mut lines,
            "one two three",
            "├─ ",
            "│  ",
            Style::default(),
            Style::default(),
            8,
        );

        assert!(lines.len() > 1);
        assert_eq!(lines[0].spans[0].content, "├─ ");
        assert_eq!(lines[1].spans[0].content, "│  ");
        assert!(lines.iter().all(|line| line.width() <= 8));
    }
}
