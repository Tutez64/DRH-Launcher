use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::{diagnostics, paths};

pub const MAX_GAME_SESSION_LOGS: usize = 20;

#[derive(Clone, Debug)]
pub struct GameSessionLog {
    pub path: PathBuf,
    pub started_at: SystemTime,
}

#[derive(Clone, Debug)]
pub struct GameSessionEntry {
    pub path: PathBuf,
    pub title: String,
    pub detail: String,
}

pub fn create(
    install_dir: &Path,
    version: Option<&str>,
    platform: &str,
    command: &str,
) -> Result<(File, GameSessionLog), String> {
    let logs_dir = paths::game_logs_dir(install_dir);
    fs::create_dir_all(&logs_dir)
        .map_err(|error| format!("Could not create {}: {error}", logs_dir.display()))?;

    let started_at = SystemTime::now();
    let path = unique_session_path(&logs_dir, started_at);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;

    writeln!(file, "=== Dungeon Rampage Haxe session ===")
        .and_then(|_| {
            writeln!(
                file,
                "Started: {}",
                diagnostics::format_system_time_utc(started_at)
            )
        })
        .and_then(|_| writeln!(file, "Version: {}", version.unwrap_or("unknown")))
        .and_then(|_| writeln!(file, "Platform: {platform}"))
        .and_then(|_| writeln!(file, "Command: {command}"))
        .and_then(|_| writeln!(file, "--- Game output ---"))
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    file.flush()
        .map_err(|error| format!("Could not flush {}: {error}", path.display()))?;

    let session = GameSessionLog { path, started_at };
    let _ = prune(install_dir, Some(&session.path), MAX_GAME_SESSION_LOGS);
    Ok((file, session))
}

pub fn finish(session: &GameSessionLog, result: &str) -> Result<(), String> {
    let ended_at = SystemTime::now();
    let duration = ended_at
        .duration_since(session.started_at)
        .unwrap_or_default();
    let mut file = OpenOptions::new()
        .append(true)
        .open(&session.path)
        .map_err(|error| format!("Could not open {}: {error}", session.path.display()))?;

    writeln!(file)
        .and_then(|_| writeln!(file, "--- Session finished ---"))
        .and_then(|_| {
            writeln!(
                file,
                "Ended: {}",
                diagnostics::format_system_time_utc(ended_at)
            )
        })
        .and_then(|_| writeln!(file, "Duration: {}", format_duration(duration)))
        .and_then(|_| writeln!(file, "Result: {result}"))
        .map_err(|error| format!("Could not finish {}: {error}", session.path.display()))
}

pub fn list(install_dir: &Path) -> Result<Vec<GameSessionEntry>, String> {
    let logs_dir = paths::game_logs_dir(install_dir);
    if !logs_dir.exists() {
        return Ok(Vec::new());
    }

    let mut paths = session_paths(&logs_dir)?;
    paths.sort_by(|left, right| right.file_name().cmp(&left.file_name()));

    paths.into_iter().map(session_entry).collect()
}

pub fn read(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn session_entry(path: PathBuf) -> Result<GameSessionEntry, String> {
    let mut file =
        File::open(&path).map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    let mut header = vec![0; 8 * 1024];
    let read = file
        .read(&mut header)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let header = String::from_utf8_lossy(&header[..read]);
    let started = header_value(&header, "Started: ").unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Unknown session")
            .to_string()
    });
    let version = header_value(&header, "Version: ").unwrap_or_else(|| "unknown".to_string());
    let size = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?
        .len();

    Ok(GameSessionEntry {
        path,
        title: started,
        detail: format!("{version} · {}", format_file_size(size)),
    })
}

fn header_value(contents: &str, prefix: &str) -> Option<String> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(str::to_string)
}

fn unique_session_path(logs_dir: &Path, started_at: SystemTime) -> PathBuf {
    let timestamp = diagnostics::filename_timestamp_utc(started_at);
    let base = logs_dir.join(format!("{timestamp}.log"));
    if !base.exists() {
        return base;
    }

    for suffix in 2.. {
        let candidate = logs_dir.join(format!("{timestamp}-{suffix}.log"));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!()
}

fn prune(install_dir: &Path, preserve: Option<&Path>, max_logs: usize) -> Result<(), String> {
    let logs_dir = paths::game_logs_dir(install_dir);
    let mut paths = session_paths(&logs_dir)?;
    paths.sort_by(|left, right| right.file_name().cmp(&left.file_name()));

    let preserve_exists =
        preserve.is_some_and(|preserve| paths.iter().any(|path| path == preserve));
    let mut kept = usize::from(preserve_exists);
    for path in paths {
        if preserve.is_some_and(|preserve| preserve == path) {
            continue;
        }
        if kept < max_logs {
            kept += 1;
            continue;
        }

        fs::remove_file(&path).map_err(|error| {
            format!("Could not remove old game log {}: {error}", path.display())
        })?;
    }

    Ok(())
}

fn session_paths(logs_dir: &Path) -> Result<Vec<PathBuf>, String> {
    fs::read_dir(logs_dir)
        .map_err(|error| format!("Could not read {}: {error}", logs_dir.display()))?
        .filter_map(|entry| match entry {
            Ok(entry)
                if entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("log") =>
            {
                Some(Ok(entry.path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(format!(
                "Could not read an entry in {}: {error}",
                logs_dir.display()
            ))),
        })
        .collect()
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn format_file_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_lists_reads_and_finishes_session_logs() {
        let temp = tempdir().unwrap();
        let (mut file, session) =
            create(temp.path(), Some("V9"), "linux-x64", "game --flag").unwrap();
        writeln!(file, "[INFO] Game started").unwrap();
        drop(file);
        finish(&session, "Exited with status 0").unwrap();

        let sessions = list(temp.path()).unwrap();
        let contents = read(&session.path).unwrap();

        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].detail.starts_with("V9 · "));
        assert!(contents.contains("Command: game --flag"));
        assert!(contents.contains("[INFO] Game started"));
        assert!(contents.contains("Result: Exited with status 0"));
    }

    #[test]
    fn keeps_only_the_most_recent_session_logs() {
        let temp = tempdir().unwrap();
        let logs_dir = paths::game_logs_dir(temp.path());
        fs::create_dir_all(&logs_dir).unwrap();
        for index in 0..5 {
            fs::write(logs_dir.join(format!("2026-01-01-00-00-0{index}Z.log")), "").unwrap();
        }

        prune(temp.path(), None, 3).unwrap();

        let sessions = session_paths(&logs_dir).unwrap();
        assert_eq!(sessions.len(), 3);
        assert!(logs_dir.join("2026-01-01-00-00-04Z.log").exists());
    }
}
