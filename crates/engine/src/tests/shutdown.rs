use super::grants::approval;
use super::*;

#[cfg(debug_assertions)]
fn quit_during_database_write(native: bool, locked: bool) -> std::time::Duration {
    use std::time::{Duration, Instant};
    let runtime = Runtime::new().unwrap();
    let scratch =
        Scratch(std::path::Path::new("/tmp").join(format!("veyra-quit-{}", ThreadId::new())));
    let engine = crate::Engine::start_with_processes(
        runtime.handle(),
        crate::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
        vec![],
    )
    .unwrap();
    let blocker = Connection::open(scratch.0.join("veyra.db")).unwrap();
    let mut pending = None;
    if locked {
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut send = Box::pin(engine.send(Command::SetDefaultMode {
            mode: PermissionMode::AskFirst,
        }));
        assert!(
            std::future::Future::poll(
                send.as_mut(),
                &mut std::task::Context::from_waker(std::task::Waker::noop())
            )
            .is_pending()
        );
        pending = Some(send);
        let wait = Instant::now();
        while engine.perf().actor_busy_ms < 100.0 {
            assert!(wait.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let release = std::thread::spawn(move || {
        if locked {
            std::thread::sleep(Duration::from_millis(800));
            blocker.execute_batch("ROLLBACK").unwrap();
        }
    });
    let started = Instant::now();
    if native {
        runtime.block_on(engine.on_app_quit());
    } else {
        engine.shutdown();
    }
    let elapsed = started.elapsed();
    release.join().unwrap();
    runtime.block_on(engine.shutdown_wait());
    if let Some(reply) = pending {
        assert!(runtime.block_on(reply).unwrap().is_ok());
        let stored = store::open(&scratch.0.join("veyra.db")).unwrap();
        assert_eq!(
            store::load(&stored).unwrap().settings.default_mode,
            PermissionMode::AskFirst
        );
    }
    assert!(matches!(
        runtime
            .block_on(engine.send(Command::SetDefaultMode {
                mode: PermissionMode::FullAccess,
            }))
            .unwrap(),
        Err(CommandError::Closed)
    ));
    drop(engine);
    drop(runtime);
    eprintln!("native={native} locked={locked} quit_took={elapsed:?}");
    elapsed
}

#[cfg(debug_assertions)]
#[test]
fn confirm_quit_gate_does_not_stall_window_on_database_lock() {
    use std::time::Duration;
    assert!(quit_during_database_write(false, false) < Duration::from_millis(10));
    let elapsed = quit_during_database_write(false, true);
    assert!(
        elapsed < Duration::from_millis(10),
        "Engine::shutdown took {elapsed:?}"
    );
}

#[cfg(debug_assertions)]
#[test]
fn native_quit_gate_stays_inside_gpui_shutdown_budget_on_database_lock() {
    use std::time::Duration;
    assert!(quit_during_database_write(true, false) < Duration::from_millis(150));
    let elapsed = quit_during_database_write(true, true);
    assert!(
        elapsed < Duration::from_millis(150),
        "native quit took {elapsed:?}"
    );
}

#[test]
fn window_thread_reason_is_available_after_reload() {
    let mut f = Fixture::new(LiveState::Running);
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    let database = std::mem::replace(
        &mut f.actor.db,
        rusqlite::Connection::open_in_memory().unwrap(),
    );
    let (sender, _) = mpsc::channel(256);
    let actor = Actor::new(database, sender, Spawner::Scripted(Scripted::default())).unwrap();
    assert_eq!(
        actor.state.window_threads.lock().unwrap()[&f.thread].wait_reason,
        Some(WaitReason::Held(HoldReason::Restarted))
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
        let mut child = engine
            .child_env()
            .spawn(
                std::path::Path::new("/bin/sleep"),
                &["10".into()],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let start = std::time::Instant::now();
        engine.on_app_quit().await;
        let elapsed = start.elapsed();
        let status = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
        child.stop().await.unwrap();
        assert!(engine.inner.groups.lock().unwrap().is_empty());
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
            environment: env::ChildEnv::isolated(),
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

#[test]
fn orderly_shutdown_persists_restarted_hold_before_reconcile() {
    for queued in [false, true] {
        let mut f = Fixture::new(LiveState::Running);
        if queued {
            f.user("later");
        }
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Shutdown);
        let mut persisted = f.loaded();
        assert_eq!(
            persisted.threads[&f.thread].hold,
            Some(HoldReason::Restarted)
        );
        let mut live = persisted
            .threads
            .keys()
            .map(|&id| (id, LiveThread::default()))
            .collect();
        crate::reconcile::decide(&persisted, now()).install(&mut persisted, &mut live);
        assert_eq!(
            persisted.threads[&f.thread].hold,
            Some(HoldReason::Restarted)
        );
        let view = ChangeSet::default();
        let view = view.view(&persisted, &live, f.thread).unwrap();
        assert_eq!(
            wait_reason(&view),
            Some(WaitReason::Held(HoldReason::Restarted))
        );
        assert_eq!(thread_status(&view), Status::Held);
    }
}

#[test]
fn shutdown_expires_requests_with_closing_reason() {
    let mut f = Fixture::new(LiveState::Running);
    let request = f.req();
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    assert_eq!(
        f.loaded().requests[&request].state,
        RequestState::Expired {
            reason: "Veyra is closing".into()
        }
    );
}

#[test]
fn quit_during_stop_drain_keeps_queued_work_held_after_reload() {
    let mut f = Fixture::new(LiveState::Running);
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: f.active_turn().unwrap(),
    })
    .unwrap();
    let Ack::Turn(queued) = f
        .command(Command::Send {
            thread: f.thread,
            text: "queued after Stop".into(),
        })
        .unwrap()
    else {
        panic!("missing queued turn")
    };
    assert_eq!(f.running(), None);
    assert_eq!(f.actor.projection.threads[&f.thread].hold, None);
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Structured {
            draining: Some(_),
            ..
        }
    ));
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    assert!(matches!(f.actor.live[&f.thread].live, Live::Stopping));
    f.event(ProviderEvent::Session(SessionEvent::Exited { crash: None }));
    assert!(
        matches!(f.actor.live[&f.thread].live, Live::Parked),
        "quit spawned a queued thread: {:?}",
        f.actor.live[&f.thread].live
    );
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::Restarted)
    );
    assert_eq!(f.loaded().turns[&queued].state, TurnState::Queued);
    let db = std::mem::replace(&mut f.actor.db, Connection::open_in_memory().unwrap());
    let (out, _) = mpsc::channel(256);
    let actor = Actor::new(db, out, Spawner::Scripted(Scripted::default())).unwrap();
    assert!(matches!(actor.live[&f.thread].live, Live::Parked));
    assert_eq!(actor.projection.turns[&queued].state, TurnState::Queued);
    assert_eq!(
        actor.projection.threads[&f.thread].hold,
        Some(HoldReason::Restarted)
    );
    assert_eq!(
        actor.state.window_threads.lock().unwrap()[&f.thread].wait_reason,
        Some(WaitReason::Held(HoldReason::Restarted))
    );
}

