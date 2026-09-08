use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::LauncherConfig;
use crate::home_view::{HomeMessage, apply_playing_home_state, refresh_home_state};
use crate::log_view::refresh_logs_view;
use crate::{AppWindow, diagnostics, game_launch, game_logs, invoke_on_event_loop, log_for_config};
use slint::{ComponentHandle, Timer, TimerMode};

#[derive(Clone)]
pub(crate) struct SessionLogFinisher {
    pending: Arc<Mutex<Vec<JoinHandle<()>>>>,
    app_shutting_down: Arc<AtomicBool>,
}

impl SessionLogFinisher {
    pub(crate) fn new(app_shutting_down: Arc<AtomicBool>) -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
            app_shutting_down,
        }
    }

    pub(crate) fn finish_session(
        &self,
        session: &game_logs::GameSessionLog,
        result: &str,
        ui: slint::Weak<AppWindow>,
        config: &LauncherConfig,
    ) -> Result<(), String> {
        game_logs::write_session_result(session, result)?;
        self.spawn_compress(session.path.clone(), ui, config.clone());
        Ok(())
    }

    pub(crate) fn wait(&self) {
        let handles = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for handle in handles {
            let _ = handle.join();
        }
    }

    fn spawn_compress(&self, path: PathBuf, ui: slint::Weak<AppWindow>, config: LauncherConfig) {
        let app_shutting_down = Arc::clone(&self.app_shutting_down);
        let handle = std::thread::spawn(move || {
            if let Err(error) = game_logs::compress_finished_log(&path) {
                log_for_config(
                    &config,
                    diagnostics::LogLevel::Error,
                    &format!("Could not compress game session log: {error}"),
                );
            }
            let event_config = config.clone();
            invoke_on_event_loop(&config, &app_shutting_down, move || {
                let Some(ui) = ui.upgrade() else {
                    return;
                };
                refresh_logs_view(&ui, &event_config);
            });
        });
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(handle);
    }
}

const GAME_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) fn refresh_playing_state(
    ui: &AppWindow,
    config: &LauncherConfig,
    message: HomeMessage,
    stopping: bool,
) {
    apply_playing_home_state(ui, config, message, stopping);
    refresh_logs_view(ui, config);
}

