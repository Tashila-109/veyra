use super::*;
use crate::provider::codex;
use serde_json::json;

fn spec(root: bool) -> SessionSpec {
    SessionSpec {
        thread: ThreadId::new(),
        cwd: "/project".into(),
        config: config(),
        native: None,
        import: None,
        root,
    }
}

#[test]
fn offered_modes_and_run_mode() {
    use PermissionMode::*;
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        assert_eq!(
            provider.offered_modes(),
            &[AskFirst, AcceptEdits, FullAccess]
        );
        for mode in [AskFirst, AcceptEdits, FullAccess] {
            assert_eq!(provider.run_mode(mode), mode);
        }
    }
    assert_eq!(ProviderKind::Grok.offered_modes(), &[AskFirst, FullAccess]);
    assert_eq!(ProviderKind::Grok.run_mode(AcceptEdits), AskFirst);
    assert_eq!(ProviderKind::Grok.run_mode(FullAccess), FullAccess);
}

#[test]
fn claude_root_args_append_family_rules() {
    for resumed in [false, true] {
        let mut root = spec(true);
        root.native = resumed.then(|| NativeSessionId("existing-session".into()));
        let args = claude::args(Some(&root));
        let flag = args
            .iter()
            .position(|arg| arg == "--append-system-prompt")
            .unwrap();
        assert_eq!(args[flag + 1], crate::tools::instructions());
        assert_eq!(
            args.iter()
                .filter(|arg| *arg == "--append-system-prompt")
                .count(),
            1
        );
        root.root = false;
        let child = claude::args(Some(&root));
        assert!(!child.iter().any(|arg| arg == "--append-system-prompt"));
        assert!(
            child
                .iter()
                .any(|arg| arg == if resumed { "--resume" } else { "--session-id" })
        );
    }
}

#[test]
fn claude_probe_args_turn_off_hooks_and_mcp() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/m8-probe-{}", ThreadId::new()));
    assert!(!root.exists());
    let args = claude::probe_args(&root);
    let settings = args.iter().position(|arg| arg == "--settings").unwrap();
    assert_eq!(args[settings + 1], r#"{"disableAllHooks":true}"#);
    assert!(args.iter().any(|arg| arg == "--strict-mcp-config"));
    assert!(!args.iter().any(|arg| arg == "--mcp-config"));
    assert!(
        !claude::args(Some(&spec(false)))
            .iter()
            .any(|arg| ["--settings", "--strict-mcp-config"].contains(&arg.as_str()))
    );
}

#[test]
fn claude_probe_args_skip_strict_mcp_beside_managed_file() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/m8-managed-{}", ThreadId::new()));
    std::fs::create_dir_all(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let managed = root.join("managed.json");
    std::fs::write(&managed, "{}").unwrap();
    let args = claude::probe_args(&managed);
    let settings = args.iter().position(|arg| arg == "--settings").unwrap();
    assert_eq!(args[settings + 1], r#"{"disableAllHooks":true}"#);
    assert!(
        !args
            .iter()
            .any(|arg| ["--strict-mcp-config", "--mcp-config"].contains(&arg.as_str()))
    );
}

#[test]
fn codex_initialize_opts_out_of_turn_diff() {
    assert_eq!(
        codex::initialize(),
        json!({
            "clientInfo":{"name":"veyra","title":"Veyra","version":"0.0.1"},
            "capabilities":{"optOutNotificationMethods":["turn/diff/updated"]}
        })
    );
    for root in [false, true] {
        let params = codex::thread_params(&spec(root));
        assert_eq!(params["approvalPolicy"], "never");
        assert!(params.get("config").is_none());
    }
}

#[test]
fn list_providers_reports_offered_modes() {
    let f = Fixture::new(LiveState::Running);
    let providers = ProviderKind::ALL.map(|provider| Probe {
        provider,
        path: Some("/owned/fake".into()),
        version: None,
        readiness: Readiness::Ready,
        models: vec![],
        account: None,
    });
    let (_, result) = crate::tools::decide(
        &f.actor.projection,
        &f.actor.live,
        f.thread,
        ToolCall {
            name: "list_providers".into(),
            arguments: json!({}),
            veyra_label: false,
            input_summary: None,
        },
        &providers,
        0,
    )
    .unwrap();
    assert_eq!(
        result["providers"][0]["offered_modes"],
        json!(["ask_first", "accept_edits", "full_access"])
    );
    assert_eq!(
        result["providers"][1]["offered_modes"],
        json!(["ask_first", "accept_edits", "full_access"])
    );
    assert_eq!(
        result["providers"][2]["offered_modes"],
        json!(["ask_first", "full_access"])
    );
}

#[test]
fn ceiling_reads_parent_run_mode() {
    let mut f = Fixture::new(LiveState::Running);
    let parent = f.actor.projection.threads.get_mut(&f.thread).unwrap();
    parent.provider = ProviderKind::Grok;
    parent.config.mode = PermissionMode::AcceptEdits;
    let providers = [Probe {
        provider: ProviderKind::Claude,
        path: Some("/owned/fake".into()),
        version: None,
        readiness: Readiness::Ready,
        models: vec![],
        account: None,
    }];
    let call = |name: &str, args| ToolCall {
        name: name.into(),
        arguments: args,
        veyra_label: false,
        input_summary: None,
    };
    let rejected = crate::tools::decide(
        &f.actor.projection,
        &f.actor.live,
        f.thread,
        call(
            "start_thread",
            json!({"provider":"claude","title":"Worker","prompt":"Check the files","mode":"accept_edits","client_request_id":"ceiling"}),
        ),
        &providers,
        0,
    );
    assert!(
        matches!(rejected, Err(CommandError::Invalid(message)) if message.contains("ask_first ceiling"))
    );
    let (_, list) = crate::tools::decide(
        &f.actor.projection,
        &f.actor.live,
        f.thread,
        call("list_providers", json!({})),
        &providers,
        0,
    )
    .unwrap();
    assert_eq!(list["mode_ceiling"], "ask_first");
    assert_eq!(
        f.actor.projection.threads[&f.thread].config.mode,
        PermissionMode::AcceptEdits
    );
}
