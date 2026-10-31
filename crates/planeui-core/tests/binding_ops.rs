#![expect(
    clippy::unwrap_used,
    reason = "test helpers panic when setup or an edit fails"
)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use planeui_core::bindings::{BindingEditError, duplicate_binding, remove_binding, upsert_binding};
use planeui_core::commands::{Command, ParsedScript, Script, ScriptShape};
use planeui_core::config_doc::{ConfigDoc, KeyPath};
use planeui_core::keys::{Chord, KeyMapping};
use similar::TextDiff;

fn chord(raw: &str) -> Chord {
    Chord::parse(raw, &KeyMapping::default()).unwrap()
}

fn commands() -> Vec<Command> {
    vec![Command::parse("focus down").unwrap()]
}

fn round_trip(doc: &ConfigDoc) {
    let output = doc.to_string();
    assert_eq!(ConfigDoc::parse(&output).unwrap().to_string(), output);
}

type Edit = fn(&mut ConfigDoc) -> Result<(), BindingEditError>;

#[test]
fn binding_diffs_per_style() {
    let edits: &[(&str, Edit)] = &[
        ("edit commands", |doc| {
            upsert_binding(doc, "main", Some("alt-h"), &chord("alt-h"), &commands())
        }),
        ("rename chord", |doc| {
            upsert_binding(doc, "main", Some("alt-h"), &chord("alt-j"), &commands())
        }),
        ("add binding", |doc| {
            upsert_binding(doc, "main", None, &chord("shift-alt-j"), &commands())
        }),
        ("remove binding", |doc| remove_binding(doc, "main", "alt-h")),
        ("duplicate to existing bindings", |doc| {
            duplicate_binding(doc, "main", "alt-h", "service")
        }),
        ("duplicate to mode without bindings", |doc| {
            duplicate_binding(doc, "main", "alt-h", "empty")
        }),
        ("add to mode without bindings", |doc| {
            upsert_binding(doc, "empty", None, &chord("alt-j"), &commands())
        }),
    ];
    for name in ["table-style", "dotted-keys", "inline-tables"] {
        let mut source = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(format!("{name}.toml")),
        )
        .unwrap();
        match name {
            "dotted-keys" => source.push_str("mode.empty.test = true\n"),
            "inline-tables" => source
                .push_str("\n[mode.service]\nbinding = { esc = 'mode main' }\n\n[mode.empty]\n"),
            _ => source.push_str("\n[mode.empty]\n"),
        }
        let mut report = String::new();
        for (label, edit) in edits {
            let mut doc = ConfigDoc::parse(&source).unwrap();
            edit(&mut doc).unwrap();
            round_trip(&doc);
            writeln!(report, "## {label}").unwrap();
            report.push_str(
                &TextDiff::from_lines(&source, doc.to_string())
                    .unified_diff()
                    .context_radius(1)
                    .to_string(),
            );
            report.push('\n');
        }
        insta::assert_snapshot!(name, report);
    }
}

#[test]
fn rename_keeps_position_comments_spacing_and_modifier_order() {
    for (source, expected) in [
        (
            "[mode.main.binding]\n# heading\n'shift-alt-h'  =  'focus left' # left\nalt-l = 'focus right'\n",
            "[mode.main.binding]\n# heading\nshift-alt-j  =  \"focus down\" # left\nalt-l = 'focus right'\n",
        ),
        (
            "mode.main.binding.shift-alt-h  =  'focus left' # left\nmode.main.binding.alt-l = 'focus right'\n",
            "mode.main.binding.shift-alt-j  =  \"focus down\" # left\nmode.main.binding.alt-l = 'focus right'\n",
        ),
        (
            "mode = { main = { binding = { shift-alt-h  =  'focus left', alt-l = 'focus right' } } }\n",
            "mode = { main = { binding = { shift-alt-j  =  \"focus down\", alt-l = 'focus right' } } }\n",
        ),
    ] {
        let mut doc = ConfigDoc::parse(source).unwrap();
        upsert_binding(
            &mut doc,
            "main",
            Some("shift-alt-h"),
            &chord("alt-shift-j"),
            &commands(),
        )
        .unwrap();
        assert_eq!(doc.to_string(), expected);
        round_trip(&doc);
    }
}

#[test]
fn script_shape_is_retained_or_promoted() {
    for (raw, count, expected) in [
        ("'focus left'", 1, ScriptShape::String),
        ("'focus left'", 2, ScriptShape::Array),
        ("['focus left']", 1, ScriptShape::Array),
        ("['focus left', 'focus right']", 1, ScriptShape::Array),
    ] {
        let mut doc =
            ConfigDoc::parse(&format!("[mode.main.binding]\nalt-h = {raw} # keep\n")).unwrap();
        let commands = vec![Command::parse("focus down").unwrap(); count];
        upsert_binding(&mut doc, "main", Some("alt-h"), &chord("alt-h"), &commands).unwrap();
        assert_eq!(
            Script::parse(
                doc.get(&KeyPath::from(["mode", "main", "binding", "alt-h"]))
                    .unwrap()
            ),
            ParsedScript::Editable {
                commands,
                shape: expected
            }
        );
        assert!(doc.to_string().ends_with(" # keep\n"));
        round_trip(&doc);
    }
}

