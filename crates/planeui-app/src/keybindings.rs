//! Bindings by mode, with lossless row actions through the config model.

use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::Button;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, Context, Entity, InteractiveElement as _, IntoElement, Keystroke, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use planeui_core::bindings::{duplicate_binding, remove_binding};
use planeui_core::commands::{Command, ParsedScript};
use planeui_core::config_doc::{ConfigDoc, KeyPath};
use planeui_core::config_view::{Binding, Mode};
use planeui_core::validate::check_chord_available;

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
            _subscriptions: vec![subscription],
        }
    }

    fn show_error(&mut self, message: String, window: &mut Window, cx: &mut Context<Self>) {
        self.error = Some(message.clone().into());
        window.push_notification(Notification::error(message).title("Binding not saved"), cx);
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
        // Binding operations have richer errors than ConfigModel's generic document edits.
        let result = self.model.update(cx, |model, cx| {
            model.edit(label, cx, |doc| {
                *doc = edited;
                Ok(())
            })
        });
        if let Err(error) = result {
            self.show_error(error.to_string(), window, cx);
        } else {
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
        }
        cx.notify();
    }

    fn render_duplicate(
        &self,
        mode: &Mode,
        binding: &Binding,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
        Button::new(("duplicate", index))
            .label("Duplicate to mode…")
            .disabled(targets.is_empty())
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
            })
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
                            .child(h_flex().w_40().flex_shrink_0().gap_1().children(
                                chips.into_iter().map(|key| {
                                    Kbd::new(Keystroke {
                                        key,
                                        ..Keystroke::default()
                                    })
                                    .outline()
                                }),
                            ))
                            .child(div().flex_1().min_w_0().child(commands))
                            .child(
                                div()
                                    .w_32()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(binding.inline_comment.clone().unwrap_or_default()),
                            )
                            .child(duplicate)
                            .child(Button::new(("delete", index)).label("Delete").on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.apply_action(
                                        &BindingAction::Delete {
                                            mode: source.clone(),
                                            raw_key: raw_key.clone(),
                                        },
                                        window,
                                        cx,
                                    );
                                }),
                            )),
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
        let names = modes
            .iter()
            .map(|mode| mode.name.value.clone())
            .collect::<Vec<_>>();
        let mode = modes.get(selected).cloned();
        let tabs = TabBar::new("binding-modes")
            .selected_index(selected)
            .children(names.iter().map(|name| Tab::new().label(name.clone())))
            .on_click(cx.listener(move |this, index: &usize, _, cx| {
                if let Some(name) = names.get(*index) {
                    this.selected_mode.clone_from(name);
                    cx.notify();
                }
            }));
        // PUI-036 adds the binding form between the mode tabs and these rows.
        let rows = mode.as_ref().map(|mode| {
            mode.bindings
                .iter()
                .enumerate()
                .map(|(index, binding)| self.render_binding(mode, binding, index, cx))
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
            .when_some(self.error.clone(), |view, error| {
                view.child(Alert::error("binding-error", error))
            })
            .child(
                v_flex()
                    .id("binding-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(empty, |list| list.child("No bindings in this mode."))
                    .children(rows.into_iter().flatten()),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AnyWindowHandle, TestAppContext};

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
}
