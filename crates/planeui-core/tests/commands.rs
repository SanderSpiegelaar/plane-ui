use planeui_core::commands::{
    CATALOGUE, Command, Direction, ParsedScript, ResizeAmount, ResizeDimension, Script,
    ScriptShape, WorkspaceName, WorkspaceTarget, format_script,
};
use proptest::prelude::*;
use toml_edit::{DocumentMut, Item, Value};

fn read_only_bindings(source: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let document = source.parse::<DocumentMut>()?;
    let mut read_only = Vec::new();
    let modes = document
        .get("mode")
        .and_then(Item::as_table_like)
        .ok_or("fixture has no modes")?;
    for (mode, item) in modes.iter() {
        let bindings = item
            .get("binding")
            .and_then(Item::as_table_like)
            .ok_or("fixture mode has no bindings")?;
        for (chord, script) in bindings.iter() {
            match Script::parse(script) {
                ParsedScript::Editable { commands, shape } => {
                    assert_eq!(
                        Script::parse(&Item::Value(format_script(&commands, shape))),
                        ParsedScript::Editable { commands, shape }
                    );
                }
                ParsedScript::ReadOnly { reason } => {
                    assert_ne!(reason, "");
                    read_only.push(format!("{mode}.{chord}: {reason}"));
                }
            }
        }
    }
    assert_eq!(document.to_string(), source);
    Ok(read_only)
}

#[test]
fn fixture_bindings_are_classified_without_mutation() {
    assert_eq!(
        read_only_bindings(include_str!("fixtures/aerospace-default.toml")).unwrap(),
        Vec::<String>::new()
    );
    insta::assert_debug_snapshot!(
        "sander_read_only_bindings",
        read_only_bindings(include_str!("fixtures/sander.toml")).unwrap()
    );
}

#[test]
fn unsupported_scripts_explain_why() {
    for (text, reason) in [
        ("focus left; close", "uses the shell operator `;`"),
        ("focus left && close", "uses the shell operator `&&`"),
        ("focus left || close", "uses the shell operator `||`"),
        ("focus left | close", "uses the shell operator `|`"),
        ("(focus left)", "uses the shell operator `(`"),
        ("focus left)", "uses the shell operator `)`"),
        ("foo", "unknown command `foo`"),
        ("focus --bar left", "unknown flag `--bar` for `focus`"),
    ] {
        assert_eq!(
            Script::parse(&Item::Value(Value::from(text))),
            ParsedScript::ReadOnly {
                reason: reason.to_owned()
            }
        );
    }
    for text in [
        "value = 42",
        "value = ['focus left', 42]",
        "value = { command = 'close' }",
    ] {
        let document = text.parse::<DocumentMut>().unwrap();
        assert!(
            matches!(Script::parse(document.get("value").unwrap()), ParsedScript::ReadOnly { reason } if !reason.is_empty())
        );
    }
}

#[test]
fn exec_suffix_is_verbatim_and_not_shell_parsed() {
    let suffix = "  echo 'hi'; true && (false || echo done) | cat\n";
    let text = format!("exec-and-forget{suffix}");
    assert_eq!(
        Command::parse(&text).unwrap(),
        Command::ExecAndForget(suffix.to_owned())
    );
    assert_eq!(Command::parse(&text).unwrap().format(), text);
}

#[test]
fn every_catalogue_default_round_trips() {
    assert_eq!(CATALOGUE.len(), 19);
    for spec in CATALOGUE {
        let command = spec.default_command().unwrap();
        assert_eq!(command.name(), spec.name);
        assert_eq!(Command::parse(&command.format()).unwrap(), command);
    }
}

#[test]
fn canonical_format_quotes_names_and_disambiguates_monitor_patterns() {
    for (source, expected) in [
        ("workspace -- A", "workspace A"),
        ("workspace 'project$1'", "workspace 'project$1'"),
        ("mode -- main", "mode main"),
        ("focus-monitor -- next", "focus-monitor -- next"),
        (
            "focus-monitor -- 'Dell.*' main",
            "focus-monitor 'Dell.*' main",
        ),
    ] {
        assert_eq!(Command::parse(source).unwrap().format(), expected);
    }
}

#[test]
fn documented_flags_and_targets_round_trip() {
    for text in [
        "workspace --auto-back-and-forth A",
        "workspace --wrap-around --no-stdin next",
        "workspace --stdin prev",
        "workspace --fail-if-noop B",
        "move-node-to-workspace --window-id 42 --focus-follows-window --fail-if-noop A",
        "move-node-to-workspace --wrap-around --stdin next",
        "focus --boundaries all-monitors-outer-frame --boundaries-action wrap-around-all-monitors --ignore-floating --fail-if-fullscreen --fail-if-macos-native-fullscreen right",
        "focus --wrap-around dfs-next",
        "focus --dfs-index 0",
        "focus --window-id 42",
        "move --window-id 42 --boundaries all-monitors-outer-frame --boundaries-action fail --fail-if-fullscreen --fail-if-macos-native-fullscreen down",
        "resize --window-id 42 smart-opposite -0",
        "resize width 100",
        "layout --workspace A --root --fail-if-noop h_tiles",
        "layout --window-id 42 floating tiling",
        "fullscreen --window-id 42 --no-outer-gaps --fail-if-noop on",
        "fullscreen off --fail-if-noop",
        "join-with --window-id 42 up",
        "flatten-workspace-tree --workspace A",
        "balance-sizes --workspace A",
        "close --window-id 42 --quit-if-last-window",
        "close-all-windows-but-current --quit-if-last-window",
        "focus-monitor --wrap-around left",
        "focus-monitor -- 'Dell.*' main",
        "focus-monitor -- '^Built-in.*Display$'",
        "focus-monitor -- 'Dell\\.Display'",
        "focus-monitor -- next",
        "move-node-to-monitor --window-id 42 --focus-follows-window --fail-if-noop -- main secondary",
        "move-node-to-monitor --wrap-around next",
        "move-workspace-to-monitor --workspace A --wrap-around prev",
        "mode -- '-some mode'",
        "reload-config --no-gui --dry-run --warnings-as-errors",
        "workspace 'project$1'",
        "workspace 'a\"b'",
        "workspace \"a'b\"",
    ] {
        let result = Command::parse(text);
        assert!(result.is_ok(), "{text}: {result:?}");
        let command = result.unwrap();
        assert_eq!(
            Command::parse(&command.format()).unwrap(),
            command,
            "{text}"
        );
    }
}

