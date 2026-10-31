//! General settings backed by the typed core catalogue.

use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, TestSupportExt as _, Window, div,
};
use planeui_core::config_doc::line_and_column;
use planeui_core::config_view::Field;
use planeui_core::settings::{
    Setting, SettingValue, inferred_persistent_workspaces, reset_setting, set_setting,
    upgrade_config_version,
};

use crate::config_model::ConfigModel;
use crate::fields::{CommitField, CommitNumberField, Committed};
use crate::gaps_view::unsigned_number;

type Picker = SelectState<SearchableVec<SharedString>>;

#[derive(Debug, Clone, Copy)]
pub struct AutoReloadEnabled;

#[derive(Debug)]
pub struct GeneralView {
    model: Entity<ConfigModel>,
    padding: Entity<CommitNumberField>,
    pickers: Vec<(Setting, Entity<Picker>)>,
    prefill: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AutoReloadEnabled> for GeneralView {}

fn effective(model: &ConfigModel, setting: Setting) -> SettingValue {
    model
        .view()
        .general
        .setting(setting)
        .value()
        .copied()
        .unwrap_or(setting.spec().default)
}

fn padding(model: &ConfigModel) -> i64 {
    match effective(model, Setting::AccordionPadding) {
        SettingValue::Count(value) => i64::from(value),
        _ => 0,
    }
}

fn label(setting: Setting) -> &'static str {
    match setting {
        Setting::StartAtLogin => "Start at login",
        Setting::AutoReloadConfig => "Auto-reload config",
        Setting::AccordionPadding => "Accordion padding",
        Setting::RootLayout => "Root layout",
        Setting::RootOrientation => "Root orientation",
        Setting::FlattenContainers => "Flatten containers",
        Setting::OppositeOrientation => "Alternate nested orientations",
        Setting::UnhideApps => "Unhide hidden apps",
        Setting::FocusFollowsMouse => "Focus follows mouse",
        Setting::KeyMappingPreset => "Key mapping",
    }
}

impl GeneralView {
    pub fn new(model: Entity<ConfigModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let padding_field =
            cx.new(|cx| CommitField::number(padding(model.read(cx)), unsigned_number, window, cx));
        let mut subscriptions = vec![cx.subscribe(
            &padding_field,
            |this: &mut Self, _, event: &Committed<i64>, cx| {
                if let Ok(value) = u32::try_from(event.0) {
                    this.set(Setting::AccordionPadding, SettingValue::Count(value), cx);
                }
            },
        )];
        let mut pickers = Vec::new();
        for setting in Setting::ALL {
            let choices = setting.spec().kind.choices();
            if choices.is_empty() {
                continue;
            }
            let initial = SharedString::from(effective(model.read(cx), setting).to_string());
            let picker = cx.new(|cx| {
                let mut picker = SelectState::new(
                    SearchableVec::new(
                        choices
                            .iter()
                            .map(|value| SharedString::from(value.to_string()))
                            .collect::<Vec<_>>(),
                    ),
                    None,
                    window,
                    cx,
                );
                picker.set_selected_value(&initial, window, cx);
                picker
            });
            subscriptions.push(cx.subscribe(
                &picker,
                move |this: &mut Self, _, event: &SelectEvent<SearchableVec<SharedString>>, cx| {
                    let SelectEvent::Confirm(Some(text)) = event else {
                        return;
                    };
                    if let Some(value) = choices
                        .iter()
                        .copied()
                        .find(|value| value.to_string() == text.as_str())
                    {
                        this.set(setting, value, cx);
                    }
                },
            ));
            pickers.push((setting, picker));
        }
        subscriptions.push(cx.observe_in(&model, window, |this, model, window, cx| {
            let value = padding(model.read(cx));
            this.padding.update(cx, |field, cx| {
                if *field.committed() != value {
                    field.set_value(value, window, cx);
                }
            });
            for (setting, picker) in &this.pickers {
                let value = SharedString::from(effective(model.read(cx), *setting).to_string());
                picker.update(cx, |picker, cx| {
                    if picker.selected_value() != Some(&value) {
                        picker.set_selected_value(&value, window, cx);
                    }
                });
            }
            cx.notify();
        }));
        Self {
            model,
            padding: padding_field,
            pickers,
            prefill: true,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn has_invalid_field(&self, cx: &App) -> bool {
        self.error.is_some() || self.padding.read(cx).has_error()
    }

    fn set(&mut self, setting: Setting, value: SettingValue, cx: &mut Context<Self>) {
        let mut doc = self.model.read(cx).doc().clone();
        self.error = set_setting(&mut doc, setting, value)
            .err()
            .map(|error| error.to_string().into());
        if self.error.is_none() {
            self.model.update(cx, |model, cx| {
                model.commit(doc, format!("Set {}", setting.spec().path.join(".")), cx);
            });
            if setting == Setting::AutoReloadConfig && value == SettingValue::Bool(true) {
                cx.emit(AutoReloadEnabled);
            }
        }
        cx.notify();
    }

    fn reset(&mut self, setting: Setting, cx: &mut Context<Self>) {
        let mut doc = self.model.read(cx).doc().clone();
        self.error = reset_setting(&mut doc, setting)
            .err()
            .map(|error| error.to_string().into());
        if self.error.is_none() {
            self.model.update(cx, |model, cx| {
                model.commit(doc, "Reset setting to default", cx);
            });
        }
        cx.notify();
    }

    fn confirm_upgrade(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prefill = true;
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let names = inferred_persistent_workspaces(view.read(cx).model.read(cx).view());
            let checkbox_view = view.clone();
            let confirm_view = view.downgrade();
            alert
                .confirm()
                .title("Upgrade to config-version 2?")
                .description(
                    "Version 2 uses explicit persistent-workspaces instead of inferring them \
                     from bindings and monitor assignments.",
                )
                .child(
                    Checkbox::new("prefill-workspaces")
                        .label("Prefill persistent-workspaces")
                        .checked(view.read(cx).prefill)
                        .on_click(move |checked, _, cx| {
                            checkbox_view.update(cx, |this, cx| {
                                this.prefill = *checked;
                                cx.notify();
                            });
                        }),
                )
                .when(!names.is_empty(), |alert| alert.child(names.join(", ")))
                .ok_text("Upgrade to 2")
                .on_ok(move |_, _, cx| {
                    if let Err(error) = confirm_view.update(cx, Self::upgrade) {
                        log::debug!("General closed before upgrading: {error}");
                    }
                    true
                })
        });
    }

