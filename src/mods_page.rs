//! The Mods page: the installed mods and the index's catalog, over
//! `mods::library`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::changelog_markdown::markdown_document;
use crate::config::LauncherConfig;
use crate::game_install::inspect_install;
use crate::mods::index::{Entry, Index};
use crate::mods::library::{InstalledMod, Library, Origin, needed_by, unneeded};
use crate::mods::readme;
use crate::mods::repository;
use crate::mods::version::{Version, parse_tags, tag_number};
use crate::{AppWindow, ModCardView, ModTagView, diagnostics, log_for_config, paths};

/// The `api` the launcher builds on: shown without a `uses` label or warning.
const OFFICIAL_API_SOURCE: &str = "https://github.com/Tutez64/DRH-Mod-API";

/// The tabs: Installed is 0.
const CATALOG: usize = 1;

#[derive(Default)]
struct Page {
    index: Option<Index>,
    index_error: Option<String>,
    fetching: bool,
    fetched: bool,
    mods: Vec<InstalledMod>,
    enabled: Vec<String>,
    /// The ids the list shows, in order.
    rows: Vec<String>,
    selected: [Option<String>; 2],
    search: String,
    confirming_uninstall: Option<String>,
    message: String,
    /// The mod an operation is running on, and what it shows meanwhile.
    working: Option<(String, &'static str)>,
}

type Shared = Arc<Mutex<Page>>;

pub(crate) fn register(
    ui: &AppWindow,
    config: &Rc<RefCell<LauncherConfig>>,
    app_shutting_down: &Arc<AtomicBool>,
) {
    let context = Context {
        ui: ui.as_weak(),
        config: Rc::clone(config),
        app_shutting_down: Arc::clone(app_shutting_down),
        page: Arc::default(),
    };

    let on = context.clone();
    ui.on_ensure_mods(move || {
        let view = on.view();
        view.rescan();
        // Nothing installed yet: the Catalog is where to start.
        if view.lock().mods.is_empty()
            && let Some(ui) = view.ui.upgrade()
        {
            ui.set_mods_page(CATALOG as i32);
            ui.set_mod_detail_open(false);
        }
        let fetch = {
            let mut page = view.lock();
            if page.index.is_none() {
                page.index = view
                    .install_dir
                    .as_deref()
                    .and_then(|dir| Index::cached(&paths::mod_index_cache_file(dir)));
            }
            !page.fetched && !page.fetching
        };
        view.render();
        if fetch {
            on.fetch_index();
        }
    });

    let on = context.clone();
    ui.on_refresh_mods(move || {
        let view = on.view();
        view.rescan();
        view.render();
        on.fetch_index();
    });

    let on = context.clone();
    ui.on_select_mods_page(move |tab| {
        let view = on.view();
        if let Some(ui) = view.ui.upgrade() {
            ui.set_mods_page(tab);
            ui.set_mod_detail_open(false);
        }
        view.lock().confirming_uninstall = None;
        view.render();
    });

    let on = context.clone();
    ui.on_select_mod(move |row| {
        let view = on.view();
        if view.select_row(row) {
            if let Some(ui) = view.ui.upgrade() {
                ui.set_mod_detail_open(true);
            }
            view.render();
        }
    });

    let on = context.clone();
    ui.on_toggle_mod(move |row| {
        if on.view().select_row(row) {
            on.primary_action();
        }
    });

    let on = context.clone();
    ui.on_move_mod(move |row, delta| {
        let view = on.view();
        if view.select_row(row) {
            view.move_selected(delta);
        }
    });

    let on = context.clone();
    ui.on_mod_search_edited(move |text| {
        let view = on.view();
        view.lock().search = text.to_string();
        view.render();
    });

    let on = context.clone();
    ui.on_mod_primary_action(move || on.primary_action());

    let on = context.clone();
    ui.on_mod_update(move || {
        let Some(entry) = on.view().selected_update() else {
            return;
        };
        on.in_background(
            (entry.id.clone(), "Updating..."),
            format!("Updating {}...", entry.name),
            move |library, index, tag| {
                let warnings = library.install(&index, &entry, tag)?;
                Ok(with_warnings(
                    format!("Updated {} to {}.", entry.name, entry.version),
                    warnings,
                ))
            },
        );
    });

    let on = context.clone();
    ui.on_mod_uninstall(move || on.view().uninstall());

    let on = context.clone();
    ui.on_open_mod_repository(move || {
        let view = on.view();
        let url = {
            let page = view.lock();
            view.selected_id(&page)
                .and_then(|id| repository_of(&page, &id))
        };
        if let (Some(ui), Some(url)) = (view.ui.upgrade(), url) {
            crate::open_external_link_from_ui(&ui, &on.config.borrow(), "Mod repository", &url);
        }
    });

    let on = context;
    ui.on_open_mods_folder(move || {
        let view = on.view();
        let Some(dir) = view.install_dir.clone() else {
            return;
        };
        let folder = paths::mods_dir(&dir);
        let result = std::fs::create_dir_all(&folder)
            .map_err(|error| format!("Could not create {}: {error}", folder.display()))
            .and_then(|()| crate::open_folder(&folder));
        if let Err(error) = result {
            view.lock().message = error;
            view.render();
        }
    });
}

/// What the callbacks hold, on the UI thread.
#[derive(Clone)]
struct Context {
    ui: slint::Weak<AppWindow>,
    config: Rc<RefCell<LauncherConfig>>,
    app_shutting_down: Arc<AtomicBool>,
    page: Shared,
}

impl Context {
    fn view(&self) -> View {
        View {
            ui: self.ui.clone(),
            page: Arc::clone(&self.page),
            install_dir: self.config.borrow().install_dir.clone(),
        }
    }

