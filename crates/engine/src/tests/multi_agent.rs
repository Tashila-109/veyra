use super::*;
use serde_json::{Value, json};

fn call(f: &mut Fixture, caller: ThreadId, name: &str, arguments: Value) -> ToolResult {
    let _entered = f.runtime.enter();
    let (reply, result) = oneshot::channel();
    f.actor.handle(EngineMsg::Tool {
        caller,
        session: None,
        call: ToolCall {
            name: name.into(),
            arguments,
            input_summary: None,
        },
        reply,
    });
    result.blocking_recv().unwrap()
}
fn success(result: ToolResult) -> Value {
    assert!(
        result.ok,
        "{}",
        result.output_tail.as_deref().unwrap_or_default()
    );
    serde_json::from_str(&result.output_tail.unwrap()).unwrap()
}
fn refusal(result: ToolResult, words: &[&str]) {
    assert!(!result.ok);
    let text = result.output_tail.unwrap();
    for word in words {
        assert!(text.contains(word), "{text} lacks {word}");
    }
}
fn start(f: &mut Fixture, key: &str) -> (ThreadId, TaskId) {
    let caller = f.thread;
    let value = success(call(
        f,
        caller,
        "start_thread",
        json!({"provider":"claude","title":"Worker","prompt":"Report on the files","client_request_id":key}),
    ));
    (
        serde_json::from_value(value["thread_id"].clone()).unwrap(),
        serde_json::from_value(value["task_id"].clone()).unwrap(),
    )
}
fn task_turn(f: &Fixture, task: TaskId) -> TurnId {
    f.actor.projection.tasks[&task].turn
}
fn child_event(f: &mut Fixture, child: ThreadId, event: ProviderEvent) {
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Provider {
        thread: child,
        epoch: f.actor.live[&child].epoch,
        event,
    });
}
fn ready_child(f: &mut Fixture, child: ThreadId) {
    child_event(
        f,
        child,
        ProviderEvent::Session(SessionEvent::Ready {
            native: NativeSessionId("child-session".into()),
            cli_version: None,
            pid: 10,
            pgid: 10,
            capabilities: vec![],
        }),
    );
}
fn end_child(f: &mut Fixture, child: ThreadId, task: TaskId) {
    let turn = task_turn(f, task);
    child_event(
        f,
        child,
        turn_event(
            Some(turn),
            TurnEvent::TextDelta {
                key: NativeItemKey("report".into()),
                text: "Child report".into(),
                streamed: true,
            },
        ),
    );
    child_event(
        f,
        child,
        turn_event(
            Some(turn),
            TurnEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
                park: false,
            },
        ),
    );
}

