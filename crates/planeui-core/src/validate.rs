//! Syntax, schema, semantic, and lint checks for a `ConfigDoc`. One entry point decides
//! whether a document may be written; later milestones add rules to [`RULES`].

use std::ops::Range;

use crate::commands::{Command, ParsedScript};
use crate::config_doc::{ConfigDoc, KeyPath, ParseError};
use crate::config_view::{Binding, ConfigView, Mode, Scope};
use crate::keys::Chord;

/// Whether an issue blocks writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Shown, but the file may still be written.
    Warning,
    /// The file must not be written while this exists.
    Error,
}

/// What kind of problem an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// The text is not valid TOML (L0).
    Syntax,
    /// `on-window-detected` written both as `[[tables]]` and as an array (L0).
    MixedRuleStyles,
    /// A key AeroSpace 0.21 does not know (L1).
    UnknownKey,
    /// A key only older AeroSpace versions accept (L1).
    DeprecatedKey,
    /// A known key with a value of the wrong type (L1).
    WrongType,
    /// A typed scalar has an unsupported value.
    InvalidValue,
    /// A binding key cannot be parsed as an AeroSpace chord.
    InvalidChord,
    /// Two keys in one mode resolve to the same chord.
    DuplicateChord,
    /// A command switches to a mode that is not defined.
    UnknownMode,
    /// An explicit mode table does not define `main`.
    MissingMainMode,
    /// A non-main mode has no binding that switches back to main.
    NoWayBack,
    /// A binding intercepts a common macOS shortcut.
    MacOsShortcut,
}

/// One problem found in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// The key the issue is about, when there is one.
    pub key_path: Option<KeyPath>,
    /// Byte range in the document text, when known.
    pub span: Option<Range<usize>>,
    /// Whether it blocks writing.
    pub severity: Severity,
    /// The class of problem.
    pub kind: IssueKind,
    /// Plain-language description for the UI.
    pub message: String,
}

/// Every issue found in one document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    /// Issues in rule order.
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// Whether any issue is an error.
    #[must_use]
    pub fn blocks_write(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == Severity::Error)
    }

    /// Only the errors.
    pub fn errors(&self) -> impl Iterator<Item = &ValidationIssue> {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
    }
}

/// A check over a parsed document. Later milestones append rules here (chords, commands,
/// patterns) without touching the pipeline.
pub type Rule = fn(&ConfigDoc, &ConfigView, &mut Vec<ValidationIssue>);

/// The rules [`validate`] runs, in order.
pub const RULES: &[Rule] = &[
    schema,
    gap_values,
    setting_values,
    invalid_chords,
    duplicate_chords,
    mode_references,
    main_mode,
    no_way_back,
    macos_shortcuts,
];

fn gap_values(_: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    for field in crate::gaps::GapField::ALL {
        if matches!(view.gaps.get(field), crate::gaps::GapValue::Invalid { .. }) {
            let path = field.path();
            issues.push(ValidationIssue {
                message: format!("{path} must be an integer of 0 or more or a per-monitor array"),
                key_path: Some(path),
                span: None,
                severity: Severity::Error,
                kind: IssueKind::WrongType,
            });
        }
    }
}

fn setting_values(document: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    for setting in crate::settings::Setting::ALL {
        let path = KeyPath::new(setting.spec().path.iter().copied());
        if let Some(item) = document.get(&path)
            && crate::config_view::key_spec(&path)
                .and_then(|spec| spec.value_type)
                .is_some_and(|kind| kind.matches(item))
            && let crate::config_view::Field::Invalid { span, .. } = view.general.setting(setting)
        {
            issues.push(ValidationIssue {
                message: format!("{path} has an invalid value"),
                key_path: Some(path),
                span: span.clone(),
                severity: Severity::Error,
                kind: IssueKind::InvalidValue,
            });
        }
    }
}

/// The first binding that already uses a proposed chord in a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordConflict<'a> {
    /// The mode containing the existing binding.
    pub mode: &'a str,
    /// The existing binding, including its raw key and parsed script.
    pub binding: &'a Binding,
}

