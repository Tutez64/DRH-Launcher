//! `--run-mod`: the mod's own run, with its dependencies from the index.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use super::index::Index;
use super::install::cached_mod;
use super::last_run;
use super::manifest::{MOD_DATA, read_needs};
use super::resolve::resolve;
use super::version::tag_number;
use crate::archive::extract_zip;
use crate::config::LauncherConfig;
use crate::game_install::inspect_install;
use crate::install_state::InstallState;
use crate::launch_options::load_installed_launch_options;
use crate::{game_launch, game_logs, paths};

/// What the game needs of the mod under test; the rest of its folder stays.
const MOD_FILES: [&str; 5] = ["mod.json", "src", "Resources", "README.md", "LICENSE"];

/// Runs DRH with the mod at `path` (its folder or its zip) and what it needs,
/// shows the game's output, then each mod's outcome.
pub fn run_mod(path: &Path) -> Result<(), String> {
    let (mut config, warning) = LauncherConfig::load_with_diagnostics();
    if let Some(warning) = warning {
        println!("warning: {warning}");
    }
    let install_dir = config.ensure_install_dir();
    let install = inspect_install(Some(&install_dir));
    if !matches!(
        install.state,
        InstallState::Installed | InstallState::LaunchableButMaybeOutdated
    ) {
        return Err(format!(
            "DRH is not ready to launch ({}): install it from DRH Launcher.",
            install.status_text()
        ));
    }
    let installed_tag = install.installed_version.as_deref().and_then(tag_number);

    let source = if path.is_file() {
        let unpacked = paths::data_dir(&install_dir).join("run-mod-zip");
        recreate(&unpacked)?;
        extract_zip(path, &unpacked)?;
        unpacked
    } else {
        path.to_path_buf()
    };
    let manifest_text = fs::read_to_string(source.join("mod.json"))
        .map_err(|error| format!("{}: no readable mod.json: {error}", path.display()))?;
    let (id, needs) = read_needs(&manifest_text)?;

    let index = Index::fetch()?;
    let resolution = resolve(
        &index,
        &[(id.clone(), needs)],
        &BTreeMap::new(),
        installed_tag,
    )?;
    for warning in &resolution.warnings {
        println!("warning: {warning}");
    }
    if !resolution.missing.is_empty() {
        return Err(resolution.missing.join("\n"));
    }

    let mods_dir = paths::run_mod_dir(&install_dir);
    clear_but(&mods_dir, MOD_DATA)?;
    let cache = paths::mod_cache_dir(&install_dir);
    let mut enabled = Vec::new();
    for entry in &resolution.entries {
        println!("{} {}", entry.id, entry.version);
        copy_tree(&cached_mod(entry, &cache)?, &mods_dir.join(&entry.id))?;
        enabled.push(entry.id.clone());
    }
    for name in MOD_FILES {
        let from = source.join(name);
        if from.exists() {
            copy_tree(&from, &mods_dir.join(&id).join(name))?;
        }
    }
    enabled.push(id.clone());
    let list = serde_json::json!({
        "mods": enabled.iter().map(|id| serde_json::json!({ "id": id })).collect::<Vec<_>>()
    });
    fs::write(mods_dir.join("enabled.json"), format!("{list:#}\n"))
        .map_err(|error| format!("Could not write enabled.json: {error}"))?;

    with_mods_dir(&mut config.game_args, &mods_dir);
    let launch_options = load_installed_launch_options(&config);
    let mut game = game_launch::launch_game_with_options(
        &config,
        launch_options.as_ref(),
        install.installed_version.as_deref(),
    )?;
    println!(
        "DRH started; session log: {}",
        game.session_log.path.display()
    );

    let mut log = File::open(&game.session_log.path).ok();
    let status = loop {
        print_new_output(log.as_mut());
        match game.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(error) => break Err(error),
        }
    };
    print_new_output(log.as_mut());
    let result = match &status {
        Ok(status) => format!("Exited with status: {status}"),
        Err(error) => format!("Could not wait for DRH: {error}"),
    };
    let _ = game_logs::finish(&game.session_log, &result);
    println!("{result}");

    report_last_run(&mods_dir, &id)
}

