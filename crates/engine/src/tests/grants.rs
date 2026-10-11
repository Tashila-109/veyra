use super::*;

fn approval(f: &mut Fixture, tool: &str, command: &str) -> RequestId {
    let before: std::collections::BTreeSet<_> =
        f.actor.projection.requests.keys().copied().collect();
    f.event(turn_event(
        f.running(),
        TurnEvent::RequestOpened {
            native: NativeRequestRef(format!("request-{}", before.len())),
            kind: RequestKind::Approval {
                action: ProposedAction {
                    tool: tool.into(),
                    command: command.into(),
                },
                grant_key: Some(GrantKey {
                    tool: tool.into(),
                    command: command.into(),
                }),
            },
        },
    ));
    *f.actor
        .projection
        .requests
        .keys()
        .find(|id| !before.contains(id))
        .unwrap()
}
fn grant(f: &mut Fixture) {
    let request = approval(f, "Bash", "cargo test");
    f.command(Command::Respond {
        request,
        decision: Decision::AllowInThread,
    })
    .unwrap();
    assert_eq!(
        f.loaded().requests[&request].state,
        RequestState::Answered(Decision::AllowInThread)
    );
    f.drain();
}
#[test]
fn grant_compound_command_docks() {
    let mut f = Fixture::new(LiveState::Running);
    grant(&mut f);
    let request = approval(&mut f, "Bash", "cargo test && touch extra");
    assert_eq!(
        f.actor.projection.requests[&request].state,
        RequestState::Pending
    );
    assert!(f.drain().iter().any(|e| matches!(e, UiEvent::Change {
        event: crate::events::ChangeEvent::RequestOpened(r), .. } if r.id == request)));
}
#[test]
fn grant_matches_exact_command_only() {
    let mut f = Fixture::new(LiveState::Running);
    grant(&mut f);
    f.actor.projection = f.loaded();
    let exact = approval(&mut f, "Bash", "cargo test");
    assert_eq!(
        f.actor.projection.requests[&exact].state,
        RequestState::Answered(Decision::AllowOnce)
    );
    assert!(!f.drain().iter().any(|e| matches!(e, UiEvent::Change {
        event: crate::events::ChangeEvent::RequestOpened(r), .. } if r.id == exact)));
    let other_tool = approval(&mut f, "shell", "cargo test");
    let trailing = approval(&mut f, "Bash", "cargo test ");
    assert_eq!(
        f.actor.projection.requests[&other_tool].state,
        RequestState::Pending
    );
    assert_eq!(
        f.actor.projection.requests[&trailing].state,
        RequestState::Pending
    );
}
#[test]
fn grant_is_per_thread() {
    let mut f = Fixture::new(LiveState::Running);
    grant(&mut f);
    let child = f.family();
    let live = f.actor.live[&f.thread].clone();
    f.actor.live.insert(child, live);
    f.thread = child;
    f.turn(TurnOrigin::Provider, TurnState::Running { since: now() });
    let request = approval(&mut f, "Bash", "cargo test");
    assert_eq!(
        f.actor.projection.requests[&request].state,
        RequestState::Pending
    );
}
#[test]
fn grant_full_access_approval_still_docks() {
    let mut f = Fixture::new(LiveState::Running);
    assert_eq!(
        f.actor.projection.threads[&f.thread].config.mode,
        PermissionMode::FullAccess
    );
    let request = approval(&mut f, "Bash", "touch full-access");
    assert_eq!(
        f.actor.projection.requests[&request].state,
        RequestState::Pending
    );
    assert!(f.drain().iter().any(|e| matches!(e, UiEvent::Change {
        event: crate::events::ChangeEvent::RequestOpened(r), .. } if r.id == request)));
}
#[test]
fn request_expires_on_turn_end() {
    let mut f = Fixture::new(LiveState::Running);
    let request = approval(&mut f, "Bash", "touch pending");
    f.event(turn_event(
        f.running(),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
            park: false,
        },
    ));
    assert_eq!(
        f.loaded().requests[&request].state,
        RequestState::Expired {
            reason: "The turn ended".into()
        }
    );
    let events = f.drain();
    let close = events
        .iter()
        .position(|e| {
            matches!(e, UiEvent::Change {
        event: crate::events::ChangeEvent::RequestClosed(r), .. } if r.id == request)
        })
        .unwrap();
    let end = events
        .iter()
        .position(|e| {
            matches!(
                e,
                UiEvent::Change {
                    event: crate::events::ChangeEvent::TurnEnded(_),
                    ..
                }
            )
        })
        .unwrap();
    assert!(close < end);
}
#[test]
fn request_expires_on_exit() {
    let mut f = Fixture::new(LiveState::Running);
    let request = approval(&mut f, "Bash", "touch pending");
    f.event(ProviderEvent::Session(SessionEvent::Exited { crash: None }));
    assert!(matches!(
        f.loaded().requests[&request].state,
        RequestState::Expired { .. }
    ));
    assert!(matches!(
        f.command(Command::Respond {
            request,
            decision: Decision::AllowOnce
        }),
        Err(CommandError::RequestClosed)
    ));
}
#[test]
fn stop_drops_queued_user_turns() {
    let mut f = Fixture::new(LiveState::Running);
    let first = f.user("one");
    let second = f.user("two");
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    for id in [first, second] {
        assert!(matches!(
            f.loaded().turns[&id].state,
            TurnState::Ended {
                outcome: TurnOutcome::Interrupted { .. },
                ..
            }
        ));
    }
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::Stopped)
    );
}
#[test]
fn shutdown_expires_pending_requests() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running().unwrap();
    let request = approval(&mut f, "Bash", "touch pending");
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    assert!(matches!(
        f.loaded().requests[&request].state,
        RequestState::Expired { .. }
    ));
    assert!(matches!(
        f.loaded().turns[&turn].state,
        TurnState::Ended {
            outcome: TurnOutcome::Interrupted { .. },
            ..
        }
    ));
    assert!(matches!(
        f.command(Command::Send {
            thread: f.thread,
            text: "late".into()
        }),
        Err(CommandError::Closed)
    ));
}