    fn fetch_index(&self) {
        let view = self.view();
        let Some(dir) = view.install_dir.clone() else {
            return;
        };
        {
            let mut page = view.lock();
            if page.fetching {
                return;
            }
            page.fetching = true;
        }
        view.render();
        let config = self.config.borrow().clone();
        let app_shutting_down = Arc::clone(&self.app_shutting_down);
        thread::spawn(move || {
            let index = Index::fetch_and_cache(&paths::mod_index_cache_file(&dir));
            if let Err(error) = &index {
                log_for_config(&config, diagnostics::LogLevel::Warn, error);
            }
            // A DRH install, update or rollback since the last resolution:
            // the dependencies again, for the new tag.
            let tag = installed_tag(&dir);
            let library = Library::new(&dir);
            let refreshed = match &index {
                Ok(index) if library.needs_refresh(tag) => {
                    Some(match library.refresh(index, tag) {
                        Ok(warnings) => with_warnings(
                            "Checked the mods' dependencies for the installed DRH.".to_string(),
                            warnings,
                        ),
                        Err(error) => error,
                    })
                }
                _ => None,
            };
            {
                let mut page = view.lock();
                page.fetching = false;
                page.fetched = true;
                match index {
                    Ok(index) => {
                        page.index = Some(index);
                        page.index_error = None;
                    }
                    Err(error) => page.index_error = Some(error),
                }
                if let Some(message) = refreshed {
                    page.message = message;
                }
            }
            crate::invoke_on_event_loop(&config, &app_shutting_down, move || {
                view.rescan();
                view.render();
            });
        });
    }

    /// Runs `operation` on a thread with the index (an empty one when it is
    /// unavailable), then shows its outcome.
    /// `working`: the mod it runs on, and what its card or row shows
    /// meanwhile.
    fn in_background(
        &self,
        working: (String, &'static str),
        progress: String,
        operation: impl FnOnce(&Library, Index, Option<u32>) -> Result<String, String> + Send + 'static,
    ) {
        let view = self.view();
        let Some(dir) = view.install_dir.clone() else {
            return;
        };
        let index = {
            let mut page = view.lock();
            page.message = progress;
            page.confirming_uninstall = None;
            page.working = Some(working);
            Index {
                entries: page
                    .index
                    .as_ref()
                    .map(|index| index.entries.clone())
                    .unwrap_or_default(),
            }
        };
        if let Some(ui) = view.ui.upgrade() {
            ui.set_mods_busy(true);
        }
        view.render();
        let config = self.config.borrow().clone();
        let app_shutting_down = Arc::clone(&self.app_shutting_down);
        thread::spawn(move || {
            let tag = installed_tag(&dir);
            let message = operation(&Library::new(&dir), index, tag).unwrap_or_else(|error| {
                log_for_config(&config, diagnostics::LogLevel::Error, &error);
                error
            });
            {
                let mut page = view.lock();
                page.message = message;
                page.working = None;
            }
            crate::invoke_on_event_loop(&config, &app_shutting_down, move || {
                if let Some(ui) = view.ui.upgrade() {
                    ui.set_mods_busy(false);
                }
                view.rescan();
                view.render();
            });
        });
    }

    fn primary_action(&self) {
        let view = self.view();
        let tab = view.tab();
        let (id, name, entry, enabled, installed) = {
            let page = view.lock();
            let Some(id) = view.selected_id(&page) else {
                return;
            };
            let entry = page
                .index
                .as_ref()
                .and_then(|index| index.versions(&id).first().map(|entry| (*entry).clone()));
            let enabled = page.enabled.contains(&id);
            let installed = page.mods.iter().any(|m| m.id == id);
            let name = names(&page, std::slice::from_ref(&id));
            (id, name, entry, enabled, installed)
        };

        if tab == CATALOG && !installed {
            let Some(entry) = entry else {
                return;
            };
            self.in_background(
                (entry.id.clone(), "Installing..."),
                format!("Installing {}...", entry.name),
                move |library, index, tag| {
                    let warnings = library.install(&index, &entry, tag)?;
                    Ok(with_warnings(
                        format!("Installed {}.", entry.name),
                        warnings,
                    ))
                },
            );
        } else if enabled {
            view.disable(&id);
        } else {
            self.in_background(
                (id.clone(), "Enabling..."),
                format!("Enabling {name}..."),
                move |library, index, tag| {
                    let warnings = library.enable(&index, &id, tag)?;
                    Ok(with_warnings(format!("Enabled {name}."), warnings))
                },
            );
        }
    }
}

/// The page's state and window, which a thread can carry back to the UI
/// thread.
#[derive(Clone)]
struct View {
    ui: slint::Weak<AppWindow>,
    page: Shared,
    install_dir: Option<PathBuf>,
}

impl View {
    fn lock(&self) -> std::sync::MutexGuard<'_, Page> {
        self.page.lock().expect("mods page lock poisoned")
    }

