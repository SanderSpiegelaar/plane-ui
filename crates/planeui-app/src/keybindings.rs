//! Bindings by mode, with lossless row actions through the config model.

use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, Keystroke,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, TestSupportExt as _, Window, div,
};
use planeui_core::bindings::{
    RemoveModeError, add_mode, duplicate_binding, remove_binding, remove_mode, upsert_binding,
};
use planeui_core::commands::{Command, ParsedScript};
use planeui_core::config_doc::{ConfigDoc, KeyPath};
use planeui_core::config_view::{Binding, Mode};
use planeui_core::keys::ModeName;
use planeui_core::validate::{IssueKind, check_chord_available};

use crate::binding_draft::{BindingDraft, CommandDraft};
use crate::binding_form::{BindingForm, FormEvent};
use crate::config_model::{ConfigModel, SaveStatus};

#[derive(Action, Debug, Clone, PartialEq, Eq)]
#[action(namespace = planeui, no_json)]
enum BindingAction {
    Delete {
        mode: String,
        raw_key: String,
    },
    Duplicate {
        mode: String,
        raw_key: String,
        target: String,
    },
}

/// Browses the model's bindings without keeping a second copy of the document.
#[derive(Debug)]
pub struct KeybindingsView {
    model: Entity<ConfigModel>,
    selected_mode: String,
    error: Option<SharedString>,
    form: Option<Entity<BindingForm>>,
    form_subscription: Option<Subscription>,
    mode_input: Option<Entity<InputState>>,
    mode_input_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl KeybindingsView {
    fn apply_action(
        &mut self,
        action: &BindingAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            BindingAction::Delete { mode, raw_key } => self.delete(mode, raw_key, window, cx),
            BindingAction::Duplicate {
                mode,
                raw_key,
                target,
            } => {
                self.duplicate(mode, raw_key, target, window, cx);
            }
        }
    }

    /// Starts on main and follows changes to the config, including external reloads.
    pub fn has_invalid_field(&self, cx: &App) -> bool {
        self.error.is_some()
            || self
                .form
                .as_ref()
                .is_some_and(|form| form.read(cx).has_invalid_field(cx))
    }

    pub fn new(model: Entity<ConfigModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe_in(&model, window, |this, _, window, cx| {
            let model = this.model.read(cx);
            if !model
                .view()
                .modes
                .iter()
                .any(|mode| mode.name.value == this.selected_mode)
            {
                "main".clone_into(&mut this.selected_mode);
                this.form = None;
                this.form_subscription = None;
            }
            if let SaveStatus::Error(message) = model.status()
                && this.error.as_deref() != Some(message.as_str())
            {
                this.show_error(message.clone(), window, cx);
            }
            cx.notify();
        });
        Self {
            model,
            selected_mode: "main".to_owned(),
            error: None,
            form: None,
            form_subscription: None,
            mode_input: None,
            mode_input_subscription: None,
            _subscriptions: vec![subscription],
        }
    }

