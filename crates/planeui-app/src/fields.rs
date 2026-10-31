//! Inputs that implement PlaneUI's commit rules once, so section editors cannot get them wrong.
//!
//! Text commits on Enter, on blur, or [`TEXT_DEBOUNCE`] after the last keystroke; a number's
//! stepper commits [`STEP_DEBOUNCE`] after the last click. Text the field's validator rejects
//! never commits: it stays in the field with an inline error until it is fixed. Toggles, selects
//! and pickers need no field: they commit from their change handler through
//! [`crate::config_model::ConfigModel::edit`].

use std::fmt;
use std::time::Duration;

use gpui_kit::component::input::{
    Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction,
};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div,
};

/// How long typing must pause before a text field commits.
pub const TEXT_DEBOUNCE: Duration = Duration::from_millis(700);
/// How long stepper clicks must pause before a number field commits.
pub const STEP_DEBOUNCE: Duration = Duration::from_millis(400);

/// A field's value is ready to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed<T>(pub T);

type Parse<T> = Box<dyn Fn(&str) -> Result<T, SharedString>>;
type Step<T> = Box<dyn Fn(&T, StepAction) -> T>;

/// A gpui-kit input that emits [`Committed`] under the commit rules.
pub struct CommitField<T> {
    input: Entity<InputState>,
    parse: Parse<T>,
    step: Option<Step<T>>,
    /// The value last committed or set from outside, so an unchanged value is not re-sent.
    committed: T,
    error: Option<SharedString>,
    debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// A free-text field.
pub type CommitTextField = CommitField<String>;
/// An integer field with a stepper.
pub type CommitNumberField = CommitField<i64>;

impl<T> EventEmitter<Committed<T>> for CommitField<T> where T: 'static {}

impl<T: fmt::Debug> fmt::Debug for CommitField<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommitField")
            .field("committed", &self.committed)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl CommitField<String> {
    /// A text field showing `initial`. `validate` returns the value to commit or the error to
    /// show.
    pub fn text(
        initial: String,
        validate: impl Fn(&str) -> Result<String, SharedString> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(initial, Box::new(validate), None, window, cx)
    }
}

impl CommitField<i64> {
    /// An integer field showing `initial` whose stepper moves by one. `validate` checks a parsed
    /// value; steps it rejects are not taken.
    pub fn number(
        initial: i64,
        validate: impl Fn(i64) -> Result<(), SharedString> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let parse = move |text: &str| {
            let value = text
                .trim()
                .parse::<i64>()
                .map_err(|_| SharedString::from("Enter a whole number"))?;
            validate(value).map(|()| value)
        };
        let step = |value: &i64, action| match action {
            StepAction::Increment => value.saturating_add(1),
            StepAction::Decrement => value.saturating_sub(1),
        };
        Self::new(initial, Box::new(parse), Some(Box::new(step)), window, cx)
    }
}

impl<T> CommitField<T>
where
    T: Clone + PartialEq + fmt::Display + 'static,
{
    fn new(
        initial: T,
        parse: Parse<T>,
        step: Option<Step<T>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).default_value(initial.to_string());
            // Without a built-in step the stepper emits `NumberInputEvent::Step`, so stepping
            // goes through the validator and the stepper debounce.
            input.set_step(None, window, cx);
            input
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => this.commit_after(TEXT_DEBOUNCE, cx),
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_now(cx),
                    InputEvent::Focus => {}
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = event;
                    this.step(*action, window, cx);
                },
            ),
        ];
        Self {
            input,
            parse,
            step,
            committed: initial,
            error: None,
            debounce: None,
            _subscriptions: subscriptions,
        }
    }

    /// The validator's message for the text in the field, if it rejects it.
    #[must_use]
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Whether the field holds text it will not commit, for section error badges.
    #[must_use]
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// The input's state, for focusing it.
    #[must_use]
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// Shows `value` without committing it, such as after an undo or a reload from disk.
    /// Replaces any text being typed.
    pub fn set_value(&mut self, value: T, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = None;
        self.error = None;
        self.input.update(cx, |input, cx| {
            input.set_value(value.to_string(), window, cx);
        });
        self.committed = value;
        cx.notify();
    }

    /// The value the field last committed or was set to.
    #[must_use]
    pub fn committed(&self) -> &T {
        &self.committed
    }

    fn step(&mut self, action: StepAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(step) = &self.step else {
            return;
        };
        let text = self.input.read(cx).value();
        let current = (self.parse)(&text).unwrap_or_else(|_| self.committed.clone());
        let next = step(&current, action).to_string();
        if (self.parse)(&next).is_err() {
            return;
        }
        self.input
            .update(cx, |input, cx| input.set_value(next, window, cx));
        self.error = None;
        self.commit_after(STEP_DEBOUNCE, cx);
        cx.notify();
    }

    /// Replacing the stored task cancels the previous wait, which is the debounce.
    fn commit_after(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            if let Err(error) = this.update(cx, Self::commit_now) {
                log::debug!("a field closed before its debounce fired: {error}");
            }
        }));
    }

    fn commit_now(&mut self, cx: &mut Context<Self>) {
        self.debounce = None;
        let text = self.input.read(cx).value();
        match (self.parse)(&text) {
            Ok(value) => {
                self.error = None;
                if self.committed != value {
                    self.committed = value.clone();
                    cx.emit(Committed(value));
                }
            }
            Err(message) => self.error = Some(message),
        }
        cx.notify();
    }
}

