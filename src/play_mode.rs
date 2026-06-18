use std::ffi::OsString;

use crate::config::LauncherConfig;
use crate::github_releases::discover_latest_platform_release;
use crate::install_state::InstallState;
use crate::launch_options::{
    load_installed_launch_options, recommended_game_args_from_launch_options,
};
use crate::platform::Platform;
use crate::release_source::ReleaseSource;
use crate::{diagnostics, game_install, game_launch, game_logs, paths, release_update_available};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupMode {
    Gui,
    Play,
}

pub enum Outcome {
    Exit(i32),
    OpenGui(String),
}

pub fn startup_mode_from_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<StartupMode, String> {
    let mut mode = StartupMode::Gui;
    for arg in args {
        if arg == "--play" {
            mode = StartupMode::Play;
        } else {
            return Err(format!("Unknown argument: {}", arg.to_string_lossy()));
        }
    }
    Ok(mode)
}

pub fn run() -> Outcome {
    let (mut config, config_load_warning) = LauncherConfig::load_with_diagnostics();
    let release_source = ReleaseSource::from_environment();
    let install_dir = match config.install_dir.clone().or_else(|| {
        let default_dir = paths::default_install_dir();
        paths::game_dir(&default_dir)
            .exists()
            .then_some(default_dir)
    }) {
        Some(install_dir) => install_dir,
        None => {
            return Outcome::OpenGui(
                "DRH is not installed yet. Install it from DRH Launcher.".to_string(),
            );
        }
    };
    config.install_dir = Some(install_dir.clone());

    if let Some(warning) = &config_load_warning {
        let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Warn, warning);
    }
    let _ = diagnostics::write(
        &install_dir,
        diagnostics::LogLevel::Info,
        &format!(
            "--play requested. Release source: {}",
            release_source.label()
        ),
    );

    let install_status = game_install::inspect_install(Some(&install_dir));
    if !matches!(
        install_status.state,
        InstallState::Installed | InstallState::LaunchableButMaybeOutdated
    ) {
        let message = match install_status.reason.as_deref() {
            Some(reason) if !reason.is_empty() => {
                format!("DRH cannot be launched from --play: {reason}")
            }
            _ => format!(
                "DRH cannot be launched from --play: {}",
                install_status.status_text()
            ),
        };
        let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Warn, &message);
        return Outcome::OpenGui(message);
    }

    match discover_latest_platform_release(&release_source, Platform::current()) {
        Ok(release) => {
            let message = format!(
                "--play update check: latest known version is {} ({}) via {}.",
                release.version,
                release.name,
                release.metadata_source.label()
            );
            let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Info, &message);
            if release_update_available(&config, &install_status, &release) {
                let installed = install_status
                    .installed_version
                    .as_deref()
                    .unwrap_or("unknown");
                let message = format!(
                    "DRH update available before --play: installed {installed}, latest {}. Open DRH Launcher to update or launch manually.",
                    release.version
                );
                let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Info, &message);
                return Outcome::OpenGui(message);
            }
        }
        Err(error) => {
            let message = format!(
                "Could not check for updates before --play; launching installed DRH: {error}"
            );
            let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Warn, &message);
        }
    }

    let installed_launch_options = load_installed_launch_options(&config);
    let recommended_game_args =
        recommended_game_args_from_launch_options(installed_launch_options.as_ref());
    let command_summary =
        game_launch::launch_command_summary_with_recommended_args(&config, &recommended_game_args)
            .unwrap_or_else(|error| format!("command summary unavailable ({error})"));

    let mut game = match game_launch::launch_game_with_recommended_args(
        &config,
        &recommended_game_args,
        install_status.installed_version.as_deref(),
    ) {
        Ok(game) => game,
        Err(error) => {
            let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Error, &error);
            return Outcome::OpenGui(error);
        }
    };

    let _ = diagnostics::write(
        &install_dir,
        diagnostics::LogLevel::Info,
        &format!(
            "DRH launched from --play with command: {}. Session log: {}",
            command_summary,
            game.session_log.path.display()
        ),
    );

    match game.child.wait() {
        Ok(status) => {
            let result = format!("Exited with status: {status}");
            let level = if status.success() {
                diagnostics::LogLevel::Info
            } else {
                diagnostics::LogLevel::Warn
            };
            match game_logs::finish(&game.session_log, &result) {
                Ok(_) => {
                    let _ = diagnostics::write(
                        &install_dir,
                        level,
                        &format!("DRH launched from --play exited with status: {status}"),
                    );
                    Outcome::Exit(status.code().unwrap_or(1))
                }
                Err(error) => {
                    let message = format!(
                        "DRH exited with status: {status}, but its session log could not be finished: {error}"
                    );
                    let _ =
                        diagnostics::write(&install_dir, diagnostics::LogLevel::Error, &message);
                    Outcome::OpenGui(message)
                }
            }
        }
        Err(error) => {
            let message = format!("Could not wait for DRH launched from --play: {error}");
            let _ = game_logs::finish(&game.session_log, &message);
            let _ = diagnostics::write(&install_dir, diagnostics::LogLevel::Error, &message);
            Outcome::OpenGui(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_startup_play_mode() {
        assert_eq!(
            startup_mode_from_args(Vec::<OsString>::new()).unwrap(),
            StartupMode::Gui
        );
        assert_eq!(
            startup_mode_from_args(vec![OsString::from("--play")]).unwrap(),
            StartupMode::Play
        );
        assert!(startup_mode_from_args(vec![OsString::from("--bad")]).is_err());
    }
}
