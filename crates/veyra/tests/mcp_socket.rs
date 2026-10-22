#![allow(clippy::disallowed_types)] // Socket tests construct and inspect MCP and configuration wire JSON.
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};
use veyra_engine::{
    actor::EngineMsg,
    domain::ThreadId,
    mcp::{Configuration, McpToken, Server},
    platform,
};
struct Run {
    root: PathBuf,
    child: Option<Child>,
    config: Option<Configuration>,
}
impl Drop for Run {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn root() -> Run {
    let root = PathBuf::from("/tmp").join(format!("veyra-mcp-{}", ThreadId::new()));
    platform::private_dir(&root).unwrap();
    Run {
        root: root.canonicalize().unwrap(),
        child: None,
        config: None,
    }
}
fn relay() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_veyra"))
        .canonicalize()
        .unwrap()
}

fn fixture(run: &Run) -> PathBuf {
    let path = run.root.join("fixture.jsonl");
    let records = [
        json!({"type":"transcript_start","provider":"claude","protocol":"stream-json","cli_version":"fake","sce\
            nario":"socket authentication","source":"handwritten"}),
        json!({"type":"record_argv","path":"argv.json"}),
        json!({"type":"expect_outbound","line":{"go":true}}),
        json!({"type":"mcp_start"}),
        json!({"type":"mcp_send","line":{"jsonrpc":"2.0","id":1,"method":"server/discover"}}),
        json!({"type":"mcp_expect","line":{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not \
            found"}}}),
        json!({"type":"mcp_send","line":{"jsonrpc":"2.0","method":"notifications/initialized"}}),
        json!({"type":"mcp_send","line":{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersio\
            n":"2024-11-05"}}}),
        json!({"type":"mcp_expect","line":{"jsonrpc":"2.0","id":2,"result":"<any>"}}),
        json!({"type":"touch","path":"connected"}),
        json!({"type":"expect_outbound","line":{"finish":true}}),
        json!({"type":"exit","code":0}),
    ];
    std::fs::write(
        &path,
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    path
}
async fn started(run: &mut Run) -> (Server, mpsc::Receiver<EngineMsg>, String, ThreadId) {
    let fixture = fixture(run);
    started_fixture(run, fixture).await
}
async fn started_fixture(
    run: &mut Run,
    fixture: PathBuf,
) -> (Server, mpsc::Receiver<EngineMsg>, String, ThreadId) {
    let (out, rx) = mpsc::channel(16);
    let server = Server::bind(&run.root, relay(), out).unwrap();
    let mut copy = None;
    let thread = ThreadId::new();
    let child = server
        .for_spawn(thread, 1, |token| {
            copy = Some(token.expose().to_owned());
            let config = Configuration::create(&run.root, thread, &relay(), &server.socket, token)?;
            let child = Command::new(env!("CARGO_BIN_EXE_veyra-test-cli"))
                .args(["--mcp-config", config.path.to_str().unwrap()])
                .env("VEYRA_FAKE_FIXTURE", &fixture)
                .current_dir(&run.root)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let pid = child.id();
            run.config = Some(config);
            Ok((child, pid))
        })
        .unwrap();
    run.child = Some(child);
    (server, rx, copy.unwrap(), thread)
}
async fn finish(run: &mut Run) {
    let child = run.child.as_mut().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"finish\":true}\n")
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(status.success(), "fake client failed its MCP expectations");
}
async fn go(run: &mut Run) {
    run.child
        .as_mut()
        .unwrap()
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"go\":true}\n")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !run.root.join("connected").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn socket_refuses_foreign_peer() {
    let mut run = root();
    let (server, _rx, token, thread) = started(&mut run).await;
    let mut foreign = platform::connect_async_socket(&server.socket)
        .await
        .unwrap();
    foreign.write_all(token.as_bytes()).await.unwrap();
    foreign.write_all(b"\n").await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), foreign.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.is_empty(), "foreign process received MCP authority");
    go(&mut run).await;
    let mut duplicate = platform::connect_async_socket(&server.socket)
        .await
        .unwrap();
    duplicate.write_all(token.as_bytes()).await.unwrap();
    duplicate.write_all(b"\n").await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), duplicate.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.is_empty(), "copied token opened a second connection");
    server.revoke(thread, 1);
    finish(&mut run).await;
}
#[tokio::test]
async fn token_never_in_argv() {
    let mut run = root();
    let (_server, _rx, token, _thread) = started(&mut run).await;
    go(&mut run).await;
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(run.root.join("argv.json")).unwrap()).unwrap();
    assert!(args.iter().any(|arg| arg == "--mcp-config"));
    assert!(
        !args.iter().any(|arg| arg.contains(&token)),
        "token entered the fake CLI argv"
    );
    let config = run.config.as_ref().unwrap();
    assert!(platform::is_private(&config.path).unwrap());
    config.system_init(&json!({"type":"system","subtype":"init","mcp_servers":[{"name":"veyra","status":"connec\
        ted","source":"dynamic"}]})).unwrap();
    assert!(
        !config.path.exists(),
        "system/init left the credential file behind"
    );
    assert_eq!(format!("{:?}", McpToken::new()), "McpToken(***)");
    assert!(platform::is_private(&run.root).unwrap());
    assert!(platform::is_private(&run.root.join("veyra.sock")).unwrap());
    finish(&mut run).await;
}
#[tokio::test]
async fn unknown_request_is_answered_and_notification_is_silent() {
    let caller = ThreadId::new();
    let (out, _rx) = mpsc::channel(1);
    let answer = tokio::time::timeout(
        Duration::from_millis(50),
        veyra_engine::mcp::response(
            caller,
            json!({"jsonrpc":"2.0","id":17,"method":"server/discover"}),
            &out,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        answer,
        Some(json!({"jsonrpc":"2.0","id":17,"error":{"code":-32601,"message":"Method not found"}}))
    );
    assert_eq!(
        veyra_engine::mcp::response(
            caller,
            json!({"jsonrpc":"2.0","method":"server/discover"}),
            &out
        )
        .await,
        None
    );
    let answer = veyra_engine::mcp::response(
        caller,
        json!({"jsonrpc":"2.0","id":18,"method":"ping"}),
        &out,
    )
    .await;
    assert_eq!(answer, Some(json!({"jsonrpc":"2.0","id":18,"result":{}})));
}
#[tokio::test]
async fn tools_list_describes_every_tool() {
    let caller = ThreadId::new();
    let (out, _rx) = mpsc::channel(1);
    let list = veyra_engine::mcp::response(caller, json!({"id":1,"method":"tools/list"}), &out)
        .await
        .unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "list_providers",
            "start_thread",
            "get_task",
            "message_thread",
            "stop_thread"
        ]
    );
    let init = veyra_engine::mcp::response(caller, json!({"id":2,"method":"initialize"}), &out)
        .await
        .unwrap();
    let instructions = init["result"]["instructions"].as_str().unwrap();
    for tool in tools {
        let description = tool["description"].as_str().unwrap();
        assert!(!description.is_empty());
        let base = description
            .split(" Family rules are in start_thread's description.")
            .next()
            .unwrap()
            .split(" Only a running root")
            .next()
            .unwrap();
        assert!(instructions.contains(base));
        if tool["name"] == "start_thread" {
            assert!(description.contains("Depth is 1"));
        } else {
            assert!(description.contains("start_thread's description"));
        }
        assert!(description.contains("refusal"));
    }
    for rule in [
        "4 children",
        "16 threads",
        "Depth is 1",
        "100,000",
        "4,000",
        "do not poll",
        "mode",
        "client_request_id",
        "own tasks",
    ] {
        assert!(
            instructions.contains(rule),
            "missing first-call rule {rule}"
        );
    }
}

