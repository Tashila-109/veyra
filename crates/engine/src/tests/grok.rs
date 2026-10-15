use crate::provider::ProviderKind;
#[test]
fn grok_is_built_and_offered_in_tool_schema() {
    assert!(ProviderKind::Grok.is_built());
    let schema = crate::tools::list();
    assert!(schema.to_string().contains("grok"));
}

#[test]
fn grok_args_per_mode() {
    use crate::{domain::*, provider::grok};
    for (mode, prefix) in [
        (
            PermissionMode::AskFirst,
            vec!["--no-auto-update", "--permission-mode", "default", "agent"],
        ),
        (
            PermissionMode::AcceptEdits,
            vec![
                "--no-auto-update",
                "--permission-mode",
                "acceptEdits",
                "agent",
            ],
        ),
        (
            PermissionMode::FullAccess,
            vec!["--no-auto-update", "agent", "--always-approve"],
        ),
    ] {
        let mut expected: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
        expected.extend(
            [
                "-m",
                "veyra-model",
                "--reasoning-effort",
                "veyra-effort",
                "--no-leader",
                "stdio",
            ]
            .map(str::to_owned),
        );
        assert_eq!(
            grok::args(&ThreadConfig {
                model: Some("veyra-model".into()),
                effort: Some("veyra-effort".into()),
                mode
            }),
            expected
        );
        let mut expected: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
        expected.extend(["--no-leader", "stdio"].map(str::to_owned));
        assert_eq!(
            grok::args(&ThreadConfig {
                model: None,
                effort: None,
                mode
            }),
            expected
        );
    }
}
#[cfg(debug_assertions)]
#[test]
fn grok_probe_without_catalog_offers_default() {
    use crate::{domain::*, provider::*};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/grok-probe-{}", ThreadId::new()));
    let _scratch = super::Scratch(root.clone());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/grok/probe-grok.jsonl"),
        root.join("probe-grok.jsonl"),
    )
    .unwrap();
    let mut env =
        env::ChildEnv::isolated().with_binary(ProviderKind::Grok, std::env::current_exe().unwrap());
    env.fixtures = Some(root.clone());
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut probes = Probes::default();
        let p = probes
            .check(
                &tokio::runtime::Handle::current(),
                ProviderKind::Grok,
                env,
                root,
            )
            .await
            .unwrap();
        assert_eq!(p.readiness, Readiness::Ready);
        assert_eq!(p.version.as_deref(), Some("1.0.52"));
        assert!(p.models.is_empty());
        assert_eq!(
            grok::args(&ThreadConfig {
                model: None,
                effort: None,
                mode: PermissionMode::AskFirst
            }),
            [
                "--no-auto-update",
                "--permission-mode",
                "default",
                "agent",
                "--no-leader",
                "stdio"
            ]
        );
    });
}

#[test]
fn grok_deny_only_consent_survives_storage_and_refuses_allow() {
    use crate::domain::*;
    let action: ProposedAction = serde_json::from_value(serde_json::json!({
        "tool":"Bash","target":{"kind":"tool","preview":{"text":"touch check","truncated":false}},
        "consent":{"DenyOnly":{"reason":"Grok offered only a lasting grant"}}
    }))
    .unwrap();
    let kind = RequestKind::Approval {
        action: action.with_grant(Some(GrantKey {
            tool: "Bash".into(),
            command: "touch check".into(),
        })),
    };
    assert!(kind.accepts(&Decision::AllowOnce).is_none());
    assert!(kind.accepts(&Decision::AllowInThread).is_none());
    assert!(kind.accepts(&Decision::Deny).is_some());
    let stored = serde_json::to_value(&kind).unwrap();
    let loaded: RequestKind = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(loaded, kind);
    assert_eq!(
        stored["Approval"]["action"]["consent"]["DenyOnly"]["reason"],
        "Grok offered only a lasting grant"
    );
}

