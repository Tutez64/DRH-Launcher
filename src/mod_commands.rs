//! The modder commands (`--new-mod`, `--check-mod`, …): run without the UI,
//! print to the console, and exit with 0 on success.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::mods::check::check_mod;
use crate::mods::pack::pack_mod;
use crate::mods::run::run_mod;

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    CheckMod(PathBuf),
    /// The mod's folder, and where the zip and its entry go.
    PackMod(PathBuf, PathBuf),
    /// The mod's folder or zip.
    RunMod(PathBuf),
}

const USAGE: &str = "usage:
  DRH-Launcher --check-mod <dir>
  DRH-Launcher --run-mod <dir or zip>
  DRH-Launcher --pack-mod <dir> [<out-dir>]";

/// `None` when the arguments name no modder command: the launcher starts as
/// usual.
pub fn from_args(args: &[OsString]) -> Option<Result<Command, String>> {
    let name = args.first()?.to_str()?;
    let rest: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
    let command = match (name, rest.as_slice()) {
        ("--check-mod", [dir]) => Command::CheckMod(dir.clone()),
        ("--pack-mod", [dir]) => Command::PackMod(dir.clone(), PathBuf::from(".")),
        ("--pack-mod", [dir, out]) => Command::PackMod(dir.clone(), out.clone()),
        ("--run-mod", [path]) => Command::RunMod(path.clone()),
        ("--check-mod" | "--run-mod" | "--pack-mod", _) => return Some(Err(USAGE.to_string())),
        _ => return None,
    };
    Some(Ok(command))
}

/// Runs the command named by the arguments, if any, and returns its exit
/// code.
pub fn run_if_requested(args: &[OsString]) -> Option<i32> {
    let command = from_args(args)?;
    attach_console();
    Some(match command {
        Ok(command) => match run(command) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        },
        Err(usage) => {
            eprintln!("{usage}");
            2
        }
    })
}

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::CheckMod(dir) => match check_mod(&dir) {
            Ok(checked) => {
                let manifest = &checked.manifest;
                println!("{} {}: ok", manifest.id, manifest.version);
                Ok(())
            }
            Err(problems) => {
                for problem in &problems {
                    println!("{problem}");
                }
                Err(format!(
                    "{} problem(s) in {}",
                    problems.len(),
                    dir.display()
                ))
            }
        },
        Command::RunMod(path) => run_mod(&path),
        Command::PackMod(dir, out) => {
            let packed = pack_mod(&dir, &out)?;
            println!("{}", packed.zip.display());
            println!("{}", packed.entry.display());
            Ok(())
        }
    }
}

/// A release build is a GUI program on Windows: it has no console of its
/// own. Started from `DRH-Launcher.com` (the console front), it writes to that
/// one.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_modder_commands() {
        assert_eq!(
            from_args(&args(&["--check-mod", "dir"])),
            Some(Ok(Command::CheckMod(PathBuf::from("dir"))))
        );
        assert_eq!(
            from_args(&args(&["--pack-mod", "dir"])),
            Some(Ok(Command::PackMod(
                PathBuf::from("dir"),
                PathBuf::from(".")
            )))
        );
        assert!(matches!(from_args(&args(&["--check-mod"])), Some(Err(_))));
        assert!(matches!(
            from_args(&args(&["--pack-mod", "a", "b", "c"])),
            Some(Err(_))
        ));
        assert_eq!(from_args(&args(&["--play"])), None);
        assert_eq!(from_args(&args(&[])), None);
    }
}
