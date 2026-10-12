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
    if rerun_without_session_markers("live_claude_ok_reply") {
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
                    root: false,
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

fn rerun_without_session_markers(test: &str) -> bool {
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
        command.args(["--ignored", "--exact", test]);
        for marker in markers {
            command.env_remove(marker);
        }
        assert!(command.status().unwrap().success());
        return true;
    }
    false
}

#[test]
#[ignore = "requires the human's consent for live Claude approval turns"]
fn live_claude_approval_round_trip() {
    use veyra_engine::{Engine, data_dir::DataDir};
    if rerun_without_session_markers("live_claude_approval_round_trip") {
        return;
    }
    for name in [
        "VEYRA_CLAUDE_BIN",
        "VEYRA_FAKE_FIXTURES",
        "VEYRA_FAKE_FIXTURE",
    ] {
        assert!(
            std::env::var_os(name).is_none(),
            "live approval test requires the real Claude binary"
        );
    }
    let root =
        std::path::Path::new("/tmp").join(format!("veyra-live-approval-{}", ThreadId::new()));
    let cwd = root.join("project");
    fs::create_dir_all(&cwd).unwrap();
    let runtime = Runtime::new().unwrap();
    let engine = Engine::start_with_processes(
        runtime.handle(),
        DataDir::at(root.join("data")).unwrap(),
        vec![],
    )
    .unwrap();
    let run = async {
        tokio::time::timeout(Duration::from_secs(180), approval_round_trip(&engine, &cwd))
            .await
            .unwrap();
    };
    let check = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(run)));
    runtime.block_on(engine.shutdown_wait());
    drop(engine);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
    if let Err(error) = check {
        std::panic::resume_unwind(error);
    }
}

async fn approval_round_trip(engine: &veyra_engine::Engine, cwd: &std::path::Path) {
    use veyra_engine::{
        commands::{Ack, Command},
        events::{ChangeEvent, UiEvent},
    };
    let mut events = engine.subscribe();
    let Ack::Project(project) = engine
        .send(Command::AddProject {
            path: cwd.to_owned(),
        })
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("missing project")
    };
    let prompt = "Use Bash to run exactly `touch veyra-check.txt` in the current project folder. \
        Do not use another tool or command. If denied, do not retry; end this turn.";
    let Ack::Thread(thread) = engine
        .send(Command::CreateThread {
            project,
            provider: ProviderKind::Claude,
            config: ThreadConfig {
                model: None,
                effort: None,
                mode: PermissionMode::AskFirst,
            },
            text: prompt.into(),
        })
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("missing thread")
    };
    for decision in [Decision::Deny, Decision::AllowOnce] {
        if decision == Decision::AllowOnce {
            assert!(matches!(
                engine
                    .send(Command::Send {
                        thread,
                        text: prompt.into()
                    })
                    .await
                    .unwrap()
                    .unwrap(),
                Ack::Turn(_)
            ));
        }
        let mut request = None;
        let mut needs_you = false;
        while request.is_none() || !needs_you {
            match events.recv().await.expect("engine event stream ended") {
                UiEvent::Change {
                    event: ChangeEvent::RequestOpened(row),
                    ..
                } if row.thread == thread => {
                    let RequestKind::Approval { action, .. } = &row.kind else {
                        panic!("Claude must dock an approval")
                    };
                    assert_eq!(action.tool, "Bash");
                    let ApprovalTarget::Command { command, .. } = &action.target else {
                        panic!("command approval")
                    };
                    assert_eq!(command, "touch veyra-check.txt");
                    assert_eq!(row.state, RequestState::Pending);
                    request = Some(row);
                }
                UiEvent::Change {
                    event: ChangeEvent::ThreadStatusChanged { thread: id, status },
                    ..
                } if id == thread => {
                    needs_you = status == Status::NeedsYou;
                }
                UiEvent::Change {
                    event: ChangeEvent::TurnEnded(row),
                    ..
                } if row.thread == thread => {
                    panic!(
                        "Claude ended before docking the exact command: {:?}",
                        row.state
                    );
                }
                _ => {}
            }
        }
        assert!(
            !cwd.join("veyra-check.txt").exists(),
            "command ran before approval"
        );
        let request = request.unwrap();
        assert_eq!(
            engine
                .send(Command::Respond {
                    request: request.id,
                    decision: decision.clone()
                })
                .await
                .unwrap()
                .unwrap(),
            Ack::Applied
        );
        let mut answered = false;
        loop {
            match events.recv().await.expect("engine event stream ended") {
                UiEvent::Change {
                    event: ChangeEvent::RequestClosed(row),
                    ..
                } if row.id == request.id => {
                    assert_eq!(row.state, RequestState::Answered(decision.clone()));
                    answered = true;
                }
                UiEvent::Change {
                    event: ChangeEvent::TurnEnded(row),
                    ..
                } if row.id == request.turn => {
                    assert!(matches!(
                        row.state,
                        TurnState::Ended {
                            outcome: TurnOutcome::Completed,
                            ..
                        }
                    ));
                    assert!(answered);
                    break;
                }
                UiEvent::Change {
                    event: ChangeEvent::RequestOpened(_),
                    ..
                } => {
                    panic!("Claude requested another tool after the approval decision");
                }
                _ => {}
            }
        }
        assert_eq!(
            cwd.join("veyra-check.txt").is_file(),
            decision == Decision::AllowOnce
        );
    }
}
