use super::grants::approval;
use super::*;

#[test]
fn request_expires_on_turn_end() {
    let mut f = Fixture::new(LiveState::Running);
    let request = approval(&mut f, "Bash", "touch pending");
    f.event(turn_event(
        f.running(),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
            park: false,
        },
    ));
    assert_eq!(
        f.loaded().requests[&request].state,
        RequestState::Expired {
            reason: "The turn ended".into()
        }
    );
    let events = f.drain();
    let close = events
        .iter()
        .position(|e| {
            matches!(e, UiEvent::Change {
        event: crate::events::ChangeEvent::RequestClosed(r), .. } if r.id == request)
        })
        .unwrap();
    let end = events
        .iter()
        .position(|e| {
            matches!(
                e,
                UiEvent::Change {
                    event: crate::events::ChangeEvent::TurnEnded(_),
                    ..
                }
            )
        })
        .unwrap();
    assert!(close < end);
}
#[test]
fn request_expires_on_exit() {
    let mut f = Fixture::new(LiveState::Running);
    let request = approval(&mut f, "Bash", "touch pending");
    f.event(ProviderEvent::Session(SessionEvent::Exited { crash: None }));
    assert!(matches!(
        f.loaded().requests[&request].state,
        RequestState::Expired { .. }
    ));
    assert!(matches!(
        f.command(Command::Respond {
            request,
            decision: Decision::AllowOnce
        }),
        Err(CommandError::RequestClosed)
    ));
}
#[test]
fn question_answer_is_persisted_and_requires_every_answer() {
    let mut f = Fixture::new(LiveState::Running);
    f.event(turn_event(
        f.running(),
        TurnEvent::RequestOpened {
            native: NativeRequestRef("question".into()),
            kind: RequestKind::Question {
                questions: vec![Question {
                    text: "Choose a route".into(),
                    options: vec!["Short".into(), "Long".into()],
                }],
            },
        },
    ));
    let request = *f.actor.projection.requests.keys().next().unwrap();
    assert!(matches!(
        f.command(Command::Respond {
            request,
            decision: Decision::Answers(vec![])
        }),
        Err(CommandError::Invalid(_))
    ));
    assert_eq!(f.loaded().requests[&request].state, RequestState::Pending);
    let answers = vec![Answer {
        text: "Short".into(),
    }];
    f.command(Command::Respond {
        request,
        decision: Decision::Answers(answers.clone()),
    })
    .unwrap();
    assert_eq!(
        f.loaded().requests[&request].state,
        RequestState::Answered(Decision::Answers(answers))
    );
    assert!(
        f.loaded()
            .items_for(f.thread)
            .any(|item| item.body == ItemBody::UserText("Choose a route: Short".into()))
    );
}
