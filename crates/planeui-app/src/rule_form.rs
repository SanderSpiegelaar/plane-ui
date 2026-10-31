//! A rule draft emits one complete edit, never partially populated conditions or commands.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, TestSupportExt as _, Window, div,
};
use planeui_core::commands::{Command, ParsedScript};
use planeui_core::rules::{Condition, LegacyMatcher, Matcher, WindowRule};

use crate::binding_draft::CommandDraft;
use crate::command_form::{CommandEvent, CommandForm, CommitMode, Picker, picker};
use crate::config_model::ConfigModel;
use crate::fields::{CommitField, CommitTextField, Committed};
use crate::live_data::{LiveData, LiveState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConditionKind {
    App,
    AppName,
    Title,
    Workspace,
}

impl ConditionKind {
    const ALL: [Self; 4] = [Self::App, Self::AppName, Self::Title, Self::Workspace];

    fn label(self) -> &'static str {
        match self {
            Self::App => "App is",
            Self::AppName => "App name matches",
            Self::Title => "Window title matches",
            Self::Workspace => "Workspace is",
        }
    }

    fn condition(self, value: String) -> Condition {
        match self {
            Self::App => Condition::AppBundleIdIs(value),
            Self::AppName => Condition::AppNameMatches(value),
            Self::Title => Condition::WindowTitleMatches(value),
            Self::Workspace => Condition::WorkspaceIs(value),
        }
    }

    fn input_label(self) -> &'static str {
        match self {
            Self::App => "Bundle ID",
            Self::AppName => "App name pattern",
            Self::Title => "Window title pattern",
            Self::Workspace => "Workspace",
        }
    }

    fn suggestions_label(self) -> &'static str {
        match self {
            Self::App => "Running apps",
            Self::Workspace => "Workspaces",
            Self::AppName | Self::Title => self.label(),
        }
    }

    fn read(condition: &Condition) -> (Self, String) {
        match condition {
            Condition::AppBundleIdIs(value) => (Self::App, value.clone()),
            Condition::AppNameMatches(value) => (Self::AppName, value.clone()),
            Condition::WindowTitleMatches(value) => (Self::Title, value.clone()),
            Condition::WorkspaceIs(value) => (Self::Workspace, value.clone()),
        }
    }
}

#[derive(Debug)]
struct ConditionRow {
    kind: ConditionKind,
    picker: Entity<Picker>,
    value: Entity<CommitTextField>,
    suggestions: Option<Entity<Picker>>,
    suggestion_subscription: Option<Subscription>,
}

#[derive(Debug, Clone)]
pub(crate) struct RuleValues {
    pub matcher: Matcher,
    pub commands: Vec<Command>,
    pub check_further: Option<bool>,
}

#[derive(Debug)]
pub(crate) struct RuleForm {
    live_data: Entity<LiveData>,
    legacy: Option<LegacyMatcher>,
    conditions: Vec<ConditionRow>,
    commands: Entity<CommandForm>,
    check_further: Option<bool>,
    error: Option<SharedString>,
    condition_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<RuleValues> for RuleForm {}

impl RuleForm {
    pub fn show_error(&mut self, error: SharedString, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }

