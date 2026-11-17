fn main() {
    println!("cargo:rerun-if-env-changed=DRHL_UPDATE_ENDPOINT");
    println!("cargo:rerun-if-env-changed=DRHL_UPDATE_PUBLIC_KEY");
    println!("cargo:rerun-if-changed=assets/icons/app-icon.ico");

    // `slint-project.json` names the main file and the edition, for the editor
    // tooling too. slint-build reruns this script when it or a `.slint` file
    // changes.
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config(".", config).expect("failed to compile Slint UI");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        winresource::WindowsResource::new()
            .set_icon("assets/icons/app-icon.ico")
            .set("CompanyName", "Tutez")
            .set(
                "Comments",
                "Installs, updates, configures, and launches Dungeon Rampage Haxe.",
            )
            .set("FileDescription", "DRH Launcher")
            .set("InternalName", "DRH-Launcher.exe")
            .set("LegalCopyright", "Copyright (c) Tutez")
            .set("OriginalFilename", "DRH-Launcher.exe")
            .set("ProductName", "DRH Launcher")
            .compile()
            .expect("failed to embed Windows resources");
    }
}
