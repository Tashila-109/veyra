use std::{
    io::{self, Write},
    path::PathBuf,
};
use veyra_engine::platform;

pub fn run() -> anyhow::Result<()> {
    let socket = PathBuf::from(
        std::env::var_os("VEYRA_SOCKET")
            .ok_or_else(|| io::Error::other("VEYRA_SOCKET is missing"))?,
    );
    let token = std::env::var("VEYRA_MCP_TOKEN")
        .map_err(|_| io::Error::other("VEYRA_MCP_TOKEN is missing"))?;
    let mut connection = platform::connect_socket(&socket)?;
    connection.write_all(token.as_bytes())?;
    connection.write_all(b"\n")?;
    connection.flush()?;
    drop(token);
    let mut input_connection = connection.try_clone()?;
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut input_connection);
        let _ = input_connection.shutdown(std::net::Shutdown::Write);
    });
    io::copy(&mut connection, &mut io::stdout().lock())?;
    Ok(())
}
