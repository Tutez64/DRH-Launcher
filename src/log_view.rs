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
    refresh_logs_view_impl(ui, config, true);
}

pub(crate) fn refresh_log_content(ui: &AppWindow, config: &LauncherConfig) {
    refresh_logs_view_impl(ui, config, false);
}

fn refresh_logs_view_impl(ui: &AppWindow, config: &LauncherConfig, refresh_game_sessions: bool) {
    let wrap_columns = if ui.get_log_source() == 0 {
        ui.get_launcher_log_wrap_columns()
    } else {
        ui.get_game_log_wrap_columns()
    }
    .max(20) as usize;
    let install_dir = config.effective_install_dir();

    if refresh_game_sessions {
        let sessions = match game_logs::list(&install_dir) {
            Ok(sessions) => sessions,
            Err(error) => {
                set_game_log_sessions_if_changed(ui, Vec::new());
                ui.set_game_log_sessions_label(session_list_label(0).into());
                ui.set_selected_game_log_index(-1);
                ui.set_selected_game_log_enabled(false);
                ui.set_selected_game_log_title("Could not list game sessions".into());
                ui.set_selected_game_log_id("".into());
                if ui.get_log_source() == 0 {
                    show_launcher_log(ui, config, wrap_columns);
                } else {
                    show_log_text(ui, &error, "", wrap_columns);
                }
                return;
            }
        };
        let session_views = sessions
            .iter()
            .map(|session| GameSessionView {
                id: game_log_session_id(&session.path).into(),
                title: session.title.clone().into(),
                detail: session.detail.clone().into(),
            })
            .collect::<Vec<_>>();
        let session_count = session_views.len();
        set_game_log_sessions_if_changed(ui, session_views);
        ui.set_game_log_sessions_label(session_list_label(session_count).into());
    }

    let sessions = ui.get_game_log_sessions();

    if ui.get_log_source() == 0 {
        ui.set_selected_game_log_enabled(sessions.row_count() != 0);
        show_launcher_log(ui, config, wrap_columns);
        return;
    }

    let selected_index = match usize::try_from(ui.get_selected_game_log_index()) {
        Ok(index) if index < sessions.row_count() => Some(index),
        _ if sessions.row_count() == 0 => None,
        _ => Some(0),
    };
    let Some(selected_index) = selected_index else {
        ui.set_selected_game_log_index(-1);
        ui.set_selected_game_log_enabled(false);
        ui.set_selected_game_log_title("No game sessions yet".into());
        ui.set_selected_game_log_id("".into());
        show_log_text(
            ui,
            "Game session logs will appear here after launching DRH.",
            "",
            wrap_columns,
        );
        return;
    };

    let session = sessions
        .row_data(selected_index)
        .expect("selected game log disappeared from its model");
    let loaded = game_logs::path_for_session_id(&install_dir, session.id.as_str())
        .ok_or_else(|| format!("Game session log no longer exists: {}", session.id))
        .and_then(|path| game_logs::read_with_size(&path));
    ui.set_selected_game_log_index(selected_index as i32);
    ui.set_selected_game_log_enabled(true);
    ui.set_selected_game_log_title(session.title.clone());
    ui.set_selected_game_log_id(session.id.clone());
    match loaded {
        Ok((content, size)) => show_log_text(
            ui,
            &content,
            &format_game_log_meta(logical_line_count(&content), size),
            wrap_columns,
        ),
        Err(error) => show_log_text(ui, &error, "", wrap_columns),
    }
}

fn show_launcher_log(ui: &AppWindow, config: &LauncherConfig, wrap_columns: usize) {
    match diagnostics::read_recent(&config.effective_install_dir()) {
        Ok(log) => {
            let title = if log.truncated && log.has_entries {
                "Recent launcher log"
            } else {
                "Launcher log"
            };
            let meta = if log.has_entries {
                format_launcher_log_meta(logical_line_count(&log.text), log.truncated)
            } else {
                String::new()
            };
            ui.set_launcher_log_title(title.into());
            show_log_text(ui, &log.text, &meta, wrap_columns);
        }
        Err(error) => {
            ui.set_launcher_log_title("Launcher log".into());
            show_log_text(
                ui,
                &format!("Could not read launcher log: {error}"),
                "",
                wrap_columns,
            );
        }
    }
}

