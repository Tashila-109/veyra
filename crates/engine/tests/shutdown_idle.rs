#![cfg(debug_assertions)]
use serde_json::{Value, json};
use std::{fs, path::PathBuf, time::Duration};
use tokio::sync::mpsc;
use veyra_engine::{
    actor::EngineMsg,
    domain::*,
    platform,
    provider::{env::ChildEnv, *},
};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[tokio::test]
async fn requested_shutdown_of_idle_provider_is_not_a_crash() {
    for provider in ProviderKind::ALL {
        for close_channel in [false, true] {
            let root =
                std::env::temp_dir().join(format!("veyra-grok-shutdown-{}", ThreadId::new()));
            fs::create_dir(&root).unwrap();
            let _scratch = Scratch(root.clone());
            let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(provider.binary())
                .join("ok-reply.jsonl");
            let mut records = fs::read_to_string(source)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(records.pop().unwrap()["type"], "exit");
            records.push(json!({"type":"spin","ms":60000}));
            fs::write(
                root.join("t1-s1.jsonl"),
                records
                    .iter()
                    .map(|r| r.to_string() + "\n")
                    .collect::<String>(),
            )
            .unwrap();
            let mut env = ChildEnv::isolated()
                .with_binary(provider, env!("CARGO_BIN_EXE_veyra-fake-cli").into());
            env.fixtures = Some(root.clone());
            let (out, mut events) = mpsc::channel(32);
            let handle = Spawner::Cli(Box::new(env))
                .spawn(
                    provider,
                    SessionSpec {
                        thread: ThreadId::new(),
                        cwd: root.canonicalize().unwrap(),
                        config: ThreadConfig {
                            model: (provider == ProviderKind::Codex).then(|| "veyra-model".into()),
                            effort: (provider == ProviderKind::Codex).then(|| "medium".into()),
                            mode: PermissionMode::AskFirst,
                        },
                        native: None,
                        import: false,
                        root: false,
                    },
                    out,
                    1,
                )
                .unwrap();
            let mut handle = Some(handle);
            let mut pid = None;
            let mut ended = false;
            tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
                    match event {
                        ProviderEvent::Session(SessionEvent::Ready { pid: owned, .. }) => {
                            pid = Some(owned);
                            handle
                                .as_ref()
                                .unwrap()
                                .control
                                .send(SessionControl::StartTurn {
                                    turn: TurnId::new(),
                                    text: "reply with the word ok".into(),
                                })
                                .unwrap();
                        }
                        ProviderEvent::Turn {
                            event:
                                TurnEvent::TurnEnded {
                                    outcome: TurnOutcome::Completed,
                                    ..
                                },
                            ..
                        } => {
                            ended = true;
                            let handle = handle.take().unwrap();
                            if !close_channel {
                                handle.control.send(SessionControl::Shutdown).unwrap();
                            }
                        }
                        ProviderEvent::Session(SessionEvent::Exited { crash }) => {
                            assert!(ended);
                            assert!(
                                crash.is_none(),
                                "{provider:?} shutdown was a crash: {crash:?}"
                            );
                            return;
                        }
                        _ => {}
                    }
                }
                panic!("provider did not exit");
            })
            .await
            .unwrap();
            assert!(!platform::group_exists(pid.unwrap()).unwrap());
        }
    }
}