    fn tab(&self) -> usize {
        self.ui
            .upgrade()
            .map_or(0, |ui| usize::from(ui.get_mods_page() == CATALOG as i32))
    }

    /// Selects the list's `row`; false when there is none.
    fn select_row(&self, row: i32) -> bool {
        let tab = self.tab();
        let mut page = self.lock();
        let id = usize::try_from(row)
            .ok()
            .and_then(|row| page.rows.get(row).cloned());
        let found = id.is_some();
        page.selected[tab] = id;
        page.confirming_uninstall = None;
        found
    }

    fn selected_id(&self, page: &Page) -> Option<String> {
        page.selected[self.tab()].clone()
    }

    /// Reads the folder again; the scan drops enabled ids whose folder is
    /// gone.
    fn rescan(&self) {
        let Some(dir) = &self.install_dir else {
            return;
        };
        let library = Library::new(dir);
        let pruned = library.prune_enabled();
        let scanned = library.scan();
        let mut page = self.lock();
        match (pruned, scanned) {
            (Ok(_), Ok(mods)) => page.mods = mods,
            (Err(error), _) | (_, Err(error)) => page.message = error,
        }
        page.enabled = library.enabled();
    }

    fn disable(&self, id: &str) {
        let Some(dir) = &self.install_dir else {
            return;
        };
        let library = Library::new(dir);
        let (before, name) = {
            let page = self.lock();
            (
                unneeded(&page.mods, &page.enabled),
                names(&page, &[id.to_string()]),
            )
        };
        let message = match library.disable(id) {
            Ok(warnings) => {
                let mods = library.scan().unwrap_or_default();
                let now: Vec<String> = unneeded(&mods, &library.enabled())
                    .into_iter()
                    .filter(|other| !before.contains(other))
                    .collect();
                let now: Vec<String> = {
                    let page = self.lock();
                    now.into_iter()
                        .filter(|other| !is_official(&page, other))
                        .collect()
                };
                let now = names_in(&mods, &now);
                let mut message = with_warnings(format!("Disabled {name}."), warnings);
                if !now.is_empty() {
                    message.push_str(&format!(
                        " Installed as a dependency, {} no longer needed: disable or uninstall it from its page.",
                        listed(&now)
                    ));
                }
                message
            }
            Err(error) => error,
        };
        self.lock().message = message;
        self.rescan();
        self.render();
    }

    fn selected_update(&self) -> Option<Entry> {
        let page = self.lock();
        let id = self.selected_id(&page)?;
        let found = page.mods.iter().find(|m| m.id == id)?;
        update_for(&page, found).cloned()
    }

    fn uninstall(&self) {
        let Some(dir) = &self.install_dir else {
            return;
        };
        let (id, name, mods) = {
            let mut page = self.lock();
            let Some(id) = self.selected_id(&page) else {
                return;
            };
            if page.confirming_uninstall.as_deref() != Some(id.as_str()) {
                page.confirming_uninstall = Some(id);
                drop(page);
                self.render();
                return;
            }
            page.confirming_uninstall = None;
            let name = names(&page, std::slice::from_ref(&id));
            (id, name, page.mods.clone())
        };
        let unneeded = Library::new(dir).uninstall(&id).map(|unneeded| {
            let page = self.lock();
            unneeded
                .into_iter()
                .filter(|other| !is_official(&page, other))
                .collect::<Vec<_>>()
        });
        let message = match unneeded {
            Ok(unneeded) if unneeded.is_empty() => format!("Uninstalled {name}."),
            Ok(unneeded) => format!(
                "Uninstalled {name}. Installed as a dependency, {} needed by no mod now: uninstall it from its page.",
                listed(&names_in(&mods, &unneeded))
            ),
            Err(error) => error,
        };
        self.lock().message = message;
        self.rescan();
        self.render();
    }