pub(crate) fn start_game_monitor(
    timer: Rc<Timer>,
    ui: slint::Weak<AppWindow>,
    config: LauncherConfig,
    game_process: Rc<RefCell<Option<game_launch::RunningGame>>>,
    session_log_finisher: SessionLogFinisher,
) {
    let timer_handle = Rc::clone(&timer);
    timer.start(TimerMode::Repeated, GAME_PROCESS_POLL_INTERVAL, move || {
        let finished = {
            let mut process = game_process.borrow_mut();
            let Some(game) = process.as_mut() else {
                timer_handle.stop();
                return;
            };

            match game.try_wait() {
                Ok(Some(status)) => {
                    let game = process.take().expect("running game disappeared");
                    Some(complete_exited_game(
                        game,
                        status,
                        &session_log_finisher,
                        ui.clone(),
                        &config,
                    ))
                }
                Ok(None) => None,
                Err(error) => {
                    let game = process.take().expect("running game disappeared");
                    let message = format!("Could not inspect DRH process: {error}");
                    let _ = session_log_finisher.finish_session(
                        &game.session_log,
                        &message,
                        ui.clone(),
                        &config,
                    );
                    Some((diagnostics::LogLevel::Error, message))
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
            refresh_home_state(&ui, &config, home_message_from_process_result(&message));
            refresh_logs_view(&ui, &config);
        }
    });
}

/// Whether a DRH process started by this instance is still running.
///
/// This only inspects the tracked handle. It does not take the process or
/// finalize the session log; the game monitor and Stop path own that.
pub(crate) fn process_is_running(
    game_process: &Rc<RefCell<Option<game_launch::RunningGame>>>,
) -> bool {
    let mut process = game_process.borrow_mut();
    let Some(game) = process.as_mut() else {
        return false;
    };

    match game.try_wait() {
        Ok(None) => true,
        Ok(Some(_)) => false,
        Err(_) => true,
    }
}

/// Finalize a tracked game that has already exited, then stop its monitor.
///
/// Used when Home, close, or a restart action cannot wait for the monitor.
/// Returns the Home message when a session was finalized.
pub(crate) fn finalize_exited_game(
    game_process: &Rc<RefCell<Option<game_launch::RunningGame>>>,
    game_monitor: &Timer,
    config: &LauncherConfig,
    ui: slint::Weak<AppWindow>,
    session_log_finisher: &SessionLogFinisher,
) -> Option<String> {
    let taken = {
        let mut process = game_process.borrow_mut();
        let game = process.as_mut()?;
        match game.try_wait() {
            Ok(Some(status)) => Some((process.take().expect("running game disappeared"), status)),
            _ => None,
        }
    };
    let (game, status) = taken?;

    game_monitor.stop();
    let (level, message) = complete_exited_game(game, status, session_log_finisher, ui, config);
    log_for_config(config, level, &message);
    Some(message)
}

fn complete_exited_game(
    game: game_launch::RunningGame,
    status: ExitStatus,
    session_log_finisher: &SessionLogFinisher,
    ui: slint::Weak<AppWindow>,
    config: &LauncherConfig,
) -> (diagnostics::LogLevel, String) {
    let result = format!("Exited with status: {status}");
    let _ = session_log_finisher.finish_session(&game.session_log, &result, ui, config);
    (
        diagnostics::LogLevel::Info,
        format!("DRH exited with status: {status}"),
    )
}

pub(crate) fn begin_game_stop(
    timer: Rc<Timer>,
    ui: slint::Weak<AppWindow>,
    config: LauncherConfig,
    game_process: Rc<RefCell<Option<game_launch::RunningGame>>>,
    game_monitor: Rc<Timer>,
    session_log_finisher: SessionLogFinisher,
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
            &format!("Requesting graceful shutdown for DRH (pid {}).", game.id()),
        );
    }
    let started_at = Instant::now();
    let mut forced = false;
    let mut logged_remaining = false;
    let mut forced_reason =
        graceful_error.map(|error| format!("graceful shutdown request failed: {error}"));
    let timer_handle = Rc::clone(&timer);

    timer.start(TimerMode::Repeated, GAME_PROCESS_POLL_INTERVAL, move || {
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
                    &session_log_finisher,
                    quit_when_finished,
                    Ok("DRH is not running.".to_string()),
                );
                return;
            };

            match game.try_wait() {
                Ok(Some(status)) => {
                    let game = process.take().expect("running game disappeared");
                    Some((game, status))
                }
                Ok(None) => {
                    if game.child_has_exited() && !logged_remaining {
                        logged_remaining = true;
                        log_for_config(
                            &config,
                            diagnostics::LogLevel::Info,
                            &format!(
                                "Tracked launch process exited; waiting for remaining DRH processes (pgid {}).",
                                game.id()
                            ),
                        );
                    }
                    if !forced
                        && forced_reason.is_none()
                        && started_at.elapsed() >= GRACEFUL_STOP_TIMEOUT
                    {
                        forced_reason = Some("graceful shutdown timed out".to_string());
                    }
                    if !forced && forced_reason.is_some() {
                        match game_launch::force_shutdown(&mut game.child) {
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
                &session_log_finisher,
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
        let finish_result = session_log_finisher
            .finish_session(&game.session_log, &result, ui.clone(), &config)
            .map(|_| "DRH stopped.".to_string())
            .map_err(|error| {
                format!("DRH stopped, but its session log could not be finished: {error}")
            });
        finish_game_stop(
            &ui,
            &config,
            &game_process,
            &game_monitor,
            &session_log_finisher,
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
    session_log_finisher: &SessionLogFinisher,
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
                refresh_home_state(&ui, config, home_message_from_process_result(&message));
                refresh_logs_view(&ui, config);
            }
        }
        Err(error) => {
            log_for_config(config, diagnostics::LogLevel::Error, &error);
            if process_is_running(game_process) {
                refresh_playing_state(&ui, config, HomeMessage::error(&error), false);
                start_game_monitor(
                    Rc::clone(game_monitor),
                    ui.as_weak(),
                    config.clone(),
                    Rc::clone(game_process),
                    session_log_finisher.clone(),
                );
            } else {
                let _ = finalize_exited_game(
                    game_process,
                    game_monitor,
                    config,
                    ui.as_weak(),
                    session_log_finisher,
                );
                refresh_home_state(&ui, config, HomeMessage::error(&error));
            }
            ui.set_close_confirmation_busy(false);
            ui.set_close_confirmation_error(error.into());
        }
    }
}

fn home_message_from_process_result(message: &str) -> HomeMessage {
    if message.starts_with("DRH exited ") {
        HomeMessage::Exited
    } else if message.starts_with("DRH stopped.") {
        HomeMessage::Stopped
    } else if message.starts_with("Could not ") {
        HomeMessage::error(message)
    } else {
        HomeMessage::Ready
    }
}
