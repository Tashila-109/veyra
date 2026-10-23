#![allow(clippy::disallowed_types)] // Replays inspect provider and recording wire envelopes.
#![cfg(debug_assertions)]
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{runtime::Runtime, sync::mpsc};
use veyra_engine::{
    actor::EngineMsg,
    domain::*,
    provider::{env::ChildEnv, *},
};
#[path = "common/replay.rs"]
mod harness;
use harness::{Inputs, Scratch};
fn fixture(name: &str) -> PathBuf {
    harness::fixture(ProviderKind::Codex, name)
}
fn environment(name: &str) -> (Scratch, ChildEnv) {
    harness::environment(ProviderKind::Codex, name)
}
fn replay(
    name: &str,
    mode: PermissionMode,
    root: bool,
    resume: bool,
    inputs: Inputs,
) -> Vec<ProviderEvent> {
    harness::replay(ProviderKind::Codex, name, mode, root, resume, inputs).0
}
fn standard(name: &str, inputs: Inputs) -> Vec<ProviderEvent> {
    replay(name, PermissionMode::AskFirst, false, false, inputs)
}
fn text(events: &[ProviderEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::TextDelta { text, .. },
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}
fn outcomes(events: &[ProviderEvent]) -> Vec<TurnOutcome> {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::TurnEnded { outcome, .. },
                ..
            } => Some(outcome.clone()),
            _ => None,
        })
        .collect()
}
fn requests(events: &[ProviderEvent]) -> Vec<&RequestKind> {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::RequestOpened { kind, .. },
                ..
            } => Some(kind),
            _ => None,
        })
        .collect()
}