    fn start_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Mode name"));
        self.mode_input_subscription =
            Some(
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.create_mode(window, cx);
                    } else if matches!(event, InputEvent::Change) {
                        this.error = None;
                        cx.notify();
                    }
                }),
            );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.mode_input = Some(input);
        self.error = None;
        cx.notify();
    }

    fn create_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.mode_input else {
            return;
        };
        let name = match ModeName::new(input.read(cx).value().to_string()) {
            Ok(name) => name,
            Err(error) => {
                self.show_error(error.to_string(), window, cx);
                return;
            }
        };
        let mut doc = self.model.read(cx).doc().clone();
        if let Err(error) = add_mode(&mut doc, &name) {
            self.show_error(error.to_string(), window, cx);
            return;
        }
        self.commit(doc, format!("Add mode {name}"), window, cx);
        if self.error.is_none() {
            self.selected_mode = name.to_string();
            self.mode_input = None;
            self.mode_input_subscription = None;
            self.form = None;
            self.form_subscription = None;
        }
        cx.notify();
    }

    fn delete_mode(&mut self, raw_name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = match ModeName::new(raw_name) {
            Ok(name) => name,
            Err(error) => {
                self.show_error(error.to_string(), window, cx);
                return;
            }
        };
        let mut doc = self.model.read(cx).doc().clone();
        match remove_mode(&mut doc, &name) {
            Ok(()) => self.commit(doc, format!("Delete mode {name}"), window, cx),
            Err(RemoveModeError::Blocked { referencing }) => {
                let bindings = referencing
                    .iter()
                    .map(|(mode, key)| format!("{mode}: {key}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.show_error(
                    format!("Cannot delete {name}. Referenced by:\n{bindings}"),
                    window,
                    cx,
                );
            }
            Err(error) => self.show_error(error.to_string(), window, cx),
        }
    }

    fn render_mode_tabs(&self, selected: usize, cx: &Context<Self>) -> TabBar {
        let names = self
            .model
            .read(cx)
            .view()
            .modes
            .iter()
            .map(|mode| mode.name.value.clone())
            .collect::<Vec<_>>();
        TabBar::new("binding-modes")
            .selected_index(selected)
            .children(
                names
                    .iter()
                    .enumerate()
                    .map(|(index, name)| self.render_mode_tab(name, index, cx)),
            )
            .when(self.mode_input.is_none(), |tabs| {
                tabs.child(Tab::new().label("+").aria_label("Add mode"))
            })
            .when_some(self.mode_input.clone(), |tabs, input| {
                tabs.suffix(
                    div()
                        .id("new-mode")
                        .w_40()
                        .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                            if event.keystroke.key == "escape" {
                                this.mode_input = None;
                                this.mode_input_subscription = None;
                                this.error = None;
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .child(Input::new(&input)),
                )
            })
            .on_click(cx.listener(move |this, index: &usize, window, cx| {
                if let Some(name) = names.get(*index) {
                    this.selected_mode.clone_from(name);
                    this.form = None;
                    this.form_subscription = None;
                    cx.notify();
                } else {
                    this.start_mode(window, cx);
                }
            }))
    }

    fn render_mode_tab(&self, name: &str, index: usize, cx: &Context<Self>) -> Tab {
        let warning = self.model.read(cx).report().issues.iter().find(|issue| {
            issue.kind == IssueKind::NoWayBack
                && issue.key_path == Some(KeyPath::from(["mode", name]))
        });
        let target = name.to_owned();
        let view = cx.weak_entity();
        let label = h_flex()
            .id(("mode-tab", index))
            .gap_1()
            .child(name.to_owned())
            .when_some(warning, |label, issue| {
                let message = issue.message.clone();
                label
                    .child(Icon::new(IconName::TriangleAlert).text_color(cx.theme().warning))
                    .tooltip(move |window, cx| Tooltip::new(message.clone()).build(window, cx))
            })
            .context_menu(move |menu, _, _| {
                let view = view.clone();
                let target = target.clone();
                menu.item(
                    PopupMenuItem::new("Delete mode")
                        .disabled(target == "main")
                        .on_click(move |_, window, cx| {
                            if let Err(error) =
                                view.update(cx, |this, cx| this.delete_mode(&target, window, cx))
                            {
                                log::debug!("keybindings closed before deleting mode: {error}");
                            }
                        }),
                )
            });
        Tab::new()
            .aria_label(warning.map_or_else(
                || name.to_owned(),
                |issue| format!("{name}: {}", issue.message),
            ))
            .child(label)
    }

    fn open_form(
        &mut self,
        binding: Option<&Binding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = match binding {
            Some(Binding {
                raw_key,
                chord: Ok(chord),
                script: ParsedScript::Editable { commands, .. },
                ..
            }) => BindingDraft {
                original_raw_key: Some(raw_key.clone()),
                modifiers: chord.modifiers.as_slice().to_vec(),
                key: Some(chord.key.as_str().to_owned()),
                commands: match commands.iter().map(CommandDraft::new).collect() {
                    Ok(commands) => commands,
                    Err(error) => {
                        self.show_error(error.0, window, cx);
                        return;
                    }
                },
            },
            Some(_) => return,
            None => BindingDraft::default(),
        };
        let model = self.model.clone();
        let mode = self.selected_mode.clone();
        let form = cx.new(|cx| BindingForm::new(model, mode.clone(), draft, window, cx));
        self.form_subscription = Some(cx.subscribe_in(
            &form,
            window,
            move |this, _, event: &FormEvent, window, cx| match event {
                FormEvent::Cancel => {
                    this.form = None;
                    this.form_subscription = None;
                    cx.notify();
                }
                FormEvent::Commit(draft) => {
                    this.apply_draft(draft, &mode, window, cx);
                }
            },
        ));
        self.error = None;
        self.form = Some(form);
        cx.notify();
    }

    fn apply_draft(
        &mut self,
        draft: &BindingDraft,
        mode: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.read(cx);
        let (chord, commands) = match draft.to_commit(model.view(), mode) {
            Ok(commit) => commit,
            Err(error) => {
                self.show_error(error.0, window, cx);
                return;
            }
        };
        let mut doc = model.doc().clone();
        if let Err(error) = upsert_binding(
            &mut doc,
            mode,
            draft.original_raw_key.as_deref(),
            &chord,
            &commands,
        ) {
            self.show_error(error.to_string(), window, cx);
            return;
        }
        self.commit(doc, "Apply binding".into(), window, cx);
        if self.error.is_none() {
            self.form = None;
            self.form_subscription = None;
        }
    }

    fn show_error(&mut self, message: String, window: &mut Window, cx: &mut Context<Self>) {
        self.error = Some(message.clone().into());
        window.push_notification(Notification::error(message).title("Change not saved"), cx);
        cx.notify();
    }

    fn delete(&mut self, mode: &str, raw_key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut doc = self.model.read(cx).doc().clone();
        match remove_binding(&mut doc, mode, raw_key) {
            Ok(()) => self.commit(doc, format!("Delete {raw_key}"), window, cx),
            Err(error) => self.show_error(error.to_string(), window, cx),
        }
    }

    fn duplicate(
        &mut self,
        from_mode: &str,
        raw_key: &str,
        to_mode: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.read(cx);
        let binding = model
            .view()
            .modes
            .iter()
            .find(|mode| mode.name.value == from_mode)
            .and_then(|mode| {
                mode.bindings
                    .iter()
                    .find(|binding| binding.raw_key == raw_key)
            });
        if let Some(Ok(chord)) = binding.map(|binding| &binding.chord)
            && let Err(conflict) = check_chord_available(model.view(), to_mode, chord, None)
        {
            let commands = binding_commands(model.doc(), to_mode, conflict.binding);
            let message = format!(
                "{} is already bound to `{commands}` in {}",
                conflict.binding.raw_key, conflict.mode
            );
            self.show_error(message, window, cx);
            return;
        }
        let mut doc = model.doc().clone();
        match duplicate_binding(&mut doc, from_mode, raw_key, to_mode) {
            Ok(()) => self.commit(doc, format!("Duplicate {raw_key} to {to_mode}"), window, cx),
            Err(error) => self.show_error(error.to_string(), window, cx),
        }
    }

    fn commit(
        &mut self,
        edited: ConfigDoc,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        self.model
            .update(cx, |model, cx| model.commit(edited, label, cx));
        let errors = self
            .model
            .read(cx)
            .report()
            .errors()
            .map(|issue| issue.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !errors.is_empty() {
            self.show_error(errors, window, cx);
        }
        cx.notify();
    }

    fn render_duplicate(
        &self,
        mode: &Mode,
        binding: &Binding,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let model = self.model.read(cx);
        let source = mode.name.value.clone();
        let raw_key = binding.raw_key.clone();
        let targets = model
            .view()
            .modes
            .iter()
            .filter(|target| target.name.value != source)
            .map(|target| target.name.value.clone())
            .collect::<Vec<_>>();
        let view = cx.weak_entity();
        if targets.is_empty() {
            return None;
        }
        Some(
            Button::new(("duplicate", index))
                .small()
                .label("Duplicate to mode…")
                .dropdown_menu(move |mut menu, _, _| {
                    for target in &targets {
                        let view = view.clone();
                        let source = source.clone();
                        let raw_key = raw_key.clone();
                        let target = target.clone();
                        menu = menu.item(PopupMenuItem::new(target.clone()).on_click(
                            move |_, window, cx| {
                                if let Err(error) = view.update(cx, |this, cx| {
                                    this.apply_action(
                                        &BindingAction::Duplicate {
                                            mode: source.clone(),
                                            raw_key: raw_key.clone(),
                                            target: target.clone(),
                                        },
                                        window,
                                        cx,
                                    );
                                }) {
                                    log::debug!("keybindings closed before duplicating: {error}");
                                }
                            },
                        ));
                    }
                    menu
                }),
        )
    }

    fn render_binding(
        &self,
        mode: &Mode,
        binding: &Binding,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let commands = if mode.editable {
            binding_commands(self.model.read(cx).doc(), &mode.name.value, binding)
        } else {
            raw_binding(self.model.read(cx).doc(), &mode.name.value, binding)
        };
        let reason = if !mode.editable {
            Some("This mode's TOML structure cannot be edited safely".to_owned())
        } else if let ParsedScript::ReadOnly { reason } = &binding.script {
            Some(reason.clone())
        } else {
            binding.chord.as_ref().err().map(ToString::to_string)
        };
        let chips = binding.chord.as_ref().map_or_else(
            |_| vec![binding.raw_key.clone()],
            |chord| {
                chord
                    .modifiers
                    .as_slice()
                    .iter()
                    .map(ToString::to_string)
                    .chain(std::iter::once(chord.key.as_str().to_owned()))
                    .collect()
            },
        );
        let duplicate = self.render_duplicate(mode, binding, index, cx);
        let editable = reason.is_none();
        let edit_binding = binding.clone();
        let source = mode.name.value.clone();
        let raw_key = binding.raw_key.clone();
        ListItem::new(("binding", index))
            .accessibility_label(format!("{}: {commands}", binding.raw_key))
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .when(reason.is_some(), |row| {
                        row.text_color(cx.theme().muted_foreground)
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .gap_3()
                            .items_start()
                            .child(h_flex().w_32().flex_shrink_0().gap_1().children(
                                chips.into_iter().map(|key| {
                                    Kbd::new(Keystroke {
                                        key,
                                        ..Keystroke::default()
                                    })
                                    .outline()
                                }),
                            ))
                            .child(v_flex().flex_1().min_w_0().child(commands).when_some(
                                binding.inline_comment.clone(),
                                |column, comment| {
                                    column.child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(comment),
                                    )
                                },
                            ))
                            .children(duplicate)
                            .when(editable, |row| {
                                row.child(
                                    Button::new(("edit", index)).small().label("Edit").on_click(
                                        cx.listener(move |this, _, window, cx| {
                                            this.open_form(Some(&edit_binding), window, cx);
                                        }),
                                    ),
                                )
                            })
                            .child(
                                Button::new(("delete", index))
                                    .small()
                                    .label("Delete")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.apply_action(
                                            &BindingAction::Delete {
                                                mode: source.clone(),
                                                raw_key: raw_key.clone(),
                                            },
                                            window,
                                            cx,
                                        );
                                    })),
                            ),
                    )
                    .when_some(reason, |row, reason| {
                        row.child(
                            div()
                                .text_sm()
                                .child(format!("edit in a text editor: {reason}")),
                        )
                    }),
            )
    }
}

