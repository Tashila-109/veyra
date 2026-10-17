#![cfg(debug_assertions)]
use std::time::Duration;
use tokio::{runtime::Runtime, sync::mpsc};
use veyra_engine::{
    actor::EngineMsg,
    domain::*,
    provider::{env::ChildEnv, *},
};
#[path = "common/replay.rs"]
mod harness;
use harness::{Inputs, Scratch};
fn environment(name: &str) -> (Scratch, ChildEnv) {
    harness::environment(ProviderKind::Grok, name)
}
fn replay(name: &str, inputs: Inputs) -> (Vec<ProviderEvent>, bool) {
    harness::replay(
        ProviderKind::Grok,
        name,
        PermissionMode::AskFirst,
        false,
        false,
        inputs,
    )
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
    let (e, exists) = replay("approval-allow", Inputs::answers([Decision::AllowOnce]));
    assert!(exists);
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn grok_approval_deny() {
    let (e, exists) = replay("approval-deny", Inputs::answers([Decision::Deny]));
    assert!(!exists);
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped after you denied a request".into()
        }]
    );
}
#[test]
fn grok_never_sends_allow_always() {
    for (name, answer) in [
        ("approval-allow", Decision::AllowOnce),
        ("approval-allow", Decision::AllowInThread),
        ("approval-deny", Decision::Deny),
    ] {
        let expected = if answer == Decision::Deny {
            TurnOutcome::Interrupted {
                reason: "Stopped after you denied a request".into(),
            }
        } else {
            TurnOutcome::Completed
        };
        assert_eq!(
            outcomes(&replay(name, Inputs::answers([answer])).0),
            [expected]
        );
    }
}
#[test]
fn grok_turn_interrupt() {
    let e = replay("turn-interrupt", Inputs::stop_accepted()).0;
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
    let e = harness::replay(
        ProviderKind::Grok,
        "resume-advertised",
        PermissionMode::AskFirst,
        false,
        true,
        Inputs::default(),
    )
    .0;
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
    assert!(e.iter().any(|event| matches!(event, ProviderEvent::Session(SessionEvent::Ready { native, cli_version, .. }) if native.0 == "retained-session" && cli_version.as_deref() == Some("1.0.60"))));
}
#[test]
fn grok_stop_cancels_pending_permission() {
    let e = replay("stop-pending-permission", Inputs::stop_request()).0;
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
}
#[test]
fn grok_stop_cancels_pending_question() {
    let events = replay("stop-pending-question", Inputs::stop_request()).0;
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
    let e = replay("approval-always-only", Inputs::answers([Decision::Deny])).0;
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
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped after you denied a request".into()
        }]
    );
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

                import: None,
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
    tokio::time::resume();
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
    harness::actor_replay(ProviderKind::Grok, name, check);
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
fn grok_signed_out_never_authenticates() {
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

                    import: None,
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

#[test]
fn grok_foreign_prompt_overlap_keeps_stop_target() {
    let (events, _) = replay("foreign-prompt-overlap", Inputs::stop_request());
    assert!(outcomes(&events).iter().any(|outcome| matches!(outcome, TurnOutcome::Interrupted { reason } if reason == "Stopped by you")));
}
#[test]
fn grok_isolated_auth_ignores_ambient_api_key() {
    if std::env::var_os("VEYRA_TEST_GROK_AUTH_CHILD").is_some() {
        let events = standard("isolated-auth");
        assert_eq!(outcomes(&events), [TurnOutcome::Completed]);
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "grok_isolated_auth_ignores_ambient_api_key",
            "--nocapture",
        ])
        .env("VEYRA_TEST_GROK_AUTH_CHILD", "1")
        .env("XAI_API_KEY", "isolated-test-presence")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}
