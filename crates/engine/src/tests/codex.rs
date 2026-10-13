use super::*;
use serde_json::json;

#[test]
fn codex_ready_provider_can_be_delegated() {
    let mut f = Fixture::new(LiveState::Running);
    let runtime = f.runtime.handle().clone();
    let _entered = runtime.enter();
    let invoke = |f: &mut Fixture, name: &str, arguments| {
        let (reply, result) = oneshot::channel();
        f.actor.handle(EngineMsg::Tool {
            caller: f.thread,
            session: None,
            call: ToolCall {
                name: name.into(),
                arguments,
                input_summary: None,
            },
            reply,
        });
        let result = result.blocking_recv().unwrap();
        assert!(result.ok, "{:?}", result.output_tail);
        serde_json::from_str::<serde_json::Value>(&result.output_tail.unwrap()).unwrap()
    };
    let listed = invoke(&mut f, "list_providers", json!({}));
    assert!(
        listed["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["provider"] == "codex")
    );
    let task = invoke(
        &mut f,
        "start_thread",
        json!({"provider":"codex","title":"Check files","prompt":"Report the files","client_request_id":"codex-child"}),
    );
    let child: ThreadId = serde_json::from_value(task["thread_id"].clone()).unwrap();
    assert_eq!(f.loaded().threads[&child].provider, ProviderKind::Codex);
    assert_eq!(
        f.loaded().threads[&child].lineage,
        Lineage::Child {
            parent: f.thread,
            root: f.thread,
            relationship: Relationship::Subagent
        }
    );
}

#[cfg(debug_assertions)]
#[test]
fn codex_debug_binary_override_is_resolved() {
    if std::env::var_os("VEYRA_TEST_BINARY_OVERRIDE").is_some() {
        let env = env::ChildEnv::isolated().with_binary(
            ProviderKind::Codex,
            std::env::var_os("VEYRA_CODEX_BIN").unwrap().into(),
        );
        assert_eq!(
            env.resolve(ProviderKind::Codex).unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::codex::codex_debug_binary_override_is_resolved",
        ])
        .env("VEYRA_TEST_BINARY_OVERRIDE", "1")
        .env("VEYRA_CODEX_BIN", std::env::current_exe().unwrap())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn codex_params_per_mode() {
    use crate::provider::codex;
    for (mode, approval, sandbox, policy) in [
        (
            PermissionMode::AskFirst,
            "untrusted",
            "read-only",
            json!({"type":"readOnly","networkAccess":true}),
        ),
        (
            PermissionMode::AcceptEdits,
            "on-request",
            "workspace-write",
            json!({"type":"workspaceWrite","networkAccess":true}),
        ),
        (
            PermissionMode::FullAccess,
            "never",
            "danger-full-access",
            json!({"type":"dangerFullAccess"}),
        ),
    ] {
        let config = ThreadConfig {
            model: Some("a-current-model".into()),
            effort: Some("a-cli-effort".into()),
            mode,
        };
        let spec = SessionSpec {
            thread: ThreadId::new(),
            cwd: "/project".into(),
            config: config.clone(),
            native: Some(NativeSessionId("native".into())),
            import: false,
            root: false,
        };
        assert_eq!(
            codex::thread_params(&spec),
            json!({"cwd":"/project","model":"a-current-model","approvalPolicy":approval,"sandbox":sandbox,"approvalsReviewer":"user","threadId":"native","excludeTurns":true})
        );
        assert_eq!(
            codex::turn_params("native", &config, "hello".into()),
            json!({"threadId":"native","input":[{"type":"text","text":"hello","text_elements":[]}],"model":"a-current-model","effort":"a-cli-effort","summary":"auto","approvalPolicy":approval,"approvalsReviewer":"user","sandboxPolicy":policy})
        );
    }
}

#[cfg(debug_assertions)]
fn fake_family(provider: ProviderKind, mode: PermissionMode, files: &[(&str, &str)]) -> Projection {
    use std::{fs, path::PathBuf, time::Duration};
    static LOGS: std::sync::OnceLock<CapturedLogs> = std::sync::OnceLock::new();
    let logs = LOGS.get_or_init(|| {
        let logs = CapturedLogs::default();
        tracing::subscriber::set_global_default(logs.clone()).unwrap();
        logs
    });
    let runtime = Runtime::new().unwrap();
    let _entered = runtime.enter();
    let root = PathBuf::from("/tmp").join(format!("veyra-codex-replay-{}", ThreadId::new()));
    fs::create_dir(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let root = root.canonicalize().unwrap();
    let binary = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex");
    for (name, lane) in files {
        fs::copy(fixtures.join(format!("{name}.jsonl")), root.join(lane)).unwrap();
    }
    let (out, mut rx) = mpsc::channel(256);
    let mut env = env::ChildEnv::isolated();
    env.codex_bin = Some(binary.join("veyra-fake-cli"));
    env.claude_bin = Some(binary.join("veyra-fake-cli"));
    env.fixtures = Some(root.clone());
    env.record = Some(root.join("rec"));
    crate::platform::private_dir(&root.join("data")).unwrap();
    env.mcp = Some(Arc::new(
        crate::mcp::Server::bind(&root.join("data"), binary.join("veyra"), out.clone()).unwrap(),
    ));
    env.catalog.lock().unwrap().insert(
        ProviderKind::Codex,
        Probe {
            provider: ProviderKind::Codex,
            path: env.codex_bin.clone(),
            version: Some("fake".into()),
            readiness: Readiness::Ready,
            models: vec![],
            account: None,
        },
    );
    let groups = env.groups.clone();
    let mut db = Connection::open_in_memory().unwrap();
    store::initialize(&mut db).unwrap();
    let project = Project {
        id: ProjectId::new(),
        path: root.clone(),
        name: "Relay proof".into(),
    };
    let thread = Thread {
        id: ThreadId::new(),
        project: project.id,
        title: "Codex root".into(),
        lineage: Lineage::Root,
        provider,
        config: ThreadConfig {
            model: None,
            effort: None,
            mode,
        },
        cwd: root.clone(),
        native: None,
        native_cursor: None,
        cli_version: None,
        hold: None,
        view: View::Chat,
        import_pending: false,
        archived: false,
    };
    let id = thread.id;
    let mut changes = ChangeSet::default();
    changes.projects.insert(project.id, project);
    changes.threads.insert(id, thread);
    let tx = db.transaction().unwrap();
    store::apply(&tx, &changes, 1).unwrap();
    tx.commit().unwrap();
    let mut actor = Actor::new(db, out, Spawner::Cli(Box::new(env))).unwrap();
    let (reply, result) = oneshot::channel();
    actor.handle(EngineMsg::Command {
        command: Command::Send {
            thread: id,
            text: "Call list_providers.".into(),
        },
        reply,
    });
    assert!(matches!(result.blocking_recv().unwrap(), Ok(Ack::Turn(_))));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(message) = rx.recv().await {
                    let exited = matches!(
                        &message,
                        EngineMsg::Provider {
                            event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                            thread, ..
                        } if *thread == id
                    );
                    actor.handle(message);
                    if exited {
                        return;
                    }
                }
                panic!("actor channel closed");
            })
            .await
            .unwrap();
        })
    }));
    actor.handle(EngineMsg::Shutdown);
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
    if provider == ProviderKind::Codex {
        let relay_pid: u32 = fs::read_to_string(root.join("relay.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            !crate::platform::group_exists(relay_pid).unwrap(),
            "Codex relay survived session exit"
        );
    }
    assert!(
        groups.lock().unwrap().is_empty(),
        "provider groups survived Exited"
    );
    let capture = fs::read_to_string(root.join("rec/t1-s1.jsonl")).unwrap();
    if provider == ProviderKind::Codex {
        assert!(capture.contains("<id:token>"));
        assert!(!root.join("data/run").exists());
    }
    let entries: Vec<_> = logs
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.get("thread") == Some(&id.to_string()))
        .cloned()
        .collect();
    assert!(entries.iter().any(|e| {
        e.get("message")
            .is_some_and(|m| m == "MCP connection accepted")
    }));
    assert!(entries.iter().any(|e| e.get("tool").is_some_and(|m| m
        == if provider == ProviderKind::Codex {
            "list_providers"
        } else {
            "start_thread"
        })));
    for entry in entries {
        println!("relay-log {}", serde_json::to_string(&entry).unwrap());
    }
    store::load(&actor.db).unwrap()
}

