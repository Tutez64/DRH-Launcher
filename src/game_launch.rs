use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::config::LauncherConfig;
use crate::game_install::{game_executable_names, is_game_executable};
use crate::{game_logs, paths, platform::Platform};

pub struct RunningGame {
    pub child: Child,
    pub session_log: game_logs::GameSessionLog,
}

#[cfg(unix)]
pub fn request_graceful_shutdown(child: &Child) -> Result<(), String> {
    let result = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "Could not send SIGTERM to DRH: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(windows)]
pub fn request_graceful_shutdown(child: &Child) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };

    struct WindowSearch {
        process_id: u32,
        close_sent: bool,
    }

    unsafe extern "system" fn close_process_window(window: HWND, data: LPARAM) -> i32 {
        let search = unsafe { &mut *(data as *mut WindowSearch) };
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut process_id);
        }
        if process_id == search.process_id && unsafe { PostMessageW(window, WM_CLOSE, 0, 0) } != 0 {
            search.close_sent = true;
        }
        1
    }

    let mut search = WindowSearch {
        process_id: child.id(),
        close_sent: false,
    };
    unsafe {
        EnumWindows(
            Some(close_process_window),
            (&mut search as *mut WindowSearch) as LPARAM,
        );
    }

    if search.close_sent {
        Ok(())
    } else {
        Err("Could not find a DRH window to close.".to_string())
    }
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
    configure_game_process_environment(&mut command, &launch_executable);
    command.stdout(Stdio::from(log_file));
    command.stderr(Stdio::from(stderr_file));
    let child = command.spawn().map_err(|error| {
        let _ = game_logs::finish(&session_log, &format!("Launch failed: {error}"));
        format!("Could not launch DRH: {error}")
    })?;

    Ok(RunningGame { child, session_log })
}

fn configure_game_process_environment(command: &mut Command, launch_executable: &Path) {
    if !cfg!(target_os = "linux") {
        return;
    }

    clear_parent_desktop_identity(command);
    clear_parent_appimage_environment(command, env::var_os("APPDIR").as_deref().map(Path::new));

    let Some(window_class) = launch_executable.file_name() else {
        return;
    };
    command.env("SDL_VIDEO_WAYLAND_WMCLASS", window_class);
    command.env("SDL_VIDEO_X11_WMCLASS", window_class);
}

fn clear_parent_desktop_identity(command: &mut Command) {
    // Do not let desktop environments associate DRH's window with DRHL's .desktop file.
    for name in [
        "BAMF_DESKTOP_FILE_HINT",
        "DESKTOP_STARTUP_ID",
        "GIO_LAUNCHED_DESKTOP_FILE",
        "GIO_LAUNCHED_DESKTOP_FILE_PID",
        "KSTARTUPID",
        "XDG_ACTIVATION_TOKEN",
    ] {
        command.env_remove(name);
    }
}

fn clear_parent_appimage_environment(command: &mut Command, appdir: Option<&Path>) {
    // AppImage runtime variables describe DRHL's mounted image, not the game process.
    for name in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
        command.env_remove(name);
    }

    let Some(appdir) = appdir else {
        return;
    };
    remove_path_entries_under(command, "LD_LIBRARY_PATH", appdir);
    remove_path_entries_under(command, "PATH", appdir);
}

fn remove_path_entries_under(command: &mut Command, name: &str, parent: &Path) {
    let Some(value) = env::var_os(name) else {
        return;
    };
    match path_entries_without_parent(&value, parent) {
        Some(value) => command.env(name, value),
        None => command.env_remove(name),
    };
}

fn path_entries_without_parent(value: &OsStr, parent: &Path) -> Option<OsString> {
    let entries = env::split_paths(value)
        .filter(|entry| !entry.starts_with(parent))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        None
    } else {
        env::join_paths(entries).ok()
    }
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
        ensure_executable_permission(&bundle_executable)?;
        return Ok(bundle_executable);
    }

    let bundle_executable = std::fs::read_dir(&macos_dir)
        .map_err(|error| format!("Could not inspect {}: {error}", macos_dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_file())
        .ok_or_else(|| format!("Could not find an executable in {}", macos_dir.display()))?;
    ensure_executable_permission(&bundle_executable)?;
    Ok(bundle_executable)
}