#[test]
fn grok_foreign_permission_docks_in_veyra_turn() {
    harness::foreign_request(
        ProviderKind::Grok,
        "foreign-permission-dock",
        Decision::AllowOnce,
    );
}
#[test]
fn grok_foreign_question_docks_in_veyra_turn() {
    harness::foreign_request(
        ProviderKind::Grok,
        "foreign-question-dock",
        Decision::Answers(vec![Answer {
            text: "Tests".into(),
        }]),
    );
}
fn foreign_request_ends(name: &str, stop: bool) {
    engine_replay(name, |engine, thread, events, runtime| {
        runtime.block_on(async {
        use veyra_engine::{commands::Command, events::{ChangeEvent, UiEvent}};
        tokio::time::timeout(Duration::from_secs(5), async {
            let (mut opened,mut expired)=(None,false);
            while let Some(event)=events.recv().await {
                match event {
                    UiEvent::Change { event:ChangeEvent::RequestOpened(request),.. } => {
                        opened=Some(request.id);
                        if stop {engine.send(Command::StopThread { thread }).await.unwrap().unwrap();}
                    }
                    UiEvent::Change { event:ChangeEvent::RequestClosed(request),.. } => {
                        assert_eq!(Some(request.id),opened);
                        assert!(matches!(request.state,RequestState::Expired {..}));
                        expired=true;
                    }
                    UiEvent::Change { event:ChangeEvent::TurnEnded(turn),.. } if turn.origin!=TurnOrigin::Provider => {
                        assert!(opened.is_some() && expired,"foreign request was not persisted and expired");
                        assert!(matches!(turn.state,TurnState::Ended { outcome:TurnOutcome::Interrupted {..},..}) == stop);
                        let snapshot=engine.snapshot(thread).await.unwrap();
                        assert!(snapshot.items.iter().any(|item|matches!(&item.body,ItemBody::AssistantText {text,..} if text=="working")));
                        native_replies_checked(engine).await;
                        return;
                    }
                    _=>{}
                }
            }
            panic!("foreign request never expired");
        }).await.unwrap();
    })
    });
}
#[test]
fn grok_foreign_requests_expire_with_veyra_turn() {
    for name in [
        "foreign-permission-dock-expires",
        "foreign-question-dock-expires",
    ] {
        foreign_request_ends(name, false);
    }
}
#[test]
fn grok_foreign_requests_stop_with_veyra_turn() {
    for name in ["foreign-permission-dock-stop", "foreign-question-dock-stop"] {
        foreign_request_ends(name, true);
    }
}
#[test]
fn grok_foreign_open_work_is_announced_before_later_send() {
    engine_replay("foreign-open-send", |engine, thread, events, runtime| {
        runtime.block_on(async {
            use veyra_engine::{
                commands::Command,
                events::{ChangeEvent, UiEvent},
            };
            tokio::time::timeout(Duration::from_secs(5), async {
                let (mut provider, mut user_ends) = (None, 0);
                while let Some(event) = events.recv().await {
                    match event {
                        UiEvent::Change {
                            event: ChangeEvent::TurnStarted(turn),
                            ..
                        } if turn.origin == TurnOrigin::Provider => {
                            assert!(engine.window_threads()[&thread].busy);
                            provider = Some(turn.id);
                            engine
                                .send(Command::Send {
                                    thread,
                                    text: "second".into(),
                                })
                                .await
                                .unwrap()
                                .unwrap();
                        }
                        UiEvent::Change {
                            event: ChangeEvent::TurnEnded(turn),
                            ..
                        } => {
                            assert!(matches!(
                                turn.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Completed,
                                    ..
                                }
                            ));
                            if turn.origin == TurnOrigin::Provider {
                                assert_eq!(Some(turn.id), provider);
                            } else {
                                user_ends += 1;
                                if user_ends == 2 {
                                    assert!(provider.is_some());
                                    return;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                panic!("foreign work blocked a later send");
            })
            .await
            .unwrap();
        })
    });
}
#[test]
fn grok_overlapping_provider_prompts_share_one_running_row() {
    engine_replay(
        "foreign-provider-overlap",
        |engine, thread, events, runtime| {
            runtime.block_on(async {
                use veyra_engine::events::{ChangeEvent, UiEvent};
                tokio::time::timeout(Duration::from_secs(5), async {
                    let mut provider = None;
                    while let Some(event) = events.recv().await {
                        match event {
                            UiEvent::Change {
                                event: ChangeEvent::TurnStarted(turn),
                                ..
                            } if turn.origin == TurnOrigin::Provider => {
                                assert!(
                                    provider.is_none(),
                                    "overlapping prompts split the Provider row"
                                );
                                provider = Some(turn.id);
                            }
                            UiEvent::Change {
                                event: ChangeEvent::TurnEnded(turn),
                                ..
                            } if turn.origin == TurnOrigin::Provider => {
                                assert_eq!(Some(turn.id), provider);
                                let snapshot = engine.snapshot(thread).await.unwrap();
                                let text: String = snapshot
                                    .items
                                    .iter()
                                    .filter(|item| item.turn == turn.id)
                                    .filter_map(|item| match &item.body {
                                        ItemBody::AssistantText { text, .. } => Some(text.as_str()),
                                        _ => None,
                                    })
                                    .collect();
                                assert_eq!(text, "firststill workingsecond");
                                return;
                            }
                            _ => {}
                        }
                    }
                    panic!("Provider work never ended");
                })
                .await
                .unwrap();
            })
        },
    );
}
#[test]
fn grok_malformed_missing_update_kind_is_skipped() {
    assert_eq!(text(&standard("malformed-missing-kind")), "ok");
}
#[test]
fn grok_malformed_null_tool_title_is_skipped() {
    assert_eq!(text(&standard("malformed-null-title")), "ok");
}
#[test]
fn grok_malformed_numeric_prompt_id_is_skipped() {
    assert_eq!(text(&standard("malformed-numeric-prompt")), "ok");
}
#[test]
fn grok_malformed_null_meta_is_skipped() {
    assert_eq!(text(&standard("malformed-null-meta")), "ok");
}
#[test]
fn grok_malformed_numeric_update_kind_is_skipped() {
    assert_eq!(text(&standard("malformed-numeric-kind")), "ok");
}
fn stop_foreign_work(thread_stop: bool) {
    engine_replay("foreign-open-stop", |engine, thread, events, runtime| {
        runtime.block_on(async {
            use veyra_engine::{
                commands::Command,
                events::{ChangeEvent, UiEvent},
            };
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut provider = None;
                while let Some(event) = events.recv().await {
                    match event {
                        UiEvent::Change {
                            event: ChangeEvent::TurnStarted(turn),
                            ..
                        } if turn.origin == TurnOrigin::Provider => {
                            provider = Some(turn.id);
                            assert!(engine.window_threads()[&thread].busy);
                            let command = if thread_stop {
                                Command::StopThread { thread }
                            } else {
                                Command::Interrupt {
                                    thread,
                                    turn: turn.id,
                                }
                            };
                            engine.send(command).await.unwrap().unwrap();
                        }
                        UiEvent::Change {
                            event: ChangeEvent::TurnEnded(turn),
                            ..
                        } if turn.origin == TurnOrigin::Provider => {
                            assert_eq!(Some(turn.id), provider);
                            assert!(matches!(
                                turn.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Interrupted { .. },
                                    ..
                                }
                            ));
                            native_replies_checked(engine).await;
                            return;
                        }
                        _ => {}
                    }
                }
                panic!("Stop did not reach promoted Provider work");
            })
            .await
            .unwrap();
        })
    });
}
#[test]
fn grok_foreign_open_work_accepts_targeted_stop() {
    stop_foreign_work(false);
}
#[test]
fn grok_foreign_open_work_accepts_stop_thread() {
    stop_foreign_work(true);
}
#[test]
fn grok_stop_cancels_every_foreign_request_before_session_cancel() {
    engine_replay("foreign-both-stop", |engine, thread, events, runtime| {
        runtime.block_on(async {
            use veyra_engine::{
                commands::Command,
                events::{ChangeEvent, UiEvent},
            };
            tokio::time::timeout(Duration::from_secs(5), async {
                let (mut opened, mut closed) = (0, 0);
                while let Some(event) = events.recv().await {
                    match event {
                        UiEvent::Change {
                            event: ChangeEvent::RequestOpened(_),
                            ..
                        } => {
                            opened += 1;
                            if opened == 2 {
                                engine
                                    .send(Command::StopThread { thread })
                                    .await
                                    .unwrap()
                                    .unwrap();
                            }
                        }
                        UiEvent::Change {
                            event: ChangeEvent::RequestClosed(request),
                            ..
                        } => {
                            assert!(matches!(request.state, RequestState::Expired { .. }));
                            closed += 1;
                        }
                        UiEvent::Change {
                            event: ChangeEvent::TurnEnded(turn),
                            ..
                        } if turn.origin != TurnOrigin::Provider => {
                            assert_eq!((opened, closed), (2, 2));
                            assert!(matches!(
                                turn.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Interrupted { .. },
                                    ..
                                }
                            ));
                            native_replies_checked(engine).await;
                            return;
                        }
                        _ => {}
                    }
                }
                panic!("foreign requests never cancelled");
            })
            .await
            .unwrap();
        })
    });
}

