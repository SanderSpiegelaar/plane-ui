//! Ordered rule cards. Each complete edit uses the model's existing write and undo pipeline.

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EntityId, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, TestSupportExt as _, Window, actions, div,
};
use planeui_core::commands::ParsedScript;
use planeui_core::config_doc::{ConfigDoc, KeyPath};
use planeui_core::config_view::ConfigView;
use planeui_core::rules::{
    Matcher, RuleEditError, WindowRule, add_rule, move_rule, remove_rule, update_rule,
};
use planeui_core::validate::{Severity, validate};

use crate::config_model::ConfigModel;
use crate::live_data::LiveData;
use crate::rule_form::{RuleForm, RuleValues};

actions!(planeui, [MoveRuleUp, MoveRuleDown]);
pub(crate) const RULE_CONTEXT: &str = "WindowRule";

#[derive(Debug)]
enum CardBody {
    Editable(Entity<RuleForm>),
    ReadOnly { raw: String, reason: String },
}

#[derive(Debug)]
struct Card {
    id: usize,
    focus: FocusHandle,
    body: CardBody,
    _subscription: Option<Subscription>,
}

#[derive(Debug, Clone)]
struct DraggedRule {
    owner: EntityId,
    card: usize,
    label: SharedString,
}

impl Render for DraggedRule {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_3()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .child(self.label.clone())
    }
}

#[derive(Debug)]
pub struct RulesView {
    model: Entity<ConfigModel>,
    live_data: Entity<LiveData>,
    rules: Vec<WindowRule>,
    cards: Vec<Card>,
    next_id: usize,
    error: Option<SharedString>,
    _subscription: Subscription,
}

