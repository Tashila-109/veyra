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
    provider::{env::ChildEnv, session, *},
};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(format!("{name}.jsonl"))
}
fn environment(name: &str, resumed: bool) -> (Scratch, ChildEnv) {
    let root = std::env::temp_dir().join(format!("veyra-replay-{}", ThreadId::new()));
    fs::create_dir(&root).unwrap();
    fs::copy(fixture(name), root.join("t1-s1.jsonl")).unwrap();
    if resumed {
        fs::copy(fixture("ok-reply"), root.join("t1-s2.jsonl")).unwrap();
    }
    fs::copy(fixture("probe-initialize"), root.join("probe.jsonl")).unwrap();
    let mut env = ChildEnv::isolated();
    env.claude_bin = Some(PathBuf::from(env!("CARGO_BIN_EXE_veyra-fake-cli")));
    env.fixtures = Some(root.clone());
    env.record = None;
    (Scratch(root), env)
}
fn stand_in(scratch: &Scratch, name: &str, script: &str) -> PathBuf {
    let binary = scratch.0.join(name);
    fs::write(&binary, script).unwrap();
    assert!(
        std::process::Command::new("/bin/chmod")
            .arg("700")
            .arg(&binary)
            .status()
            .unwrap()
            .success()
    );
    binary
}
async fn probe(env: &ChildEnv, cwd: &Path) -> Probe {
    let mut probes = Probes::default();
    probes
        .check(
            &tokio::runtime::Handle::current(),
            ProviderKind::Claude,
            env.clone(),
            cwd.into(),
        )
        .await
        .unwrap()
}
fn replay(name: &str, resume: bool) -> Vec<ProviderEvent> {
    let (_scratch, env) = environment(name, resume);
    Runtime::new().unwrap().block_on(async {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let (out, mut rx) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread,
                cwd: std::env::current_dir().unwrap(),
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: resume.then(|| NativeSessionId("missing-native".into())),

                root: false,
            },
            env,
            out,
            1,
        );
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut sent = false;
            let mut events = Vec::new();
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    handle
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                }
                if matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. })) && !sent {
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn,
                            text: "reply with the word ok".into(),
                        })
                        .unwrap();
                    sent = true;
                }
                let exited = matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                events.push(event);
                if exited {
                    break;
                }
            }
            events
        })
        .await
        .unwrap()
    })
}
fn text(events: &[ProviderEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
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
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                event: TurnEvent::TurnEnded { outcome, .. },
                ..
            } => Some(outcome.clone()),
            _ => None,
        })
        .collect()
}
#[test]
fn claude_ok_reply() {
    let events = replay("ok-reply", false);
    assert_eq!(text(&events), "ok");
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::Accepted,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event, ProviderEvent::Session(SessionEvent::Ready { cli_version, pgid, pid, .. })
            if cli_version.as_deref() == Some("2.1.294") && pgid == pid
    )));
}
#[test]
fn claude_unknown_json_shape_keeps_session() {
    let events = replay("unknown-json-shape", false);
    assert_eq!(text(&events), "ok");
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
}
#[test]
fn claude_bash_echo() {
    let events = replay("bash-echo", false);
    assert_eq!(
        text(&events),
        "Veyra checks the command.\nVeyra confirms the output."
    );
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert!(events.iter().any(|event| matches!(
        event, ProviderEvent::Turn { event: TurnEvent::ToolStarted { call, .. }, .. }
            if call.name == "Bash" && call.arguments["command"] == "echo veyra-probe"
    )));
    assert!(events.iter().any(|event| matches!(
        event, ProviderEvent::Turn {
            event: TurnEvent::ToolFinished { ok: true, output_tail: Some(output), .. }, ..
        } if output == "veyra-probe"
    )));
}
#[test]
fn claude_no_echo_unknown_mode_ends_turn() {
    let events = replay("result-no-echo-unknown-mode", false);
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert_eq!(text(&events), "ok");
}
#[test]
fn claude_echo_other_prompt_ignored() {
    let events = replay("result-echo-other-prompt", false);
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert_eq!(text(&events), "ok");
}
#[test]
fn claude_after_ack_no_echo_ignored() {
    let events = replay("result-after-ack", false);
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::Accepted,
            ..
        }
    )));
    assert_eq!(text(&events), "ok");
}
#[test]
fn claude_resume_not_found_starts_new_session() {
    let events = replay("resume-not-found", true);
    let notice = "The earlier conversation could not be resumed; this is a new session";
    assert!(events.iter().any(|event| matches!(
        event, ProviderEvent::Session(SessionEvent::Notice { text }) if text == notice
    )));
    let native = events
        .iter()
        .find_map(|event| match event {
            ProviderEvent::Session(SessionEvent::Ready { native, .. }) => Some(native),
            _ => None,
        })
        .unwrap();
    assert_ne!(native.0, "missing-native");
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert_eq!(text(&events), "ok");
}
#[test]
fn claude_crash_reports_stderr_tail() {
    let events = replay("crash-mid-turn", false);
    assert_eq!(text(&events), "before crash");
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Failed {
            message: concat!(
                "claude exited unexpectedly (exit code 7)\n",
                "Veyra fixture error: stream closed\nlast diagnostic line"
            )
            .into()
        }]
    );
}
#[test]
fn claude_result_is_error_success_fails() {
    let events = replay("result-is-error-success", false);
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Failed {
            message: "Veyra fixture failure".into()
        }]
    );
}
#[test]
fn claude_background_tasks_report_work() {
    let events = replay("background-tasks", false);
    let pending: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Session(SessionEvent::BackgroundWork { pending }) => Some(*pending),
            _ => None,
        })
        .collect();
    assert_eq!(pending, vec![true, false, false]);
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
}
#[test]
fn claude_probe_reads_models_and_account() {
    let (scratch, env) = environment("ok-reply", false);
    let path = scratch.0.join("probe.jsonl");
    let rows = fs::read_to_string(&path).unwrap();
    fs::write(&path, rows.replacen("2.1.295", "2.1.295 (Claude Code)", 1)).unwrap();
    let probe = Runtime::new().unwrap().block_on(probe(&env, &scratch.0));
    assert_eq!(probe.readiness, Readiness::Ready);
    assert_eq!(probe.version.as_deref(), Some("2.1.295"));
    assert_eq!(probe.models[0].value, "default");
    assert_eq!(probe.account, Some(serde_json::json!("<account>")));
}
#[test]
fn claude_probe_again_reaches_ready() {
    let (scratch, mut env) = environment("ok-reply", false);
    env.claude_bin = Some(scratch.0.join("installed-later"));
    let runtime = Runtime::new().unwrap();
    assert_eq!(
        runtime.block_on(probe(&env, &scratch.0)).readiness,
        Readiness::NotInstalled
    );
    fs::copy(
        env!("CARGO_BIN_EXE_veyra-fake-cli"),
        env.claude_bin.as_ref().unwrap(),
    )
    .unwrap();
    assert_eq!(
        runtime.block_on(probe(&env, &scratch.0)).readiness,
        Readiness::Ready
    );
    fs::copy(fixture("probe-no-account"), scratch.0.join("probe.jsonl")).unwrap();
    let probe = runtime.block_on(probe(&env, &scratch.0));
    assert_eq!(probe.readiness, Readiness::Ready);
    assert_eq!(probe.account, None);
    assert_eq!(probe.models[0].value, "default");
}

