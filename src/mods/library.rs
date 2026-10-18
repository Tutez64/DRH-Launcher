//! The mods folder the launcher manages, `<install-dir>/mods/`: the mods in
//! it, which are enabled and in what order (`enabled.json`, read by the
//! game), and how each got there (`data/mods.json`, the launcher's own).

// Until the Mods page uses it, Play and the tests do.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::index::{Entry, Index};
use super::install::download_and_extract;
use super::manifest::{
    MAX_AUTHOR_CHARS, MAX_DESCRIPTION_CHARS, MAX_NAME_CHARS, read_needs, valid_id,
};
use super::resolve::{Installed, Needs, entry_needs, resolve};
use super::version::Version;
use crate::paths;

/// How a mod got into the folder.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "from")]
pub enum Origin {
    Index,
    Zip,
    Repository { url: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Record {
    #[serde(flatten)]
    origin: Origin,
    /// Installed because another mod needed it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    dependency: bool,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct State {
    #[serde(default)]
    mods: BTreeMap<String, Record>,
}

/// A mod in the folder, read from its `mod.json` without refusing it: texts
/// past their limit are cut.
#[derive(Clone, Debug, PartialEq)]
pub struct InstalledMod {
    pub id: String,
    pub folder: PathBuf,
    pub name: String,
    /// As written; not always a version.
    pub version: String,
    pub author: String,
    pub description: String,
    pub drh: Option<String>,
    pub uses: Vec<String>,
    pub needs: Needs,
    /// Why its `api` or `dependencies` could not be read: the game will
    /// likely fail it.
    pub problem: Option<String>,
    /// None when it was copied there by hand.
    pub origin: Option<Origin>,
    pub dependency: bool,
}

pub struct Library {
    dir: PathBuf,
    state_file: PathBuf,
    staging: PathBuf,
}

impl Library {
    pub fn new(install_dir: &Path) -> Self {
        Self {
            dir: paths::mods_dir(install_dir),
            state_file: paths::mods_state_file(install_dir),
            staging: paths::mod_staging_dir(install_dir),
        }
    }

    /// Every folder whose `mod.json` has a valid `id`, by id. Of two folders
    /// with the same `id`, the one named after it is kept, else the first.
    pub fn scan(&self) -> Result<Vec<InstalledMod>, String> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("Could not read {}: {error}", self.dir.display())),
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect();
        folders.sort();
        let state = self.read_state();
        let mut mods: BTreeMap<String, InstalledMod> = BTreeMap::new();
        for folder in folders {
            let Some(mut found) = read_mod(&folder) else {
                continue;
            };
            let named_after = |m: &InstalledMod| m.folder.file_name() == Some(m.id.as_ref());
            if mods
                .get(&found.id)
                .is_some_and(|kept| named_after(kept) || !named_after(&found))
            {
                continue;
            }
            if let Some(record) = state.mods.get(&found.id) {
                found.origin = Some(record.origin.clone());
                found.dependency = record.dependency;
            }
            mods.insert(found.id.clone(), found);
        }
        Ok(mods.into_values().collect())
    }

    /// The enabled ids, in the user's order. A missing or unreadable file
    /// enables none, as for the game.
    pub fn enabled(&self) -> Vec<String> {
        let Ok(text) = fs::read_to_string(self.dir.join("enabled.json")) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = Vec::new();
        for item in value["mods"].as_array().into_iter().flatten() {
            if let Some(id) = item["id"].as_str()
                && !ids.iter().any(|known| known == id)
            {
                ids.push(id.to_string());
            }
        }
        ids
    }

    /// Installs `entry` from the index with what it needs, and enables
    /// them. Returns the warnings to show.
    pub fn install(
        &self,
        index: &Index,
        entry: &Entry,
        installed_tag: Option<u32>,
    ) -> Result<Vec<String>, String> {
        self.settle(index, Some(Added::Entry(entry)), installed_tag)
    }