#[cfg(unix)]
fn ensure_executable_permission(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    let mode = metadata.permissions().mode();
    if mode & 0o100 != 0 {
        return Ok(());
    }

    let mut permissions = metadata.permissions();
    permissions.set_mode(mode | 0o100);
    std::fs::set_permissions(path, permissions)
        .map_err(|error| format!("Could not mark {} as executable: {error}", path.display()))
}

#[cfg(not(unix))]
fn ensure_executable_permission(_path: &Path) -> Result<(), String> {
    Ok(())
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
    fn linux_launch_sets_game_desktop_identity() {
        let mut command = Command::new("DRH.exe");
        let launch_executable = Path::new("/games/Dungeon Rampage Haxe");

        configure_game_process_environment(&mut command, launch_executable);

        let removed = command
            .get_envs()
            .filter_map(|(name, value)| value.is_none().then_some(name))
            .collect::<Vec<_>>();
        let assigned = command
            .get_envs()
            .filter_map(|(name, value)| value.map(|value| (name, value)))
            .collect::<Vec<_>>();

        if cfg!(target_os = "linux") {
            assert!(removed.contains(&std::ffi::OsStr::new("APPIMAGE")));
            assert!(removed.contains(&std::ffi::OsStr::new("APPDIR")));
            assert!(removed.contains(&std::ffi::OsStr::new("ARGV0")));
            assert!(removed.contains(&std::ffi::OsStr::new("OWD")));
            assert!(removed.contains(&std::ffi::OsStr::new("DESKTOP_STARTUP_ID")));
            assert!(removed.contains(&std::ffi::OsStr::new("GIO_LAUNCHED_DESKTOP_FILE")));
            assert!(removed.contains(&std::ffi::OsStr::new("XDG_ACTIVATION_TOKEN")));
            assert!(assigned.contains(&(
                std::ffi::OsStr::new("SDL_VIDEO_WAYLAND_WMCLASS"),
                std::ffi::OsStr::new("Dungeon Rampage Haxe")
            )));
            assert!(assigned.contains(&(
                std::ffi::OsStr::new("SDL_VIDEO_X11_WMCLASS"),
                std::ffi::OsStr::new("Dungeon Rampage Haxe")
            )));
        } else {
            assert!(removed.is_empty());
            assert!(assigned.is_empty());
        }
    }

    #[test]
    fn removes_appimage_paths_from_path_like_environment_values() {
        let appdir = Path::new("/tmp/DRHL.AppDir");
        let value = std::env::join_paths([
            Path::new("/usr/bin"),
            Path::new("/tmp/DRHL.AppDir/usr/bin"),
            Path::new("/tmp/DRHL.AppDir/usr/lib"),
            Path::new("/opt/game/bin"),
        ])
        .unwrap();

        let filtered = path_entries_without_parent(&value, appdir).unwrap();
        let entries = std::env::split_paths(&filtered).collect::<Vec<_>>();

        assert_eq!(
            entries,
            vec![Path::new("/usr/bin"), Path::new("/opt/game/bin")]
        );
    }

    #[test]
    fn removes_empty_appimage_path_like_environment_values() {
        let appdir = Path::new("/tmp/DRHL.AppDir");
        let value = std::env::join_paths([
            Path::new("/tmp/DRHL.AppDir/usr/bin"),
            Path::new("/tmp/DRHL.AppDir/usr/lib"),
        ])
        .unwrap();

        assert!(path_entries_without_parent(&value, appdir).is_none());
    }

    #[test]
    #[cfg(unix)]
    fn repairs_missing_owner_execute_permission() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let executable = temp.path().join("Dungeon Rampage Haxe");
        fs::write(&executable, "").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();

        ensure_executable_permission(&executable).unwrap();

        let mode = fs::metadata(&executable).unwrap().permissions().mode();
        assert_ne!(mode & 0o100, 0);
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
        let log_path =
            game_logs::finish(&game.session_log, &format!("Exited with status: {status}")).unwrap();
        let contents = game_logs::read(&log_path).unwrap();

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
