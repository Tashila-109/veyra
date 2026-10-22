#![allow(clippy::disallowed_types)] // Relay tests inspect MCP tool-result wire JSON.
#![cfg(debug_assertions)]
use std::{fs, path::PathBuf, time::Duration};
use tokio::runtime::Runtime;
use veyra_engine::{
    Engine,
    commands::{Ack, Command},
    data_dir::DataDir,
    domain::*,
    events::ChangeSet,
    platform,
    provider::{Probes, ProviderKind, Readiness, env},
    store,
};
#[path = "../../engine/src/tests/logs.rs"]
mod logs;
use logs::CapturedLogs;
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[cfg(debug_assertions)]
fn fake_family(
    provider: ProviderKind,
    mode: PermissionMode,
    files: &[(&str, &str)],
    native: Option<NativeSessionId>,
) -> Projection {
    static LOGS: std::sync::OnceLock<CapturedLogs> = std::sync::OnceLock::new();
    let logs = LOGS.get_or_init(|| {
        let logs = CapturedLogs::default();
        tracing::subscriber::set_global_default(logs.clone()).unwrap();
        logs
    });
    let runtime = Runtime::new().unwrap();
    let _entered = runtime.enter();
    let root = PathBuf::from("/tmp").join(format!(
        "veyra-{}-relay-{}",
        provider.binary(),
        ThreadId::new()
    ));
    fs::create_dir(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let root = root.canonicalize().unwrap();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../engine/tests/fixtures")
        .join(if provider == ProviderKind::Grok {
            "grok"
        } else {
            "codex"
        });
    for (name, lane) in files {
        fs::copy(fixtures.join(format!("{name}.jsonl")), root.join(lane)).unwrap();
    }
    let mut env = env::ChildEnv::isolated();
    env.codex_bin = Some(PathBuf::from(env!("CARGO_BIN_EXE_veyra-test-cli")));
    env.claude_bin = Some(PathBuf::from(env!("CARGO_BIN_EXE_veyra-test-cli")));
    env.grok_bin = Some(PathBuf::from(env!("CARGO_BIN_EXE_veyra-test-cli")));
    env.fixtures = Some(root.clone());
    env.record = Some(root.join("rec"));
    platform::private_dir(&root.join("data")).unwrap();
    env.relay_command = Some(PathBuf::from(env!("CARGO_BIN_EXE_veyra")));
    let probed = if provider == ProviderKind::Grok {
        ProviderKind::Grok
    } else {
        ProviderKind::Codex
    };
    fs::copy(
        fixtures.join(if probed == ProviderKind::Grok {
            "probe-grok.jsonl"
        } else {
            "probe-ready.jsonl"
        }),
        root.join(format!("probe-{}.jsonl", probed.binary())),
    )
    .unwrap();
    let mut probes = Probes::default();
    let ready = probes.check(runtime.handle(), probed, env.clone(), root.clone());
    assert_eq!(runtime.block_on(ready).unwrap().readiness, Readiness::Ready);
    let db_path = root.join("data/veyra.db");
    let mut db = store::open(&db_path).unwrap();
    let project = Project {
        id: ProjectId::new(),
        path: root.clone(),
        name: "Relay proof".into(),
    };
    let thread = Thread {
        id: ThreadId::new(),
        project: project.id,
        title: format!("{} root", provider.label()),
        lineage: Lineage::Root,
        provider,
        config: ThreadConfig {
            model: None,
            effort: None,
            mode,
        },
        cwd: root.clone(),
        native,

        cli_version: None,
        hold: None,
        archived: false,
    };
    let id = thread.id;
    let mut changes = ChangeSet::default();
    changes.projects.insert(project.id, project);
    changes.threads.insert(id, thread);
    let tx = db.transaction().unwrap();
    store::apply(&tx, &changes, 1).unwrap();
    tx.commit().unwrap();
    drop(db);
    let engine = Engine::start_with_env(
        runtime.handle(),
        DataDir::at(root.join("data")).unwrap(),
        env,
    )
    .unwrap();
    let mut events = engine.subscribe();
    assert!(matches!(
        runtime
            .block_on(engine.send(Command::Send {
                thread: id,
                text: "Call list_providers.".into()
            }))
            .unwrap(),
        Ok(Ack::Turn(_))
    ));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                while events.recv().await.is_some() {
                    let db = store::open(&db_path).unwrap();
                    let p = store::load(&db).unwrap();
                    if p.turns_for(id)
                        .any(|turn| matches!(turn.state, TurnState::Ended { .. }))
                        && db
                            .query_row(
                                "select count(*) from threads where live_pgid is not null",
                                [],
                                |row| row.get::<_, i64>(0),
                            )
                            .unwrap()
                            == 0
                    {
                        return;
                    }
                }
                panic!("actor channel closed");
            })
            .await
            .unwrap();
        })
    }));
    runtime.block_on(engine.shutdown_wait());
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
    let projection = store::load(&store::open(&db_path).unwrap()).unwrap();
    assert!(
        projection.turns.values().all(|t| matches!(
            t.state,
            TurnState::Ended {
                outcome: TurnOutcome::Completed,
                ..
            }
        )),
        "root session did not complete: {:?}",
        projection
            .turns
            .values()
            .map(|t| &t.state)
            .collect::<Vec<_>>()
    );
    if provider == ProviderKind::Codex {
        let relay_pid: u32 = fs::read_to_string(root.join("relay.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            !platform::group_exists(relay_pid).unwrap(),
            "Codex relay survived session exit"
        );
    }
    assert!(
        engine.processes().is_empty(),
        "provider groups survived Exited"
    );
    let capture = fs::read_to_string(root.join("rec/t1-s1.jsonl")).unwrap();
    if matches!(provider, ProviderKind::Codex | ProviderKind::Grok) {
        assert!(capture.contains("<id:token>"));
        let run = root.join("data/run");
        assert!(
            !run.exists() || fs::read_dir(run).unwrap().next().is_none(),
            "Codex created a token-bearing config entry"
        );
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
        == if provider != ProviderKind::Claude {
            "list_providers"
        } else {
            "start_thread"
        })));
    for entry in entries {
        println!("relay-log {}", serde_json::to_string(&entry).unwrap());
    }
    store::load(&store::open(&db_path).unwrap()).unwrap()
}

