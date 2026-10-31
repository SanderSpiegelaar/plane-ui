#![expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test helpers outside #[test] fns; a panic is a test failure"
)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use planeui_core::config_doc::{ConfigDoc, EditError, KeyPath};
use proptest::prelude::*;
use similar::TextDiff;
use toml_edit::{Item, Value};

fn fixture(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

type Edit = fn(&mut ConfigDoc) -> Result<(), EditError>;

fn path<const N: usize>(segments: [&str; N]) -> KeyPath {
    KeyPath::from(segments)
}

/// Applies each edit to a fresh copy of `name` and renders every result as a unified diff.
fn diffs(name: &str, edits: &[(&str, Edit)]) -> String {
    let source = fixture(name);
    let mut report = String::new();
    for (label, edit) in edits {
        let mut document = ConfigDoc::parse(&source).unwrap();
        let outcome = edit(&mut document);
        let output = document.to_string();
        writeln!(report, "## {label}").unwrap();
        if let Err(error) = outcome {
            assert_eq!(output, source, "{name}: failed edit changed the document");
            writeln!(report, "error: {error}").unwrap();
        } else {
            ConfigDoc::parse(&output)
                .unwrap_or_else(|error| panic!("{name} / {label}: {error}\n{output}"));
            let diff = TextDiff::from_lines(&source, &output);
            report.push_str(&diff.unified_diff().context_radius(1).to_string());
        }
        report.push('\n');
    }
    report
}

const KEY_EDITS: &[(&str, Edit)] = &[
    ("set gaps.outer.left = 20", |document| {
        document.set_value(&path(["gaps", "outer", "left"]), Value::from(20))
    }),
    ("set new gaps.outer.planeui = 5", |document| {
        document.set_value(&path(["gaps", "outer", "planeui"]), Value::from(5))
    }),
    ("set new root accordion-padding = 12", |document| {
        document.set_value(&path(["accordion-padding"]), Value::from(12))
    }),
    (
        "set new section focus-follows-mouse.enabled = true",
        |document| document.set_value(&path(["focus-follows-mouse", "enabled"]), Value::from(true)),
    ),
    ("set new binding mode.main.binding.alt-j", |document| {
        document.set_value(
            &path(["mode", "main", "binding", "alt-j"]),
            Value::from("focus down"),
        )
    }),
    ("remove gaps.outer.left", |document| {
        document.remove(&path(["gaps", "outer", "left"]))
    }),
    ("remove config-version", |document| {
        document.remove(&path(["config-version"]))
    }),
];

#[test]
fn key_edits_per_style() {
    for name in [
        "table-style.toml",
        "dotted-keys.toml",
        "inline-tables.toml",
        "per-monitor-gaps.toml",
        "heavily-commented.toml",
        "config.crlf.toml",
    ] {
        insta::assert_snapshot!(name, diffs(name, KEY_EDITS));
    }
}

#[test]
fn inline_array_edits() {
    let table_edits: &[(&str, Edit)] = &[
        ("append to esc", |document| {
            document.array_insert(
                &path(["mode", "service", "binding", "esc"]),
                2,
                Value::from("mode alt"),
            )
        }),
        ("insert at front of esc", |document| {
            document.array_insert(
                &path(["mode", "service", "binding", "esc"]),
                0,
                Value::from("mode alt"),
            )
        }),
        ("remove last of esc", |document| {
            document.array_remove(&path(["mode", "service", "binding", "esc"]), 1)
        }),
        ("move esc 1 -> 0", |document| {
            document.array_move(&path(["mode", "service", "binding", "esc"]), 1, 0)
        }),
        ("set esc[0]", |document| {
            document.array_set(
                &path(["mode", "service", "binding", "esc"]),
                0,
                Value::from("reload-config --no-gui"),
            )
        }),
    ];
    insta::assert_snapshot!("arrays-table-style", diffs("table-style.toml", table_edits));
    let multiline_edits: &[(&str, Edit)] = &[
        ("append to after-startup-command", |document| {
            document.array_insert(
                &path(["after-startup-command"]),
                2,
                Value::from("exec-and-forget open -a Raycast"),
            )
        }),
        ("move after-startup-command 1 -> 0", |document| {
            document.array_move(&path(["after-startup-command"]), 1, 0)
        }),
        ("remove first of exec-on-workspace-change", |document| {
            document.array_remove(&path(["exec-on-workspace-change"]), 0)
        }),
        ("append to exec-on-workspace-change", |document| {
            document.array_insert(
                &path(["exec-on-workspace-change"]),
                3,
                Value::from("--verbose"),
            )
        }),
    ];
    insta::assert_snapshot!(
        "arrays-multiline",
        diffs("callbacks-and-exec.toml", multiline_edits)
    );
}

fn rule() -> Value {
    let mut table = toml_edit::InlineTable::new();
    table.insert("if", Value::from("test %{app-bundle-id} = com.apple.Notes"));
    table.insert("run", Value::from("layout floating"));
    Value::InlineTable(table)
}

#[test]
fn rule_array_edits() {
    let rule_edits: &[(&str, Edit)] = &[
        ("append rule", |document| {
            document.array_insert(&path(["on-window-detected"]), 2, rule())
        }),
        ("insert rule first", |document| {
            document.array_insert(&path(["on-window-detected"]), 0, rule())
        }),
        ("remove rule 0", |document| {
            document.array_remove(&path(["on-window-detected"]), 0)
        }),
        ("move rule 1 -> 0", |document| {
            document.array_move(&path(["on-window-detected"]), 1, 0)
        }),
        ("set rule 1", |document| {
            document.array_set(&path(["on-window-detected"]), 1, rule())
        }),
        ("string into rules", |document| {
            document.array_insert(&path(["on-window-detected"]), 0, Value::from("nope"))
        }),
    ];
    for name in [
        "legacy-rules.toml",
        "rules-array-of-tables.toml",
        "rules-inline-array.toml",
    ] {
        insta::assert_snapshot!(format!("rules-{name}"), diffs(name, rule_edits));
    }
}

#[test]
fn edits_keep_crlf_line_endings() {
    let mut document = ConfigDoc::parse(&fixture("config.crlf.toml")).unwrap();
    document
        .set_value(&path(["focus-follows-mouse", "enabled"]), Value::from(true))
        .unwrap();
    let output = document.to_string();
    assert!(output.contains("\r\n[focus-follows-mouse]\r\nenabled = true\r\n"));
    assert!(!output.replace("\r\n", "").contains('\n'));
}

/// Remove + re-add drops only the comment block directly above the key (its own) and keeps
/// every other comment.
#[test]
fn remove_then_re_add_keeps_unrelated_comments() {
    let source = fixture("heavily-commented.toml");
    let comments = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.trim_start().starts_with('#'))
            .map(str::to_owned)
            .collect()
    };
    for (segments, own_comment) in [
        (vec!["accordion-padding"], None),
        (vec!["start-at-login"], None),
        (
            vec!["gaps", "outer", "right"],
            Some("# comment at column zero inside a table"),
        ),
        (vec!["mode", "main", "binding", "alt-slash"], None),
    ] {
        let key = KeyPath::new(segments);
        let mut document = ConfigDoc::parse(&source).unwrap();
        let value = document.get(&key).and_then(Item::as_value).unwrap().clone();
        document.remove(&key).unwrap();
        document.set_value(&key, value).unwrap();
        let output = document.to_string();
        let mut expected = comments(&source);
        expected.retain(|line| Some(line.as_str()) != own_comment);
        assert_eq!(comments(&output), expected, "{key}");
        insta::assert_snapshot!(
            format!("remove-re-add-{key}"),
            TextDiff::from_lines(&source, &output)
                .unified_diff()
                .context_radius(1)
                .to_string()
        );
    }
}

