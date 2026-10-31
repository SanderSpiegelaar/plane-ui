//! The Snapshots screen: the copies taken before each write, newest first, each restorable
//! through the normal pipeline.

use std::fmt;
use std::time::SystemTime;

use anyhow::Context as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, TestSupportExt as _,
    Window, div,
};
use planeui_core::config_doc::ConfigDoc;
use planeui_core::store::{SnapshotError, SnapshotId, SnapshotStore, line_diff_stats};
use planeui_core::validate::validate_text;

use crate::config_model::ConfigModel;
use crate::shell::{TimeFormat, display_time};

/// One listed snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotRow {
    id: SnapshotId,
    taken_at: SystemTime,
    /// Lines restoring it would add and remove, or `None` when it could not be read.
    diff: Option<(usize, usize)>,
}

/// Reads every snapshot and diffs it against `current`. Blocks on the file system.
fn list_snapshots(store: &SnapshotStore, current: &str) -> Result<Vec<SnapshotRow>, SnapshotError> {
    Ok(store
        .list()?
        .into_iter()
        .map(|meta| {
            let diff = store
                .read(&meta.id)
                .inspect_err(|error| log::warn!("{error}"))
                .ok()
                .map(|bytes| line_diff_stats(current, &String::from_utf8_lossy(&bytes)));
            SnapshotRow {
                id: meta.id,
                taken_at: meta.taken_at,
                diff,
            }
        })
        .collect())
}

/// The snapshot's document, if it can be written as it is. Blocks on the file system.
fn read_restorable(store: &SnapshotStore, id: &SnapshotId) -> anyhow::Result<ConfigDoc> {
    let bytes = store.read(id)?;
    let text = String::from_utf8(bytes).context("the snapshot is not valid UTF-8")?;
    match validate_text(&text) {
        Ok((doc, _, report)) if !report.blocks_write() => Ok(doc),
        Ok((_, _, report)) | Err(report) => {
            let reason = report
                .errors()
                .next()
                .map_or_else(|| "it has errors".to_owned(), |issue| issue.message.clone());
            anyhow::bail!("the snapshot cannot be written back: {reason}")
        }
    }
}

#[derive(Debug, Default)]
struct SnapshotRows {
    rows: Vec<SnapshotRow>,
    selected: Option<usize>,
    loading: bool,
}

impl ListDelegate for SnapshotRows {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &gpui_kit::App) -> usize {
        self.rows.len()
    }

    fn render_item(
        &mut self,
        index: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let row = self.rows.get(index.row)?;
        let time = display_time(row.taken_at, TimeFormat::DateAndClock);
        let diff = row.diff.map_or_else(
            || "unreadable".to_owned(),
            |(added, removed)| format!("+{added} / −{removed}"),
        );
        Some(
            ListItem::new(("snapshot", index.row))
                .selected(self.selected == Some(index.row))
                .accessibility_label(format!("{time}, {diff}"))
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_4()
                        .child(time)
                        .child(div().text_color(cx.theme().muted_foreground).child(diff)),
                ),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div()
            .p_4()
            .text_color(cx.theme().muted_foreground)
            .child("No snapshots yet. PlaneUI takes one before every write.")
    }

    fn loading(&self, _cx: &gpui_kit::App) -> bool {
        self.loading
    }

    fn set_selected_index(
        &mut self,
        index: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = index.map(|index| index.row);
    }
}

/// Lists the snapshots and restores the chosen one.
pub struct SnapshotsView {
    model: Entity<ConfigModel>,
    list: Entity<ListState<SnapshotRows>>,
    list_error: Option<SharedString>,
    restore_error: Option<SharedString>,
    listing: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl fmt::Debug for SnapshotsView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotsView")
            .field("list_error", &self.list_error)
            .field("restore_error", &self.restore_error)
            .finish_non_exhaustive()
    }
}

