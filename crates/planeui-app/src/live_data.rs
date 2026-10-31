//! Live picker data, refreshed off the UI thread and throttled across window activations.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, Task};
use planeui_core::aerospace::{Aerospace, App, CliError, Monitor};
use planeui_core::commands::WorkspaceName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveState {
    Loading,
    Ready {
        apps: Vec<App>,
        monitors: Vec<Monitor>,
        workspaces: Vec<WorkspaceName>,
    },
    Unavailable,
}

#[derive(Debug)]
pub struct LiveData {
    aerospace: Arc<dyn Aerospace>,
    state: LiveState,
    last_refresh: Option<Instant>,
    task: Option<Task<()>>,
}

impl LiveData {
    pub fn new(aerospace: Arc<dyn Aerospace>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            aerospace,
            state: LiveState::Loading,
            last_refresh: None,
            task: None,
        };
        this.refresh(cx);
        this
    }

    #[must_use]
    pub fn state(&self) -> &LiveState {
        &self.state
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        if self.task.is_some()
            || self
                .last_refresh
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(3))
        {
            return;
        }
        self.last_refresh = Some(now);
        self.state = LiveState::Loading;
        let aerospace = self.aerospace.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    Ok::<_, CliError>(LiveState::Ready {
                        apps: aerospace.list_apps()?,
                        monitors: aerospace.list_monitors()?,
                        workspaces: aerospace.list_workspaces()?,
                    })
                })
                .await;
            let state = result.unwrap_or_else(|error| {
                log::warn!("could not refresh AeroSpace picker data: {error}");
                LiveState::Unavailable
            });
            if let Err(error) = this.update(cx, |this, cx| {
                this.state = state;
                this.task = None;
                cx.notify();
            }) {
                log::debug!("live data released before the refresh finished: {error}");
            }
        }));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;
    use planeui_core::aerospace::FakeAerospace;

    use super::*;

    #[cfg(unix)]
    #[gpui_kit::test]
    fn running_fake_publishes_apps_monitors_and_workspaces(cx: &mut TestAppContext) {
        use planeui_core::aerospace::CliAerospace;
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("fixture-cli");
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../planeui-core/tests/fixtures/cli");
        std::fs::write(&binary, format!("#!/bin/sh\ncase \"$1\" in\nlist-apps|list-monitors|list-workspaces) cat '{}/'\"$1\"'/stdout' ;;\nesac\n", fixtures.display())).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cli = CliAerospace::with_binary(binary).with_timeout(Duration::from_secs(30));
        let apps = cli.list_apps().unwrap();
        let monitors = cli.list_monitors().unwrap();
        let workspaces = cli.list_workspaces().unwrap();
        let fake = Arc::new(
            FakeAerospace::new()
                .with_apps(apps.clone())
                .with_monitors(monitors.clone())
                .with_workspaces(workspaces.clone()),
        );
        let data = cx.new(|cx| LiveData::new(fake.clone(), cx));
        assert_eq!(
            data.read_with(cx, |data, _| data.state().clone()),
            LiveState::Loading
        );
        cx.run_until_parked();
        assert_eq!(
            data.read_with(cx, |data, _| data.state().clone()),
            LiveState::Ready {
                apps,
                monitors,
                workspaces
            }
        );
        assert_eq!(fake.list_count(), 3);
    }

    #[gpui_kit::test]
    fn not_running_or_missing_fake_yields_unavailable(cx: &mut TestAppContext) {
        for fake in [
            FakeAerospace::new().with_running(false),
            FakeAerospace::missing(),
        ] {
            let data = cx.new(|cx| LiveData::new(Arc::new(fake), cx));
            cx.run_until_parked();
            assert_eq!(
                data.read_with(cx, |data, _| data.state().clone()),
                LiveState::Unavailable
            );
        }
    }

    #[gpui_kit::test]
    fn refresh_is_throttled_for_three_seconds(cx: &mut TestAppContext) {
        let fake = Arc::new(FakeAerospace::new());
        let data = cx.new(|cx| LiveData::new(fake.clone(), cx));
        data.update(cx, LiveData::refresh);
        cx.run_until_parked();
        assert_eq!(fake.list_count(), 3);
        cx.executor().advance_clock(Duration::from_millis(2999));
        data.update(cx, LiveData::refresh);
        cx.run_until_parked();
        assert_eq!(fake.list_count(), 3);
        cx.executor().advance_clock(Duration::from_millis(1));
        data.update(cx, LiveData::refresh);
        cx.run_until_parked();
        assert_eq!(fake.list_count(), 6);
    }
}