impl RulesView {
    pub fn new(
        model: Entity<ConfigModel>,
        live_data: Entity<LiveData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe_in(&model, window, |this: &mut Self, model, window, cx| {
            if this.rules != model.read(cx).view().window_rules {
                this.rebuild(window, cx);
            }
            cx.notify();
        });
        let mut this = Self {
            model,
            live_data,
            rules: Vec::new(),
            cards: Vec::new(),
            next_id: 0,
            error: None,
            _subscription: subscription,
        };
        this.rebuild(window, cx);
        this
    }

    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rules = self.model.read(cx).view().window_rules.clone();
        self.cards.clear();
        self.error = None;
        for (index, rule) in self.rules.clone().iter().enumerate() {
            let reason = match (&rule.matcher, &rule.run) {
                (Matcher::ReadOnly { reason }, _) | (_, ParsedScript::ReadOnly { reason }) => {
                    Some(reason.clone())
                }
                _ => None,
            };
            let body = match reason {
                Some(reason) => CardBody::ReadOnly {
                    raw: self.raw_rule(index, cx),
                    reason,
                },
                None => CardBody::Editable(self.form(Some(rule), window, cx)),
            };
            self.push_card(body, cx);
        }
        cx.notify();
    }

    fn raw_rule(&self, index: usize, cx: &gpui_kit::App) -> String {
        let model = self.model.read(cx);
        let Some(item) = model.doc().get(&KeyPath::from(["on-window-detected"])) else {
            return String::new();
        };
        if item.is_array_of_tables() {
            let isolated = (|| -> Result<String, RuleEditError> {
                let mut doc = model.doc().clone();
                let other_keys: Vec<_> = doc
                    .document()
                    .iter()
                    .filter(|(key, _)| *key != "on-window-detected")
                    .map(|(key, _)| key.to_owned())
                    .collect();
                for key in other_keys {
                    doc.remove(&KeyPath::from([key]))?;
                }
                for other in (0..self.rules.len()).rev().filter(|other| *other != index) {
                    remove_rule(&mut doc, other)?;
                }
                Ok(doc.to_string())
            })();
            isolated.unwrap_or_else(|error| {
                log::warn!("could not isolate window rule source: {error}");
                model.doc().to_string()
            })
        } else {
            item.as_array()
                .and_then(|array| array.get(index))
                .map_or_else(String::new, ToString::to_string)
        }
    }

    fn form(
        &self,
        rule: Option<&WindowRule>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<RuleForm> {
        cx.new(|cx| RuleForm::new(self.model.clone(), self.live_data.clone(), rule, window, cx))
    }

    fn push_card(&mut self, body: CardBody, cx: &mut Context<Self>) {
        let subscription = match &body {
            CardBody::Editable(form) => Some(cx.subscribe(form, |this, form, values: &RuleValues, cx| {
                if let Some(index) = this.cards.iter().position(|card| matches!(&card.body, CardBody::Editable(candidate) if *candidate == form)) {
                    let is_new = index == this.rules.len();
                    let edited = this.edit("Edit window rule", |doc| {
                        if is_new {
                            add_rule(doc, &values.matcher, &values.commands, values.check_further)
                        } else {
                            update_rule(doc, index, &values.matcher, &values.commands, values.check_further)
                        }
                    }, cx);
                    if !edited && let Some(error) = this.error.take() {
                        form.update(cx, |form, cx| form.show_error(error, cx));
                    }
                }
            })),
            CardBody::ReadOnly { .. } => None,
        };
        self.cards.push(Card {
            id: self.next_id,
            focus: cx.focus_handle(),
            body,
            _subscription: subscription,
        });
        self.next_id += 1;
    }

    fn edit(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut ConfigDoc) -> Result<(), RuleEditError>,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut doc = self.model.read(cx).doc().clone();
        self.error = change(&mut doc).err().map(|error| error.to_string().into());
        if self.error.is_none() {
            let view = ConfigView::from_doc(&doc);
            let report = validate(&doc, &view);
            if report.blocks_write() {
                self.error = report
                    .issues
                    .iter()
                    .find(|issue| issue.severity == Severity::Error)
                    .map(|issue| issue.message.clone().into());
            } else {
                self.rules = view.window_rules;
                self.model
                    .update(cx, |model, cx| model.commit(doc, label, cx));
            }
        }
        cx.notify();
        self.error.is_none()
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cards.len() != self.rules.len() {
            return;
        }
        let form = self.form(None, window, cx);
        self.push_card(CardBody::Editable(form), cx);
        if let Some(card) = self.cards.last() {
            window.focus(&card.focus, cx);
        }
        cx.notify();
    }

    fn move_card(&mut self, from: usize, to: usize, window: &mut Window, cx: &mut Context<Self>) {
        if from == to || from >= self.rules.len() || to >= self.rules.len() {
            return;
        }
        if self.edit("Move window rule", |doc| move_rule(doc, from, to), cx) {
            let card = self.cards.remove(from);
            window.focus(&card.focus, cx);
            self.cards.insert(to, card);
        }
    }

    fn drop_card(
        &mut self,
        drag: &DraggedRule,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if drag.owner != cx.entity_id() {
            return;
        }
        if let Some(from) = self.cards.iter().position(|card| card.id == drag.card) {
            self.move_card(from, to, window, cx);
        }
    }

    fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.rules.len()
            || self.edit("Delete window rule", |doc| remove_rule(doc, index), cx)
        {
            self.cards.remove(index);
        }
        cx.notify();
    }

    fn card(&self, index: usize, card: &Card, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let persisted = index < self.rules.len();
        let drag = DraggedRule {
            owner: cx.entity_id(),
            card: card.id,
            label: format!("Rule {}", index + 1).into(),
        };
        let body = match &card.body {
            CardBody::Editable(form) => form.clone().into_any_element(),
            CardBody::ReadOnly { raw, reason } => v_flex()
                .gap_3()
                .child(
                    div()
                        .id(("rule-source", card.id))
                        .test_support()
                        .aria_label(raw.clone())
                        .child(raw.clone()),
                )
                .child(div().child(reason.clone()))
                .into_any_element(),
        };
        v_flex()
            .id(("rule-card", card.id))
            .w_full()
            .flex_shrink_0()
            .test_support()
            .role(Role::Group)
            .aria_label(format!("Rule {}", index + 1))
            .key_context(RULE_CONTEXT)
            .track_focus(&card.focus)
            .tab_group()
            .on_action(cx.listener(move |this, _: &MoveRuleUp, window, cx| {
                this.move_card(index, index.saturating_sub(1), window, cx);
            }))
            .on_action(cx.listener(move |this, _: &MoveRuleDown, window, cx| {
                this.move_card(index, index + 1, window, cx);
            }))
            .on_drop(cx.listener(move |this, drag: &DraggedRule, window, cx| {
                this.drop_card(drag, index, window, cx);
            }))
            .gap_3()
            .p_4()
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .id(("rule-drag", card.id))
                            .test_support()
                            .flex_1()
                            .child(format!("Rule {}", index + 1))
                            .when(persisted, |handle| {
                                handle
                                    .cursor_grab()
                                    .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                            }),
                    )
                    .child(
                        Button::new(("rule-up", card.id))
                            .label("Move up")
                            .disabled(!persisted || index == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.move_card(index, index.saturating_sub(1), window, cx);
                            })),
                    )
                    .child(
                        Button::new(("rule-down", card.id))
                            .label("Move down")
                            .disabled(!persisted || index + 1 == self.rules.len())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.move_card(index, index + 1, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("rule-delete", card.id))
                            .label("Delete")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(index, cx))),
                    ),
            )
            .child(body)
    }
}

