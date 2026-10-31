//! Persistent workspaces and ordered monitor assignments.

use std::fmt::Display;

use gpui_kit::component::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, TestSupportExt as _, Window, div,
};
use planeui_core::commands::WorkspaceName;
use planeui_core::config_doc::ConfigDoc;
use planeui_core::config_view::{Assignment, Field};
use planeui_core::monitors::{self, MonitorPattern};
use planeui_core::settings::{
    add_persistent_workspace, move_persistent_workspace, remove_persistent_workspace,
};

use crate::config_model::ConfigModel;
use crate::live_data::{LiveData, LiveState};
use crate::monitor_picker::{MonitorPicker, PatternPicked};

#[derive(Debug)]
struct PatternEditor {
    workspace: String,
    index: Option<usize>,
    picker: Entity<MonitorPicker>,
    _subscription: Subscription,
}

#[derive(Debug)]
pub struct WorkspacesView {
    model: Entity<ConfigModel>,
    input: Entity<InputState>,
    live_data: Entity<LiveData>,
    assignment_input: Entity<InputState>,
    editor: Option<PatternEditor>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl WorkspacesView {
    pub fn has_invalid_field(&self, cx: &App) -> bool {
        self.error.is_some()
            || self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.picker.read(cx).has_invalid_field(cx))
    }

