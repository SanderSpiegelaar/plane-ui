//! Widget state for a binding. Only the form's commit action can emit an edit.

use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, TestSupportExt as _, Window, actions, div,
};
use planeui_core::keys::{KEY_NAMES, Modifier};

use crate::binding_draft::BindingDraft;
use crate::command_form::{CommandEvent, CommandForm, CommitMode, Picker, picker};
use crate::config_model::ConfigModel;

actions!(planeui, [CommitBinding, CancelBinding]);
pub(crate) const FORM_CONTEXT: &str = "BindingForm";

#[derive(Debug, Clone)]
pub(crate) enum FormEvent {
    Commit(BindingDraft),
    Cancel,
}

#[derive(Debug)]
pub(crate) struct BindingForm {
    pub draft: BindingDraft,
    model: Entity<ConfigModel>,
    mode: String,
    pub focus: FocusHandle,
    key: Entity<Picker>,
    commands: Entity<CommandForm>,
    _subscriptions: Vec<Subscription>,
    error: Option<String>,
}

impl EventEmitter<FormEvent> for BindingForm {}

impl BindingForm {
    pub fn new(
        model: Entity<ConfigModel>,
        mode: String,
        draft: BindingDraft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut keys: Vec<SharedString> = KEY_NAMES.iter().map(|key| (*key).into()).collect();
        keys.extend(
            model
                .read(cx)
                .view()
                .key_mapping
                .aliases
                .iter()
                .map(|(name, _)| SharedString::from(name.clone())),
        );
        keys.sort();
        keys.dedup();
        let key = picker(keys, draft.key.as_deref(), window, cx);
        let commands = cx.new(|cx| {
            CommandForm::new(
                model.clone(),
                draft.commands.clone(),
                CommitMode::Explicit,
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe(
                &key,
                |this: &mut Self, _, event: &SelectEvent<SearchableVec<SharedString>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    this.draft.key = value.as_ref().map(ToString::to_string);
                    this.error = None;
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &commands,
                window,
                |this: &mut Self, _, event: &CommandEvent, window, cx| {
                    if let CommandEvent::Changed(commands) = event {
                        this.draft.commands.clone_from(commands);
                        this.error = None;
                        cx.notify();
                    } else if matches!(event, CommandEvent::Submit) {
                        this.commit(&CommitBinding, window, cx);
                    } else if matches!(event, CommandEvent::FocusFallback) {
                        this.key.update(cx, |picker, cx| picker.focus(window, cx));
                    }
                },
            ),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            draft,
            model,
            mode,
            focus,
            key,
            commands,
            _subscriptions: subscriptions,
            error: None,
        }
    }

    pub fn commit(&mut self, _: &CommitBinding, _: &mut Window, cx: &mut Context<Self>) {
        match self.draft.to_commit(self.model.read(cx).view(), &self.mode) {
            Ok(_) => cx.emit(FormEvent::Commit(self.draft.clone())),
            Err(error) => self.error = Some(error.0),
        }
        cx.notify();
    }

    fn cancel(&mut self, _: &CancelBinding, _: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        cx.emit(FormEvent::Cancel);
    }
}

impl Render for BindingForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let result = self.draft.to_commit(self.model.read(cx).view(), &self.mode);
        let conflict = self
            .draft
            .key
            .as_ref()
            .and_then(|_| {
                self.draft
                    .chord(self.model.read(cx).view(), &self.mode)
                    .err()
            })
            .map(|error| error.0);
        v_flex()
            .id("binding-form")
            .role(Role::Group)
            .aria_label("Binding")
            .tab_group()
            .key_context(FORM_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::commit))
            .on_action(cx.listener(Self::cancel))
            .p_3()
            .gap_3()
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_3()
                    .children(
                        [
                            Modifier::Cmd,
                            Modifier::Alt,
                            Modifier::Ctrl,
                            Modifier::Shift,
                        ]
                        .into_iter()
                        .map(|modifier| {
                            Checkbox::new(modifier.as_str())
                                .label(modifier.as_str())
                                .checked(self.draft.modifiers.contains(&modifier))
                                .on_click(cx.listener(move |this, checked, _, cx| {
                                    this.draft.modifiers.retain(|value| *value != modifier);
                                    if *checked {
                                        this.draft.modifiers.push(modifier);
                                    }
                                    this.error = None;
                                    cx.notify();
                                }))
                        }),
                    )
                    .child(
                        Select::new(&self.key)
                            .placeholder("Key")
                            .accessibility_label("Key"),
                    ),
            )
            .when_some(conflict.or_else(|| self.error.clone()), |row, error| {
                row.child(
                    div()
                        .id("draft-error")
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(self.commands.clone())
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("commit-binding")
                            .label("Apply binding")
                            .disabled(result.is_err())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.commit(&CommitBinding, window, cx);
                            })),
                    )
                    .child(Button::new("cancel-binding").label("Cancel").on_click(
                        cx.listener(|this, _, window, cx| this.cancel(&CancelBinding, window, cx)),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding_draft::CommandDraft;
    use crate::test_support::{Fixture, in_window, open_window};
    use gpui_kit::{TestAppContext, test::TestWindowExt as _};
    use planeui_core::commands::Command;

    #[gpui_kit::test]
    fn resize_stepper_keeps_relative_amount_and_only_changes_the_draft(cx: &mut TestAppContext) {
        let original = "[mode.main.binding]\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let draft = BindingDraft {
            commands: vec![
                CommandDraft::new(&Command::parse("resize smart +50").unwrap()).unwrap(),
            ],
            ..BindingDraft::default()
        };
        let (window, form) = open_window(cx, |window, cx| {
            BindingForm::new(model, "main".into(), draft, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.within(("number", 1_usize)).click("increment", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            form.read_with(cx, |form, _| form
                .draft
                .commands
                .first()
                .unwrap()
                .to_command()
                .unwrap()
                .format()),
            "resize smart +51"
        );
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
    }

    #[gpui_kit::test]
    fn removing_the_last_command_returns_focus_to_the_key(cx: &mut TestAppContext) {
        use gpui_kit::Focusable as _;
        let fixture = Fixture::new("[mode.main.binding]\n");
        let model = fixture.model(cx);
        let draft = BindingDraft {
            key: Some("p".into()),
            commands: vec![
                CommandDraft::new(&Command::parse("workspace-back-and-forth").unwrap()).unwrap(),
            ],
            ..BindingDraft::default()
        };
        let (window, form) = open_window(cx, |window, cx| {
            BindingForm::new(model, "main".into(), draft, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.click(("remove-command", 0_usize), cx);
        });
        cx.run_until_parked();
        let focus = form.read_with(cx, |form, cx| form.key.read(cx).focus_handle(cx));
        in_window(cx, window, |window, _| assert!(focus.is_focused(window)));
        assert_eq!(fixture.aerospace.reload_count(), 0);
    }
}