#[test]
fn cli_result_ends_provider_turn() {
    let events = replay("provider-turn-after-background", false);
    let ended: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                turn,
                event: TurnEvent::TurnEnded { outcome, .. },
            } => Some((*turn, outcome.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(ended.len(), 2);
    assert!(ended[0].0.is_some());
    assert_eq!(ended[0].1, TurnOutcome::Completed);
    assert_eq!(ended[1], (None, TurnOutcome::Completed));
}

#[test]
fn cli_reply_omitting_empty_reasoning_has_one_item() {
    let events = replay("provider-turn-after-background", false);
    let mut items = std::collections::BTreeMap::new();
    for event in events {
        if let ProviderEvent::Turn { turn: None, event } = event {
            match event {
                TurnEvent::TextDelta { key, text, .. } => {
                    let item = items
                        .entry(key)
                        .or_insert(("assistant", String::new(), false));
                    item.1.push_str(&text);
                }
                TurnEvent::ReasoningDelta { key, text } => {
                    let item = items
                        .entry(key)
                        .or_insert(("reasoning", String::new(), false));
                    item.1.push_str(&text);
                }
                TurnEvent::ItemEnded { key } => {
                    if let Some(item) = items.get_mut(&key) {
                        item.2 = true;
                    }
                }
                TurnEvent::Started | TurnEvent::TurnEnded { .. } => {}
                _ => panic!("unexpected CLI-started item: {event:?}"),
            }
        }
    }
    assert_eq!(
        items.into_values().collect::<Vec<_>>(),
        vec![(
            "assistant",
            "The background `sleep 20` finished with exit code 0.".into(),
            true,
        )]
    );
}

#[test]
fn claude_probe_records_initialize_with_masked_account_keys() {
    let (scratch, mut env) = environment("ok-reply", false);
    env.record = Some(scratch.0.join("recorded"));
    let mut wires: Vec<serde_json::Value> = fs::read_to_string(scratch.0.join("probe.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let account = serde_json::json!({"email":"fixture-person", "subscriptionType":"fixture-plan"});
    let initialize = wires
        .iter_mut()
        .find(|record| record["line"]["response"]["request_id"] == "<id:initialize>")
        .unwrap();
    initialize["line"]["response"]["response"]["account"] = account.clone();
    let text = wires
        .iter()
        .map(|wire| serde_json::to_string(wire).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(scratch.0.join("probe.jsonl"), text + "\n").unwrap();
    let probe = Runtime::new().unwrap().block_on(probe(&env, &scratch.0));
    assert_eq!(probe.account, Some(account));
    let path = env.record.unwrap().join("probe.jsonl");
    let captured: Vec<serde_json::Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(captured[0]["cli_version"], "2.1.295");
    assert_eq!(captured[1]["type"], "expect_outbound");
    let response = captured
        .iter()
        .find(|record| {
            record["line"]["response"]["response"]
                .get("account")
                .is_some()
        })
        .unwrap();
    assert_eq!(
        response["line"]["response"]["response"]["account"],
        serde_json::json!({"email":"<any>", "subscriptionType":"<any>"})
    );
    assert!(veyra_engine::platform::is_private(&path).unwrap());
    assert!(!fs::read_to_string(path).unwrap().contains("fixture-person"));
}

#[test]
fn cli_stream_close_fails_provider_turn_even_on_exit_zero() {
    let events = replay("provider-turn-exit-zero", false);
    assert_eq!(text(&events), "okunfinished");
    let message =
        "claude exited unexpectedly (exit code 0)\nVeyra stream closed\npassword=[redacted]";
    assert_eq!(
        outcomes(&events),
        vec![
            TurnOutcome::Completed,
            TurnOutcome::Failed {
                message: message.into(),
            }
        ]
    );
    assert!(
        matches!(events.last(), Some(ProviderEvent::Session(SessionEvent::Exited {
        crash: Some(crash),
    })) if crash == message)
    );
}

#[test]
fn cli_turn_starts_before_text_at_lifecycle_or_message_start() {
    for scenario in ["provider-turn-lifecycle", "provider-turn-message-start"] {
        let events = replay(scenario, false);
        let starts: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| match event {
                ProviderEvent::Turn {
                    turn: None,
                    event: TurnEvent::Started,
                } => Some(index),
                _ => None,
            })
            .collect();
        let text_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    ProviderEvent::Turn {
                        turn: None,
                        event: TurnEvent::TextDelta { .. }
                    }
                )
            })
            .unwrap();
        assert_eq!(starts.len(), 1, "{scenario}");
        assert!(starts[0] < text_index, "{scenario}");
        assert_eq!(text(&events), "okunfinished");
        assert_eq!(
            outcomes(&events),
            vec![TurnOutcome::Completed, TurnOutcome::Completed]
        );
    }
}

#[test]
fn cancelling_probe_before_it_starts_runs_no_binary() {
    let (scratch, mut env) = environment("ok-reply", false);
    let marker = scratch.0.join("probe-marker");
    let binary = stand_in(
        &scratch,
        "probe-cli",
        "#!/bin/sh\nprintf 'started\\n' >> probe-marker\nprintf 'fixture-version\\n'\n",
    );
    let untouched = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(978285600);
    let file = fs::OpenOptions::new().write(true).open(&binary).unwrap();
    file.set_times(fs::FileTimes::new().set_accessed(untouched))
        .unwrap();
    env.claude_bin = Some(binary.clone());
    env.fixtures = None;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            assert_eq!(probe(&env, &scratch.0).await.readiness, Readiness::Ready);
            assert_eq!(fs::read_to_string(&marker).unwrap(), "started\nstarted\n");
            assert_ne!(
                fs::metadata(&binary).unwrap().accessed().unwrap(),
                untouched
            );
            fs::remove_file(&marker).unwrap();
            file.set_times(fs::FileTimes::new().set_accessed(untouched))
                .unwrap();

            let mut probes = Probes::default();
            let reply = probes.check(
                &tokio::runtime::Handle::current(),
                ProviderKind::Claude,
                env,
                scratch.0.clone(),
            );
            drop(probes);
            let result = tokio::time::timeout(Duration::from_secs(30), reply)
                .await
                .unwrap();
            assert_eq!(
                fs::metadata(&binary).unwrap().accessed().unwrap(),
                untouched,
                "cancelled probe executed its binary"
            );
            assert!(!marker.exists(), "cancelled probe executed its binary");
            assert!(
                result.is_err(),
                "cancelled probe returned a readiness reply"
            );
        });
}

