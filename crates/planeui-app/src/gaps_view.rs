//! Constant and ordered per-monitor gaps.

use gpui_kit::component::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, TestSupportExt as _, Window, div,
};
use planeui_core::config_doc::ConfigDoc;
use planeui_core::gaps::{
    self, GapEditError, GapField, GapValue, reset_gap, set_gap, set_outer_linked,
};
use planeui_core::monitors::MonitorPattern;

use crate::config_model::ConfigModel;
use crate::fields::{CommitField, CommitNumberField, Committed};
use crate::live_data::LiveData;
use crate::monitor_picker::{MonitorPicker, PatternPicked};

#[derive(Debug)]
struct RuleFields {
    gap: GapField,
    rows: Vec<(Entity<MonitorPicker>, Entity<CommitNumberField>)>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Debug)]
pub struct GapsView {
    model: Entity<ConfigModel>,
    fields: Vec<(GapField, Entity<CommitNumberField>)>,
    live_data: Entity<LiveData>,
    rules: Vec<RuleFields>,
    linked: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

pub(crate) fn unsigned_number(value: i64) -> Result<(), SharedString> {
    match u32::try_from(value) {
        Ok(_) => Ok(()),
        Err(_) if value < 0 => Err("Must be 0 or more".into()),
        Err(_) => Err("Too large".into()),
    }
}

fn effective(value: &GapValue) -> i64 {
    match value {
        GapValue::Const(value) => i64::from(*value),
        GapValue::PerMonitor(value) => i64::from(value.default),
        _ => 0,
    }
}

impl GapsView {
    pub fn new(
        model: Entity<ConfigModel>,
        live_data: Entity<LiveData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        let fields = GapField::ALL
            .into_iter()
            .map(|gap| {
                let initial = effective(model.read(cx).view().gaps.get(gap));
                let field = cx.new(|cx| CommitField::number(initial, unsigned_number, window, cx));
                subscriptions.push(cx.subscribe(
                    &field,
                    move |this: &mut Self, _, event: &Committed<i64>, cx| {
                        if let Ok(value) = u32::try_from(event.0) {
                            this.set(gap, value, cx);
                        }
                    },
                ));
                (gap, field)
            })
            .collect();
        subscriptions.push(cx.observe_in(&model, window, |this, model, window, cx| {
            for (gap, field) in &this.fields {
                let value = effective(model.read(cx).view().gaps.get(*gap));
                field.update(cx, |field, cx| {
                    if *field.committed() != value {
                        field.set_value(value, window, cx);
                    }
                });
            }
            if GapField::OUTER
                .into_iter()
                .any(|gap| matches!(model.read(cx).view().gaps.get(gap), GapValue::PerMonitor(_)))
            {
                this.linked = false;
            }
            this.sync_rules(window, cx);
            cx.notify();
        }));
        let mut this = Self {
            model,
            fields,
            live_data,
            rules: Vec::new(),
            linked: false,
            error: None,
            _subscriptions: subscriptions,
        };
        this.sync_rules(window, cx);
        this
    }

    pub fn has_invalid_field(&self, cx: &App) -> bool {
        self.error.is_some()
            || self
                .fields
                .iter()
                .any(|(_, field)| field.read(cx).has_error())
            || self
                .rules
                .iter()
                .flat_map(|rule| &rule.rows)
                .any(|(picker, field)| {
                    picker.read(cx).has_invalid_field(cx) || field.read(cx).has_error()
                })
    }

