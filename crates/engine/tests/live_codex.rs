#[path = "common/live.rs"]
mod harness;
use veyra_engine::{domain::*, provider::*};
fn live(
    mode: PermissionMode,
    prompts: &[&str],
    decisions: &[Decision],
) -> (Vec<ProviderEvent>, Vec<bool>) {
    harness::live(ProviderKind::Codex, mode, prompts, decisions)
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
        "codex_native_user_question_requests={count}; this question path needs a live record of the supported mode when this is zero"
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