    /// Enables an installed mod with what it needs, installing what is
    /// missing from `index`.
    pub fn enable(
        &self,
        index: &Index,
        id: &str,
        installed_tag: Option<u32>,
    ) -> Result<Vec<String>, String> {
        self.settle(index, Some(Added::Installed(id)), installed_tag)
    }

    /// The enabled mods' dependencies again, for a new DRH tag: after an
    /// install, update or rollback.
    pub fn refresh(
        &self,
        index: &Index,
        installed_tag: Option<u32>,
    ) -> Result<Vec<String>, String> {
        self.settle(index, None, installed_tag)
    }

    /// Returns a warning for each enabled mod that needs it.
    pub fn disable(&self, id: &str) -> Result<Vec<String>, String> {
        let mut enabled = self.enabled();
        enabled.retain(|known| known != id);
        self.write_enabled(&enabled)?;
        let mods = self.scan()?;
        Ok(needed_by(&mods, id)
            .into_iter()
            .filter(|other| enabled.contains(other))
            .map(|other| format!("{other} needs {id}: the game fails it while {id} is disabled."))
            .collect())
    }

    /// The user's load order: the enabled ids in a new order.
    pub fn set_order(&self, ids: &[String]) -> Result<(), String> {
        let enabled = self.enabled();
        let mut ordered: Vec<String> = ids
            .iter()
            .filter(|id| enabled.contains(id))
            .cloned()
            .collect();
        ordered.extend(enabled.into_iter().filter(|id| !ids.contains(id)));
        self.write_enabled(&ordered)
    }

    /// Removes a mod. Returns the mods installed as a dependency that no
    /// installed mod needs any more, to offer removing them too.
    pub fn uninstall(&self, id: &str) -> Result<Vec<String>, String> {
        let mods = self.scan()?;
        let Some(found) = mods.iter().find(|m| m.id == id) else {
            return Err(format!("{id} is not installed."));
        };
        fs::remove_dir_all(&found.folder)
            .map_err(|error| format!("Could not remove {}: {error}", found.folder.display()))?;
        let mut enabled = self.enabled();
        enabled.retain(|known| known != id);
        self.write_enabled(&enabled)?;
        let mut state = self.read_state();
        state.mods.remove(id);
        self.write_state(&state)?;

        let mut remaining: Vec<&InstalledMod> = mods.iter().filter(|m| m.id != id).collect();
        let mut unneeded = Vec::new();
        loop {
            let free = remaining.iter().position(|candidate| {
                candidate.dependency
                    && !remaining
                        .iter()
                        .any(|other| other.needs.iter().any(|(need, _)| *need == candidate.id))
            });
            let Some(free) = free else {
                break;
            };
            unneeded.push(remaining.remove(free).id.clone());
        }
        Ok(unneeded)
    }

    /// Drops the enabled ids whose folder is gone; the Mods page's scan
    /// does it, not Play. Returns them.
    pub fn prune_enabled(&self) -> Result<Vec<String>, String> {
        let mods = self.scan()?;
        let (kept, dropped): (Vec<String>, Vec<String>) = self
            .enabled()
            .into_iter()
            .partition(|id| mods.iter().any(|m| m.id == *id));
        if !dropped.is_empty() {
            self.write_enabled(&kept)?;
        }
        Ok(dropped)
    }

