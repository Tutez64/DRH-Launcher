// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod game_install;
mod github_releases;
mod install_state;
mod paths;
mod platform;
mod release_manifest;
mod release_source;

use std::cell::RefCell;
use std::rc::Rc;
use std::thread;

use config::LauncherConfig;
use game_install::inspect_install;
use github_releases::discover_latest_platform_release;
use platform::Platform;
use release_source::ReleaseSource;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let config = Rc::new(RefCell::new(LauncherConfig::load()));

    refresh_home_state(&ui, &config.borrow(), "Ready.");

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_install_or_play(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let mut config = config.borrow_mut();
            if config.install_dir.is_none() {
                config.install_dir = Some(paths::default_install_dir());
                match config.save() {
                    Ok(()) => refresh_home_state(
                        &ui,
                        &config,
                        "Install directory selected. Download support will be added next.",
                    ),
                    Err(error) => refresh_home_state(
                        &ui,
                        &config,
                        &format!("Could not save configuration: {error}"),
                    ),
                }
                return;
            }

            refresh_home_state(
                &ui,
                &config,
                "Launch support will be added after installation.",
            );
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_check_updates(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if !ui.get_update_check_enabled() {
                return;
            }

            refresh_home_state(&ui, &config.borrow(), "Checking GitHub releases...");
            ui.set_update_check_enabled(false);
            ui.set_update_check_text("Checking...".into());

            let ui = ui.as_weak();
            let config = config.borrow().clone();
            thread::spawn(move || {
                let source = ReleaseSource::drh();
                let platform = Platform::current();
                let message = match discover_latest_platform_release(&source, platform) {
                    Ok(release) => release.summary(),
                    Err(error) => error,
                };

                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };

                    refresh_home_state(&ui, &config, &message);
                    ui.set_update_check_enabled(true);
                    ui.set_update_check_text("Check for updates".into());
                });
            });
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_settings(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let config_path = paths::config_file();
            refresh_home_state(
                &ui,
                &config.borrow(),
                &format!(
                    "Settings UI is not available yet. Config: {}",
                    config_path.display()
                ),
            );
        });
    }

    ui.run()
}

fn refresh_home_state(ui: &AppWindow, config: &LauncherConfig, message: &str) {
    let status = inspect_install(config.install_dir.as_deref());

    ui.set_install_status(status.status_text().into());
    ui.set_install_action_text(status.state.primary_action().into());
    ui.set_version_status(status.version_text().into());

    let activity_message = status
        .reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
        .unwrap_or(message);
    ui.set_activity_message(activity_message.into());
}
