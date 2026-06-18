use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::config::LauncherConfig;
use crate::home_view::{
    refresh_home_state, set_status_message,
};
use crate::install_state::InstallState;
use crate::log_view::refresh_logs_view;
use crate::{AppWindow, diagnostics, game_launch, game_logs, log_for_config, paths};
use slint::{ComponentHandle, Timer, TimerMode};

pub(crate) fn refresh_playing_state(ui: &AppWindow, config: &LauncherConfig, message: &str) {
    let install_dir = config.effective_install_dir();
    let status = crate::game_install::inspect_install(Some(&install_dir));

    ui.set_install_status("DRH is running".into());
    ui.set_install_action_text(InstallState::Playing.primary_action().into());
    ui.set_install_action_enabled(true);
    ui.set_version_status(status.version_text().into());
    ui.set_update_check_text("Check for updates".into());
    ui.set_update_check_enabled(false);
    ui.set_open_install_folder_enabled(install_dir.exists());
    ui.set_open_logs_folder_enabled(
        paths::logs_dir(&install_dir).exists() || paths::launcher_log_file(&install_dir).exists(),
    );
    ui.set_restore_previous_enabled(false);
    ui.set_reinstall_current_enabled(false);
    refresh_logs_view(ui, config);
    set_status_message(ui, message);
}

pub(crate) fn start_game_monitor(
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

pub(crate) fn process_is_running(
    game_process: &Rc<RefCell<Option<game_launch::RunningGame>>>,
) -> bool {
    let mut process = game_process.borrow_mut();
    let Some(game) = process.as_mut() else {
        return false;
    };

    match game.child.try_wait() {
        Ok(None) => true,
        Ok(Some(status)) => {
            let game = process.take().expect("running game disappeared");
            let _ = game_logs::finish(&game.session_log, &format!("Exited with status: {status}"));
            if let Some(install_dir) = install_dir_from_session_log(&game.session_log.path) {
                let _ = diagnostics::write(
                    &install_dir,
                    diagnostics::LogLevel::Info,
                    &format!("DRH exited with status: {status}"),
                );
            }
            false
        }
        Err(_) => true,
    }
}

fn install_dir_from_session_log(session_log: &Path) -> Option<PathBuf> {
    Some(
        session_log
            .parent()?
            .parent()?
            .parent()?
            .parent()?
            .to_path_buf(),
    )
}

pub(crate) fn begin_game_stop(
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
    if let Some(game) = game_process.borrow().as_ref() {
        log_for_config(
            &config,
            diagnostics::LogLevel::Info,
            &format!(
                "Requesting graceful shutdown for DRH (pid {}).",
                game.child.id()
            ),
        );
    }
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
        log_for_config(
            &config,
            diagnostics::LogLevel::Info,
            &format!("DRH stop finished: {result}"),
        );
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
