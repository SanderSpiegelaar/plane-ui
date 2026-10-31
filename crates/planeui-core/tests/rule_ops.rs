use planeui_core::commands::{Command, ParsedScript};
use planeui_core::config_doc::ConfigDoc;
use planeui_core::config_view::ConfigView;
use planeui_core::rules::{
    Condition, LegacyMatcher, Matcher, RuleContainer, RuleEditError, add_rule, move_rule,
    remove_rule, update_rule,
};
use planeui_core::validate::{IssueKind, Severity, validate_text};
use proptest::prelude::*;
use similar::TextDiff;

const FIXTURES: &[(&str, &str)] = &[
    (
        "rules-array-of-tables",
        include_str!("fixtures/rules-array-of-tables.toml"),
    ),
    (
        "rules-inline-array",
        include_str!("fixtures/rules-inline-array.toml"),
    ),
    ("legacy-rules", include_str!("fixtures/legacy-rules.toml")),
    ("sander", include_str!("fixtures/sander.toml")),
    (
        "rules-new-tables",
        include_str!("fixtures/rules-new-tables.toml"),
    ),
    (
        "rules-new-inline",
        include_str!("fixtures/rules-new-inline.toml"),
    ),
];

#[test]
fn every_rule_is_classified_in_file_order() {
    for (name, source) in FIXTURES {
        let doc = ConfigDoc::parse(source).unwrap();
        let rules = ConfigView::from_doc(&doc).window_rules;
        assert!(rules.iter().all(|rule| rule.span.is_some()));
        insta::assert_debug_snapshot!(format!("rules_{name}"), (RuleContainer::read(&doc), rules));
    }
}

#[test]
fn each_operation_preserves_the_existing_representation() {
    for (name, source) in FIXTURES.iter().filter(|(name, _)| *name != "sander") {
        for operation in ["add", "update", "remove", "move"] {
            let mut doc = ConfigDoc::parse(source).unwrap();
            match operation {
                "add" => add_rule(
                    &mut doc,
                    &Matcher::Conditions(vec![Condition::AppBundleIdIs("com.foo".into())]),
                    &[Command::parse("layout floating").unwrap()],
                    Some(true),
                )
                .unwrap(),
                "update" => {
                    let mut rule = ConfigView::from_doc(&doc).window_rules.remove(0);
                    match &mut rule.matcher {
                        Matcher::Legacy(legacy) => legacy.app_id = Some("com.foo".into()),
                        matcher => {
                            *matcher = Matcher::Conditions(vec![Condition::AppBundleIdIs(
                                "com.foo".into(),
                            )]);
                        }
                    }
                    update_rule(
                        &mut doc,
                        0,
                        &rule.matcher,
                        &[Command::parse("layout tiling").unwrap()],
                        Some(true),
                    )
                    .unwrap();
                }
                "remove" => remove_rule(&mut doc, 0).unwrap(),
                _ => move_rule(&mut doc, 0, 1).unwrap(),
            }
            let output = doc.to_string();
            assert!(
                !validate_text(&output).unwrap().2.blocks_write(),
                "{name} {operation}\n{output}"
            );
            insta::assert_snapshot!(
                format!("{name}_{operation}"),
                TextDiff::from_lines(*source, &output)
                    .unified_diff()
                    .context_radius(1)
                    .to_string()
            );
        }
    }
}

