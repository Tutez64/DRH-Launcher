use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;

#[derive(Clone, Copy, Debug)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

pub fn launcher_log_file(install_dir: &Path) -> std::path::PathBuf {
    paths::launcher_log_file(install_dir)
}

pub fn write(install_dir: &Path, level: LogLevel, message: &str) -> Result<(), String> {
    let log_file = launcher_log_file(install_dir);
    if let Some(parent) = log_file.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
        .map_err(|error| format!("Could not open {}: {error}", log_file.display()))?;

    writeln!(
        file,
        "{} [{}] {}",
        timestamp(),
        level.as_str(),
        message.replace('\n', "\\n")
    )
    .map_err(|error| format!("Could not write {}: {error}", log_file.display()))
}

pub fn read_recent(install_dir: &Path) -> Result<String, String> {
    const MAX_BYTES: usize = 24 * 1024;

    let log_file = launcher_log_file(install_dir);
    if !log_file.exists() {
        return Ok("No log entries yet.".to_string());
    }

    let mut file = OpenOptions::new()
        .read(true)
        .open(&log_file)
        .map_err(|error| format!("Could not open {}: {error}", log_file.display()))?;

    let len = file
        .metadata()
        .map_err(|error| format!("Could not inspect {}: {error}", log_file.display()))?
        .len();
    if len > MAX_BYTES as u64 {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(len - MAX_BYTES as u64))
            .map_err(|error| format!("Could not seek {}: {error}", log_file.display()))?;
    }

    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|error| format!("Could not read {}: {error}", log_file.display()))?;

    if len > MAX_BYTES as u64 {
        if let Some((_, rest)) = contents.split_once('\n') {
            contents = rest.to_string();
        }
    }

    if contents.trim().is_empty() {
        Ok("No log entries yet.".to_string())
    } else {
        Ok(contents)
    }
}

pub fn timestamp() -> String {
    format_system_time_utc(SystemTime::now())
}

pub fn format_system_time_utc(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_utc_timestamp(seconds)
}

pub fn filename_timestamp_utc(time: SystemTime) -> String {
    format_system_time_utc(time)
        .replace([':', ' '], "-")
        .replace("-UTC", "Z")
}

fn format_utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

// Howard Hinnant's civil_from_days algorithm, with days counted from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };

    (year, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writes_and_reads_recent_log_entries() {
        let temp = tempdir().unwrap();

        write(temp.path(), LogLevel::Info, "Launcher started").unwrap();
        write(temp.path(), LogLevel::Error, "Could not check releases").unwrap();

        let log = read_recent(temp.path()).unwrap();

        assert!(log.contains("[INFO] Launcher started"));
        assert!(log.contains("[ERROR] Could not check releases"));
    }

    #[test]
    fn reports_empty_log_when_file_is_missing() {
        let temp = tempdir().unwrap();

        let log = read_recent(temp.path()).unwrap();

        assert_eq!(log, "No log entries yet.");
    }

    #[test]
    fn formats_unix_time_as_readable_utc_timestamp() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            format_utc_timestamp(1_704_067_200),
            "2024-01-01 00:00:00 UTC"
        );
        assert_eq!(
            filename_timestamp_utc(UNIX_EPOCH + std::time::Duration::from_secs(1_704_067_200)),
            "2024-01-01-00-00-00Z"
        );
    }
}