#[test]
fn fifth_child_refused() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let children: Vec<_> = (0..4).map(|n| start(&mut f, &n.to_string()).0).collect();
    refusal(
        call(
            &mut f,
            caller,
            "start_thread",
            json!({"provider":"claude","title":"Fifth","prompt":"extra","client_request_id":"five"}),
        ),
        &[
            "4 child threads with open tasks against the limit 4",
            "stop_thread",
        ],
    );
    assert_eq!(f.loaded().tasks.len(), 4);
    assert_eq!(children.len(), 4);
}
#[test]
fn mode_ceiling_refused() {
    let mut f = Fixture::new(LiveState::Running);
    f.actor
        .projection
        .threads
        .get_mut(&f.thread)
        .unwrap()
        .config
        .mode = PermissionMode::AskFirst;
    let caller = f.thread;
    refusal(
        call(
            &mut f,
            caller,
            "start_thread",
            json!({"provider":"claude","mode":"accept_edits","title":"Worker","prompt":"x","client_request_id":"mode"}),
        ),
        &["ceiling", "ask_first", "start_thread"],
    );
    let (_, task) = start(&mut f, "legal");
    assert_eq!(
        f.loaded().threads[&f.loaded().tasks[&task].child]
            .config
            .mode,
        PermissionMode::AskFirst
    );
}
#[test]
fn full_access_parent_starts_accept_edits_child() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let value = success(call(
        &mut f,
        caller,
        "start_thread",
        json!({"provider":"claude","mode":"accept_edits","title":"Worker","prompt":"x","client_request_id":"mode"}),
    ));
    let child: ThreadId = serde_json::from_value(value["thread_id"].clone()).unwrap();
    assert_eq!(
        f.loaded().threads[&child].config.mode,
        PermissionMode::AcceptEdits
    );
}
#[test]
fn live_turn_gate_refuses_idle_caller() {
    let mut f = Fixture::new(LiveState::Idle);
    let caller = f.thread;
    refusal(
        call(&mut f, caller, "list_providers", json!({})),
        &["running turn", "list_providers"],
    );
    f.turn(TurnOrigin::Provider, TurnState::Running { since: now() });
    let result = success(call(&mut f, caller, "list_providers", json!({})));
    assert_eq!(result["remaining_fan_out"], 4);
}
#[test]
fn child_caller_refused() {
    let mut f = Fixture::new(LiveState::Running);
    let (child, _) = start(&mut f, "child");
    ready_child(&mut f, child);
    refusal(
        call(
            &mut f,
            child,
            "start_thread",
            json!({"provider":"claude","title":"Grandchild","prompt":"x","client_request_id":"nested"}),
        ),
        &["depth limit is 1: a child cannot start threads", "get_task"],
    );
    assert_eq!(f.loaded().threads.len(), 2);
}
#[test]
fn prompt_cap_refused() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    refusal(
        call(
            &mut f,
            caller,
            "start_thread",
            json!({"provider":"claude","title":"Long","prompt":"é".repeat(100001),"client_request_id":"long"}),
        ),
        &[
            "prompt has 100001 characters against the 100000 character limit",
            "start_thread",
        ],
    );
    let (_, task) = start(&mut f, "short");
    assert_eq!(f.loaded().tasks[&task].state, TaskState::Running);
}
#[test]
fn family_cap_refused() {
    let mut f = Fixture::new(LiveState::Running);
    for _ in 0..15 {
        let child = f.family();
        let mut c = ChangeSet::default();
        let mut row = f.actor.projection.threads[&child].clone();
        row.archived = true;
        c.threads.insert(child, row);
        f.save(c);
    }
    let caller = f.thread;
    refusal(
        call(
            &mut f,
            caller,
            "start_thread",
            json!({"provider":"claude","title":"extra","prompt":"x","client_request_id":"cap"}),
        ),
        &[
            "family has 16 threads against the limit 16",
            "message_thread",
        ],
    );
    assert_eq!(f.loaded().threads.len(), 16);
}
#[test]
fn request_key_reused_with_other_prompt_refused() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let original = start(&mut f, "same");
    assert_eq!(start(&mut f, "same"), original);
    refusal(
        call(
            &mut f,
            caller,
            "start_thread",
            json!({"provider":"claude","title":"Worker","prompt":"different","client_request_id":"same"}),
        ),
        &["start_thread:same", "get_task"],
    );
    assert_eq!(f.loaded().tasks.len(), 1);
}
#[test]
fn request_key_reused_across_tools_makes_two_tasks() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let (child, task) = start(&mut f, "same");
    let value = success(call(
        &mut f,
        caller,
        "message_thread",
        json!({"thread_id":child,"text":"follow up","client_request_id":"same"}),
    ));
    let second: TaskId = serde_json::from_value(value["task_id"].clone()).unwrap();
    assert_ne!(task, second);
    assert_eq!(f.loaded().tasks[&second].request_key, "message_thread:same");
}
#[test]
fn request_key_reused_for_other_child_refused() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let (one, _) = start(&mut f, "one");
    let (two, _) = start(&mut f, "two");
    let args = json!({"thread_id":one,"text":"same text","client_request_id":"follow"});
    let first = success(call(&mut f, caller, "message_thread", args.clone()));
    refusal(
        call(
            &mut f,
            caller,
            "message_thread",
            json!({"thread_id":two,"text":"same text","client_request_id":"follow"}),
        ),
        &["message_thread:follow", &two.to_string(), "get_task"],
    );
    assert_eq!(success(call(&mut f, caller, "message_thread", args)), first);
}
#[test]
fn get_task_refuses_other_parent() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let (child, task) = start(&mut f, "own");
    let mut c = ChangeSet::default();
    let mut other = f.actor.projection.threads[&caller].clone();
    other.id = ThreadId::new();
    let other_id = other.id;
    c.threads.insert(other_id, other);
    c.live.insert(other_id, f.actor.live[&caller].clone());
    let turn = crate::commands::queue(&f.actor.projection, &mut c, other_id, TurnOrigin::Provider);
    c.turns.get_mut(&turn).unwrap().state = TurnState::Running { since: now() };
    f.save(c);
    refusal(
        call(&mut f, other_id, "get_task", json!({"task_id":task})),
        &["own", "get_task"],
    );
    let owned = success(call(&mut f, caller, "get_task", json!({"task_id":task})));
    assert_eq!(owned["thread_id"], json!(child));
}
#[test]
fn cross_family_task_refused() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&caller].clone();
    row.id = ThreadId::new();
    let foreign = row.id;
    c.threads.insert(foreign, row);
    c.live.insert(foreign, LiveThread::default());
    f.save(c);
    for (name, args) in [
        (
            "message_thread",
            json!({"thread_id":foreign,"text":"x","client_request_id":"foreign"}),
        ),
        ("stop_thread", json!({"thread_id":foreign})),
    ] {
        refusal(call(&mut f, caller, name, args), &["own", "get_task"]);
    }
    let (_, task) = start(&mut f, "owned");
    assert_eq!(f.loaded().tasks[&task].parent, caller);
}
#[test]
fn attempted_prompt_is_interrupted_after_crash() {
    let mut f = Fixture::new(LiveState::Idle);
    let Ack::Turn(turn) = f
        .command(Command::Send {
            thread: f.thread,
            text: "must not resend".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let provider_turn = TurnId::new();
    f.event(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
        provider_turn,
    }));
    assert_eq!(f.loaded().turns[&turn].state, TurnState::Queued);
    let recovered = f.loaded();
    let c = crate::reconcile::decide(&recovered, now());
    f.save(c);
    assert!(
        matches!(
            f.loaded().turns[&turn].state,
            TurnState::Ended {
                outcome: TurnOutcome::Interrupted { .. },
                ..
            }
        ),
        "possibly accepted prompt was queued for resend"
    );
    assert_eq!(
        f.loaded().threads[&f.thread].hold,
        Some(HoldReason::Restarted)
    );
}

