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
            park: false,
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
            park: false,
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
            park: false,
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
            park: false,
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
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: f.active_turn().unwrap(),
    })
    .unwrap();
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
                park: false,
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
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn: TurnId::new(),
    }));
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
            park: false,
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
            park: false,
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
            park: false,
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
            park: false,
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
fn provider_stop_tail_opens_no_turn() {
    let mut f = Fixture::new(LiveState::Idle);
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn: TurnId::new(),
    }));
    f.event(turn_event(None, TurnEvent::Started));
    let first = f.running().unwrap();
    f.event(turn_event(
        None,
        TurnEvent::TextDelta {
            key: NativeItemKey("provider".into()),
            text: "Hello ".into(),
            streamed: true,
        },
    ));
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: f.active_turn().unwrap(),
    })
    .unwrap();
    f.event(turn_event(
        None,
        TurnEvent::TextDelta {
            key: NativeItemKey("provider".into()),
            text: "dropped tail".into(),
            streamed: true,
        },
    ));
    assert_eq!(
        f.loaded()
            .turns_for(f.thread)
            .filter(|t| t.origin == TurnOrigin::Provider)
            .count(),
        1
    );
    let Ack::Turn(second) = f
        .command(Command::Send {
            thread: f.thread,
            text: "next".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(f.loaded().turns[&second].state, TurnState::Queued);
    assert_eq!(
        wait_reason(
            &ChangeSet::default()
                .view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap()
        ),
        Some(WaitReason::Stopping)
    );
    f.event(turn_event(
        None,
        TurnEvent::TurnEnded {
            park: false,
            outcome: TurnOutcome::Completed,
        },
    ));
    assert_eq!(f.running(), Some(second));
    assert_eq!(
        f.controls.try_recv().unwrap(),
        SessionControl::Interrupt {
            turn: first,
            reason: StopReason::User
        }
    );
    assert_eq!(
        f.controls.try_recv().unwrap(),
        SessionControl::StartTurn {
            turn: second,
            text: "next".into()
        }
    );
    assert!(matches!(&f.loaded().turns[&first].state, TurnState::Ended {
        outcome: TurnOutcome::Interrupted { reason }, ..
    } if reason == "Stopped by you"));
    assert!(f.loaded().items.values().any(|item| matches!(&item.body,
        ItemBody::AssistantText { text, .. } if text == "Hello ")));
}
#[test]
fn text_deltas_open_no_transaction() {
    fn stream(parts: usize) -> (usize, String) {
        let mut f = Fixture::new(LiveState::Running);
        let mut count = 0;
        for _ in 0..parts {
            let turn = f.running();
            f.event(turn_event(
                turn,
                TurnEvent::TextDelta {
                    key: NativeItemKey("stream".into()),
                    text: "x".repeat(200 / parts),
                    streamed: true,
                },
            ));
            count = f.actor.test_commits;
        }
        f.event(turn_event(
            f.running(),
            TurnEvent::TurnEnded {
                park: false,
                outcome: TurnOutcome::Completed,
            },
        ));
        let text = f
            .loaded()
            .items
            .values()
            .find_map(|i| match &i.body {
                ItemBody::AssistantText { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap();
        (count, text)
    }
    let many = stream(200);
    let one = stream(1);
    assert_eq!(many.0, one.0);
    assert_eq!(many.1, "x".repeat(200));
    assert_eq!(many.1, one.1);
}

#[cfg(debug_assertions)]
mod cli {
    use super::*;

    #[test]
    fn clean_exit_before_ready_fails_oldest_turn_without_respawning() {
        before_ready_exit("exit 0", "claude exited unexpectedly (exit code 0)");
    }

    #[test]
    fn stdout_close_before_ready_fails_oldest_turn_without_respawning() {
        before_ready_exit(
            "trap 'exit 0' TERM; exec 1>&-; /bin/sleep 30 & wait",
            "claude exited unexpectedly (exit code 0)",
        );
    }

    fn stand_in(script: &str) -> (Scratch, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("veyra-before-ready-{}", ThreadId::new()));
        std::fs::create_dir(&root).unwrap();
        let scratch = Scratch(root);
        let binary = scratch.0.join("fake-cli");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf 'started\\n' >> spawn-marker\nread -r initialize\n{script}\n"
            ),
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

    struct CliFixture {
        f: Fixture,
        messages: mpsc::Receiver<EngineMsg>,
        scratch: Scratch,
    }

    impl CliFixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("veyra-stop-{}", ThreadId::new()));
            std::fs::create_dir(&root).unwrap();
            let source =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude");
            for (fixture, target) in [(name, "t1-s1.jsonl"), ("ok-reply", "t1-s2.jsonl")] {
                std::fs::copy(source.join(format!("{fixture}.jsonl")), root.join(target)).unwrap();
            }
            let mut env = env::ChildEnv::load();
            env.claude_bin = Some(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/veyra-fake-cli")
                    .canonicalize()
                    .unwrap(),
            );
            env.fixtures = Some(root.clone());
            env.record = None;
            let mut f = Fixture::new(LiveState::Parked);
            let mut c = ChangeSet::default();
            let mut thread = f.loaded().threads[&f.thread].clone();
            thread.cwd = root.clone();
            c.threads.insert(thread.id, thread);
            f.save(c);
            let (out, messages) = mpsc::channel(256);
            f.actor = Actor::new(f.actor.db, out, Spawner::Cli(env)).unwrap();
            Self {
                f,
                messages,
                scratch: Scratch(root),
            }
        }
        fn send(&mut self, text: &str) -> TurnId {
            match self
                .f
                .command(Command::Send {
                    thread: self.f.thread,
                    text: text.into(),
                })
                .unwrap()
            {
                Ack::Turn(id) => id,
                other => panic!("unexpected ack {other:?}"),
            }
        }
        fn step(&mut self) {
            self.f.runtime.block_on(async {
                let message =
                    tokio::time::timeout(std::time::Duration::from_secs(10), self.messages.recv())
                        .await
                        .unwrap()
                        .unwrap();
                self.f.actor.handle(message);
            });
        }
        fn until(&mut self, predicate: impl Fn(&Fixture) -> bool) {
            let started = std::time::Instant::now();
            while !predicate(&self.f) {
                assert!(started.elapsed() < std::time::Duration::from_secs(10));
                self.step();
            }
        }
        fn has_text(f: &Fixture, text: &str) -> bool {
            f.loaded().items.values().any(|item| {
                matches!(&item.body,
            ItemBody::AssistantText { text: value, .. } if value == text)
            })
        }
        fn pid(&self) -> u32 {
            match self.f.actor.live[&self.f.thread].live {
                Live::Structured { pid, .. } => pid,
                _ => panic!("not structured"),
            }
        }
        fn finish(&mut self) {
            self.until(|f| matches!(f.actor.live[&f.thread].live, Live::Parked));
            assert!(self.scratch.0.is_dir());
        }
    }

    fn assistant_error_queue(name: &str, account: bool, eof: bool) {
        assistant_error_queue_with(name, account, eof, false);
    }

    fn assistant_error_queue_with(name: &str, account: bool, eof: bool, alive: bool) {
        let mut run = CliFixture::new(name);
        if alive {
            let path = run.scratch.0.join("t1-s1.jsonl");
            let mut rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let result = rows
                .iter_mut()
                .find(|row| row["line"]["type"] == "result")
                .unwrap();
            result["line"]["is_error"] = true.into();
            result["line"]["terminal_reason"] = "api_error".into();
            let exit = rows.pop().unwrap();
            rows.push(serde_json::json!({"type":"emit_inbound", "line":{
                "type":"command_lifecycle", "command_uuid":"<id:prompt>", "state":"cancelled"
            }}));
            rows.push(serde_json::json!({"type":"expect_outbound", "line":{
                "type":"user", "message":{"content":"must never arrive"}
            }}));
            rows.push(exit);
            let text = rows
                .iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(path, text + "\n").unwrap();
        }
        let first = run.send("first");
        run.until(|f| {
            CliFixture::has_text(
                f,
                if account {
                    "account error"
                } else {
                    "server error"
                },
            )
        });
        let first_pid = run.pid();
        let second = run.send("second");
        run.until(|f| matches!(f.loaded().turns[&first].state, TurnState::Ended { .. }));
        if account || eof {
            assert!(matches!(
                run.f.actor.live[&run.f.thread].live,
                Live::Stopping {
                    then: AfterStop::Park
                }
            ));
        }
        run.until(|f| f.running() == Some(second));
        let second_pid = run.pid();
        run.until(|f| matches!(f.loaded().turns[&second].state, TurnState::Ended { .. }));
        run.finish();
        let rows = run.f.loaded();
        let expected = if account {
            "Claude is signed out. Sign in from a terminal with `claude`, then send again."
        } else {
            "Claude had a server error. Send again to retry."
        };
        assert!(matches!(&rows.turns[&first].state, TurnState::Ended {
        outcome: TurnOutcome::Failed { message }, ..
    } if message == expected));
        assert!(matches!(
            rows.turns[&second].state,
            TurnState::Ended {
                outcome: TurnOutcome::Completed,
                ..
            }
        ));
        assert!(CliFixture::has_text(&run.f, "ok"));
        assert_eq!(first_pid == second_pid, !account && !eof);
    }

    #[test]
    fn assistant_account_error_parks_before_queued_turn() {
        assistant_error_queue("assistant-error-signed-out", true, false);
    }

    #[test]
    fn assistant_account_error_live_shape_retires_idle_process() {
        assistant_error_queue_with("assistant-error-signed-out", true, false, true);
    }

    #[test]
    fn assistant_error_result_never_ends_queued_turn() {
        assistant_error_queue("assistant-error-queued", false, false);
    }

    #[test]
    fn assistant_error_eof_retires_before_queued_turn() {
        assistant_error_queue("assistant-error-eof", false, true);
    }

    #[test]
    fn stop_after_account_error_retires_session() {
        let mut run = CliFixture::new("assistant-error-stop");
        let first = run.send("first");
        run.until(|f| CliFixture::has_text(f, "account error"));
        let first_pid = run.pid();
        let second = run.send("second");
        run.f
            .command(Command::Interrupt {
                thread: run.f.thread,
                turn: first,
            })
            .unwrap();
        run.until(|f| {
            matches!(
                f.actor.live[&f.thread].live,
                Live::Stopping {
                    then: AfterStop::Park
                }
            )
        });
        run.finish();
        run.f
            .command(Command::Continue {
                thread: run.f.thread,
                drop_queued: false,
            })
            .unwrap();
        run.until(|f| f.running() == Some(second));
        assert_ne!(run.pid(), first_pid);
        run.until(|f| matches!(f.loaded().turns[&second].state, TurnState::Ended { .. }));
        run.finish();
        assert!(
            matches!(&run.f.loaded().turns[&first].state, TurnState::Ended {
        outcome: TurnOutcome::Interrupted { reason }, ..
    } if reason == "Stopped by you")
        );
        assert!(matches!(
            run.f.loaded().turns[&second].state,
            TurnState::Ended {
                outcome: TurnOutcome::Completed,
                ..
            }
        ));
    }

    fn send_during_stop(name: &str, ignored: bool) {
        let mut run = CliFixture::new(name);
        let first = run.send("first");
        run.until(|f| CliFixture::has_text(f, "partial"));
        let first_pid = run.pid();
        let stopped = std::time::Instant::now();
        run.f
            .command(Command::Interrupt {
                thread: run.f.thread,
                turn: first,
            })
            .unwrap();
        let second = run.send("second");
        assert_eq!(run.f.loaded().turns[&second].state, TurnState::Queued);
        assert_eq!(
            wait_reason(
                &ChangeSet::default()
                    .view(&run.f.actor.projection, &run.f.actor.live, run.f.thread)
                    .unwrap()
            ),
            Some(WaitReason::Stopping)
        );
        if ignored {
            run.until(|f| {
                matches!(
                    f.actor.live[&f.thread].live,
                    Live::Stopping {
                        then: AfterStop::Park
                    }
                )
            });
            assert!(stopped.elapsed() < std::time::Duration::from_secs(5));
        }
        run.until(|f| f.running() == Some(second));
        assert_eq!(run.pid() == first_pid, !ignored);
        run.until(|f| matches!(f.loaded().turns[&second].state, TurnState::Ended { .. }));
        run.finish();
        assert!(
            matches!(&run.f.loaded().turns[&first].state, TurnState::Ended {
        outcome: TurnOutcome::Interrupted { reason }, ..
    } if reason == "Stopped by you")
        );
        assert!(matches!(
            run.f.loaded().turns[&second].state,
            TurnState::Ended {
                outcome: TurnOutcome::Completed,
                ..
            }
        ));
        assert!(CliFixture::has_text(&run.f, "ok"));
    }

    #[test]
    fn send_after_stop_waits_for_stopped_result() {
        send_during_stop("stop-then-send", false);
    }

    #[test]
    fn send_after_ignored_stop_runs_on_fresh_process() {
        send_during_stop("stop-never-ends", true);
    }
}

