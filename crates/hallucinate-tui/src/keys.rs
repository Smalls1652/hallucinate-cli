//! Key representation and modifier matching for the interactive UI.

use std::fmt;

use crossterm::event::{KeyCode, KeyModifiers};

/// A key with normalized modifier flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// The underlying crossterm key code.
    pub code: KeyCode,
    /// The raw modifier flags from crossterm.
    pub modifiers: KeyModifiers,
    /// Whether the Control modifier is active.
    pub ctrl: bool,
    /// Whether the Alt modifier is active.
    pub alt: bool,
    /// Whether the Shift modifier is active.
    pub shift: bool,
    /// The macOS Command key is reported as SUPER by crossterm.
    pub super_key: bool,
}

impl Key {
    /// Creates a new `Key` with no modifiers.
    pub fn new(code: KeyCode) -> Self {
        Self::with_modifiers(code, KeyModifiers::NONE)
    }

    /// Creates a new `Key` with the given modifiers.
    pub fn with_modifiers(
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Self {
        Self {
            code,
            modifiers,
            ctrl: modifiers.contains(KeyModifiers::CONTROL),
            alt: modifiers.contains(KeyModifiers::ALT),
            shift: modifiers.contains(KeyModifiers::SHIFT),
            super_key: modifiers.contains(KeyModifiers::SUPER),
        }
    }

    /// Returns whether this key matches the given character.
    pub fn matches(
        &self,
        character: char,
    ) -> bool {
        matches!(self.code, KeyCode::Char(value) if value == character)
    }
}

impl fmt::Display for Key {
    /// Formats the key as a human-readable string (e.g., "Ctrl-Enter", "Alt-a").
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let name = match self.code {
            KeyCode::Char(character) => character.to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            _ => format!("{:?}", self.code),
        };

        if self.ctrl {
            write!(f, "Ctrl-{name}")
        } else if self.alt {
            write!(f, "Alt-{name}")
        } else {
            write!(f, "{name}")
        }
    }
}
