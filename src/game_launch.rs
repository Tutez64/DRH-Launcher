use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::config::LauncherConfig;
use crate::game_install::{game_executable_names, is_game_executable};
use crate::{game_logs, paths, platform::Platform};

pub struct RunningGame {
    pub child: Child,
    pub session_log: game_logs::GameSessionLog,
}

pub fn launch_game_with_recommended_args(
    config: &LauncherConfig,
    recommended_game_args: &[String],
    installed_version: Option<&str>,
) -> Result<RunningGame, String> {
    let install_dir = config
        .install_dir
        .as_deref()
        .ok_or_else(|| "No install directory selected.".to_string())?;
    let game_dir = paths::game_dir(install_dir);
    let executable = find_game_executable(&game_dir)?;
    let launch_executable = resolve_launch_executable(&executable)?;

    let pre_launch_command = parse_command_line(&config.pre_launch_command)
        .map_err(|error| format!("Could not parse pre-launch command: {error}"))?;
    let game_args = config.effective_game_args_with_recommended(recommended_game_args);

    let mut command = if !pre_launch_command.is_empty() {
        let mut command = Command::new(&pre_launch_command[0]);
        command.args(&pre_launch_command[1..]);
        command.arg(&launch_executable);
        command.args(&game_args);
        command
    } else {
        let mut command = Command::new(&launch_executable);
        command.args(&game_args);
        command
    };

    let command_summary =
        launch_command_summary_with_recommended_args(config, recommended_game_args)?;
    let (log_file, session_log) = game_logs::create(
        install_dir,
        installed_version,
        Platform::current().id(),
        &command_summary,
    )?;
    let stderr_file = log_file.try_clone().map_err(|error| {
        let _ = game_logs::finish(
            &session_log,
            &format!("Could not prepare stderr capture: {error}"),
        );
        format!("Could not prepare game log: {error}")
    })?;

    command.current_dir(&game_dir);
    command.stdout(Stdio::from(log_file));
    command.stderr(Stdio::from(stderr_file));
    let child = command.spawn().map_err(|error| {
        let _ = game_logs::finish(&session_log, &format!("Launch failed: {error}"));
        format!("Could not launch DRH: {error}")
    })?;

    Ok(RunningGame { child, session_log })
}

pub fn launch_command_summary_with_recommended_args(
    config: &LauncherConfig,
    recommended_game_args: &[String],
) -> Result<String, String> {
    let install_dir = config
        .install_dir
        .as_deref()
        .ok_or_else(|| "No install directory selected.".to_string())?;
    let game_dir = paths::game_dir(install_dir);
    let executable = find_game_executable(&game_dir)?;
    let launch_executable = resolve_launch_executable(&executable)?;
    let pre_launch_command = parse_command_line(&config.pre_launch_command)
        .map_err(|error| format!("Could not parse pre-launch command: {error}"))?;
    let game_args = config.effective_game_args_with_recommended(recommended_game_args);

    let mut parts = Vec::new();
    if !pre_launch_command.is_empty() {
        parts.extend(pre_launch_command);
        parts.push(launch_executable.display().to_string());
        parts.extend(game_args);
    } else {
        parts.push(launch_executable.display().to_string());
        parts.extend(game_args);
    }

    Ok(parts
        .into_iter()
        .map(|part| quote_command_part(&part))
        .collect::<Vec<_>>()
        .join(" "))
}

pub fn parse_command_line(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote = None::<char>;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' | '"' if quote.is_none() => quote = Some(ch),
            ch if Some(ch) == quote => quote = None,
            '\\' => {
                let Some(next) = chars.next() else {
                    current.push(ch);
                    continue;
                };
                current.push(next);
            }
            ch if ch.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            ch => current.push(ch),
        }
    }

    if let Some(quote) = quote {
        return Err(format!("unterminated {quote} quote"));
    }

    if !current.is_empty() {
        args.push(current);
    }

    Ok(args)
}

pub fn find_game_executable(game_dir: &Path) -> Result<PathBuf, String> {
    game_executable_names()
        .iter()
        .map(|name| game_dir.join(name))
        .find(|path| is_game_executable(path))
        .ok_or_else(|| {
            format!(
                "Could not find game executable. Tried: {}",
                game_executable_names().join(", ")
            )
        })
}