#[test]
fn subagent_output_updates_running_tail_and_caps_characters() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running().unwrap();
    let key = NativeItemKey("agent-call".into());
    f.event(turn_event(
        Some(turn),
        TurnEvent::ToolStarted {
            key: key.clone(),
            call: ToolCall {
                name: "Agent".into(),
                arguments: serde_json::json!({}),
                input_summary: None,
            },
        },
    ));
    f.event(turn_event(
        Some(turn),
        TurnEvent::ToolOutput {
            key: key.clone(),
            text: "é".repeat(10001),
        },
    ));
    let rows = f.loaded();
    let item = rows
        .items_for(f.thread)
        .find(|item| matches!(item.body, ItemBody::Tool(_)))
        .unwrap();
    let ItemBody::Tool(tool) = &item.body else {
        panic!()
    };
    assert!(tool.result.is_none());
    assert_eq!(
        serde_json::to_value(tool).unwrap()["output_tail"],
        "é".repeat(10000)
    );
    assert!(f.drain().iter().any(|event| {
        matches!(
            event,
            UiEvent::Change { event: Ev::ItemAdded(row), .. }
                if row.id == item.id && row.body == item.body
        )
    }));
    f.event(turn_event(
        Some(turn),
        TurnEvent::ToolOutput {
            key: key.clone(),
            text: "end".into(),
        },
    ));
    let rows = f.loaded();
    let ItemBody::Tool(tool) = &rows.items[&item.id].body else {
        panic!()
    };
    assert!(tool.result.is_none());
    assert_eq!(
        serde_json::to_value(tool).unwrap()["output_tail"],
        "é".repeat(9997) + "end"
    );
    f.event(turn_event(
        Some(turn),
        TurnEvent::ToolFinished {
            key,
            ok: true,
            output_tail: Some("done".into()),
        },
    ));
    let rows = f.loaded();
    let ItemBody::Tool(tool) = &rows.items[&item.id].body else {
        panic!()
    };
    assert!(tool.result.as_ref().unwrap().ok);
    assert_eq!(
        tool.result.as_ref().unwrap().output_tail.as_deref(),
        Some(("é".repeat(9993) + "enddone").as_str())
    );
    let mut legacy = serde_json::to_value(tool).unwrap();
    legacy.as_object_mut().unwrap().remove("output_tail");
    let legacy: ToolItem = serde_json::from_value(legacy).unwrap();
    assert!(legacy.output_tail.is_empty());
    assert_eq!(legacy.result, tool.result);
}

