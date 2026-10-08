pub enum Signal {
    Hangup,
    Terminate,
    Kill,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
compile_error!(
    "Veyra v0.0.1 builds only on macOS. A port fills crates/engine/src/platform/, see ARCHITECTURE.md."
);

#[cfg(test)]
mod boundary;
