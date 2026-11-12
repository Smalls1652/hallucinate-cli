//! Markdown rendering for the terminal UI and non-interactive console output.
//!
//! Markdown is parsed into Comrak's AST rather than converted to HTML and then
//! stripped back to text. This keeps the useful structure (headings, lists,
//! code blocks, and inline emphasis) while allowing ratatui to preserve styles
//! when it wraps a line.

use comrak::{
    Arena,
    Options,
    nodes::{ListType, NodeValue, TableAlignment},
    parse_document,
};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Convert Markdown into styled terminal lines.
///
/// The returned lines are owned, so they can safely be retained by ratatui
/// while the parser's arena is dropped. Paragraph wrapping is deliberately
/// left to ratatui's `Paragraph` widget, which knows the available width.
pub fn markdown_to_lines(markdown: &str) -> Vec<Line<'static>> {
    markdown_to_lines_with_table_width(markdown, None)
}

/// Convert Markdown into styled terminal lines sized for a visible viewport.
///
/// Tables are laid out to fit `max_width`, wrapping their cell content before
/// borders are drawn. Other blocks retain their natural width so their caller
/// can apply its usual wrapping and prefixes.
pub fn markdown_to_lines_with_width(
    markdown: &str,
    max_width: usize,
) -> Vec<Line<'static>> {
    markdown_to_lines_with_table_width(markdown, Some(max_width))
}

/// Render Markdown with link destinations encoded in span underline colors.
/// The interactive transcript extracts these markers after wrapping and
/// removes them before painting the spans to the terminal.
pub(crate) fn markdown_to_clickable_lines(
    markdown: &str,
    max_width: usize,
) -> (Vec<Line<'static>>, Vec<String>) {
    let mut links = Vec::new();
    let lines = render_markdown(markdown, Some(max_width), Some(&mut links));
    (lines, links)
}

pub fn markdown_to_lines_with_table_width(
    markdown: &str,
    max_width: Option<usize>,
) -> Vec<Line<'static>> {
    render_markdown(markdown, max_width, None)
}

fn render_markdown(
    markdown: &str,
    max_width: Option<usize>,
    links: Option<&mut Vec<String>>,
) -> Vec<Line<'static>> {
    let arena = Arena::new();

    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;

    let root = parse_document(&arena, markdown, &options);

    let mut renderer = Renderer {
        lines: Vec::new(),
        table_max_width: max_width,
        links,
    };
    renderer.blocks(root);

    while renderer.lines.last().is_some_and(Vec::is_empty) {
        renderer.lines.pop();
    }

    renderer.lines.into_iter().map(Line::from).collect()
}

/// Return the rendered Markdown without terminal control sequences.
///
/// This is useful when `--print` is redirected or its output is consumed by
/// another program: the Markdown structure is still rendered, but the output
/// remains plain text.
pub fn markdown_to_text(markdown: &str) -> String {
    markdown_to_lines(markdown)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if markdown.is_empty() { "" } else { "\n" }
}

/// Write rendered Markdown to a regular terminal using ANSI styles.
///
/// This is used by `--print` when stdout is a terminal. ANSI sequences are
/// emitted only for spans which have a style.
pub fn markdown_to_ansi(markdown: &str) -> String {
    let mut output = String::new();

    for line in markdown_to_lines(markdown) {
        for span in line.spans {
            let style = ansi_style(span.style);

            if !style.is_empty() {
                output.push_str(&style);
                output.push_str(&span.content);
                output.push_str("\x1b[0m");
            } else {
                output.push_str(&span.content);
            }
        }

        output.push('\n');
    }

    output
}

/// Renders Comrak's Markdown AST into styled terminal [`Line`]s.
///
/// Accumulates spans in rows, applying indentation prefixes for nested blocks
/// (lists, quotes, code blocks, tables).
struct Renderer<'a> {
    /// Buffered output lines as vectors of styled spans.
    lines: Vec<Vec<Span<'static>>>,

    /// Maximum rendered width for a table, if constrained by a viewport.
    table_max_width: Option<usize>,

    links: Option<&'a mut Vec<String>>,
}

