#![cfg(debug_assertions)]
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{runtime::Runtime, sync::mpsc};
use veyra_engine::{
    actor::EngineMsg,
    domain::*,
    provider::{env::ChildEnv, *},
};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/grok")
        .join(format!("{name}.jsonl"))
}
fn environment(name: &str) -> (Scratch, ChildEnv) {
    let root = Path::new("/tmp").join(format!("veyra-m6s1-{}", ThreadId::new()));
    fs::create_dir(&root).unwrap();
    fs::copy(fixture(name), root.join("t1-s1.jsonl")).unwrap();
    fs::copy(fixture("probe-grok"), root.join("probe-grok.jsonl")).unwrap();
    let mut env = ChildEnv::isolated().with_binary(
        ProviderKind::Grok,
        env!("CARGO_BIN_EXE_veyra-fake-cli").into(),
    );
    env.fixtures = Some(root.clone());
    (Scratch(root), env)
}
#[derive(Default)]
struct Inputs {
    answers: std::collections::VecDeque<Decision>,
    stop_request: bool,
    stop_accepted: bool,
    multi: bool,
    resume: bool,
}
fn replay(name: &str, mut inputs: Inputs) -> (Vec<ProviderEvent>, bool) {
    let (scratch, env) = environment(name);
    Runtime::new().unwrap().block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let turn = TurnId::new();
        let handle = Spawner::Cli(Box::new(env))
            .spawn(
                ProviderKind::Grok,
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: scratch.0.canonicalize().unwrap(),
                    config: config(),
                    native: inputs
                        .resume
                        .then(|| NativeSessionId("retained-session".into())),
                    import: false,
                    root: false,
                },
                out,
                1,
            )
            .unwrap();
        let events = tokio::time::timeout(Duration::from_secs(10), async {
            let mut events = vec![];
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                match &event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) => {
                        handle
                            .control
                            .send(SessionControl::StartTurn {
                                turn,
                                text: "reply with the word ok".into(),
                            })
                            .unwrap();
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::Accepted,
                        ..
                    } if inputs.stop_accepted => {
                        inputs.stop_accepted = false;
                        handle
                            .control
                            .send(SessionControl::Interrupt {
                                turn,
                                reason: StopReason::User,
                            })
                            .unwrap();
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::RequestOpened { native, .. },
                        ..
                    } => {
                        let control = if inputs.stop_request {
                            SessionControl::Interrupt {
                                turn,
                                reason: StopReason::User,
                            }
                        } else {
                            SessionControl::Answer {
                                native: native.clone(),
                                reply: inputs.answers.pop_front().expect("missing test answer"),
                            }
                        };
                        handle.control.send(control).unwrap();
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::TurnEnded { .. },
                        ..
                    } if inputs.multi => {
                        inputs.multi = false;
                        handle
                            .control
                            .send(SessionControl::StartTurn {
                                turn: TurnId::new(),
                                text: "second prompt".into(),
                            })
                            .unwrap();
                    }
                    _ => {}
                }
                let exited = matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                events.push(event);
                if exited {
                    break;
                }
            }
            assert!(inputs.answers.is_empty());
            assert!(
                events.iter().any(|e| matches!(
                    e,
                    ProviderEvent::Session(SessionEvent::Exited { crash: None })
                )),
                "{events:?}"
            );
            events
        })
        .await
        .unwrap();
        (events, scratch.0.join("veyra-check.txt").exists())
    })
}
fn config() -> ThreadConfig {
    ThreadConfig {
        model: None,
        effort: None,
        mode: PermissionMode::AskFirst,
    }
}
fn text(events: &[ProviderEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::TextDelta { text, .. },
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}
fn outcomes(events: &[ProviderEvent]) -> Vec<TurnOutcome> {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::TurnEnded { outcome, .. },
                ..
            } => Some(outcome.clone()),
            _ => None,
        })
        .collect()
}
fn standard(name: &str) -> Vec<ProviderEvent> {
    replay(name, Inputs::default()).0
}
#[test]
fn grok_ok_reply() {
    let e = standard("ok-reply");
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn grok_turn_ends_once() {
    let e = standard("ok-reply");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
    assert_eq!(
        e.iter()
            .filter(|e| matches!(
                e,
                ProviderEvent::Turn {
                    event: TurnEvent::Accepted,
                    ..
                }
            ))
            .count(),
        1
    );
}
#[test]
fn grok_multi_turn() {
    let e = replay(
        "multi-turn",
        Inputs {
            multi: true,
            ..Default::default()
        },
    )
    .0;
    assert_eq!(text(&e), "firstsecond");
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Completed, TurnOutcome::Completed]
    );
}
#[test]
fn grok_approval_allow() {
    let (e, exists) = replay(
        "approval-allow",
        Inputs {
            answers: [Decision::AllowOnce].into(),
            ..Default::default()
        },
    );
    assert!(exists);
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn grok_approval_deny() {
    let (e, exists) = replay(
        "approval-deny",
        Inputs {
            answers: [Decision::Deny].into(),
            ..Default::default()
        },
    );
    assert!(!exists);
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn grok_never_sends_allow_always() {
    for (name, answer) in [
        ("approval-allow", Decision::AllowOnce),
        ("approval-allow", Decision::AllowInThread),
        ("approval-deny", Decision::Deny),
    ] {
        assert_eq!(
            outcomes(
                &replay(
                    name,
                    Inputs {
                        answers: [answer].into(),
                        ..Default::default()
                    }
                )
                .0
            ),
            [TurnOutcome::Completed]
        );
    }
}
#[test]
fn grok_turn_interrupt() {
    let e = replay(
        "turn-interrupt",
        Inputs {
            stop_accepted: true,
            ..Default::default()
        },
    )
    .0;
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
}
#[test]
fn grok_prompt_error() {
    let e = standard("prompt-error");
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Failed {
            message: "The Veyra test prompt was refused".into()
        }]
    );
    assert!(!e.iter().any(|e| matches!(
        e,
        ProviderEvent::Turn {
            event: TurnEvent::Accepted,
            ..
        }
    )));
}
#[test]
fn grok_foreign_prompt_id_is_cli_work() {
    let e = standard("foreign-prompt-id");
    assert_eq!(text(&e), "okbackground reply");
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Completed, TurnOutcome::Completed]
    );
    assert!(e.iter().any(|e| matches!(
        e,
        ProviderEvent::Session(SessionEvent::ProviderTurnStarted { .. })
    )));
    assert!(e.iter().any(|e|matches!(e,ProviderEvent::Turn {turn:None,event:TurnEvent::TextDelta {text,..}} if text=="background reply")));
}
#[test]
fn grok_auth_retries_once() {
    let e = standard("auth-retry");
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn grok_resume_uses_session_resume() {
    let e = replay(
        "resume-advertised",
        Inputs {
            resume: true,
            ..Default::default()
        },
    )
    .0;
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn grok_stop_cancels_pending_permission() {
    let e = replay(
        "stop-pending-permission",
        Inputs {
            stop_request: true,
            ..Default::default()
        },
    )
    .0;
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
}
#[test]
fn grok_stop_cancels_pending_question() {
    let events = replay(
        "stop-pending-question",
        Inputs {
            stop_request: true,
            ..Default::default()
        },
    )
    .0;
    assert_eq!(
        outcomes(&events),
        [TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
    engine_replay(
        "stop-pending-question",
        |engine, thread, events, runtime| {
            runtime.block_on(async {
                use veyra_engine::{
                    commands::Command,
                    events::{ChangeEvent, UiEvent},
                };
                let mut opened = None;
                while let Some(event) = events.recv().await {
                    match event {
                        UiEvent::Change {
                            event: ChangeEvent::RequestOpened(r),
                            ..
                        } => {
                            opened = Some(r.id);
                            engine
                                .send(Command::StopThread { thread })
                                .await
                                .unwrap()
                                .unwrap();
                        }
                        UiEvent::Change {
                            event: ChangeEvent::RequestClosed(r),
                            ..
                        } if Some(r.id) == opened => {
                            assert!(matches!(r.state, RequestState::Expired { .. }));
                            return;
                        }
                        _ => {}
                    }
                }
                panic!("question never expired");
            })
        },
    );
}
#[test]
fn grok_always_only_disables_allow_once() {
    let e = replay(
        "approval-always-only",
        Inputs {
            answers: [Decision::Deny].into(),
            ..Default::default()
        },
    )
    .0;
    let kind = e
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::RequestOpened { kind, .. },
                ..
            } => Some(kind),
            _ => None,
        })
        .unwrap();
    assert!(kind.accepts(&Decision::AllowOnce).is_none());
    assert!(kind.accepts(&Decision::AllowInThread).is_none());
    assert!(kind.accepts(&Decision::Deny).is_some());
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[tokio::test]
async fn grok_no_first_frame_fails_at_60s() {
    let (scratch, env) = environment("no-first-frame");
    let (out, mut events) = mpsc::channel(32);
    let turn = TurnId::new();
    let handle = Spawner::Cli(Box::new(env))
        .spawn(
            ProviderKind::Grok,
            SessionSpec {
                thread: ThreadId::new(),
                cwd: scratch.0.canonicalize().unwrap(),
                config: config(),
                native: None,
                import: false,
                root: false,
            },
            out,
            1,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Some(EngineMsg::Provider { event, .. }) = events.recv().await else {
                panic!("no ready")
            };
            if matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. })) {
                break;
            }
        }
    })
    .await
    .unwrap();
    handle
        .control
        .send(SessionControl::StartTurn {
            turn,
            text: "reply with the word ok".into(),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !scratch.0.join("prompt-received").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(59)).await;
    tokio::task::yield_now().await;
    assert!(events.try_recv().is_err());
    tokio::time::advance(Duration::from_secs(1)).await;
    let (mut failed, mut exited) = (false, false);
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
            match event {
                ProviderEvent::Turn {
                    turn: Some(id),
                    event:
                        TurnEvent::TurnEnded {
                            outcome: TurnOutcome::Failed { message },
                            park: true,
                        },
                } => {
                    assert_eq!(id, turn);
                    assert!(message.contains("no frame within 60 seconds"));
                    failed = true;
                }
                ProviderEvent::Session(SessionEvent::Exited { .. }) => {
                    exited = true;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(failed && exited);
}
fn engine_replay(
    name: &str,
    check: impl FnOnce(
        &veyra_engine::Engine,
        ThreadId,
        &mut mpsc::UnboundedReceiver<veyra_engine::events::UiEvent>,
        &Runtime,
    ),
) {
    use veyra_engine::{
        Engine,
        commands::{Ack, Command},
        data_dir::DataDir,
    };
    let (scratch, env) = environment(name);
    let runtime = Runtime::new().unwrap();
    let engine = Engine::start_with_env(
        runtime.handle(),
        DataDir::at(scratch.0.join("data")).unwrap(),
        env,
    )
    .unwrap();
    let mut events = engine.subscribe();
    let thread = runtime.block_on(async {
        let Ack::Project(project) = engine
            .send(Command::AddProject {
                path: scratch.0.clone(),
            })
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("no project")
        };
        let Ack::Thread(thread) = engine
            .send(Command::CreateThread {
                project,
                provider: ProviderKind::Grok,
                config: config(),
                text: "reply with the word ok".into(),
            })
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("no thread")
        };
        thread
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check(&engine, thread, &mut events, &runtime)
    }));
    runtime.block_on(engine.shutdown_wait());
    drop(engine);
    drop(runtime);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
