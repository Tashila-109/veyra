use super::*;

#[test]
fn interrupt_reason_names_you() {
    for command in [
        Command::Interrupt {
            thread: ThreadId::new(),
        },
        Command::StopThread {
            thread: ThreadId::new(),
        },
        Command::ArchiveThread {
            thread: ThreadId::new(),
            keep_children: false,
        },
    ] {
        let mut f = Fixture::new(LiveState::Running);
        let turn = f.running().unwrap();
        let command = match command {
            Command::Interrupt { .. } => Command::Interrupt { thread: f.thread },
            Command::StopThread { .. } => Command::StopThread { thread: f.thread },
            _ => Command::ArchiveThread {
                thread: f.thread,
                keep_children: false,
            },
        };
        f.command(command).unwrap();
        assert!(matches!(&f.loaded().turns[&turn].state, TurnState::Ended {
            outcome: TurnOutcome::Interrupted { reason }, ..
        } if reason == "Stopped by you"));
    }
}
