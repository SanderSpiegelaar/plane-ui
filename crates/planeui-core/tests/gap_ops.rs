use planeui_core::config_doc::ConfigDoc;
use planeui_core::config_view::ConfigView;
use planeui_core::gaps::{GapEditError, GapField, GapValue, reset_gap, set_gap, set_outer_linked};
use planeui_core::validate::{IssueKind, validate_text};
use proptest::prelude::*;
use similar::TextDiff;

#[test]
fn edits_preserve_each_style() {
    for (name, source) in [
        (
            "grouped",
            "[gaps]\ninner.horizontal = 10 # inner\nouter.left = 8 # left\nouter.right = 8\n",
        ),
        ("tables", include_str!("fixtures/table-style.toml")),
        ("dotted", include_str!("fixtures/dotted-keys.toml")),
        ("inline", include_str!("fixtures/inline-tables.toml")),
    ] {
        for reset in [false, true] {
            let mut doc = ConfigDoc::parse(source).unwrap();
            if reset {
                reset_gap(&mut doc, GapField::OuterLeft).unwrap();
            } else {
                set_gap(&mut doc, GapField::OuterLeft, 12).unwrap();
            }
            let output = doc.to_string();
            let diff = TextDiff::from_lines(source, &output)
                .unified_diff()
                .context_radius(1)
                .to_string();
            insta::assert_snapshot!(
                format!("{name}_{}", if reset { "reset" } else { "set" }),
                diff
            );
            assert!(!validate_text(&output).unwrap().2.blocks_write());
        }
    }
}

#[test]
fn monitor_arrays_never_change() {
    let source = include_str!("fixtures/per-monitor-gaps.toml");
    let mut doc = ConfigDoc::parse(source).unwrap();
    for field in GapField::ALL {
        let original = GapValue::read(&doc, field);
        if matches!(original, GapValue::PerMonitor(_)) {
            assert_eq!(
                set_gap(&mut doc, field, 12),
                Err(GapEditError::PerMonitor(field))
            );
            assert_eq!(
                reset_gap(&mut doc, field),
                Err(GapEditError::PerMonitor(field))
            );
        } else {
            set_gap(&mut doc, field, 12).unwrap();
            reset_gap(&mut doc, field).unwrap();
        }
        for monitor in [
            GapField::InnerHorizontal,
            GapField::OuterLeft,
            GapField::OuterTop,
        ] {
            assert_eq!(
                GapValue::read(&doc, monitor),
                GapValue::read(&ConfigDoc::parse(source).unwrap(), monitor)
            );
        }
    }
    let before = doc.to_string();
    assert!(set_outer_linked(&mut doc, 12).is_err());
    assert_eq!(doc.to_string(), before);
}

#[test]
fn invalid_gap_types_block_writes() {
    for raw in ["-1", "1.5", "true", "'wide'", "{}", "4294967296"] {
        let (_, _, report) = validate_text(&format!("gaps.outer.left = {raw}\n")).unwrap();
        let error = report.errors().next().unwrap();
        assert_eq!(error.kind, IssueKind::WrongType);
        assert_eq!(
            error.message,
            "gaps.outer.left must be an integer of 0 or more or a per-monitor array"
        );
    }
}

proptest! {
    #[test]
    fn linked_values_round_trip(value in any::<u32>()) {
        let mut doc = ConfigDoc::parse("# untouched\nunknown = 'keep'\n").unwrap();
        set_outer_linked(&mut doc, value).unwrap();
        let parsed = ConfigDoc::parse(&doc.to_string()).unwrap();
        let view = ConfigView::from_doc(&parsed);
        for field in GapField::OUTER {
            prop_assert_eq!(view.gaps.get(field), &GapValue::Const(value));
        }
        for field in GapField::OUTER {
            reset_gap(&mut doc, field).unwrap();
        }
        prop_assert_eq!(doc.to_string(), "# untouched\nunknown = 'keep'\n");
    }
}
