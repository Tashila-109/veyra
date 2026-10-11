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