#[test]
fn replacing_hung_probe_ends_its_group_before_ready_reply() {
    let (scratch, mut env) = environment("ok-reply", false);
    let mut rows: Vec<serde_json::Value> = fs::read_to_string(scratch.0.join("probe.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    rows.truncate(2);
    rows.push(serde_json::json!({"type":"expect_outbound", "line":{"type":"user"}}));
    fs::write(
        scratch.0.join("probe.jsonl"),
        rows.iter()
            .map(|row| row.to_string())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let wrapper = stand_in(
        &scratch,
        "probe-cli",
        &format!(
            "#!/bin/sh\nprintf '%s\n' \"$$\" > probe-pid\nexec '{}' \"$@\"\n",
            env!("CARGO_BIN_EXE_veyra-fake-cli")
        ),
    );
    env.claude_bin = Some(wrapper);
    Runtime::new().unwrap().block_on(async {
        let mut probes = Probes::default();
        let first = probes.check(
            &tokio::runtime::Handle::current(),
            ProviderKind::Claude,
            env.clone(),
            scratch.0.clone(),
        );
        let pid: u32 = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(text) = fs::read_to_string(scratch.0.join("probe-pid"))
                    && let Ok(pid) = text.trim().parse()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        assert!(veyra_engine::platform::group_exists(pid).unwrap());
        fs::copy(fixture("probe-initialize"), scratch.0.join("probe.jsonl")).unwrap();
        env.claude_bin = Some(PathBuf::from(env!("CARGO_BIN_EXE_veyra-fake-cli")));
        let second = probes.check(
            &tokio::runtime::Handle::current(),
            ProviderKind::Claude,
            env,
            scratch.0.clone(),
        );
        let ready = tokio::time::timeout(Duration::from_secs(30), second)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.readiness, Readiness::Ready);
        assert_eq!(ready.version.as_deref(), Some("2.1.295"));
        assert_eq!(ready.models[0].value, "default");
        assert!(!veyra_engine::platform::group_exists(pid).unwrap());
        first.await.unwrap();
    });
}

fn controlled_replay(
    name: &str,
    prompts: &[&str],
    interrupt_text: Option<&str>,
) -> Vec<ProviderEvent> {
    controlled_replay_with(name, prompts, interrupt_text, |_| {})
}
fn controlled_replay_with(
    name: &str,
    prompts: &[&str],
    interrupt_text: Option<&str>,
    edit: impl FnOnce(&Scratch),
) -> Vec<ProviderEvent> {
    let (scratch, env) = environment(name, false);
    edit(&scratch);
    Runtime::new().unwrap().block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: scratch.0.clone(),
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: None,

                root: false,
            },
            env,
            out,
            1,
        );
        let mut next = 0;
        let mut turn = TurnId::new();
        let mut interrupted = false;
        let events = tokio::time::timeout(Duration::from_secs(10), async {
            let mut events = Vec::new();
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    handle
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                }
                let start = matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. }))
                    && next == 0
                    || matches!(
                        event,
                        ProviderEvent::Turn {
                            event: TurnEvent::TurnEnded { .. },
                            ..
                        }
                    );
                if start && let Some(text) = prompts.get(next) {
                    turn = TurnId::new();
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn,
                            text: (*text).into(),
                        })
                        .unwrap();
                    next += 1;
                }
                if !interrupted
                    && matches!(&event, ProviderEvent::Turn {
                    event: TurnEvent::TextDelta { text, .. }, ..
                } if interrupt_text.is_some_and(|trigger| text.ends_with(trigger)))
                {
                    handle
                        .control
                        .send(SessionControl::Interrupt {
                            turn,
                            reason: StopReason::User,
                        })
                        .unwrap();
                    interrupted = true;
                }
                let exited = matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                events.push(event);
                if exited {
                    break;
                }
            }
            events
        })
        .await;
        handle.control.send(SessionControl::Shutdown).ok();
        events.unwrap()
    })
}
#[test]
fn claude_assistant_error_fails_and_parks() {
    let events = replay("assistant-error-signed-out", false);
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::TurnEnded { park: true, .. },
            ..
        }
    )));
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Failed {
            message:
                "Claude is signed out. Sign in from a terminal with `claude`, then send again."
                    .into(),
        }]
    );
    assert!(matches!(
        events.last(),
        Some(ProviderEvent::Session(SessionEvent::Exited { crash: None }))
    ));
}
#[test]
fn claude_interrupt_result_is_interrupted() {
    let events = controlled_replay(
        "interrupt-error-during-execution",
        &["first"],
        Some("partial"),
    );
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
    assert_eq!(text(&events), "partial");
}
fn subagent_events(late: bool) -> Vec<ProviderEvent> {
    controlled_replay_with(
        "subagent-task",
        &["reply with the word ok"],
        None,
        |scratch| {
            if !late {
                return;
            }
            let path = scratch.0.join("t1-s1.jsonl");
            let mut rows: Vec<serde_json::Value> = fs::read_to_string(&path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let parent = rows
                .iter()
                .find_map(|row| row["line"]["parent_tool_use_id"].as_str())
                .unwrap()
                .to_owned();
            let exit = rows.pop().unwrap();
            for owner in [&parent, "missing-call"] {
                for kind in ["assistant", "user", "stream_event"] {
                    rows.push(serde_json::json!({"type":"emit_inbound", "line":{
                        "type":kind, "parent_tool_use_id":owner,
                        "message":{"id":"late", "content":[{"type":"text","text":"late child"}]},
                        "event":{"type":"content_block_delta","index":0,
                            "delta":{"type":"text_delta","text":"late child"}}
                    }}));
                }
            }
            rows.push(exit);
            let text = rows
                .iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(path, text + "\n").unwrap();
        },
    )
}
fn assert_subagent_row(events: &[ProviderEvent], root: &str) {
    assert_eq!(text(events), root);
    assert_eq!(outcomes(events), vec![TurnOutcome::Completed]);
    let calls: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                event: TurnEvent::ToolStarted { key, call },
                ..
            } => Some((key, call)),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 1);
    assert!(matches!(calls[0].1.name.as_str(), "Task" | "Agent"));
    let output: String = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                event: TurnEvent::ToolOutput { key, text },
                ..
            } => {
                assert_eq!(key, calls[0].0);
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    if root == "done" {
        assert!(output.contains("List the files in <cwd>"));
        assert!(output.contains("alpha.txt"));
        assert!(output.contains("beta.txt"));
    } else {
        assert!(output.contains("child words"));
    }
    assert!(!output.contains("late child"));
    assert!(!output.contains("unowned words"));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                ProviderEvent::Turn {
                    event: TurnEvent::ToolFinished { .. },
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProviderEvent::Turn { turn: None, .. }))
    );
}
#[test]
fn claude_subagent_frames_feed_tool_row() {
    assert_subagent_row(&subagent_events(false), "done");
}
#[test]
fn claude_subagent_late_frames_dropped() {
    assert_subagent_row(&subagent_events(true), "done");
    assert_subagent_row(&replay("subagent-late-frames", false), "");
}
#[test]
fn claude_compact_and_clear_add_notices() {
    let events = controlled_replay("compact-and-clear", &["/compact", "/clear"], None);
    let notices: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Session(SessionEvent::Notice { text }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        [
            "Claude compacted this conversation.",
            "Claude cleared this conversation. The CLI no longer sees the messages above this notice."
        ]
    );
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Completed, TurnOutcome::Completed]
    );
}
#[test]
fn claude_bad_lines_keep_session() {
    let events = replay("bad-lines", false);
    assert_eq!(text(&events), "ok");
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Session(SessionEvent::Notice { .. })))
            .count(),
        1
    );
}
#[test]
fn claude_retry_limit_fallback_notices() {
    let events = replay("notices", false);
    let notices: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Session(SessionEvent::Notice { text }) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        [
            "Claude is retrying the API (attempt 2) in 1000 ms.",
            "Claude usage limit: rejected; resets at 12345.",
            "Claude is falling back to another model."
        ]
    );
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
}
#[test]
fn claude_interrupt_honoured() {
    let recorded = controlled_replay("interrupt-honoured", &["first"], Some("9: It equ"));
    assert_eq!(
        outcomes(&recorded),
        vec![TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
    assert!(text(&recorded).starts_with("1: It is the multiplicative identity."));
    assert!(text(&recorded).ends_with("9: It equ"));
    assert!(recorded.iter().all(|event| !matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::TurnEnded { park: true, .. },
            ..
        }
    )));

    let events = controlled_replay("interrupt-continued", &["first", "second"], Some("partial"));
    assert_eq!(text(&events), "partialok");
    assert_eq!(
        outcomes(&events),
        vec![
            TurnOutcome::Interrupted {
                reason: "Stopped by you".into()
            },
            TurnOutcome::Completed
        ]
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. })))
            .count(),
        1
    );
}
#[test]
fn claude_interrupt_ignored() {
    let events = controlled_replay("interrupt-ignored", &["first"], Some("partial"));
    assert_eq!(text(&events), "partial");
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
}

