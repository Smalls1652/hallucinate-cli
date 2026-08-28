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
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;

    let root = parse_document(&arena, markdown, &options);
    let mut renderer = Renderer { lines: Vec::new() };
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

struct Renderer {
    lines: Vec<Vec<Span<'static>>>,
}

impl Renderer {
    fn blocks<'a>(
        &mut self,
        parent: &'a comrak::nodes::AstNode<'a>,
    ) {
        for node in parent.children() {
            self.block(node, 0);
        }
    }

    fn block<'a>(
        &mut self,
        node: &'a comrak::nodes::AstNode<'a>,
        indent: usize,
    ) {
        let value = node.data().value.clone();
        match value {
            NodeValue::Document => self.blocks(node),
            NodeValue::Paragraph => {
                for line in inline_lines(node, Style::default()) {
                    self.lines.push(with_prefix(line, " ".repeat(indent)));
                }
                self.separator();
            },
            NodeValue::Heading(heading) => {
                let style = Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD);
                for line in inline_lines(node, style) {
                    self.lines.push(with_prefix(line, " ".repeat(indent)));
                }
                self.separator();
                let _ = heading;
            },
            NodeValue::CodeBlock(code) => {
                let code_style = Style::default().fg(Color::Green);
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
                let mut nested = Renderer { lines: Vec::new() };
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

            let mut nested = Renderer { lines: Vec::new() };
            nested.blocks(item);
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
                    .map(|cell| inline_lines(cell, style))
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

        let border_style = Style::default().fg(Color::DarkGray);
        for (header, cells) in rows {
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
            if header {
                self.lines.push(vec![Span::styled(
                    table_divider(indent, &widths),
                    border_style,
                )]);
            }
        }
    }

    fn separator(&mut self) {
        if !self.lines.last().is_some_and(Vec::is_empty) {
            self.lines.push(Vec::new());
        }
    }

    fn remove_trailing_separators(&mut self) {
        while self.lines.last().is_some_and(Vec::is_empty) {
            self.lines.pop();
        }
    }
}

fn inline_lines<'a>(
    node: &'a comrak::nodes::AstNode<'a>,
    style: Style,
) -> Vec<Vec<Span<'static>>> {
    let mut lines = vec![Vec::new()];
    for child in node.children() {
        inline_node(child, style, &mut lines);
    }
    lines
}

fn inline_node<'a>(
    node: &'a comrak::nodes::AstNode<'a>,
    style: Style,
    lines: &mut Vec<Vec<Span<'static>>>,
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
        NodeValue::Emph => children_inline(node, style.add_modifier(Modifier::ITALIC), lines),
        NodeValue::Strong => children_inline(node, style.add_modifier(Modifier::BOLD), lines),
        NodeValue::Strikethrough => {
            children_inline(node, style.add_modifier(Modifier::CROSSED_OUT), lines)
        },
        NodeValue::Link(link) => {
            let link_style = style.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
            children_inline(node, link_style, lines);
            if !link.url.is_empty() {
                add_text(lines, format!(" ({})", link.url), style.fg(Color::DarkGray));
            }
        },
        NodeValue::Image(link) => {
            add_text(lines, "[".to_string(), style);
            children_inline(node, style.fg(Color::Cyan), lines);
            add_text(lines, format!("]({})", link.url), style.fg(Color::DarkGray));
        },
        NodeValue::HtmlInline(text) | NodeValue::Raw(text) => add_text(lines, text, style),
        _ => children_inline(node, style, lines),
    }
}

fn children_inline<'a>(
    node: &'a comrak::nodes::AstNode<'a>,
    style: Style,
    lines: &mut Vec<Vec<Span<'static>>>,
) {
    for child in node.children() {
        inline_node(child, style, lines);
    }
}

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

fn line_width(line: &[Span<'static>]) -> usize {
    line.iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

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

fn with_prefix(
    mut line: Vec<Span<'static>>,
    prefix: String,
) -> Vec<Span<'static>> {
    if !prefix.is_empty() {
        line.insert(0, Span::raw(prefix));
    }
    line
}

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
    fn renders_markdown_structure_without_markers() {
        let output = plain(markdown_to_lines("# Title\n\n- **one**\n- `two`"));
        assert!(output.contains("Title"));
        assert!(output.contains("• one"));
        assert!(output.contains("two"));
        assert!(!output.contains("**"));
    }

    #[test]
    fn ansi_output_contains_emphasis_codes() {
        let output = markdown_to_ansi("**bold**");
        assert!(output.contains("\x1b[1m"));
        assert!(output.contains("bold"));
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
