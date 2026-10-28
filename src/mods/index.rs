//! The mod index: one entry per mod version, read from a static file.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};

use super::manifest::{MAX_AUTHOR_CHARS, MAX_DESCRIPTION_CHARS, MAX_NAME_CHARS, shortened};
use super::version::Version;

use crate::error_chain::with_sources;

const DEFAULT_INDEX_URL: &str = "https://tutez64.github.io/DRH-Mods-Index/index.json";

/// The generated file's format this launcher reads.
pub const FORMAT: u64 = 1;

/// `DRHL_MOD_INDEX_URL` at run time, then at compile time, then the
/// official index: test builds point at another one, as with
/// `DRHL_UPDATE_ENDPOINT`.
pub fn index_url() -> String {
    let runtime = std::env::var("DRHL_MOD_INDEX_URL").ok();
    [runtime.as_deref(), option_env!("DRHL_MOD_INDEX_URL")]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|url| !url.is_empty())
        .unwrap_or(DEFAULT_INDEX_URL)
        .to_string()
}

/// One version of a mod, as `--pack-mod` writes it and the index lists it.
/// Field order is the game document's.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    #[serde(default)]
    pub long_description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drh: Option<String>,
    #[serde(default)]
    pub uses: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yanked: Option<String>,
}

impl Entry {
    pub fn parsed_version(&self) -> Option<Version> {
        Version::parse(&self.version)
    }

    /// As an index file: two-space JSON and a final newline.
    pub fn to_file_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("an entry serializes");
        text.push('\n');
        text
    }
}

#[derive(Debug, Deserialize)]
struct IndexFile {
    #[serde(default)]
    entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct Index {
    pub entries: Vec<Entry>,
}

impl Index {
    pub fn parse(text: &str) -> Result<Self, String> {
        let format = serde_json::from_str::<serde_json::Value>(text)
            .map_err(|error| format!("The mod index is not valid JSON: {error}"))?
            .get("format")
            .and_then(serde_json::Value::as_u64);
        if format != Some(FORMAT) {
            return Err(
                "The mod index has a format this launcher does not know: update DRH Launcher."
                    .to_string(),
            );
        }
        let mut file: IndexFile = serde_json::from_str(text)
            .map_err(|error| format!("Could not read the mod index: {error}"))?;
        // The index CI holds entries to these; a hand-made index may not.
        for entry in &mut file.entries {
            entry.name = shortened(&entry.name, MAX_NAME_CHARS);
            entry.author = shortened(&entry.author, MAX_AUTHOR_CHARS);
            entry.description = shortened(&entry.description, MAX_DESCRIPTION_CHARS);
        }
        Ok(Self {
            entries: file.entries,
        })
    }

    pub fn fetch() -> Result<Self, String> {
        Self::parse(&fetch_text()?)
    }

    /// Fetches the index and keeps a copy in `cache`, for when it cannot be
    /// fetched.
    pub fn fetch_and_cache(cache: &Path) -> Result<Self, String> {
        let text = fetch_text()?;
        let index = Self::parse(&text)?;
        if let Some(parent) = cache.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(cache, &text);
        Ok(index)
    }

    /// The copy `fetch_and_cache` kept, if it still reads.
    pub fn cached(cache: &Path) -> Option<Self> {
        Self::parse(&fs::read_to_string(cache).ok()?).ok()
    }

    /// Versions of `id` that are not yanked, highest first.
    pub fn versions(&self, id: &str) -> Vec<&Entry> {
        let mut versions: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.id == id && entry.yanked.is_none())
            .filter(|entry| entry.parsed_version().is_some())
            .collect();
        versions.sort_by_key(|entry| std::cmp::Reverse(entry.parsed_version()));
        versions
    }
}

fn fetch_text() -> Result<String, String> {
    let url = index_url();
    http_client()?
        .get(&url)
        .header(USER_AGENT, "DRH-Launcher")
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.text())
        .map_err(|error| {
            format!(
                "Could not fetch the mod index {url}: {}",
                with_sources(&error)
            )
        })
}

pub fn http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("Could not create HTTP client: {}", with_sources(&error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "name": id, "version": version, "author": "me",
            "description": "d", "api": "1.0", "uses": [],
            "url": "https://github.com/me/m/releases/download/v1/m.zip",
            "size": 1, "sha256": "00", "source": "https://github.com/me/m"
        })
    }

    #[test]
    fn refuses_an_unknown_format() {
        let error = Index::parse(r#"{"format":2,"entries":[]}"#).unwrap_err();
        assert!(error.contains("update DRH Launcher"));
    }

    #[test]
    fn lists_versions_highest_first_without_yanked_ones() {
        let mut yanked = entry("lib", "1.2.0");
        yanked["yanked"] = "broken".into();
        let text = serde_json::json!({
            "format": 1, "generated": "2026-10-10T12:00:00Z",
            "entries": [entry("lib", "1.10.0"), entry("lib", "1.9.0"), yanked, entry("other", "2.0.0")]
        })
        .to_string();
        let index = Index::parse(&text).unwrap();
        let versions: Vec<&str> = index
            .versions("lib")
            .iter()
            .map(|entry| entry.version.as_str())
            .collect();
        assert_eq!(versions, ["1.10.0", "1.9.0"]);
    }

    #[test]
    fn writes_fields_in_the_documented_order() {
        let entry: Entry = serde_json::from_value(entry("lib", "1.0.0")).unwrap();
        let text = entry.to_file_text();
        let keys: Vec<&str> = text
            .lines()
            .filter_map(|line| line.trim().strip_prefix('"')?.split('"').next())
            .collect();
        assert_eq!(
            keys,
            [
                "id",
                "name",
                "version",
                "author",
                "description",
                "longDescription",
                "api",
                "uses",
                "url",
                "size",
                "sha256",
                "source"
            ]
        );
    }
}
