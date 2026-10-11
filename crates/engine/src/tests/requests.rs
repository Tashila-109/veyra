use super::grants::approval;
use super::*;
use crate::provider::claude::Codec;
use serde_json::{Value, json};

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

pub(super) fn request(id: &str, tool: &str, input: Value) -> Value {
    json!({"type":"control_request", "request_id":id,
        "request":{"subtype":"can_use_tool", "tool_name":tool, "input":input}})
}
fn question(text: &str) -> Value {
    json!({"question":text, "options":[{"label":"No"}, {"label":"Yes"}]})
}
pub(super) fn codec(f: &Fixture) -> Codec {
    let mut codec = Codec::new(
        NativeSessionId("regression".into()),
        9,
        std::path::Path::new("/project"),
    );
    codec.start(f.running().unwrap(), "first".into());
    codec
}

#[test]
fn reused_native_id_cannot_redirect_delayed_answer_or_grant() {
    for automatic in [false, true] {
        for as_question in [false, true] {
            let mut f = Fixture::new(LiveState::Running);
            if automatic {
                let mut c = ChangeSet::default();
                c.grants.push((
                    f.thread,
                    GrantKey {
                        tool: "Bash".into(),
                        command: "echo safe".into(),
                    },
                ));
                f.save(c);
            }
            let mut codec = codec(&f);
            for event in codec
                .value(request("reused", "Bash", json!({"command":"echo safe"})))
                .unwrap()
                .events
            {
                f.event(event);
            }
            let old_request = *f.actor.projection.requests.keys().next().unwrap();
            let turn = f.running().unwrap();
            let ended = codec
                .value(
                    json!({"type":"result", "subtype":"success", "is_error":false,
                "user_message_uuid":turn.to_string()}),
                )
                .unwrap();
            let (tool, input) = if as_question {
                (
                    "AskUserQuestion",
                    json!({"questions":[question("Proceed?")]}),
                )
            } else {
                ("Bash", json!({"command":"touch unapproved"}))
            };
            let replacement = codec.value(request("reused", tool, input)).unwrap();
            if !automatic {
                f.command(Command::Respond {
                    request: old_request,
                    decision: Decision::AllowOnce,
                })
                .unwrap();
            }
            let SessionControl::Answer { native, reply } = f.controls.try_recv().unwrap() else {
                panic!("answer");
            };
            assert!(
                codec.answer(native, reply).is_none(),
                "delayed reply must not authorize B"
            );
            assert!(replacement.events.is_empty());
            assert_eq!(replacement.replies[0]["response"]["subtype"], "error");
            for event in ended.events {
                f.event(event);
            }
        }
    }
}

#[test]
fn mismatched_codec_decision_keeps_pending_request() {
    let f = Fixture::new(LiveState::Running);
    let mut codec = codec(&f);
    codec
        .value(request(
            "question",
            "AskUserQuestion",
            json!({"questions":[question("Proceed?")]}),
        ))
        .unwrap();
    for decision in [
        Decision::AllowOnce,
        Decision::AllowInThread,
        Decision::Deny,
        Decision::Answers(vec![]),
    ] {
        assert!(
            codec
                .answer(NativeRequestRef("question".into()), decision)
                .is_none()
        );
    }
    assert!(
        codec
            .answer(
                NativeRequestRef("question".into()),
                Decision::Answers(vec![Answer { text: "No".into() }])
            )
            .is_some()
    );
    codec
        .value(request("approval", "Bash", json!({"command":"echo safe"})))
        .unwrap();
    assert!(
        codec
            .answer(
                NativeRequestRef("approval".into()),
                Decision::Answers(vec![])
            )
            .is_none()
    );
    assert!(
        codec
            .answer(NativeRequestRef("approval".into()), Decision::Deny)
            .is_some()
    );
}

#[test]
fn duplicate_question_text_is_rejected_before_opening() {
    let f = Fixture::new(LiveState::Running);
    let mut codec = codec(&f);
    let decoded = codec
        .value(request(
            "duplicate",
            "AskUserQuestion",
            json!({
        "questions":[question("Proceed?"), question("Proceed?")]}),
        ))
        .unwrap();
    assert!(decoded.events.is_empty());
    assert_eq!(decoded.replies[0]["response"]["subtype"], "error");
    assert!(
        codec
            .answer(
                NativeRequestRef("duplicate".into()),
                Decision::Answers(vec![
                    Answer { text: "No".into() },
                    Answer { text: "Yes".into() }
                ])
            )
            .is_none()
    );
}

#[test]
fn legacy_approval_rows_load_as_typed_display() {
    let mut f = Fixture::new(LiveState::Running);
    let id = approval(&mut f, "Bash", "echo old");
    for (action, expected) in [
        (
            json!({"tool":"Bash","command":"echo old"}),
            ApprovalTarget::Command {
                command: "echo old".into(),
                preview: ApprovalPreview::new(String::new()),
            },
        ),
        (
            json!({"tool":"Write","command":"duplicate raw input",
            "input":{"file_path":"/tmp/x","content":"old contents"}}),
            ApprovalTarget::Tool {
                preview: ApprovalPreview::new(
                    "{\n  \"content\": \"old contents\",\n  \"file_path\": \"/tmp/x\"\n}".into(),
                ),
            },
        ),
    ] {
        let body = json!(["old-native", {"Approval":{"action":action,"grant_key":null}}]);
        f.actor
            .db
            .execute(
                "UPDATE requests SET body_json=?1 WHERE id=?2",
                rusqlite::params![body.to_string(), id.to_string()],
            )
            .unwrap();
        let loaded = f.loaded();
        let RequestKind::Approval { action, .. } = &loaded.requests[&id].kind else {
            panic!("approval")
        };
        if let ApprovalTarget::Tool { preview } = &expected {
            let ApprovalTarget::Tool { preview: actual } = &action.target else {
                panic!("tool preview")
            };
            assert_eq!(
                serde_json::from_str::<Value>(&actual.text).unwrap(),
                serde_json::from_str::<Value>(&preview.text).unwrap()
            );
            assert!(!actual.truncated);
        } else {
            assert_eq!(action.target, expected);
        }
        let stored = serde_json::to_value(action).unwrap();
        assert!(stored.get("input").is_none());
        assert!(stored.get("command").is_none());
    }
}
