//! The permanent window layout: section sidebar, content pane, reload-error and auto-reload
//! banners, status bar and toasts.

use std::time::SystemTime;

use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::sidebar::{Sidebar, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, TestSupportExt as _, Window, div,
};
use planeui_core::aerospace::newer_than_catalogue;
use planeui_core::commands::CATALOGUE_VERSION;
use planeui_core::config_doc::KeyPath;
use planeui_core::monitors::ASSIGNMENTS;
use planeui_core::prefs::{Prefs, PrefsStore};
use planeui_core::store::UtcDateTime;

use crate::config_model::{AerospaceInfo, AerospaceState, ConfigEvent, ConfigModel, SaveStatus};
use crate::gaps_view::GapsView;
use crate::general_view::{AutoReloadEnabled, GeneralView};
use crate::keybindings::KeybindingsView;
use crate::live_data::LiveData;
use crate::rules_view::RulesView;
use crate::snapshots::SnapshotsView;
use crate::workspaces_view::WorkspacesView;
use crate::{NextSection, PreviousSection, Redo, ShowSection, Undo};

/// The key context in which the arrow keys move between sections.
pub const SECTIONS_CONTEXT: &str = "Sections";

/// A sidebar entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Keybindings per mode.
    Keybindings,
    /// Inner and outer gaps.
    Gaps,
    /// General settings.
    General,
    /// `on-window-detected` rules.
    WindowRules,
    /// Workspace-to-monitor assignment.
    WorkspacesAndMonitors,
    /// Snapshots taken before each write.
    Snapshots,
}

impl Section {
    /// Every section in sidebar order.
    pub const ALL: [Self; 6] = [
        Self::Keybindings,
        Self::Gaps,
        Self::General,
        Self::WindowRules,
        Self::WorkspacesAndMonitors,
        Self::Snapshots,
    ];

    /// The sidebar label.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Keybindings => "Keybindings",
            Self::Gaps => "Gaps",
            Self::General => "General",
            Self::WindowRules => "Window rules",
            Self::WorkspacesAndMonitors => "Workspaces and monitors",
            Self::Snapshots => "Snapshots",
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|section| *section == self)
            .unwrap_or_default()
    }

    fn previous(self) -> Self {
        let index = self.index().saturating_sub(1);
        Self::ALL.get(index).copied().unwrap_or(self)
    }

    fn next(self) -> Self {
        Self::ALL.get(self.index() + 1).copied().unwrap_or(self)
    }

    /// The section that edits `key`. Keys no form edits belong to General, which lists them.
    fn owning(key: Option<&KeyPath>) -> Self {
        match key
            .and_then(|key| key.segments().first())
            .map(String::as_str)
        {
            Some("mode") => Self::Keybindings,
            Some("gaps") => Self::Gaps,
            Some("on-window-detected") => Self::WindowRules,
            Some("persistent-workspaces" | ASSIGNMENTS) => Self::WorkspacesAndMonitors,
            _ => Self::General,
        }
    }
}