    fn move_selected(&self, delta: i32) {
        let Some(dir) = &self.install_dir else {
            return;
        };
        if self.ui.upgrade().is_some_and(|ui| ui.get_mods_busy()) {
            return;
        }
        let order = {
            let page = self.lock();
            let Some(id) = self.selected_id(&page) else {
                return;
            };
            let mut order = load_order(&page);
            let Some(at) = order.iter().position(|known| *known == id) else {
                return;
            };
            let Some(to) = at
                .checked_add_signed(delta as isize)
                .filter(|to| *to < order.len() && !is_official(&page, &order[*to]))
            else {
                return;
            };
            order.swap(at, to);
            order
        };
        if let Err(error) = Library::new(dir).set_order(&order) {
            self.lock().message = error;
        }
        self.rescan();
        self.render();
    }

    fn render(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let installed_tag = self.install_dir.as_deref().and_then(installed_tag);
        let tab = self.tab();
        let mut page = self.lock();

        let rows: Vec<(String, ModCardView)> = if tab == CATALOG {
            catalog_rows(&page, installed_tag)
        } else {
            installed_rows(&page, installed_tag)
        };
        page.rows = rows.iter().map(|(id, _)| id.clone()).collect();
        // A mod uninstalled from its page, or filtered out, closes it.
        let selected = page.selected[tab]
            .clone()
            .filter(|id| page.rows.contains(id));
        page.selected[tab] = selected.clone();
        if selected.is_none() {
            ui.set_mod_detail_open(false);
        }

        ui.set_mod_list_empty_text(
            if tab != CATALOG {
                "No mod installed: find some in Catalog."
            } else if page.fetching && page.index.is_none() {
                "Loading the index..."
            } else if page.index.is_none() {
                "The index could not be loaded: Refresh to try again."
            } else {
                "No mod matches."
            }
            .into(),
        );
        ui.set_mod_entries(ModelRc::new(VecModel::from(
            rows.into_iter().map(|(_, view)| view).collect::<Vec<_>>(),
        )));
        ui.set_mods_status(page.message.clone().into());
        let updates = page
            .mods
            .iter()
            .filter(|m| update_for(&page, m).is_some())
            .count();
        ui.set_mods_installed_tab_text(
            match updates {
                0 => "Installed".to_string(),
                1 => "Installed · 1 update".to_string(),
                count => format!("Installed · {count} updates"),
            }
            .into(),
        );
        ui.set_mods_index_status(index_status(&page).into());

        let detail = selected
            .as_deref()
            .map(|id| detail(&page, id, tab == CATALOG, installed_tag))
            .unwrap_or_default();
        ui.set_selected_mod_title(
            if detail.title.is_empty() {
                "No mod selected".to_string()
            } else {
                detail.title
            }
            .into(),
        );
        ui.set_selected_mod_description(detail.description.into());
        ui.set_selected_mod_facts(detail.facts.into());
        ui.set_selected_mod_tags(ModelRc::new(VecModel::from(detail.tags)));
        ui.set_selected_mod_notes(detail.notes.join("\n").into());
        ui.set_selected_mod_blocks(ModelRc::new(VecModel::from(markdown_document(
            &detail.long_description,
        ))));
        ui.set_selected_mod_primary_text(detail.primary.into());
        ui.set_selected_mod_primary_enabled(detail.primary_enabled);
        ui.set_selected_mod_update_text(detail.update.into());
        ui.set_selected_mod_uninstall_visible(detail.installed);
        ui.set_selected_mod_uninstall_confirming(
            selected.is_some() && page.confirming_uninstall == selected,
        );
        ui.set_selected_mod_repository_enabled(detail.repository);
    }
}

/// `a`, `a and b`, `a, b and c`, then `is` or `are`.
fn listed(ids: &[String]) -> String {
    let verb = if ids.len() == 1 { "is" } else { "are" };
    match ids {
        [] => String::new(),
        [one] => format!("{one} {verb}"),
        [rest @ .., last] => format!("{} and {last} {verb}", rest.join(", ")),
    }
}

#[derive(Default)]
struct Detail {
    title: String,
    description: String,
    facts: String,
    notes: Vec<String>,
    tags: Vec<ModTagView>,
    long_description: String,
    primary: String,
    primary_enabled: bool,
    update: String,
    installed: bool,
    repository: bool,
}

fn detail(page: &Page, id: &str, catalog: bool, tag: Option<u32>) -> Detail {
    let installed = page.mods.iter().find(|m| m.id == id);
    let indexed = page
        .index
        .as_ref()
        .and_then(|index| index.versions(id).first().copied());
    let enabled = page.enabled.iter().any(|known| known == id);
    let mut detail = Detail {
        installed: installed.is_some() && !catalog,
        repository: repository_of(page, id).is_some(),
        ..Detail::default()
    };

    let source = source_of(page, id);
    // The Installed tab shows the installed version, the Catalog the indexed
    // one when there is one.
    let shown = installed.filter(|_| !catalog || indexed.is_none());
    let (name, version, author, description, drh, uses) = match (shown, indexed) {
        (Some(m), _) => (
            m.name.clone(),
            m.version.clone(),
            m.author.clone(),
            m.description.clone(),
            m.drh.clone(),
            m.uses.clone(),
        ),
        (_, Some(entry)) => (
            entry.name.clone(),
            entry.version.clone(),
            entry.author.clone(),
            entry.description.clone(),
            entry.drh.clone(),
            entry.uses.clone(),
        ),
        (None, None) => return detail,
    };
    detail.title = format!("{name}  {version}");
    detail.description = description;

    let mut facts = vec![format!("by {}", author_text(&author, source.as_deref()))];
    if let Some(m) = installed {
        facts.push(origin_text(m.origin.as_ref()).to_string());
    }
    detail.facts = facts.join(" · ");

    let issues = match shown {
        Some(m) => issues(page, m, tag),
        None => drh_issue(drh.as_deref(), &uses, tag).into_iter().collect(),
    };
    detail.notes = issues.iter().map(|issue| issue.message.clone()).collect();
    detail.tags = tags(
        page,
        id,
        &uses,
        installed.map(|m| m.origin.as_ref()),
        &issues,
    );

    if let Some(m) = installed.filter(|_| !is_official(page, id)) {
        let needers: Vec<String> = needed_by(&page.mods, id)
            .into_iter()
            .filter(|other| page.enabled.contains(other))
            .collect();
        if m.dependency {
            if free_dependencies(page).iter().any(|free| free == id) {
                detail.notes.push(
                    "Installed as a dependency, no enabled mod needs it any more.".to_string(),
                );
            } else {
                detail.notes.push(format!(
                    "Installed as a dependency of {}.",
                    names(page, &needed_by(&page.mods, id))
                ));
            }
        } else if !needers.is_empty() {
            detail
                .notes
                .push(format!("Needed by {}.", names(page, &needers)));
        }
        if !enabled && !needers.is_empty() {
            detail.notes.push(format!(
                "Disabled: the game fails {} until it is enabled.",
                names(page, &needers)
            ));
        }
    }

    detail.long_description = match (installed, indexed) {
        (Some(m), _) if !catalog => indexed
            .filter(|entry| entry.version == m.version && m.origin == Some(Origin::Index))
            .map(|entry| entry.long_description.clone())
            .unwrap_or_else(|| readme_text(&m.folder)),
        (_, Some(entry)) => entry.long_description.clone(),
        (Some(m), None) => readme_text(&m.folder),
        (None, None) => String::new(),
    };

    let update = installed.and_then(|m| update_for(page, m));
    match (catalog, installed) {
        (true, None) => {
            detail.primary = "Install".to_string();
            detail.primary_enabled = indexed.is_some();
        }
        (true, Some(_)) => {
            detail.primary = "Installed".to_string();
            detail.update = update
                .map(|entry| format!("Update to {}", entry.version))
                .unwrap_or_default();
        }
        _ => {
            detail.primary = if enabled { "Disable" } else { "Enable" }.to_string();
            detail.primary_enabled = true;
            detail.update = update
                .map(|entry| format!("Update to {}", entry.version))
                .unwrap_or_default();
        }
    }
    detail
}

/// Something that may keep a mod from working: a short label for its tag,
/// and the full message.
struct Issue {
    label: String,
    message: String,
}

/// What may keep an installed mod from working. Its dependencies only
/// matter while it is enabled: enabling it installs them.
fn issues(page: &Page, m: &InstalledMod, tag: Option<u32>) -> Vec<Issue> {
    let mut issues = Vec::new();
    if let Some(problem) = &m.problem {
        issues.push(Issue {
            label: "mod.json".to_string(),
            message: format!("{problem}: the game may fail it."),
        });
    }
    if let Some(reason) = yanked_reason(page, m) {
        issues.push(Issue {
            label: "withdrawn".to_string(),
            message: format!("This version was withdrawn from the index: {reason}"),
        });
    }
    issues.extend(drh_issue(m.drh.as_deref(), &m.uses, tag));
    if !page.enabled.contains(&m.id) {
        return issues;
    }
    for (need, requirement) in &m.needs {
        let name = names(page, std::slice::from_ref(need));
        let issue = match page.mods.iter().find(|other| other.id == *need) {
            None => Issue {
                label: format!("needs {name}"),
                message: format!(
                    "Needs {name} {requirement}, which is not installed: the game fails it."
                ),
            },
            Some(_) if !page.enabled.contains(need) => Issue {
                label: format!("needs {name}"),
                message: format!("Needs {name}, which is disabled: the game fails it."),
            },
            Some(other)
                if !Version::parse(&other.version)
                    .is_some_and(|version| requirement.matches(version)) =>
            {
                Issue {
                    label: format!("{name} {}", other.version),
                    message: format!(
                        "Needs {name} {requirement}, and {name} {} is installed: it may not work.",
                        other.version
                    ),
                }
            }
            Some(_) => continue,
        };
        issues.push(issue);
    }
    issues
}

/// An `extends` / `replace` mod built for other DRH tags than the installed
/// one.
fn drh_issue(drh: Option<&str>, uses: &[String], tag: Option<u32>) -> Option<Issue> {
    let tag = tag?;
    if uses.is_empty() {
        return None;
    }
    let built_for = drh.and_then(parse_tags);
    if built_for.is_some_and(|tags| tags.contains(&tag)) {
        return None;
    }
    let drh = drh.unwrap_or("no tag");
    Some(Issue {
        label: format!("DRH {drh}"),
        message: format!("Built for DRH {drh}, and DRH {tag} is installed: it may not work."),
    })
}

/// The enabled mods in load order, the official `api` first: the game starts
/// a mod after what it needs anyway, and every mod needs it.
fn load_order(page: &Page) -> Vec<String> {
    let (mut order, rest): (Vec<String>, Vec<String>) = page
        .enabled
        .iter()
        .cloned()
        .partition(|id| is_official(page, id));
    order.extend(rest);
    order
}

fn installed_rows(page: &Page, tag: Option<u32>) -> Vec<(String, ModCardView)> {
    let order = load_order(page);
    let mut mods: Vec<&InstalledMod> = page.mods.iter().collect();
    // Enabled first, in load order; then the others by name.
    mods.sort_by_key(|m| {
        let at = order.iter().position(|id| *id == m.id);
        (at.is_none(), at, m.name.to_lowercase())
    });
    let free = free_dependencies(page);
    mods.into_iter()
        .map(|m| {
            let official = is_official(page, &m.id);
            let summary = if official || !m.dependency {
                m.description.clone()
            } else if free.contains(&m.id) {
                "Installed as a dependency, no longer needed".to_string()
            } else {
                format!(
                    "Installed as a dependency of {}",
                    names(page, &needed_by(&page.mods, &m.id))
                )
            };
            let issues = issues(page, m, tag);
            let at = order.iter().position(|id| *id == m.id);
            let first_movable = order.iter().position(|id| !is_official(page, id));
            let status = match &page.working {
                Some((id, doing)) if *id == m.id => doing,
                _ if update_for(page, m).is_some() => "Update available",
                _ => "",
            };
            (
                m.id.clone(),
                ModCardView {
                    name: m.name.clone().into(),
                    version: m.version.clone().into(),
                    summary: summary.into(),
                    author: author_text(&m.author, source_of(page, &m.id).as_deref()).into(),
                    tags: ModelRc::new(VecModel::from(tags(
                        page,
                        &m.id,
                        &m.uses,
                        Some(m.origin.as_ref()),
                        &issues,
                    ))),
                    status: status.into(),
                    enabled: at.is_some(),
                    orderable: at.is_some() && !official,
                    can_move_up: at.is_some() && at > first_movable,
                    can_move_down: at.is_some_and(|at| at + 1 < order.len()),
                },
            )
        })
        .collect()
}

fn catalog_rows(page: &Page, tag: Option<u32>) -> Vec<(String, ModCardView)> {
    let Some(index) = &page.index else {
        return Vec::new();
    };
    let search = page.search.trim().to_lowercase();
    let mut ids: Vec<&str> = index
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut rows: Vec<(String, ModCardView)> = ids
        .into_iter()
        .filter_map(|id| index.versions(id).first().copied())
        .filter(|entry| {
            let owner = repository::owner(&entry.source).unwrap_or_default();
            search.is_empty()
                || [&entry.name, &entry.id, &entry.author, &owner.to_string()]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&search))
        })
        .map(|entry| {
            let installed = page.mods.iter().find(|m| m.id == entry.id);
            let status = match (&page.working, installed) {
                (Some((id, doing)), _) if *id == entry.id => doing,
                (_, Some(m)) if update_for(page, m).is_some() => "Update available",
                (_, Some(_)) => "Installed",
                (_, None) => "",
            };
            let issues: Vec<Issue> = drh_issue(entry.drh.as_deref(), &entry.uses, tag)
                .into_iter()
                .collect();
            (
                entry.id.clone(),
                ModCardView {
                    name: entry.name.clone().into(),
                    version: entry.version.clone().into(),
                    summary: entry.description.clone().into(),
                    author: author_text(&entry.author, Some(&entry.source)).into(),
                    tags: ModelRc::new(VecModel::from(tags(
                        page,
                        &entry.id,
                        &entry.uses,
                        None,
                        &issues,
                    ))),
                    status: status.into(),
                    enabled: installed.is_some_and(|m| page.enabled.contains(&m.id)),
                    orderable: false,
                    can_move_up: false,
                    can_move_down: false,
                },
            )
        })
        .collect();
    rows.sort_by_key(|(_, view)| view.name.to_lowercase());
    rows
}

