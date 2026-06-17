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
mod launcher_update;
mod linux_appimage;
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
use std::time::{Duration, Instant};

use archive::extract_to_staging;
use config::{LaunchArgumentsMode, LauncherConfig};
use download::{
    DownloadProgress, download_and_verify_with_progress, prune_download_cache,
    update_download_cache, verify_cached_archive_by_metadata,
};
use game_install::inspect_install;
use github_releases::{
    PlatformRelease, PlatformReleaseHistoryEntry, ReleaseMetadataSource, RepositoryRelease,
    discover_latest_platform_release, discover_latest_platform_release_for_install,
    discover_platform_release_by_tag_for_install, discover_platform_release_history,
    discover_repository_release_history,
};
use install_state::InstallState;
use installer::{
    install_extracted_archive_with_blocked_update, repair_active_install_from_extracted_archive,
    repair_release_from_extracted_archive, restore_previous_version,
};
use platform::Platform;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use release_manifest::ManifestLaunchOptions;
use release_source::ReleaseSource;
use slint::private_unstable_api::re_exports::{StyledText, string_to_styled_text};
use slint::{Brush, CloseRequestResponse, Color, Model, ModelRc, Timer, TimerMode, VecModel};

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

fn main() {
    install_panic_hook();

    if let Err(error) = run() {
        let message = format!("DRH Launcher failed to start: {error}");
        if retry_with_software_renderer_if_needed(&message) {
            return;
        }
        report_startup_failure(&message);
        std::process::exit(1);
    }
}

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

