//! Syntax, schema, semantic, and lint checks for a `ConfigDoc`. One entry point decides
//! whether a document may be written; later milestones add rules to [`RULES`].

use std::ops::Range;

use crate::config_doc::{ConfigDoc, KeyPath, ParseError};
use crate::config_view::{ConfigView, Scope};

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
pub const RULES: &[Rule] = &[schema];

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