#[test]
fn creates_table_sections_and_quotes_conditions_losslessly() {
    for value in [
        "com.foo",
        "with space",
        "Sander's app",
        "a \"quote\"",
        "literal && ; ()",
        "",
        "é",
        "a\\b",
    ] {
        let mut doc = ConfigDoc::parse("").unwrap();
        let matcher = Matcher::Conditions(vec![Condition::WindowTitleMatches(value.into())]);
        add_rule(&mut doc, &matcher, &[], None).unwrap();
        assert_eq!(
            RuleContainer::read(&doc),
            Some(RuleContainer::ArrayOfTables)
        );
        assert_eq!(
            ConfigView::from_doc(&doc)
                .window_rules
                .first()
                .unwrap()
                .matcher,
            matcher
        );
    }
    let mut doc = ConfigDoc::parse("").unwrap();
    add_rule(
        &mut doc,
        &Matcher::Conditions(vec![Condition::AppBundleIdIs("com.foo".into())]),
        &[],
        None,
    )
    .unwrap();
    assert_eq!(
        doc.to_string(),
        "[[on-window-detected]]\nif = 'test %{app-bundle-id} = com.foo'\nrun = []\n"
    );
    for value in ["both ' and \" quotes", "%{workspace}"] {
        let before = doc.to_string();
        assert!(matches!(
            add_rule(
                &mut doc,
                &Matcher::Conditions(vec![Condition::WindowTitleMatches(value.into())]),
                &[],
                None
            ),
            Err(RuleEditError::Unrepresentable(_))
        ));
        assert_eq!(doc.to_string(), before);
    }
}

#[test]
fn no_op_updates_preserve_bytes_and_absent_if() {
    for (_, source) in FIXTURES {
        let mut doc = ConfigDoc::parse(source).unwrap();
        for (index, rule) in ConfigView::from_doc(&doc).window_rules.iter().enumerate() {
            if let ParsedScript::Editable { commands, .. } = &rule.run {
                let result = update_rule(
                    &mut doc,
                    index,
                    &rule.matcher,
                    commands,
                    rule.check_further_callbacks,
                );
                assert_eq!(
                    result.is_err(),
                    matches!(rule.matcher, Matcher::ReadOnly { .. })
                );
                assert_eq!(doc.to_string(), *source);
            }
        }
    }
}

#[test]
fn unsupported_rules_can_move_and_delete_but_cannot_be_updated() {
    for raw_if in [
        "'false'",
        "42",
        "{ future = 'keep' }",
        "'test-not %{app-name} ~= app'",
        "'true || false'",
        "'eval true'",
    ] {
        for inline in [false, true] {
            let source = if inline {
                format!(
                    "on-window-detected = [{{ if = {raw_if}, run = [] }}, {{if = 'true', run = []}}]\n"
                )
            } else {
                format!(
                    "[[on-window-detected]]\nif = {raw_if}\nrun = []\n[[on-window-detected]]\nif = 'true'\nrun = []\n"
                )
            };
            let mut doc = ConfigDoc::parse(&source).unwrap();
            assert!(matches!(
                update_rule(&mut doc, 0, &Matcher::Conditions(vec![]), &[], None),
                Err(RuleEditError::ReadOnly { .. })
            ));
            assert_eq!(doc.to_string(), source);
            move_rule(&mut doc, 0, 1).unwrap();
            assert!(doc.to_string().contains(raw_if));
            remove_rule(&mut doc, 1).unwrap();
            assert!(!doc.to_string().contains(raw_if));
        }
    }
    for run in [
        "'future-command --raw'",
        "['layout floating', 'future-command --raw']",
    ] {
        let source = format!("[[on-window-detected]]\nif = 'true'\nrun = {run}\n");
        let mut doc = ConfigDoc::parse(&source).unwrap();
        assert!(matches!(
            update_rule(&mut doc, 0, &Matcher::Conditions(vec![]), &[], None),
            Err(RuleEditError::ReadOnly { .. })
        ));
        assert_eq!(doc.to_string(), source);
    }
}

#[test]
fn legacy_leaf_edits_keep_dotted_and_inline_tables() {
    for source in [
        "[[on-window-detected]]\nif.app-id = 'com.foo' # id\nif.workspace = 'A' # workspace\nrun = ['layout floating'] # run\n",
        "[[on-window-detected]]\nif = { app-id = 'com.foo', workspace = 'A' } # if\nrun = ['layout floating'] # run\n",
        "on-window-detected = [{if.app-id = 'com.foo', if.workspace = 'A', run = ['layout floating']}]\n",
        "on-window-detected = [{if = {app-id = 'com.foo', workspace = 'A'}, run = ['layout floating']}]\n",
    ] {
        let mut doc = ConfigDoc::parse(source).unwrap();
        let mut rule = ConfigView::from_doc(&doc).window_rules.remove(0);
        if let Matcher::Legacy(legacy) = &mut rule.matcher {
            legacy.app_id = Some("com.bar".into());
        }
        update_rule(
            &mut doc,
            0,
            &rule.matcher,
            &[Command::parse("layout floating").unwrap()],
            None,
        )
        .unwrap();
        assert_eq!(doc.to_string(), source.replace("'com.foo'", "\"com.bar\""));
    }
}