#[test]
fn recovered_child_without_a_queue_is_interrupted() {
    let mut f = Fixture::new(LiveState::Running);
    let (child, _) = start(&mut f, "recovered status");
    ready_child(&mut f, child);
    let recovered = crate::reconcile::decide(&f.loaded(), now());
    f.save(recovered);
    for (thread, expected) in [(f.thread, Status::Held), (child, Status::Interrupted)] {
        assert_eq!(
            thread_status(
                &ChangeSet::default()
                    .view(&f.actor.projection, &f.actor.live, thread)
                    .unwrap()
            ),
            expected
        );
        assert_eq!(
            f.actor.projection.threads[&thread].hold,
            Some(HoldReason::Restarted)
        );
    }
}

#[test]
fn blocked_changes_are_written_during_quit() {
    let mut f = Fixture::new(LiveState::Running);
    f.actor.fail_write = true;
    f.event(ProviderEvent::Session(SessionEvent::Notice {
        text: "The provider finished a filesystem check".into(),
    }));
    assert!(!f.loaded().items_for(f.thread).any(|item|matches!(&item.body,ItemBody::Notice(n) if n.text=="The provider finished a filesystem check")));
    f.actor.fail_write = false;
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    assert!(f.loaded().items_for(f.thread).any(|item|matches!(&item.body,ItemBody::Notice(n) if n.text=="The provider finished a filesystem check")),"quit lost the retained provider change");
}

