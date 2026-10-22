//! Semantic color tokens for the interface.
//!
//! One token per *meaning*, not per widget: a color says something about the
//! state it paints, and the same meaning always wears the same color. Only
//! ANSI named colors are used — no hardcoded RGB — so the app inherits the
//! terminal's light/dark theme and degrades cleanly on 16/256-color terminals.
//! The background is never painted; the terminal's own background shows
//! through.
//!
//! | Token       | Meaning                                            |
//! |-------------|----------------------------------------------------|
//! | [`STRUCTURE`] | layout: project headers, dialog borders, titles |
//! | [`SELECT`]    | selection and success: "this will be affected"  |
//! | [`DANGER`]    | destruction and failure. Reserved — nothing else may use red |
//! | [`CAUTION`]   | work in progress: measuring sizes, deleting     |
//! | [`KEY`]       | keycaps and interactive affordances             |
//! | [`MUTED`]     | metadata: timestamps, hints, secondary text     |

use ratatui::style::{Color, Modifier, Style};

pub const STRUCTURE: Color = Color::Blue;
pub const SELECT: Color = Color::Green;
pub const DANGER: Color = Color::Red;
pub const CAUTION: Color = Color::Yellow;
pub const KEY: Color = Color::Cyan;
pub const MUTED: Color = Color::DarkGray;

/// Emphasized text (dialog titles, totals): bold in the terminal's own
/// foreground rather than a token color, so emphasis never competes with
/// meaning.
pub fn emphasis() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Cursor row highlight: reverse video.
///
/// A background fill can never collide with the foreground colors that carry
/// meaning — a selected row under the cursor keeps its green foreground,
/// reversed, instead of the cursor color erasing the selection signal.
pub fn cursor() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// Quiet secondary text: timestamps, hints, descriptions.
pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

/// Keycap accent inside hints and buttons: `[Enter]`, `[Space]`.
pub fn keycap() -> Style {
    Style::default().fg(KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_distinct_ansi_colors() {
        let tokens = [STRUCTURE, SELECT, DANGER, CAUTION, KEY, MUTED];
        for (i, a) in tokens.iter().enumerate() {
            for b in &tokens[i + 1..] {
                assert_ne!(a, b, "semantic tokens must not share a color");
            }
        }
    }

    #[test]
    fn cursor_is_reverse_video_without_foreground() {
        let style = cursor();
        assert!(style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(style.fg, None, "cursor must not carry a foreground color");
    }

    #[test]
    fn emphasis_is_bold_without_foreground() {
        let style = emphasis();
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(style.fg, None);
    }
}