fn binding_commands(doc: &ConfigDoc, mode: &str, binding: &Binding) -> String {
    match &binding.script {
        ParsedScript::Editable { commands, .. } => commands
            .iter()
            .map(Command::format)
            .collect::<Vec<_>>()
            .join(", "),
        ParsedScript::ReadOnly { .. } => raw_binding(doc, mode, binding),
    }
}

fn raw_binding(doc: &ConfigDoc, mode: &str, binding: &Binding) -> String {
    doc.get(&KeyPath::from(["mode", mode, "binding", &binding.raw_key]))
        .map(|item| {
            let mut item = item.clone();
            if let Some(value) = item.as_value_mut() {
                value.decor_mut().clear();
            }
            item.to_string()
        })
        .unwrap_or_default()
}

impl Render for KeybindingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let modes = &self.model.read(cx).view().modes;
        let selected = modes
            .iter()
            .position(|mode| mode.name.value == self.selected_mode)
            .unwrap_or_default();
        let mode = modes.get(selected).cloned();
        let tabs = self.render_mode_tabs(selected, cx);
        let original = self
            .form
            .as_ref()
            .and_then(|form| form.read(cx).draft.original_raw_key.clone());
        let rows = mode.as_ref().map(|mode| {
            mode.bindings
                .iter()
                .enumerate()
                .map(|(index, binding)| {
                    if original.as_deref() == Some(binding.raw_key.as_str()) {
                        div().children(self.form.clone()).into_any_element()
                    } else {
                        self.render_binding(mode, binding, index, cx)
                            .into_any_element()
                    }
                })
                .collect::<Vec<_>>()
        });
        let empty = rows.as_ref().is_none_or(Vec::is_empty);
        v_flex()
            .id("keybindings")
            .on_action(cx.listener(Self::apply_action))
            .flex_1()
            .min_h_0()
            .gap_3()
            .child(tabs)
            .when(mode.as_ref().is_some_and(|mode| mode.editable), |view| {
                view.child(
                    Button::new("add-binding").label("Add binding").on_click(
                        cx.listener(|this, _, window, cx| this.open_form(None, window, cx)),
                    ),
                )
            })
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .id("binding-error")
                        .aria_label(error.clone())
                        .test_support()
                        .child(Alert::error("binding-alert", error)),
                )
            })
            .child(
                v_flex()
                    .id("binding-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(original.is_none(), |list| list.children(self.form.clone()))
                    .when(empty && self.form.is_none(), |list| {
                        list.child("No bindings in this mode.")
                    })
                    .children(rows.into_iter().flatten()),
            )
    }
}

