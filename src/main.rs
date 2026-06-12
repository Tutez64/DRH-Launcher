// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod archive;
mod config;
mod diagnostics;
mod download;
mod game_install;
mod game_launch;
mod game_logs;
mod github_releases;
mod install_metadata;
mod install_state;
mod installer;
mod paths;
mod platform;
mod release_manifest;
mod release_source;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use archive::extract_to_staging;
use config::{LaunchArgumentsMode, LauncherConfig};
use download::{
    DownloadProgress, download_and_verify_with_progress, prune_download_cache,
    update_download_cache, verify_cached_archive_by_metadata,
};
use game_install::inspect_install;
use github_releases::{
    PlatformRelease, ReleaseMetadataSource, discover_latest_platform_release,
    discover_latest_platform_release_for_install,
};
use install_state::InstallState;
use installer::{
    install_extracted_archive, repair_active_install_from_extracted_archive,
    repair_release_from_extracted_archive, restore_previous_version,
};
use platform::Platform;
use release_manifest::ManifestLaunchOptions;
use release_source::ReleaseSource;
use slint::{Brush, Color, Model, ModelRc, Timer, TimerMode, VecModel};

slint::include_modules!();

const DISCORD_URL: &str = "https://discord.gg/SSwUQ8fmcb";
const DRH_GITHUB_URL: &str = concat!(
    "https://github.com/Tutez64/Dungeon-Rampage-Haxe",
    "?utm_source=drh_launcher&utm_medium=about_page",
    "&utm_campaign=project_links&utm_content=drh_github",
);
const DRHL_GITHUB_URL: &str = concat!(
    "https://github.com/Tutez64/DRH-Launcher",
    "?utm_source=drh_launcher&utm_medium=about_page",
    "&utm_campaign=project_links&utm_content=drhl_github",
);
const AX4_GITHUB_URL: &str = concat!(
    "https://github.com/Tutez64/ax4",
    "?utm_source=drh_launcher&utm_medium=about_page",
    "&utm_campaign=project_links&utm_content=ax4_github",
);
const XDG_APP_ID: &str = "io.github.Tutez64.DRHLauncher";

struct HomeViewState {
    install_status: String,
    install_action_text: String,
    install_action_enabled: bool,
    version_status: String,
    home_support_text: String,
    update_check_text: String,
    update_check_enabled: bool,
    open_install_folder_enabled: bool,
    open_logs_folder_enabled: bool,
    restore_previous_visible: bool,
    restore_previous_enabled: bool,
    restore_previous_text: String,
    reinstall_current_enabled: bool,
    reinstall_current_text: String,
    status_detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LogViewportPosition {
    y: f32,
    at_end: bool,
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    slint::set_xdg_app_id(XDG_APP_ID)?;

    let config = Rc::new(RefCell::new(LauncherConfig::load()));
    let latest_release = Arc::new(Mutex::new(None::<PlatformRelease>));
    let game_process = Rc::new(RefCell::new(None::<game_launch::RunningGame>));
    let game_monitor = Rc::new(Timer::default());
    let game_log_positions = Rc::new(RefCell::new(HashMap::<String, LogViewportPosition>::new()));
    let release_source = ReleaseSource::from_environment();

    ui.set_launcher_version(env!("CARGO_PKG_VERSION").into());
    ui.set_config_path(paths::config_file().display().to_string().into());
    refresh_settings_view(&ui, &config.borrow(), "Save");
    let installed_launch_options = load_installed_launch_options(&config.borrow());
    refresh_launch_options_view(
        &ui,
        &config.borrow(),
        installed_launch_options.as_ref(),
        "Save",
    );

    refresh_home_state(
        &ui,
        &config.borrow(),
        &format!("Ready. Release source: {}", release_source.label()),
    );
    log_for_config(
        &config.borrow(),
        diagnostics::LogLevel::Info,
        &format!(
            "DRH Launcher started. Release source: {}",
            release_source.label()
        ),
    );
    refresh_logs_view(&ui, &config.borrow());

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
                    Some(game) => match game.child.try_wait() {
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
                        log_for_config(&config, diagnostics::LogLevel::Info, &message);
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
                    log_for_config(&config, diagnostics::LogLevel::Error, &error.to_string());
                    refresh_home_state(
                        &ui,
                        &config,
                        &format!("Could not save configuration: {error}"),
                    );
                    return;
                }
                refresh_settings_view(&ui, &config, "Save");
            }

            let Some(install_dir) = config.install_dir.clone() else {
                log_for_config(
                    &config,
                    diagnostics::LogLevel::Warn,
                    "No install directory selected.",
                );
                refresh_home_state(&ui, &config, "No install directory selected.");
                return;
            };

            let release = latest_release
                .lock()
                .expect("latest release lock poisoned")
                .clone();
            let install_status = inspect_install(Some(&install_dir));
            let update_available = release
                .as_ref()
                .is_some_and(|release| release_update_available(&config, &install_status, release));

            if matches!(
                install_status.state,
                InstallState::Installed | InstallState::LaunchableButMaybeOutdated
            ) && !update_available
            {
                let config = config.clone();
                let installed_launch_options = load_installed_launch_options(&config);
                let recommended_game_args =
                    recommended_game_args_from_launch_options(installed_launch_options.as_ref());
                match game_launch::launch_game_with_recommended_args(
                    &config,
                    &recommended_game_args,
                    install_status.installed_version.as_deref(),
                ) {
                    Ok(game) => {
                        log_for_config(
                            &config,
                            diagnostics::LogLevel::Info,
                            &format!(
                                "DRH launched with command: {}. Session log: {}",
                                game_launch::launch_command_summary_with_recommended_args(
                                    &config,
                                    &recommended_game_args
                                )
                                .unwrap_or_else(|error| format!(
                                    "command summary unavailable ({error})"
                                )),
                                game.session_log.path.display(),
                            ),
                        );
                        game_process.borrow_mut().replace(game);
                        refresh_playing_state(&ui, &config, "DRH is running.");
                        start_game_monitor(
                            Rc::clone(&game_monitor),
                            ui.as_weak(),
                            config.clone(),
                            Rc::clone(&game_process),
                        );
                    }
                    Err(error) => {
                        log_for_config(&config, diagnostics::LogLevel::Error, &error);
                        refresh_home_state(&ui, &config, &error);
                    }
                }
                return;
            }