fn byte_blocks(doc: &ConfigDoc) -> Result<Vec<String>, &'static str> {
    let item = doc
        .get(&["on-window-detected"].into())
        .ok_or("rules are missing")?;
    let mut blocks: Vec<_> = if let Some(tables) = item.as_array_of_tables() {
        tables
            .iter()
            .map(|table| {
                let mut document = toml_edit::DocumentMut::new();
                let mut array = toml_edit::ArrayOfTables::new();
                array.push(table.clone());
                document
                    .as_table_mut()
                    .insert("on-window-detected", toml_edit::Item::ArrayOfTables(array));
                document.to_string()
            })
            .collect()
    } else {
        item.as_array()
            .ok_or("rules are not an array")?
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    blocks.sort();
    Ok(blocks)
}

#[test]
fn first_rules_leading_comment_moves_with_it() {
    let source = "on-window-detected = [\n    # first\n    { if = 'true', run = [] },\n    # second\n    { if = 'true', run = [] },\n]\n";
    let mut doc = ConfigDoc::parse(source).unwrap();
    move_rule(&mut doc, 0, 1).unwrap();
    assert_eq!(
        doc.to_string(),
        "on-window-detected = [\n    # second\n    { if = 'true', run = [] },\n    # first\n    { if = 'true', run = [] },\n]\n"
    );
    move_rule(&mut doc, 1, 0).unwrap();
    assert_eq!(doc.to_string(), source);
}

#[test]
fn missing_if_stays_absent_when_other_fields_change() {
    for source in [
        "[[on-window-detected]]\ncheck-further-callbacks = true\nrun = ['layout floating'] # keep\n",
        "on-window-detected = [{ check-further-callbacks = true, run = ['layout floating'] }]\n",
    ] {
        let mut doc = ConfigDoc::parse(source).unwrap();
        update_rule(
            &mut doc,
            0,
            &Matcher::Conditions(vec![]),
            &[Command::parse("layout tiling").unwrap()],
            Some(true),
        )
        .unwrap();
        assert_eq!(
            doc.to_string(),
            source.replace("['layout floating']", "[\"layout tiling\"]")
        );
        assert!(!validate_text(&doc.to_string()).unwrap().2.blocks_write());
    }
}

#[test]
fn removing_legacy_conditions_retains_a_legacy_matcher() {
    for source in [
        "[[on-window-detected]]\nif.app-id = 'com.foo'\nrun = []\n",
        "[[on-window-detected]]\nif = { app-id = 'com.foo' }\nrun = []\n",
        "on-window-detected = [{if.app-id = 'com.foo', run = []}]\n",
        "on-window-detected = [{if = { app-id = 'com.foo' }, run = []}]\n",
    ] {
        let mut doc = ConfigDoc::parse(source).unwrap();
        let matcher = Matcher::Legacy(LegacyMatcher::default());
        update_rule(&mut doc, 0, &matcher, &[], None).unwrap();
        let (_, view, report) = validate_text(&doc.to_string()).unwrap();
        assert!(!report.blocks_write(), "{}", doc);
        assert_eq!(view.window_rules.first().unwrap().matcher, matcher);
    }
}