#[test]
fn readonly_edits_and_collisions_leave_the_document_unchanged() {
    let sources = [
        "[mode.main.binding]\nalt-h = 'focus left && focus right'\n",
        "[mode.main.binding]\nalt-h = 'focus left'\nalt-l.command = 'focus right'\n",
        "mode.main.binding = false\n",
    ];
    for source in sources {
        let mut doc = ConfigDoc::parse(source).unwrap();
        assert!(matches!(
            upsert_binding(
                &mut doc,
                "main",
                Some("alt-h"),
                &chord("alt-j"),
                &commands()
            ),
            Err(BindingEditError::ReadOnly { .. })
        ));
        assert_eq!(doc.to_string(), source);
    }
    let source = "[mode.main.binding]\nalt-h = 'focus left'\nalt-l = 'focus right'\n";
    let edits: &[Edit] = &[
        |doc| upsert_binding(doc, "main", None, &chord("alt-h"), &commands()),
        |doc| upsert_binding(doc, "main", Some("alt-h"), &chord("alt-l"), &commands()),
        |doc| {
            upsert_binding(
                doc,
                "main",
                Some("alt-missing"),
                &chord("alt-j"),
                &commands(),
            )
        },
        |doc| duplicate_binding(doc, "main", "alt-h", "main"),
        |doc| duplicate_binding(doc, "main", "missing", "main"),
        |doc| remove_binding(doc, "main", "missing"),
    ];
    for edit in edits {
        let mut doc = ConfigDoc::parse(source).unwrap();
        assert!(edit(&mut doc).is_err());
        assert_eq!(doc.to_string(), source);
    }
}

#[test]
fn readonly_bindings_can_be_duplicated_verbatim_and_removed() {
    let source = "[mode.main.binding]\n# own\nalt-h =  ['unknown-command', 'focus left && focus right'] # raw\n\n[mode.service]\n";
    let mut doc = ConfigDoc::parse(source).unwrap();
    let from = KeyPath::from(["mode", "main", "binding", "alt-h"]);
    let raw = doc.get(&from).unwrap().to_string();
    duplicate_binding(&mut doc, "main", "alt-h", "service").unwrap();
    assert_eq!(
        doc.get(&KeyPath::from(["mode", "service", "binding", "alt-h"]))
            .unwrap()
            .to_string(),
        raw
    );
    remove_binding(&mut doc, "main", "alt-h").unwrap();
    assert!(!doc.to_string().contains("# own"));
    assert!(doc.to_string().contains("# raw"));
    round_trip(&doc);
}

#[test]
fn mixed_line_endings_refuse_all_operations() {
    let source = "[mode.main.binding]\r\nalt-h = 'focus left'\n[mode.service]\n";
    let edits: &[Edit] = &[
        |doc| upsert_binding(doc, "main", Some("alt-h"), &chord("alt-j"), &commands()),
        |doc| remove_binding(doc, "main", "alt-h"),
        |doc| duplicate_binding(doc, "main", "alt-h", "service"),
    ];
    for edit in edits {
        let mut doc = ConfigDoc::parse(source).unwrap();
        assert!(edit(&mut doc).is_err());
        assert_eq!(doc.to_string(), source);
    }
}

#[test]
fn unsafe_raw_comment_copy_to_inline_table_is_refused() {
    let source = "[mode.service]\nbinding = { esc = 'mode main' }\n[mode.main.binding]\nalt-h = 'unknown-command' # raw\n";
    let mut doc = ConfigDoc::parse(source).unwrap();
    assert!(matches!(
        duplicate_binding(&mut doc, "main", "alt-h", "service"),
        Err(BindingEditError::IncompatibleStyle(_))
    ));
    assert_eq!(doc.to_string(), source);
}

#[test]
fn renaming_preserves_quoted_siblings_and_crlf() {
    for source in [
        "[mode.main.binding]\r\n'alt-h' = 'focus left'\r\n\"alt-l\"  =  'focus right' # untouched\r\n",
        "mode = { main = { binding = { 'alt-h' = 'focus left', \"alt-l\"  =  'focus right' } } }\r\n",
    ] {
        let mut doc = ConfigDoc::parse(source).unwrap();
        upsert_binding(
            &mut doc,
            "main",
            Some("alt-h"),
            &chord("alt-j"),
            &commands(),
        )
        .unwrap();
        let expected = source
            .replace("'alt-h'", "alt-j")
            .replace("'focus left'", "\"focus down\"");
        assert_eq!(doc.to_string(), expected);
        round_trip(&doc);
    }
}

#[test]
fn only_literal_collisions_are_rejected() {
    let source = "[mode.main.binding]\nshift-alt-h = 'focus left'\n[mode.service.binding]\nalt-shift-h = 'focus right'\n";
    let mut doc = ConfigDoc::parse(source).unwrap();
    duplicate_binding(&mut doc, "main", "shift-alt-h", "service").unwrap();
    upsert_binding(&mut doc, "main", None, &chord("alt-shift-h"), &commands()).unwrap();
    for mode in ["main", "service"] {
        for raw in ["shift-alt-h", "alt-shift-h"] {
            assert!(
                doc.get(&KeyPath::from(["mode", mode, "binding", raw]))
                    .is_some()
            );
        }
    }
    round_trip(&doc);
}
