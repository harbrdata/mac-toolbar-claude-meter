mod api;
mod app;
mod gauge;
mod keychain;
mod launch_agent;
mod notification;

fn main() {
    // Strip macOS quarantine xattr so the app works after drag-to-install
    // from a DMG without requiring right-click > Open.
    if let Ok(exe) = std::env::current_exe()
        && let Some(app_bundle) = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|ext| ext == "app"))
    {
        strip_quarantine(app_bundle);
    }

    app::run();
}

/// Recursively remove com.apple.quarantine xattr from a path using native API.
fn strip_quarantine(path: &std::path::Path) {
    let attr = c"com.apple.quarantine";
    if let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
        unsafe {
            libc::removexattr(c_path.as_ptr(), attr.as_ptr(), libc::XATTR_NOFOLLOW);
        }
    }
    // Recurse into directory contents
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            strip_quarantine(&entry.path());
        }
    }
}