#[test]
fn quit_while_stopping_keeps_queued_work_parked() {
    let mut f = Fixture::new(LiveState::Stopping);
    f.actor.live.get_mut(&f.thread).unwrap().live = Live::Stopping;
    let queued = f.user("waiting for retiring session");
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    f.event(ProviderEvent::Session(SessionEvent::Exited { crash: None }));
    assert!(
        matches!(f.actor.live[&f.thread].live, Live::Parked),
        "quit restarted a stopping thread: {:?}",
        f.actor.live[&f.thread].live
    );
    assert_eq!(f.running(), None);
    assert_eq!(f.loaded().turns[&queued].state, TurnState::Queued);
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::Restarted)
    );
}

#[cfg(debug_assertions)]
#[test]
fn actor_shutdown_during_missing_resume_spawns_no_replacement() {
    use std::{fs, path::PathBuf, time::Duration};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/actor-quit-missing-resume-{}",
        ThreadId::new()
    ));
    fs::create_dir_all(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let marker = root.join("replacement-spawned");
    let started = root.join("resume-started");
    let binary = root.join("owned-session-stand-in");
    fs::write(
        &binary,
        r#"#!/bin/sh
for arg do
  if [ "$arg" = "--resume" ]; then
    printf 'No conversation found with session ID: missing-native\n' >&2
    printf started > resume-started
    read initialize
    read until_stdin_closes
    exit 1
  fi
done
printf launched > replacement-spawned
read initialize
"#,
    )
    .unwrap();
    assert!(
        std::process::Command::new("/bin/chmod")
            .arg("700")
            .arg(&binary)
            .status()
            .unwrap()
            .success()
    );
    let mut env = env::ChildEnv::isolated();
    env.groups = Default::default();
    env.claude_bin = Some(binary);
    env.fixtures = None;
    env.record = None;
    let mut f = Fixture::new(LiveState::Parked);
    let mut c = ChangeSet::default();
    let mut thread = f.loaded().threads[&f.thread].clone();
    thread.cwd = root;
    thread.native = Some(NativeSessionId("missing-native".into()));
    c.threads.insert(thread.id, thread);
    f.save(c);
    let (out, mut messages) = mpsc::channel(256);
    f.actor = Actor::new(f.actor.db, out, Spawner::Cli(Box::new(env.clone()))).unwrap();
    let Ack::Turn(queued) = f
        .command(Command::Send {
            thread: f.thread,
            text: "waiting for resume".into(),
        })
        .unwrap()
    else {
        panic!("missing queued turn")
    };
    f.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    });
    {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Shutdown);
    }
    f.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let message = messages.recv().await.unwrap();
                let exited = matches!(
                    message,
                    EngineMsg::Provider {
                        event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                        ..
                    }
                );
                f.actor.handle(message);
                if exited {
                    break;
                }
            }
        })
        .await
        .unwrap();
    });
    assert!(
        env.groups.lock().unwrap().is_empty(),
        "owned groups were not reaped"
    );
    assert_eq!(f.loaded().turns[&queued].state, TurnState::Queued);
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::Restarted)
    );
    assert!(
        !marker.exists(),
        "Actor Shutdown launched a fresh provider replacement"
    );
}