    fn sync_rules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for gap in GapField::ALL {
            let value = self.model.read(cx).view().gaps.get(gap).clone();
            let GapValue::PerMonitor(value) = value else {
                self.rules.retain(|fields| fields.gap != gap);
                continue;
            };
            if let Some(fields) = self.rules.iter().find(|fields| fields.gap == gap)
                && fields.rows.len() == value.rules.len()
            {
                for ((picker, field), (pattern, value)) in fields.rows.iter().zip(&value.rules) {
                    picker.update(cx, |picker, cx| picker.set_value(pattern, window, cx));
                    field.update(cx, |field, cx| {
                        if *field.committed() != i64::from(*value) {
                            field.set_value(i64::from(*value), window, cx);
                        }
                    });
                }
                continue;
            }
            self.rules.retain(|fields| fields.gap != gap);
            let mut subscriptions = Vec::new();
            let rows = value
                .rules
                .iter()
                .enumerate()
                .map(|(index, (pattern, value))| {
                    let picker = cx.new(|cx| {
                        MonitorPicker::new(self.live_data.clone(), Some(pattern), window, cx)
                    });
                    let field = cx.new(|cx| {
                        CommitField::number(i64::from(*value), unsigned_number, window, cx)
                    });
                    subscriptions.push(cx.subscribe(
                        &picker,
                        move |this: &mut Self, _, event: &PatternPicked, cx| {
                            if let GapValue::PerMonitor(value) =
                                this.model.read(cx).view().gaps.get(gap)
                                && let Some((_, value)) = value.rules.get(index)
                            {
                                let value = *value;
                                this.edit(
                                    "Set gap monitor",
                                    |doc| gaps::set_gap_rule(doc, gap, index, &event.0, value),
                                    cx,
                                );
                            }
                        },
                    ));
                    subscriptions.push(cx.subscribe(
                        &field,
                        move |this: &mut Self, _, event: &Committed<i64>, cx| {
                            if let Ok(value) = u32::try_from(event.0)
                                && let GapValue::PerMonitor(current) =
                                    this.model.read(cx).view().gaps.get(gap)
                                && let Some((pattern, _)) = current.rules.get(index)
                            {
                                let pattern = pattern.clone();
                                this.edit(
                                    "Set monitor gap",
                                    |doc| gaps::set_gap_rule(doc, gap, index, &pattern, value),
                                    cx,
                                );
                            }
                        },
                    ));
                    (picker, field)
                })
                .collect();
            self.rules.push(RuleFields {
                gap,
                rows,
                _subscriptions: subscriptions,
            });
        }
    }

    fn edit(
        &mut self,
        label: &str,
        change: impl FnOnce(&mut ConfigDoc) -> Result<(), GapEditError>,
        cx: &mut Context<Self>,
    ) {
        self.error = self
            .model
            .update(cx, |model, cx| model.edit(label, cx, change))
            .err()
            .map(|error| error.to_string().into());
        cx.notify();
    }

    fn set(&mut self, gap: GapField, value: u32, cx: &mut Context<Self>) {
        let per_monitor = matches!(
            self.model.read(cx).view().gaps.get(gap),
            GapValue::PerMonitor(_)
        );
        let linked = self.linked && GapField::OUTER.contains(&gap);
        self.edit(
            "Set gaps",
            |doc| {
                if per_monitor {
                    gaps::set_gap_default(doc, gap, value)
                } else if linked {
                    set_outer_linked(doc, value)
                } else {
                    set_gap(doc, gap, value)
                }
            },
            cx,
        );
    }

    fn reset(&mut self, gap: GapField, cx: &mut Context<Self>) {
        self.edit("Reset gap to default", |doc| reset_gap(doc, gap), cx);
        if self.error.is_none() {
            self.linked = false;
        }
    }

