use planeui_core::config_doc::ConfigDoc;
use planeui_core::gaps::{
    GapField, GapValue, PerMonitor, add_gap_rule, move_gap_rule, remove_gap_rule, set_gap,
    set_gap_default, set_gap_rule, switch_to_constant, switch_to_per_monitor,
};
use planeui_core::history::History;
use planeui_core::monitors::MonitorPattern;
use planeui_core::validate::validate_text;
use proptest::prelude::*;
use similar::TextDiff;

#[test]
fn exact_shape_is_required_and_unsupported_values_are_read_only() {
    for raw in [
        "[]",
        "[8]",
        "[{ monitor.main = 2 }]",
        "[8, { monitor.main = 2 }]",
        "[{ monitor.main = -1 }, 8]",
        "[{ monitor.main = 1 }, -1]",
        "[{ monitor.main = 1, unknown = 2 }, 8]",
        "[{ monitor = { main = 1, secondary = 2 } }, 8]",
        "[{ monitor.'' = 1 }, 8]",
        "[{ monitor.0 = 1 }, 8]",
        "[{}, 8]",
        "[true, 8]",
    ] {
        let source = format!("gaps.outer.top = {raw}\n");
        let (mut doc, _, report) = validate_text(&source).unwrap();
        assert!(report.blocks_write(), "{raw}");
        assert!(matches!(
            GapValue::read(&doc, GapField::OuterTop),
            GapValue::Invalid { .. }
        ));
        assert!(switch_to_constant(&mut doc, GapField::OuterTop).is_err());
        assert!(switch_to_per_monitor(&mut doc, GapField::OuterTop).is_err());
        assert!(set_gap(&mut doc, GapField::OuterTop, 4).is_err());
        assert_eq!(doc.to_string(), source);
    }
}

#[test]
fn fixture_parses_without_changing_a_byte() {
    let source = include_str!("fixtures/per-monitor-gaps.toml");
    let (doc, _, report) = validate_text(source).unwrap();
    assert!(!report.blocks_write());
    assert_eq!(
        GapValue::read(&doc, GapField::InnerHorizontal),
        GapValue::PerMonitor(PerMonitor {
            rules: vec![
                (MonitorPattern::Main, 16),
                (MonitorPattern::Regex("some-pattern".into()), 32)
            ],
            default: 24,
        })
    );
    assert_eq!(doc.to_string(), source);
    let (_, _, report) = validate_text("gaps.outer.top = [{ monitor.'[' = 1 }, 8]\n").unwrap();
    assert!(!report.blocks_write());
    assert_eq!(report.issues.len(), 1);
}

#[test]
fn every_edit_is_lossless_and_one_undo_entry() {
    for (style, source) in [
        ("grouped", include_str!("fixtures/per-monitor-gaps.toml")),
        (
            "dotted",
            "# keep\ngaps.outer.top = [{ monitor.main = 1 }, { monitor.'lg' = 2 }, 8] # gap\ngaps.outer.bottom = 4\n",
        ),
        (
            "inline",
            "# keep\ngaps = { outer = { top = [{ monitor.main = 1 }, { monitor.'lg' = 2 }, 8], bottom = 4 } } # gap\n",
        ),
    ] {
        for operation in [
            "to_constant",
            "add",
            "remove",
            "move",
            "set_pattern",
            "set_value",
            "default",
            "to_per_monitor",
        ] {
            let mut doc = ConfigDoc::parse(source).unwrap();
            let field = GapField::OuterTop;
            match operation {
                "to_constant" => switch_to_constant(&mut doc, field).unwrap(),
                "add" => add_gap_rule(&mut doc, field, &MonitorPattern::Regex("retina".into()), 6)
                    .unwrap(),
                "remove" => remove_gap_rule(&mut doc, field, 0).unwrap(),
                "move" => move_gap_rule(&mut doc, field, 0, 1).unwrap(),
                "set_pattern" => set_gap_rule(
                    &mut doc,
                    field,
                    0,
                    &if style == "grouped" {
                        MonitorPattern::Main
                    } else {
                        MonitorPattern::Secondary
                    },
                    if style == "grouped" { 4 } else { 1 },
                )
                .unwrap(),
                "set_value" => {
                    let GapValue::PerMonitor(value) = GapValue::read(&doc, field) else {
                        unreachable!()
                    };
                    set_gap_rule(&mut doc, field, 0, &value.rules.first().unwrap().0, 99).unwrap();
                }
                "default" => set_gap_default(&mut doc, field, 20).unwrap(),
                _ => switch_to_per_monitor(&mut doc, GapField::OuterBottom).unwrap(),
            }
            let output = doc.to_string();
            assert!(!validate_text(&output).unwrap().2.blocks_write());
            let mut history = History::new();
            history.record(source.into(), output.clone(), operation);
            assert_eq!(history.undo().unwrap().text, source);
            assert!(history.undo().is_none());
            insta::assert_snapshot!(
                format!("{style}_{operation}"),
                TextDiff::from_lines(source, &output)
                    .unified_diff()
                    .context_radius(1)
                    .to_string()
            );
        }
    }
}

