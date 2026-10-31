use anyhow::Context as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, KeyBinding, ParentElement as _, Render,
    Styled as _, Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    px, size,
};

actions!(planeui, [Quit]);

struct PlaneUi {
    _subscriptions: Vec<Subscription>,
}

impl PlaneUi {
    fn new(window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let appearance = window.observe_window_appearance(|window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });
        Self {
            _subscriptions: vec![appearance],
        }
    }
}

impl Render for PlaneUi {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child("PlaneUI")
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("AeroSpace config editor"),
            )
            .child(
                Button::new("quit")
                    .primary()
                    .label("Quit")
                    .on_click(|_, _, cx| cx.quit()),
            )
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

        if let Err(error) = open_main_window(cx) {
            log::error!("{error:#}");
            cx.quit();
        }
    });
}

fn open_main_window(cx: &mut App) -> anyhow::Result<()> {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("PlaneUI".into()),
            ..TitlebarOptions::default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(960.), px(640.)), cx)),
        is_resizable: true,
        ..WindowOptions::default()
    };
    let (window, _view) = gpui_kit::open_window(options, cx, |window, cx| {
        Theme::sync_system_appearance(Some(window), cx);
        cx.new(|cx| PlaneUi::new(window, cx))
    })
    .context("failed to open the PlaneUI window")?;
    window
        .update(cx, |_, window, _| window.activate_window())
        .context("failed to activate the PlaneUI window")?;
    cx.activate(true);
    Ok(())
}