    fn settle(
        &self,
        index: &Index,
        added: Option<Added>,
        installed_tag: Option<u32>,
    ) -> Result<Vec<String>, String> {
        let mods = self.scan()?;
        let mut enabled = self.enabled();
        let added_id = added.as_ref().map(Added::id);

        let mut roots: Vec<(String, Needs)> = mods
            .iter()
            .filter(|m| enabled.contains(&m.id) && !m.dependency && Some(m.id.as_str()) != added_id)
            .map(|m| (m.id.clone(), m.needs.clone()))
            .collect();
        match &added {
            Some(Added::Entry(entry)) => roots.push((entry.id.clone(), entry_needs(entry)?)),
            Some(Added::Installed(id)) => {
                let found = mods
                    .iter()
                    .find(|m| m.id == *id)
                    .ok_or_else(|| format!("{id} is not installed."))?;
                roots.push((found.id.clone(), found.needs.clone()));
            }
            None => {}
        }
        // The entry being installed replaces whatever version is there.
        let installed: BTreeMap<String, Installed> = mods
            .iter()
            .filter(|m| !matches!(&added, Some(Added::Entry(entry)) if entry.id == m.id))
            .filter_map(|m| {
                let version = Version::parse(&m.version)?;
                let replaceable = m.origin == Some(Origin::Index) && m.dependency;
                Some((
                    m.id.clone(),
                    Installed {
                        version,
                        needs: m.needs.clone(),
                        fixed: !replaceable,
                    },
                ))
            })
            .collect();
        let resolution = resolve(index, &roots, &installed, installed_tag)?;

        let mut to_install: Vec<(&Entry, bool)> = resolution
            .entries
            .iter()
            .filter(|entry| {
                installed
                    .get(&entry.id)
                    .is_none_or(|local| Some(local.version) != entry.parsed_version())
            })
            .map(|entry| (entry, true))
            .collect();
        if let Some(Added::Entry(entry)) = &added {
            to_install.push((entry, false));
        }
        self.put(&mods, &to_install)?;

        let mut wanted: Vec<&str> = resolution
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        wanted.extend(resolution.kept.iter().map(String::as_str));
        wanted.extend(added_id);
        for id in wanted {
            if !enabled.iter().any(|known| known == id) {
                enabled.push(id.to_string());
            }
        }
        self.write_enabled(&enabled)?;

        let mut warnings = resolution.warnings;
        warnings.extend(resolution.missing);
        Ok(warnings)
    }

    /// Downloads every entry first, then swaps each into the folder, so a
    /// failed download changes nothing. `true`: installed as a dependency,
    /// unless it was already there.
    fn put(&self, mods: &[InstalledMod], entries: &[(&Entry, bool)]) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut staged = Vec::new();
        for (entry, _) in entries {
            let folder = self.staging.join(format!("{}-{}", entry.id, entry.version));
            let _ = fs::remove_dir_all(&folder);
            let result =
                download_and_extract(entry, &folder).and_then(|()| match read_mod(&folder) {
                    Some(found) if found.id == entry.id => Ok(()),
                    _ => Err(format!(
                        "{} {}: its zip's mod.json is not that mod's",
                        entry.id, entry.version
                    )),
                });
            staged.push(folder);
            if let Err(error) = result {
                for folder in &staged {
                    let _ = fs::remove_dir_all(folder);
                }
                return Err(error);
            }
        }