    fn upgrade(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let names = self
            .prefill
            .then(|| inferred_persistent_workspaces(model.view()));
        let mut doc = model.doc().clone();
        self.error = upgrade_config_version(&mut doc, names)
            .err()
            .map(|error| error.to_string().into());
        if self.error.is_none() {
            self.model.update(cx, |model, cx| {
                model.commit(doc, "Upgrade config-version to 2", cx);
            });
        }
        cx.notify();
    }

    fn row(&self, setting: Setting, cx: &mut Context<Self>) -> AnyElement {
        let path = setting.spec().path.join(".");
        let value = effective(self.model.read(cx), setting);
        let field = self.model.read(cx).view().general.setting(setting).clone();
        let editor = match value {
            SettingValue::Bool(checked) => Switch::new(SharedString::from(path.clone()))
                .accessibility_label(path.clone())
                .checked(checked)
                .on_change(cx.listener(move |this, checked, _, cx| {
                    this.set(setting, SettingValue::Bool(*checked), cx);
                }))
                .into_any_element(),
            SettingValue::Count(_) => div()
                .id("accordion-padding")
                .test_support()
                .w_40()
                .child(self.padding.clone())
                .into_any_element(),
            _ => div()
                .w_40()
                .children(
                    self.pickers
                        .iter()
                        .find(|(key, _)| *key == setting)
                        .map(|(_, picker)| Select::new(picker).accessibility_label(path.clone())),
                )
                .into_any_element(),
        };
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .child(div().flex_1().min_w_0().child(label(setting)))
                    .child(editor)
                    .when(matches!(field, Field::Missing), |row| {
                        row.child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("default: {}", setting.spec().default)),
                        )
                    })
                    .when(!matches!(field, Field::Missing), |row| {
                        row.child(
                            Button::new(SharedString::from(format!("reset-{setting:?}")))
                                .label("Reset to default")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.reset(setting, cx)),
                                ),
                        )
                    }),
            )
            .when_some(
                match field {
                    Field::Invalid { raw, .. } => Some(raw),
                    _ => None,
                },
                |row, raw| row.child(div().text_color(cx.theme().danger).child(raw)),
            )
            .into_any_element()
    }
}

impl Render for GeneralView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let version = self.model.read(cx).view().general.config_version.clone();
        let current = version.value().copied().unwrap_or(1);
        let shown_version = match &version {
            Field::Invalid { raw, .. } => raw.clone(),
            _ => current.to_string(),
        };
        let rows = Setting::ALL
            .into_iter()
            .map(|setting| self.row(setting, cx))
            .collect::<Vec<_>>();
        v_flex()
            .id("general-form")
            .overflow_y_scroll()
            .gap_4()
            .child(
                h_flex()
                    .gap_4()
                    .child("config-version")
                    .child(shown_version)
                    .when(matches!(version, Field::Missing), |row| {
                        row.child("default: 1")
                    })
                    .when(current == 1, |row| {
                        row.child(
                            Button::new("upgrade-config-version")
                                .label("Upgrade to 2")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_upgrade(window, cx);
                                })),
                        )
                    }),
            )
            .children(rows)
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_color(cx.theme().danger).child(error))
            })
            .children(self.render_other_settings(cx))
    }
}

