//! The ordered version 2 persistent-workspace list.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    TestSupportExt as _, Window, div,
};
use planeui_core::commands::WorkspaceName;
use planeui_core::config_doc::ConfigDoc;
use planeui_core::config_view::Field;
use planeui_core::settings::{
    WorkspaceEditError, add_persistent_workspace, move_persistent_workspace,
    remove_persistent_workspace,
};

use crate::config_model::ConfigModel;

#[derive(Debug)]
pub struct WorkspacesView {
    model: Entity<ConfigModel>,
    input: Entity<InputState>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl WorkspacesView {
    pub fn new(model: Entity<ConfigModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name"));
        let subscriptions = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
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
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn edit(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut ConfigDoc) -> Result<(), WorkspaceEditError>,
        cx: &mut Context<Self>,
    ) {
        let mut doc = self.model.read(cx).doc().clone();
        self.error = change(&mut doc).err().map(|error| error.to_string().into());
        if self.error.is_none() {
            self.model.update(cx, |model, cx| {
                model.commit(doc, label, cx);
            });
        }
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
        let count = names.len();
        let rows = names
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
            })
            .collect::<Vec<_>>();
        v_flex()
            .id("workspaces-form")
            .overflow_y_scroll()
            .gap_4()
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

    #[gpui_kit::test]
    fn add_move_remove_and_undo_use_pipeline(cx: &mut TestAppContext) {
        let original = "config-version = 2\npersistent-workspaces = ['A', 'B'] # keep\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let (window, _) = open_window(cx, |window, cx| {
            WorkspacesView::new(model.clone(), window, cx)
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
        let (window, _) = open_window(cx, |window, cx| WorkspacesView::new(model, window, cx));
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("add-workspace").is_none());
        });
        assert_eq!(fixture.aerospace.reload_count(), 0);
    }
}