async fn native_replies_checked(engine: &veyra_engine::Engine) {
    let path = engine
        .child_env()
        .fixtures
        .unwrap()
        .join("native-replies-checked");
    while !path.exists() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
#[test]
fn grok_stopped_veyra_turn_announces_unfinished_foreign_work() {
    engine_replay(
        "foreign-open-after-stop",
        |engine, thread, events, runtime| {
            runtime.block_on(async {
                use veyra_engine::{
                    commands::Command,
                    events::{ChangeEvent, UiEvent},
                };
                tokio::time::timeout(Duration::from_secs(5), async {
                    let (mut stopped, mut provider) = (false, None);
                    while let Some(event) = events.recv().await {
                        match event {
                            UiEvent::TextAppended { .. } if !stopped => {
                                stopped = true;
                                engine
                                    .send(Command::StopThread { thread })
                                    .await
                                    .unwrap()
                                    .unwrap();
                            }
                            UiEvent::Change {
                                event: ChangeEvent::TurnStarted(turn),
                                ..
                            } if turn.origin == TurnOrigin::Provider => {
                                assert!(stopped);
                                assert!(engine.window_threads()[&thread].busy);
                                provider = Some(turn.id);
                            }
                            UiEvent::Change {
                                event: ChangeEvent::TurnEnded(turn),
                                ..
                            } if turn.origin == TurnOrigin::Provider => {
                                assert_eq!(Some(turn.id), provider);
                                assert!(matches!(
                                    turn.state,
                                    TurnState::Ended {
                                        outcome: TurnOutcome::Interrupted { .. },
                                        ..
                                    }
                                ));
                                native_replies_checked(engine).await;
                                return;
                            }
                            _ => {}
                        }
                    }
                    panic!("unfinished foreign work was not announced after Stop");
                })
                .await
                .unwrap();
            })
        },
    );
}
#[test]
fn grok_foreign_completion_with_null_reason_ends_provider_turn() {
    engine_replay(
        "foreign-completion-null",
        |engine, thread, events, runtime| {
            runtime.block_on(async {
                use veyra_engine::events::{ChangeEvent, UiEvent};
                tokio::time::timeout(Duration::from_secs(3), async {
                    let mut provider = None;
                    while let Some(event) = events.recv().await {
                        match event {
                            UiEvent::Change {
                                event: ChangeEvent::TurnStarted(turn),
                                ..
                            } if turn.origin == TurnOrigin::Provider => provider = Some(turn.id),
                            UiEvent::Change {
                                event: ChangeEvent::TurnEnded(turn),
                                ..
                            } if turn.origin == TurnOrigin::Provider => {
                                assert_eq!(provider, Some(turn.id));
                                assert_eq!(turn.thread, thread);
                                assert!(
                                    matches!(
                                        turn.state,
                                        TurnState::Ended {
                                            outcome: TurnOutcome::Completed,
                                            ..
                                        }
                                    ),
                                    "{turn:?}"
                                );
                                assert!(!engine.window_threads()[&thread].busy);
                                return;
                            }
                            _ => {}
                        }
                    }
                    panic!("null completion did not end Provider work");
                })
                .await
                .unwrap();
            })
        },
    );
}
#[test]
fn grok_plan_defaults_each_entry_status() {
    let events = standard("plan-entry-defaults");
    let plans: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                event: TurnEvent::Plan(steps),
                ..
            } => Some(steps.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        plans,
        [vec![
            PlanStep {
                text: "Step one".into(),
                done: true
            },
            PlanStep {
                text: "Step two".into(),
                done: false
            },
            PlanStep {
                text: "Step three".into(),
                done: false
            },
        ]]
    );
    assert_eq!(text(&events), "ok");
    assert_eq!(outcomes(&events), [TurnOutcome::Completed]);
}
#[test]
fn grok_foreign_tool_keeps_start_metadata_after_promotion() {
    let events = standard("foreign-tool-span");
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                turn,
                event: TurnEvent::ToolStarted { key, call },
            } => Some((turn, key, call)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 3);
    for ((owner, key, call), (name, arguments, tail)) in starts.into_iter().zip([
        ("Bash", serde_json::json!({"command":"ls"}), "file.txt"),
        ("Read", serde_json::json!({"path":"two"}), "two.txt"),
        ("Read", serde_json::json!({"path":"three"}), "three.txt"),
    ]) {
        assert_eq!(*owner, None);
        assert_eq!(call.name, name);
        assert_eq!(call.arguments, arguments);
        assert_eq!(events.iter().filter(|event| matches!(event, ProviderEvent::Turn { turn: None, event: TurnEvent::ToolFinished { key: finished, ok: true, output_tail: Some(output) } } if finished == key && output == tail)).count(), 1);
    }
    assert!(!events.iter().any(|event| matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::ToolOutput { .. }
                | TurnEvent::ReasoningDelta { .. }
                | TurnEvent::Plan(_),
            ..
        }
    )));
    assert_eq!(text(&events), "okafter");
    assert_eq!(
        outcomes(&events),
        [TurnOutcome::Completed, TurnOutcome::Completed]
    );
}