#[test]
fn grok_question_answered() {
    engine_replay("question", |engine, thread, events, runtime| {
        runtime.block_on(async {
            use veyra_engine::{
                commands::Command,
                events::{ChangeEvent, UiEvent},
            };
            tokio::time::timeout(Duration::from_secs(10), async {
                let (mut opened, mut answered) = (0, 0);
                while let Some(event) = events.recv().await {
                    match event {
                        UiEvent::Change {
                            event: ChangeEvent::RequestOpened(r),
                            ..
                        } => {
                            let RequestKind::Question { questions } = &r.kind else {
                                panic!("not a question")
                            };
                            let (question, answer) = if opened == 0 {
                                ("Which check should run first?", "Tests")
                            } else {
                                ("Which report should be saved?", "Summary")
                            };
                            assert_eq!(questions[0].text, question);
                            assert_eq!(r.state, RequestState::Pending);
                            opened += 1;
                            engine
                                .send(Command::Respond {
                                    request: r.id,
                                    decision: Decision::Answers(vec![Answer {
                                        text: answer.into(),
                                    }]),
                                })
                                .await
                                .unwrap()
                                .unwrap();
                        }
                        UiEvent::Change {
                            event: ChangeEvent::RequestClosed(r),
                            ..
                        } => {
                            assert!(matches!(
                                r.state,
                                RequestState::Answered(Decision::Answers(_))
                            ));
                            answered += 1;
                        }
                        UiEvent::Change {
                            event: ChangeEvent::TurnEnded(t),
                            ..
                        } => {
                            assert!(matches!(
                                t.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Completed,
                                    ..
                                }
                            ));
                            assert_eq!((opened, answered), (2, 2));
                            let snapshot = engine.snapshot(thread).await.unwrap();
                            for expected in [
                                "Which check should run first?: Tests",
                                "Which report should be saved?: Summary",
                            ] {
                                assert!(snapshot.items.iter().any(
                                    |i| matches!(&i.body,ItemBody::UserText(t) if t==expected)
                                ));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                panic!("question turn did not finish");
            })
            .await
            .unwrap();
        })
    });
}
#[test]
fn grok_background_work_blocks_replacement() {
    engine_replay("background-work", |engine, thread, events, runtime| {
        runtime.block_on(async {
            use veyra_engine::{
                commands::{Command, CommandError},
                events::{ChangeEvent, UiEvent},
            };
            tokio::time::timeout(Duration::from_secs(10), async {
                let (mut foreground, mut provider, mut checked) = (false, false, false);
                while let Some(event) = events.recv().await {
                    match event {
                        UiEvent::Change {
                            event: ChangeEvent::TurnEnded(t),
                            ..
                        } if !foreground => {
                            assert!(matches!(
                                t.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Completed,
                                    ..
                                }
                            ));
                            foreground = true;
                            assert!(matches!(
                                engine
                                    .send(Command::Configure {
                                        thread,
                                        config: config()
                                    })
                                    .await
                                    .unwrap(),
                                Err(CommandError::Busy(WaitReason::BackgroundWork))
                            ));
                            assert!(matches!(
                                engine
                                    .send(Command::SwitchView {
                                        thread,
                                        to: View::Terminal
                                    })
                                    .await
                                    .unwrap(),
                                Err(CommandError::Busy(WaitReason::BackgroundWork))
                            ));
                            checked = true;
                        }
                        UiEvent::Change {
                            event: ChangeEvent::TurnStarted(t),
                            ..
                        } if matches!(t.origin, TurnOrigin::Provider) => {
                            provider = true;
                        }
                        UiEvent::Change {
                            event: ChangeEvent::TurnEnded(t),
                            ..
                        } if provider => {
                            assert!(matches!(
                                t.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Completed,
                                    ..
                                }
                            ));
                            assert!(checked);
                            assert!(!engine.window_threads()[&thread].busy);
                            return;
                        }
                        _ => {}
                    }
                }
                panic!("background turn did not finish");
            })
            .await
            .unwrap();
        })
    });
}
#[test]
fn grok_signed_out_names_login() {
    let (scratch, env) = environment("signed-out");
    Runtime::new().unwrap().block_on(async {
        let (out, mut events) = mpsc::channel(32);
        let _handle = Spawner::Cli(Box::new(env))
            .spawn(
                ProviderKind::Grok,
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: scratch.0.canonicalize().unwrap(),
                    config: config(),
                    native: None,
                    import: false,
                    root: false,
                },
                out,
                1,
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
                match event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) => {
                        panic!("signed-out session became ready")
                    }
                    ProviderEvent::Session(SessionEvent::Exited {
                        crash: Some(message),
                    }) => {
                        assert!(message.contains("Sign in with `grok login`"));
                        return;
                    }
                    _ => {}
                }
            }
            panic!("no sign-in failure")
        })
        .await
        .unwrap();
    });
}
#[test]
fn grok_stream_2000() {
    let e = standard("stream-2000");
    assert_eq!(text(&e), "x".repeat(2000));
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}

