//! `--check-mod`: what a mod's folder must hold, checked without the game.
//! The index CI runs the same checks on an extracted zip.

use std::fs;
use std::path::{Path, PathBuf};

use super::manifest::Manifest;
use super::readme;

/// Values `DRH-Mod-Template` ships with, which a mod must replace.
const TEMPLATE_ID: &str = "my_mod";
const TEMPLATE_NAME: &str = "My Mod";
const TEMPLATE_DESCRIPTION: &str = "What the mod does, in one line.";
const TEMPLATE_README: &str =
    "What the mod does and how to use it: the launcher shows this part on the mod's page.";

/// A mod that passed: its manifest and what its README shows.
pub struct CheckedMod {
    pub manifest: Manifest,
    pub shown_readme: String,
}

pub fn check_mod(dir: &Path) -> Result<CheckedMod, Vec<String>> {
    let mut problems = Vec::new();

    let manifest = match fs::read_to_string(dir.join("mod.json")) {
        Ok(text) => {
            let parsed = Manifest::parse(&text);
            problems.extend(parsed.as_ref().err().cloned().unwrap_or_default());
            check_template_values(&text, &mut problems);
            parsed.ok()
        }
        Err(error) => {
            problems.push(format!("mod.json: cannot be read: {error}"));
            None
        }
    };

    let shown_readme = match fs::read_to_string(dir.join("README.md")) {
        Ok(text) => match readme::shown(&text) {
            Ok(shown) => {
                if shown.chars().count() > readme::MAX_SHOWN_CHARS {
                    problems.push(format!(
                        "README.md: what it shows is longer than {} characters",
                        readme::MAX_SHOWN_CHARS
                    ));
                }
                if shown.contains(TEMPLATE_README) {
                    problems.push(
                        "README.md: its shown zone still has the template's sentence".to_string(),
                    );
                }
                shown
            }
            Err(reason) => {
                problems.push(format!("README.md: {reason}"));
                String::new()
            }
        },
        Err(error) => {
            problems.push(format!("README.md: cannot be read: {error}"));
            String::new()
        }
    };

    match fs::read_to_string(dir.join("LICENSE")) {
        Ok(text) if !text.trim().is_empty() => {}
        Ok(_) => problems.push("LICENSE: is empty".to_string()),
        Err(error) => problems.push(format!("LICENSE: cannot be read: {error}")),
    }

    if let Some(manifest) = &manifest {
        check_sources(dir, manifest, &mut problems);
    }

    match manifest {
        Some(manifest) if problems.is_empty() => Ok(CheckedMod {
            manifest,
            shown_readme,
        }),
        _ => Err(problems),
    }
}

/// Read from the raw file, so they are reported even when other fields fail.
fn check_template_values(text: &str, problems: &mut Vec<String>) {
    let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(text) else {
        return;
    };
    for (key, template) in [
        ("id", TEMPLATE_ID),
        ("name", TEMPLATE_NAME),
        ("description", TEMPLATE_DESCRIPTION),
    ] {
        if fields.get(key).and_then(|value| value.as_str()) == Some(template) {
            problems.push(format!("mod.json: {key} is still the template's"));
        }
    }
}