#[test]
fn rule_edits_keep_original_key_spelling_and_comments() {
    let source = "gaps.outer.top = [\n    { monitor = { '002' = 12 } }, # second\n    { monitor.main = 4 }, # main\n    8, # fallback\n]\n";
    let mut doc = ConfigDoc::parse(source).unwrap();
    let pattern = MonitorPattern::parse_str("2").unwrap();
    set_gap_rule(&mut doc, GapField::OuterTop, 0, &pattern, 99).unwrap();
    assert_eq!(doc.to_string(), source.replace("12", "99"));
    move_gap_rule(&mut doc, GapField::OuterTop, 0, 1).unwrap();
    assert_eq!(
        doc.to_string(),
        "gaps.outer.top = [\n    { monitor.main = 4 }, # main\n    { monitor = { '002' = 99 } }, # second\n    8, # fallback\n]\n"
    );
}

#[test]
fn invalid_edits_are_atomic_and_default_cannot_be_moved_as_a_rule() {
    let source = "gaps.outer.top = [{ monitor.main = 16 }, 24]\n";
    let mut doc = ConfigDoc::parse(source).unwrap();
    for operation in [0, 1, 2, 3] {
        let result = match operation {
            0 => remove_gap_rule(&mut doc, GapField::OuterTop, 0),
            1 => move_gap_rule(&mut doc, GapField::OuterTop, 0, 1),
            2 => set_gap_rule(&mut doc, GapField::OuterTop, 1, &MonitorPattern::Main, 4),
            _ => add_gap_rule(
                &mut doc,
                GapField::OuterTop,
                &MonitorPattern::Regex(String::new()),
                4,
            ),
        };
        assert!(result.is_err());
        assert_eq!(doc.to_string(), source);
    }
}

proptest! {
    #[test]
    fn conversions_and_rule_edits_validate(value in any::<u32>(), edits in proptest::collection::vec(any::<u32>(), 1..20)) {
        let source = format!("# untouched\ngaps.outer.top = {value}\n[unknown]\nkeep = 'verbatim' # keep\n");
        let mut doc = ConfigDoc::parse(&source).unwrap();
        switch_to_per_monitor(&mut doc, GapField::OuterTop).unwrap();
        for value in edits {
            set_gap_default(&mut doc, GapField::OuterTop, value).unwrap();
            add_gap_rule(&mut doc, GapField::OuterTop, &MonitorPattern::Secondary, value).unwrap();
            move_gap_rule(&mut doc, GapField::OuterTop, 1, 0).unwrap();
            remove_gap_rule(&mut doc, GapField::OuterTop, 0).unwrap();
            let (_, _, report) = validate_text(&doc.to_string()).unwrap();
            prop_assert!(report.errors().all(|issue| issue.key_path.as_ref().is_some_and(|path| path.to_string() == "unknown")));
            prop_assert!(doc.to_string().ends_with("[unknown]\nkeep = 'verbatim' # keep\n"));
        }
        set_gap_default(&mut doc, GapField::OuterTop, value).unwrap();
        switch_to_constant(&mut doc, GapField::OuterTop).unwrap();
        prop_assert_eq!(doc.to_string(), source);
    }
}