#[test]
fn stop_parent_stops_children() {
    let mut f = Fixture::new(LiveState::Running);
    let (child, task) = start(&mut f, "stop");
    ready_child(&mut f, child);
    let parent = f.thread;
    f.command(Command::StopThread { thread: parent }).unwrap();
    let p = f.loaded();
    assert_eq!(p.threads[&parent].hold, Some(HoldReason::Stopped));
    assert_eq!(p.threads[&child].hold, Some(HoldReason::Stopped));
    let TaskState::Settled {
        delivery: Delivery::Pending { wake },
        ..
    } = p.tasks[&task].state
    else {
        panic!()
    };
    assert_eq!(p.turns[&wake].state, TurnState::Queued);
    assert_eq!(f.running(), None);
}
#[test]
fn task_waits_for_child_background_work() {
    let mut f = Fixture::new(LiveState::Running);
    let (child, task) = start(&mut f, "background");
    ready_child(&mut f, child);
    child_event(
        &mut f,
        child,
        ProviderEvent::Session(SessionEvent::BackgroundWork { pending: true }),
    );
    end_child(&mut f, child, task);
    assert_eq!(f.loaded().tasks[&task].state, TaskState::Running);
    child_event(
        &mut f,
        child,
        ProviderEvent::Session(SessionEvent::BackgroundWork { pending: false }),
    );
    assert!(
        matches!(&f.loaded().tasks[&task].state,TaskState::Settled{summary,..} if summary=="Child report")
    );
}
#[test]
fn stop_child_alone_holds_parent_wake() {
    let mut f = Fixture::new(LiveState::Running);
    let (child, task) = start(&mut f, "stop");
    ready_child(&mut f, child);
    f.command(Command::StopThread { thread: child }).unwrap();
    let p = f.loaded();
    assert_eq!(p.threads[&f.thread].hold, Some(HoldReason::ChildReport));
    let TaskState::Settled {
        delivery: Delivery::Pending { wake },
        ..
    } = p.tasks[&task].state
    else {
        panic!()
    };
    assert_eq!(p.turns[&wake].state, TurnState::Queued);
}
#[test]
fn stop_thread_tool_wakes_parent_unheld() {
    let mut f = Fixture::new(LiveState::Running);
    let parent = f.thread;
    let (child, task) = start(&mut f, "stop");
    ready_child(&mut f, child);
    assert_eq!(
        success(call(
            &mut f,
            parent,
            "stop_thread",
            json!({"thread_id":child})
        ))["stopped"],
        true
    );
    let p = f.loaded();
    assert_eq!(p.threads[&parent].hold, None);
    assert!(
        matches!(&p.turns[&p.tasks[&task].turn].state,TurnState::Ended{outcome:TurnOutcome::Interrupted{reason},..} if reason=="Stopped by its parent")
    );
    let TaskState::Settled {
        delivery: Delivery::Pending { wake },
        ..
    } = p.tasks[&task].state
    else {
        panic!()
    };
    assert_eq!(p.turns[&wake].state, TurnState::Queued);
}
#[test]
fn kept_child_delivery_stays_pending() {
    let mut f = Fixture::new(LiveState::Running);
    let parent = f.thread;
    let (child, task) = start(&mut f, "kept");
    ready_child(&mut f, child);
    f.command(Command::ArchiveThread {
        thread: parent,
        keep_children: true,
    })
    .unwrap();
    end_child(&mut f, child, task);
    let p = f.loaded();
    assert!(p.threads[&parent].archived);
    assert!(!p.threads[&child].archived);
    let TaskState::Settled {
        delivery: Delivery::Pending { wake },
        ..
    } = p.tasks[&task].state
    else {
        panic!()
    };
    assert_eq!(p.turns[&wake].state, TurnState::Queued);
}
#[test]
fn child_does_not_inherit_parent_grant() {
    let mut f = Fixture::new(LiveState::Running);
    let mut c = ChangeSet::default();
    let grant = GrantKey {
        tool: "Bash".into(),
        command: "echo test".into(),
    };
    c.grants.push((f.thread, grant.clone()));
    f.save(c);
    let (child, _) = start(&mut f, "grant");
    let p = f.loaded();
    assert!(p.grants.contains(&(f.thread, grant.clone())));
    assert!(!p.grants.contains(&(child, grant)));
}
#[test]
fn child_first_turn_has_guidance() {
    let mut f = Fixture::new(LiveState::Running);
    let (_, task) = start(&mut f, "first");
    let p = f.loaded();
    let text =
        crate::commands::turn_text(&p, &ChangeSet::default(), &p.turns[&p.tasks[&task].turn]);
    assert!(text.starts_with("Your last message is your report"));
    assert!(text.contains("4,000 characters"));
    assert!(text.contains("cannot start child threads"));
    assert!(text.ends_with("Report on the files"));
}
#[test]
fn later_turns_have_no_guidance() {
    let mut f = Fixture::new(LiveState::Running);
    let parent = f.thread;
    let (child, _) = start(&mut f, "first");
    let value = success(call(
        &mut f,
        parent,
        "message_thread",
        json!({"thread_id":child,"text":"plain follow up","client_request_id":"later"}),
    ));
    let task: TaskId = serde_json::from_value(value["task_id"].clone()).unwrap();
    let p = f.loaded();
    assert_eq!(
        crate::commands::turn_text(&p, &ChangeSet::default(), &p.turns[&p.tasks[&task].turn]),
        "plain follow up"
    );
    let Ack::Turn(user) = f
        .command(Command::Send {
            thread: child,
            text: "plain user text".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let p = f.loaded();
    assert_eq!(
        crate::commands::turn_text(&p, &ChangeSet::default(), &p.turns[&user]),
        "plain user text"
    );
}
#[test]
fn message_thread_reports_delivery() {
    let mut f = Fixture::new(LiveState::Running);
    let parent = f.thread;
    let (child, task) = start(&mut f, "first");
    ready_child(&mut f, child);
    let queued = success(call(
        &mut f,
        parent,
        "message_thread",
        json!({"thread_id":child,"text":"queued follow up","client_request_id":"queued"}),
    ));
    assert_eq!(queued["delivery"], "queued");
    assert_eq!(queued["waits_for"], "Waits for the current turn");
    end_child(&mut f, child, task);
    let next: TaskId = serde_json::from_value(queued["task_id"].clone()).unwrap();
    end_child(&mut f, child, next);
    let started = success(call(
        &mut f,
        parent,
        "message_thread",
        json!({"thread_id":child,"text":"idle follow up","client_request_id":"idle"}),
    ));
    assert_eq!(started["delivery"], "started");
    assert_eq!(started["waits_for"], Value::Null);
}
#[test]
fn recovered_child_settles_on_continue() {
    for parent_held in [false, true] {
        let mut f = Fixture::new(LiveState::Running);
        let parent = f.thread;
        let (child, task) = start(&mut f, "recovered");
        ready_child(&mut f, child);
        let c = crate::reconcile::decide(&f.loaded(), now());
        f.save(c);
        if !parent_held {
            f.hold(None);
        }
        assert_eq!(f.loaded().tasks[&task].state, TaskState::Running);
        f.command(Command::Continue {
            thread: child,
            drop_queued: false,
        })
        .unwrap();
        let p = f.loaded();
        assert_eq!(
            p.threads[&parent].hold,
            Some(if parent_held {
                HoldReason::Restarted
            } else {
                HoldReason::ChildReport
            })
        );
        let TaskState::Settled {
            delivery: Delivery::Pending { wake },
            ..
        } = p.tasks[&task].state
        else {
            panic!()
        };
        assert_eq!(p.turns[&wake].state, TurnState::Queued);
    }
}
#[test]
fn delivery_survives_crash_before_accept() {
    let mut f = Fixture::new(LiveState::Running);
    let parent = f.thread;
    let (child, task) = start(&mut f, "delivery");
    ready_child(&mut f, child);
    end_child(&mut f, child, task);
    let parent_turn = f.running().unwrap();
    f.event(turn_event(
        Some(parent_turn),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
            park: false,
        },
    ));
    let wake = f.running().unwrap();
    let c = crate::reconcile::decide(&f.loaded(), now());
    f.save(c);
    let p = f.loaded();
    let TaskState::Settled {
        delivery: Delivery::Pending { wake: replacement },
        ..
    } = p.tasks[&task].state
    else {
        panic!()
    };
    assert_ne!(wake, replacement);
    assert_eq!(p.turns[&replacement].state, TurnState::Queued);
    assert_eq!(p.threads[&parent].hold, Some(HoldReason::Restarted));
}
#[test]
fn successor_wake_while_wake_runs() {
    let mut f = Fixture::new(LiveState::Running);
    let (one, first) = start(&mut f, "one");
    let (two, second) = start(&mut f, "two");
    ready_child(&mut f, one);
    ready_child(&mut f, two);
    end_child(&mut f, one, first);
    let parent_turn = f.running().unwrap();
    f.event(turn_event(
        Some(parent_turn),
        TurnEvent::TurnEnded {
            outcome: TurnOutcome::Completed,
            park: false,
        },
    ));
    let first_wake = f.running().unwrap();
    f.event(turn_event(Some(first_wake), TurnEvent::Accepted));
    end_child(&mut f, two, second);
    let p = f.loaded();
    let TaskState::Settled {
        delivery: Delivery::Pending { wake },
        ..
    } = p.tasks[&second].state
    else {
        panic!()
    };
    assert_ne!(wake, first_wake);
    assert_eq!(p.turns[&wake].state, TurnState::Queued);
    assert_eq!(
        p.tasks[&first].state,
        TaskState::Settled {
            outcome: TurnOutcome::Completed,
            summary: "Child report".into(),
            delivery: Delivery::Delivered { wake: first_wake }
        }
    );
}

#[test]
fn get_task_status_matches_window() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let (child, task) = start(&mut f, "status");
    let mut observed = std::collections::BTreeSet::new();
    for state in [
        "Idle",
        "Working",
        "NeedsYou",
        "Held",
        "Failed",
        "Interrupted",
        "Done",
        "InTerminal",
    ] {
        let mut c = ChangeSet::default();
        let mut row = f.actor.projection.threads[&child].clone();
        row.hold = None;
        row.view = View::Chat;
        c.threads.insert(child, row);
        let mut live = f.actor.live[&child].clone();
        live.background_work = false;
        live.admission = None;
        live.live = Live::Parked;
        for turn in f.actor.projection.turns_for(child) {
            let mut turn = turn.clone();
            turn.state = TurnState::Queued;
            c.turns.insert(turn.id, turn);
        }
        for request in f
            .actor
            .projection
            .requests
            .values()
            .filter(|request| request.thread == child)
        {
            let mut request = request.clone();
            request.state = RequestState::Expired {
                reason: "status transition".into(),
            };
            c.requests.insert(request.id, request);
        }
        let turn = c.turns.get_mut(&task_turn(&f, task)).unwrap();
        match state {
            "Idle" => {}
            "Working" => turn.state = TurnState::Running { since: 1 },
            "NeedsYou" => {
                turn.state = TurnState::Running { since: 1 };
                let request = Request {
                    id: RequestId::new(),
                    thread: child,
                    turn: turn.id,
                    native: NativeRequestRef("status approval".into()),
                    kind: RequestKind::Approval {
                        action: ProposedAction {
                            tool: "Bash".into(),
                            target: ApprovalTarget::Command {
                                command: "echo status".into(),
                                preview: ApprovalPreview::new(String::new()),
                            },
                        },
                        grant_key: None,
                    },
                    state: RequestState::Pending,
                };
                c.requests.insert(request.id, request);
            }
            "Held" => c.threads.get_mut(&child).unwrap().hold = Some(HoldReason::Stopped),
            "Failed" => {
                turn.state = TurnState::Ended {
                    outcome: TurnOutcome::Failed {
                        message: "failed".into(),
                    },
                    started: Some(1),
                    at: 2,
                }
            }
            "Interrupted" => {
                turn.state = TurnState::Ended {
                    outcome: TurnOutcome::Interrupted {
                        reason: "interrupted".into(),
                    },
                    started: Some(1),
                    at: 2,
                }
            }
            "Done" => {
                turn.state = TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    started: Some(1),
                    at: 2,
                }
            }
            "InTerminal" => {
                let (control, _) = mpsc::unbounded_channel();
                live.live = Live::Terminal {
                    term: TerminalHandle { control },
                    pid: 1,
                };
            }
            _ => unreachable!(),
        }
        c.live.insert(child, live);
        f.save(c);
        let words = success(call(&mut f, caller, "get_task", json!({"task_id":task})))["status"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(words, state);
        assert_eq!(
            words,
            thread_status(
                &ChangeSet::default()
                    .view(&f.actor.projection, &f.actor.live, child)
                    .unwrap()
            )
            .words()
        );
        observed.insert(words);
    }
    assert_eq!(observed.len(), 8);
}