        fs::create_dir_all(&self.dir)
            .map_err(|error| format!("Could not create {}: {error}", self.dir.display()))?;
        let mut state = self.read_state();
        for ((entry, as_dependency), folder) in entries.iter().zip(&staged) {
            let old = mods
                .iter()
                .find(|m| m.id == entry.id)
                .map(|m| m.folder.as_path());
            self.swap(folder, old, &self.dir.join(&entry.id))?;
            let dependency = match state.mods.get(&entry.id) {
                Some(record) if *as_dependency => record.dependency,
                None if *as_dependency && old.is_none() => true,
                _ => false,
            };
            state.mods.insert(
                entry.id.clone(),
                Record {
                    origin: Origin::Index,
                    dependency,
                },
            );
            self.write_state(&state)?;
        }
        Ok(())
    }

    /// Moves `staged` to `destination`, in place of `old`; `old` comes back
    /// if the move fails.
    fn swap(&self, staged: &Path, old: Option<&Path>, destination: &Path) -> Result<(), String> {
        let mut backup = staged.as_os_str().to_owned();
        backup.push(".old");
        let backup = PathBuf::from(backup);
        let _ = fs::remove_dir_all(&backup);
        let old = old.filter(|old| old.exists());
        if let Some(old) = old {
            fs::rename(old, &backup)
                .map_err(|error| format!("Could not move {}: {error}", old.display()))?;
        }
        if let Err(error) = fs::rename(staged, destination) {
            if let Some(old) = old {
                let _ = fs::rename(&backup, old);
            }
            let _ = fs::remove_dir_all(staged);
            return Err(format!("Could not move {}: {error}", destination.display()));
        }
        let _ = fs::remove_dir_all(&backup);
        Ok(())
    }

    fn write_enabled(&self, ids: &[String]) -> Result<(), String> {
        let list = serde_json::json!({
            "mods": ids.iter().map(|id| serde_json::json!({ "id": id })).collect::<Vec<_>>()
        });
        fs::create_dir_all(&self.dir)
            .map_err(|error| format!("Could not create {}: {error}", self.dir.display()))?;
        write_file(&self.dir.join("enabled.json"), &format!("{list:#}\n"))
    }

    /// Missing or unreadable: no records, so every mod reads as copied by
    /// hand.
    fn read_state(&self) -> State {
        fs::read_to_string(&self.state_file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write_state(&self, state: &State) -> Result<(), String> {
        if let Some(parent) = self.state_file.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(state).expect("the mods state serializes");
        write_file(&self.state_file, &format!("{text}\n"))
    }
}

enum Added<'a> {
    Entry(&'a Entry),
    Installed(&'a str),
}

impl Added<'_> {
    fn id(&self) -> &str {
        match self {
            Added::Entry(entry) => &entry.id,
            Added::Installed(id) => id,
        }
    }
}

/// The installed mods that need `id`.
pub fn needed_by(mods: &[InstalledMod], id: &str) -> Vec<String> {
    mods.iter()
        .filter(|m| m.needs.iter().any(|(need, _)| need == id))
        .map(|m| m.id.clone())
        .collect()
}

/// Whether the game arguments already name a mods folder: the user's, which
/// wins over the launcher's.
pub fn names_mods_dir(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--mods-dir" || arg.starts_with("--mods-dir="))
}

/// Adds `--mods-dir` with the launcher's folder when a mod is enabled and the
/// arguments name no folder of their own.
pub fn add_mods_dir(install_dir: &Path, args: &mut Vec<String>) {
    if names_mods_dir(args) {
        return;
    }
    let library = Library::new(install_dir);
    if library.enabled().is_empty() {
        return;
    }
    let dir = std::path::absolute(&library.dir).unwrap_or(library.dir);
    args.push("--mods-dir".to_string());
    args.push(dir.display().to_string());
}

/// Before Play: a `last-run.json` present afterwards is that session's.
pub fn clear_last_run(install_dir: &Path) {
    let _ = fs::remove_file(paths::mods_dir(install_dir).join("last-run.json"));
}

fn read_mod(folder: &Path) -> Option<InstalledMod> {
    let text = fs::read_to_string(folder.join("mod.json")).ok()?;
    let fields: Map<String, Value> = serde_json::from_str(&text).ok()?;
    let id = fields
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))?
        .to_string();
    let text_of = |key: &str, max: usize| -> String {
        fields
            .get(key)
            .and_then(Value::as_str)
            .map(|text| text.chars().take(max).collect())
            .unwrap_or_default()
    };
    let (needs, problem) = match read_needs(&text) {
        Ok((_, needs)) => (needs, None),
        Err(problem) => (Vec::new(), Some(problem)),
    };
    let name = text_of("name", MAX_NAME_CHARS);
    Some(InstalledMod {
        name: if name.trim().is_empty() {
            id.clone()
        } else {
            name
        },
        version: text_of("version", usize::MAX),
        author: text_of("author", MAX_AUTHOR_CHARS),
        description: text_of("description", MAX_DESCRIPTION_CHARS),
        drh: fields
            .get("drh")
            .and_then(Value::as_str)
            .map(str::to_string),
        uses: fields
            .get("uses")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        needs,
        problem,
        origin: None,
        dependency: false,
        id,
        folder: folder.to_path_buf(),
    })
}

