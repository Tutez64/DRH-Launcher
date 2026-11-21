//! What `--check-mod` reads in a mod's code: what `mod.json` must say for
//! it, and what a reviewer would otherwise point out. Problems refuse the
//! mod; notes are shown to the modder, and to the reviewer by the index CI.

use std::collections::BTreeMap;

use super::manifest::{API_ID, Manifest, valid_id};

const INTERNAL: &str = "mods.api._internal";

#[derive(Debug, Default)]
pub struct Scan {
    pub problems: Vec<String>,
    pub notes: Vec<String>,
    /// The other mods the code names that `dependencies` does not list.
    pub named_mods: Vec<String>,
}

/// `files`: each `.hx` under `src/`, as shown (`src/Main.hx`) and its text.
pub fn scan(files: &[(String, String)], manifest: &Manifest) -> Scan {
    let mut scan = Scan::default();
    let lexed: Vec<(&str, Lexed)> = files
        .iter()
        .map(|(shown, text)| (shown.as_str(), lex(text)))
        .collect();
    let is_api = manifest.id == API_ID;

    if !is_api {
        for (shown, lexed) in &lexed {
            let in_code = find_all(&lexed.code, INTERNAL.as_bytes()).next();
            let in_string = lexed
                .strings
                .iter()
                .find(|(_, text)| text.contains(INTERNAL))
                .map(|(offset, _)| *offset);
            if let Some(offset) = in_code.into_iter().chain(in_string).min() {
                scan.problems.push(format!(
                    "{shown}:{}: {INTERNAL} is the api's own code, not part of the api",
                    line_of(&lexed.code, offset)
                ));
            }
        }
    }

    if !is_api && !manifest.uses.iter().any(|kind| kind == "extends") {
        let reach = lexed.iter().find_map(|(shown, lexed)| {
            core_access(&lexed.code).map(|offset| (shown, line_of(&lexed.code, offset)))
        });
        if let Some((shown, line)) = reach {
            scan.notes.push(format!(
                "{shown}:{line}: reaches the core (.core), which makes the mod extends: add \"uses\": [\"extends\"] and drh to mod.json"
            ));
        }
    }

    // For each other mod, the first place of the way that says the most.
    let mut named: BTreeMap<String, (Naming, &str, usize)> = BTreeMap::new();
    for (shown, lexed) in &lexed {
        for (offset, other, naming) in other_mods(lexed) {
            if other == manifest.id || other == API_ID || manifest.dependencies.contains_key(&other)
            {
                continue;
            }
            let line = line_of(&lexed.code, offset);
            let held = named.entry(other).or_insert((naming, shown, line));
            if naming < held.0 {
                *held = (naming, shown, line);
            }
        }
    }
    scan.named_mods = named.keys().cloned().collect();
    let mut named: Vec<_> = named.into_iter().collect();
    named.sort_by_key(|(_, (_, shown, line))| (*shown, *line));
    for (other, (naming, shown, line)) in named {
        scan.notes.push(match naming {
            Naming::Import => format!(
                "{shown}:{line}: imports mod {other}, which dependencies in mod.json does not list"
            ),
            Naming::IsActive => format!("{shown}:{line}: adapts to mod {other} when it runs"),
            Naming::Class => format!(
                "{shown}:{line}: looks for mod {other} by its classes: context.isActive(\"{other}\") (api 1.5) says whether it runs"
            ),
        });
    }

    for (shown, lexed) in &lexed {
        let empty = empty_catches(&lexed.code);
        if empty > 0 {
            scan.notes.push(format!(
                "{shown}: {empty} empty catch block(s): the errors they catch are reported nowhere"
            ));
        }
    }

    scan
}

/// A source whose comments, and the contents of its strings and regular
/// expressions, are blanked, so that what is searched in `code` is code;
/// offsets and lines are the source's.
struct Lexed {
    code: Vec<u8>,
    /// Each string literal: the offset of its content, and its content.
    strings: Vec<(usize, String)>,
}

