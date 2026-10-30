//! Main-thread runner for the production views' headless Metal checks.
#![allow(dead_code, unused_imports)]

#[cfg(debug_assertions)]
#[path = "../src/control.rs"]
mod control;
#[path = "../src/relay.rs"]
mod relay;
#[path = "../src/tests.rs"]
mod tests;
#[path = "../src/ui/mod.rs"]
mod ui;

fn main() {
    #[cfg(debug_assertions)]
    ui::headless::run();
}
