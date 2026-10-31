//! AeroSpace key notation, ordered chords, and canonical conflict identities.

use std::fmt;

use crate::config_doc::{ConfigDoc, KeyPath};

/// Key names accepted by AeroSpace v0.21.3-Beta in every mapping preset.
// Only keyNotationToKeyCode accepts binding names; Key.toString includes unsupported keys.
// https://raw.githubusercontent.com/nikitabobko/AeroSpace/v0.21.3-Beta/Sources/AppBundle/config/keysMap.swift
pub const KEY_NAMES: &[&str] = &[
    "sectionSign",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "minus",
    "equal",
    "q",
    "w",
    "e",
    "r",
    "t",
    "y",
    "u",
    "i",
    "o",
    "p",
    "leftSquareBracket",
    "rightSquareBracket",
    "backslash",
    "a",
    "s",
    "d",
    "f",
    "g",
    "h",
    "j",
    "k",
    "l",
    "semicolon",
    "quote",
    "z",
    "x",
    "c",
    "v",
    "b",
    "n",
    "m",
    "comma",
    "period",
    "slash",
    "keypad0",
    "keypad1",
    "keypad2",
    "keypad3",
    "keypad4",
    "keypad5",
    "keypad6",
    "keypad7",
    "keypad8",
    "keypad9",
    "keypadClear",
    "keypadDecimalMark",
    "keypadDivide",
    "keypadEnter",
    "keypadEqual",
    "keypadMinus",
    "keypadMultiply",
    "keypadPlus",
    "pageUp",
    "pageDown",
    "home",
    "end",
    "forwardDelete",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "f13",
    "f14",
    "f15",
    "f16",
    "f17",
    "f18",
    "f19",
    "f20",
    "backtick",
    "space",
    "enter",
    "esc",
    "backspace",
    "tab",
    "left",
    "down",
    "up",
    "right",
];

/// A chord modifier, ordered canonically as cmd, alt, ctrl, shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Modifier {
    /// Command.
    Cmd,
    /// Option.
    Alt,
    /// Control.
    Ctrl,
    /// Shift.
    Shift,
}

impl Modifier {
    /// The modifier's AeroSpace spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cmd => "cmd",
            Self::Alt => "alt",
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "cmd" => Some(Self::Cmd),
            "alt" => Some(Self::Alt),
            "ctrl" => Some(Self::Ctrl),
            "shift" => Some(Self::Shift),
            _ => None,
        }
    }
}

impl fmt::Display for Modifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Unique modifiers in written order. Equality includes that order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ModifierSet(Vec<Modifier>);

impl ModifierSet {
    /// Collects modifiers without changing their order.
    ///
    /// # Errors
    ///
    /// Returns [`ChordError::DuplicateModifier`] for a repeated modifier.
    pub fn new(modifiers: impl IntoIterator<Item = Modifier>) -> Result<Self, ChordError> {
        let mut ordered = Vec::new();
        for modifier in modifiers {
            if ordered.contains(&modifier) {
                return Err(ChordError::DuplicateModifier(modifier));
            }
            ordered.push(modifier);
        }
        Ok(Self(ordered))
    }

    /// The modifiers in written order.
    #[must_use]
    pub fn as_slice(&self) -> &[Modifier] {
        &self.0
    }
}

/// A validated built-in AeroSpace key name, independent of its physical preset position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyName(String);

impl KeyName {
    /// Validates a built-in key name, not an alias.
    ///
    /// # Errors
    ///
    /// Returns [`ChordError::UnknownKey`] when the name is absent from [`KEY_NAMES`].
    pub fn new(name: impl Into<String>) -> Result<Self, ChordError> {
        let name = name.into();
        if KEY_NAMES.contains(&name.as_str()) {
            Ok(Self(name))
        } else {
            Err(ChordError::UnknownKey(name))
        }
    }

    /// The validated key name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// AeroSpace's keyboard mapping preset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Preset {
    /// The default QWERTY layout.
    #[default]
    Qwerty,
    /// The Dvorak layout.
    Dvorak,
    /// The Colemak layout.
    Colemak,
}

/// A mapping preset and explicit notation-to-key-code overrides in document order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyMapping {
    /// The keyboard layout. Missing or unrecognized values fall back to QWERTY.
    pub preset: Preset,
    /// Alias spellings and their built-in targets. Overrides may shadow built-in names.
    pub aliases: Vec<(String, KeyName)>,
}