#[test]
fn storage_loss_notice_survives_a_process_end() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/storage-loss-{}", ThreadId::new()));
    crate::platform::private_dir(&root.join("run")).unwrap();
    let _scratch = Scratch(root.clone());
    let mut f = Fixture::new(LiveState::Running);
    let rows = ChangeSet {
        projects: f.actor.projection.projects.clone(),
        threads: f.actor.projection.threads.clone(),
        turns: f.actor.projection.turns.clone(),
        live: f.actor.live.clone(),
        ..Default::default()
    };
    let (out, _rx) = mpsc::channel(16);
    f.actor = Actor::new(
        store::open(&root.join("veyra.db")).unwrap(),
        out,
        Spawner::Scripted(Scripted::default()),
    )
    .unwrap();
    f.save(rows);
    f.actor.fail_write = true;
    f.event(ProviderEvent::Session(SessionEvent::Notice {
        text: "unwritten provider notice".into(),
    }));
    assert!(root.join("run/storage-blocked.json").exists());
    let (out, _rx) = mpsc::channel(16);
    let recovered = Actor::new(
        store::open(&root.join("veyra.db")).unwrap(),
        out,
        Spawner::Scripted(Scripted::default()),
    )
    .unwrap();
    let p = store::load(&recovered.db).unwrap();
    assert!(p.items_for(f.thread).any(|item|matches!(&item.body,ItemBody::Notice(notice) if notice.text.contains(&f.thread.to_string())&&notice.text.contains("provider notice item")&&notice.text.contains("may have lost"))));
    assert!(!root.join("run/storage-blocked.json").exists());
}

