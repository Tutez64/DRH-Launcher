//! `--new-mod`: a fresh mod repository from `DRH-Mod-Template`.

use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

use super::index::Index;
use super::manifest::{API_ID, MAX_AUTHOR_CHARS, MAX_NAME_CHARS, valid_id};
use super::repository::git;

const TEMPLATE_URL: &str = "https://github.com/Tutez64/DRH-Mod-Template";

/// The answers `--new-mod` asks for.
pub struct NewMod {
    pub id: String,
    pub name: String,
    pub author: String,
}

pub fn new_mod(dir: &Path, input: &mut impl BufRead) -> Result<(), String> {
    if dir.exists()
        && fs::read_dir(dir)
            .map_err(|error| format!("Could not read {}: {error}", dir.display()))?
            .next()
            .is_some()
    {
        return Err(format!("{} is not empty", dir.display()));
    }
    let answers = ask(input)?;

    let dir_text = dir.to_string_lossy().into_owned();
    git(
        None,
        &["clone", "--quiet", "--depth", "1", TEMPLATE_URL, &dir_text],
    )?;
    fs::remove_dir_all(dir.join(".git"))
        .map_err(|error| format!("Could not drop the template's history: {error}"))?;

    let api = match Index::fetch() {
        Ok(index) => index.versions(API_ID).first().map(|entry| {
            let mut parts = entry.version.split('.');
            format!(
                "{}.{}",
                parts.next().unwrap_or("1"),
                parts.next().unwrap_or("0")
            )
        }),
        Err(error) => {
            println!("warning: {error}; api keeps the template's version");
            None
        }
    };
    fill_template(dir, &answers, api.as_deref())?;

    git(Some(dir), &["init", "--quiet"])?;
    git(Some(dir), &["add", "--all"])?;
    if let Err(error) = git(Some(dir), &["commit", "--quiet", "-m", "Initial commit"]) {
        println!("warning: the first commit failed, make it yourself: {error}");
    }

    let repository = repository_name(&answers.name);
    let url = format!(
        "https://github.com/new?name={}&visibility=public",
        repository
    );
    println!("{} is ready in {}.", answers.id, dir.display());
    println!("Create its repository (opened in your browser: {url}),");
    println!("add the topic drh-mod to it, then push:");
    println!(
        "  git -C \"{}\" remote add origin https://github.com/<you>/{repository}",
        dir.display()
    );
    println!("  git -C \"{}\" push -u origin HEAD", dir.display());
    if let Err(error) = crate::open_url(&url) {
        println!("warning: could not open the browser: {error}");
    }
    Ok(())
}

fn ask(input: &mut impl BufRead) -> Result<NewMod, String> {
    let id = prompt(
        input,
        "id (the mod's folder and package, such as my_cool_mod)",
        |id| {
            if valid_id(id) {
                None
            } else {
                Some("3-64 characters of a-z, 0-9 and _, a letter first, no _ last, not a Haxe keyword".to_string())
            }
        },
    )?;
    let name = prompt(input, "name (shown in the catalog)", |name| {
        bounded(name, MAX_NAME_CHARS)
    })?;
    let author = prompt(
        input,
        "author (your name, shown next to your GitHub account)",
        |author| bounded(author, MAX_AUTHOR_CHARS),
    )?;
    Ok(NewMod { id, name, author })
}

fn bounded(text: &str, max: usize) -> Option<String> {
    (text.chars().count() > max).then(|| format!("at most {max} characters"))
}

