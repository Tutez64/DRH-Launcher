use std::collections::HashMap;
use std::path::Path;

use crate::config::LauncherConfig;
use crate::{AppWindow, GameSessionView, LogLineView, diagnostics, game_logs};
use slint::{Brush, Color, Model, ModelRc, VecModel};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LogViewportPosition {
    pub(crate) y: f32,
    pub(crate) at_end: bool,
}

pub(crate) fn refresh_logs_view(ui: &AppWindow, config: &LauncherConfig) {
    let wrap_columns = if ui.get_log_source() == 0 {
        ui.get_launcher_log_wrap_columns()
    } else {
        ui.get_game_log_wrap_columns()
    }
    .max(20) as usize;
    let install_dir = config.effective_install_dir();

    let sessions = match game_logs::list(&install_dir) {
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

pub(crate) fn game_log_session_id(path: &Path) -> String {
    path.file_name()
        .map(|name| {
            let name = name.to_string_lossy();
            name.strip_suffix(".zst")
                .unwrap_or(name.as_ref())
                .to_string()
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub(crate) fn remember_game_log_position(
    positions: &mut HashMap<String, LogViewportPosition>,
    session_id: String,
    y: f32,
    at_end: bool,
) {
    positions.insert(session_id, LogViewportPosition { y, at_end });
}

pub(crate) fn saved_game_log_position(
    positions: &HashMap<String, LogViewportPosition>,
    session_id: &str,
) -> Option<LogViewportPosition> {
    positions.get(session_id).copied()
}

fn log_lines(config: &LauncherConfig, wrap_columns: usize) -> Vec<LogLineView> {
    let install_dir = config.effective_install_dir();
    let content = diagnostics::read_recent(&install_dir)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
