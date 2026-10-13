//! Which version of each needed mod to install: for each id, the highest
//! version that satisfies every mod naming it. One version per id, no
//! backtracking.

use std::collections::BTreeMap;

use super::index::{Entry, Index};
use super::manifest::API_ID;
use super::version::{Requirement, parse_tags};

#[derive(Debug)]
pub struct Resolution {
    /// The `api` first, then by id.
    pub entries: Vec<Entry>,
    /// Versions installed although their `drh` does not list the installed
    /// tag.
    pub warnings: Vec<String>,
}

/// What `root` needs, through its dependencies too. `installed_tag` is the
/// installed DRH tag's number: for an `extends` / `replace` mod, a version
/// whose `drh` contains it comes first.
pub fn resolve(
    index: &Index,
    root: &str,
    requirements: &[(String, Requirement)],
    installed_tag: Option<u32>,
) -> Result<Resolution, String> {
    let mut chosen: BTreeMap<String, &Entry> = BTreeMap::new();
    // Each round adds what the last one chose needs; a mod set settles in
    // as many rounds as its longest dependency chain.
    for _ in 0..64 {
        let mut needs: BTreeMap<&str, Vec<(&str, Requirement)>> = BTreeMap::new();
        for (id, requirement) in requirements {
            needs.entry(id).or_default().push((root, *requirement));
        }
        for entry in chosen.values() {
            for (id, requirement) in entry_requirements(entry)? {
                needs.entry(id).or_default().push((&entry.id, requirement));
            }
        }

        let mut next = BTreeMap::new();
        let mut warnings = Vec::new();
        for (id, needed_by) in &needs {
            if *id == root {
                continue;
            }
            let (entry, warning) = pick(index, id, needed_by, installed_tag)?;
            warnings.extend(warning);
            next.insert(id.to_string(), entry);
        }
        if next == chosen {
            let mut entries: Vec<Entry> = chosen.into_values().cloned().collect();
            entries.sort_by_key(|entry| (entry.id != API_ID, entry.id.clone()));
            return Ok(Resolution { entries, warnings });
        }
        chosen = next;
    }
    Err("The mods' dependencies do not settle on one version each.".to_string())
}

fn entry_requirements(entry: &Entry) -> Result<Vec<(&str, Requirement)>, String> {
    let api = entry.api.iter().map(|api| (API_ID, api.as_str()));
    let dependencies = entry
        .dependencies
        .iter()
        .map(|(id, version)| (id.as_str(), version.as_str()));
    api.chain(dependencies)
        .map(|(id, text)| {
            Requirement::parse(text)
                .map(|requirement| (id, requirement))
                .ok_or_else(|| {
                    format!(
                        "{} {} names {id} {text}, which is not a version",
                        entry.id, entry.version
                    )
                })
        })
        .collect()
}

fn pick<'a>(
    index: &'a Index,
    id: &str,
    needed_by: &[(&str, Requirement)],
    installed_tag: Option<u32>,
) -> Result<(&'a Entry, Option<String>), String> {
    let candidates: Vec<&Entry> = index
        .versions(id)
        .into_iter()
        .filter(|entry| {
            let version = entry.parsed_version().expect("listed versions parse");
            needed_by
                .iter()
                .all(|(_, requirement)| requirement.matches(version))
        })
        .collect();
    let Some(highest) = candidates.first() else {
        let wanted = needed_by
            .iter()
            .map(|(by, requirement)| format!("{by} needs {requirement}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("No indexed version of {id} fits: {wanted}."));
    };
    if highest.uses.is_empty() {
        return Ok((highest, None));
    }
    let Some(tag) = installed_tag else {
        return Ok((highest, None));
    };
    let built_for_tag = |entry: &&&Entry| {
        entry
            .drh
            .as_deref()
            .and_then(parse_tags)
            .is_some_and(|tags| tags.contains(&tag))
    };
    match candidates.iter().find(built_for_tag) {
        Some(entry) => Ok((entry, None)),
        None => Ok((
            highest,
            Some(format!(
                "{id} {}: its drh does not list the installed DRH tag {tag}",
                highest.version
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str, api: Option<&str>, deps: &[(&str, &str)]) -> Entry {
        Entry {
            id: id.to_string(),
            name: id.to_string(),
            version: version.to_string(),
            author: "me".to_string(),
            description: "d".to_string(),
            long_description: String::new(),
            api: api.map(str::to_string),
            drh: None,
            uses: Vec::new(),
            dependencies: deps
                .iter()
                .map(|(id, version)| (id.to_string(), version.to_string()))
                .collect(),
            url: String::new(),
            size: 0,
            sha256: String::new(),
            source: String::new(),
            yanked: None,
        }
    }

    fn api(version: &str, drh: &str) -> Entry {
        Entry {
            uses: vec!["extends".to_string()],
            drh: Some(drh.to_string()),
            ..entry("api", version, None, &[])
        }
    }

    fn requirement(text: &str) -> Requirement {
        Requirement::parse(text).unwrap()
    }

    fn versions(resolution: &Resolution) -> Vec<String> {
        resolution
            .entries
            .iter()
            .map(|entry| format!("{} {}", entry.id, entry.version))
            .collect()
    }

    #[test]
    fn picks_the_highest_version_every_mod_accepts() {
        let index = Index {
            entries: vec![
                api("1.0.0", "14"),
                api("1.2.0", "14"),
                api("2.0.0", "14"),
                entry("lib", "0.3.0", Some("1.2"), &[]),
                entry("lib", "0.3.4", Some("1.2"), &[]),
                entry("lib", "0.4.0", Some("1.2"), &[]),
            ],
        };
        let root = [
            ("api".to_string(), requirement("1.0")),
            ("lib".to_string(), requirement("0.3")),
        ];
        let resolution = resolve(&index, "mine", &root, Some(14)).unwrap();
        assert_eq!(versions(&resolution), ["api 1.2.0", "lib 0.3.4"]);
        assert!(resolution.warnings.is_empty());
    }

    #[test]
    fn prefers_a_version_built_for_the_installed_tag() {
        let index = Index {
            entries: vec![api("1.0.0", "13-14"), api("1.1.0", "15")],
        };
        let root = [("api".to_string(), requirement("1.0"))];
        let resolution = resolve(&index, "mine", &root, Some(14)).unwrap();
        assert_eq!(versions(&resolution), ["api 1.0.0"]);

        let resolution = resolve(&index, "mine", &root, Some(16)).unwrap();
        assert_eq!(versions(&resolution), ["api 1.1.0"]);
        assert_eq!(resolution.warnings.len(), 1);
    }

    #[test]
    fn refuses_when_no_version_fits_everyone() {
        let index = Index {
            entries: vec![api("1.0.0", "14"), entry("lib", "0.1.0", Some("1.3"), &[])],
        };
        let root = [("lib".to_string(), requirement("0.1"))];
        let error = resolve(&index, "mine", &root, Some(14)).unwrap_err();
        assert_eq!(error, "No indexed version of api fits: lib needs 1.3.0.");
    }

    #[test]
    fn does_not_install_the_root_itself() {
        let index = Index {
            entries: vec![
                api("1.0.0", "14"),
                entry("lib", "0.1.0", Some("1.0"), &[("mine", "0.1")]),
            ],
        };
        let root = [
            ("api".to_string(), requirement("1.0")),
            ("lib".to_string(), requirement("0.1")),
        ];
        let resolution = resolve(&index, "mine", &root, Some(14)).unwrap();
        assert_eq!(versions(&resolution), ["api 1.0.0", "lib 0.1.0"]);
    }
}
