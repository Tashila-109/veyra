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
        let root = PathBuf::from("/tmp").join(format!(
            "veyra-relay-blocked-{}",
            veyra_engine::domain::ThreadId::new()
        ));
        platform::private_dir(&root).unwrap();
        let _scratch = Scratch(root.clone());
        let script = r#"
import json,os,pathlib,secrets,socket,subprocess,sys,time
root=pathlib.Path(sys.argv[1]); results=[]
for case in ['positive','socket_write_blocked','stdout_write_blocked']:
    path=root/'relay.sock'
    server=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
    server.bind(str(path));server.listen();server.settimeout(3)
    env=os.environ.copy();env['VEYRA_SOCKET']=str(path);env['VEYRA_MCP_TOKEN']=secrets.token_hex(32);env['VEYRA_RELAY_CHILD']='1'
    child=subprocess.Popen([sys.argv[2],'--exact','relay::tests::relay_child','--ignored','--nocapture'],stdin=subprocess.PIPE,
        stdout=subprocess.PIPE if case=='stdout_write_blocked' else subprocess.DEVNULL,stderr=subprocess.DEVNULL,env=env)
    del env['VEYRA_MCP_TOKEN'];conn=None
    try:
        conn,_=server.accept();conn.settimeout(3)
        while conn.recv(1)!=b'\n': pass
        sent=0;deadline=time.monotonic()+0.5
        if case=='socket_write_blocked':
            os.set_blocking(child.stdin.fileno(),False)
            while time.monotonic()<deadline and sent<262144:
                try:sent+=os.write(child.stdin.fileno(),b'x'*65536)
                except BlockingIOError:time.sleep(0.005)
        elif case=='stdout_write_blocked':
            conn.setblocking(False)
            while time.monotonic()<deadline and sent<262144:
                try:sent+=conn.send(b'x'*65536)
                except BlockingIOError:time.sleep(0.005)
        child.stdin.close()
        try:child.wait(timeout=1);alive=False
        except subprocess.TimeoutExpired:alive=True
        results.append({'case':case,'payload_bytes':sent,'alive_after_eof':alive})
    finally:
        if child.poll() is None:child.kill()
        child.wait(timeout=3)
        if child.stdout:child.stdout.close()
        if conn:conn.close()
        server.close();path.unlink(missing_ok=True)
print(json.dumps(results))
"#;
        let output = Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(&root)
            .arg(std::env::current_exe().unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(cases.len(), 3);
        for case in &cases {
            if case["case"] != "positive" {
                assert!(case["payload_bytes"].as_u64().unwrap() > 0);
            }
            assert_eq!(case["alive_after_eof"], false, "{cases:?}");
        }
    }
}
