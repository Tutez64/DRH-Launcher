#[cfg(target_os = "linux")]
mod platform {
    use directories::BaseDirs;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const APP_ID: &str = "io.github.Tutez64.DRHLauncher";
    const ICON_NAME: &str = "DRH-Launcher";
    const APPIMAGE_NAME: &str = "DRH-Launcher.AppImage";
    const ICON_SIZES: &[u32] = &[16, 32, 48, 64, 128, 256];

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum IntegrationState {
        NotAppImage,
        Portable,
        Installed,
        NeedsRepair,
    }

    #[derive(Clone, Debug)]
    struct IntegrationPaths {
        appimage: PathBuf,
        desktop_entry: PathBuf,
        icons: Vec<PathBuf>,
    }

    pub fn state() -> IntegrationState {
        let Some(source) = current_appimage() else {
            return IntegrationState::NotAppImage;
        };
        let Ok(paths) = integration_paths() else {
            return IntegrationState::Portable;
        };

        if !paths_match(&source, &paths.appimage) {
            return IntegrationState::Portable;
        }

        if paths.desktop_entry.is_file() && paths.icons.iter().all(|icon| icon.is_file()) {
            IntegrationState::Installed
        } else {
            IntegrationState::NeedsRepair
        }
    }

    pub fn is_managed_install() -> bool {
        matches!(
            state(),
            IntegrationState::Installed | IntegrationState::NeedsRepair
        )
    }

    pub fn application_path() -> Option<PathBuf> {
        current_appimage()
    }

    pub fn install_and_restart() -> Result<(), String> {
        let source = current_appimage()
            .ok_or_else(|| "DRH Launcher is not running from an AppImage.".to_string())?;
        let app_dir = env::var_os("APPDIR")
            .map(PathBuf::from)
            .ok_or_else(|| "Could not locate the mounted AppImage contents.".to_string())?;
        let icon_sources = ICON_SIZES
            .iter()
            .map(|size| bundled_icon_path(&app_dir, *size))
            .collect::<Vec<_>>();
        let paths = integration_paths()?;

        install_from(&source, &icon_sources, &paths)?;
        refresh_desktop_caches(&paths);
        remove_portable_source(&source, &paths.appimage);

        Command::new(&paths.appimage)
            .args(env::args_os().skip(1))
            .spawn()
            .map_err(|error| {
                format!("DRH Launcher was installed, but could not be restarted: {error}")
            })?;
        std::process::exit(0);
    }

    fn bundled_icon_path(app_dir: &Path, size: u32) -> PathBuf {
        if size == 256 {
            return app_dir.join("usr/share/icons/hicolor/256x256/apps/DRH-Launcher.png");
        }

        app_dir.join(format!("usr/lib/DRH-Launcher/icons/{size}.png"))
    }

    fn remove_portable_source(source: &Path, installed: &Path) {
        if !paths_match(source, installed) {
            let _ = fs::remove_file(source);
        }
    }

    pub fn uninstall() -> Result<(), String> {
        if !is_managed_install() {
            return Err("This AppImage is not installed by DRH Launcher.".to_string());
        }

        let paths = integration_paths()?;
        remove_integration(&paths)?;
        refresh_desktop_caches(&paths);
        Ok(())
    }

    fn current_appimage() -> Option<PathBuf> {
        env::var_os("APPIMAGE").map(PathBuf::from)
    }

    fn integration_paths() -> Result<IntegrationPaths, String> {
        let dirs = BaseDirs::new()
            .ok_or_else(|| "Could not determine the current user's home directory.".to_string())?;
        Ok(paths_for(dirs.home_dir(), dirs.data_dir()))
    }

    fn paths_for(home: &Path, data_home: &Path) -> IntegrationPaths {
        IntegrationPaths {
            appimage: home.join("Applications").join(APPIMAGE_NAME),
            desktop_entry: data_home
                .join("applications")
                .join(format!("{APP_ID}.desktop")),
            icons: ICON_SIZES
                .iter()
                .map(|size| {
                    data_home
                        .join("icons")
                        .join("hicolor")
                        .join(format!("{size}x{size}"))
                        .join("apps")
                        .join(format!("{ICON_NAME}.png"))
                })
                .collect(),
        }
    }