#[test]
fn late_stop_of_completed_user_prompt_leaves_provider_turn_unstopped() {
    use crate::provider::claude::Codec;
    use serde_json::json;
    let mut f = Fixture::new(LiveState::Idle);
    let Ok(Ack::Turn(first)) = f.command(Command::Send {
        thread: f.thread,
        text: "first".into(),
    }) else {
        panic!("Send failed")
    };
    assert!(
        matches!(f.controls.try_recv().unwrap(), SessionControl::StartTurn { turn, .. } if turn == first)
    );
    let mut codec = Codec::new(
        NativeSessionId("owned-test".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    codec.start(first, "first".into());
    let old_end = codec
        .value(
            json!({"type":"result", "subtype":"success", "is_error":false,
        "user_message_uuid":first.to_string()}),
        )
        .unwrap();
    let later = codec
        .value(
            json!({"type":"assistant", "message":{"id":"later-provider", "content":[
        {"type":"text", "text":"background follow-up"}]}}),
        )
        .unwrap();
    assert!(later.events.iter().any(|event| matches!(
        event,
        ProviderEvent::Turn {
            turn: None,
            event: TurnEvent::TextDelta { .. }
        }
    )));
    assert_eq!(f.running(), Some(first));
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: f.active_turn().unwrap(),
    })
    .unwrap();
    let SessionControl::Interrupt { turn, reason } = f.controls.try_recv().unwrap() else {
        panic!("missing Interrupt")
    };
    let replies = codec
        .interrupt_lines(turn, reason)
        .iter()
        .map(|line| line.value())
        .collect::<Vec<_>>();
    for event in old_end
        .events
        .into_iter()
        .chain(later.provider_turn.map(|provider_turn| {
            ProviderEvent::Session(SessionEvent::ProviderTurnStarted { provider_turn })
        }))
        .chain(later.events)
    {
        f.event(event);
    }
    let next = f.running().unwrap();
    assert_ne!(first, next);
    assert_eq!(f.actor.projection.turns[&next].origin, TurnOrigin::Provider);
    let next_end = codec
        .value(json!({"type":"result", "subtype":"success", "is_error":false}))
        .unwrap();
    assert!(replies.is_empty());
    for event in next_end.events {
        f.event(event);
    }
    assert!(
        matches!(
            f.loaded().turns[&next].state,
            TurnState::Ended {
                outcome: TurnOutcome::Completed,
                ..
            }
        ),
        "Stop for the completed user prompt interrupted the later Provider turn: {:?}",
        f.loaded().turns[&next].state
    );
}

