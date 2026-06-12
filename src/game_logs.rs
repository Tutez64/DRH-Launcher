use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::{diagnostics, paths};

const ZSTD_COMPRESSION_LEVEL: i32 = 10;

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

    Ok((file, GameSessionLog { path, started_at }))
}

pub fn finish(session: &GameSessionLog, result: &str) -> Result<PathBuf, String> {
    let ended_at = SystemTime::now();
    let duration = ended_at
        .duration_since(session.started_at)
        .unwrap_or_default();
    {
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
            .and_then(|_| file.flush())
            .map_err(|error| format!("Could not finish {}: {error}", session.path.display()))?;
    }

    compress(&session.path)
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
    let bytes = read_bytes(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn path_for_external_open(path: &Path) -> Result<PathBuf, String> {
    if !is_compressed_path(path) {
        return Ok(path.to_path_buf());
    }

    let export_dir = std::env::temp_dir().join("drh-launcher").join("game-logs");
    fs::create_dir_all(&export_dir)
        .map_err(|error| format!("Could not create {}: {error}", export_dir.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".zst"))
        .ok_or_else(|| format!("Invalid compressed game log path: {}", path.display()))?;
    let export_path = export_dir.join(file_name);
    let temporary_path = appended_extension(&export_path, ".tmp");

    fs::write(&temporary_path, read_bytes(path)?)
        .map_err(|error| format!("Could not write {}: {error}", temporary_path.display()))?;
    replace_file(&temporary_path, &export_path)?;
    Ok(export_path)
}

fn session_entry(path: PathBuf) -> Result<GameSessionEntry, String> {
    if is_compressed_path(&path) {
        let size = fs::metadata(&path)
            .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?
            .len();
        let contents = read(&path)?;
        return Ok(session_entry_from_sections(
            path, size, &contents, &contents,
        ));
    }

    let mut file =
        File::open(&path).map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    let size = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?
        .len();
    let header = read_file_section(&mut file, 0, 8 * 1024, &path)?;
    let tail_offset = size.saturating_sub(8 * 1024);
    let tail = read_file_section(&mut file, tail_offset, 8 * 1024, &path)?;

    Ok(session_entry_from_sections(path, size, &header, &tail))
}

fn session_entry_from_sections(
    path: PathBuf,
    size: u64,
    header: &str,
    tail: &str,
) -> GameSessionEntry {
    let started = header_value(header, "Started: ").unwrap_or_else(|| {
        session_name(&path)
            .and_then(|name| name.into_string().ok())
            .unwrap_or_else(|| "Unknown session".to_string())
    });
    let version = header_value(header, "Version: ").unwrap_or_else(|| "unknown".to_string());
    let duration = header_value(tail, "Duration: ").unwrap_or_else(|| "in progress".to_string());

    GameSessionEntry {
        path,
        title: started,
        detail: format!("{version} · {duration} · {}", format_file_size(size)),
    }
}

fn read_file_section(
    file: &mut File,
    offset: u64,
    max_bytes: usize,
    path: &Path,
) -> Result<String, String> {
    file.seek(std::io::SeekFrom::Start(offset))
        .map_err(|error| format!("Could not seek {}: {error}", path.display()))?;
    let mut bytes = vec![0; max_bytes];
    let read = file
        .read(&mut bytes)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes[..read]).into_owned())
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
    if session_path_is_available(&base) {
        return base;
    }

    for suffix in 2.. {
        let candidate = logs_dir.join(format!("{timestamp}-{suffix}.log"));
        if session_path_is_available(&candidate) {
            return candidate;
        }
    }

    unreachable!()
}

fn session_path_is_available(path: &Path) -> bool {
    !path.exists() && !appended_extension(path, ".zst").exists()
}

fn session_paths(logs_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut sessions = BTreeMap::<OsString, PathBuf>::new();
    for entry in fs::read_dir(logs_dir)
        .map_err(|error| format!("Could not read {}: {error}", logs_dir.display()))?
    {
        let entry = entry.map_err(|error| {
            format!("Could not read an entry in {}: {error}", logs_dir.display())
        })?;
        let path = entry.path();
        let Some(session_name) = session_name(&path) else {
            continue;
        };

        match sessions.get(&session_name) {
            Some(existing) if is_compressed_path(existing) => {}
            _ => {
                sessions.insert(session_name, path);
            }
        }
    }

    Ok(sessions.into_values().collect())
}

fn session_name(path: &Path) -> Option<OsString> {
    let file_name = path.file_name()?;
    if path.extension().and_then(|extension| extension.to_str()) == Some("log") {
        return Some(file_name.to_os_string());
    }
    if is_compressed_path(path) {
        return path.file_stem().map(|name| name.to_os_string());
    }
    None
}

