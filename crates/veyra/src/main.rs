#[cfg(debug_assertions)]
mod control;
mod relay;
pub mod ui;
#[cfg(test)]
mod verify_map;

use anyhow::Context;
use std::{fs::OpenOptions, process};
use veyra_engine::{
    Engine, StartError,
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
        Some("mcp") => return relay::run(),
        #[cfg(debug_assertions)]
        Some("ctl") => return control::client(),
        Some(_) => {
            eprintln!("usage: veyra [mcp]");
            process::exit(2);
        }
    }
    let data = DataDir::resolve().context("resolve Veyra data folder")?;
    let runtime = tokio::runtime::Runtime::new().context("start session runtime")?;
    let data_path = data.path().to_owned();
    let log = platform::private_file(
        &data_path.join("logs/veyra.log"),
        OpenOptions::new().create(true).append(true),
    )
    .context("open Veyra log")?;
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_writer(log)
        .init();
    let _engine = match Engine::start(runtime.handle(), data) {
        Ok(engine) => engine,
        Err(StartError::Lock(LockError::AlreadyRunning)) => {
            eprintln!("Veyra is already running");
            process::exit(1);
        }
        Err(error) => return Err(error.into()),
    };
    tracing::info!(build_rev = env!("VEYRA_BUILD_REV"), "Veyra starting");
    #[cfg(debug_assertions)]
    let requests = control::listen(&data_path, _engine.schema_version())?;
    ui::run(
        _engine,
        runtime.handle().clone(),
        data_path,
        #[cfg(debug_assertions)]
        requests,
    );
    Ok(())
}