/// The user's own `--mods-dir`, if any, gives way: the game keeps the first.
fn with_mods_dir(args: &mut Vec<String>, mods_dir: &Path) {
    let mut kept = Vec::new();
    let mut skip_value = false;
    for arg in args.drain(..) {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        if arg == "--mods-dir" {
            skip_value = true;
        } else if !arg.starts_with("--mods-dir=") {
            kept.push(arg);
        }
    }
    kept.push("--mods-dir".to_string());
    kept.push(mods_dir.display().to_string());
    *args = kept;
}

fn print_new_output(log: Option<&mut File>) {
    let Some(log) = log else {
        return;
    };
    let mut chunk = Vec::new();
    if log.read_to_end(&mut chunk).is_ok() && !chunk.is_empty() {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(&chunk);
        let _ = stdout.flush();
    }
}

/// Each mod's outcome; an error unless the mod under test is `ok`.
fn report_last_run(mods_dir: &Path, id: &str) -> Result<(), String> {
    let path = mods_dir.join("last-run.json");
    let text = fs::read_to_string(&path)
        .map_err(|_| "No last-run.json: the game stopped before loading the mods.".to_string())?;
    let last_run = last_run::parse(&text)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    println!();
    for record in &last_run.mods {
        let mut line = format!("{} {}: {}", record.id, record.version, record.status);
        if let Some(mode) = &record.mode {
            line.push_str(&format!(" ({mode})"));
        }
        if let Some(error) = &record.error {
            line.push_str(&format!(": {error}"));
        }
        println!("{line}");
    }
    if !last_run.ready {
        println!("The game stopped before it was ready.");
    }
    match last_run.mods.iter().find(|record| record.id == id) {
        Some(record) if record.status == "ok" => Ok(()),
        Some(record) => Err(format!("{id}: {}", record.status)),
        None => Err(format!("{id} is not in last-run.json")),
    }
}

fn recreate(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        fs::remove_dir_all(dir)
            .map_err(|error| format!("Could not clear {}: {error}", dir.display()))?;
    }
    fs::create_dir_all(dir).map_err(|error| format!("Could not create {}: {error}", dir.display()))
}

/// Empties `dir` but its `kept` entry, creating it if needed.
fn clear_but(dir: &Path, kept: &str) -> Result<(), String> {
    fs::create_dir_all(dir)
        .map_err(|error| format!("Could not create {}: {error}", dir.display()))?;
    let entries =
        fs::read_dir(dir).map_err(|error| format!("Could not read {}: {error}", dir.display()))?;
    for entry in entries.filter_map(Result::ok) {
        if entry.file_name() == kept {
            continue;
        }
        let path = entry.path();
        let removed = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        removed.map_err(|error| format!("Could not clear {}: {error}", path.display()))?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        fs::create_dir_all(to)
            .map_err(|error| format!("Could not create {}: {error}", to.display()))?;
        let entries = fs::read_dir(from)
            .map_err(|error| format!("Could not read {}: {error}", from.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("Could not read {}: {error}", from.display()))?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
        }
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|error| format!("Could not copy {}: {error}", from.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_a_run_but_what_the_mods_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path().join("run-mod");
        fs::create_dir_all(mods.join("data/mine")).unwrap();
        fs::write(mods.join("data/mine/settings.json"), "{}").unwrap();
        fs::create_dir_all(mods.join("mine/src")).unwrap();
        fs::write(mods.join("enabled.json"), "{}").unwrap();

        clear_but(&mods, MOD_DATA).unwrap();
        let left: Vec<_> = fs::read_dir(&mods)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["data"]);
        assert!(mods.join("data/mine/settings.json").is_file());
    }

    #[test]
    fn replaces_the_users_mods_dir() {
        let mut args: Vec<String> = [
            "--fps=60",
            "--mods-dir",
            "/mine",
            "--mods-dir=/other",
            "--debug",
        ]
        .map(String::from)
        .to_vec();
        with_mods_dir(&mut args, Path::new("/run"));
        assert_eq!(args, ["--fps=60", "--debug", "--mods-dir", "/run"]);
    }

    #[test]
    fn reports_the_mod_under_test() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("last-run.json"),
            r#"{"drh":"14","ready":true,"mods":[
                {"id":"api","version":"1.0.0","status":"ok","mode":"compiled"},
                {"id":"mine","version":"0.1.0","status":"failed","error":"boom"}]}"#,
        )
        .unwrap();
        assert_eq!(report_last_run(dir.path(), "api"), Ok(()));
        assert_eq!(
            report_last_run(dir.path(), "mine"),
            Err("mine: failed".to_string())
        );
        assert!(report_last_run(dir.path(), "other").is_err());
    }
}