impl KeyMapping {
    /// Reads all TOML table styles without changing the document.
    /// Invalid alias names, non-string targets, and unknown targets are skipped.
    #[must_use]
    pub fn from_doc(document: &ConfigDoc) -> Self {
        let preset = match document
            .get(&KeyPath::from(["key-mapping", "preset"]))
            .and_then(toml_edit::Item::as_str)
        {
            Some("dvorak") => Preset::Dvorak,
            Some("colemak") => Preset::Colemak,
            _ => Preset::Qwerty,
        };
        let aliases = document
            .get(&KeyPath::from(["key-mapping", "key-notation-to-key-code"]))
            .and_then(toml_edit::Item::as_table_like)
            .map(|table| {
                table
                    .iter()
                    .filter_map(|(name, target)| {
                        if name.contains('-') || name.chars().any(char::is_whitespace) {
                            return None;
                        }
                        Some((name.to_owned(), KeyName::new(target.as_str()?).ok()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { preset, aliases }
    }
}

/// A key's written notation and its resolved built-in target.
/// The target is captured when parsed, so normalization needs no mutable mapping.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChordKey {
    notation: String,
    target: KeyName,
}

impl ChordKey {
    /// The original key name or alias spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.notation
    }

    /// The built-in key name after resolving an alias.
    #[must_use]
    pub fn target(&self) -> &KeyName {
        &self.target
    }
}

impl From<KeyName> for ChordKey {
    fn from(target: KeyName) -> Self {
        Self {
            notation: target.as_str().to_owned(),
            target,
        }
    }
}

/// A parsed chord that retains modifier order and alias spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chord {
    /// Unique modifiers in their original order.
    pub modifiers: ModifierSet,
    /// Written key notation and its resolved target.
    pub key: ChordKey,
}

impl Chord {
    /// Parses modifier tokens followed by one key or alias. Aliases override built-in names.
    ///
    /// # Errors
    ///
    /// Returns [`ChordError`] for an empty chord, missing key, unknown token, or duplicate modifier.
    pub fn parse(raw: &str, mapping: &KeyMapping) -> Result<Self, ChordError> {
        if raw.is_empty() {
            return Err(ChordError::Empty);
        }
        let mut tokens = raw.split('-');
        let key = tokens.next_back().unwrap_or_default();
        let modifiers = ModifierSet::new(
            tokens
                .map(|token| {
                    Modifier::parse(token)
                        .ok_or_else(|| ChordError::UnknownModifier(token.to_owned()))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )?;
        if key.is_empty() || Modifier::parse(key).is_some() {
            return Err(ChordError::MissingKey);
        }
        let target = mapping
            .aliases
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, target)| target.clone())
            .map_or_else(|| KeyName::new(key), Ok)?;
        Ok(Self {
            modifiers,
            key: ChordKey {
                notation: key.to_owned(),
                target,
            },
        })
    }

    /// Canonical conflict identity, with sorted modifiers and the alias target.
    #[must_use]
    pub fn normalized(&self) -> NormalizedChord {
        let mut modifiers = self.modifiers.0.clone();
        modifiers.sort_unstable();
        NormalizedChord {
            modifiers,
            key: self.key.target.clone(),
        }
    }

    /// Formats the written key notation with canonical or existing modifier order.
    /// Canonical output preserves normalized equality, not arbitrary written-order equality.
    #[must_use]
    pub fn format(&self, style: ChordStyle<'_>) -> String {
        let mut modifiers = Vec::new();
        if let ChordStyle::LikeRaw(raw) = style {
            let mut tokens = raw.split('-');
            tokens.next_back();
            for modifier in tokens.filter_map(Modifier::parse) {
                if self.modifiers.0.contains(&modifier) && !modifiers.contains(&modifier) {
                    modifiers.push(modifier);
                }
            }
        }
        let mut canonical = self.modifiers.0.clone();
        canonical.sort_unstable();
        for modifier in canonical {
            if !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
        }
        format_chord(&modifiers, self.key.as_str())
    }
}

/// Modifier ordering when writing a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordStyle<'a> {
    /// Use cmd, alt, ctrl, shift order.
    Canonical,
    /// Retain shared modifiers' order from this raw chord, then append new ones canonically.
    LikeRaw(&'a str),
}

/// A chord identity for conflict checks within one key mapping.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NormalizedChord {
    modifiers: Vec<Modifier>,
    key: KeyName,
}

impl fmt::Display for NormalizedChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&format_chord(&self.modifiers, self.key.as_str()))
    }
}

fn format_chord(modifiers: &[Modifier], key: &str) -> String {
    modifiers
        .iter()
        .map(|modifier| modifier.as_str())
        .chain(std::iter::once(key))
        .collect::<Vec<_>>()
        .join("-")
}

/// Why a key name or chord is invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChordError {
    /// No chord text was supplied.
    #[error("the chord is empty")]
    Empty,
    /// The final token is not a built-in key or configured alias.
    #[error("`{0}` is not an AeroSpace key name")]
    UnknownKey(String),
    /// A token before the key is not a modifier.
    #[error("`{0}` is not an AeroSpace modifier")]
    UnknownModifier(String),
    /// A modifier occurs more than once.
    #[error("the `{0}` modifier is repeated")]
    DuplicateModifier(Modifier),
    /// The chord ends in a modifier or a separator.
    #[error("the chord must end with a key name")]
    MissingKey,
}

