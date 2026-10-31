//! Incomplete form values stay separate from the config until the commit gate succeeds.

use planeui_core::commands::{ArgKind, ArgSpec, Command, CommandSpec, command_spec};
use planeui_core::config_view::ConfigView;
use planeui_core::keys::{Chord, Modifier};
use planeui_core::validate::check_chord_available;

#[derive(Debug, Clone)]
pub(crate) struct ArgumentDraft {
    pub spec: ArgSpec,
    pub values: Vec<String>,
    pub enabled: bool,
}

impl ArgumentDraft {
    pub fn stepped_value(&self, index: usize, delta: i8) -> Option<String> {
        let value = self.values.get(index)?;
        let number = value.parse::<i128>().ok()?.checked_add(i128::from(delta))?;
        let kind = match self.spec.kind {
            ArgKind::Flag(flag) => flag.value.copied()?,
            kind => kind,
        };
        if matches!(kind, ArgKind::SignedAmount) && value.starts_with(['+', '-']) {
            Some(format!("{number:+}"))
        } else {
            Some(number.max(0).to_string())
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CommandDraft {
    pub spec: &'static CommandSpec,
    pub arguments: Vec<ArgumentDraft>,
    literal: bool,
}

impl CommandDraft {
    pub fn new(command: &Command) -> Result<Self, DraftError> {
        let spec = command_spec(command.name()).ok_or_else(|| {
            DraftError(format!("Missing command metadata for {}", command.name()))
        })?;
        let mut arguments: Vec<_> = spec
            .args
            .iter()
            .map(|spec| ArgumentDraft {
                spec: *spec,
                values: Vec::new(),
                enabled: false,
            })
            .collect();
        if let Command::ExecAndForget(suffix) = command {
            if let Some(argument) = arguments.first_mut() {
                argument.values.push(suffix.clone());
                argument.enabled = true;
            }
            return Ok(Self {
                spec,
                arguments,
                literal: false,
            });
        }
        let formatted = command.format();
        let tokens = tokens(&formatted);
        let mut tokens = tokens.into_iter().skip(1);
        let mut positional = Vec::new();
        let mut literal = false;
        while let Some(token) = tokens.next() {
            if token == "--" && !literal {
                literal = true;
            } else if !literal && token.starts_with("--") {
                if let Some(argument) = arguments
                    .iter_mut()
                    .find(|argument| argument.spec.name == token)
                {
                    argument.enabled = true;
                    if matches!(argument.spec.kind, ArgKind::Flag(flag) if flag.value.is_some()) {
                        argument.values.extend(tokens.next());
                    }
                }
            } else {
                positional.push(token);
            }
        }
        let mut positional = positional.into_iter();
        for argument in &mut arguments {
            if !matches!(argument.spec.kind, ArgKind::Flag(_)) {
                argument.enabled = true;
                if matches!(
                    argument.spec.kind,
                    ArgKind::LayoutOptions | ArgKind::MonitorTarget
                ) {
                    argument.values.extend(positional.by_ref());
                } else {
                    argument.values.extend(positional.next());
                }
            }
        }
        Ok(Self {
            spec,
            arguments,
            literal,
        })
    }

    pub fn to_command(&self) -> Result<Command, DraftError> {
        if self.spec.name == "exec-and-forget" {
            let suffix = self
                .arguments
                .first()
                .and_then(|argument| argument.values.first())
                .cloned()
                .unwrap_or_default();
            if suffix.trim().is_empty() {
                return Err(DraftError("Enter a shell command".into()));
            }
            return Ok(Command::ExecAndForget(
                if suffix.starts_with(char::is_whitespace) {
                    suffix
                } else {
                    format!(" {suffix}")
                },
            ));
        }
        if self.arguments.iter().any(|argument| {
            (!matches!(argument.spec.kind, ArgKind::Flag(_)) || argument.enabled)
                && argument
                    .values
                    .iter()
                    .any(|value| value.contains('\'') && value.contains('"'))
        }) {
            return Err(DraftError(
                "An argument cannot contain both quote styles".into(),
            ));
        }
        let mut output = vec![self.spec.name.to_owned()];
        for argument in &self.arguments {
            if let ArgKind::Flag(flag) = argument.spec.kind
                && argument.enabled
            {
                output.push(flag.name.to_owned());
                if flag.value.is_some() {
                    output.extend(argument.values.iter().map(|value| quote(value)));
                }
            }
        }
        if self.literal {
            output.push("--".into());
        }
        for argument in &self.arguments {
            if !matches!(argument.spec.kind, ArgKind::Flag(_)) {
                output.extend(argument.values.iter().map(|value| quote(value)));
            }
        }
        Command::parse(&output.join(" ")).map_err(|error| DraftError(error.to_string()))
    }
}

fn quote(value: &str) -> String {
    if value.contains('\'') {
        format!("\"{value}\"")
    } else {
        format!("'{value}'")
    }
}

fn tokens(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character.is_whitespace() {
            continue;
        }
        let mut value = String::new();
        if matches!(character, '\'' | '"') {
            for next in chars.by_ref() {
                if next == character {
                    break;
                }
                value.push(next);
            }
        } else {
            value.push(character);
            while chars.peek().is_some_and(|next| !next.is_whitespace()) {
                if let Some(next) = chars.next() {
                    value.push(next);
                }
            }
        }
        values.push(value);
    }
    values
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BindingDraft {
    pub original_raw_key: Option<String>,
    pub modifiers: Vec<Modifier>,
    pub key: Option<String>,
    pub commands: Vec<CommandDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DraftError(pub String);

impl BindingDraft {
    pub fn chord(&self, view: &ConfigView, mode: &str) -> Result<Chord, DraftError> {
        let key = self
            .key
            .as_ref()
            .ok_or_else(|| DraftError("Choose a key".into()))?;
        let raw = self
            .modifiers
            .iter()
            .map(ToString::to_string)
            .chain(std::iter::once(key.clone()))
            .collect::<Vec<_>>()
            .join("-");
        let chord =
            Chord::parse(&raw, &view.key_mapping).map_err(|error| DraftError(error.to_string()))?;
        check_chord_available(view, mode, &chord, self.original_raw_key.as_deref()).map_err(
            |conflict| {
                let commands = match &conflict.binding.script {
                    planeui_core::commands::ParsedScript::Editable { commands, .. } => commands
                        .iter()
                        .map(Command::format)
                        .collect::<Vec<_>>()
                        .join(", "),
                    planeui_core::commands::ParsedScript::ReadOnly { .. } => {
                        "read-only script".into()
                    }
                };
                DraftError(format!(
                    "{} is already bound to `{commands}` in {}",
                    conflict.binding.raw_key, conflict.mode
                ))
            },
        )?;
        Ok(chord)
    }

    pub fn to_commit(
        &self,
        view: &ConfigView,
        mode: &str,
    ) -> Result<(Chord, Vec<Command>), DraftError> {
        if !view
            .modes
            .iter()
            .any(|entry| entry.name.value == mode && entry.editable)
        {
            return Err(DraftError(format!("Mode {mode} cannot be edited")));
        }
        let chord = self.chord(view, mode)?;
        if self.commands.is_empty() {
            return Err(DraftError("Add a command".into()));
        }
        let commands = self
            .commands
            .iter()
            .map(CommandDraft::to_command)
            .collect::<Result<Vec<_>, _>>()?;
        for command in &commands {
            if let Command::Mode(name) = command
                && !view
                    .modes
                    .iter()
                    .any(|mode| mode.name.value == name.as_str())
            {
                return Err(DraftError(format!("Unknown mode {}", name.as_str())));
            }
        }
        Ok((chord, commands))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use planeui_core::commands::{CATALOGUE, ParsedScript};
    use planeui_core::config_doc::ConfigDoc;

    #[test]
    fn every_catalogue_default_round_trips_through_its_argument_fields() {
        for spec in CATALOGUE {
            let command = spec.default_command().unwrap();
            assert_eq!(
                CommandDraft::new(&command).unwrap().to_command().unwrap(),
                command,
                "{}",
                spec.name
            );
        }
    }

    #[test]
    fn stepping_preserves_resize_operations_across_zero() {
        for (value, delta, expected) in [
            ("+50", 1, "+51"),
            ("-50", 1, "-49"),
            ("+0", -1, "-1"),
            ("-0", 1, "+1"),
            ("0", -1, "0"),
            ("50", 1, "51"),
        ] {
            let draft =
                CommandDraft::new(&Command::parse(&format!("resize smart {value}")).unwrap())
                    .unwrap();
            let amount = draft
                .arguments
                .iter()
                .find(|argument| argument.spec.name == "Amount")
                .unwrap();
            assert_eq!(amount.stepped_value(0, delta).as_deref(), Some(expected));
        }
    }

    #[test]
    fn existing_typed_commands_keep_flags_quotes_order_and_shell_suffixes() {
        for text in [
            "focus --boundaries all-monitors-outer-frame --boundaries-action wrap-around-all-monitors right",
            "focus --window-id 17",
            "focus --dfs-index 2",
            "workspace --wrap-around --no-stdin next",
            "resize --window-id 13 smart-opposite -0",
            "layout --root --workspace P h_tiles accordion",
            "focus-monitor -- 'next' 'Built-in Retina'",
            "exec-and-forget  echo \"a'b\" && echo done",
            "mode 'a\"b'",
            "fullscreen --no-outer-gaps on",
        ] {
            let command = Command::parse(text).unwrap();
            assert_eq!(
                CommandDraft::new(&command).unwrap().to_command().unwrap(),
                command,
                "{text}"
            );
        }
    }

    #[test]
    fn editable_fixture_commands_round_trip() {
        for text in [
            include_str!("../../planeui-core/tests/fixtures/aerospace-default.toml"),
            include_str!("../../planeui-core/tests/fixtures/sander.toml"),
        ] {
            let doc = ConfigDoc::parse(text).unwrap();
            let view = ConfigView::from_doc(&doc);
            for mode in view.modes {
                for binding in mode.bindings {
                    if let ParsedScript::Editable { commands, .. } = binding.script {
                        for command in commands {
                            assert_eq!(
                                CommandDraft::new(&command).unwrap().to_command().unwrap(),
                                command
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn disabled_flag_values_do_not_block_the_command() {
        let command = Command::parse("close").unwrap();
        let mut draft = CommandDraft::new(&command).unwrap();
        draft
            .arguments
            .iter_mut()
            .find(|argument| argument.spec.name == "--window-id")
            .unwrap()
            .values = vec!["'\"".into()];
        assert_eq!(draft.to_command().unwrap(), command);
    }

    #[test]
    fn incomplete_conflicting_and_unknown_mode_drafts_cannot_commit() {
        let doc = ConfigDoc::parse("[mode.main.binding]\nalt-h = 'focus left'\n").unwrap();
        let view = ConfigView::from_doc(&doc);
        let mut draft = BindingDraft::default();
        assert!(draft.to_commit(&view, "main").is_err());
        draft.key = Some("h".into());
        draft.modifiers.push(Modifier::Alt);
        assert_eq!(
            draft.chord(&view, "main").unwrap_err().0,
            "alt-h is already bound to `focus left` in main"
        );
        draft.key = Some("p".into());
        assert!(draft.to_commit(&view, "main").is_err());
        draft
            .commands
            .push(CommandDraft::new(&Command::parse("mode absent").unwrap()).unwrap());
        assert_eq!(
            draft.to_commit(&view, "main").unwrap_err().0,
            "Unknown mode absent"
        );
        draft.original_raw_key = Some("alt-h".into());
        draft.key = Some("h".into());
        assert!(draft.chord(&view, "main").is_ok());
    }
}