/// The `uses` tag (not on the official `api`), "not reviewed" for an
/// installed mod that is not from the index, then the issues.
fn tags(
    page: &Page,
    id: &str,
    uses: &[String],
    installed_from: Option<Option<&Origin>>,
    issues: &[Issue],
) -> Vec<ModTagView> {
    let mut tags = Vec::new();
    if !is_official(page, id) {
        let (text, tooltip) = match uses_label(uses) {
            "replace" => (
                "replace",
                "Rewrites parts of DRH's code: it may break when DRH updates, and clash with other mods that rewrite the same parts.",
            ),
            "extends" => (
                "extends",
                "Builds on DRH's own code: it may break when DRH updates.",
            ),
            _ => (
                "api",
                "Only uses the api: it should keep working across DRH updates.",
            ),
        };
        tags.push(tag_view(text, tooltip, false));
    }
    if let Some(origin) = installed_from
        && origin != Some(&Origin::Index)
    {
        tags.push(tag_view(
            "not reviewed",
            &format!("{}: no one reviewed it.", origin_text(origin)),
            false,
        ));
    }
    tags.extend(
        issues
            .iter()
            .map(|issue| tag_view(&format!("⚠ {}", issue.label), &issue.message, true)),
    );
    tags
}

fn tag_view(text: &str, tooltip: &str, warning: bool) -> ModTagView {
    ModTagView {
        text: text.into(),
        tooltip: tooltip.into(),
        warning,
    }
}