impl GeneralView {
    /// Keys no form edits, so the user knows PlaneUI saw them and leaves them alone.
    fn render_other_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let model = self.model.read(cx);
        let keys = &model.view().out_of_scope;
        if keys.is_empty() {
            return None;
        }
        let text = model.doc().to_string();
        let muted = cx.theme().muted_foreground;
        let rows = keys.iter().map(|key| {
            let line = key
                .span
                .as_ref()
                .map(|span| format!("line {}", line_and_column(&text, span.start).0));
            h_flex()
                .id(SharedString::from(format!("other-{}", key.value)))
                .test_support()
                .aria_label(SharedString::from(match &line {
                    Some(line) => format!("{}, {line}, kept as is", key.value),
                    None => format!("{}, kept as is", key.value),
                }))
                .gap_4()
                .child(div().flex_1().min_w_0().child(key.value.to_string()))
                .children(line.map(|line| div().text_color(muted).child(line)))
                .child(div().text_color(muted).child("kept as is"))
        });
        Some(
            v_flex()
                .id("other-settings")
                .gap_2()
                .pt_4()
                .child(div().text_lg().child("Other settings in your file"))
                .children(rows)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Fixture, in_window, open_window};
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt as _;

    #[gpui_kit::test]
    fn switch_commits_immediately_and_reset_removes_key(cx: &mut TestAppContext) {
        let fixture = Fixture::new("");
        let model = fixture.model(cx);
        let (window, _) = open_window(cx, |window, cx| GeneralView::new(model.clone(), window, cx));
        in_window(cx, window, |window, cx| window.click("start-at-login", cx));
        cx.run_until_parked();
        assert_eq!(fixture.disk(), "start-at-login = true\n");
        assert_eq!(fixture.aerospace.reload_count(), 1);
        in_window(cx, window, |window, cx| {
            window.click("reset-StartAtLogin", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), "");
        assert_eq!(fixture.aerospace.reload_count(), 2);
    }

    #[gpui_kit::test]
    fn padding_pipeline_and_undo(cx: &mut TestAppContext) {
        let original = "accordion-padding = 30 # px\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let (window, view) =
            open_window(cx, |window, cx| GeneralView::new(model.clone(), window, cx));
        in_window(cx, window, |window, cx| {
            window.click("accordion-padding", cx);
            window.press("cmd-a", cx);
            window.input("12", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), "accordion-padding = 12 # px\n");
        assert_eq!(fixture.aerospace.reload_count(), 1);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
        assert_eq!(
            view.read_with(cx, |view, cx| view
                .padding
                .read(cx)
                .input()
                .read(cx)
                .value()
                .to_string()),
            "30"
        );
    }

    #[gpui_kit::test]
    fn keys_no_form_edits_are_listed_with_their_line(cx: &mut TestAppContext) {
        let fixture = Fixture::new(
            "accordion-padding = 30\nafter-startup-command = ['exec-and-forget true']\n",
        );
        let model = fixture.model(cx);
        let (window, _) = open_window(cx, |window, cx| GeneralView::new(model.clone(), window, cx));
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("other-after-startup-command").label(),
                Some("after-startup-command, line 2, kept as is")
            );
            assert!(window.try_find("other-accordion-padding").is_none());
        });
    }

    #[gpui_kit::test]
    fn no_other_settings_list_when_every_key_is_edited(cx: &mut TestAppContext) {
        let fixture = Fixture::new("accordion-padding = 30\n");
        let model = fixture.model(cx);
        let (window, _) = open_window(cx, |window, cx| GeneralView::new(model.clone(), window, cx));
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("other-settings").is_none());
        });
    }

    #[gpui_kit::test]
    fn upgrade_prefill_is_one_write_and_undo(cx: &mut TestAppContext) {
        let original = "[mode.main.binding]\na = 'workspace A'\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let (window, _view) =
            open_window(cx, |window, cx| GeneralView::new(model.clone(), window, cx));
        in_window(cx, window, |window, cx| {
            window.click("upgrade-config-version", cx);
        });
        in_window(cx, window, |window, cx| {
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert!(fixture.disk().contains("persistent-workspaces = [\"A\"]"));
        assert_eq!(fixture.aerospace.reload_count(), 1);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
    }
}