#[test]
fn ladder_synthesizes_interrupted_end() {
    let events = controlled_replay("interrupt-ignored", &["first"], Some("partial"));
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::TurnEnded { park: true, .. },
            ..
        }
    )));
    let pid = events
        .iter()
        .find_map(|event| match event {
            ProviderEvent::Session(SessionEvent::Ready { pid, .. }) => Some(*pid),
            _ => None,
        })
        .unwrap();
    assert!(!veyra_engine::platform::group_exists(pid).unwrap());
}
#[test]
fn ladder_stops_at_cli_turn_end() {
    let started = std::time::Instant::now();
    let events = controlled_replay("interrupt-continued", &["first", "second"], Some("partial"));
    assert!(started.elapsed() >= Duration::from_millis(4500));
    assert_eq!(text(&events), "partialok");
    assert_eq!(
        outcomes(&events),
        vec![
            TurnOutcome::Interrupted {
                reason: "Stopped by you".into()
            },
            TurnOutcome::Completed
        ]
    );
    assert!(
        events
            .iter()
            .filter_map(|event| match event {
                ProviderEvent::Turn {
                    event: TurnEvent::TurnEnded { park, .. },
                    ..
                } => Some(*park),
                _ => None,
            })
            .all(|park| !park)
    );
}
#[test]
fn shutdown_during_spawn_never_ready() {
    let (scratch, env) = environment("ok-reply", false);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (out, mut rx) = mpsc::channel(256);
            let handle = session::spawn(
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: scratch.0.clone(),
                    config: ThreadConfig {
                        model: None,
                        effort: None,
                        mode: PermissionMode::AskFirst,
                    },
                    native: None,

                    root: false,
                },
                env,
                out,
                1,
            );
            handle.control.send(SessionControl::Shutdown).unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let EngineMsg::Provider { event, .. } = rx.recv().await.unwrap() else {
                        panic!()
                    };
                    assert!(!matches!(
                        event,
                        ProviderEvent::Session(SessionEvent::Ready { .. })
                    ));
                    if matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. })) {
                        break;
                    }
                }
            })
            .await
            .unwrap();
        });
}

#[test]
fn claude_probe_signed_out() {
    let (scratch, env) = environment("ok-reply", false);
    fs::copy(fixture("probe-signed-out"), scratch.0.join("probe.jsonl")).unwrap();
    let runtime = Runtime::new().unwrap();
    let result = runtime.block_on(probe(&env, &scratch.0));
    assert_eq!(result.readiness, Readiness::SignedOut);
    assert_eq!(result.models.len(), 5);
    fs::copy(fixture("probe-no-account"), scratch.0.join("probe.jsonl")).unwrap();
    assert_eq!(
        runtime.block_on(probe(&env, &scratch.0)).readiness,
        Readiness::Ready
    );
}