fn is_official(page: &Page, id: &str) -> bool {
    source_of(page, id).as_deref() == Some(OFFICIAL_API_SOURCE)
}

/// The mods installed as a dependency that nothing enabled needs, but the
/// official `api`, which needs no explaining.
fn free_dependencies(page: &Page) -> Vec<String> {
    unneeded(&page.mods, &page.enabled)
        .into_iter()
        .filter(|id| !is_official(page, id))
        .collect()
}

/// The highest indexed version every enabled mod needing it accepts, when it
/// is newer than the installed one. Only for a mod from the index.
fn update_for<'a>(page: &'a Page, installed: &InstalledMod) -> Option<&'a Entry> {
    if installed.origin != Some(Origin::Index) {
        return None;
    }
    let current = Version::parse(&installed.version)?;
    let requirements: Vec<_> = page
        .mods
        .iter()
        .filter(|m| page.enabled.contains(&m.id))
        .flat_map(|m| m.needs.iter())
        .filter(|(id, _)| *id == installed.id)
        .map(|(_, requirement)| *requirement)
        .collect();
    page.index
        .as_ref()?
        .versions(&installed.id)
        .into_iter()
        .find(|entry| {
            entry.parsed_version().is_some_and(|version| {
                requirements
                    .iter()
                    .all(|requirement| requirement.matches(version))
            })
        })
        .filter(|entry| entry.parsed_version() > Some(current))
}

