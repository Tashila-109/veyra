use super::*;
use crate::events::ChangeEvent as Ev;

#[test]
fn spawn_failure_survives_full_actor_mailbox() {
    let mut f = Fixture::new(LiveState::Parked);
    let db = std::mem::replace(&mut f.actor.db, Connection::open_in_memory().unwrap());
    let (out, mut inbox) = mpsc::channel(1);
    out.try_send(EngineMsg::Tick).unwrap();
    f.actor = Actor::new(db, out, Spawner::Cli).unwrap();
    let (event_tx, mut events) = mpsc::unbounded_channel();
    f.actor.handle(EngineMsg::Subscribe(event_tx));
    let Ack::Turn(turn) = f
        .command(Command::Send {
            thread: f.thread,
            text: "a spawn that is not built".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(matches!(inbox.try_recv().unwrap(), EngineMsg::Tick));
    let outcome = f.runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(2), inbox.recv())
            .await
            .unwrap()
            .unwrap()
    });
    {
        let _enter = f.runtime.enter();
        f.actor.handle(outcome);
    }
    assert!(
        matches!(&f.loaded().turns[&turn].state, TurnState::Ended { outcome: TurnOutcome::Failed { message }, .. } if message.contains("not built"))
    );
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::Stopped)
    );
    assert!(matches!(f.actor.live[&f.thread].live, Live::Parked));
    let mut hold_seen = false;
    while let Ok(event) = events.try_recv() {
        hold_seen |= matches!(event, UiEvent::Change { event: Ev::ThreadHoldChanged { thread, hold: Some(HoldReason::Stopped) }, .. } if thread == f.thread);
    }
    assert!(hold_seen);
}
#[test]
fn full_actor_queue_refuses_commands_and_read_requests() {
    let (sender, _receiver) = mpsc::channel(1);
    sender.try_send(EngineMsg::Tick).unwrap();
    let engine = crate::Engine {
        inner: std::sync::Arc::new(crate::EngineInner {
            sender,
            schema_version: 1,
        }),
    };
    assert!(matches!(
        engine
            .send(Command::SetDefaultMode {
                mode: PermissionMode::AskFirst
            })
            .blocking_recv()
            .unwrap(),
        Err(CommandError::Busy(_))
    ));
    assert!(matches!(
        engine.subscribe().try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    assert!(matches!(
        engine.snapshot(ThreadId::new()).try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
}
#[test]
fn permission_mode_order() {
    assert!(PermissionMode::AskFirst < PermissionMode::AcceptEdits);
    assert!(PermissionMode::AcceptEdits < PermissionMode::FullAccess);
    let mut f = Fixture::new(LiveState::Parked);
    f.command(Command::Configure {
        thread: f.thread,
        config: ThreadConfig {
            mode: PermissionMode::AcceptEdits,
            ..config()
        },
    })
    .unwrap();
    assert_eq!(
        f.actor
            .db
            .query_row(
                "SELECT json_extract(config_json,'$.mode') FROM threads WHERE id=?1",
                [f.thread.to_string()],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "accept_edits"
    );
    assert!(serde_json::from_str::<PermissionMode>("\"unknown\"").is_err());
}
#[test]
fn hold_reason_check_refuses_unknown() {
    let mut f = Fixture::new(LiveState::Parked);
    assert!(
        f.actor
            .db
            .execute(
                "UPDATE threads SET hold_reason='surprise' WHERE id=?1",
                [f.thread.to_string()]
            )
            .is_err()
    );
    f.hold(Some(HoldReason::ChildReport));
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::ChildReport)
    );
}
#[test]
fn settings_check_refuses_bad_mode() {
    let mut f = Fixture::new(LiveState::Parked);
    assert!(
        f.actor
            .db
            .execute("UPDATE settings SET default_mode='all_powerful'", [])
            .is_err()
    );
    f.command(Command::SetDefaultMode {
        mode: PermissionMode::AskFirst,
    })
    .unwrap();
    assert_eq!(f.loaded().settings.default_mode, PermissionMode::AskFirst);
}
#[test]
fn default_mode_starts_full_access() {
    let mut db = Connection::open_in_memory().unwrap();
    store::initialize(&mut db).unwrap();
    assert_eq!(
        store::load(&db).unwrap().settings.default_mode,
        PermissionMode::FullAccess
    );
    store::initialize(&mut db).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM settings", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
}
#[test]
fn set_default_mode_persists_and_publishes() {
    let mut f = Fixture::new(LiveState::Parked);
    assert_eq!(
        f.drain(),
        vec![UiEvent::Change {
            counter: None,
            event: Ev::SettingsChanged(Settings {
                default_mode: PermissionMode::FullAccess
            })
        }]
    );
    f.command(Command::SetDefaultMode {
        mode: PermissionMode::AskFirst,
    })
    .unwrap();
    assert_eq!(f.loaded().settings.default_mode, PermissionMode::AskFirst);
    assert_eq!(
        f.loaded().threads[&f.thread].config.mode,
        PermissionMode::FullAccess
    );
    assert_eq!(
        f.drain(),
        vec![UiEvent::Change {
            counter: None,
            event: Ev::SettingsChanged(Settings {
                default_mode: PermissionMode::AskFirst
            })
        }]
    );
}
#[test]
fn lineage_check_refuses_grandchild() {
    let mut f = Fixture::new(LiveState::Parked);
    let child = f.family();
    for root in [child, f.thread] {
        let mut t = f.actor.projection.threads[&child].clone();
        t.id = ThreadId::new();
        t.lineage = Lineage::Child {
            parent: child,
            root,
            relationship: Relationship::Subagent,
        };
        let mut c = ChangeSet::default();
        c.threads.insert(t.id, t);
        let tx = f.actor.db.transaction().unwrap();
        assert!(store::apply(&tx, &c, now()).is_err());
    }
    assert!(
        f.actor
            .db
            .execute(
                "UPDATE threads SET root_thread_id=?1 WHERE id=?2",
                [child.to_string(), f.thread.to_string()]
            )
            .is_err()
    );
    let sibling = f.family();
    assert!(
        f.actor
            .db
            .execute(
                "UPDATE threads SET parent_thread_id=?1,root_thread_id=?1 WHERE id=?2",
                [child.to_string(), sibling.to_string()]
            )
            .is_err()
    );
    assert_eq!(f.loaded().threads.len(), 3);
}
#[test]
fn failing_store_on_turn_ended_blocks_and_retries() {
    let mut f = Fixture::new(LiveState::Running);
    let req = f.req();
    let turn = f.running().unwrap();
    f.drain();
    f.actor.fail_write = true;
    f.event(turn_event(
        Some(turn),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    assert_eq!(f.running(), Some(turn));
    assert_eq!(f.loaded().requests[&req].state, RequestState::Pending);
    assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Interrupt);
    assert!(matches!(
        f.drain()[0],
        UiEvent::StorageBlocked { error: Some(_) }
    ));
    assert!(matches!(
        f.command(Command::Send {
            thread: f.thread,
            text: "refused until durable".into()
        }),
        Err(CommandError::StorageBlocked(_))
    ));
    f.event(ProviderEvent::Session(SessionEvent::Exited { crash: None }));
    f.actor.fail_write = false;
    {
        let _enter = f.runtime.enter();
        f.actor.handle(EngineMsg::Tick);
    }
    assert!(matches!(
        f.loaded().turns[&turn].state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    ));
    assert!(matches!(
        f.loaded().requests[&req].state,
        RequestState::Expired { .. }
    ));
    assert!(matches!(f.actor.live[&f.thread].live, Live::Parked));
    assert!(
        f.drain()
            .iter()
            .any(|e| matches!(e, UiEvent::StorageBlocked { error: None }))
    );
}
#[test]
fn failed_command_rolls_back_without_publication() {
    let mut f = Fixture::new(LiveState::Idle);
    f.drain();
    f.actor.fail_write = true;
    assert!(matches!(
        f.command(Command::Send {
            thread: f.thread,
            text: "not committed".into()
        }),
        Err(CommandError::Store(_))
    ));
    assert_eq!(f.loaded().turns.len(), 0);
    assert_eq!(f.drain(), vec![]);
    f.actor.fail_write = false;
    f.command(Command::Send {
        thread: f.thread,
        text: "committed".into(),
    })
    .unwrap();
    assert_eq!(
        f.loaded().items.values().next().unwrap().body,
        ItemBody::UserText("committed".into())
    );
}
#[test]
fn family_status_ignores_archived_child() {
    let mut f = Fixture::new(LiveState::Idle);
    let child = f.family();
    let mut c = ChangeSet::default();
    let t = crate::commands::queue(&f.actor.projection, &mut c, child, TurnOrigin::Provider);
    c.turns.get_mut(&t).unwrap().state = TurnState::Running { since: 100 };
    let req = Request {
        id: RequestId::new(),
        thread: child,
        turn: t,
        native: NativeRequestRef("child-request".into()),
        kind: RequestKind::Question {
            questions: vec![Question {
                text: "Choose".into(),
                options: vec!["one".into()],
            }],
        },
        state: RequestState::Pending,
    };
    c.requests.insert(req.id, req);
    f.save(c);
    let c = ChangeSet::default();
    assert_eq!(
        family_status(
            &c.view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap(),
            &[c.view(&f.actor.projection, &f.actor.live, child).unwrap()]
        ),
        Status::NeedsYou
    );
    f.command(Command::ArchiveThread {
        thread: child,
        keep_children: false,
    })
    .unwrap();
    let c = ChangeSet::default();
    assert_eq!(
        family_status(
            &c.view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap(),
            &[c.view(&f.actor.projection, &f.actor.live, child).unwrap()]
        ),
        Status::Idle
    );
}
