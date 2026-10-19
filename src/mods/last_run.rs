//! `last-run.json`: what the game reports of the mods it started, in the
//! mods folder (game document, Last-run report).

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct LastRun {
    /// The tag number of the game that wrote it.
    #[serde(default)]
    pub drh: Option<String>,
    /// `false` until the game's `onReady` has run.
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub mods: Vec<ModRun>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModRun {
    pub id: String,
    #[serde(default)]
    pub version: String,
    /// `ok`, `failed` or `skipped`.
    pub status: String,
    /// `compiled`, `interpreted` or `mixed`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn parse(text: &str) -> Result<LastRun, String> {
    serde_json::from_str(text).map_err(|error| error.to_string())
}

/// None when there is none. One that does not parse (read during a write,
/// or cut by a crash) is read again once shortly after, then counts as
/// none.
pub fn read(mods_dir: &Path) -> Option<LastRun> {
    let path = mods_dir.join("last-run.json");
    let text = fs::read_to_string(&path).ok()?;
    parse(&text).ok().or_else(|| {
        thread::sleep(Duration::from_millis(150));
        parse(&fs::read_to_string(&path).ok()?).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_report_or_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).is_none());
        fs::write(dir.path().join("last-run.json"), "{\"ready\": tr").unwrap();
        assert!(read(dir.path()).is_none());
        fs::write(
            dir.path().join("last-run.json"),
            r#"{"drh": "13", "started": "2026-10-19T10:00:00Z", "ready": true,
                "mods": [{"id": "mine", "version": "1.0.0", "status": "failed",
                          "mode": "interpreted", "error": "boom"}]}"#,
        )
        .unwrap();
        let report = read(dir.path()).unwrap();
        assert_eq!(report.drh.as_deref(), Some("13"));
        assert_eq!(report.mods[0].error.as_deref(), Some("boom"));
    }
}
