use std::sync::Mutex;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::USER_AGENT;

use crate::config::LauncherConfig;
use crate::install_metadata::InstalledState;

pub const STEAM_APP_ID: u32 = 3053950;
const PRODUCT_INFO_URL: &str = "https://api.steamcmd.net/v1/info/3053950";
const HTTP_USER_AGENT: &str = "DRH-Launcher";

static OFFICIAL_PUBLIC_BUILDID: Mutex<Option<u64>> = Mutex::new(None);

// Official Steam public-branch BuildIDs for DRH releases published before
// `steam_buildid` existed in the GitHub release manifest.
const KNOWN_STEAM_BUILDIDS: &[(&str, u64)] = &[
    ("V10", 23435799),
    ("V11", 23435799),
    ("V12", 23435799),
    ("V13", 25038329),
];

pub fn known_steam_buildid(version: &str) -> Option<u64> {
    let version = version.trim();
    KNOWN_STEAM_BUILDIDS
        .iter()
        .find(|(known, _)| *known == version)
        .map(|(_, buildid)| *buildid)
}

pub fn resolve_steam_buildid(version: &str, from_manifest: Option<u64>) -> Option<u64> {
    from_manifest
        .filter(|buildid| *buildid > 0)
        .or_else(|| known_steam_buildid(version))
}

pub fn installed_release_steam_buildid(config: &LauncherConfig) -> Option<u64> {
    InstalledState::load(&config.effective_install_dir())
        .ok()
        .and_then(|state| resolve_steam_buildid(&state.active.version, state.active.steam_buildid))
}

pub fn cached_official_public_buildid() -> Option<u64> {
    *lock_official_public_buildid()
}

pub fn cache_official_public_buildid(buildid: u64) {
    *lock_official_public_buildid() = Some(buildid);
}

pub fn official_update_text(
    installed_buildid: Option<u64>,
    official_buildid: Option<u64>,
    installed_version: Option<&str>,
    latest_version: Option<&str>,
) -> String {
    let (Some(installed_buildid), Some(official_buildid)) = (installed_buildid, official_buildid)
    else {
        return String::new();
    };
    if installed_buildid == official_buildid {
        return String::new();
    }

    let action = match (
        installed_version
            .map(str::trim)
            .filter(|version| !version.is_empty()),
        latest_version
            .map(str::trim)
            .filter(|version| !version.is_empty()),
    ) {
        (Some(installed), Some(latest)) if installed != latest => {
            format!("Update or restore the latest DRH release ({latest}).")
        }
        (Some(_), Some(_)) => "Wait for the next DRH release.".to_string(),
        _ => String::new(),
    };

    if action.is_empty() {
        "Official Dungeon Rampage was updated. This DRH version may no longer connect.".to_string()
    } else {
        format!(
            "Official Dungeon Rampage was updated. This DRH version may no longer connect. {action}"
        )
    }
}

pub fn fetch_official_public_buildid() -> Result<u64, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not create Steam product-info client: {error}"))?;
    let value = client
        .get(PRODUCT_INFO_URL)
        .header(USER_AGENT, HTTP_USER_AGENT)
        .send()
        .map_err(|error| format!("Could not check the official Steam build: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Official Steam build request failed: {error}"))?
        .json::<serde_json::Value>()
        .map_err(|error| format!("Could not parse the official Steam build: {error}"))?;

    parse_official_public_buildid(&value)
        .ok_or_else(|| "Official Steam public BuildID was missing from the response.".to_string())
}