#[test]
fn shutdown_request_prevents_ready_start_before_mailbox_shutdown() {
    let mut f = Fixture::new(LiveState::Starting);
    let _entered = f.runtime.enter();
    let queued = f.user("waiting before quit");
    let (sender, mut receiver) = mpsc::channel(256);
    let engine = crate::Engine {
        inner: std::sync::Arc::new(crate::EngineInner {
            sender,
            schema_version: 2,
            state: f.actor.state.clone(),
            runtime: f.runtime.handle().clone(),
            groups: Default::default(),
            environment: env::ChildEnv::isolated(),
        }),
    };
    engine
        .inner
        .sender
        .try_send(EngineMsg::Provider {
            thread: f.thread,
            epoch: 1,
            event: ProviderEvent::Session(SessionEvent::Ready {
                native: NativeSessionId("owned-native".into()),
                cli_version: None,
                pid: 1,
                pgid: 1,
                capabilities: vec![],
            }),
        })
        .unwrap();
    engine.shutdown();
    f.actor.handle(receiver.try_recv().unwrap());
    let mut started = false;
    while let Ok(control) = f.controls.try_recv() {
        started |= matches!(control, SessionControl::StartTurn { .. });
    }
    assert!(!started, "Ready started queued work after Engine::shutdown");
    assert_eq!(f.loaded().turns[&queued].state, TurnState::Queued);
}