impl Renderer<'_> {
    /// Recursively render all child nodes of the given parent.
    fn blocks<'a>(
        &mut self,
        parent: &'a comrak::nodes::AstNode<'a>,
    ) {
        for node in parent.children() {
            self.block(node, 0);
        }
    }

    /// Render a single AST node into the output lines.
    ///
    /// Dispatches on [`NodeValue`] variant (paragraph, heading, code block,
    /// list, table, etc.) and renders each with appropriate styling.
    fn block<'a>(
        &mut self,
        node: &'a comrak::nodes::AstNode<'a>,
        indent: usize,
    ) {
        let value = node.data().value.clone();

        match value {
            NodeValue::Document => self.blocks(node),

            NodeValue::Paragraph => {
                for line in inline_lines(node, Style::default(), self.links.as_deref_mut()) {
                    self.lines.push(with_prefix(line, " ".repeat(indent)));
                }

                self.separator();
            },

            NodeValue::Heading(heading) => {
                let style = Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD);

                for line in inline_lines(node, style, self.links.as_deref_mut()) {
                    self.lines.push(with_prefix(
                        line,
                        format!(
                            "{indent} {heading_level} ",
                            indent = " ".repeat(indent),
                            heading_level = "#".repeat(heading.level.into())
                        ),
                    ));
                }

                self.separator();
            },

            NodeValue::CodeBlock(code) => {
                let code_style = Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(ratatui::style::Modifier::DIM);

                for code_line in code.literal.trim_end_matches('\n').split('\n') {
                    let mut line = vec![Span::styled(
                        format!("{}│ ", " ".repeat(indent)),
                        Style::default().fg(Color::DarkGray),
                    )];

                    line.push(Span::styled(code_line.to_string(), code_style));

                    self.lines.push(line);
                }

                if code.literal.is_empty() {
                    self.lines.push(vec![Span::styled(
                        format!("{}│ ", " ".repeat(indent)),
                        Style::default().fg(Color::DarkGray),
                    )]);
                }

                self.separator();
            },

            NodeValue::List(list) => {
                self.render_list(node, list, indent);
                self.separator();
            },

            NodeValue::BlockQuote => {
                let mut nested = Renderer {
                    lines: Vec::new(),
                    table_max_width: self.table_max_width,
                    links: self.links.as_deref_mut(),
                };
                nested.blocks(node);
                nested.remove_trailing_separators();

                for line in nested.lines {
                    let mut prefixed = vec![Span::styled(
                        format!("{}│ ", " ".repeat(indent)),
                        Style::default().fg(Color::DarkGray),
                    )];
                    prefixed.extend(line);

                    self.lines.push(prefixed);
                }

                self.separator();
            },

            NodeValue::ThematicBreak => {
                self.lines.push(vec![Span::styled(
                    format!("{}────────────────", " ".repeat(indent)),
                    Style::default().fg(Color::DarkGray),
                )]);
                self.separator();
            },

            NodeValue::Table(_) => {
                self.render_table(node, indent);
                self.separator();
            },

            NodeValue::HtmlBlock(html) => {
                for line in html.literal.trim_end_matches('\n').split('\n') {
                    self.lines.push(vec![Span::styled(
                        format!("{}{}", " ".repeat(indent), line),
                        Style::default().fg(Color::DarkGray),
                    )]);
                }

                self.separator();
            },

            // Item and cell nodes are rendered by their containing list/table.
            // For less common container extensions, recursively rendering the
            // children is more useful than dropping their text.
            NodeValue::Item(_) | NodeValue::TableRow(_) | NodeValue::TableCell => self.blocks(node),

            _ => {
                if node.first_child().is_some() {
                    self.blocks(node);
                } else if let NodeValue::Text(text) = value {
                    self.lines.push(vec![Span::raw(text.into_owned())]);
                }
            },
        }
    }

    /// Recursively render a list node and all its items.
    ///
    /// Supports ordered, bullet, and task lists. Task list items are rendered
    /// with `[ ]` or `[x]` markers; ordered lists use incremental numbers.
    fn render_list<'a>(
        &mut self,
        node: &'a comrak::nodes::AstNode<'a>,
        list: comrak::nodes::NodeList,
        indent: usize,
    ) {
        let mut number = list.start;

        for item in node.children() {
            let item_value = item.data().value.clone();

            let task = match item_value {
                NodeValue::TaskItem(task) => task.symbol,
                _ => None,
            };

            let marker = match (list.list_type, task) {
                (_, Some(symbol)) => format!(
                    "[{}] ",
                    if symbol == 'x' || symbol == 'X' {
                        'x'
                    } else {
                        ' '
                    }
                ),

                (ListType::Ordered, None) => {
                    let marker = format!("{}. ", number);
                    number += 1;
                    marker
                },

                (ListType::Bullet, None) => "• ".to_string(),
            };

            let mut nested = Renderer {
                lines: Vec::new(),
                table_max_width: self.table_max_width,
                links: self.links.as_deref_mut(),
            };
            nested.list_item_blocks(item);
            nested.remove_trailing_separators();

            if nested.lines.is_empty() {
                nested.lines.push(Vec::new());
            }

            for (index, mut line) in nested.lines.into_iter().enumerate() {
                let prefix = if index == 0 {
                    format!("{}{}", " ".repeat(indent), marker)
                } else {
                    " ".repeat(indent + marker.chars().count())
                };

                line.insert(
                    0,
                    Span::styled(
                        prefix,
                        if index == 0 {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default()
                        },
                    ),
                );

                self.lines.push(line);
            }
        }
    }

    /// Render a list item's children, keeping nested lists adjacent to their
    /// parent item text.
    fn list_item_blocks<'a>(
        &mut self,
        item: &'a comrak::nodes::AstNode<'a>,
    ) {
        for child in item.children() {
            if matches!(child.data().value, NodeValue::List(_)) {
                // Paragraph rendering terminates with a separator. A nested
                // list is part of the preceding item, not a new block beside
                // it, so remove that separator before emitting the child.
                self.remove_trailing_separators();
            }

            self.block(child, 0);
        }
    }

    /// Render a table node with aligned columns and border dividers.
    ///
    /// Measures all cells first so every column divider aligns across rows,
    /// regardless of how many columns exist in the table.
    fn render_table<'a>(
        &mut self,
        node: &'a comrak::nodes::AstNode<'a>,
        indent: usize,
    ) {
        // Render the complete table as a unit. The previous implementation
        // emitted each cell at its natural width and used a fixed two-column
        // divider (`├─┼─┤`), which made tables with more than two columns look
        // like their borders were broken. Measuring the cells first keeps all
        // column dividers aligned.
        let alignments = match &node.data().value {
            NodeValue::Table(table) => table.alignments.clone(),
            _ => Vec::new(),
        };

        let rows = node
            .children()
            .map(|row| {
                let header = matches!(row.data().value, NodeValue::TableRow(true));

                let style = if header {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                let cells = row
                    .children()
                    .map(|cell| inline_lines(cell, style, self.links.as_deref_mut()))
                    .collect::<Vec<_>>();

                (header, cells)
            })
            .collect::<Vec<_>>();

        let column_count = rows.iter().map(|(_, cells)| cells.len()).max().unwrap_or(0);

        if column_count == 0 {
            return;
        }

        let mut widths = vec![0; column_count];

        for (_, cells) in &rows {
            for (column, cell_lines) in cells.iter().enumerate() {
                widths[column] = widths[column].max(
                    cell_lines
                        .iter()
                        .map(|line| line_width(line))
                        .max()
                        .unwrap_or(0),
                );
            }
        }

        let max_cell_width = self.table_max_width.and_then(|max_width| {
            let border_width = indent + 1 + column_count * 3;
            max_width.checked_sub(border_width)
        });

        if let Some(max_cell_width) = max_cell_width {
            fit_table_widths(&mut widths, max_cell_width);
        }

        let border_style = Style::default().fg(Color::DarkGray);

        for (row_index, (_, cells)) in rows.into_iter().enumerate() {
            if row_index > 0 {
                self.lines.push(vec![Span::styled(
                    table_divider(indent, &widths),
                    border_style,
                )]);
            }

            let cells = cells
                .into_iter()
                .enumerate()
                .map(|(column, cell_lines)| {
                    cell_lines
                        .into_iter()
                        .flat_map(|line| {
                            let width = u16::try_from(widths[column]).unwrap_or(u16::MAX);
                            crate::utils::wrap_line_to_width(Line::from(line), width)
                        })
                        .map(|line| line.spans)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1);

            for line_index in 0..height {
                let mut line = vec![Span::styled(
                    format!("{}│", " ".repeat(indent)),
                    border_style,
                )];

                for column in 0..column_count {
                    line.push(Span::raw(" "));

                    let cell_line = cells
                        .get(column)
                        .and_then(|lines| lines.get(line_index))
                        .cloned()
                        .unwrap_or_default();

                    let content_width = line_width(&cell_line);

                    let extra = widths[column].saturating_sub(content_width);

                    let (left, right) = table_padding(
                        alignments
                            .get(column)
                            .copied()
                            .unwrap_or(TableAlignment::None),
                        extra,
                    );

                    if left > 0 {
                        line.push(Span::raw(" ".repeat(left)));
                    }
                    line.extend(cell_line);

                    if right > 0 {
                        line.push(Span::raw(" ".repeat(right)));
                    }
                    line.push(Span::raw(" "));

                    line.push(Span::styled("│", border_style));
                }

                self.lines.push(line);
            }
        }
    }

    /// Push a blank separator line between blocks, avoiding duplicates.
    fn separator(&mut self) {
        if !self.lines.last().is_some_and(Vec::is_empty) {
            self.lines.push(Vec::new());
        }
    }

    /// Remove trailing blank separator lines from the buffer.
    fn remove_trailing_separators(&mut self) {
        while self.lines.last().is_some_and(Vec::is_empty) {
            self.lines.pop();
        }
    }
}

/// Render inline nodes under a single AST node into styled lines.
fn inline_lines<'a>(
    node: &'a comrak::nodes::AstNode<'a>,
    style: Style,
    mut links: Option<&mut Vec<String>>,
) -> Vec<Vec<Span<'static>>> {
    let mut lines = vec![Vec::new()];

    for child in node.children() {
        inline_node(child, style, &mut lines, links.as_deref_mut());
    }

    lines
}