fn request_replay(
    name: &str,
    decisions: &[Decision],
    stop_after: Option<usize>,
) -> (Vec<ProviderEvent>, bool) {
    let (scratch, env) = environment(name, false);
    let events = Runtime::new().unwrap().block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: scratch.0.clone(),
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: None,

                root: false,
            },
            env,
            out,
            1,
        );
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            let mut events = Vec::new();
            let mut requests = 0;
            let turn = TurnId::new();
            let mut started = false;
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    handle
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                }
                if !started && matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. })) {
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn,
                            text: "test".into(),
                        })
                        .unwrap();
                    started = true;
                }
                if let ProviderEvent::Turn {
                    event: TurnEvent::RequestOpened { native, .. },
                    ..
                } = &event
                {
                    if let Some(decision) = decisions.get(requests) {
                        handle
                            .control
                            .send(SessionControl::Answer {
                                native: native.clone(),
                                reply: decision.clone(),
                            })
                            .unwrap();
                    }
                    requests += 1;
                    if stop_after == Some(requests) {
                        handle
                            .control
                            .send(SessionControl::Interrupt {
                                turn,
                                reason: StopReason::User,
                            })
                            .unwrap();
                    }
                }
                let exited = matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                events.push(event);
                if exited {
                    break;
                }
            }
            events
        })
        .await;
        handle.control.send(SessionControl::Shutdown).ok();
        result.unwrap()
    });
    let created = scratch.0.join("veyra-check.txt").exists();
    (events, created)
}
#[test]
fn claude_approval_allow() {
    let (events, created) = request_replay("approval-allow", &[Decision::AllowOnce], None);
    assert!(created);
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
}
#[test]
fn claude_approval_deny() {
    let (events, created) = request_replay("approval-deny", &[Decision::Deny], None);
    assert!(!created);
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::Turn {
        event: TurnEvent::RequestOpened { kind: RequestKind::Approval { action, .. }, .. }, ..
    } if matches!(&action.target, ApprovalTarget::Command { command, .. } if command == "touch veyra-check.txt")))
    );
}
#[test]
fn claude_question_answered() {
    let (events, _) = request_replay(
        "question",
        &[Decision::Answers(vec![Answer {
            text: "Short".into(),
        }])],
        None,
    );
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    let expected = vec![Question {
        text: "Choose a route".into(),
        options: vec!["Short".into(), "Long".into()],
    }];
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::Turn {
        event: TurnEvent::RequestOpened { kind: RequestKind::Question { questions }, .. }, ..
    } if questions == &expected))
    );
}
#[test]
fn claude_unknown_request_gets_error() {
    assert_eq!(
        outcomes(&replay("unknown-request", false)),
        vec![TurnOutcome::Completed]
    );
}
#[test]
fn claude_stop_cancels_pending_request() {
    let (events, created) = request_replay("stop-pending-approval", &[], Some(2));
    assert!(!created);
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }]
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                ProviderEvent::Turn {
                    event: TurnEvent::RequestOpened { .. },
                    ..
                }
            ))
            .count(),
        2
    );
}
#[test]
fn late_interrupt_after_end_does_not_kill_next_prompt() {
    let (scratch, mut env) = environment("ok-reply", false);
    env.fixtures = None;
    env.claude_bin = Some(stand_in(
        &scratch,
        "late-interrupt-cli",
        r#"#!/usr/bin/env python3
import json, sys, time
count = 0
for line in sys.stdin:
    wire = json.loads(line)
    if wire.get('request', {}).get('subtype') == 'initialize':
        print(json.dumps({'type':'control_response','response':{'subtype':'success',
            'request_id':wire['request_id'],'response':{}}}), flush=True)
    if wire.get('type') == 'user':
        count += 1
        if count == 2: time.sleep(5)
        print(json.dumps({'type':'result','subtype':'success','is_error':False,
            'user_message_uuid':wire['uuid']}), flush=True)
        if count == 2: break
"#,
    ));
    let events = Runtime::new().unwrap().block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: scratch.0.clone(),
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: None,

                root: false,
            },
            env,
            out,
            1,
        );
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            let mut events = Vec::new();
            let mut ends = 0;
            let turn = TurnId::new();
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    handle
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                }
                if matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. })) {
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn,
                            text: "first".into(),
                        })
                        .unwrap();
                }
                if matches!(
                    event,
                    ProviderEvent::Turn {
                        event: TurnEvent::TurnEnded { .. },
                        ..
                    }
                ) {
                    ends += 1;
                    if ends == 1 {
                        handle
                            .control
                            .send(SessionControl::Interrupt {
                                turn,
                                reason: StopReason::User,
                            })
                            .unwrap();
                        handle
                            .control
                            .send(SessionControl::StartTurn {
                                turn: TurnId::new(),
                                text: "second".into(),
                            })
                            .unwrap();
                    }
                }
                let exited = matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                events.push(event);
                if exited {
                    break;
                }
            }
            events
        })
        .await;
        handle.control.send(SessionControl::Shutdown).ok();
        result.unwrap()
    });
    assert_eq!(
        outcomes(&events),
        vec![TurnOutcome::Completed, TurnOutcome::Completed]
    );
}

#[test]
fn chatty_probe_stderr_does_not_block_initialize() {
    let (scratch, mut env) = environment("ok-reply", false);
    env.fixtures = None;
    env.claude_bin = Some(stand_in(
        &scratch,
        "chatty-probe",
        r#"#!/usr/bin/env python3
import json, os, sys
if '--version' in sys.argv:
    print('2.1.296')
else:
    with open('probe-pid', 'w') as marker: marker.write(str(os.getpid()))
    wire = json.loads(sys.stdin.readline())
    sys.stderr.write('x' * (2 * 1024 * 1024))
    sys.stderr.flush()
    print(json.dumps({'type':'control_response','response':{'subtype':'success',
        'request_id':wire['request_id'],'response':{'models':[{'value':'default'}]}}}), flush=True)
"#,
    ));
    Runtime::new().unwrap().block_on(async {
        let mut probes = Probes::default();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            probes.check(
                &tokio::runtime::Handle::current(),
                ProviderKind::Claude,
                env.clone(),
                scratch.0.clone(),
            ),
        )
        .await;
        drop(probes);
        if let Ok(pid) = fs::read_to_string(scratch.0.join("probe-pid")) {
            let pid: u32 = pid.parse().unwrap();
            let cleaned = tokio::time::timeout(Duration::from_secs(3), async {
                while veyra_engine::platform::is_alive(pid).unwrap() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await;
            if cleaned.is_err() {
                let _ =
                    veyra_engine::platform::signal_group(pid, veyra_engine::platform::Signal::Kill);
            }
            cleaned.unwrap();
        }
        let probe = result.expect("stderr blocked initialize").unwrap();
        assert_eq!(probe.readiness, Readiness::Ready);
        assert_eq!(probe.models[0].value, "default");
    });
}

#[test]
fn fake_touch_rejects_absolute_and_parent_paths() {
    let (scratch, env) = environment("ok-reply", false);
    let transcript = scratch.0.join("touch.jsonl");
    for path in ["/tmp/veyra-invalid-touch", "../veyra-invalid-touch"] {
        let record = serde_json::json!({"type":"touch", "path":path});
        fs::write(&transcript, format!("{record}\n")).unwrap();
        let output = std::process::Command::new(env.claude_bin.as_ref().unwrap())
            .env("VEYRA_FAKE_FIXTURE", &transcript)
            .current_dir(&scratch.0)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&output.stderr).contains("touch path must stay within"));
    }
    assert!(!scratch.0.join("veyra-invalid-touch").exists());
}

#[derive(Clone, Copy)]
enum FollowupStop {
    None,
    Previous,
    Provider,
    Storage,
}

fn provider_followup(stop: FollowupStop) {
    let scratch = Scratch(std::env::temp_dir().join(format!("veyra-followup-{}", ThreadId::new())));
    fs::create_dir(&scratch.0).unwrap();
    let binary = stand_in(
        &scratch,
        "owned-provider-stand-in",
        r#"#!/usr/bin/env python3
import json, sys, time
for line in sys.stdin:
    wire = json.loads(line)
    if wire.get('request', {}).get('subtype') == 'initialize':
        print(json.dumps({'type': 'control_response', 'response': {'subtype': 'success',
            'request_id': wire['request_id'], 'response': {}}}), flush=True)
    if wire.get('type') == 'user':
        print(json.dumps({'type': 'result', 'subtype': 'success', 'is_error': False,
            'user_message_uuid': wire['uuid']}), flush=True)
        print(json.dumps({'type': 'assistant', 'message': {'id': 'later-provider',
            'content': [{'type': 'text', 'text': 'background follow-up'}]}}), flush=True)
        time.sleep(5)
        print(json.dumps({'type': 'result', 'subtype': 'success', 'is_error': False}), flush=True)
"#,
    );
    let mut env = ChildEnv::isolated();
    env.claude_bin = Some(binary);
    env.fixtures = None;
    env.record = None;
    let first = TurnId::new();
    let outcomes = Runtime::new().unwrap().block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: scratch.0.clone(),
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: None,

                root: false,
            },
            env,
            out,
            1,
        );
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            let mut outcomes = Vec::new();
            let mut old_end = false;
            let mut provider = None;
            let mut interrupted = false;
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    handle
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                }
                if matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. })) {
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn: first,
                            text: "first".into(),
                        })
                        .unwrap();
                }
                if let ProviderEvent::Session(SessionEvent::ProviderTurnStarted { provider_turn }) =
                    &event
                {
                    provider = Some(*provider_turn);
                }
                if let ProviderEvent::Turn {
                    turn,
                    event: TurnEvent::TurnEnded { outcome, .. },
                } = &event
                {
                    if *turn == Some(first) {
                        old_end = true;
                    }
                    outcomes.push(outcome.clone());
                    if outcomes.len() == 2 {
                        handle.control.send(SessionControl::Shutdown).ok();
                    }
                }
                if !matches!(stop, FollowupStop::None)
                    && old_end
                    && !interrupted
                    && matches!(
                        event,
                        ProviderEvent::Turn {
                            turn: None,
                            event: TurnEvent::TextDelta { .. }
                        }
                    )
                {
                    interrupted = true;
                    let control = match stop {
                        FollowupStop::Storage => {
                            SessionControl::InterruptActive(StopReason::StorageBlocked)
                        }
                        FollowupStop::Provider => SessionControl::Interrupt {
                            turn: provider.unwrap_or(first),
                            reason: StopReason::User,
                        },
                        FollowupStop::Previous => SessionControl::Interrupt {
                            turn: first,
                            reason: StopReason::User,
                        },
                        FollowupStop::None => unreachable!(),
                    };
                    handle.control.send(control).unwrap();
                }
                if matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. })) {
                    break;
                }
            }
            outcomes
        })
        .await;
        handle.control.send(SessionControl::Shutdown).ok();
        result.unwrap()
    });
    assert_eq!(
        outcomes,
        vec![
            TurnOutcome::Completed,
            if matches!(stop, FollowupStop::Provider | FollowupStop::Storage) {
                TurnOutcome::Interrupted {
                    reason: if matches!(stop, FollowupStop::Storage) {
                        StopReason::StorageBlocked.text().into()
                    } else {
                        StopReason::User.text().into()
                    },
                }
            } else {
                TurnOutcome::Completed
            }
        ]
    );
}