    fn toggle_link(&mut self, linked: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !linked {
            self.linked = false;
            cx.notify();
            return;
        }
        let mut values = Vec::new();
        for gap in GapField::OUTER {
            let value = self.model.read(cx).view().gaps.get(gap);
            if matches!(value, GapValue::PerMonitor(_) | GapValue::Invalid { .. }) {
                return;
            }
            let value = effective(value);
            if !values.contains(&value) {
                values.push(value);
            }
        }
        if values.len() <= 1 {
            self.linked = true;
            cx.notify();
            return;
        }
        let view = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let choices = values.iter().copied().map(|value| {
                let view = view.clone();
                Button::new(SharedString::from(format!("link-{value}")))
                    .label(value.to_string())
                    .on_click(move |_, window, cx| {
                        if let Err(error) = view.update(cx, |this, cx| {
                            if let Ok(value) = u32::try_from(value) {
                                this.linked = true;
                                this.set(GapField::OuterLeft, value, cx);
                                if this.error.is_some() {
                                    this.linked = false;
                                }
                            }
                        }) {
                            log::debug!("gaps closed before linking: {error}");
                        }
                        window.close_dialog(cx);
                    })
            });
            alert.title("Choose the value for all outer gaps").footer(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("cancel-link")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .children(choices),
            )
        });
    }

    fn row(
        &self,
        gap: GapField,
        field: &Entity<CommitNumberField>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let value = self.model.read(cx).view().gaps.get(gap).clone();
        let path = gap.path().to_string();
        let label = gap.path().segments().last().cloned().unwrap_or_default();
        let editor = match &value {
            GapValue::PerMonitor(value) => self
                .per_monitor_editor(gap, field, value, cx)
                .into_any_element(),
            GapValue::Invalid { raw } => div()
                .text_color(cx.theme().danger)
                .child(raw.clone())
                .into_any_element(),
            _ => div()
                .id(SharedString::from(path))
                .test_support()
                .w_40()
                .child(field.clone())
                .into_any_element(),
        };
        v_flex()
            .gap_4()
            .child(
                h_flex().gap_4().child(div().w_24().child(label)).child(
                    Switch::new(SharedString::from(format!("per-monitor-{gap:?}")))
                        .label("Per monitor")
                        .checked(matches!(value, GapValue::PerMonitor(_)))
                        .disabled(matches!(value, GapValue::Invalid { .. }))
                        .on_change(cx.listener(move |this, enabled, _, cx| {
                            this.linked = false;
                            this.edit(
                                "Change gap mode",
                                |doc| {
                                    if *enabled {
                                        gaps::switch_to_per_monitor(doc, gap)
                                    } else {
                                        gaps::switch_to_constant(doc, gap)
                                    }
                                },
                                cx,
                            );
                        })),
                ),
            )
            .child(editor)
            .when(matches!(value, GapValue::Const(_)), |row| {
                row.child(
                    Button::new(SharedString::from(format!("reset-{gap:?}")))
                        .label("Reset to default")
                        .on_click(cx.listener(move |this, _, _, cx| this.reset(gap, cx))),
                )
            })
            .into_any_element()
    }

    fn per_monitor_editor(
        &self,
        gap: GapField,
        field: &Entity<CommitNumberField>,
        value: &gaps::PerMonitor,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = self
            .rules
            .iter()
            .find(|fields| fields.gap == gap)
            .into_iter()
            .flat_map(|fields| fields.rows.iter())
            .enumerate()
            .map(|(index, fields)| {
                Self::rule_row(gap, index, value.rules.len(), fields, cx).into_any_element()
            })
            .collect::<Vec<_>>();
        let default = value.default;
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .children(rows)
            .child(
                h_flex().gap_2().child(div().w_24().child("Default")).child(
                    div()
                        .id(SharedString::from(gap.path().to_string()))
                        .test_support()
                        .w_40()
                        .child(field.clone()),
                ),
            )
            .child(
                Button::new(SharedString::from(format!("add-rule-{gap:?}")))
                    .label("Add monitor")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit(
                            "Add gap rule",
                            |doc| gaps::add_gap_rule(doc, gap, &MonitorPattern::Main, default),
                            cx,
                        );
                    })),
            )
    }

    fn rule_row(
        gap: GapField,
        index: usize,
        count: usize,
        fields: &(Entity<MonitorPicker>, Entity<CommitNumberField>),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(SharedString::from(format!("{}-rule-{index}", gap.path())))
            .test_support()
            .items_start()
            .gap_2()
            .child(div().flex_1().min_w_0().child(fields.0.clone()))
            .child(
                div()
                    .id("gap-rule-value")
                    .test_support()
                    .w_24()
                    .child(fields.1.clone()),
            )
            .child(
                Button::new(("gap-up", index))
                    .icon(IconName::ChevronUp)
                    .accessibility_label("Move up")
                    .tooltip("Move up")
                    .disabled(index == 0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit(
                            "Move gap rule up",
                            |doc| gaps::move_gap_rule(doc, gap, index, index.saturating_sub(1)),
                            cx,
                        );
                    })),
            )
            .child(
                Button::new(("gap-down", index))
                    .icon(IconName::ChevronDown)
                    .accessibility_label("Move down")
                    .tooltip("Move down")
                    .disabled(index + 1 == count)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit(
                            "Move gap rule down",
                            |doc| gaps::move_gap_rule(doc, gap, index, index + 1),
                            cx,
                        );
                    })),
            )
            .child(
                Button::new(("gap-remove", index))
                    .icon(IconName::Close)
                    .accessibility_label("Remove monitor")
                    .tooltip("Remove monitor")
                    .disabled(count == 1)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit(
                            "Remove gap rule",
                            |doc| gaps::remove_gap_rule(doc, gap, index),
                            cx,
                        );
                    })),
            )
    }
}

