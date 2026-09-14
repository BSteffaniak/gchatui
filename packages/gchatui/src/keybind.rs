use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use bmux_keyboard::{KeyCode, KeyStroke, Modifiers};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    FocusNext,
    FocusPrevious,
    MoveDown,
    MoveUp,
    Activate,
    PageDown,
    PageUp,
    GoTop,
    GoBottom,
    Cancel,
    Authenticate,
    SetSenderAlias,
    Refresh,
    Help,
    Quit,
}

impl Action {
    pub const fn label(self) -> &'static str {
        match self {
            Self::FocusNext => "Next pane",
            Self::FocusPrevious => "Previous pane",
            Self::MoveDown => "Move down",
            Self::MoveUp => "Move up",
            Self::Activate => "Open",
            Self::PageDown => "Page down",
            Self::PageUp => "Page up",
            Self::GoTop => "Top",
            Self::GoBottom => "Bottom",
            Self::Cancel => "Cancel",
            Self::Authenticate => "Sign in / storage",
            Self::SetSenderAlias => "Set sender name",
            Self::Refresh => "Refresh",
            Self::Help => "Help",
            Self::Quit => "Quit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyChord(KeyStroke);

impl KeyChord {
    pub const fn new(stroke: KeyStroke) -> Self {
        Self(stroke)
    }

    #[cfg(test)]
    pub const fn for_character(character: char) -> Self {
        Self(KeyStroke::simple(KeyCode::Char(character)))
    }

    #[cfg(test)]
    pub const fn stroke(self) -> KeyStroke {
        self.0
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let modifiers = self.0.modifiers;
        if modifiers.ctrl {
            write!(formatter, "Ctrl+")?;
        }
        if modifiers.alt {
            write!(formatter, "Alt+")?;
        }
        if modifiers.shift {
            write!(formatter, "Shift+")?;
        }
        if modifiers.super_key {
            write!(formatter, "Super+")?;
        }
        if modifiers.hyper {
            write!(formatter, "Hyper+")?;
        }
        if modifiers.meta {
            write!(formatter, "Meta+")?;
        }
        match self.0.key {
            KeyCode::Char(character) => write!(formatter, "{character}"),
            KeyCode::Enter => formatter.write_str("Enter"),
            KeyCode::Tab => formatter.write_str("Tab"),
            KeyCode::Backspace => formatter.write_str("Backspace"),
            KeyCode::Delete => formatter.write_str("Delete"),
            KeyCode::Escape => formatter.write_str("Escape"),
            KeyCode::Space => formatter.write_str("Space"),
            KeyCode::Up => formatter.write_str("Up"),
            KeyCode::Down => formatter.write_str("Down"),
            KeyCode::Left => formatter.write_str("Left"),
            KeyCode::Right => formatter.write_str("Right"),
            KeyCode::Home => formatter.write_str("Home"),
            KeyCode::End => formatter.write_str("End"),
            KeyCode::PageUp => formatter.write_str("PageUp"),
            KeyCode::PageDown => formatter.write_str("PageDown"),
            KeyCode::Insert => formatter.write_str("Insert"),
            KeyCode::F(number) => write!(formatter, "F{number}"),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KeybindingError {
    #[error("key chord is empty")]
    EmptyChord,
    #[error("unknown key or modifier `{0}`")]
    UnknownToken(String),
    #[error("key chord must contain exactly one key")]
    InvalidChord,
    #[error("key `{chord}` is assigned to both {existing:?} and {requested:?}")]
    Conflict {
        chord: KeyChord,
        existing: Action,
        requested: Action,
    },
}

impl FromStr for KeyChord {
    type Err = KeybindingError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err(KeybindingError::EmptyChord);
        }

        let mut modifiers = Modifiers::NONE;
        let mut key = None;
        for token in value.split('+').map(str::trim) {
            let normalized = token.to_ascii_lowercase();
            match normalized.as_str() {
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "super" | "cmd" | "win" => modifiers.super_key = true,
                "hyper" => modifiers.hyper = true,
                "meta" => modifiers.meta = true,
                _ if key.is_none() => key = parse_key(token),
                _ => return Err(KeybindingError::InvalidChord),
            }
            if key.is_none() && !is_modifier(&normalized) {
                return Err(KeybindingError::UnknownToken(token.to_string()));
            }
        }
        let key = key.ok_or(KeybindingError::InvalidChord)?;
        Ok(Self(KeyStroke::with_modifiers(key, modifiers)))
    }
}

fn is_modifier(token: &str) -> bool {
    matches!(
        token,
        "ctrl" | "control" | "alt" | "shift" | "super" | "cmd" | "win" | "hyper" | "meta"
    )
}

fn parse_key(token: &str) -> Option<KeyCode> {
    let normalized = token.to_ascii_lowercase();
    match normalized.as_str() {
        "enter" | "return" => Some(KeyCode::Enter),
        "tab" => Some(KeyCode::Tab),
        "backspace" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "escape" | "esc" => Some(KeyCode::Escape),
        "space" => Some(KeyCode::Space),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "insert" => Some(KeyCode::Insert),
        _ if normalized.len() == 1 => normalized.chars().next().map(KeyCode::Char),
        _ if normalized.starts_with('f') => normalized[1..].parse().ok().map(KeyCode::F),
        _ => None,
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeybindingOverrides {
    #[serde(default)]
    pub bind: BTreeMap<Action, Vec<String>>,
    #[serde(default)]
    pub unbind: Vec<Action>,
}

#[derive(Debug, Clone)]
pub struct KeybindingRegistry {
    by_chord: BTreeMap<KeyChord, Action>,
    by_action: BTreeMap<Action, Vec<KeyChord>>,
}

impl Default for KeybindingRegistry {
    fn default() -> Self {
        let defaults = [
            (Action::FocusNext, &["Tab"][..]),
            (Action::FocusPrevious, &["Shift+Tab"][..]),
            (Action::MoveDown, &["j", "Down"][..]),
            (Action::MoveUp, &["k", "Up"][..]),
            (Action::Activate, &["Enter"][..]),
            (Action::PageDown, &["PageDown"][..]),
            (Action::PageUp, &["PageUp"][..]),
            (Action::GoTop, &["g", "Home"][..]),
            (Action::GoBottom, &["Shift+g", "End"][..]),
            (Action::Cancel, &["Escape"][..]),
            (Action::Authenticate, &["Ctrl+l"][..]),
            (Action::SetSenderAlias, &["a"][..]),
            (Action::Refresh, &["r"][..]),
            (Action::Help, &["?"][..]),
            (Action::Quit, &["q", "Ctrl+c"][..]),
        ];
        Self::from_defaults(defaults).expect("built-in keybindings must be valid")
    }
}

impl KeybindingRegistry {
    fn from_defaults<'a>(
        defaults: impl IntoIterator<Item = (Action, &'a [&'a str])>,
    ) -> Result<Self, KeybindingError> {
        let mut registry = Self {
            by_chord: BTreeMap::new(),
            by_action: BTreeMap::new(),
        };
        for (action, chords) in defaults {
            for chord in chords {
                registry.bind(action, chord.parse()?)?;
            }
        }
        Ok(registry)
    }

    pub fn with_overrides(overrides: &KeybindingOverrides) -> Result<Self, KeybindingError> {
        let mut registry = Self::default();
        for action in &overrides.unbind {
            registry.unbind(*action);
        }
        for (action, values) in &overrides.bind {
            registry.unbind(*action);
            for value in values {
                registry.bind(*action, value.parse()?)?;
            }
        }
        Ok(registry)
    }

    pub fn action_for(&self, stroke: KeyStroke) -> Option<Action> {
        self.by_chord.get(&KeyChord::new(stroke)).copied()
    }

    pub fn chords_for(&self, action: Action) -> &[KeyChord] {
        self.by_action.get(&action).map_or(&[], Vec::as_slice)
    }

    pub fn labels_for(&self, action: Action) -> Vec<String> {
        self.chords_for(action)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn bind(&mut self, action: Action, chord: KeyChord) -> Result<(), KeybindingError> {
        if let Some(existing) = self.by_chord.get(&chord)
            && *existing != action
        {
            return Err(KeybindingError::Conflict {
                chord,
                existing: *existing,
                requested: action,
            });
        }
        self.by_chord.insert(chord, action);
        self.by_action.entry(action).or_default().push(chord);
        Ok(())
    }

    fn unbind(&mut self, action: Action) {
        if let Some(chords) = self.by_action.remove(&action) {
            for chord in chords {
                self.by_chord.remove(&chord);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_labels_and_dispatch_share_registry() {
        let registry = KeybindingRegistry::default();
        let chord: KeyChord = "r".parse().unwrap();
        assert_eq!(registry.action_for(chord.stroke()), Some(Action::Refresh));
        assert_eq!(registry.labels_for(Action::Refresh), ["r"]);
    }

    #[test]
    fn override_replaces_behavior_and_label() {
        let overrides = KeybindingOverrides {
            bind: BTreeMap::from([(Action::Refresh, vec!["Ctrl+r".to_string()])]),
            unbind: Vec::new(),
        };
        let registry = KeybindingRegistry::with_overrides(&overrides).unwrap();
        let old: KeyChord = "r".parse().unwrap();
        let new: KeyChord = "Ctrl+r".parse().unwrap();
        assert_eq!(registry.action_for(old.stroke()), None);
        assert_eq!(registry.action_for(new.stroke()), Some(Action::Refresh));
        assert_eq!(registry.labels_for(Action::Refresh), ["Ctrl+r"]);
    }

    #[test]
    fn unbound_action_has_no_behavior_or_label() {
        let overrides = KeybindingOverrides {
            unbind: vec![Action::Help],
            ..KeybindingOverrides::default()
        };
        let registry = KeybindingRegistry::with_overrides(&overrides).unwrap();
        let chord: KeyChord = "?".parse().unwrap();
        assert_eq!(registry.action_for(chord.stroke()), None);
        assert!(registry.labels_for(Action::Help).is_empty());
    }

    #[test]
    fn conflicting_override_is_rejected() {
        let overrides = KeybindingOverrides {
            bind: BTreeMap::from([(Action::Refresh, vec!["q".to_string()])]),
            unbind: Vec::new(),
        };
        assert!(matches!(
            KeybindingRegistry::with_overrides(&overrides),
            Err(KeybindingError::Conflict { .. })
        ));
    }

    #[test]
    fn configuration_deserializes_actions() {
        let parsed: KeybindingOverrides = toml::from_str(
            r#"
            unbind = ["help"]
            [bind]
            refresh = ["Ctrl+r"]
            "#,
        )
        .unwrap();
        assert_eq!(parsed.unbind, [Action::Help]);
        assert_eq!(parsed.bind[&Action::Refresh], ["Ctrl+r"]);
    }
}
