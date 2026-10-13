// Installed on Windows next to `DRH-Launcher.exe` as `DRH-Launcher.com`. A
// shell picks `.com` before `.exe` for a name typed without extension, waits
// for it as a console program, and reads its exit code; the GUI-subsystem
// `.exe` gets none of that. This front only runs the `.exe` with the same
// arguments in the same console and returns its exit code.

use std::process::Command;

fn main() {
    let launcher = match std::env::current_exe() {
        Ok(path) => path.with_file_name(if cfg!(windows) {
            "DRH-Launcher.exe"
        } else {
            "DRH-Launcher"
        }),
        Err(error) => {
            eprintln!("DRH Launcher: cannot locate itself: {error}");
            std::process::exit(1);
        }
    };

    // Ctrl+C reaches every process of the console: the launcher handles it,
    // this front keeps waiting for it.
    #[cfg(windows)]
    ignore_ctrl_c();

    match Command::new(&launcher)
        .args(std::env::args_os().skip(1))
        .status()
    {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("DRH Launcher: cannot start {}: {error}", launcher.display());
            std::process::exit(1);
        }
    }
}

#[cfg(windows)]
fn ignore_ctrl_c() {
    use windows_sys::Win32::Foundation::{BOOL, TRUE};
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    // A handler, not `SetConsoleCtrlHandler(None, TRUE)`: ignoring Ctrl+C that
    // way is inherited by the launcher.
    unsafe extern "system" fn handled(_: u32) -> BOOL {
        TRUE
    }
    unsafe {
        SetConsoleCtrlHandler(Some(handled), TRUE);
    }
}
