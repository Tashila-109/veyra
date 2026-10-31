//! Main-thread runner for the production views' headless Metal checks.
// The imported production modules and their unit-test helpers have no callers in this separate runner.
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
    #[cfg(not(debug_assertions))]
    println!("headless checks run in debug builds only");
}