#[test]
fn refused_edits_leave_the_document_unchanged() {
    for source in [
        include_str!("fixtures/rules-new-tables.toml"),
        include_str!("fixtures/rules-new-inline.toml"),
    ] {
        let mut doc = ConfigDoc::parse(source).unwrap();
        assert!(remove_rule(&mut doc, 50).is_err());
        assert!(move_rule(&mut doc, 0, 50).is_err());
        assert!(move_rule(&mut doc, 50, 0).is_err());
        assert!(update_rule(&mut doc, 50, &Matcher::Conditions(vec![]), &[], None).is_err());
        assert!(matches!(
            update_rule(
                &mut doc,
                0,
                &Matcher::Legacy(LegacyMatcher::default()),
                &[],
                None
            ),
            Err(RuleEditError::MatcherStyle(0))
        ));
        assert_eq!(doc.to_string(), source);
        let mixed = source.replacen('\n', "\r\n", 1);
        let mut doc = ConfigDoc::parse(&mixed).unwrap();
        assert!(add_rule(&mut doc, &Matcher::Conditions(vec![]), &[], None).is_err());
        assert!(remove_rule(&mut doc, 0).is_err());
        assert!(move_rule(&mut doc, 0, 1).is_err());
        assert!(update_rule(&mut doc, 0, &Matcher::Conditions(vec![]), &[], None).is_err());
        assert_eq!(doc.to_string(), mixed);
    }
}

#[test]
fn rule_edits_preserve_unrelated_sections_and_crlf_and_support_exact_undo() {
    use planeui_core::history::History;

    for (_, source) in FIXTURES.iter().filter(|(name, _)| *name != "sander") {
        for ending in ["\n", "\r\n"] {
            let untouched = "\n# Keep exactly\n[exec.env-vars]\nMY_VAR  =  'hello' # unknown\n"
                .replace('\n', ending);
            let source = format!("{}{untouched}", source.replace('\n', ending));
            let mut doc = ConfigDoc::parse(&source).unwrap();
            let mut history = History::new();
            add_rule(&mut doc, &Matcher::Conditions(vec![]), &[], None).unwrap();
            let after = doc.to_string();
            assert!(after.ends_with(&untouched), "{after}");
            assert!(ending == "\n" || !after.replace("\r\n", "").contains('\n'));
            history.record(source.clone(), after.clone(), "Add rule");
            assert_eq!(history.undo().unwrap().text, source);
            assert_eq!(history.redo().unwrap().text, after);
        }
    }
    let mut doc = ConfigDoc::parse("on-window-detected = [] # keep\n").unwrap();
    add_rule(&mut doc, &Matcher::Conditions(vec![]), &[], None).unwrap();
    assert_eq!(RuleContainer::read(&doc), Some(RuleContainer::InlineArray));
    remove_rule(&mut doc, 0).unwrap();
    assert_eq!(doc.to_string(), "on-window-detected = [] # keep\n");
}

proptest! {
    #[test]
    fn random_move_sequences_preserve_rule_byte_blocks(moves in prop::collection::vec((any::<usize>(), any::<usize>()), 0..40)) {
        for (_, source) in FIXTURES.iter().filter(|(name, _)| *name != "sander") {
            let mut doc = ConfigDoc::parse(source).unwrap();
            let blocks = byte_blocks(&doc).unwrap();
            let count = blocks.len();
            for &(from, to) in &moves {
                move_rule(&mut doc, from % count, to % count).unwrap();
                prop_assert_eq!(byte_blocks(&doc).unwrap(), blocks.clone());
                let (reparsed, _, report) = validate_text(&doc.to_string()).unwrap();
                prop_assert_eq!(byte_blocks(&reparsed).unwrap(), blocks.clone());
                prop_assert!(!report.blocks_write());
                doc = reparsed;
            }
        }
    }
}