/// Render a single inline node and append styled spans to [`lines`].
fn inline_node<'a>(
    node: &'a comrak::nodes::AstNode<'a>,
    style: Style,
    lines: &mut Vec<Vec<Span<'static>>>,
    mut links: Option<&mut Vec<String>>,
) {
    let value = node.data().value.clone();

    match value {
        NodeValue::Text(text) => add_text(lines, text.into_owned(), style),

        // A terminal has no HTML whitespace collapsing, so retaining soft
        // breaks makes Markdown source line breaks visible and predictable.
        NodeValue::SoftBreak => lines.push(Vec::new()),

        NodeValue::LineBreak => lines.push(Vec::new()),

        NodeValue::Code(code) => add_text(
            lines,
            code.literal,
            style
                .fg(Color::Yellow)
                .bg(Color::Rgb(45, 45, 45))
                .add_modifier(Modifier::BOLD),
        ),

        NodeValue::Emph => {
            children_inline(node, style.add_modifier(Modifier::ITALIC), lines, links)
        },

        NodeValue::Strong => {
            children_inline(node, style.add_modifier(Modifier::BOLD), lines, links)
        },

        NodeValue::Strikethrough => children_inline(
            node,
            style.add_modifier(Modifier::CROSSED_OUT),
            lines,
            links,
        ),

        NodeValue::Link(link) => {
            let link_style = style.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
            let link_style = mark_link(link_style, &link.url, links.as_deref_mut());
            children_inline(node, link_style, lines, links.as_deref_mut());

            let label = node
                .children()
                .map(|child| match &child.data().value {
                    NodeValue::Text(text) => text.to_string(),
                    _ => String::new(),
                })
                .collect::<String>();
            if !link.url.is_empty() && label != link.url {
                add_text(lines, " (".to_string(), style);
                add_text(lines, link.url, link_style);
                add_text(lines, ")".to_string(), style);
            }
        },

        NodeValue::Image(link) => {
            add_text(lines, "[".to_string(), style);
            children_inline(node, style.fg(Color::Cyan), lines, links.as_deref_mut());
            add_text(lines, "](".to_string(), style);
            let link_style = mark_link(
                style.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED),
                &link.url,
                links,
            );
            add_text(lines, link.url, link_style);
            add_text(lines, ")".to_string(), style);
        },

        NodeValue::HtmlInline(text) | NodeValue::Raw(text) => add_text(lines, text, style),

        _ => children_inline(node, style, lines, links),
    }
}

