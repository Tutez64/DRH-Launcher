//! The Mods page: the installed mods and the index's catalog, over
//! `mods::library`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

use crate::changelog_markdown::markdown_document;
use crate::config::LauncherConfig;
use crate::diagnostics::{self, LogLevel};
use crate::game_install::inspect_install;
use crate::game_launch::RunningGame;
use crate::game_runtime::process_is_running;
use crate::mods::index::{Entry, Index};
use crate::mods::last_run::{self, LastRun};
use crate::mods::library::{InstalledMod, Library, Origin, Plan, needed_by, unneeded};
use crate::mods::readme;
use crate::mods::repository;
use crate::mods::resolve::offered;
use crate::mods::version::{Version, parse_tags, tag_number};
use crate::{AppWindow, ModCardView, ModTagKind, ModTagView, log_for_config, paths};

/// The `api` the launcher builds on: shown without a `uses` label or warning.
const OFFICIAL_API_SOURCE: &str = "https://github.com/Tutez64/DRH-Mod-API";

/// The tabs: Installed is 0.
const CATALOG: usize = 1;

/// The window's Mods page.
const MODS_PAGE: i32 = 3;

const MOD_GUIDE_URL: &str = concat!(
    "https://github.com/Tutez64/Dungeon-Rampage-Haxe/blob/master/docs/making-a-mod.md",
    "?utm_source=drh_launcher&utm_medium=mods_page",
    "&utm_campaign=project_links&utm_content=making_a_mod",
);

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
    // Each tab as it was left: its search, whether a mod's page is open,
    // and where the list and the page were scrolled.
    selected: [Option<String>; 2],
    search: [String; 2],
    detail_open: [bool; 2],
    list_scroll: [f32; 2],
    detail_scroll: [f32; 2],
    /// The last action's failure, until dismissed or an action succeeds.
    error: Option<String>,
    /// An action waiting for the user to confirm it.
    confirm: Option<Confirm>,
    /// The mod an operation is running on, and what it shows meanwhile.
    working: Option<(String, &'static str)>,
    /// The installed DRH tag's number.
    tag: Option<u32>,
    /// Every installed mod in the user's order, the disabled ones in their
    /// place.
    order: Vec<String>,
    game: Game,
    /// The game's report on the mods it started, read with the folder.
    last_run: Option<LastRun>,
    /// The latest release of each mod installed from its repository, read
    /// with the index.
    releases: BTreeMap<String, Entry>,
    /// Counts the reads of a link or zip, so a cancelled one is dropped.
    elsewhere: u64,
    /// What the last one gave.
    elsewhere_found: Option<Result<Elsewhere, String>>,
}

/// The last game this launcher started with its mods folder.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Game {
    /// None this session: a report present is from an earlier one.
    #[default]
    None,
    Running,
    Exited,
}

type Shared = Arc<Mutex<Page>>;

/// What waits for the user to go past the notice on playing with mods.
type Deferred = Box<dyn FnOnce()>;

/// What the user asked for on a mod.
#[derive(Clone)]
enum Action {
    Install(Entry),
    Update(Entry),
    Enable(String),
    Disable(String),
    Uninstall(String),
    InstallFrom(Elsewhere),
}

/// A mod not from the index.
#[derive(Clone)]
enum Elsewhere {
    /// Its repository's latest release.
    Release(Entry),
    /// Extracted from a zip, not installed yet.
    Zip(InstalledMod),
}

impl Elsewhere {
    fn id(&self) -> &str {
        match self {
            Elsewhere::Release(entry) => &entry.id,
            Elsewhere::Zip(staged) => &staged.id,
        }
    }

    fn name_and_version(&self) -> String {
        match self {
            Elsewhere::Release(entry) => format!("{} {}", entry.name, entry.version),
            Elsewhere::Zip(staged) => format!("{} {}", staged.name, staged.version),
        }
    }
}

struct Confirm {
    action: Action,
    title: String,
    text: String,
    button: String,
}

/// The page, for the game launch to tell it about a game.
pub(crate) struct ModsPage(Context);

impl ModsPage {
    /// Runs `then` once the user went past the notice on playing with mods,
    /// showing it first if they never did.
    pub(crate) fn disclose_then(&self, then: impl FnOnce() + 'static) {
        self.0.disclose_then(then);
    }

    /// A game was just started: with the launcher's mods folder, the page
    /// follows it until it exits, reading its report.
    pub(crate) fn game_launched(&self, with_mods: bool) {
        if !with_mods {
            return;
        }
        let context = &self.0;
        {
            let mut page = context.page.lock().expect("mods page lock poisoned");
            page.game = Game::Running;
            page.last_run = None;
        }
        let on = context.clone();
        context
            .game_poll
            .start(TimerMode::Repeated, GAME_POLL_INTERVAL, move || {
                on.follow_game()
            });
    }
}