    pub fn new(
        model: Entity<ConfigModel>,
        live_data: Entity<LiveData>,
        rule: Option<&WindowRule>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let legacy = rule.and_then(|rule| match &rule.matcher {
            Matcher::Legacy(legacy) => Some(legacy.clone()),
            _ => None,
        });
        let conditions = match rule.map(|rule| &rule.matcher) {
            Some(Matcher::Conditions(conditions)) => {
                conditions.iter().map(ConditionKind::read).collect()
            }
            Some(Matcher::Legacy(legacy)) => [
                (ConditionKind::App, &legacy.app_id),
                (ConditionKind::AppName, &legacy.app_name_regex_substring),
                (ConditionKind::Title, &legacy.window_title_regex_substring),
                (ConditionKind::Workspace, &legacy.workspace),
            ]
            .into_iter()
            .filter_map(|(kind, value)| value.clone().map(|value| (kind, value)))
            .collect(),
            _ => vec![(ConditionKind::App, String::new())],
        };
        let commands = rule
            .and_then(|rule| match &rule.run {
                ParsedScript::Editable { commands, .. } => Some(commands),
                ParsedScript::ReadOnly { .. } => None,
            })
            .map_or_else(
                || Ok(Vec::new()),
                |commands| commands.iter().map(CommandDraft::new).collect(),
            );
        let (commands, error) = match commands {
            Ok(commands) => (commands, None),
            Err(error) => (Vec::new(), Some(SharedString::from(error.0))),
        };
        let commands =
            cx.new(|cx| CommandForm::new(model, commands, CommitMode::Automatic, window, cx));
        let subscriptions = vec![
            cx.subscribe(&commands, |this: &mut Self, _, event: &CommandEvent, cx| {
                if matches!(event, CommandEvent::Ready) {
                    this.commit(cx);
                }
                cx.notify();
            }),
            cx.observe_in(&live_data, window, |this: &mut Self, _, window, cx| {
                this.refresh_suggestions(window, cx);
                cx.notify();
            }),
        ];
        let mut this = Self {
            live_data,
            legacy,
            conditions: Vec::new(),
            commands,
            check_further: rule.and_then(|rule| rule.check_further_callbacks),
            error,
            condition_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.build_conditions(conditions, window, cx);
        this
    }

    fn condition_values(&self, cx: &gpui_kit::App) -> Vec<(ConditionKind, String)> {
        self.conditions
            .iter()
            .map(|row| {
                (
                    row.kind,
                    row.value.read(cx).input().read(cx).value().to_string(),
                )
            })
            .collect()
    }

    fn build_conditions(
        &mut self,
        values: Vec<(ConditionKind, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.conditions.clear();
        self.condition_subscriptions.clear();
        for (index, (kind, value)) in values.into_iter().enumerate() {
            let select = picker(
                ConditionKind::ALL
                    .into_iter()
                    .map(|kind| kind.label().into())
                    .collect(),
                Some(kind.label()),
                window,
                cx,
            );
            let field =
                cx.new(|cx| CommitField::text(value, |text| Ok(text.to_owned()), window, cx));
            self.condition_subscriptions
                .push(cx.subscribe(&field, |this, _, _: &Committed<String>, cx| this.commit(cx)));
            self.condition_subscriptions.push(cx.subscribe_in(
                &select,
                window,
                move |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(kind) = ConditionKind::ALL
                        .into_iter()
                        .find(|kind| Some(kind.label()) == value.as_deref())
                    {
                        if let Some(row) = this.conditions.get_mut(index) {
                            row.kind = kind;
                        }
                        this.refresh_suggestions(window, cx);
                        this.commit(cx);
                    }
                },
            ));
            self.conditions.push(ConditionRow {
                kind,
                picker: select,
                value: field,
                suggestions: None,
                suggestion_subscription: None,
            });
        }
        self.refresh_suggestions(window, cx);
        cx.notify();
    }

    fn refresh_suggestions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for index in 0..self.conditions.len() {
            let Some(row) = self.conditions.get(index) else {
                continue;
            };
            row.value.read(cx).input().clone().update(cx, |input, cx| {
                input.set_placeholder(row.kind.input_label(), window, cx);
            });
            let values: Vec<(SharedString, String)> =
                match (row.kind, self.live_data.read(cx).state()) {
                    (ConditionKind::App, LiveState::Ready { apps, .. }) => apps
                        .iter()
                        .map(|app| {
                            (
                                format!("{} ({})", app.name, app.bundle_id).into(),
                                app.bundle_id.clone(),
                            )
                        })
                        .collect(),
                    (ConditionKind::Workspace, LiveState::Ready { workspaces, .. }) => workspaces
                        .iter()
                        .map(|workspace| {
                            (
                                workspace.as_str().to_owned().into(),
                                workspace.as_str().to_owned(),
                            )
                        })
                        .collect(),
                    _ => Vec::new(),
                };
            let suggestion = (!values.is_empty()).then(|| {
                picker(
                    values.iter().map(|(label, _)| label.clone()).collect(),
                    None,
                    window,
                    cx,
                )
            });
            let subscription = suggestion.as_ref().map(|suggestion| {
                cx.subscribe_in(
                    suggestion,
                    window,
                    move |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                        let SelectEvent::Confirm(selected) = event;
                        if let Some((_, value)) = values
                            .iter()
                            .find(|(label, _)| Some(label) == selected.as_ref())
                        {
                            if let Some(row) = this.conditions.get(index) {
                                row.value.update(cx, |field, cx| {
                                    field.set_value(value.clone(), window, cx);
                                });
                            }
                            this.commit(cx);
                        }
                    },
                )
            });
            if let Some(row) = self.conditions.get_mut(index) {
                row.suggestions = suggestion;
                row.suggestion_subscription = subscription;
            }
        }
    }

