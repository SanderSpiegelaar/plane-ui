use std::num::NonZeroU32;

use planeui_core::aerospace::Monitor;
use planeui_core::config_doc::ConfigDoc;
use planeui_core::config_view::ConfigView;
use planeui_core::monitors::{
    MonitorPattern, PatternShape, add_assignment, add_pattern, move_pattern, remove_assignment,
    remove_pattern, resolve, set_pattern,
};
use planeui_core::validate::{Severity, validate_text};
use similar::TextDiff;
use toml_edit::Value;

#[test]
fn pattern_grammar_and_resolution_match_recorded_monitors() {
    let index = |value| MonitorPattern::Index(NonZeroU32::new(value).unwrap());
    let mut monitors: Vec<Monitor> =
        serde_json::from_str(include_str!("fixtures/cli/list-monitors/stdout")).unwrap();
    monitors.reverse();
    for (raw, pattern, resolved) in [
        ("1", index(1), Some(1)),
        ("'2'", index(2), Some(2)),
        ("'main'", MonitorPattern::Main, Some(2)),
        ("'secondary'", MonitorPattern::Secondary, None),
        (
            "'ultragear'",
            MonitorPattern::Regex("ultragear".into()),
            Some(1),
        ),
        ("'RETINA'", MonitorPattern::Regex("RETINA".into()), Some(2)),
        (
            "'\\(2\\)'",
            MonitorPattern::Regex("\\(2\\)".into()),
            Some(3),
        ),
        ("9", index(9), None),
        ("'missing'", MonitorPattern::Regex("missing".into()), None),
        ("'['", MonitorPattern::Regex("[".into()), None),
    ] {
        assert_eq!(
            MonitorPattern::parse(&raw.parse::<Value>().unwrap()).unwrap(),
            pattern
        );
        assert_eq!(
            resolve(&[pattern], &monitors).map(|monitor| monitor.id),
            resolved,
            "{raw}"
        );
    }
    for raw in ["0", "-1", "'0'", "'-1'", "''", "true", "[]", "{}"] {
        assert!(
            MonitorPattern::parse(&raw.parse::<Value>().unwrap()).is_err(),
            "{raw}"
        );
        assert_eq!(
            validate_text(&format!(
                "workspace-to-monitor-force-assignment.A = {raw}\n"
            ))
            .unwrap()
            .2
            .blocks_write(),
            raw != "[]",
            "{raw}"
        );
    }
    assert_eq!(
        resolve(&[index(9), MonitorPattern::Main, index(1)], &monitors)
            .unwrap()
            .id,
        2
    );
    monitors.retain(|monitor| monitor.id != 3);
    assert_eq!(
        resolve(&[MonitorPattern::Secondary], &monitors).unwrap().id,
        1
    );
    assert!(resolve(&[MonitorPattern::Main], &[]).is_none());
}

#[test]
fn invalid_regex_is_only_a_warning_and_bad_rows_remain_visible() {
    let (_, view, report) =
        validate_text("[workspace-to-monitor-force-assignment]\nA = '['\nB = 0\nC = 'main'\n")
            .unwrap();
    assert_eq!(view.workspace_to_monitor.len(), 3);
    assert!(view.workspace_to_monitor.get(1).unwrap().error.is_some());
    assert!(
        view.workspace_to_monitor
            .iter()
            .all(|assignment| assignment.span.is_some())
    );
    assert_eq!(
        report
            .issues
            .iter()
            .filter(|issue| issue.severity == Severity::Warning)
            .count(),
        1
    );
    assert_eq!(report.errors().count(), 1);
}

#[test]
fn edits_preserve_each_style() {
    let index = |value| MonitorPattern::Index(NonZeroU32::new(value).unwrap());
    for (style, source) in [
        (
            "table",
            "# keep\n[workspace-to-monitor-force-assignment]\nA = '1' # alpha\nB = ['main', 'secondary', 'lg'] # beta\n",
        ),
        (
            "dotted",
            "# keep\nworkspace-to-monitor-force-assignment.A = '1' # alpha\nworkspace-to-monitor-force-assignment.B = ['main', 'secondary', 'lg'] # beta\n",
        ),
        (
            "inline",
            "# keep\nworkspace-to-monitor-force-assignment = { A = '1', B = ['main', 'secondary', 'lg'] } # keep\n",
        ),
    ] {
        for operation in [
            "add_assignment",
            "remove_assignment",
            "add_scalar_pattern",
            "add_list_pattern",
            "remove_pattern",
            "move_pattern",
            "set_scalar_pattern",
            "set_list_pattern",
        ] {
            let mut doc = ConfigDoc::parse(source).unwrap();
            match operation {
                "add_assignment" => add_assignment(&mut doc, "C", &index(2)).unwrap(),
                "remove_assignment" => remove_assignment(&mut doc, "A").unwrap(),
                "add_scalar_pattern" => add_pattern(&mut doc, "A", &MonitorPattern::Main).unwrap(),
                "add_list_pattern" => add_pattern(&mut doc, "B", &index(2)).unwrap(),
                "remove_pattern" => remove_pattern(&mut doc, "B", 1).unwrap(),
                "move_pattern" => move_pattern(&mut doc, "B", 0, 2).unwrap(),
                "set_scalar_pattern" => set_pattern(&mut doc, "A", 0, &index(2)).unwrap(),
                _ => {
                    set_pattern(&mut doc, "B", 1, &MonitorPattern::Regex("retina".into())).unwrap();
                }
            }
            assert!(!validate_text(&doc.to_string()).unwrap().2.blocks_write());
            insta::assert_snapshot!(
                format!("{style}_{operation}"),
                TextDiff::from_lines(source, doc.to_string())
                    .unified_diff()
                    .context_radius(1)
                    .to_string()
            );
        }
    }
}

#[test]
fn representation_and_failed_edits_are_stable() {
    let index = |value| MonitorPattern::Index(NonZeroU32::new(value).unwrap());
    let source = "workspace-to-monitor-force-assignment.A = '1' # keep\n";
    let mut doc = ConfigDoc::parse(source).unwrap();
    set_pattern(&mut doc, "A", 0, &index(1)).unwrap();
    assert_eq!(doc.to_string(), source);
    add_pattern(&mut doc, "A", &MonitorPattern::Main).unwrap();
    remove_pattern(&mut doc, "A", 1).unwrap();
    assert_eq!(
        ConfigView::from_doc(&doc)
            .workspace_to_monitor
            .first()
            .unwrap()
            .shape,
        PatternShape::List
    );
    for operation in [0, 1, 2, 3] {
        let before = doc.to_string();
        let result = match operation {
            0 => set_pattern(&mut doc, "A", 5, &MonitorPattern::Main),
            1 => add_assignment(&mut doc, "A", &MonitorPattern::Main),
            2 => add_pattern(&mut doc, "A", &MonitorPattern::Regex("main".into())),
            _ => move_pattern(&mut doc, "A", 0, 5),
        };
        assert!(result.is_err());
        assert_eq!(doc.to_string(), before);
    }
}