#[test]
fn provider_followup_without_late_stop_completes() {
    provider_followup(FollowupStop::None);
}
#[test]
fn late_stop_after_user_end_does_not_stop_provider_followup() {
    provider_followup(FollowupStop::Previous);
}

#[test]
fn stop_targets_reported_provider_turn() {
    provider_followup(FollowupStop::Provider);
}

#[test]
fn storage_stop_interrupts_unregistered_provider_turn() {
    provider_followup(FollowupStop::Storage);
}

fn queued_prompt_with_followup(followup: bool) {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("veyra-start-followup-{}", ThreadId::new())));
    fs::create_dir(&scratch.0).unwrap();
    let script = format!(
        "#!/usr/bin/env python3\nimport json, sys, time\nfollowup = {}\nusers = 0\nfor line in sys.stdin:\n    \
            wire = json.loads(line)\n    if wire.get('request', {{}}).get('subtype') == 'initialize':\n        \
                print(json.dumps({{'type':'control_response','response':{{'subtype':'success',\n            're\
                    quest_id':wire['request_id'],'response':{{}}}}}}), flush=True)\n    if wire.get('type') == \
                        'user':\n        users += 1\n        if users == 1:\n            print(json.dumps({{'ty\
                            pe':'result','subtype':'success','is_error':False,\n                'user_message_u\
                                uid':wire['uuid']}}), flush=True)\n            if followup:\n                pr\
                                    int(json.dumps({{'type':'assistant','message':{{'id':'followup','content':[\
                                        \n                    {{'type':'text','text':'provider followup'}}]}}}}\
                                            ), flush=True)\n                time.sleep(.5)\n                pri\
                                                nt(json.dumps({{'type':'result','subtype':'success','is_error':\
                                                False}}), flush=True)\n        else:\n            print(json.du\
                                                mps({{'type':'assistant','message':{{'id':'next-user','content'\
                                                :[\n                {{'type':'text','text':'actual queued reply\
                                                '}}]}}}}), flush=True)\n            print(json.dumps({{'type':'\
                                                result','subtype':'success','is_error':False,\n                \
                                                'user_message_uuid':wire['uuid']}}), flush=True)\n            break\n",
        if followup { "True" } else { "False" }
    );
    let binary = stand_in(&scratch, "owned-followup-stand-in", &script);
    let mut env = ChildEnv::isolated();
    env.claude_bin = Some(binary);
    env.fixtures = None;
    env.record = None;
    let first = TurnId::new();
    let next = TurnId::new();
    let events = Runtime::new().unwrap().block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: scratch.0.clone(),
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: None,

                root: false,
            },
            env,
            out,
            1,
        );
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut events = Vec::new();
            let mut sent_next = false;
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    handle
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                }
                if matches!(&event, ProviderEvent::Session(SessionEvent::Ready { .. })) {
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn: first,
                            text: "first".into(),
                        })
                        .unwrap();
                }
                let send_next = if followup {
                    matches!(
                        &event,
                        ProviderEvent::Turn {
                            turn: None,
                            event: TurnEvent::TextDelta { .. }
                        }
                    )
                } else {
                    matches!(&event, ProviderEvent::Turn {
                        turn:Some(id), event:TurnEvent::TurnEnded { .. }
                    } if *id == first)
                };
                if send_next && !sent_next {
                    sent_next = true;
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn: next,
                            text: "queued user prompt".into(),
                        })
                        .unwrap();
                }
                if matches!(
                    &event,
                    ProviderEvent::Turn {
                        turn: None,
                        event: TurnEvent::TurnEnded { .. }
                    }
                ) {
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn: next,
                            text: "queued user prompt".into(),
                        })
                        .unwrap();
                }
                let exited = matches!(&event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                events.push(event);
                if exited {
                    break;
                }
            }
            events
        })
        .await;
        handle.control.send(SessionControl::Shutdown).ok();
        result.unwrap()
    });
    let ends = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::Turn {
                turn,
                event: TurnEvent::TurnEnded { outcome, .. },
            } => Some((*turn, outcome.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::Turn {
        turn: Some(id), event: TurnEvent::TextDelta { text, .. }
    } if *id == next && text == "actual queued reply"))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event,
        ProviderEvent::Session(SessionEvent::StartTurnRefused { turn }) if *turn == next))
            .count(),
        usize::from(followup)
    );
    let expected = if followup {
        vec![
            (Some(first), TurnOutcome::Completed),
            (None, TurnOutcome::Completed),
            (Some(next), TurnOutcome::Completed),
        ]
    } else {
        vec![
            (Some(first), TurnOutcome::Completed),
            (Some(next), TurnOutcome::Completed),
        ]
    };
    assert_eq!(
        ends, expected,
        "StartTurn must preserve the already active Provider turn and its result"
    );
}
#[test]
fn queued_prompt_behind_provider_followup_keeps_three_turns() {
    queued_prompt_with_followup(true);
}
#[test]
fn queued_prompt_without_provider_followup_completes() {
    queued_prompt_with_followup(false);
}

