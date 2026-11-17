Working changelog for the next release. Update it as work lands.
At tag time, copy these sections into the GitHub release notes under `Changelog`.

### Added

- The Mods lists can be sorted. Catalog: by name, last update, newest, or downloads (of every version, or of the latest one). Installed: by load order, name, or updates first.
- Catalog cards and a mod's page show its downloads, and its page when it was published and last updated. These come from the mod index once it is generated with this version.

### Fixed

- Searching the Mods lists while scrolled down shows the results from the first one, instead of an empty area below them.
- A Mods list that gets shorter than where it was scrolled to (a mod uninstalled, a taller window) no longer leaves an empty area.

### Improved

- `--new-mod` asks for the mod's name and takes its id from it (`Cool Mod` gives `cool_mod`), asking for one only when the name gives none.
- `--check-mod` and the index's check note an id that is not the one its name gives, without refusing it.
- Searching the Mods lists by name shows first the names that start with the search.

### Internal

- `--build-index` adds to each version its release date and download count, read from GitHub (`GITHUB_TOKEN` or `GH_TOKEN` when set).
- Slint 1.19, on its edition 1.19, and the other dependencies are updated. The Home glows get gradient stops that keep their look under Slint 1.19.