fn yanked_reason(page: &Page, installed: &InstalledMod) -> Option<String> {
    page.index
        .as_ref()?
        .entries
        .iter()
        .find(|entry| entry.id == installed.id && entry.version == installed.version)?
        .yanked
        .clone()
}

/// The repository a mod comes from: its index entry's `source`, or the link
/// it was installed from.
fn source_of(page: &Page, id: &str) -> Option<String> {
    let installed = page.mods.iter().find(|m| m.id == id);
    if let Some(Some(Origin::Repository { url })) = installed.map(|m| &m.origin) {
        return Some(url.clone());
    }
    if let Some(m) = installed {
        if m.origin != Some(Origin::Index) {
            return None;
        }
        if let Some(source) = &m.source {
            return Some(source.clone());
        }
    }
    page.index
        .as_ref()?
        .entries
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.source.clone())
}

fn repository_of(page: &Page, id: &str) -> Option<String> {
    source_of(page, id).filter(|url| url.starts_with("https://"))
}

/// `author`, then the repository's owner when it differs.
fn author_text(author: &str, source: Option<&str>) -> String {
    match source.and_then(repository::owner) {
        Some(owner) if !owner.eq_ignore_ascii_case(author) => format!("{author} ({owner})"),
        _ => author.to_string(),
    }
}