#[test]
fn grok_tool_title_updates_one_persisted_row() {
    engine_replay("tool-title", |engine, thread, events, runtime| {
        runtime.block_on(async {
            use veyra_engine::events::{ChangeEvent, UiEvent};
            tokio::time::timeout(Duration::from_secs(10), async {
                let mut ids = std::collections::BTreeSet::new();
                let mut titles = Vec::new();
                let mut completions = 0;
                while let Some(event) = events.recv().await {
                    match event {
                        UiEvent::Change {event:ChangeEvent::ItemAdded(item) | ChangeEvent::ItemCompleted(item),..} => {
                            if let ItemBody::Tool(tool) = item.body {
                                ids.insert((item.id,item.seq));
                                if tool.result.is_some() { completions += 1; }
                                titles.push(tool.call.name);
                            }
                        }
                        UiEvent::Change {event:ChangeEvent::TurnEnded(_),..} => {
                            let snapshot = engine.snapshot(thread).await.unwrap();
                            let tools = snapshot.items.iter().filter_map(|item|match &item.body {ItemBody::Tool(tool)=>Some(tool),_=>None}).collect::<Vec<_>>();
                            assert_eq!(tools.len(),1);
                            assert_eq!(ids.len(),1);
                            assert!(titles.iter().any(|title|title=="Running pwd"));
                            assert_eq!(completions,1);
                            assert_eq!(tools[0].call.name,"Execute `pwd`");
                            assert_eq!(tools[0].call.arguments,serde_json::json!({"variant":"Bash","command":"pwd"}));
                            assert_eq!(tools[0].result.as_ref().unwrap().output_tail.as_deref(),Some("/project\n"));
                            let db = rusqlite::Connection::open(engine.child_env().fixtures.unwrap().join("data/veyra.db")).unwrap();
                            let (count,body):(i64,String) = db.query_row("select count(*),body_json from items where thread_id=?1 and kind='tool'",[thread.to_string()],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
                            assert_eq!(count,1);
                            assert!(body.contains("Execute `pwd`"));
                            return;
                        }
                        _=>{}
                    }
                }
                panic!("tool turn did not end");
            }).await.unwrap();
        });
    });
}

#[test]
fn grok_accept_edits_docks_inside_and_outside() {
    let (events, wrote) = harness::replay(
        ProviderKind::Grok,
        "approval-accept-edits",
        PermissionMode::AcceptEdits,
        false,
        false,
        Inputs::answers([Decision::AllowOnce, Decision::Deny]),
    );
    assert!(!wrote);
    let requests = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                event: TurnEvent::RequestOpened { native, kind },
                ..
            } => Some((native, kind)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests.iter().map(|r| r.0.0.as_str()).collect::<Vec<_>>(),
        ["\"edit-inside\"", "\"edit-outside\""]
    );
    assert!(text(&events).is_empty());
    assert_eq!(
        outcomes(&events),
        [TurnOutcome::Interrupted {
            reason: "Stopped after you denied a request".into()
        }]
    );
}