#[test]
fn provider_interrupt_targets_provider_turn_and_preserves_cancel_order() {
    use crate::provider::claude::Codec;
    use serde_json::json;
    let mut f = Fixture::new(LiveState::Idle);
    let mut codec = Codec::new(
        NativeSessionId("owned-test".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    let first = codec
        .value(json!({"type":"assistant", "message":{"id":"first-provider",
        "content":[{"type":"text", "text":"first"}]}}))
        .unwrap();
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn: first.provider_turn.unwrap(),
    }));
    for event in first.events {
        f.event(event);
    }
    let first = f.running().unwrap();
    let first_provider_turn = first;
    let old_end = codec
        .value(json!({"type":"result", "subtype":"success", "is_error":false}))
        .unwrap();
    let later = codec
        .value(json!({"type":"assistant", "message":{"id":"next-provider",
        "content":[{"type":"text", "text":"next"}]}}))
        .unwrap();
    let mut events = vec![ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn: later.provider_turn.unwrap(),
    })];
    events.extend(later.events);
    for id in ["approval-b", "approval-a"] {
        events.extend(codec.value(json!({"type":"control_request", "request_id":id,
            "request":{"subtype":"can_use_tool", "tool_name":"Bash", "input":{"command":"echo ok"}}
        })).unwrap().events);
    }
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: f.active_turn().unwrap(),
    })
    .unwrap();
    let SessionControl::Interrupt { turn, reason } = f.controls.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!(turn, first_provider_turn);
    assert!(codec.interrupt_lines(turn, reason).is_empty());
    assert!(codec.interrupt_deadline().is_none());
    for event in old_end.events.into_iter().chain(events) {
        f.event(event);
    }
    let next = f.running().unwrap();
    assert_ne!(first, next);
    assert_ne!(first_provider_turn, next);
    assert_eq!(
        f.loaded()
            .requests
            .values()
            .filter(|r| r.state == RequestState::Pending)
            .count(),
        2
    );
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: f.active_turn().unwrap(),
    })
    .unwrap();
    let SessionControl::Interrupt { turn, reason } = f.controls.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!(turn, next);
    assert_eq!(
        codec
            .interrupt_lines(turn, reason)
            .iter()
            .map(|line| line.value())
            .collect::<Vec<_>>(),
        vec![
            json!({"type":"control_response", "response":{"subtype":"success", "request_id":"approval-a",
            "response":{"behavior":"deny", "message":"Stopped by you", "interrupt":true}}}),
            json!({"type":"control_response", "response":{"subtype":"success", "request_id":"approval-b",
            "response":{"behavior":"deny", "message":"Stopped by you", "interrupt":true}}}),
            json!({"type":"control_request", "request_id":"veyra-interrupt", "request":{"subtype":"interrupt"}}),
        ]
    );
    assert!(codec.interrupt_deadline().is_some());
    assert!(matches!(&f.loaded().turns[&next].state, TurnState::Ended {
        outcome: TurnOutcome::Interrupted { reason }, ..
    } if reason == "Stopped by you"));
}

