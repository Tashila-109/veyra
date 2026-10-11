use super::grants::approval;
use super::*;

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
    f.command(Command::Interrupt { thread: f.thread }).unwrap();
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
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Stopping {
            then: AfterStop::Park
        }
    ));
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
fn quit_while_stopping_cancels_session_and_terminal_restarts() {
    for then in [
        AfterStop::Park,
        AfterStop::StartStructured { import: false },
        AfterStop::StartTerminal,
    ] {
        let mut f = Fixture::new(LiveState::Stopping);
        f.actor.live.get_mut(&f.thread).unwrap().live = Live::Stopping { then };
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
}

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
    let mut env = env::ChildEnv::load();
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
    f.actor = Actor::new(f.actor.db, out, Spawner::Cli(env.clone())).unwrap();
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
        tokio::time::timeout(Duration::from_secs(2), async {
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
        tokio::time::timeout(Duration::from_secs(3), async {
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
