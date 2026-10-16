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
pub fn live(
    provider: ProviderKind,
    mode: PermissionMode,
    prompts: &[&str],
    decisions: &[Decision],
) -> (Vec<ProviderEvent>, Vec<bool>) {
    let override_key = format!("VEYRA_{}_BIN", provider.binary().to_uppercase());
    for key in [
        override_key.as_str(),
        "VEYRA_FAKE_FIXTURES",
        "VEYRA_FAKE_FIXTURE",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "live provider tests require the installed binary"
        );
    }
    let root = PathBuf::from("/tmp").join(format!(
        "veyra-{}-live-{}",
        provider.binary(),
        ThreadId::new()
    ));
    fs::create_dir(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let runtime = Runtime::new().unwrap();
    let env = ChildEnv::load();
    runtime.block_on(async {
        let (out, mut rx) = mpsc::channel(256);
        let handle = Spawner::Cli(Box::new(env))
            .spawn(
                provider,
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
                "provider did not finish each live turn"
            );
            (events, exists)
        })
        .await;
        let _ = handle.control.send(SessionControl::Shutdown);
        result.expect("live provider deadline expired")
    })
}

pub fn ok_reply(provider: ProviderKind) {
    let (events, _) = live(
        provider,
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

pub fn approval_round_trip(provider: ProviderKind) {
    let command = if provider == ProviderKind::Grok {
        "printf 'veyra\\n' > veyra-check.txt"
    } else {
        "touch veyra-check.txt"
    };
    let (events, exists) = live(
        provider,
        PermissionMode::AskFirst,
        &[
            &format!("Run only {command}. If denied, stop and report the denial."),
            &format!("Run only {command} and report completion."),
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
    assert!(commands.iter().all(|c| c.contains(command)));
}

pub fn full_access_write(provider: ProviderKind) {
    let (events, exists) = live(
        provider,
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

pub fn question(provider: ProviderKind) {
    let (events, _) = live(
        provider,
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
        "{}_native_user_question_requests={count}; this question path needs a live record of the supported mode when this is zero",
        provider.binary()
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