fn run() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    slint::set_xdg_app_id(XDG_APP_ID)?;

    let config = Rc::new(RefCell::new(LauncherConfig::load()));
    let latest_release = Arc::new(Mutex::new(None::<PlatformRelease>));
    let latest_launcher_update = Arc::new(Mutex::new(None::<launcher_update::LauncherUpdate>));
    let drh_version_history = Arc::new(Mutex::new(Vec::<PlatformReleaseHistoryEntry>::new()));
    let launcher_version_history = Arc::new(Mutex::new(Vec::<RepositoryRelease>::new()));
    let pending_drh_version_install = Arc::new(Mutex::new(None::<String>));
    let game_process = Rc::new(RefCell::new(None::<game_launch::RunningGame>));
    let game_monitor = Rc::new(Timer::default());
    let game_stop_timer = Rc::new(Timer::default());
    let game_log_positions = Rc::new(RefCell::new(HashMap::<String, LogViewportPosition>::new()));
    let release_source = ReleaseSource::from_environment();

    ui.set_launcher_version(env!("CARGO_PKG_VERSION").into());
    refresh_linux_integration_view(&ui);
    if matches!(
        linux_appimage::state(),
        linux_appimage::IntegrationState::Portable | linux_appimage::IntegrationState::NeedsRepair
    ) {
        ui.set_appimage_dialog_mode(1);
        ui.set_appimage_dialog_visible(true);
    }
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
        let game_stop_timer = Rc::clone(&game_stop_timer);
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
                        game_monitor.stop();
                        refresh_playing_state(&ui, &config, "Stopping DRH...");
                        ui.set_install_action_enabled(false);
                        begin_game_stop(
                            Rc::clone(&game_stop_timer),
                            ui.as_weak(),
                            config.clone(),
                            Rc::clone(&game_process),
                            Rc::clone(&game_monitor),
                            false,
                        );
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
        ui.unwrap().on_linux_integration_action(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            match linux_appimage::state() {
                linux_appimage::IntegrationState::Portable
                | linux_appimage::IntegrationState::NeedsRepair => {
                    ui.set_appimage_dialog_mode(1);
                    ui.set_appimage_dialog_error("".into());
                    ui.set_appimage_dialog_visible(true);
                }
                linux_appimage::IntegrationState::Installed => {
                    ui.set_appimage_dialog_mode(2);
                    ui.set_appimage_dialog_error("".into());
                    ui.set_appimage_dialog_visible(true);
                }
                linux_appimage::IntegrationState::NotAppImage => {}
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_linux_integration_folder(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let result = linux_appimage::application_path()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .ok_or_else(|| "Could not locate the DRH Launcher AppImage folder.".to_string())
                .and_then(|folder| open_folder(&folder));

            match result {
                Ok(()) => set_status_message(&ui, "Application folder opened."),
                Err(error) => {
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &error);
                    set_status_message(&ui, &format!("Could not open application folder: {error}"));
                }
            }
        });
    }

    {
        let ui = ui.as_weak();
        ui.unwrap().on_cancel_appimage_dialog(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if ui.get_appimage_dialog_busy() {
                return;
            }
            ui.set_appimage_dialog_visible(false);
            ui.set_appimage_dialog_error("".into());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let game_process = Rc::clone(&game_process);
        ui.unwrap().on_confirm_appimage_action(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            if ui.get_appimage_dialog_busy() {
                return;
            }

            let mode = ui.get_appimage_dialog_mode();
            if process_is_running(&game_process) {
                ui.set_appimage_dialog_error(appimage_running_game_message(mode).into());
                return;
            }

            ui.set_appimage_dialog_busy(true);
            ui.set_appimage_dialog_error("".into());
            let ui = ui.as_weak();
            let config = config.borrow().clone();
            thread::spawn(move || {
                let result = if mode == 2 {
                    linux_appimage::uninstall()
                } else {
                    linux_appimage::install_and_restart()
                };

                if let Err(error) = result {
                    log_for_config(&config, diagnostics::LogLevel::Error, &error);
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = ui.upgrade() else {
                            return;
                        };
                        ui.set_appimage_dialog_busy(false);
                        ui.set_appimage_dialog_error(error.into());
                        refresh_linux_integration_view(&ui);
                    });
                    return;
                }

                if mode == 2 {
                    std::process::exit(0);
                }
            });
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let drh_version_history = Arc::clone(&drh_version_history);
        let launcher_version_history = Arc::clone(&launcher_version_history);
        let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
        ui.unwrap().on_cancel_selected_drh_version_install(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            pending_drh_version_install
                .lock()
                .expect("pending DRH version install lock poisoned")
                .take();
            refresh_selected_version_view(
                &ui,
                &config.borrow(),
                &drh_version_history,
                &launcher_version_history,
                &pending_drh_version_install,
            );
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let game_process = Rc::clone(&game_process);
        let latest_launcher_update = Arc::clone(&latest_launcher_update);
        ui.unwrap().on_launcher_update_action(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if !launcher_update::automatic_update_supported() {
                return;
            }

            if process_is_running(&game_process) {
                let message = "Stop DRH before updating DRH Launcher.";
                log_for_config(&config.borrow(), diagnostics::LogLevel::Warn, message);
                ui.set_launcher_update_status(message.into());
                return;
            }

            let update = latest_launcher_update
                .lock()
                .expect("launcher update lock poisoned")
                .clone();
            let Some(update) = update else {
                start_launcher_update_check(
                    &ui,
                    config.borrow().clone(),
                    Arc::clone(&latest_launcher_update),
                );
                return;
            };

            let version = update.version().to_string();
            let message = format!("Installing DRH Launcher {version}...");
            log_for_config(&config.borrow(), diagnostics::LogLevel::Info, &message);
            ui.set_launcher_update_status(message.into());
            ui.set_launcher_update_action_text("Installing...".into());
            ui.set_launcher_update_action_enabled(false);

            let ui = ui.as_weak();
            let config = config.borrow().clone();
            thread::spawn(move || {
                if let Err(error) = update.install_and_restart() {
                    log_for_config(&config, diagnostics::LogLevel::Error, &error);
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(ui) = ui.upgrade() else {
                            return;
                        };
                        ui.set_launcher_update_status(error.into());
                        ui.set_launcher_update_action_text("Try again".into());
                        ui.set_launcher_update_action_enabled(true);
                    });
                }
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
        let drh_version_history = Arc::clone(&drh_version_history);
        let launcher_version_history = Arc::clone(&launcher_version_history);
        let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
        let release_source = release_source.clone();
        ui.unwrap().on_ensure_version_history(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let has_history = !drh_version_history
                .lock()
                .expect("DRH version history lock poisoned")
                .is_empty()
                || !launcher_version_history
                    .lock()
                    .expect("launcher version history lock poisoned")
                    .is_empty();
            if has_history {
                refresh_selected_version_view(
                    &ui,
                    &config.borrow(),
                    &drh_version_history,
                    &launcher_version_history,
                    &pending_drh_version_install,
                );
            } else if ui.get_refresh_version_history_enabled() {
                start_version_history_refresh(
                    &ui,
                    config.borrow().clone(),
                    release_source.clone(),
                    Arc::clone(&drh_version_history),
                    Arc::clone(&launcher_version_history),
                    Arc::clone(&pending_drh_version_install),
                );
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let drh_version_history = Arc::clone(&drh_version_history);
        let launcher_version_history = Arc::clone(&launcher_version_history);
        let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
        let release_source = release_source.clone();
        ui.unwrap().on_refresh_version_history(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if !ui.get_refresh_version_history_enabled() {
                return;
            }

            start_version_history_refresh(
                &ui,
                config.borrow().clone(),
                release_source.clone(),
                Arc::clone(&drh_version_history),
                Arc::clone(&launcher_version_history),
                Arc::clone(&pending_drh_version_install),
            );
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let drh_version_history = Arc::clone(&drh_version_history);
        let launcher_version_history = Arc::clone(&launcher_version_history);
        let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
        let release_source = release_source.clone();
        ui.unwrap().on_select_version_history_source(move |source| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            ui.set_version_history_page(source);
            refresh_selected_version_view(
                &ui,
                &config.borrow(),
                &drh_version_history,
                &launcher_version_history,
                &pending_drh_version_install,
            );
            let source_empty = if source == 0 {
                drh_version_history
                    .lock()
                    .expect("DRH version history lock poisoned")
                    .is_empty()
            } else {
                launcher_version_history
                    .lock()
                    .expect("launcher version history lock poisoned")
                    .is_empty()
            };
            if source_empty && ui.get_refresh_version_history_enabled() {
                start_version_history_refresh(
                    &ui,
                    config.borrow().clone(),
                    release_source.clone(),
                    Arc::clone(&drh_version_history),
                    Arc::clone(&launcher_version_history),
                    Arc::clone(&pending_drh_version_install),
                );
            }
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let drh_version_history = Arc::clone(&drh_version_history);
        let launcher_version_history = Arc::clone(&launcher_version_history);
        let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
        ui.unwrap().on_select_drh_version(move |index| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            ui.set_selected_drh_version_index(index);
            pending_drh_version_install
                .lock()
                .expect("pending DRH version install lock poisoned")
                .take();
            refresh_selected_version_view(
                &ui,
                &config.borrow(),
                &drh_version_history,
                &launcher_version_history,
                &pending_drh_version_install,
            );
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let drh_version_history = Arc::clone(&drh_version_history);
        let launcher_version_history = Arc::clone(&launcher_version_history);
        let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
        ui.unwrap().on_select_launcher_version(move |index| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            ui.set_selected_launcher_version_index(index);
            refresh_selected_version_view(
                &ui,
                &config.borrow(),
                &drh_version_history,
                &launcher_version_history,
                &pending_drh_version_install,
            );
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let drh_version_history = Arc::clone(&drh_version_history);
        let launcher_version_history = Arc::clone(&launcher_version_history);
        ui.unwrap().on_open_selected_version_release(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let release_url =
                selected_version_release_url(&ui, &drh_version_history, &launcher_version_history);
            let Some(release_url) = release_url else {
                set_status_message(&ui, "No release selected.");
                return;
            };

            open_external_link_from_ui(&ui, &config.borrow(), "Release", &release_url);
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_markdown_link(move |url| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            open_external_link_from_ui(&ui, &config.borrow(), "Changelog link", url.as_str());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let game_process = Rc::clone(&game_process);
        let latest_release = Arc::clone(&latest_release);
        let drh_version_history = Arc::clone(&drh_version_history);
        let launcher_version_history = Arc::clone(&launcher_version_history);
        let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
        let release_source = release_source.clone();
        ui.unwrap().on_install_selected_drh_version(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if process_is_running(&game_process) {
                let message = "Stop DRH before installing another version.";
                log_for_config(&config.borrow(), diagnostics::LogLevel::Warn, message);
                set_status_message(&ui, message);
                return;
            }

            let selected = selected_drh_history_entry(&ui, &drh_version_history);
            let Some(selected) = selected else {
                set_status_message(&ui, "No DRH release selected.");
                return;
            };
            if selected.platform_release.is_none() && !selected.manifest_available {
                set_status_message(&ui, "Selected release is not available for this platform.");
                return;
            }

            let version = selected.release.version.clone();
            let installed_version = installed_active_release_version(&config.borrow());
            if installed_version
                .as_deref()
                .is_some_and(|installed| installed.trim() != version.trim())
            {
                let mut pending = pending_drh_version_install
                    .lock()
                    .expect("pending DRH version install lock poisoned");
                if pending.as_deref() != Some(version.as_str()) {
                    pending.replace(version.clone());
                    drop(pending);
                    refresh_selected_version_view(
                        &ui,
                        &config.borrow(),
                        &drh_version_history,
                        &launcher_version_history,
                        &pending_drh_version_install,
                    );
                    return;
                }
            }

            {
                let mut config = config.borrow_mut();
                if config.install_dir.is_none() {
                    config.install_dir = Some(paths::default_install_dir());
                    if let Err(error) = config.save() {
                        let message = format!("Could not save configuration: {error}");
                        log_for_config(&config, diagnostics::LogLevel::Error, &message);
                        set_status_message(&ui, &message);
                        return;
                    }
                    refresh_settings_view(&ui, &config, "Save");
                }
            }

            let config_snapshot = config.borrow().clone();
            let Some(install_dir) = config_snapshot.install_dir.clone() else {
                set_status_message(&ui, "No install directory selected.");
                return;
            };

            pending_drh_version_install
                .lock()
                .expect("pending DRH version install lock poisoned")
                .take();
            refresh_home_state(&ui, &config_snapshot, &format!("Installing {version}..."));
            ui.set_install_action_enabled(false);
            ui.set_update_check_enabled(false);
            ui.set_restore_previous_enabled(false);
            ui.set_reinstall_current_enabled(false);
            ui.set_install_action_text(InstallState::Updating.primary_action().into());
            ui.set_refresh_version_history_enabled(false);
            ui.set_selected_version_action_enabled(false);
            ui.set_selected_version_replace_confirmation_visible(false);
            set_status_message(&ui, &format!("Installing DRH {version}..."));

            let ui = ui.as_weak();
            let release_source = release_source.clone();
            let latest_release = Arc::clone(&latest_release);
            let drh_version_history = Arc::clone(&drh_version_history);
            let launcher_version_history = Arc::clone(&launcher_version_history);
            let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
            thread::spawn(move || {
                let latest_version =
                    latest_drh_release_version_for_update_block(&latest_release, &release_source);
                let message = match discover_platform_release_by_tag_for_install(
                    &release_source,
                    Platform::current(),
                    &version,
                ) {
                    Ok(release) => install_platform_release(
                        &ui,
                        &install_dir,
                        &config_snapshot,
                        &release_source,
                        InstallPlatformReleaseRequest {
                            latest_release: None,
                            release,
                            repair_current: false,
                            blocked_update_version: selected_history_blocked_update_version(
                                latest_version.as_deref(),
                                version.as_str(),
                            ),
                        },
                    ),
                    Err(error) => error,
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

                    let installed_launch_options = load_installed_launch_options(&config_snapshot);
                    refresh_launch_options_view(
                        &ui,
                        &config_snapshot,
                        installed_launch_options.as_ref(),
                        "Save",
                    );
                    refresh_home_state(&ui, &config_snapshot, &message);
                    ui.set_install_action_enabled(true);
                    ui.set_update_check_enabled(true);
                    ui.set_update_check_text("Check for updates".into());
                    ui.set_refresh_version_history_enabled(true);
                    set_status_message(&ui, &message);
                    refresh_version_history_selection(
                        &ui,
                        &config_snapshot,
                        &drh_version_history,
                        &launcher_version_history,
                        &pending_drh_version_install,
                    );
                });
            });
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

    {
        let ui = ui.as_weak();
        let game_process = Rc::clone(&game_process);
        ui.unwrap().window().on_close_requested(move || {
            let Some(ui) = ui.upgrade() else {
                return CloseRequestResponse::HideWindow;
            };

            if ui.get_close_confirmation_visible() {
                return CloseRequestResponse::KeepWindowShown;
            }

            if !process_is_running(&game_process) {
                return CloseRequestResponse::HideWindow;
            }

            ui.set_close_confirmation_error("".into());
            ui.set_close_confirmation_busy(false);
            ui.set_close_confirmation_visible(true);
            CloseRequestResponse::KeepWindowShown
        });
    }

    {
        let ui = ui.as_weak();
        ui.unwrap().on_cancel_close(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if ui.get_close_confirmation_busy() {
                return;
            }

            ui.set_close_confirmation_visible(false);
            ui.set_close_confirmation_error("".into());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let game_process = Rc::clone(&game_process);
        let game_monitor = Rc::clone(&game_monitor);
        let game_stop_timer = Rc::clone(&game_stop_timer);
        ui.unwrap().on_confirm_close(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if ui.get_close_confirmation_busy() {
                return;
            }

            ui.set_close_confirmation_busy(true);
            ui.set_close_confirmation_error("".into());
            game_monitor.stop();
            begin_game_stop(
                Rc::clone(&game_stop_timer),
                ui.as_weak(),
                config.borrow().clone(),
                Rc::clone(&game_process),
                Rc::clone(&game_monitor),
                true,
            );
        });
    }

    start_release_check(
        &ui,
        config.borrow().clone(),
        Arc::clone(&latest_release),
        release_source.clone(),
    );
    start_launcher_update_check(
        &ui,
        config.borrow().clone(),
        Arc::clone(&latest_launcher_update),
    );

    ui.run()
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report_startup_failure(&format!("DRH Launcher panicked: {info}"));
        default_hook(info);
    }));
}

fn report_startup_failure(message: &str) {
    let install_dir = paths::default_install_dir();
    let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Error, message);
    show_startup_failure(message);
}

#[cfg(windows)]
fn retry_with_software_renderer_if_needed(message: &str) -> bool {
    const RETRY_ENV: &str = "DRHL_SOFTWARE_RENDERER_RETRY";
    const SLINT_BACKEND_ENV: &str = "SLINT_BACKEND";

    if std::env::var_os(SLINT_BACKEND_ENV).is_some() || std::env::var_os(RETRY_ENV).is_some() {
        return false;
    }
    if !message.contains("glCreateShader") {
        return false;
    }

    let Ok(executable) = std::env::current_exe() else {
        return false;
    };

    let mut command = Command::new(executable);
    command
        .args(std::env::args_os().skip(1))
        .env(SLINT_BACKEND_ENV, "winit-software")
        .env(RETRY_ENV, "1");

    match command.spawn() {
        Ok(_) => {
            let install_dir = paths::default_install_dir();
            let _ = diagnostics::write(
                &install_dir,
                diagnostics::LogLevel::Warn,
                "OpenGL startup failed; retrying with Slint software renderer.",
            );
            true
        }
        Err(error) => {
            let install_dir = paths::default_install_dir();
            let _ = diagnostics::write(
                &install_dir,
                diagnostics::LogLevel::Error,
                &format!("Could not retry with Slint software renderer: {error}"),
            );
            false
        }
    }
}

#[cfg(not(windows))]
fn retry_with_software_renderer_if_needed(_message: &str) -> bool {
    false
}

#[cfg(windows)]
fn show_startup_failure(message: &str) {
    use std::ffi::OsStr;
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(iter::once(0))
            .collect()
    }

    let text = wide(message);
    let title = wide("DRH Launcher");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_startup_failure(message: &str) {
    eprintln!("{message}");
}

fn start_launcher_update_check(
    ui: &AppWindow,
    config: LauncherConfig,
    latest_launcher_update: Arc<Mutex<Option<launcher_update::LauncherUpdate>>>,
) {
    latest_launcher_update
        .lock()
        .expect("launcher update lock poisoned")
        .take();

    if !launcher_update::automatic_update_supported() {
        ui.set_launcher_update_visible(false);
        return;
    }

    ui.set_launcher_update_visible(false);

    let ui = ui.as_weak();
    thread::spawn(move || {
        let result = launcher_update::check();
        let message = match &result {
            Ok(Some(update)) => format!(
                "DRH Launcher {} is available. The launcher will restart after updating.",
                update.version()
            ),
            Ok(None) => format!("DRH Launcher {} is up to date.", env!("CARGO_PKG_VERSION")),
            Err(error) => error.clone(),
        };
        let level = if result.is_err() {
            diagnostics::LogLevel::Warn
        } else {
            diagnostics::LogLevel::Info
        };
        log_for_config(&config, level, &message);

        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            match result {
                Ok(Some(update)) => {
                    latest_launcher_update
                        .lock()
                        .expect("launcher update lock poisoned")
                        .replace(update);
                    ui.set_launcher_update_status(message.into());
                    ui.set_launcher_update_action_text("Update and restart".into());
                    ui.set_launcher_update_action_enabled(true);
                    ui.set_launcher_update_visible(true);
                }
                Ok(None) => {
                    ui.set_launcher_update_visible(false);
                }
                Err(_) => {
                    ui.set_launcher_update_visible(false);
                }
            }
        });
    });
}

fn refresh_linux_integration_view(ui: &AppWindow) {
    let (visible, advanced, status, action) = match linux_appimage::state() {
        linux_appimage::IntegrationState::NotAppImage => (false, false, "", ""),
        linux_appimage::IntegrationState::Portable => (
            true,
            false,
            "DRH Launcher is currently running as a portable AppImage. Install it to add it to your applications.",
            "Install",
        ),
        linux_appimage::IntegrationState::Installed => (
            true,
            true,
            "DRH Launcher is installed for the current Linux user.",
            "Uninstall",
        ),
        linux_appimage::IntegrationState::NeedsRepair => (
            true,
            false,
            "The AppImage is installed, but its desktop integration is incomplete.",
            "Repair",
        ),
    };
    let application_path = linux_appimage::application_path();
    let open_folder_enabled = application_path
        .as_deref()
        .and_then(Path::parent)
        .is_some_and(Path::is_dir);
    ui.set_linux_integration_visible(visible);
    ui.set_linux_integration_advanced(advanced);
    ui.set_linux_integration_status(status.into());
    ui.set_linux_integration_path(
        application_path
            .map(|path| path.display().to_string())
            .unwrap_or_default()
            .into(),
    );
    ui.set_open_linux_integration_folder_enabled(open_folder_enabled);
    ui.set_linux_integration_action_text(action.into());
    ui.set_linux_integration_action_enabled(visible);
}

fn appimage_running_game_message(mode: i32) -> &'static str {
    if mode == 2 {
        "Stop Dungeon Rampage Haxe before uninstalling DRH Launcher."
    } else {
        "Stop Dungeon Rampage Haxe before installing or repairing DRH Launcher."
    }
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
        Ok(release) => install_platform_release(
            ui,
            install_dir,
            config,
            release_source,
            InstallPlatformReleaseRequest {
                latest_release: Some(latest_release),
                release,
                repair_current,
                blocked_update_version: None,
            },
        ),
        Err(error) => error,
    }
}

struct InstallPlatformReleaseRequest {
    latest_release: Option<Arc<Mutex<Option<PlatformRelease>>>>,
    release: PlatformRelease,
    repair_current: bool,
    blocked_update_version: Option<String>,
}

fn install_platform_release(
    ui: &slint::Weak<AppWindow>,
    install_dir: &Path,
    config: &LauncherConfig,
    release_source: &ReleaseSource,
    request: InstallPlatformReleaseRequest,
) -> String {
    let InstallPlatformReleaseRequest {
        latest_release,
        release,
        repair_current,
        blocked_update_version,
    } = request;

    if repair_current
        && rollback_blocked_update_version(config).as_deref() == Some(release.version.as_str())
    {
        return format!(
            "Could not repair from cached archive, and update {} is skipped until a newer release is available.",
            release.version
        );
    }

    if let Some(latest_release) = latest_release {
        latest_release
            .lock()
            .expect("latest release lock poisoned")
            .replace(release.clone());
    }
    let _ = diagnostics::write(
        install_dir,
        diagnostics::LogLevel::Info,
        &format!("Found release {}. Starting install.", release.version),
    );
    report_background_activity(ui, format!("Preparing download for {}...", release.version));
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
            if !reported_verification && progress.total > 0 && progress.downloaded == progress.total
            {
                reported_verification = true;
                let message = "Verifying download...".to_string();
                let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
                report_background_activity(ui, message);
            }
        },
        |cache_error| {
            let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Warn, &cache_error);
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
            let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &verified_message);
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
                    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
                    report_background_activity(ui, message);
                    let install_result = if repair_current {
                        repair_release_from_extracted_archive(
                            &extracted,
                            install_dir,
                            &release,
                            release_source,
                        )
                    } else {
                        install_extracted_archive_with_blocked_update(
                            &extracted,
                            install_dir,
                            &release,
                            release_source,
                            blocked_update_version,
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
            if ui.get_install_action_text() != InstallState::Playing.primary_action() {
                apply_home_view_state(&ui, state);
            }
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
        return (Vec::new(), config.game_args.join(" "));
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

fn start_version_history_refresh(
    ui: &AppWindow,
    config: LauncherConfig,
    release_source: ReleaseSource,
    drh_version_history: Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
    launcher_version_history: Arc<Mutex<Vec<RepositoryRelease>>>,
    pending_drh_version_install: Arc<Mutex<Option<String>>>,
) {
    log_for_config(
        &config,
        diagnostics::LogLevel::Info,
        "Checking release history.",
    );
    ui.set_refresh_version_history_enabled(false);
    ui.set_version_history_loading(true);
    ui.set_version_history_status("".into());
    ui.set_selected_version_action_enabled(false);

    let ui = ui.as_weak();
    thread::spawn(move || {
        let platform = Platform::current();
        let drh_result = discover_platform_release_history(&release_source, platform);
        let launcher_source = ReleaseSource::launcher();
        let launcher_result = discover_repository_release_history(&launcher_source);

        let log_status = match (&drh_result, &launcher_result) {
            (Ok(drh), Ok(launcher)) => {
                format!(
                    "Loaded {} DRH release(s) and {} DRH Launcher release(s).",
                    drh.len(),
                    launcher.len()
                )
            }
            (Err(drh_error), Ok(launcher)) => {
                format!(
                    "Loaded {} DRH Launcher release(s), but DRH history failed: {drh_error}",
                    launcher.len()
                )
            }
            (Ok(drh), Err(launcher_error)) => {
                format!(
                    "Loaded {} DRH release(s), but DRH Launcher history failed: {launcher_error}",
                    drh.len()
                )
            }
            (Err(drh_error), Err(launcher_error)) => {
                format!(
                    "Could not load release history. DRH: {drh_error}. DRH Launcher: {launcher_error}"
                )
            }
        };
        let status = match (&drh_result, &launcher_result) {
            (Ok(_), Ok(_)) => String::new(),
            (Err(drh_error), Ok(_)) => {
                format!("Could not load DRH releases: {drh_error}")
            }
            (Ok(_), Err(launcher_error)) => {
                format!("Could not load DRH Launcher releases: {launcher_error}")
            }
            (Err(drh_error), Err(launcher_error)) => {
                format!(
                    "Could not load release history. DRH: {drh_error}. DRH Launcher: {launcher_error}"
                )
            }
        };
        let level = if drh_result.is_ok() || launcher_result.is_ok() {
            diagnostics::LogLevel::Info
        } else {
            diagnostics::LogLevel::Error
        };
        log_for_config(&config, level, &log_status);

        let drh_entries = drh_result.ok();
        let launcher_entries = launcher_result.ok();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if let Some(entries) = drh_entries {
                drh_version_history
                    .lock()
                    .expect("DRH version history lock poisoned")
                    .clone_from(&entries);
            }
            if let Some(entries) = launcher_entries {
                launcher_version_history
                    .lock()
                    .expect("launcher version history lock poisoned")
                    .clone_from(&entries);
            }
            pending_drh_version_install
                .lock()
                .expect("pending DRH version install lock poisoned")
                .take();
            ui.set_version_history_status(status.into());
            ui.set_version_history_loading(false);
            ui.set_refresh_version_history_enabled(true);
            refresh_version_history_selection(
                &ui,
                &config,
                &drh_version_history,
                &launcher_version_history,
                &pending_drh_version_install,
            );
        });
    });
}

fn refresh_version_history_selection(
    ui: &AppWindow,
    config: &LauncherConfig,
    drh_version_history: &Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
    launcher_version_history: &Arc<Mutex<Vec<RepositoryRelease>>>,
    pending_drh_version_install: &Arc<Mutex<Option<String>>>,
) {
    let drh_entries = drh_version_history
        .lock()
        .expect("DRH version history lock poisoned")
        .clone();
    let launcher_entries = launcher_version_history
        .lock()
        .expect("launcher version history lock poisoned")
        .clone();
    let pending_version = pending_drh_version_install
        .lock()
        .expect("pending DRH version install lock poisoned")
        .clone();

    let drh_views = drh_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| drh_version_entry_view(config, entry, index))
        .collect::<Vec<_>>();
    ui.set_drh_version_list_label(release_list_label("DRH releases", drh_views.len()).into());
    ui.set_drh_version_entries(ModelRc::new(VecModel::from(drh_views)));

    let launcher_views = launcher_entries
        .iter()
        .enumerate()
        .map(|(index, release)| launcher_version_entry_view(release, index))
        .collect::<Vec<_>>();
    ui.set_launcher_version_list_label(
        release_list_label("DRH Launcher releases", launcher_views.len()).into(),
    );
    ui.set_launcher_version_entries(ModelRc::new(VecModel::from(launcher_views)));

    apply_selected_version_view(
        ui,
        config,
        &drh_entries,
        &launcher_entries,
        pending_version.as_deref(),
    );
}

