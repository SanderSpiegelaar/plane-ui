//! A debug control for `accordion-padding` that runs every change through the whole pipeline.
//! PUI-043 replaces it with the General section.

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, TestSupportExt as _, Window, div, px,
};
use planeui_core::config_doc::KeyPath;

use crate::config_model::ConfigModel;
use crate::fields::{CommitField, CommitNumberField, Committed};

/// AeroSpace's value when the key is absent.
const DEFAULT_ACCORDION_PADDING: i64 = 30;

/// The `accordion-padding` number field, kept in step with the model.
#[derive(Debug)]
pub struct AccordionPaddingControl {
    model: Entity<ConfigModel>,
    field: Entity<CommitNumberField>,
    _subscriptions: Vec<Subscription>,
}

impl AccordionPaddingControl {
    /// A control editing `model`'s `accordion-padding`.
    pub fn new(model: Entity<ConfigModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial = padding(model.read(cx));
        let field = cx.new(|cx| CommitField::number(initial, non_negative, window, cx));
        let subscriptions = vec![
            cx.subscribe(&field, |this, _, event, cx| {
                let Committed(padding) = event;
                this.commit(*padding, cx);
            }),
            // Undo, redo and reloads from disk change the value without the field.
            cx.observe_in(&model, window, |this, model, window, cx| {
                let padding = padding(model.read(cx));
                this.field.update(cx, |field, cx| {
                    if *field.committed() != padding {
                        field.set_value(padding, window, cx);
                    }
                });
            }),
        ];
        Self {
            model,
            field,
            _subscriptions: subscriptions,
        }
    }

    fn commit(&self, padding: i64, cx: &mut Context<Self>) {
        let result = self.model.update(cx, |model, cx| {
            model.edit(format!("Set accordion-padding to {padding}"), cx, |doc| {
                doc.set_value(&KeyPath::from(["accordion-padding"]), padding.into())
            })
        });
        if let Err(error) = result {
            log::error!("could not set accordion-padding: {error}");
        }
    }
}

fn padding(model: &ConfigModel) -> i64 {
    model
        .view()
        .general
        .accordion_padding
        .value()
        .copied()
        .unwrap_or(DEFAULT_ACCORDION_PADDING)
}

fn non_negative(padding: i64) -> Result<(), SharedString> {
    if padding < 0 {
        Err("Must be 0 or more".into())
    } else {
        Ok(())
    }
}

impl Render for AccordionPaddingControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .w(px(260.))
            .child(div().text_sm().child("accordion-padding"))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Debug control: each change is validated, snapshotted, written and reloaded."),
            )
            .child(
                div()
                    .id("accordion-padding")
                    .test_support()
                    .child(self.field.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt as _;

    use super::*;
    use crate::test_support::{Fixture, in_window, open_window};

    #[gpui_kit::test]
    fn the_field_round_trips_through_the_pipeline(cx: &mut TestAppContext) {
        let original = "accordion-padding = 30 # px\n";
        let fixture = Fixture::new(original);
        let model = fixture.model(cx);
        let (window, control) = open_window(cx, |window, cx| {
            AccordionPaddingControl::new(model.clone(), window, cx)
        });

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
        let shown = control.read_with(cx, |control, cx| {
            control.field.read(cx).input().read(cx).value().to_string()
        });
        assert_eq!(shown, "30", "the field follows the undo");
    }
}
