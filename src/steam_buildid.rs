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
}
