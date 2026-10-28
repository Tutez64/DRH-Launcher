//! A mod's GitHub repository: its `source` in the index.

use std::path::Path;
use std::process::Command;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Deserialize;

use super::index::{Entry, http_client};
use super::manifest::valid_id;

use crate::error_chain::with_sources;

const GITHUB_API: &str = "https://api.github.com";

/// The largest `.json` asset read as a mod's entry.
const MAX_ENTRY_BYTES: u64 = 64 * 1024;

/// `https://github.com/<owner>/<repo>`: the repository the release workflow
/// runs in (`GITHUB_REPOSITORY`), or else the `origin` remote of `dir`.
pub fn source_of(dir: &Path) -> Result<String, String> {
    if let Ok(repository) = std::env::var("GITHUB_REPOSITORY") {
        let server = std::env::var("GITHUB_SERVER_URL").unwrap_or_default();
        return normalize(&format!("{server}/{repository}")).ok_or_else(|| {
            format!("GITHUB_REPOSITORY {repository} is not a github.com repository")
        });
    }
    let remote = git(Some(dir), &["remote", "get-url", "origin"]).map_err(|_| {
        format!(
            "{} has no origin remote: push it to GitHub first",
            dir.display()
        )
    })?;
    normalize(&remote).ok_or_else(|| format!("The origin remote {remote} is not on github.com"))
}

/// What an indexed mod's repository is named after: `DRH-Mod-<Name>`.
const MOD_REPOSITORY_PREFIX: &str = "DRH-Mod-";

/// An indexed mod's repository is named `DRH-Mod-<Name>`, so its name says
/// it is a DRH mod.
pub fn check_mod_repository(source: &str) -> Result<(), String> {
    let name = source.rsplit('/').next().unwrap_or_default();
    if name.len() > MOD_REPOSITORY_PREFIX.len() && name.starts_with(MOD_REPOSITORY_PREFIX) {
        Ok(())
    } else {
        Err(format!(
            "The repository {source} must be named {MOD_REPOSITORY_PREFIX}<Name> to be indexed"
        ))
    }
}

/// A github.com remote or page URL as `https://github.com/<owner>/<repo>`.
pub fn normalize(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repository) = path.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    (valid(owner) && valid(repository) && !repository.contains('/'))
        .then(|| format!("https://github.com/{owner}/{repository}"))
}

/// A link to a github.com repository, or to a page in it, as
/// `https://github.com/<owner>/<repo>`.
pub fn from_link(link: &str) -> Option<String> {
    let link = link.trim();
    let path = link
        .strip_prefix("https://")
        .or_else(|| link.strip_prefix("http://"))
        .unwrap_or(link);
    let path = path.strip_prefix("www.").unwrap_or(path);
    let path = path.strip_prefix("github.com/")?;
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut parts = path.split('/');
    let (owner, repository) = (parts.next()?, parts.next()?);
    normalize(&format!("https://github.com/{owner}/{repository}"))
}

/// The owner of a normalized source.
pub fn owner(source: &str) -> Option<&str> {
    source
        .strip_prefix("https://github.com/")?
        .split('/')
        .next()
}

/// The entry the mod release workflow attaches to `source`'s latest
/// published release, next to the zip it describes; read without an
/// account. Its `source` is `source`.
pub fn latest_entry(source: &str) -> Result<Entry, String> {
    latest_entry_from(GITHUB_API, source)
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub(crate) fn latest_entry_from(api: &str, source: &str) -> Result<Entry, String> {
    let path = source
        .strip_prefix("https://github.com/")
        .ok_or_else(|| format!("{source} is not a GitHub repository"))?;
    let client = http_client()?;
    let get = |url: &str| {
        client
            .get(url)
            .header(USER_AGENT, "DRH-Launcher")
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .map_err(|error| format!("Could not reach {source}: {}", with_sources(&error)))
    };
    let response = get(&format!("{api}/repos/{path}/releases/latest"))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(format!(
            "{source} has no published release, or is not a public repository."
        ));
    }
    let release: Release = response
        .error_for_status()
        .and_then(|response| response.json())
        .map_err(|error| {
            format!(
                "Could not read the latest release of {source}: {}",
                with_sources(&error)
            )
        })?;
    let zips: Vec<&str> = release
        .assets
        .iter()
        .filter(|asset| asset.name.ends_with(".zip"))
        .map(|asset| asset.browser_download_url.as_str())
        .collect();
    for asset in release
        .assets
        .iter()
        .filter(|asset| asset.name.ends_with(".json") && asset.size <= MAX_ENTRY_BYTES)
    {
        let Ok(mut entry) = get(&asset.browser_download_url)
            .and_then(|response| response.json::<Entry>().map_err(|error| error.to_string()))
        else {
            continue;
        };
        if valid_id(&entry.id) && zips.contains(&entry.url.as_str()) {
            entry.source = source.to_string();
            return Ok(entry);
        }
    }
    Err(format!(
        "The latest release of {source} holds no mod zip with its entry: it was not made with the DRH mod release workflow."
    ))
}

/// Runs git, in `dir` when given; its error output on failure.
pub fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    let output = command
        .args(args)
        .output()
        .map_err(|error| format!("Could not run git (is it installed?): {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_github_remotes() {
        for remote in [
            "https://github.com/me/DRH-Mod-Some",
            "https://github.com/me/DRH-Mod-Some.git",
            "https://github.com/me/DRH-Mod-Some/",
            "git@github.com:me/DRH-Mod-Some.git",
            "ssh://git@github.com/me/DRH-Mod-Some.git",
        ] {
            assert_eq!(
                normalize(remote).as_deref(),
                Some("https://github.com/me/DRH-Mod-Some"),
                "{remote}"
            );
        }
        for remote in [
            "https://gitlab.com/me/m",
            "https://github.com/me",
            "https://github.com/me/m/tree/main",
            "https://github.com/me/m?x",
        ] {
            assert_eq!(normalize(remote), None, "{remote}");
        }
        assert_eq!(owner("https://github.com/me/m"), Some("me"));
    }

    #[test]
    fn reads_a_repository_from_a_link_to_it() {
        for link in [
            "https://github.com/me/DRH-Mod-Some",
            " github.com/me/DRH-Mod-Some/ ",
            "https://www.github.com/me/DRH-Mod-Some.git",
            "https://github.com/me/DRH-Mod-Some/releases/latest",
            "https://github.com/me/DRH-Mod-Some/tree/main?tab=readme#top",
        ] {
            assert_eq!(
                from_link(link).as_deref(),
                Some("https://github.com/me/DRH-Mod-Some"),
                "{link}"
            );
        }
        for link in ["https://github.com/me", "https://gitlab.com/me/m", "me/m"] {
            assert_eq!(from_link(link), None, "{link}");
        }
    }

    #[test]
    fn a_mod_repository_is_named_drh_mod_something() {
        assert!(check_mod_repository("https://github.com/me/DRH-Mod-Cool-HUD").is_ok());
        for source in [
            "https://github.com/me/cool-hud",
            "https://github.com/me/DRH-Mod-",
            "https://github.com/me/drh-mod-cool",
            "https://github.com/Tutez64/DRH-Mods-Index",
        ] {
            assert!(check_mod_repository(source).is_err(), "{source}");
        }
    }
}
