//! Helper utilities used across the crate.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Return whether a [`Line`] contains no non-empty spans.
pub(crate) fn line_is_empty(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.is_empty())
}

/// Wrap a styled Markdown line to the available content width.
///
/// Ratatui's [`Paragraph`] wrapper is intentionally used here so word breaking,
/// whitespace, and Unicode graphemes behave exactly as they do in the final
/// widget. Rendering into a temporary buffer lets us turn the wrapped visual
/// rows back into [`Line`]s without losing inline styles.
pub(crate) fn wrap_line_to_width(
    line: Line<'static>,
    width: u16,
) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from("")];
    }

    let line_style = line.style;
    let alignment = line.alignment;

    let paragraph = Paragraph::new(line).wrap(Wrap { trim: false });

    let line_count = paragraph.line_count(width).max(1);

    let height = u16::try_from(line_count).unwrap_or(u16::MAX);
    let area = Rect::new(0, 0, width, height);

    let mut buffer = Buffer::empty(area);
    Widget::render(paragraph, area, &mut buffer);

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

                spans.push(Span::styled(symbol.to_string(), cell.style()));

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

            Line {
                style: line_style,
                alignment,
                spans,
            }
        })
        .collect()
}

/// Append text that may contain newlines, using the supplied visual prefix on
/// continuation lines (useful for tool calls and results).
pub(crate) fn append_prefixed_lines(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    prefix: &'static str,
    continuation_prefix: &'static str,
    prefix_style: Style,
    text_style: Style,
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
            Line::from(Span::styled(
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

            let mut spans = vec![Span::styled(line_prefix, prefix_style)];
            spans.extend(line.spans);

            lines.push(Line::from(spans));
        }
    }
}

/// Truncate a string to one display line and a maximum display width.
///
/// Carriage returns and newlines are replaced with spaces so the result fits
/// on a single terminal row, then the result is truncated to `max_width`.
pub(crate) fn truncate_one_line(
    s: &str,
    max_width: usize,
) -> String {
    let single_line = s.replace(['\r', '\n'], " ");

    truncate_width(&single_line, max_width)
}

/// Truncate a string to a maximum display width.
pub(crate) fn truncate_width(
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

    #[test]
    fn width_truncation_does_not_split_wide_unicode() {
        let truncated = truncate_width("abc😀def", 5);

        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.ends_with('…'));
    }
}