/// Leaf paths of every scalar reachable through tables (not arrays), in file order.
fn scalar_paths(document: &ConfigDoc) -> Vec<KeyPath> {
    fn walk(item: &Item, prefix: &KeyPath, out: &mut Vec<KeyPath>) {
        if let Some(table) = item.as_table_like() {
            for (key, child) in table.iter() {
                walk(child, &prefix.join(key), out);
            }
        } else if item
            .as_value()
            .is_some_and(|value| !value.is_array() && !value.is_inline_table())
        {
            out.push(prefix.clone());
        }
    }
    let mut out = Vec::new();
    walk(
        document.document().as_item(),
        &KeyPath::new(Vec::<String>::new()),
        &mut out,
    );
    out
}

const NEW_KEYS: &[&[&str]] = &[
    &["planeui-root"],
    &["gaps", "outer", "planeui"],
    &["planeui-section", "nested", "value"],
    &["mode", "main", "binding", "planeui-x"],
];

#[derive(Debug, Clone)]
enum Step {
    SetExisting(usize, i64),
    Toggle(usize, i64),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (any::<usize>(), any::<i64>()).prop_map(|(index, value)| Step::SetExisting(index, value)),
        (any::<usize>(), any::<i64>()).prop_map(|(index, value)| Step::Toggle(index, value)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn edit_sequences_reparse_and_invert(
        fixture_index in 0usize..6,
        steps in proptest::collection::vec(step(), 1..12),
    ) {
        let names = [
            "table-style.toml",
            "dotted-keys.toml",
            "heavily-commented.toml",
            "per-monitor-gaps.toml",
            "sander.toml",
            "aerospace-default.toml",
        ];
        let source = fixture(names[fixture_index]);
        let mut document = ConfigDoc::parse(&source).unwrap();
        let existing = scalar_paths(&document);
        prop_assume!(!existing.is_empty());
        let untouched_before: Vec<(String, String)> = top_level(&document);

        let mut inverse: Vec<(KeyPath, Option<Value>)> = Vec::new();
        let mut touched_roots = std::collections::BTreeSet::new();
        for step in steps {
            let (key, value) = match step {
                Step::SetExisting(index, value) => (existing[index % existing.len()].clone(), value),
                Step::Toggle(index, value) => (KeyPath::new(NEW_KEYS[index % NEW_KEYS.len()].iter().copied()), value),
            };
            touched_roots.insert(key.segments()[0].clone());
            let previous = document.get(&key).and_then(Item::as_value).cloned();
            let is_new = matches!(step, Step::Toggle(..));
            if is_new && previous.is_some() {
                document.remove(&key).unwrap();
            } else if document.set_value(&key, Value::from(value)).is_err() {
                continue;
            }
            inverse.push((key, previous));
            let output = document.to_string();
            prop_assert!(ConfigDoc::parse(&output).is_ok(), "does not re-parse:\n{output}");
        }

        for (name, rendered) in untouched_before {
            if !touched_roots.contains(&name) {
                let after = top_level(&document).into_iter().find(|(key, _)| *key == name);
                prop_assert_eq!(after.map(|(_, text)| text), Some(rendered), "{} changed", name);
            }
        }

        for (key, previous) in inverse.into_iter().rev() {
            match previous {
                Some(value) => document.set_value(&key, value).unwrap(),
                None => document.remove(&key).unwrap(),
            }
        }
        prop_assert_eq!(document.to_string(), source);
    }
}

fn top_level(document: &ConfigDoc) -> Vec<(String, String)> {
    document
        .document()
        .as_table()
        .iter()
        .map(|(key, item)| (key.to_owned(), item.to_string()))
        .collect()
}

/// Each `[[a]]` element's id and the values of its nested `[a.x.y]` and `[[a.z]]` tables.
fn rule_groups(text: &str) -> Vec<(i64, Vec<i64>)> {
    let document: toml_edit::DocumentMut = text.parse().unwrap();
    document["a"]
        .as_array_of_tables()
        .unwrap()
        .iter()
        .map(|element| {
            let mut nested = Vec::new();
            if let Some(x) = element.get("x") {
                nested.push(x["y"]["v"].as_integer().unwrap());
            }
            if let Some(z) = element.get("z") {
                for table in z.as_array_of_tables().unwrap() {
                    nested.push(table["v"].as_integer().unwrap());
                }
            }
            (element["id"].as_integer().unwrap(), nested)
        })
        .collect()
}

fn element(id: i64) -> Value {
    let mut table = toml_edit::InlineTable::new();
    table.insert("id", Value::from(id));
    Value::InlineTable(table)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Table-array edits keep each element with its sub-tables, never touch unrelated tables,
    /// and a move followed by its inverse is byte-identical.
    #[test]
    fn table_array_edits_keep_elements_whole(
        shape in proptest::collection::vec((any::<bool>(), 0usize..3, any::<bool>()), 1..5),
        steps in proptest::collection::vec((0u8..4, any::<usize>(), any::<usize>()), 0..20),
    ) {
        let mut source = String::from("root = 42\n");
        let mut model: Vec<(i64, Vec<i64>)> = Vec::new();
        let (mut has_b, mut has_c) = (false, false);
        for (index, (has_x, z_count, interleave)) in shape.iter().enumerate() {
            let id = i64::try_from(index).unwrap();
            let mut nested = Vec::new();
            write!(source, "[[a]]\nid={id}\n").unwrap();
            if *interleave && !has_b {
                source.push_str("[b]\nv=100\n");
                has_b = true;
            }
            if *has_x {
                write!(source, "[a.x.y]\nv={id}\n").unwrap();
                nested.push(id);
            }
            if *interleave && !has_c {
                source.push_str("[c]\nv=200\n");
                has_c = true;
            }
            for z in 0..*z_count {
                write!(source, "[[a.z]]\nv={z}\n").unwrap();
                nested.push(i64::try_from(z).unwrap());
            }
            model.push((id, nested));
        }

        let path = path(["a"]);
        let mut document = ConfigDoc::parse(&source).unwrap();
        document.array_move(&path, 0, 0).unwrap();
        prop_assert_eq!(document.to_string(), source.clone());

        for (step, (kind, first, second)) in steps.into_iter().enumerate() {
            let len = model.len();
            let (from, to) = (first % len, second % len);
            let before = document.to_string();
            document.array_move(&path, from, to).unwrap();
            document.array_move(&path, to, from).unwrap();
            prop_assert_eq!(document.to_string(), before);

            let id = 1000 + i64::try_from(step).unwrap();
            match kind {
                0 => {
                    let index = first % (len + 1);
                    document.array_insert(&path, index, element(id)).unwrap();
                    model.insert(index, (id, Vec::new()));
                }
                1 => {
                    document.array_move(&path, from, to).unwrap();
                    let moved = model.remove(from);
                    model.insert(to, moved);
                }
                2 if len > 1 => {
                    document.array_remove(&path, from).unwrap();
                    model.remove(from);
                }
                2 => {}
                _ => {
                    document.array_set(&path, from, element(id)).unwrap();
                    model[from] = (id, Vec::new());
                }
            }

            let output = document.to_string();
            prop_assert_eq!(rule_groups(&output), model.clone(), "{}", output);
            let parsed: toml_edit::DocumentMut = output.parse().unwrap();
            prop_assert_eq!(parsed["root"].as_integer(), Some(42));
            if has_b {
                prop_assert_eq!(parsed["b"]["v"].as_integer(), Some(100));
            }
            if has_c {
                prop_assert_eq!(parsed["c"]["v"].as_integer(), Some(200));
            }
            if has_b && has_c {
                prop_assert!(output.find("[b]") < output.find("[c]"));
            }
        }
    }
}
