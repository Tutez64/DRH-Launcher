//! What the index CI runs: `--check-entry` on each entry a pull request
//! adds, `--build-index` on the main branch.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

use super::check::check_mod;
use super::index::{Entry, FORMAT, Index};
use super::install::download_and_extract;
use super::manifest::{API_ID, valid_id};
use super::pack::entry_for;
use super::repository::{check_mod_repository, normalize};
use super::resolve::resolve;
use super::version::Requirement;
use crate::diagnostics::format_system_time_utc;

const FIELDS: [&str; 15] = [
    "id",
    "name",
    "version",
    "author",
    "description",
    "longDescription",
    "api",
    "drh",
    "uses",
    "dependencies",
    "url",
    "size",
    "sha256",
    "source",
    "yanked",
];

/// What `--check-entry` found: problems fail the pull request, notes go to
/// the reviewer.
#[derive(Debug, Default)]
pub struct EntryReport {
    pub problems: Vec<String>,
    pub notes: Vec<String>,
}

/// Checks `entry_path`, a `mods/<id>/<version>.json` a pull request adds,
/// against `base`, the index it would join. The zip is extracted into
/// `<work>/new`, and the last indexed version's into `<work>/previous`, for
/// the reviewer's diff. `takeover`: a reviewer accepted another `source`.
pub fn check_entry(entry_path: &Path, base: &Path, work: &Path, takeover: bool) -> EntryReport {
    let mut report = EntryReport::default();
    let entry = match read_new_entry(entry_path) {
        Ok(entry) => entry,
        Err(problems) => {
            report.problems = problems;
            return report;
        }
    };
    let indexed = match read_index_dir(base) {
        Ok(entries) => entries,
        Err(problem) => {
            report.problems.push(problem);
            return report;
        }
    };
    let previous = check_against_index(&entry, &indexed, takeover, &mut report);

    let new = work.join("new");
    let _ = fs::remove_dir_all(&new);
    match download_and_extract(&entry, &new) {
        Ok(()) => match check_mod(&new) {
            Ok(checked) => {
                let expected = entry_for(&checked, &entry.source, entry.size, entry.sha256.clone());
                report.problems.extend(differences(&expected, &entry));
            }
            Err(problems) => report.problems.extend(
                problems
                    .into_iter()
                    .map(|problem| format!("zip: {problem}")),
            ),
        },
        Err(problem) => report.problems.push(problem),
    }

    if let Some(previous) = previous {
        let folder = work.join("previous");
        let _ = fs::remove_dir_all(&folder);
        if let Err(problem) = download_and_extract(previous, &folder) {
            report.notes.push(format!(
                "no diff: the indexed {} {} could not be read: {problem}",
                previous.id, previous.version
            ));
        }
    }
    report
}

/// The checks against the index the entry would join: not indexed yet, the
/// id's `source`, dependencies that resolve. Returns the last indexed
/// version, for the reviewer's diff.
fn check_against_index<'a>(
    entry: &Entry,
    indexed: &'a [Entry],
    takeover: bool,
    report: &mut EntryReport,
) -> Option<&'a Entry> {
    let label = format!("{} {}", entry.id, entry.version);

    let mut same_id: Vec<&Entry> = indexed
        .iter()
        .filter(|other| other.id == entry.id)
        .collect();
    same_id.sort_by_key(|other| other.parsed_version());
    let version = entry.parsed_version();
    if same_id
        .iter()
        .any(|other| other.parsed_version() == version)
    {
        report
            .problems
            .push(format!("{label} is already in the index"));
    }

    if let Some(owner) = same_id.last()
        && owner.source != entry.source
    {
        if takeover {
            report.notes.push(format!(
                "takeover accepted: {} moves from {} to {}",
                entry.id, owner.source, entry.source
            ));
        } else {
            report.problems.push(format!(
                "{} belongs to {}, not {}: a reviewer accepts a takeover with the takeover label",
                entry.id, owner.source, entry.source
            ));
        }
    }

    let previous = same_id
        .iter()
        .rev()
        .find(|other| other.parsed_version() < version)
        .copied();
    if let Some(previous) = previous
        && previous.author != entry.author
    {
        report.notes.push(format!(
            "author changed since {}: {} -> {}",
            previous.version, previous.author, entry.author
        ));
    }

    let needs: Vec<(String, Requirement)> = entry
        .api
        .iter()
        .map(|api| (API_ID.to_string(), api.clone()))
        .chain(entry.dependencies.clone())
        .filter_map(|(id, text)| Some((id, Requirement::parse(&text)?)))
        .collect();
    let index = Index {
        entries: indexed.to_vec(),
    };
    match resolve(&index, &[(entry.id.clone(), needs)], &BTreeMap::new(), None) {
        Ok(resolution) => report.problems.extend(resolution.missing),
        Err(problem) => report.problems.push(problem),
    }

    previous
}

