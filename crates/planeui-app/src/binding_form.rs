//! Widget state for a binding. Only the form's commit action can emit an edit.

use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{
    Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction,
};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Selectable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Role, SharedString, Styled as _, Subscription, Window,
    actions, div,
};
use planeui_core::commands::{ArgKind, CATALOGUE, Command, WorkspaceTarget};
use planeui_core::config_doc::KeyPath;
use planeui_core::keys::{KEY_NAMES, Modifier};

use crate::binding_draft::{BindingDraft, CommandDraft};
use crate::config_model::ConfigModel;
use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::TestSupportExt as _;

actions!(planeui, [CommitBinding, CancelBinding]);
pub(crate) const FORM_CONTEXT: &str = "BindingForm";
type Picker = SelectState<SearchableVec<SharedString>>;

#[derive(Debug, Clone)]
pub(crate) enum FormEvent {
    Commit(BindingDraft),
    Cancel,
}

#[derive(Debug)]
enum Widget {
    Text(Entity<InputState>),
    Number(Entity<InputState>),
    Select(Entity<Picker>),
    Layout,
}

#[derive(Debug)]
pub(crate) struct BindingForm {
    pub draft: BindingDraft,
    model: Entity<ConfigModel>,
    mode: String,
    pub focus: FocusHandle,
    key: Entity<Picker>,
    pickers: Vec<Entity<Picker>>,
    fields: Vec<Vec<Vec<Widget>>>,
    suggestions: Vec<Vec<Option<Entity<Picker>>>>,
    subscriptions: Vec<Subscription>,
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
        let mut form = Self {
            draft,
            model,
            mode,
            focus: cx.focus_handle(),
            key,
            pickers: Vec::new(),
            fields: Vec::new(),
            suggestions: Vec::new(),
            subscriptions: Vec::new(),
            error: None,
        };
        form.rebuild(window, cx);
        window.focus(&form.focus, cx);
        form
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

    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.subscriptions.clear();
        self.pickers.clear();
        self.fields.clear();
        self.suggestions.clear();
        self.subscriptions.push(cx.subscribe(
            &self.key,
            |this, _, event: &SelectEvent<SearchableVec<SharedString>>, cx| {
                let SelectEvent::Confirm(value) = event;
                this.draft.key = value.as_ref().map(ToString::to_string);
                this.error = None;
                cx.notify();
            },
        ));
        for command_index in 0..self.draft.commands.len() {
            self.build_command(command_index, window, cx);
        }
        cx.notify();
    }

    fn build_command(&mut self, command_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.draft.commands.get(command_index).cloned() else {
            return;
        };
        let select = picker(
            CATALOGUE
                .iter()
                .map(|spec| spec.display_name.into())
                .collect(),
            Some(command.spec.display_name),
            window,
            cx,
        );
        self.subscriptions.push(cx.subscribe_in(
            &select,
            window,
            move |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                let SelectEvent::Confirm(value) = event;
                if let Some(spec) = CATALOGUE
                    .iter()
                    .find(|spec| Some(spec.display_name) == value.as_deref())
                {
                    match spec.default_command() {
                        Ok(command) => {
                            let command = match CommandDraft::new(&command) {
                                Ok(command) => command,
                                Err(error) => {
                                    this.error = Some(error.0);
                                    cx.notify();
                                    return;
                                }
                            };
                            if let Some(draft) = this.draft.commands.get_mut(command_index) {
                                *draft = command;
                            }
                            this.error = None;
                            this.rebuild(window, cx);
                            this.focus_command(command_index, window, cx);
                        }
                        Err(error) => {
                            this.error = Some(error.to_string());
                            cx.notify();
                        }
                    }
                }
            },
        ));
        self.pickers.push(select);
        let mut fields = Vec::new();
        let mut suggestions = Vec::new();
        for (argument_index, argument) in command.arguments.iter().enumerate() {
            let kind = match argument.spec.kind {
                ArgKind::Flag(flag) => flag.value.copied(),
                kind => Some(kind),
            };
            let mut widgets = Vec::new();
            if let Some(kind) = kind {
                for value_index in 0..argument.values.len().max(1) {
                    let value = argument
                        .values
                        .get(value_index)
                        .cloned()
                        .unwrap_or_default();
                    let widget = self.widget(
                        kind,
                        value,
                        (command_index, argument_index, value_index),
                        window,
                        cx,
                    );
                    widgets.push(widget);
                    if matches!(kind, ArgKind::LayoutOptions) {
                        break;
                    }
                }
            }
            let suggestion = if matches!(kind, Some(ArgKind::Workspace)) {
                let select = picker(self.workspaces(cx), None, window, cx);
                self.subscriptions.push(cx.subscribe_in(
                    &select,
                    window,
                    move |this, _, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                        let SelectEvent::Confirm(value) = event;
                        if let Some(value) = value {
                            this.set_value(command_index, argument_index, 0, value.to_string(), cx);
                            this.rebuild(window, cx);
                            this.focus_command(command_index, window, cx);
                        }
                    },
                ));
                Some(select)
            } else {
                None
            };
            fields.push(widgets);
            suggestions.push(suggestion);
        }
        self.fields.push(fields);
        self.suggestions.push(suggestions);
    }

    fn widget(
        &mut self,
        kind: ArgKind,
        value: String,
        (command_index, argument_index, value_index): (usize, usize, usize),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Widget {
        let choices = self.choices(kind, cx);
        if matches!(kind, ArgKind::LayoutOptions) {
            return Widget::Layout;
        }
        if !choices.is_empty() {
            let select = picker(choices, Some(&value), window, cx);
            self.subscriptions.push(cx.subscribe(
                &select,
                move |this, _, event: &SelectEvent<SearchableVec<SharedString>>, cx| {
                    let SelectEvent::Confirm(value) = event;
                    this.set_value(
                        command_index,
                        argument_index,
                        value_index,
                        value.as_ref().map(ToString::to_string).unwrap_or_default(),
                        cx,
                    );
                },
            ));
            return Widget::Select(select);
        }
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).default_value(value);
            input.set_step(None, window, cx);
            input
        });
        if matches!(kind, ArgKind::SignedAmount | ArgKind::UnsignedInteger) {
            self.subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this, input, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = event;
                    let delta = match action {
                        StepAction::Increment => 1,
                        StepAction::Decrement => -1,
                    };
                    if let Some(value) = this
                        .draft
                        .commands
                        .get(command_index)
                        .and_then(|command| command.arguments.get(argument_index))
                        .and_then(|argument| argument.stepped_value(value_index, delta))
                    {
                        input.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                        this.set_value(command_index, argument_index, value_index, value, cx);
                    }
                },
            ));
        }
        self.subscriptions.push(cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.set_value(
                    command_index,
                    argument_index,
                    value_index,
                    input.read(cx).value().to_string(),
                    cx,
                ),
                InputEvent::PressEnter { .. } => {
                    window.dispatch_action(Box::new(CommitBinding), cx);
                }
                _ => {}
            },
        ));
        if matches!(kind, ArgKind::SignedAmount | ArgKind::UnsignedInteger) {
            Widget::Number(input)
        } else {
            Widget::Text(input)
        }
    }

    fn set_value(
        &mut self,
        command: usize,
        argument: usize,
        index: usize,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(argument) = self
            .draft
            .commands
            .get_mut(command)
            .and_then(|command| command.arguments.get_mut(argument))
        {
            if argument.values.len() <= index {
                argument.values.resize(index + 1, String::new());
            }
            if let Some(current) = argument.values.get_mut(index) {
                *current = value;
            }
        }
        self.error = None;
        cx.notify();
    }

    fn focus_command(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = self.pickers.get(index) {
            picker.update(cx, |picker, cx| picker.focus(window, cx));
        } else {
            self.key.update(cx, |picker, cx| picker.focus(window, cx));
        }
    }

    fn add_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = CATALOGUE.first() else {
            return;
        };
        match spec
            .default_command()
            .map_err(|error| crate::binding_draft::DraftError(error.to_string()))
            .and_then(|command| CommandDraft::new(&command))
        {
            Ok(command) => {
                self.draft.commands.push(command);
                self.rebuild(window, cx);
                self.focus_command(self.draft.commands.len() - 1, window, cx);
            }
            Err(error) => {
                self.error = Some(error.0);
                cx.notify();
            }
        }
    }

    fn choices(&self, kind: ArgKind, cx: &Context<Self>) -> Vec<SharedString> {
        let values: &[&str] = match kind {
            ArgKind::Direction => &["left", "down", "up", "right", "dfs-next", "dfs-prev"],
            ArgKind::ResizeDimension => &["smart", "smart-opposite", "width", "height"],
            ArgKind::Choice(values) => values,
            ArgKind::Mode => {
                return self
                    .model
                    .read(cx)
                    .view()
                    .modes
                    .iter()
                    .map(|mode| mode.name.value.clone().into())
                    .collect();
            }
            _ => &[],
        };
        values.iter().map(|value| (*value).into()).collect()
    }

    fn workspaces(&self, cx: &Context<Self>) -> Vec<SharedString> {
        let model = self.model.read(cx);
        let mut names = Vec::new();
        for mode in &model.view().modes {
            for binding in &mode.bindings {
                if let planeui_core::commands::ParsedScript::Editable { commands, .. } =
                    &binding.script
                {
                    for command in commands {
                        if let Command::Workspace {
                            target: WorkspaceTarget::Name(name),
                            ..
                        }
                        | Command::MoveNodeToWorkspace {
                            target: WorkspaceTarget::Name(name),
                            ..
                        } = command
                        {
                            names.push(SharedString::from(name.as_str().to_owned()));
                        }
                    }
                }
            }
        }
        if let Some(array) = model
            .doc()
            .get(&KeyPath::from(["persistent-workspaces"]))
            .and_then(|item| item.as_array())
        {
            names.extend(
                array
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(|value| SharedString::from(value.to_owned())),
            );
        }
        names.sort();
        names.dedup();
        names
    }

    fn render_argument(
        &self,
        command_index: usize,
        argument_index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let argument = self
            .draft
            .commands
            .get(command_index)
            .and_then(|command| command.arguments.get(argument_index))
            .cloned();
        let mut row = v_flex().gap_1();
        let Some(argument) = argument else {
            return row;
        };
        let flag = matches!(argument.spec.kind, ArgKind::Flag(_));
        if flag {
            row = row.child(
                Checkbox::new(("flag", command_index * 100 + argument_index))
                    .label(argument.spec.name)
                    .checked(argument.enabled)
                    .on_click(cx.listener(move |this, checked, _, cx| {
                        if let Some(argument) = this
                            .draft
                            .commands
                            .get_mut(command_index)
                            .and_then(|command| command.arguments.get_mut(argument_index))
                        {
                            argument.enabled = *checked;
                        }
                        this.error = None;
                        cx.notify();
                    })),
            );
        } else {
            row = row.child(argument.spec.name);
        }
        if !flag || argument.enabled {
            if let Some(widgets) = self
                .fields
                .get(command_index)
                .and_then(|fields| fields.get(argument_index))
            {
                for widget in widgets {
                    row = match widget {
                        Widget::Text(input) => {
                            row.child(Input::new(input).aria_label(argument.spec.name))
                        }
                        Widget::Number(input) => row.child(
                            div()
                                .id(("number", command_index * 100 + argument_index))
                                .test_support()
                                .child(
                                    NumberInput::new(input).accessibility_label(argument.spec.name),
                                ),
                        ),
                        Widget::Select(select) => {
                            row.child(Select::new(select).accessibility_label(argument.spec.name))
                        }
                        Widget::Layout => row.child(Self::render_layouts(
                            &argument.values,
                            command_index,
                            argument_index,
                            cx,
                        )),
                    };
                }
            }
            if let Some(Some(select)) = self
                .suggestions
                .get(command_index)
                .and_then(|fields| fields.get(argument_index))
            {
                row = row.child(
                    Select::new(select)
                        .placeholder("Workspaces")
                        .accessibility_label("Workspace suggestions"),
                );
            }
        }
        row
    }

    fn render_layouts(
        values: &[String],
        command_index: usize,
        argument_index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        h_flex()
            .flex_wrap()
            .gap_1()
            .children(LAYOUTS.iter().map(|layout| {
                let layout = *layout;
                Button::new((SharedString::from(layout), command_index))
                    .label(layout)
                    .selected(values.iter().any(|value| value == layout))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(argument) = this
                            .draft
                            .commands
                            .get_mut(command_index)
                            .and_then(|command| command.arguments.get_mut(argument_index))
                        else {
                            return;
                        };
                        if argument.values.iter().any(|value| value == layout) {
                            argument.values.retain(|value| value != layout);
                        } else {
                            argument.values.push(layout.into());
                        }
                        this.error = None;
                        cx.notify();
                    }))
            }))
    }

    fn render_command(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let arguments = self
            .draft
            .commands
            .get(index)
            .map_or(0, |command| command.arguments.len());
        v_flex()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(cx.theme().border)
            .children(
                self.pickers
                    .get(index)
                    .map(|picker| Select::new(picker).accessibility_label("Command")),
            )
            .children((0..arguments).map(|argument| self.render_argument(index, argument, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(("up", index))
                            .label("Move up")
                            .disabled(index == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.draft.commands.swap(index, index - 1);
                                this.rebuild(window, cx);
                                this.focus_command(index - 1, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("down", index))
                            .label("Move down")
                            .disabled(index + 1 == self.draft.commands.len())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.draft.commands.swap(index, index + 1);
                                this.rebuild(window, cx);
                                this.focus_command(index + 1, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("remove-command", index))
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.draft.commands.remove(index);
                                this.rebuild(window, cx);
                                this.focus_command(
                                    index.min(this.draft.commands.len().saturating_sub(1)),
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
    }
}

const LAYOUTS: &[&str] = &[
    "tiles",
    "accordion",
    "horizontal",
    "vertical",
    "h_tiles",
    "v_tiles",
    "h_accordion",
    "v_accordion",
    "tiling",
    "floating",
];

fn picker<V: 'static>(
    values: Vec<SharedString>,
    selected: Option<&str>,
    window: &mut Window,
    cx: &mut Context<V>,
) -> Entity<Picker> {
    cx.new(|cx| {
        let mut state =
            SelectState::new(SearchableVec::new(values), None, window, cx).searchable(true);
        if let Some(selected) = selected {
            state.set_selected_value(&SharedString::from(selected.to_owned()), window, cx);
        }
        state
    })
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
            .children((0..self.draft.commands.len()).map(|index| self.render_command(index, cx)))
            .child(
                Button::new("add-command")
                    .label("Add command")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_command(window, cx);
                    })),
            )
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
    use crate::test_support::{Fixture, in_window, open_window};
    use gpui_kit::{TestAppContext, test::TestWindowExt as _};

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
}
