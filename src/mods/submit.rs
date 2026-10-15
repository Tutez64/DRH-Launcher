//! `--submit-mod`: the index entry of a published release, on a branch of
//! the author's fork of the index, ready for a pull request.

use std::fs;
use std::path::Path;
use std::process::Command;

use reqwest::StatusCode;
use reqwest::header::USER_AGENT;

use super::index::{Entry, http_client};
use super::manifest::Manifest;
use super::repository::{self, git};

const INDEX_REPOSITORY: &str = "Tutez64/DRH-Mods-Index";

/// `fork_owner` is the account holding the fork, the source's owner unless
/// given.
pub fn submit_mod(dir: &Path, fork_owner: Option<&str>) -> Result<(), String> {
    let manifest_text = fs::read_to_string(dir.join("mod.json"))
        .map_err(|error| format!("{}: no readable mod.json: {error}", dir.display()))?;
    let manifest = Manifest::parse(&manifest_text).map_err(|problems| problems.join("\n"))?;
    let version = manifest.version.to_string();
    let source = repository::source_of(dir)?;
    let owner = match fork_owner {
        Some(owner) => owner.to_string(),
        None => repository::owner(&source)
            .ok_or_else(|| format!("{source} has no owner"))?
            .to_string(),
    };

    let entry_url = format!(
        "{source}/releases/download/v{version}/{}-{version}.json",
        manifest.id
    );
    let entry = fetch_entry(&entry_url)?;
    if entry.id != manifest.id || entry.version != version || entry.source != source {
        return Err(format!(
            "{entry_url} is the entry of {} {} from {}, not of this mod.json ({} {version}) from {source}",
            entry.id, entry.version, entry.source, manifest.id
        ));
    }

    let fork = format!("https://github.com/{owner}/DRH-Mods-Index");
    if !page_exists(&fork)? {
        let fork_page = format!("https://github.com/{INDEX_REPOSITORY}/fork");
        let _ = crate::open_url(&fork_page);
        return Err(format!(
            "No fork of the index at {fork}: fork it ({fork_page}, opened in your browser), then run this again."
        ));
    }

    let work = std::env::temp_dir().join(format!("drhl-submit-{}-{version}", manifest.id));
    if work.exists() {
        fs::remove_dir_all(&work)
            .map_err(|error| format!("Could not clear {}: {error}", work.display()))?;
    }
    let upstream = format!("https://github.com/{INDEX_REPOSITORY}");
    let result = commit_and_push(&work, &entry, &upstream, &fork);
    let _ = fs::remove_dir_all(&work);
    let (base, branch) = result?;

    let compare = format!(
        "https://github.com/{INDEX_REPOSITORY}/compare/{base}...{owner}:DRH-Mods-Index:{branch}?expand=1"
    );
    println!("Pushed {branch} to {fork}. Open the pull request: {compare}");
    if let Err(error) = crate::open_url(&compare) {
        println!("warning: could not open the browser: {error}");
    }
    Ok(())
}

/// Clones the index, adds the entry on a new branch, and pushes it to the
/// fork; the index's default branch and the new one.
fn commit_and_push(
    work: &Path,
    entry: &Entry,
    upstream: &str,
    fork: &str,
) -> Result<(String, String), String> {
    let work_text = work.to_string_lossy().into_owned();
    git(None, &["clone", "--quiet", upstream, &work_text])?;
    let base = git(Some(work), &["symbolic-ref", "--short", "HEAD"])?;

    let file = format!("mods/{}/{}.json", entry.id, entry.version);
    let path = work.join(&file);
    if path.exists() {
        return Err(format!(
            "{} {} is already in the index",
            entry.id, entry.version
        ));
    }
    let branch = format!("{}-{}", entry.id, entry.version);
    git(Some(work), &["checkout", "--quiet", "-b", &branch])?;
    fs::create_dir_all(path.parent().expect("an entry path has a folder"))
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
    fs::write(&path, entry.to_file_text())
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    git(Some(work), &["add", &file])?;
    git(
        Some(work),
        &[
            "commit",
            "--quiet",
            "-m",
            &format!("Add {} {}", entry.id, entry.version),
        ],
    )?;

    // Inherits the console: git may ask for the account's credentials.
    let pushed = Command::new("git")
        .arg("-C")
        .arg(work)
        .args([
            "push",
            "--quiet",
            fork,
            &format!("HEAD:refs/heads/{branch}"),
        ])
        .status()
        .map_err(|error| format!("Could not run git: {error}"))?;
    if !pushed.success() {
        return Err(format!("Could not push {branch} to {fork}"));
    }
    Ok((base, branch))
}

