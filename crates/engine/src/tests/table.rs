use super::*;
use crate::{
    commands,
    events::{ChangeEvent as Ev, Effect},
};

#[derive(Clone, Copy)]
enum Row {
    Send,
    SendHeld,
    Wake,
    Continue,
    Interrupt,
    Respond,
    Configure,
    Stop,
    Archive,
    Park,
}
fn cell(row: Row, state: LiveState) {
    for drop_queued in if matches!(row, Row::Continue) {
        vec![false, true]
    } else {
        vec![false]
    } {
        let mut f = Fixture::new(state);
        let mut old_user = None;
        if matches!(row, Row::SendHeld | Row::Continue) {
            f.hold(Some(HoldReason::Interrupted));
            old_user = Some(f.user("old queued message"));
        }
        let req = if matches!(row, Row::Respond) {
            Some(f.req())
        } else {
            None
        };
        let interrupt_target = if matches!(row, Row::Interrupt) {
            Some(
                f.active_turn()
                    .unwrap_or_else(|| f.user("interrupt target")),
            )
        } else {
            None
        };
        let result = match row {
            Row::Send | Row::SendHeld => f.command(Command::Send {
                thread: f.thread,
                text: "new queued message".into(),
            }),
            Row::Wake => {
                let mut c = ChangeSet::default();
                let turn = commands::queue(
                    &f.actor.projection,
                    &mut c,
                    f.thread,
                    TurnOrigin::Wake { tasks: vec![] },
                );
                commands::start_changed(&f.actor.projection, &f.actor.live, &mut c, now());
                let tx = f.actor.db.transaction().unwrap();
                store::apply(&tx, &c, now()).unwrap();
                tx.commit().unwrap();
                c.install(&mut f.actor.projection, &mut f.actor.live);
                Ok(Ack::Turn(turn))
            }
            Row::Continue => f.command(Command::Continue {
                thread: f.thread,
                drop_queued,
            }),
            Row::Interrupt => f.command(Command::Interrupt {
                thread: f.thread,
                turn: interrupt_target.unwrap(),
            }),
            Row::Respond => f.command(Command::Respond {
                request: req.unwrap(),
                decision: Decision::AllowOnce,
            }),
            Row::Configure => f.command(Command::Configure {
                thread: f.thread,
                config: ThreadConfig {
                    model: Some("model-for-next-turn".into()),
                    ..config()
                },
            }),
            Row::Stop => f.command(Command::StopThread { thread: f.thread }),
            Row::Archive => f.command(Command::ArchiveThread {
                thread: f.thread,
                keep_children: false,
            }),
            Row::Park => {
                let mut c = ChangeSet::default();
                commands::idle_park_tick(
                    &f.actor.projection,
                    &f.actor.live,
                    &mut c,
                    f.thread,
                    now() - 600001,
                    now(),
                );
                let shutdown = c.effects.iter().any(|e| {
                    matches!(
                        e,
                        Effect::Control {
                            control: SessionControl::Shutdown,
                            ..
                        }
                    )
                });
                assert_eq!(shutdown, state == LiveState::Idle);
                f.save(c);
                Ok(Ack::Applied)
            }
        };
        let refused = match row {
            Row::Interrupt => false,
            Row::Respond => state != LiveState::Running,
            Row::Configure => !matches!(state, LiveState::Parked | LiveState::Idle),
            _ => false,
        };
        assert_eq!(result.is_err(), refused, "cell state {state:?}");
        let p = f.loaded();
        let t = &p.threads[&f.thread];
        let live = &f.actor.live[&f.thread].live;
        match row {
            Row::Send | Row::SendHeld | Row::Wake => {
                let Ack::Turn(id) = result.unwrap() else {
                    panic!("turn ack")
                };
                assert_eq!(
                    matches!(p.turns[&id].state, TurnState::Running { .. }),
                    state == LiveState::Idle && !matches!(row, Row::SendHeld)
                );
                if state == LiveState::Parked {
                    assert_eq!(
                        matches!(live, Live::Starting { .. }),
                        !matches!(row, Row::SendHeld)
                    );
                }
                if matches!(row, Row::SendHeld) {
                    assert_eq!(t.hold, Some(HoldReason::Interrupted));
                }
                if matches!(row, Row::Send | Row::SendHeld) {
                    assert!(p.items.values().any(|i| i.turn == id
                        && i.body == ItemBody::UserText("new queued message".into())));
                }
            }
            Row::Continue => {
                assert_eq!(t.hold, None);
                let old = &p.turns[&old_user.unwrap()];
                assert_eq!(
                    matches!(
                        old.state,
                        TurnState::Ended {
                            outcome: TurnOutcome::Interrupted { .. },
                            ..
                        }
                    ),
                    drop_queued
                );
                if !drop_queued && state == LiveState::Idle {
                    assert!(matches!(old.state, TurnState::Running { .. }));
                }
            }
            Row::Interrupt => {
                if state != LiveState::Running {
                    assert!(f.controls.try_recv().is_err());
                }
                if state != LiveState::Running {
                    assert_eq!(p.turns[&interrupt_target.unwrap()].state, TurnState::Queued);
                    assert!(match state {
                        LiveState::Parked => matches!(live, Live::Parked),
                        LiveState::Starting => matches!(live, Live::Starting { .. }),
                        LiveState::Idle => matches!(live, Live::Structured { draining: None, .. }),
                        LiveState::Stopping => matches!(
                            live,
                            Live::Stopping {
                                then: AfterStop::Park
                            }
                        ),
                        LiveState::Running => unreachable!(),
                    });
                }
                assert_eq!(
                    t.hold,
                    if state == LiveState::Running {
                        Some(HoldReason::Interrupted)
                    } else {
                        None
                    }
                );
                if state == LiveState::Running {
                    assert_eq!(f.running(), None);
                    assert_eq!(
                        f.controls.try_recv().unwrap(),
                        SessionControl::Interrupt {
                            turn: f
                                .actor
                                .projection
                                .turns_for(f.thread)
                                .find(|t| t.origin == TurnOrigin::Provider)
                                .unwrap()
                                .id,
                            reason: StopReason::User
                        }
                    );
                }
            }
            Row::Respond => {
                assert_eq!(
                    p.requests[&req.unwrap()].state,
                    if refused {
                        RequestState::Pending
                    } else {
                        RequestState::Answered(Decision::AllowOnce)
                    }
                );
                if !refused {
                    assert_eq!(
                        f.controls.try_recv().unwrap(),
                        SessionControl::Answer {
                            native: NativeRequestRef("approval-one".into()),
                            reply: Decision::AllowOnce
                        }
                    );
                }
            }
            Row::Configure => {
                assert_eq!(
                    t.config.model,
                    if refused {
                        None
                    } else {
                        Some("model-for-next-turn".into())
                    }
                );
                if state == LiveState::Idle {
                    assert!(matches!(
                        live,
                        Live::Stopping {
                            then: AfterStop::Park
                        }
                    ));
                    assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Shutdown);
                }
            }
            Row::Stop | Row::Archive => {
                assert_eq!(t.hold, Some(HoldReason::Stopped));
                assert_eq!(t.archived, matches!(row, Row::Archive));
                assert_eq!(f.running(), None);
                if matches!(state, LiveState::Starting | LiveState::Stopping) {
                    assert!(matches!(
                        live,
                        Live::Stopping {
                            then: AfterStop::Park
                        }
                    ));
                }
                if matches!(row, Row::Archive) {
                    assert!(f.drain().iter().any(|e| matches!(
                        e,
                        UiEvent::Change {
                            event: Ev::ThreadArchived { .. },
                            ..
                        }
                    )));
                }
            }
            Row::Park => assert_eq!(
                matches!(
                    live,
                    Live::Stopping {
                        then: AfterStop::Park
                    }
                ),
                matches!(state, LiveState::Idle | LiveState::Stopping)
            ),
        }
    }
}
macro_rules! cells {
    ($row:ident; $($name:ident $state:ident),+) => {$ (#[test] fn $name() {cell(Row::$row,LiveState::$state);})+};
}
cells!(Send;
    table_send_parked Parked,
    table_send_starting Starting,
    table_send_idle Idle,
    table_send_running Running,
    table_send_stopping Stopping
);
cells!(SendHeld;
    table_send_held_parked Parked,
    table_send_held_starting Starting,
    table_send_held_idle Idle,
    table_send_held_running Running,
    table_send_held_stopping Stopping
);
cells!(Wake;
    table_wake_parked Parked,
    table_wake_starting Starting,
    table_wake_idle Idle,
    table_wake_running Running,
    table_wake_stopping Stopping
);
cells!(Continue;
    table_continue_parked Parked,
    table_continue_starting Starting,
    table_continue_idle Idle,
    table_continue_running Running,
    table_continue_stopping Stopping
);
cells!(Interrupt;
    table_interrupt_parked Parked,
    table_interrupt_starting Starting,
    table_interrupt_idle Idle,
    table_interrupt_running Running,
    table_interrupt_stopping Stopping
);
cells!(Respond;
    table_respond_parked Parked,
    table_respond_starting Starting,
    table_respond_idle Idle,
    table_respond_running Running,
    table_respond_stopping Stopping
);
cells!(Configure;
    table_configure_parked Parked,
    table_configure_starting Starting,
    table_configure_idle Idle,
    table_configure_running Running,
    table_configure_stopping Stopping
);
cells!(Stop;
    table_stop_parked Parked,
    table_stop_starting Starting,
    table_stop_idle Idle,
    table_stop_running Running,
    table_stop_stopping Stopping
);
cells!(Archive;
    table_archive_parked Parked,
    table_archive_starting Starting,
    table_archive_idle Idle,
    table_archive_running Running,
    table_archive_stopping Stopping
);
cells!(Park;
    table_park_parked Parked,
    table_park_starting Starting,
    table_park_idle Idle,
    table_park_running Running,
    table_park_stopping Stopping
);