impl SnapshotsView {
    /// The snapshots of `model`'s config, listed in the background and again after each change.
    pub fn new(model: Entity<ConfigModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = cx.new(|cx| ListState::new(SnapshotRows::default(), window, cx));
        let subscriptions = vec![
            // Every write adds a snapshot and changes what the others are diffed against.
            cx.observe(&model, |this, _, cx| this.refresh(cx)),
            cx.subscribe_in(&list, window, |this, _, event, window, cx| {
                if let ListEvent::Confirm(index) = event {
                    this.confirm_restore(index.row, window, cx);
                }
            }),
        ];
        let mut this = Self {
            model,
            list,
            list_error: None,
            restore_error: None,
            listing: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let store = model.snapshots().clone();
        let current = model.doc().to_string();
        self.list.update(cx, |list, cx| {
            list.delegate_mut().loading = list.delegate().rows.is_empty();
            cx.notify();
        });
        self.listing = cx.spawn(async move |this, cx| {
            let listed = cx
                .background_spawn(async move { list_snapshots(&store, &current) })
                .await;
            let shown = this.update(cx, |this, cx| {
                let rows = match listed {
                    Ok(rows) => {
                        this.list_error = None;
                        rows
                    }
                    Err(error) => {
                        log::error!("{error}");
                        this.list_error = Some(error.to_string().into());
                        Vec::new()
                    }
                };
                this.list.update(cx, |list, cx| {
                    let delegate = list.delegate_mut();
                    delegate.rows = rows;
                    delegate.loading = false;
                    cx.notify();
                });
                cx.notify();
            });
            if let Err(error) = shown {
                log::debug!("the snapshots screen closed while listing: {error}");
            }
        });
    }

    fn confirm_restore(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.list.read(cx).delegate().rows.get(index).cloned() else {
            return;
        };
        let time = display_time(row.taken_at, TimeFormat::DateAndClock);
        let view = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let row = row.clone();
            alert
                .confirm()
                .title(format!("Restore the snapshot from {time}?"))
                .description(
                    "PlaneUI snapshots the current file first, and Cmd-Z undoes the restore.",
                )
                .ok_text("Restore")
                .on_ok(move |_, _, cx| {
                    let row = row.clone();
                    if let Err(error) = view.update(cx, |this, cx| this.restore(row, cx)) {
                        log::debug!("the snapshots screen closed before restoring: {error}");
                    }
                    true
                })
        });
    }

    /// Reads the snapshot first, so nothing is snapshotted or written unless it can be restored.
    fn restore(&mut self, row: SnapshotRow, cx: &mut Context<Self>) {
        self.restore_error = None;
        cx.notify();
        let store = self.model.read(cx).snapshots().clone();
        let label = format!(
            "Restored snapshot from {}",
            display_time(row.taken_at, TimeFormat::DateAndClock)
        );
        // The commit must not depend on this view: navigating away drops it, and a confirmed
        // restore still has to land. The task holds the model and is detached; the view is only
        // told about a failure if it still exists.
        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            let id = row.id;
            let read = cx
                .background_spawn(async move { read_restorable(&store, &id) })
                .await;
            match read {
                Ok(doc) => model.update(cx, |model, cx| model.commit(doc, label, cx)),
                Err(error) => {
                    log::error!("could not restore: {error:#}");
                    let shown = this.update(cx, |this, cx| {
                        this.restore_error = Some(format!("{error:#}").into());
                        cx.notify();
                    });
                    if let Err(error) = shown {
                        log::debug!("the snapshots screen closed before showing it: {error}");
                    }
                }
            }
        })
        .detach();
    }
}

impl Focusable for SnapshotsView {
    fn focus_handle(&self, cx: &gpui_kit::App) -> FocusHandle {
        self.list.focus_handle(cx)
    }
}