fn refresh_selected_version_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    drh_version_history: &Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
    launcher_version_history: &Arc<Mutex<Vec<RepositoryRelease>>>,
    pending_drh_version_install: &Arc<Mutex<Option<String>>>,
) {
    let drh_entries = drh_version_history
        .lock()
        .expect("DRH version history lock poisoned")
        .clone();
    let launcher_entries = launcher_version_history
        .lock()
        .expect("launcher version history lock poisoned")
        .clone();
    let pending_version = pending_drh_version_install
        .lock()
        .expect("pending DRH version install lock poisoned")
        .clone();

    apply_selected_version_view(
        ui,
        config,
        &drh_entries,
        &launcher_entries,
        pending_version.as_deref(),
    );
}

fn apply_selected_version_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    drh_entries: &[PlatformReleaseHistoryEntry],
    launcher_entries: &[RepositoryRelease],
    pending_version: Option<&str>,
) {
    if ui.get_version_history_page() == 0 {
        let Some(index) =
            normalize_selected_index(ui.get_selected_drh_version_index(), drh_entries.len())
        else {
            ui.set_selected_drh_version_index(-1);
            clear_selected_version_view(ui);
            return;
        };
        ui.set_selected_drh_version_index(index as i32);
        apply_selected_drh_version_view(ui, config, &drh_entries[index], pending_version);
    } else {
        let Some(index) = normalize_selected_index(
            ui.get_selected_launcher_version_index(),
            launcher_entries.len(),
        ) else {
            ui.set_selected_launcher_version_index(-1);
            clear_selected_version_view(ui);
            return;
        };
        ui.set_selected_launcher_version_index(index as i32);
        apply_selected_launcher_version_view(ui, &launcher_entries[index], index);
    }
}

