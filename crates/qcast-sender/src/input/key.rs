//! OS-neutral key classification.
//!
//! Decides whether a `GstNavigation` key string is a **named control/navigation
//! key** or **printable text to type**. Kept platform-neutral so the layout-proof
//! keyboard logic (the trickiest part of remote control) is unit-testable on any OS;
//! the Windows backend maps [`NamedKey`] → a virtual key and types [`KeyAction::Text`]
//! as Unicode.

/// A named control / navigation key — the non-text keys that must be injected as a
/// virtual key (with real press/release) rather than typed as a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Backspace,
    Tab,
    Escape,
    Space,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

/// What a `GstNavigation` key string means for injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// A named control/navigation key.
    Named(NamedKey),
    /// Printable text to type as Unicode (layout-proof).
    Text(String),
    /// A keysym we don't act on in v1 (F-keys, bare modifiers like `Shift_L`, …) —
    /// dropped rather than typed literally.
    Ignore,
}

/// Classify a `GstNavigation` key string. Control keys arrive as X11 keysym names
/// (`"Return"`, `"BackSpace"`); printable keys arrive as the produced character
/// (`"a"`, `"@"`). A single character is text; an unmapped multi-char keysym is
/// ignored (so we never type e.g. the literal string "F1").
pub fn classify_key(key: &str) -> KeyAction {
    use NamedKey::*;
    let named = match key {
        "Return" | "KP_Enter" => Enter,
        "BackSpace" => Backspace,
        "Tab" => Tab,
        "Escape" => Escape,
        "space" => Space,
        "Delete" => Delete,
        "Left" => Left,
        "Right" => Right,
        "Up" => Up,
        "Down" => Down,
        "Home" => Home,
        "End" => End,
        "Page_Up" => PageUp,
        "Page_Down" => PageDown,
        _ => {
            return if key.chars().count() == 1 {
                KeyAction::Text(key.to_string())
            } else {
                KeyAction::Ignore
            };
        }
    };
    KeyAction::Named(named)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_named_control_keys() {
        assert_eq!(classify_key("Return"), KeyAction::Named(NamedKey::Enter));
        assert_eq!(classify_key("KP_Enter"), KeyAction::Named(NamedKey::Enter));
        assert_eq!(classify_key("BackSpace"), KeyAction::Named(NamedKey::Backspace));
        assert_eq!(classify_key("space"), KeyAction::Named(NamedKey::Space));
        assert_eq!(classify_key("Page_Up"), KeyAction::Named(NamedKey::PageUp));
        assert_eq!(classify_key("Left"), KeyAction::Named(NamedKey::Left));
    }

    #[test]
    fn single_characters_are_text() {
        assert_eq!(classify_key("a"), KeyAction::Text("a".into()));
        assert_eq!(classify_key("A"), KeyAction::Text("A".into()));
        assert_eq!(classify_key("@"), KeyAction::Text("@".into()));
        assert_eq!(classify_key("1"), KeyAction::Text("1".into()));
    }

    #[test]
    fn unmapped_multichar_keysyms_are_ignored_not_typed() {
        // The bug guard: without this, F1 / Shift_L would be typed as literal text.
        assert_eq!(classify_key("F1"), KeyAction::Ignore);
        assert_eq!(classify_key("Shift_L"), KeyAction::Ignore);
        assert_eq!(classify_key("Control_L"), KeyAction::Ignore);
    }
}