fn blocked_interrupt_probe(stop: bool) {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("veyra-blocked-writer-{}", ThreadId::new())));
    fs::create_dir(&scratch.0).unwrap();
    let binary = stand_in(
        &scratch,
        "owned-blocked-writer",
        "#!/usr/bin/env python3\nimport json, os, pathlib, select, sys, time\nbuffer = b''\ndef read_line(timeo\
            ut=5):\n    global buffer\n    deadline = time.monotonic() + timeout\n    while b'\\n' not in buffe\
                r:\n        remaining = deadline - time.monotonic()\n        if remaining <= 0 or not select.se\
                    lect([0], [], [], remaining)[0]: return None\n        data = os.read(0, 65536)\n        if \
                        not data: return None\n        buffer += data\n    line, buffer = buffer.split(b'\\n', \
                            1)\n    return json.loads(line)\ndef emit(value): print(json.dumps(value), flush=Tr\
                                ue)\ndef wait_file(name):\n    deadline = time.monotonic() + 10\n    while not \
                                    pathlib.Path(name).exists():\n        if time.monotonic() >= deadline: rais\
                                        e RuntimeError(name)\n        time.sleep(.01)\nwire = read_line()\nemit\
                                            ({'type':'control_response','response':{'subtype':'success','reques\
                                                t_id':wire['request_id'],'response':{}}})\nuser = read_line()\n\
                                                emit({'type':'assistant','message':{'id':'first','content':[{'t\
                                                ype':'text','text':'first turn'}]}})\nemit({'type':'control_req\
                                                uest','request_id':'x' * (1024*1024),'request':{'subtype':'unsu\
                                                pported'}})\ntime.sleep(.2)\npathlib.Path('writer-blocked').tou\
                                                ch()\nwait_file('finish-a')\nemit({'type':'result','subtype':'s\
                                                uccess','is_error':False,'user_message_uuid':user['uuid']})\nem\
                                                it({'type':'assistant','message':{'id':'later','content':[{'typ\
                                                e':'text','text':'provider B'}]}})\nwait_file('read-stdin')\nre\
                                                ply = read_line()\nassert reply['type'] == 'control_response'\n\
                                                interrupt = read_line(1)\nif interrupt and interrupt.get('reque\
                                                st',{}).get('subtype') == 'interrupt':\n    pathlib.Path('inter\
                                                rupt-delivered-to-b').touch()\n    emit({'type':'result','subty\
                                                pe':'error_during_execution','is_error':False,'terminal_reason'\
                                                :'aborted_streaming'})\nelse:\n    emit({'type':'result','subty\
                                                pe':'success','is_error':False})\nwhile read_line(5) is not Non\
                                                e: pass\n",
    );
    let mut env = ChildEnv::isolated();
    env.claude_bin = Some(binary);
    env.fixtures = None;
    env.record = None;
    let first = TurnId::new();
    let ends = Runtime::new().unwrap().block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = session::spawn(SessionSpec {
            thread: ThreadId::new(), cwd: scratch.0.clone(),
            config: ThreadConfig { model: None, effort: None, mode: PermissionMode::AskFirst },
            native: None,
                    root: false,
        }, env, out, 1);
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            let mut sent = false;
            let mut released = false;
            let mut ends = Vec::new();
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        if sent && !released && scratch.0.join("writer-blocked").exists() {
                            if stop {
                                handle.control.send(SessionControl::Interrupt {
                                    turn: first, reason: StopReason::User,
                                }).unwrap();
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            fs::write(scratch.0.join("finish-a"), "").unwrap();
                            released = true;
                        }
                    }
                    msg = rx.recv() => {
                        let Some(EngineMsg::Provider { event, .. }) = msg else { break; };
                        if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                            handle.control.send(SessionControl::DispatchCommitted { turn: *turn }).unwrap();
                        }
                        match event {
                            ProviderEvent::Session(SessionEvent::Ready { .. }) if !sent => {
                                sent = true;
                                handle.control.send(SessionControl::StartTurn {
                                    turn: first, text: "first".into(),
                                }).unwrap();
                            }
                            ProviderEvent::Turn { turn: None, event: TurnEvent::TextDelta { .. } } => {
                                assert_eq!(ends.len(), 1);
                                if stop { assert!(matches!(ends[0], TurnOutcome::Interrupted { .. })); }
                                fs::write(scratch.0.join("read-stdin"), "").unwrap();
                            }
                            ProviderEvent::Turn { event: TurnEvent::TurnEnded { outcome, .. }, .. } => {
                                ends.push(outcome);
                                if ends.len() == 2 { handle.control.send(SessionControl::Shutdown).unwrap(); }
                            }
                            ProviderEvent::Session(SessionEvent::Exited { .. }) => break,
                            _ => {}
                        }
                    }
                }
            }
            ends
        }).await;
        handle.control.send(SessionControl::Shutdown).ok();
        result.unwrap()
    });
    eprintln!(
        "stop={stop} ends={ends:?} interrupt_delivered_to_b={}",
        scratch.0.join("interrupt-delivered-to-b").exists()
    );
    assert_eq!(ends.len(), 2);
    assert_eq!(
        ends[1],
        TurnOutcome::Completed,
        "the old turn's queued wire interrupt stopped Provider B"
    );
}
#[test]
fn blocked_writer_stop_cannot_reach_later_provider_turn() {
    blocked_interrupt_probe(true);
}
#[test]
fn blocked_writer_without_stop_leaves_provider_turn_completed() {
    blocked_interrupt_probe(false);
}

