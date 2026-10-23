//! Campfire, all generated from its spec (apps/campfire): the module etchc wrote (LLVM IR, linked
//! under cross-language LTO, and its Rust glue).

#[allow(clippy::all, non_camel_case_types, non_snake_case, dead_code, unused)]
pub mod gen {
    include!(env!("ETCH_GEN_RS"));
}

/// The app's external functions in Rust (apps/<app>/ext/ext.rs; empty when there are none).
#[allow(clippy::all, dead_code, unused)]
pub mod ext {
    include!(env!("ETCH_EXT_RS"));
}