fn queued_prompt_waits_for_provider_followup(defer_admission: bool) {
    use crate::provider::claude::Codec;
    use serde_json::json;
    let mut f = Fixture::new(LiveState::Idle);
    let Ok(Ack::Turn(first)) = f.command(Command::Send {
        thread: f.thread,
        text: "first".into(),
    }) else {
        panic!()
    };
    assert!(
        matches!(f.controls.try_recv().unwrap(), SessionControl::StartTurn { turn, .. } if turn == first)
    );
    let mut codec = Codec::new(
        NativeSessionId("owned-test".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    assert!(codec.start(first, "first".into()).is_some());
    let Ok(Ack::Turn(next)) = f.command(Command::Send {
        thread: f.thread,
        text: "queued user prompt".into(),
    }) else {
        panic!()
    };
    let ordinal = f.loaded().turns[&next].ordinal;
    let old_end = codec
        .value(
            json!({"type":"result", "subtype":"success", "is_error":false,
        "user_message_uuid":first.to_string()}),
        )
        .unwrap();
    let followup = codec
        .value(
            json!({"type":"assistant", "message":{"id":"provider-followup", "content":[
        {"type":"text", "text":"provider followup before queued prompt"}]}}),
        )
        .unwrap();
    let provider = followup.provider_turn.unwrap();
    for event in old_end.events {
        f.event(event);
    }
    assert_eq!(f.running(), Some(next));
    let SessionControl::StartTurn { turn, text } = f.controls.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!(turn, next);
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn: provider,
    }));
    for event in followup.events {
        f.event(event);
    }
    assert_eq!(f.running(), Some(provider));
    assert_eq!(f.loaded().turns[&next].state, TurnState::Queued);
    assert_eq!(f.loaded().turns[&next].ordinal, ordinal);
    assert_eq!(
        wait_reason(
            &ChangeSet::default()
                .view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap()
        ),
        Some(WaitReason::TurnRunning)
    );
    if !defer_admission {
        assert!(codec.start(turn, text.clone()).is_none());
        f.event(ProviderEvent::Session(SessionEvent::StartTurnRefused {
            turn,
        }));
    }
    let followup_end = codec
        .value(json!({"type":"result", "subtype":"success", "is_error":false}))
        .unwrap();
    for event in followup_end.events {
        f.event(event);
    }
    if defer_admission {
        assert!(f.running().is_none());
        assert!(
            f.controls.try_recv().is_err(),
            "wait for the outstanding admission before retrying"
        );
    } else {
        assert_eq!(f.running(), Some(next));
        assert_eq!(
            f.controls.try_recv().unwrap(),
            SessionControl::StartTurn {
                turn: next,
                text: text.clone()
            }
        );
    }
    assert!(codec.start(next, text).is_some());
    for event in codec
        .value(
            json!({"type":"assistant", "message":{"id":"queued-reply", "content":[
        {"type":"text", "text":"actual queued reply"}]}}),
        )
        .unwrap()
        .events
    {
        f.event(event);
    }
    assert_eq!(f.running(), Some(next));
    for event in codec
        .value(
            json!({"type":"result", "subtype":"success", "is_error":false,
        "user_message_uuid":next.to_string()}),
        )
        .unwrap()
        .events
    {
        f.event(event);
    }
    let rows = f.loaded();
    for id in [first, provider, next] {
        assert!(matches!(
            rows.turns[&id].state,
            TurnState::Ended {
                outcome: TurnOutcome::Completed,
                ..
            }
        ));
    }
    assert!(rows.items_for(f.thread).any(|item| item.turn == provider
        && matches!(&item.body,
        ItemBody::AssistantText { text, .. } if text == "provider followup before queued prompt")));
    assert!(rows.items_for(f.thread).any(|item| item.turn == next
        && matches!(&item.body,
        ItemBody::AssistantText { text, .. } if text == "actual queued reply")));
    assert!(f.controls.try_recv().is_err());
}

#[test]
fn queued_prompt_does_not_take_provider_followup_result() {
    queued_prompt_waits_for_provider_followup(false);
}

#[test]
fn provider_end_before_pending_admission_does_not_send_duplicate_prompt() {
    queued_prompt_waits_for_provider_followup(true);
}

#[test]
fn refused_stopped_prompt_does_not_strand_provider_turn() {
    use crate::provider::claude::Codec;
    use serde_json::json;
    let mut f = Fixture::new(LiveState::Idle);
    let Ok(Ack::Turn(stopped)) = f.command(Command::Send {
        thread: f.thread,
        text: "pending".into(),
    }) else {
        panic!()
    };
    f.controls.try_recv().unwrap();
    let mut codec = Codec::new(
        NativeSessionId("owned-test".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    let provider = codec
        .value(
            json!({"type":"assistant", "message":{"id":"provider", "content":[
        {"type":"text", "text":"CLI work"}]}}),
        )
        .unwrap();
    let provider_turn = provider.provider_turn.unwrap();
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: f.active_turn().unwrap(),
    })
    .unwrap();
    let SessionControl::Interrupt { turn, reason } = f.controls.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!(turn, stopped);
    assert!(codec.interrupt_lines(turn, reason).is_empty());
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn,
    }));
    for event in provider.events {
        f.event(event);
    }
    assert_eq!(f.running(), Some(provider_turn));
    assert!(codec.start(stopped, "pending".into()).is_none());
    f.event(ProviderEvent::Session(SessionEvent::StartTurnRefused {
        turn: stopped,
    }));
    let Ok(Ack::Turn(next)) = f.command(Command::Send {
        thread: f.thread,
        text: "next".into(),
    }) else {
        panic!()
    };
    assert_eq!(f.loaded().turns[&next].state, TurnState::Queued);
    for event in codec
        .value(json!({"type":"result", "subtype":"success", "is_error":false}))
        .unwrap()
        .events
    {
        f.event(event);
    }
    assert_eq!(f.running(), Some(next));
    assert_eq!(
        f.controls.try_recv().unwrap(),
        SessionControl::StartTurn {
            turn: next,
            text: "next".into()
        }
    );
    assert!(matches!(
        f.loaded().turns[&stopped].state,
        TurnState::Ended {
            outcome: TurnOutcome::Interrupted { .. },
            ..
        }
    ));
}

