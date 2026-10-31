//! Exact, unlimited in-session undo and redo. Configs are small, so each entry stores whole
//! documents instead of inverse edits; undoing is then a plain write of the stored text.

/// One committed change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Document text before the change.
    pub before: String,
    /// Document text after the change.
    pub after: String,
    /// Short plain-language description, e.g. "Set accordion-padding to 20".
    pub label: String,
    /// The file was reloaded from disk after this change, so undoing it reaches back past an
    /// edit made outside PlaneUI.
    pub crosses_external_change: bool,
}

/// The document text an undo or redo should write, with what it reverts or reapplies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryStep {
    /// Text to write.
    pub text: String,
    /// The label of the change being undone or redone.
    pub label: String,
    /// Whether the step reaches back past an external edit.
    pub crosses_external_change: bool,
}

/// Undo and redo stacks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

impl History {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a committed change. A new change clears the redo stack.
    pub fn record(&mut self, before: String, after: String, label: impl Into<String>) {
        self.redo.clear();
        self.undo.push(Entry {
            before,
            after,
            label: label.into(),
            crosses_external_change: false,
        });
    }

    /// Pops the latest change; the returned text is the document before it.
    pub fn undo(&mut self) -> Option<HistoryStep> {
        let entry = self.undo.pop()?;
        let step = HistoryStep {
            text: entry.before.clone(),
            label: entry.label.clone(),
            crosses_external_change: entry.crosses_external_change,
        };
        self.redo.push(entry);
        Some(step)
    }

    /// Re-applies the latest undone change; the returned text is the document after it.
    pub fn redo(&mut self) -> Option<HistoryStep> {
        let entry = self.redo.pop()?;
        let step = HistoryStep {
            text: entry.after.clone(),
            label: entry.label.clone(),
            crosses_external_change: entry.crosses_external_change,
        };
        self.undo.push(entry);
        Some(step)
    }

    /// Marks every recorded change as older than an external edit. History is kept; the write
    /// pipeline checks the disk before any undo lands, like any other write.
    pub fn note_external_change(&mut self) {
        for entry in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            entry.crosses_external_change = true;
        }
    }

    /// The label of the change [`Self::undo`] would revert.
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|entry| entry.label.as_str())
    }

    /// The label of the change [`Self::redo`] would reapply.
    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|entry| entry.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn new_record_clears_redo() {
        let mut history = History::new();
        history.record("a".into(), "b".into(), "first");
        history.undo();
        history.record("a".into(), "c".into(), "second");
        assert_eq!(history.redo(), None);
        assert_eq!(history.undo_label(), Some("second"));
    }

    #[test]
    fn external_change_marks_existing_entries_only() {
        let mut history = History::new();
        history.record("a".into(), "b".into(), "before reload");
        history.note_external_change();
        history.record("x".into(), "y".into(), "after reload");
        assert!(!history.undo().unwrap().crosses_external_change);
        assert!(history.undo().unwrap().crosses_external_change);
    }

    proptest! {
        #[test]
        fn undo_all_restores_the_original_and_redo_replays(
            texts in proptest::collection::vec(".{0,40}", 1..20),
        ) {
            let original = "original".to_owned();
            let mut history = History::new();
            let mut current = original.clone();
            for text in &texts {
                history.record(current.clone(), text.clone(), "edit");
                current.clone_from(text);
            }
            let latest = current.clone();
            for _ in &texts {
                current = history.undo().unwrap().text;
            }
            prop_assert_eq!(&current, &original);
            prop_assert!(history.undo().is_none());
            for _ in &texts {
                current = history.redo().unwrap().text;
            }
            prop_assert_eq!(current, latest);
            prop_assert!(history.redo().is_none());
        }
    }
}