fn probe_reads(name: &str, expected: Readiness) {
    let (scratch, env) = environment("ok-reply", false);
    fs::copy(fixture(name), scratch.0.join("probe.jsonl")).unwrap();
    let result = Runtime::new().unwrap().block_on(probe(&env, &scratch.0));
    assert_eq!(result.readiness, expected);
    assert_eq!(result.models[0].value, "default");
}
#[test]
fn claude_probe_signed_out_needs_token_source_none() {
    probe_reads("probe-signed-out", Readiness::SignedOut);
}
#[test]
fn claude_probe_api_key_reads_ready() {
    probe_reads("probe-api-key", Readiness::Ready);
}
#[test]
fn claude_probe_third_party_reads_ready() {
    probe_reads("probe-third-party", Readiness::Ready);
}
#[test]
fn claude_probe_silent_account_reads_ready() {
    probe_reads("probe-no-account", Readiness::Ready);
}
#[test]
fn claude_probe_env_turns_off_ide_and_connectors() {
    if std::env::var_os("VEYRA_PROBE_ENV_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "claude_probe_env_turns_off_ide_and_connectors",
                "--nocapture",
            ])
            .env("VEYRA_PROBE_ENV_CHILD", "1")
            .env("CLAUDE_CODE_AUTO_CONNECT_IDE", "parent-ide")
            .env("ENABLE_CLAUDEAI_MCP_SERVERS", "parent-connectors")
            .env("__MISE_DIFF", "parent-diff")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    for key in [
        "CLAUDE_CODE_AUTO_CONNECT_IDE",
        "ENABLE_CLAUDEAI_MCP_SERVERS",
        "__MISE_DIFF",
    ] {
        assert!(std::env::var_os(key).is_some(), "{key} was not planted");
    }
    let (scratch, mut env) = environment("ok-reply", false);
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then printf '2.1.296\\n'; exit; fi\nprintf '%s\\n' \"$@\" > '{0}\
            /probe-args'\nprintf '%s\\n' \"$CLAUDE_CODE_AUTO_CONNECT_IDE\" \"$ENABLE_CLAUDEAI_MCP_SERVERS\" > '\
                {0}/probe-env'\nread -r initialize\nprintf '%s\\n' '{{\"type\":\"control_response\",\"response\
                    \":{{\"subtype\":\"success\",\"request_id\":\"veyra-initialize\",\"response\":{{}}}}}}'\n",
        scratch.0.display()
    );
    env.claude_bin = Some(stand_in(&scratch, "probe-stand-in", &script));
    let result = Runtime::new().unwrap().block_on(probe(&env, &scratch.0));
    assert_eq!(result.readiness, Readiness::Ready);
    assert_eq!(
        fs::read_to_string(scratch.0.join("probe-env")).unwrap(),
        "0\nfalse\n"
    );
    let args = fs::read_to_string(scratch.0.join("probe-args")).unwrap();
    assert!(args.contains("--settings\n{\"disableAllHooks\":true}\n"));
    assert!(args.contains("--strict-mcp-config\n"));
    let command = env.command(Path::new("/bin/sh"), &[], &scratch.0);
    let variables: std::collections::BTreeMap<_, _> = command.as_std().get_envs().collect();
    assert!(!variables.contains_key(std::ffi::OsStr::new("CLAUDE_CODE_AUTO_CONNECT_IDE")));
    assert!(!variables.contains_key(std::ffi::OsStr::new("ENABLE_CLAUDEAI_MCP_SERVERS")));
    assert!(!variables.contains_key(std::ffi::OsStr::new("__MISE_DIFF")));
    let explicit = env
        .clone()
        .with_variable("__MISE_DIFF", "explicit-diff")
        .with_variable("CLAUDE_CODE_AUTO_CONNECT_IDE", "explicit-ide")
        .with_variable("ENABLE_CLAUDEAI_MCP_SERVERS", "explicit-connectors");
    let command = explicit.command(Path::new("/bin/sh"), &[], &scratch.0);
    let variables: std::collections::BTreeMap<_, _> = command.as_std().get_envs().collect();
    for (key, value) in [
        ("__MISE_DIFF", "explicit-diff"),
        ("CLAUDE_CODE_AUTO_CONNECT_IDE", "explicit-ide"),
        ("ENABLE_CLAUDEAI_MCP_SERVERS", "explicit-connectors"),
    ] {
        assert_eq!(
            variables[std::ffi::OsStr::new(key)],
            Some(std::ffi::OsStr::new(value))
        );
    }
}
fn two_turns_on_one_process(name: &str, first: &str) {
    let (scratch, env) = environment(name, false);
    let events = Runtime::new().unwrap().block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: scratch.0.clone(),
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: None,

                root: false,
            },
            env,
            out,
            1,
        );
        let mut events = Vec::new();
        let mut turns = 0;
        let finished = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                let send = matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. }))
                    || matches!(
                        event,
                        ProviderEvent::Turn {
                            event: TurnEvent::TurnEnded { .. },
                            ..
                        }
                    ) && turns == 1;
                if let ProviderEvent::Session(SessionEvent::Dispatched { turn }) = &event {
                    handle
                        .control
                        .send(SessionControl::DispatchCommitted { turn: *turn })
                        .unwrap();
                }
                if send {
                    turns += 1;
                    handle
                        .control
                        .send(SessionControl::StartTurn {
                            turn: TurnId::new(),
                            text: "Continue here".into(),
                        })
                        .unwrap();
                }
                let exited = matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                events.push(event);
                if exited {
                    break;
                }
            }
        })
        .await;
        if finished.is_err() {
            handle.control.send(SessionControl::Shutdown).unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                    let exited =
                        matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                    events.push(event);
                    if exited {
                        break;
                    }
                }
            })
            .await
            .unwrap();
        }
        events
    });
    assert_eq!(
        outcomes(&events),
        [
            TurnOutcome::Failed {
                message: first.into()
            },
            TurnOutcome::Completed
        ]
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. })))
            .count(),
        1
    );
    assert!(events.iter().all(|event| !matches!(
        event,
        ProviderEvent::Turn {
            event: TurnEvent::TurnEnded { park: true, .. },
            ..
        }
    )));
    assert!(text(&events).contains("The next message ran on this process."));
}
#[test]
fn claude_403_shows_cli_text_and_keeps_session() {
    two_turns_on_one_process(
        "result-403",
        "Your organization cannot use this model. Choose another model and send again.",
    );
}
#[test]
fn claude_skipped_prompt_fails_turn() {
    two_turns_on_one_process(
        "prompt-skipped",
        "Claude took this message but never started a turn for it. Send it again.",
    );
}

fn shell_snapshot_case(test: &str, failing: bool, check: fn(bool)) {
    if std::env::var_os("VEYRA_LOGIN_SHELL_CHILD").is_some() {
        check(failing);
        return;
    }
    let (scratch, _) = environment("ok-reply", false);
    let script = if failing {
        "#!/bin/sh\nexit 7\n"
    } else {
        "#!/bin/sh\nexport PATH=/shell-path\nexport __MISE_DIFF=shell-diff\nexec /bin/sh -c \"$3\"\n"
    };
    let shell = stand_in(&scratch, "login-shell", script);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("VEYRA_LOGIN_SHELL_CHILD", "1")
        .env("SHELL", shell)
        .env("PATH", "/inherited-path")
        .env("__MISE_DIFF", "inherited-diff")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    );
}
fn check_child_env_snapshot(failing: bool) {
    let env = ChildEnv::load();
    let result = Runtime::new().unwrap().block_on(async {
        env.wait_for_login_path().await;
        env.command(
            Path::new("/bin/sh"),
            &[
                "-c".into(),
                "printf '%s\\n%s\\n' \"$PATH\" \"$__MISE_DIFF\"".into(),
            ],
            Path::new("/tmp"),
        )
        .output()
        .await
        .unwrap()
    });
    assert!(result.status.success());
    assert_eq!(
        result.stdout,
        if failing {
            b"/inherited-path\ninherited-diff\n".as_slice()
        } else {
            b"/shell-path\nshell-diff\n".as_slice()
        }
    );
}
#[test]
fn login_shell_reads_mise_diff() {
    shell_snapshot_case("login_shell_reads_mise_diff", false, |_| {
        assert_eq!(
            veyra_engine::platform::login_shell_path().unwrap(),
            ("/shell-path".into(), Some("shell-diff".into()))
        );
    });
}
#[test]
fn failing_login_shell_keeps_inherited_path_and_mise_diff() {
    shell_snapshot_case(
        "failing_login_shell_keeps_inherited_path_and_mise_diff",
        true,
        check_child_env_snapshot,
    );
}

#[test]
fn child_env_carries_mise_diff_with_path() {
    shell_snapshot_case(
        "child_env_carries_mise_diff_with_path",
        false,
        check_child_env_snapshot,
    );
}