impl Render for GapsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_link = GapField::OUTER.into_iter().all(|gap| {
            matches!(
                self.model.read(cx).view().gaps.get(gap),
                GapValue::Absent | GapValue::Const(_)
            )
        });
        let inner = self
            .fields
            .iter()
            .filter(|(gap, _)| !GapField::OUTER.contains(gap))
            .map(|(gap, field)| self.row(*gap, field, cx))
            .collect::<Vec<_>>();
        let outer = self
            .fields
            .iter()
            .filter(|(gap, _)| GapField::OUTER.contains(gap))
            .map(|(gap, field)| self.row(*gap, field, cx))
            .collect::<Vec<_>>();
        v_flex()
            .id("gaps-form")
            .overflow_y_scroll()
            .gap_4()
            .child(div().text_lg().child("Inner"))
            .children(inner)
            .child(
                h_flex()
                    .gap_4()
                    .child(div().text_lg().child("Outer"))
                    .child(
                        Switch::new("link-outer")
                            .label("Link all outer")
                            .checked(self.linked)
                            .disabled(!can_link)
                            .on_change(cx.listener(|this, linked, window, cx| {
                                this.toggle_link(*linked, window, cx);
                            })),
                    ),
            )
            .children(outer)
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
    fn monitor_gap_patterns_and_values_validate_inline(cx: &mut TestAppContext) {
        let original = "[gaps.inner]\nhorizontal = [{ monitor.main = 4 }, 8] # keep\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| GapsView::new(model, live_data, window, cx));
        in_window(cx, window, |window, cx| {
            window.click("monitor-pattern", cx);
            window.press("cmd-a", cx);
            window.input("secondary", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            original.replace("monitor.main", "monitor.secondary")
        );
        in_window(cx, window, |window, cx| {
            window.click("gap-rule-value", cx);
            window.press("cmd-a", cx);
            window.input("-1", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            original.replace("monitor.main", "monitor.secondary")
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);
    }

    #[gpui_kit::test]
    fn per_monitor_toggle_round_trip_and_two_undos_preserve_bytes(cx: &mut TestAppContext) {
        let original = "[gaps.inner]\nhorizontal = 0x10 # keep\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            GapsView::new(model.clone(), live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.click("per-monitor-InnerHorizontal", cx);
        });
        cx.run_until_parked();
        let per_monitor = fixture.disk();
        assert_eq!(
            per_monitor,
            "[gaps.inner]\nhorizontal = [{ monitor.main = 16 }, 16] # keep\n"
        );
        in_window(cx, window, |window, cx| {
            window.click("per-monitor-InnerHorizontal", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), "[gaps.inner]\nhorizontal = 16 # keep\n");
        assert_eq!(fixture.aerospace.reload_count(), 2);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), per_monitor);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 4);
    }

    #[gpui_kit::test]
    fn monitor_gap_rows_commit_reorder_and_remove(cx: &mut TestAppContext) {
        let original = "[gaps.inner]\nhorizontal = [{ monitor.main = 4 }, 8] # keep\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| {
            GapsView::new(model.clone(), live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.click("gap-rule-value", cx);
            window.press("cmd-a", cx);
            window.input("12", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original.replace("main = 4", "main = 12"));
        in_window(cx, window, |window, cx| {
            window.click("add-rule-InnerHorizontal", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.click(("gap-down", 0_usize), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[gaps.inner]\nhorizontal = [{ monitor.main = 8 }, { monitor.main = 12 }, 8] # keep\n"
        );
        in_window(cx, window, |window, cx| {
            window.click(("gap-remove", 1_usize), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[gaps.inner]\nhorizontal = [{ monitor.main = 8 }, 8] # keep\n"
        );
        in_window(cx, window, |window, cx| {
            window.click("gaps.inner.horizontal", cx);
            window.press("cmd-a", cx);
            window.input("20", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "[gaps.inner]\nhorizontal = [{ monitor.main = 8 }, 20] # keep\n"
        );
        assert_eq!(fixture.aerospace.reload_count(), 5);
        for _ in 0..5 {
            model.update(cx, ConfigModel::undo);
            cx.run_until_parked();
        }
        assert_eq!(fixture.disk(), original);
    }

    #[gpui_kit::test]
    fn linked_outer_is_one_write_and_undo(cx: &mut TestAppContext) {
        let original = "[gaps.outer]\nleft = 0\nright = 0\ntop = 0\nbottom = 0\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, view) = open_window(cx, |window, cx| {
            GapsView::new(model.clone(), live_data, window, cx)
        });
        in_window(cx, window, |window, cx| {
            window.click("link-outer", cx);
            window.click("gaps.outer.left", cx);
            window.press("cmd-a", cx);
            window.input("12", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original.replace("= 0", "= 12"));
        assert_eq!(fixture.aerospace.reload_count(), 1);
        model.update(cx, ConfigModel::undo);
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
        view.read_with(cx, |view, cx| {
            for (_, field) in &view.fields {
                assert_eq!(*field.read(cx).committed(), 0);
            }
        });
    }

    #[gpui_kit::test]
    fn different_outer_values_wait_for_a_choice(cx: &mut TestAppContext) {
        let original = "[gaps.outer]\nleft = 4\nright = 8\ntop = 4\nbottom = 8\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let live_data = fixture.live_data(cx);
        let (window, _) = open_window(cx, |window, cx| GapsView::new(model, live_data, window, cx));
        in_window(cx, window, |window, cx| window.click("link-outer", cx));
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
        assert_eq!(fixture.aerospace.reload_count(), 0);
        in_window(cx, window, |window, cx| window.click("link-4", cx));
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original.replace("= 8", "= 4"));
        assert_eq!(fixture.aerospace.reload_count(), 1);
    }
}
