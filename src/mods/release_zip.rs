//! The checks on a release's zip that need no index, shared by
//! `--check-entry` (the index CI) and `--submit-mod` (before the pull
//! request): its `size` and `sha256`, `--check-mod` on what it holds, the
//! entry it gives, and the sources of its tag.

use std::fs;
use std::path::{Path, PathBuf};

use reqwest::header::USER_AGENT;

use super::check::check_mod;
use super::index::{Entry, http_client};
use super::index_ci::{EntryReport, differences};
use super::install::download_and_extract;
use super::pack::{entry_for, packed_files};
use crate::archive::extract_zip;
use crate::error_chain::with_sources;

/// How many paths a note names before it counts the others.
const NAMED_PATHS: usize = 5;

/// Downloads the release's zip of `entry`, extracts it into `folder`, and
/// adds to `report` what it finds.
pub fn check_zip(entry: &Entry, folder: &Path, report: &mut EntryReport) {
    let _ = fs::remove_dir_all(folder);
    if let Err(problem) = download_and_extract(entry, folder) {
        report.problems.push(problem);
        return;
    }
    match check_mod(folder) {
        Ok(checked) => {
            let expected = entry_for(&checked, &entry.source, entry.size, entry.sha256.clone());
            report.problems.extend(differences(&expected, entry));
            report.notes.extend(checked.notes);
        }
        Err(problems) => report.problems.extend(
            problems
                .into_iter()
                .map(|problem| format!("zip: {problem}")),
        ),
    }
    report.notes.extend(compare_with_tag(entry, folder));
}

/// Whether the zip, extracted in `folder`, holds what its tag does: the
/// release workflow builds it so; a release made by hand may not.
fn compare_with_tag(entry: &Entry, folder: &Path) -> Vec<String> {
    let tag = format!("v{}", entry.version);
    let checkout = beside(folder, "tag");
    let result = download_tag(entry, &tag, &checkout).and_then(|root| compare(folder, &root));
    let _ = fs::remove_dir_all(&checkout);
    match result {
        Ok(comparison) => comparison.notes(&tag),
        Err(problem) => vec![format!(
            "the zip could not be compared with tag {tag}: {problem}"
        )],
    }
}

fn beside(folder: &Path, suffix: &str) -> PathBuf {
    let name = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    folder.with_file_name(format!("{name}.{suffix}"))
}

/// Extracts GitHub's archive of the tag into `checkout`; the folder it holds.
fn download_tag(entry: &Entry, tag: &str, checkout: &Path) -> Result<PathBuf, String> {
    let url = format!("{}/archive/refs/tags/{tag}.zip", entry.source);
    let bytes = http_client()?
        .get(&url)
        .header(USER_AGENT, "DRH-Launcher")
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.bytes())
        .map_err(|error| format!("Could not download {url}: {}", with_sources(&error)))?;
    let _ = fs::remove_dir_all(checkout);
    let zip = beside(checkout, "zip");
    fs::write(&zip, &bytes)
        .map_err(|error| format!("Could not write {}: {error}", zip.display()))?;
    let extracted = extract_zip(&zip, checkout);
    let _ = fs::remove_file(&zip);
    extracted?;
    let mut folders = fs::read_dir(checkout)
        .map_err(|error| format!("Could not read {}: {error}", checkout.display()))?
        .filter_map(Result::ok)
        .filter(|item| item.path().is_dir());
    match (folders.next(), folders.next()) {
        (Some(root), None) => Ok(root.path()),
        _ => Err(format!("{url} does not hold one folder")),
    }
}

#[derive(Debug, Default, PartialEq)]
struct Comparison {
    differ: Vec<String>,
    only_in_zip: Vec<String>,
    only_in_tag: Vec<String>,
}

impl Comparison {
    fn notes(&self, tag: &str) -> Vec<String> {
        [
            (&self.differ, format!("the zip and tag {tag} differ in")),
            (
                &self.only_in_zip,
                format!("the zip holds what tag {tag} does not:"),
            ),
            (
                &self.only_in_tag,
                format!("the zip lacks what tag {tag} holds:"),
            ),
        ]
        .into_iter()
        .filter(|(paths, _)| !paths.is_empty())
        .map(|(paths, said)| format!("{said} {}", named(paths)))
        .collect()
    }
}

