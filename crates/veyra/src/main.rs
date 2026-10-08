#[cfg(debug_assertions)]
mod control;
pub mod ui;

use anyhow::Context;
use std::{fs::OpenOptions, process};
use veyra_engine::{
    data_dir::{DataDir, LockError},
    platform,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None => {}
        #[cfg(debug_assertions)]
        Some("ctl") => return control::client(),
        Some(_) => {
            eprintln!("usage: veyra [mcp]");
            process::exit(2);
        }
    }
    let data = DataDir::resolve().context("resolve Veyra data folder")?;
    let _lock = match data.lock() {
        Ok(lock) => lock,
        Err(LockError::AlreadyRunning) => {
            eprintln!("Veyra is already running");
            process::exit(1);
        }
        Err(error) => return Err(error.into()),
    };
    let log = platform::private_file(
        &data.path().join("logs/veyra.log"),
        OpenOptions::new().create(true).append(true),
    )
    .context("open Veyra log")?;
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(log)
        .init();
    tracing::info!(build_rev = env!("VEYRA_BUILD_REV"), "Veyra starting");
    #[cfg(debug_assertions)]
    let requests = control::listen(data.path())?;
    ui::run(
        data.path().to_owned(),
        #[cfg(debug_assertions)]
        requests,
    );
    Ok(())
}