#[test]
fn queued_tool_request_keeps_its_originating_session() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let epoch = f.actor.live[&caller].epoch;
    let pid = match f.actor.live[&caller].live {
        Live::Structured { pid, .. } => pid,
        _ => unreachable!(),
    };
    for (request_epoch, request_pid, ok) in [
        (epoch + 1, pid, false),
        (epoch, pid + 1, false),
        (epoch, pid, true),
    ] {
        let _entered = f.runtime.enter();
        let (reply, result) = oneshot::channel();
        f.actor.handle(EngineMsg::Tool {
            caller,
            session: Some(crate::mcp::Caller {
                epoch: request_epoch,
                pid: request_pid,
                token: crate::mcp::TokenHash([0; 32]),
            }),
            call: ToolCall {
                name: "list_providers".into(),
                arguments: json!({}),
                input_summary: None,
            },
            reply,
        });
        let result = result.blocking_recv().unwrap();
        if ok {
            success(result);
        } else {
            refusal(result, &["session", "list_providers"]);
        }
    }
}

#[test]
fn tool_request_without_session_is_refused_by_cli_actor() {
    let mut f = Fixture::new(LiveState::Running);
    f.actor.spawner = Spawner::Cli(crate::provider::env::ChildEnv::load());
    let caller = f.thread;
    refusal(
        call(&mut f, caller, "list_providers", json!({})),
        &["session", "list_providers"],
    );
}

