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
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .map_err(|error| format!("Could not run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{} has no origin remote: push it to GitHub first",
            dir.display()
        ));
    }
    let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
    normalize(&remote).ok_or_else(|| format!("The origin remote {remote} is not on github.com"))
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
}
