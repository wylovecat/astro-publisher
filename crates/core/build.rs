//! Stamp Windows version information into this crate's executable.
//!
//! `installer.nsi` calls `VIAddVersionKey`, which writes the version resource
//! of the **installer** — it says nothing about the three binaries the
//! installer copies into the program directory. Without this build script the
//! installed `astro-publisher.exe` carries no version resource at all, so its
//! Properties dialog shows a blank version and the only way to tell which
//! build is installed is to read the uninstall entry or the installer filename.
//!
//! The version comes from `Cargo.toml` by way of Cargo's own environment, so
//! there is still one source of truth: bumping the workspace version and
//! rebuilding is enough.
//!
//! A version resource is a PE/COFF concept. Building on Linux or macOS must not
//! need it, and this script deliberately does nothing there.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo always sets the package version");
    let description = std::env::var("CARGO_PKG_DESCRIPTION").unwrap_or_default();
    let authors = std::env::var("CARGO_PKG_AUTHORS").unwrap_or_default();

    let mut res = winresource::WindowsResource::new();
    res.set("FileDescription", &description);
    res.set("FileVersion", &version);
    res.set("LegalCopyright", "MIT License");
    res.set("ProductName", "Astro Publisher");
    res.set("ProductVersion", &version);
    res.set("CompanyName", &authors);
    res.compile()
        .expect("the Windows version resource should compile");
}
