//! The one entity that owns the config: document, typed view, validation, undo history and save
//! status. Writes run the core pipeline off the UI thread, at most one at a time.

use std::mem;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui_kit::{AppContext as _, Context, EventEmitter, Task};
use planeui_core::aerospace::{AerospaceVersion, CliError, ReloadMessage, ReloadOutcome};
use planeui_core::apply::{ApplyContext, ApplyOutcome, apply};
use planeui_core::config_doc::{ConfigDoc, EditError, ParseError};
use planeui_core::config_view::ConfigView;
use planeui_core::history::{History, HistoryStep};
use planeui_core::store::{DiskStamp, Loaded};
use planeui_core::validate::{ValidationReport, validate};

/// What happened to the latest change, for the status bar and the reload-error banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveStatus {
    /// Nothing has been written this session.
    Idle,
    /// A write and reload are in flight.
    Writing,
    /// Written, and AeroSpace reloaded it.
    Applied {
        /// When the reload finished.
        at: SystemTime,
    },
    /// Written, but AeroSpace reported errors while reloading it.
    ReloadFailed {
        /// AeroSpace's messages, each naming its key when it did.
        messages: Vec<ReloadMessage>,
    },
    /// Written; AeroSpace is not running, so the change applies when it next starts.
    NotRunning,
    /// Written; the `aerospace` command was not found, so nothing was reloaded.
    NotInstalled,
    /// The change was not written, or the reload could not be run; the text says why.
    Error(String),
}

impl SaveStatus {
    fn after_reload(reload: Result<ReloadOutcome, CliError>) -> Self {
        match reload {
            Ok(ReloadOutcome::Applied) => Self::Applied {
                at: SystemTime::now(),
            },
            Ok(ReloadOutcome::Failed { errors, .. }) => Self::ReloadFailed { messages: errors },
            Ok(ReloadOutcome::NotRunning) => Self::NotRunning,
            Err(CliError::BinaryNotFound) => Self::NotInstalled,
            Err(error) => Self::Error(format!("Saved, but AeroSpace could not reload it: {error}")),
        }
    }
}

/// Whether AeroSpace's server answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AerospaceState {
    /// Not asked yet, or the answer was unreadable.
    #[default]
    Unknown,
    /// The server is running.
    Running,
    /// The CLI is installed but the server is not running.
    NotRunning,
    /// The `aerospace` command was not found.
    NotInstalled,
}

impl AerospaceState {
    fn after_reload(reload: &Result<ReloadOutcome, CliError>) -> Option<Self> {
        match reload {
            Ok(ReloadOutcome::Applied | ReloadOutcome::Failed { .. }) => Some(Self::Running),
            Ok(ReloadOutcome::NotRunning) => Some(Self::NotRunning),
            Err(CliError::BinaryNotFound) => Some(Self::NotInstalled),
            Err(_) => None,
        }
    }
}

/// What PlaneUI last learned about AeroSpace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AerospaceInfo {
    /// The installed version, once known.
    pub version: Option<AerospaceVersion>,
    /// Whether the server is running.
    pub state: AerospaceState,
}

/// One-off occurrences views react to; lasting state is read from the model instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigEvent {
    /// The file changed outside PlaneUI and the model now holds what is on disk.
    ReloadedFromDisk {
        /// The label of the latest change that was dropped instead of written over the edit.
        discarded: Option<String>,
    },
}

#[derive(Debug)]
struct Write {
    text: String,
    label: String,
}

#[derive(Debug, Default)]
enum WriteQueue {
    #[default]
    Idle,
    /// `next` is the latest state committed while `_task` was writing; older ones were replaced,
    /// so a burst of commits costs one trailing write.
    Busy {
        _task: Task<()>,
        next: Option<Write>,
    },
}

/// Owns the config document and everything derived from it.
#[derive(Debug)]
pub struct ConfigModel {
    display_path: PathBuf,
    context: ApplyContext,
    doc: ConfigDoc,
    view: ConfigView,
    report: ValidationReport,
    /// The text of the latest valid state: on disk, or queued to be. Undo history is recorded
    /// against it, so an invalid in-memory edit never becomes an undo target.
    committed: String,
    /// The text last known to be on disk, which `stamp` describes.
    on_disk: String,
    history: History,
    stamp: DiskStamp,
    last_result: SaveStatus,
    aerospace: AerospaceInfo,
    queue: WriteQueue,
    aerospace_check: Option<Task<()>>,
    disk_check: Option<Task<()>>,
}

