//! Monitor selectors shared by assignments and gaps.

use std::num::NonZeroU32;

use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, TestSupportExt as _,
    Window, div,
};
use planeui_core::monitors::MonitorPattern;

use crate::fields::{CommitField, CommitTextField, Committed};
use crate::live_data::{LiveData, LiveState};

type Choices = SelectState<SearchableVec<MonitorChoice>>;

#[derive(Debug, Clone)]
struct MonitorChoice {
    label: SharedString,
    pattern: MonitorPattern,
}

impl SearchableListItem for MonitorChoice {
    type Value = MonitorPattern;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.pattern
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PatternPicked(pub MonitorPattern);

#[derive(Debug)]
pub(crate) struct MonitorPicker {
    live_data: Entity<LiveData>,
    text: Entity<CommitTextField>,
    choices: Entity<Choices>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PatternPicked> for MonitorPicker {}

fn monitor_options(state: &LiveState) -> Vec<MonitorChoice> {
    let LiveState::Ready { monitors, .. } = state else {
        return Vec::new();
    };
    let mut options = vec![
        MonitorChoice {
            label: "main".into(),
            pattern: MonitorPattern::Main,
        },
        MonitorChoice {
            label: "secondary".into(),
            pattern: MonitorPattern::Secondary,
        },
    ];
    let mut monitors = monitors.iter().collect::<Vec<_>>();
    monitors.sort_by_key(|monitor| monitor.id);
    for (index, monitor) in monitors.into_iter().enumerate() {
        if let Some(index) = u32::try_from(index + 1).ok().and_then(NonZeroU32::new) {
            options.push(MonitorChoice {
                label: index.to_string().into(),
                pattern: MonitorPattern::Index(index),
            });
        }
        let mut escaped = String::from("^");
        for character in monitor.name.chars() {
            if "\\.+*?()|[]{}^$#&-~".contains(character) {
                escaped.push('\\');
            }
            escaped.push(character);
        }
        escaped.push('$');
        options.push(MonitorChoice {
            label: monitor.name.clone().into(),
            pattern: MonitorPattern::Regex(escaped),
        });
    }
    options
}

impl MonitorPicker {
    pub(crate) fn has_invalid_field(&self, cx: &App) -> bool {
        self.text.read(cx).has_error()
    }

    pub(crate) fn new(
        live_data: Entity<LiveData>,
        initial: Option<&MonitorPattern>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = cx.new(|cx| {
            CommitField::text(
                initial.map_or_else(String::new, MonitorPattern::format),
                |text| {
                    MonitorPattern::parse_str(text)
                        .map(|_| text.to_owned())
                        .map_err(|error| error.to_string().into())
                },
                window,
                cx,
            )
        });
        let options = monitor_options(live_data.read(cx).state());
        let choices = cx.new(|cx| {
            let mut choices =
                SelectState::new(SearchableVec::new(options), None, window, cx).searchable(true);
            if let Some(initial) = initial {
                choices.set_selected_value(initial, window, cx);
            }
            choices
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &text,
                window,
                |this: &mut Self, _, event: &Committed<String>, window, cx| {
                    if let Ok(pattern) = MonitorPattern::parse_str(&event.0) {
                        this.sync_choice(&pattern, window, cx);
                        cx.emit(PatternPicked(pattern));
                    }
                    cx.notify();
                },
            ),
            cx.subscribe_in(
                &choices,
                window,
                |this: &mut Self,
                 _,
                 event: &SelectEvent<SearchableVec<MonitorChoice>>,
                 window,
                 cx| {
                    let SelectEvent::Confirm(value) = event;
                    if let Some(pattern) = value {
                        let pattern = pattern.clone();
                        this.text
                            .update(cx, |text, cx| text.set_value(pattern.format(), window, cx));
                        cx.emit(PatternPicked(pattern));
                    }
                },
            ),
            cx.observe_in(&live_data, window, |this, data, window, cx| {
                let options = monitor_options(data.read(cx).state());
                this.choices.update(cx, |choices, cx| {
                    choices.set_items(SearchableVec::new(options), window, cx);
                });
                if let Ok(pattern) = MonitorPattern::parse_str(this.text.read(cx).committed()) {
                    this.sync_choice(&pattern, window, cx);
                }
                cx.notify();
            }),
        ];
        Self {
            live_data,
            text,
            choices,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_value(
        &mut self,
        pattern: &MonitorPattern,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = pattern.format();
        if self.text.read(cx).committed() != &value {
            self.text
                .update(cx, |text, cx| text.set_value(value, window, cx));
            self.sync_choice(pattern, window, cx);
        }
    }

    fn sync_choice(&self, pattern: &MonitorPattern, window: &mut Window, cx: &mut Context<Self>) {
        self.choices.update(cx, |choices, cx| {
            choices.set_selected_value(pattern, window, cx);
        });
    }
}

impl Render for MonitorPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let warning = MonitorPattern::parse_str(self.text.read(cx).committed())
            .ok()
            .and_then(|pattern| pattern.regex_warning());
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .when(
                        matches!(self.live_data.read(cx).state(), LiveState::Ready { .. }),
                        |row| {
                            row.child(
                                div().id("monitor-choices").test_support().w_48().child(
                                    Select::new(&self.choices)
                                        .placeholder("Monitor")
                                        .accessibility_label("Monitor"),
                                ),
                            )
                        },
                    )
                    .child(
                        div()
                            .id("monitor-pattern")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .child(self.text.clone()),
                    ),
            )
            .when_some(warning, |view, warning| {
                view.child(
                    div()
                        .id("monitor-warning")
                        .test_support()
                        .text_color(cx.theme().warning)
                        .child(warning),
                )
            })
    }
}