fn lock_official_public_buildid() -> std::sync::MutexGuard<'static, Option<u64>> {
    OFFICIAL_PUBLIC_BUILDID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn parse_official_public_buildid(value: &serde_json::Value) -> Option<u64> {
    json_u64(
        value
            .get("data")?
            .get(STEAM_APP_ID.to_string())?
            .pointer("/depots/branches/public/buildid")?,
    )
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_catalogued_releases() {
        assert_eq!(known_steam_buildid("V10"), Some(23435799));
        assert_eq!(known_steam_buildid("V13"), Some(25038329));
        assert_eq!(known_steam_buildid(" V12 "), Some(23435799));
        assert!(known_steam_buildid("V9").is_none());
        assert!(known_steam_buildid("V14").is_none());
    }

    #[test]
    fn prefers_manifest_buildid_over_catalog() {
        assert_eq!(resolve_steam_buildid("V13", Some(25038329)), Some(25038329));
        assert_eq!(resolve_steam_buildid("V13", Some(1)), Some(1));
        assert_eq!(resolve_steam_buildid("V13", Some(0)), Some(25038329));
        assert_eq!(resolve_steam_buildid("V13", None), Some(25038329));
        assert_eq!(resolve_steam_buildid("V14", Some(26000000)), Some(26000000));
        assert!(resolve_steam_buildid("V14", None).is_none());
        assert!(resolve_steam_buildid("V9", None).is_none());
    }

    #[test]
    fn official_update_text_compares_installed_and_public_buildids() {
        assert!(
            official_update_text(Some(23435799), Some(23435799), Some("V13"), Some("V13"))
                .is_empty()
        );
        assert!(official_update_text(None, Some(24000000), Some("V13"), Some("V13")).is_empty());
        assert!(official_update_text(Some(23435799), None, Some("V13"), Some("V13")).is_empty());

        let waiting =
            official_update_text(Some(23435799), Some(24000000), Some("V13"), Some("V13"));
        assert!(waiting.contains("may no longer connect"));
        assert!(waiting.contains("Wait for the next DRH release"));
        assert!(!waiting.contains("Update or restore"));

        let older = official_update_text(Some(23435799), Some(24000000), Some("V10"), Some("V13"));
        assert!(older.contains("may no longer connect"));
        assert!(older.contains("Update or restore the latest DRH release (V13)"));
        assert!(!older.contains("Wait for the next"));

        let unknown_latest =
            official_update_text(Some(23435799), Some(24000000), Some("V10"), None);
        assert!(unknown_latest.contains("may no longer connect"));
        assert!(!unknown_latest.contains("Wait for the next"));
        assert!(!unknown_latest.contains("Update or restore"));
    }

    #[test]
    fn parses_public_buildid_from_steamcmd_payload() {
        let product_info = serde_json::json!({
            "data": {
                "3053950": {
                    "depots": {
                        "branches": {
                            "public": { "buildid": "23435799" },
                            "experimental": { "buildid": "23435621" }
                        }
                    }
                }
            }
        });
        assert_eq!(parse_official_public_buildid(&product_info), Some(23435799));

        let numeric = serde_json::json!({
            "data": { "3053950": { "depots": { "branches": { "public": { "buildid": 25038329 } } } } }
        });
        assert_eq!(parse_official_public_buildid(&numeric), Some(25038329));
        assert!(parse_official_public_buildid(&serde_json::json!({})).is_none());
    }

    #[test]
    fn reads_installed_steam_buildid_from_metadata_or_catalog() {
        use crate::install_metadata::{InstalledRelease, InstalledState};
        use crate::release_source::ReleaseSource;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        InstalledState {
            active: InstalledRelease {
                version: "V10".to_string(),
                platform: "linux-x64".to_string(),
                source: ReleaseSource::fixtures().label(),
                release_url: "https://example.test/V10".to_string(),
                archive: "archive.tar.gz".to_string(),
                archive_sha256: "abc123".to_string(),
                archive_size: 1,
                installed_at: "unix:0".to_string(),
                launch_options: None,
                steam_buildid: Some(23435799),
            },
            previous: None,
            blocked_update_version: None,
        }
        .save(temp.path())
        .unwrap();

        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };
        assert_eq!(installed_release_steam_buildid(&config), Some(23435799));

        let catalog_only = tempdir().unwrap();
        InstalledState {
            active: InstalledRelease {
                version: "V13".to_string(),
                platform: "linux-x64".to_string(),
                source: ReleaseSource::fixtures().label(),
                release_url: "https://example.test/V13".to_string(),
                archive: "archive.tar.gz".to_string(),
                archive_sha256: "abc123".to_string(),
                archive_size: 1,
                installed_at: "unix:0".to_string(),
                launch_options: None,
                steam_buildid: None,
            },
            previous: None,
            blocked_update_version: None,
        }
        .save(catalog_only.path())
        .unwrap();
        let catalog_config = LauncherConfig {
            install_dir: Some(catalog_only.path().to_path_buf()),
            ..LauncherConfig::default()
        };
        assert_eq!(
            installed_release_steam_buildid(&catalog_config),
            Some(25038329)
        );
    }
}