impl EventEmitter<ConfigEvent> for ConfigModel {}

impl ConfigModel {
    /// A model of `loaded`, shown to the user as `display_path`, that writes through `context`.
    ///
    /// # Errors
    ///
    /// The loaded text is not valid TOML, so there is nothing to edit.
    pub fn open(
        display_path: PathBuf,
        context: ApplyContext,
        loaded: Loaded,
    ) -> Result<Self, ParseError> {
        let doc = ConfigDoc::parse(&loaded.text)?;
        let view = ConfigView::from_doc(&doc);
        let report = validate(&doc, &view);
        Ok(Self {
            display_path,
            context,
            doc,
            view,
            report,
            committed: loaded.text.clone(),
            on_disk: loaded.text,
            history: History::new(),
            stamp: loaded.stamp,
            last_result: SaveStatus::Idle,
            aerospace: AerospaceInfo::default(),
            queue: WriteQueue::Idle,
            aerospace_check: None,
            disk_check: None,
        })
    }

    /// The config path as the user knows it.
    #[must_use]
    pub fn display_path(&self) -> &Path {
        &self.display_path
    }

    /// The document as currently edited, which may hold an invalid, unwritten edit.
    #[must_use]
    pub fn doc(&self) -> &ConfigDoc {
        &self.doc
    }

    /// The typed view of [`Self::doc`].
    #[must_use]
    pub fn view(&self) -> &ConfigView {
        &self.view
    }

    /// The validation issues of [`Self::doc`].
    #[must_use]
    pub fn report(&self) -> &ValidationReport {
        &self.report
    }

    /// Where the latest change stands.
    #[must_use]
    pub fn status(&self) -> SaveStatus {
        match self.queue {
            WriteQueue::Busy { .. } => SaveStatus::Writing,
            WriteQueue::Idle => self.last_result.clone(),
        }
    }

    /// What is known about AeroSpace; see [`Self::refresh_aerospace`].
    #[must_use]
    pub fn aerospace(&self) -> &AerospaceInfo {
        &self.aerospace
    }