/// Recursively render all child inline nodes with the given [`Style`].
fn children_inline<'a>(
    node: &'a comrak::nodes::AstNode<'a>,
    style: Style,
    lines: &mut Vec<Vec<Span<'static>>>,
    mut links: Option<&mut Vec<String>>,
) {
    for child in node.children() {
        inline_node(child, style, lines, links.as_deref_mut());
    }
}

/// Reserve an underline color as a temporary link identifier. Zero is never
/// a link; the marker is stripped before the transcript reaches the terminal.
fn mark_link(
    style: Style,
    url: &str,
    links: Option<&mut Vec<String>>,
) -> Style {
    let Some(links) = links else { return style };

    if url.is_empty() || links.len() >= 0xff_ffff {
        return style;
    }

    links.push(url.to_string());

    let id = links.len();

    style.underline_color(Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8))
}

/// Append a text span (split by newlines) onto [`lines`] with the given style.
fn add_text(
    lines: &mut Vec<Vec<Span<'static>>>,
    text: String,
    style: Style,
) {
    for (index, part) in text.split('\n').enumerate() {
        if index > 0 {
            lines.push(Vec::new());
        }

        if !part.is_empty() {
            lines
                .last_mut()
                .expect("inline renderer always has a line")
                .push(Span::styled(part.to_string(), style));
        }
    }
}

