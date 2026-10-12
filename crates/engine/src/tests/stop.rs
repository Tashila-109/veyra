use super::*;

#[test]
fn interrupt_reason_names_you() {
    for command in [
        |thread, turn| Command::Interrupt { thread, turn },
        |thread, _| Command::StopThread { thread },
        |thread, _| Command::ArchiveThread {
            thread,
            keep_children: false,
        },
    ] {
        let mut f = Fixture::new(LiveState::Running);
        let turn = f.running().unwrap();
        let command = command(f.thread, turn);
        f.command(command).unwrap();
        assert!(matches!(&f.loaded().turns[&turn].state, TurnState::Ended {
            outcome: TurnOutcome::Interrupted { reason }, ..
        } if reason == "Stopped by you"));
    }
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
fn delayed_stop_command_preserves_the_later_turn() {
    for delayed in [false, true] {
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
        f.event(turn_event(Some(first), TurnEvent::Accepted));
        let stop_for_first = Command::Interrupt {
            thread: f.thread,
            turn: first,
        };
        if delayed {
            let second = match f
                .command(Command::Send {
                    thread: f.thread,
                    text: "second".into(),
                })
                .unwrap()
            {
                Ack::Turn(turn) => turn,
                _ => panic!(),
            };
            f.event(turn_event(
                Some(first),
                TurnEvent::TurnEnded {
                    outcome: TurnOutcome::Completed,
                    park: false,
                },
            ));
            f.event(turn_event(Some(second), TurnEvent::Accepted));
            assert_eq!(f.running(), Some(second));
            f.command(stop_for_first).unwrap();
            assert!(
                matches!(f.loaded().turns[&second].state, TurnState::Running { .. }),
                "the Stop built while A ran must not end B: {:?}",
                f.loaded().turns[&second].state
            );
            assert_eq!(f.loaded().threads[&f.thread].hold, None);
            assert!(matches!(
                f.actor.live[&f.thread].live,
                Live::Structured { draining: None, .. }
            ));
            f.event(turn_event(
                Some(second),
                TurnEvent::TurnEnded {
                    outcome: TurnOutcome::Completed,
                    park: false,
                },
            ));
            assert!(matches!(
                f.loaded().turns[&second].state,
                TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    ..
                }
            ));
        } else {
            f.command(stop_for_first).unwrap();
            assert!(matches!(
                f.loaded().turns[&first].state,
                TurnState::Ended {
                    outcome: TurnOutcome::Interrupted { .. },
                    ..
                }
            ));
        }
    }
}

#[test]
fn stop_aimed_at_ended_turn_leaves_next_turn_running() {
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
    let Ok(Ack::Turn(second)) = f.command(Command::Send {
        thread: f.thread,
        text: "second".into(),
    }) else {
        panic!()
    };
    f.event(turn_event(Some(first), TurnEvent::Accepted));
    eprintln!(
        "window shows first={:?} running, human presses Stop",
        f.loaded().turns[&first].state
    );
    f.event(turn_event(
        Some(first),
        TurnEvent::TurnEnded {
            park: false,
            outcome: TurnOutcome::Completed,
        },
    ));
    let started = f.controls.try_recv();
    eprintln!("after first ends, control={started:?}");
    f.command(Command::Interrupt {
        thread: f.thread,
        turn: first,
    })
    .unwrap();
    let mut controls = vec![];
    while let Ok(control) = f.controls.try_recv() {
        controls.push(format!("{control:?}"));
    }
    let rows = f.loaded();
    eprintln!("first={:?}", rows.turns[&first].state);
    eprintln!("second={:?}", rows.turns[&second].state);
    eprintln!("controls after Stop={controls:?}");
    assert!(
        !matches!(
            rows.turns[&second].state,
            TurnState::Ended {
                outcome: TurnOutcome::Interrupted { .. },
                ..
            }
        ),
        "the Stop the human aimed at the first turn ended the second turn"
    );
    f.event(turn_event(
        Some(second),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
            park: false,
        },
    ));
    assert!(matches!(
        f.loaded().turns[&second].state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    ));
}
