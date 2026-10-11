use std::{fs, time::Duration};
use tokio::{runtime::Runtime, sync::mpsc};
use veyra_engine::{
    actor::EngineMsg,
    domain::*,
    provider::{env::ChildEnv, *},
};

#[test]
#[ignore = "requires the human's consent for a live Claude turn"]
fn live_claude_ok_reply() {
    let markers: Vec<_> = std::env::vars_os()
        .filter_map(|(key, _)| {
            let name = key.to_string_lossy();
            ((name.starts_with("CLAUDE_CODE_") && !name.starts_with("CLAUDE_CODE_USE_"))
                || [
                    "CLAUDE_PID",
                    "CLAUDE_EFFORT",
                    "CLAUDECODE",
                    "CLAUDE_AGENT_SDK_VERSION",
                ]
                .contains(&name.as_ref()))
            .then_some(key)
        })
        .collect();
    if !markers.is_empty() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--ignored", "--exact", "live_claude_ok_reply"]);
        for marker in markers {
            command.env_remove(marker);
        }
        assert!(command.status().unwrap().success());
        return;
    }
    let cwd = std::env::temp_dir().join(format!("veyra-live-{}", ThreadId::new()));
    fs::create_dir(&cwd).unwrap();
    let runtime = Runtime::new().unwrap();
    runtime.block_on(async {
        let (out, mut events) = mpsc::channel(256);
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let handle = Spawner::Cli(ChildEnv::load())
            .spawn(
                ProviderKind::Claude,
                SessionSpec {
                    thread,
                    cwd: cwd.clone(),
                    config: ThreadConfig {
                        model: None,
                        effort: None,
                        mode: PermissionMode::FullAccess,
                    },
                    native: None,
                    import: false,
                },
                out,
                1,
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut sent = false;
            let mut completed = false;
            let mut text = String::new();
            while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
                match event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) if !sent => {
                        handle
                            .control
                            .send(SessionControl::StartTurn {
                                turn,
                                text: "reply with the word ok".into(),
                            })
                            .unwrap();
                        sent = true;
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::TextDelta { text: delta, .. },
                        ..
                    } => text.push_str(&delta),
                    ProviderEvent::Turn {
                        event: TurnEvent::TurnEnded { outcome, .. },
                        ..
                    } => {
                        assert_eq!(outcome, TurnOutcome::Completed);
                        assert_eq!(text.trim().to_lowercase(), "ok");
                        completed = true;
                        handle.control.send(SessionControl::Shutdown).unwrap();
                    }
                    ProviderEvent::Session(SessionEvent::Exited { crash }) => {
                        assert_eq!(crash, None);
                        break;
                    }
                    _ => {}
                }
            }
            assert!(completed, "Claude exited before completing the live turn");
        })
        .await
        .unwrap();
    });
    drop(runtime);
    fs::remove_dir_all(cwd).unwrap();
}
