//! Constant gaps, with one pipeline commit for a linked outer edit.

use gpui_kit::component::button::Button;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    TestSupportExt as _, Window, div,
};
use planeui_core::gaps::{GapField, GapValue, reset_gap, set_gap, set_outer_linked};

use crate::config_model::ConfigModel;
use crate::fields::{CommitField, CommitNumberField, Committed};

#[derive(Debug)]
pub struct GapsView {
    model: Entity<ConfigModel>,
    fields: Vec<(GapField, Entity<CommitNumberField>)>,
    linked: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

pub(crate) fn unsigned_number(value: i64) -> Result<(), SharedString> {
    u32::try_from(value)
        .map(|_| ())
        .map_err(|_| "Must be between 0 and 4294967295".into())
}

fn effective(value: &GapValue) -> i64 {
    match value {
        GapValue::Const(value) => i64::from(*value),
        _ => 0,
    }
}

impl GapsView {
    pub fn new(model: Entity<ConfigModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            cx.notify();
        }));
        Self {
            model,
            fields,
            linked: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn set(&mut self, gap: GapField, value: u32, cx: &mut Context<Self>) {
        let mut doc = self.model.read(cx).doc().clone();
        let result = if self.linked && GapField::OUTER.contains(&gap) {
            set_outer_linked(&mut doc, value)
        } else {
            set_gap(&mut doc, gap, value)
        };
        self.error = result.err().map(|error| error.to_string().into());
        if self.error.is_none() {
            self.model
                .update(cx, |model, cx| model.commit(doc, "Set gaps", cx));
        }
        cx.notify();
    }

    fn reset(&mut self, gap: GapField, cx: &mut Context<Self>) {
        let mut doc = self.model.read(cx).doc().clone();
        self.error = reset_gap(&mut doc, gap)
            .err()
            .map(|error| error.to_string().into());
        if self.error.is_none() {
            self.linked = false;
            self.model.update(cx, |model, cx| {
                model.commit(doc, "Reset gap to default", cx);
            });
        }
        cx.notify();
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
            GapValue::PerMonitor(raw) => v_flex()
                .gap_1()
                .child(raw.0.clone())
                .child("per-monitor (editable in a later version)")
                .into_any_element(),
            _ => div()
                .id(SharedString::from(path))
                .test_support()
                .w_40()
                .child(field.clone())
                .into_any_element(),
        };
        h_flex()
            .gap_4()
            .items_start()
            .child(div().w_24().child(label))
            .child(editor)
            .when(matches!(value, GapValue::Absent), |row| {
                row.child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child("default: 0"),
                )
            })
            .when(
                matches!(value, GapValue::Const(_) | GapValue::Invalid { .. }),
                |row| {
                    row.child(
                        Button::new(SharedString::from(format!("reset-{gap:?}")))
                            .label("Reset to default")
                            .on_click(cx.listener(move |this, _, _, cx| this.reset(gap, cx))),
                    )
                },
            )
            .when_some(
                match value {
                    GapValue::Invalid { raw } => Some(raw),
                    _ => None,
                },
                |row, raw| row.child(div().text_color(cx.theme().danger).child(raw)),
            )
            .into_any_element()
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
    fn linked_outer_is_one_write_and_undo(cx: &mut TestAppContext) {
        let original = "[gaps.outer]\nleft = 0\nright = 0\ntop = 0\nbottom = 0\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let (window, view) = open_window(cx, |window, cx| GapsView::new(model.clone(), window, cx));
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
        let (window, _) = open_window(cx, |window, cx| GapsView::new(model, window, cx));
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