#[test]
fn mcp_config_is_unlinked_for_every_reported_status() {
    for status in ["connected", "failed", "needs-auth", "pending"] {
        let run = root();
        platform::private_dir(&run.root.join("run")).unwrap();
        let config = veyra_engine::mcp::Configuration::create(
            &run.root,
            ThreadId::new(),
            &relay(),
            &run.root.join("veyra.sock"),
            &McpToken::new(),
        )
        .unwrap();
        assert!(config.path.is_file());
        assert!(platform::is_private(&config.path).unwrap());
        assert!(platform::is_private(config.path.parent().unwrap()).unwrap());
        config.system_init(&json!({"type":"system","subtype":"init","mcp_servers":[{"name":"other","status":"co\
            nnected"}]})).unwrap();
        assert!(config.path.is_file());
        config
            .system_init(
                &json!({"type":"system","subtype":"init","mcp_servers":[{"name":"veyra","status\
            ":status,"source":"dynamic"}]}),
            )
            .unwrap();
        assert!(
            !config.path.exists(),
            "reported server left its config on disk"
        );
    }
}
#[test]
fn mcp_config_is_unlinked_when_session_ends_before_init() {
    let run = root();
    platform::private_dir(&run.root.join("run")).unwrap();
    let config = veyra_engine::mcp::Configuration::create(
        &run.root,
        ThreadId::new(),
        &relay(),
        &run.root.join("veyra.sock"),
        &McpToken::new(),
    )
    .unwrap();
    let path = config.path.clone();
    assert!(path.is_file());
    drop(config);
    assert!(!path.exists());
}