fn admitted_while_queued() -> (Fixture, crate::provider::claude::Codec, TurnId) {
    let mut f = Fixture::new(LiveState::Idle);
    let Ok(Ack::Turn(first)) = f.command(Command::Send {
        thread: f.thread,
        text: "first".into(),
    }) else {
        panic!()
    };
    f.controls.try_recv().unwrap();
    use crate::provider::claude::Codec;
    use serde_json::json;
    let mut codec = Codec::new(
        NativeSessionId("owned-probe".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    assert!(codec.start(first, "first".into()).is_some());
    let Ok(Ack::Turn(next)) = f.command(Command::Send {
        thread: f.thread,
        text: "queued".into(),
    }) else {
        panic!()
    };
    let old_end = codec.value(json!({
        "type":"result", "subtype":"success", "is_error":false, "user_message_uuid":first.to_string()
    })).unwrap();
    let followup = codec.value(json!({
        "type":"assistant", "message":{"id":"pf", "content":[{"type":"text", "text":"provider"}]}
    })).unwrap();
    let provider = followup.provider_turn.unwrap();
    for e in old_end.events {
        f.event(e);
    }
    let SessionControl::StartTurn { turn, text } = f.controls.try_recv().unwrap() else {
        panic!()
    };
    assert_eq!(turn, next);
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn: provider,
    }));
    for e in followup.events {
        f.event(e);
    }
    let followup_end = codec
        .value(json!({"type":"result","subtype":"success","is_error":false}))
        .unwrap();
    assert!(codec.start(next, text).is_some());
    for e in followup_end.events {
        f.event(e);
    }
    assert!(f.running().is_none());
    assert_eq!(f.loaded().turns[&next].state, TurnState::Queued);
    (f, codec, next)
}

fn apply_admission_controls(
    f: &mut Fixture,
    codec: &mut crate::provider::claude::Codec,
) -> Vec<serde_json::Value> {
    let mut lines = vec![];
    while let Ok(control) = f.controls.try_recv() {
        eprintln!("control={control:?}");
        match control {
            SessionControl::Interrupt { turn, reason } => lines.extend(
                codec
                    .interrupt_lines(turn, reason)
                    .iter()
                    .map(|line| line.value())
                    .collect::<Vec<_>>(),
            ),
            SessionControl::InterruptActive(reason) => lines.extend(
                codec
                    .interrupt_active(reason)
                    .iter()
                    .map(|line| line.value())
                    .collect::<Vec<_>>(),
            ),
            _ => {}
        }
    }
    lines
}

#[test]
fn stop_reaches_admitted_requeued_prompt() {
    let (mut f, mut codec, next) = admitted_while_queued();
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    let lines = apply_admission_controls(&mut f, &mut codec);
    eprintln!("next_state={:?}", f.loaded().turns[&next].state);
    eprintln!("codec_lines={lines:?}");
    assert_eq!(
        lines,
        vec![serde_json::json!({
            "type":"control_request", "request_id":"veyra-interrupt", "request":{"subtype":"interrupt"}
        })]
    );
    assert!(matches!(&f.loaded().turns[&next].state, TurnState::Ended {
        outcome: TurnOutcome::Interrupted { reason }, ..
    } if reason == "Stopped by you"));
}

#[test]
fn interrupt_keeps_admitted_requeued_prompt_held() {
    let (mut f, mut codec, next) = admitted_while_queued();
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: f.active_turn().unwrap(),
    })
    .unwrap();
    let lines = apply_admission_controls(&mut f, &mut codec);
    eprintln!("codec_lines={lines:?}");
    for e in codec.value(serde_json::json!({
        "type":"assistant", "message":{"id":"qr", "content":[{"type":"text", "text":"queued reply"}]}
    })).unwrap().events {
        f.event(e);
    }
    eprintln!(
        "hold={:?} next_state={:?}",
        f.loaded().threads[&f.thread].hold,
        f.loaded().turns[&next].state
    );
    assert!(
        !lines.is_empty()
            && matches!(
                f.loaded().turns[&next].state,
                TurnState::Ended {
                    outcome: TurnOutcome::Interrupted { .. },
                    ..
                }
            ),
        "Interrupt sent nothing, then C started running under an Interrupted hold"
    );
}

#[test]
fn refusal_waits_for_active_turn_end() {
    let (mut f, mut codec, _next) = admitted_while_queued();
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    apply_admission_controls(&mut f, &mut codec);
    let Ok(Ack::Turn(d)) = f.command(Command::Send {
        thread: f.thread,
        text: "after stop".into(),
    }) else {
        panic!()
    };
    assert!(f.controls.try_recv().is_err());
    for e in codec.value(serde_json::json!({
        "type":"assistant", "message":{"id":"qr", "content":[{"type":"text", "text":"discarded prompt runs"}]}
    })).unwrap().events {
        f.event(e);
    }
    assert!(!f.loaded().items_for(f.thread).any(|item| {
        matches!(&item.body, ItemBody::AssistantText { text, .. } if text == "discarded prompt runs")
    }));
    let mut starts = 0;
    for _ in 0..5 {
        let Ok(SessionControl::StartTurn { turn, text }) = f.controls.try_recv() else {
            break;
        };
        assert_eq!(turn, d);
        starts += 1;
        assert!(codec.start(turn, text).is_none());
        f.event(ProviderEvent::Session(SessionEvent::StartTurnRefused {
            turn,
        }));
    }
    eprintln!("start_turn_dispatches={starts}");
    assert!(
        starts <= 1,
        "refusal immediately re-dispatched StartTurn {starts} times"
    );
}

#[test]
fn pending_admission_blocks_configuration_and_view_switch() {
    let (mut f, _, _) = admitted_while_queued();
    for command in [
        Command::Configure {
            thread: f.thread,
            config: config(),
        },
        Command::SwitchView {
            thread: f.thread,
            to: View::Terminal,
        },
    ] {
        assert!(matches!(
            f.command(command),
            Err(CommandError::Busy(WaitReason::TurnRunning))
        ));
        assert!(f.controls.try_recv().is_err());
    }
}

