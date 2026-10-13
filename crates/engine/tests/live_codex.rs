use std::{fs, path::PathBuf, time::Duration};
use tokio::{runtime::Runtime, sync::mpsc};
use veyra_engine::{
    actor::EngineMsg,
    domain::*,
    provider::{env::ChildEnv, *},
};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn live(
    mode: PermissionMode,
    prompts: &[&str],
    decisions: &[Decision],
) -> (Vec<ProviderEvent>, Vec<bool>) {
    for key in [
        "VEYRA_CODEX_BIN",
        "VEYRA_FAKE_FIXTURES",
        "VEYRA_FAKE_FIXTURE",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "live Codex tests require the installed binary"
        );
    }
    let root = PathBuf::from("/tmp").join(format!("veyra-live-codex-{}", ThreadId::new()));
    fs::create_dir(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let runtime = Runtime::new().unwrap();
    let env = ChildEnv::load();
    runtime.block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = Spawner::Cli(env)
            .spawn(
                ProviderKind::Codex,
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: root.clone(),
                    config: ThreadConfig {
                        model: None,
                        effort: None,
                        mode,
                    },
                    native: None,
                    import: false,
                    root: false,
                },
                out,
                1,
            )
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(180), async {
            let mut events = vec![];
            let mut sent = 0;
            let mut answered = 0;
            let mut exists = vec![];
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                match &event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) => {
                        handle
                            .control
                            .send(SessionControl::StartTurn {
                                turn: TurnId::new(),
                                text: prompts[0].into(),
                            })
                            .unwrap();
                        sent = 1;
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::RequestOpened { native, .. },
                        ..
                    } => {
                        let decision = decisions
                            .get(answered)
                            .expect("unexpected live request")
                            .clone();
                        answered += 1;
                        handle
                            .control
                            .send(SessionControl::Answer {
                                native: native.clone(),
                                reply: decision,
                            })
                            .unwrap();
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::TurnEnded { outcome, .. },
                        ..
                    } => {
                        assert_eq!(outcome, &TurnOutcome::Completed);
                        exists.push(root.join("veyra-check.txt").exists());
                        if sent < prompts.len() {
                            handle
                                .control
                                .send(SessionControl::StartTurn {
                                    turn: TurnId::new(),
                                    text: prompts[sent].into(),
                                })
                                .unwrap();
                            sent += 1;
                        } else {
                            handle.control.send(SessionControl::Shutdown).unwrap();
                        }
                    }
                    ProviderEvent::Session(SessionEvent::Exited { crash }) => {
                        assert_eq!(crash, &None);
                        events.push(event);
                        break;
                    }
                    _ => {}
                }
                events.push(event);
            }
            assert_eq!(
                exists.len(),
                prompts.len(),
                "Codex did not finish each live turn"
            );
            (events, exists)
        })
        .await;
        let _ = handle.control.send(SessionControl::Shutdown);
        result.expect("live Codex deadline expired")
    })
}
#[test]
#[ignore = "requires the human's consent for a live Codex turn"]
fn live_codex_ok_reply() {
    let (events, _) = live(
        PermissionMode::FullAccess,
        &["Reply with exactly the word ok."],
        &[],
    );
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event: TurnEvent::TextDelta { text, .. },
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text.trim().to_lowercase(), "ok");
    assert!(events.iter().any(|e| matches!(
        e,
        ProviderEvent::Turn {
            event: TurnEvent::Accepted,
            ..
        }
    )));
}
#[test]
#[ignore = "requires the human's consent for live Codex approval turns"]
fn live_codex_approval_round_trip() {
    let (events, exists) = live(
        PermissionMode::AskFirst,
        &[
            "Run only touch veyra-check.txt. If denied, stop and report the denial.",
            "Run only touch veyra-check.txt and report completion.",
        ],
        &[Decision::Deny, Decision::AllowOnce],
    );
    assert_eq!(exists, [false, true]);
    let commands: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Turn {
                event:
                    TurnEvent::RequestOpened {
                        kind:
                            RequestKind::Approval {
                                action:
                                    ProposedAction {
                                        target: ApprovalTarget::Command { command, .. },
                                        ..
                                    },
                                ..
                            },
                        ..
                    },
                ..
            } => Some(command),
            _ => None,
        })
        .collect();
    assert_eq!(commands.len(), 2);
    assert!(commands.iter().all(|c| c.contains("touch veyra-check.txt")));
}
#[test]
#[ignore = "requires the human's consent for a live Codex full-access write"]
fn live_codex_full_access_write() {
    let (events, exists) = live(
        PermissionMode::FullAccess,
        &["Run only touch veyra-check.txt and report completion."],
        &[],
    );
    assert_eq!(exists, [true]);
    assert!(!events.iter().any(|e| matches!(
        e,
        ProviderEvent::Turn {
            event: TurnEvent::RequestOpened { .. },
            ..
        }
    )));
}
#[test]
#[ignore = "requires the human's consent; records whether Codex sends a native user question"]
fn live_codex_question() {
    let (events, _) = live(
        PermissionMode::FullAccess,
        &[
            "Ask me whether to use Violet or Blue using request_user_input, then repeat my choice. Make no file changes.",
        ],
        &[Decision::Answers(vec![Answer {
            text: "Violet".into(),
        }])],
    );
    let count = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ProviderEvent::Turn {
                    event: TurnEvent::RequestOpened {
                        kind: RequestKind::Question { .. },
                        ..
                    },
                    ..
                }
            )
        })
        .count();
    eprintln!(
        "codex_native_user_question_requests={count}; criterion 8 needs Lane 6's record when this is zero"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        ProviderEvent::Turn {
            event: TurnEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
                ..
            },
            ..
        }
    )));
}
