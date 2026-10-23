pub enum Signal {
    Hangup,
    Terminate,
    Kill,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayOptions {
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
compile_error!(
    "Veyra v0.0.1 builds only on macOS. A port fills crates/engine/src/platform/, including user_shell, \
    foreground_group and child_pids; see ARCHITECTURE.md."
);

#[cfg(test)]
mod boundary;