fn normalize_selected_index(current: i32, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if current < 0 || current as usize >= len {
        Some(0)
    } else {
        Some(current as usize)
    }
}

fn release_list_label(label: &str, count: usize) -> String {
    format!("{label} ({count})")
}

fn clear_selected_version_view(ui: &AppWindow) {
    ui.set_selected_version_title("No release selected".into());
    ui.set_selected_version_detail("".into());
    ui.set_selected_version_changelog_blocks(ModelRc::new(VecModel::from(markdown_blocks(
        "Select a release to read its changelog.",
    ))));
    ui.set_selected_version_release_enabled(false);
    ui.set_selected_version_action_text("Install selected version".into());
    ui.set_selected_version_action_enabled(false);
    ui.set_selected_version_replace_confirmation_visible(false);
}

fn drh_version_entry_view(
    config: &LauncherConfig,
    entry: &PlatformReleaseHistoryEntry,
    index: usize,
) -> VersionEntryView {
    VersionEntryView {
        title: release_title(&entry.release.version, &entry.release.name).into(),
        detail: drh_version_entry_detail(entry).into(),
        status: drh_version_status(config, entry, index).into(),
    }
}

fn launcher_version_entry_view(release: &RepositoryRelease, index: usize) -> VersionEntryView {
    VersionEntryView {
        title: release_title(&release.version, &release.name).into(),
        detail: repository_release_detail(release).into(),
        status: launcher_version_status(release, index).into(),
    }
}

