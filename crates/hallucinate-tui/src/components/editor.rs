//! Editor component
/// A multi-line editor with cursor movement, undo/redo, and text manipulation.
use ratatui::Frame;
use ratatui::{
    crossterm::event::KeyCode,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use unicode_width::UnicodeWidthChar;

use super::super::{
    input::InputEvent,
    keys::Key,
    undo::{EditOperation, UndoStack},
};

/// An editor component for text input.
pub struct Editor {
    /// The current text content.
    content: String,

    /// Byte offset of the cursor within `content`.
    cursor_pos: usize,

    /// The preferred display column used when moving through lines of
    /// different lengths.
    vertical_column: Option<usize>,

    /// Placeholder text shown when the editor is empty.
    placeholder: Option<String>,

    /// Style applied to the rendered text.
    style: Style,

    /// Stack for undo/redo operations.
    undo_stack: UndoStack,

    /// Byte offset of the selection anchor (None when no selection is active).
    selection_anchor: Option<usize>,

    /// Inner editor width from the most recent render, used for visual-row
    /// cursor movement through wrapped input.
    visual_width: usize,
}

impl Editor {
    /// Create a new editor.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            cursor_pos: 0,
            vertical_column: None,
            placeholder: None,
            style: Style::default().fg(Color::White),
            undo_stack: UndoStack::new(100),
            selection_anchor: None,
            visual_width: 0,
        }
    }

    /// Set the placeholder text.
    pub fn placeholder(
        mut self,
        placeholder: impl Into<String>,
    ) -> Self {
        self.placeholder = Some(placeholder.into());

        self
    }

    /// Get the current content.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Set the content.
    pub fn set_content(
        &mut self,
        content: impl Into<String>,
    ) {
        self.content = content.into();
        self.cursor_pos = self.content.len();
        self.vertical_column = None;
        self.selection_anchor = None;
        self.undo_stack.clear();
    }

    /// Whether the editor occupies more than one visual row.
    ///
    /// Explicit newlines always create rows. Once rendered, soft-wrapped input
    /// also uses the current inner width so Up/Down move the editor cursor
    /// rather than scrolling the transcript.
    pub fn is_multiline(&self) -> bool {
        if self.content.contains('\n') {
            return true;
        }

        self.visual_width > 0
            && editor_visual_lines(&self.content, self.visual_width, self.cursor_pos).len() > 1
    }

    /// Return the height needed to display the editor, including its border.
    /// The editor grows for short messages and scrolls internally for longer
    /// messages so it cannot completely hide the transcript.
    pub fn desired_height(
        &self,
        area_width: u16,
    ) -> u16 {
        let width = usize::from(area_width.saturating_sub(2));

        let rows = if self.content.is_empty() {
            1
        } else {
            editor_visual_lines(&self.content, width, self.cursor_pos).len()
        };

        (rows as u16 + 2).clamp(3, 8)
    }

    /// Insert text at the cursor position.
    pub fn insert_text(
        &mut self,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }

        // Paste sources can use either Unix or Windows line endings. Keep the
        // editor's internal representation predictable.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");

        self.delete_selection();

        let pos = self.cursor_pos.min(self.content.len());
        self.content.insert_str(pos, &text);
        self.cursor_pos += text.len();
        self.vertical_column = None;

        self.undo_stack.push(EditOperation::Insert {
            position: pos,
            text,
        });
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(
        &mut self,
        ch: char,
    ) {
        self.insert_text(&ch.to_string());
    }

    /// Return the active selection range as byte offsets, or None if no
    /// selection.
    fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor?;

        let range = if anchor <= self.cursor_pos {
            (anchor, self.cursor_pos)
        } else {
            (self.cursor_pos, anchor)
        };

        (range.0 < range.1).then_some(range)
    }

    /// Delete the active selection and return true if one existed.
    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection_range() else {
            self.selection_anchor = None;

            return false;
        };

        let deleted = self.content.drain(start..end).collect::<String>();

        self.cursor_pos = start;
        self.vertical_column = None;
        self.selection_anchor = None;

        self.undo_stack.push(EditOperation::Delete {
            position: start,
            length: end - start,
            deleted_text: deleted,
        });

        true
    }

    /// Delete the character before the cursor (or current selection).
    pub fn delete_char_back(&mut self) {
        if self.delete_selection() || self.cursor_pos == 0 {
            return;
        }

        let pos = self
            .content
            .char_indices()
            .rev()
            .find(|(i, _)| *i < self.cursor_pos)
            .map(|(i, ch)| (i, ch.len_utf8()))
            .unwrap_or((0, 1));

        let deleted = self.content.drain(pos.0..pos.0 + pos.1).collect::<String>();

        self.cursor_pos -= pos.1;
        self.vertical_column = None;

        self.undo_stack.push(EditOperation::Delete {
            position: pos.0,
            length: pos.1,
            deleted_text: deleted,
        });
    }

    /// Delete the character at the cursor.
    pub fn delete_char(&mut self) {
        if self.delete_selection() || self.cursor_pos >= self.content.len() {
            return;
        }

        let ch = self.content[self.cursor_pos..].chars().next().unwrap();
        let len = ch.len_utf8();

        let deleted = self
            .content
            .drain(self.cursor_pos..self.cursor_pos + len)
            .collect::<String>();

        self.vertical_column = None;

        self.undo_stack.push(EditOperation::Delete {
            position: self.cursor_pos,
            length: len,
            deleted_text: deleted,
        });
    }

    /// Move the cursor left by one character, optionally extending the
    /// selection.
    fn move_left(
        &mut self,
        extend: bool,
    ) {
        self.vertical_column = None;

        if extend && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.cursor_pos);
        } else if !extend {
            self.selection_anchor = None;
        }

        if self.cursor_pos > 0 {
            self.cursor_pos = self
                .content
                .char_indices()
                .rev()
                .find(|(i, _)| *i < self.cursor_pos)
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    /// Move the cursor right by one character, optionally extending the
    /// selection.
    fn move_right(
        &mut self,
        extend: bool,
    ) {
        self.vertical_column = None;

        if extend && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.cursor_pos);
        } else if !extend {
            self.selection_anchor = None;
        }

        if self.cursor_pos < self.content.len() {
            self.cursor_pos = self
                .content
                .char_indices()
                .find(|(i, _)| *i > self.cursor_pos)
                .map(|(i, _)| i)
                .unwrap_or(self.content.len());
        }
    }

    /// Move the cursor vertically by one line, optionally extending the
    /// selection.
    fn move_vertical(
        &mut self,
        direction: i8,
        extend: bool,
    ) {
        let lines = editor_visual_lines(&self.content, self.visual_width, self.cursor_pos);
        let (current_row, current_column) = cursor_position(&lines, self.cursor_pos);
        let column = *self.vertical_column.get_or_insert(current_column);

        let target_row = if direction < 0 {
            current_row.checked_sub(1)
        } else {
            (current_row + 1 < lines.len()).then_some(current_row + 1)
        };

        let Some(target_row) = target_row else {
            return;
        };

        if extend && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.cursor_pos);
        } else if !extend {
            self.selection_anchor = None;
        }

        let target = &lines[target_row];
        self.cursor_pos = byte_at_display_column(&self.content, target.start, target.end, column);
    }

    /// Move the cursor left and clear any selection.
    pub fn cursor_left(&mut self) {
        self.move_left(false);
    }

    /// Move the cursor right and clear any selection.
    pub fn cursor_right(&mut self) {
        self.move_right(false);
    }

    /// Delete the byte range from `start` to `end`, recording the operation.
    fn delete_range(
        &mut self,
        start: usize,
        end: usize,
    ) {
        if start >= end || end > self.content.len() {
            return;
        }

        let deleted = self.content.drain(start..end).collect::<String>();

        self.cursor_pos = start;
        self.vertical_column = None;
        self.selection_anchor = None;

        self.undo_stack.push(EditOperation::Delete {
            position: start,
            length: end - start,
            deleted_text: deleted,
        });
    }

    /// Delete the word immediately before the cursor (keeping trailing
    /// whitespace).
    fn delete_word_back(&mut self) {
        let mut start = self.cursor_pos;

        while start > 0 {
            let previous = self.content[..start].char_indices().next_back();

            let Some((index, character)) = previous else {
                break;
            };

            if !character.is_whitespace() {
                break;
            }

            start = index;
        }

        while start > 0 {
            let previous = self.content[..start].char_indices().next_back();

            let Some((index, character)) = previous else {
                break;
            };

            if character.is_whitespace() {
                break;
            }

            start = index;
        }

        self.delete_range(start, self.cursor_pos);
    }

    /// Delete from the cursor to the end of content.
    fn delete_to_end(&mut self) {
        self.delete_range(self.cursor_pos, self.content.len());
    }

    /// Delete from the beginning of content to the cursor.
    fn delete_to_beginning(&mut self) {
        self.delete_range(0, self.cursor_pos);
    }

    /// Move the cursor back one word boundary.
    pub fn word_back(&mut self) {
        self.vertical_column = None;

        // Skip any trailing whitespace before the word boundary.
        let mut pos = self.cursor_pos;

        while pos > 0 {
            let previous = self.content[..pos].char_indices().next_back();

            let Some((_index, character)) = previous else {
                break;
            };

            if !character.is_whitespace() {
                break;
            }

            pos = _index;
        }

        // Skip through the word to its start.
        while pos > 0 {
            let previous = self.content[..pos].char_indices().next_back();

            let Some((_index, character)) = previous else {
                break;
            };

            if character.is_whitespace() {
                break;
            }

            pos = _index;
        }

        self.cursor_pos = pos;
    }

    /// Move the cursor forward one word boundary.
    pub fn word_forward(&mut self) {
        self.vertical_column = None;

        let len = self.content.len();
        if self.cursor_pos >= len {
            return;
        }

        // Skip any leading whitespace after the cursor.
        let mut pos = self.cursor_pos;

        while pos < len {
            let next = self.content[pos..].chars().next().unwrap();

            if !next.is_whitespace() {
                break;
            }

            pos += next.len_utf8();
        }

        // Skip through the word to its end.
        while pos < len {
            let next = self.content[pos..].chars().next().unwrap();

            if next.is_whitespace() {
                break;
            }

            pos += next.len_utf8();
        }

        self.cursor_pos = pos;
    }

    /// Move the cursor to the beginning.
    pub fn cursor_beginning(&mut self) {
        self.cursor_pos = 0;
        self.vertical_column = None;
        self.selection_anchor = None;
    }

    /// Move the cursor to the end.
    pub fn cursor_end(&mut self) {
        self.cursor_pos = self.content.len();
        self.vertical_column = None;
        self.selection_anchor = None;
    }

    /// Undo the last edit and restore the cursor to the operation boundary.
    pub fn undo(&mut self) {
        if let Some(operation) = self.undo_stack.undo(&mut self.content) {
            self.cursor_pos = match operation {
                EditOperation::Insert { position, .. } => position,

                EditOperation::Delete {
                    position,
                    deleted_text,
                    ..
                } => position + deleted_text.len(),
            };

            self.normalize_cursor();
        }
    }

    /// Redo the last undone edit and restore the cursor after the edit.
    pub fn redo(&mut self) {
        if let Some(operation) = self.undo_stack.redo(&mut self.content) {
            self.cursor_pos = match operation {
                EditOperation::Insert { position, text } => position + text.len(),

                EditOperation::Delete { position, .. } => position,
            };

            self.normalize_cursor();
        }
    }

    /// Constrain the cursor to valid byte boundaries.
    fn normalize_cursor(&mut self) {
        self.vertical_column = None;
        self.cursor_pos = self.cursor_pos.min(self.content.len());

        while self.cursor_pos > 0 && !self.content.is_char_boundary(self.cursor_pos) {
            self.cursor_pos -= 1;
        }
    }

    /// Handle an input event.
    pub fn handle_event(
        &mut self,
        event: InputEvent,
    ) -> bool {
        match event {
            InputEvent::Key(key) => self.handle_key(key),

            InputEvent::Paste(text) => {
                self.insert_text(&text);
                false
            },

            _ => false,
        }
    }

    /// Handle a key event. Returns true if the editor should close.
    fn handle_key(
        &mut self,
        key: Key,
    ) -> bool {
        match key.code {
            KeyCode::Char(c) if key.ctrl => match c {
                'a' => {
                    self.cursor_beginning();
                },

                'e' => {
                    self.cursor_end();
                },

                'b' => {
                    self.cursor_left();
                },

                'f' => {
                    self.cursor_right();
                },

                'h' | '\x7f' => {
                    self.delete_char_back();
                },

                'd' => {
                    self.delete_char();
                },

                'p' => {
                    self.undo();
                },

                'r' => {
                    self.redo();
                },

                'w' => self.delete_word_back(),

                // Ctrl-J is a reliable fallback for terminals that do not
                // report Shift+Enter separately from Enter.
                'j' => self.insert_char('\n'),

                'k' => self.delete_to_end(),

                'u' => self.delete_to_beginning(),

                _ => {},
            },

            KeyCode::Char(c) if key.alt => match c {
                'b' => {
                    self.word_back();
                },

                'f' => {
                    self.word_forward();
                },

                _ => {},
            },

            KeyCode::Char(c) => {
                self.insert_char(c);
            },

            KeyCode::Backspace if key.alt => {
                self.delete_word_back();
            },

            KeyCode::Backspace => {
                self.delete_char_back();
            },

            KeyCode::Delete => {
                self.delete_char();
            },

            KeyCode::Left if key.alt => {
                self.word_back();
            },

            KeyCode::Left => {
                self.move_left(key.shift);
            },

            KeyCode::Right if key.alt => {
                self.word_forward();
            },

            KeyCode::Right => {
                self.move_right(key.shift);
            },

            KeyCode::Up => {
                self.move_vertical(-1, key.shift);
            },

            KeyCode::Down => {
                self.move_vertical(1, key.shift);
            },

            KeyCode::Home => {
                if key.shift && self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor_pos);
                } else if !key.shift {
                    self.selection_anchor = None;
                }

                self.vertical_column = None;
                self.cursor_pos = line_bounds(&self.content, self.cursor_pos).0;
            },

            KeyCode::End => {
                if key.shift && self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor_pos);
                } else if !key.shift {
                    self.selection_anchor = None;
                }

                self.vertical_column = None;
                self.cursor_pos = line_bounds(&self.content, self.cursor_pos).1;
            },

            KeyCode::Enter if key.shift || key.alt => {
                // Some Ghostty configurations emit ESC+Return for
                // Shift+Enter, which crossterm reports as Alt+Enter.
                self.insert_char('\n');
            },

            KeyCode::Enter => {
                return true; // Submit
            },

            KeyCode::Esc => {
                return true; // Cancel
            },

            _ => {},
        }

        false
    }

    /// Render the editor and place the terminal cursor at the logical cursor
    /// position. Explicit newlines and long lines are represented as separate
    /// visual rows, with the editor scrolling vertically when necessary.
    pub fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
    ) {
        let inner_width = usize::from(area.width.saturating_sub(2));
        self.visual_width = inner_width;

        let (lines, cursor_row, cursor_column) = if self.content.is_empty() {
            (
                vec![Line::from(Span::styled(
                    self.placeholder.as_deref().unwrap_or(""),
                    Style::default().fg(Color::DarkGray),
                ))],
                0,
                0,
            )
        } else {
            let visual = editor_visual_lines(&self.content, inner_width, self.cursor_pos);
            let (row, column) = cursor_position(&visual, self.cursor_pos);

            let lines = visual
                .iter()
                .map(|line| Line::from(Span::styled(line.text.clone(), self.style)))
                .collect();

            (lines, row, column)
        };

        let total_rows = lines.len();

        let (scroll, cursor_y) = if area.height > 2 {
            let visible_rows = usize::from(area.height.saturating_sub(2));
            let max_scroll = cursor_row.saturating_sub(visible_rows.saturating_sub(1));
            let scroll = max_scroll.min(total_rows.saturating_sub(visible_rows));

            (scroll, cursor_row.saturating_sub(scroll))
        } else {
            (0, 0)
        };

        let paragraph = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" Input "))
            .scroll((scroll as u16, 0));

        f.render_widget(paragraph, area);

        if area.width > 2 && area.height > 2 {
            let x = area.x + 1 + cursor_column.min(inner_width) as u16;
            let y = area.y + 1 + cursor_y as u16;

            f.set_cursor_position((
                x.min(area.x + area.width - 2),
                y.min(area.y + area.height - 2),
            ));
        }
    }
}

