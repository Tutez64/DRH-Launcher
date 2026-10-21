//! `mod.json`, the launcher/game contract of the game document.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::version::{Requirement, Version, parse_tags};

/// The mod every other one builds on; it names no `api` version.
pub const API_ID: &str = "api";

pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_DESCRIPTION_CHARS: usize = 120;
pub const MAX_AUTHOR_CHARS: usize = 32;

const FIELDS: [&str; 10] = [
    "id",
    "name",
    "description",
    "version",
    "author",
    "api",
    "drh",
    "entry",
    "uses",
    "dependencies",
];

const HAXE_KEYWORDS: [&str; 42] = [
    "abstract",
    "break",
    "case",
    "cast",
    "catch",
    "class",
    "continue",
    "default",
    "dynamic",
    "else",
    "enum",
    "extends",
    "extern",
    "false",
    "final",
    "for",
    "function",
    "implements",
    "import",
    "inline",
    "interface",
    "macro",
    "new",
    "null",
    "operator",
    "overload",
    "override",
    "package",
    "private",
    "public",
    "return",
    "static",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typedef",
    "untyped",
    "using",
    "var",
    "while",
];

/// The mods folder's `data/`, where the game gives each mod a folder to
/// write in: not an id.
pub const MOD_DATA: &str = "data";

/// `^[a-z][a-z0-9_]{1,62}[a-z0-9]$`, neither a Haxe keyword, a Windows
/// reserved device name nor `data`: the mod's folder and package.
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let shape = (3..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1] != b'_'
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_');
    shape && !HAXE_KEYWORDS.contains(&id) && !reserved_device_name(id) && id != MOD_DATA
}

fn reserved_device_name(id: &str) -> bool {
    if matches!(id, "con" | "prn" | "aux" | "nul") {
        return true;
    }
    let (prefix, digit) = id.split_at(id.len().saturating_sub(1));
    matches!(prefix, "com" | "lpt") && matches!(digit.as_bytes(), [b'1'..=b'9'])
}

/// A short class name, resolved as `mods.<id>.<entry>`.
pub fn valid_entry(entry: &str) -> bool {
    let mut bytes = entry.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: Version,
    pub author: String,
    /// Absent only in the `api` mod.
    pub api: Option<String>,
    pub drh: Option<String>,
    pub entry: String,
    pub uses: Vec<String>,
    pub dependencies: BTreeMap<String, String>,
    /// The file as written, for copying its fields into an index entry.
    pub fields: Map<String, Value>,
}

impl Manifest {
    /// Every rule of the game document, all problems at once.
    pub fn parse(text: &str) -> Result<Self, Vec<String>> {
        let fields = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(fields)) => fields,
            Ok(_) => return Err(vec!["mod.json is not a JSON object".to_string()]),
            Err(error) => return Err(vec![format!("mod.json is not valid JSON: {error}")]),
        };
        let mut problems = Vec::new();
        for key in fields.keys() {
            if !FIELDS.contains(&key.as_str()) {
                problems.push(format!("mod.json: unknown field {key}"));
            }
        }

        let id = text_field(&fields, "id", &mut problems);
        if !id.is_empty() && !valid_id(&id) {
            problems.push(format!(
                "mod.json: id {id} must be 3-64 characters of a-z, 0-9 and _, start with a letter, not end with _, and be neither a Haxe keyword nor a Windows reserved name"
            ));
        }
        let name = bounded_text(&fields, "name", MAX_NAME_CHARS, &mut problems);
        let description =
            bounded_text(&fields, "description", MAX_DESCRIPTION_CHARS, &mut problems);
        if description.contains(['\n', '\r']) {
            problems.push("mod.json: description must be one line".to_string());
        }
        let author = bounded_text(&fields, "author", MAX_AUTHOR_CHARS, &mut problems);

        let version_text = text_field(&fields, "version", &mut problems);
        let version = Version::parse(&version_text);
        if version.is_none() && !version_text.is_empty() {
            problems.push(format!(
                "mod.json: version {version_text} must be MAJOR.MINOR.PATCH, digits only"
            ));
        }

        let api = optional_text(&fields, "api", &mut problems);
        match &api {
            Some(api) if Requirement::parse(api).is_none() => problems.push(format!(
                "mod.json: api {api} must be a version, such as \"1.0\""
            )),
            None if id != API_ID => problems.push("mod.json: api is required".to_string()),
            _ => {}
        }

        let entry = text_field(&fields, "entry", &mut problems);
        if !entry.is_empty() && !valid_entry(&entry) {
            problems.push(format!(
                "mod.json: entry {entry} must be a short class name, such as Main"
            ));
        }

        let uses = read_uses(&fields, &mut problems);
        let drh = optional_text(&fields, "drh", &mut problems);
        match (&drh, uses.is_empty()) {
            (None, false) => problems.push("mod.json: drh is required with uses".to_string()),
            (Some(_), true) => {
                problems.push("mod.json: drh is only for extends or replace".to_string())
            }
            (Some(drh), false) if parse_tags(drh).is_none() => problems.push(format!(
                "mod.json: drh {drh} must be tags such as \"20\", \"20,21\" or \"20-22\""
            )),
            _ => {}
        }

        let dependencies = read_dependencies(&fields, &id, &mut problems);

        match version {
            Some(version) if problems.is_empty() => Ok(Self {
                id,
                name,
                description,
                version,
                author,
                api,
                drh,
                entry,
                uses,
                dependencies,
                fields,
            }),
            _ => Err(problems),
        }
    }
}