/// The entry's own rules: its path, its fields, its `url` and `source`.
fn read_new_entry(path: &Path) -> Result<Entry, Vec<String>> {
    let text = fs::read_to_string(path)
        .map_err(|error| vec![format!("{}: cannot be read: {error}", path.display())])?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| vec![format!("{}: not valid JSON: {error}", path.display())])?;
    let mut problems = Vec::new();
    if let Some(fields) = value.as_object() {
        for key in fields.keys() {
            if !FIELDS.contains(&key.as_str()) {
                problems.push(format!("unknown field {key}"));
            }
        }
        if fields.contains_key("yanked") {
            problems.push("a new entry is not yanked".to_string());
        }
    }
    let entry: Entry = serde_json::from_value(value)
        .map_err(|error| vec![format!("{}: not an index entry: {error}", path.display())])?;

    if !valid_id(&entry.id) {
        problems.push(format!("id {} is not a valid mod id", entry.id));
    }
    match entry.parsed_version() {
        None => problems.push(format!(
            "version {} is not MAJOR.MINOR.PATCH",
            entry.version
        )),
        Some(_) if !at_its_path(path, &entry) => problems.push(format!(
            "{} must be mods/{}/{}.json",
            path.display(),
            entry.id,
            entry.version
        )),
        Some(_) => {}
    }
    if normalize(&entry.source).as_deref() != Some(entry.source.as_str()) {
        problems.push(format!(
            "source {} must be https://github.com/<owner>/<repository>",
            entry.source
        ));
    } else if let Err(problem) = check_mod_repository(&entry.source) {
        problems.push(problem);
    }
    let url = format!(
        "{}/releases/download/v{}/{}-{}.zip",
        entry.source, entry.version, entry.id, entry.version
    );
    if entry.url != url {
        problems.push(format!("url must be {url}"));
    }
    if problems.is_empty() {
        Ok(entry)
    } else {
        Err(problems)
    }
}

fn at_its_path(path: &Path, entry: &Entry) -> bool {
    let name = |path: Option<&Path>| {
        path.and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    };
    let folder = path.parent();
    name(Some(path)) == Some(format!("{}.json", entry.version))
        && name(folder) == Some(entry.id.clone())
        && name(folder.and_then(Path::parent)) == Some("mods".to_string())
}

/// The fields where the entry says something else than its zip.
fn differences(expected: &Entry, entry: &Entry) -> Vec<String> {
    let (Ok(Value::Object(expected)), Ok(Value::Object(actual))) =
        (serde_json::to_value(expected), serde_json::to_value(entry))
    else {
        return vec!["the entry could not be compared with its zip".to_string()];
    };
    FIELDS
        .iter()
        .filter(|key| expected.get(**key) != actual.get(**key))
        .map(|key| format!("{key} does not match the zip's mod.json and README.md"))
        .collect()
}