#[cfg(debug_assertions)]
#[test]
fn codex_root_reaches_real_relay() {
    let p = fake_family(
        ProviderKind::Codex,
        PermissionMode::AskFirst,
        &[("root-tool-call", "t1-s1.jsonl")],
    );
    assert_eq!(p.turns.len(), 1);
    assert!(p.turns.values().all(|t| matches!(
        t.state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    )));
    assert!(p.items.values().any(|item|matches!(&item.body,ItemBody::Tool(tool) if tool.result.as_ref().and_then(|r|r.output_tail.as_ref()).is_some_and(|text|text.contains("codex") && text.contains("remaining_fan_out")))));
    println!(
        "Codex root completed one turn through the production relay, socket, peer check and list_providers; recorder uses <id:token>; no config file or process group remains."
    );
}
#[cfg(debug_assertions)]
#[test]
fn claude_root_codex_child_runs_on_fake_cli() {
    let p = fake_family(
        ProviderKind::Claude,
        PermissionMode::FullAccess,
        &[
            ("claude-parent-codex-child", "t1-s1.jsonl"),
            ("child-policy-full-access", "t2-s1.jsonl"),
        ],
    );
    assert_eq!(p.threads.len(), 2);
    assert_eq!(p.turns.len(), 3);
    assert_eq!(p.tasks.len(), 1);
    let task = p.tasks.values().next().unwrap();
    assert_eq!(p.threads[&task.child].provider, ProviderKind::Codex);
    assert!(
        matches!(&task.state,TaskState::Settled {summary,delivery:Delivery::Delivered{..},..} if summary=="ok")
    );
    assert!(p.turns.values().all(|t| matches!(
        t.state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    )));
}