    fn install_from(
        source: &Path,
        icon_sources: &[PathBuf],
        paths: &IntegrationPaths,
    ) -> Result<(), String> {
        if !source.is_file() {
            return Err(format!("AppImage not found: {}", source.display()));
        }
        for icon_source in icon_sources {
            if !icon_source.is_file() {
                return Err(format!(
                    "Application icon not found inside the AppImage: {}",
                    icon_source.display()
                ));
            }
        }

        copy_atomic(source, &paths.appimage, 0o755)?;
        for (icon_source, icon_destination) in icon_sources.iter().zip(&paths.icons) {
            copy_atomic(icon_source, icon_destination, 0o644)?;
        }
        write_atomic(
            &paths.desktop_entry,
            desktop_entry_contents(
                &paths.appimage,
                paths
                    .icons
                    .last()
                    .expect("at least one application icon size"),
            )
            .as_bytes(),
            0o644,
        )?;
        Ok(())
    }

    fn desktop_entry_contents(appimage: &Path, icon: &Path) -> String {
        let executable = escape_desktop_exec_argument(appimage.as_os_str());
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=DRH Launcher\n\
             Comment=Install, update, configure, and play Dungeon Rampage Haxe\n\
             Exec=\"{executable}\"\n\
             Icon={}\n\
             Terminal=false\n\
             Categories=Game;\n\
             StartupNotify=true\n\
             StartupWMClass={APP_ID}\n",
            icon.display()
        )
    }

    fn escape_desktop_exec_argument(value: &std::ffi::OsStr) -> String {
        value
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('%', "%%")
    }

    fn copy_atomic(source: &Path, destination: &Path, mode: u32) -> Result<(), String> {
        let contents = fs::read(source)
            .map_err(|error| format!("Could not read {}: {error}", source.display()))?;
        write_atomic(destination, &contents, mode)
    }

    fn write_atomic(destination: &Path, contents: &[u8], mode: u32) -> Result<(), String> {
        let parent = destination.parent().ok_or_else(|| {
            format!(
                "Could not determine parent directory for {}",
                destination.display()
            )
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;

        let temporary = temporary_path(destination);
        let result = (|| {
            fs::write(&temporary, contents)
                .map_err(|error| format!("Could not write {}: {error}", temporary.display()))?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)).map_err(|error| {
                format!(
                    "Could not set permissions on {}: {error}",
                    temporary.display()
                )
            })?;
            fs::rename(&temporary, destination).map_err(|error| {
                format!(
                    "Could not replace {} with installed file: {error}",
                    destination.display()
                )
            })
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn temporary_path(destination: &Path) -> PathBuf {
        let mut name = destination
            .file_name()
            .map(OsString::from)
            .unwrap_or_else(|| OsString::from("drh-launcher"));
        name.push(format!(".installing-{}", std::process::id()));
        destination.with_file_name(name)
    }

    fn remove_file_if_present(path: &Path) -> Result<(), String> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not remove {}: {error}", path.display())),
        }
    }

    fn remove_integration(paths: &IntegrationPaths) -> Result<(), String> {
        remove_file_if_present(&paths.desktop_entry)?;
        for icon in &paths.icons {
            remove_file_if_present(icon)?;
        }
        remove_file_if_present(&paths.appimage)?;
        Ok(())
    }

    fn paths_match(left: &Path, right: &Path) -> bool {
        match (fs::canonicalize(left), fs::canonicalize(right)) {
            (Ok(left), Ok(right)) => left == right,
            _ => left == right,
        }
    }

    fn refresh_desktop_caches(paths: &IntegrationPaths) {
        if let Some(applications_dir) = paths.desktop_entry.parent() {
            let _ = Command::new("update-desktop-database")
                .arg(applications_dir)
                .status();
        }

        if let Some(hicolor_dir) = paths.icons.first().and_then(|icon| icon.ancestors().nth(3)) {
            let _ = Command::new("gtk-update-icon-cache")
                .args(["--force", "--ignore-theme-index"])
                .arg(hicolor_dir)
                .status();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn installs_appimage_icon_and_desktop_entry() {
            let temp = tempdir().unwrap();
            let source = temp.path().join("download.AppImage");
            fs::write(&source, b"appimage").unwrap();
            let icon_sources = ICON_SIZES
                .iter()
                .map(|size| {
                    let icon = temp.path().join(format!("DRH-Launcher-{size}.png"));
                    fs::write(&icon, format!("icon-{size}")).unwrap();
                    icon
                })
                .collect::<Vec<_>>();
            let paths = paths_for(
                &temp.path().join("home"),
                &temp.path().join("home/.local/share"),
            );

            install_from(&source, &icon_sources, &paths).unwrap();

            assert_eq!(fs::read(&paths.appimage).unwrap(), b"appimage");
            for (size, icon) in ICON_SIZES.iter().zip(&paths.icons) {
                assert_eq!(fs::read_to_string(icon).unwrap(), format!("icon-{size}"));
            }
            assert_eq!(
                paths.appimage,
                temp.path().join("home/Applications/DRH-Launcher.AppImage")
            );
            assert_eq!(
                fs::metadata(&paths.appimage).unwrap().permissions().mode() & 0o777,
                0o755
            );
            let desktop = fs::read_to_string(&paths.desktop_entry).unwrap();
            assert!(desktop.contains("Name=DRH Launcher"));
            assert!(desktop.contains(&format!("Exec=\"{}\"", paths.appimage.to_string_lossy())));
            assert!(desktop.contains(&format!("Icon={}", paths.icons.last().unwrap().display())));
        }

        #[test]
        fn locates_public_and_private_bundled_icons() {
            let app_dir = Path::new("/tmp/DRH-Launcher.AppDir");

            assert_eq!(
                bundled_icon_path(app_dir, 128),
                app_dir.join("usr/lib/DRH-Launcher/icons/128.png")
            );
            assert_eq!(
                bundled_icon_path(app_dir, 256),
                app_dir.join("usr/share/icons/hicolor/256x256/apps/DRH-Launcher.png")
            );
        }

        #[test]
        fn removes_portable_source_after_install() {
            let temp = tempdir().unwrap();
            let source = temp.path().join("download.AppImage");
            let installed = temp.path().join("Applications/DRH-Launcher.AppImage");
            fs::write(&source, b"portable").unwrap();
            fs::create_dir_all(installed.parent().unwrap()).unwrap();
            fs::write(&installed, b"installed").unwrap();

            remove_portable_source(&source, &installed);

            assert!(!source.exists());
            assert_eq!(fs::read(installed).unwrap(), b"installed");
        }

        #[test]
        fn keeps_source_when_already_running_from_installed_path() {
            let temp = tempdir().unwrap();
            let installed = temp.path().join("Applications/DRH-Launcher.AppImage");
            fs::create_dir_all(installed.parent().unwrap()).unwrap();
            fs::write(&installed, b"installed").unwrap();

            remove_portable_source(&installed, &installed);

            assert_eq!(fs::read(installed).unwrap(), b"installed");
        }

        #[test]
        fn escapes_special_characters_in_desktop_exec_paths() {
            let escaped =
                escape_desktop_exec_argument(std::ffi::OsStr::new("/home/a $b/100%/quote\"/app"));

            assert_eq!(escaped, "/home/a \\$b/100%%/quote\\\"/app");
        }

        #[test]
        fn uninstall_removes_only_launcher_integration() {
            let temp = tempdir().unwrap();
            let paths = paths_for(
                &temp.path().join("home"),
                &temp.path().join("home/.local/share"),
            );
            for path in [&paths.appimage, &paths.desktop_entry] {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"owned by launcher").unwrap();
            }
            for icon in &paths.icons {
                fs::create_dir_all(icon.parent().unwrap()).unwrap();
                fs::write(icon, b"owned by launcher").unwrap();
            }
            let game_data = temp.path().join("home/.local/share/drh-game-data");
            fs::create_dir_all(&game_data).unwrap();
            fs::write(game_data.join("installed.json"), b"keep").unwrap();

            remove_integration(&paths).unwrap();

            assert!(!paths.appimage.exists());
            assert!(!paths.desktop_entry.exists());
            assert!(paths.icons.iter().all(|icon| !icon.exists()));
            assert_eq!(fs::read(game_data.join("installed.json")).unwrap(), b"keep");
        }
    }
}

#[cfg(target_os = "linux")]
pub use platform::*;

#[cfg(not(target_os = "linux"))]
mod platform {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum IntegrationState {
        NotAppImage,
        Portable,
        Installed,
        NeedsRepair,
    }

    pub fn state() -> IntegrationState {
        IntegrationState::NotAppImage
    }

    pub fn is_managed_install() -> bool {
        false
    }

    pub fn application_path() -> Option<std::path::PathBuf> {
        None
    }

    pub fn install_and_restart() -> Result<(), String> {
        Err("AppImage installation is only available on Linux.".to_string())
    }

    pub fn uninstall() -> Result<(), String> {
        Err("AppImage installation is only available on Linux.".to_string())
    }
}

#[cfg(not(target_os = "linux"))]
pub use platform::*;