#[cfg(test)]
mod tests {
    use crate::binding_form::{CancelBinding, CommitBinding};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AnyWindowHandle, TestAppContext};
    use planeui_core::keys::Modifier;

    use super::*;
    use crate::test_support::{Fixture, in_window, open_window};

    const CONFIG: &str = "# Bindings\n[mode.main.binding]\nalt-h = 'focus left' # left\nalt-shift-h = 'focus up'\n\n[mode.service.binding]\nshift-alt-h = 'focus right'\n\n[mode.free.binding]\n";

    fn open_view(
        cx: &mut TestAppContext,
        fixture: &Fixture,
    ) -> (
        AnyWindowHandle,
        Entity<KeybindingsView>,
        Entity<ConfigModel>,
    ) {
        let model = fixture.model(cx);
        let view_model = model.clone();
        let (window, view) = open_window(cx, |window, cx| {
            KeybindingsView::new(view_model, window, cx)
        });
        (window, view, model)
    }

    fn act(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<KeybindingsView>,
        action: &BindingAction,
    ) {
        in_window(cx, window, |window, cx| {
            view.update(cx, |view, cx| view.apply_action(action, window, cx));
        });
        cx.run_until_parked();
    }

    fn duplicate(raw_key: &str, target: &str) -> BindingAction {
        BindingAction::Duplicate {
            mode: "main".to_owned(),
            raw_key: raw_key.to_owned(),
            target: target.to_owned(),
        }
    }

    #[gpui_kit::test]
    fn enter_creates_and_selects_a_mode_and_escape_cancels(cx: &mut TestAppContext) {
        let fixture = Fixture::new("[mode.main.binding]\n");
        let (window, view, model) = open_view(cx, &fixture);
        in_window(cx, window, |window, cx| {
            window.within("binding-modes").click(1usize, cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.input("service", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[mode.main.binding]\n\n[mode.service.binding]\n"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.selected_mode.clone()),
            "service"
        );
        assert!(view.read_with(cx, |view, _| view.mode_input.is_none()));
        assert_eq!(fixture.aerospace.reload_count(), 1);
        assert!(model.read_with(cx, |model, _| {
            model
                .report()
                .issues
                .iter()
                .any(|issue| issue.kind == IssueKind::NoWayBack)
        }));
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.within("binding-modes").find(1usize).label(),
                Some("service: mode service has no binding back to main")
            );
        });
        let before = fixture.disk();
        in_window(cx, window, |window, cx| {
            view.update(cx, |view, cx| view.start_mode(window, cx));
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.input("cancelled", cx);
            window.press("escape", cx);
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.mode_input.is_none()));
        assert_eq!(fixture.disk(), before);
    }

    #[gpui_kit::test]
    fn invalid_and_duplicate_mode_names_do_not_write(cx: &mut TestAppContext) {
        let original = "[mode.main.binding]\n";
        let fixture = Fixture::new(original);
        let (window, view, _) = open_view(cx, &fixture);
        for (name, expected) in [
            ("", "the mode name is empty"),
            ("main", "mode.main already exists"),
        ] {
            in_window(cx, window, |window, cx| {
                view.update(cx, |view, cx| view.start_mode(window, cx));
            });
            cx.run_until_parked();
            in_window(cx, window, |window, cx| {
                if !name.is_empty() {
                    window.input(name, cx);
                }
                window.press("enter", cx);
            });
            cx.run_until_parked();
            assert_eq!(
                view.read_with(cx, |view, _| view.error.clone()).as_deref(),
                Some(expected)
            );
            assert_eq!(fixture.disk(), original);
            assert_eq!(fixture.aerospace.reload_count(), 0);
        }
    }

    #[gpui_kit::test]
    fn blocked_mode_deletion_shows_every_reference_and_never_writes(cx: &mut TestAppContext) {
        let original = "[mode.main.binding]\nalt-s='mode service'\nesc=['close', 'mode service']\n[mode.service.binding]\nesc='mode main'\n";
        let fixture = Fixture::new(original);
        let (window, view, _) = open_view(cx, &fixture);
        in_window(cx, window, |window, cx| {
            window.within("binding-modes").right_click(1usize, cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.within("popup-menu").click(0usize, cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        assert_eq!(fixture.snapshots(), Vec::<String>::new());
        assert_eq!(
            view.read_with(cx, |view, _| view.error.clone()).as_deref(),
            Some("Cannot delete service. Referenced by:\nmain: alt-s\nmain: esc")
        );
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("binding-error").label(),
                Some("Cannot delete service. Referenced by:\nmain: alt-s\nmain: esc")
            );
        });
    }

    #[gpui_kit::test]
    fn unreferenced_mode_deletion_is_undoable(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        let (window, view, model) = open_view(cx, &fixture);
        view.update(cx, |view, _| view.selected_mode = "free".into());
        in_window(cx, window, |window, cx| {
            view.update(cx, |view, cx| view.delete_mode("free", window, cx));
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            CONFIG.replace("\n[mode.free.binding]\n", "")
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.selected_mode.clone()),
            "main"
        );
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), CONFIG);
    }

    #[gpui_kit::test]
    fn delete_removes_only_the_binding_line_and_is_undoable(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        let (window, view, model) = open_view(cx, &fixture);
        let before = fixture.disk();

        act(
            cx,
            window,
            &view,
            &BindingAction::Delete {
                mode: "main".to_owned(),
                raw_key: "alt-h".to_owned(),
            },
        );

        assert_eq!(
            fixture.disk(),
            before.replace("alt-h = 'focus left' # left\n", "")
        );
        assert_eq!(fixture.snapshots(), vec![before.clone()]);
        assert_eq!(
            model.read_with(cx, |model, _| model.undo_label().map(str::to_owned)),
            Some("Delete alt-h".to_owned())
        );
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), before);
    }

    #[gpui_kit::test]
    fn duplicate_blocks_normalized_chord_conflicts_without_writing(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        let (window, view, _) = open_view(cx, &fixture);
        let before = fixture.disk();

        act(cx, window, &view, &duplicate("alt-shift-h", "service"));

        assert_eq!(fixture.disk(), before);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        assert_eq!(fixture.snapshots(), Vec::<String>::new());
        assert_eq!(
            view.read_with(cx, |view, _| view.error.clone()).as_deref(),
            Some("shift-alt-h is already bound to `focus right` in service")
        );
    }

    #[gpui_kit::test]
    fn duplicate_to_a_free_mode_copies_the_line_verbatim(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        let (window, view, _) = open_view(cx, &fixture);
        let before = fixture.disk();

        act(cx, window, &view, &duplicate("alt-h", "free"));

        assert_eq!(
            fixture.disk(),
            before.replace(
                "[mode.free.binding]\n",
                "[mode.free.binding]\nalt-h = 'focus left' # left\n"
            )
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
        assert!(view.read_with(cx, |view, _| view.error.is_none()));
    }

    #[gpui_kit::test]
    fn an_invalid_chord_still_checks_for_a_literal_key_collision(cx: &mut TestAppContext) {
        let original = "[mode.main.binding]\nnot-a-key = 'focus left'\n[mode.free.binding]\nnot-a-key = 'focus right'\n";
        let fixture = Fixture::new(original);
        let (window, view, _) = open_view(cx, &fixture);

        act(cx, window, &view, &duplicate("not-a-key", "free"));

        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        assert_eq!(
            view.read_with(cx, |view, _| view.error.clone()).as_deref(),
            Some("mode.free.binding.not-a-key already exists")
        );
    }

    #[gpui_kit::test]
    fn readonly_scripts_can_be_duplicated_and_deleted(cx: &mut TestAppContext) {
        let original = "[mode.main.binding]\nalt-h = 'focus left && focus down' # advanced\n[mode.free.binding]\n";
        let fixture = Fixture::new(original);
        let (window, view, _) = open_view(cx, &fixture);

        assert_eq!(
            view.read_with(cx, |view, cx| {
                let model = view.model.read(cx);
                let mode = model.view().modes.first().unwrap();
                binding_commands(
                    model.doc(),
                    &mode.name.value,
                    mode.bindings.first().unwrap(),
                )
            }),
            "'focus left && focus down'"
        );

        act(cx, window, &view, &duplicate("alt-h", "free"));
        assert_eq!(
            fixture.disk(),
            format!("{original}alt-h = 'focus left && focus down' # advanced\n")
        );
        act(
            cx,
            window,
            &view,
            &BindingAction::Delete {
                mode: "main".to_owned(),
                raw_key: "alt-h".to_owned(),
            },
        );
        assert_eq!(
            fixture.disk(),
            "[mode.main.binding]\n[mode.free.binding]\nalt-h = 'focus left && focus down' # advanced\n"
        );
    }

    #[gpui_kit::test]
    fn selected_mode_survives_updates_and_falls_back_when_removed(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        let (window, view, model) = open_view(cx, &fixture);
        view.update(cx, |view, cx| {
            view.selected_mode = "free".to_owned();
            cx.notify();
        });
        act(cx, window, &view, &duplicate("alt-h", "free"));
        assert_eq!(
            view.read_with(cx, |view, _| view.selected_mode.clone()),
            "free"
        );

        model.update(cx, |model, cx| {
            model
                .edit("Remove free mode", cx, |doc| {
                    doc.remove(&KeyPath::from(["mode", "free"]))
                })
                .unwrap();
        });
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view.selected_mode.clone()),
            "main"
        );
    }

    #[gpui_kit::test]
    fn failed_writes_reach_the_view(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        fixture.block_snapshots();
        let (window, view, _) = open_view(cx, &fixture);

        act(cx, window, &view, &duplicate("alt-h", "free"));

        assert_eq!(fixture.disk(), CONFIG);
        assert!(view.read_with(cx, |view, _| view.error.is_some()));
    }

    fn draft_form(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<KeybindingsView>,
        edit: bool,
    ) -> Entity<BindingForm> {
        in_window(cx, window, |window, cx| {
            view.update(cx, |view, cx| {
                if edit {
                    view.selected_mode = "service".into();
                }
                let binding = edit.then(|| {
                    view.model
                        .read(cx)
                        .view()
                        .modes
                        .iter()
                        .find(|mode| mode.name.value == "service")
                        .unwrap()
                        .bindings
                        .first()
                        .unwrap()
                        .clone()
                });
                view.open_form(binding.as_ref(), window, cx);
                view.form.clone().unwrap()
            })
        })
    }

    fn set_draft(cx: &mut TestAppContext, form: &Entity<BindingForm>, key: &str, command: &str) {
        form.update(cx, |form, cx| {
            form.draft.key = Some(key.into());
            form.draft.modifiers = vec![Modifier::Shift, Modifier::Alt];
            form.draft.commands =
                vec![CommandDraft::new(&Command::parse(command).unwrap()).unwrap()];
            cx.notify();
        });
    }

    #[gpui_kit::test]
    fn focused_form_enter_adds_one_canonical_line(cx: &mut TestAppContext) {
        let original = "[mode.main.binding]\n";
        let fixture = Fixture::new(original);
        let (window, view, _) = open_view(cx, &fixture);
        let form = draft_form(cx, window, &view, false);
        set_draft(cx, &form, "p", "workspace P");
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
        in_window(cx, window, |window, cx| window.press("enter", cx));
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[mode.main.binding]\nalt-shift-p = \"workspace P\"\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
        assert_eq!(fixture.snapshots(), vec![original]);
        assert!(view.read_with(cx, |view, _| view.form.is_none()));
    }

    #[gpui_kit::test]
    fn focused_commit_action_preserves_existing_modifier_order(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        let (window, view, _) = open_view(cx, &fixture);
        let form = draft_form(cx, window, &view, true);
        set_draft(cx, &form, "h", "workspace P");
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.dispatch_action(Box::new(CommitBinding), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            CONFIG.replace(
                "shift-alt-h = 'focus right'",
                "shift-alt-h = \"workspace P\""
            )
        );
    }

    #[gpui_kit::test]
    fn conflicting_form_action_never_writes_and_names_binding(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        let (window, view, _) = open_view(cx, &fixture);
        let form = draft_form(cx, window, &view, false);
        set_draft(cx, &form, "h", "workspace P");
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("draft-error").label(),
                Some("alt-shift-h is already bound to `focus up` in main")
            );
        });
        in_window(cx, window, |window, cx| {
            window.dispatch_action(Box::new(CommitBinding), cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), CONFIG);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        assert_eq!(fixture.snapshots(), Vec::<String>::new());
        assert_eq!(
            view.read_with(cx, |view, cx| form
                .read(cx)
                .draft
                .to_commit(view.model.read(cx).view(), "main")
                .unwrap_err()
                .0),
            "alt-shift-h is already bound to `focus up` in main"
        );
    }

    #[gpui_kit::test]
    fn escape_and_cancel_action_discard_draft_without_writing(cx: &mut TestAppContext) {
        let fixture = Fixture::new(CONFIG);
        let (window, view, _) = open_view(cx, &fixture);
        let form = draft_form(cx, window, &view, false);
        set_draft(cx, &form, "p", "workspace P");
        cx.run_until_parked();
        in_window(cx, window, |window, cx| window.press("escape", cx));
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.form.is_none()));
        let _form = draft_form(cx, window, &view, false);
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.dispatch_action(Box::new(CancelBinding), cx);
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.form.is_none()));
        assert_eq!(fixture.disk(), CONFIG);
        assert_eq!(fixture.aerospace.reload_count(), 0);
    }

    #[gpui_kit::test]
    fn escape_cancels_while_an_argument_input_has_focus(cx: &mut TestAppContext) {
        let original = "[mode.main.binding]\n[mode.service.binding]\nshift-alt-h = 'workspace P'\n";
        let fixture = Fixture::new(original);
        let (window, view, _) = open_view(cx, &fixture);
        let _form = draft_form(cx, window, &view, true);
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            for _ in 0..7 {
                window.press("tab", cx);
            }
            window.input("Q", cx);
            window.press("escape", cx);
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.form.is_none()));
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
    }

    #[gpui_kit::test]
    fn full_binding_flow_is_keyboard_operable_in_tab_order(cx: &mut TestAppContext) {
        let fixture = Fixture::new("[mode.main.binding]\n");
        let (window, view, _) = open_view(cx, &fixture);
        let form = draft_form(cx, window, &view, false);
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            for _ in 0..4 {
                window.press("tab", cx);
            }
            window.press("space", cx);
            window.press("shift-tab", cx);
            window.press("shift-tab", cx);
            window.press("space", cx);
        });
        assert_eq!(
            form.read_with(cx, |form, _| form.draft.modifiers.clone()),
            vec![Modifier::Shift, Modifier::Alt]
        );
        in_window(cx, window, |window, cx| {
            for _ in 0..3 {
                window.press("tab", cx);
            }
            window.press("enter", cx);
            window.input("p", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            let mut matches = planeui_core::keys::KEY_NAMES
                .iter()
                .copied()
                .filter(|name| name.to_lowercase().contains('p'))
                .collect::<Vec<_>>();
            matches.sort_unstable();
            for _ in 0..matches.iter().position(|name| *name == "p").unwrap() {
                window.press("down", cx);
            }
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            form.read_with(cx, |form, _| form.draft.key.clone()),
            Some("p".into())
        );
        in_window(cx, window, |window, cx| {
            window.press("tab", cx);
            window.press("space", cx);
        });
        cx.run_until_parked();
        assert_eq!(form.read_with(cx, |form, _| form.draft.commands.len()), 1);
        assert_eq!(fixture.disk(), "[mode.main.binding]\n");
        in_window(cx, window, |window, cx| {
            window.press("enter", cx);
            window.input("Workspace", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.press("enter", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.press("tab", cx);
            window.press("cmd-a", cx);
            window.input("P", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), "[mode.main.binding]\n");
        in_window(cx, window, |window, cx| {
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[mode.main.binding]\nalt-shift-p = \"workspace P\"\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
    }
}