/// A single visual line (explicit or wrapped).
#[derive(Debug)]
struct VisualLine {
    /// The display text for this visual line.
    text: String,

    /// Byte offset where this visual line starts in the content.
    start: usize,

    /// Byte offset where this visual line ends in the content.
    end: usize,
}

/// Split explicit and wrapped lines into rows that fit the editor.
fn visual_lines(
    content: &str,
    width: usize,
) -> Vec<VisualLine> {
    let mut lines = Vec::new();
    let mut line_start = 0;

    for (offset, character) in content.char_indices() {
        if character == '\n' {
            append_wrapped_line(&mut lines, content, line_start, offset, width);

            line_start = offset + character.len_utf8();
        }
    }

    append_wrapped_line(&mut lines, content, line_start, content.len(), width);

    lines
}

/// Push one or more word-wrapped lines for the given content segment.
fn append_wrapped_line(
    lines: &mut Vec<VisualLine>,
    content: &str,
    start: usize,
    end: usize,
    width: usize,
) {
    if start == end || width == 0 {
        lines.push(VisualLine {
            text: content[start..end].to_string(),
            start,
            end,
        });

        return;
    }

    let mut segment_start = start;

    while segment_start < end {
        let mut segment_width = 0;
        let mut wrap_at = None;
        let mut segment_end = end;

        for (offset, character) in content[segment_start..end].char_indices() {
            let position = segment_start + offset;
            let character_width = character.width().unwrap_or(0);

            if segment_width > 0 && segment_width + character_width > width {
                // A separating space belongs to the next row when the word
                // preceding it exactly fills this row. Otherwise wrap at the
                // last separator so an entire word stays together.
                segment_end = if character.is_whitespace() {
                    position
                } else {
                    wrap_at.unwrap_or(position)
                };
                break;
            }

            segment_width += character_width;

            // Preserve whitespace in the preceding visual line. This keeps
            // byte offsets continuous, which in turn makes cursor placement
            // and selections correct while wrapping at word boundaries.
            if character.is_whitespace() {
                wrap_at = Some(position + character.len_utf8());
            }
        }

        lines.push(VisualLine {
            text: content[segment_start..segment_end].to_string(),
            start: segment_start,
            end: segment_end,
        });

        segment_start = segment_end;
    }
}

