//! Embeds the application icon into the Windows executable (shown by Explorer and the
//! taskbar before the window sets its own icon).

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/gravekeeper.ico");
    // Needs the Windows resource compiler, so only when building on Windows for Windows.
    if cfg!(windows) && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/icons/gravekeeper.ico")
            .canonicalize()
            .expect("packaging/icons/gravekeeper.ico exists");
        let rc = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("gravekeeper.rc");
        let path = icon.to_string_lossy().replace('\\', "\\\\");
        std::fs::write(&rc, format!("1 ICON \"{path}\"\n")).expect("write rc");
        embed_resource::compile(&rc, embed_resource::NONE)
            .manifest_optional()
            .expect("compile resources");
    }
}
