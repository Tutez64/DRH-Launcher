// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod download;
mod game_install;
mod github_releases;
mod install_state;
mod paths;
mod platform;
mod release_manifest;
mod release_source;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use config::LauncherConfig;
use download::download_and_verify;
use game_install::inspect_install;
use github_releases::{PlatformRelease, discover_latest_platform_release};
use platform::Platform;
use release_source::ReleaseSource;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let config = Rc::new(RefCell::new(LauncherConfig::load()));
    let latest_release = Arc::new(Mutex::new(None::<PlatformRelease>));
    let release_source = ReleaseSource::from_environment();

    refresh_home_state(
        &ui,
        &config.borrow(),
        &format!("Ready. Release source: {}", release_source.label()),
    );

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let latest_release = Arc::clone(&latest_release);
        let release_source = release_source.clone();
        ui.unwrap().on_install_or_play(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if !ui.get_install_action_enabled() {
                return;
            }

            let mut config = config.borrow_mut();
            if config.install_dir.is_none() {
                config.install_dir = Some(paths::default_install_dir());
                if let Err(error) = config.save() {
                    refresh_home_state(
                        &ui,
                        &config,
                        &format!("Could not save configuration: {error}"),
                    );
                    return;
                }
            }

            let Some(install_dir) = config.install_dir.clone() else {
                refresh_home_state(&ui, &config, "No install directory selected.");
                return;
            };

            let release = latest_release
                .lock()
                .expect("latest release lock poisoned")
                .clone();

            let initial_message = if let Some(release) = &release {
                format!("Downloading {}...", release.asset.name)
            } else {
                "Checking GitHub releases...".to_string()
            };
            refresh_home_state(&ui, &config, &initial_message);
            ui.set_install_action_enabled(false);
            ui.set_update_check_enabled(false);
            ui.set_install_action_text(if release.is_some() {
                "Downloading...".into()
            } else {
                "Checking...".into()
            });

            let ui = ui.as_weak();
            let config = config.clone();
            let latest_release = Arc::clone(&latest_release);
            let release_source = release_source.clone();
            thread::spawn(move || {
                let release = match release {
                    Some(release) => Ok(release),
                    None => discover_latest_platform_release(&release_source, Platform::current()),
                };

                let message = match release {
                    Ok(release) => {
                        latest_release
                            .lock()
                            .expect("latest release lock poisoned")
                            .replace(release.clone());
                        match download_and_verify(&release.asset, &install_dir) {
                            Ok(download) => format!(
                                "Archive verified: {} ({} bytes, sha256:{})",
                                download.path.display(),
                                download.size,
                                download.sha256
                            ),
                            Err(error) => error,
                        }
                    }
                    Err(error) => error,
                };

                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };

                    refresh_home_state(&ui, &config, &message);
                    ui.set_install_action_enabled(true);
                    ui.set_update_check_enabled(true);
                    ui.set_update_check_text("Check for updates".into());
                });
            });
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let latest_release = Arc::clone(&latest_release);
        let release_source = release_source.clone();
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
            let latest_release = Arc::clone(&latest_release);
            let release_source = release_source.clone();
            thread::spawn(move || {
                let platform = Platform::current();
                let result = discover_latest_platform_release(&release_source, platform);
                let release = result.as_ref().ok().cloned();
                let message = match result {
                    Ok(release) => release.summary(),
                    Err(error) => error,
                };

                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };

                    if let Some(release) = release {
                        latest_release
                            .lock()
                            .expect("latest release lock poisoned")
                            .replace(release);
                    }
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
    ui.set_install_action_enabled(true);
    ui.set_version_status(status.version_text().into());

    let activity_message = status
        .reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
        .filter(|_| message == "Ready.")
        .unwrap_or(message);
    ui.set_activity_message(activity_message.into());
}