impl Render for SnapshotsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Line counts are what restoring would add and remove. Arrow keys \
                         select, Enter restores.",
                    ),
            )
            .when_some(self.list_error.clone(), |this, error| {
                this.child(Alert::error("list-error", error).title("Could not list the snapshots"))
            })
            .when_some(self.restore_error.clone(), |this, error| {
                this.child(
                    div().id("restore-error").test_support().child(
                        Alert::error("restore-error-alert", error)
                            .title("Could not restore the snapshot"),
                    ),
                )
            })
            .child(
                div()
                    .id("snapshot-list")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .child(List::new(&self.list)),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AnyWindowHandle, TestAppContext};

    use super::*;
    use crate::test_support::{Fixture, in_window, open_window, set_padding};

    const V1: &str = "# Gaps\naccordion-padding = 30 # px\n";
    const V2: &str = "# Gaps\naccordion-padding = 2 # px\n";
    const V3: &str = "# Gaps\naccordion-padding = 3 # px\n";

    /// V1 on disk at first, then V2 and V3 written, so the snapshots hold V1 and V2.
    fn with_history(cx: &mut TestAppContext) -> (Fixture, Entity<ConfigModel>) {
        let fixture = Fixture::new(V1);
        let model = fixture.model(cx);
        set_padding(&model, 2, cx);
        cx.run_until_parked();
        set_padding(&model, 3, cx);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), V3);
        (fixture, model)
    }

    fn open_view(
        cx: &mut TestAppContext,
        model: &Entity<ConfigModel>,
    ) -> (AnyWindowHandle, Entity<SnapshotsView>) {
        let model = model.clone();
        open_window(cx, |window, cx| SnapshotsView::new(model, window, cx))
    }

    fn rows(cx: &mut TestAppContext, view: &Entity<SnapshotsView>) -> Vec<SnapshotRow> {
        view.read_with(cx, |view, cx| view.list.read(cx).delegate().rows.clone())
    }

    /// Selects the row `downs` presses from the top and restores it, confirming the dialog with
    /// Enter too.
    fn restore_with_keys(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<SnapshotsView>,
        downs: usize,
    ) {
        let list = view.read_with(cx, Focusable::focus_handle);
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.focus(&list, cx);
            for _ in 0..downs {
                window.press("down", cx);
            }
            window.press("enter", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx), "asks before restoring");
            window.press("enter", cx);
        });
        cx.run_until_parked();
    }

    fn snapshot_files(fixture: &Fixture) -> usize {
        fs::read_dir(&fixture.snapshot_dir).unwrap().count()
    }

    fn restore_error_shown(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.try_find("restore-error").is_some()
        })
    }

    #[gpui_kit::test]
    fn lists_snapshots_newest_first_with_line_counts(cx: &mut TestAppContext) {
        let (fixture, model) = with_history(cx);
        fs::write(fixture.snapshot_dir.join("20200101T000000.000Z.toml"), "").unwrap();
        let (window, view) = open_view(cx, &model);

        let rows = rows(cx, &view);
        assert_eq!(
            rows.iter().map(|row| row.diff).collect::<Vec<_>>(),
            vec![Some((1, 1)), Some((1, 1)), Some((0, 2))],
            "against the current file, restoring the empty one removes both lines"
        );
        let times = rows.iter().map(|row| row.taken_at).collect::<Vec<_>>();
        assert!(
            times.is_sorted_by(|newer, older| newer > older),
            "newest first"
        );
        let label = display_time(*times.last().unwrap(), TimeFormat::DateAndClock);
        assert_eq!(label, "2020-01-01 00:00:00 UTC");
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("snapshot-list").is_some());
        });
    }

    #[gpui_kit::test]
    fn restoring_writes_the_snapshot_and_undo_returns(cx: &mut TestAppContext) {
        let (fixture, model) = with_history(cx);
        let (window, view) = open_view(cx, &model);

        restore_with_keys(cx, window, &view, 2);

        assert_eq!(fixture.disk(), V1, "the older snapshot, byte for byte");
        assert_eq!(
            fixture.snapshots(),
            vec![V1.to_owned(), V2.to_owned(), V3.to_owned()],
            "the pre-restore file was snapshotted"
        );
        assert_eq!(fixture.aerospace.reload_count(), 3);
        let label = model.read_with(cx, |model, _| model.undo_label().map(str::to_owned));
        assert!(
            label.as_deref().is_some_and(
                |label| label.starts_with("Restored snapshot from ") && label.ends_with(" UTC")
            ),
            "{label:?}"
        );
        assert_eq!(rows(cx, &view).len(), 3, "the list follows the write");

        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), V3);
    }

    #[gpui_kit::test]
    fn an_unreadable_snapshot_shows_an_error_and_writes_nothing(cx: &mut TestAppContext) {
        let (fixture, model) = with_history(cx);
        let (window, view) = open_view(cx, &model);
        let newest = rows(cx, &view).first().unwrap().id.clone();
        let path = fixture.snapshot_dir.join(format!("{newest}.toml"));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let files = snapshot_files(&fixture);

        restore_with_keys(cx, window, &view, 1);

        assert!(restore_error_shown(cx, window));
        assert_eq!(fixture.disk(), V3);
        assert_eq!(fixture.aerospace.reload_count(), 2);
        assert_eq!(snapshot_files(&fixture), files);
    }

    #[gpui_kit::test]
    fn a_snapshot_that_does_not_parse_is_not_restored(cx: &mut TestAppContext) {
        let (fixture, model) = with_history(cx);
        fs::write(
            fixture.snapshot_dir.join("20200101T000000.000Z.toml"),
            "accordion-padding = [\n",
        )
        .unwrap();
        let (window, view) = open_view(cx, &model);
        let files = snapshot_files(&fixture);

        restore_with_keys(cx, window, &view, 3);

        assert!(restore_error_shown(cx, window));
        assert_eq!(fixture.disk(), V3);
        assert_eq!(fixture.aerospace.reload_count(), 2);
        assert_eq!(snapshot_files(&fixture), files);
        assert_eq!(
            model.read_with(cx, |model, _| model.doc().to_string()),
            V3,
            "the editor still shows the current file"
        );
    }

    #[gpui_kit::test]
    fn cancelling_the_confirmation_restores_nothing(cx: &mut TestAppContext) {
        let (fixture, model) = with_history(cx);
        let (window, view) = open_view(cx, &model);
        let list = view.read_with(cx, Focusable::focus_handle);
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.focus(&list, cx);
            window.press("down", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            window.press("escape", cx);
        });
        cx.run_until_parked();

        assert_eq!(fixture.disk(), V3);
        assert_eq!(fixture.aerospace.reload_count(), 2);
    }

    #[gpui_kit::test]
    fn a_confirmed_restore_lands_even_if_the_screen_closes(cx: &mut TestAppContext) {
        let (fixture, model) = with_history(cx);
        let (window, view) = open_view(cx, &model);
        let oldest = rows(cx, &view).last().cloned().unwrap();

        view.update(cx, |view, cx| view.restore(oldest, cx));
        in_window(cx, window, |window, _| window.remove_window());
        drop(view);
        cx.run_until_parked();

        assert_eq!(fixture.disk(), V1);
    }
}
