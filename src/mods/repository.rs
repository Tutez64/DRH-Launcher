//! A mod's GitHub repository: its `source` in the index.

use std::path::Path;
use std::process::Command;

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

/// The owner of a normalized source.
pub fn owner(source: &str) -> Option<&str> {
    source
        .strip_prefix("https://github.com/")?
        .split('/')
        .next()
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
