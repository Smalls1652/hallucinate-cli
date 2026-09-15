//! Thinking/reasoning surface for the TUI transcript.
//!
//! Renders streamed LLM reasoning inside a muted, boxed panel — distinct from
//! the main assistant response and tool output. Supports markdown rendering and
//! line wrapping with consistent padding.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    components::{box_width, markdown_to_lines},
    utils::{line_is_empty, wrap_line_to_width},
};

/// Append a muted, boxed reasoning surface inside an assistant card.
///
/// Thinking uses the same compact dimensions and padded rows as tool output,
/// making streamed reasoning a distinct transcript item instead of text that
/// blends into the assistant response.
pub fn append_thinking_box(
    lines: &mut Vec<Line<'static>>,
    thinking: &str,
    width: u16,
) {
    if width == 0 {
        return;
    }

    let style = Style::default()
        .fg(Color::Gray)
        .bg(Color::Rgb(45, 45, 45))
        .add_modifier(Modifier::DIM);
    let rail_style = Style::default().fg(Color::Gray);

    let box_width = box_width(width);
    let horizontal_padding = usize::from(box_width >= 3);
    let content_width = box_width.saturating_sub(horizontal_padding * 2).max(1);

    lines.push(Line::from(vec![
        Span::styled("│ ", rail_style),
        Span::styled(" ".repeat(box_width), style),
    ]));

    let heading_style = style.add_modifier(Modifier::ITALIC);
    append_thinking_box_line(
        lines,
        Line::from(Span::styled("💭 Thinking", heading_style)),
        style,
        rail_style,
        content_width,
        horizontal_padding,
    );

    let mut thinking_lines = markdown_to_lines(thinking);
    while thinking_lines.last().is_some_and(line_is_empty) {
        thinking_lines.pop();
    }

    for thinking_line in thinking_lines {
        // Reasoning formatting is intentionally flattened so this surface stays
        // quiet.
        let thinking_line = Line::from(
            thinking_line
                .spans
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), style))
                .collect::<Vec<_>>(),
        );

        append_thinking_box_line(
            lines,
            thinking_line,
            style,
            rail_style,
            content_width,
            horizontal_padding,
        );
    }

    lines.push(Line::from(vec![
        Span::styled("│ ", rail_style),
        Span::styled(" ".repeat(box_width), style),
    ]));
}

/// Append one wrapped line to a thinking surface, preserving its padding.
pub fn append_thinking_box_line(
    lines: &mut Vec<Line<'static>>,
    line: Line<'static>,
    style: Style,
    rail_style: Style,
    content_width: usize,
    horizontal_padding: usize,
) {
    for line in wrap_line_to_width(line, u16::try_from(content_width).unwrap_or(u16::MAX)) {
        let remaining_width = content_width.saturating_sub(line.width());

        let mut spans = vec![Span::styled("│ ", rail_style)];

        if horizontal_padding > 0 {
            spans.push(Span::styled(" ".repeat(horizontal_padding), style));
        }
        spans.extend(line.spans);

        if remaining_width > 0 {
            spans.push(Span::styled(" ".repeat(remaining_width), style));
        }

        if horizontal_padding > 0 {
            spans.push(Span::styled(" ".repeat(horizontal_padding), style));
        }

        lines.push(Line::from(spans));
    }
}