#[test]
fn pending_admission_is_busy_for_quit_awake_and_idle_parking() {
    let (mut f, _, next) = admitted_while_queued();
    let changes = ChangeSet::default();
    let view = changes
        .view(&f.actor.projection, &f.actor.live, f.thread)
        .unwrap();
    assert!(crate::live::busy(&view));
    assert!(!crate::live::park_due(&view, 0, 10 * 60 * 1000));
    assert!(crate::live::needs_awake(&f.actor.projection, &f.actor.live));
    assert!(f.actor.state.window_threads.lock().unwrap()[&f.thread].busy);
    assert!(f.actor.state.window_threads.lock().unwrap()[&f.thread].turn_busy);
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    let rows = f.loaded();
    assert_eq!(rows.threads[&f.thread].hold, Some(HoldReason::Restarted));
    assert!(matches!(
        rows.turns[&next].state,
        TurnState::Ended {
            outcome: TurnOutcome::Interrupted { .. },
            ..
        }
    ));
}

#[test]
fn refused_prompt_waits_for_cli_end() {
    let mut f = Fixture::new(LiveState::Idle);
    let Ok(Ack::Turn(first)) = f.command(Command::Send {
        thread: f.thread,
        text: "first".into(),
    }) else {
        panic!()
    };
    assert!(
        matches!(f.controls.try_recv().unwrap(), SessionControl::StartTurn { turn, .. } if turn == first)
    );
    f.event(ProviderEvent::Session(SessionEvent::StartTurnRefused {
        turn: first,
    }));
    assert_eq!(f.loaded().turns[&first].state, TurnState::Queued);
    assert!(
        f.controls.try_recv().is_err(),
        "refusal immediately resent the prompt"
    );
    f.command(Command::Send {
        thread: f.thread,
        text: "second".into(),
    })
    .unwrap();
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Tick);
    assert!(
        f.controls.try_recv().is_err(),
        "send or tick retried before the CLI ended"
    );
    f.event(turn_event(
        None,
        TurnEvent::TurnEnded {
            park: false,
            outcome: TurnOutcome::Completed,
        },
    ));
    assert!(matches!(f.controls.try_recv().unwrap(),
            SessionControl::StartTurn { turn, text } if turn == first && text == "first"));
}

#[test]
fn unannounced_provider_content_opens_no_turn() {
    let mut f = Fixture::new(LiveState::Idle);
    f.event(turn_event(
        None,
        TurnEvent::TextDelta {
            key: NativeItemKey("unannounced".into()),
            text: "dropped".into(),
            streamed: true,
        },
    ));
    assert!(f.loaded().turns.is_empty());
    let provider_turn = TurnId::new();
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn,
    }));
    f.event(turn_event(
        None,
        TurnEvent::TextDelta {
            key: NativeItemKey("announced".into()),
            text: "kept".into(),
            streamed: true,
        },
    ));
    f.event(turn_event(
        None,
        TurnEvent::TurnEnded {
            park: false,
            outcome: TurnOutcome::Completed,
        },
    ));
    let rows = f.loaded();
    assert_eq!(rows.turns.len(), 1);
    assert!(
        rows.items_for(f.thread)
            .any(|item| item.turn == provider_turn
                && matches!(&item.body, ItemBody::AssistantText { text, .. } if text == "kept"))
    );
}

#[test]
fn session_exit_ends_running_and_pending_admission_rows() {
    let (mut f, _, pending) = admitted_while_queued();
    let mut changes = ChangeSet::default();
    let mut provider = f
        .loaded()
        .turns_for(f.thread)
        .find(|turn| turn.origin == TurnOrigin::Provider)
        .unwrap()
        .clone();
    provider.state = TurnState::Running { since: now() };
    let provider_id = provider.id;
    changes.turns.insert(provider.id, provider);
    f.save(changes);
    f.event(ProviderEvent::Session(SessionEvent::Exited {
        crash: Some("owned session failed".into()),
    }));
    let rows = f.loaded();
    for id in [provider_id, pending] {
        assert!(matches!(&rows.turns[&id].state,
            TurnState::Ended { outcome: TurnOutcome::Failed { message }, .. } if message == "owned session failed"));
    }
    assert!(f.controls.try_recv().is_err());
}

fn open_foreign_lifecycle(
    f: &mut Fixture,
    codec: &mut crate::provider::claude::Codec,
    command: &str,
    state: &str,
) -> TurnId {
    let decoded = codec
        .value(serde_json::json!({"type":"command_lifecycle",
            "command_uuid":command, "state":state}))
        .unwrap();
    let turn = decoded.provider_turn.unwrap();
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn: turn,
    }));
    for event in decoded.events {
        f.event(event);
    }
    assert_eq!(f.running(), Some(turn));
    assert_eq!(f.loaded().turns[&turn].origin, TurnOrigin::Provider);
    turn
}

