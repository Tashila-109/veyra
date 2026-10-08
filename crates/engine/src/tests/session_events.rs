use super::*;
use crate::events::ChangeEvent as Ev;

#[test]
fn first_ready_starts_queued_turn() {
    let mut f = Fixture::new(LiveState::Starting);
    let turn = f.user("first prompt");
    f.event(ProviderEvent::Session(SessionEvent::Ready {
        native: NativeSessionId("conversation-ready".into()),
        cli_version: "test-one".into(),
        pid: 23,
    }));
    assert_eq!(f.running(), Some(turn));
    assert_eq!(
        f.loaded().threads[&f.thread].native,
        Some(NativeSessionId("conversation-ready".into()))
    );
    assert_eq!(
        f.controls.try_recv().unwrap(),
        SessionControl::StartTurn {
            turn,
            text: "first prompt".into()
        }
    );
}
#[test]
fn idle_exited_parks() {
    let mut f = Fixture::new(LiveState::Idle);
    f.event(ProviderEvent::Session(SessionEvent::Exited { crash: None }));
    assert!(matches!(f.actor.live[&f.thread].live, Live::Parked));
    assert_eq!(f.actor.live[&f.thread].epoch, 1);
}
#[test]
fn exited_after_turn_ended_is_ignored() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running().unwrap();
    f.event(turn_event(
        Some(turn),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    f.event(ProviderEvent::Session(SessionEvent::Exited {
        crash: Some("late exit".into()),
    }));
    assert!(matches!(
        f.loaded().turns[&turn].state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    ));
    assert!(matches!(f.actor.live[&f.thread].live, Live::Parked));
}
#[test]
fn background_work_during_user_turn() {
    let mut f = Fixture::new(LiveState::Idle);
    let turn = match f
        .command(Command::Send {
            thread: f.thread,
            text: "user prompt".into(),
        })
        .unwrap()
    {
        Ack::Turn(id) => id,
        _ => panic!(),
    };
    f.event(ProviderEvent::Session(SessionEvent::BackgroundWork {
        pending: true,
    }));
    assert!(f.actor.live[&f.thread].background_work);
    assert_eq!(f.running(), Some(turn));
    let queued = f.user("next prompt");
    f.event(turn_event(
        Some(turn),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    assert_eq!(f.loaded().turns[&queued].state, TurnState::Queued);
    f.event(ProviderEvent::Session(SessionEvent::BackgroundWork {
        pending: false,
    }));
    assert_eq!(f.running(), Some(queued));
}
#[test]
fn background_work_keeps_completed_thread_working() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running();
    f.event(ProviderEvent::Session(SessionEvent::BackgroundWork {
        pending: true,
    }));
    f.event(turn_event(
        turn,
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    let c = ChangeSet::default();
    assert_eq!(
        thread_status(
            &c.view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap()
        ),
        Status::Working { since: 0 }
    );
    f.event(ProviderEvent::Session(SessionEvent::BackgroundWork {
        pending: false,
    }));
    assert!(matches!(
        thread_status(
            &c.view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap()
        ),
        Status::Done { .. }
    ));
}
#[test]
fn interrupt_completes_and_preserves_buffered_text() {
    let mut f = Fixture::new(LiveState::Running);
    f.event(turn_event(
        f.running(),
        TurnEvent::TextDelta {
            key: NativeItemKey("interrupted-stream".into()),
            text: "kept text".into(),
        },
    ));
    f.drain();
    f.command(Command::Interrupt { thread: f.thread }).unwrap();
    assert_eq!(
        f.loaded().items.values().next().unwrap().body,
        ItemBody::AssistantText {
            text: "kept text".into(),
            done: true
        }
    );
    let events = f.drain();
    let completed = events
        .iter()
        .position(|e| {
            matches!(
                e,
                UiEvent::Change {
                    event: Ev::ItemCompleted(_),
                    ..
                }
            )
        })
        .unwrap();
    let ended = events
        .iter()
        .position(|e| {
            matches!(
                e,
                UiEvent::Change {
                    event: Ev::TurnEnded(_),
                    ..
                }
            )
        })
        .unwrap();
    assert!(completed < ended);
}
#[test]
fn stale_epoch_dropped() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running().unwrap();
    f.event_epoch(
        0,
        turn_event(
            Some(turn),
            TurnEvent::TurnEnded {
                outcome: TurnOutcome::Failed {
                    message: "old".into(),
                },
            },
        ),
    );
    f.event_epoch(
        0,
        ProviderEvent::Session(SessionEvent::Exited { crash: None }),
    );
    assert_eq!(f.running(), Some(turn));
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Structured { pid: 9, .. }
    ));
}
#[test]
fn provider_turn_join() {
    let mut f = Fixture::new(LiveState::Idle);
    f.event(turn_event(None, TurnEvent::Started));
    let turn = f.running().unwrap();
    f.event(turn_event(
        None,
        TurnEvent::TextDelta {
            key: NativeItemKey("provider-item".into()),
            text: "Hello ".into(),
        },
    ));
    f.event(turn_event(
        None,
        TurnEvent::TextDelta {
            key: NativeItemKey("provider-item".into()),
            text: "world".into(),
        },
    ));
    f.event(turn_event(
        None,
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    let p = f.loaded();
    assert_eq!(p.turns.len(), 1);
    assert_eq!(p.turns[&turn].origin, TurnOrigin::Provider);
    assert_eq!(
        p.items.values().next().unwrap().body,
        ItemBody::AssistantText {
            text: "Hello world".into(),
            done: true
        }
    );
}
#[test]
fn lone_turn_end_and_wrong_turn_are_dropped() {
    let mut f = Fixture::new(LiveState::Idle);
    f.event(turn_event(
        None,
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    assert_eq!(f.loaded().turns.len(), 0);
    f.command(Command::Send {
        thread: f.thread,
        text: "actual turn".into(),
    })
    .unwrap();
    let turn = f.running().unwrap();
    f.event(turn_event(
        Some(TurnId::new()),
        TurnEvent::TextDelta {
            key: NativeItemKey("wrong".into()),
            text: "not this turn".into(),
        },
    ));
    assert_eq!(f.running(), Some(turn));
    assert_eq!(
        f.loaded()
            .items
            .values()
            .map(|i| i.body.clone())
            .collect::<Vec<_>>(),
        vec![ItemBody::UserText("actual turn".into())]
    );
}
#[test]
fn snapshot_joins_buffer_without_duplicate_text() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running();
    f.drain();
    for text in ["a", "b", "c"] {
        f.event(turn_event(
            turn,
            TurnEvent::TextDelta {
                key: NativeItemKey("stream".into()),
                text: text.into(),
            },
        ));
    }
    let (reply, mut rx) = oneshot::channel();
    {
        let _enter = f.runtime.enter();
        f.actor.handle(EngineMsg::Snapshot {
            thread: f.thread,
            reply,
        });
    }
    assert!(matches!(
        rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    f.command(Command::SetDefaultMode {
        mode: PermissionMode::AskFirst,
    })
    .unwrap();
    assert_eq!(f.loaded().settings.default_mode, PermissionMode::AskFirst);
    f.actor.handle(EngineMsg::TextFlush);
    let snapshot = rx.blocking_recv().unwrap();
    assert_eq!(
        snapshot.items[0].body,
        ItemBody::AssistantText {
            text: String::new(),
            done: false
        }
    );
    assert_eq!(snapshot.buffered_text[&snapshot.items[0].id], "abc");
    let events = f.drain();
    let appends: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            UiEvent::TextAppended { counter, text, .. } => Some((*counter, text)),
            _ => None,
        })
        .collect();
    assert_eq!(appends.len(), 1);
    assert_eq!(appends[0].1, "abc");
    assert!(appends[0].0 <= snapshot.counter);
    let mut joined = snapshot.buffered_text[&snapshot.items[0].id].clone();
    for (counter, text) in appends {
        if counter > snapshot.counter {
            joined.push_str(text);
        }
    }
    assert_eq!(joined, "abc");
    f.actor.handle(EngineMsg::TextWrite);
    let p = f.loaded();
    assert_eq!(
        p.items.values().next().unwrap().body,
        ItemBody::AssistantText {
            text: "abc".into(),
            done: false
        }
    );
    f.event(turn_event(
        turn,
        TurnEvent::TextDelta {
            key: NativeItemKey("stream".into()),
            text: "d".into(),
        },
    ));
    f.event(turn_event(
        turn,
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    assert_eq!(
        f.loaded().items.values().next().unwrap().body,
        ItemBody::AssistantText {
            text: "abcd".into(),
            done: true
        }
    );
    for event in f.drain() {
        if let UiEvent::TextAppended { counter, text, .. } = event
            && counter > snapshot.counter
        {
            joined.push_str(&text);
        }
    }
    assert_eq!(joined, "abcd");
}
#[test]
fn session_exit_expires_requests_and_preserves_epoch() {
    let mut f = Fixture::new(LiveState::Running);
    let req = f.req();
    f.event(ProviderEvent::Session(SessionEvent::Exited {
        crash: Some("session failed".into()),
    }));
    assert!(matches!(
        f.loaded().requests[&req].state,
        RequestState::Expired { .. }
    ));
    assert_eq!(f.actor.live[&f.thread].epoch, 1);
    assert!(f.drain().iter().any(|e|matches!(e,UiEvent::Change {event:Ev::TurnEnded(Turn {state:TurnState::Ended {outcome:TurnOutcome::Failed {message},..},..}),..} if message=="session failed")));
}
