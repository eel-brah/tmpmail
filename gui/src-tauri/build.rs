fn main() {
    // Only these commands get permissions, and the `main` capability lists
    // each one: by default every registered command is open to every window.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "init",
        "create_inbox",
        "select_inbox",
        "open_message",
        "probe_finish",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("tauri build script");
}
