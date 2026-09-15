//! Components used by the purpose-built interactive UI.

pub mod editor;
pub mod markdown;
pub mod picker_overlays;
pub mod status_bar;
pub mod thinking;
pub mod tool_results;

pub use editor::Editor;
pub use markdown::{markdown_to_ansi, markdown_to_lines, markdown_to_text};

const MINIMUM_HALF_WIDTH_BOX_TERMINAL_WIDTH: u16 = 95;

/// Return the width used by compact transcript surfaces.
///
/// In narrow terminals the box fills the remaining width; in wide ones it
/// stays at half width for a balanced layout.
pub(crate) fn box_width(width: u16) -> usize {
    if width < MINIMUM_HALF_WIDTH_BOX_TERMINAL_WIDTH {
        // Half-width output becomes too narrow for useful content below a
        // conventional 95-column terminal.
        usize::from(width.saturating_sub(2).max(1))
    } else {
        usize::from(width / 2)
    }
}