    pub fn new(
        model: Entity<ConfigModel>,
        live_data: Entity<LiveData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name"));
        let assignment_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name"));
        let subscriptions = vec![
            cx.observe_in(&model, window, |this, model, window, cx| {
                if let Some(editor) = &this.editor
                    && let Some(index) = editor.index
                {
                    let pattern = model
                        .read(cx)
                        .view()
                        .workspace_to_monitor
                        .iter()
                        .find(|row| row.workspace == editor.workspace && row.error.is_none())
                        .and_then(|row| row.patterns.get(index))
                        .cloned();
                    if let Some(pattern) = pattern {
                        editor
                            .picker
                            .update(cx, |picker, cx| picker.set_value(&pattern, window, cx));
                    } else {
                        this.editor = None;
                    }
                }
                cx.notify();
            }),
            cx.observe(&live_data, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &assignment_input,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add_assignment(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add(window, cx);
                    } else if matches!(event, InputEvent::Change) {
                        this.error = None;
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            model,
            input,
            live_data,
            assignment_input,
            editor: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn edit<E: Display>(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut ConfigDoc) -> Result<(), E>,
        cx: &mut Context<Self>,
    ) {
        self.error = self
            .model
            .update(cx, |model, cx| model.edit(label, cx, change))
            .err()
            .map(|error| error.to_string().into());
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match WorkspaceName::new(self.input.read(cx).value().trim().to_owned()) {
            Ok(name) => {
                self.edit(
                    "Add persistent workspace",
                    |doc| add_persistent_workspace(doc, name.as_str()),
                    cx,
                );
                if self.error.is_none() {
                    self.input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
            Err(error) => {
                self.error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    fn add_assignment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match WorkspaceName::new(self.assignment_input.read(cx).value().trim().to_owned()) {
            Ok(name) => {
                self.edit(
                    "Add monitor assignment",
                    |doc| monitors::add_assignment(doc, name.as_str(), &MonitorPattern::Main),
                    cx,
                );
                if self.error.is_none() {
                    self.assignment_input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
            Err(error) => {
                self.error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    fn edit_pattern(
        &mut self,
        workspace: String,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = index.and_then(|index| {
            self.model
                .read(cx)
                .view()
                .workspace_to_monitor
                .iter()
                .find(|row| row.workspace == workspace)?
                .patterns
                .get(index)
                .cloned()
        });
        let picker =
            cx.new(|cx| MonitorPicker::new(self.live_data.clone(), initial.as_ref(), window, cx));
        let subscription =
            cx.subscribe(&picker, |this: &mut Self, _, event: &PatternPicked, cx| {
                let Some(editor) = &this.editor else {
                    return;
                };
                let target = editor.workspace.clone();
                let index = editor.index;
                let added_index = this
                    .model
                    .read(cx)
                    .view()
                    .workspace_to_monitor
                    .iter()
                    .find(|row| row.workspace == target)
                    .map_or(0, |row| row.patterns.len());
                this.edit(
                    "Set monitor pattern",
                    |doc| match index {
                        Some(index) => monitors::set_pattern(doc, &target, index, &event.0),
                        None => monitors::add_pattern(doc, &target, &event.0),
                    },
                    cx,
                );
                if this.error.is_none()
                    && let Some(editor) = &mut this.editor
                {
                    editor.index = Some(index.unwrap_or(added_index));
                }
            });
        self.editor = Some(PatternEditor {
            workspace,
            index,
            picker,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn assignment_rows(&self, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        self.model
            .read(cx)
            .view()
            .workspace_to_monitor
            .clone()
            .into_iter()
            .enumerate()
            .map(|(row_index, assignment)| self.assignment_row(row_index, assignment, cx))
            .collect()
    }

    fn assignment_row(
        &self,
        row_index: usize,
        assignment: Assignment,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let workspace = assignment.workspace.clone();
        let count = assignment.patterns.len();
        let mut row = v_flex()
            .id(("assignment", row_index))
            .test_support()
            .gap_2();
        if let Some(error) = assignment.error {
            let raw = self
                .model
                .read(cx)
                .doc()
                .get(&planeui_core::config_doc::KeyPath::from([
                    "workspace-to-monitor-force-assignment",
                    workspace.as_str(),
                ]))
                .map(ToString::to_string)
                .unwrap_or_default();
            return row
                .child(
                    h_flex()
                        .gap_4()
                        .child(div().w_24().child(workspace))
                        .child(raw)
                        .child(div().text_color(cx.theme().danger).child(error.to_string())),
                )
                .into_any_element();
        }
        let chips = assignment
            .patterns
            .iter()
            .enumerate()
            .map(|(index, pattern)| {
                Self::pattern_chip(&workspace, index, count, pattern, cx).into_any_element()
            })
            .collect::<Vec<_>>();
        let add_workspace = workspace.clone();
        let remove_workspace = workspace.clone();
        let preview = match self.live_data.read(cx).state() {
            LiveState::Ready { monitors, .. } => Some(
                monitors::resolve(&assignment.patterns, monitors)
                    .map_or_else(|| "No match".to_owned(), |monitor| monitor.name.clone()),
            ),
            _ => None,
        };
        row = row.child(
            h_flex()
                .gap_4()
                .items_start()
                .child(div().w_24().child(workspace.clone()))
                .child(
                    v_flex().flex_1().min_w_0().gap_2().children(chips).child(
                        Button::new("add-pattern")
                            .label("Add pattern")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_pattern(add_workspace.clone(), None, window, cx);
                            })),
                    ),
                )
                .when_some(preview, |row, preview| {
                    row.child(
                        div()
                            .id("monitor-preview")
                            .test_support()
                            .w_32()
                            .aria_label(preview.clone())
                            .child(preview),
                    )
                })
                .child(
                    Button::new("remove-assignment")
                        .icon(IconName::Delete)
                        .accessibility_label("Remove assignment")
                        .tooltip("Remove assignment")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.editor = None;
                            this.edit(
                                "Remove monitor assignment",
                                |doc| monitors::remove_assignment(doc, &remove_workspace),
                                cx,
                            );
                        })),
                ),
        );
        if let Some(editor) = &self.editor
            && editor.workspace == workspace
        {
            row = row.child(Self::pattern_editor(editor, cx));
        }
        row.into_any_element()
    }

    fn pattern_chip(
        workspace: &str,
        index: usize,
        count: usize,
        pattern: &MonitorPattern,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let edit_workspace = workspace.to_owned();
        let up_workspace = workspace.to_owned();
        let down_workspace = workspace.to_owned();
        let remove_workspace = workspace.to_owned();
        h_flex()
            .gap_1()
            .child(
                Button::new(("edit-pattern", index))
                    .label(pattern.format())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_pattern(edit_workspace.clone(), Some(index), window, cx);
                    })),
            )
            .child(
                Button::new(("pattern-up", index))
                    .icon(IconName::ChevronUp)
                    .accessibility_label("Move up")
                    .tooltip("Move up")
                    .disabled(index == 0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.editor = None;
                        this.edit(
                            "Move monitor pattern up",
                            |doc| {
                                monitors::move_pattern(
                                    doc,
                                    &up_workspace,
                                    index,
                                    index.saturating_sub(1),
                                )
                            },
                            cx,
                        );
                    })),
            )
            .child(
                Button::new(("pattern-down", index))
                    .icon(IconName::ChevronDown)
                    .accessibility_label("Move down")
                    .tooltip("Move down")
                    .disabled(index + 1 == count)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.editor = None;
                        this.edit(
                            "Move monitor pattern down",
                            |doc| monitors::move_pattern(doc, &down_workspace, index, index + 1),
                            cx,
                        );
                    })),
            )
            .child(
                Button::new(("remove-pattern", index))
                    .icon(IconName::Close)
                    .accessibility_label("Remove pattern")
                    .tooltip("Remove pattern")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.editor = None;
                        this.edit(
                            "Remove monitor pattern",
                            |doc| monitors::remove_pattern(doc, &remove_workspace, index),
                            cx,
                        );
                    })),
            )
    }

    fn pattern_editor(editor: &PatternEditor, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .child(div().flex_1().child(editor.picker.clone()))
            .child(
                Button::new("cancel-pattern")
                    .label("Close")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.editor = None;
                        cx.notify();
                    })),
            )
    }

    fn persistent_rows(names: Vec<String>, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let count = names.len();
        names
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(name))
                    .child(
                        Button::new(("workspace-up", index))
                            .label("Move up")
                            .disabled(index == 0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(
                                    "Move persistent workspace up",
                                    |doc| {
                                        move_persistent_workspace(
                                            doc,
                                            index,
                                            index.saturating_sub(1),
                                        )
                                    },
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new(("workspace-down", index))
                            .label("Move down")
                            .disabled(index + 1 == count)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(
                                    "Move persistent workspace down",
                                    |doc| move_persistent_workspace(doc, index, index + 1),
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new(("workspace-remove", index))
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(
                                    "Remove persistent workspace",
                                    |doc| remove_persistent_workspace(doc, index),
                                    cx,
                                );
                            })),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for WorkspacesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.model.read(cx).view();
        let version_two = view.general.config_version.value() == Some(&2);
        let version_label = match &view.general.config_version {
            Field::Invalid { raw, .. } => format!("config-version {raw}"),
            field => format!("config-version {}", field.value().copied().unwrap_or(1)),
        };
        let field = view.persistent_workspaces.clone();
        let names = field.value().cloned().unwrap_or_default();
        let rows = Self::persistent_rows(names, cx);
        v_flex()
            .id("workspaces-form")
            .overflow_y_scroll()
            .gap_4()
            .child(div().text_lg().child("Monitor assignments"))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("assignment-name")
                            .test_support()
                            .w_48()
                            .child(Input::new(&self.assignment_input).aria_label("Workspace name")),
                    )
                    .child(
                        Button::new("add-assignment")
                            .label("Add assignment")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_assignment(window, cx)),
                            ),
                    ),
            )
            .children(self.assignment_rows(cx))
            .when(version_two, |view| {
                view.child(div().text_lg().child("Persistent workspaces"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("workspace-name")
                                    .test_support()
                                    .flex_1()
                                    .child(Input::new(&self.input).aria_label("Workspace name")),
                            )
                            .child(Button::new("add-workspace").label("Add").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.add(window, cx);
                                }),
                            )),
                    )
                    .children(rows)
                    .when_some(
                        match field {
                            Field::Invalid { raw, .. } => Some(raw),
                            _ => None,
                        },
                        |view, raw| view.child(div().text_color(cx.theme().danger).child(raw)),
                    )
            })
            .when(!version_two, |view| view.child(version_label))
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_color(cx.theme().danger).child(error))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Fixture, in_window, open_window};
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt as _;
    use planeui_core::aerospace::{FakeAerospace, Monitor};

    #[gpui_kit::test]
    fn unsupported_assignments_are_read_only(cx: &mut TestAppContext) {
        let original = "[workspace-to-monitor-force-assignment]\nA = true # keep\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            WorkspacesView::new(model, live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("edit-pattern", 0_usize)).is_none());
            assert!(window.try_find("add-pattern").is_none());
            assert!(window.try_find("remove-assignment").is_none());
        });
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
    }

    #[gpui_kit::test]
    fn regex_warnings_do_not_block_pattern_commits(cx: &mut TestAppContext) {
        let fixture = Fixture::new("[workspace-to-monitor-force-assignment]\nA = 'main'\n");
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            WorkspacesView::new(model, live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.click(("edit-pattern", 0_usize), cx);
            window.click("monitor-pattern", cx);
            window.press("cmd-a", cx);
            window.input("[", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[workspace-to-monitor-force-assignment]\nA = \"[\"\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.find("monitor-warning").visible());
        });
    }

    #[gpui_kit::test]
    fn monitor_name_is_escaped_and_preview_tracks_edits(cx: &mut TestAppContext) {
        let original = "[workspace-to-monitor-force-assignment]\nA = 'main' # keep\n";
        let fixture = Fixture::with_aerospace(
            original,
            FakeAerospace::new().with_monitors(vec![
                Monitor {
                    id: 2,
                    name: "LG ULTRAGEAR+ (1)".into(),
                    is_main: false,
                },
                Monitor {
                    id: 1,
                    name: "Built-in".into(),
                    is_main: true,
                },
            ]),
        );
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            WorkspacesView::new(model.clone(), live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("monitor-preview").label(), Some("Built-in"));
            window.click(("edit-pattern", 0_usize), cx);
            window.within("monitor-choices").click("input", cx);
            window.input("LG ULTRAGEAR", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| window.press("enter", cx));
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[workspace-to-monitor-force-assignment]\nA = '^LG ULTRAGEAR\\+ \\(1\\)$' # keep\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("monitor-preview").label(),
                Some("LG ULTRAGEAR+ (1)")
            );
            window.click("monitor-pattern", cx);
            window.press("cmd-a", cx);
            window.input("unplugged", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("monitor-preview").label(), Some("No match"));
        });
        assert_eq!(fixture.aerospace.reload_count(), 2);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("monitor-preview").label(),
                Some("LG ULTRAGEAR+ (1)")
            );
        });
    }

    #[gpui_kit::test]
    fn unavailable_aerospace_has_only_manual_pattern_entry(cx: &mut TestAppContext) {
        let original = "[workspace-to-monitor-force-assignment]\nA = 'main'\n";
        let fixture = Fixture::with_aerospace(original, FakeAerospace::new().with_running(false));
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            WorkspacesView::new(model, live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.click(("edit-pattern", 0_usize), cx);
            window.render_frame(cx);
            assert!(window.try_find("monitor-choices").is_none());
            assert!(window.try_find("monitor-preview").is_none());
            window.click("monitor-pattern", cx);
            window.press("cmd-a", cx);
            window.input("0", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        in_window(cx, window, |window, cx| {
            window.press("cmd-a", cx);
            window.input("dell.*", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[workspace-to-monitor-force-assignment]\nA = \"dell.*\"\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
    }

    #[gpui_kit::test]
    fn assignments_and_patterns_add_reorder_remove_and_undo(cx: &mut TestAppContext) {
        let original =
            "[workspace-to-monitor-force-assignment]\nA = ['main', 'secondary'] # keep\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            WorkspacesView::new(model.clone(), live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.click(("pattern-down", 0_usize), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            original.replace("'main', 'secondary'", "'secondary', 'main'")
        );
        in_window(cx, window, |window, cx| {
            window.click(("remove-pattern", 1_usize), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[workspace-to-monitor-force-assignment]\nA = ['secondary'] # keep\n"
        );
        in_window(cx, window, |window, cx| {
            window.click("add-pattern", cx);
            window.click("monitor-pattern", cx);
            window.input("main", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[workspace-to-monitor-force-assignment]\nA = ['secondary', \"main\"] # keep\n"
        );
        in_window(cx, window, |window, cx| {
            window.click("assignment-name", cx);
            window.input("B", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert!(fixture.disk().contains("B = \"main\""));
        in_window(cx, window, |window, cx| {
            window
                .within(("assignment", 1_usize))
                .click("remove-assignment", cx);
        });
        cx.run_until_parked();
        assert!(!fixture.disk().contains("B ="));
        assert_eq!(fixture.aerospace.reload_count(), 5);
        for _ in 0..5 {
            model.update(cx, ConfigModel::undo);
            cx.run_until_parked();
        }
        assert_eq!(fixture.disk(), original);
    }

    #[gpui_kit::test]
    fn add_move_remove_and_undo_use_pipeline(cx: &mut TestAppContext) {
        let original = "config-version = 2\npersistent-workspaces = ['A', 'B'] # keep\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            WorkspacesView::new(model.clone(), live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.click("workspace-name", cx);
            window.input("C", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "config-version = 2\npersistent-workspaces = ['A', 'B', \"C\"] # keep\n"
        );
        in_window(cx, window, |window, cx| {
            window.click(("workspace-up", 2_usize), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "config-version = 2\npersistent-workspaces = ['A', \"C\", 'B'] # keep\n"
        );
        in_window(cx, window, |window, cx| {
            window.click(("workspace-remove", 0_usize), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "config-version = 2\npersistent-workspaces = [\"C\", 'B'] # keep\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 3);
        for _ in 0..3 {
            model.update(cx, ConfigModel::undo);
            cx.run_until_parked();
        }
        assert_eq!(fixture.disk(), original);
    }

    #[gpui_kit::test]
    fn version_one_never_offers_an_invalid_list(cx: &mut TestAppContext) {
        let fixture = Fixture::new("");
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            WorkspacesView::new(model, live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("add-workspace").is_none());
        });
        assert_eq!(fixture.aerospace.reload_count(), 0);
    }
}
