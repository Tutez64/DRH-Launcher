Working changelog for the next release. Update it as work lands.
At tag time, copy these sections into the GitHub release notes under `Changelog`.

### Fixed

- The Windows installer no longer contains `DRH-Launcher.com`, which several antivirus products reported as a trojan. It was a false positive: this small program only started the launcher from a console, for the modder commands. Updating removes it. Until the launcher is signed, these commands are run through a script, `bin\DRH-Launcher.cmd` in the launcher's folder: `bin\DRH-Launcher --check-mod .` in place of `DRH-Launcher --check-mod .`.