fn show_log_text(ui: &AppWindow, text: &str, meta: &str, wrap_columns: usize) {
    ui.set_log_meta(meta.into());
    set_log_lines_if_changed(ui, log_lines_from_text(text, wrap_columns));
}

fn logical_line_count(text: &str) -> usize {
    text.lines().count()
}

fn format_line_count(count: usize) -> String {
    if count == 1 {
        "1 line".to_string()
    } else {
        format!("{count} lines")
    }
}

fn format_launcher_log_meta(count: usize, truncated: bool) -> String {
    if truncated {
        format!("last {}", format_line_count(count))
    } else {
        format_line_count(count)
    }
}

fn format_game_log_meta(count: usize, size: u64) -> String {
    format!(
        "{} · {}",
        format_line_count(count),
        game_logs::format_file_size(size)
    )
}

fn session_list_label(count: usize) -> String {
    format!("Game sessions ({count})")
}

fn set_log_lines_if_changed(ui: &AppWindow, lines: Vec<LogLineView>) {
    let current = ui.get_log_lines();
    let unchanged = current.row_count() == lines.len()
        && lines.iter().enumerate().all(|(index, line)| {
            current.row_data(index).is_some_and(|current| {
                current.text == line.text
                    && current.color == line.color
                    && current.continuation == line.continuation
            })
        });
    if !unchanged {
        ui.set_log_lines(ModelRc::new(VecModel::from(lines)));
    }
}

