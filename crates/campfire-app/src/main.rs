//! Campfire, served by the runtime from its generated module.

fn main() {
    etch_rt::app::main::<campfire_app::gen::Spec>(campfire_app::gen::CONFIGS, campfire_app::gen::SESSION_COOKIE)
}