impl<T> Render for CommitField<T>
where
    T: 'static,
{
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = if self.step.is_some() {
            NumberInput::new(&self.input).into_any_element()
        } else {
            Input::new(&self.input).into_any_element()
        };
        v_flex()
            .gap_1()
            .child(input)
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui_kit::TestSupportExt as _;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
        Render, Subscription, TestAppContext, Window, div, px,
    };

    use super::*;
    use crate::test_support::{in_window, open_window};

    struct Harness<T> {
        field: Entity<CommitField<T>>,
        elsewhere: FocusHandle,
        commits: Vec<T>,
        _subscription: Subscription,
    }

    impl<T: 'static> Render for Harness<T> {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .p_4()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .id("field")
                        .test_support()
                        .w(px(240.))
                        .child(self.field.clone()),
                )
                .child(
                    div()
                        .id("elsewhere")
                        .size(px(40.))
                        .track_focus(&self.elsewhere),
                )
        }
    }

    fn open<T: Clone + 'static>(
        cx: &mut TestAppContext,
        build: impl FnOnce(&mut Window, &mut Context<CommitField<T>>) -> CommitField<T>,
    ) -> (AnyWindowHandle, Entity<Harness<T>>) {
        open_window(cx, |window, cx| {
            let field = cx.new(|cx| build(window, cx));
            let subscription = cx.subscribe(&field, |this: &mut Harness<T>, _, event, _| {
                let Committed(value) = event;
                this.commits.push(value.clone());
            });
            Harness {
                field,
                elsewhere: cx.focus_handle(),
                commits: Vec::new(),
                _subscription: subscription,
            }
        })
    }

    fn padding(window: &mut Window, cx: &mut Context<CommitNumberField>) -> CommitNumberField {
        CommitField::number(
            30,
            |value| {
                if value < 0 {
                    Err("Must be 0 or more".into())
                } else {
                    Ok(())
                }
            },
            window,
            cx,
        )
    }

    fn select_field_text(cx: &mut TestAppContext, window: AnyWindowHandle) {
        in_window(cx, window, |window, cx| {
            window.click("field", cx);
            window.press("cmd-a", cx);
        });
    }

    fn type_text(cx: &mut TestAppContext, window: AnyWindowHandle, text: &str) {
        in_window(cx, window, |window, cx| window.input(text, cx));
    }

    fn press(cx: &mut TestAppContext, window: AnyWindowHandle, key: &str) {
        in_window(cx, window, |window, cx| window.press(key, cx));
    }

    fn blur<T: 'static>(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        harness: &Entity<Harness<T>>,
    ) {
        let elsewhere = harness.read_with(cx, |harness, _| harness.elsewhere.clone());
        in_window(cx, window, |window, cx| {
            window.focus(&elsewhere, cx);
            window.render_frame(cx);
        });
        cx.run_until_parked();
    }

    fn wait(cx: &mut TestAppContext, millis: u64) {
        cx.executor().advance_clock(Duration::from_millis(millis));
    }

    fn commits<T: Clone + 'static>(
        cx: &mut TestAppContext,
        harness: &Entity<Harness<T>>,
    ) -> Vec<T> {
        harness.read_with(cx, |harness, _| harness.commits.clone())
    }

    fn field_error(cx: &mut TestAppContext, harness: &Entity<Harness<i64>>) -> Option<String> {
        harness.read_with(cx, |harness, cx| {
            harness.field.read(cx).error().map(ToString::to_string)
        })
    }

    #[gpui_kit::test]
    fn typing_commits_once_after_the_pause(cx: &mut TestAppContext) {
        let (window, harness) = open(cx, padding);
        select_field_text(cx, window);
        for character in ["1", "2", "3", "4", "5"] {
            type_text(cx, window, character);
            wait(cx, 100);
        }
        wait(cx, 500);
        assert_eq!(
            commits(cx, &harness),
            Vec::<i64>::new(),
            "600 ms after the last key"
        );
        wait(cx, 200);
        assert_eq!(commits(cx, &harness), vec![12345]);
    }

    #[gpui_kit::test]
    fn enter_commits_immediately(cx: &mut TestAppContext) {
        let (window, harness) = open(cx, padding);
        select_field_text(cx, window);
        type_text(cx, window, "7");
        press(cx, window, "enter");
        assert_eq!(commits(cx, &harness), vec![7]);
        wait(cx, 1000);
        assert_eq!(
            commits(cx, &harness),
            vec![7],
            "the debounce does not commit again"
        );
    }

    #[gpui_kit::test]
    fn blur_commits(cx: &mut TestAppContext) {
        let (window, harness) = open(cx, padding);
        select_field_text(cx, window);
        type_text(cx, window, "8");
        blur(cx, window, &harness);
        assert_eq!(commits(cx, &harness), vec![8]);
    }

    #[gpui_kit::test]
    fn invalid_text_never_commits_and_keeps_its_error(cx: &mut TestAppContext) {
        let (window, harness) = open(cx, padding);
        select_field_text(cx, window);
        type_text(cx, window, "-5");
        wait(cx, 1000);
        assert_eq!(
            field_error(cx, &harness).as_deref(),
            Some("Must be 0 or more")
        );
        press(cx, window, "enter");
        blur(cx, window, &harness);

        assert_eq!(commits(cx, &harness), Vec::<i64>::new());
        assert_eq!(
            field_error(cx, &harness).as_deref(),
            Some("Must be 0 or more")
        );
        let text = harness.read_with(cx, |harness, cx| {
            harness.field.read(cx).input().read(cx).value().to_string()
        });
        assert_eq!(text, "-5", "the invalid text stays for the user to fix");
    }

    #[gpui_kit::test]
    fn stepper_clicks_commit_once_after_the_pause(cx: &mut TestAppContext) {
        let (window, harness) = open(cx, padding);
        for _ in 0..4 {
            in_window(cx, window, |window, cx| {
                window.within("field").click("increment", cx);
            });
            wait(cx, 100);
        }
        wait(cx, 200);
        assert_eq!(
            commits(cx, &harness),
            Vec::<i64>::new(),
            "300 ms after the last click"
        );
        wait(cx, 200);
        assert_eq!(commits(cx, &harness), vec![34]);
    }

    #[gpui_kit::test]
    fn text_field_commits_valid_text_only(cx: &mut TestAppContext) {
        let (window, harness) = open(cx, |window, cx| {
            CommitField::text(
                "main".to_owned(),
                |text| {
                    if text.is_empty() {
                        Err("Enter a name".into())
                    } else {
                        Ok(text.to_owned())
                    }
                },
                window,
                cx,
            )
        });
        select_field_text(cx, window);
        press(cx, window, "backspace");
        press(cx, window, "enter");
        type_text(cx, window, "service");
        press(cx, window, "enter");
        assert_eq!(commits(cx, &harness), vec!["service".to_owned()]);
    }
}