fn fetch_entry(url: &str) -> Result<Entry, String> {
    let response = http_client()?
        .get(url)
        .header(USER_AGENT, "DRH-Launcher")
        .send()
        .map_err(|error| format!("Could not fetch {url}: {error}"))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(format!(
            "{url} is not there: publish the release (not a draft) that the release workflow built"
        ));
    }
    let text = response
        .error_for_status()
        .and_then(|response| response.text())
        .map_err(|error| format!("Could not fetch {url}: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("{url} is not an index entry: {error}"))
}

fn page_exists(url: &str) -> Result<bool, String> {
    let response = http_client()?
        .get(url)
        .header(USER_AGENT, "DRH-Launcher")
        .send()
        .map_err(|error| format!("Could not reach {url}: {error}"))?;
    match response.status() {
        status if status.is_success() => Ok(true),
        StatusCode::NOT_FOUND => Ok(false),
        status => Err(format!("{url} answered {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        serde_json::from_value(serde_json::json!({
            "id": "cool_hud", "name": "Cool HUD", "version": "0.2.0", "author": "me",
            "description": "d", "api": "1.0", "uses": [],
            "url": "https://github.com/me/m/releases/download/v0.2.0/cool_hud-0.2.0.zip",
            "size": 1, "sha256": "00", "source": "https://github.com/me/m"
        }))
        .unwrap()
    }

    #[test]
    fn pushes_the_entry_on_a_branch_of_the_fork() {
        let root = tempfile::tempdir().unwrap();
        let seed = root.path().join("seed");
        let upstream = root.path().join("upstream.git");
        let fork = root.path().join("fork.git");
        let text = |path: &Path| path.to_string_lossy().into_owned();
        git(None, &["init", "--quiet", "-b", "master", &text(&seed)]).unwrap();
        fs::write(seed.join("README.md"), "index").unwrap();
        git(Some(&seed), &["add", "README.md"]).unwrap();
        git(
            Some(&seed),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--quiet",
                "-m",
                "seed",
            ],
        )
        .unwrap();
        git(
            None,
            &["clone", "--quiet", "--bare", &text(&seed), &text(&upstream)],
        )
        .unwrap();
        git(
            None,
            &["clone", "--quiet", "--bare", &text(&upstream), &text(&fork)],
        )
        .unwrap();

        let work = root.path().join("work");
        // The commit needs an identity; a test machine may have none.
        unsafe {
            std::env::set_var("GIT_AUTHOR_NAME", "t");
            std::env::set_var("GIT_AUTHOR_EMAIL", "t@t");
            std::env::set_var("GIT_COMMITTER_NAME", "t");
            std::env::set_var("GIT_COMMITTER_EMAIL", "t@t");
        }
        let (base, branch) =
            commit_and_push(&work, &entry(), &text(&upstream), &text(&fork)).unwrap();
        assert_eq!(
            (base.as_str(), branch.as_str()),
            ("master", "cool_hud-0.2.0")
        );
        let pushed = git(
            Some(&fork),
            &["show", "cool_hud-0.2.0:mods/cool_hud/0.2.0.json"],
        )
        .unwrap();
        assert_eq!(pushed + "\n", entry().to_file_text());

        fs::remove_dir_all(&work).unwrap();
        git(
            Some(&upstream),
            &["fetch", "--quiet", &text(&fork), "cool_hud-0.2.0:master"],
        )
        .unwrap();
        let error = commit_and_push(&work, &entry(), &text(&upstream), &text(&fork)).unwrap_err();
        assert_eq!(error, "cool_hud 0.2.0 is already in the index");
    }
}