/// Written next to the file, then moved over it.
fn write_file(path: &Path, text: &str) -> Result<(), String> {
    let partial = path.with_extension("json.partial");
    fs::write(&partial, text)
        .and_then(|()| fs::rename(&partial, path))
        .map_err(|error| format!("Could not write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use sha2::{Digest, Sha256};
    use zip::write::SimpleFileOptions;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn manifest(id: &str, version: &str, api: Option<&str>) -> String {
        let mut fields = serde_json::json!({
            "id": id, "name": id, "description": "d", "version": version,
            "author": "me", "entry": "Main", "uses": []
        });
        if let Some(api) = api {
            fields["api"] = api.into();
        }
        fields.to_string()
    }

    fn zip_of(mod_json: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("mod.json", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(mod_json.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    /// Serves each path's bytes over HTTP, for as long as the tests run.
    fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request = [0_u8; 4096];
                let read = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..read]);
                let path = request.split_whitespace().nth(1).unwrap_or("");
                let body = files
                    .iter()
                    .find(|(served, _)| served == path)
                    .map(|(_, bytes)| bytes.clone());
                let response = match &body {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                };
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body.unwrap_or_default());
            }
        });
        format!("http://{address}")
    }

    fn entry(base: &str, id: &str, version: &str, api: Option<&str>, zip: &[u8]) -> Entry {
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
            dependencies: BTreeMap::new(),
            url: format!("{base}/{id}-{version}.zip"),
            size: zip.len() as u64,
            sha256: Sha256::digest(zip)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            source: format!("https://github.com/me/DRH-Mod-{id}"),
            yanked: None,
        }
    }

    /// An index of `api` 0.1.0 and `lib` 0.1.0, which needs it, served.
    fn served_index() -> Index {
        let api_zip = zip_of(&manifest("api", "0.1.0", None));
        let lib_zip = zip_of(&manifest("lib", "0.1.0", Some("0.1")));
        let base = serve(vec![
            ("/api-0.1.0.zip".to_string(), api_zip.clone()),
            ("/lib-0.1.0.zip".to_string(), lib_zip.clone()),
        ]);
        Index {
            entries: vec![
                entry(&base, "api", "0.1.0", None, &api_zip),
                entry(&base, "lib", "0.1.0", Some("0.1"), &lib_zip),
            ],
        }
    }

    fn ids(mods: &[InstalledMod]) -> Vec<&str> {
        mods.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn scans_mod_json_without_refusing_it() {
        let install = tempfile::tempdir().unwrap();
        let mods = install.path().join("mods");
        let long_name = "N".repeat(100);
        write(
            &mods.join("copied/mod.json"),
            &serde_json::json!({ "id": "some_mod", "name": long_name, "version": "x" }).to_string(),
        );
        write(
            &mods.join("some_mod/mod.json"),
            &manifest("some_mod", "1.0.0", Some("0.1")),
        );
        write(
            &mods.join("other/mod.json"),
            &manifest("other", "1.0.0", Some("0.1")),
        );
        write(
            &mods.join("bad/mod.json"),
            &manifest("Bad-Id", "1.0.0", Some("0.1")),
        );
        write(&mods.join("notes/readme.txt"), "not a mod");
        write(
            &mods.join("broken/mod.json"),
            &serde_json::json!({ "id": "broken", "api": 1 }).to_string(),
        );

        let found = Library::new(install.path()).scan().unwrap();
        assert_eq!(ids(&found), ["broken", "other", "some_mod"]);
        assert_eq!(found[2].folder, mods.join("some_mod"));
        assert_eq!(
            found[0].problem.as_deref(),
            Some("mod.json: api must be a version, such as \"1.0\"")
        );

        fs::remove_dir_all(mods.join("some_mod")).unwrap();
        let found = Library::new(install.path()).scan().unwrap();
        assert_eq!(found[2].folder, mods.join("copied"));
        assert_eq!(found[2].name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(found[2].origin, None);
    }

    #[test]
    fn installs_and_enables_what_a_mod_needs() {
        let install = tempfile::tempdir().unwrap();
        let library = Library::new(install.path());
        let index = served_index();

        let warnings = library
            .install(&index, &index.entries[1], Some(14))
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let found = library.scan().unwrap();
        assert_eq!(ids(&found), ["api", "lib"]);
        assert_eq!(found[0].origin, Some(Origin::Index));
        assert!(found[0].dependency);
        assert!(!found[1].dependency);
        assert_eq!(library.enabled(), ["api", "lib"]);
        assert!(
            fs::read_dir(install.path().join("data/mod-staging"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[test]
    fn a_failed_download_changes_nothing() {
        let install = tempfile::tempdir().unwrap();
        let library = Library::new(install.path());
        let mut index = served_index();
        index.entries[0].sha256 = "00".repeat(32);

        let error = library
            .install(&index, &index.entries[1].clone(), Some(14))
            .unwrap_err();
        assert_eq!(error, "api 0.1.0: SHA-256 does not match the index");
        assert!(library.scan().unwrap().is_empty());
        assert!(library.enabled().is_empty());
    }

    #[test]
    fn enabling_warns_about_what_cannot_be_installed() {
        let install = tempfile::tempdir().unwrap();
        let library = Library::new(install.path());
        write(
            &install.path().join("mods/mine/mod.json"),
            &manifest("mine", "1.0.0", Some("0.1")),
        );

        let index = Index {
            entries: Vec::new(),
        };
        let warnings = library.enable(&index, "mine", Some(14)).unwrap();
        assert_eq!(
            warnings,
            ["No indexed version of api fits: mine needs 0.1.0."]
        );
        assert_eq!(library.enabled(), ["mine"]);
    }

    #[test]
    fn disabling_and_uninstalling_say_what_is_left() {
        let install = tempfile::tempdir().unwrap();
        let library = Library::new(install.path());
        let index = served_index();
        library
            .install(&index, &index.entries[1], Some(14))
            .unwrap();

        assert_eq!(
            library.disable("api").unwrap(),
            ["lib needs api: the game fails it while api is disabled."]
        );
        assert_eq!(library.enabled(), ["lib"]);
        assert_eq!(library.uninstall("lib").unwrap(), ["api"]);
        assert_eq!(ids(&library.scan().unwrap()), ["api"]);
        assert!(library.enabled().is_empty());
    }

    #[test]
    fn prunes_enabled_ids_without_a_folder() {
        let install = tempfile::tempdir().unwrap();
        let library = Library::new(install.path());
        write(
            &install.path().join("mods/mine/mod.json"),
            &manifest("mine", "1.0.0", None),
        );
        library
            .write_enabled(&["gone".to_string(), "mine".to_string()])
            .unwrap();
        assert_eq!(library.prune_enabled().unwrap(), ["gone"]);
        assert_eq!(library.enabled(), ["mine"]);
    }

    #[test]
    fn passes_its_folder_unless_the_user_names_one() {
        let install = tempfile::tempdir().unwrap();
        let library = Library::new(install.path());
        let mut args = vec!["--fps=60".to_string()];
        add_mods_dir(install.path(), &mut args);
        assert_eq!(args, ["--fps=60"]);

        library.write_enabled(&["mine".to_string()]).unwrap();
        add_mods_dir(install.path(), &mut args);
        assert_eq!(args[1], "--mods-dir");
        assert_eq!(Path::new(&args[2]), install.path().join("mods"));

        let mut args = vec!["--mods-dir=/dev/mods".to_string()];
        add_mods_dir(install.path(), &mut args);
        assert_eq!(args, ["--mods-dir=/dev/mods"]);
    }
}
