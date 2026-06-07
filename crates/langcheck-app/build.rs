//! Build script: embed the LangCheck application icon as Win32 resource id 1, so
//! `langcheck.exe` (and its tray icon, which loads resource id 1) shows the custom
//! icon instead of the default Windows application icon.
//!
//! Non-fatal: if the platform's resource compiler is unavailable, it logs a warning
//! and continues — the build still succeeds and the tray falls back to the default
//! icon at runtime.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/langcheck.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon_with_id("assets/langcheck.ico", "1");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=langcheck icon embed skipped: {error}");
        }
    }
}