/// Every entry of an index checkout, each at its `mods/<id>/<version>.json`.
pub fn read_index_dir(dir: &Path) -> Result<Vec<Entry>, String> {
    let mods = dir.join("mods");
    let mut entries = Vec::new();
    if !mods.exists() {
        return Ok(entries);
    }
    for path in files_under(&mods)? {
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("{}: cannot be read: {error}", path.display()))?;
        let entry: Entry = serde_json::from_str(&text)
            .map_err(|error| format!("{}: not an index entry: {error}", path.display()))?;
        if entry.parsed_version().is_none() || !at_its_path(&path, &entry) {
            return Err(format!(
                "{} is not at mods/{}/{}.json",
                path.display(),
                entry.id,
                entry.version
            ));
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn files_under(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let read = |dir: &Path| {
        fs::read_dir(dir).map_err(|error| format!("Could not read {}: {error}", dir.display()))
    };
    for folder in read(dir)? {
        let folder = folder.map_err(|error| error.to_string())?.path();
        if !folder.is_dir() {
            continue;
        }
        for file in read(&folder)? {
            let file = file.map_err(|error| error.to_string())?.path();
            if file
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.push(file);
            }
        }
    }
    Ok(files)
}

/// The file the launcher reads: every entry, sorted by id then version.
pub fn build_index(dir: &Path, out: &Path) -> Result<usize, String> {
    let mut entries = read_index_dir(dir)?;
    entries.sort_by(|a, b| {
        (a.id.as_str(), a.parsed_version()).cmp(&(b.id.as_str(), b.parsed_version()))
    });
    let generated = format_system_time_utc(SystemTime::now())
        .replace(" UTC", "Z")
        .replacen(' ', "T", 1);
    #[derive(Serialize)]
    struct IndexFile<'a> {
        format: u64,
        generated: String,
        entries: &'a [Entry],
    }
    let file = serde_json::to_string_pretty(&IndexFile {
        format: FORMAT,
        generated,
        entries: &entries,
    })
    .map_err(|error| format!("Could not write the index: {error}"))?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }
    fs::write(out, format!("{file}\n"))
        .map_err(|error| format!("Could not write {}: {error}", out.display()))?;
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str) -> Entry {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "version": version, "author": "me",
            "description": "d", "api": "1.0", "uses": [],
            "url": format!("https://github.com/me/DRH-Mod-M/releases/download/v{version}/{id}-{version}.zip"),
            "size": 1, "sha256": "00", "source": "https://github.com/me/DRH-Mod-M"
        }))
        .unwrap()
    }

    fn write_entry(dir: &Path, entry: &Entry) -> PathBuf {
        let path = dir
            .join("mods")
            .join(&entry.id)
            .join(format!("{}.json", entry.version));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, entry.to_file_text()).unwrap();
        path
    }

    #[test]
    fn a_new_entry_follows_its_path_and_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut bad = entry("cool_hud", "0.2.0");
        bad.url = "https://example.com/cool_hud.zip".to_string();
        bad.yanked = Some("no".to_string());
        let path = dir.path().join("mods/other/0.2.0.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bad.to_file_text()).unwrap();
        let problems = read_new_entry(&path).unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn an_id_keeps_its_source_and_its_dependencies_must_be_indexed() {
        let mut api = entry("api", "1.0.0");
        api.api = None;
        let indexed = [api, entry("cool_hud", "0.1.0")];

        let mut moved = entry("cool_hud", "0.2.0");
        moved.source = "https://github.com/other/DRH-Mod-M".to_string();
        moved.author = "other".to_string();
        moved.api = Some("2.0".to_string());

        let mut report = EntryReport::default();
        let previous = check_against_index(&moved, &indexed, false, &mut report);
        assert_eq!(previous.map(|entry| entry.version.as_str()), Some("0.1.0"));
        assert!(
            report.problems[0].starts_with("cool_hud belongs to https://github.com/me/DRH-Mod-M")
        );
        assert_eq!(
            report.problems[1],
            "No indexed version of api fits: cool_hud needs 2.0.0."
        );
        assert_eq!(report.notes, ["author changed since 0.1.0: me -> other"]);

        let mut report = EntryReport::default();
        check_against_index(&moved, &indexed, true, &mut report);
        assert!(report.notes[0].starts_with("takeover accepted"));

        let mut report = EntryReport::default();
        check_against_index(&entry("cool_hud", "0.1.0"), &indexed, false, &mut report);
        assert_eq!(report.problems, ["cool_hud 0.1.0 is already in the index"]);
    }

    #[test]
    fn an_entry_must_say_what_its_zip_says() {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "mod.json",
            r#"{"id":"cool_hud","name":"Cool HUD","description":"Shows things.","version":"0.2.0","author":"me","api":"1.0","entry":"Main","uses":[]}"#,
        );
        write("src/Main.hx", "package mods.cool_hud;\n");
        write(
            "README.md",
            "# Cool HUD\n<!-- drh:show -->\nShows things.\n<!-- /drh:show -->\n",
        );
        write("LICENSE", "GPL\n");
        let checked = check_mod(dir.path()).unwrap();
        let source = "https://github.com/me/DRH-Mod-M";
        let packed = entry_for(&checked, source, 10, "ab".to_string());
        assert!(differences(&packed, &packed).is_empty());

        let mut submitted = packed.clone();
        submitted.description = "Something else.".to_string();
        submitted.long_description = String::new();
        assert_eq!(
            differences(&packed, &submitted),
            [
                "description does not match the zip's mod.json and README.md",
                "longDescription does not match the zip's mod.json and README.md",
            ]
        );
    }

    #[test]
    fn builds_the_index_sorted_by_id_then_version() {
        let dir = tempfile::tempdir().unwrap();
        for (id, version) in [("lib", "0.10.0"), ("api", "1.0.0"), ("lib", "0.9.0")] {
            write_entry(dir.path(), &entry(id, version));
        }
        let out = dir.path().join("site/index.json");
        assert_eq!(build_index(dir.path(), &out).unwrap(), 3);
        let index = Index::parse(&fs::read_to_string(&out).unwrap()).unwrap();
        let order: Vec<String> = index
            .entries
            .iter()
            .map(|entry| format!("{} {}", entry.id, entry.version))
            .collect();
        assert_eq!(order, ["api 1.0.0", "lib 0.9.0", "lib 0.10.0"]);
    }
}
