Working changelog for the next release. Update it as work lands.
At tag time, copy these sections into the GitHub release notes under `Changelog`.

### Added

- 
### Fixed

- On Windows, checking and downloading DRH, mods, and launcher updates no longer fails with "error sending request for url" when Windows has not yet fetched the root certificate GitHub chains to: the launcher now carries its own list of trusted roots in addition to the system's.
- The Linux AppImage starts on distributions as old as Ubuntu 22.04 (glibc 2.35) instead of requiring glibc 2.39, and on systems without libxkbcommon-x11, which it now bundles.

### Improved

- Network errors in the logs give their cause (DNS, refused connection, rejected certificate…), not only "error sending request for url".