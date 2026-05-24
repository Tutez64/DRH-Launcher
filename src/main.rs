// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod paths;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use config::LauncherConfig;

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

            refresh_home_state(
                &ui,
                &config.borrow(),
                "Update checks are not wired to GitHub releases yet.",
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

    ui.run()
}

fn refresh_home_state(ui: &AppWindow, config: &LauncherConfig, message: &str) {
    let install_dir = config.install_dir.as_deref();
    let installed = install_dir.is_some_and(is_game_installed);

    if installed {
        ui.set_install_status("DRH is installed".into());
        ui.set_install_action_text("Play".into());
    } else {
        ui.set_install_status("DRH is not installed".into());
        ui.set_install_action_text("Install DRH".into());
    }

    let version_status = install_dir
        .map(|path| format!("Install directory: {}", path.display()))
        .unwrap_or_else(|| "No install directory selected.".to_string());

    ui.set_version_status(version_status.into());
    ui.set_activity_message(message.into());
}

fn is_game_installed(path: &Path) -> bool {
    path.join("game").join("version.json").is_file()
}