fn completed_lifecycle(state: &str) {
    use crate::provider::claude::Codec;
    use serde_json::json;
    let mut f = Fixture::new(LiveState::Idle);
    let first = match f
        .command(Command::Send {
            thread: f.thread,
            text: "first".into(),
        })
        .unwrap()
    {
        Ack::Turn(turn) => turn,
        _ => panic!(),
    };
    let mut codec = Codec::new(
        NativeSessionId("lifecycle-correlation".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    codec.start(first, "first".into()).unwrap();
    let accepted = codec
        .value(json!({"type":"command_lifecycle",
            "command_uuid":first.to_string(), "state":"started"}))
        .unwrap();
    for event in accepted.events {
        f.event(event);
    }
    let ended = codec
        .value(
            json!({"type":"result", "subtype":"success", "is_error":false,
            "user_message_uuid":first.to_string()}),
        )
        .unwrap();
    for event in ended.events {
        f.event(event);
    }
    assert!(f.running().is_none());
    let stale = codec
        .value(json!({"type":"command_lifecycle",
            "command_uuid":first.to_string(), "state":state}))
        .unwrap();
    if let Some(provider_turn) = stale.provider_turn {
        f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
            provider_turn,
        }));
    }
    for event in stale.events {
        f.event(event);
    }
    assert!(
        f.running().is_none(),
        "late {state} for completed A opened phantom {:?}",
        f.running()
    );
    open_foreign_lifecycle(&mut f, &mut codec, "fresh-cli-work", state);
}

#[test]
fn completed_prompt_late_queued_cannot_open_a_provider_turn() {
    completed_lifecycle("queued");
}

#[test]
fn completed_prompt_late_started_cannot_open_a_provider_turn() {
    completed_lifecycle("started");
}

#[test]
fn foreign_lifecycle_still_opens_a_provider_turn() {
    let mut f = Fixture::new(LiveState::Idle);
    let mut codec = crate::provider::claude::Codec::new(
        NativeSessionId("lifecycle-correlation".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    open_foreign_lifecycle(&mut f, &mut codec, "cli-owned-work", "started");
}

fn completed_provider_lifecycle(state: &str) {
    use crate::provider::claude::Codec;
    use serde_json::json;
    let mut f = Fixture::new(LiveState::Idle);
    let mut codec = Codec::new(
        NativeSessionId("lifecycle-correlation".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    let command = "11111111-1111-4111-8111-111111111111";
    let first = open_foreign_lifecycle(&mut f, &mut codec, command, "started");
    let end = codec
        .value(
            json!({"type":"result", "subtype":"success", "is_error":false,
            "origin":{"kind":"background"}}),
        )
        .unwrap();
    for event in end.events {
        f.event(event);
    }
    assert!(codec.active.is_none());
    assert!(f.running().is_none());
    assert!(matches!(
        f.loaded().turns[&first].state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    ));
    let late = codec
        .value(json!({"type":"command_lifecycle", "command_uuid":command, "state":state}))
        .unwrap();
    let reopened = late.provider_turn;
    if let Some(provider_turn) = reopened {
        f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
            provider_turn,
        }));
    }
    for event in late.events {
        f.event(event);
    }
    assert!(
        reopened.is_none(),
        "late lifecycle reopened completed CLI work"
    );
    assert!(f.running().is_none());
    assert_eq!(
        f.loaded()
            .turns_for(f.thread)
            .filter(|turn| turn.origin == TurnOrigin::Provider)
            .count(),
        1
    );
    let fresh = open_foreign_lifecycle(
        &mut f,
        &mut codec,
        "22222222-2222-4222-8222-222222222222",
        state,
    );
    assert_ne!(first, fresh);
    for event in codec
        .value(json!({"type":"result", "subtype":"success", "is_error":false}))
        .unwrap()
        .events
    {
        f.event(event);
    }
    let user = match f
        .command(Command::Send {
            thread: f.thread,
            text: "next user prompt".into(),
        })
        .unwrap()
    {
        Ack::Turn(turn) => turn,
        _ => panic!(),
    };
    assert!(matches!(
        f.loaded().turns[&user].state,
        TurnState::Running { .. }
    ));
    assert!(
        matches!(f.controls.try_recv().unwrap(), SessionControl::StartTurn { turn, .. } if turn == user)
    );
}
#[test]
fn completed_provider_late_started_opens_no_phantom_turn() {
    completed_provider_lifecycle("started");
}
#[test]
fn completed_provider_late_queued_opens_no_phantom_turn() {
    completed_provider_lifecycle("queued");
}

#[test]
fn completed_provider_new_foreign_command_still_opens() {
    let mut f = Fixture::new(LiveState::Idle);
    let mut codec = crate::provider::claude::Codec::new(
        NativeSessionId("lifecycle-correlation".into()),
        9,
        std::path::Path::new("/tmp"),
    );
    let first = open_foreign_lifecycle(
        &mut f,
        &mut codec,
        "11111111-1111-4111-8111-111111111111",
        "started",
    );
    for event in codec
        .value(serde_json::json!({"type":"result", "subtype":"success", "is_error":false}))
        .unwrap()
        .events
    {
        f.event(event);
    }
    assert!(codec.active.is_none());
    assert!(f.running().is_none());
    let next = open_foreign_lifecycle(
        &mut f,
        &mut codec,
        "22222222-2222-4222-8222-222222222222",
        "started",
    );
    assert_ne!(first, next);
}

#[test]
fn refused_admission_has_no_running_or_pending_stop_target() {
    let mut f = Fixture::new(LiveState::Idle);
    let Ok(Ack::Turn(turn)) = f.command(Command::Send {
        thread: f.thread,
        text: "refused prompt".into(),
    }) else {
        panic!()
    };
    f.event(ProviderEvent::Session(SessionEvent::StartTurnRefused {
        turn,
    }));
    assert_eq!(
        f.actor.live[&f.thread].admission,
        Some(crate::live::TurnAdmission::Refused)
    );
    assert!(f.running().is_none());
    let published = f.actor.state.window_threads.lock().unwrap()[&f.thread];
    assert!(published.turn_busy);
    assert_eq!(published.pending_turn, None);
}
