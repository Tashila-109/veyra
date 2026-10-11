use super::*;

#[test]
fn interrupt_reason_names_you() {
    for command in [
        |thread| Command::Interrupt { thread },
        |thread| Command::StopThread { thread },
        |thread| Command::ArchiveThread {
            thread,
            keep_children: false,
        },
    ] {
        let mut f = Fixture::new(LiveState::Running);
        let turn = f.running().unwrap();
        let command = command(f.thread);
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