#[test]
fn codex_sign_in_failure_names_login_once() {
    let events = standard("sign-in-required", Inputs::default());
    assert_eq!(
        outcomes(&events),
        [TurnOutcome::Failed {
            message:
                "Codex is signed out. Sign in from a terminal with `codex login`, then send again."
                    .into(),
        }]
    );
}
#[test]
fn codex_ok_reply() {
    let e = standard("ok-reply", Inputs::default());
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
    assert!(e.iter().any(|e| matches!(
        e,
        ProviderEvent::Turn {
            event: TurnEvent::Accepted,
            ..
        }
    )));
}
#[test]
fn codex_shell_echo() {
    let e = replay(
        "shell-echo",
        PermissionMode::AcceptEdits,
        false,
        false,
        Inputs::default(),
    );
    assert_eq!(
        text(&e),
        "I’ll run the command and report its output.veyra-probe"
    );
    assert!(e.iter().any(
        |e| matches!(e,ProviderEvent::Turn{event:TurnEvent::ToolFinished{ok:true,
        output_tail:Some(output),..},..} if output=="veyra-probe\n")
    ));
}
#[test]
fn codex_command_approval() {
    let e = standard("approval-command", Inputs::answers([Decision::AllowOnce]));
    assert!(
        matches!(requests(&e).as_slice(),[RequestKind::Approval {action:ProposedAction{
            target:ApprovalTarget::Command{command,..},..},..}] if command=="touch veyra-check.txt")
    );
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_file_change_approval() {
    let e = standard("approval-file-change", Inputs::answers([Decision::Deny]));
    assert!(
        matches!(requests(&e).as_slice(),[RequestKind::Approval{action:ProposedAction{tool,
            consent:ApprovalConsent::Once{grant:None},..}}] if tool=="File change")
    );
    assert_eq!(text(&e), "ok");
}
#[test]
fn codex_permissions_deny_grants_nothing() {
    let e = standard("permissions", Inputs::answers([Decision::Deny]));
    assert!(
        matches!(requests(&e).as_slice(),[RequestKind::Approval{action:ProposedAction{tool,
            consent:ApprovalConsent::Once{grant:None},..}}] if tool=="Permissions")
    );
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_elicitation_declined() {
    let e = standard("elicitation", Inputs::default());
    assert_eq!(requests(&e).len(), 0);
    assert!(e.iter().any(
        |e| matches!(e,
        ProviderEvent::Session(SessionEvent::Notice(veyra_engine::domain::Notice { text, .. })) if text=="Codex reque\
        sted an MCP form that Veyra cannot display; it was declined.")
    ));
    assert_eq!(text(&e), "ok");
}
#[test]
fn codex_question_answered() {
    let e = standard(
        "question",
        Inputs::answers([Decision::Answers(vec![Answer {
            text: "Violet".into(),
        }])]),
    );
    assert_eq!(
        requests(&e),
        [&RequestKind::Question {
            questions: vec![Question {
                text: "Which color should Veyra use?".into(),
                options: vec!["Violet".into(), "Blue".into()]
            }]
        }]
    );
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_unknown_request_gets_32601() {
    let e = standard("unknown-request", Inputs::default());
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_resume_not_found_falls_back() {
    let e = replay(
        "resume-not-found",
        PermissionMode::AskFirst,
        false,
        true,
        Inputs::default(),
    );
    assert!(e.iter().any(
        |e| matches!(e,
        ProviderEvent::Session(SessionEvent::Notice(veyra_engine::domain::Notice { text, .. })) if text=="The earlie\
        r conversation could not be resumed; this is a new session")
    ));
    assert_eq!(text(&e), "ok");
}
#[test]
fn codex_other_turn_id_is_cli_work() {
    let e = standard("other-turn", Inputs::default());
    assert_eq!(text(&e), "okCLI work");
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Completed, TurnOutcome::Completed]
    );
    assert!(e.iter().any(|e| matches!(
        e,
        ProviderEvent::Session(SessionEvent::ProviderTurnStarted { .. })
    )));
    assert!(e.iter().any(
        |e| matches!(e,ProviderEvent::Turn{turn:None,event:TurnEvent::TextDelta{text,..}} if
        text=="CLI work")
    ));
}
#[test]
fn codex_stop_cancels_pending_request() {
    let e = standard("stop-pending-approval", Inputs::stop_request());
    assert_eq!(requests(&e).len(), 1);
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
}
#[test]
fn codex_interrupt_retries_after_started() {
    let e = standard("interrupt-before-started", Inputs::stop_accepted());
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
}
#[test]
fn codex_completed_before_started_ends_turn() {
    let e = standard("completed-before-started", Inputs::default());
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped by Codex".into()
        }]
    );
    assert_eq!(
        e.iter()
            .filter(|e| matches!(
                e,
                ProviderEvent::Turn {
                    event: TurnEvent::Started,
                    ..
                }
            ))
            .count(),
        0
    );
}
#[test]
fn codex_resume_retries_active_writer() {
    let e = replay(
        "resume-active-writer",
        PermissionMode::AskFirst,
        false,
        true,
        Inputs::default(),
    );
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_resume_sends_exclude_turns() {
    let e = replay(
        "resume-to-ask-first",
        PermissionMode::AskFirst,
        false,
        true,
        Inputs::default(),
    );
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_every_turn_has_network_access() {
    for (name, mode, root) in [
        ("root-policy", PermissionMode::AskFirst, true),
        (
            "child-policy-accept-edits",
            PermissionMode::AcceptEdits,
            false,
        ),
        (
            "child-policy-full-access",
            PermissionMode::FullAccess,
            false,
        ),
    ] {
        let e = replay(name, mode, root, false, Inputs::default());
        assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
        assert_eq!(text(&e), "ok");
    }
}
#[test]
fn codex_mcp_tool_approval_docks() {
    let e = standard(
        "mcp-tool-approval",
        Inputs::answers([Decision::AllowOnce, Decision::Deny]),
    );
    assert_eq!(requests(&e).len(), 2);
    for r in requests(&e) {
        assert!(
            matches!(r,RequestKind::Approval{action:ProposedAction{tool,consent:ApprovalConsent::Once{
                grant:None},..}} if tool=="veyra / list_providers")
        );
    }
    assert!(
        !e.iter()
            .any(|e| matches!(e, ProviderEvent::Session(SessionEvent::Notice(_))))
    );
    assert_eq!(text(&e), "ok");
}
#[test]
fn codex_other_thread_frames_are_filtered() {
    let e = standard("subagent-other-thread", Inputs::default());
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
    assert_eq!(
        e.iter()
            .filter(|e| matches!(
                e,
                ProviderEvent::Turn {
                    event: TurnEvent::Started,
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(!e.iter().any(|e| matches!(
        e,
        ProviderEvent::Session(SessionEvent::ProviderTurnStarted { .. })
            | ProviderEvent::Session(SessionEvent::BackgroundWork { pending: true })
    )));
}
#[test]
fn codex_retry_and_limit_notices() {
    let e = standard("retry-and-limits", Inputs::default());
    let n: Vec<_> = e
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Session(SessionEvent::Notice(veyra_engine::domain::Notice {
                text,
                ..
            })) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        n,
        [
            "Codex is retrying after a provider error.",
            "Codex is near its usage limit: primary window, 95% used."
        ]
    );
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_command_output_falls_back_to_deltas() {
    let e = standard("output-null", Inputs::answers([Decision::AllowOnce]));
    assert!(e.iter().any(
        |e| matches!(e,ProviderEvent::Turn{event:TurnEvent::ToolFinished{ok:true,
        output_tail:Some(output),..},..} if output=="veyra-probe\n")
    ));
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_probe_reads_catalog_without_a_turn() {
    let (scratch, env) = environment("ok-reply");
    Runtime::new().unwrap().block_on(async {
        let mut probes = Probes::default();
        let p = probes
            .check(
                &tokio::runtime::Handle::current(),
                ProviderKind::Codex,
                env,
                scratch.0.clone(),
            )
            .await
            .unwrap();
        assert_eq!(p.readiness, Readiness::Ready);
        assert_eq!(p.version.as_deref(), Some("0.161.0"));
        assert_eq!(p.models[0].value, "veyra-model");
        assert_eq!(p.models[0].effort_levels, ["medium"]);
    });
}

fn engine_replay(
    test: &str,
    scenarios: &[&str],
    check: impl FnOnce(&veyra_engine::Engine, &Path, ThreadId, &Runtime),
) {
    if std::env::var("VEYRA_TEST_ENGINE_REPLAY").ok().as_deref() != Some(test) {
        let root = Path::new("/tmp").join(format!("veyra-codex-replay-{}", ThreadId::new()));
        fs::create_dir(&root).unwrap();
        let _scratch = Scratch(root.clone());
        let fixtures = root.join("fixtures");
        fs::create_dir(&fixtures).unwrap();
        for (i, name) in scenarios.iter().enumerate() {
            fs::copy(fixture(name), fixtures.join(format!("t1-s{}.jsonl", i + 1))).unwrap();
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env("VEYRA_TEST_ENGINE_REPLAY", test)
            .env("VEYRA_TEST_ENGINE_ROOT", &root)
            .env("VEYRA_FAKE_FIXTURES", &fixtures)
            .env("VEYRA_CODEX_BIN", env!("CARGO_BIN_EXE_veyra-fake-cli"))
            .env("VEYRA_CLAUDE_BIN", "/nonexistent/claude")
            .env_remove("VEYRA_RECORD")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = PathBuf::from(std::env::var_os("VEYRA_TEST_ENGINE_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let data = veyra_engine::data_dir::DataDir::at(root.join("data")).unwrap();
    veyra_engine::platform::private_dir(data.path()).unwrap();
    let mut db = veyra_engine::store::open(&data.path().join("veyra.db")).unwrap();
    let project = Project {
        id: ProjectId::new(),
        path: root.clone(),
        name: "Codex replay".into(),
    };
    let parent = Thread {
        id: ThreadId::new(),
        project: project.id,
        title: "Claude parent".into(),
        lineage: Lineage::Root,
        provider: ProviderKind::Claude,
        config: ThreadConfig {
            mode: PermissionMode::FullAccess,
            model: None,
            effort: None,
        },
        cwd: root.clone(),
        native: None,
        cli_version: None,
        hold: None,
        archived: false,
    };
    let child = Thread {
        id: ThreadId::new(),
        title: "Codex child".into(),
        lineage: Lineage::Child {
            parent: parent.id,
            root: parent.id,
            relationship: Relationship::Subagent,
        },
        provider: ProviderKind::Codex,
        config: ThreadConfig {
            mode: if scenarios[0] == "subagent-other-thread" {
                PermissionMode::AskFirst
            } else {
                PermissionMode::FullAccess
            },
            model: Some("veyra-model".into()),
            effort: Some("medium".into()),
        },
        ..parent.clone()
    };
    let mut changes = veyra_engine::events::ChangeSet::default();
    changes.projects.insert(project.id, project);
    changes.threads.insert(parent.id, parent);
    changes.threads.insert(child.id, child.clone());
    let tx = db.transaction().unwrap();
    veyra_engine::store::apply(&tx, &changes, 1).unwrap();
    tx.commit().unwrap();
    drop(db);
    let runtime = Runtime::new().unwrap();
    let mut environment = ChildEnv::isolated().with_binary(
        ProviderKind::Codex,
        env!("CARGO_BIN_EXE_veyra-fake-cli").into(),
    );
    environment.fixtures = Some(root.join("fixtures"));
    let engine = veyra_engine::Engine::start_with_env(runtime.handle(), data, environment).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check(&engine, &root, child.id, &runtime)
    }));
    runtime.block_on(engine.shutdown_wait());
    drop(engine);
    drop(runtime);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
async fn run_engine_turn(engine: &veyra_engine::Engine, child: ThreadId) -> TurnId {
    use veyra_engine::{
        commands::{Ack, Command},
        events::{ChangeEvent, UiEvent},
    };
    let mut events = engine.subscribe();
    let Ack::Turn(turn) = engine
        .send(Command::Send {
            thread: child,
            text: "reply with the word ok".into(),
        })
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("missing turn")
    };
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = events.recv().await {
            if let UiEvent::Change {
                event: ChangeEvent::TurnEnded(row),
                ..
            } = event
                && row.id == turn
            {
                assert!(
                    matches!(
                        row.state,
                        TurnState::Ended {
                            outcome: TurnOutcome::Completed,
                            ..
                        }
                    ),
                    "{:?}",
                    row.state
                );
                return;
            }
        }
        panic!("event stream ended");
    })
    .await
    .unwrap();
    turn
}
#[test]
fn codex_resume_reapplies_mode() {
    engine_replay(
        "codex_resume_reapplies_mode",
        &[
            "child-policy-full-access",
            "resume-to-ask-first",
            "resume-to-full-access",
            "resume-to-accept-edits",
        ],
        |engine, root, child, runtime| {
            runtime.block_on(async {
                run_engine_turn(engine, child).await;
                for mode in [
                    PermissionMode::AskFirst,
                    PermissionMode::FullAccess,
                    PermissionMode::AcceptEdits,
                ] {
                    engine
                        .send(veyra_engine::commands::Command::Configure {
                            thread: child,
                            config: ThreadConfig {
                                mode,
                                model: Some("veyra-model".into()),
                                effort: Some("medium".into()),
                            },
                        })
                        .await
                        .unwrap()
                        .unwrap();
                    run_engine_turn(engine, child).await;
                }
            });
            let db = veyra_engine::store::open(&root.join("data/veyra.db")).unwrap();
            let p = veyra_engine::store::load(&db).unwrap();
            assert_eq!(p.turns.len(), 4);
            assert!(p.turns.values().all(|t| matches!(
                t.state,
                TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    ..
                }
            )));
            assert_eq!(p.threads[&child].config.mode, PermissionMode::AcceptEdits);
            assert_eq!(
                p.threads[&child].native.as_ref().unwrap().0,
                "native-thread"
            );
        },
    );
}

#[test]
fn codex_other_thread_never_marks_working() {
    engine_replay(
        "codex_other_thread_never_marks_working",
        &["subagent-other-thread"],
        |engine, root, child, runtime| {
            runtime.block_on(run_engine_turn(engine, child));
            let db = veyra_engine::store::open(&root.join("data/veyra.db")).unwrap();
            let p = veyra_engine::store::load(&db).unwrap();
            assert_eq!(p.turns.len(), 1);
            assert!(
                p.turns
                    .values()
                    .all(|t| matches!(t.origin, TurnOrigin::User { .. }))
            );
            assert_eq!(
                p.items
                    .values()
                    .filter_map(|i| match &i.body {
                        ItemBody::AssistantText { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
                "ok"
            );
        },
    );
}

#[test]
fn codex_request_before_reply_is_docked() {
    let e = standard("request-before-reply", Inputs::answers([Decision::Deny]));
    assert_eq!(requests(&e).len(), 1);
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_completion_before_reply_ends_once() {
    let e = standard("completed-before-reply", Inputs::default());
    assert_eq!(
        outcomes(&e),
        [TurnOutcome::Interrupted {
            reason: "Stopped by Codex".into()
        }]
    );
    assert!(!e.iter().any(|e| matches!(
        e,
        ProviderEvent::Session(SessionEvent::ProviderTurnStarted { .. })
    )));
}
#[test]
fn codex_bad_lines_keep_the_session() {
    let e = standard("bad-lines", Inputs::default());
    assert_eq!(text(&e), "ok");
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}

#[test]
fn codex_request_can_answer_before_the_start_reply() {
    let events = replay(
        "early-request-waits-for-answer",
        PermissionMode::AskFirst,
        false,
        false,
        Inputs::answers([Decision::Deny]),
    );
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::RequestOpened {
                kind: RequestKind::Approval { .. },
                ..
            },
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
                ..
            },
            ..
        }
    )));
}
#[test]
fn codex_session_cancels_early_request_before_interrupt() {
    let (scratch, mut env) = environment("stop-before-start-reply");
    env.record = Some(scratch.0.join("rec"));
    let mut owned = vec![];
    Runtime::new().unwrap().block_on(async {
        let (out, mut messages) = mpsc::channel(256);
        let turn = TurnId::new();
        let session = Spawner::Cli(Box::new(env)).spawn(
            ProviderKind::Codex,
            SessionSpec {
                thread: ThreadId::new(),
                cwd: std::env::current_dir().unwrap(),
                config: ThreadConfig {
                    mode: PermissionMode::AskFirst,
                    model: Some("veyra-model".into()),
                    effort: Some("medium".into()),
                },
                native: None,
                root: false,
            },
            out,
            1,
        );
        let mut ended = None;
        tokio::time::timeout(Duration::from_secs(8), async {
            while let Some(EngineMsg::Provider { event, .. }) = messages.recv().await {
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    session
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                    session
                        .control
                        .send(SessionControl::Interrupt {
                            turn: *turn,
                            reason: StopReason::User,
                        })
                        .unwrap();
                }
                match event {
                    ProviderEvent::Session(SessionEvent::Ready { pid, .. }) => {
                        owned.push(pid);
                        session
                            .control
                            .send(SessionControl::StartTurn {
                                turn,
                                text: "work".into(),
                            })
                            .unwrap();
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::TurnEnded { outcome, .. },
                        ..
                    } => ended = Some(outcome),
                    ProviderEvent::Session(SessionEvent::Exited { .. }) => break,
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            ended,
            Some(TurnOutcome::Interrupted {
                reason: "Stopped by you".into()
            })
        );
    });
    assert!(
        owned
            .into_iter()
            .all(|pid| !veyra_engine::platform::group_exists(pid).unwrap())
    );
    let frames: Vec<serde_json::Value> = fs::read_to_string(scratch.0.join("rec/t1-s1.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|record| record["type"] == "expect_outbound")
        .map(|record| record["line"].clone())
        .filter(|line| {
            line["method"] == "turn/interrupt"
                || line["result"] == serde_json::json!({"decision":"cancel"})
        })
        .collect();
    assert_eq!(
        frames.len(),
        2,
        "actual session writer did not send cancellation and interrupt"
    );
    assert_eq!(
        frames[0]["result"],
        serde_json::json!({"decision":"cancel"})
    );
    assert_eq!(frames[1]["method"], "turn/interrupt");
}

#[test]
fn codex_low_usage_adds_no_notice() {
    let e = standard("rate-limit-low", Inputs::default());
    assert_eq!(
        e.iter()
            .filter(|e| matches!(e, ProviderEvent::Session(SessionEvent::Notice(_))))
            .count(),
        0
    );
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}
#[test]
fn codex_repeated_near_limit_adds_one_notice() {
    let e = standard("rate-limit-repeated", Inputs::default());
    assert_eq!(
        e.iter()
            .filter(|e| matches!(e, ProviderEvent::Session(SessionEvent::Notice(_))))
            .count(),
        1
    );
    assert_eq!(outcomes(&e), [TurnOutcome::Completed]);
}

#[test]
fn codex_accept_edits_grants_no_git_roots() {
    use serde_json::{Value, json};
    let root = Path::new("/tmp").join(format!("veyra-codex-policy-{}", ThreadId::new()));
    fs::create_dir(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let common = root.join("repository/.git");
    let git_dir = common.join("worktrees/linked");
    let worktree = root.join("worktree");
    let project = worktree.join("nested");
    for directory in [
        &git_dir,
        &project,
        &common.join("objects"),
        &common.join("refs"),
        &common.join("logs"),
    ] {
        fs::create_dir_all(directory).unwrap();
    }
    let common = common.canonicalize().unwrap();
    let git_dir = git_dir.canonicalize().unwrap();
    let worktree = worktree.canonicalize().unwrap();
    let project = project.canonicalize().unwrap();
    fs::write(common.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        common.join("config"),
        "[core]\nrepositoryformatversion = 0\nbare = false\n",
    )
    .unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(git_dir.join("commondir"), "../..\n").unwrap();
    fs::write(
        git_dir.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", git_dir.display()),
    )
    .unwrap();
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir", "--git-common-dir"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(output.status.success());
    let printed: Vec<_> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .map(|p| project.join(p).canonicalize().unwrap())
        .collect();
    assert_eq!(printed, [git_dir.clone(), common.clone()]);
    assert_ne!(printed[0], printed[1]);
    let mut records: Vec<Value> = fs::read_to_string(fixture("ok-reply"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for record in &mut records {
        if record["type"] == "expect_outbound"
            && matches!(
                record["line"]["method"].as_str(),
                Some("thread/start" | "turn/start")
            )
        {
            record["line"]["params"]["approvalPolicy"] = json!("on-request");
            if record["line"]["method"] == "thread/start" {
                record["line"]["params"]["sandbox"] = json!("workspace-write");
            } else {
                record["line"]["params"]["sandboxPolicy"] =
                    json!({"type":"workspaceWrite","networkAccess":true});
            }
        }
    }
    fs::write(
        root.join("t1-s1.jsonl"),
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let mut env = ChildEnv::isolated().with_binary(
        ProviderKind::Codex,
        env!("CARGO_BIN_EXE_veyra-fake-cli").into(),
    );
    env.fixtures = Some(root);
    Runtime::new().unwrap().block_on(async {
        let (out, mut events) = mpsc::channel(256);
        let turn = TurnId::new();
        let handle = Spawner::Cli(Box::new(env)).spawn(
            ProviderKind::Codex,
            SessionSpec {
                thread: ThreadId::new(),
                cwd: project,
                config: ThreadConfig {
                    mode: PermissionMode::AcceptEdits,
                    model: None,
                    effort: None,
                },
                native: None,
                root: false,
            },
            out,
            1,
        );
        let mut ended = None;
        tokio::time::timeout(Duration::from_secs(8), async {
            while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    handle
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                }
                match event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) => handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn,
                            text: "ok".into(),
                        })
                        .unwrap(),
                    ProviderEvent::Turn {
                        event: TurnEvent::TurnEnded { outcome, .. },
                        ..
                    } => ended = Some(outcome),
                    ProviderEvent::Session(SessionEvent::Exited { .. }) => break,
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            ended,
            Some(TurnOutcome::Completed),
            "workspaceWrite must grant no git metadata roots"
        );
    });
}

#[test]
fn codex_async_question_card() {
    engine_replay(
        "codex_async_question_card",
        &["async-question"],
        |engine, root, thread, runtime| {
            use veyra_engine::{
                commands::{Ack, Command},
                events::{ChangeEvent, UiEvent},
            };
            runtime.block_on(async {
                let mut events = engine.subscribe();
                engine
                    .send(Command::Send {
                        thread,
                        text: "Choose a color".into(),
                    })
                    .await
                    .unwrap()
                    .unwrap();
                let mut card = None;
                let mut pending_request = false;
                tokio::time::timeout(Duration::from_secs(5), async {
                    while card.is_none() || !pending_request {
                        if let UiEvent::Change { event, .. } = events.recv().await.unwrap() {
                            match event {
                                ChangeEvent::ItemAdded(item)
                                    if item.thread == thread
                                        && serde_json::to_value(&item.body)
                                            .unwrap()
                                            .get("AsyncQuestions")
                                            .is_some() =>
                                {
                                    card = Some(item.id)
                                }
                                ChangeEvent::RequestOpened(request) if request.thread == thread => {
                                    pending_request = true
                                }
                                _ => {}
                            }
                        }
                    }
                })
                .await
                .expect("async agent message never became a persisted question card");
                let card = card.unwrap();
                assert!(
                    engine
                        .snapshot(thread)
                        .await
                        .unwrap()
                        .items
                        .iter()
                        .any(|item| item.id == card)
                );
                let Ack::Turn(answer) = engine
                    .send(Command::ChooseAsyncOption {
                        item: card,
                        question: 0,
                        option: 0,
                    })
                    .await
                    .unwrap()
                    .unwrap()
                else {
                    panic!("missing answer turn")
                };
                tokio::time::timeout(Duration::from_secs(8), async {
                    loop {
                        if let UiEvent::Change {
                            event: ChangeEvent::TurnEnded(turn),
                            ..
                        } = events.recv().await.unwrap()
                            && turn.id == answer
                        {
                            assert!(matches!(
                                turn.state,
                                TurnState::Ended {
                                    outcome: TurnOutcome::Completed,
                                    ..
                                }
                            ));
                            break;
                        }
                    }
                })
                .await
                .expect("card answer waited for Continue or was not sent after interrupt");
                let db = rusqlite::Connection::open_with_flags(
                    root.join("data/veyra.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let projection = veyra_engine::store::load(&db).unwrap();
                assert!(projection.threads[&thread].hold.is_none());
                assert_eq!(projection.turns_for(thread).count(), 2);
                assert!(
                    projection
                        .turns_for(thread)
                        .all(|turn| matches!(turn.origin, TurnOrigin::User { .. }))
                );
                assert!(projection.items_for(thread).any(
                    |item| matches!(&item.body, ItemBody::UserText(text) if text == "Violet")
                ));
                let ItemBody::AsyncQuestions { text, questions } = &projection.items[&card].body
                else {
                    panic!("card disappeared")
                };
                assert!(text.contains("Should Veyra use Violet or Blue?"));
                assert_eq!(questions[0].selected, Some(0));
                assert!(
                    projection
                        .requests
                        .values()
                        .filter(|request| request.thread == thread)
                        .all(|request| matches!(request.state, RequestState::Expired { .. }))
                );
                assert!(
                    engine
                        .send(Command::ChooseAsyncOption {
                            item: card,
                            question: 0,
                            option: 1
                        })
                        .await
                        .unwrap()
                        .is_err()
                );
                assert_eq!(
                    veyra_engine::store::load(&db)
                        .unwrap()
                        .turns_for(thread)
                        .count(),
                    2
                );
            });
        },
    );
}

#[test]
fn codex_subagent_live_is_one_turn_without_child_text() {
    let events = standard("subagent-live", Inputs::default());
    assert_eq!(outcomes(&events), [TurnOutcome::Completed]);
    assert!(!events.iter().any(|e| matches!(
        e,
        ProviderEvent::Session(SessionEvent::ProviderTurnStarted { .. })
    )));
    let labels: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::ToolStarted { call, .. },
                ..
            } => Some(call.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(labels.contains(&"Wait for subagent"));
    let text: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::TextDelta { text, .. },
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        text.iter().filter(|s| **s == "ok").count(),
        1,
        "child text leaked alongside the parent's ok"
    );
}

#[test]
fn codex_subagent_live_shows_one_started_and_finished_activity() {
    let events = standard("subagent-live", Inputs::default());
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                event: TurnEvent::ToolStarted { key, call },
                ..
            } if call.name == "Subagent" => Some(key.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "the live capture's subagent start disappeared"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Turn { event:
        TurnEvent::ToolFinished { key, ok: true, .. }, .. } if key == &starts[0]))
            .count(),
        1
    );
    assert_eq!(outcomes(&events), [TurnOutcome::Completed]);
}
#[test]
fn codex_foreign_permission_docks_in_veyra_turn() {
    harness::foreign_request(
        ProviderKind::Codex,
        "foreign-permission-dock",
        Decision::AllowOnce,
    );
}
#[test]
fn codex_foreign_request_expiry_sends_cancel() {
    harness::actor_replay(
        ProviderKind::Codex,
        "foreign-permission-expiry",
        |engine, thread, events, runtime| {
            runtime.block_on(async {
                use veyra_engine::events::{ChangeEvent, UiEvent};
                tokio::time::timeout(Duration::from_secs(3), async {
                    let (mut opened, mut expired) = (None, false);
                    while let Some(event) = events.recv().await {
                        match event {
                            UiEvent::Change {
                                event: ChangeEvent::RequestOpened(request),
                                ..
                            } => {
                                assert_eq!(request.thread, thread);
                                assert_eq!(request.state, RequestState::Pending);
                                opened = Some((request.id, request.turn));
                            }
                            UiEvent::Change {
                                event: ChangeEvent::RequestClosed(request),
                                ..
                            } => {
                                assert_eq!(opened, Some((request.id, request.turn)));
                                assert!(matches!(request.state, RequestState::Expired { .. }));
                                expired = true;
                            }
                            UiEvent::Change {
                                event: ChangeEvent::TurnEnded(turn),
                                ..
                            } if turn.origin != TurnOrigin::Provider => {
                                assert_eq!(opened.map(|(_, turn)| turn), Some(turn.id));
                                assert!(expired);
                                assert!(matches!(
                                    turn.state,
                                    TurnState::Ended {
                                        outcome: TurnOutcome::Completed,
                                        ..
                                    }
                                ));
                                let marker = engine
                                    .child_env()
                                    .fixtures
                                    .unwrap()
                                    .join("native-cancel-checked");
                                while !marker.exists() {
                                    tokio::time::sleep(Duration::from_millis(5)).await;
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                    panic!("foreign request did not expire with a native cancel");
                })
                .await
                .unwrap();
            })
        },
    );
}
#[test]
fn codex_untagged_elicitation_expires_without_an_extra_reply() {
    harness::actor_replay(
        ProviderKind::Codex,
        "elicitation-without-turn-completes",
        |engine, thread, events, runtime| {
            runtime.block_on(async {
                use veyra_engine::{
                    commands::Command,
                    events::{ChangeEvent, UiEvent},
                };
                tokio::time::timeout(Duration::from_secs(5), async {
                    let (mut opened, mut expired, mut ended) = (None, false, 0);
                    while let Some(event) = events.recv().await {
                        match event {
                            UiEvent::Change {
                                event: ChangeEvent::RequestOpened(request),
                                ..
                            } => {
                                assert_eq!(request.thread, thread);
                                assert_eq!(request.state, RequestState::Pending);
                                opened = Some((request.id, request.turn));
                            }
                            UiEvent::Change {
                                event: ChangeEvent::RequestClosed(request),
                                ..
                            } => {
                                assert_eq!(opened, Some((request.id, request.turn)));
                                assert!(matches!(request.state, RequestState::Expired { .. }));
                                expired = true;
                            }
                            UiEvent::Change {
                                event: ChangeEvent::TurnEnded(turn),
                                ..
                            } => {
                                assert_eq!(turn.thread, thread);
                                assert!(
                                    matches!(
                                        turn.state,
                                        TurnState::Ended {
                                            outcome: TurnOutcome::Completed,
                                            ..
                                        }
                                    ),
                                    "{turn:?}"
                                );
                                ended += 1;
                                if ended == 1 {
                                    assert_eq!(opened.map(|(_, turn)| turn), Some(turn.id));
                                    assert!(expired);
                                    engine
                                        .send(Command::Send {
                                            thread,
                                            text: "second".into(),
                                        })
                                        .await
                                        .unwrap()
                                        .unwrap();
                                } else {
                                    assert!(
                                        engine
                                            .child_env()
                                            .fixtures
                                            .unwrap()
                                            .join("no-extra-reply-checked")
                                            .exists()
                                    );
                                    let snapshot = engine.snapshot(thread).await.unwrap();
                                    assert!(snapshot.items.iter().any(|item| matches!(&item.body,
                                    ItemBody::AssistantText { text, .. } if text == "second")));
                                    return;
                                }
                            }
                            _ => {}
                        }
                    }
                    panic!("the next prompt did not follow the expired elicitation");
                })
                .await
                .unwrap();
            })
        },
    );
}
