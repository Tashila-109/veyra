use super::requests::{codec, request};
use super::*;
use serde_json::json;

pub(super) fn approval(f: &mut Fixture, tool: &str, command: &str) -> RequestId {
    let before: std::collections::BTreeSet<_> =
        f.actor.projection.requests.keys().copied().collect();
    f.event(turn_event(
        f.running(),
        TurnEvent::RequestOpened {
            native: NativeRequestRef(format!("request-{}", before.len())),
            kind: RequestKind::Approval {
                action: ProposedAction {
                    consent: Default::default(),
                    tool: tool.into(),
                    target: ApprovalTarget::Command {
                        command: command.into(),
                        cwd: Default::default(),
                        reason: None,
                        preview: ApprovalPreview::new(String::new()),
                    },
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
    let mut env = env::ChildEnv::isolated();
    env.claude_bin = Some(
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("veyra-fake-cli")
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
            Spawner::Cli(Box::new(env)),
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
                            let ApprovalTarget::Command { command, .. } = action.target else {
                                panic!("command approval")
                            };
                            docked.push(command);
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
fn grant_does_not_answer_commands_with_extra_fields() {
    let mut f = Fixture::new(LiveState::Running);
    let mut c = ChangeSet::default();
    c.grants.push((
        f.thread,
        GrantKey {
            tool: "Bash".into(),
            command: "echo safe".into(),
        },
    ));
    f.save(c);
    let mut codec = codec(&f);
    let plain = codec
        .value(request(
            "plain",
            "Bash",
            json!({
        "command":"echo safe", "description":"safe", "timeout":1000}),
        ))
        .unwrap();
    for event in plain.events {
        f.event(event);
    }
    assert!(matches!(
        f.controls.try_recv().unwrap(),
        SessionControl::Answer {
            reply: Decision::AllowOnce,
            ..
        }
    ));
    for field in ["dangerouslyDisableSandbox", "run_in_background"] {
        let mut input = json!({"command":"echo safe"});
        input[field] = json!(true);
        let opened = codec.value(request(field, "Bash", input)).unwrap();
        assert!(matches!(
            &opened.events[0],
            ProviderEvent::Turn {
                event: TurnEvent::RequestOpened {
                    kind: RequestKind::Approval {
                        grant_key: None,
                        ..
                    },
                    ..
                },
                ..
            }
        ));
        for event in opened.events {
            f.event(event);
        }
        assert!(
            f.controls.try_recv().is_err(),
            "extra input must dock without the grant"
        );
    }
    assert_eq!(
        f.loaded()
            .requests
            .values()
            .filter(|r| r.state == RequestState::Pending)
            .count(),
        2
    );
}

fn codex_grant_codec(f: &Fixture) -> crate::provider::codex::Codec {
    let mut codec = crate::provider::codex::Codec::new(
        SessionSpec {
            thread: f.thread,
            cwd: "/tmp".into(),
            config: config(),
            native: Some(NativeSessionId("codex-thread".into())),
            import: false,
            root: false,
        },
        1,
        None,
    );
    codec.initialize();
    codec.value(json!({"id":1,"result":{}})).unwrap();
    codec
        .value(json!({"id":2,"result":{"thread":{"id":"codex-thread"}}}))
        .unwrap();
    codec
        .start(f.running().unwrap(), "test grants".into())
        .unwrap();
    codec
        .value(json!({"id":3,"result":{"turn":{"id":"native-turn","status":"inProgress"}}}))
        .unwrap();
    codec
}
fn codex_approval(
    f: &mut Fixture,
    codec: &mut crate::provider::codex::Codec,
    id: &str,
    cwd: &str,
) -> RequestId {
    let decoded = codec.value(json!({"id":id,"method":"item/commandExecution/requestApproval",
        "params":{"threadId":"codex-thread","turnId":"native-turn","itemId":id,"command":"rm -rf build","cwd":cwd}})).unwrap();
    for event in decoded.events {
        f.event(event);
    }
    f.actor
        .projection
        .requests
        .values()
        .find(|r| r.native == NativeRequestRef(format!("\"{id}\"")))
        .unwrap()
        .id
}
#[test]
fn codex_grant_other_cwd_always_docks() {
    let mut f = Fixture::new(LiveState::Running);
    let runtime = f.runtime.handle().clone();
    let _entered = runtime.enter();
    let mut codec = codex_grant_codec(&f);
    let first = codex_approval(&mut f, &mut codec, "first", "/tmp");
    f.command(Command::Respond {
        request: first,
        decision: Decision::AllowInThread,
    })
    .unwrap();
    assert_eq!(
        f.loaded().requests[&first].state,
        RequestState::Answered(Decision::AllowInThread)
    );
    f.controls.try_recv().unwrap();
    f.drain();
    let other = codex_approval(&mut f, &mut codec, "other", "/tmp/elsewhere");
    assert_eq!(f.loaded().requests[&other].state, RequestState::Pending);
    assert!(
        f.controls.try_recv().is_err(),
        "another cwd was automatically allowed"
    );
    assert!(f.drain().iter().any(|e| matches!(e, UiEvent::Change { event: crate::events::ChangeEvent::RequestOpened(r), .. } if r.id == other)));
}
#[test]
fn codex_grant_same_command_and_cwd_is_automatic() {
    let mut f = Fixture::new(LiveState::Running);
    let runtime = f.runtime.handle().clone();
    let _entered = runtime.enter();
    let mut codec = codex_grant_codec(&f);
    let first = codex_approval(&mut f, &mut codec, "first", "/tmp");
    f.command(Command::Respond {
        request: first,
        decision: Decision::AllowInThread,
    })
    .unwrap();
    f.controls.try_recv().unwrap();
    f.drain();
    let same = codex_approval(&mut f, &mut codec, "same", "/tmp");
    assert_eq!(
        f.loaded().requests[&same].state,
        RequestState::Answered(Decision::AllowOnce)
    );
    assert!(matches!(
        f.controls.try_recv().unwrap(),
        SessionControl::Answer {
            reply: Decision::AllowOnce,
            ..
        }
    ));
}