#[cfg(debug_assertions)]
async fn session_config_lifecycle(root_thread: bool, reported: Option<&str>) {
    use veyra_engine::{
        domain::*,
        provider::{
            ProviderEvent, SessionControl, SessionEvent, SessionSpec, TurnEvent, env::ChildEnv,
            session,
        },
    };
    let run = root();
    platform::private_dir(&run.root.join("fixtures")).unwrap();
    platform::private_dir(&run.root.join("rec")).unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../engine/tests/fixtures/claude/mcp-connected.jsonl");
    let mut records: Vec<Value> = std::fs::read_to_string(source)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    records.insert(1, json!({"type":"record_argv","path":"argv.json"}));
    if let Some(status) = reported {
        for record in &mut records {
            if record["line"]["subtype"] == "init" {
                record["line"]["mcp_servers"][0]["status"] = json!(status);
            }
        }
    } else {
        records.retain(|record| record["line"]["subtype"] != "init");
    }
    let fixture = run.root.join("fixtures/t1-s1.jsonl");
    std::fs::write(
        fixture,
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let (out, mut events) = mpsc::channel(256);
    let server = Server::bind(&run.root, relay(), out.clone()).unwrap();
    let mut env = ChildEnv::isolated();
    env.mcp = Some(std::sync::Arc::new(server));
    env.claude_bin = Some(PathBuf::from(env!("CARGO_BIN_EXE_veyra-test-cli")));
    env.fixtures = Some(run.root.join("fixtures"));
    env.record = Some(run.root.join("rec"));
    let thread = ThreadId::new();
    let config = run
        .root
        .join("run")
        .join(thread.to_string())
        .join("mcp.json");
    let handle = session::spawn(
        SessionSpec {
            thread,
            cwd: run.root.clone(),
            config: ThreadConfig {
                mode: PermissionMode::FullAccess,
                model: None,
                effort: None,
            },
            native: None,

            root: root_thread,
        },
        env,
        out,
        1,
    );
    let mut token = None;
    let mut started = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if let EngineMsg::Provider {
                event: ProviderEvent::Session(SessionEvent::Dispatched { turn }),
                ..
            } = &event
            {
                handle
                    .control
                    .send(SessionControl::DispatchCommitted { turn: *turn })
                    .unwrap();
            }
            match event {
                EngineMsg::Provider {
                    event: ProviderEvent::Session(SessionEvent::Ready { .. }),
                    ..
                } => {
                    if started {
                        continue;
                    }
                    started = true;
                    assert_eq!(config.is_file(), root_thread);
                    if root_thread {
                        let value: Value =
                            serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
                        token = value["mcpServers"]["veyra"]["env"]["VEYRA_MCP_TOKEN"]
                            .as_str()
                            .map(str::to_owned);
                    }
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn: TurnId::new(),
                            text: "Connection check".into(),
                        })
                        .unwrap();
                }
                EngineMsg::Provider {
                    event:
                        ProviderEvent::Turn {
                            event: TurnEvent::TurnEnded { .. },
                            ..
                        },
                    ..
                } => {
                    if reported.is_some() {
                        assert!(!config.exists(), "reported MCP server left its config");
                    }
                }
                EngineMsg::Provider {
                    event: ProviderEvent::Session(SessionEvent::Exited { crash }),
                    ..
                } => {
                    assert!(crash.is_none(), "fake session crashed");
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(
        !config.exists(),
        "finished session left its credential file"
    );
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(run.root.join("argv.json")).unwrap()).unwrap();
    assert_eq!(args.iter().any(|arg| arg == "--mcp-config"), root_thread);
    if let Some(token) = token {
        assert!(
            !args.iter().any(|arg| arg.contains(&token)),
            "credential entered argv"
        );
        for file in std::fs::read_dir(run.root.join("rec")).unwrap() {
            assert!(
                !std::fs::read_to_string(file.unwrap().path())
                    .unwrap()
                    .contains(&token),
                "credential entered a recording"
            );
        }
    }
}
#[cfg(debug_assertions)]
#[tokio::test]
async fn root_session_owns_private_config_until_init_or_exit() {
    for status in [Some("connected"), Some("failed"), None] {
        session_config_lifecycle(true, status).await;
    }
}
#[cfg(debug_assertions)]
#[tokio::test]
async fn child_spawn_has_no_tool_server() {
    session_config_lifecycle(false, Some("connected")).await;
}
#[test]
fn session_hash_redacts_token_in_nested_values_and_hex_neighbors() {
    let token = McpToken::new();
    let hash = veyra_engine::mcp::TokenHash(token.hash());
    let mut value = json!({token.expose():[format!("a{}b",token.expose()), {"content":format!("prefix {} suffix\
        ",token.expose())}]});
    hash.sanitize(&mut value, "<id:token>");
    assert!(
        !value.to_string().contains(token.expose()),
        "credential escaped sanitization"
    );
    assert_eq!(
        value,
        json!({"<id:token>":["a<id:token>b",{"content":"prefix <id:token> suffix"}]})
    );
}