/// Split explicit and wrapped lines, appending a trailing empty line when the
/// cursor would fall off the visible edge.
fn editor_visual_lines(
    content: &str,
    width: usize,
    cursor: usize,
) -> Vec<VisualLine> {
    let mut lines = visual_lines(content, width);

    if width > 0 {
        let (row, column) = cursor_position(&lines, cursor);

        if column >= width && row + 1 == lines.len() {
            lines.push(VisualLine {
                text: String::new(),
                start: cursor,
                end: cursor,
            });
        }
    }

    lines
}

/// Return the (row, column) of the given byte offset within the visual lines.
fn cursor_position(
    lines: &[VisualLine],
    cursor: usize,
) -> (usize, usize) {
    for (row, line) in lines.iter().enumerate() {
        if cursor < line.end {
            return (row, display_width(&line.text[..cursor - line.start]));
        }

        if cursor == line.end {
            if lines.get(row + 1).is_some_and(|next| next.start == cursor) {
                return (row + 1, 0);
            }

            return (row, display_width(&line.text));
        }
    }

    (lines.len().saturating_sub(1), 0)
}

/// Return the display width (column count) of a text segment.
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|character| character.width().unwrap_or(0))
        .sum()
}

/// Return the byte range (start, end) of the line containing the cursor.
fn line_bounds(
    content: &str,
    cursor: usize,
) -> (usize, usize) {
    let cursor = cursor.min(content.len());

    let start = content[..cursor]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = content[cursor..]
        .find('\n')
        .map(|index| cursor + index)
        .unwrap_or(content.len());

    (start, end)
}

