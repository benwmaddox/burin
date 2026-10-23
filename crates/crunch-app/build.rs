// Links the fragments burinc generated: scripts/build-burin.sh runs burinc, assembles its LLVM IR to
// bitcode (rustc's own LLVM) and points BURIN_GEN_DIR at libburinapp.a and app_gen.rs; the app's
// external functions (apps/<app>/ext/) are libburinext.a (C) and BURIN_EXT_RS (Rust).
fn main() {
    let gen = std::env::var("BURIN_GEN_DIR").expect("BURIN_GEN_DIR is unset: build with scripts/build-burin.sh");
    println!("cargo:rustc-link-search=native={gen}");
    println!("cargo:rustc-link-lib=static=burinapp");
    if std::path::Path::new(&format!("{gen}/libburinext.a")).exists() {
        println!("cargo:rustc-link-lib=static=burinext");
    }
    println!("cargo:rustc-env=BURIN_GEN_RS={gen}/app_gen.rs");
    let ext = std::env::var("BURIN_EXT_RS").unwrap_or_else(|_| {
        let none = format!("{gen}/ext_none.rs");
        std::fs::write(&none, "").expect("an empty ext.rs");
        none
    });
    println!("cargo:rustc-env=BURIN_EXT_RS={ext}");
    println!("cargo:rerun-if-changed={ext}");
    println!("cargo:rerun-if-changed={gen}/libburinapp.a");
    println!("cargo:rerun-if-changed={gen}/libburinext.a");
    println!("cargo:rerun-if-changed={gen}/app_gen.rs");
    println!("cargo:rerun-if-env-changed=BURIN_GEN_DIR");
    println!("cargo:rerun-if-env-changed=BURIN_EXT_RS");
}