/// Checks a proposed chord, excluding the original key when editing a binding.
/// A missing mode has no conflicting bindings.
///
/// # Errors
///
/// Returns the first binding with the same normalized chord in `mode`.
pub fn check_chord_available<'a>(
    view: &'a ConfigView,
    mode: &str,
    chord: &Chord,
    ignoring_raw_key: Option<&str>,
) -> Result<(), ChordConflict<'a>> {
    let Some(mode) = view.modes.iter().find(|entry| entry.name.value == mode) else {
        return Ok(());
    };
    match conflicting_binding(mode.bindings.iter(), chord, ignoring_raw_key) {
        Some(binding) => Err(ChordConflict {
            mode: &mode.name.value,
            binding,
        }),
        None => Ok(()),
    }
}

fn conflicting_binding<'a>(
    mut bindings: impl Iterator<Item = &'a Binding>,
    chord: &Chord,
    ignoring_raw_key: Option<&str>,
) -> Option<&'a Binding> {
    let normalized = chord.normalized();
    bindings.find(|binding| {
        Some(binding.raw_key.as_str()) != ignoring_raw_key
            && binding
                .chord
                .as_ref()
                .is_ok_and(|existing| existing.normalized() == normalized)
    })
}

fn binding_path(mode: &Mode, binding: &Binding) -> KeyPath {
    KeyPath::from([
        "mode",
        mode.name.value.as_str(),
        "binding",
        binding.raw_key.as_str(),
    ])
}

fn binding_issue(
    mode: &Mode,
    binding: &Binding,
    severity: Severity,
    kind: IssueKind,
    message: String,
) -> ValidationIssue {
    ValidationIssue {
        key_path: Some(binding_path(mode, binding)),
        span: binding.span.clone(),
        severity,
        kind,
        message,
    }
}

fn invalid_chords(_: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    for mode in &view.modes {
        for binding in &mode.bindings {
            if let Err(error) = &binding.chord {
                issues.push(binding_issue(
                    mode,
                    binding,
                    Severity::Error,
                    IssueKind::InvalidChord,
                    format!("{}: {error}", binding.raw_key),
                ));
            }
        }
    }
}

fn duplicate_chords(document: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    for mode in &view.modes {
        for (index, binding) in mode.bindings.iter().enumerate() {
            let Ok(chord) = &binding.chord else {
                continue;
            };
            if let Some(existing) =
                conflicting_binding(mode.bindings.iter().take(index), chord, None)
            {
                let commands = match &existing.script {
                    ParsedScript::Editable { commands, .. } => commands
                        .iter()
                        .map(Command::format)
                        .collect::<Vec<_>>()
                        .join("; "),
                    ParsedScript::ReadOnly { .. } => document
                        .get(&binding_path(mode, existing))
                        .map(|item| item.to_string().trim().to_owned())
                        .unwrap_or_default(),
                };
                issues.push(binding_issue(
                    mode,
                    binding,
                    Severity::Error,
                    IssueKind::DuplicateChord,
                    format!(
                        "{} is already bound to `{commands}` in {}",
                        existing.raw_key, mode.name.value
                    ),
                ));
            }
        }
    }
}

fn mode_references(_: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    for mode in &view.modes {
        for binding in &mode.bindings {
            let ParsedScript::Editable { commands, .. } = &binding.script else {
                continue;
            };
            for command in commands {
                if let Command::Mode(target) = command
                    && !view
                        .modes
                        .iter()
                        .any(|mode| mode.name.value == target.as_str())
                {
                    issues.push(binding_issue(
                        mode,
                        binding,
                        Severity::Error,
                        IssueKind::UnknownMode,
                        format!("mode {} refers to an unknown mode", target.as_str()),
                    ));
                }
            }
        }
    }
}