    /// The label of the change [`Self::undo`] would revert.
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_label()
    }

    /// Applies `change` to a copy of the document and commits the result.
    ///
    /// # Errors
    ///
    /// The edit itself failed; nothing changed.
    pub fn edit(
        &mut self,
        label: impl Into<String>,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut ConfigDoc) -> Result<(), EditError>,
    ) -> Result<(), EditError> {
        let mut doc = self.doc.clone();
        change(&mut doc)?;
        self.commit(doc, label, cx);
        Ok(())
    }

    /// Makes `doc` the document. A valid document is recorded for undo and queued for writing;
    /// an invalid one stays in memory with its issues until a later commit fixes it.
    pub fn commit(&mut self, doc: ConfigDoc, label: impl Into<String>, cx: &mut Context<Self>) {
        let text = doc.to_string();
        self.show(doc);
        if !self.report.blocks_write() && text != self.committed {
            let label = label.into();
            let before = mem::replace(&mut self.committed, text.clone());
            self.history.record(before, text.clone(), label.clone());
            self.enqueue(Write { text, label }, cx);
        }
        cx.notify();
    }

    /// Writes the document as it was before the latest change.
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(step) = self.history.undo() {
            self.restore(step, "Undo", cx);
        }
    }

    /// Writes the document as it was after the latest undone change.
    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(step) = self.history.redo() {
            self.restore(step, "Redo", cx);
        }
    }

    /// Asks AeroSpace for its version and whether it runs, in the background.
    pub fn refresh_aerospace(&mut self, cx: &mut Context<Self>) {
        let aerospace = self.context.aerospace.clone();
        self.aerospace_check = Some(cx.spawn(async move |this, cx| {
            let (version, config_path) = cx
                .background_spawn(async move { (aerospace.version(), aerospace.config_path()) })
                .await;
            let state = match config_path {
                Ok(Some(_)) => AerospaceState::Running,
                Ok(None) => AerospaceState::NotRunning,
                Err(CliError::BinaryNotFound) => AerospaceState::NotInstalled,
                Err(error) => {
                    log::warn!("could not ask AeroSpace whether it runs: {error}");
                    AerospaceState::Unknown
                }
            };
            let version = version
                .inspect_err(|error| {
                    if !matches!(error, CliError::BinaryNotFound) {
                        log::warn!("could not read the AeroSpace version: {error}");
                    }
                })
                .ok();
            log_if_released(this.update(cx, |this, cx| {
                this.aerospace = AerospaceInfo { version, state };
                cx.notify();
            }));
        }));
    }

    /// Reloads the document if the file changed outside PlaneUI, checking in the background.
    /// Skipped while a write is in flight: that write checks the disk itself, and a half-done
    /// write would look like an external edit.
    pub fn check_disk(&mut self, cx: &mut Context<Self>) {
        if matches!(self.queue, WriteQueue::Busy { .. }) {
            return;
        }
        let store = self.context.store.clone();
        let expected = self.stamp.clone();
        self.disk_check = Some(cx.spawn(async move |this, cx| {
            let current = cx.background_spawn(async move { store.load() }).await;
            log_if_released(this.update(cx, |this, cx| {
                // A write that started or finished meanwhile owns the stamp now.
                if matches!(this.queue, WriteQueue::Busy { .. }) || this.stamp != expected {
                    return;
                }
                match current {
                    Ok(current) if current.stamp != expected => {
                        this.reload_from_disk(current, None, cx);
                    }
                    Ok(_) => return,
                    Err(error) => {
                        this.last_result =
                            SaveStatus::Error(format!("Could not read the config: {error}"));
                    }
                }
                cx.notify();
            }));
        }));
    }

    fn show(&mut self, doc: ConfigDoc) {
        self.view = ConfigView::from_doc(&doc);
        self.report = validate(&doc, &self.view);
        self.doc = doc;
    }

    fn restore(&mut self, step: HistoryStep, verb: &str, cx: &mut Context<Self>) {
        match ConfigDoc::parse(&step.text) {
            Ok(doc) => {
                self.show(doc);
                self.committed.clone_from(&step.text);
                let label = format!("{verb} {}", step.label);
                self.enqueue(
                    Write {
                        text: step.text,
                        label,
                    },
                    cx,
                );
            }
            Err(error) => {
                log::error!("history held a document that does not parse: {error}");
                self.last_result = SaveStatus::Error(format!("Could not {verb}: {error}"));
            }
        }
        cx.notify();
    }

    fn enqueue(&mut self, write: Write, cx: &mut Context<Self>) {
        match &mut self.queue {
            WriteQueue::Busy { next, .. } => *next = Some(write),
            WriteQueue::Idle => self.start_write(write, cx),
        }
    }

    fn start_write(&mut self, write: Write, cx: &mut Context<Self>) {
        let context = self.context.clone();
        let stamp = self.stamp.clone();
        let Write { text, label } = write;
        let task = cx.spawn(async move |this, cx| {
            let (text, outcome) = cx
                .background_spawn(async move {
                    let outcome = apply(&context, &stamp, &text);
                    (text, outcome)
                })
                .await;
            log_if_released(
                this.update(cx, |this, cx| this.finish_write(label, &text, outcome, cx)),
            );
        });
        self.queue = WriteQueue::Busy {
            _task: task,
            next: None,
        };
    }

    fn finish_write(
        &mut self,
        label: String,
        text: &str,
        outcome: ApplyOutcome,
        cx: &mut Context<Self>,
    ) {
        let next = match mem::take(&mut self.queue) {
            WriteQueue::Busy { next, .. } => next,
            WriteQueue::Idle => None,
        };
        let not_written = |reason: String| SaveStatus::Error(format!("Not saved: {reason}"));
        match outcome {
            ApplyOutcome::ExternalChange { current } => {
                // The queued state was built on the old file, so writing it would clobber the
                // external edit.
                let discarded = next.map_or(label, |next| next.label);
                self.reload_from_disk(*current, Some(discarded), cx);
                cx.notify();
                return;
            }
            ApplyOutcome::Written { reload, new_stamp } => {
                self.stamp = new_stamp;
                text.clone_into(&mut self.on_disk);
                if let Some(state) = AerospaceState::after_reload(&reload) {
                    self.aerospace.state = state;
                }
                self.last_result = SaveStatus::after_reload(reload);
            }
            ApplyOutcome::Rejected(report) => {
                let reason = report.errors().next().map_or_else(
                    || "the document has errors".to_owned(),
                    |issue| issue.message.clone(),
                );
                self.last_result = not_written(reason);
            }
            ApplyOutcome::CheckFailed(error) => self.last_result = not_written(error.to_string()),
            ApplyOutcome::SnapshotFailed(error) => {
                self.last_result = not_written(error.to_string());
            }
            ApplyOutcome::WriteFailed(error) => self.last_result = not_written(error.to_string()),
        }
        match next {
            Some(next) => self.start_write(next, cx),
            // Nothing was written, so the disk text is the latest valid state again; committing
            // the same value after fixing the cause must count as a change.
            None if self.on_disk != text => self.committed.clone_from(&self.on_disk),
            None => {}
        }
        cx.notify();
    }

    fn reload_from_disk(
        &mut self,
        current: Loaded,
        discarded: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match ConfigDoc::parse(&current.text) {
            Ok(doc) => {
                self.show(doc);
                self.committed.clone_from(&current.text);
                self.stamp = current.stamp;
                // The external edit is a change like any other, so undo reverts it first and
                // redo can bring it back instead of losing it.
                let previous = mem::replace(&mut self.on_disk, current.text.clone());
                self.history.note_external_change();
                self.history
                    .record(previous, current.text, "Edit made outside PlaneUI");
                cx.emit(ConfigEvent::ReloadedFromDisk { discarded });
            }
            Err(error) => {
                // Keeping the old stamp stops every later write at the external-change check,
                // so the broken file is never overwritten.
                self.last_result = SaveStatus::Error(format!(
                    "{} changed on disk and is not valid TOML ({error}); fix it there",
                    self.display_path.display()
                ));
            }
        }
    }
}

