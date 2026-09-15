//! Tool result rendering for the TUI transcript.
//!
//! Renders color-coded boxes for tool output — amber for running, green for
//! success, red for errors — with line wrapping, edit-diff styling, and compact
//! vs. full display modes.

use std::borrow::Cow;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    components::box_width,
    utils::{truncate_width, wrap_line_to_width},
};

/// Cached visual transcript layout for a particular content width and display
/// configuration. It intentionally excludes scrolling so viewport redraws can
/// reuse the complete layout without reparsing historic Markdown or cloning
/// every visual line.
const COMPACT_TOOL_RESULT_WIDTH: usize = 100;

/// Append a color-coded surface for tool output.
///
/// The assistant card rail remains visible on the left, with a small gap before
/// the result box. The box is half-width in sufficiently wide terminals, but
/// fills the remaining width in narrow terminals so its content stays legible.
/// Running output uses an amber surface distinct from completed success and
/// failure results, so streamed output visibly indicates that the tool has not
/// finished yet.
pub fn append_tool_result_box(
    lines: &mut Vec<Line<'static>>,
    result: &str,
    is_error: bool,
    is_running: bool,
    width: u16,
) {
    if width == 0 {
        return;
    }

    let background = if is_running {
        Color::Rgb(65, 50, 15)
    } else if is_error {
        Color::Rgb(80, 20, 20)
    } else {
        Color::Rgb(15, 65, 30)
    };

    let style = Style::default().fg(Color::DarkGray).bg(background);
    let rail_style = Style::default().fg(Color::LightBlue);

    let box_width = box_width(width);

    // Keep the output clear of the coloured box edges without overflowing very
    // narrow terminal layouts.
    let horizontal_padding = usize::from(box_width >= 3);
    let content_width = box_width.saturating_sub(horizontal_padding * 2).max(1);

    lines.push(Line::from(vec![
        Span::styled("│ ", rail_style),
        Span::styled(" ".repeat(box_width), style),
    ]));

    for physical_line in result.split('\n') {
        for line in wrap_line_to_width(
            styled_tool_result_line(physical_line.trim_end(), style),
            u16::try_from(content_width).unwrap_or(u16::MAX),
        ) {
            let content_width_used = line.width();
            let remaining_width = content_width.saturating_sub(content_width_used);

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
}

/// Style edit diffs directly in tool-result surfaces without exposing Markdown
/// syntax in the transcript.
pub fn styled_tool_result_line(
    content: &str,
    default_style: Style,
) -> Line<'static> {
    let (marker, text, marker_style) = if let Some(text) = content.strip_prefix("  − ") {
        (
            "  − ",
            text,
            default_style
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some(text) = content.strip_prefix("  + ") {
        (
            "  + ",
            text,
            default_style
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
    } else if content.starts_with("Edit ") {
        (
            "",
            content,
            default_style
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        return Line::from(Span::styled(content.to_string(), default_style));
    };

    Line::from(vec![
        Span::styled(marker, marker_style),
        Span::styled(text.to_string(), default_style),
    ])
}

/// Remove the internal success or failure marker from persisted tool output.
///
/// Status is represented by the result box colour rather than a repeated emoji
/// in the transcript.
pub fn tool_result_content(result: &str) -> &str {
    result
        .strip_prefix("✅ ")
        .or_else(|| result.strip_prefix("❌ "))
        .unwrap_or(result)
}

/// Return a tool result in its selected transcript display mode.
///
/// Full output remains subject to the tool's execution-time result limit; this
/// setting only controls the TUI's compact display.
pub fn tool_result_for_display<'a>(
    result: &'a str,
    show_full_tool_output: bool,
) -> Cow<'a, str> {
    if show_full_tool_output {
        Cow::Borrowed(result)
    } else {
        Cow::Owned(truncate_width(result, COMPACT_TOOL_RESULT_WIDTH))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        append_tool_result_box(&mut success_lines, "done", false, false, 95);

        assert_eq!(success_lines.len(), 2);
        assert_eq!(success_lines[0].width(), 49);
        assert_eq!(success_lines[0].spans[0].content, "│ ");
        assert_eq!(success_lines[0].spans[0].style.bg, None);
        assert_eq!(
            success_lines[0].spans[1].style.bg,
            Some(Color::Rgb(15, 65, 30)),
        );
        assert_eq!(success_lines[0].spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(success_lines[0].spans[1].content, " ".repeat(47));
        assert_eq!(success_lines[1].width(), 49);
        assert_eq!(success_lines[1].spans[0].content, "│ ");
        assert_eq!(success_lines[1].spans[0].style.bg, None);
        assert_eq!(
            success_lines[1].spans[1].style.bg,
            Some(Color::Rgb(15, 65, 30)),
        );
        assert_eq!(success_lines[1].spans[1].style.fg, Some(Color::DarkGray));

        let success_content = success_lines[1]
            .spans
            .iter()
            .skip(1)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(success_content, format!(" done{}", " ".repeat(42)));

        let mut error_lines = Vec::new();
        append_tool_result_box(&mut error_lines, "failed", true, false, 95);

        assert_eq!(error_lines.len(), 2);
        assert_eq!(error_lines[0].width(), 49);
        assert_eq!(
            error_lines[0].spans[1].style.bg,
            Some(Color::Rgb(80, 20, 20)),
        );
        assert_eq!(error_lines[0].spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(error_lines[0].spans[1].content, " ".repeat(47));
        assert_eq!(error_lines[1].spans[1].style.fg, Some(Color::DarkGray));

        let mut running_lines = Vec::new();
        append_tool_result_box(&mut running_lines, "streaming", false, true, 95);

        assert_eq!(
            running_lines[0].spans[1].style.bg,
            Some(Color::Rgb(65, 50, 15)),
        );
        assert_eq!(
            running_lines[1].spans[1].style.bg,
            Some(Color::Rgb(65, 50, 15)),
        );
    }

    #[test]
    fn tool_result_boxes_fill_narrow_terminal_widths() {
        let mut lines = Vec::new();
        append_tool_result_box(&mut lines, "done", false, false, 20);

        assert_eq!(lines[0].width(), 20);
        assert_eq!(lines[0].spans[1].content, " ".repeat(18));
        assert_eq!(lines[1].width(), 20);
    }

    #[test]
    fn styles_edit_diff_lines_without_markdown() {
        let default_style = Style::default().fg(Color::DarkGray);
        let removed = styled_tool_result_line("  − old value", default_style);
        let added = styled_tool_result_line("  + new value", default_style);
        let header = styled_tool_result_line("Edit 1 · 1 occurrence(s)", default_style);

        assert_eq!(removed.spans[0].content, "  − ");
        assert_eq!(removed.spans[1].content, "old value");
        assert_eq!(removed.spans[1].style, default_style);
        assert_eq!(added.spans[0].content, "  + ");
        assert_eq!(added.spans[0].style.fg, Some(Color::LightGreen));
        assert_eq!(header.spans[0].style.fg, Some(Color::LightCyan));
    }

    #[test]
    fn tool_result_content_removes_the_status_emoji() {
        assert_eq!(tool_result_content("✅ completed"), "completed");
        assert_eq!(tool_result_content("❌ failed"), "failed");
        assert_eq!(tool_result_content("unmarked output"), "unmarked output");
    }
}