fn prompt(
    input: &mut impl BufRead,
    label: &str,
    invalid: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    loop {
        print!("{label}: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let read = input
            .read_line(&mut line)
            .map_err(|error| format!("Could not read the answer: {error}"))?;
        if read == 0 {
            return Err("No answer: nothing created.".to_string());
        }
        let answer = line.trim();
        if answer.is_empty() {
            continue;
        }
        match invalid(answer) {
            None => return Ok(answer.to_string()),
            Some(reason) => println!("  {reason}"),
        }
    }
}

/// The template's values replaced in place, so its layout stays.
pub fn fill_template(dir: &Path, answers: &NewMod, api: Option<&str>) -> Result<(), String> {
    let json = |text: &str| serde_json::to_string(text).expect("a string serializes");
    let mut edits = vec![
        (
            "\"id\": \"my_mod\"".to_string(),
            format!("\"id\": {}", json(&answers.id)),
        ),
        (
            "\"name\": \"My Mod\"".to_string(),
            format!("\"name\": {}", json(&answers.name)),
        ),
        (
            "\"author\": \"\"".to_string(),
            format!("\"author\": {}", json(&answers.author)),
        ),
    ];
    if let Some(api) = api {
        edits.push((
            "\"api\": \"1.0\"".to_string(),
            format!("\"api\": {}", json(api)),
        ));
    }
    replace_in(&dir.join("mod.json"), &edits)?;
    replace_in(
        &dir.join("src").join("Main.hx"),
        &[(
            "package mods.my_mod;".to_string(),
            format!("package mods.{};", answers.id),
        )],
    )?;
    replace_in(
        &dir.join("README.md"),
        &[("# My Mod".to_string(), format!("# {}", answers.name))],
    )
}

fn replace_in(path: &Path, edits: &[(String, String)]) -> Result<(), String> {
    let mut text = fs::read_to_string(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    for (from, to) in edits {
        if text.matches(from.as_str()).count() != 1 {
            return Err(format!(
                "The template's {} no longer has {from}: update DRH Launcher",
                path.display()
            ));
        }
        text = text.replacen(from.as_str(), to, 1);
    }
    fs::write(path, text).map_err(|error| format!("Could not write {}: {error}", path.display()))
}

/// `DRH-Mod-<Name>`, the suggested repository name.
fn repository_name(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect();
    if words.is_empty() {
        "DRH-Mod".to_string()
    } else {
        format!("DRH-Mod-{}", words.join("-"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("mod.json"),
            "{\n  \"id\": \"my_mod\",\n  \"name\": \"My Mod\",\n  \"author\": \"\",\n  \"api\": \"1.0\"\n}\n",
        )
        .unwrap();
        fs::write(dir.path().join("src/Main.hx"), "package mods.my_mod;\n").unwrap();
        fs::write(dir.path().join("README.md"), "# My Mod\n").unwrap();
        dir
    }

    #[test]
    fn fills_the_template_in_place() {
        let dir = template();
        let answers = NewMod {
            id: "cool_mod".to_string(),
            name: "Cool \"Mod\"".to_string(),
            author: "Me".to_string(),
        };
        fill_template(dir.path(), &answers, Some("1.3")).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("mod.json")).unwrap(),
            "{\n  \"id\": \"cool_mod\",\n  \"name\": \"Cool \\\"Mod\\\"\",\n  \"author\": \"Me\",\n  \"api\": \"1.3\"\n}\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("src/Main.hx")).unwrap(),
            "package mods.cool_mod;\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "# Cool \"Mod\"\n"
        );
    }

    #[test]
    fn asks_again_until_an_answer_fits() {
        let mut input = "Bad Id\nnew\ncool_mod\n\nCool Mod\nMe\n".as_bytes();
        let answers = ask(&mut input).unwrap();
        assert_eq!(
            (
                answers.id.as_str(),
                answers.name.as_str(),
                answers.author.as_str()
            ),
            ("cool_mod", "Cool Mod", "Me")
        );
        assert!(ask(&mut "cool_mod\n".as_bytes()).is_err());
    }

    #[test]
    fn suggests_a_repository_name() {
        assert_eq!(repository_name("Cool HUD 2"), "DRH-Mod-Cool-HUD-2");
        assert_eq!(repository_name("chat-macros"), "DRH-Mod-Chat-Macros");
        assert_eq!(repository_name("é"), "DRH-Mod");
    }
}