fn log_if_released(result: anyhow::Result<()>) {
    if let Err(error) = result {
        log::debug!("the config model closed before its task finished: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{Entity, TestAppContext};
    use planeui_core::aerospace::FakeAerospace;
    use planeui_core::config_doc::KeyPath;

    use super::*;
    use crate::test_support::{Fixture, set_padding};

    const ORIGINAL: &str = "# Gaps between windows\naccordion-padding = 30 # px\n";

    fn events(
        cx: &mut TestAppContext,
        model: &Entity<ConfigModel>,
    ) -> Rc<RefCell<Vec<ConfigEvent>>> {
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        cx.update(|cx| {
            cx.subscribe(model, move |_, event: &ConfigEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
        });
        events
    }

    #[gpui_kit::test]
    fn commits_during_a_write_coalesce_into_one_trailing_write(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let model = fixture.model(cx);

        set_padding(&model, 1, cx);
        assert_eq!(
            model.read_with(cx, |model, _| model.status()),
            SaveStatus::Writing
        );
        set_padding(&model, 2, cx);
        set_padding(&model, 3, cx);
        cx.run_until_parked();

        assert_eq!(
            fixture.aerospace.reload_count(),
            2,
            "one write per burst plus the first"
        );
        assert_eq!(
            fixture.snapshots(),
            vec![
                ORIGINAL.to_owned(),
                "# Gaps between windows\naccordion-padding = 1 # px\n".to_owned(),
            ],
            "the trailing write ran against the first write's stamp"
        );
        assert_eq!(
            fixture.disk(),
            "# Gaps between windows\naccordion-padding = 3 # px\n"
        );
        assert!(matches!(
            model.read_with(cx, |model, _| model.status()),
            SaveStatus::Applied { .. }
        ));
    }

    #[gpui_kit::test]
    fn undo_writes_the_previous_bytes_and_reloads(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let model = fixture.model(cx);
        set_padding(&model, 12, cx);
        cx.run_until_parked();
        let edited = fixture.disk();
        assert_ne!(edited, ORIGINAL);

        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), ORIGINAL);
        assert_eq!(fixture.aerospace.reload_count(), 2);
        assert_eq!(
            model.read_with(cx, |model, _| model.doc().to_string()),
            ORIGINAL
        );

        model.update(cx, ConfigModel::redo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), edited);
        assert_eq!(fixture.aerospace.reload_count(), 3);
    }

    #[gpui_kit::test]
    fn external_change_during_a_burst_reloads_and_never_overwrites(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let model = fixture.model(cx);
        let events = events(cx, &model);

        set_padding(&model, 1, cx);
        let external = "accordion-padding = 99\n";
        fixture.write_disk(external);
        set_padding(&model, 2, cx);
        cx.run_until_parked();

        assert_eq!(fixture.disk(), external);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        assert_eq!(fixture.snapshots(), Vec::<String>::new());
        assert_eq!(
            *events.borrow(),
            vec![ConfigEvent::ReloadedFromDisk {
                discarded: Some("Set accordion-padding to 2".to_owned()),
            }]
        );
        assert_eq!(
            model.read_with(cx, |model, _| model.doc().to_string()),
            external
        );

        set_padding(&model, 5, cx);
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "accordion-padding = 5\n",
            "the next change builds on the external edit"
        );
    }

    #[gpui_kit::test]
    fn a_disk_check_during_a_write_is_not_an_external_change(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let model = fixture.model(cx);
        let events = events(cx, &model);

        set_padding(&model, 1, cx);
        model.update(cx, ConfigModel::check_disk);
        cx.run_until_parked();
        model.update(cx, ConfigModel::check_disk);
        cx.run_until_parked();

        assert_eq!(*events.borrow(), Vec::new());
        assert_eq!(
            fixture.disk(),
            "# Gaps between windows\naccordion-padding = 1 # px\n"
        );
    }

    #[gpui_kit::test]
    fn an_invalid_document_stays_in_memory_and_is_not_written(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let model = fixture.model(cx);

        model.update(cx, |model, cx| {
            model
                .edit("Set accordion-padding to wide", cx, |doc| {
                    doc.set_value(&KeyPath::from(["accordion-padding"]), "wide".into())
                })
                .unwrap();
        });
        cx.run_until_parked();

        assert_eq!(fixture.disk(), ORIGINAL);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        model.read_with(cx, |model, _| {
            assert!(model.report().blocks_write());
            assert!(model.doc().to_string().contains("wide"));
            assert_eq!(model.undo_label(), None);
            assert_eq!(model.status(), SaveStatus::Idle);
        });

        set_padding(&model, 8, cx);
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "# Gaps between windows\naccordion-padding = 8 # px\n"
        );
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            ORIGINAL,
            "undo skips the never-written edit"
        );
    }

    #[gpui_kit::test]
    fn missing_aerospace_still_writes_and_says_so(cx: &mut TestAppContext) {
        let fixture = Fixture::with_aerospace(ORIGINAL, FakeAerospace::missing());
        let model = fixture.model(cx);
        model.update(cx, ConfigModel::refresh_aerospace);
        set_padding(&model, 4, cx);
        cx.run_until_parked();

        assert!(fixture.disk().contains("accordion-padding = 4"));
        model.read_with(cx, |model, _| {
            assert_eq!(model.status(), SaveStatus::NotInstalled);
            assert_eq!(
                *model.aerospace(),
                AerospaceInfo {
                    version: None,
                    state: AerospaceState::NotInstalled,
                }
            );
        });
    }

    #[gpui_kit::test]
    fn a_failed_snapshot_blocks_the_write_and_reports_it(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        fixture.block_snapshots();
        let model = fixture.model(cx);
        set_padding(&model, 4, cx);
        cx.run_until_parked();

        assert_eq!(fixture.disk(), ORIGINAL);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        assert!(matches!(
            model.read_with(cx, |model, _| model.status()),
            SaveStatus::Error(message) if message.starts_with("Not saved:")
        ));
    }

    #[gpui_kit::test]
    fn undo_and_redo_across_an_external_edit_keep_it_in_history(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let model = fixture.model(cx);
        set_padding(&model, 1, cx);
        cx.run_until_parked();
        let edited = fixture.disk();

        let external = "accordion-padding = 99\n";
        fixture.write_disk(external);
        model.update(cx, ConfigModel::check_disk);
        cx.run_until_parked();

        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            edited,
            "undo reverts the external edit first"
        );
        model.update(cx, ConfigModel::redo);
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            external,
            "redo brings the external edit back"
        );
        model.update(cx, ConfigModel::undo);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), ORIGINAL);
    }

    #[gpui_kit::test]
    fn a_failed_write_can_be_retried_with_the_same_value(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        fixture.block_snapshots();
        let model = fixture.model(cx);
        set_padding(&model, 4, cx);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), ORIGINAL);

        fixture.unblock_snapshots();
        set_padding(&model, 4, cx);
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "# Gaps between windows\naccordion-padding = 4 # px\n"
        );
    }
}