#[cfg(debug_assertions)]
#[test]
fn inline_credential_recording_preserves_redacted_binding() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/provider-record-{}", ThreadId::new()));
    std::fs::create_dir_all(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let path = root.join("capture.jsonl");
    let mut recorder =
        crate::record::Recorder::create(&path, "codex", "fake", &root, None, None).unwrap();
    recorder
        .line(
            true,
            json!({"params":{"config":{"mcp_servers.veyra.env":{"VEYRA_MCP_TOKEN":"<id:token>"}}}}),
        )
        .unwrap();
    drop(recorder);
    let line: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .nth(1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        line["line"]["params"]["config"]["mcp_servers.veyra.env"]["VEYRA_MCP_TOKEN"],
        "<id:token>"
    );
}

#[cfg(debug_assertions)]
#[test]
fn recording_distinguishes_changed_native_thread_and_version() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/provider-native-record-{}",
        ThreadId::new()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let path = root.join("capture.jsonl");
    let mut recorder =
        crate::record::Recorder::create(&path, "codex", "unknown", &root, None, None).unwrap();
    recorder.bind("old-native".into(), "<id:session>".into());
    recorder
        .line(
            true,
            json!({"method":"thread/resume","params":{"threadId":"old-native"}}),
        )
        .unwrap();
    recorder.bind("new-native".into(), "<id:session>".into());
    recorder
        .line(
            false,
            json!({"id":2,"result":{"thread":{"id":"new-native","cliVersion":"0.162.1"}}}),
        )
        .unwrap();
    drop(recorder);
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(lines[1]["line"]["params"]["threadId"], "<id:session>");
    assert_eq!(lines[2]["line"]["result"]["thread"]["id"], "<id:session:1>");
    assert_eq!(lines[0]["cli_version"], "0.162.1");
}
