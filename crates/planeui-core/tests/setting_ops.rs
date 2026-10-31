use planeui_core::config_doc::{ConfigDoc, KeyPath};
use planeui_core::config_view::ConfigView;
use planeui_core::settings::{Setting, SettingValue, reset_setting, set_setting};
use planeui_core::validate::{IssueKind, validate_text};
use similar::TextDiff;

#[test]
fn defaults_match_vendored_config() {
    let doc = ConfigDoc::parse(include_str!("fixtures/aerospace-default.toml")).unwrap();
    let view = ConfigView::from_doc(&doc);
    for setting in Setting::ALL {
        assert_eq!(
            view.general.setting(setting).value(),
            Some(&setting.spec().default)
        );
    }
}

#[test]
fn every_setting_sets_and_resets_losslessly() {
    for setting in Setting::ALL {
        let source = "# keep\nunknown = 'untouched'\n\n[mode.main.binding]\nalt-h = 'focus left'\n";
        let mut doc = ConfigDoc::parse(source).unwrap();
        set_setting(&mut doc, setting, setting.spec().default).unwrap();
        let output = doc.to_string();
        insta::assert_snapshot!(
            format!("{setting:?}_set"),
            TextDiff::from_lines(source, &output)
                .unified_diff()
                .context_radius(1)
                .to_string()
        );
        assert_eq!(
            ConfigView::from_doc(&doc).general.setting(setting).value(),
            Some(&setting.spec().default)
        );
        reset_setting(&mut doc, setting).unwrap();
        assert_eq!(doc.to_string(), source);
    }
}

#[test]
fn wrong_kind_is_atomic() {
    let mut doc = ConfigDoc::parse("start-at-login = false\n").unwrap();
    let before = doc.to_string();
    assert!(set_setting(&mut doc, Setting::StartAtLogin, SettingValue::Count(12)).is_err());
    assert_eq!(doc.to_string(), before);
}

#[test]
fn wrong_types_and_values_block_writes() {
    for setting in Setting::ALL {
        let key = KeyPath::new(setting.spec().path.iter().copied());
        let (_, _, report) = validate_text(&format!("{key} = []\n")).unwrap();
        assert_eq!(report.errors().next().unwrap().kind, IssueKind::WrongType);
    }
    for text in [
        "accordion-padding = -1",
        "default-root-container-layout = 'grid'",
        "default-root-container-orientation = 'diagonal'",
        "key-mapping.preset = 'azerty'",
    ] {
        let (_, _, report) = validate_text(text).unwrap();
        let error = report.errors().next().unwrap();
        assert_eq!(error.kind, IssueKind::InvalidValue);
        assert!(error.message.ends_with("has an invalid value"));
    }
}
