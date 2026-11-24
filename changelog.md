Working changelog for the next release. Update it as work lands.
At tag time, copy these sections into the GitHub release notes under `Changelog`.
In each section, what matters to the most players comes first. What only people making mods will use goes under `For modders`.

### Added

- The Mods lists can be sorted, and keep the sort last chosen. Catalog: by downloads (of every version, which it starts with, or of the latest one), newest, last update, or name. Installed: by load order, name, or updates first.
- The mods that joined the catalog since it was last opened are marked: the Catalog tab says how many, and their cards carry a **NEW** mark by their name.
- **Hide installed**, in the Catalog, lists only the mods you do not have; **Hide disabled**, in the Installed list, only the enabled ones.
- **Update all**, in the Installed list when a mod has an update: it lists each mod with its installed and its new version, then updates them one after the other.
- Catalog cards and a mod's page show its downloads, and its page when it was published and last updated.
- **Start Steam**: when Steam is not running, its notice on Home has a button that starts it in the background. **Start Steam when the launcher opens**, off by default in Settings → General, does it each time the launcher opens; the first click on the button offers to turn it on.

### Fixed

- On Windows and Linux, the launcher no longer crashes at startup on old processors below the x86-64-v2 level, those without SSE4.2 for instance. DRH itself still needs that level for now.
- A Mods list no longer shows an empty area once it is shorter than where it was scrolled to: after a search, a mod uninstalled, or in a taller window.
- Updating a disabled mod no longer enables it.
- Installing or updating a mod whose zip is gone, or is no longer the one the index checked, says so and that its author has to fix it, in place of a download or SHA-256 error.
- A link in a mod's description or in a changelog only opens a web address (`https://` or `http://`). Any other target, a file's path among them, was handed to the system as it was, which opens a file and runs a program.

### Improved

- Searching the Mods lists by name shows first the names that start with the search.
- The Catalog's count says how many mods a search leaves (`3 of 26 mods`).

### For modders

- `--submit-mod` checks the release as the index CI will before proposing it, its zip and its place in the index (a version not indexed yet, dependencies, the id's repository): it prints the notes, and stops on a problem the index would refuse.
- `--submit-mod` keeps one pull request a mod: run for a new version while the previous one is not merged yet, it updates that pull request, which then holds only the new version. Its title names the mod without a version (`Add cool_hud`, `Update cool_hud`), so it stays true, and the command reminds to click Re-request review when a review asked for changes.
- `--check-mod` reads the mod's code. It refuses `mods.api._internal`, the `api`'s own code, and notes, without refusing, code that reaches the core without `"uses": ["extends"]`, names another mod that `dependencies` does not list (in an `import`, in `context.isActive`, or by its classes, where it points to `context.isActive`), or has empty `catch` blocks. The index CI shows these notes to the reviewer.
- `--new-mod` asks for the mod's name and takes its id from it (`Cool Mod` gives `cool_mod`), asking for one only when the name gives none.
- `--submit-mod` and the index's check refuse a `drh` that names a DRH not released yet, which would keep the launcher from warning its players, and note one that leaves out the latest DRH.
- `--submit-mod` and the index's check note a zip that does not hold the files of its tag, such as a release made by hand from other sources, and a mod its code names that the index does not have.
- `--check-mod` and the index's check note an id that is not the one its name gives, without refusing it.
- `--check-mod` refuses a file under `src/` that is not a `.hx`, and a source that holds a character not seen when the code is read: a control character such as NUL, a zero-width one, a direction mark. Write it as an escape in a string (`\u{202E}`).
- `--submit-mod` and the index's check refuse a zip that holds anything but what `--pack-mod` packs (`mod.json`, `README.md`, `LICENSE`, `src/`, `Resources/`), or a name that Windows would write as another one or over another file.

### Internal

- `--build-index` adds to each version its release date and download count, read from GitHub (`GITHUB_TOKEN` or `GH_TOKEN` when set). It notes a zip GitHub no longer has, or whose digest is no longer the entry's `sha256`, and, given a third argument, writes these versions to that file as JSON, with the repositories GitHub did not answer for: the index CI opens an issue for each.
- Slint 1.19.1, on its edition 1.19, and the other dependencies are updated. The Home glows get gradient stops that keep their look under Slint 1.19.