/// How often the page reads the report while the game runs.
const GAME_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn register(
    ui: &AppWindow,
    config: &Rc<RefCell<LauncherConfig>>,
    app_shutting_down: &Arc<AtomicBool>,
    game_process: &Rc<RefCell<Option<RunningGame>>>,
) -> ModsPage {
    let context = Context {
        ui: ui.as_weak(),
        config: Rc::clone(config),
        app_shutting_down: Arc::clone(app_shutting_down),
        page: Arc::default(),
        game_process: Rc::clone(game_process),
        game_poll: Rc::default(),
        after_disclosure: Rc::default(),
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
        let Some(ui) = view.ui.upgrade() else {
            return;
        };
        let (Ok(new), old) = (usize::try_from(tab), view.tab()) else {
            return;
        };
        {
            let mut page = view.lock();
            page.detail_open[old] = ui.get_mod_detail_open();
            page.list_scroll[old] = ui.get_mod_list_scroll();
            page.detail_scroll[old] = ui.get_mod_detail_scroll();
        }
        ui.set_mods_page(tab);
        let page = view.lock();
        ui.set_mod_search(page.search[new].clone().into());
        ui.set_mod_detail_open(page.detail_open[new]);
        ui.set_mod_list_scroll(page.list_scroll[new]);
        ui.set_mod_detail_scroll(page.detail_scroll[new]);
        drop(page);
        view.render();
    });

    let on = context.clone();
    ui.on_select_mod(move |row| {
        let view = on.view();
        if view.select_row(row) {
            if let Some(ui) = view.ui.upgrade() {
                ui.set_mod_detail_open(true);
                ui.set_mod_detail_scroll(0.0);
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
        let tab = view.tab();
        view.lock().search[tab] = text.to_string();
        view.render();
    });

    let on = context.clone();
    ui.on_mod_primary_action(move || on.primary_action());

    let on = context.clone();
    ui.on_mod_update(move || {
        if let Some(entry) = on.view().selected_update() {
            on.ask(Action::Update(entry));
        }
    });

    let on = context.clone();
    ui.on_mod_uninstall(move || {
        let view = on.view();
        let id = view.selected_id(&view.lock());
        if let Some(id) = id {
            on.ask(Action::Uninstall(id));
        }
    });

    let on = context.clone();
    ui.on_mod_confirm_accepted(move || {
        let view = on.view();
        let confirm = view.lock().confirm.take();
        view.render();
        if let Some(confirm) = confirm {
            on.run(confirm.action);
        }
    });

    let on = context.clone();
    ui.on_mod_confirm_cancelled(move || {
        let view = on.view();
        let confirm = view.lock().confirm.take();
        if let (Some(dir), Some(Action::InstallFrom(Elsewhere::Zip(_)))) =
            (&view.install_dir, confirm.map(|confirm| confirm.action))
        {
            Library::new(dir).discard_staged();
        }
        view.render();
    });

    let on = context.clone();
    ui.on_mod_install_from_link(move |link| {
        let Some(ui) = on.ui.upgrade() else {
            return;
        };
        let Some(source) = repository::from_link(&link) else {
            ui.set_mod_elsewhere_error(
                "That is not a GitHub repository's link, such as https://github.com/owner/DRH-Mod-Name."
                    .into(),
            );
            return;
        };
        on.read_elsewhere(
            format!("Reading the latest release of {source}..."),
            move |_| repository::latest_entry(&source).map(Elsewhere::Release),
        );
    });

    let on = context.clone();
    ui.on_mod_install_from_zip(move || {
        let Some(ui) = on.ui.upgrade() else {
            return;
        };
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Choose a mod's zip")
            .add_filter("Zip", &["zip"])
            .set_parent(&ui.window().window_handle());
        let on = on.clone();
        let _ = slint::spawn_local(async move {
            if let Some(file) = dialog.pick_file().await {
                let zip = file.path().to_path_buf();
                on.read_elsewhere("Reading the zip...".to_string(), move |library| {
                    library.stage_zip(&zip).map(Elsewhere::Zip)
                });
            }
        });
    });

    let on = context.clone();
    ui.on_mod_elsewhere_cancelled(move || {
        on.view().lock().elsewhere += 1;
        if let Some(ui) = on.ui.upgrade() {
            ui.set_mod_elsewhere_visible(false);
        }
    });

    let on = context.clone();
    ui.on_mod_elsewhere_ready(move || {
        let view = on.view();
        let (Some(ui), Some(found)) = (view.ui.upgrade(), view.lock().elsewhere_found.take())
        else {
            return;
        };
        ui.set_mod_elsewhere_working("".into());
        match found {
            Ok(found) => {
                ui.set_mod_elsewhere_visible(false);
                on.ask(Action::InstallFrom(found));
            }
            Err(error) => {
                if let Some(dir) = &view.install_dir {
                    log(dir, LogLevel::Warn, &format!("Mods: {error}"));
                }
                ui.set_mod_elsewhere_error(error.into());
            }
        }
    });

    let on = context.clone();
    ui.on_open_mod_guide(move || {
        if let Some(ui) = on.ui.upgrade() {
            crate::open_external_link_from_ui(
                &ui,
                &on.config.borrow(),
                "Making a mod",
                MOD_GUIDE_URL,
            );
        }
    });

    let on = context.clone();
    ui.on_mods_disclosure_continue(move || {
        {
            let mut config = on.config.borrow_mut();
            config.mods_disclosure_confirmed = true;
            if let Err(error) = config.save() {
                log_for_config(&config, diagnostics::LogLevel::Error, &error.to_string());
            }
        }
        if let Some(ui) = on.ui.upgrade() {
            ui.set_mods_disclosure_visible(false);
        }
        let then = on.after_disclosure.borrow_mut().take();
        if let Some(then) = then {
            then();
        }
    });

    let on = context.clone();
    ui.on_mods_disclosure_close(move || {
        on.after_disclosure.borrow_mut().take();
        if let Some(ui) = on.ui.upgrade() {
            ui.set_mods_disclosure_visible(false);
        }
    });

    let on = context.clone();
    ui.on_dismiss_mods_error(move || {
        let view = on.view();
        view.lock().error = None;
        view.render();
    });

    let on = context.clone();
    ui.on_open_mod_folder(move || {
        let view = on.view();
        let folder = {
            let page = view.lock();
            view.selected_id(&page)
                .and_then(|id| page.mods.iter().find(|m| m.id == id))
                .map(|m| m.folder.clone())
        };
        if let Some(Err(error)) = folder.map(|folder| crate::open_folder(&folder)) {
            view.fail(error);
            view.render();
        }
    });

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

    let on = context.clone();
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
            view.fail(error);
            view.render();
        }
    });

    ModsPage(context)
}

/// What the callbacks hold, on the UI thread.
#[derive(Clone)]
struct Context {
    ui: slint::Weak<AppWindow>,
    config: Rc<RefCell<LauncherConfig>>,
    app_shutting_down: Arc<AtomicBool>,
    page: Shared,
    game_process: Rc<RefCell<Option<RunningGame>>>,
    /// Runs while a game started with the mods folder does.
    game_poll: Rc<Timer>,
    /// What the notice on playing with mods holds back until it is
    /// confirmed.
    after_disclosure: Rc<RefCell<Option<Deferred>>>,
}

impl Context {
    fn disclose_then(&self, then: impl FnOnce() + 'static) {
        if self.config.borrow().mods_disclosure_confirmed {
            then();
            return;
        }
        *self.after_disclosure.borrow_mut() = Some(Box::new(then));
        if let Some(ui) = self.ui.upgrade() {
            ui.set_mods_disclosure_asking(true);
            ui.set_mods_disclosure_visible(true);
        }
    }