fn resolve_launch_executable(executable: &Path) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") || !executable.is_dir() {
        return Ok(executable.to_path_buf());
    }

    let macos_dir = executable.join("Contents").join("MacOS");
    let bundle_name = executable
        .file_stem()
        .and_then(|name| name.to_str())
        .map(|name| macos_dir.join(name));
    if let Some(bundle_executable) = bundle_name.filter(|path| path.is_file()) {
        return Ok(bundle_executable);
    }

    std::fs::read_dir(&macos_dir)
        .map_err(|error| format!("Could not inspect {}: {error}", macos_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_file())
        .ok_or_else(|| format!("Could not find an executable in {}", macos_dir.display()))
}

fn quote_command_part(part: &str) -> String {
    if !part.is_empty()
        && part
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '='))
    {
        return part.to_string();
    }

    format!("'{}'", part.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LaunchArgumentsMode;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn finds_primary_game_executable() {
        let temp = tempdir().unwrap();
        create_executable(&temp.path().join(game_executable_names()[0]));

        let executable = find_game_executable(temp.path()).unwrap();

        assert_eq!(executable, temp.path().join(game_executable_names()[0]));
    }

    #[test]
    fn finds_legacy_game_executable() {
        let temp = tempdir().unwrap();
        create_executable(&temp.path().join(game_executable_names()[1]));

        let executable = find_game_executable(temp.path()).unwrap();

        assert_eq!(executable, temp.path().join(game_executable_names()[1]));
    }

    #[test]
    fn reports_missing_game_executable() {
        let temp = tempdir().unwrap();

        let error = find_game_executable(temp.path()).unwrap_err();

        assert!(error.contains("Could not find game executable"));
    }

    #[test]
    fn parses_command_line_with_quotes() {
        let args = parse_command_line("--name \"Dungeon Rampage\" --enabled true").unwrap();

        assert_eq!(args, vec!["--name", "Dungeon Rampage", "--enabled", "true"]);
    }

    #[test]
    fn rejects_unclosed_quotes() {
        let error = parse_command_line("--name \"Dungeon Rampage").unwrap_err();

        assert!(error.contains("unterminated"));
    }

    #[test]
    fn launch_command_summary_uses_installed_executable_path() {
        let temp = tempdir().unwrap();
        let game_dir = paths::game_dir(temp.path());
        fs::create_dir_all(&game_dir).unwrap();
        let executable = game_dir.join(game_executable_names()[0]);
        create_executable(&executable);
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            launch_arguments_mode: LaunchArgumentsMode::Custom,
            game_args: vec!["--name".to_string(), "Dungeon Rampage".to_string()],
            ..LauncherConfig::default()
        };

        let summary = launch_command_summary_with_recommended_args(&config, &[]).unwrap();

        assert!(summary.contains(&quote_command_part(&executable.display().to_string())));
        assert!(summary.contains("'Dungeon Rampage'"));
        assert!(!summary.contains("<DRH executable>"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn captures_game_stdout_and_stderr_in_session_log() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let game_dir = paths::game_dir(temp.path());
        fs::create_dir_all(&game_dir).unwrap();
        let executable = game_dir.join(game_executable_names()[0]);
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'stdout line\\n'\nprintf 'stderr line\\n' >&2\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };

        let mut game = launch_game_with_recommended_args(&config, &[], Some("V-test")).unwrap();
        let status = game.child.wait().unwrap();
        game_logs::finish(&game.session_log, &format!("Exited with status: {status}")).unwrap();
        let contents = game_logs::read(&game.session_log.path).unwrap();

        assert!(status.success());
        assert!(contents.contains("stdout line"));
        assert!(contents.contains("stderr line"));
        assert!(contents.contains("Version: V-test"));
    }

    fn create_executable(path: &Path) {
        if cfg!(target_os = "macos") {
            fs::create_dir_all(path).unwrap();
        } else {
            fs::write(path, "").unwrap();
        }
    }
}
