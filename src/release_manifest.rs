use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::platform::Platform;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReleaseManifest {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_buildid: Option<u64>,
    pub platforms: HashMap<String, ManifestPlatform>,
    #[serde(default)]
    pub launch_options: Option<ManifestLaunchOptions>,
}

impl ReleaseManifest {
    pub fn parse(contents: &str) -> Result<Self, String> {
        let manifest: Self = serde_json::from_str(contents)
            .map_err(|error| format!("Could not parse release manifest: {error}"))?;
        if manifest.steam_buildid == Some(0) {
            return Err("Manifest steam_buildid must be a positive Steam BuildID".to_string());
        }
        if let Some(frame_rate) = manifest
            .launch_options
            .as_ref()
            .and_then(|options| options.frame_rate.as_ref())
        {
            frame_rate.validate()?;
        }
        Ok(manifest)
    }

    pub fn platform(&self, platform: Platform) -> Result<&ManifestPlatform, String> {
        self.platforms.get(platform.id()).ok_or_else(|| {
            format!(
                "Manifest {} does not define platform {}",
                self.version,
                platform.id()
            )
        })
    }
}

pub const STEAM_BUILDID_MANIFEST_SINCE: u32 = 14;
pub const FRAME_RATE_MANIFEST_SINCE: u32 = 11;

pub fn drh_release_number(version: &str) -> Option<u32> {
    version.trim().strip_prefix('V')?.parse().ok()
}

pub fn manifest_includes_steam_buildid(version: &str) -> bool {
    drh_release_number(version).is_some_and(|number| number >= STEAM_BUILDID_MANIFEST_SINCE)
}