/// The main window's content once a config is open.
#[derive(Debug)]
pub struct Shell {
    model: Entity<ConfigModel>,
    live_data: Entity<LiveData>,
    section: Section,
    general: Entity<GeneralView>,
    keybindings: Entity<KeybindingsView>,
    gaps: Entity<GapsView>,
    rules: Entity<RulesView>,
    workspaces: Entity<WorkspacesView>,
    /// Built when the Snapshots section opens and dropped when it closes, so the list is read
    /// fresh each time and not at all otherwise.
    snapshots: Option<Entity<SnapshotsView>>,
    prefs_store: PrefsStore,
    prefs: Prefs,
    prefs_save: Option<Task<()>>,
    focus_handle: FocusHandle,
    sections_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// The editor for `model`, landing on Keybindings. `prefs` were read from `prefs_store`,
    /// where changes to them are saved.
    pub fn new(
        model: Entity<ConfigModel>,
        prefs_store: PrefsStore,
        prefs: Prefs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let aerospace = model.read(cx).aerospace_client();
        let live_data = cx.new(|cx| LiveData::new(aerospace, cx));
        let rules = cx.new(|cx| RulesView::new(model.clone(), live_data.clone(), window, cx));
        let general = cx.new(|cx| GeneralView::new(model.clone(), window, cx));
        let keybindings = cx.new(|cx| KeybindingsView::new(model.clone(), window, cx));
        let gaps = cx.new(|cx| GapsView::new(model.clone(), live_data.clone(), window, cx));
        let workspaces =
            cx.new(|cx| WorkspacesView::new(model.clone(), live_data.clone(), window, cx));
        // Undo and Redo dispatch from the focused element, so the shell holds focus until a
        // control takes it.
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let subscriptions = vec![
            cx.subscribe_in(
                &general,
                window,
                |this, _, _: &AutoReloadEnabled, window, cx| {
                    this.keep_auto_reload(window, cx);
                },
            ),
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe_in(&model, window, |_, _, event: &ConfigEvent, window, cx| {
                window.push_notification(notification(event), cx);
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.model.update(cx, ConfigModel::check_disk);
                    this.live_data.update(cx, LiveData::refresh);
                }
            }),
        ];
        Self {
            model,
            live_data,
            section: Section::Keybindings,
            general,
            keybindings,
            gaps,
            rules,
            workspaces,
            snapshots: None,
            prefs_store,
            prefs,
            prefs_save: None,
            focus_handle,
            sections_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// The section shown in the content pane.
    #[must_use]
    pub fn section(&self) -> Section {
        self.section
    }

    /// Shared live data for form pickers and monitor previews.
    #[must_use]
    pub fn live_data(&self) -> &Entity<LiveData> {
        &self.live_data
    }

    /// Shows `section`.
    pub fn select(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        self.section = section;
        self.snapshots = (section == Section::Snapshots).then(|| {
            self.snapshots.take().unwrap_or_else(|| {
                let model = self.model.clone();
                cx.new(|cx| SnapshotsView::new(model, window, cx))
            })
        });
        cx.notify();
    }

    /// Whether `section` holds a field that will not commit, or the document has an error in
    /// a key the section edits.
    fn has_errors(&self, section: Section, cx: &App) -> bool {
        let invalid_field = match section {
            Section::Keybindings => self.keybindings.read(cx).has_invalid_field(cx),
            Section::Gaps => self.gaps.read(cx).has_invalid_field(cx),
            Section::General => self.general.read(cx).has_invalid_field(cx),
            Section::WindowRules => self.rules.read(cx).has_invalid_field(cx),
            Section::WorkspacesAndMonitors => self.workspaces.read(cx).has_invalid_field(cx),
            Section::Snapshots => false,
        };
        invalid_field
            || self
                .model
                .read(cx)
                .report()
                .errors()
                .any(|issue| Section::owning(issue.key_path.as_ref()) == section)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = Section::ALL.map(|section| {
            SidebarMenuItem::new(section.title())
                .active(section == self.section)
                .when(self.has_errors(section, cx), |item| {
                    item.suffix(move |_, cx| {
                        div()
                            .id(SharedString::from(format!("{section:?}-errors")))
                            .test_support()
                            .role(Role::Status)
                            .aria_label("Has errors")
                            .size_2()
                            .rounded_full()
                            .bg(cx.theme().danger)
                    })
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    window.focus(&this.sections_focus, cx);
                    this.select(section, window, cx);
                }))
        });
        div()
            .id("sections")
            .role(Role::Tree)
            .aria_label("Sections")
            .key_context(SECTIONS_CONTEXT)
            .track_focus(&self.sections_focus)
            .on_action(cx.listener(|this, _: &PreviousSection, window, cx| {
                this.select(this.section.previous(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextSection, window, cx| {
                this.select(this.section.next(), window, cx);
            }))
            .h_full()
            .child(
                Sidebar::new("sidebar")
                    .collapsible(false)
                    .child(SidebarMenu::new().children(items)),
            )
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let model = self.model.read(cx);
        let SaveStatus::ReloadFailed { messages } = model.status() else {
            return None;
        };
        let text = messages
            .iter()
            .map(|message| match &message.key_path {
                Some(key_path) => format!("{key_path}: {}", message.message),
                None => message.message.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let can_undo = model.undo_label().is_some();
        Some(
            h_flex()
                .id("reload-error")
                .test_support()
                .items_start()
                .gap_3()
                .p_3()
                .child(
                    Alert::error("reload-error-alert", text)
                        .title("AeroSpace reported errors in the config")
                        .flex_1(),
                )
                .when(can_undo, |this| {
                    this.child(
                        Button::new("undo-last-change")
                            .label("Undo last change")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, ConfigModel::undo);
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_auto_reload_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let model = self.model.read(cx);
        let auto_reload = model
            .view()
            .general
            .setting(planeui_core::settings::Setting::AutoReloadConfig)
            .value()
            == Some(&planeui_core::settings::SettingValue::Bool(true));
        if !auto_reload || self.prefs.is_auto_reload_dismissed(model.display_path()) {
            return None;
        }
        Some(
            h_flex()
                .id("auto-reload-banner")
                .test_support()
                .items_start()
                .gap_3()
                .p_3()
                .child(
                    Alert::info(
                        "auto-reload-alert",
                        "PlaneUI already reloads AeroSpace after every change, so with \
                         auto-reload-config on each change is loaded twice.",
                    )
                    .title("auto-reload-config is on")
                    .flex_1()
                    .min_w_0(),
                )
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .gap_2()
                        .child(
                            Button::new("turn-off-auto-reload")
                                .primary()
                                .label("Turn off")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.turn_off_auto_reload(window, cx);
                                })),
                        )
                        .child(Button::new("keep-auto-reload").label("Keep on").on_click(
                            cx.listener(|this, _, window, cx| this.keep_auto_reload(window, cx)),
                        )),
                )
                .into_any_element(),
        )
    }

    fn turn_off_auto_reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let edited = self.model.update(cx, |model, cx| {
            model.edit("Turn off auto-reload-config", cx, |doc| {
                doc.set_value(&KeyPath::from(["auto-reload-config"]), false.into())
            })
        });
        if let Err(error) = edited {
            log::error!("could not turn off auto-reload-config: {error}");
            window.push_notification(
                Notification::error(error.to_string()).title("Could not turn off auto-reload"),
                cx,
            );
        }
    }

    fn keep_auto_reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = self.model.read(cx).display_path().to_path_buf();
        self.prefs.dismiss_auto_reload(&config);
        cx.notify();

        let store = self.prefs_store.clone();
        let prefs = self.prefs.clone();
        self.prefs_save = Some(cx.spawn_in(window, async move |this, cx| {
            let saved = cx.background_spawn(async move { store.save(&prefs) }).await;
            let Err(error) = saved else { return };
            log::error!("{error}");
            let shown = this.update_in(cx, |_, window, cx| {
                window.push_notification(
                    Notification::error(error.to_string())
                        .title("PlaneUI will offer this again next time"),
                    cx,
                );
            });
            if let Err(error) = shown {
                log::debug!("the shell closed before the preferences were saved: {error}");
            }
        }));
    }

    fn render_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let heading = div().text_lg().child(self.section.title());
        let body = match (self.section, &self.snapshots) {
            (Section::Keybindings, _) => self.keybindings.clone().into_any_element(),
            (Section::Gaps, _) => self.gaps.clone().into_any_element(),
            (Section::WorkspacesAndMonitors, _) => self.workspaces.clone().into_any_element(),
            (Section::General, _) => self.general.clone().into_any_element(),
            (Section::WindowRules, _) => self.rules.clone().into_any_element(),
            (Section::Snapshots, Some(snapshots)) => snapshots.clone().into_any_element(),
            _ => div()
                .text_color(cx.theme().muted_foreground)
                .child("This section is not editable yet.")
                .into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .p_6()
            .gap_4()
            .child(heading)
            .child(body)
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let path: SharedString = model.display_path().display().to_string().into();
        let aerospace = aerospace_text(model.aerospace());
        let status = status_text(&model.status());
        let muted = cx.theme().muted_foreground;
        let warning = compatibility_warning(model.aerospace());
        let status_bar = StatusBar::new()
            .left(div().text_color(muted).child(path))
            .child(
                div()
                    .id("aerospace-state")
                    .role(Role::Status)
                    .aria_label(aerospace.clone())
                    .test_support()
                    .text_color(muted)
                    .child(aerospace),
            )
            .right(
                div()
                    .id("save-status")
                    .role(Role::Status)
                    .aria_label(status.clone())
                    .test_support()
                    .child(status),
            );
        v_flex()
            .child(status_bar)
            .when_some(warning, |footer, message| {
                footer.child(
                    StatusBar::new().child(
                        div()
                            .id("catalogue-warning")
                            .role(Role::Status)
                            .aria_label(message.clone())
                            .test_support()
                            .text_color(cx.theme().warning)
                            .child(message),
                    ),
                )
            })
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let banner = self.render_banner(cx);
        let auto_reload_banner = self.render_auto_reload_banner(cx);
        v_flex()
            .id("shell")
            .role(Role::Group)
            .aria_label("PlaneUI")
            .track_focus(&self.focus_handle)
            // Cmd-Z in a text field never gets here: the field's own binding wins. Only the
            // menu's Undo does, so it is handed to the field to match the key.
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                if window.is_action_available(&input::Undo, cx) {
                    window.dispatch_action(Box::new(input::Undo), cx);
                } else {
                    this.model.update(cx, ConfigModel::undo);
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, window, cx| {
                if window.is_action_available(&input::Redo, cx) {
                    window.dispatch_action(Box::new(input::Redo), cx);
                } else {
                    this.model.update(cx, ConfigModel::redo);
                }
            }))
            .on_action(cx.listener(|this, action: &ShowSection, window, cx| {
                this.select(action.0, window, cx);
            }))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_sidebar(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .children(banner)
                            .children(auto_reload_banner)
                            .child(self.render_content(cx)),
                    ),
            )
            .child(self.render_status_bar(cx))
    }
}