/// A non-empty AeroSpace binding mode name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModeName(String);

impl ModeName {
    /// Validates that the mode name is non-empty, preserving its spelling.
    ///
    /// # Errors
    ///
    /// Returns [`ModeNameError`] for an empty name.
    pub fn new(name: impl Into<String>) -> Result<Self, ModeNameError> {
        let name = name.into();
        if name.is_empty() {
            Err(ModeNameError)
        } else {
            Ok(Self(name))
        }
    }

    /// The mode name as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An empty mode name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the mode name is empty")]
pub struct ModeNameError;

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn accepts_upstream_names_and_valid_chords() {
        let mapping = KeyMapping::default();
        for name in KEY_NAMES {
            assert_eq!(KeyName::new(*name).unwrap().as_str(), *name);
            assert_eq!(Chord::parse(name, &mapping).unwrap().key.as_str(), *name);
        }
        for raw in [
            "a",
            "alt-h",
            "shift-alt-h",
            "cmd-alt-ctrl-shift-f20",
            "ctrl-keypadPlus",
        ] {
            let chord = Chord::parse(raw, &mapping).unwrap();
            assert_eq!(chord.format(ChordStyle::LikeRaw(raw)), raw);
        }
        for name in ["capsLock", "help", "volumeUp", "f21", "A", "return"] {
            assert_eq!(
                KeyName::new(name),
                Err(ChordError::UnknownKey(name.to_owned()))
            );
        }
    }

    #[test]
    fn errors_have_exact_user_facing_messages() {
        for (raw, error, message) in [
            ("", ChordError::Empty, "the chord is empty"),
            (
                "foo",
                ChordError::UnknownKey("foo".to_owned()),
                "`foo` is not an AeroSpace key name",
            ),
            (
                "meta-q",
                ChordError::UnknownModifier("meta".to_owned()),
                "`meta` is not an AeroSpace modifier",
            ),
            (
                "alt-alt-q",
                ChordError::DuplicateModifier(Modifier::Alt),
                "the `alt` modifier is repeated",
            ),
            (
                "alt-",
                ChordError::MissingKey,
                "the chord must end with a key name",
            ),
            (
                "alt",
                ChordError::MissingKey,
                "the chord must end with a key name",
            ),
            (
                "alt-shift",
                ChordError::MissingKey,
                "the chord must end with a key name",
            ),
            (
                "alt--q",
                ChordError::UnknownModifier(String::new()),
                "`` is not an AeroSpace modifier",
            ),
        ] {
            let actual = Chord::parse(raw, &KeyMapping::default()).unwrap_err();
            assert_eq!(actual, error);
            assert_eq!(actual.to_string(), message);
        }
        assert_eq!(
            ModifierSet::new([Modifier::Cmd, Modifier::Cmd]),
            Err(ChordError::DuplicateModifier(Modifier::Cmd))
        );
    }

