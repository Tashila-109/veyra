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
    for n in 1..=11 {
        fs::copy(fixture(name), root.join(format!("t{n}-s1.jsonl"))).unwrap();
        if resumed {
            fs::copy(fixture("ok-reply"), root.join(format!("t{n}-s2.jsonl"))).unwrap();
        }
    }
    fs::copy(fixture("probe-initialize"), root.join("probe.jsonl")).unwrap();
    let env = ChildEnv {
        path: std::env::var_os("PATH").unwrap(),
        claude_bin: Some(PathBuf::from(env!("CARGO_BIN_EXE_veyra-fake-cli"))),
        fixtures: Some(root.clone()),
        record: None,
    };
    (Scratch(root), env)
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
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Session(SessionEvent::Ready { cli_version, pgid, pid, .. }) if cli_version == "2.1.294" && pgid == pid)));
}
#[test]
fn claude_bash_echo() {
    let events = replay("bash-echo", false);
    assert_eq!(outcomes(&events), vec![TurnOutcome::Completed]);
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Turn { event: TurnEvent::ToolStarted { call, .. }, .. } if call.name == "Bash" && call.arguments["command"] == "echo veyra-probe")));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Turn { event: TurnEvent::ToolFinished { ok: true, output_tail: Some(output), .. }, .. } if output == "veyra-probe")));
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
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Session(SessionEvent::Notice { text }) if text == "The earlier conversation could not be resumed; this is a new session")));
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
    assert_eq!(outcomes(&events), vec![TurnOutcome::Failed { message: "claude exited unexpectedly (exit code 7)\nVeyra fixture error: stream closed\nlast diagnostic line".into() }]);
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
    let (_scratch, env) = environment("ok-reply", false);
    let probe = Runtime::new()
        .unwrap()
        .block_on(ProviderKind::Claude.probe(&env));
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
        runtime.block_on(ProviderKind::Claude.probe(&env)).readiness,
        Readiness::NotInstalled
    );
    fs::copy(
        env!("CARGO_BIN_EXE_veyra-fake-cli"),
        env.claude_bin.as_ref().unwrap(),
    )
    .unwrap();
    assert_eq!(
        runtime.block_on(ProviderKind::Claude.probe(&env)).readiness,
        Readiness::Ready
    );
    fs::copy(fixture("probe-no-account"), scratch.0.join("probe.jsonl")).unwrap();
    let probe = runtime.block_on(ProviderKind::Claude.probe(&env));
    assert_eq!(probe.readiness, Readiness::Ready);
    assert_eq!(probe.account, None);
    assert_eq!(probe.models[0].value, "default");
}
