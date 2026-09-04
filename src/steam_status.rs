#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Mutex;

use crate::steam_buildid::STEAM_APP_ID;

static STEAM_STATUS: Mutex<SteamStatus> = Mutex::new(SteamStatus::not_detected());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SteamStatus {
    pub steam_installed: bool,
    pub steam_running: bool,
    pub official_game_installed: bool,
}

impl SteamStatus {
    const fn not_detected() -> Self {
        Self {
            steam_installed: false,
            steam_running: false,
            official_game_installed: false,
        }
    }

    pub fn probe() -> Self {
        let steam_dirs = steam_dirs();
        Self {
            steam_installed: !steam_dirs.is_empty(),
            steam_running: steam_is_running(),
            official_game_installed: official_game_is_installed(&steam_dirs),
        }
    }
}

pub fn cached_status() -> SteamStatus {
    *lock_steam_status()
}

pub fn refresh_status() -> SteamStatus {
    let status = SteamStatus::probe();
    *lock_steam_status() = status;
    status
}

fn lock_steam_status() -> std::sync::MutexGuard<'static, SteamStatus> {
    STEAM_STATUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn steam_dirs() -> Vec<steamlocate::SteamDir> {
    steamlocate::locate_all().unwrap_or_default()
}

fn official_game_is_installed(steam_dirs: &[steamlocate::SteamDir]) -> bool {
    steam_dirs
        .iter()
        .any(|steam_dir| steam_dir.find_app(STEAM_APP_ID).ok().flatten().is_some())
}

fn steam_is_running() -> bool {
    #[cfg(unix)]
    if steam_pid_is_running() {
        return true;
    }
    steam_process_is_running()
}

#[cfg(unix)]
fn steam_pid_is_running() -> bool {
    steam_pid_files().iter().any(|path| {
        fs::read_to_string(path)
            .ok()
            .and_then(|contents| contents.trim().parse::<i32>().ok())
            .is_some_and(pid_is_alive)
    })
}

#[cfg(unix)]
fn steam_pid_files() -> Vec<PathBuf> {
    let mut pid_files = Vec::new();
    if let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
        pid_files.push(home.join(".steam/steam.pid"));
        pid_files.push(home.join(".steam/steam/steam.pid"));
        pid_files.push(home.join(".var/app/com.valvesoftware.Steam/.steam/steam.pid"));
    }
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        pid_files.push(PathBuf::from(runtime_dir).join("steam/steam.pid"));
    }
    pid_files
}

#[cfg(unix)]
fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(target_os = "linux")]
fn steam_process_is_running() -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.chars().all(|character| character.is_ascii_digit()))
            && fs::read_to_string(entry.path().join("comm"))
                .ok()
                .is_some_and(|comm| is_linux_steam_comm(&comm))
    })
}

#[cfg(any(target_os = "linux", test))]
fn is_linux_steam_comm(comm: &str) -> bool {
    comm.trim() == "steam"
}

#[cfg(target_os = "macos")]
fn steam_process_is_running() -> bool {
    ["steam", "steam_osx"].iter().any(|name| {
        std::process::Command::new("pgrep")
            .args(["-x", name])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

#[cfg(windows)]
fn steam_process_is_running() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == 0 || snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry = std::mem::zeroed::<PROCESSENTRY32W>();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut running = false;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if is_windows_steam_exe(&wide_c_string(&entry.szExeFile)) {
                    running = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        running
    }
}

#[cfg(windows)]
fn wide_c_string(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

#[cfg(any(windows, test))]
fn is_windows_steam_exe(name: &str) -> bool {
    name.eq_ignore_ascii_case("steam.exe")
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn steam_process_is_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_steam_process_names() {
        assert!(is_linux_steam_comm("steam\n"));
        assert!(!is_linux_steam_comm("steamwebhelper"));
        assert!(is_windows_steam_exe("steam.exe"));
        assert!(is_windows_steam_exe("Steam.exe"));
        assert!(!is_windows_steam_exe("steamwebhelper.exe"));
    }

    #[test]
    fn probe_does_not_panic() {
        let _ = SteamStatus::probe();
    }

    #[cfg(unix)]
    #[test]
    fn current_process_pid_is_alive() {
        let pid = i32::try_from(std::process::id()).expect("pid fits i32");
        assert!(pid_is_alive(pid));
        assert!(!pid_is_alive(0));
        assert!(!pid_is_alive(-1));
    }
}