#[test]
fn storage_failure_stops_starting_spawn() {
    let mut f = Fixture::new(LiveState::Starting);
    f.actor.fail_write = true;
    f.event(ProviderEvent::Session(SessionEvent::BackgroundWork {
        pending: true,
    }));
    assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Shutdown);
    assert!(matches!(
        f.command(Command::Send {
            thread: f.thread,
            text: "later".into()
        }),
        Err(CommandError::StorageBlocked(_))
    ));
}
#[test]
fn storage_failure_uses_storage_control() {
    let mut f = Fixture::new(LiveState::Running);
    f.actor.fail_write = true;
    f.event(turn_event(
        f.running(),
        TurnEvent::TextDelta {
            key: NativeItemKey("blocked-item".into()),
            text: "partial".into(),
            streamed: true,
        },
    ));
    assert_eq!(
        f.controls.try_recv().unwrap(),
        SessionControl::StorageBlocked
    );
    assert!(matches!(
        f.loaded().turns[&f.running().unwrap()].state,
        TurnState::Running { .. }
    ));
}

#[test]
fn question_answer_is_persisted_and_requires_every_answer() {
    let mut f = Fixture::new(LiveState::Running);
    f.event(turn_event(
        f.running(),
        TurnEvent::RequestOpened {
            native: NativeRequestRef("question".into()),
            kind: RequestKind::Question {
                questions: vec![Question {
                    text: "Choose a route".into(),
                    options: vec!["Short".into(), "Long".into()],
                }],
            },
        },
    ));
    let request = *f.actor.projection.requests.keys().next().unwrap();
    assert!(matches!(
        f.command(Command::Respond {
            request,
            decision: Decision::Answers(vec![])
        }),
        Err(CommandError::Invalid(_))
    ));
    assert_eq!(f.loaded().requests[&request].state, RequestState::Pending);
    let answers = vec![Answer {
        text: "Short".into(),
    }];
    f.command(Command::Respond {
        request,
        decision: Decision::Answers(answers.clone()),
    })
    .unwrap();
    assert_eq!(
        f.loaded().requests[&request].state,
        RequestState::Answered(Decision::Answers(answers))
    );
    assert!(
        f.loaded()
            .items_for(f.thread)
            .any(|item| item.body == ItemBody::UserText("Choose a route: Short".into()))
    );
}