            let initial_message = if matches!(install_status.state, InstallState::BrokenInstall) {
                "Repairing DRH installation...".to_string()
            } else if let Some(release) = &release {
                format!("Downloading {}...", release.asset.name)
            } else {
                "Checking GitHub releases...".to_string()
            };
            refresh_home_state(&ui, &config, &initial_message);
            let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Info, &initial_message);
            ui.set_install_action_enabled(false);
            ui.set_update_check_enabled(false);
            ui.set_restore_previous_enabled(false);
            ui.set_reinstall_current_enabled(false);
            ui.set_install_action_text(InstallState::Updating.primary_action().into());

            let ui = ui.as_weak();
            let config = config.clone();
            let latest_release = Arc::clone(&latest_release);
            let release_source = release_source.clone();
            let repair_from_cache = matches!(install_status.state, InstallState::BrokenInstall);
            thread::spawn(move || {
                let message = if repair_from_cache {
                    match repair_from_cached_active_archive(
                        &ui,
                        &install_dir,
                        config.download_cache_limit,
                    ) {
                        Ok(message) => message,
                        Err(error) => {
                            let message = format!(
                                "Cached repair unavailable: {error}. Checking GitHub releases."
                            );
                            let _ = diagnostics::write(
                                &install_dir,
                                diagnostics::LogLevel::Warn,
                                &message,
                            );
                            report_background_activity(&ui, message);
                            install_latest_release(
                                &ui,
                                &install_dir,
                                &config,
                                &release_source,
                                Arc::clone(&latest_release),
                                release,
                                true,
                            )
                        }
                    }
                } else {
                    install_latest_release(
                        &ui,
                        &install_dir,
                        &config,
                        &release_source,
                        Arc::clone(&latest_release),
                        release,
                        false,
                    )
                };

                let level = if is_error_message(&message) {
                    diagnostics::LogLevel::Error
                } else {
                    diagnostics::LogLevel::Info
                };
                let _ = diagnostics::write(&install_dir, level, &message);

                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };

                    let installed_launch_options = load_installed_launch_options(&config);
                    refresh_launch_options_view(
                        &ui,
                        &config,
                        installed_launch_options.as_ref(),
                        "Save",
                    );
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

            start_release_check(
                &ui,
                config.borrow().clone(),
                Arc::clone(&latest_release),
                release_source.clone(),
            );
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let game_process = Rc::clone(&game_process);
        ui.unwrap().on_restore_previous_version(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if process_is_running(&game_process) {
                let message = "Stop DRH before restoring the previous version.";
                log_for_config(&config.borrow(), diagnostics::LogLevel::Warn, message);
                refresh_home_state(&ui, &config.borrow(), message);
                return;
            }

            let Some(install_dir) = config.borrow().install_dir.clone() else {
                let message = "No install directory selected.";
                log_for_config(&config.borrow(), diagnostics::LogLevel::Warn, message);
                refresh_home_state(&ui, &config.borrow(), message);
                return;
            };

            match restore_previous_version(&install_dir) {
                Ok(restored) => {
                    let message = format!(
                        "Restored previous version {}. Rollback target is now {}.",
                        restored.active.version,
                        restored
                            .previous
                            .as_ref()
                            .map(|previous| previous.version.as_str())
                            .unwrap_or("none")
                    );
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Info, &message);
                    let installed_launch_options = load_installed_launch_options(&config.borrow());
                    refresh_launch_options_view(
                        &ui,
                        &config.borrow(),
                        installed_launch_options.as_ref(),
                        "Save",
                    );
                    refresh_home_state(&ui, &config.borrow(), &message);
                }
                Err(error) => {
                    let message = format!("Could not restore previous version: {error}");
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &message);
                    refresh_home_state(&ui, &config.borrow(), &message);
                }
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let game_process = Rc::clone(&game_process);
        ui.unwrap().on_reinstall_current_version(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if process_is_running(&game_process) {
                let message = "Stop DRH before reinstalling the current version.";
                log_for_config(&config.borrow(), diagnostics::LogLevel::Warn, message);
                refresh_home_state(&ui, &config.borrow(), message);
                return;
            }

            let Some(install_dir) = config.borrow().install_dir.clone() else {
                let message = "No install directory selected.";
                log_for_config(&config.borrow(), diagnostics::LogLevel::Warn, message);
                refresh_home_state(&ui, &config.borrow(), message);
                return;
            };

            let config = config.borrow().clone();
            refresh_home_state(&ui, &config, "Reinstalling current version...");
            ui.set_install_action_enabled(false);
            ui.set_update_check_enabled(false);
            ui.set_restore_previous_enabled(false);
            ui.set_reinstall_current_enabled(false);
            ui.set_install_action_text(InstallState::Updating.primary_action().into());

            let ui = ui.as_weak();
            thread::spawn(move || {
                let message = repair_from_cached_active_archive(
                    &ui,
                    &install_dir,
                    config.download_cache_limit,
                )
                .unwrap_or_else(|error| format!("Could not reinstall current version: {error}"));
                let level = if is_error_message(&message) {
                    diagnostics::LogLevel::Error
                } else {
                    diagnostics::LogLevel::Info
                };
                let _ = diagnostics::write(&install_dir, level, &message);

                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };

                    let installed_launch_options = load_installed_launch_options(&config);
                    refresh_launch_options_view(
                        &ui,
                        &config,
                        installed_launch_options.as_ref(),
                        "Save",
                    );
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
        ui.unwrap().on_open_config_folder(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            match open_config_folder() {
                Ok(()) => set_status_message(&ui, "Config folder opened."),
                Err(error) => {
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &error);
                    set_status_message(&ui, &format!("Could not open config folder: {error}"));
                }
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_increment_download_cache_limit(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let limit = ui_download_cache_limit(&ui)
                .unwrap_or_else(|_| config.borrow().download_cache_limit);
            ui.set_download_cache_limit((limit.saturating_add(1)).to_string().into());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_decrement_download_cache_limit(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let limit = ui_download_cache_limit(&ui)
                .unwrap_or_else(|_| config.borrow().download_cache_limit);
            ui.set_download_cache_limit(limit.saturating_sub(1).to_string().into());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_save_general_settings(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let Ok(limit) = ui_download_cache_limit(&ui) else {
                ui.set_download_cache_limit_error(
                    "Use a non-negative whole number, for example 0, 1, or 3.".into(),
                );
                set_status_message(
                    &ui,
                    "Download cache limit must be a non-negative whole number.",
                );
                return;
            };

            let mut config = config.borrow_mut();
            config.download_cache_limit = limit;
            match config.save() {
                Ok(()) => {
                    log_for_config(
                        &config,
                        diagnostics::LogLevel::Info,
                        &format!("Download cache limit saved: {limit}."),
                    );
                    let prune_result = config
                        .install_dir
                        .as_deref()
                        .map(|install_dir| prune_download_cache(install_dir, limit));
                    match prune_result {
                        Some(Ok(removed)) => {
                            for archive in &removed {
                                log_for_config(
                                    &config,
                                    diagnostics::LogLevel::Info,
                                    &format!("Removed cached archive {}.", archive.display()),
                                );
                            }
                            refresh_settings_view(&ui, &config, "Saved successfully");
                            if removed.is_empty() {
                                set_status_message(&ui, "Download cache limit saved.");
                            } else {
                                set_status_message(
                                    &ui,
                                    &format!(
                                        "Download cache limit saved. Removed {} cached archive(s).",
                                        removed.len()
                                    ),
                                );
                            }
                        }
                        Some(Err(error)) => {
                            log_for_config(&config, diagnostics::LogLevel::Error, &error);
                            refresh_settings_view(&ui, &config, "Saved successfully");
                            set_status_message(
                                &ui,
                                &format!(
                                    "Download cache limit saved, but cache pruning failed: {error}"
                                ),
                            );
                        }
                        None => {
                            refresh_settings_view(&ui, &config, "Saved successfully");
                            set_status_message(&ui, "Download cache limit saved.");
                        }
                    }
                }
                Err(error) => {
                    let message = format!("Could not save download cache limit: {error}");
                    log_for_config(&config, diagnostics::LogLevel::Error, &message);
                    set_status_message(&ui, &message);
                }
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_cancel_general_settings(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            refresh_settings_view(&ui, &config.borrow(), "Save");
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_save_launch_options(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let mut config = config.borrow_mut();
            config.pre_launch_command = ui.get_pre_launch_command().trim().to_string();
            config.launch_arguments_mode =
                LaunchArgumentsMode::from_ui_index(ui.get_launch_arguments_mode());
            let installed_launch_options = load_installed_launch_options(&config);
            let game_args = match launch_options_game_args(&ui, installed_launch_options.as_ref()) {
                Ok(args) => args,
                Err(error) => {
                    log_for_config(&config, diagnostics::LogLevel::Error, &error);
                    refresh_launch_options_view(
                        &ui,
                        &config,
                        installed_launch_options.as_ref(),
                        "Invalid arguments",
                    );
                    return;
                }
            };

            config.game_args = game_args;
            match config.save() {
                Ok(()) => {
                    log_for_config(
                        &config,
                        diagnostics::LogLevel::Info,
                        "Launch options saved.",
                    );
                    refresh_launch_options_view(
                        &ui,
                        &config,
                        installed_launch_options.as_ref(),
                        "Saved successfully",
                    );
                }
                Err(error) => {
                    let message = format!("Could not save launch options: {error}");
                    log_for_config(&config, diagnostics::LogLevel::Error, &message);
                    refresh_launch_options_view(
                        &ui,
                        &config,
                        installed_launch_options.as_ref(),
                        "Save failed",
                    );
                }
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_select_launch_arguments_mode(move |mode| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let config = config.borrow();
            let installed_launch_options = load_installed_launch_options(&config);
            apply_launch_arguments_mode_to_view(
                &ui,
                &config,
                installed_launch_options.as_ref(),
                LaunchArgumentsMode::from_ui_index(mode),
                ui.get_custom_game_args().to_string(),
            );
        });
    }

    {
        let ui = ui.as_weak();
        ui.unwrap().on_launch_option_width_measured(move |width| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if width > ui.get_launch_option_min_width_px() {
                ui.set_launch_option_min_width_px(width);
            }
        });
    }

    {
        let ui = ui.as_weak();
        ui.unwrap().on_drhl_metric_width_measured(move |width| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if width > ui.get_drhl_metric_min_width_px() {
                ui.set_drhl_metric_min_width_px(width);
            }
        });
    }

    {
        let ui = ui.as_weak();
        ui.unwrap().on_link_button_width_measured(move |width| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if width > ui.get_link_button_min_width_px() {
                ui.set_link_button_min_width_px(width);
            }
        });
    }

    {
        let ui = ui.as_weak();
        ui.unwrap().on_toggle_launch_option(move |index, checked| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let mut options = launch_options_from_model(&ui);
            let Some(option) = options.get_mut(index as usize) else {
                return;
            };
            option.checked = checked;
            ui.set_launch_arguments_mode(LaunchArgumentsMode::Custom.ui_index());
            apply_launch_options_to_view(&ui, options, ui.get_custom_game_args().to_string());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_cancel_launch_options(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let config = config.borrow();
            let installed_launch_options = load_installed_launch_options(&config);
            refresh_launch_options_view(&ui, &config, installed_launch_options.as_ref(), "Save");
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_install_folder(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let Some(install_dir) = config.borrow().install_dir.clone() else {
                log_for_config(
                    &config.borrow(),
                    diagnostics::LogLevel::Warn,
                    "No install directory selected.",
                );
                refresh_home_state(&ui, &config.borrow(), "No install directory selected.");
                return;
            };

            match open_folder(&install_dir) {
                Ok(()) => {
                    log_for_config(
                        &config.borrow(),
                        diagnostics::LogLevel::Info,
                        "Install folder opened.",
                    );
                    set_status_message(&ui, "Install folder opened.");
                }
                Err(error) => {
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &error);
                    set_status_message(&ui, &format!("Could not open install folder: {error}"))
                }
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_logs_folder(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let Some(install_dir) = config.borrow().install_dir.clone() else {
                log_for_config(
                    &config.borrow(),
                    diagnostics::LogLevel::Warn,
                    "No install directory selected.",
                );
                refresh_home_state(&ui, &config.borrow(), "No install directory selected.");
                return;
            };

            match open_logs_folder(&install_dir) {
                Ok(()) => {
                    refresh_logs_view(&ui, &config.borrow());
                    set_status_message(&ui, "Logs folder opened.");
                }
                Err(error) => {
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &error);
                    set_status_message(&ui, &format!("Could not open logs folder: {error}"))
                }
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_discord_link(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            open_external_link_from_ui(&ui, &config.borrow(), "Discord", DISCORD_URL);
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_drh_github_link(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            open_external_link_from_ui(&ui, &config.borrow(), "DRH GitHub", DRH_GITHUB_URL);
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_drhl_github_link(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            open_external_link_from_ui(&ui, &config.borrow(), "DRHL GitHub", DRHL_GITHUB_URL);
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_ax4_github_link(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            open_external_link_from_ui(&ui, &config.borrow(), "ax4 GitHub", AX4_GITHUB_URL);
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_refresh_logs(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            refresh_logs_view(&ui, &config.borrow());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap()
            .on_log_wrap_columns_measured(move |source, columns| {
                let Some(ui) = ui.upgrade() else {
                    return;
                };

                let columns = columns.max(20);
                match source {
                    0 if ui.get_launcher_log_wrap_columns() != columns => {
                        ui.set_launcher_log_wrap_columns(columns);
                    }
                    1 if ui.get_game_log_wrap_columns() != columns => {
                        ui.set_game_log_wrap_columns(columns);
                    }
                    _ => return,
                }

                if ui.get_log_source() == source {
                    refresh_logs_view(&ui, &config.borrow());
                }
            });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_select_log_source(move |source| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            ui.set_log_source(source);
            refresh_logs_view(&ui, &config.borrow());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let game_log_positions = Rc::clone(&game_log_positions);
        ui.unwrap().on_select_game_log(move |index| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let Some(install_dir) = config.borrow().install_dir.clone() else {
                return;
            };
            let Ok(sessions) = game_logs::list(&install_dir) else {
                ui.set_selected_game_log_index(index);
                ui.set_game_log_viewport_y(0.0);
                ui.set_game_log_position_known(false);
                ui.set_game_log_at_end(false);
                refresh_logs_view(&ui, &config.borrow());
                return;
            };
            game_log_positions.borrow_mut().retain(|session_id, _| {
                sessions
                    .iter()
                    .any(|session| game_log_session_id(&session.path) == *session_id)
            });

            let current_session_id = ui.get_selected_game_log_id().to_string();
            if ui.get_game_log_position_known() && !current_session_id.is_empty() {
                remember_game_log_position(
                    &mut game_log_positions.borrow_mut(),
                    current_session_id,
                    ui.get_game_log_viewport_y(),
                    ui.get_game_log_at_end(),
                );
            }

            let restored_position = usize::try_from(index)
                .ok()
                .and_then(|target_index| sessions.get(target_index))
                .map(|session| game_log_session_id(&session.path))
                .and_then(|session_id| {
                    saved_game_log_position(&game_log_positions.borrow(), &session_id)
                });
            if let Some(position) = restored_position {
                ui.set_game_log_viewport_y(position.y);
                ui.set_game_log_at_end(position.at_end);
                ui.set_game_log_position_known(true);
            } else {
                ui.set_game_log_viewport_y(0.0);
                ui.set_game_log_at_end(false);
                ui.set_game_log_position_known(false);
            }

            ui.set_selected_game_log_index(index);
            refresh_logs_view(&ui, &config.borrow());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_selected_game_log(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let Some(install_dir) = config.borrow().install_dir.clone() else {
                set_status_message(&ui, "No install directory selected.");
                return;
            };
            let sessions = match game_logs::list(&install_dir) {
                Ok(sessions) => sessions,
                Err(error) => {
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &error);
                    set_status_message(&ui, &error);
                    return;
                }
            };
            let index = ui.get_selected_game_log_index();
            let Some(session) = usize::try_from(index)
                .ok()
                .and_then(|index| sessions.get(index))
            else {
                set_status_message(&ui, "No game session selected.");
                return;
            };

            let open_path = match game_logs::path_for_external_open(&session.path) {
                Ok(path) => path,
                Err(error) => {
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &error);
                    set_status_message(&ui, &error);
                    return;
                }
            };

            match open_file(&open_path) {
                Ok(()) => set_status_message(&ui, "Game session log opened."),
                Err(error) => {
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &error);
                    set_status_message(&ui, &error);
                }
            }
        });
    }

    start_release_check(
        &ui,
        config.borrow().clone(),
        Arc::clone(&latest_release),
        release_source.clone(),
    );

    ui.run()
}

fn install_latest_release(
    ui: &slint::Weak<AppWindow>,
    install_dir: &Path,
    config: &LauncherConfig,
    release_source: &ReleaseSource,
    latest_release: Arc<Mutex<Option<PlatformRelease>>>,
    release: Option<PlatformRelease>,
    repair_current: bool,
) -> String {
    let release = match release {
        Some(release) if release.metadata_source == ReleaseMetadataSource::Manifest => Ok(release),
        _ => discover_latest_platform_release_for_install(release_source, Platform::current()),
    };

    match release {
        Ok(release) => {
            if repair_current
                && rollback_blocked_update_version(config).as_deref()
                    == Some(release.version.as_str())
            {
                return format!(
                    "Could not repair from cached archive, and update {} is skipped until a newer release is available.",
                    release.version
                );
            }

            latest_release
                .lock()
                .expect("latest release lock poisoned")
                .replace(release.clone());
            let _ = diagnostics::write(
                install_dir,
                diagnostics::LogLevel::Info,
                &format!("Found release {}. Starting install.", release.version),
            );
            report_background_activity(
                ui,
                format!("Preparing download for {}...", release.version),
            );
            let mut last_percent = None::<u64>;
            let mut last_logged_percent = None::<u64>;
            let mut reported_verification = false;
            match download_and_verify_with_progress(
                &release.asset,
                install_dir,
                |progress| {
                    let percent = progress_percent(&progress);
                    if percent != last_percent
                        || progress.downloaded == 0
                        || progress.downloaded == progress.total
                    {
                        last_percent = percent;
                        report_background_activity(
                            ui,
                            format_download_progress(&release.asset.name, &progress),
                        );
                    }
                    if should_log_download_progress(percent, last_logged_percent) {
                        last_logged_percent = percent;
                        let _ = diagnostics::write(
                            install_dir,
                            diagnostics::LogLevel::Info,
                            &format_download_progress(&release.asset.name, &progress),
                        );
                    }
                    if !reported_verification
                        && progress.total > 0
                        && progress.downloaded == progress.total
                    {
                        reported_verification = true;
                        let message = "Verifying download...".to_string();
                        let _ =
                            diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
                        report_background_activity(ui, message);
                    }
                },
                |cache_error| {
                    let _ =
                        diagnostics::write(install_dir, diagnostics::LogLevel::Warn, &cache_error);
                },
            ) {
                Ok(download) => {
                    let verified_message = if download.reused {
                        format!(
                            "Reused cached archive: {} ({} bytes, sha256:{})",
                            download.path.display(),
                            download.size,
                            download.sha256
                        )
                    } else {
                        format!(
                            "Download verified: {} ({} bytes, sha256:{})",
                            download.path.display(),
                            download.size,
                            download.sha256
                        )
                    };
                    let _ = diagnostics::write(
                        install_dir,
                        diagnostics::LogLevel::Info,
                        &verified_message,
                    );
                    let message = "Extracting archive...".to_string();
                    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
                    report_background_activity(ui, message);
                    match extract_to_staging(&download, install_dir) {
                        Ok(extracted) => {
                            let _ = diagnostics::write(
                                install_dir,
                                diagnostics::LogLevel::Info,
                                "Archive extracted.",
                            );
                            let message = "Installing files...".to_string();
                            let _ = diagnostics::write(
                                install_dir,
                                diagnostics::LogLevel::Info,
                                &message,
                            );
                            report_background_activity(ui, message);
                            let install_result = if repair_current {
                                repair_release_from_extracted_archive(
                                    &extracted,
                                    install_dir,
                                    &release,
                                    release_source,
                                )
                            } else {
                                install_extracted_archive(
                                    &extracted,
                                    install_dir,
                                    &release,
                                    release_source,
                                )
                            };
                            match install_result {
                                Ok(installed) => {
                                    match update_download_cache(
                                        install_dir,
                                        &release.asset.name,
                                        config.download_cache_limit,
                                    ) {
                                        Ok(removed) => {
                                            for archive in removed {
                                                let _ = diagnostics::write(
                                                    install_dir,
                                                    diagnostics::LogLevel::Info,
                                                    &format!(
                                                        "Removed cached archive {}.",
                                                        archive.display()
                                                    ),
                                                );
                                            }
                                        }
                                        Err(error) => {
                                            let _ = diagnostics::write(
                                                install_dir,
                                                diagnostics::LogLevel::Warn,
                                                &error,
                                            );
                                        }
                                    }
                                    let message = format!(
                                        "Installed {}. Previous version: {}",
                                        installed.active.version,
                                        installed
                                            .previous
                                            .as_ref()
                                            .map(|previous| previous.version.as_str())
                                            .unwrap_or("none")
                                    );
                                    let _ = diagnostics::write(
                                        install_dir,
                                        diagnostics::LogLevel::Info,
                                        &message,
                                    );
                                    message
                                }
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
    }
}

fn repair_from_cached_active_archive(
    ui: &slint::Weak<AppWindow>,
    install_dir: &Path,
    download_cache_limit: usize,
) -> Result<String, String> {
    let installed = install_metadata::InstalledState::load(install_dir)?;
    let active = installed.active;
    let message = format!("Checking cached archive for {}...", active.version);
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
    report_background_activity(ui, message);

    let download = verify_cached_archive_by_metadata(
        install_dir,
        &active.archive,
        active.archive_size,
        &active.archive_sha256,
    )?;
    let message = format!(
        "Reused cached archive for repair: {} ({} bytes, sha256:{})",
        download.path.display(),
        download.size,
        download.sha256
    );
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);

    let message = "Extracting archive...".to_string();
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
    report_background_activity(ui, message);
    let extracted = extract_to_staging(&download, install_dir)?;
    let _ = diagnostics::write(
        install_dir,
        diagnostics::LogLevel::Info,
        "Archive extracted.",
    );

    let message = "Repairing files...".to_string();
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
    report_background_activity(ui, message);
    let repaired = repair_active_install_from_extracted_archive(&extracted, install_dir)?;
    match update_download_cache(install_dir, &active.archive, download_cache_limit) {
        Ok(removed) => {
            for archive in removed {
                let _ = diagnostics::write(
                    install_dir,
                    diagnostics::LogLevel::Info,
                    &format!("Removed cached archive {}.", archive.display()),
                );
            }
        }
        Err(error) => {
            let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Warn, &error);
        }
    }

    Ok(format!(
        "Repaired {} from cached archive.",
        repaired.active.version
    ))
}

fn start_release_check(
    ui: &AppWindow,
    config: LauncherConfig,
    latest_release: Arc<Mutex<Option<PlatformRelease>>>,
    release_source: ReleaseSource,
) {
    log_for_config(
        &config,
        diagnostics::LogLevel::Info,
        "Checking GitHub releases.",
    );
    let mut checking_state = home_view_state(&config, None, "Checking GitHub releases...");
    checking_state.update_check_enabled = false;
    checking_state.update_check_text = "Checking...".to_string();
    apply_home_view_state(ui, checking_state);

    let ui = ui.as_weak();
    thread::spawn(move || {
        let platform = Platform::current();
        let result = discover_latest_platform_release(&release_source, platform);
        let release = result.as_ref().ok().cloned();
        let message = match &result {
            Ok(release) => format!(
                "Latest known version: {} ({}) via {}",
                release.version,
                release.name,
                release.metadata_source.label()
            ),
            Err(error) => error.to_string(),
        };
        let level = if result.is_ok() {
            diagnostics::LogLevel::Info
        } else {
            diagnostics::LogLevel::Error
        };
        log_for_config(&config, level, &message);

        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let state = if let Some(release) = release {
                latest_release
                    .lock()
                    .expect("latest release lock poisoned")
                    .replace(release.clone());
                let installed_launch_options = load_installed_launch_options(&config);
                refresh_launch_options_view(
                    &ui,
                    &config,
                    installed_launch_options.as_ref(),
                    "Save",
                );
                home_view_state(&config, Some(&release), &message)
            } else {
                let installed_launch_options = load_installed_launch_options(&config);
                refresh_launch_options_view(
                    &ui,
                    &config,
                    installed_launch_options.as_ref(),
                    "Save",
                );
                let mut state = home_view_state(&config, None, &message);
                if matches!(
                    inspect_install(config.install_dir.as_deref()).state,
                    InstallState::Installed
                ) {
                    state.install_status = InstallState::LaunchableButMaybeOutdated
                        .status_text()
                        .to_string();
                    state.install_action_text = InstallState::LaunchableButMaybeOutdated
                        .primary_action()
                        .to_string();
                    state.home_support_text =
                        "Could not check for updates; installed DRH can still be launched."
                            .to_string();
                }
                state
            };
            apply_home_view_state(&ui, state);
        });
    });
}

fn refresh_home_state(ui: &AppWindow, config: &LauncherConfig, message: &str) {
    let state = home_view_state(config, None, message);
    apply_home_view_state(ui, state);
}

fn refresh_launch_options_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    launch_options: Option<&ManifestLaunchOptions>,
    save_text: &str,
) {
    let (view_options, custom_game_args) = launch_options_view_state(config, launch_options);
    let launch_options_state = launch_options_state(&view_options);

    ui.set_saved_pre_launch_command(config.pre_launch_command.clone().into());
    ui.set_saved_launch_arguments_mode(config.launch_arguments_mode.ui_index());
    ui.set_saved_custom_game_args(custom_game_args.clone().into());
    ui.set_saved_launch_options_state(launch_options_state.clone().into());

    ui.set_pre_launch_command(config.pre_launch_command.clone().into());
    ui.set_launch_arguments_mode(config.launch_arguments_mode.ui_index());
    ui.set_custom_game_args(custom_game_args.into());
    ui.set_launch_options_save_text(save_text.into());
    apply_launch_options_to_view(ui, view_options, ui.get_custom_game_args().to_string());
}

fn load_installed_launch_options(config: &LauncherConfig) -> Option<ManifestLaunchOptions> {
    let install_dir = config.install_dir.as_deref()?;
    install_metadata::InstalledState::load(install_dir)
        .ok()
        .and_then(|state| state.active.launch_options)
}

fn restore_previous_version_text(config: &LauncherConfig) -> String {
    restore_previous_release_version(config)
        .map(|version| format!("Restore {version}"))
        .unwrap_or_else(|| "Restore previous".to_string())
}

fn restore_previous_version_available(config: &LauncherConfig) -> bool {
    restore_previous_release_version(config).is_some()
}

fn restore_previous_release_version(config: &LauncherConfig) -> Option<String> {
    let install_dir = config.install_dir.as_deref()?;
    let state = install_metadata::InstalledState::load(install_dir).ok()?;
    state.previous.map(|previous| previous.version)
}

fn reinstall_current_version_text(config: &LauncherConfig) -> String {
    installed_active_release_version(config)
        .map(|version| format!("Reinstall {version}"))
        .unwrap_or_else(|| "Reinstall current".to_string())
}

fn reinstall_current_version_available(config: &LauncherConfig) -> bool {
    installed_active_release_version(config).is_some()
}

fn installed_active_release_version(config: &LauncherConfig) -> Option<String> {
    let install_dir = config.install_dir.as_deref()?;
    install_metadata::InstalledState::load(install_dir)
        .ok()
        .map(|state| state.active.version)
}

fn recommended_game_args_from_launch_options(
    launch_options: Option<&ManifestLaunchOptions>,
) -> Vec<String> {
    launch_options
        .map(recommended_launch_option_args)
        .unwrap_or_default()
}

fn recommended_launch_option_args(launch_options: &ManifestLaunchOptions) -> Vec<String> {
    launch_options
        .game_arguments
        .iter()
        .flat_map(|argument| match argument.recommended {
            Some(recommended) if recommended != argument.default => {
                vec![argument.flag.clone(), recommended.to_string()]
            }
            _ => Vec::new(),
        })
        .collect()
}

fn launch_options_view_state(
    config: &LauncherConfig,
    manifest_options: Option<&ManifestLaunchOptions>,
) -> (Vec<LaunchOptionView>, String) {
    let Some(manifest_options) = manifest_options else {
        let extra_args = if matches!(config.launch_arguments_mode, LaunchArgumentsMode::Custom) {
            config.game_args.join(" ")
        } else {
            config.game_args.join(" ")
        };
        return (Vec::new(), extra_args);
    };

    let (values, extra_args) = match config.launch_arguments_mode {
        LaunchArgumentsMode::GameDefaults => (
            manifest_options
                .game_arguments
                .iter()
                .map(|argument| argument.default)
                .collect(),
            config.game_args.clone(),
        ),
        LaunchArgumentsMode::Recommended => (
            manifest_options
                .game_arguments
                .iter()
                .map(|argument| argument.recommended.unwrap_or(argument.default))
                .collect(),
            config.game_args.clone(),
        ),
        LaunchArgumentsMode::Custom => parse_known_game_args(
            manifest_options,
            &config.game_args,
            default_launch_option_values(manifest_options),
        ),
    };

    (
        manifest_options
            .game_arguments
            .iter()
            .zip(values)
            .map(|(argument, checked)| LaunchOptionView {
                name: argument.name.clone().into(),
                flag: argument.flag.clone().into(),
                checked,
            })
            .collect(),
        quote_args_for_display(&extra_args),
    )
}

fn apply_launch_arguments_mode_to_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    manifest_options: Option<&ManifestLaunchOptions>,
    mode: LaunchArgumentsMode,
    extra_args: String,
) {
    ui.set_launch_arguments_mode(mode.ui_index());

    if matches!(mode, LaunchArgumentsMode::Custom) {
        return;
    }

    let preview_config = LauncherConfig {
        launch_arguments_mode: mode,
        game_args: config.game_args.clone(),
        ..config.clone()
    };
    let (options, _) = launch_options_view_state(&preview_config, manifest_options);
    apply_launch_options_to_view(ui, options, extra_args);
}

fn apply_launch_options_to_view(
    ui: &AppWindow,
    options: Vec<LaunchOptionView>,
    extra_args: String,
) {
    let state = launch_options_state(&options);
    ui.set_launch_option_min_width_px(132.0);
    ui.set_launch_options(ModelRc::new(VecModel::from(options)));
    ui.set_launch_options_state(state.into());
    ui.set_custom_game_args(extra_args.into());
}

fn launch_options_from_model(ui: &AppWindow) -> Vec<LaunchOptionView> {
    let model = ui.get_launch_options();
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .collect()
}

fn launch_options_state(options: &[LaunchOptionView]) -> String {
    options
        .iter()
        .map(|option| if option.checked { '1' } else { '0' })
        .collect()
}

fn launch_options_game_args(
    ui: &AppWindow,
    manifest_options: Option<&ManifestLaunchOptions>,
) -> Result<Vec<String>, String> {
    let mut args = if LaunchArgumentsMode::from_ui_index(ui.get_launch_arguments_mode())
        == LaunchArgumentsMode::Custom
    {
        known_launch_options_game_args(ui, manifest_options)
    } else {
        Vec::new()
    };
    let extra_args = game_launch::parse_command_line(ui.get_custom_game_args().trim())
        .map_err(|error| format!("Could not parse extra arguments: {error}"))?;
    args.extend(extra_args);
    Ok(args)
}

fn known_launch_options_game_args(
    ui: &AppWindow,
    manifest_options: Option<&ManifestLaunchOptions>,
) -> Vec<String> {
    let Some(manifest_options) = manifest_options else {
        return Vec::new();
    };

    launch_options_from_model(ui)
        .into_iter()
        .zip(manifest_options.game_arguments.iter())
        .flat_map(|(option, argument)| {
            if option.checked == argument.default {
                Vec::new()
            } else {
                vec![argument.flag.clone(), option.checked.to_string()]
            }
        })
        .collect()
}

fn default_launch_option_values(launch_options: &ManifestLaunchOptions) -> Vec<bool> {
    launch_options
        .game_arguments
        .iter()
        .map(|argument| argument.default)
        .collect()
}

fn parse_known_game_args(
    launch_options: &ManifestLaunchOptions,
    args: &[String],
    mut values: Vec<bool>,
) -> (Vec<bool>, Vec<String>) {
    let mut extra_args = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        let Some(option_index) = launch_options
            .game_arguments
            .iter()
            .position(|option| option.flag == *arg)
        else {
            extra_args.push(arg.clone());
            index += 1;
            continue;
        };

        let mut value = true;
        if let Some(next) = args.get(index + 1) {
            match next.to_ascii_lowercase().as_str() {
                "true" => {
                    value = true;
                    index += 1;
                }
                "false" => {
                    value = false;
                    index += 1;
                }
                _ => {}
            }
        }

        values[option_index] = value;
        index += 1;
    }

    (values, extra_args)
}

fn quote_args_for_display(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_arg_for_display(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_arg_for_display(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }

    if !arg.chars().any(char::is_whitespace) && !arg.contains('"') && !arg.contains('\\') {
        return arg.to_string();
    }

    format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
}

fn home_view_state(
    config: &LauncherConfig,
    latest_release: Option<&PlatformRelease>,
    message: &str,
) -> HomeViewState {
    let status = inspect_install(config.install_dir.as_deref());
    let version_status = status.version_text();
    let activity_message = status
        .reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
        .filter(|_| message == "Ready.")
        .unwrap_or(message);

    let mut state = HomeViewState {
        install_status: status.status_text(),
        install_action_text: status.state.primary_action().to_string(),
        install_action_enabled: true,
        version_status: version_status.clone(),
        home_support_text: home_support_text(&version_status, activity_message),
        update_check_text: "Check for updates".to_string(),
        update_check_enabled: true,
        open_install_folder_enabled: status.install_dir.as_deref().is_some_and(Path::exists),
        open_logs_folder_enabled: status.install_dir.as_deref().is_some_and(Path::exists),
        restore_previous_visible: restore_previous_version_available(config),
        restore_previous_enabled: restore_previous_version_available(config),
        restore_previous_text: restore_previous_version_text(config),
        reinstall_current_enabled: reinstall_current_version_available(config),
        reinstall_current_text: reinstall_current_version_text(config),
        status_detail: status_detail(activity_message),
    };

    if let Some(release) = latest_release {
        apply_release_to_home_view_state(&mut state, config, &status, release);
    }

    state
}

fn apply_release_to_home_view_state(
    state: &mut HomeViewState,
    config: &LauncherConfig,
    status: &game_install::InstallStatus,
    release: &PlatformRelease,
) {
    match status.state {
        InstallState::Installed | InstallState::LaunchableButMaybeOutdated => {
            match status.installed_version.as_deref() {
                Some(installed_version) if release_update_available(config, status, release) => {
                    state.install_status = "A DRH update is available".to_string();
                    state.install_action_text =
                        InstallState::UpdateAvailable.primary_action().to_string();
                    state.home_support_text = format!(
                        "Update available: installed {installed_version}, latest {}.",
                        release.version
                    );
                }
                Some(installed_version)
                    if rollback_blocked_update_version(config).as_deref()
                        == Some(release.version.as_str()) =>
                {
                    state.home_support_text = format!(
                        "You restored {installed_version}. Update {} is skipped until a newer release is available.",
                        release.version
                    );
                }
                Some(installed_version) => {
                    state.home_support_text = format!("You are up to date: {installed_version}.");
                }
                None => {
                    state.home_support_text = format!(
                        "Latest available version: {}. Installed version unknown.",
                        release.version
                    );
                }
            }
        }
        InstallState::NotInstalled => {
            state.home_support_text = format!("Latest available version: {}.", release.version);
        }
        InstallState::BrokenInstall => {
            state.home_support_text = format!(
                "Latest available version: {}. Repair is required.",
                release.version
            );
        }
        InstallState::UpdateAvailable | InstallState::Updating | InstallState::Playing => {}
    }
}

fn apply_home_view_state(ui: &AppWindow, state: HomeViewState) {
    ui.set_install_status(state.install_status.into());
    ui.set_install_action_text(state.install_action_text.into());
    ui.set_install_action_enabled(state.install_action_enabled);
    ui.set_version_status(state.version_status.into());
    ui.set_home_support_text(state.home_support_text.into());
    ui.set_update_check_text(state.update_check_text.into());
    ui.set_update_check_enabled(state.update_check_enabled);
    ui.set_open_install_folder_enabled(state.open_install_folder_enabled);
    ui.set_open_logs_folder_enabled(state.open_logs_folder_enabled);
    ui.set_restore_previous_visible(state.restore_previous_visible);
    ui.set_restore_previous_enabled(state.restore_previous_enabled);
    ui.set_restore_previous_text(state.restore_previous_text.into());
    ui.set_reinstall_current_enabled(state.reinstall_current_enabled);
    ui.set_reinstall_current_text(state.reinstall_current_text.into());
    ui.set_status_detail(state.status_detail.into());
}

fn refresh_settings_view(ui: &AppWindow, config: &LauncherConfig, save_text: &str) {
    ui.set_install_folder_path(install_folder_path_text(config).into());
    let cache_limit = config.download_cache_limit.to_string();
    ui.set_download_cache_limit(cache_limit.clone().into());
    ui.set_saved_download_cache_limit(cache_limit.into());
    ui.set_download_cache_save_text(save_text.into());
    ui.set_download_cache_limit_error("".into());
}

fn install_folder_path_text(config: &LauncherConfig) -> String {
    match config.install_dir.as_deref() {
        Some(path) => path.display().to_string(),
        None => format!(
            "No install folder selected yet. First install will use {}.",
            paths::default_install_dir().display()
        ),
    }
}

fn ui_download_cache_limit(ui: &AppWindow) -> Result<usize, ()> {
    ui.get_download_cache_limit()
        .trim()
        .parse::<usize>()
        .map_err(|_| ())
}

fn refresh_logs_view(ui: &AppWindow, config: &LauncherConfig) {
    let wrap_columns = if ui.get_log_source() == 0 {
        ui.get_launcher_log_wrap_columns()
    } else {
        ui.get_game_log_wrap_columns()
    }
    .max(20) as usize;
    let Some(install_dir) = config.install_dir.as_deref() else {
        ui.set_game_log_sessions(ModelRc::new(VecModel::from(Vec::new())));
        ui.set_selected_game_log_index(-1);
        ui.set_selected_game_log_enabled(false);
        ui.set_selected_game_log_title("No game session selected".into());
        ui.set_selected_game_log_id("".into());
        set_log_lines_if_changed(
            ui,
            log_lines_from_text("No install directory selected.", wrap_columns),
        );
        return;
    };

    let sessions = match game_logs::list(install_dir) {
        Ok(sessions) => sessions,
        Err(error) => {
            ui.set_game_log_sessions(ModelRc::new(VecModel::from(Vec::new())));
            ui.set_selected_game_log_index(-1);
            ui.set_selected_game_log_enabled(false);
            ui.set_selected_game_log_title("Could not list game sessions".into());
            ui.set_selected_game_log_id("".into());
            let lines = if ui.get_log_source() == 0 {
                log_lines(config, wrap_columns)
            } else {
                log_lines_from_text(&error, wrap_columns)
            };
            set_log_lines_if_changed(ui, lines);
            return;
        }
    };
    let session_views = sessions
        .iter()
        .map(|session| GameSessionView {
            title: session.title.clone().into(),
            detail: session.detail.clone().into(),
        })
        .collect::<Vec<_>>();
    ui.set_game_log_sessions(ModelRc::new(VecModel::from(session_views)));

    if ui.get_log_source() == 0 {
        ui.set_selected_game_log_enabled(!sessions.is_empty());
        set_log_lines_if_changed(ui, log_lines(config, wrap_columns));
        return;
    }

    let selected_index = match usize::try_from(ui.get_selected_game_log_index()) {
        Ok(index) if index < sessions.len() => Some(index),
        _ if sessions.is_empty() => None,
        _ => Some(0),
    };
    let Some(selected_index) = selected_index else {
        ui.set_selected_game_log_index(-1);
        ui.set_selected_game_log_enabled(false);
        ui.set_selected_game_log_title("No game sessions yet".into());
        ui.set_selected_game_log_id("".into());
        set_log_lines_if_changed(
            ui,
            log_lines_from_text(
                "Game session logs will appear here after launching DRH.",
                wrap_columns,
            ),
        );
        return;
    };

    let session = &sessions[selected_index];
    let content = game_logs::read(&session.path)
        .unwrap_or_else(|error| format!("Could not read game session log: {error}"));
    ui.set_selected_game_log_index(selected_index as i32);
    ui.set_selected_game_log_enabled(true);
    ui.set_selected_game_log_title(session.title.clone().into());
    ui.set_selected_game_log_id(game_log_session_id(&session.path).into());
    set_log_lines_if_changed(ui, log_lines_from_text(&content, wrap_columns));
}

fn set_log_lines_if_changed(ui: &AppWindow, lines: Vec<LogLineView>) {
    let current = ui.get_log_lines();
    let unchanged = current.row_count() == lines.len()
        && lines.iter().enumerate().all(|(index, line)| {
            current
                .row_data(index)
                .is_some_and(|current| current.text == line.text && current.color == line.color)
        });
    if !unchanged {
        ui.set_log_lines(ModelRc::new(VecModel::from(lines)));
    }
}

fn game_log_session_id(path: &Path) -> String {
    path.file_name()
        .map(|name| {
            let name = name.to_string_lossy();
            name.strip_suffix(".zst")
                .unwrap_or(name.as_ref())
                .to_string()
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn remember_game_log_position(
    positions: &mut HashMap<String, LogViewportPosition>,
    session_id: String,
    y: f32,
    at_end: bool,
) {
    positions.insert(session_id, LogViewportPosition { y, at_end });
}

fn saved_game_log_position(
    positions: &HashMap<String, LogViewportPosition>,
    session_id: &str,
) -> Option<LogViewportPosition> {
    positions.get(session_id).copied()
}

fn log_lines(config: &LauncherConfig, wrap_columns: usize) -> Vec<LogLineView> {
    let Some(install_dir) = config.install_dir.as_deref() else {
        return log_lines_from_text("No install directory selected.", wrap_columns);
    };

    let content = diagnostics::read_recent(install_dir)
        .unwrap_or_else(|error| format!("Could not read launcher log: {error}"));
    log_lines_from_text(&content, wrap_columns)
}

fn log_lines_from_text(content: &str, wrap_columns: usize) -> Vec<LogLineView> {
    content
        .lines()
        .flat_map(|line| {
            let color = log_line_color(line);
            split_display_line(line, wrap_columns)
                .into_iter()
                .map(move |text| LogLineView {
                    text: text.into(),
                    color: color.clone(),
                })
        })
        .collect()
}

fn split_display_line(line: &str, max_chars: usize) -> Vec<String> {
    if line.is_empty() || max_chars == 0 {
        return vec![line.to_string()];
    }

    let mut remaining = line;
    let mut segments = Vec::new();
    while let Some((hard_split, _)) = remaining.char_indices().nth(max_chars) {
        let preferred_split = preferred_display_split(&remaining[..hard_split], max_chars);
        let split_at = preferred_split.unwrap_or(hard_split);
        segments.push(remaining[..split_at].to_string());
        remaining = &remaining[split_at..];
    }
    segments.push(remaining.to_string());
    segments
}

fn preferred_display_split(candidate: &str, max_chars: usize) -> Option<usize> {
    let minimum_chars = (max_chars * 2 / 3).max(1);
    let mut preferred_split = None;

    for (char_index, (byte_index, character)) in candidate.char_indices().enumerate() {
        let split_at = byte_index + character.len_utf8();
        if char_index + 1 < minimum_chars {
            continue;
        }
        if character.is_whitespace() || matches!(character, ',' | ';' | ':') {
            preferred_split = Some(split_at);
        }
    }

    preferred_split
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LogLineLevel {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    Other,
}

fn log_line_level(line: &str) -> LogLineLevel {
    if line.contains("[FATAL]") {
        LogLineLevel::Fatal
    } else if line.contains("[ERROR]") {
        LogLineLevel::Error
    } else if line.contains("[WARN]") {
        LogLineLevel::Warn
    } else if line.contains("[INFO]") {
        LogLineLevel::Info
    } else if line.contains("[DEBUG]") {
        LogLineLevel::Debug
    } else {
        LogLineLevel::Other
    }
}

fn log_line_color(line: &str) -> Brush {
    match log_line_level(line) {
        LogLineLevel::Fatal => Color::from_rgb_u8(255, 34, 56).into(),
        LogLineLevel::Error => Color::from_rgb_u8(255, 108, 82).into(),
        LogLineLevel::Warn => Color::from_rgb_u8(240, 194, 102).into(),
        LogLineLevel::Info => Color::from_rgb_u8(201, 216, 205).into(),
        LogLineLevel::Debug => Color::from_rgb_u8(154, 164, 172).into(),
        LogLineLevel::Other => Color::from_rgb_u8(234, 216, 202).into(),
    }
}

fn log_for_config(config: &LauncherConfig, level: diagnostics::LogLevel, message: &str) {
    let Some(install_dir) = config.install_dir.as_deref() else {
        return;
    };

    let _ = diagnostics::write(install_dir, level, message);
}

fn is_error_message(message: &str) -> bool {
    message.starts_with("Could not ")
        || message.starts_with("No ")
        || message.contains(" failed")
        || message.contains(" missing")
}

fn release_update_available(
    config: &LauncherConfig,
    status: &game_install::InstallStatus,
    release: &PlatformRelease,
) -> bool {
    installed_version_needs_update(status.installed_version.as_deref(), &release.version)
        && rollback_blocked_update_version(config).as_deref() != Some(release.version.as_str())
}

fn rollback_blocked_update_version(config: &LauncherConfig) -> Option<String> {
    let install_dir = config.install_dir.as_deref()?;
    install_metadata::InstalledState::load(install_dir)
        .ok()
        .and_then(|state| state.blocked_update_version)
}

fn installed_version_needs_update(installed_version: Option<&str>, latest_version: &str) -> bool {
    installed_version
        .is_some_and(|installed_version| installed_version.trim() != latest_version.trim())
}

fn report_background_activity(ui: &slint::Weak<AppWindow>, message: String) {
    let ui = ui.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = ui.upgrade() else {
            return;
        };

        set_status_message(&ui, &message);
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

fn should_log_download_progress(percent: Option<u64>, last_logged_percent: Option<u64>) -> bool {
    match percent {
        Some(percent) if percent == 0 || percent == 100 => last_logged_percent != Some(percent),
        Some(percent) if percent % 10 == 0 => last_logged_percent != Some(percent),
        None => last_logged_percent.is_none(),
        _ => false,
    }
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
    ui.set_open_install_folder_enabled(status.install_dir.as_deref().is_some_and(Path::exists));
    ui.set_open_logs_folder_enabled(status.install_dir.as_deref().is_some_and(Path::exists));
    ui.set_restore_previous_visible(restore_previous_version_available(config));
    ui.set_restore_previous_enabled(false);
    ui.set_reinstall_current_enabled(false);
    refresh_logs_view(ui, config);
    set_status_message(ui, message);
}

fn open_folder(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }

    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("explorer");
        command.arg(path);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(path);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn open_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{} does not exist", path.display()));
    }

    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(path);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(path);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn open_external_link_from_ui(ui: &AppWindow, config: &LauncherConfig, label: &str, url: &str) {
    match open_url(url) {
        Ok(()) => {
            log_for_config(
                config,
                diagnostics::LogLevel::Info,
                &format!("{label} link opened."),
            );
            set_status_message(ui, &format!("{label} link opened."));
        }
        Err(error) => {
            let message = format!("Could not open {label} link: {error}");
            log_for_config(config, diagnostics::LogLevel::Error, &message);
            set_status_message(ui, &message);
        }
    }
}

fn open_url(url: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn open_logs_folder(install_dir: &Path) -> Result<(), String> {
    let logs_dir = paths::logs_dir(install_dir);
    std::fs::create_dir_all(&logs_dir)
        .map_err(|error| format!("Could not create {}: {error}", logs_dir.display()))?;
    open_folder(&logs_dir)
}

fn open_config_folder() -> Result<(), String> {
    let config_file = paths::config_file();
    let Some(config_dir) = config_file.parent() else {
        return Err(format!(
            "Could not determine config folder for {}",
            config_file.display()
        ));
    };

    std::fs::create_dir_all(config_dir)
        .map_err(|error| format!("Could not create {}: {error}", config_dir.display()))?;
    open_folder(config_dir)
}

fn set_status_message(ui: &AppWindow, message: &str) {
    let version_text = ui.get_version_status();
    ui.set_home_support_text(home_support_text(&version_text, message).into());
    ui.set_status_detail(status_detail(message).into());
}

fn home_support_text(version_text: &str, message: &str) -> String {
    if message.starts_with("Ready.") || message.starts_with("DRH is running.") {
        return version_text.to_string();
    }

    if is_progress_message(message) {
        return message.to_string();
    }

    if let Some(installed_version) = message
        .strip_prefix("Installed ")
        .and_then(|message| message.split_once('.').map(|(version, _)| version))
    {
        return format!("Installed {installed_version}.");
    }

    if message.starts_with("Latest known version: ") {
        return message.to_string();
    }

    if message.starts_with("DRH stopped.") {
        return "DRH has stopped.".to_string();
    }

    if message.starts_with("DRH exited ") {
        return "DRH exited.".to_string();
    }

    if message.starts_with("Could not ") || message.starts_with("No ") {
        return "Action failed. Check logs for details.".to_string();
    }

    version_text.to_string()
}

fn is_progress_message(message: &str) -> bool {
    message.starts_with("Checking ")
        || message.starts_with("Preparing download ")
        || message.starts_with("Downloading ")
        || message.starts_with("Verifying download")
        || message.starts_with("Extracting archive")
        || message.starts_with("Installing files")
}

#[cfg(test)]
mod home_support_text_tests {
    use super::*;
    use crate::github_releases::{ReleaseAsset, ReleaseMetadataSource};
    use crate::install_metadata::{InstalledRelease, InstalledState};
    use tempfile::tempdir;

    #[test]
    fn keeps_durable_ready_state_on_version_text() {
        assert_eq!(home_support_text("Version: V1", "Ready."), "Version: V1");
        assert_eq!(
            home_support_text("Version: V1", "Install folder opened."),
            "Version: V1"
        );
    }

    #[test]
    fn shows_progress_messages() {
        assert_eq!(
            home_support_text("Version: V1", "Extracting archive..."),
            "Extracting archive..."
        );
        assert_eq!(
            home_support_text("Version: V1", "Installing files..."),
            "Installing files..."
        );
    }

    #[test]
    fn summarizes_terminal_and_error_states() {
        assert_eq!(
            home_support_text("Version: V1", "Installed V2."),
            "Installed V2."
        );
        assert_eq!(
            home_support_text("Version: V1", "Could not open logs folder: denied"),
            "Action failed. Check logs for details."
        );
    }

    #[test]
    fn recognizes_all_game_log_levels() {
        assert_eq!(log_line_level("[DEBUG] details"), LogLineLevel::Debug);
        assert_eq!(log_line_level("[INFO] details"), LogLineLevel::Info);
        assert_eq!(log_line_level("[WARN] details"), LogLineLevel::Warn);
        assert_eq!(log_line_level("[ERROR] details"), LogLineLevel::Error);
        assert_eq!(log_line_level("[FATAL] details"), LogLineLevel::Fatal);
        assert_eq!(log_line_level("plain output"), LogLineLevel::Other);
    }

    #[test]
    fn splits_long_display_lines_without_losing_unicode_content() {
        let line = "é".repeat(11);
        let segments = split_display_line(&line, 4);

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.chars().count())
                .collect::<Vec<_>>(),
            vec![4, 4, 3]
        );
        assert_eq!(segments.concat(), line);
    }

    #[test]
    fn prefers_readable_display_line_boundaries() {
        let whitespace_line = "123456 890123";
        let punctuation_line = "123456,890123";
        let nearest_separator_line = "123456 89,0123";
        let hard_split_line = "abcdefghijk";

        assert_eq!(
            split_display_line(whitespace_line, 10),
            vec!["123456 ", "890123"]
        );
        assert_eq!(
            split_display_line(punctuation_line, 10),
            vec!["123456,", "890123"]
        );
        assert_eq!(
            split_display_line(nearest_separator_line, 10),
            vec!["123456 89,", "0123"]
        );
        assert_eq!(
            split_display_line(hard_split_line, 10),
            vec!["abcdefghij", "k"]
        );
    }

    #[test]
    fn remembers_scroll_positions_for_each_game_log_file() {
        let mut positions = HashMap::new();
        remember_game_log_position(&mut positions, "session-a.log".to_string(), -120.0, false);
        remember_game_log_position(&mut positions, "session-b.log".to_string(), -340.0, true);

        assert_eq!(
            saved_game_log_position(&positions, "session-a.log"),
            Some(LogViewportPosition {
                y: -120.0,
                at_end: false,
            })
        );
        assert_eq!(
            saved_game_log_position(&positions, "session-b.log"),
            Some(LogViewportPosition {
                y: -340.0,
                at_end: true,
            })
        );
        assert_eq!(saved_game_log_position(&positions, "session-c.log"), None);
        assert_eq!(
            game_log_session_id(Path::new("session-a.log.zst")),
            game_log_session_id(Path::new("session-a.log"))
        );
    }

    #[test]
    fn skips_rollback_blocked_update_until_a_newer_release_exists() {
        let temp = tempdir().unwrap();
        InstalledState {
            active: test_installed_release("V9"),
            previous: Some(test_installed_release("V10")),
            blocked_update_version: Some("V10".to_string()),
        }
        .save(temp.path())
        .unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };
        let status = game_install::InstallStatus {
            state: InstallState::Installed,
            install_dir: Some(temp.path().to_path_buf()),
            installed_version: Some("V9".to_string()),
            reason: None,
        };

        assert!(!release_update_available(
            &config,
            &status,
            &test_release("V10")
        ));
        assert!(release_update_available(
            &config,
            &status,
            &test_release("V11")
        ));
    }

    #[test]
    fn keeps_restore_visible_when_previous_directory_is_broken() {
        let temp = tempdir().unwrap();
        InstalledState {
            active: test_installed_release("V9"),
            previous: Some(test_installed_release("V10")),
            blocked_update_version: None,
        }
        .save(temp.path())
        .unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };

        assert!(restore_previous_version_available(&config));
        assert_eq!(restore_previous_version_text(&config), "Restore V10");
    }

    fn test_release(version: &str) -> PlatformRelease {
        PlatformRelease {
            version: version.to_string(),
            name: format!("Dungeon Rampage Haxe {version}"),
            html_url: format!("https://example.test/{version}"),
            metadata_source: ReleaseMetadataSource::GitHubAssetFallback,
            launch_options: None,
            asset: ReleaseAsset {
                platform_id: "linux-x64".to_string(),
                name: format!("Dungeon.Rampage.Haxe.{version}.Linux.tar.gz"),
                download_url: "https://example.test/archive.tar.gz".to_string(),
                size: 123,
                digest: Some("sha256:abc123".to_string()),
            },
        }
    }

    fn test_installed_release(version: &str) -> InstalledRelease {
        InstalledRelease {
            version: version.to_string(),
            platform: "linux-x64".to_string(),
            source: "Tutez64/DRHL-Release-Fixtures".to_string(),
            release_url: format!("https://example.test/{version}"),
            archive: format!("archive-{version}.tar.gz"),
            archive_sha256: "abc123".to_string(),
            archive_size: 123,
            installed_at: "unix:0".to_string(),
            launch_options: None,
        }
    }
}

fn status_detail(message: &str) -> String {
    if message == "Ready." {
        "No operation in progress.".to_string()
    } else {
        message.to_string()
    }
}

fn start_game_monitor(
    timer: Rc<Timer>,
    ui: slint::Weak<AppWindow>,
    config: LauncherConfig,
    game_process: Rc<RefCell<Option<game_launch::RunningGame>>>,
) {
    let timer_handle = Rc::clone(&timer);
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        let finished = {
            let mut process = game_process.borrow_mut();
            let Some(game) = process.as_mut() else {
                timer_handle.stop();
                return;
            };

            match game.child.try_wait() {
                Ok(Some(status)) => {
                    let game = process.take().expect("running game disappeared");
                    let _ = game_logs::finish(
                        &game.session_log,
                        &format!("Exited with status: {status}"),
                    );
                    Some((
                        diagnostics::LogLevel::Info,
                        format!("DRH exited with status: {status}"),
                    ))
                }
                Ok(None) => None,
                Err(error) => {
                    let game = process.take().expect("running game disappeared");
                    let _ = game_logs::finish(
                        &game.session_log,
                        &format!("Process inspection failed: {error}"),
                    );
                    Some((
                        diagnostics::LogLevel::Error,
                        format!("Could not inspect DRH process: {error}"),
                    ))
                }
            }
        };

        let Some((level, message)) = finished else {
            return;
        };

        timer_handle.stop();
        log_for_config(&config, level, &message);
        if let Some(ui) = ui.upgrade() {
            refresh_home_state(&ui, &config, &message);
            refresh_logs_view(&ui, &config);
        }
    });
}

fn process_is_running(game_process: &Rc<RefCell<Option<game_launch::RunningGame>>>) -> bool {
    let mut process = game_process.borrow_mut();
    let Some(game) = process.as_mut() else {
        return false;
    };

    match game.child.try_wait() {
        Ok(None) => true,
        Ok(Some(status)) => {
            let game = process.take().expect("running game disappeared");
            let _ = game_logs::finish(&game.session_log, &format!("Exited with status: {status}"));
            false
        }
        Err(_) => true,
    }
}

fn stop_game(game_process: &Rc<RefCell<Option<game_launch::RunningGame>>>) -> String {
    let mut process = game_process.borrow_mut();
    let Some(mut game) = process.take() else {
        return "DRH is not running.".to_string();
    };

    match game.child.kill() {
        Ok(()) => {
            let status = game.child.wait().ok();
            let result = status
                .map(|status| format!("Stopped by user with status: {status}"))
                .unwrap_or_else(|| "Stopped by user".to_string());
            let _ = game_logs::finish(&game.session_log, &result);
            "DRH stopped.".to_string()
        }
        Err(error) => {
            process.replace(game);
            format!("Could not stop DRH: {error}")
        }
    }
}
