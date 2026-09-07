// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod archive;
mod atomic_file;
mod bytes;
mod changelog_markdown;
mod config;
mod diagnostics;
mod download;
mod game_install;
mod game_launch;
mod game_logs;
mod game_runtime;
mod github_releases;
mod home_notices;
mod home_view;
mod install_metadata;
mod install_state;
mod installer;
mod launch_options;
mod launcher_update;
mod linux_appimage;
mod log_view;
mod paths;
mod platform;
mod play_mode;
mod release_manifest;
mod release_source;
mod steam_buildid;
mod steam_players;
mod steam_status;
mod version_history;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use archive::extract_to_staging;
use config::{FrameRateMode, LaunchArgumentsMode, LauncherConfig};
use download::{
    DownloadProgress, VerifiedDownload, download_and_verify_with_progress, prune_download_cache,
    update_download_cache, verify_cached_archive_by_metadata,
};
use game_install::inspect_install;
use game_runtime::{
    begin_game_stop, finalize_exited_game, process_is_running, refresh_playing_state,
    start_game_monitor,
};
use github_releases::{
    PlatformRelease, PlatformReleaseHistoryEntry, ReleaseMetadataSource, RepositoryRelease,
    discover_latest_platform_release, discover_latest_platform_release_for_install,
    discover_platform_release_by_tag_for_install, fetch_release_manifest,
};
use home_notices::apply_home_notices_view;
use home_view::{
    apply_home_view_state, apply_player_count_view, apply_updating_home_state,
    cached_latest_drh_release, home_view_state, home_view_state_from_cache,
    installed_active_release_version, refresh_home_state, remember_latest_drh_release,
    set_status_message,
};
use install_state::InstallState;
use installer::{
    install_extracted_archive_preserving_previous, install_extracted_archive_with_blocked_update,
    repair_active_install_from_extracted_archive, repair_release_from_extracted_archive,
    restore_previous_version,
};
use launch_options::{
    apply_frame_rate_mode_to_view, apply_frame_rate_preset_to_view,
    apply_launch_arguments_mode_to_view, apply_launch_options_to_view,
    frame_rate_preference_from_view, launch_options_from_model, launch_options_game_args,
    launch_options_saved_message, load_installed_launch_options, refresh_launch_options_view,
};
use log_view::{
    LogViewportPosition, extract_log_selection, refresh_log_content, refresh_logs_view,
    remember_game_log_position, saved_game_log_position,
};
use platform::Platform;
use release_source::ReleaseSource;
use slint::{CloseRequestResponse, Model, Timer, TimerMode};
use version_history::{
    latest_drh_release_version_for_update_block, preserve_previous_slot_on_install,
    refresh_selected_version_view, refresh_version_history_selection, selected_drh_history_entry,
    selected_history_blocked_update_version, selected_version_release_url,
    start_version_history_refresh,
};

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

    let startup_notice = match play_mode::startup_mode_from_args(std::env::args_os().skip(1)) {
        Ok(play_mode::StartupMode::Gui) => None,
        Ok(play_mode::StartupMode::Play) => match play_mode::run() {
            play_mode::Outcome::Exit(code) => std::process::exit(code),
            play_mode::Outcome::OpenGui(message) => Some(message),
        },
        Err(error) => {
            let message = format!("DRH Launcher failed to start: {error}");
            report_startup_failure(&message);
            std::process::exit(2);
        }
    };

    if let Err(error) = run(startup_notice) {
        let message = format!("DRH Launcher failed to start: {error}");
        if retry_with_software_renderer_if_needed(&message) {
            return;
        }
        report_startup_failure(&message);
        std::process::exit(1);
    }
}