fn notification(event: &ConfigEvent) -> Notification {
    match event {
        ConfigEvent::ReloadedFromDisk { discarded } => {
            let message = match discarded {
                Some(label) => {
                    format!("The file changed outside PlaneUI, so “{label}” was not saved.")
                }
                None => "The file changed outside PlaneUI.".to_owned(),
            };
            Notification::info(message).title("Reloaded from disk")
        }
        ConfigEvent::RestoreFailed { message } => {
            Notification::error(message.clone()).title("Snapshot not restored")
        }
    }
}

/// The status bar's description of the latest change.
fn status_text(status: &SaveStatus) -> SharedString {
    match status {
        SaveStatus::Idle => "No changes yet".into(),
        SaveStatus::Writing => "Saving…".into(),
        SaveStatus::Applied { at } => {
            format!("Applied {}", display_time(*at, TimeFormat::Clock)).into()
        }
        SaveStatus::ReloadFailed { .. } => "Saved, but AeroSpace reported errors".into(),
        SaveStatus::NotRunning => "AeroSpace not running: changes apply on next start".into(),
        SaveStatus::NotInstalled => "AeroSpace not found: changes apply on next start".into(),
        SaveStatus::Error(message) => message.clone().into(),
    }
}

fn aerospace_text(info: &AerospaceInfo) -> SharedString {
    let name = info.version.as_ref().map_or_else(
        || "AeroSpace".to_owned(),
        |version| format!("AeroSpace {version}"),
    );
    match info.state {
        AerospaceState::Unknown => name.into(),
        AerospaceState::Running => format!("{name}, running").into(),
        AerospaceState::NotRunning => format!("{name}, not running").into(),
        AerospaceState::NotInstalled => "AeroSpace not installed".into(),
    }
}

