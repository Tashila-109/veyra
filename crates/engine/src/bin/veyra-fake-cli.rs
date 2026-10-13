use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    time::Duration,
};
use veyra_engine::fixture::{self, Matcher, Record};

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(3);
        }
    }
}
fn run() -> io::Result<i32> {
    let path = PathBuf::from(
        std::env::var_os("VEYRA_FAKE_FIXTURE")
            .ok_or_else(|| io::Error::other("VEYRA_FAKE_FIXTURE is missing"))?,
    );
    let source = std::fs::read_to_string(&path)?;
    let header: serde_json::Value =
        serde_json::from_str(source.lines().next().unwrap_or_default())?;
    if header["ignore_sigterm"] == true {
        veyra_engine::platform::ignore_terminate();
    }
    let mut matcher = Matcher::default();
    matcher.bind("<cwd>", serde_json::json!(std::env::current_dir()?));
    let args: Vec<_> = std::env::args().collect();
    if let Some(index) = args
        .iter()
        .position(|a| a == "--session-id" || a == "--resume")
        && let Some(native) = args.get(index + 1)
    {
        matcher.bind("<id:session>", serde_json::json!(native));
    }
    if let Some(index) = args.iter().position(|arg| arg == "--mcp-config") {
        let config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
            args.get(index + 1)
                .ok_or_else(|| io::Error::other("missing MCP config path"))?,
        )?)?;
        let server = &config["mcpServers"]["veyra"];
        if std::env::var_os("VEYRA_MCP_TOKEN").is_some() {
            return Err(io::Error::other(
                "root environment contains a relay credential",
            ));
        }
        for (name, key) in [
            ("mcp_command", "command"),
            ("mcp_args", "args"),
            ("mcp_env", "env"),
        ] {
            matcher.bind(&format!("<id:{name}>"), server[key].clone());
        }
    }
    let mut relay: Option<Relay> = None;
    let mut input = io::stdin().lock().lines();
    let mut output = io::stdout().lock();
    for record in fixture::load(&path)? {
        match record {
            Record::TranscriptStart { .. } => {}
            Record::ExpectOutbound { line } => {
                let actual = input
                    .next()
                    .transpose()?
                    .ok_or_else(|| io::Error::other("expected outbound frame, actual EOF"))?;
                let value: serde_json::Value = serde_json::from_str(&actual)?;
                if value["method"] == "thread/start" || value["method"] == "thread/resume" {
                    let config = &value["params"]["config"];
                    if config.get("mcp_servers.veyra.command").is_some() {
                        for (name, key) in [
                            ("mcp_command", "command"),
                            ("mcp_args", "args"),
                            ("mcp_env", "env"),
                        ] {
                            matcher.bind(
                                &format!("<id:{name}>"),
                                config[format!("mcp_servers.veyra.{key}")].clone(),
                            );
                        }
                    }
                }
                if !matcher.matches(&line, &value) {
                    return Err(io::Error::other("outbound frame did not match the fixture"));
                }
            }
            Record::EmitInbound { line, after_ms } => {
                if let Some(delay) = after_ms {
                    std::thread::sleep(Duration::from_millis(delay));
                }
                writeln!(output, "{}", matcher.expand(line))?;
                output.flush()?;
            }
            Record::EmitRaw { base64, repeat } => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(base64)
                    .map_err(io::Error::other)?;
                for _ in 0..repeat {
                    output.write_all(&bytes)?;
                }
                output.flush()?;
            }
            Record::McpStart => {
                let command = matcher.expand(serde_json::json!("<id:mcp_command>"));
                let args = matcher.expand(serde_json::json!("<id:mcp_args>"));
                let env = matcher.expand(serde_json::json!("<id:mcp_env>"));
                let mut child = std::process::Command::new(
                    command
                        .as_str()
                        .ok_or_else(|| io::Error::other("MCP command binding is missing"))?,
                );
                for arg in args
                    .as_array()
                    .ok_or_else(|| io::Error::other("MCP arguments binding is missing"))?
                {
                    child.arg(
                        arg.as_str()
                            .ok_or_else(|| io::Error::other("MCP argument must be a string"))?,
                    );
                }
                match env {
                    serde_json::Value::Object(env) => {
                        for (name, value) in env {
                            child.env(
                                name,
                                value.as_str().ok_or_else(|| {
                                    io::Error::other("MCP environment value must be a string")
                                })?,
                            );
                        }
                    }
                    serde_json::Value::Array(env) => {
                        for pair in env {
                            child.env(
                                pair["name"].as_str().ok_or_else(|| {
                                    io::Error::other("MCP environment name is missing")
                                })?,
                                pair["value"].as_str().ok_or_else(|| {
                                    io::Error::other("MCP environment value is missing")
                                })?,
                            );
                        }
                    }
                    _ => return Err(io::Error::other("MCP environment binding is missing")),
                }
                if relay.is_some() {
                    return Err(io::Error::other("MCP relay is already running"));
                }
                child
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null());
                let mut command = tokio::process::Command::from(child);
                if header["provider"] == "codex" {
                    veyra_engine::platform::own_group(&mut command);
                }
                let mut child = command.as_std_mut().spawn()?;
                let input = child.stdin.take().unwrap();
                let output = io::BufReader::new(child.stdout.take().unwrap());
                relay = Some(Relay {
                    child,
                    input: Some(input),
                    output,
                });
            }
            Record::RecordRelayPid { path } => {
                let path = std::path::Path::new(&path);
                if path.is_absolute()
                    || path
                        .components()
                        .any(|part| part == std::path::Component::ParentDir)
                {
                    return Err(io::Error::other(
                        "relay pid path must stay within the project folder",
                    ));
                }
                let relay = relay
                    .as_ref()
                    .ok_or_else(|| io::Error::other("MCP relay has not started"))?;
                std::fs::write(path, relay.child.id().to_string())?;
            }
            Record::McpSend { line } => {
                let relay = relay
                    .as_mut()
                    .ok_or_else(|| io::Error::other("MCP relay has not started"))?;
                let input = relay.input.as_mut().unwrap();
                writeln!(input, "{}", matcher.expand(line))?;
                input.flush()?;
            }
            Record::McpExpect { line } => {
                let relay = relay
                    .as_mut()
                    .ok_or_else(|| io::Error::other("MCP relay has not started"))?;
                let mut actual = String::new();
                relay.output.read_line(&mut actual)?;
                let value = serde_json::from_str(&actual)
                    .map_err(|_| io::Error::other("MCP reply was not JSON"))?;
                if !matcher.matches(&line, &value) {
                    return Err(io::Error::other("MCP reply did not match the fixture"));
                }
            }
            Record::RecordArgv { path } => {
                let path = std::path::Path::new(&path);
                if path.is_absolute()
                    || path
                        .components()
                        .any(|part| part == std::path::Component::ParentDir)
                {
                    return Err(io::Error::other(
                        "argv record path must stay within the project folder",
                    ));
                }
                std::fs::write(path, serde_json::to_vec(&args)?)?;
            }
            Record::Touch { path } => {
                let path = std::path::Path::new(&path);
                if path.is_absolute()
                    || path
                        .components()
                        .any(|part| part == std::path::Component::ParentDir)
                {
                    return Err(io::Error::other(
                        "touch path must stay within the project folder",
                    ));
                }
                std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(false)
                    .open(path)?;
            }
            Record::Spin { ms } => {
                let start = std::time::Instant::now();
                while start.elapsed() < Duration::from_millis(ms) {
                    std::hint::spin_loop();
                }
            }
            Record::Exit { code, stderr } => {
                for line in stderr {
                    eprintln!("{line}");
                }
                return Ok(code);
            }
        }
    }
    Ok(0)
}

struct Relay {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    output: io::BufReader<std::process::ChildStdout>,
}
impl Drop for Relay {
    fn drop(&mut self) {
        drop(self.input.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
