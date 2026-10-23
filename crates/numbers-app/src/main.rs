//! Numbers and time end to end, all generated from its spec (apps/numbers): the module burinc wrote (LLVM IR, linked
//! under cross-language LTO, and its Rust glue), served by the runtime.

#[allow(clippy::all, non_camel_case_types, non_snake_case, dead_code, unused)]
mod gen {
    include!(env!("BURIN_GEN_RS"));
}

/// The app's external functions in Rust (apps/<app>/ext/ext.rs; empty when there are none).
#[allow(clippy::all, dead_code, unused)]
pub mod ext {
    include!(env!("BURIN_EXT_RS"));
}

fn main() {
    burin_rt::app::main::<gen::Spec>(gen::CONFIGS, gen::SESSION_COOKIE)
}