/// A mod's `id` and what it needs installed (`api` and `dependencies`),
/// without the other rules: enough to run a mod still being written.
pub fn read_needs(text: &str) -> Result<(String, Vec<(String, Requirement)>), String> {
    let fields: Map<String, Value> = serde_json::from_str(text)
        .map_err(|error| format!("mod.json is not a valid JSON object: {error}"))?;
    let id = fields
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or("mod.json: id is missing or invalid")?
        .to_string();
    let mut needs = Vec::new();
    if let Some(api) = fields.get("api") {
        let requirement = api
            .as_str()
            .and_then(Requirement::parse)
            .ok_or("mod.json: api must be a version, such as \"1.0\"")?;
        needs.push((API_ID.to_string(), requirement));
    } else if id != API_ID {
        return Err("mod.json: api is required".to_string());
    }
    if let Some(dependencies) = fields.get("dependencies") {
        let map = dependencies
            .as_object()
            .ok_or("mod.json: dependencies must map ids to versions")?;
        for (dependency, version) in map {
            let requirement = version
                .as_str()
                .and_then(Requirement::parse)
                .filter(|_| valid_id(dependency))
                .ok_or_else(|| format!("mod.json: dependency {dependency} {version} is invalid"))?;
            needs.push((dependency.clone(), requirement));
        }
    }
    Ok((id, needs))
}

fn text_field(fields: &Map<String, Value>, key: &str, problems: &mut Vec<String>) -> String {
    match fields.get(key) {
        Some(Value::String(text)) if !text.trim().is_empty() => text.clone(),
        Some(Value::String(_)) | None => {
            problems.push(format!("mod.json: {key} is required"));
            String::new()
        }
        Some(_) => {
            problems.push(format!("mod.json: {key} must be a string"));
            String::new()
        }
    }
}

fn bounded_text(
    fields: &Map<String, Value>,
    key: &str,
    max: usize,
    problems: &mut Vec<String>,
) -> String {
    let text = text_field(fields, key, problems);
    if text.chars().count() > max {
        problems.push(format!("mod.json: {key} is longer than {max} characters"));
    }
    text
}

/// `text` within `max` characters, for a mod no one checked: cut after the
/// last whole word that fits, then "…".
pub fn shortened(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    let words = match kept.rfind(char::is_whitespace) {
        Some(end) if end > 0 => &kept[..end],
        _ => &kept,
    };
    format!("{}…", words.trim_end())
}

fn optional_text(
    fields: &Map<String, Value>,
    key: &str,
    problems: &mut Vec<String>,
) -> Option<String> {
    match fields.get(key) {
        None => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => {
            problems.push(format!("mod.json: {key} must be a string"));
            None
        }
    }
}

fn read_uses(fields: &Map<String, Value>, problems: &mut Vec<String>) -> Vec<String> {
    let mut uses = Vec::new();
    let Some(value) = fields.get("uses") else {
        return uses;
    };
    let Value::Array(items) = value else {
        problems.push("mod.json: uses must be a list".to_string());
        return uses;
    };
    for item in items {
        match item.as_str() {
            Some(kind @ ("extends" | "replace")) if !uses.iter().any(|seen| seen == kind) => {
                uses.push(kind.to_string());
            }
            Some(kind @ ("extends" | "replace")) => {
                problems.push(format!("mod.json: uses lists {kind} twice"));
            }
            _ => problems.push(format!(
                "mod.json: uses only takes \"extends\" and \"replace\", not {item}"
            )),
        }
    }
    uses
}