pub fn manifest_includes_frame_rate(version: &str) -> bool {
    drh_release_number(version).is_some_and(|number| number >= FRAME_RATE_MANIFEST_SINCE)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestPlatform {
    pub archive: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestLaunchOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<ManifestFrameRate>,
    #[serde(default)]
    pub game_arguments: Vec<ManifestGameArgument>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestFrameRate {
    pub flag: String,
    pub auto: ManifestAutoFrameRate,
    pub custom_min: u32,
    pub custom_max: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestAutoFrameRate {
    pub fallback: u32,
    pub step: u32,
    pub maximum: u32,
}

impl ManifestFrameRate {
    pub(crate) fn preset_values(&self) -> Vec<u32> {
        (self.auto.step..=self.auto.maximum)
            .step_by(self.auto.step as usize)
            .collect()
    }

    fn validate(&self) -> Result<(), String> {
        if !self.flag.starts_with("--") || self.flag.len() <= 2 {
            return Err("Frame-rate flag must start with -- and contain a name".to_string());
        }
        if self.custom_min == 0 || self.custom_max < self.custom_min {
            return Err("Frame-rate custom range is invalid".to_string());
        }
        if self.auto.step == 0
            || self.auto.maximum < self.auto.step
            || !self.auto.maximum.is_multiple_of(self.auto.step)
        {
            return Err("Automatic frame-rate policy is invalid".to_string());
        }
        if self.auto.fallback < self.auto.step
            || self.auto.fallback > self.auto.maximum
            || !self.auto.fallback.is_multiple_of(self.auto.step)
        {
            return Err(
                "Automatic frame-rate fallback must be one of the generated presets".to_string(),
            );
        }
        if self.auto.step < self.custom_min || self.auto.maximum > self.custom_max {
            return Err("Frame-rate presets must fit inside the custom range".to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestGameArgument {
    pub name: String,
    pub flag: String,
    pub default: bool,
    #[serde(default)]
    pub recommended: Option<bool>,
    #[serde(default)]
    pub config_key: Option<String>,
}

pub fn is_manifest_asset_name(name: &str, version: &str) -> bool {
    name == "manifest.json" || name == format!("Dungeon.Rampage.Haxe.{version}.manifest.json")
}

pub fn normalize_sha256(value: &str) -> String {
    value
        .strip_prefix("sha256:")
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase()
        .to_string()
}

pub fn validate_sha256(value: &str, description: &str) -> Result<String, String> {
    let normalized = normalize_sha256(value);
    if normalized.len() != 64
        || !normalized
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(format!(
            "{description} must be a 64-character SHA-256 hex digest"
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_and_selects_platform() {
        let manifest = ReleaseManifest::parse(
            r#"{
                "version": "V3",
                "steam_buildid": 23435799,
                "platforms": {
                    "linux-x64": {
                        "archive": "Dungeon.Rampage.Haxe.V3.Linux.tar.gz",
                        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "size": 123
                    }
                },
                "launch_options": {
                    "frame_rate": {
                        "flag": "--fps",
                        "auto": {
                            "fallback": 120,
                            "step": 24,
                            "maximum": 240
                        },
                        "custom_min": 1,
                        "custom_max": 10000
                    },
                    "game_arguments": [
                        {
                            "name": "want-zoom",
                            "flag": "--want-zoom",
                            "default": false,
                            "recommended": true
                        },
                        {
                            "name": "quality-control-button",
                            "flag": "--quality-control-button",
                            "default": true,
                            "config_key": "quality-control-button"
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let platform = manifest.platform(Platform::LinuxX64).unwrap();

        assert_eq!(manifest.version, "V3");
        assert_eq!(manifest.steam_buildid, Some(23435799));
        assert_eq!(platform.archive, "Dungeon.Rampage.Haxe.V3.Linux.tar.gz");
        assert_eq!(
            platform.sha256,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(platform.size, 123);
        let launch_options = manifest.launch_options.unwrap();
        let frame_rate = launch_options.frame_rate.unwrap();
        assert_eq!(frame_rate.auto.fallback, 120);
        assert_eq!(frame_rate.auto.step, 24);
        assert_eq!(
            frame_rate.preset_values(),
            vec![24, 48, 72, 96, 120, 144, 168, 192, 216, 240]
        );
        assert_eq!(frame_rate.custom_min, 1);
        assert_eq!(launch_options.game_arguments.len(), 2);
        assert_eq!(launch_options.game_arguments[0].flag, "--want-zoom");
        assert_eq!(launch_options.game_arguments[0].recommended, Some(true));
        assert_eq!(
            launch_options.game_arguments[1].config_key.as_deref(),
            Some("quality-control-button")
        );
    }

    #[test]
    fn rejects_invalid_frame_rate_metadata() {
        let manifest = r#"{
            "version": "V3",
            "platforms": {},
            "launch_options": {
                "frame_rate": {
                    "flag": "--fps",
                    "auto": {
                        "fallback": 100,
                        "step": 24,
                        "maximum": 144
                    },
                    "custom_min": 1,
                    "custom_max": 10000
                }
            }
        }"#;

        assert!(ReleaseManifest::parse(manifest).is_err());
    }

    #[test]
    fn rejects_zero_steam_buildid() {
        let manifest = r#"{
            "version": "V3",
            "steam_buildid": 0,
            "platforms": {}
        }"#;

        assert!(ReleaseManifest::parse(manifest).is_err());
    }

    #[test]
    fn parses_drh_release_numbers_and_manifest_field_support() {
        assert_eq!(drh_release_number("V11"), Some(11));
        assert_eq!(drh_release_number(" V14 "), Some(14));
        assert!(drh_release_number("V9").is_some());
        assert!(drh_release_number("v11").is_none());
        assert!(drh_release_number("V11.1").is_none());

        assert!(!manifest_includes_frame_rate("V10"));
        assert!(manifest_includes_frame_rate("V11"));
        assert!(!manifest_includes_steam_buildid("V13"));
        assert!(manifest_includes_steam_buildid("V14"));
    }

    #[test]
    fn treats_missing_steam_buildid_as_optional() {
        let manifest = ReleaseManifest::parse(
            r#"{
                "version": "V3",
                "platforms": {}
            }"#,
        )
        .unwrap();

        assert!(manifest.steam_buildid.is_none());
    }

    #[test]
    fn recognizes_manifest_asset_names() {
        assert!(is_manifest_asset_name("manifest.json", "V4"));
        assert!(is_manifest_asset_name(
            "Dungeon.Rampage.Haxe.V4.manifest.json",
            "V4"
        ));
        assert!(!is_manifest_asset_name(
            "Dungeon.Rampage.Haxe.V4.Linux.tar.gz",
            "V4"
        ));
    }

    #[test]
    fn normalizes_sha256_values() {
        assert_eq!(normalize_sha256("sha256:ABC123"), "abc123");
        assert_eq!(normalize_sha256("abc123"), "abc123");
    }

    #[test]
    fn validates_sha256_values() {
        assert!(
            validate_sha256(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "digest"
            )
            .is_ok()
        );
        assert!(validate_sha256("abc123", "digest").is_err());
    }
}
