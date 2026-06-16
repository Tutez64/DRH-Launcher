fn main() {
    println!("cargo:rerun-if-env-changed=DRHL_UPDATE_ENDPOINT");
    println!("cargo:rerun-if-env-changed=DRHL_UPDATE_PUBLIC_KEY");

    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config).expect("failed to compile Slint UI");
}