    fn values(&self, cx: &gpui_kit::App) -> Result<RuleValues, SharedString> {
        let mut conditions = Vec::new();
        for row in &self.conditions {
            let field = row.value.read(cx);
            if field.committed().is_empty() {
                return Err("Enter a condition value".into());
            }
            conditions.push(row.kind.condition(field.committed().clone()));
        }
        let matcher = if let Some(legacy) = &self.legacy {
            let mut legacy = LegacyMatcher {
                during_aerospace_startup: legacy.during_aerospace_startup,
                ..LegacyMatcher::default()
            };
            for condition in conditions {
                let field = match condition {
                    Condition::AppBundleIdIs(value) => (value, &mut legacy.app_id),
                    Condition::AppNameMatches(value) => {
                        (value, &mut legacy.app_name_regex_substring)
                    }
                    Condition::WindowTitleMatches(value) => {
                        (value, &mut legacy.window_title_regex_substring)
                    }
                    Condition::WorkspaceIs(value) => (value, &mut legacy.workspace),
                };
                if field.1.is_some() {
                    return Err("Legacy rules allow each condition only once".into());
                }
                *field.1 = Some(field.0);
            }
            Matcher::Legacy(legacy)
        } else {
            Matcher::Conditions(conditions)
        };
        let commands = &self.commands.read(cx).commands;
        if commands.is_empty() {
            return Err("Add a command".into());
        }
        let commands = commands
            .iter()
            .map(CommandDraft::to_command)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SharedString::from(error.0))?;
        Ok(RuleValues {
            matcher,
            commands,
            check_further: self.check_further,
        })
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let settled = self.commands.read(cx).is_settled(cx)
            && self.conditions.iter().all(|row| {
                let field = row.value.read(cx);
                field.committed().as_str() == field.input().read(cx).value().as_str()
            });
        if settled {
            match self.values(cx) {
                Ok(values) => cx.emit(values),
                Err(error) => self.error = Some(error),
            }
        }
        cx.notify();
    }
}

impl RuleForm {
    fn render_condition(
        index: usize,
        row: &ConditionRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id(("condition-kind", index))
                            .test_support()
                            .w_56()
                            .flex_shrink_0()
                            .child(Select::new(&row.picker).accessibility_label("Condition")),
                    )
                    .child(
                        div()
                            .id(("condition-value", index))
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(row.value.read(cx).input())
                                    .aria_label(row.kind.input_label()),
                            ),
                    )
                    .child(
                        Button::new(("remove-condition", index))
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let mut values = this.condition_values(cx);
                                values.remove(index);
                                this.build_conditions(values, window, cx);
                                this.commit(cx);
                            })),
                    ),
            )
            .children(row.suggestions.as_ref().map(|suggestion| {
                div()
                    .id(("condition-suggestions", index))
                    .test_support()
                    .child(
                        Select::new(suggestion)
                            .placeholder(row.kind.suggestions_label())
                            .accessibility_label(row.kind.suggestions_label()),
                    )
            }))
    }
}

impl Render for RuleForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .child(div().child("When"))
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .id("rule-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .when(self.conditions.is_empty(), |view| view.child("Any window"))
            .children(
                self.conditions
                    .iter()
                    .enumerate()
                    .map(|(index, row)| Self::render_condition(index, row, cx)),
            )
            .child(
                Button::new("add-condition")
                    .label("Add condition")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let mut values = this.condition_values(cx);
                        values.push((ConditionKind::App, String::new()));
                        this.build_conditions(values, window, cx);
                    })),
            )
            .child(div().child("then"))
            .child(self.commands.clone())
            .child(
                Switch::new("check-further")
                    .label("Keep checking further rules")
                    .checked(self.check_further.unwrap_or(false))
                    .on_click(cx.listener(|this, checked, _, cx| {
                        this.check_further = Some(*checked);
                        this.commit(cx);
                    })),
            )
            .when_some(
                self.legacy
                    .as_ref()
                    .and_then(|legacy| legacy.during_aerospace_startup),
                |view, startup| {
                    view.child(
                        Switch::new("during-startup")
                            .label("During AeroSpace startup")
                            .checked(startup)
                            .on_click(cx.listener(|this, checked, _, cx| {
                                if let Some(legacy) = &mut this.legacy {
                                    legacy.during_aerospace_startup = Some(*checked);
                                }
                                this.commit(cx);
                            })),
                    )
                },
            )
    }
}