/// The strongest of `uses`; stable only reads as `api`.
fn uses_label(uses: &[String]) -> &'static str {
    if uses.iter().any(|kind| kind == "replace") {
        "replace"
    } else if uses.iter().any(|kind| kind == "extends") {
        "extends"
    } else {
        "api"
    }
}

fn origin_text(origin: Option<&Origin>) -> &'static str {
    match origin {
        Some(Origin::Index) => "From the index",
        Some(Origin::Zip) => "From a zip, not index-reviewed",
        Some(Origin::Repository { .. }) => "From its repository, not index-reviewed",
        None => "Copied into the mods folder, not index-reviewed",
    }
}

/// The mods' names, joined.
fn names(page: &Page, ids: &[String]) -> String {
    names_in(&page.mods, ids).join(", ")
}

/// Each id's name, the id when it is not installed.
fn names_in(mods: &[InstalledMod], ids: &[String]) -> Vec<String> {
    ids.iter()
        .map(|id| {
            mods.iter()
                .find(|m| m.id == *id)
                .map_or_else(|| id.clone(), |m| m.name.clone())
        })
        .collect()
}

/// What its README shows: all of it when its markers are malformed.
fn readme_text(folder: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(folder.join("README.md")) else {
        return String::new();
    };
    readme::shown(&text).unwrap_or(text)
}

fn index_status(page: &Page) -> String {
    let count = page.index.as_ref().map(|index| {
        let mut ids: Vec<&str> = index
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    });
    match (page.fetching, &page.index_error, count) {
        (true, _, _) => "Loading the index...".to_string(),
        (false, Some(_), Some(count)) => format!("Index offline: {count} mods, last copy"),
        (false, Some(_), None) => "Index unavailable".to_string(),
        (false, None, Some(count)) => format!("Index: {count} mods"),
        (false, None, None) => String::new(),
    }
}

fn with_warnings(message: String, warnings: Vec<String>) -> String {
    if warnings.is_empty() {
        message
    } else {
        format!("{message} {}", warnings.join(" "))
    }
}

fn installed_tag(install_dir: &Path) -> Option<u32> {
    inspect_install(Some(install_dir))
        .installed_version
        .as_deref()
        .and_then(tag_number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::version::Requirement;

    fn installed(id: &str, version: &str, needs: &[(&str, &str)]) -> InstalledMod {
        InstalledMod {
            id: id.to_string(),
            folder: PathBuf::from(id),
            name: id.to_string(),
            version: version.to_string(),
            author: "me".to_string(),
            description: String::new(),
            drh: None,
            uses: Vec::new(),
            needs: needs
                .iter()
                .map(|(id, text)| (id.to_string(), Requirement::parse(text).unwrap()))
                .collect(),
            problem: None,
            origin: Some(Origin::Index),
            dependency: false,
            source: None,
        }
    }

    fn entry(id: &str, version: &str) -> Entry {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "version": version, "author": "me",
            "description": "d", "url": "https://example.com/m.zip",
            "size": 1, "sha256": "00", "source": "https://github.com/me/DRH-Mod-M"
        }))
        .unwrap()
    }

    #[test]
    fn offers_the_newest_version_the_enabled_mods_accept() {
        let page = Page {
            index: Some(Index {
                entries: vec![
                    entry("lib", "0.1.0"),
                    entry("lib", "0.1.2"),
                    entry("lib", "0.2.0"),
                ],
            }),
            mods: vec![
                installed("lib", "0.1.0", &[]),
                installed("mine", "1.0.0", &[("lib", "0.1")]),
            ],
            enabled: vec!["lib".to_string(), "mine".to_string()],
            ..Page::default()
        };
        let update = update_for(&page, &page.mods[0]).unwrap();
        assert_eq!(update.version, "0.1.2");

        let mut hand_copied = page.mods[0].clone();
        hand_copied.origin = None;
        assert!(update_for(&page, &hand_copied).is_none());
    }

    #[test]
    fn shows_the_owner_next_to_a_different_author() {
        let source = Some("https://github.com/Tutez64/DRH-Mod-API");
        assert_eq!(author_text("Tutez", source), "Tutez (Tutez64)");
        assert_eq!(author_text("tutez64", source), "tutez64");
        assert_eq!(author_text("Someone", None), "Someone");
    }

    #[test]
    fn lists_names_in_a_sentence() {
        let ids = |list: &[&str]| list.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        assert_eq!(listed(&ids(&["API"])), "API is");
        assert_eq!(listed(&ids(&["A", "B", "C"])), "A, B and C are");
    }
}