    #[test]
    fn normalization_resolves_aliases_and_modifier_order() {
        let mapping = KeyMapping {
            aliases: vec![("leader".to_owned(), KeyName::new("h").unwrap())],
            ..KeyMapping::default()
        };
        let first = Chord::parse("shift-alt-h", &mapping).unwrap();
        let second = Chord::parse("alt-shift-h", &mapping).unwrap();
        let alias = Chord::parse("alt-shift-leader", &mapping).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.normalized(), second.normalized());
        assert_eq!(alias.normalized(), second.normalized());
        assert_eq!(alias.normalized().to_string(), "alt-shift-h");
        assert_eq!(alias.key.target().as_str(), "h");
        assert_eq!(alias.format(ChordStyle::Canonical), "alt-shift-leader");
        let identities = std::collections::HashSet::from([
            first.normalized(),
            second.normalized(),
            alias.normalized(),
        ]);
        assert_eq!(identities.len(), 1);
    }

    #[test]
    fn formats_canonical_and_existing_order() {
        for (raw, style, expected) in [
            ("shift-alt-h", ChordStyle::Canonical, "alt-shift-h"),
            (
                "shift-ctrl-alt-cmd-x",
                ChordStyle::Canonical,
                "cmd-alt-ctrl-shift-x",
            ),
            (
                "cmd-alt-ctrl-shift-x",
                ChordStyle::LikeRaw("shift-alt-h"),
                "shift-alt-cmd-ctrl-x",
            ),
            (
                "cmd-shift-x",
                ChordStyle::LikeRaw("shift-alt-h"),
                "shift-cmd-x",
            ),
            ("ctrl-alt-x", ChordStyle::LikeRaw("h"), "alt-ctrl-x"),
            ("x", ChordStyle::LikeRaw("shift-alt-h"), "x"),
            (
                "alt-shift-x",
                ChordStyle::LikeRaw("shift-shift-alt-h"),
                "shift-alt-x",
            ),
        ] {
            assert_eq!(
                Chord::parse(raw, &KeyMapping::default())
                    .unwrap()
                    .format(style),
                expected
            );
        }
    }

    #[test]
    fn reads_mapping_from_every_table_style_without_writes() {
        for source in [
            "[key-mapping]\npreset = 'dvorak'\n[key-mapping.key-notation-to-key-code]\nleader = 'q'\nbad = 'unknown'\nnumber = 1\n",
            "key-mapping.preset = 'dvorak'\nkey-mapping.key-notation-to-key-code = { leader = 'q', bad = 'unknown', number = 1 }\n",
            "key-mapping = { preset = 'dvorak', key-notation-to-key-code = { leader = 'q', bad = 'unknown', number = 1 } }\n",
        ] {
            let document = ConfigDoc::parse(source).unwrap();
            let mapping = KeyMapping::from_doc(&document);
            assert_eq!(
                mapping,
                KeyMapping {
                    preset: Preset::Dvorak,
                    aliases: vec![("leader".to_owned(), KeyName::new("q").unwrap())]
                }
            );
            assert_eq!(document.to_string(), source);
            assert_eq!(
                Chord::parse("alt-leader", &mapping).unwrap().normalized(),
                Chord::parse("alt-q", &mapping).unwrap().normalized()
            );
        }
        for (source, preset) in [
            ("", Preset::Qwerty),
            ("key-mapping.preset = 'qwerty'", Preset::Qwerty),
            ("key-mapping.preset = 'colemak'", Preset::Colemak),
            ("key-mapping.preset = 'invalid'", Preset::Qwerty),
            ("key-mapping = 1", Preset::Qwerty),
        ] {
            let mapping = KeyMapping::from_doc(&ConfigDoc::parse(source).unwrap());
            assert_eq!(mapping.preset, preset);
            assert_eq!(mapping.aliases.len(), 0);
        }
    }

    #[test]
    fn aliases_override_builtins_and_reject_unusable_notation() {
        let document = ConfigDoc::parse("key-mapping.key-notation-to-key-code = { q = 'x', 'bad-name' = 'h', 'bad name' = 'h' }").unwrap();
        let mapping = KeyMapping::from_doc(&document);
        assert_eq!(mapping.aliases.len(), 1);
        assert_eq!(
            Chord::parse("alt-q", &mapping)
                .unwrap()
                .normalized()
                .to_string(),
            "alt-x"
        );
    }

    #[test]
    fn mode_names_preserve_spelling_and_reject_empty() {
        assert_eq!(ModeName::new(""), Err(ModeNameError));
        assert_eq!(ModeNameError.to_string(), "the mode name is empty");
        for name in ["main", "service", "mode with spaces"] {
            let mode = ModeName::new(name).unwrap();
            assert_eq!(mode.as_str(), name);
            assert_eq!(mode.to_string(), name);
        }
    }

    proptest! {
        #[test]
        fn formatting_round_trips(
            modifiers in proptest::collection::vec(proptest::sample::select(vec![Modifier::Cmd, Modifier::Alt, Modifier::Ctrl, Modifier::Shift]), 0..12),
            key in proptest::sample::select(KEY_NAMES.to_vec()),
            use_alias in any::<bool>(),
        ) {
            let target = KeyName::new(key).unwrap();
            let mapping = KeyMapping { preset: Preset::Qwerty, aliases: vec![("leader".to_owned(), target.clone())] };
            let mut unique = Vec::new();
            for modifier in modifiers {
                if !unique.contains(&modifier) {
                    unique.push(modifier);
                }
            }
            let notation = if use_alias { "leader" } else { key };
            let raw = format_chord(&unique, notation);
            let chord = Chord {
                modifiers: ModifierSet::new(unique.clone()).unwrap(),
                key: ChordKey { notation: notation.to_owned(), target },
            };
            prop_assert_eq!(Chord::parse(&chord.format(ChordStyle::LikeRaw(&raw)), &mapping), Ok(chord.clone()));
            let canonical = chord.format(ChordStyle::Canonical);
            let reparsed = Chord::parse(&canonical, &mapping).unwrap();
            prop_assert_eq!(reparsed.normalized(), chord.normalized());
            unique.sort_unstable();
            let canonical_chord = Chord {
                modifiers: ModifierSet::new(unique).unwrap(),
                key: chord.key,
            };
            prop_assert_eq!(Chord::parse(&canonical_chord.format(ChordStyle::Canonical), &mapping), Ok(canonical_chord));
        }
    }
}