#[test]
fn grok_provider_permission_without_prompt_is_persisted_and_answered() {
    engine_replay(
        "provider-permission-no-prompt",
        |engine, thread, events, runtime| {
            runtime.block_on(async {
                use veyra_engine::{
                    commands::Command,
                    events::{ChangeEvent, UiEvent},
                };
                tokio::time::timeout(Duration::from_secs(5), async {
                    let mut opened = None;
                    while let Some(event) = events.recv().await {
                        match event {
                            UiEvent::Change {
                                event: ChangeEvent::RequestOpened(request),
                                ..
                            } => {
                                assert_eq!(request.thread, thread);
                                assert_eq!(request.state, RequestState::Pending);
                                let db = rusqlite::Connection::open(
                                    engine.child_env().fixtures.unwrap().join("data/veyra.db"),
                                )
                                .unwrap();
                                let (turn_id, state): (String, String) = db
                                    .query_row(
                                        "select turn_id,state from requests where id=?1",
                                        [request.id.to_string()],
                                        |row| Ok((row.get(0)?, row.get(1)?)),
                                    )
                                    .unwrap();
                                assert_eq!(
                                    (turn_id, state),
                                    (request.turn.to_string(), "pending".into())
                                );
                                let origin: String = db
                                    .query_row(
                                        "select origin_json from turns where id=?1",
                                        [request.turn.to_string()],
                                        |row| row.get(0),
                                    )
                                    .unwrap();
                                assert!(origin.contains("Provider"));
                                opened = Some(request.id);
                                engine
                                    .send(Command::Respond {
                                        request: request.id,
                                        decision: Decision::AllowOnce,
                                    })
                                    .await
                                    .unwrap()
                                    .unwrap();
                            }
                            UiEvent::Change {
                                event: ChangeEvent::TurnEnded(turn),
                                ..
                            } if turn.origin == TurnOrigin::Provider => {
                                assert!(matches!(
                                    turn.state,
                                    TurnState::Ended {
                                        outcome: TurnOutcome::Completed,
                                        ..
                                    }
                                ));
                                let snapshot = engine.snapshot(thread).await.unwrap();
                                assert!(snapshot.items.iter().any(|item| matches!(
                                    &item.body, ItemBody::AssistantText {text,..} if text=="report"
                                )));
                                let db = rusqlite::Connection::open(
                                    engine.child_env().fixtures.unwrap().join("data/veyra.db"),
                                )
                                .unwrap();
                                let state: String = db
                                    .query_row(
                                        "select state from requests where id=?1",
                                        [opened.unwrap().to_string()],
                                        |row| row.get(0),
                                    )
                                    .unwrap();
                                assert_eq!(state, "answered");
                                native_replies_checked(engine).await;
                                return;
                            }
                            _ => {}
                        }
                    }
                    panic!("Provider permission never docked and completed");
                })
                .await
                .unwrap();
            });
        },
    );
}

