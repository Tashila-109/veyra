#![allow(clippy::disallowed_types)] // Request tests construct provider wire input and tool output.
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
            tool: None,
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
                cwd: Default::default(),
                reason: None,
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

#[test]
fn async_choice_precedes_queued_messages_and_persists_once() {
    let mut f = Fixture::new(LiveState::Running);
    f.command(Command::Send {
        thread: f.thread,
        text: "already queued".into(),
    })
    .unwrap();
    f.event(turn_event(
        f.running(),
        TurnEvent::AsyncQuestions {
            key: NativeItemKey("async-question".into()),
            text: "Which colour should Veyra use?".into(),
            questions: vec![AsyncQuestion {
                title: "Colour".into(),
                options: vec!["Violet".into(), "Blue".into()],
                selected: None,
            }],
        },
    ));
    let item = f
        .loaded()
        .items_for(f.thread)
        .find(|i| matches!(i.body, ItemBody::AsyncQuestions { .. }))
        .unwrap()
        .id;
    let Ack::Turn(answer) = f
        .command(Command::ChooseAsyncOption {
            item,
            question: 0,
            option: 0,
        })
        .unwrap()
    else {
        panic!("choice did not queue a user turn")
    };
    let p = f.loaded();
    assert_eq!(p.threads[&f.thread].hold, None);
    let mut queued: Vec<_> = p
        .turns_for(f.thread)
        .filter(|t| t.state == TurnState::Queued)
        .collect();
    queued.sort_by_key(|t| t.ordinal);
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].id, answer);
    assert!(matches!(queued[0].origin, TurnOrigin::User { .. }));
    assert!(
        p.items_for(f.thread)
            .any(|i| i.turn == answer && i.body == ItemBody::UserText("Violet".into()))
    );
    assert!(
        matches!(&p.items[&item].body, ItemBody::AsyncQuestions { text, questions } if text == "Which colour sh\
            ould Veyra use?" && questions[0].selected == Some(0))
    );
    assert!(
        std::iter::from_fn(|| f.controls.try_recv().ok()).any(|control| matches!(
            control,
            SessionControl::Interrupt {
                reason: StopReason::QuestionChoice,
                ..
            }
        ))
    );
    assert!(
        f.command(Command::ChooseAsyncOption {
            item,
            question: 0,
            option: 1
        })
        .is_err()
    );
    assert_eq!(f.loaded().turns_for(f.thread).count(), 3);
}

#[test]
fn later_typed_user_message_closes_async_card_choices() {
    let mut f = Fixture::new(LiveState::Running);
    f.event(turn_event(
        f.running(),
        TurnEvent::AsyncQuestions {
            key: NativeItemKey("typed-question".into()),
            text: "Choose a colour".into(),
            questions: vec![AsyncQuestion {
                title: "Colour".into(),
                options: vec!["Violet".into(), "Blue".into()],
                selected: None,
            }],
        },
    ));
    let item = f
        .loaded()
        .items_for(f.thread)
        .find(|i| matches!(i.body, ItemBody::AsyncQuestions { .. }))
        .unwrap()
        .id;
    f.command(Command::Send {
        thread: f.thread,
        text: "Blue, please".into(),
    })
    .unwrap();
    assert!(
        f.command(Command::ChooseAsyncOption {
            item,
            question: 0,
            option: 0
        })
        .is_err(),
        "a typed answer left stale controls active"
    );
    assert_eq!(f.loaded().turns_for(f.thread).count(), 2);
    assert!(
        !std::iter::from_fn(|| f.controls.try_recv().ok())
            .any(|control| matches!(control, SessionControl::Interrupt { .. }))
    );
}

