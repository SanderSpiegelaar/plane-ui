//! Shared structured command controls for bindings and window rules.

use crate::binding_draft::CommandDraft;
use crate::config_model::ConfigModel;
use crate::fields::{CommitField, CommitTextField, Committed};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{
    Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction,
};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Selectable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
};
use planeui_core::commands::{ArgKind, CATALOGUE, Command, WorkspaceTarget};
use planeui_core::config_doc::KeyPath;

pub(crate) type Picker = SelectState<SearchableVec<SharedString>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitMode {
    Explicit,
    Automatic,
}

#[derive(Debug, Clone)]
pub(crate) enum CommandEvent {
    Changed(Vec<CommandDraft>),
    Ready,
    Submit,
    FocusFallback,
}

#[derive(Debug)]
enum Widget {
    Text(Entity<InputState>),
    Number(Entity<InputState>),
    Select(Entity<Picker>),
    Layout,
}

#[derive(Debug)]
pub(crate) struct CommandForm {
    pub commands: Vec<CommandDraft>,
    model: Entity<ConfigModel>,
    commit_mode: CommitMode,
    pickers: Vec<Entity<Picker>>,
    fields: Vec<Vec<Vec<Widget>>>,
    suggestions: Vec<Vec<Option<Entity<Picker>>>>,
    subscriptions: Vec<Subscription>,
    error: Option<String>,
    commit_fields: Vec<Entity<CommitTextField>>,
}

impl EventEmitter<CommandEvent> for CommandForm {}

impl CommandForm {
    pub fn new(
        model: Entity<ConfigModel>,
        commands: Vec<CommandDraft>,
        commit_mode: CommitMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut form = Self {
            commands,
            model,
            commit_mode,
            pickers: Vec::new(),
            fields: Vec::new(),
            suggestions: Vec::new(),
            subscriptions: Vec::new(),
            error: None,
            commit_fields: Vec::new(),
        };
        form.rebuild(window, cx);
        form
    }

    pub fn is_settled(&self, cx: &gpui_kit::App) -> bool {
        self.commit_fields.iter().all(|field| {
            let field = field.read(cx);
            field.committed().as_str() == field.input().read(cx).value().as_str()
        })
    }

    fn changed(&mut self, ready: bool, cx: &mut Context<Self>) {
        cx.emit(CommandEvent::Changed(self.commands.clone()));
        if ready {
            cx.emit(CommandEvent::Ready);
        }
        cx.notify();
    }

    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.subscriptions.clear();
        self.pickers.clear();
        self.fields.clear();
        self.suggestions.clear();
        self.commit_fields.clear();
        for command_index in 0..self.commands.len() {
            self.build_command(command_index, window, cx);
        }
        self.changed(true, cx);
    }

    fn build_command(&mut self, command_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.commands.get(command_index).cloned() else {
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
                            if let Some(draft) = this.commands.get_mut(command_index) {
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
                    this.changed(true, cx);
                },
            ));
            return Widget::Select(select);
        }
        let input = if self.commit_mode == CommitMode::Automatic {
            let field =
                cx.new(|cx| CommitField::text(value, |text| Ok(text.to_owned()), window, cx));
            let input = field.read(cx).input().clone();
            self.subscriptions
                .push(cx.subscribe(&field, |_, _, _: &Committed<String>, cx| {
                    cx.emit(CommandEvent::Ready);
                }));
            self.commit_fields.push(field);
            input
        } else {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).default_value(value);
                input.set_step(None, window, cx);
                input
            })
        };
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
        self.subscriptions.push(cx.subscribe(
            &input,
            move |this, input, event: &InputEvent, cx| match event {
                InputEvent::Change => this.set_value(
                    command_index,
                    argument_index,
                    value_index,
                    input.read(cx).value().to_string(),
                    cx,
                ),
                InputEvent::PressEnter { .. } if this.commit_mode == CommitMode::Explicit => {
                    cx.emit(CommandEvent::Submit);
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
        self.changed(false, cx);
    }

    fn focus_command(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = self.pickers.get(index) {
            picker.update(cx, |picker, cx| picker.focus(window, cx));
        } else {
            cx.emit(CommandEvent::FocusFallback);
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
                self.commands.push(command);
                self.rebuild(window, cx);
                self.focus_command(self.commands.len() - 1, window, cx);
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
            .commands
            .get(command_index)
            .and_then(|command| command.arguments.get(argument_index))
            .cloned();
        let mut row = v_flex()
            .id(("command-argument", command_index * 100 + argument_index))
            .test_support()
            .gap_1();
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
                            .commands
                            .get_mut(command_index)
                            .and_then(|command| command.arguments.get_mut(argument_index))
                        {
                            argument.enabled = *checked;
                        }
                        this.error = None;
                        this.changed(true, cx);
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
                        this.changed(true, cx);
                    }))
            }))
    }

    fn render_command(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let arguments = self
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
                                this.commands.swap(index, index - 1);
                                this.rebuild(window, cx);
                                this.focus_command(index - 1, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("down", index))
                            .label("Move down")
                            .disabled(index + 1 == self.commands.len())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.commands.swap(index, index + 1);
                                this.rebuild(window, cx);
                                this.focus_command(index + 1, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("remove-command", index))
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.commands.remove(index);
                                this.rebuild(window, cx);
                                this.focus_command(
                                    index.min(this.commands.len().saturating_sub(1)),
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

pub(crate) fn picker<V: 'static>(
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

impl Render for CommandForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .when_some(self.error.clone(), |row, error| {
                row.child(div().text_color(cx.theme().danger).child(error))
            })
            .children((0..self.commands.len()).map(|index| self.render_command(index, cx)))
            .child(
                Button::new("add-command")
                    .label("Add command")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_command(window, cx);
                    })),
            )
    }
}
