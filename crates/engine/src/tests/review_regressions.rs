use super::*;
use crate::provider::claude::Codec;
use serde_json::{Value, json};

fn request(id: &str, tool: &str, input: Value) -> Value {
    json!({"type":"control_request", "request_id":id,
        "request":{"subtype":"can_use_tool", "tool_name":tool, "input":input}})
}
fn question(text: &str) -> Value {
    json!({"question":text, "options":[{"label":"No"}, {"label":"Yes"}]})
}
fn codec(f: &Fixture) -> Codec {
    let mut codec = Codec::new(NativeSessionId("regression".into()), 9);
    codec.start(f.running().unwrap(), "first".into());
    codec
}
#[test]
fn grant_does_not_answer_commands_with_extra_fields() {
    let mut f = Fixture::new(LiveState::Running);
    let mut c = ChangeSet::default();
    c.grants.push((
        f.thread,
        GrantKey {
            tool: "Bash".into(),
            command: "echo safe".into(),
        },
    ));
    f.save(c);
    let mut codec = codec(&f);
    let plain = codec
        .value(request(
            "plain",
            "Bash",
            json!({
        "command":"echo safe", "description":"safe", "timeout":1000}),
        ))
        .unwrap();
    for event in plain.events {
        f.event(event);
    }
    assert!(matches!(
        f.controls.try_recv().unwrap(),
        SessionControl::Answer {
            reply: Decision::AllowOnce,
            ..
        }
    ));
    for field in ["dangerouslyDisableSandbox", "run_in_background"] {
        let mut input = json!({"command":"echo safe"});
        input[field] = json!(true);
        let opened = codec.value(request(field, "Bash", input)).unwrap();
        assert!(matches!(
            &opened.events[0],
            ProviderEvent::Turn {
                event: TurnEvent::RequestOpened {
                    kind: RequestKind::Approval {
                        grant_key: None,
                        ..
                    },
                    ..
                },
                ..
            }
        ));
        for event in opened.events {
            f.event(event);
        }
        assert!(
            f.controls.try_recv().is_err(),
            "extra input must dock without the grant"
        );
    }
    assert_eq!(
        f.loaded()
            .requests
            .values()
            .filter(|r| r.state == RequestState::Pending)
            .count(),
        2
    );
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
fn orderly_shutdown_persists_restarted_hold_before_reconcile() {
    for queued in [false, true] {
        let mut f = Fixture::new(LiveState::Running);
        if queued {
            f.user("later");
        }
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Shutdown);
        let mut persisted = f.loaded();
        assert_eq!(
            persisted.threads[&f.thread].hold,
            Some(HoldReason::Restarted)
        );
        let mut live = persisted
            .threads
            .keys()
            .map(|&id| (id, LiveThread::default()))
            .collect();
        crate::reconcile::decide(&persisted, now()).install(&mut persisted, &mut live);
        assert_eq!(
            persisted.threads[&f.thread].hold,
            Some(HoldReason::Restarted)
        );
        let view = ChangeSet::default();
        let view = view.view(&persisted, &live, f.thread).unwrap();
        assert_eq!(
            wait_reason(&view),
            Some(WaitReason::Held(HoldReason::Restarted))
        );
        assert_eq!(thread_status(&view), Status::Held);
    }
}
#[test]
fn shutdown_expires_requests_with_closing_reason() {
    let mut f = Fixture::new(LiveState::Running);
    let request = f.req();
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    assert_eq!(
        f.loaded().requests[&request].state,
        RequestState::Expired {
            reason: "Veyra is closing".into()
        }
    );
}
