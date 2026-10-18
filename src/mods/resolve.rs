//! Which version of each needed mod to install: for each id, the highest
//! version that satisfies every mod naming it. One version per id, no
//! backtracking.

use std::collections::BTreeMap;

use super::index::{Entry, Index};
use super::manifest::API_ID;
use super::version::{Requirement, Version, parse_tags};

/// What a mod needs: each id and the version it accepts.
pub type Needs = Vec<(String, Requirement)>;

/// A mod already in the mods folder.
#[derive(Clone, Debug)]
pub struct Installed {
    pub version: Version,
    pub needs: Needs,
    /// Kept as it is: anything but an indexed version installed as a
    /// dependency, which the index may replace.
    pub fixed: bool,
}

#[derive(Debug)]
pub struct Resolution {
    /// Indexed versions to have installed: the `api` first, then by id.
    pub entries: Vec<Entry>,
    /// Needed mods kept as installed.
    pub kept: Vec<String>,
    /// Versions installed although their `drh` does not list the installed
    /// tag, and needed mods kept although they do not fit.
    pub warnings: Vec<String>,
    /// Needed mods with no version to install or keep: the game fails what
    /// needs them.
    pub missing: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Choice<'a> {
    Indexed(&'a Entry),
    Kept,
}

/// What the `roots` (each one's id and needs) need, through their
/// dependencies too; a root is never picked. A needed mod in `installed` is
/// kept when it is fixed or no indexed version fits. `installed_tag` is the
/// installed DRH tag's number: for an `extends` / `replace` mod, a version
/// whose `drh` contains it comes first.
pub fn resolve(
    index: &Index,
    roots: &[(String, Needs)],
    installed: &BTreeMap<String, Installed>,
    installed_tag: Option<u32>,
) -> Result<Resolution, String> {
    let is_root = |id: &str| roots.iter().any(|(root, _)| root == id);
    let mut chosen: BTreeMap<String, Choice> = BTreeMap::new();
    // Each round adds what the last one chose needs; a mod set settles in
    // as many rounds as its longest dependency chain.
    for _ in 0..64 {
        let mut needs: BTreeMap<String, Needs> = BTreeMap::new();
        for (root, requirements) in roots {
            for (id, requirement) in requirements {
                needs
                    .entry(id.clone())
                    .or_default()
                    .push((root.clone(), *requirement));
            }
        }
        for (id, choice) in &chosen {
            let requirements = match choice {
                Choice::Indexed(entry) => entry_needs(entry)?,
                Choice::Kept => installed[id].needs.clone(),
            };
            for (needed, requirement) in requirements {
                needs
                    .entry(needed)
                    .or_default()
                    .push((id.clone(), requirement));
            }
        }

        let mut next = BTreeMap::new();
        let mut warnings = Vec::new();
        let mut missing = Vec::new();
        for (id, needed_by) in &needs {
            let local = installed.get(id);
            if is_root(id) || local.is_some_and(|local| local.fixed) {
                if let Some(local) = local {
                    warnings.extend(misfit(id, local.version, needed_by));
                }
                if !is_root(id) {
                    next.insert(id.clone(), Choice::Kept);
                }
                continue;
            }
            match pick(index, id, needed_by, installed_tag) {
                Ok((entry, warning)) => {
                    warnings.extend(warning);
                    next.insert(id.clone(), Choice::Indexed(entry));
                }
                Err(reason) => match local {
                    Some(local) => {
                        warnings.push(format!(
                            "{reason} Keeping the installed {id} {}.",
                            local.version
                        ));
                        next.insert(id.clone(), Choice::Kept);
                    }
                    None => missing.push(reason),
                },
            }
        }
        if next == chosen {
            let mut entries = Vec::new();
            let mut kept = Vec::new();
            for (id, choice) in chosen {
                match choice {
                    Choice::Indexed(entry) => entries.push(entry.clone()),
                    Choice::Kept => kept.push(id),
                }
            }
            entries.sort_by_key(|entry| (entry.id != API_ID, entry.id.clone()));
            return Ok(Resolution {
                entries,
                kept,
                warnings,
                missing,
            });
        }
        chosen = next;
    }
    Err("The mods' dependencies do not settle on one version each.".to_string())
}

/// The `api` version and the dependencies an index entry names.
pub fn entry_needs(entry: &Entry) -> Result<Needs, String> {
    let api = entry.api.iter().map(|api| (API_ID, api.as_str()));
    let dependencies = entry
        .dependencies
        .iter()
        .map(|(id, version)| (id.as_str(), version.as_str()));
    api.chain(dependencies)
        .map(|(id, text)| {
            Requirement::parse(text)
                .map(|requirement| (id.to_string(), requirement))
                .ok_or_else(|| {
                    format!(
                        "{} {} names {id} {text}, which is not a version",
                        entry.id, entry.version
                    )
                })
        })
        .collect()
}

fn wanted(needed_by: &[(String, Requirement)]) -> String {
    needed_by
        .iter()
        .map(|(by, requirement)| format!("{by} needs {requirement}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A warning when a kept version does not satisfy every mod naming it.
fn misfit(id: &str, version: Version, needed_by: &[(String, Requirement)]) -> Option<String> {
    let fits = needed_by
        .iter()
        .all(|(_, requirement)| requirement.matches(version));
    (!fits).then(|| {
        format!(
            "The installed {id} {version} does not fit: {}.",
            wanted(needed_by)
        )
    })
}

/// The version of `id` to install for mods needing it as `needed_by` says
/// (none for a mod installed for itself): the highest that fits, one built
/// for the installed DRH tag first.
pub fn offered<'a>(
    index: &'a Index,
    id: &str,
    needed_by: &[(String, Requirement)],
    installed_tag: Option<u32>,
) -> Option<&'a Entry> {
    pick(index, id, needed_by, installed_tag)
        .ok()
        .map(|(entry, _)| entry)
}

fn pick<'a>(
    index: &'a Index,
    id: &str,
    needed_by: &[(String, Requirement)],
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
        return Err(format!(
            "No indexed version of {id} fits: {}.",
            wanted(needed_by)
        ));
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

    fn resolve_one(
        index: &Index,
        root: &[(String, Requirement)],
        installed_tag: Option<u32>,
    ) -> Resolution {
        let roots = [("mine".to_string(), root.to_vec())];
        resolve(index, &roots, &BTreeMap::new(), installed_tag).unwrap()
    }

    fn installed(version: &str, needs: &[(&str, &str)], fixed: bool) -> Installed {
        Installed {
            version: Version::parse(version).unwrap(),
            needs: needs
                .iter()
                .map(|(id, text)| (id.to_string(), requirement(text)))
                .collect(),
            fixed,
        }
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
        let resolution = resolve_one(&index, &root, Some(14));
        assert_eq!(versions(&resolution), ["api 1.2.0", "lib 0.3.4"]);
        assert!(resolution.warnings.is_empty());
    }

    #[test]
    fn prefers_a_version_built_for_the_installed_tag() {
        let index = Index {
            entries: vec![api("1.0.0", "13-14"), api("1.1.0", "15")],
        };
        let root = [("api".to_string(), requirement("1.0"))];
        let resolution = resolve_one(&index, &root, Some(14));
        assert_eq!(versions(&resolution), ["api 1.0.0"]);

        let resolution = resolve_one(&index, &root, Some(16));
        assert_eq!(versions(&resolution), ["api 1.1.0"]);
        assert_eq!(resolution.warnings.len(), 1);
    }

    #[test]
    fn refuses_when_no_version_fits_everyone() {
        let index = Index {
            entries: vec![api("1.0.0", "14"), entry("lib", "0.1.0", Some("1.3"), &[])],
        };
        let root = [("lib".to_string(), requirement("0.1"))];
        let resolution = resolve_one(&index, &root, Some(14));
        assert_eq!(versions(&resolution), ["lib 0.1.0"]);
        assert_eq!(
            resolution.missing,
            ["No indexed version of api fits: lib needs 1.3.0."]
        );
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
        let resolution = resolve_one(&index, &root, Some(14));
        assert_eq!(versions(&resolution), ["api 1.0.0", "lib 0.1.0"]);
    }

    #[test]
    fn picks_what_every_root_accepts() {
        let index = Index {
            entries: vec![api("1.0.0", "14"), api("1.1.0", "14"), api("1.2.0", "14")],
        };
        let roots = [
            (
                "a".to_string(),
                vec![("api".to_string(), requirement("1.0"))],
            ),
            (
                "b".to_string(),
                vec![("api".to_string(), requirement("1.1"))],
            ),
        ];
        let resolution = resolve(&index, &roots, &BTreeMap::new(), Some(14)).unwrap();
        assert_eq!(versions(&resolution), ["api 1.2.0"]);
    }

    #[test]
    fn keeps_a_fixed_mod_and_what_it_needs() {
        let index = Index {
            entries: vec![api("1.0.0", "14"), entry("lib", "0.2.0", Some("1.0"), &[])],
        };
        let local = BTreeMap::from([(
            "lib".to_string(),
            installed("0.1.0", &[("api", "1.0")], true),
        )]);
        let roots = [(
            "mine".to_string(),
            vec![("lib".to_string(), requirement("0.2"))],
        )];
        let resolution = resolve(&index, &roots, &local, Some(14)).unwrap();
        assert_eq!(versions(&resolution), ["api 1.0.0"]);
        assert_eq!(resolution.kept, ["lib"]);
        assert_eq!(
            resolution.warnings,
            ["The installed lib 0.1.0 does not fit: mine needs 0.2.0."]
        );
    }

    #[test]
    fn keeps_an_installed_dependency_when_no_indexed_version_fits() {
        let index = Index {
            entries: vec![api("1.0.0", "14")],
        };
        let local = BTreeMap::from([("lib".to_string(), installed("0.1.0", &[], false))]);
        let roots = [(
            "mine".to_string(),
            vec![("lib".to_string(), requirement("0.1"))],
        )];
        let resolution = resolve(&index, &roots, &local, Some(14)).unwrap();
        assert!(resolution.entries.is_empty());
        assert_eq!(resolution.kept, ["lib"]);
        assert_eq!(
            resolution.warnings,
            ["No indexed version of lib fits: mine needs 0.1.0. Keeping the installed lib 0.1.0."]
        );
        assert!(resolution.missing.is_empty());
    }
}