fn initial_codec(api_key_set: bool) -> crate::provider::grok::Codec {
    use crate::{domain::*, provider::*};
    grok::Codec::new(
        SessionSpec {
            thread: ThreadId::new(),
            cwd: "/project".into(),
            config: ThreadConfig {
                model: None,
                effort: None,
                mode: PermissionMode::AskFirst,
            },
            native: None,
            root: false,
            import: false,
        },
        1,
        None,
        api_key_set,
    )
}
fn ready_codec() -> crate::provider::grok::Codec {
    let mut codec = initial_codec(false);
    codec.initialize();
    codec
        .value(serde_json::json!({"id":1,"result":{}}))
        .unwrap();
    codec
        .value(serde_json::json!({"id":2,"result":{"sessionId":"native"}}))
        .unwrap();
    codec
}
#[test]
fn grok_lone_foreign_completion_opens_no_provider_turn() {
    let mut codec = ready_codec();
    let decoded=codec.value(serde_json::json!({"method":"_x.ai/session/prompt_complete","params":{"sessionId":"native","promptId":"never-started","stopReason":"end_turn"}})).unwrap();
    assert!(decoded.events.is_empty());
    assert!(decoded.provider_turn.is_none());
    let started=codec.value(serde_json::json!({"method":"session/update","params":{"sessionId":"native","_meta":{"promptId":"task-completed-fresh"},"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"report"}}}})).unwrap();
    assert!(started.provider_turn.is_some());
    assert!(started.events.iter().any(|e|matches!(e,crate::provider::ProviderEvent::Turn {event:crate::provider::TurnEvent::TextDelta {text,..},..} if text=="report")));
}
#[test]
fn grok_cumulative_tool_output_is_emitted_once() {
    use crate::{domain::*, provider::*};
    let mut codec = ready_codec();
    let turn = TurnId::new();
    codec.start(turn, "work".into()).unwrap();
    let mut output = String::new();
    let mut ended = 0;
    for (status, text) in [
        ("in_progress", "a"),
        ("in_progress", "ab"),
        ("completed", "abc"),
        ("completed", "abc"),
    ] {
        let decoded=codec.value(serde_json::json!({"method":"session/update","params":{"sessionId":"native","_meta":{"promptId":turn.to_string()},"update":{"sessionUpdate":"tool_call_update","toolCallId":"tool","title":"Bash","rawInput":{"command":"echo abc"},"status":status,"rawOutput":text}}})).unwrap();
        for event in decoded.events {
            match event {
                ProviderEvent::Turn {
                    event: TurnEvent::ToolOutput { text, .. },
                    ..
                } => output.push_str(&text),
                ProviderEvent::Turn {
                    event: TurnEvent::ToolFinished { output_tail, .. },
                    ..
                } => {
                    ended += 1;
                    output.push_str(output_tail.as_deref().unwrap_or_default());
                }
                _ => {}
            }
        }
    }
    assert_eq!(output, "abc");
    assert_eq!(ended, 1);
}

#[test]
fn grok_ready_provider_can_be_delegated() {
    use super::*;
    use serde_json::json;
    let mut f = Fixture::new(LiveState::Running);
    let runtime = f.runtime.handle().clone();
    let _entered = runtime.enter();
    let mut invoke = |name: &str, arguments| {
        let (reply, result) = oneshot::channel();
        f.actor.handle(EngineMsg::Tool {
            caller: f.thread,
            session: None,
            call: ToolCall {
                name: name.into(),
                arguments,
                input_summary: None,
                veyra_label: false,
            },
            reply,
        });
        let result = result.blocking_recv().unwrap();
        assert!(result.ok);
        serde_json::from_str::<serde_json::Value>(&result.output_tail.unwrap()).unwrap()
    };
    let listed = invoke("list_providers", json!({}));
    assert!(
        listed["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["provider"] == "grok")
    );
    let task = invoke(
        "start_thread",
        json!({"provider":"grok","title":"Grok worker","prompt":"Report the files","client_request_id":"grok-child"}),
    );
    let child: ThreadId = serde_json::from_value(task["thread_id"].clone()).unwrap();
    let loaded = f.loaded();
    assert_eq!(loaded.threads[&child].provider, ProviderKind::Grok);
    assert_eq!(
        loaded.threads[&child].lineage,
        Lineage::Child {
            parent: f.thread,
            root: f.thread,
            relationship: Relationship::Subagent
        }
    );
}
#[tokio::test(start_paused = true)]
async fn grok_any_frame_clears_first_frame_guard_without_accepting() {
    use crate::{domain::*, provider::*};
    let mut codec = ready_codec();
    let turn = TurnId::new();
    codec.start(turn, "work".into()).unwrap();
    tokio::time::advance(std::time::Duration::from_secs(59)).await;
    assert!(codec.timer_lines().unwrap().is_empty());
    let decoded = codec
        .value(serde_json::json!({"method":"unknown/progress","params":{}}))
        .unwrap();
    assert!(decoded.events.is_empty());
    tokio::time::advance(std::time::Duration::from_secs(600)).await;
    assert!(codec.next_deadline().is_none());
    assert!(codec.timer_lines().is_ok());
    let decoded = codec
        .value(serde_json::json!({"id":3,"result":{"stopReason":"end_turn"}}))
        .unwrap();
    assert!(matches!(decoded.events.as_slice(),[
        ProviderEvent::Turn {turn:Some(a),event:TurnEvent::Accepted},
        ProviderEvent::Turn {turn:Some(b),event:TurnEvent::TurnEnded {outcome:TurnOutcome::Completed,..}}
    ] if *a==turn && *b==turn));
}

#[test]
fn grok_first_tagged_unknown_frame_accepts_prompt_once() {
    use crate::{domain::*, provider::*};
    let mut codec = ready_codec();
    let turn = TurnId::new();
    codec.start(turn, "work".into()).unwrap();
    let frame = serde_json::json!({"method":"x.ai/new_progress_kind","params":{"sessionId":"native","_meta":{"promptId":turn.to_string()}}});
    let decoded = codec.value(frame.clone()).unwrap();
    assert!(
        matches!(decoded.events.as_slice(),[ProviderEvent::Turn {turn:Some(id),event:TurnEvent::Accepted}] if *id==turn)
    );
    assert!(codec.value(frame).unwrap().events.is_empty());
    let end = codec
        .value(serde_json::json!({"id":3,"result":{"stopReason":"end_turn"}}))
        .unwrap();
    assert!(
        matches!(end.events.as_slice(),[ProviderEvent::Turn {turn:Some(id),event:TurnEvent::TurnEnded {outcome:TurnOutcome::Completed,..}}] if *id==turn)
    );
}

#[test]
fn grok_unknown_notification_logs_only_method() {
    let logs = super::logs::CapturedLogs::default();
    tracing::subscriber::with_default(logs.clone(), || {
        let mut codec = ready_codec();
        let decoded = codec.value(serde_json::json!({"method":"x.ai/unknown_method","params":{"sessionId":"native","payload":"private-payload"}})).unwrap();
        assert!(decoded.events.is_empty());
    });
    let entries = logs.0.lock().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.get("method").is_some_and(|s| s == "x.ai/unknown_method"))
    );
    assert!(!format!("{entries:?}").contains("private-payload"));
}
#[test]
fn grok_unknown_update_kind_is_logged_and_skipped() {
    let logs = super::logs::CapturedLogs::default();
    tracing::subscriber::with_default(logs.clone(), || {
        let mut codec = ready_codec();
        let decoded = codec.value(serde_json::json!({"method":"session/update","params":{"sessionId":"native","update":{"sessionUpdate":"future_update","payload":"private-payload"}}})).unwrap();
        assert!(decoded.events.is_empty());
        assert!(decoded.provider_turn.is_none());
    });
    let entries = logs.0.lock().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.get("kind").is_some_and(|s| s == "future_update"))
    );
    assert!(!format!("{entries:?}").contains("private-payload"));
}
#[test]
fn grok_stored_deny_only_cannot_retain_a_thread_grant() {
    use crate::domain::*;
    let kind: RequestKind = serde_json::from_value(serde_json::json!({"Approval":{
        "action":{"tool":"Bash","target":{"kind":"tool","preview":{"text":"touch check","truncated":false}},
            "consent":{"DenyOnly":{"reason":"lasting only"}}},
        "grant_key":{"tool":"Bash","command":"touch check"}
    }})).unwrap();
    assert!(kind.accepts(&Decision::AllowInThread).is_none());
    let stored = serde_json::to_value(&kind).unwrap();
    assert!(stored["Approval"]["grant_key"].is_null());
    assert!(stored["Approval"]["action"]["consent"]["Once"].is_null());
}

#[test]
fn grok_legacy_stored_approval_keeps_its_thread_grant() {
    use crate::domain::*;
    let kind: RequestKind = serde_json::from_value(serde_json::json!({"Approval":{
        "action":{"tool":"Bash","command":"echo own-check","input":null},
        "grant_key":{"tool":"Bash","command":"echo own-check"}
    }}))
    .unwrap();
    assert_eq!(kind.grant_key().unwrap().command, "echo own-check");
    assert!(matches!(
        kind.accepts(&Decision::AllowInThread),
        Some(AcceptedDecision::Allow { grant: Some(_) })
    ));
    let loaded: RequestKind = serde_json::from_value(serde_json::to_value(&kind).unwrap()).unwrap();
    assert_eq!(loaded, kind);
}
#[test]
fn grok_tool_delta_keeps_text_before_actor_tail() {
    use crate::{domain::*, provider::*};
    let mut codec = ready_codec();
    let turn = TurnId::new();
    codec.start(turn, "work".into()).unwrap();
    let output = "x".repeat(TOOL_OUTPUT_CHARS + 1);
    let decoded = codec.value(serde_json::json!({"method":"session/update","params":{"sessionId":"native","_meta":{"promptId":turn.to_string()},"update":{"sessionUpdate":"tool_call_update","toolCallId":"tool","status":"in_progress","rawOutput":output}}})).unwrap();
    assert!(decoded.events.iter().any(|e| matches!(e, ProviderEvent::Turn { event: TurnEvent::ToolOutput {text,..}, .. } if text.len() == TOOL_OUTPUT_CHARS + 1)));
}

#[test]
fn grok_auth_uses_captured_api_key_presence() {
    for (present, method) in [(true, "xai.api_key"), (false, "cached_token")] {
        let mut codec = initial_codec(present);
        codec.initialize();
        let decoded = codec.value(serde_json::json!({"id":1,"result":{"authMethods":[{"id":"xai.api_key"},{"id":"cached_token"}]}})).unwrap();
        assert_eq!(decoded.replies[0]["params"]["methodId"], method);
    }
}

#[test]
fn grok_error_completion_keeps_failure_with_nontext_result() {
    use crate::{domain::*, provider::*};
    let mut codec = ready_codec();
    let turn = TurnId::new();
    codec.start(turn, "work".into()).unwrap();
    let decoded = codec.value(serde_json::json!({"id":3,"result":{"stopReason":"error","agentResult":{"detail":"unfamiliar metadata"}}})).unwrap();
    assert!(decoded.events.iter().any(|e| matches!(e, ProviderEvent::Turn { event: TurnEvent::TurnEnded { outcome:TurnOutcome::Failed {message},..}, .. } if message == "Grok could not complete the turn")));
}