fn main_mode(document: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    // AeroSpace 0.21 checks for main only inside parseModes, not for an absent mode table.
    if document
        .get(&KeyPath::from(["mode"]))
        .is_some_and(toml_edit::Item::is_table_like)
        && !view.modes.iter().any(|mode| mode.name.value == "main")
    {
        issues.push(ValidationIssue {
            key_path: Some(KeyPath::from(["mode", "main"])),
            span: view
                .keys
                .iter()
                .find(|key| key.key.value == KeyPath::from(["mode"]))
                .and_then(|key| key.key.span.clone()),
            severity: Severity::Error,
            kind: IssueKind::MissingMainMode,
            message: "mode.main is required".to_owned(),
        });
    }
}

const MACOS_SHORTCUTS: &[&str] = &["cmd-tab", "cmd-space", "cmd-h", "cmd-q"];

fn no_way_back(document: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    for mode in &view.modes {
        if mode.name.value != "main"
            && !mode.bindings.iter().any(|binding| {
                crate::bindings::binding_switches_to(document, &mode.name.value, binding, "main")
            })
        {
            issues.push(ValidationIssue {
                key_path: Some(KeyPath::from(["mode", mode.name.value.as_str()])),
                span: mode.name.span.clone(),
                severity: Severity::Warning,
                kind: IssueKind::NoWayBack,
                message: format!("mode {} has no binding back to main", mode.name.value),
            });
        }
    }
}

fn macos_shortcuts(_: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    for mode in &view.modes {
        for binding in &mode.bindings {
            if let Ok(chord) = &binding.chord {
                let normalized = chord.normalized().to_string();
                if MACOS_SHORTCUTS.contains(&normalized.as_str()) {
                    issues.push(binding_issue(
                        mode,
                        binding,
                        Severity::Warning,
                        IssueKind::MacOsShortcut,
                        format!(
                            "{} shadows the macOS shortcut {normalized}",
                            binding.raw_key
                        ),
                    ));
                }
            }
        }
    }
}

/// Runs every rule over a parsed document.
#[must_use]
pub fn validate(document: &ConfigDoc, view: &ConfigView) -> ValidationReport {
    let mut issues = Vec::new();
    for rule in RULES {
        rule(document, view, &mut issues);
    }
    ValidationReport { issues }
}

/// Parses `text` and validates it; a syntax error is reported as the only issue.
///
/// # Errors
///
/// Returns the report with the syntax error when `text` is not valid TOML.
pub fn validate_text(
    text: &str,
) -> Result<(ConfigDoc, ConfigView, ValidationReport), ValidationReport> {
    match ConfigDoc::parse(text) {
        Ok(document) => {
            let view = ConfigView::from_doc(&document);
            let report = validate(&document, &view);
            Ok((document, view, report))
        }
        Err(error) => Err(ValidationReport {
            issues: vec![syntax_issue(text, &error)],
        }),
    }
}

/// The L0 issue for a parse error, naming the rule-style mix when that caused it.
#[must_use]
pub fn syntax_issue(text: &str, error: &ParseError) -> ValidationIssue {
    let mixes_rule_styles = text
        .lines()
        .any(|line| line.trim_start().starts_with("[[on-window-detected]]"))
        && text.lines().any(|line| {
            line.trim_start()
                .strip_prefix("on-window-detected")
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        });
    if mixes_rule_styles {
        ValidationIssue {
            key_path: Some(KeyPath::from(["on-window-detected"])),
            span: None,
            severity: Severity::Error,
            kind: IssueKind::MixedRuleStyles,
            message: "on-window-detected is written both as [[on-window-detected]] tables and as \
                      an array; use one form"
                .to_owned(),
        }
    } else {
        ValidationIssue {
            key_path: None,
            span: None,
            severity: Severity::Error,
            kind: IssueKind::Syntax,
            message: format!("TOML syntax error at {error}"),
        }
    }
}

