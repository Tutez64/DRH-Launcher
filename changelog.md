Working changelog for the next release. Update it as work lands.
At tag time, copy these sections into the GitHub release notes under `Changelog`.

### Added

- The Mods lists can be sorted, and keep the sort last chosen. Catalog: by downloads (of every version, which it starts with, or of the latest one), newest, last update, or name. Installed: by load order, name, or updates first.
- **Update all**, in the Installed list when a mod has an update: it lists each mod with its installed and its new version, then updates them one after the other.
- **Hide disabled**, in the Installed list, shows only the enabled mods.
- **Hide installed**, in the Catalog, lists only the mods you do not have.
- The mods that joined the catalog since it was last opened are marked: the Catalog tab says how many, and their cards carry a **NEW** mark by their name.
- Catalog cards and a mod's page show its downloads, and its page when it was published and last updated. These come from the mod index once it is generated with this version.

### Fixed

- Searching the Mods lists while scrolled down shows the results from the first one, instead of an empty area below them.
- Updating a disabled mod no longer enables it.
- A Mods list that gets shorter than where it was scrolled to (a mod uninstalled, a taller window) no longer leaves an empty area.

### Improved

- `--new-mod` asks for the mod's name and takes its id from it (`Cool Mod` gives `cool_mod`), asking for one only when the name gives none.
- `--check-mod` and the index's check note an id that is not the one its name gives, without refusing it.
- `--check-mod` reads the mod's code. It refuses `mods.api._internal`, the `api`'s own code, and notes, without refusing, code that reaches the core without `"uses": ["extends"]`, names another mod that `dependencies` does not list (in an `import`, in `context.isActive`, or by its classes, where it points to `context.isActive`), or has empty `catch` blocks. The index CI shows these notes to the reviewer.
- `--submit-mod` checks the release's zip as the index CI will before proposing it: it prints the notes, and stops on a problem the index would refuse.
- `--submit-mod` and the index's check note a zip that does not hold the files of its tag, such as a release made by hand from other sources.
- Searching the Mods lists by name shows first the names that start with the search.
- The Catalog's count says how many mods a search leaves (`3 of 26 mods`).

### Internal

- `--build-index` adds to each version its release date and download count, read from GitHub (`GITHUB_TOKEN` or `GH_TOKEN` when set).
- Slint 1.19, on its edition 1.19, and the other dependencies are updated. The Home glows get gradient stops that keep their look under Slint 1.19.