fn apply_selected_drh_version_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    entry: &PlatformReleaseHistoryEntry,
    pending_version: Option<&str>,
) {
    let (action_text, action_enabled, confirmation_visible) =
        selected_drh_version_action(config, entry, pending_version, version_action_blocker(ui));
    ui.set_selected_version_title(
        release_title(&entry.release.version, &entry.release.name).into(),
    );
    ui.set_selected_version_detail(selected_drh_version_detail(entry).into());
    ui.set_selected_version_changelog_blocks(ModelRc::new(VecModel::from(markdown_blocks(
        &entry.release.body,
    ))));
    ui.set_selected_version_release_enabled(true);
    ui.set_selected_version_action_text(action_text.into());
    ui.set_selected_version_action_enabled(action_enabled);
    ui.set_selected_version_replace_confirmation_visible(confirmation_visible);
}

fn apply_selected_launcher_version_view(ui: &AppWindow, release: &RepositoryRelease, index: usize) {
    let mut detail = repository_release_detail(release);
    let status = launcher_version_status(release, index);
    if !status.is_empty() {
        detail = format!("{detail} · {status}");
    }

    ui.set_selected_version_title(release_title(&release.version, &release.name).into());
    ui.set_selected_version_detail(detail.into());
    ui.set_selected_version_changelog_blocks(ModelRc::new(VecModel::from(markdown_blocks(
        &release.body,
    ))));
    ui.set_selected_version_release_enabled(true);
    ui.set_selected_version_action_text("Use Home update banner".into());
    ui.set_selected_version_action_enabled(false);
    ui.set_selected_version_replace_confirmation_visible(false);
}

fn selected_drh_history_entry(
    ui: &AppWindow,
    drh_version_history: &Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
) -> Option<PlatformReleaseHistoryEntry> {
    let index = ui.get_selected_drh_version_index();
    if index < 0 {
        return None;
    }
    drh_version_history
        .lock()
        .expect("DRH version history lock poisoned")
        .get(index as usize)
        .cloned()
}

fn selected_history_blocked_update_version(
    latest_version: Option<&str>,
    selected_version: &str,
) -> Option<String> {
    latest_version
        .filter(|latest_version| latest_version.trim() != selected_version.trim())
        .map(ToOwned::to_owned)
}

fn latest_drh_release_version_for_update_block(
    latest_release: &Arc<Mutex<Option<PlatformRelease>>>,
    release_source: &ReleaseSource,
) -> Option<String> {
    let cached = latest_release
        .lock()
        .expect("latest release lock poisoned")
        .as_ref()
        .map(|release| release.version.clone());
    cached.or_else(|| {
        discover_latest_platform_release(release_source, Platform::current())
            .ok()
            .map(|release| release.version)
    })
}

fn selected_version_release_url(
    ui: &AppWindow,
    drh_version_history: &Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
    launcher_version_history: &Arc<Mutex<Vec<RepositoryRelease>>>,
) -> Option<String> {
    if ui.get_version_history_page() == 0 {
        selected_drh_history_entry(ui, drh_version_history).map(|entry| entry.release.html_url)
    } else {
        let index = ui.get_selected_launcher_version_index();
        if index < 0 {
            return None;
        }
        launcher_version_history
            .lock()
            .expect("launcher version history lock poisoned")
            .get(index as usize)
            .map(|release| release.html_url.clone())
    }
}

