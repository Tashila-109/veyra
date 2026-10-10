use super::*;
use crate::events::ChangeEvent as Ev;

#[test]
fn completed_items_and_ended_turns_free_streaming_entries() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running().unwrap();
    let key = NativeItemKey("first-text".into());
    f.event(turn_event(
        Some(turn),
        TurnEvent::TextDelta {
            streamed: true,
            key: key.clone(),
            text: "first".into(),
        },
    ));
    assert_eq!(f.actor.streaming_entries(), (1, 1));
    f.event(turn_event(Some(turn), TurnEvent::ItemEnded { key }));
    assert_eq!(f.actor.streaming_entries(), (0, 0));
    let key = NativeItemKey("unfinished-text".into());
    f.event(turn_event(
        Some(turn),
        TurnEvent::TextDelta {
            streamed: true,
            key,
            text: "last".into(),
        },
    ));
    assert_eq!(f.actor.streaming_entries(), (1, 1));
    f.event(turn_event(
        Some(turn),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    assert_eq!(f.actor.streaming_entries(), (0, 0));
    let items = f.loaded();
    let texts: Vec<_> = items
        .items_for(f.thread)
        .filter_map(|item| match &item.body {
            ItemBody::AssistantText { text, done } => Some((text.as_str(), *done)),
            _ => None,
        })
        .collect();
    assert_eq!(texts, [("first", true), ("last", true)]);
    assert!(matches!(
        items.turns[&turn].state,
        TurnState::Ended {
            started: Some(_),
            ..
        }
    ));
}

#[test]
fn first_ready_starts_queued_turn() {
    let mut f = Fixture::new(LiveState::Starting);
    let turn = f.user("first prompt");
    f.event(ProviderEvent::Session(SessionEvent::Ready {
        native: NativeSessionId("conversation-ready".into()),
        cli_version: Some("test-one".into()),
        pid: 23,
        pgid: 23,
        capabilities: vec![],
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
#[cfg(debug_assertions)]
fn clean_exit_before_ready_fails_oldest_turn_without_respawning() {
    before_ready_exit("exit 0", "claude exited unexpectedly (exit code 0)");
}
#[test]
#[cfg(debug_assertions)]
fn stdout_close_before_ready_fails_oldest_turn_without_respawning() {
    before_ready_exit(
        "trap 'exit 0' TERM; exec 1>&-; /bin/sleep 30 & wait",
        "claude exited unexpectedly (exit code 0)",
    );
}
#[cfg(debug_assertions)]
struct Scratch(std::path::PathBuf);
#[cfg(debug_assertions)]
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[cfg(debug_assertions)]
fn stand_in(script: &str) -> (Scratch, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("veyra-before-ready-{}", ThreadId::new()));
    std::fs::create_dir(&root).unwrap();
    let scratch = Scratch(root);
    let binary = scratch.0.join("fake-cli");
    std::fs::write(
        &binary,
        format!("#!/bin/sh\nprintf 'started\\n' >> spawn-marker\nread -r initialize\n{script}\n"),
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
    (scratch, binary)
}
#[cfg(debug_assertions)]
fn before_ready_exit(script: &str, message: &str) {
    use std::{fs, time::Duration};

    let (scratch, binary) = stand_in(script);
    let root = &scratch.0;
    let mut env = env::ChildEnv::load();
    env.claude_bin = Some(binary);
    env.fixtures = None;
    env.record = None;
    let mut f = Fixture::new(LiveState::Parked);
    let mut c = ChangeSet::default();
    let mut thread = f.loaded().threads[&f.thread].clone();
    thread.cwd = root.clone();
    c.threads.insert(thread.id, thread);
    f.save(c);
    let (out, mut messages) = mpsc::channel(256);
    f.actor = Actor::new(f.actor.db, out, Spawner::Cli(env)).unwrap();
    let first = match f
        .command(Command::Send {
            thread: f.thread,
            text: "first".into(),
        })
        .unwrap()
    {
        Ack::Turn(id) => id,
        ack => panic!("unexpected ack: {ack:?}"),
    };
    let second = f.user("second");
    let exited = f.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), messages.recv())
            .await
            .unwrap()
            .unwrap()
    });
    {
        let _entered = f.runtime.enter();
        f.actor.handle(exited);
    }
    let rows = f.loaded();
    let epoch = f.actor.live[&f.thread].epoch;
    let parked = matches!(f.actor.live[&f.thread].live, Live::Parked);
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    f.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            while !matches!(f.actor.live[&f.thread].live, Live::Parked) {
                f.actor.handle(messages.recv().await.unwrap());
            }
        })
        .await
        .unwrap();
    });
    let spawns = fs::read_to_string(root.join("spawn-marker")).unwrap();
    assert!(
        matches!(
            &rows.turns[&first].state,
            TurnState::Ended { outcome: TurnOutcome::Failed { message: text }, started: None, .. }
                if text == message
        ),
        "oldest queued turn: {:?}",
        rows.turns[&first].state
    );
    assert_eq!(rows.turns[&second].state, TurnState::Queued);
    assert_eq!(rows.threads[&f.thread].hold, Some(HoldReason::Stopped));
    assert!(parked);
    assert_eq!(epoch, 1);
    assert_eq!(spawns, "started\n");
}
#[test]
fn pre_ready_exit_without_crash_fails_and_holds_queue() {
    for hold in [None, Some(HoldReason::Restarted)] {
        let mut f = Fixture::new(LiveState::Starting);
        f.hold(hold);
        let first = f.user("first");
        let second = f.user("second");
        f.event(ProviderEvent::Session(SessionEvent::Exited { crash: None }));
        let rows = f.loaded();
        assert!(matches!(
            &rows.turns[&first].state,
            TurnState::Ended { outcome: TurnOutcome::Failed { message }, started: None, .. }
                if message == "The session exited before it was ready"
        ));
        assert_eq!(rows.turns[&second].state, TurnState::Queued);
        assert_eq!(
            rows.threads[&f.thread].hold,
            hold.or(Some(HoldReason::Stopped))
        );
        assert!(matches!(f.actor.live[&f.thread].live, Live::Parked));
        assert_eq!(f.actor.live[&f.thread].epoch, 1);
    }
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
            streamed: true,
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
            streamed: true,
            key: NativeItemKey("provider-item".into()),
            text: "Hello ".into(),
        },
    ));
    f.event(turn_event(
        None,
        TurnEvent::TextDelta {
            streamed: true,
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
            streamed: true,
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
                streamed: true,
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
    #[cfg(debug_assertions)]
    assert_eq!(snapshot.delta_counts[&snapshot.items[0].id], 3);
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
            streamed: true,
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
    assert!(f.drain().iter().any(|e| matches!(
        e,
        UiEvent::Change {
            event: Ev::TurnEnded(Turn {
                state: TurnState::Ended { outcome: TurnOutcome::Failed { message }, .. }, ..
            }), ..
        } if message == "session failed"
    )));
}

#[test]
fn session_notice_attaches_to_latest_turn() {
    let mut f = Fixture::new(LiveState::Running);
    f.event(turn_event(
        f.running(),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ));
    let latest = f.user("next prompt");
    f.event(ProviderEvent::Session(SessionEvent::Notice {
        text: "new session".into(),
    }));
    let loaded = f.loaded();
    let notice = loaded
        .items
        .values()
        .find(|item| matches!(item.body, ItemBody::Notice(_)))
        .unwrap();
    assert_eq!(notice.turn, latest);
}
#[test]
#[cfg(debug_assertions)]
fn delta_count_excludes_complete_message_fallback() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running();
    f.event(turn_event(
        turn,
        TurnEvent::TextDelta {
            key: NativeItemKey("fallback".into()),
            text: "complete message".into(),
            streamed: false,
        },
    ));
    let (reply, rx) = oneshot::channel();
    {
        let _enter = f.runtime.enter();
        f.actor.handle(EngineMsg::Snapshot {
            thread: f.thread,
            reply,
        });
    }
    f.actor.handle(EngineMsg::TextFlush);
    let snapshot = rx.blocking_recv().unwrap();
    f.event(turn_event(
        turn,
        TurnEvent::TextDelta {
            key: NativeItemKey("streamed".into()),
            text: "streamed text".into(),
            streamed: true,
        },
    ));
    let (reply, rx) = oneshot::channel();
    {
        let _enter = f.runtime.enter();
        f.actor.handle(EngineMsg::Snapshot {
            thread: f.thread,
            reply,
        });
    }
    f.actor.handle(EngineMsg::TextFlush);
    let streamed = rx.blocking_recv().unwrap();
    let item = streamed
        .items
        .iter()
        .find(|item| !snapshot.items.iter().any(|old| old.id == item.id))
        .unwrap();
    assert_eq!(streamed.delta_counts[&item.id], 1);
    assert_eq!(
        snapshot
            .delta_counts
            .get(&snapshot.items[0].id)
            .copied()
            .unwrap_or(0),
        0
    );
}

#[test]
fn completed_turn_status_reports_five_milliseconds() {
    let mut f = Fixture::new(LiveState::Idle);
    f.turn(
        TurnOrigin::Provider,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            started: Some(2),
            at: 7,
        },
    );
    let event = ChangeSet::default().status_event(&f.actor.projection, &f.actor.live, f.thread);
    assert_eq!(
        event,
        Ev::ThreadStatusChanged {
            thread: f.thread,
            status: Status::Done { took: 5 }
        }
    );
}

#[test]
#[cfg(debug_assertions)]
fn archived_items_free_debug_delta_counts() {
    let mut f = Fixture::new(LiveState::Running);
    f.event(turn_event(
        f.running(),
        TurnEvent::TextDelta {
            key: NativeItemKey("counted".into()),
            text: "one delta".into(),
            streamed: true,
        },
    ));
    fn snapshot(f: &mut Fixture) -> crate::events::ThreadSnapshot {
        let (reply, rx) = oneshot::channel();
        {
            let _enter = f.runtime.enter();
            f.actor.handle(EngineMsg::Snapshot {
                thread: f.thread,
                reply,
            });
        }
        f.actor.handle(EngineMsg::TextFlush);
        rx.blocking_recv().unwrap()
    }
    let before = snapshot(&mut f);
    let item = before.items[0].id;
    assert_eq!(before.delta_counts[&item], 1);
    f.command(Command::ArchiveThread {
        thread: f.thread,
        keep_children: false,
    })
    .unwrap();
    let after = snapshot(&mut f);
    assert!(!after.delta_counts.contains_key(&item));
    assert_eq!(
        f.loaded().items[&item].body,
        ItemBody::AssistantText {
            text: "one delta".into(),
            done: true
        }
    );
}