/// L1: known keys and their types. Unknown keys are warnings because a newer AeroSpace may
/// accept them and the file already has them.
fn schema(document: &ConfigDoc, view: &ConfigView, issues: &mut Vec<ValidationIssue>) {
    for key in &view.keys {
        let path = &key.key.value;
        let issue = |severity, kind, message| ValidationIssue {
            key_path: Some(path.clone()),
            span: key.key.span.clone(),
            severity,
            kind,
            message,
        };
        match key.spec {
            None => issues.push(issue(
                Severity::Warning,
                IssueKind::UnknownKey,
                format!("{path} is not a key AeroSpace 0.21 knows"),
            )),
            Some(spec) if spec.scope == Scope::Deprecated => issues.push(issue(
                Severity::Warning,
                IssueKind::DeprecatedKey,
                format!("{path} is deprecated in AeroSpace 0.21"),
            )),
            Some(spec) => {
                let wrong_type = spec.value_type.filter(|expected| {
                    document
                        .get(path)
                        .is_some_and(|item| !expected.matches(item))
                });
                if let Some(expected) = wrong_type {
                    issues.push(issue(
                        Severity::Error,
                        IssueKind::WrongType,
                        format!("{path} must be {}", expected.describe()),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn issues(text: &str) -> Vec<(Severity, IssueKind, String)> {
        let report = match validate_text(text) {
            Ok((_, _, report)) | Err(report) => report,
        };
        report
            .issues
            .into_iter()
            .map(|issue| (issue.severity, issue.kind, issue.message))
            .collect()
    }

    #[test]
    fn each_error_class_has_its_message() {
        let cases: &[(&str, Severity, IssueKind, &str)] = &[
            (
                "gaps = = 1\n",
                Severity::Error,
                IssueKind::Syntax,
                "TOML syntax error at line 1, column 8: extra `=`, expected nothing",
            ),
            (
                "on-window-detected = []\n[[on-window-detected]]\nrun = 'x'\n",
                Severity::Error,
                IssueKind::MixedRuleStyles,
                "on-window-detected is written both as [[on-window-detected]] tables and as an array; use one form",
            ),
            (
                "accordion-padding = 'wide'\n",
                Severity::Error,
                IssueKind::WrongType,
                "accordion-padding must be an integer",
            ),
            (
                "after-startup-command = [1]\n",
                Severity::Error,
                IssueKind::WrongType,
                "after-startup-command must be a command or an array of commands",
            ),
            (
                "[key-mapping]\npreset = 1\n",
                Severity::Error,
                IssueKind::WrongType,
                "key-mapping.preset must be a string",
            ),
            (
                "on-window-detected = ['x']\n",
                Severity::Error,
                IssueKind::WrongType,
                "on-window-detected must be an array of tables",
            ),
            (
                "new-shiny-key = true\n",
                Severity::Warning,
                IssueKind::UnknownKey,
                "new-shiny-key is not a key AeroSpace 0.21 knows",
            ),
            (
                "focus-follows-mouse.speed = 3\n",
                Severity::Warning,
                IssueKind::UnknownKey,
                "focus-follows-mouse.speed is not a key AeroSpace 0.21 knows",
            ),
            (
                "after-login-command = []\n",
                Severity::Warning,
                IssueKind::DeprecatedKey,
                "after-login-command is deprecated in AeroSpace 0.21",
            ),
        ];
        for (text, severity, kind, message) in cases {
            assert_eq!(
                issues(text),
                [(*severity, *kind, (*message).to_owned())],
                "{text}"
            );
        }
    }

    #[test]
    fn warnings_do_not_block_writes() {
        let (_, _, report) = validate_text("new-shiny-key = true\n").unwrap();
        assert!(!report.blocks_write());
        let (_, _, report) = validate_text("accordion-padding = 'wide'\n").unwrap();
        assert!(report.blocks_write());
    }

    #[test]
    fn binding_error_classes_have_exact_messages() {
        let cases = [
            (
                "[mode.main.binding]\nalt-noSuchKey = 'focus left'\n",
                IssueKind::InvalidChord,
                "alt-noSuchKey: `noSuchKey` is not an AeroSpace key name",
                "mode.main.binding.alt-noSuchKey",
            ),
            (
                "[mode.main.binding]\nhyper-h = 'focus left'\n",
                IssueKind::InvalidChord,
                "hyper-h: `hyper` is not an AeroSpace modifier",
                "mode.main.binding.hyper-h",
            ),
            (
                "[mode.main.binding]\nalt-alt-h = 'focus left'\n",
                IssueKind::InvalidChord,
                "alt-alt-h: the `alt` modifier is repeated",
                "mode.main.binding.alt-alt-h",
            ),
            (
                "[mode.main.binding]\nshift-alt-h = 'focus left'\nalt-shift-h = 'focus right'\n",
                IssueKind::DuplicateChord,
                "shift-alt-h is already bound to `focus left` in main",
                "mode.main.binding.alt-shift-h",
            ),
            (
                "key-mapping.key-notation-to-key-code.homeKey = 'h'\n\
                 [mode.main.binding]\nalt-h = 'focus left'\nalt-homeKey = 'focus right'\n",
                IssueKind::DuplicateChord,
                "alt-h is already bound to `focus left` in main",
                "mode.main.binding.alt-homeKey",
            ),
            (
                "[mode.main.binding]\nshift-alt-h = ['focus left', 'mode main']\nalt-shift-h = 'close'\n",
                IssueKind::DuplicateChord,
                "shift-alt-h is already bound to `focus left; mode main` in main",
                "mode.main.binding.alt-shift-h",
            ),
            (
                "[mode.main.binding]\nshift-alt-h = 'future-command --flag'\nalt-shift-h = 'close'\n",
                IssueKind::DuplicateChord,
                "shift-alt-h is already bound to `'future-command --flag'` in main",
                "mode.main.binding.alt-shift-h",
            ),
            (
                "[mode.main.binding]\nalt-h = ['focus left', 'mode missing']\n",
                IssueKind::UnknownMode,
                "mode missing refers to an unknown mode",
                "mode.main.binding.alt-h",
            ),
            (
                "[mode.service.binding]\nesc = 'close'\n",
                IssueKind::MissingMainMode,
                "mode.main is required",
                "mode.main",
            ),
            (
                "mode = {}\n",
                IssueKind::MissingMainMode,
                "mode.main is required",
                "mode.main",
            ),
        ];
        for (text, kind, message, path) in cases {
            let (_, _, report) = validate_text(text).unwrap();
            let errors = report.errors().collect::<Vec<_>>();
            assert_eq!(errors.len(), 1, "{text}: {report:?}");
            let issue = errors.first().unwrap();
            assert_eq!(issue.severity, Severity::Error, "{text}");
            assert_eq!(issue.kind, kind, "{text}");
            assert_eq!(issue.message, message, "{text}");
            assert_eq!(issue.key_path.as_ref().unwrap().to_string(), path, "{text}");
            assert!(issue.span.is_some(), "{text}");
            assert!(report.blocks_write(), "{text}");
        }
    }

    #[test]
    fn chord_availability_checks_aliases_modes_and_ignored_keys() {
        let (_, view, _) = validate_text(
            "key-mapping.key-notation-to-key-code.homeKey = 'h'\n\
             [mode.main.binding]\nshift-alt-h = 'focus left'\n\
             [mode.service.binding]\nalt-l = 'mode main'\n",
        )
        .unwrap();
        let chord = Chord::parse("alt-shift-homeKey", &view.key_mapping).unwrap();
        let conflict = check_chord_available(&view, "main", &chord, None).unwrap_err();
        assert_eq!(conflict.mode, "main");
        assert_eq!(conflict.binding.raw_key, "shift-alt-h");
        assert!(check_chord_available(&view, "main", &chord, Some("shift-alt-h")).is_ok());
        assert!(check_chord_available(&view, "main", &chord, Some("alt-shift-h")).is_err());
        assert!(check_chord_available(&view, "service", &chord, None).is_ok());
        assert!(check_chord_available(&view, "new-mode", &chord, None).is_ok());
    }

    #[test]
    fn duplicate_rule_reports_each_later_binding_against_the_first() {
        let (_, view, report) = validate_text(
            "key-mapping.key-notation-to-key-code.homeKey = 'h'\n\
             [mode.main.binding]\nshift-alt-h = 'focus left'\n\
             alt-shift-h = 'focus right'\nalt-shift-homeKey = 'close'\n",
        )
        .unwrap();
        assert_eq!(report.issues.len(), 2);
        for issue in &report.issues {
            assert_eq!(issue.kind, IssueKind::DuplicateChord);
            assert_eq!(
                issue.message,
                "shift-alt-h is already bound to `focus left` in main"
            );
        }
        let chord = Chord::parse("alt-shift-h", &view.key_mapping).unwrap();
        let conflict =
            check_chord_available(&view, "main", &chord, Some("shift-alt-h")).unwrap_err();
        assert_eq!(conflict.binding.raw_key, "alt-shift-h");
    }

    #[test]
    fn macos_shortcut_warnings_do_not_block_writes() {
        for raw in MACOS_SHORTCUTS {
            let source = format!("[mode.main.binding]\n{raw} = 'close'\n");
            let (_, _, report) = validate_text(&source).unwrap();
            assert_eq!(report.issues.len(), 1);
            let issue = report.issues.first().unwrap();
            assert_eq!(issue.kind, IssueKind::MacOsShortcut);
            assert_eq!(issue.severity, Severity::Warning);
            assert_eq!(
                issue.message,
                format!("{raw} shadows the macOS shortcut {raw}")
            );
            assert_eq!(
                issue.key_path,
                Some(KeyPath::from(["mode", "main", "binding", raw]))
            );
            assert!(issue.span.is_some());
            assert!(!report.blocks_write());
        }
        let (_, _, report) = validate_text(
            "key-mapping.key-notation-to-key-code.homeKey = 'h'\n\
             [mode.main.binding]\ncmd-homeKey = 'close'\ncmd-shift-q = 'close'\n",
        )
        .unwrap();
        assert_eq!(report.issues.len(), 1);
        assert_eq!(
            report.issues.first().unwrap().message,
            "cmd-homeKey shadows the macOS shortcut cmd-h"
        );
    }

    #[test]
    fn unsupported_scripts_and_valid_mode_references_are_not_errors() {
        for text in [
            "",
            "[mode.main]\n",
            "[mode.main.binding]\nalt-h = 'focus sideways'\n",
            "[mode.main.binding]\nalt-h = 'future-command'\n",
            "[mode.main.binding]\nalt-h = 'mode missing; close'\n",
            "[mode.main.binding]\nalt-h = 'mode service'\n\
             [mode.service.binding]\nalt-h = 'mode main'\n",
        ] {
            let (_, _, report) = validate_text(text).unwrap();
            assert!(report.issues.is_empty(), "{text}: {report:?}");
        }
    }

    #[test]
    fn non_main_modes_without_a_return_binding_warn_but_allow_writes() {
        for (script, warns) in [
            ("", true),
            ("esc='close'\n", true),
            ("esc='mode main'\n", false),
            ("esc=['close', 'mode main']\n", false),
            ("esc=['future-command', 'mode main']\n", false),
        ] {
            let source = format!("[mode.main.binding]\n[mode.service.binding]\n{script}");
            let (_, _, report) = validate_text(&source).unwrap();
            assert!(!report.blocks_write());
            assert_eq!(report.issues.len(), usize::from(warns));
            if warns {
                let issue = report.issues.first().unwrap();
                assert_eq!(issue.kind, IssueKind::NoWayBack);
                assert_eq!(issue.severity, Severity::Warning);
                assert_eq!(issue.message, "mode service has no binding back to main");
                assert_eq!(issue.key_path, Some(KeyPath::from(["mode", "service"])));
            }
        }
    }

    #[test]
    fn every_fixture_validates_without_errors() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "toml") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            let (_, _, report) = validate_text(&text).unwrap();
            let errors: Vec<_> = report.errors().map(|issue| &issue.message).collect();
            assert!(errors.is_empty(), "{}: {errors:?}", path.display());
        }
    }
}
