// Embeds app.manifest so the app gets themed common controls (v6) and
// per-monitor DPI awareness, and compiles app.rc for the app icon.
fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("app.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
    // Optional so `cargo clippy --target x86_64-pc-windows-msvc` still works
    // on Linux without a resource compiler; a Windows build always has one.
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
