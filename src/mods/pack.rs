//! `--pack-mod`: a mod's zip, built from a fixed list, and its index entry.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use super::check::{CheckedMod, check_mod};
use super::index::Entry;
use super::repository;
use crate::download::sha256_file;

/// What the zip holds, flat at its root: extracted as is into `mods/<id>/`.
const FILES: [&str; 3] = ["mod.json", "README.md", "LICENSE"];
const FOLDERS: [&str; 2] = ["src", "Resources"];

pub struct Packed {
    pub zip: PathBuf,
    pub entry: PathBuf,
}

/// Checks `dir`, then writes `<id>-<version>.zip` and `<id>-<version>.json`
/// (its index entry) into `out`.
pub fn pack_mod(dir: &Path, out: &Path) -> Result<Packed, String> {
    let checked = check_mod(dir).map_err(|problems| {
        format!(
            "{}\n{} problem(s) in {}: nothing packed",
            problems.join("\n"),
            problems.len(),
            dir.display()
        )
    })?;
    let manifest = &checked.manifest;
    check_release_tag(&manifest.version.to_string())?;
    let source = repository::source_of(dir)?;
    repository::check_mod_repository(&source)?;

    let stem = format!("{}-{}", manifest.id, manifest.version);
    fs::create_dir_all(out)
        .map_err(|error| format!("Could not create {}: {error}", out.display()))?;
    let zip = out.join(format!("{stem}.zip"));
    write_zip(dir, &zip)?;

    let size = fs::metadata(&zip)
        .map_err(|error| format!("Could not read {}: {error}", zip.display()))?
        .len();
    let sha256 =
        sha256_file(&zip).map_err(|error| format!("Could not hash {}: {error}", zip.display()))?;
    let entry = entry_for(&checked, &source, size, sha256);
    let entry_path = out.join(format!("{stem}.json"));
    fs::write(&entry_path, entry.to_file_text())
        .map_err(|error| format!("Could not write {}: {error}", entry_path.display()))?;

    Ok(Packed {
        zip,
        entry: entry_path,
    })
}

/// In the release workflow, a tag that is not `v<version>` builds nothing.
fn check_release_tag(version: &str) -> Result<(), String> {
    if std::env::var("GITHUB_REF_TYPE").as_deref() != Ok("tag") {
        return Ok(());
    }
    let tag = std::env::var("GITHUB_REF_NAME").unwrap_or_default();
    if tag == format!("v{version}") {
        Ok(())
    } else {
        Err(format!(
            "Tag {tag} does not match mod.json version {version}: tag v{version}"
        ))
    }
}

pub fn entry_for(checked: &CheckedMod, source: &str, size: u64, sha256: String) -> Entry {
    let manifest = &checked.manifest;
    let version = manifest.version.to_string();
    Entry {
        url: format!(
            "{source}/releases/download/v{version}/{}-{version}.zip",
            manifest.id
        ),
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version,
        author: manifest.author.clone(),
        description: manifest.description.clone(),
        long_description: checked.shown_readme.clone(),
        api: manifest.api.clone(),
        drh: manifest.drh.clone(),
        uses: manifest.uses.clone(),
        dependencies: manifest.dependencies.clone(),
        size,
        sha256,
        source: source.to_string(),
        yanked: None,
    }
}

/// Sorted entries and fixed times, so the same sources give the same zip.
fn write_zip(dir: &Path, zip: &Path) -> Result<(), String> {
    let mut files: Vec<String> = FILES.iter().map(|name| name.to_string()).collect();
    for folder in FOLDERS {
        let root = dir.join(folder);
        if root.is_dir() {
            collect(dir, &root, &mut files)?;
        }
    }
    files.sort();

    let file = File::create(zip)
        .map_err(|error| format!("Could not create {}: {error}", zip.display()))?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);
    for name in &files {
        let bytes =
            fs::read(dir.join(name)).map_err(|error| format!("Could not read {name}: {error}"))?;
        writer
            .start_file(name.as_str(), options)
            .and_then(|()| writer.write_all(&bytes).map_err(Into::into))
            .map_err(|error| format!("Could not write {name} to {}: {error}", zip.display()))?;
    }
    writer
        .finish()
        .map_err(|error| format!("Could not write {}: {error}", zip.display()))?;
    Ok(())
}

fn collect(base: &Path, dir: &Path, files: &mut Vec<String>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("Could not read {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("Could not read {}: {error}", dir.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        if kind.is_dir() {
            collect(base, &path, files)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(base).unwrap_or(&path);
            let parts: Vec<String> = relative
                .iter()
                .map(|part| part.to_string_lossy().into_owned())
                .collect();
            files.push(parts.join("/"));
        } else {
            return Err(format!(
                "{}: only files and folders are packed",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn zips_the_fixed_list_only_and_the_same_way_each_time() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "mod.json", "{}");
        write(dir.path(), "README.md", "r");
        write(dir.path(), "LICENSE", "l");
        write(dir.path(), "src/Main.hx", "m");
        write(dir.path(), "src/ui/Panel.hx", "p");
        write(dir.path(), "Resources/a.png", "a");
        write(dir.path(), ".github/workflows/release.yml", "w");
        write(dir.path(), "notes.txt", "n");

        let out = tempfile::tempdir().unwrap();
        let (first, second) = (out.path().join("1.zip"), out.path().join("2.zip"));
        write_zip(dir.path(), &first).unwrap();
        write_zip(dir.path(), &second).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

        let mut archive = zip::ZipArchive::new(File::open(&first).unwrap()).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "LICENSE",
                "README.md",
                "Resources/a.png",
                "mod.json",
                "src/Main.hx",
                "src/ui/Panel.hx"
            ]
        );
        let mut text = String::new();
        archive
            .by_name("src/ui/Panel.hx")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "p");
    }
}
