//! Undo and redo support for the interactive text editor.

use std::collections::VecDeque;

/// A byte-indexed edit operation. The editor only creates operations at UTF-8
/// character boundaries, so replaying them cannot split a character.
#[derive(Debug, Clone)]
pub enum EditOperation {
    /// An insertion operation.
    Insert {
        /// Byte position in the text where the text was inserted.
        position: usize,

        /// The text that was inserted.
        text: String,
    },

    /// A deletion operation.
    Delete {
        /// Byte position in the text where the deletion started.
        position: usize,

        /// Number of bytes removed.
        length: usize,

        /// The text that was deleted.
        deleted_text: String,
    },
}

impl EditOperation {
    /// Apply this operation in reverse (i.e., undo it) against `text`.
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
    /// Stack of operations available to undo, front is oldest.
    undo_stack: VecDeque<EditOperation>,

    /// Stack of operations available to redo, front is oldest.
    redo_stack: VecDeque<EditOperation>,

    /// Maximum number of entries in the undo stack.
    max_size: usize,
}

impl UndoStack {
    /// Create a new undo stack with the given maximum size.
    pub fn new(max_size: usize) -> Self {
        Self {
            undo_stack: VecDeque::with_capacity(max_size),
            redo_stack: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    /// Push an operation onto the undo stack, clearing the redo stack.
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

    /// Undo the most recent operation and return it for cursor restoration.
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

    /// Redo the most recent undone operation.
    pub fn redo(
        &mut self,
        text: &mut String,
    ) -> Option<EditOperation> {
        let operation = self.redo_stack.pop_back()?;
        operation.apply(text);

        self.undo_stack.push_back(operation.clone());

        Some(operation)
    }

    /// Clear both the undo and redo stacks.
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
