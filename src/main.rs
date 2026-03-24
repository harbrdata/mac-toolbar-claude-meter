mod api;
mod app;
mod gauge;
mod keychain;
mod launch_agent;

fn main() {
    // Strip macOS quarantine flag so the app works after drag-to-install
    // from a DMG without requiring right-click > Open.
    if let Ok(exe) = std::env::current_exe() {
        // Walk up to the .app bundle root
        if let Some(app_bundle) = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        {
            let _ = std::process::Command::new("xattr")
                .args(["-dr", "com.apple.quarantine"])
                .arg(app_bundle)
                .output();
        }
    }

    app::run();
}
