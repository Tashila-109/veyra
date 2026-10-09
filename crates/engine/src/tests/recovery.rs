use super::*;
use crate::{commands, events::ChangeEvent as Ev, reconcile};
fn reconcile_fixture(f: &mut Fixture) {
    let c = reconcile::decide(&f.actor.projection, now());
    f.save(c);
}
#[test]
fn send_to_held_empty_queue_clears_hold() {
    let mut f = Fixture::new(LiveState::Idle);
    f.hold(Some(HoldReason::Restarted));
    let Ack::Turn(turn) = f
        .command(Command::Send {
            thread: f.thread,
            text: "my go-ahead".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(f.loaded().threads[&f.thread].hold, None);
    assert_eq!(f.running(), Some(turn));
}
#[test]
fn continue_child_without_task_does_not_hold_parent() {
    let mut f = Fixture::new(LiveState::Idle);
    let child = f.family();
    f.command(Command::Continue {
        thread: child,
        drop_queued: false,
    })
    .unwrap();
    assert_eq!(f.loaded().threads[&f.thread].hold, None);
    assert_eq!(f.loaded().threads[&child].hold, None);
}
#[test]
fn child_settlement_keeps_final_buffered_report() {
    let mut f = Fixture::new(LiveState::Idle);
    let child = f.family();
    let task = TaskId::new();
    let turn = TurnId::new();
    let mut c = ChangeSet::default();
    c.turns.insert(
        turn,
        Turn {
            id: turn,
            thread: child,
            ordinal: 1,
            origin: TurnOrigin::Task { task },
            state: TurnState::Running { since: now() },
        },
    );
    c.tasks.insert(
        task,
        Task {
            id: task,
            parent: f.thread,
            child,
            turn,
            request_key: "seeded-task".into(),
            state: TaskState::Running,
        },
    );
    c.live.insert(child, f.actor.live[&f.thread].clone());
    f.save(c);
    let child_epoch = f.actor.live[&child].epoch;
    for event in [
        TurnEvent::TextDelta {
            key: NativeItemKey("child-report".into()),
            text: "final report".into(),
        },
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
        },
    ] {
        let _enter = f.runtime.enter();
        f.actor.handle(EngineMsg::Provider {
            thread: child,
            epoch: child_epoch,
            event: turn_event(Some(turn), event),
        });
    }
    assert!(
        matches!(&f.loaded().tasks[&task].state, TaskState::Settled { summary, .. } if summary == "final report")
    );
    assert!(f.drain().iter().any(|e| matches!(
        e,
        UiEvent::Change {
            event: Ev::TaskSettled(Task { state: TaskState::Settled { summary, .. }, .. }), ..
        } if summary == "final report"
    )));
}
#[test]
fn interrupt_holds_queue() {
    let mut f = Fixture::new(LiveState::Running);
    let queued = f.user("wait for me");
    f.command(Command::Interrupt { thread: f.thread }).unwrap();
    assert_eq!(f.loaded().turns[&queued].state, TurnState::Queued);
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::Interrupted)
    );
    assert_eq!(f.running(), None);
    f.command(Command::Send {
        thread: f.thread,
        text: "still waits".into(),
    })
    .unwrap();
    assert_eq!(f.running(), None);
}
#[test]
fn reconcile_interrupts_running_turns() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running().unwrap();
    reconcile_fixture(&mut f);
    assert_eq!(
        f.loaded().turns[&turn].state,
        TurnState::Ended {
            outcome: TurnOutcome::Interrupted {
                reason: "Veyra closed while this turn ran".into()
            },
            at: f.actor.projection.turns[&turn].state.clone().ended_at()
        }
    );
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::Restarted)
    );
}
trait EndedAt {
    fn ended_at(self) -> Millis;
}
impl EndedAt for TurnState {
    fn ended_at(self) -> Millis {
        match self {
            Self::Ended { at, .. } => at,
            _ => panic!("expected ended turn"),
        }
    }
}
#[test]
fn reconcile_expires_pending_requests() {
    let mut f = Fixture::new(LiveState::Running);
    let req = f.req();
    reconcile_fixture(&mut f);
    assert!(matches!(
        f.loaded().requests[&req].state,
        RequestState::Expired { .. }
    ));
}
fn pending_delivery(f: &mut Fixture, started: bool) -> (TaskId, TurnId) {
    let child = f.family();
    let task = TaskId::new();
    let wake = f.turn(
        TurnOrigin::Wake { tasks: vec![task] },
        if started {
            TurnState::Running { since: now() }
        } else {
            TurnState::Queued
        },
    );
    let mut c = ChangeSet::default();
    c.tasks.insert(
        task,
        Task {
            id: task,
            parent: f.thread,
            child,
            turn: wake,
            request_key: "test:delivery".into(),
            state: TaskState::Settled {
                outcome: TurnOutcome::Completed,
                summary: "a child report".into(),
                delivery: Delivery::Pending { wake },
            },
        },
    );
    f.save(c);
    (task, wake)
}
#[test]
fn reconcile_requeues_unaccepted_wake() {
    let mut f = Fixture::new(LiveState::Idle);
    let (task, wake) = pending_delivery(&mut f, true);
    reconcile_fixture(&mut f);
    let p = f.loaded();
    let TaskState::Settled {
        delivery: Delivery::Pending { wake: new },
        ..
    } = p.tasks[&task].state
    else {
        panic!()
    };
    assert_ne!(new, wake);
    assert_eq!(p.turns[&new].state, TurnState::Queued);
    assert_eq!(p.turns[&new].origin, TurnOrigin::Wake { tasks: vec![task] });
    assert_eq!(p.threads[&f.thread].hold, Some(HoldReason::Restarted));
}
#[test]
fn reconcile_terminal_view_sets_import_pending() {
    let mut f = Fixture::new(LiveState::Terminal);
    reconcile_fixture(&mut f);
    let t = &f.loaded().threads[&f.thread];
    assert_eq!(t.view, View::Chat);
    assert!(t.import_pending);
    assert_eq!(t.hold, Some(HoldReason::Restarted));
}
#[test]
fn reconcile_is_idempotent() {
    let mut f = Fixture::new(LiveState::Running);
    f.req();
    pending_delivery(&mut f, true);
    reconcile_fixture(&mut f);
    let first = f.loaded();
    reconcile_fixture(&mut f);
    let second = f.loaded();
    assert_eq!(first.turns, second.turns);
    assert_eq!(first.requests, second.requests);
    assert_eq!(first.tasks, second.tasks);
    assert_eq!(first.threads, second.threads);
    assert_eq!(
        first
            .turns
            .values()
            .filter(|t| t.state == TurnState::Queued)
            .count(),
        1
    );
}
#[test]
fn continue_drop_queued_discards_user_turns() {
    let mut f = Fixture::new(LiveState::Starting);
    f.hold(Some(HoldReason::Stopped));
    let user = f.user("stale prompt");
    let task = f.turn(
        TurnOrigin::Task {
            task: TaskId::new(),
        },
        TurnState::Queued,
    );
    let wake = f.turn(TurnOrigin::Wake { tasks: vec![] }, TurnState::Queued);
    f.command(Command::Continue {
        thread: f.thread,
        drop_queued: true,
    })
    .unwrap();
    let p = f.loaded();
    assert!(matches!(
        p.turns[&user].state,
        TurnState::Ended {
            outcome: TurnOutcome::Interrupted { .. },
            ..
        }
    ));
    assert_eq!(p.turns[&task].state, TurnState::Queued);
    assert_eq!(p.turns[&wake].state, TurnState::Queued);
    assert_eq!(p.threads[&f.thread].hold, None);
}
#[test]
fn interrupt_expires_pending_requests() {
    let mut f = Fixture::new(LiveState::Running);
    let req = f.req();
    f.command(Command::Interrupt { thread: f.thread }).unwrap();
    assert!(matches!(
        f.loaded().requests[&req].state,
        RequestState::Expired { .. }
    ));
    assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Interrupt);
}
#[test]
fn stop_expires_pending_requests() {
    let mut f = Fixture::new(LiveState::Running);
    let req = f.req();
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    assert!(matches!(
        f.loaded().requests[&req].state,
        RequestState::Expired { .. }
    ));
    assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Interrupt);
}
#[test]
fn hold_reason_names_its_command() {
    for (command, reason) in [
        (0, HoldReason::Interrupted),
        (1, HoldReason::Stopped),
        (2, HoldReason::Restarted),
    ] {
        let mut f = Fixture::new(LiveState::Running);
        match command {
            0 => {
                f.command(Command::Interrupt { thread: f.thread }).unwrap();
            }
            1 => {
                f.command(Command::StopThread { thread: f.thread }).unwrap();
            }
            _ => reconcile_fixture(&mut f),
        }
        assert_eq!(f.loaded().threads[&f.thread].hold, Some(reason));
        f.command(Command::StopThread { thread: f.thread }).unwrap();
        assert_eq!(f.loaded().threads[&f.thread].hold, Some(reason));
    }
}
#[test]
fn stop_thread_cascades_children_first() {
    let mut f = Fixture::new(LiveState::Running);
    let child = f.family();
    let req = f.req();
    let mut c = ChangeSet::default();
    commands::queue_user(&f.actor.projection, &mut c, child, "child prompt".into());
    f.save(c);
    f.drain();
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    let p = f.loaded();
    assert_eq!(p.threads[&child].hold, Some(HoldReason::Stopped));
    assert!(matches!(
        p.requests[&req].state,
        RequestState::Expired { .. }
    ));
    let order: Vec<_> = f
        .drain()
        .into_iter()
        .filter_map(|e| match e {
            UiEvent::Change {
                event: Ev::ThreadHoldChanged { thread, .. },
                ..
            } => Some(thread),
            _ => None,
        })
        .collect();
    assert_eq!(order, vec![child, f.thread]);
}
#[test]
fn archive_root_cascades_to_children() {
    let mut f = Fixture::new(LiveState::Idle);
    let child = f.family();
    f.command(Command::ArchiveThread {
        thread: f.thread,
        keep_children: false,
    })
    .unwrap();
    let p = f.loaded();
    assert!(p.threads[&child].archived);
    assert!(p.threads[&f.thread].archived);
}
#[test]
fn archive_keep_children_leaves_children() {
    let mut f = Fixture::new(LiveState::Idle);
    let child = f.family();
    f.command(Command::ArchiveThread {
        thread: f.thread,
        keep_children: true,
    })
    .unwrap();
    let p = f.loaded();
    assert!(!p.threads[&child].archived);
    assert_eq!(p.threads[&child].hold, None);
    assert!(p.threads[&f.thread].archived);
    assert!(
        f.command(Command::ArchiveThread {
            thread: child,
            keep_children: true
        })
        .is_err()
    );
}
#[test]
fn root_title_from_first_prompt() {
    let mut f = Fixture::new(LiveState::Idle);
    let text = format!(" \n {}\t second \n", "α".repeat(81));
    let Ack::Thread(id) = f
        .command(Command::CreateThread {
            project: f.project,
            provider: ProviderKind::Codex,
            config: config(),
            text,
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        f.loaded().threads[&id].title,
        format!("{}…", "α".repeat(80))
    );
    let Ack::Thread(id) = f
        .command(Command::CreateThread {
            project: f.project,
            provider: ProviderKind::Codex,
            config: config(),
            text: "  first\t prompt\n here  ".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(f.loaded().threads[&id].title, "first prompt here");
}
#[test]
fn wait_reason_follows_start_rule() {
    use crate::domain::wait_reason;
    let mut f = Fixture::new(LiveState::Idle);
    f.user("queued");
    for reason in [
        HoldReason::Interrupted,
        HoldReason::Stopped,
        HoldReason::Restarted,
        HoldReason::ChildReport,
    ] {
        f.hold(Some(reason));
        assert_eq!(
            wait_reason(
                &ChangeSet::default()
                    .view(&f.actor.projection, &f.actor.live, f.thread)
                    .unwrap()
            ),
            Some(WaitReason::Held(reason))
        );
    }
    f.hold(None);
    assert_eq!(
        wait_reason(
            &ChangeSet::default()
                .view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap()
        ),
        None
    );
    for (state, reason) in [
        (LiveState::Running, WaitReason::TurnRunning),
        (LiveState::Starting, WaitReason::Starting),
        (LiveState::Terminal, WaitReason::TerminalView),
        (LiveState::Stopping, WaitReason::Stopping),
    ] {
        let f = Fixture::new(state);
        assert_eq!(
            wait_reason(
                &ChangeSet::default()
                    .view(&f.actor.projection, &f.actor.live, f.thread)
                    .unwrap()
            ),
            Some(reason)
        );
    }
    f.actor.live.get_mut(&f.thread).unwrap().background_work = true;
    assert_eq!(
        wait_reason(
            &ChangeSet::default()
                .view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap()
        ),
        Some(WaitReason::BackgroundWork)
    );
    f.actor.live.get_mut(&f.thread).unwrap().background_work = false;
    let mut c = ChangeSet::default();
    let mut t = f.actor.projection.threads[&f.thread].clone();
    t.archived = true;
    c.threads.insert(t.id, t);
    f.save(c);
    assert_eq!(
        wait_reason(
            &ChangeSet::default()
                .view(&f.actor.projection, &f.actor.live, f.thread)
                .unwrap()
        ),
        Some(WaitReason::Archived)
    );
}