/// Find the byte offset at the given display column within a text segment.
fn byte_at_display_column(
    content: &str,
    start: usize,
    end: usize,
    column: usize,
) -> usize {
    let mut current_column = 0;

    for (offset, character) in content[start..end].char_indices() {
        let position = start + offset;
        let character_width = character.width().unwrap_or(0);

        if current_column + character_width > column {
            return position;
        }

        current_column += character_width;
    }

    end
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    use super::*;

    #[test]
    fn cursor_right_advances_by_one_unicode_scalar() {
        let mut editor = Editor::new("");
        editor.set_content("a😀b");
        editor.cursor_beginning();

        editor.cursor_right();
        assert_eq!(editor.cursor_pos, 1);

        editor.cursor_right();
        assert_eq!(editor.cursor_pos, 5);

        editor.cursor_right();
        assert_eq!(editor.cursor_pos, 6);
    }

    #[test]
    fn control_word_and_line_deletions_are_supported() {
        let mut editor = Editor::new("");

        editor.set_content("one two three");
        editor.handle_key(Key::with_modifiers(
            KeyCode::Char('w'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(editor.content(), "one two ");

        editor.handle_key(Key::with_modifiers(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(editor.content(), "");
    }

    #[test]
    fn shift_movement_selects_text_and_undo_restores_cursor() {
        let mut editor = Editor::new("");

        editor.set_content("one two three");
        for _ in 0..3 {
            editor.handle_key(Key::with_modifiers(KeyCode::Left, KeyModifiers::SHIFT));
        }
        editor.handle_key(Key::new(KeyCode::Delete));
        assert_eq!(editor.content(), "one two th");

        editor.undo();

        assert_eq!(editor.content(), "one two three");
        assert_eq!(editor.cursor_pos, editor.content.len());

        editor.redo();
        assert_eq!(editor.content(), "one two th");
        assert_eq!(editor.cursor_pos, 10);
    }

    #[test]
    fn long_unicode_input_wraps_without_splitting_characters() {
        let lines = editor_visual_lines("012😀3456789", 6, 7);

        assert!(
            lines
                .iter()
                .all(|line| line.text.is_char_boundary(line.text.len()))
        );

        assert!(lines.iter().any(|line| line.text.contains('😀')));
    }

    #[test]
    fn wrapping_prefers_word_boundaries_and_breaks_long_words() {
        let lines = visual_lines("one two three", 7);
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>();

        assert_eq!(text, vec!["one two", " three"]);

        let long_word = visual_lines("abcdefgh", 3);
        let text = long_word
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(text, vec!["abc", "def", "gh"]);
    }

    #[test]
    fn soft_wrapping_makes_the_editor_multiline_after_rendering() {
        let mut editor = Editor::new("one two three");
        editor.visual_width = 7;

        assert!(editor.is_multiline());

        editor.visual_width = 20;
        assert!(!editor.is_multiline());
    }

    #[test]
    fn vertical_movement_follows_wrapped_rows() {
        let mut editor = Editor::new("");
        editor.set_content("one two three");
        editor.visual_width = 7;
        editor.cursor_beginning();

        editor.handle_key(Key::new(KeyCode::Down));
        assert_eq!(editor.cursor_pos, 7);

        editor.handle_key(Key::new(KeyCode::Up));
        assert_eq!(editor.cursor_pos, 0);
    }

    #[test]
    fn editor_uses_the_available_width_for_wrapping() {
        let mut editor = Editor::new("");
        editor.set_content("a".repeat(99));

        // The two border cells leave 100 columns for an editor area 102
        // columns wide. The input should remain on one visual row rather
        // than wrapping at an unrelated fixed width.
        assert_eq!(editor.desired_height(102), 3);
    }

    #[test]
    fn shift_enter_inserts_a_newline_while_enter_submits() {
        let mut editor = Editor::new("");

        editor.insert_text("first");
        assert!(!editor.handle_key(Key::with_modifiers(KeyCode::Enter, KeyModifiers::SHIFT)));

        editor.insert_text("second");
        assert_eq!(editor.content(), "first\nsecond");
        assert!(editor.handle_key(Key::new(KeyCode::Enter)));
    }

    #[test]
    fn ghostty_alt_return_sequence_inserts_a_newline() {
        let mut editor = Editor::new("");

        editor.insert_text("first");
        assert!(!editor.handle_key(Key::with_modifiers(KeyCode::Enter, KeyModifiers::ALT)));
        assert_eq!(editor.content(), "first\n");
    }

    #[test]
    fn vertical_movement_preserves_the_preferred_column() {
        let mut editor = Editor::new("");

        editor.set_content("abcd\nxy\n1234");
        editor.handle_key(Key::new(KeyCode::Up));
        assert_eq!(editor.cursor_pos, 7); // column four clamps to "xy"

        editor.handle_key(Key::new(KeyCode::Up));
        assert_eq!(editor.cursor_pos, 4); // column four is restored on "abcd"

        editor.handle_key(Key::new(KeyCode::Down));
        assert_eq!(editor.cursor_pos, 7);

        editor.handle_key(Key::new(KeyCode::Down));
        assert_eq!(editor.cursor_pos, editor.content.len());
    }

    #[test]
    fn pasted_line_endings_become_editor_newlines() {
        let mut editor = Editor::new("");

        assert!(!editor.handle_event(InputEvent::Paste("one\r\ntwo\rthree".to_string())));
        assert_eq!(editor.content(), "one\ntwo\nthree");
    }

    #[test]
    fn alt_left_arrow_moves_cursor_back_one_word() {
        use ratatui::crossterm::event::KeyModifiers;

        let mut editor = Editor::new("");
        editor.set_content("hello world foo");

        // Start at end.
        assert_eq!(editor.cursor_pos, 15);

        // Alt+Left from end of "foo" should jump to start of "foo".
        editor.handle_key(Key::with_modifiers(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 12);

        // Another Alt+Left from start of "foo" jumps to start of "world".
        editor.handle_key(Key::with_modifiers(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 6);

        // Another Alt+Left jumps to start of "hello".
        editor.handle_key(Key::with_modifiers(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 0);

        // Cannot move back further.
        editor.handle_key(Key::with_modifiers(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 0);
    }

    #[test]
    fn alt_right_arrow_moves_cursor_forward_one_word() {
        use ratatui::crossterm::event::KeyModifiers;

        let mut editor = Editor::new("");
        editor.set_content("hello world foo");
        editor.cursor_beginning(); // set_content sets cursor to end, move to start

        // Start at beginning.
        assert_eq!(editor.cursor_pos, 0);

        // Alt+Right from start should jump to end of "hello".
        editor.handle_key(Key::with_modifiers(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 5);

        // Another Alt+Right jumps to end of "world".
        editor.handle_key(Key::with_modifiers(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 11);

        // Cannot move forward further (already at end).
        editor.handle_key(Key::with_modifiers(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 15);
    }

    #[test]
    fn word_navigation_handles_unicode_characters() {
        use ratatui::crossterm::event::KeyModifiers;

        let mut editor = Editor::new("");
        // "ab😀 cd" — 😀 is 4 bytes, total length is 9.
        editor.set_content("ab😀 cd");
        editor.cursor_beginning(); // set_content sets cursor to end, move to start

        assert_eq!(editor.cursor_pos, 0);

        // Skip past "ab😀" (bytes 0..5) → should land at byte offset 6 (the
        // space).
        editor.handle_key(Key::with_modifiers(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 6);

        // Skip past "cd" → should land at end (byte length = 9).
        editor.handle_key(Key::with_modifiers(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(editor.cursor_pos, 9);
    }
}