fn run(startup_notice: Option<String>) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    slint::set_xdg_app_id(XDG_APP_ID)?;

    let (mut loaded_config, config_load_warning) = LauncherConfig::load_with_diagnostics();
    let install_dir_was_unset = loaded_config.install_dir.is_none();
    let install_dir = loaded_config.ensure_install_dir();
    let _ = std::fs::create_dir_all(paths::logs_dir(&install_dir));
    if install_dir_was_unset {
        let _ = loaded_config.save();
    }
    let config = Rc::new(RefCell::new(loaded_config));
    let latest_release = Arc::new(Mutex::new(None::<PlatformRelease>));
    let latest_launcher_update = Arc::new(Mutex::new(None::<launcher_update::LauncherUpdate>));
    let drh_version_history = Arc::new(Mutex::new(Vec::<PlatformReleaseHistoryEntry>::new()));
    let launcher_version_history = Arc::new(Mutex::new(Vec::<RepositoryRelease>::new()));
    let pending_drh_version_install = Arc::new(Mutex::new(None::<String>));
    let app_shutting_down = Arc::new(AtomicBool::new(false));
    let game_process = Rc::new(RefCell::new(None::<game_launch::RunningGame>));
    let game_monitor = Rc::new(Timer::default());
    let game_stop_timer = Rc::new(Timer::default());
    let game_log_positions = Rc::new(RefCell::new(HashMap::<String, LogViewportPosition>::new()));
    let steam_network_timer = Timer::default();
    let steam_local_timer = Timer::default();
    let last_players_text = Arc::new(Mutex::new(String::new()));
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

    steam_status::refresh_status();
    let initial_home_message = startup_notice
        .or_else(|| {
            config_load_warning
                .as_ref()
                .map(|warning| format!("Configuration warning: {warning}"))
        })
        .unwrap_or_else(|| format!("Ready. Release source: {}", release_source.label()));
    refresh_home_state(&ui, &config.borrow(), &initial_home_message);
    log_for_config(
        &config.borrow(),
        diagnostics::LogLevel::Info,
        &format!(
            "DRH Launcher started. Release source: {}",
            release_source.label()
        ),
    );
    if let Some(warning) = &config_load_warning {
        log_for_config(&config.borrow(), diagnostics::LogLevel::Warn, warning);
    }
    refresh_logs_view(&ui, &config.borrow());

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        let latest_release = Arc::clone(&latest_release);
        let game_process = Rc::clone(&game_process);
        let game_monitor = Rc::clone(&game_monitor);
        let game_stop_timer = Rc::clone(&game_stop_timer);
        let app_shutting_down = Arc::clone(&app_shutting_down);
        let release_source = release_source.clone();
        ui.unwrap().on_install_or_play(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if !ui.get_install_action_enabled() {
                return;
            }

            let mut config = config.borrow_mut();
            if process_is_running(&game_process) {
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
                return;
            }
            if let Some(message) = finalize_exited_game(&game_process, &game_monitor, &config) {
                refresh_home_state(&ui, &config, &message);
                refresh_logs_view(&ui, &config);
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
                match game_launch::launch_game_with_options(
                    &config,
                    installed_launch_options.as_ref(),
                    install_status.installed_version.as_deref(),
                ) {
                    Ok(game) => {
                        log_for_config(
                            &config,
                            diagnostics::LogLevel::Info,
                            &format!(
                                "DRH launched with command: {}. Session log: {}",
                                game_launch::launch_command_summary_with_options(
                                    &config,
                                    installed_launch_options.as_ref()
                                )
                                .unwrap_or_else(|error| format!(
                                    "command summary unavailable ({error})"
                                )),
                                game.session_log.path.display(),
                            ),
                        );
                        game_process.borrow_mut().replace(game);
                        refresh_playing_state(&ui, &config, "Running.");
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
            apply_updating_home_state(&ui, &config, &initial_message);
            let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Info, &initial_message);

            let ui = ui.as_weak();
            let config = config.clone();
            let latest_release = Arc::clone(&latest_release);
            let release_source = release_source.clone();
            let repair_from_cache = matches!(install_status.state, InstallState::BrokenInstall);
            let app_shutting_down = Arc::clone(&app_shutting_down);
            thread::spawn(move || {
                let message = if repair_from_cache {
                    repair_active_install(
                        &ui,
                        &install_dir,
                        &config,
                        &app_shutting_down,
                        &release_source,
                    )
                    .unwrap_or_else(|error| error)
                } else {
                    install_latest_release(
                        &ui,
                        &install_dir,
                        &config,
                        &app_shutting_down,
                        &release_source,
                        InstallLatestReleaseRequest {
                            latest_release: Arc::clone(&latest_release),
                            release,
                            repair_current: false,
                        },
                    )
                };

                log_install_failure(&install_dir, &message);

                let event_config = config.clone();
                invoke_on_event_loop(&config, &app_shutting_down, move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };

                    finish_install_operation_view(&ui, &event_config, &message);
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
        let game_monitor = Rc::clone(&game_monitor);
        let app_shutting_down = Arc::clone(&app_shutting_down);
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
            let _ = finalize_exited_game(&game_process, &game_monitor, &config.borrow());

            ui.set_appimage_dialog_busy(true);
            ui.set_appimage_dialog_error("".into());
            let ui = ui.as_weak();
            let config = config.borrow().clone();
            let app_shutting_down = Arc::clone(&app_shutting_down);
            thread::spawn(move || {
                let result = if mode == 2 {
                    linux_appimage::uninstall()
                } else {
                    linux_appimage::install_and_restart()
                };

                if let Err(error) = result {
                    log_for_config(&config, diagnostics::LogLevel::Error, &error);
                    invoke_on_event_loop(&config, &app_shutting_down, move || {
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
        let game_monitor = Rc::clone(&game_monitor);
        let latest_launcher_update = Arc::clone(&latest_launcher_update);
        let app_shutting_down = Arc::clone(&app_shutting_down);
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
            let _ = finalize_exited_game(&game_process, &game_monitor, &config.borrow());

            let update = latest_launcher_update
                .lock()
                .expect("launcher update lock poisoned")
                .clone();
            let Some(update) = update else {
                start_launcher_update_check(
                    &ui,
                    config.borrow().clone(),
                    Arc::clone(&app_shutting_down),
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
            let app_shutting_down = Arc::clone(&app_shutting_down);
            thread::spawn(move || {
                if let Err(error) = update.install_and_restart() {
                    log_for_config(&config, diagnostics::LogLevel::Error, &error);
                    invoke_on_event_loop(&config, &app_shutting_down, move || {
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
        let app_shutting_down = Arc::clone(&app_shutting_down);
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
                Arc::clone(&app_shutting_down),
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
        let release_source = release_source.clone();
        let app_shutting_down = Arc::clone(&app_shutting_down);
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
            apply_updating_home_state(&ui, &config, "Reinstalling current version...");

            let ui = ui.as_weak();
            let release_source = release_source.clone();
            let app_shutting_down = Arc::clone(&app_shutting_down);
            thread::spawn(move || {
                let message = repair_active_install(
                    &ui,
                    &install_dir,
                    &config,
                    &app_shutting_down,
                    &release_source,
                )
                .unwrap_or_else(|error| format!("Could not reinstall current version: {error}"));
                log_install_failure(&install_dir, &message);

                let event_config = config.clone();
                invoke_on_event_loop(&config, &app_shutting_down, move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };

                    finish_install_operation_view(&ui, &event_config, &message);
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
            config.hide_official_ownership_notice = ui.get_hide_official_ownership_notice();
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
                            apply_home_notices_view(&ui, &config);
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
                            apply_home_notices_view(&ui, &config);
                            set_status_message(
                                &ui,
                                &format!(
                                    "Download cache limit saved, but cache pruning failed: {error}"
                                ),
                            );
                        }
                        None => {
                            refresh_settings_view(&ui, &config, "Saved successfully");
                            apply_home_notices_view(&ui, &config);
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
        ui.unwrap().on_select_frame_rate_mode(move |mode| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let installed_launch_options = load_installed_launch_options(&config.borrow());
            apply_frame_rate_mode_to_view(
                &ui,
                installed_launch_options.as_ref(),
                FrameRateMode::from_ui_index(mode),
            );
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_select_frame_rate_preset(move |index| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let installed_launch_options = load_installed_launch_options(&config.borrow());
            apply_frame_rate_preset_to_view(
                &ui,
                installed_launch_options.as_ref(),
                index.round().max(0.0) as usize,
            );
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_save_launch_options(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let installed_launch_options = load_installed_launch_options(&config.borrow());
            let frame_rate =
                match frame_rate_preference_from_view(&ui, installed_launch_options.as_ref()) {
                    Ok(frame_rate) => frame_rate,
                    Err(error) => {
                        ui.set_game_frame_rate_error(error.clone().into());
                        set_status_message(&ui, &format!("Invalid frame rate: {error}"));
                        return;
                    }
                };

            let game_args = match launch_options_game_args(&ui, installed_launch_options.as_ref()) {
                Ok(args) => args,
                Err(error) => {
                    let config = config.borrow();
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

            let mut config = config.borrow_mut();
            let previous_config = config.clone();
            config.pre_launch_command = ui.get_pre_launch_command().trim().to_string();
            if let Some(frame_rate) = frame_rate {
                config.frame_rate = frame_rate;
            }
            config.launch_arguments_mode =
                LaunchArgumentsMode::from_ui_index(ui.get_launch_arguments_mode());
            config.game_args = game_args;
            match config.save() {
                Ok(()) => {
                    let message = launch_options_saved_message(
                        &previous_config,
                        &config,
                        installed_launch_options.as_ref(),
                    );
                    log_for_config(&config, diagnostics::LogLevel::Info, &message);
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

            let install_dir = config.borrow().effective_install_dir();

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

            let install_dir = config.borrow().effective_install_dir();

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
        let app_shutting_down = Arc::clone(&app_shutting_down);
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
                    Arc::clone(&app_shutting_down),
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
        let app_shutting_down = Arc::clone(&app_shutting_down);
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
                Arc::clone(&app_shutting_down),
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
        let app_shutting_down = Arc::clone(&app_shutting_down);
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
                    Arc::clone(&app_shutting_down),
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
        let app_shutting_down = Arc::clone(&app_shutting_down);
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
            let install_dir = config_snapshot.effective_install_dir();
            pending_drh_version_install
                .lock()
                .expect("pending DRH version install lock poisoned")
                .take();
            let drh_entries = drh_version_history
                .lock()
                .expect("DRH version history lock poisoned")
                .clone();
            let preserve_previous =
                preserve_previous_slot_on_install(&drh_entries, &config_snapshot, &version);
            let message = format!("Installing DRH {version}...");
            apply_updating_home_state(&ui, &config_snapshot, &message);
            ui.set_refresh_version_history_enabled(false);
            ui.set_selected_version_replace_confirmation_visible(false);
            refresh_selected_version_view(
                &ui,
                &config_snapshot,
                &drh_version_history,
                &launcher_version_history,
                &pending_drh_version_install,
            );

            let ui = ui.as_weak();
            let release_source = release_source.clone();
            let latest_release = Arc::clone(&latest_release);
            let drh_version_history = Arc::clone(&drh_version_history);
            let launcher_version_history = Arc::clone(&launcher_version_history);
            let pending_drh_version_install = Arc::clone(&pending_drh_version_install);
            let app_shutting_down = Arc::clone(&app_shutting_down);
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
                        &app_shutting_down,
                        &release_source,
                        InstallPlatformReleaseRequest {
                            latest_release: None,
                            release,
                            repair_current: false,
                            preserve_previous,
                            blocked_update_version: selected_history_blocked_update_version(
                                latest_version.as_deref(),
                                version.as_str(),
                            ),
                        },
                    ),
                    Err(error) => error,
                };
                log_install_failure(&install_dir, &message);

                let event_config = config_snapshot.clone();
                invoke_on_event_loop(&config_snapshot, &app_shutting_down, move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };

                    finish_install_operation_view(&ui, &event_config, &message);
                    ui.set_refresh_version_history_enabled(true);
                    set_status_message(&ui, &message);
                    refresh_version_history_selection(
                        &ui,
                        &event_config,
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
                    refresh_log_content(&ui, &config.borrow());
                }
            });
    }

    {
        let ui = ui.as_weak();
        ui.unwrap().on_copy_log_selection(
            move |anchor_line, anchor_col, cursor_line, cursor_col| {
                ui.upgrade().map_or_else(Default::default, |ui| {
                    extract_log_selection(
                        &ui.get_log_lines(),
                        anchor_line,
                        anchor_col,
                        cursor_line,
                        cursor_col,
                    )
                    .into()
                })
            },
        );
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_select_log_source(move |source| {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            ui.set_log_source(source);
            refresh_log_content(&ui, &config.borrow());
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap()
            .on_set_hide_haxe_iteratee_log_lines(move |hide| {
                let Some(ui) = ui.upgrade() else {
                    return;
                };

                let mut config = config.borrow_mut();
                config.hide_haxe_iteratee_log_lines = hide;
                if let Err(error) = config.save() {
                    let message =
                        format!("Could not save Haxe iteratee warning visibility: {error}");
                    log_for_config(&config, diagnostics::LogLevel::Error, &message);
                    set_status_message(&ui, &message);
                }
                refresh_log_content(&ui, &config);
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

            let sessions = ui.get_game_log_sessions();
            game_log_positions.borrow_mut().retain(|session_id, _| {
                (0..sessions.row_count()).any(|index| {
                    sessions
                        .row_data(index)
                        .is_some_and(|session| session.id == session_id.as_str())
                })
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
                .and_then(|target_index| sessions.row_data(target_index))
                .map(|session| session.id.to_string())
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
            refresh_log_content(&ui, &config.borrow());
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
            let session_id = ui.get_selected_game_log_id();
            let Some(session_path) = game_logs::path_for_session_id(&install_dir, &session_id)
            else {
                set_status_message(&ui, "No game session selected.");
                return;
            };

            let open_path = match game_logs::path_for_external_open(&session_path) {
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
        let config = Rc::clone(&config);
        let game_process = Rc::clone(&game_process);
        let game_monitor = Rc::clone(&game_monitor);
        let app_shutting_down = Arc::clone(&app_shutting_down);
        ui.unwrap().window().on_close_requested(move || {
            let Some(ui) = ui.upgrade() else {
                return CloseRequestResponse::HideWindow;
            };

            if ui.get_close_confirmation_visible() {
                return CloseRequestResponse::KeepWindowShown;
            }

            if process_is_running(&game_process) {
                ui.set_close_confirmation_error("".into());
                ui.set_close_confirmation_busy(false);
                ui.set_close_confirmation_visible(true);
                return CloseRequestResponse::KeepWindowShown;
            }

            let _ = finalize_exited_game(&game_process, &game_monitor, &config.borrow());
            app_shutting_down.store(true, Ordering::Relaxed);
            CloseRequestResponse::HideWindow
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
        let app_shutting_down = Arc::clone(&app_shutting_down);
        ui.unwrap().on_confirm_close(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if ui.get_close_confirmation_busy() {
                return;
            }

            ui.set_close_confirmation_busy(true);
            ui.set_close_confirmation_error("".into());
            app_shutting_down.store(true, Ordering::Relaxed);
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

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_select_home_notice(move |index| {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            home_notices::select_home_notice(index, &config.borrow(), &ui);
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_dismiss_home_notice(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            let mut config = config.borrow_mut();
            home_notices::dismiss_current_home_notice(&mut config, &ui);
            refresh_settings_view(&ui, &config, "Save");
        });
    }

    {
        let ui = ui.as_weak();
        let config = Rc::clone(&config);
        ui.unwrap().on_open_home_notice_link(move |url| {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            let opened =
                open_url(url.as_str()).or_else(|_| open_url(steam_buildid::STEAM_STORE_URL));
            match opened {
                Ok(()) => {
                    log_for_config(
                        &config.borrow(),
                        diagnostics::LogLevel::Info,
                        "Steam store opened.",
                    );
                    set_status_message(&ui, "Steam store opened.");
                }
                Err(error) => {
                    let message = format!("Could not open Steam store: {error}");
                    log_for_config(&config.borrow(), diagnostics::LogLevel::Error, &message);
                    set_status_message(&ui, &message);
                }
            }
        });
    }

    start_installed_manifest_refresh(
        ui.as_weak(),
        config.borrow().clone(),
        Arc::clone(&app_shutting_down),
        release_source.clone(),
    );
    start_release_check(
        &ui,
        config.borrow().clone(),
        Arc::clone(&app_shutting_down),
        Arc::clone(&latest_release),
        release_source.clone(),
    );
    start_launcher_update_check(
        &ui,
        config.borrow().clone(),
        Arc::clone(&app_shutting_down),
        Arc::clone(&latest_launcher_update),
    );
    start_official_buildid_check(
        ui.as_weak(),
        config.borrow().clone(),
        Arc::clone(&app_shutting_down),
    );
    start_player_count_check(
        ui.as_weak(),
        config.borrow().clone(),
        Arc::clone(&app_shutting_down),
        Arc::clone(&last_players_text),
    );
    refresh_steam_status_view(&ui, &config.borrow());
    start_steam_network_timer(
        ui.as_weak(),
        Rc::clone(&config),
        Arc::clone(&app_shutting_down),
        Arc::clone(&last_players_text),
        &steam_network_timer,
    );
    start_steam_local_timer(ui.as_weak(), Rc::clone(&config), &steam_local_timer);

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
    app_shutting_down: Arc<AtomicBool>,
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

        invoke_on_event_loop(&config, &app_shutting_down, move || {
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
    app_shutting_down: &AtomicBool,
    release_source: &ReleaseSource,
    request: InstallLatestReleaseRequest,
) -> String {
    let InstallLatestReleaseRequest {
        latest_release,
        release,
        repair_current,
    } = request;
    let release = match release {
        Some(release) if release.metadata_source == ReleaseMetadataSource::Manifest => Ok(release),
        _ => discover_latest_platform_release_for_install(release_source, Platform::current()),
    };

    match release {
        Ok(release) => install_platform_release(
            ui,
            install_dir,
            config,
            app_shutting_down,
            release_source,
            InstallPlatformReleaseRequest {
                latest_release: Some(latest_release),
                release,
                repair_current,
                preserve_previous: false,
                blocked_update_version: None,
            },
        ),
        Err(error) => error,
    }
}

struct InstallLatestReleaseRequest {
    latest_release: Arc<Mutex<Option<PlatformRelease>>>,
    release: Option<PlatformRelease>,
    repair_current: bool,
}

struct InstallPlatformReleaseRequest {
    latest_release: Option<Arc<Mutex<Option<PlatformRelease>>>>,
    release: PlatformRelease,
    repair_current: bool,
    preserve_previous: bool,
    blocked_update_version: Option<String>,
}

fn install_platform_release(
    ui: &slint::Weak<AppWindow>,
    install_dir: &Path,
    config: &LauncherConfig,
    app_shutting_down: &AtomicBool,
    release_source: &ReleaseSource,
    request: InstallPlatformReleaseRequest,
) -> String {
    let InstallPlatformReleaseRequest {
        latest_release,
        release,
        repair_current,
        preserve_previous,
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
    report_background_activity(
        ui,
        config,
        app_shutting_down,
        format!("Preparing download for {}...", release.version),
    );
    match download_release_with_progress(ui, install_dir, config, app_shutting_down, &release) {
        Ok(download) => {
            log_verified_download(install_dir, &download);
            let message = "Extracting archive...".to_string();
            let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
            report_background_activity(ui, config, app_shutting_down, message);
            match extract_to_staging(&download, install_dir) {
                Ok(extracted) => {
                    let _ = diagnostics::write(
                        install_dir,
                        diagnostics::LogLevel::Info,
                        "Archive extracted.",
                    );
                    let message = "Installing files...".to_string();
                    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
                    report_background_activity(ui, config, app_shutting_down, message);
                    let install_result = if repair_current {
                        repair_release_from_extracted_archive(
                            &extracted,
                            install_dir,
                            &release,
                            release_source,
                        )
                    } else if preserve_previous {
                        install_extracted_archive_preserving_previous(
                            &extracted,
                            install_dir,
                            &release,
                            release_source,
                            blocked_update_version,
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

fn repair_active_install(
    ui: &slint::Weak<AppWindow>,
    install_dir: &Path,
    config: &LauncherConfig,
    app_shutting_down: &AtomicBool,
    release_source: &ReleaseSource,
) -> Result<String, String> {
    let installed = install_metadata::InstalledState::load(install_dir)?;
    let active = installed.active;
    let message = format!("Checking cached archive for {}...", active.version);
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
    report_background_activity(ui, config, app_shutting_down, message);

    let (download, downloaded) = match verify_cached_archive_by_metadata(
        install_dir,
        &active.archive,
        active.archive_size,
        &active.archive_sha256,
    ) {
        Ok(download) => {
            let message = format!(
                "Reused cached archive for repair: {} ({} bytes, sha256:{})",
                download.path.display(),
                download.size,
                download.sha256
            );
            let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
            (download, false)
        }
        Err(cache_error) => {
            let message = format!(
                "Cached archive unavailable: {cache_error}. Downloading {}...",
                active.version
            );
            let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Warn, &message);
            report_background_activity(ui, config, app_shutting_down, message.clone());
            let release = discover_platform_release_by_tag_for_install(
                release_source,
                Platform::current(),
                &active.version,
            )?;
            let download = download_release_with_progress(
                ui,
                install_dir,
                config,
                app_shutting_down,
                &release,
            )?;
            log_verified_download(install_dir, &download);
            (download, true)
        }
    };

    let message = "Extracting archive...".to_string();
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
    report_background_activity(ui, config, app_shutting_down, message);
    let extracted = extract_to_staging(&download, install_dir)?;
    let _ = diagnostics::write(
        install_dir,
        diagnostics::LogLevel::Info,
        "Archive extracted.",
    );

    let message = "Repairing files...".to_string();
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
    report_background_activity(ui, config, app_shutting_down, message);
    let repaired = repair_active_install_from_extracted_archive(&extracted, install_dir)?;
    match update_download_cache(install_dir, &active.archive, config.download_cache_limit) {
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

    let message = if downloaded {
        format!("Reinstalled {}.", repaired.active.version)
    } else {
        format!("Repaired {} from cached archive.", repaired.active.version)
    };
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, &message);
    Ok(message)
}

fn download_release_with_progress(
    ui: &slint::Weak<AppWindow>,
    install_dir: &Path,
    config: &LauncherConfig,
    app_shutting_down: &AtomicBool,
    release: &PlatformRelease,
) -> Result<VerifiedDownload, String> {
    let mut last_percent = None::<u64>;
    let mut last_logged_percent = None::<u64>;
    let mut reported_verification = false;
    download_and_verify_with_progress(
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
                    config,
                    app_shutting_down,
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
                report_background_activity(ui, config, app_shutting_down, message);
            }
        },
        |cache_error| {
            let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Warn, &cache_error);
        },
    )
}

fn log_verified_download(install_dir: &Path, download: &VerifiedDownload) {
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
}

fn start_installed_manifest_refresh(
    ui: slint::Weak<AppWindow>,
    config: LauncherConfig,
    app_shutting_down: Arc<AtomicBool>,
    release_source: ReleaseSource,
) {
    let install_dir = config.effective_install_dir();
    let Ok(mut state) = install_metadata::InstalledState::load(&install_dir) else {
        return;
    };
    if state.active.apply_known_steam_buildid() {
        let version = state.active.version.clone();
        let buildid = state.active.steam_buildid;
        match state.save(&install_dir) {
            Ok(()) => log_for_config(
                &config,
                diagnostics::LogLevel::Info,
                &recorded_steam_buildid_message(&version, buildid),
            ),
            Err(error) => log_for_config(
                &config,
                diagnostics::LogLevel::Warn,
                &format!("Could not save Steam BuildID metadata: {error}"),
            ),
        }
    }
    if !state.active.needs_manifest_fetch() {
        return;
    }

    let version = state.active.version.clone();
    thread::spawn(move || {
        if app_shutting_down.load(Ordering::Relaxed) {
            return;
        }
        match fetch_release_manifest(&release_source, &version) {
            Ok(Some(manifest)) => {
                let Ok(mut state) = install_metadata::InstalledState::load(&install_dir) else {
                    return;
                };
                if state.active.version != version || !state.active.needs_manifest_fetch() {
                    return;
                }
                let filled_frame_rate = state.active.needs_frame_rate_fetch();
                if !state.active.apply_fetched_manifest(&manifest) {
                    return;
                }
                if let Err(error) = state.save(&install_dir) {
                    log_for_config(
                        &config,
                        diagnostics::LogLevel::Warn,
                        &format!("Could not save refreshed release metadata: {error}"),
                    );
                    return;
                }
                if let Some(buildid) = state.active.steam_buildid {
                    log_for_config(
                        &config,
                        diagnostics::LogLevel::Info,
                        &recorded_steam_buildid_message(&version, Some(buildid)),
                    );
                }
                let filled_frame_rate = filled_frame_rate && !state.active.needs_frame_rate_fetch();
                if filled_frame_rate {
                    log_for_config(
                        &config,
                        diagnostics::LogLevel::Info,
                        &format!("Loaded frame-rate options for installed {version}."),
                    );
                }
                let event_config = config.clone();
                invoke_on_event_loop(&config, &app_shutting_down, move || {
                    let Some(ui) = ui.upgrade() else {
                        return;
                    };
                    if filled_frame_rate {
                        let installed_launch_options = load_installed_launch_options(&event_config);
                        refresh_launch_options_view(
                            &ui,
                            &event_config,
                            installed_launch_options.as_ref(),
                            "Save",
                        );
                    }
                    apply_home_notices_view(&ui, &event_config);
                });
            }
            Ok(None) => log_for_config(
                &config,
                diagnostics::LogLevel::Warn,
                &format!("Release {version} has no manifest to refresh installed metadata."),
            ),
            Err(error) => log_for_config(
                &config,
                diagnostics::LogLevel::Warn,
                &format!("Could not refresh installed metadata for {version}: {error}"),
            ),
        }
    });
}

fn recorded_steam_buildid_message(version: &str, buildid: Option<u64>) -> String {
    match buildid {
        Some(buildid) => format!("Recorded Steam BuildID {buildid} for installed {version}."),
        None => format!("No Steam BuildID is known for installed {version}."),
    }
}

fn start_steam_local_timer(
    ui: slint::Weak<AppWindow>,
    config: Rc<RefCell<LauncherConfig>>,
    timer: &Timer,
) {
    timer.start(TimerMode::Repeated, Duration::from_secs(10), move || {
        let Some(ui) = ui.upgrade() else {
            return;
        };
        refresh_steam_status_view(&ui, &config.borrow());
    });
}

fn refresh_steam_status_view(ui: &AppWindow, config: &LauncherConfig) {
    steam_status::refresh_status();
    apply_home_notices_view(ui, config);
}

fn start_steam_network_timer(
    ui: slint::Weak<AppWindow>,
    config: Rc<RefCell<LauncherConfig>>,
    app_shutting_down: Arc<AtomicBool>,
    last_players_text: Arc<Mutex<String>>,
    timer: &Timer,
) {
    timer.start(TimerMode::Repeated, Duration::from_secs(300), move || {
        start_official_buildid_check(
            ui.clone(),
            config.borrow().clone(),
            Arc::clone(&app_shutting_down),
        );
        start_player_count_check(
            ui.clone(),
            config.borrow().clone(),
            Arc::clone(&app_shutting_down),
            Arc::clone(&last_players_text),
        );
    });
}

fn start_official_buildid_check(
    ui: slint::Weak<AppWindow>,
    config: LauncherConfig,
    app_shutting_down: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let buildid = match steam_buildid::fetch_official_public_buildid() {
            Ok(buildid) => buildid,
            Err(error) => {
                log_for_config(&config, diagnostics::LogLevel::Warn, &error);
                return;
            }
        };
        steam_buildid::cache_official_public_buildid(buildid);

        let event_config = config.clone();
        invoke_on_event_loop(&config, &app_shutting_down, move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            apply_home_notices_view(&ui, &event_config);
        });
    });
}

fn start_player_count_check(
    ui: slint::Weak<AppWindow>,
    config: LauncherConfig,
    app_shutting_down: Arc<AtomicBool>,
    last_players_text: Arc<Mutex<String>>,
) {
    thread::spawn(move || {
        let count = match steam_players::fetch_player_count() {
            Ok(count) => count,
            Err(error) => {
                log_for_config(&config, diagnostics::LogLevel::Warn, &error);
                return;
            }
        };
        steam_players::cache_player_count(count);

        let event_config = config.clone();
        invoke_on_event_loop(&config, &app_shutting_down, move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };
            apply_player_count_view(&ui);
            let text = steam_players::players_text(count);
            let mut last_text = last_players_text
                .lock()
                .expect("player count text lock poisoned");
            if text != *last_text {
                log_for_config(&event_config, diagnostics::LogLevel::Info, &text);
                *last_text = text;
            }
        });
    });
}

fn start_release_check(
    ui: &AppWindow,
    config: LauncherConfig,
    app_shutting_down: Arc<AtomicBool>,
    latest_release: Arc<Mutex<Option<PlatformRelease>>>,
    release_source: ReleaseSource,
) {
    log_for_config(
        &config,
        diagnostics::LogLevel::Info,
        "Checking GitHub releases.",
    );
    let mut checking_state = home_view_state_from_cache(&config, "Checking GitHub releases...");
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

        let event_config = config.clone();
        invoke_on_event_loop(&config, &app_shutting_down, move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            let state = if let Some(release) = release {
                latest_release
                    .lock()
                    .expect("latest release lock poisoned")
                    .replace(release.clone());
                remember_latest_drh_release(&release);
                home_view_state(&event_config, Some(&release), &message)
            } else {
                let mut state = home_view_state_from_cache(&event_config, &message);
                if cached_latest_drh_release().is_none()
                    && matches!(
                        inspect_install(event_config.install_dir.as_deref()).state,
                        InstallState::Installed
                    )
                {
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
            apply_home_notices_view(&ui, &event_config);
        });
    });
}

fn refresh_settings_view(ui: &AppWindow, config: &LauncherConfig, save_text: &str) {
    ui.set_install_folder_path(install_folder_path_text(config).into());
    let cache_limit = config.download_cache_limit.to_string();
    ui.set_download_cache_limit(cache_limit.clone().into());
    ui.set_saved_download_cache_limit(cache_limit.into());
    ui.set_download_cache_save_text(save_text.into());
    ui.set_download_cache_limit_error("".into());
    ui.set_hide_official_ownership_notice(config.hide_official_ownership_notice);
    ui.set_saved_hide_official_ownership_notice(config.hide_official_ownership_notice);
    ui.set_hide_haxe_iteratee_log_lines(config.hide_haxe_iteratee_log_lines);
}

fn install_folder_path_text(config: &LauncherConfig) -> String {
    config.effective_install_dir().display().to_string()
}

fn ui_download_cache_limit(ui: &AppWindow) -> Result<usize, ()> {
    ui.get_download_cache_limit()
        .trim()
        .parse::<usize>()
        .map_err(|_| ())
}

pub(crate) fn log_for_config(config: &LauncherConfig, level: diagnostics::LogLevel, message: &str) {
    let install_dir = config.effective_install_dir();
    let _ = diagnostics::write(&install_dir, level, message);
}

fn log_install_failure(install_dir: &Path, message: &str) {
    if diagnostics::is_operation_error_message(message) {
        let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Error, message);
    }
}

fn finish_install_operation_view(ui: &AppWindow, config: &LauncherConfig, message: &str) {
    let installed_launch_options = load_installed_launch_options(config);
    refresh_launch_options_view(ui, config, installed_launch_options.as_ref(), "Save");
    refresh_home_state(ui, config, message);
    ui.set_install_action_enabled(true);
    ui.set_update_check_enabled(true);
    ui.set_update_check_text("Check for updates".into());
}

pub(crate) fn release_update_available(
    config: &LauncherConfig,
    status: &game_install::InstallStatus,
    release: &PlatformRelease,
) -> bool {
    installed_version_needs_update(status.installed_version.as_deref(), &release.version)
        && rollback_blocked_update_version(config).as_deref() != Some(release.version.as_str())
}

pub(crate) fn rollback_blocked_update_version(config: &LauncherConfig) -> Option<String> {
    install_metadata::InstalledState::load(&config.effective_install_dir())
        .ok()
        .and_then(|state| state.blocked_update_version)
}

fn installed_version_needs_update(installed_version: Option<&str>, latest_version: &str) -> bool {
    installed_version
        .is_some_and(|installed_version| installed_version.trim() != latest_version.trim())
}

fn report_background_activity(
    ui: &slint::Weak<AppWindow>,
    config: &LauncherConfig,
    app_shutting_down: &AtomicBool,
    message: String,
) {
    let ui = ui.clone();
    invoke_on_event_loop(config, app_shutting_down, move || {
        let Some(ui) = ui.upgrade() else {
            return;
        };

        set_status_message(&ui, &message);
    });
}

pub(crate) fn invoke_on_event_loop(
    config: &LauncherConfig,
    app_shutting_down: &AtomicBool,
    callback: impl FnOnce() + Send + 'static,
) {
    if app_shutting_down.load(Ordering::Relaxed) {
        log_for_config(
            config,
            diagnostics::LogLevel::Info,
            "Skipped background UI update because DRH Launcher is shutting down.",
        );
        return;
    }

    match slint::invoke_from_event_loop(callback) {
        Ok(()) => {}
        Err(slint::EventLoopError::EventLoopTerminated) => {
            log_for_config(
                config,
                diagnostics::LogLevel::Warn,
                "Skipped background UI update after Slint event loop ended.",
            );
        }
        Err(error) => {
            log_for_config(
                config,
                diagnostics::LogLevel::Warn,
                &format!("Could not schedule background UI update: {error}"),
            );
        }
    }
}

fn format_download_progress(asset_name: &str, progress: &DownloadProgress) -> String {
    let downloaded = bytes::format_bytes(progress.downloaded);
    let total = bytes::format_bytes(progress.total);

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

#[cfg(test)]
mod home_support_text_tests {
    use super::*;
    use crate::github_releases::{ReleaseAsset, ReleaseMetadataSource};
    use crate::home_view::{
        home_support_text, restore_previous_version_available, restore_previous_version_text,
    };
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
    fn shows_startup_configuration_warning() {
        assert_eq!(
            home_support_text(
                "Version: unknown",
                "Configuration warning: Could not parse config.json; using defaults"
            ),
            "Configuration warning: Could not parse config.json; using defaults"
        );
    }

    #[test]
    fn shows_play_mode_blocker_messages() {
        assert_eq!(
            home_support_text(
                "Version: V1",
                "DRH update available before --play: installed V1, latest V2. Open DRH Launcher to update or launch manually."
            ),
            "DRH update available before --play: installed V1, latest V2. Open DRH Launcher to update or launch manually."
        );
        assert_eq!(
            home_support_text(
                "Version: unknown",
                "DRH cannot be launched from --play: Game directory does not exist."
            ),
            "DRH cannot be launched from --play: Game directory does not exist."
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
        assert_eq!(
            home_support_text("Version: V1", "Installing V3..."),
            "Installing V3..."
        );
        assert_eq!(
            home_support_text("Version: V1", "Repairing DRH installation..."),
            "Repairing DRH installation..."
        );
        assert_eq!(
            home_support_text("Version: V1", "Reinstalling current version..."),
            "Reinstalling current version..."
        );
        assert_eq!(
            home_support_text("Version: V1", "Stopping DRH..."),
            "Stopping DRH..."
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
            "Could not open logs folder: denied. See Settings → Logs for details."
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
    fn disables_restore_when_previous_directory_is_missing() {
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

        assert!(!restore_previous_version_available(&config));
        assert_eq!(restore_previous_version_text(&config), "Restore V10");
    }

    #[test]
    fn enables_restore_when_previous_directory_exists() {
        let temp = tempdir().unwrap();
        InstalledState {
            active: test_installed_release("V9"),
            previous: Some(test_installed_release("V10")),
            blocked_update_version: None,
        }
        .save(temp.path())
        .unwrap();
        std::fs::create_dir_all(crate::paths::previous_game_dir(temp.path())).unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };

        assert!(restore_previous_version_available(&config));
    }

    fn test_release(version: &str) -> PlatformRelease {
        PlatformRelease {
            version: version.to_string(),
            name: format!("Dungeon Rampage Haxe {version}"),
            html_url: format!("https://example.test/{version}"),
            metadata_source: ReleaseMetadataSource::GitHubAssetFallback,
            launch_options: None,
            steam_buildid: None,
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
            steam_buildid: None,
        }
    }
}