#[test]
fn shutdown_request_rejects_backlogged_start_commands() {
    let mut accepted = Vec::new();
    for state in [
        LiveState::Parked,
        LiveState::Idle,
        LiveState::Starting,
        LiveState::Stopping,
    ] {
        let mut f = Fixture::new(state);
        let _entered = f.runtime.enter();
        let (sender, mut receiver) = mpsc::channel(256);
        let engine = crate::Engine {
            inner: std::sync::Arc::new(crate::EngineInner {
                sender,
                schema_version: 2,
                state: f.actor.state.clone(),
                runtime: f.runtime.handle().clone(),
                groups: Default::default(),
                environment: env::ChildEnv::isolated(),
            }),
        };
        let mut reply = std::pin::pin!(engine.send(Command::Send {
            thread: f.thread,
            text: "queued before quit".into(),
        }));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(reply.as_mut(), &mut cx).is_pending());
        engine.shutdown();
        f.actor.handle(receiver.try_recv().unwrap());
        if !matches!(
            std::future::Future::poll(reply.as_mut(), &mut cx),
            std::task::Poll::Ready(Ok(Err(CommandError::Closed)))
        ) {
            accepted.push(state);
        }
    }
    assert!(
        accepted.is_empty(),
        "backlogged Send accepted after Engine::shutdown: {accepted:?}"
    );
}
#[test]
fn shutdown_request_prevents_retiring_session_restart_before_mailbox_shutdown() {
    let mut restarted = Vec::new();
    for hold in [None, Some(HoldReason::Interrupted)] {
        let mut f = Fixture::new(LiveState::Stopping);
        f.hold(hold);
        let _entered = f.runtime.enter();
        f.actor.live.get_mut(&f.thread).unwrap().live = Live::Stopping;
        let queued = f.user("queued before quit");
        let (sender, mut receiver) = mpsc::channel(256);
        let engine = crate::Engine {
            inner: std::sync::Arc::new(crate::EngineInner {
                sender,
                schema_version: 2,
                state: f.actor.state.clone(),
                runtime: f.runtime.handle().clone(),
                groups: Default::default(),
                environment: env::ChildEnv::isolated(),
            }),
        };
        engine
            .inner
            .sender
            .try_send(EngineMsg::Provider {
                thread: f.thread,
                epoch: 1,
                event: ProviderEvent::Session(SessionEvent::Exited { crash: None }),
            })
            .unwrap();
        engine.shutdown();
        f.actor.handle(receiver.try_recv().unwrap());
        if !matches!(f.actor.live[&f.thread].live, Live::Parked) {
            restarted.push(hold);
        }
        assert_eq!(f.loaded().turns[&queued].state, TurnState::Queued);
        assert_eq!(
            f.loaded().threads[&f.thread].hold,
            Some(hold.unwrap_or(HoldReason::Restarted))
        );
    }

    assert!(
        restarted.is_empty(),
        "session restarted after Engine::shutdown: {restarted:?}"
    );
}

#[test]
fn engine_and_provider_environment_share_closing_state() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    store::initialize(&mut db).unwrap();
    let env = env::ChildEnv::isolated();
    let engine = crate::Engine::launch(
        runtime.handle(),
        db,
        Spawner::Cli(Box::new(env.clone())),
        None,
    )
    .unwrap();
    let probe_env = engine.child_env();
    assert!(std::sync::Arc::ptr_eq(
        &env.closing,
        &engine.inner.state.closing
    ));
    assert!(std::sync::Arc::ptr_eq(
        &probe_env.closing,
        &engine.inner.state.closing
    ));
    assert!(std::sync::Arc::ptr_eq(&probe_env.groups, &env.groups));
    engine.shutdown();
    assert!(env.closing.is_closing());
    assert!(probe_env.closing.is_closing());
    runtime.block_on(engine.shutdown_wait());
}

