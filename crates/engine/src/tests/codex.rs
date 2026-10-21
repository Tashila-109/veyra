#![allow(clippy::disallowed_types)] // Codec tests construct and inspect provider wire envelopes.
use super::*;
use serde_json::json;

#[test]
fn codex_ready_provider_can_be_delegated() {
    let mut f = Fixture::new(LiveState::Running);
    let runtime = f.runtime.handle().clone();
    let _entered = runtime.enter();
    let invoke = |f: &mut Fixture, name: &str, arguments| {
        let (reply, result) = oneshot::channel();
        f.actor.handle(EngineMsg::Tool {
            caller: f.thread,
            session: None,
            call: ToolCall {
                veyra_label: false,
                name: name.into(),
                arguments,
                input_summary: None,
            },
            reply,
        });
        let result = result.blocking_recv().unwrap();
        assert!(result.ok, "{:?}", result.output_tail);
        serde_json::from_str::<serde_json::Value>(&result.output_tail.unwrap()).unwrap()
    };
    let listed = invoke(&mut f, "list_providers", json!({}));
    assert!(
        listed["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["provider"] == "codex")
    );
    let task = invoke(
        &mut f,
        "start_thread",
        json!({"provider":"codex","title":"Check files","prompt":"Report the files","client_request_id":"codex-child"}),
    );
    let child: ThreadId = serde_json::from_value(task["thread_id"].clone()).unwrap();
    assert_eq!(f.loaded().threads[&child].provider, ProviderKind::Codex);
    assert_eq!(
        f.loaded().threads[&child].lineage,
        Lineage::Child {
            parent: f.thread,
            root: f.thread,
            relationship: Relationship::Subagent
        }
    );
}

#[cfg(debug_assertions)]
#[test]
fn codex_debug_binary_override_is_resolved() {
    if std::env::var_os("VEYRA_TEST_BINARY_OVERRIDE").is_some() {
        let env = env::ChildEnv::isolated().with_binary(
            ProviderKind::Codex,
            std::env::var_os("VEYRA_CODEX_BIN").unwrap().into(),
        );
        assert_eq!(
            env.resolve(ProviderKind::Codex).unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::codex::codex_debug_binary_override_is_resolved",
        ])
        .env("VEYRA_TEST_BINARY_OVERRIDE", "1")
        .env("VEYRA_CODEX_BIN", std::env::current_exe().unwrap())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn codex_params_per_mode() {
    use crate::provider::codex;
    for (mode, approval, sandbox, policy) in [
        (
            PermissionMode::AskFirst,
            "untrusted",
            "read-only",
            json!({"type":"readOnly","networkAccess":true}),
        ),
        (
            PermissionMode::AcceptEdits,
            "on-request",
            "workspace-write",
            json!({"type":"workspaceWrite","networkAccess":true}),
        ),
        (
            PermissionMode::FullAccess,
            "never",
            "danger-full-access",
            json!({"type":"dangerFullAccess"}),
        ),
    ] {
        let config = ThreadConfig {
            model: Some("a-current-model".into()),
            effort: Some("a-cli-effort".into()),
            mode,
        };
        let spec = SessionSpec {
            thread: ThreadId::new(),
            cwd: "/project".into(),
            config: config.clone(),
            native: Some(NativeSessionId("native".into())),

            import: None,
            root: false,
        };
        assert_eq!(
            codex::thread_params(&spec),
            json!({"cwd":"/project","model":"a-current-model","approvalPolicy":approval,"sandbox":sandbox,"appr\
                ovalsReviewer":"user","threadId":"native","excludeTurns":true})
        );
        assert_eq!(
            codex::turn_params("native", &config, "hello".into()),
            json!({"threadId":"native","input":[{"type":"text","text":"hello","text_elements":[]}],"model":"a-c\
                urrent-model","effort":"a-cli-effort","summary":"auto","approvalPolicy":approval,"approvalsRevi\
                    ewer":"user","sandboxPolicy":policy})
        );
    }
}

#[cfg(debug_assertions)]
#[test]
fn inline_credential_recording_preserves_redacted_binding() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/provider-record-{}", ThreadId::new()));
    std::fs::create_dir_all(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let path = root.join("capture.jsonl");
    let mut recorder = crate::record::Recorder::create(
        &path,
        crate::provider::ProviderKind::Codex,
        "fake",
        &root,
        None,
        None,
    )
    .unwrap();
    recorder
        .line(
            true,
            json!({"params":{"config":{"mcp_servers.veyra.env":{"VEYRA_MCP_TOKEN":"<id:token>"}}}}),
        )
        .unwrap();
    drop(recorder);
    let line: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .nth(1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        line["line"]["params"]["config"]["mcp_servers.veyra.env"]["VEYRA_MCP_TOKEN"],
        "<id:token>"
    );
}

#[cfg(debug_assertions)]
#[test]
fn recording_distinguishes_changed_native_thread_and_version() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/provider-native-record-{}",
        ThreadId::new()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let path = root.join("capture.jsonl");
    let mut recorder = crate::record::Recorder::create(
        &path,
        crate::provider::ProviderKind::Codex,
        "unknown",
        &root,
        None,
        None,
    )
    .unwrap();
    recorder.bind("old-native".into(), "<id:session>".into());
    recorder
        .line(
            true,
            json!({"method":"thread/resume","params":{"threadId":"old-native"}}),
        )
        .unwrap();
    recorder
        .metadata(&crate::record::Metadata {
            native: Some("new-native".into()),
            version: Some("0.162.1".into()),
        })
        .unwrap();
    recorder
        .line(
            false,
            json!({"id":2,"result":{"thread":{"id":"new-native","cliVersion":"0.162.1"}}}),
        )
        .unwrap();
    drop(recorder);
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(lines[1]["line"]["params"]["threadId"], "<id:session>");
    assert_eq!(lines[2]["line"]["result"]["thread"]["id"], "<id:session:1>");
    assert_eq!(lines[0]["cli_version"], "0.162.1");
}