fn set_game_log_sessions_if_changed(ui: &AppWindow, sessions: Vec<GameSessionView>) {
    let current = ui.get_game_log_sessions();
    let unchanged = current.row_count() == sessions.len()
        && sessions.iter().enumerate().all(|(index, session)| {
            current.row_data(index).is_some_and(|current| {
                current.id == session.id
                    && current.title == session.title
                    && current.detail == session.detail
            })
        });
    if !unchanged {
        ui.set_game_log_sessions(ModelRc::new(VecModel::from(sessions)));
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

fn log_lines_from_text(content: &str, wrap_columns: usize) -> Vec<LogLineView> {
    content
        .lines()
        .flat_map(|line| {
            let color = log_line_color(line);
            split_display_line(line, wrap_columns)
                .into_iter()
                .enumerate()
                .map(move |(index, text)| LogLineView {
                    text: text.into(),
                    color: color.clone(),
                    continuation: index > 0,
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

pub(crate) fn extract_log_selection(
    lines: &impl Model<Data = LogLineView>,
    anchor_line: i32,
    anchor_col: i32,
    cursor_line: i32,
    cursor_col: i32,
) -> String {
    let count = lines.row_count();
    if count == 0 {
        return String::new();
    }

    let last = (count - 1) as i32;
    let start = anchor_line.min(cursor_line).clamp(0, last) as usize;
    let end = anchor_line.max(cursor_line).clamp(0, last) as usize;
    let selected = (start..=end)
        .filter_map(|index| {
            lines.row_data(index).map(|line| SelectableLogLine {
                text: line.text.to_string(),
                continuation: line.continuation,
            })
        })
        .collect::<Vec<_>>();
    let local_anchor = (anchor_line.clamp(0, last) as usize).saturating_sub(start) as i32;
    let local_cursor = (cursor_line.clamp(0, last) as usize).saturating_sub(start) as i32;
    extract_log_selection_from_lines(
        &selected,
        local_anchor,
        anchor_col,
        local_cursor,
        cursor_col,
    )
}

#[derive(Clone, Debug)]
struct SelectableLogLine {
    text: String,
    continuation: bool,
}

fn extract_log_selection_from_lines(
    lines: &[SelectableLogLine],
    anchor_line: i32,
    anchor_col: i32,
    cursor_line: i32,
    cursor_col: i32,
) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let last_index = (lines.len() - 1) as i32;
    let clamp_pos = |line: i32, col: i32| {
        let line = line.clamp(0, last_index) as usize;
        let max_col = i32::try_from(lines[line].text.chars().count()).unwrap_or(i32::MAX);
        (line, col.clamp(0, max_col) as usize)
    };

    let (anchor_line, anchor_col) = clamp_pos(anchor_line, anchor_col);
    let (cursor_line, cursor_col) = clamp_pos(cursor_line, cursor_col);
    let (start_line, start_col, end_line, end_col) =
        if (anchor_line, anchor_col) <= (cursor_line, cursor_col) {
            (anchor_line, anchor_col, cursor_line, cursor_col)
        } else {
            (cursor_line, cursor_col, anchor_line, anchor_col)
        };

    if start_line == end_line && start_col == end_col {
        return String::new();
    }

    let mut output = String::new();
    for (index, line) in lines.iter().enumerate().take(end_line + 1).skip(start_line) {
        if index != start_line && !line.continuation {
            output.push('\n');
        }
        let from = if index == start_line { start_col } else { 0 };
        let to = if index == end_line {
            end_col
        } else {
            line.text.chars().count()
        };
        output.extend(line.text.chars().skip(from).take(to.saturating_sub(from)));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_game_session_list_label_with_count() {
        assert_eq!(session_list_label(0), "Game sessions (0)");
        assert_eq!(session_list_label(3), "Game sessions (3)");
    }

    #[test]
    fn counts_logical_log_lines_not_display_wraps() {
        assert_eq!(logical_line_count(""), 0);
        assert_eq!(logical_line_count("one"), 1);
        assert_eq!(logical_line_count("one\n"), 1);
        assert_eq!(logical_line_count("one\ntwo\nthree"), 3);
        assert_eq!(log_lines_from_text("abcdefghij", 5).len(), 2);
        assert_eq!(logical_line_count("abcdefghij"), 1);
    }

    #[test]
    fn formats_launcher_and_game_log_file_meta() {
        assert_eq!(format_launcher_log_meta(42, false), "42 lines");
        assert_eq!(format_launcher_log_meta(1, false), "1 line");
        assert_eq!(format_launcher_log_meta(187, true), "last 187 lines");
        assert_eq!(format_launcher_log_meta(1, true), "last 1 line");
        assert_eq!(format_game_log_meta(1847, 1024), "1847 lines · 1.0 KiB");
        assert_eq!(format_game_log_meta(1, 80), "1 line · 80 B");
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
    fn copies_character_range_across_wrapped_log_lines() {
        let lines = vec![
            selectable("alpha beta", false),
            selectable("gamma", false),
            selectable("delta epsilon", false),
        ];

        assert_eq!(
            extract_log_selection_from_lines(&lines, 0, 6, 2, 5),
            "beta\ngamma\ndelta"
        );
        assert_eq!(
            extract_log_selection_from_lines(&lines, 2, 5, 0, 6),
            "beta\ngamma\ndelta"
        );
        assert_eq!(extract_log_selection_from_lines(&lines, 1, 1, 1, 4), "amm");
        assert_eq!(extract_log_selection_from_lines(&lines, 0, 2, 0, 2), "");
        assert_eq!(extract_log_selection_from_lines(&[], 0, 0, 1, 1), "");
    }

    #[test]
    fn copy_joins_wrapped_display_segments_without_extra_newlines() {
        let lines = vec![
            selectable("very long ", false),
            selectable("error line", true),
            selectable("next record", false),
        ];

        assert_eq!(
            extract_log_selection_from_lines(&lines, 0, 0, 2, 11),
            "very long error line\nnext record"
        );
        assert_eq!(
            extract_log_selection_from_lines(&lines, 0, 5, 1, 5),
            "long error"
        );
    }

    fn selectable(text: &str, continuation: bool) -> SelectableLogLine {
        SelectableLogLine {
            text: text.to_string(),
            continuation,
        }
    }
}