#[test]
fn message_thread_cannot_reopen_a_fifth_child_slot() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let (finished, task) = start(&mut f, "finished");
    ready_child(&mut f, finished);
    let (occupied, _) = start(&mut f, "occupied");
    for key in ["third", "fourth"] {
        start(&mut f, key);
    }
    end_child(&mut f, finished, task);
    start(&mut f, "replacement");
    let args = json!({"thread_id":occupied,"text":"more","client_request_id":"same-slot"});
    let first = success(call(&mut f, caller, "message_thread", args.clone()));
    assert_eq!(success(call(&mut f, caller, "message_thread", args)), first);
    refusal(
        call(
            &mut f,
            caller,
            "message_thread",
            json!({"thread_id":finished,"text":"more","client_request_id":"fifth"}),
        ),
        &[
            "4 child threads with open tasks against the limit 4",
            "stop_thread",
        ],
    );
    let p = f.loaded();
    let open: std::collections::BTreeSet<_> = p
        .tasks
        .values()
        .filter(|task| task.state == TaskState::Running)
        .map(|task| task.child)
        .collect();
    assert_eq!(open.len(), 4);
}
#[test]
fn blocked_streamed_text_is_stored_once_at_quit() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running().unwrap();
    f.event(turn_event(
        Some(turn),
        TurnEvent::TextDelta {
            key: NativeItemKey("reply".into()),
            text: "one copy".into(),
            streamed: true,
        },
    ));
    f.actor.fail_write = true;
    {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::TextWrite);
    }
    f.actor.fail_write = false;
    {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Shutdown);
    }
    let p = f.loaded();
    let text = p
        .items_for(f.thread)
        .find_map(|item| match &item.body {
            ItemBody::AssistantText { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .unwrap();
    assert_eq!(text, "one copy");
}
#[test]
fn storage_tool_refusals_name_a_recovery_call() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    f.actor.fail_write = true;
    refusal(
        call(
            &mut f,
            caller,
            "start_thread",
            json!({"provider":"claude","title":"Worker","prompt":"x","client_request_id":"retry-me"}),
        ),
        &["storage", "retry", "start_thread", "retry-me"],
    );
    f.event(ProviderEvent::Session(SessionEvent::Notice {
        text: "blocked notice".into(),
    }));
    refusal(
        call(&mut f, caller, "list_providers", json!({})),
        &["storage", "list_providers", "{}"],
    );
}
#[test]
fn tool_catalog_refuses_unbuilt_providers() {
    let mut f = Fixture::new(LiveState::Running);
    let caller = f.thread;
    let listed = success(call(&mut f, caller, "list_providers", json!({})));
    let names: Vec<_> = listed["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|provider| provider["provider"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["claude"]);
    for provider in ["codex", "grok"] {
        refusal(
            call(
                &mut f,
                caller,
                "start_thread",
                json!({"provider":provider,"title":"Worker","prompt":"x","client_request_id":provider}),
            ),
            &["not built", "list_providers", "{}"],
        );
    }
}
#[test]
fn parent_stop_keeps_the_discarded_queue_reason() {
    let mut f = Fixture::new(LiveState::Running);
    let (child, _) = start(&mut f, "reason");
    ready_child(&mut f, child);
    let Ack::Turn(queued) = f
        .command(Command::Send {
            thread: child,
            text: "queued".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let caller = f.thread;
    success(call(
        &mut f,
        caller,
        "stop_thread",
        json!({"thread_id":child}),
    ));
    let p = f.loaded();
    assert!(
        matches!(&p.turns[&queued].state,TurnState::Ended{outcome:TurnOutcome::Interrupted{reason},..} if reason=="Queued turn discarded")
    );
}
#[test]
fn malformed_storage_receipt_does_not_prevent_startup() {
    for unreadable in [false, true] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/lost-receipt-{}", ThreadId::new()));
        crate::platform::private_dir(&root.join("run")).unwrap();
        let mut db = Connection::open(root.join("veyra.db")).unwrap();
        store::initialize(&mut db).unwrap();
        let receipt = root.join("run/storage-blocked.json");
        if unreadable {
            crate::platform::private_dir(&receipt).unwrap();
        } else {
            crate::platform::private_text(&receipt, "invalid receipt").unwrap();
        }
        let (out, _rx) = mpsc::channel(16);
        let mut actor = Actor::new(db, out, Spawner::Scripted(Scripted::default()))
            .expect("a lost receipt refused startup");
        let (events, mut received) = mpsc::unbounded_channel();
        actor.handle(EngineMsg::Subscribe(events));
        let mut notices = Vec::new();
        while let Ok(event) = received.try_recv() {
            if let UiEvent::RecoveryNotice(text) = event {
                notices.push(text);
            }
        }
        assert_eq!(
            notices,
            [
                "Veyra could not read its storage recovery receipt. A private copy was kept when possible. Check the provider conversations before sending more work."
            ]
        );
        assert!(!receipt.exists());
        let kept = std::fs::read_dir(root.join("run"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("storage-blocked-lost-")
            })
            .unwrap();
        assert!(crate::platform::is_private(&kept).unwrap());
        if !unreadable {
            assert_eq!(std::fs::read_to_string(kept).unwrap(), "invalid receipt");
        }
        drop(actor);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[test]
fn quit_retries_retained_changes_after_the_write_lock_clears() {
    use std::time::{Duration, Instant};
    let f = Fixture::new(LiveState::Running);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/quit-retry-{}", ThreadId::new()));
    crate::platform::private_dir(&root.join("run")).unwrap();
    let path = root.join("veyra.db");
    f.actor
        .db
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let db = Connection::open(&path).unwrap();
    let (out, rx) = mpsc::channel(16);
    let mut actor = Actor::new(db, out.clone(), Spawner::Scripted(Scripted::default())).unwrap();
    actor.db.busy_timeout(Duration::ZERO).unwrap();
    let lock = Connection::open(&path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    {
        let _entered = f.runtime.enter();
        actor.handle(EngineMsg::Provider {
            thread: f.thread,
            epoch: actor.live[&f.thread].epoch,
            event: ProviderEvent::Session(SessionEvent::Notice {
                text: "retained across quit retries".into(),
            }),
        });
    }
    assert!(root.join("run/storage-blocked.json").exists());
    let (seen, shutdown) = std::sync::mpsc::channel();
    actor.after_admission = Some(Box::new(move || seen.send(()).unwrap()));
    let runtime = f.runtime.handle().clone();
    let running = std::thread::spawn(move || actor.run(rx, runtime));
    out.blocking_send(EngineMsg::Shutdown).unwrap();
    shutdown.recv_timeout(Duration::from_secs(1)).unwrap();
    let waiting = Instant::now();
    std::thread::sleep(Duration::from_millis(125));
    lock.execute_batch("ROLLBACK").unwrap();
    running.join().unwrap();
    assert!(waiting.elapsed() < Duration::from_secs(2));
    let loaded = store::load(&Connection::open(&path).unwrap()).unwrap();
    assert!(loaded.items_for(f.thread).any(|item|matches!(&item.body,ItemBody::Notice(notice) if notice.text=="retained across quit retries")));
    assert!(!root.join("run/storage-blocked.json").exists());
    std::fs::remove_dir_all(root).unwrap();
}
