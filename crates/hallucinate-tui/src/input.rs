//! Keyboard and terminal event conversion for the interactive event loop.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};

use super::keys::Key;

/// Input values consumed by the interactive UI and editor.
#[derive(Debug, Clone)]
pub enum InputEvent {
    Key(Key),
    Mouse(MouseEvent),
    Resize(u16, u16),
    FocusGained,
    FocusLost,
    Paste(String),
}

/// Convert a crossterm terminal event into the application's input
/// representation.
///
/// Handles special cases for macOS Terminal.app Shift+Enter, Kitty protocol
/// alternate key codes, and release/repeat key events.
pub fn parse_event(event: Event) -> InputEvent {
    match event {
        Event::Key(mut key_event) => {
            // With REPORT_ALTERNATE_KEYS enabled, crossterm represents the
            // Kitty Shift+Enter sequence as Char('\r') and clears Shift while
            // applying the alternate codepoint. Restore the logical key
            // before the editor sees it.
            if key_event.code == KeyCode::Char('\r') {
                key_event.code = KeyCode::Enter;
                if key_event.modifiers == KeyModifiers::NONE {
                    key_event.modifiers |= KeyModifiers::SHIFT;
                }
            }

            // Terminal.app historically emits plain Return for Shift+Enter,
            // even though the Shift key is still physically held. Recover the
            // modifier from macOS so the shortcut works without a terminal
            // configuration change.
            if key_event.code == KeyCode::Enter
                && key_event.modifiers == KeyModifiers::NONE
                && apple_terminal_shift_pressed()
            {
                key_event.modifiers |= KeyModifiers::SHIFT;
            }

            // Ignore release and repeat events. Returning Null keeps the
            // caller's event handling total without inventing a new variant.
            if matches!(key_event.kind, KeyEventKind::Release | KeyEventKind::Repeat) {
                return InputEvent::Key(Key::new(KeyCode::Null));
            }

            InputEvent::Key(key_to_key_event(key_event))
        },

        Event::Mouse(mouse_event) => InputEvent::Mouse(mouse_event),

        Event::Resize(width, height) => InputEvent::Resize(width, height),

        Event::FocusGained => InputEvent::FocusGained,

        Event::FocusLost => InputEvent::FocusLost,

        Event::Paste(text) => InputEvent::Paste(text),
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceKeyState(
        state_id: u32,
        key: u16,
    ) -> bool;
}

/// Terminal.app does not encode Shift+Enter. Query the local modifier state as
/// a fallback; this intentionally only applies to a local Apple Terminal
/// session because the keyboard cannot be observed over SSH.
fn apple_terminal_shift_pressed() -> bool {
    #[cfg(target_os = "macos")]
    {
        if std::env::var("TERM_PROGRAM").as_deref() != Ok("Apple_Terminal") {
            return false;
        }

        // kVK_Shift (56) and kVK_RightShift (60), using the combined session
        // state (0) documented by CoreGraphics.
        unsafe { CGEventSourceKeyState(0, 56) || CGEventSourceKeyState(0, 60) }
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Convert a crossterm [`KeyEvent`] into the application's [`Key`]
/// representation.
///
/// Preserves the key code and applies modifier information (Shift, Ctrl, Alt,
/// Super) to produce a fully qualified key event.
fn key_to_key_event(key_event: KeyEvent) -> Key {
    let modifiers = match key_event.modifiers {
        KeyModifiers::NONE => KeyModifiers::NONE,
        KeyModifiers::SHIFT => KeyModifiers::SHIFT,
        KeyModifiers::CONTROL => KeyModifiers::CONTROL,
        KeyModifiers::ALT => KeyModifiers::ALT,
        _ => key_event.modifiers,
    };

    Key::with_modifiers(key_event.code, modifiers)
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyEvent, KeyEventKind};

    use super::*;

    #[test]
    fn kitty_shift_enter_alternate_codepoint_is_restored() {
        let event = parse_event(Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\r'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )));

        assert!(matches!(
            event,
            InputEvent::Key(Key {
                code: KeyCode::Enter,
                shift: true,
                ..
            })
        ));
    }

    #[test]
    fn command_modifier_is_preserved_for_macos_copy() {
        let event = parse_event(Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('c'),
            KeyModifiers::SUPER,
            KeyEventKind::Press,
        )));

        assert!(matches!(
            event,
            InputEvent::Key(Key {
                code: KeyCode::Char('c'),
                super_key: true,
                ctrl: false,
                ..
            })
        ));
    }
}
