use planeui_core::config_doc::ConfigDoc;
use planeui_core::config_view::ConfigView;
use planeui_core::rules::{Condition, Matcher, RuleContainer};
use planeui_core::validate::{IssueKind, Severity, validate_text};

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
        // TOML quoting is independent of the command tokenizer.
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