#[test]
fn shutdown_between_actor_check_and_decide_commits_then_holds_turn() {
    for closed_before_read in [true, false] {
        let mut f = Fixture::new(LiveState::Idle);
        let (sender, _receiver) = mpsc::channel(256);
        let engine = crate::Engine {
            inner: std::sync::Arc::new(crate::EngineInner {
                sender,
                schema_version: 2,
                state: f.actor.state.clone(),
                runtime: f.runtime.handle().clone(),
                groups: Default::default(),
                environment: env::ChildEnv::isolated(),
            }),
        };
        if closed_before_read {
            engine.shutdown();
        }
        f.actor.after_admission = Some(Box::new(move || engine.shutdown()));
        let result = f.command(Command::Send {
            thread: f.thread,
            text: "committed before quit".into(),
        });
        assert!(f.actor.state.closing.is_closing());
        if closed_before_read {
            assert!(matches!(result, Err(CommandError::Closed)));
            assert!(f.loaded().turns.is_empty());
            assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Shutdown);
            assert!(f.controls.try_recv().is_err());
            continue;
        }
        let Ok(Ack::Turn(turn)) = result else {
            panic!("a command past its closing read must commit: {result:?}");
        };
        assert_eq!(f.running(), Some(turn));
        assert!(matches!(
            f.loaded().turns[&turn].state,
            TurnState::Running { .. }
        ));
        assert!(
            f.controls.try_recv().is_err(),
            "a prompt was dispatched after closing"
        );
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Shutdown);
        assert!(f.running().is_none());
        assert_eq!(
            f.loaded().threads[&f.thread].hold,
            Some(HoldReason::Restarted)
        );
        assert!(matches!(
            f.loaded().turns[&turn].state,
            TurnState::Ended {
                outcome: TurnOutcome::Interrupted { .. },
                ..
            }
        ));
        assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Shutdown);
        assert!(f.controls.try_recv().is_err());
    }
}

#[cfg(debug_assertions)]
fn fake_binary() -> std::path::PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("veyra-fake-cli")
}
#[cfg(debug_assertions)]
#[allow(clippy::disallowed_types)] // Hand-written fake-provider wire records.
fn session_fixture(
    provider: ProviderKind,
    records: &[serde_json::Value],
) -> (Fixture, Scratch, env::ChildEnv) {
    let mut f = Fixture::new(LiveState::Parked);
    let root = std::path::Path::new("/tmp").join(format!("veyra-term-fixture-{}", ThreadId::new()));
    crate::platform::private_dir(&root).unwrap();
    std::fs::write(
        root.join("t1-s1.jsonl"),
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.provider = provider;
    row.cwd = root.clone();
    row.native = Some(NativeSessionId(ThreadId::new().to_string()));
    c.threads.insert(f.thread, row);
    f.save(c);
    let mut db = store::open(&root.join("test.db")).unwrap();
    let tx = db.transaction().unwrap();
    store::apply(
        &tx,
        &ChangeSet {
            projects: f.actor.projection.projects.clone(),
            threads: f.actor.projection.threads.clone(),
            ..Default::default()
        },
        now(),
    )
    .unwrap();
    tx.commit().unwrap();
    f.actor.db = db;
    let mut env = env::ChildEnv::isolated().with_binary(provider, fake_binary());
    env.fixtures = Some(root.clone());
    (f, Scratch(root), env)
}
#[cfg(debug_assertions)]
#[test]
fn plain_shutdown_has_no_switch_grace() {
    use serde_json::json;
    use std::time::{Duration, Instant};
    let (f, scratch, env) = session_fixture(
        ProviderKind::Claude,
        &[
            json!({"type":"expect_outbound","line":{"type":"control_request","request_id":"<id:init>","request\
                ":{"subtype":"initialize"}}}),
            json!({"type":"emit_inbound","line":{"type":"control_response","response":{"subtype":"success","req\
                uest_id":"<id:init>","response":{}}}}),
            json!({"type":"spin","ms":1600}),
            json!({"type":"touch","path":"graceful-exit"}),
            json!({"type":"exit","code":0}),
        ],
    );
    f.runtime.block_on(async {
        let (out, mut events) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: f.thread,
                cwd: scratch.0.clone(),
                config: config(),
                native: None,
                root: false,
            },
            env,
            out,
            1,
        );
        let elapsed = tokio::time::timeout(Duration::from_secs(5), async {
            let mut started = None;
            while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
                match event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) => {
                        started = Some(Instant::now());
                        handle.control.send(SessionControl::Shutdown).unwrap();
                    }
                    ProviderEvent::Session(SessionEvent::Exited { crash }) => {
                        assert_eq!(crash, None);
                        return started.unwrap().elapsed();
                    }
                    _ => {}
                }
            }
            panic!("missing exit")
        })
        .await
        .unwrap();
        assert!(
            elapsed < Duration::from_millis(1300),
            "ordinary Shutdown took {elapsed:?}"
        );
        assert!(!scratch.0.join("graceful-exit").exists());
    });
}
