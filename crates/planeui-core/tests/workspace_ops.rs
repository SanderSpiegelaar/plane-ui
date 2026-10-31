use planeui_core::config_doc::ConfigDoc;
use planeui_core::config_view::ConfigView;
use planeui_core::settings::{
    add_persistent_workspace, inferred_persistent_workspaces, move_persistent_workspace,
    remove_persistent_workspace, upgrade_config_version,
};
use planeui_core::validate::validate_text;
use similar::TextDiff;

#[test]
fn trailing_comments_follow_their_workspace() {
    let mut doc = ConfigDoc::parse(
        "config-version = 2\npersistent-workspaces = [\n    'A', # alpha\n    'B', # beta\n]\n",
    )
    .unwrap();
    move_persistent_workspace(&mut doc, 0, 1).unwrap();
    assert_eq!(
        doc.to_string(),
        "config-version = 2\npersistent-workspaces = [\n    'B', # beta\n    'A', # alpha\n]\n"
    );
}

#[test]
fn inference_matches_aerospace_sources() {
    let source = "[mode.other.binding]\na = ['move-node-to-workspace Z', 'workspace A', 'workspace next', 'exec-and-forget echo hi']\nb = 'workspace B; workspace A && move-node-to-workspace C'\n[mode.main.binding]\nc = 'workspace --wrap-around prev'\nd = 'workspace D'\n[workspace-to-monitor-force-assignment]\nA = 'main'\nE = 'secondary'\n";
    let doc = ConfigDoc::parse(source).unwrap();
    assert_eq!(
        inferred_persistent_workspaces(&ConfigView::from_doc(&doc)),
        ["A", "Z", "B", "C", "D", "E"]
    );
}

#[test]
fn upgrade_prefill_is_one_atomic_change() {
    let source = "[mode.main.binding]\na = 'workspace A'\n";
    let mut doc = ConfigDoc::parse(source).unwrap();
    let names = inferred_persistent_workspaces(&ConfigView::from_doc(&doc));
    upgrade_config_version(&mut doc, Some(names)).unwrap();
    assert!(!validate_text(&doc.to_string()).unwrap().2.blocks_write());
    insta::assert_snapshot!(
        "upgrade",
        TextDiff::from_lines(source, doc.to_string())
            .unified_diff()
            .context_radius(1)
            .to_string()
    );
    let before = doc.to_string();
    assert!(upgrade_config_version(&mut doc, Some(vec!["A".into(), "A".into()])).is_err());
    assert_eq!(doc.to_string(), before);
}

#[test]
fn list_edits_preserve_style_and_comments() {
    for (style, source) in [
        (
            "compact",
            "config-version = 2\npersistent-workspaces = ['A', 'B'] # keep\n",
        ),
        (
            "multiline",
            "config-version = 2\npersistent-workspaces = [\n    'A', # alpha\n    'B', # beta\n]\n",
        ),
    ] {
        for operation in ["add", "remove", "move"] {
            let mut doc = ConfigDoc::parse(source).unwrap();
            match operation {
                "add" => add_persistent_workspace(&mut doc, "C").unwrap(),
                "remove" => remove_persistent_workspace(&mut doc, 0).unwrap(),
                _ => move_persistent_workspace(&mut doc, 0, 1).unwrap(),
            }
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
fn versions_and_duplicates_are_validated() {
    for source in [
        "config-version = 0",
        "config-version = 3",
        "persistent-workspaces = []",
        "config-version = 1\npersistent-workspaces = ['A']",
        "config-version = 2\npersistent-workspaces = ['A', 'A']",
    ] {
        assert!(validate_text(source).unwrap().2.blocks_write(), "{source}");
    }
    let mut doc = ConfigDoc::parse("").unwrap();
    assert!(add_persistent_workspace(&mut doc, "A").is_err());
    assert_eq!(doc.to_string(), "");
    upgrade_config_version(&mut doc, None).unwrap();
    add_persistent_workspace(&mut doc, "A").unwrap();
    let before = doc.to_string();
    assert!(add_persistent_workspace(&mut doc, "A").is_err());
    assert_eq!(doc.to_string(), before);
}