impl Render for RulesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("window-rules")
            .test_support()
            .w_full()
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .gap_4()
            .children(
                self.cards
                    .iter()
                    .enumerate()
                    .map(|(index, card)| self.card(index, card, cx)),
            )
            .child(
                Button::new("new-rule")
                    .label("New rule")
                    .disabled(self.cards.len() != self.rules.len())
                    .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .id("rules-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Fixture, in_window, open_window};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AnyWindowHandle, TestAppContext};
    use planeui_core::aerospace::{App, FakeAerospace};
    use std::time::Duration;

    fn open(
        cx: &mut TestAppContext,
        fixture: &Fixture,
    ) -> (AnyWindowHandle, Entity<RulesView>, Entity<ConfigModel>) {
        let model = fixture.model(cx);
        let live = cx.new(|cx| LiveData::new(fixture.aerospace.clone(), cx));
        let (window, view) = open_window(cx, |window, cx| {
            RulesView::new(model.clone(), live, window, cx)
        });
        (window, view, model)
    }

    fn click(cx: &mut TestAppContext, window: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>) {
        in_window(cx, window, |window, cx| window.click(id, cx));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn selecting_a_live_app_commits_one_complete_rule(cx: &mut TestAppContext) {
        let fixture = Fixture::with_aerospace(
            "",
            FakeAerospace::new().with_apps(vec![
                App {
                    name: "Notes".into(),
                    bundle_id: "com.apple.Notes".into(),
                },
                App {
                    name: "Safari".into(),
                    bundle_id: "com.apple.Safari".into(),
                },
            ]),
        );
        let (window, _, model) = open(cx, &fixture);
        click(cx, window, "new-rule");
        click(cx, window, "add-command");
        assert_eq!(fixture.disk(), "", "an incomplete rule stays in the card");
        assert_eq!(fixture.aerospace.reload_count(), 0);
        in_window(cx, window, |window, cx| {
            window.click(("condition-suggestions", 0_usize), cx);
            window.input("Safari", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| window.press("enter", cx));
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[[on-window-detected]]\nif = 'test %{app-bundle-id} = com.apple.Safari'\nrun = \"workspace 1\"\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
        assert_eq!(fixture.snapshots(), vec![String::new()]);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), "");
    }

    const RULES: &str = "# first\n[[on-window-detected]]\nif = 'test %{app-bundle-id} = one'\nrun = 'layout floating'\n\n# second\n[[on-window-detected]]\nif = 'test %{app-bundle-id} = two'\nrun = 'layout tiles'\n";

    #[gpui_kit::test]
    fn keyboard_move_is_one_write_and_one_undo_entry(cx: &mut TestAppContext) {
        let fixture = Fixture::new(RULES);
        let (window, view, model) = open(cx, &fixture);
        let focus = view.read_with(cx, |view, _| view.cards.first().unwrap().focus.clone());
        in_window(cx, window, |window, cx| {
            window.focus(&focus, cx);
            window.press("cmd-alt-down", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "\n# second\n[[on-window-detected]]\nif = 'test %{app-bundle-id} = two'\nrun = 'layout tiles'\n# first\n[[on-window-detected]]\nif = 'test %{app-bundle-id} = one'\nrun = 'layout floating'\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
        assert_eq!(fixture.snapshots(), vec![RULES.to_owned()]);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), RULES);
        assert!(model.read_with(cx, |model, _| model.undo_label().is_none()));
    }

    #[gpui_kit::test]
    fn drag_reorders_rules_through_the_same_single_write(cx: &mut TestAppContext) {
        let fixture = Fixture::new(
            "on-window-detected = [{ if = 'test %{app-bundle-id} = one || true', run = 'layout floating' }, { if = 'test %{app-bundle-id} = two || true', run = 'layout tiles' }]\n",
        );
        let (window, _, model) = open(cx, &fixture);
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            let start = window.find(("rule-drag", 0_usize)).bounds().center();
            let end = window.find(("rule-drag", 1_usize)).bounds().center();
            window.drag(start, end, cx);
        });
        cx.run_until_parked();
        let disk = fixture.disk();
        assert!(disk.find("two || true").unwrap() < disk.find("one || true").unwrap());
        assert_eq!(fixture.aerospace.reload_count(), 1);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.snapshots().len(), 2);
    }

    #[gpui_kit::test]
    fn editing_a_legacy_rule_keeps_its_syntax_and_run_shape(cx: &mut TestAppContext) {
        let original = "[[on-window-detected]]\nif.app-id = 'com.old.App' # keep\nif.during-aerospace-startup = false\nrun = ['layout floating']\n";
        let fixture = Fixture::new(original);
        let (window, _, _) = open(cx, &fixture);
        in_window(cx, window, |window, cx| {
            window
                .within(("rule-card", 0_usize))
                .click(("condition-value", 0_usize), cx);
            window.press("cmd-a", cx);
            window.input("com.new.App", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            original.replace("'com.old.App'", "\"com.new.App\"")
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
    }

    #[gpui_kit::test]
    fn not_running_app_picker_has_only_manual_entry(cx: &mut TestAppContext) {
        let fixture = Fixture::with_aerospace("", FakeAerospace::new().with_running(false));
        let (window, _, _) = open(cx, &fixture);
        click(cx, window, "new-rule");
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .try_find(("condition-suggestions", 0_usize))
                    .is_none()
            );
            assert!(window.find(("condition-value", 0_usize)).visible());
            window
                .within(("rule-card", 0_usize))
                .click(("condition-value", 0_usize), cx);
            window.input("com.manual.App", cx);
            window.press("enter", cx);
        });
        click(cx, window, "add-command");
        assert!(
            fixture
                .disk()
                .contains("test %{app-bundle-id} = com.manual.App")
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
    }

    #[gpui_kit::test]
    fn condition_typing_debounces_and_invalid_quoting_never_writes(cx: &mut TestAppContext) {
        let fixture = Fixture::new(RULES);
        let (window, _, _) = open(cx, &fixture);
        in_window(cx, window, |window, cx| {
            window
                .within(("rule-card", 0_usize))
                .click(("condition-value", 0_usize), cx);
            window.press("cmd-a", cx);
            window.input("changed", cx);
        });
        cx.executor().advance_clock(Duration::from_millis(600));
        assert_eq!(fixture.disk(), RULES);
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(fixture.aerospace.reload_count(), 1);
        let saved = fixture.disk();
        in_window(cx, window, |window, cx| {
            window
                .within(("rule-card", 0_usize))
                .click(("condition-value", 0_usize), cx);
            window.press("cmd-a", cx);
            window.input("%{workspace}", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), saved);
        assert_eq!(fixture.aerospace.reload_count(), 1);
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .find("rule-error")
                    .label()
                    .is_some_and(|label| label.contains("condition cannot be represented"))
            );
        });
    }

    #[gpui_kit::test]
    fn workspace_suggestions_and_manual_entry_use_the_same_condition(cx: &mut TestAppContext) {
        let fixture = Fixture::with_aerospace(
            "",
            FakeAerospace::new().with_workspaces(vec![
                planeui_core::commands::WorkspaceName::new("Work").unwrap(),
            ]),
        );
        let (window, _, _) = open(cx, &fixture);
        click(cx, window, "new-rule");
        in_window(cx, window, |window, cx| {
            window.click(("condition-kind", 0_usize), cx);
            window.input("Workspace is", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| window.press("enter", cx));
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.click(("condition-suggestions", 0_usize), cx);
            window.input("Work", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| window.press("enter", cx));
        cx.run_until_parked();
        click(cx, window, "add-command");
        assert!(fixture.disk().contains("test %{workspace} = Work"));
        assert_eq!(fixture.aerospace.reload_count(), 1);
        in_window(cx, window, |window, cx| {
            window.click(("condition-value", 0_usize), cx);
            window.press("cmd-a", cx);
            window.input("Personal", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert!(fixture.disk().contains("test %{workspace} = Personal"));
        assert_eq!(fixture.aerospace.reload_count(), 2);
    }

    #[gpui_kit::test]
    fn action_arguments_commit_on_enter_not_each_key(cx: &mut TestAppContext) {
        let original =
            "[[on-window-detected]]\nif = 'test %{app-bundle-id} = one'\nrun = 'workspace A'\n";
        let fixture = Fixture::new(original);
        let (window, _, model) = open(cx, &fixture);
        in_window(cx, window, |window, cx| {
            window
                .within(("command-argument", 0_usize))
                .click("input-state", cx);
            window.press("cmd-a", cx);
            window.input("Work", cx);
        });
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        in_window(cx, window, |window, cx| window.press("enter", cx));
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            original.replace("'workspace A'", "\"workspace Work\"")
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
    }

    #[gpui_kit::test]
    fn continuation_switch_commits_immediately(cx: &mut TestAppContext) {
        let original =
            "[[on-window-detected]]\nif = 'test %{app-bundle-id} = one'\nrun = 'layout floating'\n";
        let fixture = Fixture::new(original);
        let (window, _, _) = open(cx, &fixture);
        click(cx, window, "check-further");
        assert_eq!(
            fixture.disk(),
            format!("{original}check-further-callbacks = true\n")
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
    }

    #[gpui_kit::test]
    fn read_only_cards_show_nested_source_and_can_be_deleted(cx: &mut TestAppContext) {
        let original = "[[on-window-detected]]\nrun = 'layout floating'\n[on-window-detected.if]\napp-id = 'one'\nunknown = 'preserve'\n";
        let fixture = Fixture::new(original);
        let (window, _, model) = open(cx, &fixture);
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find(("rule-source", 0_usize)).label(),
                Some(original)
            );
        });
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        click(cx, window, ("rule-delete", 0_usize));
        assert_eq!(fixture.disk(), "");
        assert_eq!(fixture.aerospace.reload_count(), 1);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
    }
}