    /// Reads the report again while the page is shown, and once the game
    /// has exited.
    fn follow_game(&self) {
        let view = self.view();
        let running = process_is_running(&self.game_process);
        if !running {
            view.lock().game = Game::Exited;
            self.game_poll.stop();
        }
        let shown = view
            .ui
            .upgrade()
            .is_some_and(|ui| ui.get_active_page() == MODS_PAGE);
        if !running || shown {
            view.read_last_run();
            view.render();
        }
    }

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
                log(&dir, LogLevel::Warn, error);
            }
            // A DRH install, update or rollback since the last resolution:
            // the dependencies again, for the new tag.
            let tag = installed_tag(&dir);
            let library = Library::new(&dir);
            let refreshed = match &index {
                Ok(index) if library.needs_refresh(tag) => Some(library.refresh(index, tag)),
                _ => None,
            };
            if let Some(Ok(warnings)) = &refreshed {
                log(
                    &dir,
                    LogLevel::Info,
                    "Mods: resolved the dependencies again for the installed DRH.",
                );
                for warning in warnings {
                    log(&dir, LogLevel::Warn, &format!("Mods: {warning}"));
                }
            }
            let releases = latest_releases(&dir, &library);
            {
                let mut page = view.lock();
                page.releases = releases;
                page.fetching = false;
                page.fetched = true;
                match index {
                    Ok(index) => {
                        page.index = Some(index);
                        page.index_error = None;
                    }
                    Err(error) => page.index_error = Some(error),
                }
            }
            if let Some(Err(error)) = refreshed {
                view.fail(error);
            }
            crate::invoke_on_event_loop(&config, &app_shutting_down, move || {
                view.rescan();
                view.render();
            });
        });
    }

    /// Reads a mod from a link or a zip on a thread, `working` showing
    /// meanwhile; `mod-elsewhere-ready` takes it.
    fn read_elsewhere(
        &self,
        working: String,
        read: impl FnOnce(&Library) -> Result<Elsewhere, String> + Send + 'static,
    ) {
        let view = self.view();
        let (Some(dir), Some(ui)) = (view.install_dir.clone(), view.ui.upgrade()) else {
            return;
        };
        ui.set_mod_elsewhere_working(working.into());
        ui.set_mod_elsewhere_error("".into());
        let attempt = {
            let mut page = view.lock();
            page.elsewhere += 1;
            page.elsewhere
        };
        let config = self.config.borrow().clone();
        let app_shutting_down = Arc::clone(&self.app_shutting_down);
        thread::spawn(move || {
            let library = Library::new(&dir);
            let found = read(&library);
            {
                let mut page = view.lock();
                if page.elsewhere != attempt {
                    if let Ok(Elsewhere::Zip(_)) = found {
                        library.discard_staged();
                    }
                    return;
                }
                page.elsewhere_found = Some(found);
            }
            crate::invoke_on_event_loop(&config, &app_shutting_down, move || {
                if let Some(ui) = view.ui.upgrade() {
                    ui.invoke_mod_elsewhere_ready();
                }
            });
        });
    }

    /// Runs `action` once the user has seen what it would break, and has
    /// confirmed an uninstall or a mod from elsewhere.
    fn ask(&self, action: Action) {
        // Installing or enabling is playing with mods from then on.
        if matches!(
            action,
            Action::Install(_) | Action::Enable(_) | Action::InstallFrom(_)
        ) && !self.config.borrow().mods_disclosure_confirmed
        {
            let on = self.clone();
            self.disclose_then(move || on.ask(action));
            return;
        }
        let view = self.view();
        let Some(dir) = view.install_dir.clone() else {
            return;
        };
        let asked = {
            let page = view.lock();
            consequences(&page, &Library::new(&dir), &action)
                .map(|consequences| confirmation(&page, &action, &consequences))
        };
        match asked {
            Ok(None) => self.run(action),
            Ok(Some(confirm)) => {
                view.lock().confirm = Some(confirm);
                view.render();
            }
            Err(error) => {
                view.fail(error);
                view.render();
            }
        }
    }

    fn run(&self, action: Action) {
        let view = self.view();
        match action {
            Action::Install(entry) => self.in_background(
                (entry.id.clone(), "Installing..."),
                move |library, index, tag| {
                    let warnings = library.install(&index, &entry, tag)?;
                    Ok((
                        format!("Installed {} {}.", entry.name, entry.version),
                        warnings,
                    ))
                },
            ),
            Action::Update(entry) => self.in_background(
                (entry.id.clone(), "Updating..."),
                move |library, index, tag| {
                    let warnings = library.install(&index, &entry, tag)?;
                    Ok((
                        format!("Updated {} to {}.", entry.name, entry.version),
                        warnings,
                    ))
                },
            ),
            Action::Enable(id) => {
                let name = names(&view.lock(), std::slice::from_ref(&id));
                self.in_background((id.clone(), "Enabling..."), move |library, index, tag| {
                    let warnings = library.enable(&index, &id, tag)?;
                    Ok((format!("Enabled {name}."), warnings))
                });
            }
            Action::Disable(id) => view.disable(&id),
            Action::Uninstall(id) => view.uninstall(&id),
            Action::InstallFrom(found) => {
                // It shows there once installed.
                if let Some(ui) = view.ui.upgrade()
                    && view.tab() == CATALOG
                {
                    ui.invoke_select_mods_page(0);
                }
                let working = (found.id().to_string(), "Installing...");
                let done = format!("Installed {}", found.name_and_version());
                match found {
                    Elsewhere::Release(entry) => {
                        self.in_background(working, move |library, index, tag| {
                            let warnings = library.install_from_repository(&index, &entry, tag)?;
                            Ok((format!("{done} from {}.", entry.source), warnings))
                        });
                    }
                    Elsewhere::Zip(staged) => {
                        self.in_background(working, move |library, index, tag| {
                            let warnings = library.install_staged(&index, &staged, tag)?;
                            Ok((format!("{done} from a zip."), warnings))
                        });
                    }
                }
            }
        }
    }

    /// Runs `operation` on a thread with the index (an empty one when it is
    /// unavailable), then logs what it did and its warnings, or its error.
    /// `working`: the mod it runs on, and what its card, row or button shows
    /// meanwhile.
    fn in_background(
        &self,
        working: (String, &'static str),
        operation: impl FnOnce(&Library, Index, Option<u32>) -> Result<(String, Vec<String>), String>
        + Send
        + 'static,
    ) {
        let view = self.view();
        let Some(dir) = view.install_dir.clone() else {
            return;
        };
        let index = {
            let mut page = view.lock();
            page.working = Some(working);
            page.index.clone().unwrap_or(Index {
                entries: Vec::new(),
            })
        };
        if let Some(ui) = view.ui.upgrade() {
            ui.set_mods_busy(true);
        }
        view.render();
        let config = self.config.borrow().clone();
        let app_shutting_down = Arc::clone(&self.app_shutting_down);
        thread::spawn(move || {
            let tag = installed_tag(&dir);
            match operation(&Library::new(&dir), index, tag) {
                Ok((done, warnings)) => view.succeed(&done, &warnings),
                Err(error) => view.fail(error),
            }
            view.lock().working = None;
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
        let action = {
            let page = view.lock();
            let Some(id) = view.selected_id(&page) else {
                return;
            };
            if !page.mods.iter().any(|m| m.id == id) {
                let Some(entry) = offered_entry(&page, &id) else {
                    return;
                };
                Action::Install(entry.clone())
            } else if page.enabled.contains(&id) {
                Action::Disable(id)
            } else {
                Action::Enable(id)
            }
        };
        self.ask(action);
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
        let order = library.order();
        match (pruned, scanned, order) {
            (Ok(_), Ok(mods), Ok(order)) => {
                let mut page = self.lock();
                page.mods = mods;
                page.order = order;
            }
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => self.fail(error),
        }
        let mut page = self.lock();
        page.enabled = library.enabled();
        page.tag = installed_tag(dir);
        drop(page);
        self.read_last_run();
    }

    fn read_last_run(&self) {
        if let Some(dir) = &self.install_dir {
            self.lock().last_run = last_run::read(&paths::mods_dir(dir));
        }
    }

    /// Logs what an action did and its warnings, and clears the last
    /// failure.
    fn succeed(&self, done: &str, warnings: &[String]) {
        if let Some(dir) = &self.install_dir {
            log(dir, LogLevel::Info, &format!("Mods: {done}"));
            for warning in warnings {
                log(dir, LogLevel::Warn, &format!("Mods: {warning}"));
            }
        }
        self.lock().error = None;
    }

    /// Logs an action's failure and shows it above the list.
    fn fail(&self, error: String) {
        if let Some(dir) = &self.install_dir {
            log(dir, LogLevel::Error, &format!("Mods: {error}"));
        }
        self.lock().error = Some(error);
    }

    fn disable(&self, id: &str) {
        let Some(dir) = &self.install_dir else {
            return;
        };
        let name = names(&self.lock(), &[id.to_string()]);
        match Library::new(dir).disable(id) {
            Ok(warnings) => self.succeed(&format!("Disabled {name}."), &warnings),
            Err(error) => self.fail(error),
        }
        self.rescan();
        self.render();
    }

    fn selected_update(&self) -> Option<Entry> {
        let page = self.lock();
        let id = self.selected_id(&page)?;
        let found = page.mods.iter().find(|m| m.id == id)?;
        update_for(&page, found).cloned()
    }

    fn uninstall(&self, id: &str) {
        let Some(dir) = &self.install_dir else {
            return;
        };
        let name = names(&self.lock(), &[id.to_string()]);
        match Library::new(dir).uninstall(id) {
            Ok(()) => self.succeed(&format!("Uninstalled {name}."), &[]),
            Err(error) => self.fail(error),
        }
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
            let mut order = list_order(&page);
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
        match Library::new(dir).set_order(&order) {
            Ok(()) => self.succeed(&format!("Order: {}.", order.join(", ")), &[]),
            Err(error) => self.fail(error),
        }
        self.rescan();
        self.render();
    }

    fn render(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let tab = self.tab();
        let mut page = self.lock();
        let installed_tag = page.tag;

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
            if tab != CATALOG && page.mods.is_empty() {
                "No mod installed: find some in Catalog, or under More mods."
            } else if tab != CATALOG {
                "No mod matches."
            } else if page.fetching && page.index.is_none() {
                "Loading the index..."
            } else if page.index.is_none() {
                "The index could not be loaded: Refresh to try again, or see More mods."
            } else {
                "No mod matches. Not in the catalog? See More mods."
            }
            .into(),
        );
        ui.set_mod_entries(ModelRc::new(VecModel::from(
            rows.into_iter().map(|(_, view)| view).collect::<Vec<_>>(),
        )));
        ui.set_mods_error(page.error.clone().unwrap_or_default().into());
        let (run_text, run_warning) = run_summary(&page);
        ui.set_mods_run_text(run_text.into());
        ui.set_mods_run_warning(run_warning);
        let confirm = page.confirm.as_ref();
        ui.set_mod_confirm_visible(confirm.is_some());
        ui.set_mod_confirm_title(confirm.map(|c| c.title.clone()).unwrap_or_default().into());
        ui.set_mod_confirm_text(confirm.map(|c| c.text.clone()).unwrap_or_default().into());
        ui.set_mod_confirm_button(confirm.map(|c| c.button.clone()).unwrap_or_default().into());
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
            .map(|id| detail(&page, id, installed_tag))
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
        ui.set_selected_mod_author(detail.author.into());
        ui.set_selected_mod_owner(detail.owner.into());
        ui.set_selected_mod_folder_visible(detail.installed);
        ui.set_selected_mod_tags(ModelRc::new(VecModel::from(detail.tags)));
        ui.set_selected_mod_blocks(ModelRc::new(VecModel::from(markdown_document(
            &detail.long_description,
        ))));
        ui.set_selected_mod_primary_text(detail.primary.into());
        ui.set_selected_mod_primary_enabled(detail.primary_enabled);
        ui.set_selected_mod_update_text(detail.update.into());
        ui.set_selected_mod_uninstall_visible(detail.installed);
        ui.set_selected_mod_repository_enabled(detail.repository);
    }
}

#[derive(Default)]
struct Detail {
    title: String,
    description: String,
    author: String,
    owner: String,
    tags: Vec<ModTagView>,
    long_description: String,
    primary: String,
    primary_enabled: bool,
    update: String,
    installed: bool,
    repository: bool,
}

/// A mod's page, the same from either tab: the installed version, which is
/// what runs, else the one the index offers.
fn detail(page: &Page, id: &str, tag: Option<u32>) -> Detail {
    let installed = page.mods.iter().find(|m| m.id == id);
    let indexed = offered_entry(page, id);
    let enabled = page.enabled.iter().any(|known| known == id);
    let mut detail = Detail {
        installed: installed.is_some(),
        repository: repository_of(page, id).is_some(),
        ..Detail::default()
    };

    let source = source_of(page, id);
    let (name, version, author, description, drh, uses) = match (installed, indexed) {
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

    (detail.author, detail.owner) = byline(&author, source.as_deref());

    let issues = match installed {
        Some(m) => issues(page, m, tag),
        None => drh_issue(drh.as_deref(), &uses, tag).into_iter().collect(),
    };
    detail.tags = uses_tag(page, id, &uses)
        .into_iter()
        .chain(installed.map_or_else(Vec::new, |m| dependency_tags(page, m)))
        .chain(other_tags(installed.map(|m| m.origin.as_ref()), &issues))
        .chain(installed.map_or_else(Vec::new, |m| run_tags(page, m)))
        .collect();

    detail.long_description = match (installed, indexed) {
        // The index's text when the installed version came from it.
        (Some(m), _) => installed_entry(page, m)
            .filter(|_| m.origin == Some(Origin::Index))
            .map(|entry| entry.long_description.clone())
            .unwrap_or_else(|| readme_text(&m.folder)),
        (None, Some(entry)) => entry.long_description.clone(),
        (None, None) => String::new(),
    };

    match installed {
        None => {
            detail.primary = "Install".to_string();
            detail.primary_enabled = indexed.is_some();
        }
        Some(m) => {
            detail.primary = if enabled { "Disable" } else { "Enable" }.to_string();
            detail.primary_enabled = true;
            detail.update = update_for(page, m)
                .map(|entry| format!("Update to {}", entry.version))
                .unwrap_or_default();
        }
    }
    // What runs on it shows on the button that started it.
    if let Some((_, doing)) = page.working.as_ref().filter(|(working, _)| working == id) {
        if *doing == "Updating..." {
            detail.update = doing.to_string();
        } else {
            detail.primary = doing.to_string();
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

/// Every installed mod in the user's order, the official `api` first: the
/// game starts a mod after what it needs anyway, and every mod needs it.
fn list_order(page: &Page) -> Vec<String> {
    let installed = |id: &String| page.mods.iter().any(|m| m.id == *id);
    let mut order: Vec<String> = page
        .order
        .iter()
        .filter(|id| installed(id))
        .cloned()
        .collect();
    let mut rest: Vec<&InstalledMod> = page
        .mods
        .iter()
        .filter(|m| !order.contains(&m.id))
        .collect();
    rest.sort_by_key(|m| m.name.to_lowercase());
    order.extend(rest.into_iter().map(|m| m.id.clone()));
    let (mut first, rest): (Vec<String>, Vec<String>) =
        order.into_iter().partition(|id| is_official(page, id));
    first.extend(rest);
    first
}

fn installed_rows(page: &Page, tag: Option<u32>) -> Vec<(String, ModCardView)> {
    let order = list_order(page);
    let search = &page.search[0];
    // Moving swaps with the neighbour in the whole order, which a search may
    // hide.
    let reorderable = search.trim().is_empty();
    let mut mods: Vec<&InstalledMod> = page
        .mods
        .iter()
        .filter(|m| matches(search, &m.name, &m.id, source_of(page, &m.id).as_deref()))
        .collect();
    mods.sort_by_key(|m| order.iter().position(|id| *id == m.id));
    mods.into_iter()
        .map(|m| {
            let official = is_official(page, &m.id);
            let issues = issues(page, m, tag);
            let (author, owner) = byline(&m.author, source_of(page, &m.id).as_deref());
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
                    summary: m.description.clone().into(),
                    author: author.into(),
                    owner: owner.into(),
                    uses: uses_tag(page, &m.id, &m.uses).unwrap_or_default(),
                    tags: ModelRc::new(VecModel::from(
                        dependency_tags(page, m)
                            .into_iter()
                            .chain(other_tags(Some(m.origin.as_ref()), &issues))
                            .chain(run_tags(page, m))
                            .collect::<Vec<_>>(),
                    )),
                    status: status.into(),
                    enabled: page.enabled.contains(&m.id),
                    orderable: !official && reorderable,
                    can_move_up: at > first_movable,
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
    let mut ids: Vec<&str> = index
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut rows: Vec<(String, ModCardView)> = ids
        .into_iter()
        .filter_map(|id| offered_entry(page, id))
        .filter(|entry| {
            matches(
                &page.search[CATALOG],
                &entry.name,
                &entry.id,
                Some(&entry.source),
            )
        })
        .map(|entry| {
            let installed = page.mods.iter().find(|m| m.id == entry.id);
            let status = match (&page.working, installed) {
                (Some((id, doing)), _) if *id == entry.id => doing,
                (_, Some(m)) if update_for(page, m).is_some() => "Update available",
                (_, Some(_)) => "Installed",
                (_, None) => "",
            };
            let (author, owner) = byline(&entry.author, Some(&entry.source));
            let issues: Vec<Issue> = drh_issue(entry.drh.as_deref(), &entry.uses, tag)
                .into_iter()
                .collect();
            (
                entry.id.clone(),
                ModCardView {
                    name: entry.name.clone().into(),
                    version: entry.version.clone().into(),
                    summary: entry.description.clone().into(),
                    author: author.into(),
                    owner: owner.into(),
                    uses: uses_tag(page, &entry.id, &entry.uses).unwrap_or_default(),
                    tags: ModelRc::new(VecModel::from(other_tags(None, &issues))),
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

/// What the mod is to the others: needed by an enabled mod, or installed as
/// a dependency and needed no more. None on the official `api`.
fn dependency_tags(page: &Page, m: &InstalledMod) -> Vec<ModTagView> {
    if is_official(page, &m.id) {
        return Vec::new();
    }
    let needers: Vec<String> = needed_by(&page.mods, &m.id)
        .into_iter()
        .filter(|other| page.enabled.contains(other))
        .collect();
    let enabled = page.enabled.contains(&m.id);
    let tag = if !needers.is_empty() && !enabled {
        tag_view(
            "⚠ needed",
            &format!(
                "Disabled: the game fails {} until it is enabled.",
                names(page, &needers)
            ),
            ModTagKind::Problem,
        )
    } else if !needers.is_empty() {
        let tooltip = if m.dependency {
            format!("Installed as a dependency of {}.", names(page, &needers))
        } else {
            format!("Needed by {}.", names(page, &needers))
        };
        tag_view("dependency", &tooltip, ModTagKind::Info)
    } else if free_dependencies(page).contains(&m.id) {
        let tooltip = if enabled {
            "Installed as a dependency, and no enabled mod needs it any more: disable or uninstall it."
        } else {
            "Installed as a dependency, and no enabled mod needs it any more: uninstall it."
        };
        tag_view("not needed", tooltip, ModTagKind::Info)
    } else {
        return Vec::new();
    };
    vec![tag]
}

/// What the mod builds on; none on the official `api`.
fn uses_tag(page: &Page, id: &str, uses: &[String]) -> Option<ModTagView> {
    if is_official(page, id) {
        return None;
    }
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
    Some(tag_view(text, tooltip, ModTagKind::Info))
}

/// "not reviewed" for an installed mod that is not from the index, then the
/// issues.
fn other_tags(installed_from: Option<Option<&Origin>>, issues: &[Issue]) -> Vec<ModTagView> {
    let mut tags = Vec::new();
    if let Some(origin) = installed_from
        && origin != Some(&Origin::Index)
    {
        let updated = matches!(origin, Some(Origin::Repository { .. }));
        tags.push(tag_view(
            "not reviewed",
            &format!(
                "{}: no one has reviewed its code{}. Only keep it if you trust where it comes from.",
                origin_text(origin),
                if updated {
                    ""
                } else {
                    ", and the launcher does not update it"
                }
            ),
            ModTagKind::Risk,
        ));
    }
    tags.extend(issues.iter().map(|issue| {
        tag_view(
            &format!("⚠ {}", issue.label),
            &issue.message,
            ModTagKind::Problem,
        )
    }));
    tags
}

fn tag_view(text: &str, tooltip: &str, kind: ModTagKind) -> ModTagView {
    ModTagView {
        text: text.into(),
        tooltip: tooltip.into(),
        kind,
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

/// The version Install picks: the highest, one built for the installed DRH
/// tag first.
fn offered_entry<'a>(page: &'a Page, id: &str) -> Option<&'a Entry> {
    offered(page.index.as_ref()?, id, &[], page.tag)
}

/// The version the index offers with what every enabled mod needing it
/// accepts, when it is newer than the installed one; for a mod from its
/// repository, that repository's latest release.
fn update_for<'a>(page: &'a Page, installed: &InstalledMod) -> Option<&'a Entry> {
    let current = Version::parse(&installed.version)?;
    // One from its repository: that repository's latest release.
    if let Some(Origin::Repository { .. }) = installed.origin {
        return page
            .releases
            .get(&installed.id)
            .filter(|entry| entry.parsed_version() > Some(current));
    }
    if installed.origin != Some(Origin::Index) {
        return None;
    }
    let needed_by: Vec<_> = page
        .mods
        .iter()
        .filter(|m| page.enabled.contains(&m.id))
        .flat_map(|m| {
            m.needs
                .iter()
                .filter(|(id, _)| *id == installed.id)
                .map(|(_, requirement)| (m.id.clone(), *requirement))
        })
        .collect();
    offered(page.index.as_ref()?, &installed.id, &needed_by, page.tag)
        .filter(|entry| entry.parsed_version() > Some(current))
}

/// The index entry with the installed version's id and version.
fn installed_entry<'a>(page: &'a Page, installed: &InstalledMod) -> Option<&'a Entry> {
    page.index
        .as_ref()?
        .entries
        .iter()
        .find(|entry| entry.id == installed.id && entry.version == installed.version)
}

fn yanked_reason(page: &Page, installed: &InstalledMod) -> Option<String> {
    installed_entry(page, installed)?.yanked.clone()
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

/// The author to show, then the GitHub account of the mod's repository: the
/// author alone without one, the account alone when they are the same.
fn byline(author: &str, source: Option<&str>) -> (String, String) {
    match source.and_then(repository::owner) {
        Some(owner) if owner.eq_ignore_ascii_case(author) => (String::new(), owner.to_string()),
        Some(owner) => (author.to_string(), owner.to_string()),
        None => (author.to_string(), String::new()),
    }
}

/// Whether `search` is in the mod's name, id or GitHub account; not its
/// `author`, which anyone can write.
fn matches(search: &str, name: &str, id: &str, source: Option<&str>) -> bool {
    let search = search.trim().to_lowercase();
    let owner = source.and_then(repository::owner).unwrap_or_default();
    search.is_empty()
        || [name, id, owner]
            .iter()
            .any(|text| text.to_lowercase().contains(&search))
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
        Some(Origin::Index) => "Installed from the index",
        Some(Origin::Zip) => "Installed from a zip",
        Some(Origin::Repository { .. }) => "Installed from its GitHub repository",
        None => "Copied into the mods folder",
    }
}

/// The mods' names, joined: the installed one's, else the index's, else
/// the id.
fn names(page: &Page, ids: &[String]) -> String {
    ids.iter()
        .map(|id| {
            let installed = page.mods.iter().find(|m| m.id == *id).map(|m| &m.name);
            let indexed = || {
                page.index
                    .as_ref()?
                    .entries
                    .iter()
                    .find(|entry| entry.id == *id)
                    .map(|entry| &entry.name)
            };
            installed.or_else(indexed).unwrap_or(id).clone()
        })
        .collect::<Vec<_>>()
        .join(", ")
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

/// What an action changes besides its own mod, and the problems it adds:
/// each mod's issues it does not have yet, by label.
#[derive(Debug, Default)]
struct Consequences {
    /// Each other mod it installs, updates or enables.
    also: Vec<String>,
    problems: Vec<String>,
}

fn consequences(page: &Page, library: &Library, action: &Action) -> Result<Consequences, String> {
    let empty = Index {
        entries: Vec::new(),
    };
    let index = page.index.as_ref().unwrap_or(&empty);
    let without = |id: &str| -> Vec<String> {
        page.enabled
            .iter()
            .filter(|known| *known != id)
            .cloned()
            .collect()
    };
    let plan = match action {
        Action::Install(entry) | Action::Update(entry) => {
            Some((&entry.id, library.plan_install(index, entry, page.tag)?))
        }
        Action::Enable(id) => Some((id, library.plan_enable(index, id, page.tag)?)),
        Action::InstallFrom(Elsewhere::Release(entry)) => {
            Some((&entry.id, library.plan_install(index, entry, page.tag)?))
        }
        Action::InstallFrom(Elsewhere::Zip(staged)) => {
            Some((&staged.id, library.plan_staged(index, staged, page.tag)?))
        }
        Action::Disable(_) | Action::Uninstall(_) => None,
    };
    let also = plan
        .as_ref()
        .map(|(id, plan)| others_changed(page, id, plan))
        .unwrap_or_default();
    let (mut mods, enabled) = match (action, plan) {
        (_, Some((_, plan))) => planned(page, plan),
        (Action::Uninstall(id), None) => (
            page.mods.iter().filter(|m| m.id != *id).cloned().collect(),
            without(id),
        ),
        // Disable.
        (_, None) => (page.mods.clone(), without(id_of(action))),
    };
    if let Action::InstallFrom(Elsewhere::Zip(staged)) = action {
        mods.retain(|m| m.id != staged.id);
        mods.push(staged.clone());
    }
    let after = Page {
        index: page.index.clone(),
        mods,
        enabled,
        tag: page.tag,
        ..Page::default()
    };
    let problems = after
        .mods
        .iter()
        .flat_map(|m| {
            let known: Vec<String> = page
                .mods
                .iter()
                .find(|before| before.id == m.id)
                .map(|before| {
                    issues(page, before, page.tag)
                        .into_iter()
                        .map(|issue| issue.label)
                        .collect()
                })
                .unwrap_or_default();
            issues(&after, m, after.tag)
                .into_iter()
                .filter(move |issue| !known.contains(&issue.label))
                .map(move |issue| format!("{}: {}", m.name, issue.message))
        })
        .collect();
    Ok(Consequences { also, problems })
}

fn id_of(action: &Action) -> &str {
    match action {
        Action::Install(entry) | Action::Update(entry) => &entry.id,
        Action::Enable(id) | Action::Disable(id) | Action::Uninstall(id) => id,
        Action::InstallFrom(found) => found.id(),
    }
}

/// The mods `plan` installs, updates or enables besides `id`, but the
/// official `api`, which every mod needs.
fn others_changed(page: &Page, id: &str, plan: &Plan) -> Vec<String> {
    let mut also: Vec<String> = plan
        .entries
        .iter()
        .filter(|(entry, _)| entry.id != id && entry.source != OFFICIAL_API_SOURCE)
        .map(
            |(entry, _)| match page.mods.iter().find(|m| m.id == entry.id) {
                Some(m) => format!("updates {} to {}", m.name, entry.version),
                None => format!("installs {} {}", entry.name, entry.version),
            },
        )
        .collect();
    also.extend(
        plan.enabled
            .iter()
            .filter(|other| {
                *other != id && !page.enabled.contains(other) && !is_official(page, other)
            })
            .filter_map(|other| page.mods.iter().find(|m| m.id == *other))
            .map(|m| format!("enables {}", m.name)),
    );
    also
}

/// The installed mods and enabled ids once `plan` is carried out.
fn planned(page: &Page, plan: Plan) -> (Vec<InstalledMod>, Vec<String>) {
    let mut mods = page.mods.clone();
    for (entry, as_dependency) in &plan.entries {
        match mods.iter_mut().find(|m| m.id == entry.id) {
            // An update keeps how the mod first came.
            Some(m) => *m = InstalledMod::planned(entry, m.dependency),
            None => mods.push(InstalledMod::planned(entry, *as_dependency)),
        }
    }
    (mods, plan.enabled)
}

/// What to ask before `action`: always before an uninstall or installing a
/// mod from elsewhere, else only when it changes other mods or adds
/// problems.
fn confirmation(page: &Page, action: &Action, consequences: &Consequences) -> Option<Confirm> {
    let Consequences { also, problems } = consequences;
    let uninstall = matches!(action, Action::Uninstall(_));
    let elsewhere = matches!(action, Action::InstallFrom(_));
    if also.is_empty() && problems.is_empty() && !uninstall && !elsewhere {
        return None;
    }
    let (title, verb) = match action {
        Action::Install(entry) => (format!("Install {}?", entry.name), "Install"),
        Action::Update(entry) => (
            format!("Update {} to {}?", entry.name, entry.version),
            "Update",
        ),
        Action::Enable(id) => (
            format!("Enable {}?", names(page, std::slice::from_ref(id))),
            "Enable",
        ),
        Action::Disable(id) => (
            format!("Disable {}?", names(page, std::slice::from_ref(id))),
            "Disable",
        ),
        Action::Uninstall(id) => (
            format!("Uninstall {}?", names(page, std::slice::from_ref(id))),
            "Uninstall",
        ),
        Action::InstallFrom(found) => (format!("Install {}?", found.name_and_version()), "Install"),
    };
    let mut text = match action {
        Action::Uninstall(_) => "Its folder is deleted from the mods folder.".to_string(),
        Action::InstallFrom(found) => {
            let mut text =
                "No one has reviewed its code: only install it if you trust where it comes from."
                    .to_string();
            if let Some(old) = page.mods.iter().find(|m| m.id == found.id()) {
                text.push_str(&match (&old.origin, found) {
                    (Some(Origin::Index), Elsewhere::Zip(_)) => format!(
                        " It replaces {} {} from the catalog, which the launcher will no longer update.",
                        old.name, old.version
                    ),
                    (Some(Origin::Index), Elsewhere::Release(_)) => format!(
                        " It replaces {} {} from the catalog: the launcher will update it from this repository instead.",
                        old.name, old.version
                    ),
                    _ => format!(" It replaces the installed {} {}.", old.name, old.version),
                });
            }
            text
        }
        _ => String::new(),
    };
    if !also.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("This also:");
        for change in also {
            text.push_str(&format!("\n• {change}"));
        }
    }
    if !problems.is_empty() {
        if !text.is_empty() {
            // After an uninstall's one sentence, in the same paragraph.
            text.push_str(if uninstall && also.is_empty() {
                " "
            } else {
                "\n\n"
            });
        }
        text.push_str("This leaves these problems:");
        for problem in problems {
            text.push_str(&format!("\n• {problem}"));
        }
    }
    Some(Confirm {
        action: action.clone(),
        title,
        text,
        button: if problems.is_empty() {
            verb.to_string()
        } else {
            format!("{verb} anyway")
        },
    })
}

/// The game's report, unless it was written by another DRH than the
/// installed one.
fn report(page: &Page) -> Option<&LastRun> {
    page.last_run.as_ref().filter(|report| {
        let written_by = report
            .drh
            .as_deref()
            .and_then(|drh| drh.parse::<u32>().ok());
        page.tag.is_none() || written_by.is_none() || written_by == page.tag
    })
}

/// How the last game went with the mods, and whether that is a warning;
/// empty when there is nothing to say.
fn run_summary(page: &Page) -> (String, bool) {
    let stale = page.last_run.is_some() && report(page).is_none();
    match (page.game, report(page)) {
        _ if stale => (String::new(), false),
        (Game::Running, None) => ("Game starting...".to_string(), false),
        (Game::Running, Some(report)) if !report.ready => (
            format!("Game loading: {} started.", count(report.mods.len(), "mod")),
            false,
        ),
        (Game::Running, Some(report)) => (format!("Game running: {}.", outcome(report)), false),
        (Game::Exited, None) => (
            "The last game stopped before loading the mods.".to_string(),
            true,
        ),
        (_, Some(report)) if !report.ready => (
            "The last game stopped before it was ready.".to_string(),
            true,
        ),
        (_, Some(report)) => (
            format!("Last game: {}.", outcome(report)),
            report.mods.iter().any(|run| run.status != "ok"),
        ),
        (Game::None, None) => (String::new(), false),
    }
}

/// `N mods loaded`, then how many failed or were skipped.
fn outcome(report: &LastRun) -> String {
    let with = |status: &str| {
        report
            .mods
            .iter()
            .filter(|run| run.status == status)
            .count()
    };
    let mut text = format!("{} loaded", count(with("ok"), "mod"));
    for (status, label) in [("failed", "failed"), ("skipped", "skipped")] {
        if with(status) > 0 {
            text.push_str(&format!(", {} {label}", with(status)));
        }
    }
    text
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// What the last game reported of `m`: only while it is the version that
/// ran, and enabled.
fn run_tags(page: &Page, m: &InstalledMod) -> Vec<ModTagView> {
    let Some(run) = report(page)
        .and_then(|report| report.mods.iter().find(|run| run.id == m.id))
        .filter(|run| run.version == m.version && page.enabled.contains(&m.id))
    else {
        return Vec::new();
    };
    let reason = |text: &str| match &run.error {
        Some(error) => format!("{text}: {error}"),
        None => format!("{text}."),
    };
    let mut tags = Vec::new();
    match run.status.as_str() {
        "failed" => tags.push(tag_view(
            "⚠ failed",
            &reason("The last game failed it"),
            ModTagKind::Problem,
        )),
        "skipped" => tags.push(tag_view(
            "⚠ skipped",
            &reason("The last game skipped it: its folder or mod.json was missing"),
            ModTagKind::Problem,
        )),
        _ => {}
    }
    match run.mode.as_deref() {
        Some("interpreted") => tags.push(tag_view(
            "interpreted",
            &reason("Its code could not be compiled, so it ran interpreted, slower"),
            ModTagKind::Info,
        )),
        Some("mixed") => tags.push(tag_view(
            "mixed",
            &reason("Part of its code could not be compiled, so it ran interpreted, slower"),
            ModTagKind::Info,
        )),
        _ => {}
    }
    tags
}

/// The latest release of each mod installed from its repository; what could
/// not be read is logged.
fn latest_releases(dir: &Path, library: &Library) -> BTreeMap<String, Entry> {
    let mods = library.scan().unwrap_or_default();
    let mut releases = BTreeMap::new();
    for m in &mods {
        let Some(Origin::Repository { url }) = &m.origin else {
            continue;
        };
        match repository::latest_entry(url) {
            Ok(entry) if entry.id == m.id => {
                releases.insert(m.id.clone(), entry);
            }
            Ok(entry) => log(
                dir,
                LogLevel::Warn,
                &format!(
                    "Mods: the latest release of {url} is {}, not {}.",
                    entry.id, m.id
                ),
            ),
            Err(error) => log(
                dir,
                LogLevel::Warn,
                &format!("Mods: could not check {} for updates: {error}", m.name),
            ),
        }
    }
    releases
}

fn log(install_dir: &Path, level: LogLevel, message: &str) {
    let _ = diagnostics::write(install_dir, level, message);
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
    fn offers_a_version_built_for_the_installed_tag() {
        let built_for = |version: &str, drh: &str| Entry {
            uses: vec!["extends".to_string()],
            drh: Some(drh.to_string()),
            ..entry("meter", version)
        };
        let mut page = Page {
            index: Some(Index {
                entries: vec![built_for("2.0.0", "13"), built_for("2.1.0", "14")],
            }),
            mods: vec![installed("meter", "2.0.0", &[])],
            enabled: vec!["meter".to_string()],
            tag: Some(13),
            ..Page::default()
        };
        assert_eq!(offered_entry(&page, "meter").unwrap().version, "2.0.0");
        assert!(update_for(&page, &page.mods[0]).is_none());

        page.tag = Some(14);
        assert_eq!(offered_entry(&page, "meter").unwrap().version, "2.1.0");
        assert!(update_for(&page, &page.mods[0]).is_some());
    }

    #[test]
    fn shows_the_owner_next_to_a_different_author() {
        let source = Some("https://github.com/Tutez64/DRH-Mod-API");
        let pair = |author: &str, owner: &str| (author.to_string(), owner.to_string());
        assert_eq!(byline("Tutez", source), pair("Tutez", "Tutez64"));
        assert_eq!(byline("tutez64", source), pair("", "Tutez64"));
        assert_eq!(byline("Someone", None), pair("Someone", ""));
    }

    #[test]
    fn searches_the_account_not_the_author() {
        let source = Some("https://github.com/Tutez64/DRH-Mod-API");
        assert!(matches("tutez6", "API", "api", source));
        assert!(matches(" Party ", "Party Board", "party_board", None));
        assert!(!matches("tutez", "Mine", "mine", None));
    }

    #[test]
    fn hides_the_arrows_while_searching() {
        let mut page = library_page();
        let orderable = |page: &Page| {
            installed_rows(page, None)
                .iter()
                .map(|(id, row)| (id.clone(), row.orderable))
                .collect::<Vec<_>>()
        };
        assert!(orderable(&page).iter().all(|(_, orderable)| *orderable));
        page.search[0] = "mine".to_string();
        assert_eq!(orderable(&page), [("mine".to_string(), false)]);
    }

    fn library_page() -> Page {
        let mut lib = installed("lib", "0.1.0", &[]);
        lib.dependency = true;
        Page {
            mods: vec![
                lib,
                installed("mine", "1.0.0", &[("lib", "0.1")]),
                installed("other", "1.0.0", &[]),
            ],
            enabled: ["lib", "mine", "other"].map(String::from).to_vec(),
            ..Page::default()
        }
    }

    #[test]
    fn asks_before_what_breaks_another_mod() {
        let page = library_page();
        let library = Library::new(Path::new("unused"));
        let found = consequences(&page, &library, &Action::Disable("lib".to_string())).unwrap();
        assert!(found.also.is_empty());
        assert_eq!(
            found.problems,
            ["mine: Needs lib, which is disabled: the game fails it."]
        );
        assert!(confirmation(&page, &Action::Disable("lib".to_string()), &found).is_some());

        let none = consequences(&page, &library, &Action::Disable("other".to_string())).unwrap();
        assert!(none.problems.is_empty());
        assert!(confirmation(&page, &Action::Disable("other".to_string()), &none).is_none());
        assert!(confirmation(&page, &Action::Uninstall("other".to_string()), &none).is_some());
    }

    #[test]
    fn asks_before_installing_more_or_what_does_not_fit() {
        let meter = Entry {
            uses: vec!["extends".to_string()],
            drh: Some("14".to_string()),
            api: Some("0.1".to_string()),
            dependencies: [("lib".to_string(), "1.0".to_string())].into(),
            ..entry("meter", "1.0.0")
        };
        let page = Page {
            index: Some(Index {
                entries: vec![
                    meter.clone(),
                    entry("lib", "1.0.2"),
                    Entry {
                        source: OFFICIAL_API_SOURCE.to_string(),
                        ..entry("api", "0.1.1")
                    },
                ],
            }),
            tag: Some(13),
            ..Page::default()
        };
        // Nothing installed: the plan only reads the folder.
        let library = Library::new(Path::new("no-such-install"));
        let action = Action::Install(meter);
        let found = consequences(&page, &library, &action).unwrap();
        assert_eq!(found.also, ["installs lib 1.0.2"]);
        assert_eq!(
            found.problems,
            ["meter: Built for DRH 14, and DRH 13 is installed: it may not work."]
        );
        let asked = confirmation(&page, &action, &found).unwrap();
        assert_eq!(asked.button, "Install anyway");
        assert!(
            asked
                .text
                .starts_with("This also:\n• installs lib 1.0.2\n\n")
        );
    }

    #[test]
    fn always_asks_before_a_mod_from_elsewhere() {
        let page = Page {
            mods: vec![installed("mine", "1.0.0", &[])],
            ..Page::default()
        };
        let action = Action::InstallFrom(Elsewhere::Release(entry("mine", "1.1.0")));
        let asked = confirmation(&page, &action, &Consequences::default()).unwrap();
        assert_eq!(asked.title, "Install mine 1.1.0?");
        assert_eq!(
            asked.text,
            "No one has reviewed its code: only install it if you trust where it comes from. It replaces mine 1.0.0 from the catalog: the launcher will update it from this repository instead."
        );
        assert_eq!(asked.button, "Install");
    }

    #[test]
    fn updates_a_mod_from_its_repository_latest_release() {
        let mut mine = installed("mine", "1.0.0", &[]);
        mine.origin = Some(Origin::Repository {
            url: "https://github.com/me/DRH-Mod-M".to_string(),
        });
        let mut page = Page {
            index: Some(Index {
                entries: vec![entry("mine", "2.0.0")],
            }),
            mods: vec![mine],
            ..Page::default()
        };
        assert!(update_for(&page, &page.mods[0]).is_none());
        page.releases
            .insert("mine".to_string(), entry("mine", "1.1.0"));
        assert_eq!(update_for(&page, &page.mods[0]).unwrap().version, "1.1.0");
    }

    #[test]
    fn shows_an_installed_mod_as_installed() {
        let page = Page {
            index: Some(Index {
                entries: vec![entry("mine", "1.0.0"), entry("mine", "1.1.0")],
            }),
            mods: vec![installed("mine", "1.0.0", &[])],
            enabled: vec!["mine".to_string()],
            ..Page::default()
        };
        let shown = detail(&page, "mine", None);
        assert_eq!(shown.title, "mine  1.0.0");
        assert_eq!(shown.primary, "Disable");
        assert_eq!(shown.update, "Update to 1.1.0");
        assert!(shown.installed);
    }

    fn reported(ready: bool, drh: &str) -> LastRun {
        last_run::parse(&format!(
            r#"{{"drh": "{drh}", "ready": {ready}, "mods": [
                {{"id": "mine", "version": "1.0.0", "status": "failed", "error": "boom"}},
                {{"id": "other", "version": "1.0.0", "status": "ok", "mode": "mixed",
                  "error": "2 modules left interpreted"}}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn sums_up_the_last_game() {
        let mut page = Page {
            tag: Some(13),
            ..library_page()
        };
        assert_eq!(run_summary(&page), (String::new(), false));
        page.game = Game::Running;
        assert_eq!(run_summary(&page).0, "Game starting...");
        page.last_run = Some(reported(false, "13"));
        assert_eq!(run_summary(&page).0, "Game loading: 2 mods started.");
        page.game = Game::Exited;
        assert_eq!(
            run_summary(&page),
            (
                "The last game stopped before it was ready.".to_string(),
                true
            )
        );
        page.last_run = Some(reported(true, "13"));
        assert_eq!(
            run_summary(&page),
            ("Last game: 1 mod loaded, 1 failed.".to_string(), true)
        );
        page.last_run = None;
        assert_eq!(
            run_summary(&page),
            (
                "The last game stopped before loading the mods.".to_string(),
                true
            )
        );
        // Written by another DRH: it says nothing of this one.
        page.last_run = Some(reported(true, "12"));
        assert_eq!(run_summary(&page), (String::new(), false));
    }

    #[test]
    fn labels_what_the_last_game_reported() {
        let mut page = Page {
            tag: Some(13),
            last_run: Some(reported(true, "13")),
            ..library_page()
        };
        let text = |page: &Page, id: &str| {
            let m = page.mods.iter().find(|m| m.id == id).unwrap();
            run_tags(page, m)
                .iter()
                .map(|tag| tag.text.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(text(&page, "mine"), ["⚠ failed"]);
        assert_eq!(text(&page, "other"), ["mixed"]);
        assert!(text(&page, "lib").is_empty());

        // Another version than the one that ran, or disabled since.
        page.mods[1].version = "1.1.0".to_string();
        assert!(text(&page, "mine").is_empty());
        page.enabled.retain(|id| id != "other");
        assert!(text(&page, "other").is_empty());
    }

    #[test]
    fn labels_what_a_mod_is_to_the_others() {
        let text = |page: &Page, id: &str| {
            let m = page.mods.iter().find(|m| m.id == id).unwrap();
            dependency_tags(page, m)
                .iter()
                .map(|tag| tag.text.to_string())
                .collect::<Vec<_>>()
        };
        let mut page = library_page();
        assert_eq!(text(&page, "lib"), ["dependency"]);
        assert!(text(&page, "other").is_empty());

        page.enabled.retain(|id| id != "lib");
        assert_eq!(text(&page, "lib"), ["⚠ needed"]);

        page.enabled.retain(|id| id != "mine");
        assert_eq!(text(&page, "lib"), ["not needed"]);
    }
}
