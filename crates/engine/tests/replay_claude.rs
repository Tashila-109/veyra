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
    let mut env = ChildEnv::load();
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
                import: false,
            },
            env,
            out,
            1,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut sent = false;
            let mut events = Vec::new();
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
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
                event: TurnEvent::TurnEnded { outcome },
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
                event: TurnEvent::TurnEnded { outcome },
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
