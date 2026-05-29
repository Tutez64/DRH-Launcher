// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod archive;
mod config;
mod diagnostics;
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
use std::path::Path;
use std::process::{Child, Command};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use archive::extract_to_staging;
use config::{LaunchArgumentsMode, LauncherConfig};
use download::{DownloadProgress, download_and_verify_with_progress};
use game_install::inspect_install;
use github_releases::{
    PlatformRelease, ReleaseMetadataSource, discover_latest_platform_release,
    discover_latest_platform_release_for_install,
};
use install_state::InstallState;
use installer::install_extracted_archive;
use platform::Platform;
use release_manifest::ManifestLaunchOptions;
use release_source::ReleaseSource;
use slint::{Model, ModelRc, Timer, TimerMode, VecModel};

slint::include_modules!();

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
    status_detail: String,
    logs_content: String,
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let config = Rc::new(RefCell::new(LauncherConfig::load()));
    let latest_release = Arc::new(Mutex::new(None::<PlatformRelease>));
    let game_process = Rc::new(RefCell::new(None::<Child>));
    let game_monitor = Rc::new(Timer::default());
    let release_source = ReleaseSource::from_environment();

    ui.set_launcher_version(env!("CARGO_PKG_VERSION").into());
    ui.set_release_source_label(release_source.label().into());
    ui.set_config_path(paths::config_file().display().to_string().into());
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
            let update_available = release.as_ref().is_some_and(|release| {
                installed_version_needs_update(
                    install_status.installed_version.as_deref(),
                    &release.version,
                )
            });

            if matches!(
                install_status.state,
                InstallState::Installed | InstallState::LaunchableButMaybeOutdated
            ) && !update_available
            {
                let config = config.clone();
                let installed_launch_options = load_installed_launch_options(&config);
                let recommended_game_args =
                    release_recommended_game_args(installed_launch_options.as_ref());
                match game_launch::launch_game_with_recommended_args(
                    &config,
                    &recommended_game_args,
                ) {
                    Ok(child) => {
                        log_for_config(
                            &config,
                            diagnostics::LogLevel::Info,
                            &format!(
                                "DRH launched with command: {}",
                                game_launch::launch_command_summary_with_recommended_args(
                                    &config,
                                    &recommended_game_args
                                )
                                .unwrap_or_else(|error| format!(
                                    "command summary unavailable ({error})"
                                ))
                            ),
                        );
                        game_process.borrow_mut().replace(child);
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

            let initial_message = if let Some(release) = &release {
                format!("Downloading {}...", release.asset.name)
            } else {
                "Checking GitHub releases...".to_string()
            };
            refresh_home_state(&ui, &config, &initial_message);
            let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Info, &initial_message);
            ui.set_install_action_enabled(false);
            ui.set_update_check_enabled(false);
            ui.set_install_action_text(InstallState::Updating.primary_action().into());

            let ui = ui.as_weak();
            let config = config.clone();
            let latest_release = Arc::clone(&latest_release);
            let release_source = release_source.clone();
            thread::spawn(move || {
                let release = match release {
                    Some(release) if release.metadata_source == ReleaseMetadataSource::Manifest => {
                        Ok(release)
                    }
                    _ => discover_latest_platform_release_for_install(
                        &release_source,
                        Platform::current(),
                    ),
                };

                let message = match release {
                    Ok(release) => {
                        latest_release
                            .lock()
                            .expect("latest release lock poisoned")
                            .replace(release.clone());
                        let _ = diagnostics::write(
                            &install_dir,
                            diagnostics::LogLevel::Info,
                            &format!("Found release {}. Starting install.", release.version),
                        );
                        report_background_activity(
                            &ui,
                            format!("Preparing download for {}...", release.version),
                        );
                        let mut last_percent = None::<u64>;
                        let mut last_logged_percent = None::<u64>;
                        let mut reported_verification = false;
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
                                    );
                                }
                                if should_log_download_progress(percent, last_logged_percent) {
                                    last_logged_percent = percent;
                                    let _ = diagnostics::write(
                                        &install_dir,
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
                                    let _ = diagnostics::write(
                                        &install_dir,
                                        diagnostics::LogLevel::Info,
                                        &message,
                                    );
                                    report_background_activity(&ui, message);
                                }
                            },
                        ) {
                            Ok(download) => {
                                let verified_message = format!(
                                    "Download verified: {} ({} bytes, sha256:{})",
                                    download.path.display(),
                                    download.size,
                                    download.sha256
                                );
                                let _ = diagnostics::write(
                                    &install_dir,
                                    diagnostics::LogLevel::Info,
                                    &verified_message,
                                );
                                report_background_activity(
                                    &ui,
                                    "Extracting archive...".to_string(),
                                );
                                match extract_to_staging(&download, &install_dir) {
                                    Ok(extracted) => {
                                        let _ = diagnostics::write(
                                            &install_dir,
                                            diagnostics::LogLevel::Info,
                                            "Archive extracted. Installing files.",
                                        );
                                        report_background_activity(
                                            &ui,
                                            "Installing files...".to_string(),
                                        );
                                        match install_extracted_archive(
                                            &extracted,
                                            &install_dir,
                                            &release,
                                            &release_source,
                                        ) {
                                            Ok(installed) => {
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
                                                    &install_dir,
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
        ui.unwrap().on_refresh_logs(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            refresh_logs_view(&ui, &config.borrow());
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
                    state.install_status =
                        "DRH is launchable, but update status is unknown".to_string();
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

fn release_recommended_game_args(launch_options: Option<&ManifestLaunchOptions>) -> Vec<String> {
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
        status_detail: status_detail(activity_message),
        logs_content: logs_content(config),
    };

    if let Some(release) = latest_release {
        apply_release_to_home_view_state(&mut state, &status, release);
    }

    state
}

fn apply_release_to_home_view_state(
    state: &mut HomeViewState,
    status: &game_install::InstallStatus,
    release: &PlatformRelease,
) {
    match status.state {
        InstallState::Installed | InstallState::LaunchableButMaybeOutdated => {
            match status.installed_version.as_deref() {
                Some(installed_version)
                    if installed_version_needs_update(
                        Some(installed_version),
                        &release.version,
                    ) =>
                {
                    state.install_status = "A DRH update is available".to_string();
                    state.install_action_text =
                        InstallState::UpdateAvailable.primary_action().to_string();
                    state.home_support_text = format!(
                        "Update available: installed {installed_version}, latest {}.",
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
    ui.set_status_detail(state.status_detail.into());
    ui.set_logs_content(state.logs_content.into());
}

fn refresh_logs_view(ui: &AppWindow, config: &LauncherConfig) {
    ui.set_logs_content(logs_content(config).into());
}

fn logs_content(config: &LauncherConfig) -> String {
    let Some(install_dir) = config.install_dir.as_deref() else {
        return "No install directory selected.".to_string();
    };

    diagnostics::read_recent(install_dir)
        .unwrap_or_else(|error| format!("Could not read launcher log: {error}"))
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

fn open_logs_folder(install_dir: &Path) -> Result<(), String> {
    let logs_dir = paths::logs_dir(install_dir);
    std::fs::create_dir_all(&logs_dir)
        .map_err(|error| format!("Could not create {}: {error}", logs_dir.display()))?;
    open_folder(&logs_dir)
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
                    Some((
                        diagnostics::LogLevel::Info,
                        format!("DRH exited with status: {status}"),
                    ))
                }
                Ok(None) => None,
                Err(error) => {
                    process.take();
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
