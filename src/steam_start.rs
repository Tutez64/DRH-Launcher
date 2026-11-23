//! Starts the Steam client for the player, from the Home notice or as the
//! launcher opens.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::game_launch;
use crate::steam_status;

/// How long Home says Steam is starting before it offers again to start it.
const START_GRACE: Duration = Duration::from_secs(45);
/// Starts Steam in the background: no main window, only what it has to show.
const SILENT_ARGUMENT: &str = "-silent";
/// Last resort, through the system's handler for `steam://`: it cannot ask
/// for a silent start.
const STEAM_CLIENT_URL: &str = "steam://open/main";

static START_REQUESTED: Mutex<Option<Instant>> = Mutex::new(None);

/// Records that Steam is being started. False when it already is.
pub(crate) fn begin() -> bool {
    if is_starting() {
        return false;
    }
    *lock_start_requested() = Some(Instant::now());
    true
}

/// Forgets the start in progress, for one that could not be launched.
pub(crate) fn cancel() {
    *lock_start_requested() = None;
}

/// Whether Steam was asked to start and is not seen running yet.
pub(crate) fn is_starting() -> bool {
    let mut requested = lock_start_requested();
    let starting = still_starting(
        *requested,
        steam_status::cached_status().steam_running,
        Instant::now(),
    );
    if !starting {
        *requested = None;
    }
    starting
}

fn still_starting(requested: Option<Instant>, steam_running: bool, now: Instant) -> bool {
    requested.is_some_and(|at| !steam_running && now.saturating_duration_since(at) < START_GRACE)
}

/// Launches Steam and returns the command that did it.
pub(crate) fn launch() -> Result<String, String> {
    let steam_dirs = steamlocate::locate_all()
        .unwrap_or_default()
        .iter()
        .map(|steam_dir| steam_dir.path().to_path_buf())
        .collect::<Vec<_>>();
    let mut failures = Vec::new();
    for command in start_commands(&steam_dirs) {
        match command.spawn() {
            Ok(()) => return Ok(command.summary()),
            Err(error) => failures.push(format!("{}: {error}", command.summary())),
        }
    }
    Err(failures.join("; "))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StartCommand {
    program: OsString,
    args: Vec<String>,
}

impl StartCommand {
    fn new(program: impl Into<OsString>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
        }
    }

    fn summary(&self) -> String {
        let mut summary = self.program.to_string_lossy().into_owned();
        for arg in &self.args {
            summary.push(' ');
            summary.push_str(arg);
        }
        summary
    }

    fn spawn(&self) -> io::Result<()> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        game_launch::clear_launcher_environment(&mut command);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            // Steam outlives the launcher: a Ctrl+C in the launcher's
            // terminal must not reach it.
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        // Reaped when it exits: left as a zombie, a Steam the player has
        // quit would still be seen running.
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// The commands that start Steam, to try in order until one can be run.
fn start_commands(steam_dirs: &[PathBuf]) -> Vec<StartCommand> {
    let mut commands = Vec::new();
    for command in installed_client_commands(steam_dirs) {
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    commands.push(url_handler_command());
    commands
}

#[cfg(target_os = "linux")]
fn installed_client_commands(steam_dirs: &[PathBuf]) -> Vec<StartCommand> {
    let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    steam_dirs
        .iter()
        .map(|steam_dir| linux_client_command(steam_dir, home.as_deref()))
        .collect()
}

/// A Steam folder says how that Steam was installed, and so what starts it.
#[cfg(any(target_os = "linux", test))]
fn linux_client_command(steam_dir: &Path, home: Option<&Path>) -> StartCommand {
    const FLATPAK_APP: &str = "com.valvesoftware.Steam";

    let under_home =
        |relative: &str| home.is_some_and(|home| steam_dir.starts_with(home.join(relative)));
    if under_home(".var/app/com.valvesoftware.Steam") {
        StartCommand::new("flatpak", &["run", FLATPAK_APP, SILENT_ARGUMENT])
    } else if under_home("snap/steam") {
        StartCommand::new("snap", &["run", "steam", SILENT_ARGUMENT])
    } else {
        StartCommand::new("steam", &[SILENT_ARGUMENT])
    }
}

#[cfg(windows)]
fn installed_client_commands(steam_dirs: &[PathBuf]) -> Vec<StartCommand> {
    steam_dirs
        .iter()
        .map(|steam_dir| windows_client_command(steam_dir))
        .collect()
}

#[cfg(any(windows, test))]
fn windows_client_command(steam_dir: &Path) -> StartCommand {
    StartCommand::new(steam_dir.join("steam.exe"), &[SILENT_ARGUMENT])
}

#[cfg(target_os = "macos")]
fn installed_client_commands(_steam_dirs: &[PathBuf]) -> Vec<StartCommand> {
    // `-g` keeps Steam from coming in front of the launcher.
    vec![StartCommand::new(
        "open",
        &["-g", "-a", "Steam", "--args", SILENT_ARGUMENT],
    )]
}

fn url_handler_command() -> StartCommand {
    if cfg!(target_os = "windows") {
        StartCommand::new(
            "rundll32",
            &["url.dll,FileProtocolHandler", STEAM_CLIENT_URL],
        )
    } else if cfg!(target_os = "macos") {
        StartCommand::new("open", &[STEAM_CLIENT_URL])
    } else {
        StartCommand::new("xdg-open", &[STEAM_CLIENT_URL])
    }
}

fn lock_start_requested() -> std::sync::MutexGuard<'static, Option<Instant>> {
    START_REQUESTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_lasts_until_steam_runs_or_the_grace_ends() {
        let requested = Instant::now();
        let soon = requested + Duration::from_secs(5);

        assert!(!still_starting(None, false, soon));
        assert!(still_starting(Some(requested), false, soon));
        assert!(!still_starting(Some(requested), true, soon));
        assert!(!still_starting(
            Some(requested),
            false,
            requested + START_GRACE
        ));
    }

    #[test]
    fn linux_command_follows_how_steam_was_installed() {
        let home = Path::new("/home/player");

        let native = linux_client_command(
            Path::new("/home/player/.steam/debian-installation"),
            Some(home),
        );
        assert_eq!(native.summary(), "steam -silent");

        let flatpak = linux_client_command(
            Path::new("/home/player/.var/app/com.valvesoftware.Steam/.local/share/Steam"),
            Some(home),
        );
        assert_eq!(
            flatpak.summary(),
            "flatpak run com.valvesoftware.Steam -silent"
        );

        let snap = linux_client_command(
            Path::new("/home/player/snap/steam/common/.local/share/Steam"),
            Some(home),
        );
        assert_eq!(snap.summary(), "snap run steam -silent");

        let no_home = linux_client_command(
            Path::new("/home/player/snap/steam/common/.steam/root"),
            None,
        );
        assert_eq!(no_home.summary(), "steam -silent");
    }

    #[test]
    fn windows_command_runs_the_client_of_the_steam_folder() {
        let command = windows_client_command(Path::new("steam-folder"));
        assert_eq!(
            PathBuf::from(&command.program),
            Path::new("steam-folder").join("steam.exe")
        );
        assert_eq!(command.args, ["-silent"]);
    }

    #[test]
    fn url_handler_is_tried_last_and_installs_once() {
        let dirs = [PathBuf::from("first"), PathBuf::from("first")];
        let commands = start_commands(&dirs);
        assert_eq!(commands.last(), Some(&url_handler_command()));
        assert!(commands.len() <= 2);
    }
}