fn compatibility_warning(info: &AerospaceInfo) -> Option<SharedString> {
    let installed = info
        .version
        .as_ref()
        .filter(|version| newer_than_catalogue(version))?;
    let catalogue = CATALOGUE_VERSION
        .rsplit_once('.')
        .map_or(CATALOGUE_VERSION, |(series, _)| series);
    Some(format!("AeroSpace {}.{} is newer than PlaneUI's command catalogue ({catalogue}): unknown commands stay read-only", installed.major, installed.minor).into())
}

/// How much of a time [`display_time`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    /// `17:15:03 UTC`.
    Clock,
    /// `2026-10-31 17:15:03 UTC`.
    DateAndClock,
}

/// Std has no time zone database, so times are shown in UTC and say so. Every time PlaneUI shows
/// goes through here, so switching to local time is a change in one place.
#[must_use]
pub fn display_time(at: SystemTime, format: TimeFormat) -> String {
    let Some(utc) = UtcDateTime::from_system_time(at) else {
        return "before 1970".to_owned();
    };
    let clock = format!("{:02}:{:02}:{:02} UTC", utc.hour, utc.minute, utc.second);
    match format {
        TimeFormat::Clock => clock,
        TimeFormat::DateAndClock => {
            format!("{:04}-{:02}-{:02} {clock}", utc.year, utc.month, utc.day)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
    use gpui_kit::{AnyWindowHandle, Focusable as _, TestAppContext};
    use planeui_core::aerospace::{ReloadMessage, ReloadOutcome};

    use super::*;
    use crate::test_support::{Fixture, in_window, open_window, set_padding};

    const ORIGINAL: &str = "accordion-padding = 30 # px\n";

    fn open_shell(cx: &mut TestAppContext, fixture: &Fixture) -> (AnyWindowHandle, Entity<Shell>) {
        let model = fixture.model(cx);
        let prefs_store = fixture.prefs_store();
        let prefs = prefs_store.load().unwrap_or_default();
        open_window(cx, |window, cx| {
            Shell::new(model, prefs_store, prefs, window, cx)
        })
    }

    fn model(cx: &mut TestAppContext, shell: &Entity<Shell>) -> Entity<ConfigModel> {
        shell.read_with(cx, |shell, _| shell.model.clone())
    }

    fn status_label(cx: &mut TestAppContext, window: AnyWindowHandle, id: &'static str) -> String {
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.find(id).label().unwrap_or_default().to_owned()
        })
    }

    #[gpui_kit::test]
    fn newer_aerospace_series_shows_the_catalogue_warning(cx: &mut TestAppContext) {
        use planeui_core::aerospace::{AerospaceVersion, FakeAerospace};

        let fixture = Fixture::with_aerospace(
            ORIGINAL,
            FakeAerospace::new().with_version(AerospaceVersion {
                major: 0,
                minor: 22,
                patch: 1,
                pre_release: Some("Beta".into()),
            }),
        );
        let (window, shell) = open_shell(cx, &fixture);
        model(cx, &shell).update(cx, ConfigModel::refresh_aerospace);
        cx.run_until_parked();
        assert_eq!(
            status_label(cx, window, "catalogue-warning"),
            "AeroSpace 0.22 is newer than PlaneUI's command catalogue (0.21): unknown commands stay read-only"
        );
        assert_eq!(status_label(cx, window, "save-status"), "No changes yet");
        assert_eq!(fixture.disk(), ORIGINAL);
        assert_eq!(fixture.aerospace.reload_count(), 0);
    }

    #[gpui_kit::test]
    fn same_aerospace_series_does_not_show_a_catalogue_warning(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        model(cx, &shell).update(cx, ConfigModel::refresh_aerospace);
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("catalogue-warning").is_none());
        });
    }

    #[gpui_kit::test]
    fn window_activation_refreshes_live_data_after_the_throttle(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, _) = open_shell(cx, &fixture);
        assert_eq!(fixture.aerospace.list_count(), 3);
        let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
        visual.deactivate_window();
        in_window(cx, window, |window, _| window.activate_window());
        cx.run_until_parked();
        assert_eq!(fixture.aerospace.list_count(), 3);
        cx.executor().advance_clock(Duration::from_secs(3));
        visual.deactivate_window();
        in_window(cx, window, |window, _| window.activate_window());
        cx.run_until_parked();
        assert_eq!(fixture.aerospace.list_count(), 6);
    }

    #[gpui_kit::test]
    fn a_failed_reload_shows_the_banner_and_its_undo_reverts(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        fixture.aerospace.queue_reload(Ok(ReloadOutcome::Failed {
            errors: vec![ReloadMessage {
                key_path: Some("accordion-padding".to_owned()),
                message: "bad value".to_owned(),
            }],
            raw: "[ERROR] accordion-padding: bad value".to_owned(),
        }));
        let (window, shell) = open_shell(cx, &fixture);
        set_padding(&model(cx, &shell), 12, cx);
        cx.run_until_parked();

        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.find("reload-error").visible());
            window.click("undo-last-change", cx);
        });
        cx.run_until_parked();

        assert_eq!(fixture.disk(), ORIGINAL);
        assert_eq!(fixture.aerospace.reload_count(), 2);
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("reload-error").is_none());
        });
    }

    #[gpui_kit::test]
    fn the_status_bar_reflects_the_latest_apply(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        let model = model(cx, &shell);
        model.update(cx, ConfigModel::refresh_aerospace);
        cx.run_until_parked();
        assert_eq!(
            status_label(cx, window, "aerospace-state"),
            "AeroSpace 0.21.3-Beta, not running"
        );
        assert_eq!(status_label(cx, window, "save-status"), "No changes yet");

        set_padding(&model, 1, cx);
        assert_eq!(status_label(cx, window, "save-status"), "Saving…");
        cx.run_until_parked();
        let applied = status_label(cx, window, "save-status");
        assert!(
            applied.starts_with("Applied ") && applied.ends_with(" UTC"),
            "{applied}"
        );
        assert_eq!(
            status_label(cx, window, "aerospace-state"),
            "AeroSpace 0.21.3-Beta, running"
        );

        fixture
            .aerospace
            .queue_reload(Ok(ReloadOutcome::NotRunning));
        set_padding(&model, 2, cx);
        cx.run_until_parked();
        assert_eq!(
            status_label(cx, window, "save-status"),
            "AeroSpace not running: changes apply on next start"
        );

        fixture.aerospace.queue_reload(Ok(ReloadOutcome::Failed {
            errors: Vec::new(),
            raw: String::new(),
        }));
        set_padding(&model, 3, cx);
        cx.run_until_parked();
        assert_eq!(
            status_label(cx, window, "save-status"),
            "Saved, but AeroSpace reported errors"
        );
    }

    #[gpui_kit::test]
    async fn activating_the_window_reloads_an_external_edit(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        let external = "accordion-padding = 99\n";
        fixture.write_disk(external);

        cx.add_empty_window()
            .update(|other, _| other.activate_window());
        cx.run_until_parked();
        in_window(cx, window, |window, _| window.activate_window());
        cx.run_until_parked();

        let shown = model(cx, &shell).read_with(cx, |model, _| model.doc().to_string());
        assert_eq!(shown, external);
        cx.wait_for(window, Duration::from_secs(1), |window, _| {
            window.try_find("notification").is_some()
        })
        .await;
        assert_eq!(fixture.disk(), external, "reloading never writes");
    }

    fn banner_shown(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.try_find("auto-reload-banner").is_some()
        })
    }

    #[gpui_kit::test]
    fn turning_off_auto_reload_writes_false_and_cmd_z_restores_it(cx: &mut TestAppContext) {
        let original =
            "# Reload on save\nauto-reload-config = true # mine\naccordion-padding = 30\n";
        let fixture = Fixture::new(original);
        let (window, _) = open_shell(cx, &fixture);
        assert!(banner_shown(cx, window));

        in_window(cx, window, |window, cx| {
            window.click("turn-off-auto-reload", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            "# Reload on save\nauto-reload-config = false # mine\naccordion-padding = 30\n"
        );
        assert_eq!(fixture.snapshots(), vec![original.to_owned()]);
        assert_eq!(fixture.aerospace.reload_count(), 1);
        assert!(!banner_shown(cx, window));

        in_window(cx, window, |window, cx| window.press("cmd-z", cx));
        cx.run_until_parked();
        assert_eq!(fixture.disk(), original);
        assert!(banner_shown(cx, window), "offered again once it is back on");
    }

    #[gpui_kit::test]
    async fn a_failed_prefs_save_is_shown(cx: &mut TestAppContext) {
        let mut fixture = Fixture::new("auto-reload-config = true\n");
        let blocker = fixture.prefs_path.with_file_name("not-a-directory");
        fs::write(&blocker, "").unwrap();
        fixture.prefs_path = blocker.join("prefs.json");
        let (window, _) = open_shell(cx, &fixture);

        in_window(cx, window, |window, cx| {
            window.click("keep-auto-reload", cx);
        });
        cx.run_until_parked();

        assert!(!banner_shown(cx, window), "kept for this session");
        cx.wait_for(window, Duration::from_secs(1), |window, _| {
            window.try_find("notification").is_some()
        })
        .await;
    }

    #[gpui_kit::test]
    fn no_banner_when_auto_reload_is_off_or_absent(cx: &mut TestAppContext) {
        for text in ["auto-reload-config = false\n", ORIGINAL] {
            let fixture = Fixture::new(text);
            let (window, _) = open_shell(cx, &fixture);
            assert!(!banner_shown(cx, window), "{text}");
        }
    }

    #[gpui_kit::test]
    fn enabling_auto_reload_in_general_remembers_keep_on(cx: &mut TestAppContext) {
        let fixture = Fixture::new("auto-reload-config = false\n");
        let (window, shell) = open_shell(cx, &fixture);
        in_window(cx, window, |window, cx| {
            shell.update(cx, |shell, cx| shell.select(Section::General, window, cx));
            window.click("auto-reload-config", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), "auto-reload-config = true\n");
        assert!(!banner_shown(cx, window));
        let path = model(cx, &shell).read_with(cx, |model, _| model.display_path().to_path_buf());
        assert!(
            fixture
                .prefs_store()
                .load()
                .unwrap()
                .is_auto_reload_dismissed(&path)
        );
        let (next_window, _) = open_shell(cx, &fixture);
        assert!(!banner_shown(cx, next_window));
    }

    #[gpui_kit::test]
    fn arrow_keys_move_between_sections(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        let sections = shell.read_with(cx, |shell, _| shell.sections_focus.clone());
        in_window(cx, window, |window, cx| {
            window.focus(&sections, cx);
            window.press("down", cx);
            window.press("down", cx);
        });
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.section()),
            Section::General
        );
        in_window(cx, window, |window, cx| window.press("up", cx));
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.section()),
            Section::Gaps
        );
    }

    #[gpui_kit::test]
    fn cmd_z_undoes_a_restore_made_from_the_snapshots_list(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        set_padding(&model(cx, &shell), 12, cx);
        cx.run_until_parked();
        let edited = fixture.disk();

        in_window(cx, window, |window, cx| {
            shell.update(cx, |shell, cx| shell.select(Section::Snapshots, window, cx));
        });
        cx.run_until_parked();
        let list = shell.read_with(cx, |shell, cx| {
            shell
                .snapshots
                .as_ref()
                .map(|snapshots| snapshots.focus_handle(cx))
        });
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.focus(list.as_ref().unwrap(), cx);
            window.press("down", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), ORIGINAL);
        // The dialog hands focus back to the list once its exit animation ends.
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();

        in_window(cx, window, |window, cx| window.press("cmd-z", cx));
        cx.run_until_parked();
        assert_eq!(fixture.disk(), edited);

        in_window(cx, window, |window, cx| {
            shell.update(cx, |shell, cx| shell.select(Section::Gaps, window, cx));
        });
        assert!(shell.read_with(cx, |shell, _| shell.snapshots.is_none()));
    }

    fn badged(cx: &mut TestAppContext, window: AnyWindowHandle, section: Section) -> bool {
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window
                .try_find(SharedString::from(format!("{section:?}-errors")))
                .is_some()
        })
    }

    fn badged_sections(cx: &mut TestAppContext, window: AnyWindowHandle) -> Vec<Section> {
        Section::ALL
            .into_iter()
            .filter(|section| badged(cx, window, *section))
            .collect()
    }

    #[gpui_kit::test]
    fn an_invalid_gap_value_badges_gaps_until_fixed(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        in_window(cx, window, |window, cx| {
            shell.update(cx, |shell, cx| shell.select(Section::Gaps, window, cx));
            window.render_frame(cx);
            window.click("gaps.inner.horizontal", cx);
            window.press("cmd-a", cx);
            window.input("-5", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(badged_sections(cx, window), vec![Section::Gaps]);

        in_window(cx, window, |window, cx| {
            window.press("cmd-a", cx);
            window.input("5", cx);
            window.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(badged_sections(cx, window), Vec::<Section>::new());
        assert_eq!(
            fixture.aerospace.reload_count(),
            1,
            "only the fix is written"
        );
    }

    #[gpui_kit::test]
    fn errors_in_the_file_badge_the_section_that_edits_the_key(cx: &mut TestAppContext) {
        let fixture = Fixture::new("[gaps]\ninner.horizontal = 'wide'\n");
        let (window, _) = open_shell(cx, &fixture);
        assert_eq!(badged_sections(cx, window), vec![Section::Gaps]);
    }

    #[gpui_kit::test]
    fn cmd_number_shows_the_section_in_sidebar_order(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        for (section, keys) in Section::ALL
            .into_iter()
            .zip(["cmd-1", "cmd-2", "cmd-3", "cmd-4", "cmd-5", "cmd-6"])
            .rev()
        {
            in_window(cx, window, |window, cx| window.press(keys, cx));
            assert_eq!(shell.read_with(cx, |shell, _| shell.section()), section);
        }
    }

    #[gpui_kit::test]
    fn menu_undo_acts_on_a_focused_text_field_like_cmd_z(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        set_padding(&model(cx, &shell), 12, cx);
        cx.run_until_parked();
        let edited = fixture.disk();

        in_window(cx, window, |window, cx| {
            shell.update(cx, |shell, cx| shell.select(Section::General, window, cx));
            window.render_frame(cx);
            window.click("accordion-padding", cx);
            window.input("9", cx);
            window.dispatch_action(Box::new(Undo), cx);
        });
        cx.run_until_parked();
        in_window(cx, window, |window, cx| window.press("enter", cx));
        cx.run_until_parked();
        assert_eq!(
            fixture.disk(),
            edited,
            "the typing was undone, not the file"
        );
        assert_eq!(fixture.aerospace.reload_count(), 1);

        let shell_focus = shell.read_with(cx, |shell, _| shell.focus_handle.clone());
        in_window(cx, window, |window, cx| {
            window.focus(&shell_focus, cx);
            window.dispatch_action(Box::new(Undo), cx);
        });
        cx.run_until_parked();
        assert_eq!(fixture.disk(), ORIGINAL);
    }

    #[gpui_kit::test]
    fn cmd_z_undoes_and_cmd_shift_z_redoes(cx: &mut TestAppContext) {
        let fixture = Fixture::new(ORIGINAL);
        let (window, shell) = open_shell(cx, &fixture);
        set_padding(&model(cx, &shell), 12, cx);
        cx.run_until_parked();
        let edited = fixture.disk();

        in_window(cx, window, |window, cx| window.press("cmd-z", cx));
        cx.run_until_parked();
        assert_eq!(fixture.disk(), ORIGINAL);

        in_window(cx, window, |window, cx| window.press("cmd-shift-z", cx));
        cx.run_until_parked();
        assert_eq!(fixture.disk(), edited);
    }
}
