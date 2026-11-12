//! Embeds the icon and the version information in the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/snapkit.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/snapkit.ico");
        res.set("ProductName", "SnapKit");
        res.set("FileDescription", "SnapKit");
        res.set("LegalCopyright", "Copyright (c) 2026 SnapKit contributors. MIT License.");
        res.compile().expect("failed to embed the icon and the version data");
    }
}
