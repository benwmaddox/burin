//! The message board, all generated from its spec (apps/board): the module etchc wrote (LLVM IR, linked
//! under cross-language LTO, and its Rust glue), served by the runtime.

#[allow(clippy::all, non_camel_case_types, non_snake_case, dead_code, unused)]
mod gen {
    include!(env!("ETCH_GEN_RS"));
}

fn main() {
    etch_rt::app::main::<gen::Spec>(gen::CONFIGS, gen::SESSION_COOKIE)
}