/// Compute the display width of a line using Unicode character widths.
fn line_width(line: &[Span<'static>]) -> usize {
    line.iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

/// Reduce column widths until their combined content width fits the viewport.
///
/// Every column retains one content cell when possible. Extra cells are
/// distributed in column order, which keeps narrow columns compact and gives
/// long cells the remaining room to wrap naturally.
fn fit_table_widths(
    widths: &mut [usize],
    max_total_width: usize,
) {
    let natural_widths = widths.to_vec();
    let minimum_total_width = widths.len();
    let target_width = max_total_width.max(minimum_total_width);
    let natural_total_width = natural_widths.iter().sum::<usize>();

    if natural_total_width <= target_width {
        return;
    }

    widths.fill(1);
    let mut remaining_width = target_width.saturating_sub(minimum_total_width);

    while remaining_width > 0 {
        let mut allocated_width = false;

        for (width, natural_width) in widths.iter_mut().zip(&natural_widths) {
            if remaining_width == 0 {
                break;
            }

            if *width < *natural_width {
                *width += 1;
                remaining_width -= 1;
                allocated_width = true;
            }
        }

        if !allocated_width {
            break;
        }
    }
}

/// Compute left and right padding around a table cell based on alignment.
fn table_padding(
    alignment: TableAlignment,
    extra: usize,
) -> (usize, usize) {
    match alignment {
        TableAlignment::Right => (extra, 0),

        TableAlignment::Center => (extra / 2, extra - extra / 2),

        TableAlignment::None | TableAlignment::Left => (0, extra),
    }
}

/// Build a divider row for a rendered table.
fn table_divider(
    indent: usize,
    widths: &[usize],
) -> String {
    let segments = widths
        .iter()
        .map(|width| "─".repeat(width + 2))
        .collect::<Vec<_>>();

    format!("{}├{}┤", " ".repeat(indent), segments.join("┼"))
}

/// Prepend a prefix [`Span`] to an existing line.
fn with_prefix(
    mut line: Vec<Span<'static>>,
    prefix: String,
) -> Vec<Span<'static>> {
    if !prefix.is_empty() {
        line.insert(0, Span::raw(prefix));
    }

    line
}

/// Convert a [`ratatui::style::Style`] into an ANSI escape-code string.
///
/// Emits foreground/background color codes (256-color, true color, or indexed)
/// and modifier flags (bold, italic, underline, crossed-out, dim). Returns an
/// empty string when the style has no effects.
fn ansi_style(style: Style) -> String {
    let mut codes = Vec::new();

    if let Some(color) = style.fg {
        codes.push(match color {
            Color::Reset => "39".to_string(),

            Color::Black => "30".to_string(),

            Color::Red => "31".to_string(),

            Color::Green => "32".to_string(),

            Color::Yellow => "33".to_string(),

            Color::Blue => "34".to_string(),

            Color::Magenta => "35".to_string(),

            Color::Cyan => "36".to_string(),

            Color::Gray => "37".to_string(),

            Color::DarkGray => "90".to_string(),

            Color::LightRed => "91".to_string(),

            Color::LightGreen => "92".to_string(),

            Color::LightYellow => "93".to_string(),

            Color::LightBlue => "94".to_string(),

            Color::LightMagenta => "95".to_string(),

            Color::LightCyan => "96".to_string(),

            Color::White => "97".to_string(),

            Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),

            Color::Indexed(index) => format!("38;5;{index}"),
        });
    }

    if let Some(color) = style.bg {
        codes.push(match color {
            Color::Reset => "49".to_string(),

            Color::Black => "40".to_string(),

            Color::Red => "41".to_string(),

            Color::Green => "42".to_string(),

            Color::Yellow => "43".to_string(),

            Color::Blue => "44".to_string(),

            Color::Magenta => "45".to_string(),

            Color::Cyan => "46".to_string(),

            Color::Gray => "47".to_string(),

            Color::DarkGray => "100".to_string(),

            Color::LightRed => "101".to_string(),

            Color::LightGreen => "102".to_string(),

            Color::LightYellow => "103".to_string(),

            Color::LightBlue => "104".to_string(),

            Color::LightMagenta => "105".to_string(),

            Color::LightCyan => "106".to_string(),

            Color::White => "107".to_string(),

            Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),

            Color::Indexed(index) => format!("48;5;{index}"),
        });
    }

    let modifiers = style.add_modifier;

    if modifiers.contains(Modifier::BOLD) {
        codes.push("1".to_string());
    }

    if modifiers.contains(Modifier::ITALIC) {
        codes.push("3".to_string());
    }

    if modifiers.contains(Modifier::UNDERLINED) {
        codes.push("4".to_string());
    }

    if modifiers.contains(Modifier::CROSSED_OUT) {
        codes.push("9".to_string());
    }

    if modifiers.contains(Modifier::DIM) {
        codes.push("2".to_string());
    }

    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: Vec<Line<'static>>) -> String {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn bare_urls_are_shown_once_and_remain_clickable() {
        let (lines, links) = markdown_to_clickable_lines("https://example.com/path", 80);

        assert_eq!(plain(lines.clone()), "https://example.com/path");
        assert_eq!(links, ["https://example.com/path"]);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.style.underline_color.is_some())
        );
    }

    #[test]
    fn labeled_links_show_a_clickable_destination() {
        let (lines, links) = markdown_to_clickable_lines("[docs](https://example.com)", 80);

        assert_eq!(plain(lines.clone()), "docs (https://example.com)");
        assert_eq!(links, ["https://example.com"]);
        assert!(lines[0].spans.iter().any(|span| {
            span.content == "https://example.com" && span.style.underline_color.is_some()
        }));
    }

    #[test]
    fn plain_output_does_not_include_terminal_link_markers() {
        assert_eq!(
            markdown_to_text("https://example.com"),
            "https://example.com\n"
        );
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
    fn renders_markdown_structure_without_markers() {
        let output = plain(markdown_to_lines("# Title\n\n- **one**\n- `two`"));

        assert!(output.contains("Title"));
        assert!(output.contains("• one"));
        assert!(output.contains("two"));
        assert!(!output.contains("**"));
    }

    #[test]
    fn renders_nested_lists_without_internal_padding() {
        let output = plain(markdown_to_lines(
            "- Parent\n  - Child one\n  - Child two\n- Sibling",
        ));

        assert_eq!(output, "• Parent\n  • Child one\n  • Child two\n• Sibling");
    }

    #[test]
    fn ansi_output_contains_emphasis_codes() {
        let output = markdown_to_ansi("**bold**");

        assert!(output.contains("\x1b[1m"));
        assert!(output.contains("bold"));
    }

    #[test]
    fn wraps_table_cells_before_rendering_their_borders() {
        let lines = markdown_to_lines_with_width(
            "| Name | Description |\n| --- | --- |\n| A | A long description that must wrap inside its cell. |",
            30,
        );

        let table_lines = lines
            .into_iter()
            .filter(|line| String::from(line.clone()).contains('│'))
            .collect::<Vec<_>>();

        assert!(table_lines.len() > 3);
        assert!(table_lines.iter().all(|line| line.width() <= 30));
        assert!(table_lines.iter().all(|line| {
            let text = String::from(line.clone());
            text.starts_with('│') && text.ends_with('│')
        }));
    }

    #[test]
    fn separates_each_logical_row_without_splitting_wrapped_cells() {
        let lines = markdown_to_lines_with_width(
            "| Name | Description |\n| --- | --- |\n| A | A long description that wraps |\n| B | Another long description that wraps |",
            24,
        );
        let text = plain(lines.clone());
        let rows = text.lines().collect::<Vec<_>>();
        let dividers = rows
            .iter()
            .enumerate()
            .filter(|(_, line)| line.starts_with('├'))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        assert_eq!(dividers.len(), 2);
        assert_eq!(dividers[0], 1);
        assert!(dividers[1] > dividers[0] + 2);
        assert!(rows[dividers[0] + 1].contains("A"));
        assert!(rows[dividers[1] + 1].contains("B"));
        assert!(rows.last().unwrap().ends_with('│'));
        assert!(lines.iter().all(|line| line.width() <= 24));
        assert!(
            dividers
                .iter()
                .all(|&index| { lines[index].spans[0].style.fg == Some(Color::DarkGray) })
        );
    }

    #[test]
    fn separates_body_rows_with_aligned_dividers() {
        let output = plain(markdown_to_lines(
            "| Name | Age | City |\n| :--- | ---: | :---: |\n| A | 2 | X |\n| B | 30 | Y |",
        ));

        assert_eq!(
            output,
            "│ Name │ Age │ City │\n├──────┼─────┼──────┤\n│ A    │   2 │  X   │\n├──────┼─────┼──────┤\n│ B    │  30 │  Y   │"
        );
    }

    #[test]
    fn header_only_table_has_no_trailing_divider() {
        let output = plain(markdown_to_lines("| Name | Value |\n| --- | --- |"));

        assert_eq!(output, "│ Name │ Value │");
    }

    #[test]
    fn renders_tables_with_aligned_columns_and_dividers() {
        let output = plain(markdown_to_lines(
            "| Name | Age | City |\n| :--- | ---: | :---: |\n| A | 2 | X |",
        ));

        assert!(output.contains("├──────┼─────┼──────┤"));
        assert!(output.contains("│ A    │   2 │  X   │"));

        let mut table_lines = output
            .lines()
            .filter(|line| line.contains('│') || line.contains('├'));
        assert!(table_lines.all(|line| { unicode_width::UnicodeWidthStr::width(line) == 21 }));
    }
}
