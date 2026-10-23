//! Compute in idiomatic burin (apps/crunch), served by the runtime from its generated module.

fn main() {
    burin_rt::app::main::<crunch_app::gen::Spec>(crunch_app::gen::CONFIGS, crunch_app::gen::SESSION_COOKIE)
}