#[test]
fn matcher_uses_the_shared_command_quoting_rules() {
    for (text, expected) in [
        ("true", Some(vec![])),
        (
            "test %{app-bundle-id} = com.foo",
            Some(vec![Condition::AppBundleIdIs("com.foo".into())]),
        ),
        (
            "test %{app-name} ~= 'My app'&&test %{workspace} = W",
            Some(vec![
                Condition::AppNameMatches("My app".into()),
                Condition::WorkspaceIs("W".into()),
            ]),
        ),
        (
            "test %{window-title} ~= \"Sander's dialog\"",
            Some(vec![Condition::WindowTitleMatches(
                "Sander's dialog".into(),
            )]),
        ),
        (
            "test %{window-title} ~= 'literal && || ; ()'",
            Some(vec![Condition::WindowTitleMatches(
                "literal && || ; ()".into(),
            )]),
        ),
        ("false", None),
        ("test-not %{app-name} ~= app", None),
        ("test %{app-name} ~= app || true", None),
        ("test %{app-name} ~= app; true", None),
        ("(test %{app-name} ~= app)", None),
        ("test %{app-name} = app", None),
        ("test %{other} = app", None),
        ("test %{app-name} ~= '%{workspace}'", None),
        ("test %{app-name} ~= app &&", None),
    ] {
        let mut doc = ConfigDoc::parse("on-window-detected = [{run = []}]").unwrap();
        doc.set_value(&["condition"].into(), text.into()).unwrap();
        let value = doc.get(&["condition"].into()).unwrap().to_string();
        let doc = ConfigDoc::parse(&format!(
            "on-window-detected = [{{if = {value}, run = []}}]"
        ))
        .unwrap();
        let rule = ConfigView::from_doc(&doc).window_rules.remove(0);
        match expected {
            Some(conditions) => assert_eq!(rule.matcher, Matcher::Conditions(conditions), "{text}"),
            None => assert!(matches!(rule.matcher, Matcher::ReadOnly { .. }), "{text}"),
        }
    }
}

#[test]
fn rule_validation_has_exact_messages() {
    for (rule, severity, kind, message) in [
        (
            "run = []",
            Severity::Error,
            IssueKind::MissingRuleField,
            "on-window-detected rule requires if unless check-further-callbacks is true",
        ),
        (
            "run = [], check-further-callbacks = false",
            Severity::Error,
            IssueKind::MissingRuleField,
            "on-window-detected rule requires if unless check-further-callbacks is true",
        ),
        (
            "if = 'true'",
            Severity::Error,
            IssueKind::MissingRuleField,
            "on-window-detected rule requires run",
        ),
        (
            "if = 'true', run = 42",
            Severity::Error,
            IssueKind::WrongType,
            "on-window-detected run must be a command or an array of commands",
        ),
        (
            "if = 'true', run = [], check-further-callbacks = 42",
            Severity::Error,
            IssueKind::WrongType,
            "on-window-detected check-further-callbacks must be true or false",
        ),
        (
            "if = 'test %{app-name} ~= \"[\"', run = []",
            Severity::Warning,
            IssueKind::Regex,
            "regex `[` could not be checked locally; AeroSpace's regex engine may accept it",
        ),
        (
            "if = 'test %{window-title} ~= \"[\"', run = []",
            Severity::Warning,
            IssueKind::Regex,
            "regex `[` could not be checked locally; AeroSpace's regex engine may accept it",
        ),
        (
            "if.app-name-regex-substring = '[', run = []",
            Severity::Warning,
            IssueKind::Regex,
            "regex `[` could not be checked locally; AeroSpace's regex engine may accept it",
        ),
        (
            "if.window-title-regex-substring = '[', run = []",
            Severity::Warning,
            IssueKind::Regex,
            "regex `[` could not be checked locally; AeroSpace's regex engine may accept it",
        ),
    ] {
        let (_, _, report) = validate_text(&format!("on-window-detected = [{{{rule}}}]")).unwrap();
        assert_eq!(report.issues.len(), 1, "{rule}");
        let issue = report.issues.first().unwrap();
        assert_eq!(
            (issue.severity, issue.kind, issue.message.as_str()),
            (severity, kind, message)
        );
        assert_eq!(report.blocks_write(), severity == Severity::Error);
        assert!(issue.span.is_some());
    }
    for rule in [
        "run = [], check-further-callbacks = true",
        "if = 'true', run = []",
    ] {
        assert!(
            !validate_text(&format!("on-window-detected = [{{{rule}}}]"))
                .unwrap()
                .2
                .blocks_write()
        );
    }
}