fn named(paths: &[String]) -> String {
    let shown = paths
        .iter()
        .take(NAMED_PATHS)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match paths.len().saturating_sub(NAMED_PATHS) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// Every file of the zip against the files of the tag that a release packs.
/// Line ends do not count: git may write them either way.
fn compare(zip: &Path, tag: &Path) -> Result<Comparison, String> {
    let mut in_zip = Vec::new();
    files_under(zip, zip, &mut in_zip)?;
    in_zip.sort();
    let in_tag = packed_files(tag)?;
    let mut comparison = Comparison::default();
    for path in &in_zip {
        if !in_tag.contains(path) {
            comparison.only_in_zip.push(path.clone());
        } else if normalized(&zip.join(path))? != normalized(&tag.join(path))? {
            comparison.differ.push(path.clone());
        }
    }
    comparison.only_in_tag = in_tag
        .into_iter()
        .filter(|path| !in_zip.contains(path))
        .collect();
    Ok(comparison)
}

fn files_under(base: &Path, dir: &Path, files: &mut Vec<String>) -> Result<(), String> {
    let items =
        fs::read_dir(dir).map_err(|error| format!("Could not read {}: {error}", dir.display()))?;
    for item in items {
        let path = item
            .map_err(|error| format!("Could not read {}: {error}", dir.display()))?
            .path();
        if path.is_dir() {
            files_under(base, &path, files)?;
        } else {
            let relative = path.strip_prefix(base).unwrap_or(&path);
            files.push(
                relative
                    .iter()
                    .map(|part| part.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
    Ok(())
}

fn normalized(path: &Path) -> Result<Vec<u8>, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let mut out = Vec::with_capacity(bytes.len());
    for (i, &byte) in bytes.iter().enumerate() {
        if !(byte == b'\r' && bytes.get(i + 1) == Some(&b'\n')) {
            out.push(byte);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn compares_the_zip_with_the_packed_files_of_its_tag() {
        let zip = tempfile::tempdir().unwrap();
        let tag = tempfile::tempdir().unwrap();
        for dir in [zip.path(), tag.path()] {
            write(dir, "mod.json", "{}");
            write(dir, "LICENSE", "GPL");
        }
        write(zip.path(), "src/Main.hx", "a\r\nb\r\n");
        write(tag.path(), "src/Main.hx", "a\nb\n");
        write(zip.path(), "README.md", "new");
        write(tag.path(), "README.md", "old");
        write(zip.path(), "notes.txt", "x");
        write(tag.path(), "Resources/icon.png", "png");
        write(tag.path(), ".github/workflows/release.yml", "not packed");

        let comparison = compare(zip.path(), tag.path()).unwrap();
        assert_eq!(
            comparison,
            Comparison {
                differ: vec!["README.md".to_string()],
                only_in_zip: vec!["notes.txt".to_string()],
                only_in_tag: vec!["Resources/icon.png".to_string()],
            }
        );
        assert_eq!(
            comparison.notes("v0.1.0"),
            [
                "the zip and tag v0.1.0 differ in README.md",
                "the zip holds what tag v0.1.0 does not: notes.txt",
                "the zip lacks what tag v0.1.0 holds: Resources/icon.png",
            ]
        );
    }

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in files {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_the_tag_from_its_github_archive() {
        let release = zip_of(&[("mod.json", "{}"), ("README.md", "new")]);
        let archive = zip_of(&[
            ("DRH-Mod-M-0.1.0/mod.json", "{}"),
            ("DRH-Mod-M-0.1.0/README.md", "old"),
            (
                "DRH-Mod-M-0.1.0/.github/workflows/release.yml",
                "not packed",
            ),
        ]);
        let source = crate::mods::test_server::serve(vec![(
            "/archive/refs/tags/v0.1.0.zip".to_string(),
            archive,
        )]);
        let entry: Entry = serde_json::from_value(serde_json::json!({
            "id": "m", "name": "M", "version": "0.1.0", "author": "me",
            "description": "D", "api": "1.0", "url": format!("{source}/m.zip"),
            "size": release.len(), "sha256": "", "source": source,
        }))
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("new");
        let zip = dir.path().join("release.zip");
        fs::write(&zip, &release).unwrap();
        extract_zip(&zip, &folder).unwrap();

        assert_eq!(
            compare_with_tag(&entry, &folder),
            ["the zip and tag v0.1.0 differ in README.md"]
        );
        assert!(!beside(&folder, "tag").exists());
    }

    #[test]
    fn names_a_few_paths_and_counts_the_others() {
        let paths: Vec<String> = (1..=7).map(|n| format!("f{n}")).collect();
        assert_eq!(named(&paths), "f1, f2, f3, f4, f5 and 2 more");
        assert_eq!(named(&paths[..2]), "f1, f2");
    }
}