#[cfg(debug_assertions)]
#[test]
fn grant_replay_automatically_answers_only_the_exact_command() {
    let root = std::env::temp_dir().join(format!("veyra-grant-{}", ThreadId::new()));
    std::fs::create_dir(&root).unwrap();
    let scratch = Scratch(root.clone());
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::copy(
        source.join("tests/fixtures/claude/approval-grant.jsonl"),
        root.join("t1-s1.jsonl"),
    )
    .unwrap();
    let mut env = env::ChildEnv::load();
    env.claude_bin = Some(
        source
            .join("../../target/debug/veyra-fake-cli")
            .canonicalize()
            .unwrap(),
    );
    env.fixtures = Some(root.clone());
    env.record = None;
    let runtime = Runtime::new().unwrap();
    runtime.block_on(async {
        let mut db = Connection::open_in_memory().unwrap();
        store::initialize(&mut db).unwrap();
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            db,
            Spawner::Cli(env),
            None,
        )
        .unwrap();
        let mut events = engine.subscribe();
        let Ack::Project(project) = engine
            .send(Command::AddProject { path: root.clone() })
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("project ack");
        };
        let Ack::Thread(thread) = engine
            .send(Command::CreateThread {
                project,
                provider: ProviderKind::Claude,
                config: config(),
                text: "grant test".into(),
            })
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("thread ack");
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let mut docked = Vec::new();
            let mut automatic = 0;
            while let Some(event) = events.recv().await {
                if let UiEvent::Change { event, .. } = event {
                    match event {
                        crate::events::ChangeEvent::RequestOpened(request) => {
                            let RequestKind::Approval { action, .. } = request.kind else {
                                panic!("approval");
                            };
                            docked.push(action.command);
                            let decision = if docked.len() == 1 {
                                Decision::AllowInThread
                            } else {
                                Decision::Deny
                            };
                            engine
                                .send(Command::Respond {
                                    request: request.id,
                                    decision,
                                })
                                .await
                                .unwrap()
                                .unwrap();
                        }
                        crate::events::ChangeEvent::RequestClosed(request)
                            if request.state == RequestState::Answered(Decision::AllowOnce) =>
                        {
                            automatic += 1
                        }
                        crate::events::ChangeEvent::TurnEnded(turn) if turn.thread == thread => {
                            assert!(matches!(
                                turn.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Completed,
                                    ..
                                }
                            ));
                            break;
                        }
                        _ => {}
                    }
                }
            }
            (docked, automatic)
        })
        .await;
        engine.shutdown_wait().await;
        let (docked, automatic) = result.unwrap();
        assert_eq!(
            docked,
            vec![
                "touch veyra-grant.txt",
                "touch veyra-grant.txt && touch compound.txt"
            ]
        );
        assert_eq!(automatic, 1);
        assert!(root.join("veyra-grant.txt").exists());
        assert!(!root.join("compound.txt").exists());
    });
    drop(scratch);
}

#[test]
fn shutdown_stops_sessions_even_when_storage_is_blocked() {
    let mut f = Fixture::new(LiveState::Running);
    f.actor.fail_write = true;
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Shutdown);
    assert!(matches!(
        f.command(Command::Continue {
            thread: f.thread,
            drop_queued: false
        }),
        Err(CommandError::Closed)
    ));
}

#[test]
fn on_app_quit_terminates_owned_groups_and_waits_100ms() {
    let runtime = Runtime::new().unwrap();
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    store::initialize(&mut db).unwrap();
    let engine = crate::Engine::scripted(runtime.handle(), db, Scripted::default()).unwrap();
    runtime.block_on(async {
        let mut child = env::ChildEnv::load()
            .spawn(
                std::path::Path::new("/bin/sleep"),
                &["10".into()],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let pid = child.id().unwrap();
        engine.inner.groups.lock().unwrap().insert(pid);
        let start = std::time::Instant::now();
        engine.on_app_quit().await;
        let elapsed = start.elapsed();
        let status = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
        if status.is_err() {
            crate::platform::signal_group(pid, crate::platform::Signal::Kill).ok();
            child.wait().await.ok();
        }
        engine.inner.groups.lock().unwrap().remove(&pid);
        engine.shutdown_wait().await;
        assert!(status.unwrap().unwrap().code().is_none());
        assert!(elapsed >= std::time::Duration::from_millis(100));
        assert!(elapsed < std::time::Duration::from_secs(1));
    });
}

#[test]
fn shutdown_wait_is_bounded_when_actor_is_stalled() {
    let runtime = Runtime::new().unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    sender.try_send(EngineMsg::Tick).unwrap();
    let engine = crate::Engine {
        inner: std::sync::Arc::new(crate::EngineInner {
            sender,
            schema_version: 2,
            state: Default::default(),
            runtime: runtime.handle().clone(),
            groups: Default::default(),
        }),
    };
    runtime.block_on(async {
        let start = std::time::Instant::now();
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(4), engine.shutdown_wait()).await;
        assert!(result.is_ok(), "shutdown waited for the stalled actor");
        assert!(start.elapsed() >= std::time::Duration::from_secs(3));
        assert!(matches!(
            engine
                .send(Command::SetDefaultMode {
                    mode: PermissionMode::FullAccess
                })
                .await
                .unwrap(),
            Err(CommandError::Closed)
        ));
        assert!(matches!(receiver.recv().await, Some(EngineMsg::Tick)));
        assert!(matches!(receiver.recv().await, Some(EngineMsg::Shutdown)));
    });
}