#[test]
fn invalid_argument_combinations_stay_read_only() {
    for text in [
        "workspace",
        "workspace 'has spaces'",
        "workspace _reserved",
        "workspace focused",
        "workspace --wrap-around A",
        "workspace --auto-back-and-forth next",
        "workspace --fail-if-noop --auto-back-and-forth A",
        "workspace --stdin A",
        "workspace --stdin --no-stdin next",
        "move-node-to-workspace --window-id 1 next",
        "focus sideways",
        "focus --boundaries universe left",
        "focus --wrap-around --boundaries workspace left",
        "focus --boundaries-action wrap-around-all-monitors left",
        "focus --fail-if-fullscreen dfs-next",
        "focus --boundaries all-monitors-outer-frame dfs-prev",
        "focus --window-id 42 left",
        "focus --window-id 42 --dfs-index 1",
        "focus --dfs-index -1",
        "focus --window-id 4294967296",
        "move --boundaries-action wrap-around-the-workspace left",
        "resize smart 1.5",
        "resize smart +-50",
        "resize smart --50",
        "layout",
        "layout sticky",
        "layout --workspace A tiles",
        "layout --root floating",
        "layout --root --workspace A --window-id 42 tiles",
        "layout --fail-if-noop tiles accordion",
        "fullscreen toggle",
        "fullscreen off --no-outer-gaps",
        "fullscreen --fail-if-noop",
        "close extra",
        "close --quit-if-last-window --quit-if-last-window",
        "mode",
        "focus-monitor --wrap-around main",
        "focus-monitor 0",
        "focus-monitor -- '[unclosed'",
        "focus-monitor -- '*bad'",
        "focus-monitor -- 'unclosed\\'",
        "focus-monitor next main",
        "move-node-to-monitor --fail-if-noop next",
        "focus --window-id",
        "reload-config --new-flag",
        "workspace '$bad\"quote\'combination'",
        "workspace unquoted$var",
        "mode 'unclosed",
        "mode 'word'joined",
        "focus left\nclose",
    ] {
        assert!(Command::parse(text).is_err(), "{text}");
    }
}

#[test]
fn script_shape_is_retained_or_promoted() {
    let command = Command::parse("close").unwrap();
    assert!(format_script(std::slice::from_ref(&command), ScriptShape::String).is_str());
    assert!(format_script(std::slice::from_ref(&command), ScriptShape::Array).is_array());
    assert!(format_script(&[command.clone(), command], ScriptShape::String).is_array());
    assert!(format_script(&[], ScriptShape::String).is_array());
}

#[test]
fn complex_monitor_regex_stays_read_only_without_an_icu_engine() {
    assert_eq!(
        Script::parse(&Item::Value(Value::from("focus-monitor -- '(Dell|LG)'"))),
        ParsedScript::ReadOnly {
            reason:
                "monitor pattern uses regex groups, classes, or repetition not modelled by the form"
                    .to_owned()
        }
    );
}

fn editable_command() -> impl Strategy<Value = Command> {
    prop_oneof![
        (
            "[A-Za-z][A-Za-z0-9$;|()]{0,12}",
            any::<bool>(),
            any::<bool>()
        )
            .prop_filter_map(
                "valid workspace name",
                |(name, auto_back_and_forth, fail_if_noop)| WorkspaceName::new(name).ok().map(
                    |name| Command::Workspace {
                        target: WorkspaceTarget::Name(name),
                        auto_back_and_forth,
                        fail_if_noop: fail_if_noop && !auto_back_and_forth,
                    }
                )
            ),
        (any::<u64>(), 0_u8..3, prop::option::of(any::<u32>())).prop_map(
            |(amount, operation, window_id)| Command::Resize {
                dimension: ResizeDimension::Smart,
                amount: match operation {
                    0 => ResizeAmount::Set(amount),
                    1 => ResizeAmount::Add(amount),
                    _ => ResizeAmount::Subtract(amount),
                },
                window_id,
            }
        ),
        Just(Command::JoinWith {
            direction: Direction::Left,
            window_id: None
        }),
        prop::sample::select(
            CATALOGUE
                .iter()
                .filter_map(|spec| spec.default_command().ok())
                .collect::<Vec<_>>()
        ),
        "[ -~]{0,80}".prop_map(|script| Command::ExecAndForget(format!(" {script}"))),
    ]
}

proptest! {
    #[test]
    fn editable_lists_round_trip(commands in prop::collection::vec(editable_command(), 0..12), array in any::<bool>()) {
        let shape = if array || commands.len() != 1 { ScriptShape::Array } else { ScriptShape::String };
        let formatted = Item::Value(format_script(&commands, shape));
        prop_assert_eq!(Script::parse(&formatted), ParsedScript::Editable { commands, shape });
    }
}