fn lex(source: &str) -> Lexed {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut strings = Vec::new();
    let blank = |code: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut code[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        let next = bytes.get(i + 1).copied();
        match (bytes[i], next) {
            (b'/', Some(b'/')) => {
                let end = bytes[i..]
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .map_or(bytes.len(), |at| i + at);
                blank(&mut code, i, end);
                i = end;
            }
            (b'/', Some(b'*')) => {
                let end = bytes[i + 2..]
                    .windows(2)
                    .position(|pair| pair == b"*/")
                    .map_or(bytes.len(), |at| i + 2 + at + 2);
                blank(&mut code, i, end);
                i = end;
            }
            (b'"' | b'\'', _) => {
                let start = i + 1;
                let end = closing(bytes, start, bytes[i], false);
                strings.push((
                    start,
                    String::from_utf8_lossy(&bytes[start..end]).into_owned(),
                ));
                blank(&mut code, start, end);
                i = end + 1;
            }
            (b'~', Some(b'/')) => {
                let start = i + 2;
                let end = closing(bytes, start, b'/', true);
                blank(&mut code, start, end);
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    Lexed { code, strings }
}

/// The offset of the unescaped `quote` that closes a literal begun at
/// `from`, or the end of the source.
fn closing(bytes: &[u8], from: usize, quote: u8, one_line: bool) -> usize {
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'\n' if one_line => return i,
            byte if byte == quote => return i,
            _ => i += 1,
        }
    }
    bytes.len()
}

fn line_of(code: &[u8], offset: usize) -> usize {
    code[..offset].iter().filter(|&&byte| byte == b'\n').count() + 1
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn find_all<'a>(code: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    code.windows(needle.len())
        .enumerate()
        .filter(move |(_, window)| *window == needle)
        .map(|(at, _)| at)
}

/// Where `word` stands alone, not inside a longer name.
fn find_word<'a>(code: &'a [u8], word: &'a str) -> impl Iterator<Item = usize> + 'a {
    find_all(code, word.as_bytes()).filter(move |&at| {
        let before = at.checked_sub(1).map(|i| code[i]);
        let after = code.get(at + word.len()).copied();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

fn skip_space(code: &[u8], mut i: usize) -> usize {
    while code.get(i).is_some_and(|byte| byte.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

/// From an `open` at `at`, the offset just past the matching `close`.
fn balanced(code: &[u8], at: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &byte) in code.iter().enumerate().skip(at) {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(i + 1);
            }
        }
    }
    None
}

/// The start of the line `at` is on, and that line's first word.
fn first_word_of_line(code: &[u8], at: usize) -> &[u8] {
    let start = code[..at]
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |i| i + 1);
    let from = skip_space(code, start);
    let end = code[from..]
        .iter()
        .position(|&byte| !is_ident(byte))
        .map_or(code.len(), |i| from + i);
    &code[from..end]
}

/// A member access `x.core`, not a package such as `starling.core.Starling`
/// nor an import.
fn core_access(code: &[u8]) -> Option<usize> {
    find_all(code, b".core").find(|&at| {
        let after = at + ".core".len();
        if !at.checked_sub(1).is_some_and(|i| is_ident(code[i]))
            || code.get(after).copied().is_some_and(is_ident)
        {
            return false;
        }
        if matches!(
            first_word_of_line(code, at),
            b"import" | b"using" | b"package"
        ) {
            return false;
        }
        let next = skip_space(code, after);
        !(code.get(next) == Some(&b'.')
            && code
                .get(skip_space(code, next + 1))
                .is_some_and(u8::is_ascii_uppercase))
    })
}

/// How a source names another mod, the one that says the most first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Naming {
    /// `import mods.other.Thing;`: the mod needs it.
    Import,
    /// `context.isActive("other")`: the documented way to adapt to it.
    IsActive,
    /// `Type.resolveClass("mods.other.Main")`: its code, loaded or not.
    Class,
}

/// The other mods a source names, and how.
fn other_mods(lexed: &Lexed) -> Vec<(usize, String, Naming)> {
    let mod_of = |path: &str| -> Option<String> {
        let id = path.strip_prefix("mods.")?.split('.').next()?;
        (!id.is_empty()
            && id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
        .then(|| id.to_string())
    };
    let code = &lexed.code;
    let mut found = Vec::new();
    for keyword in ["import", "using"] {
        for at in find_word(code, keyword) {
            if first_word_of_line(code, at) != keyword.as_bytes() {
                continue;
            }
            let from = at + keyword.len();
            let end = code[from..]
                .iter()
                .position(|&byte| byte == b';' || byte == b'\n')
                .map_or(code.len(), |i| from + i);
            let path: String = String::from_utf8_lossy(&code[from..end])
                .split_whitespace()
                .collect();
            if let Some(id) = mod_of(&path) {
                found.push((at, id, Naming::Import));
            }
        }
    }
    for (offset, text) in &lexed.strings {
        if let Some(id) = mod_of(text.trim()) {
            found.push((*offset, id, Naming::Class));
        }
    }
    for at in find_word(code, "isActive") {
        let open = skip_space(code, at + "isActive".len());
        let quote = skip_space(code, open + 1);
        if code.get(open) != Some(&b'(') || !matches!(code.get(quote), Some(b'"' | b'\'')) {
            continue;
        }
        let id = lexed
            .strings
            .iter()
            .find(|(offset, _)| *offset == quote + 1)
            .map(|(_, text)| text.trim());
        if let Some(id) = id.filter(|id| valid_id(id)) {
            found.push((at, id.to_string(), Naming::IsActive));
        }
    }
    found.sort_by_key(|(offset, _, _)| *offset);
    found
}

/// `catch (e:Dynamic) {}`: a block with nothing but blanks or comments.
fn empty_catches(code: &[u8]) -> usize {
    find_word(code, "catch")
        .filter(|&at| {
            let open = skip_space(code, at + "catch".len());
            if code.get(open) != Some(&b'(') {
                return false;
            }
            let Some(after) = balanced(code, open, b'(', b')') else {
                return false;
            };
            let brace = skip_space(code, after);
            code.get(brace) == Some(&b'{') && code.get(skip_space(code, brace + 1)) == Some(&b'}')
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, uses: &[&str], dependencies: &[&str]) -> Manifest {
        let uses = uses
            .iter()
            .map(|kind| format!("\"{kind}\""))
            .collect::<Vec<_>>()
            .join(",");
        let drh = if uses.is_empty() {
            ""
        } else {
            r#","drh":"18""#
        };
        let dependencies = dependencies
            .iter()
            .map(|id| format!("\"{id}\":\"0.1\""))
            .collect::<Vec<_>>()
            .join(",");
        Manifest::parse(&format!(
            r#"{{"id":"{id}","name":"N","description":"D","version":"0.1.0","author":"me","api":"1.0","entry":"Main","uses":[{uses}]{drh},"dependencies":{{{dependencies}}}}}"#
        ))
        .unwrap()
    }

    fn scan_one(text: &str, manifest: &Manifest) -> Scan {
        scan(&[("src/Main.hx".to_string(), text.to_string())], manifest)
    }

    #[test]
    fn lexing_blanks_comments_strings_and_regular_expressions() {
        let lexed = lex("a // x.core\nb /* {\n} */ \"s{\\\"\" 'q' ~/a{\\/b/g c");
        assert_eq!(
            String::from_utf8(lexed.code).unwrap(),
            "a          \nb     \n     \"    \" ' ' ~/     /g c"
        );
        assert_eq!(
            lexed.strings,
            [(25, "s{\\\"".to_string()), (32, "q".to_string())]
        );
    }

    #[test]
    fn refuses_the_api_internals_outside_the_api() {
        let text = "package mods.m;\nimport mods.api._internal.Hub;\nvar c = Type.resolveClass(\"mods.api._internal.Options\");\n";
        assert_eq!(
            scan_one(text, &manifest("some_mod", &[], &[])).problems,
            ["src/Main.hx:2: mods.api._internal is the api's own code, not part of the api"]
        );
        assert!(
            scan_one(text, &manifest("api", &[], &[]))
                .problems
                .is_empty()
        );
    }

    #[test]
    fn warns_of_the_core_without_extends() {
        let text = "import starling.core.Starling;\nvar s = starling.core.Starling.current;\n// ctx.core\nvar f = ctx.core.facade;\n";
        assert_eq!(
            scan_one(text, &manifest("some_mod", &[], &[])).notes,
            [
                "src/Main.hx:4: reaches the core (.core), which makes the mod extends: add \"uses\": [\"extends\"] and drh to mod.json"
            ]
        );
        assert!(
            scan_one(text, &manifest("some_mod", &["extends"], &[]))
                .notes
                .is_empty()
        );
    }

    #[test]
    fn notes_other_mods_not_in_dependencies() {
        let text = "package mods.some_mod;\nimport mods.some_mod.ui.Panel;\nimport mods.api.Mod;\nimport mods.needed.Thing;\nvar a = mods.push(x);\nvar c = Type.resolveClass(\"mods.other_mod.Main\");\nvar d = Type.resolveClass(\"mods.other_mod.Api\");\nimport mods.missing.Thing;\nif (context.isActive(\"needed\") || context.isActive( 'optional' )) {}\nif (context.isActive(\"bridged\")) Type.resolveClass(\"mods.bridged.Api\");\nvar n = context.isActive(name);\n";
        assert_eq!(
            scan_one(text, &manifest("some_mod", &[], &["needed"])).named_mods,
            ["bridged", "missing", "optional", "other_mod"]
        );
        assert_eq!(
            scan_one(text, &manifest("some_mod", &[], &["needed"])).notes,
            [
                "src/Main.hx:6: looks for mod other_mod by its classes: context.isActive(\"other_mod\") (api 1.5) says whether it runs",
                "src/Main.hx:8: imports mod missing, which dependencies in mod.json does not list",
                "src/Main.hx:9: adapts to mod optional when it runs",
                "src/Main.hx:10: adapts to mod bridged when it runs",
            ]
        );
    }

    #[test]
    fn counts_empty_catches() {
        let text = "try a() catch (e:Dynamic) {}\ntry b() catch(_) { /* ignored */ }\ntry c() catch (e:Dynamic) { trace(e); }\nvar s = \"catch (e) {}\";\n";
        assert_eq!(
            scan_one(text, &manifest("some_mod", &[], &[])).notes,
            ["src/Main.hx: 2 empty catch block(s): the errors they catch are reported nowhere"]
        );
    }
}