#[derive(Clone)]
struct Logs(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
impl tracing::Subscriber for Logs {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push_str(&format!("{}={value:?} ", field.name()));
            }
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        if fields.0.contains("MCP connection") || fields.0.contains("MCP tools/call") {
            self.0.lock().unwrap().push(fields.0);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
#[test]
#[ignore = "requires an explicitly built production relay in VEYRA_M6_RELAY; uses only the fake Grok CLI"]
fn grok_root_real_relay() {
    use veyra_engine::{
        Engine,
        commands::{Ack, Command},
        data_dir::DataDir,
        events::{ChangeEvent, UiEvent},
    };
    let logs = Logs(Default::default());
    tracing::subscriber::set_global_default(logs.clone()).unwrap();
    let (scratch, mut env) = environment("root-tool-call");
    env.relay_command = Some(
        fs::canonicalize(std::env::var_os("VEYRA_M6_RELAY").expect("production relay path"))
            .unwrap(),
    );
    let runtime = Runtime::new().unwrap();
    let engine = Engine::start_with_env(
        runtime.handle(),
        DataDir::at(scratch.0.join("data")).unwrap(),
        env,
    )
    .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let mut probes = Probes::default();
            let probe = probes
                .check(
                    &tokio::runtime::Handle::current(),
                    ProviderKind::Grok,
                    engine.child_env(),
                    scratch.0.clone(),
                )
                .await
                .unwrap();
            assert_eq!(probe.readiness, Readiness::Ready);
            let mut events = engine.subscribe();
            let Ack::Project(project) = engine
                .send(Command::AddProject {
                    path: scratch.0.clone(),
                })
                .await
                .unwrap()
                .unwrap()
            else {
                panic!("no project")
            };
            let Ack::Thread(thread) = engine
                .send(Command::CreateThread {
                    project,
                    provider: ProviderKind::Grok,
                    config: config(),
                    text: "reply with the word ok".into(),
                })
                .await
                .unwrap()
                .unwrap()
            else {
                panic!("no thread")
            };
            tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(event) = events.recv().await {
                    if let UiEvent::Change {
                        event: ChangeEvent::TurnEnded(t),
                        ..
                    } = event
                    {
                        assert!(
                            matches!(
                                t.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Completed,
                                    ..
                                }
                            ),
                            "{:?}",
                            t.state
                        );
                        let snapshot = engine.snapshot(thread).await.unwrap();
                        let result = snapshot
                            .items
                            .iter()
                            .find_map(|i| match &i.body {
                                ItemBody::AssistantText { text, .. } => {
                                    text.strip_prefix("Checking providers")
                                }
                                _ => None,
                            })
                            .expect("relay result in the assistant reply");
                        let result: serde_json::Value = serde_json::from_str(result).unwrap();
                        assert_eq!(result["providers"][0]["provider"], "grok");
                        assert_eq!(result["mode_ceiling"], "ask_first");
                        assert_eq!(result["remaining_fan_out"], 4);
                        eprintln!("Grok root relay returned {result}");
                        return;
                    }
                }
                panic!("root turn never ended");
            })
            .await
            .unwrap();
        })
    }));
    runtime.block_on(engine.shutdown_wait());
    drop(engine);
    drop(runtime);
    let lines = logs.0.lock().unwrap().join("\n");
    assert!(
        lines.contains("MCP connection accepted"),
        "no accepted relay connection"
    );
    assert!(lines.contains("list_providers"), "no list_providers call");
    eprintln!("{lines}");
    if let Some(root) = std::env::var_os("VEYRA_M6_RELAY_EVIDENCE") {
        let root = PathBuf::from(root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("mcp.log"), lines).unwrap();
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
