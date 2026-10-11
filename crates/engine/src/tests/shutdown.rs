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
