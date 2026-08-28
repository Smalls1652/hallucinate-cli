//! Undo and redo support for the interactive text editor.

use std::collections::VecDeque;

/// A byte-indexed edit operation. The editor only creates operations at UTF-8
/// character boundaries, so replaying them cannot split a character.
#[derive(Debug, Clone)]
pub enum EditOperation {
    Insert {
        position: usize,
        text: String,
    },
    Delete {
        position: usize,
        length: usize,
        deleted_text: String,
    },
}

impl EditOperation {
    fn apply(
        &self,
        text: &mut String,
    ) {
        match self {
            Self::Insert {
                position,
                text: inserted,
            } => {
                let position = (*position).min(text.len());
                text.insert_str(position, inserted);
            },
            Self::Delete {
                position, length, ..
            } => {
                let position = (*position).min(text.len());
                let end = position.saturating_add(*length).min(text.len());
                if position < end && text.is_char_boundary(position) && text.is_char_boundary(end) {
                    text.drain(position..end);
                }
            },
        }
    }
}

/// Bounded undo history. It returns the replayed operation so the editor can
/// restore the cursor position as well as the text.
pub struct UndoStack {
    undo_stack: VecDeque<EditOperation>,
    redo_stack: VecDeque<EditOperation>,
    max_size: usize,
}

impl UndoStack {
    pub fn new(max_size: usize) -> Self {
        Self {
            undo_stack: VecDeque::with_capacity(max_size),
            redo_stack: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    pub fn push(
        &mut self,
        operation: EditOperation,
    ) {
        self.undo_stack.push_back(operation);
        self.redo_stack.clear();
        while self.undo_stack.len() > self.max_size {
            self.undo_stack.pop_front();
        }
    }

    pub fn undo(
        &mut self,
        text: &mut String,
    ) -> Option<EditOperation> {
        let operation = self.undo_stack.pop_back()?;
        match &operation {
            EditOperation::Insert {
                position,
                text: inserted,
            } => {
                let position = (*position).min(text.len());
                let end = position.saturating_add(inserted.len()).min(text.len());
                if position < end && text.is_char_boundary(position) && text.is_char_boundary(end) {
                    text.drain(position..end);
                }
            },
            EditOperation::Delete {
                position,
                deleted_text,
                ..
            } => {
                let position = (*position).min(text.len());
                if text.is_char_boundary(position) {
                    text.insert_str(position, deleted_text);
                }
            },
        }
        self.redo_stack.push_back(operation.clone());
        Some(operation)
    }

    pub fn redo(
        &mut self,
        text: &mut String,
    ) -> Option<EditOperation> {
        let operation = self.redo_stack.pop_back()?;
        operation.apply(text);
        self.undo_stack.push_back(operation.clone());
        Some(operation)
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new(100)
    }
}
