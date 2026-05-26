// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod archive;
mod config;
mod download;
mod game_install;
mod game_launch;
mod github_releases;
mod install_metadata;
mod install_state;
mod installer;
mod paths;
mod platform;
mod release_manifest;
mod release_source;

use std::cell::RefCell;
use std::process::Child;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use archive::extract_to_staging;
use config::LauncherConfig;
use download::{DownloadProgress, download_and_verify_with_progress};
use game_install::inspect_install;
use game_launch::launch_game;
use github_releases::{PlatformRelease, discover_latest_platform_release};
use install_state::InstallState;
use installer::install_extracted_archive;
use platform::Platform;
use release_source::ReleaseSource;
use slint::{Timer, TimerMode};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let config = Rc::new(RefCell::new(LauncherConfig::load()));
    let latest_release = Arc::new(Mutex::new(None::<PlatformRelease>));
    let game_process = Rc::new(RefCell::new(None::<Child>));
    let game_monitor = Rc::new(Timer::default());
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
        let game_process = Rc::clone(&game_process);
        let game_monitor = Rc::clone(&game_monitor);
        let release_source = release_source.clone();
        ui.unwrap().on_install_or_play(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if !ui.get_install_action_enabled() {
                return;
            }

            let mut config = config.borrow_mut();
            let process_error = {
                let mut process = game_process.borrow_mut();
                match process.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(None) => Some(Ok(())),
                        Ok(Some(_)) => {
                            process.take();
                            game_monitor.stop();
                            None
                        }
                        Err(error) => Some(Err(error.to_string())),
                    },
                    None => None,
                }
            };
            if let Some(process_error) = process_error {
                match process_error {
                    Ok(()) => {
                        let message = stop_game(&game_process);
                        game_monitor.stop();
                        refresh_home_state(&ui, &config, &message);
                    }
                    Err(error) => refresh_playing_state(
                        &ui,
                        &config,
                        &format!("Could not inspect DRH process: {error}"),
                    ),
                }
                return;
            }

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

            let install_status = inspect_install(Some(&install_dir));
            if matches!(
                install_status.state,
                InstallState::Installed | InstallState::LaunchableButMaybeOutdated
            ) {
                let config = config.clone();
                match launch_game(&config) {
                    Ok(child) => {
                        game_process.borrow_mut().replace(child);
                        refresh_playing_state(&ui, &config, "DRH is running.");
                        start_game_monitor(
                            Rc::clone(&game_monitor),
                            ui.as_weak(),
                            config.clone(),
                            Rc::clone(&game_process),
                        );
                    }
                    Err(error) => refresh_home_state(&ui, &config, &error),
                }
                return;
            }

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
                        report_background_activity(
                            &ui,
                            format!("Found {}. Preparing download...", release.version),
                            Some("Downloading..."),
                        );
                        let mut last_percent = None::<u64>;
                        match download_and_verify_with_progress(
                            &release.asset,
                            &install_dir,
                            |progress| {
                                let percent = progress_percent(&progress);
                                if percent != last_percent
                                    || progress.downloaded == 0
                                    || progress.downloaded == progress.total
                                {
                                    last_percent = percent;
                                    report_background_activity(
                                        &ui,
                                        format_download_progress(&release.asset.name, &progress),
                                        Some("Downloading..."),
                                    );
                                }
                            },
                        ) {
                            Ok(download) => {
                                report_background_activity(
                                    &ui,
                                    "Download verified. Extracting archive...".to_string(),
                                    Some("Extracting..."),
                                );
                                match extract_to_staging(&download, &install_dir) {
                                    Ok(extracted) => {
                                        report_background_activity(
                                            &ui,
                                            "Archive extracted. Installing files...".to_string(),
                                            Some("Installing..."),
                                        );
                                        match install_extracted_archive(
                                            &extracted,
                                            &install_dir,
                                            &release,
                                            &release_source,
                                        ) {
                                            Ok(installed) => format!(
                                                "Installed {}. Previous version: {}",
                                                installed.active.version,
                                                installed
                                                    .previous
                                                    .as_ref()
                                                    .map(|previous| previous.version.as_str())
                                                    .unwrap_or("none")
                                            ),
                                            Err(error) => error,
                                        }
                                    }
                                    Err(error) => error,
                                }
                            }
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

fn report_background_activity(
    ui: &slint::Weak<AppWindow>,
    message: String,
    action_text: Option<&str>,
) {
    let ui = ui.clone();
    let action_text = action_text.map(str::to_string);
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = ui.upgrade() else {
            return;
        };

        ui.set_activity_message(message.into());
        if let Some(action_text) = action_text {
            ui.set_install_action_text(action_text.into());
        }
    });
}

fn format_download_progress(asset_name: &str, progress: &DownloadProgress) -> String {
    let downloaded = format_bytes(progress.downloaded);
    let total = format_bytes(progress.total);

    match progress_percent(progress) {
        Some(percent) => format!("Downloading {asset_name}: {percent}% ({downloaded} / {total})"),
        None => format!("Downloading {asset_name}: {downloaded}"),
    }
}

fn progress_percent(progress: &DownloadProgress) -> Option<u64> {
    if progress.total == 0 {
        return None;
    }

    Some(progress.downloaded.saturating_mul(100) / progress.total)
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn refresh_playing_state(ui: &AppWindow, config: &LauncherConfig, message: &str) {
    let status = inspect_install(config.install_dir.as_deref());

    ui.set_install_status("DRH is running".into());
    ui.set_install_action_text(InstallState::Playing.primary_action().into());
    ui.set_install_action_enabled(true);
    ui.set_version_status(status.version_text().into());
    ui.set_activity_message(message.into());
}

fn start_game_monitor(
    timer: Rc<Timer>,
    ui: slint::Weak<AppWindow>,
    config: LauncherConfig,
    game_process: Rc<RefCell<Option<Child>>>,
) {
    let timer_handle = Rc::clone(&timer);
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        let finished = {
            let mut process = game_process.borrow_mut();
            let Some(child) = process.as_mut() else {
                timer_handle.stop();
                return;
            };

            match child.try_wait() {
                Ok(Some(status)) => {
                    process.take();
                    Some(format!("DRH exited with status: {status}"))
                }
                Ok(None) => None,
                Err(error) => {
                    process.take();
                    Some(format!("Could not inspect DRH process: {error}"))
                }
            }
        };

        let Some(message) = finished else {
            return;
        };

        timer_handle.stop();
        if let Some(ui) = ui.upgrade() {
            refresh_home_state(&ui, &config, &message);
        }
    });
}

fn stop_game(game_process: &Rc<RefCell<Option<Child>>>) -> String {
    let Some(mut child) = game_process.borrow_mut().take() else {
        return "DRH is not running.".to_string();
    };

    match child.kill() {
        Ok(()) => {
            let _ = child.wait();
            "DRH stopped.".to_string()
        }
        Err(error) => format!("Could not stop DRH: {error}"),
    }
}