fn drh_version_entry_detail(entry: &PlatformReleaseHistoryEntry) -> String {
    let mut parts = vec![release_date(&entry.release)];
    if entry.release.prerelease {
        parts.push("pre-release".to_string());
    }
    match &entry.platform_release {
        Some(release) => {
            parts.push(format_bytes(release.asset.size));
            parts.push(release.metadata_source.label().to_string());
        }
        None if entry.manifest_available => {
            parts.push("manifest metadata".to_string());
        }
        None => parts.push(format!("no {} package", Platform::current().id())),
    }
    parts.join(" · ")
}

fn selected_drh_version_detail(entry: &PlatformReleaseHistoryEntry) -> String {
    let mut parts = vec![format!("Published: {}", release_date(&entry.release))];
    if entry.release.prerelease {
        parts.push("Pre-release".to_string());
    }
    match &entry.platform_release {
        Some(release) => {
            parts.push(format!("Asset: {}", release.asset.name));
            parts.push(format!("Size: {}", format_bytes(release.asset.size)));
            parts.push(format!("Metadata: {}", release.metadata_source.label()));
        }
        None if entry.manifest_available => {
            parts.push(
                "Package details will be resolved from the release manifest before install."
                    .to_string(),
            );
        }
        None => {
            parts.push(format!(
                "Unavailable on {}: {}",
                Platform::current().id(),
                entry
                    .unsupported_reason
                    .as_deref()
                    .unwrap_or("no compatible package was found")
            ));
        }
    }
    parts.join("\n")
}

fn repository_release_detail(release: &RepositoryRelease) -> String {
    let mut parts = vec![release_date(release)];
    if release.prerelease {
        parts.push("pre-release".to_string());
    }
    parts.join(" · ")
}

fn drh_version_status(
    config: &LauncherConfig,
    entry: &PlatformReleaseHistoryEntry,
    index: usize,
) -> String {
    if installed_active_release_version(config)
        .as_deref()
        .is_some_and(|version| version.trim() == entry.release.version.trim())
        || restore_previous_release_version(config)
            .as_deref()
            .is_some_and(|version| version.trim() == entry.release.version.trim())
    {
        "Installed".to_string()
    } else if entry.platform_release.is_none() && !entry.manifest_available {
        "Unavailable".to_string()
    } else if entry.platform_release.is_none() && entry.manifest_available {
        "Manifest".to_string()
    } else if index == 0 {
        "Latest".to_string()
    } else if entry.release.prerelease {
        "Pre-release".to_string()
    } else {
        "Older".to_string()
    }
}

fn launcher_version_status(release: &RepositoryRelease, index: usize) -> String {
    if normalized_version_tag(&release.version) == normalized_version_tag(env!("CARGO_PKG_VERSION"))
    {
        "Current".to_string()
    } else if index == 0 {
        "Latest".to_string()
    } else if release.prerelease {
        "Pre-release".to_string()
    } else {
        "Older".to_string()
    }
}

fn selected_drh_version_action(
    config: &LauncherConfig,
    entry: &PlatformReleaseHistoryEntry,
    pending_version: Option<&str>,
    blocker: Option<&str>,
) -> (String, bool, bool) {
    if entry.platform_release.is_none() && !entry.manifest_available {
        return ("Unavailable".to_string(), false, false);
    }

    let version = entry.release.version.as_str();
    match installed_active_release_version(config) {
        Some(installed) if installed.trim() == version.trim() => {
            ("Installed".to_string(), false, false)
        }
        _ if restore_previous_release_version(config)
            .as_deref()
            .is_some_and(|previous| previous.trim() == version.trim()) =>
        {
            ("Use Restore".to_string(), false, false)
        }
        _ if let Some(blocker) = blocker => (blocker.to_string(), false, false),
        Some(_) if pending_version == Some(version) => (format!("Install {version}"), true, true),
        Some(_) => (format!("Install {version}"), true, false),
        None => (format!("Install {version}"), true, false),
    }
}

fn version_action_blocker(ui: &AppWindow) -> Option<&'static str> {
    let install_action = ui.get_install_action_text();
    if install_action == InstallState::Updating.primary_action() {
        Some("Installing...")
    } else if install_action == InstallState::Playing.primary_action() {
        Some("Stop DRH first")
    } else {
        None
    }
}

fn release_title(version: &str, name: &str) -> String {
    let name = name.trim();
    if name.is_empty() || name == version || name.contains(version) {
        if name.is_empty() {
            version.to_string()
        } else {
            name.to_string()
        }
    } else {
        format!("{version} - {name}")
    }
}

fn release_date(release: &RepositoryRelease) -> String {
    release
        .published_at
        .as_deref()
        .filter(|published_at| published_at.len() >= 10)
        .map(|published_at| published_at[..10].to_string())
        .or_else(|| release.published_at.clone())
        .unwrap_or_else(|| "unknown date".to_string())
}

fn markdown_blocks(body: &str) -> Vec<MarkdownBlockView> {
    let body = body.trim().replace("\r\n", "\n");
    if body.is_empty() {
        return vec![markdown_block(
            "No changelog was provided for this release.",
            MarkdownBlockKind::Paragraph,
        )];
    }

    let mut renderer = MarkdownRenderer::default();
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    for event in Parser::new_ext(&body, options) {
        renderer.push_event(event);
    }
    renderer.finish()
}