#[test]
fn sanitized_option_display_does_not_change_the_user_message() {
    let mut f = Fixture::new(LiveState::Running);
    let exact = "Vi\u{202e}olet\nnext";
    f.event(turn_event(
        f.running(),
        TurnEvent::AsyncQuestions {
            key: NativeItemKey("exact-option".into()),
            text: "Colour".into(),
            questions: vec![AsyncQuestion {
                title: "Colour".into(),
                options: vec![exact.into()],
                selected: None,
            }],
        },
    ));
    let item = f
        .loaded()
        .items_for(f.thread)
        .find(|item| matches!(item.body, ItemBody::AsyncQuestions { .. }))
        .unwrap()
        .id;
    let Ack::Turn(answer) = f
        .command(Command::ChooseAsyncOption {
            item,
            question: 0,
            option: 0,
        })
        .unwrap()
    else {
        panic!("choice did not queue user text")
    };
    assert!(
        f.loaded()
            .items_for(f.thread)
            .any(|item| item.turn == answer && item.body == ItemBody::UserText(exact.into()))
    );
}

#[test]
fn invocation_approval_links_survive_both_event_orders_and_repeated_commands() {
    for request_first in [false, true] {
        let mut f = Fixture::new(LiveState::Running);
        for (index, decision) in [Decision::Deny, Decision::AllowOnce]
            .into_iter()
            .enumerate()
        {
            let key = NativeItemKey(format!("invocation-{index}"));
            let started = TurnEvent::ToolStarted {
                key: key.clone(),
                call: ToolCall {
                    name: "Bash".into(),
                    veyra_label: false,
                    arguments: json!({"command":"same command"}),
                    input_summary: None,
                },
            };
            let opened = TurnEvent::RequestOpened {
                tool: Some(key.clone()),
                native: NativeRequestRef(format!("approval-{index}")),
                kind: RequestKind::Approval {
                    action: ProposedAction::new(
                        "Bash".into(),
                        ApprovalTarget::Command {
                            command: "same command".into(),
                            cwd: CommandCwd::ProjectFolder,
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    ),
                },
            };
            let events = if request_first {
                [opened, started]
            } else {
                [started, opened]
            };
            for event in events {
                f.event(turn_event(f.running(), event));
            }
            let loaded = f.loaded();
            let request = loaded
                .requests
                .values()
                .find(|request| request.native.0 == format!("approval-{index}"))
                .unwrap()
                .id;
            f.command(Command::Respond {
                request,
                decision: decision.clone(),
            })
            .unwrap();
            f.event(turn_event(
                f.running(),
                TurnEvent::ToolFinished {
                    key,
                    ok: false,
                    output_tail: Some("failed independently".into()),
                },
            ));
            let loaded = f.loaded();
            assert_eq!(
                loaded.requests[&request].state,
                RequestState::Answered(decision)
            );
            let linked: Vec<_> = loaded
                .items_for(f.thread)
                .filter_map(|item| match &item.body {
                    ItemBody::Tool(tool) if tool.approval == Some(request) => Some(tool),
                    _ => None,
                })
                .collect();
            assert_eq!(linked.len(), 1, "each decision belongs to one invocation");
            assert!(!linked[0].result.as_ref().unwrap().ok);
        }
        let loaded = f.loaded();
        let links: std::collections::BTreeSet<_> = loaded
            .items_for(f.thread)
            .filter_map(|item| match &item.body {
                ItemBody::Tool(tool) => tool.approval,
                _ => None,
            })
            .collect();
        assert_eq!(links.len(), 2);
    }
}

#[test]
fn command_reason_is_codec_owned_at_construction_and_hydrated_on_legacy_loading() {
    let target = ApprovalTarget::Command {
        command: "printf check".into(),
        cwd: CommandCwd::ProjectFolder,
        reason: None,
        preview: ApprovalPreview::new(r#"{"description":"Explain this request"}"#.into()),
    };
    let action = ProposedAction::new("Bash".into(), target);
    let ApprovalTarget::Command { reason, .. } = &action.target else {
        unreachable!()
    };
    assert!(reason.is_none());
    let mut stored = serde_json::to_value(&action).unwrap();
    stored["target"].as_object_mut().unwrap().remove("reason");
    let loaded: ProposedAction = serde_json::from_value(stored).unwrap();
    let ApprovalTarget::Command { reason, .. } = loaded.target else {
        unreachable!()
    };
    assert_eq!(reason.unwrap().preview.text, "Explain this request");
}