fn read_dependencies(
    fields: &Map<String, Value>,
    id: &str,
    problems: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut dependencies = BTreeMap::new();
    let Some(value) = fields.get("dependencies") else {
        return dependencies;
    };
    let Value::Object(map) = value else {
        problems.push("mod.json: dependencies must map ids to versions".to_string());
        return dependencies;
    };
    for (dependency, version) in map {
        let Some(version) = version.as_str() else {
            problems.push("mod.json: dependencies must map ids to versions".to_string());
            continue;
        };
        if dependency == API_ID {
            problems.push("mod.json: api goes in its own field, not in dependencies".to_string());
        } else if dependency == id {
            problems.push("mod.json: a mod cannot depend on itself".to_string());
        } else if !valid_id(dependency) {
            problems.push(format!(
                "mod.json: dependency {dependency} is not a valid id"
            ));
        }
        if Requirement::parse(version).is_none() {
            problems.push(format!(
                "mod.json: dependency {dependency} must name a version, such as \"0.3\", not {version}"
            ));
        }
        dependencies.insert(dependency.clone(), version.to_string());
    }
    dependencies
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
        "id": "some_mod",
        "name": "Some Mod",
        "description": "Short summary.",
        "version": "0.1.0",
        "author": "example",
        "api": "1.2",
        "entry": "Main",
        "uses": [],
        "dependencies": { "cool_lib": "0.3" }
    }"#;

    fn problems(edit: impl FnOnce(&mut Map<String, Value>)) -> Vec<String> {
        let Value::Object(mut fields) = serde_json::from_str(VALID).unwrap() else {
            unreachable!()
        };
        edit(&mut fields);
        Manifest::parse(&Value::Object(fields).to_string())
            .err()
            .unwrap_or_default()
    }

    #[test]
    fn reads_a_valid_manifest() {
        let manifest = Manifest::parse(VALID).unwrap();
        assert_eq!(manifest.id, "some_mod");
        assert_eq!(manifest.version.to_string(), "0.1.0");
        let (id, needs) = read_needs(VALID).unwrap();
        assert_eq!(id, "some_mod");
        assert_eq!(
            needs
                .iter()
                .map(|(id, requirement)| format!("{id} {requirement}"))
                .collect::<Vec<_>>(),
            ["api 1.2.0", "cool_lib 0.3.0"]
        );
    }

    #[test]
    fn ids_follow_the_game_rules() {
        for id in ["abc", "some_mod", "a1_b2"] {
            assert!(valid_id(id), "{id}");
        }
        for id in [
            "ab", "1ab", "Abc", "abc_", "a-b", "new", "class", "con", "com1", "lpt9", "data",
        ] {
            assert!(!valid_id(id), "{id}");
        }
        assert!(valid_id("com"));
        assert!(valid_id(&"a".repeat(64)));
        assert!(!valid_id(&"a".repeat(65)));
    }

    #[test]
    fn reports_every_problem() {
        let found = problems(|fields| {
            fields.insert("name".into(), "x".repeat(65).into());
            fields.insert("version".into(), "1.0".into());
            fields.insert("entry".into(), "mods.Main".into());
            fields.insert("dependencies".into(), serde_json::json!({ "api": "1.0" }));
            fields.insert("colour".into(), "red".into());
        });
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn api_is_required_except_in_the_api_mod() {
        assert_eq!(
            problems(|fields| _ = fields.remove("api")),
            ["mod.json: api is required"]
        );
        let found = problems(|fields| {
            fields.remove("api");
            fields.insert("id".into(), "api".into());
        });
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn drh_goes_with_uses() {
        let found =
            problems(|fields| _ = fields.insert("uses".into(), serde_json::json!(["extends"])));
        assert_eq!(found, ["mod.json: drh is required with uses"]);
        let found = problems(|fields| _ = fields.insert("drh".into(), "20".into()));
        assert_eq!(found, ["mod.json: drh is only for extends or replace"]);
        let found = problems(|fields| {
            fields.insert("uses".into(), serde_json::json!(["replace", "extends"]));
            fields.insert("drh".into(), "20-22".into());
        });
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn shortens_after_a_whole_word() {
        assert_eq!(shortened("Fits as is.", 11), "Fits as is.");
        assert_eq!(
            shortened("Cut after this word, here", 22),
            "Cut after this word,…"
        );
        assert_eq!(shortened("Unbroken", 5), "Unbr…");
        assert_eq!(shortened("éééé", 3), "éé…");
    }

    #[test]
    fn lengths_count_characters() {
        let found = problems(|fields| _ = fields.insert("author".into(), "é".repeat(32).into()));
        assert!(found.is_empty(), "{found:?}");
    }
}