fn normalized_version_tag(version: &str) -> &str {
    version.trim().trim_start_matches('v')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkdownBlockKind {
    Paragraph = 0,
    Heading1 = 1,
    Heading2 = 2,
    Heading3 = 3,
    Bullet = 4,
    Numbered = 5,
    Code = 6,
    Quote = 7,
    Rule = 8,
    Image = 9,
}

impl MarkdownBlockKind {
    fn from_heading(level: HeadingLevel) -> Self {
        match level {
            HeadingLevel::H1 => Self::Heading1,
            HeadingLevel::H2 => Self::Heading2,
            HeadingLevel::H3 | HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => {
                Self::Heading3
            }
        }
    }
}

#[derive(Default)]
struct MarkdownRenderer {
    blocks: Vec<MarkdownBlockView>,
    current: Option<MarkdownBlockBuilder>,
    list_stack: Vec<Option<u64>>,
    capture_stack: Vec<MarkdownCapture>,
    link_stack: Vec<String>,
    quote_depth: usize,
}

impl MarkdownRenderer {
    fn push_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => self.push_inline_code(&code),
            Event::InlineMath(math) => self.push_text(&format!("${math}$")),
            Event::DisplayMath(math) => self.push_text(&format!("$$\n{math}\n$$")),
            Event::Html(html) | Event::InlineHtml(html) => self.push_text(&html),
            Event::FootnoteReference(reference) => self.push_text(&format!("[^{reference}]")),
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_text("\n"),
            Event::Rule => {
                self.flush_current();
                self.blocks
                    .push(markdown_block("", MarkdownBlockKind::Rule));
            }
            Event::TaskListMarker(checked) => {
                self.push_text(if checked { "[x] " } else { "[ ] " });
            }
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.current.is_none() {
                    self.start_block(if self.quote_depth > 0 {
                        MarkdownBlockKind::Quote
                    } else {
                        MarkdownBlockKind::Paragraph
                    });
                }
            }
            Tag::Heading { level, .. } => self.start_block(MarkdownBlockKind::from_heading(level)),
            Tag::BlockQuote(_) => {
                self.flush_current();
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.start_block(MarkdownBlockKind::Code);
                if let CodeBlockKind::Fenced(language) = kind {
                    let language = language.trim();
                    if !language.is_empty() {
                        self.push_raw_text(language);
                        self.push_raw_text("\n");
                    }
                }
            }
            Tag::List(start) => {
                self.flush_current();
                self.list_stack.push(start);
            }
            Tag::Item => {
                self.flush_current();
                let depth = self.list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                if let Some(Some(number)) = self.list_stack.last_mut() {
                    let current = *number;
                    *number += 1;
                    self.start_block(MarkdownBlockKind::Numbered);
                    self.push_raw_text(&format!("{indent}{current}. "));
                } else {
                    self.start_block(MarkdownBlockKind::Bullet);
                    self.push_raw_text(&format!("{indent}• "));
                }
            }
            Tag::Link { dest_url, .. } => {
                self.push_raw_text("[");
                self.link_stack.push(dest_url.to_string());
            }
            Tag::Image { dest_url, .. } => {
                self.capture_stack
                    .push(MarkdownCapture::new(MarkdownCaptureKind::Image, &dest_url));
            }
            Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::FootnoteDefinition(_)
            | Tag::MetadataBlock(_) => {
                if self.current.is_none() {
                    self.start_block(MarkdownBlockKind::Paragraph);
                }
            }
            Tag::Emphasis => self.push_raw_text("*"),
            Tag::Strong => self.push_raw_text("**"),
            Tag::Strikethrough => self.push_raw_text("~~"),
            Tag::Superscript | Tag::Subscript => {}
            Tag::HtmlBlock => self.start_block(MarkdownBlockKind::Code),
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item => {
                self.flush_current();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.flush_current();
                self.list_stack.pop();
            }
            TagEnd::Link => {
                let url = self.link_stack.pop().unwrap_or_default();
                self.push_raw_text(&format!("]({})", escape_markdown_link_url(&url)));
            }
            TagEnd::Image => self.finish_capture(MarkdownCaptureKind::Image),
            TagEnd::TableRow => {
                self.push_text("\n");
            }
            TagEnd::TableCell => {
                self.push_text("  ");
            }
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::MetadataBlock(_) => {}
            TagEnd::Emphasis => self.push_raw_text("*"),
            TagEnd::Strong => self.push_raw_text("**"),
            TagEnd::Strikethrough => self.push_raw_text("~~"),
        }
    }

    fn start_block(&mut self, kind: MarkdownBlockKind) {
        self.flush_current();
        self.current = Some(MarkdownBlockBuilder {
            text: String::new(),
            kind,
        });
    }

    fn push_text(&mut self, text: &str) {
        if let Some(capture) = self.capture_stack.last_mut() {
            capture.text.push_str(text);
            if capture.kind == MarkdownCaptureKind::Image {
                return;
            }
        }

        let text = if self
            .current
            .as_ref()
            .is_some_and(|current| current.kind == MarkdownBlockKind::Code)
        {
            text.to_string()
        } else {
            escape_markdown_text(text)
        };
        self.push_raw_text(&text);
    }

    fn push_raw_text(&mut self, text: &str) {
        if self.current.is_none() {
            self.start_block(if self.quote_depth > 0 {
                MarkdownBlockKind::Quote
            } else {
                MarkdownBlockKind::Paragraph
            });
        }
        if let Some(current) = &mut self.current {
            current.text.push_str(text);
        }
    }

    fn push_inline_code(&mut self, code: &str) {
        if let Some(capture) = self.capture_stack.last_mut() {
            capture.text.push_str(code);
            if capture.kind == MarkdownCaptureKind::Image {
                return;
            }
        }

        if !code.is_empty() {
            self.push_raw_text("`");
            self.push_raw_text(&escape_markdown_code_span(code));
            self.push_raw_text("`");
        }
    }

    fn finish_capture(&mut self, kind: MarkdownCaptureKind) {
        let Some(capture) = self.capture_stack.pop() else {
            return;
        };
        if capture.kind != kind {
            return;
        }

        let label = capture.text.trim();
        let target = capture.target.trim();
        if target.is_empty() {
            return;
        }

        if kind == MarkdownCaptureKind::Image {
            self.flush_current();
            let label = if label.is_empty() { "Image" } else { label };
            self.blocks.push(markdown_target_block(
                &format!("Image: {label}"),
                target,
                MarkdownBlockKind::Image,
            ));
        }
    }

    fn flush_current(&mut self) {
        if let Some(current) = self.current.take() {
            let text = current.text.trim();
            if !text.is_empty() {
                self.blocks.push(markdown_block(text, current.kind));
            }
        }
    }

    fn finish(mut self) -> Vec<MarkdownBlockView> {
        self.flush_current();
        if self.blocks.is_empty() {
            self.blocks.push(markdown_block(
                "No changelog was provided for this release.",
                MarkdownBlockKind::Paragraph,
            ));
        }
        self.blocks
    }
}

struct MarkdownBlockBuilder {
    text: String,
    kind: MarkdownBlockKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkdownCaptureKind {
    Image,
}

struct MarkdownCapture {
    kind: MarkdownCaptureKind,
    target: String,
    text: String,
}

impl MarkdownCapture {
    fn new(kind: MarkdownCaptureKind, target: &str) -> Self {
        Self {
            kind,
            target: target.to_string(),
            text: String::new(),
        }
    }
}

fn markdown_block(text: &str, kind: MarkdownBlockKind) -> MarkdownBlockView {
    markdown_target_block(text, "", kind)
}

fn markdown_target_block(text: &str, target: &str, kind: MarkdownBlockKind) -> MarkdownBlockView {
    MarkdownBlockView {
        text: markdown_block_text(text, kind),
        kind: kind as i32,
        target: target.into(),
    }
}

fn markdown_block_text(text: &str, kind: MarkdownBlockKind) -> StyledText {
    if kind == MarkdownBlockKind::Code || kind == MarkdownBlockKind::Image {
        return string_to_styled_text(text.to_string());
    }

    StyledText::parse_interpolated(text, &[] as &[StyledText])
        .unwrap_or_else(|_| string_to_styled_text(unescape_markdown_text(text)))
}

fn escape_markdown_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '<'
                | '>'
                | '~'
                | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn escape_markdown_code_span(text: &str) -> String {
    text.replace('`', "'")
}

fn escape_markdown_link_url(url: &str) -> String {
    url.replace(')', "%29")
}

fn unescape_markdown_text(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            plain.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            plain.push(character);
        }
    }
    if escaped {
        plain.push('\\');
    }
    plain
}