/// Every `.hx` under `src/` declares the package of its folder, there is no
/// `import.hx`, and the entry's module exists.
fn check_sources(dir: &Path, manifest: &Manifest, problems: &mut Vec<String>) {
    let src = dir.join("src");
    let entry = src.join(format!("{}.hx", manifest.entry));
    if !entry.is_file() {
        problems.push(format!(
            "mod.json: entry {}: src/{}.hx not found",
            manifest.entry, manifest.entry
        ));
    }
    let mut files = Vec::new();
    if let Err(error) = collect_files(&src, &mut files) {
        problems.push(format!("src: cannot be read: {error}"));
        return;
    }
    for file in files {
        let relative = file.strip_prefix(dir).unwrap_or(&file);
        let shown = relative.to_string_lossy().replace('\\', "/");
        let folders: Vec<String> = file
            .strip_prefix(&src)
            .unwrap_or(&file)
            .parent()
            .into_iter()
            .flat_map(|parent| parent.iter())
            .map(|part| part.to_string_lossy().into_owned())
            .collect();
        if let Some(folder) = folders.iter().find(|folder| !valid_package_part(folder)) {
            problems.push(format!(
                "{shown}: folder {folder} is not a package name (a-z or _ first, then letters, digits and _)"
            ));
            continue;
        }
        if file.file_name().is_some_and(|name| name == "import.hx") {
            problems.push(format!(
                "{shown}: import.hx is not supported; each file lists its imports"
            ));
            continue;
        }
        if file.extension().is_none_or(|extension| extension != "hx") {
            continue;
        }
        let expected = std::iter::once(format!("mods.{}", manifest.id))
            .chain(folders)
            .collect::<Vec<_>>()
            .join(".");
        match fs::read_to_string(&file) {
            Ok(text) => match declared_package(&text) {
                Some(package) if package == expected => {}
                Some(package) => problems.push(format!(
                    "{shown}: declares package {package}, its folder is {expected}"
                )),
                None => problems.push(format!("{shown}: must start with package {expected};")),
            },
            Err(error) => problems.push(format!("{shown}: cannot be read: {error}")),
        }
    }
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn valid_package_part(part: &str) -> bool {
    let mut bytes = part.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The path of the `package` declaration a module starts with, after
/// comments; `None` when it has none.
fn declared_package(source: &str) -> Option<String> {
    let mut rest = source.trim_start_matches('\u{feff}');
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("//") {
            rest = after.split_once('\n').map_or("", |(_, next)| next);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/")?.1;
        } else {
            break;
        }
    }
    let after = rest.strip_prefix("package")?;
    if !after.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let (path, _) = after.split_once(';')?;
    let path: String = path.split_whitespace().collect();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn valid_mod() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "mod.json",
            r#"{"id":"some_mod","name":"Some Mod","description":"Does things.","version":"0.1.0","author":"me","api":"1.0","entry":"Main","uses":[]}"#,
        );
        write(
            dir.path(),
            "src/Main.hx",
            "// Entry.\npackage mods.some_mod;\n\nclass Main {}\n",
        );
        write(
            dir.path(),
            "src/ui/Panel.hx",
            "/* Panel */ package mods.some_mod.ui;\n",
        );
        write(
            dir.path(),
            "README.md",
            "# Some Mod\n<!-- drh:show -->\nDoes things.\n<!-- /drh:show -->\n",
        );
        write(dir.path(), "LICENSE", "GPL\n");
        dir
    }

    fn problems(dir: &Path) -> Vec<String> {
        check_mod(dir).err().unwrap_or_default()
    }

    #[test]
    fn accepts_a_valid_mod() {
        let dir = valid_mod();
        let checked = check_mod(dir.path()).unwrap();
        assert_eq!(checked.shown_readme, "Does things.");
    }

    #[test]
    fn package_must_match_the_folder() {
        let dir = valid_mod();
        write(dir.path(), "src/ui/Panel.hx", "package mods.some_mod;\n");
        write(dir.path(), "src/Other.hx", "class Other {}\n");
        write(dir.path(), "src/Bad-Folder/A.hx", "package x;\n");
        assert_eq!(
            problems(dir.path()),
            [
                "src/Bad-Folder/A.hx: folder Bad-Folder is not a package name (a-z or _ first, then letters, digits and _)",
                "src/Other.hx: must start with package mods.some_mod;",
                "src/ui/Panel.hx: declares package mods.some_mod, its folder is mods.some_mod.ui",
            ]
        );
    }

    #[test]
    fn refuses_import_hx_and_a_missing_entry() {
        let dir = valid_mod();
        write(dir.path(), "src/import.hx", "import mods.api.Mod;\n");
        fs::remove_file(dir.path().join("src/Main.hx")).unwrap();
        assert_eq!(
            problems(dir.path()),
            [
                "mod.json: entry Main: src/Main.hx not found",
                "src/import.hx: import.hx is not supported; each file lists its imports",
            ]
        );
    }

    #[test]
    fn refuses_template_values_and_an_empty_license() {
        let dir = valid_mod();
        write(
            dir.path(),
            "mod.json",
            r#"{"id":"my_mod","name":"My Mod","description":"What the mod does, in one line.","version":"0.1.0","author":"","api":"1.0","entry":"Main","uses":[]}"#,
        );
        write(dir.path(), "src/Main.hx", "package mods.my_mod;\n");
        fs::remove_dir_all(dir.path().join("src/ui")).unwrap();
        write(
            dir.path(),
            "README.md",
            &format!("<!-- drh:show -->\n{TEMPLATE_README}\n<!-- /drh:show -->"),
        );
        write(dir.path(), "LICENSE", "\n");
        assert_eq!(
            problems(dir.path()),
            [
                "mod.json: author is required",
                "mod.json: id is still the template's",
                "mod.json: name is still the template's",
                "mod.json: description is still the template's",
                "README.md: its shown zone still has the template's sentence",
                "LICENSE: is empty",
            ]
        );
    }

    #[test]
    fn reads_package_declarations() {
        assert_eq!(declared_package("package a.b;").as_deref(), Some("a.b"));
        assert_eq!(
            declared_package("\u{feff}// x\n/* y */\npackage  a . b ;").as_deref(),
            Some("a.b")
        );
        assert_eq!(declared_package("packages a;"), None);
        assert_eq!(declared_package("import a;\npackage a;"), None);
    }
}
