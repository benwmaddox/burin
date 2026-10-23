// Links the fragments etchc generated: scripts/build-etch.sh runs etchc, assembles its LLVM IR to
// bitcode (rustc's own LLVM) and points ETCH_GEN_DIR at libetchapp.a and app_gen.rs; the app's
// external functions (apps/<app>/ext/) are libetchext.a (C) and ETCH_EXT_RS (Rust).
fn main() {
    let gen = std::env::var("ETCH_GEN_DIR").expect("ETCH_GEN_DIR is unset: build with scripts/build-etch.sh");
    println!("cargo:rustc-link-search=native={gen}");
    println!("cargo:rustc-link-lib=static=etchapp");
    if std::path::Path::new(&format!("{gen}/libetchext.a")).exists() {
        println!("cargo:rustc-link-lib=static=etchext");
    }
    println!("cargo:rustc-env=ETCH_GEN_RS={gen}/app_gen.rs");
    let ext = std::env::var("ETCH_EXT_RS").unwrap_or_else(|_| {
        let none = format!("{gen}/ext_none.rs");
        std::fs::write(&none, "").expect("an empty ext.rs");
        none
    });
    println!("cargo:rustc-env=ETCH_EXT_RS={ext}");
    println!("cargo:rerun-if-changed={ext}");
    println!("cargo:rerun-if-changed={gen}/libetchapp.a");
    println!("cargo:rerun-if-changed={gen}/libetchext.a");
    println!("cargo:rerun-if-changed={gen}/app_gen.rs");
    println!("cargo:rerun-if-env-changed=ETCH_GEN_DIR");
    println!("cargo:rerun-if-env-changed=ETCH_EXT_RS");
}
