//! The modder commands (`--new-mod`, `--check-mod`, …): run without the UI,
//! print to the console, and exit with 0 on success.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::mods::check::check_mod;
use crate::mods::index_ci::{build_index, check_entry};
use crate::mods::new::new_mod;
use crate::mods::pack::pack_mod;
use crate::mods::run::run_mod;
use crate::mods::submit::submit_mod;

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    New(PathBuf),
    Check(PathBuf),
    /// The mod's folder, and where the zip and its entry go.
    Pack(PathBuf, PathBuf),
    /// The mod's folder or zip.
    Run(PathBuf),
    /// The mod's folder, and the account holding the index fork when it is
    /// not the repository's owner.
    Submit(PathBuf, Option<String>),
    /// Index CI: an added entry, the index it joins, a work folder, and
    /// whether a reviewer accepted a takeover.
    CheckEntry(PathBuf, PathBuf, PathBuf, bool),
    /// Index CI: the index checkout and the file to write.
    BuildIndex(PathBuf, PathBuf),
}

const USAGE: &str = "usage:
  DRH-Launcher --new-mod <dir>
  DRH-Launcher --check-mod <dir>
  DRH-Launcher --run-mod <dir or zip>
  DRH-Launcher --pack-mod <dir> [<out-dir>]
  DRH-Launcher --submit-mod <dir> [<fork-owner>]
index CI:
  DRH-Launcher --check-entry <entry.json> <base-index-dir> <work-dir> [--takeover]
  DRH-Launcher --build-index <index-dir> <out-file>";

const NAMES: [&str; 7] = [
    "--new-mod",
    "--check-mod",
    "--run-mod",
    "--pack-mod",
    "--submit-mod",
    "--check-entry",
    "--build-index",
];

/// `None` when the arguments name no modder command: the launcher starts as
/// usual.
pub fn from_args(args: &[OsString]) -> Option<Result<Command, String>> {
    let name = args.first()?.to_str()?;
    let rest: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
    if !NAMES.contains(&name) {
        return None;
    }
    let command = match (name, rest.as_slice()) {
        ("--new-mod", [dir]) => Command::New(dir.clone()),
        ("--check-mod", [dir]) => Command::Check(dir.clone()),
        ("--pack-mod", [dir]) => Command::Pack(dir.clone(), PathBuf::from(".")),
        ("--pack-mod", [dir, out]) => Command::Pack(dir.clone(), out.clone()),
        ("--run-mod", [path]) => Command::Run(path.clone()),
        ("--submit-mod", [dir]) => Command::Submit(dir.clone(), None),
        ("--submit-mod", [dir, owner]) => {
            Command::Submit(dir.clone(), Some(owner.to_string_lossy().into_owned()))
        }
        ("--check-entry", [entry, base, work]) => {
            Command::CheckEntry(entry.clone(), base.clone(), work.clone(), false)
        }
        ("--check-entry", [entry, base, work, flag]) if flag.as_os_str() == "--takeover" => {
            Command::CheckEntry(entry.clone(), base.clone(), work.clone(), true)
        }
        ("--build-index", [dir, out]) => Command::BuildIndex(dir.clone(), out.clone()),
        _ => return Some(Err(USAGE.to_string())),
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
        Command::New(dir) => new_mod(&dir, &mut std::io::stdin().lock()),
        Command::Check(dir) => match check_mod(&dir) {
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
        Command::Run(path) => run_mod(&path),
        Command::Pack(dir, out) => {
            let packed = pack_mod(&dir, &out)?;
            println!("{}", packed.zip.display());
            println!("{}", packed.entry.display());
            Ok(())
        }
        Command::Submit(dir, owner) => submit_mod(&dir, owner.as_deref()),
        Command::CheckEntry(entry, base, work, takeover) => {
            let report = check_entry(&entry, &base, &work, takeover);
            for note in &report.notes {
                println!("note: {note}");
            }
            for problem in &report.problems {
                println!("error: {problem}");
            }
            if report.problems.is_empty() {
                println!("{}: ok", entry.display());
                Ok(())
            } else {
                Err(format!(
                    "{} problem(s) in {}",
                    report.problems.len(),
                    entry.display()
                ))
            }
        }
        Command::BuildIndex(dir, out) => {
            let count = build_index(&dir, &out)?;
            println!("{}: {count} entries", out.display());
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
            Some(Ok(Command::Check(PathBuf::from("dir"))))
        );
        assert_eq!(
            from_args(&args(&["--pack-mod", "dir"])),
            Some(Ok(Command::Pack(PathBuf::from("dir"), PathBuf::from("."))))
        );
        assert_eq!(
            from_args(&args(&["--submit-mod", "dir", "me"])),
            Some(Ok(Command::Submit(
                PathBuf::from("dir"),
                Some("me".to_string())
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