fn refresh_playing_state(ui: &AppWindow, config: &LauncherConfig, message: &str) {
    let status = inspect_install(config.install_dir.as_deref());

    ui.set_install_status("DRH is running".into());
    ui.set_install_action_text(InstallState::Playing.primary_action().into());
    ui.set_install_action_enabled(true);
    ui.set_version_status(status.version_text().into());
    ui.set_update_check_text("Check for updates".into());
    ui.set_update_check_enabled(false);
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
            if ui.get_close_confirmation_visible() {
                let _ = ui.window().hide();
                return;
            }
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

fn begin_game_stop(
    timer: Rc<Timer>,
    ui: slint::Weak<AppWindow>,
    config: LauncherConfig,
    game_process: Rc<RefCell<Option<game_launch::RunningGame>>>,
    game_monitor: Rc<Timer>,
    quit_when_finished: bool,
) {
    const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(3);

    let graceful_error = {
        let process = game_process.borrow();
        process
            .as_ref()
            .map(|game| game_launch::request_graceful_shutdown(&game.child))
            .transpose()
            .err()
    };
    let started_at = Instant::now();
    let mut forced = false;
    let mut forced_reason =
        graceful_error.map(|error| format!("graceful shutdown request failed: {error}"));
    let timer_handle = Rc::clone(&timer);

    timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
        let mut failure = None;
        let finished = {
            let mut process = game_process.borrow_mut();
            let Some(game) = process.as_mut() else {
                timer_handle.stop();
                finish_game_stop(
                    &ui,
                    &config,
                    &game_process,
                    &game_monitor,
                    quit_when_finished,
                    Ok("DRH is not running.".to_string()),
                );
                return;
            };

            match game.child.try_wait() {
                Ok(Some(status)) => {
                    let game = process.take().expect("running game disappeared");
                    Some((game, status))
                }
                Ok(None) => {
                    if !forced
                        && forced_reason.is_none()
                        && started_at.elapsed() >= GRACEFUL_STOP_TIMEOUT
                    {
                        forced_reason = Some("graceful shutdown timed out".to_string());
                    }
                    if !forced && forced_reason.is_some() {
                        match game.child.kill() {
                            Ok(()) => forced = true,
                            Err(error) => {
                                failure = Some(format!("Could not stop DRH: {error}"));
                            }
                        }
                    }
                    None
                }
                Err(error) => {
                    failure = Some(format!("Could not inspect DRH process: {error}"));
                    None
                }
            }
        };

        if let Some(error) = failure {
            timer_handle.stop();
            finish_game_stop(
                &ui,
                &config,
                &game_process,
                &game_monitor,
                quit_when_finished,
                Err(error),
            );
            return;
        }

        let Some((game, status)) = finished else {
            return;
        };

        timer_handle.stop();
        let stop_kind = if forced {
            format!(
                "Force-stopped after {}",
                forced_reason
                    .as_deref()
                    .unwrap_or("graceful shutdown failed")
            )
        } else {
            "Exited after graceful shutdown request".to_string()
        };
        let result = format!("{stop_kind} with status: {status}");
        let finish_result = game_logs::finish(&game.session_log, &result)
            .map(|_| "DRH stopped.".to_string())
            .map_err(|error| {
                format!("DRH stopped, but its session log could not be finished: {error}")
            });
        finish_game_stop(
            &ui,
            &config,
            &game_process,
            &game_monitor,
            quit_when_finished,
            finish_result,
        );
    });
}

fn finish_game_stop(
    ui: &slint::Weak<AppWindow>,
    config: &LauncherConfig,
    game_process: &Rc<RefCell<Option<game_launch::RunningGame>>>,
    game_monitor: &Rc<Timer>,
    quit_when_finished: bool,
    result: Result<String, String>,
) {
    let Some(ui) = ui.upgrade() else {
        return;
    };

    match result {
        Ok(message) => {
            log_for_config(config, diagnostics::LogLevel::Info, &message);
            if quit_when_finished {
                let _ = ui.window().hide();
            } else {
                refresh_home_state(&ui, config, &message);
                refresh_logs_view(&ui, config);
            }
        }
        Err(error) => {
            log_for_config(config, diagnostics::LogLevel::Error, &error);
            if process_is_running(game_process) {
                refresh_playing_state(&ui, config, &error);
                start_game_monitor(
                    Rc::clone(game_monitor),
                    ui.as_weak(),
                    config.clone(),
                    Rc::clone(game_process),
                );
            } else {
                refresh_home_state(&ui, config, &error);
            }
            ui.set_close_confirmation_busy(false);
            ui.set_close_confirmation_error(error.into());
        }
    }
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
    fn appimage_actions_are_blocked_while_game_runs() {
        assert_eq!(
            appimage_running_game_message(1),
            "Stop Dungeon Rampage Haxe before installing or repairing DRH Launcher."
        );
        assert_eq!(
            appimage_running_game_message(2),
            "Stop Dungeon Rampage Haxe before uninstalling DRH Launcher."
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
    fn renders_markdown_changelog_as_display_blocks() {
        let blocks = markdown_blocks(
            r#"# V2

Changes:

- Added **feature**
- Fixed [bug](https://example.test/bug) with `--safe-mode`

![Preview](https://example.test/preview.png)

```text
code sample
```
"#,
        );

        assert_eq!(blocks[0].kind, MarkdownBlockKind::Heading1 as i32);
        assert_eq!(blocks[1].kind, MarkdownBlockKind::Paragraph as i32);
        assert_eq!(blocks[2].kind, MarkdownBlockKind::Bullet as i32);
        assert_eq!(blocks[3].kind, MarkdownBlockKind::Bullet as i32);
        assert!(format!("{:?}", blocks[3].text).contains("Fixed bug with --safe-mode"));
        assert!(format!("{:?}", blocks[3].text).contains("https://example.test/bug"));
        assert_eq!(blocks[4].target, "https://example.test/preview.png");
        assert_eq!(blocks[4].kind, MarkdownBlockKind::Image as i32);
        assert!(format!("{:?}", blocks[5].text).contains("text\\ncode sample"));
        assert_eq!(blocks[5].kind, MarkdownBlockKind::Code as i32);
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
    fn historical_install_blocks_latest_loaded_release() {
        assert_eq!(
            selected_history_blocked_update_version(Some("V10"), "V7"),
            Some("V10".to_string())
        );
        assert_eq!(
            selected_history_blocked_update_version(Some("V10"), "V10"),
            None
        );
        assert_eq!(selected_history_blocked_update_version(None, "V7"), None);
    }

    #[test]
    fn marks_active_and_previous_history_versions_as_installed() {
        let temp = tempdir().unwrap();
        InstalledState {
            active: test_installed_release("V7"),
            previous: Some(test_installed_release("V10")),
            blocked_update_version: Some("V10".to_string()),
        }
        .save(temp.path())
        .unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };
        let active_entry = test_history_entry("V7");
        let previous_entry = test_history_entry("V10");

        assert_eq!(drh_version_status(&config, &active_entry, 1), "Installed");
        assert_eq!(drh_version_status(&config, &previous_entry, 0), "Installed");
        assert_eq!(
            selected_drh_version_action(&config, &active_entry, None, None),
            ("Installed".to_string(), false, false)
        );
        assert_eq!(
            selected_drh_version_action(&config, &previous_entry, None, None),
            ("Use Restore".to_string(), false, false)
        );
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

    fn test_history_entry(version: &str) -> PlatformReleaseHistoryEntry {
        PlatformReleaseHistoryEntry {
            release: RepositoryRelease {
                version: version.to_string(),
                name: format!("Dungeon Rampage Haxe {version}"),
                html_url: format!("https://example.test/{version}"),
                body: format!("Changelog for {version}"),
                published_at: Some("2026-01-01T00:00:00Z".to_string()),
                prerelease: false,
            },
            platform_release: Some(test_release(version)),
            manifest_available: false,
            unsupported_reason: None,
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
