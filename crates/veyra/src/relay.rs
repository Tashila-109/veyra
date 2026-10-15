use std::{io, path::PathBuf};
use veyra_engine::platform;

pub fn run() -> anyhow::Result<()> {
    let socket = PathBuf::from(
        std::env::var_os("VEYRA_SOCKET")
            .ok_or_else(|| io::Error::other("VEYRA_SOCKET is missing"))?,
    );
    let token = std::env::var("VEYRA_MCP_TOKEN")
        .map_err(|_| io::Error::other("VEYRA_MCP_TOKEN is missing"))?;
    let connection = platform::connect_socket(&socket)?;
    platform::relay_stdio(connection, token.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::BufRead,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    #[ignore = "owned stdio fixture entrypoint"]
    fn relay_child() {
        assert!(
            std::env::var_os("VEYRA_RELAY_CHILD").is_some(),
            "fixture entrypoint requires its run marker"
        );
        std::process::exit(if run().is_ok() { 0 } else { 1 });
    }
    #[test]
    fn relay_stdin_eof_exits_with_socket_held_open() {
        let root = PathBuf::from("/tmp").join(format!(
            "veyra-codex-replay-{}",
            veyra_engine::domain::ThreadId::new()
        ));
        platform::private_dir(&root).unwrap();
        let _scratch = Scratch(root.clone());
        let socket = root.join("held.sock");
        let server = platform::bind_socket(&socket).unwrap();
        server.set_nonblocking(true).unwrap();
        let token = veyra_engine::mcp::McpToken::new();
        let mut child = Child(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "relay::tests::relay_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("VEYRA_RELAY_CHILD", "1")
                .env("VEYRA_SOCKET", &socket)
                .env("VEYRA_MCP_TOKEN", token.expose())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let connection = loop {
            match server.accept() {
                Ok((socket, _)) => break socket,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };
        connection.set_nonblocking(false).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut authentication = String::new();
        io::BufReader::new(connection.try_clone().unwrap())
            .read_line(&mut authentication)
            .unwrap();
        drop(authentication);
        drop(child.0.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(1);
        let exited = loop {
            if child.0.try_wait().unwrap().is_some() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(connection);
        assert!(
            exited,
            "relay stayed alive after stdin EOF while the socket remained open"
        );
    }
    #[test]
    #[ignore = "owned parent-process fixture entrypoint"]
    fn codex_stdio_child() {
        if let Some(path) = std::env::var_os("VEYRA_CODEX_RELAY_PID") {
            let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "relay::tests::relay_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("VEYRA_RELAY_CHILD", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            platform::own_group(&mut command);
            let relay = Child(command.as_std_mut().spawn().unwrap());
            std::fs::write(path, relay.0.id().to_string()).unwrap();
            let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
        }
    }
    #[test]
    fn codex_process_end_leaves_no_relay() {
        let root = PathBuf::from("/tmp").join(format!(
            "veyra-codex-replay-{}",
            veyra_engine::domain::ThreadId::new()
        ));
        platform::private_dir(&root).unwrap();
        let _scratch = Scratch(root.clone());
        let socket = root.join("held.sock");
        let server = platform::bind_socket(&socket).unwrap();
        server.set_nonblocking(true).unwrap();
        let token = veyra_engine::mcp::McpToken::new();
        let pid_path = root.join("relay.pid");
        let mut codex = Child(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "relay::tests::codex_stdio_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("VEYRA_CODEX_RELAY_PID", &pid_path)
                .env("VEYRA_SOCKET", &socket)
                .env("VEYRA_MCP_TOKEN", token.expose())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let connection = loop {
            match server.accept() {
                Ok((socket, _)) => break socket,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };
        connection.set_nonblocking(false).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut authentication = String::new();
        io::BufReader::new(connection.try_clone().unwrap())
            .read_line(&mut authentication)
            .unwrap();
        drop(authentication);
        let pid: u32 = std::fs::read_to_string(pid_path).unwrap().parse().unwrap();
        codex.0.kill().unwrap();
        codex.0.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let ended = loop {
            if !platform::group_exists(pid).unwrap() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if !ended {
            let _ = platform::signal_group(pid, platform::Signal::Kill);
        }
        drop(connection);
        assert!(
            ended,
            "the Codex stdio process ended but its separate relay group survived"
        );
    }
    #[test]
    fn relay_eof_exits_with_blocked_socket_and_stdout_writes() {
        use std::io::Write;
        use tokio::io::AsyncWriteExt;
        let root = PathBuf::from("/tmp").join(format!(
            "veyra-relay-blocked-{}",
            veyra_engine::domain::ThreadId::new()
        ));
        platform::private_dir(&root).unwrap();
        let _scratch = Scratch(root.clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for case in ["positive", "socket_write_blocked", "stdout_write_blocked"] {
            let socket = root.join("relay.sock");
            let server = platform::bind_socket(&socket).unwrap();
            server.set_nonblocking(true).unwrap();
            let token = veyra_engine::mcp::McpToken::new();
            let mut child = Child(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "relay::tests::relay_child",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("VEYRA_RELAY_CHILD", "1")
                    .env("VEYRA_SOCKET", &socket)
                    .env("VEYRA_MCP_TOKEN", token.expose())
                    .stdin(Stdio::piped())
                    .stdout(if case == "stdout_write_blocked" {
                        Stdio::piped()
                    } else {
                        Stdio::null()
                    })
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut connection = loop {
                match server.accept() {
                    Ok((connection, _)) => break connection,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            connection.set_nonblocking(false).unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut authentication = String::new();
            io::BufReader::new(connection.try_clone().unwrap())
                .read_line(&mut authentication)
                .unwrap();
            assert!(authentication.ends_with('\n'));
            drop(authentication);
            let input = child.0.stdin.take().unwrap();
            let mut sent = 0;
            if case == "socket_write_blocked" {
                runtime.block_on(async {
                    let mut input = tokio::process::ChildStdin::from_std(input).unwrap();
                    let _ = tokio::time::timeout(Duration::from_millis(500), async {
                        while sent < 262144 {
                            sent += input.write(&[b'x'; 65536]).await.unwrap();
                        }
                    })
                    .await;
                });
            } else {
                if case == "stdout_write_blocked" {
                    connection.set_nonblocking(true).unwrap();
                    let deadline = Instant::now() + Duration::from_millis(500);
                    while sent < 262144 && Instant::now() < deadline {
                        match connection.write(&[b'x'; 65536]) {
                            Ok(count) => sent += count,
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(5))
                            }
                            Err(error) => panic!("{error}"),
                        }
                    }
                }
                drop(input);
            }
            if case != "positive" {
                assert!(sent > 0);
            }
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                if let Some(status) = child.0.try_wait().unwrap() {
                    assert!(status.success(), "relay failed in {case}");
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "relay survived stdin EOF in {case} after {sent} bytes"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            println!("relay EOF case={case} payload_bytes={sent} exited=true");
            drop(connection);
            drop(server);
            platform::remove_optional_file(&socket).unwrap();
        }
    }
}