fn is_compressed_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".log.zst"))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    if !is_compressed_path(path) {
        return fs::read(path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()));
    }

    let file =
        File::open(path).map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    let mut decoder = zstd::stream::read::Decoder::new(file)
        .map_err(|error| format!("Could not decompress {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not decompress {}: {error}", path.display()))?;
    Ok(bytes)
}

fn compress(path: &Path) -> Result<PathBuf, String> {
    let compressed_path = appended_extension(path, ".zst");
    let temporary_path = appended_extension(&compressed_path, ".tmp");

    let result = (|| {
        let input = File::open(path)
            .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
        let output = File::create(&temporary_path)
            .map_err(|error| format!("Could not create {}: {error}", temporary_path.display()))?;
        let mut encoder = zstd::stream::write::Encoder::new(output, ZSTD_COMPRESSION_LEVEL)
            .map_err(|error| format!("Could not compress {}: {error}", path.display()))?;
        std::io::copy(&mut BufReader::new(input), &mut encoder)
            .map_err(|error| format!("Could not compress {}: {error}", path.display()))?;
        let output = encoder
            .finish()
            .map_err(|error| format!("Could not finish compressing {}: {error}", path.display()))?;
        output
            .sync_all()
            .map_err(|error| format!("Could not flush {}: {error}", temporary_path.display()))?;

        if compressed_path.exists() {
            return Err(format!(
                "Could not create {}: file already exists",
                compressed_path.display()
            ));
        }
        fs::rename(&temporary_path, &compressed_path).map_err(|error| {
            format!(
                "Could not move {} to {}: {error}",
                temporary_path.display(),
                compressed_path.display()
            )
        })?;
        if let Err(error) = fs::remove_file(path) {
            let _ = fs::remove_file(&compressed_path);
            return Err(format!("Could not remove {}: {error}", path.display()));
        }
        Ok(compressed_path.clone())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|error| format!("Could not replace {}: {error}", destination.display()))?;
    }
    fs::rename(source, destination).map_err(|error| {
        format!(
            "Could not move {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })
}

fn appended_extension(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
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
        let compressed_path = finish(&session, "Exited with status 0").unwrap();

        let sessions = list(temp.path()).unwrap();
        let contents = read(&compressed_path).unwrap();

        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].detail.starts_with("V9 · 0s · "));
        assert_eq!(sessions[0].path, compressed_path);
        assert!(!session.path.exists());
        assert_eq!(
            compressed_path
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("zst")
        );
        assert!(contents.contains("Command: game --flag"));
        assert!(contents.contains("[INFO] Game started"));
        assert!(contents.contains("Result: Exited with status 0"));
    }

    #[test]
    fn marks_unfinished_sessions_as_in_progress() {
        let temp = tempdir().unwrap();
        let (_file, session) = create(temp.path(), Some("V9"), "linux-x64", "game").unwrap();

        let sessions = list(temp.path()).unwrap();

        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].detail.starts_with("V9 · in progress · "));
        assert_eq!(sessions[0].path, session.path);
    }

    #[test]
    fn prefers_completed_archive_when_both_session_states_exist() {
        let temp = tempdir().unwrap();
        let logs_dir = paths::game_logs_dir(temp.path());
        fs::create_dir_all(&logs_dir).unwrap();
        let active = logs_dir.join("2026-01-01-00-00-00Z.log");
        fs::write(&active, "incomplete").unwrap();
        let duplicate = appended_extension(&active, ".zst");
        let output = File::create(&duplicate).unwrap();
        let mut encoder =
            zstd::stream::write::Encoder::new(output, ZSTD_COMPRESSION_LEVEL).unwrap();
        encoder.write_all(b"compressed").unwrap();
        encoder.finish().unwrap();
        fs::write(logs_dir.join("not-a-session.txt"), "").unwrap();

        let sessions = session_paths(&logs_dir).unwrap();
        assert_eq!(sessions, vec![duplicate]);
    }

    #[test]
    fn materializes_compressed_logs_for_external_opening() {
        let temp = tempdir().unwrap();
        let (mut file, session) = create(temp.path(), None, "linux-x64", "game").unwrap();
        writeln!(file, "[WARN] Test line").unwrap();
        drop(file);
        let compressed_path = finish(&session, "Done").unwrap();

        let external_path = path_for_external_open(&compressed_path).unwrap();
        let contents = fs::read_to_string(&external_path).unwrap();

        assert_eq!(
            external_path
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("log")
        );
        assert!(contents.contains("[WARN] Test line"));
    }

    #[test]
    fn does_not_reuse_a_compressed_session_name() {
        let temp = tempdir().unwrap();
        let started_at = SystemTime::UNIX_EPOCH;
        let first_path = unique_session_path(temp.path(), started_at);
        fs::write(appended_extension(&first_path, ".zst"), "").unwrap();

        let next_path = unique_session_path(temp.path(), started_at);

        assert_ne!(next_path, first_path);
        assert!(next_path.ends_with("1970-01-01-00-00-00Z-2.log"));
    }
}