#[test]
fn grok_load_replay_import() {
    let (_scratch, env) = harness::environment(ProviderKind::Grok, "load-replay-import");
    Runtime::new().unwrap().block_on(async {
        let (out, mut events) = mpsc::channel(256);
        let handle = Spawner::Cli(Box::new(env))
            .spawn(
                ProviderKind::Grok,
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: _scratch.0.clone(),
                    config: ThreadConfig {
                        model: None,
                        effort: None,
                        mode: PermissionMode::FullAccess,
                    },
                    native: Some(NativeSessionId("native-import".into())),
                    import: Some(HistoryRequest {
                        cursor: Some("old".into()),
                        sent: ["sent".into()].into(),
                    }),
                    root: false,
                },
                out,
                1,
            )
            .unwrap();
        let mut history = None;
        let mut ready = false;
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
                match event {
                    ProviderEvent::Session(SessionEvent::History {
                        native,
                        history: value,
                    }) => {
                        assert!(!ready);
                        assert!(history.is_none());
                        assert_eq!(native.0, "native-import");
                        history = Some(value);
                    }
                    ProviderEvent::Session(SessionEvent::Ready { .. }) => {
                        assert!(history.is_some(), "Ready preceded the import");
                        ready = true;
                        handle.control.send(SessionControl::Shutdown).unwrap();
                    }
                    ProviderEvent::Session(SessionEvent::Exited { crash }) => {
                        assert_eq!(crash, None);
                        return;
                    }
                    ProviderEvent::Turn { .. } => panic!("replay escaped as a new running turn"),
                    _ => {}
                }
            }
            panic!("import session ended without Exited");
        })
        .await
        .unwrap();
        assert!(ready);
        assert_eq!(
            history.unwrap(),
            History {
                failed: false,
                cursor: Some("terminal".into()),
                text: "second\nreply second".into()
            }
        );
    });
}
