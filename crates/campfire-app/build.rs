// Links the fragments etchc generated: scripts/build-etch.sh runs etchc, assembles its LLVM IR to
// bitcode (rustc's own LLVM) and points ETCH_GEN_DIR at libetchapp.a and app_gen.rs.
fn main() {
    let gen = std::env::var("ETCH_GEN_DIR").expect("ETCH_GEN_DIR is unset: build with scripts/build-etch.sh");
    println!("cargo:rustc-link-search=native={gen}");
    println!("cargo:rustc-link-lib=static=etchapp");
    println!("cargo:rustc-env=ETCH_GEN_RS={gen}/app_gen.rs");
    println!("cargo:rerun-if-changed={gen}/libetchapp.a");
    println!("cargo:rerun-if-changed={gen}/app_gen.rs");
    println!("cargo:rerun-if-env-changed=ETCH_GEN_DIR");
}