#[cfg(debug_assertions)]
#[test]
fn codex_root_reaches_real_relay() {
    let p = fake_family(
        ProviderKind::Codex,
        PermissionMode::AskFirst,
        &[("root-tool-call", "t1-s1.jsonl")],
        None,
    );
    assert_eq!(p.turns.len(), 1);
    assert!(p.turns.values().all(|t| matches!(
        t.state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    )));
    assert!(p.items.values().any(|item|matches!(&item.body,ItemBody::Tool(tool) if tool.result.as_ref(
        ).and_then(|r|r.output_tail.as_ref()).is_some_and(|text|text.contains("codex") && text.contains("remain\
            ing_fan_out")))));
    println!(
        "Codex root completed one turn through the production relay, socket, peer check and list_providers; rec\
            order uses <id:token>; no config file or process group remains."
    );
}
#[test]
fn grok_root_reaches_real_relay() {
    let p = fake_family(
        ProviderKind::Grok,
        PermissionMode::AskFirst,
        &[("root-tool-call", "t1-s1.jsonl")],
        None,
    );
    assert_eq!(p.turns.len(), 1);
    let text: String = p
        .items
        .values()
        .filter_map(|i| match &i.body {
            ItemBody::AssistantText { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let result: serde_json::Value =
        serde_json::from_str(text.strip_prefix("Checking providers").unwrap()).unwrap();
    assert_eq!(result["providers"][0]["provider"], "grok");
    assert_eq!(result["mode_ceiling"], "ask_first");
    assert_eq!(result["remaining_fan_out"], 4);
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
        None,
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
fn codex_refused_resume_relay_does_not_spend_fallback_credential() {
    let p = fake_family(
        ProviderKind::Codex,
        PermissionMode::AskFirst,
        &[("resume-relay-fallback", "t1-s1.jsonl")],
        Some(NativeSessionId("missing-native".into())),
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
fn codex_refused_resume_relays_do_not_spend_retry_credentials() {
    let p = fake_family(
        ProviderKind::Codex,
        PermissionMode::AskFirst,
        &[("resume-relay-retry", "t1-s1.jsonl")],
        Some(NativeSessionId("missing-native".into())),
    );
    assert!(p.turns.values().all(|t| matches!(
        t.state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    )));
}
