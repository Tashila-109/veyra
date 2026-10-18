use super::*;
use crate::live::needs_awake;

#[test]
fn an_open_terminal_does_not_hold_keep_awake() {
    let f = Fixture::new(LiveState::Terminal);
    assert!(!needs_awake(&f.actor.projection, &f.actor.live));
}

#[test]
fn needs_awake_while_running_or_starting() {
    let mut f = Fixture::new(LiveState::Idle);
    assert!(!needs_awake(&f.actor.projection, &f.actor.live));
    let turn = f.turn(TurnOrigin::Provider, TurnState::Running { since: now() });
    assert!(needs_awake(&f.actor.projection, &f.actor.live));
    f.event(turn_event(
        Some(turn),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
            park: false,
        },
    ));
    assert!(!needs_awake(&f.actor.projection, &f.actor.live));
    f.actor.live.get_mut(&f.thread).unwrap().live = Live::Starting { session: None };
    assert!(needs_awake(&f.actor.projection, &f.actor.live));
    f.actor.live.get_mut(&f.thread).unwrap().live = Live::Parked;
    assert!(!needs_awake(&f.actor.projection, &f.actor.live));
}
#[test]
fn needs_awake_while_background_work_pending() {
    let mut f = Fixture::new(LiveState::Running);
    f.event(ProviderEvent::Session(SessionEvent::BackgroundWork {
        pending: true,
    }));
    f.event(turn_event(
        f.running(),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
            park: false,
        },
    ));
    assert!(needs_awake(&f.actor.projection, &f.actor.live));
    assert!(
        f.actor
            .state
            .awake
            .load(std::sync::atomic::Ordering::Acquire)
    );
    f.event(ProviderEvent::Session(SessionEvent::BackgroundWork {
        pending: false,
    }));
    assert!(!needs_awake(&f.actor.projection, &f.actor.live));
    assert!(
        !f.actor
            .state
            .awake
            .load(std::sync::atomic::Ordering::Acquire)
    );
}