#[tokio::test]
async fn startup_removes_dead_session_configs() {
    let run = root();
    let stale = run
        .root
        .join("run")
        .join(ThreadId::new().to_string())
        .join("mcp.json");
    platform::private_dir(stale.parent().unwrap()).unwrap();
    platform::private_text(&stale, "expired configuration").unwrap();
    let (out, _rx) = mpsc::channel(1);
    let _server = Server::bind(&run.root, relay(), out).unwrap();
    assert!(!stale.exists(), "startup kept a dead session config");
}

#[tokio::test]
async fn production_relay_with_wrong_parent_is_refused() {
    let mut run = root();
    let (server, _rx, token, _thread) = started(&mut run).await;
    let mut child = tokio::process::Command::new(relay())
        .arg("mcp")
        .env("VEYRA_SOCKET", &server.socket)
        .env("VEYRA_MCP_TOKEN", &token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n")
        .await
        .unwrap();
    let mut output = Vec::new();
    let read = tokio::time::timeout(
        Duration::from_secs(3),
        child.stdout.take().unwrap().read_to_end(&mut output),
    )
    .await;
    let _ = child.kill().await;
    let _ = child.wait().await;
    assert!(
        matches!(read, Ok(Ok(_))),
        "wrong-parent relay did not close"
    );
    assert!(output.is_empty(), "wrong-parent relay received authority");
    go(&mut run).await;
    finish(&mut run).await;
}
#[tokio::test]
async fn second_relay_from_the_same_cli_cannot_reuse_a_token() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let run = root();
    let (out, _rx) = mpsc::channel(16);
    let server = Server::bind(&run.root, relay(), out).unwrap();
    let token = server
        .for_spawn(ThreadId::new(), 1, |token| {
            Ok((token.expose().to_owned(), std::process::id()))
        })
        .unwrap();
    for connection in 1..=2 {
        let mut child = tokio::process::Command::new(relay())
            .arg("mcp")
            .env("VEYRA_SOCKET", &server.socket)
            .env("VEYRA_MCP_TOKEN", &token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n")
            .await
            .unwrap();
        let mut output = String::new();
        let read = tokio::time::timeout(
            Duration::from_secs(3),
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut output),
        )
        .await;
        drop(input);
        let _ = child.kill().await;
        let _ = child.wait().await;
        assert!(
            matches!(read, Ok(Ok(_))),
            "relay response exceeded its bound"
        );
        if connection == 1 {
            let value: Value = serde_json::from_str(&output).unwrap();
            assert_eq!(value["result"]["serverInfo"]["name"], "veyra");
        } else {
            assert!(output.is_empty(), "second relay's init was accepted");
        }
    }
}
