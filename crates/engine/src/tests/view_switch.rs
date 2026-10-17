use super::*;

#[test]
fn switch_refused_while_turn_runs() {
    let mut f = Fixture::new(LiveState::Running);
    assert!(matches!(
        f.command(Command::SwitchView {
            thread: f.thread,
            to: View::Terminal
        }),
        Err(CommandError::Busy(WaitReason::TurnRunning))
    ));
    assert_eq!(f.loaded().threads[&f.thread].view, View::Chat);
}

#[test]
fn switch_refused_with_pending_request() {
    let mut f = Fixture::new(LiveState::Idle);
    let request = f.req();
    assert!(matches!(
        f.command(Command::SwitchView { thread: f.thread, to: View::Terminal }),
        Err(CommandError::Invalid(reason)) if reason == "Answer the pending request before switching views"
    ));
    assert_eq!(f.loaded().requests[&request].state, RequestState::Pending);
    assert_eq!(f.loaded().threads[&f.thread].view, View::Chat);
}

#[cfg(debug_assertions)]
#[test]
fn switch_from_parked_opens_pty_with_new_epoch() {
    let mut f = Fixture::new(LiveState::Parked);
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.native = Some(NativeSessionId("m7-parked".into()));
    row.provider = ProviderKind::Grok;
    c.threads.insert(f.thread, row);
    f.save(c);
    let (changes, _) = crate::commands::decide(
        &f.actor.projection,
        &f.actor.live,
        Command::SwitchView {
            thread: f.thread,
            to: View::Terminal,
        },
        now(),
    )
    .unwrap();
    assert_eq!(changes.live[&f.thread].epoch, 2);
    let thread = f.thread;
    let runtime = f.runtime;
    runtime.block_on(async move {
        let (_scratch, env) = grok_terminal_env("m7-parked");
        let engine = crate::Engine::launch(&tokio::runtime::Handle::current(), f.actor.db, Spawner::Cli(Box::new(env)), None).unwrap();
        let mut events = engine.subscribe();
        engine.send(Command::SwitchView { thread, to: View::Terminal }).await.unwrap().unwrap();
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while let Some(event) = events.recv().await {
                if matches!(event, UiEvent::Change { event: crate::events::ChangeEvent::ThreadStatusChanged { thread: id, status: Status::InTerminal }, .. } if id == thread) {
                    return true;
                }
            }
            false
        }).await;
        engine.shutdown_wait().await;
        assert_eq!(terminal.ok(), Some(true), "real-provider terminal launch never reached InTerminal");
    });
}

#[test]
fn stop_in_terminal_sets_import_pending() {
    let mut f = Fixture::new(LiveState::Terminal);
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    let p = f.loaded();
    assert_eq!(p.threads[&f.thread].view, View::Chat);
    assert!(p.threads[&f.thread].import_pending);
    assert_eq!(p.threads[&f.thread].hold, Some(HoldReason::Stopped));
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Stopping {
            then: AfterStop::Park
        }
    ));
}

#[cfg(debug_assertions)]
fn fake_binary() -> std::path::PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("veyra-fake-cli")
}

#[cfg(debug_assertions)]
fn grok_terminal_env(native: &str) -> (Scratch, env::ChildEnv) {
    use serde_json::json;
    let root =
        std::path::Path::new("/tmp").join(format!("veyra-m7s1-terminal-{}", ThreadId::new()));
    std::fs::create_dir(&root).unwrap();
    let records = [
        json!({"type":"transcript_start","provider":"grok","protocol":"acp","cli_version":"fake","scenario":"cursor capture","source":"Veyra authored"}),
        json!({"type":"expect_outbound","line":{"jsonrpc":"2.0","id":"<id:init>","method":"initialize","params":"<id:init-params>"}}),
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","id":"<id:init>","result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"version":"fake"}}}}),
        json!({"type":"expect_outbound","line":{"jsonrpc":"2.0","id":"<id:load>","method":"session/load","params":"<id:load-params>"}}),
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","id":"<id:load>","result":{"sessionId":native}}}),
        json!({"type":"spin","ms":200}),
        json!({"type":"exit","code":0}),
    ];
    std::fs::write(
        root.join("t1-s1.jsonl"),
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    std::fs::write(root.join("t1-s2.jsonl"), "").unwrap();
    let mut env = env::ChildEnv::isolated().with_binary(ProviderKind::Grok, fake_binary());
    env.fixtures = Some(root.clone());
    (Scratch(root), env)
}

#[cfg(debug_assertions)]
#[test]
fn terminal_input_skips_actor_mailbox() {
    use std::time::{Duration, Instant};
    let mut f = Fixture::new(LiveState::Parked);
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.provider = ProviderKind::Grok;
    row.native = Some(NativeSessionId("m7-echo".into()));
    c.threads.insert(f.thread, row);
    f.save(c);
    let (out, mut receiver) = mpsc::channel(256);
    let (_scratch, env) = grok_terminal_env("m7-echo");
    let groups = env.groups.clone();
    let mut actor = Actor::new(f.actor.db, out.clone(), Spawner::Cli(Box::new(env))).unwrap();
    let (reply, result) = oneshot::channel();
    {
        let _entered = f.runtime.enter();
        actor.handle(EngineMsg::Command {
            command: Command::SwitchView {
                thread: f.thread,
                to: View::Terminal,
            },
            reply,
        });
    }
    result.blocking_recv().unwrap().unwrap();
    let handle = actor.state.terminals.lock().unwrap()[&f.thread].clone();
    let terminal = handle.terminal.as_ref().unwrap().clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !terminal
        .emulator
        .lock()
        .text()
        .iter()
        .any(|line| line == "VEYRA_TUI_READY")
        && Instant::now() < deadline
    {
        while let Ok(message) = receiver.try_recv() {
            let _entered = f.runtime.enter();
            actor.handle(message);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let (entered, held) = std::sync::mpsc::sync_channel(1);
    let (release, released) = std::sync::mpsc::sync_channel(1);
    actor.after_admission = Some(Box::new(move || {
        entered.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    let runtime = f.runtime.handle().clone();
    let worker = std::thread::spawn(move || actor.run(receiver, runtime));
    let (reply, _snapshot) = oneshot::channel();
    out.blocking_send(EngineMsg::Snapshot {
        thread: f.thread,
        reply,
    })
    .unwrap();
    held.recv_timeout(Duration::from_secs(5)).unwrap();
    let wrote = handle.write(b"K");
    let resized = handle.resize(portable_pty::PtySize {
        cols: 96,
        rows: 30,
        ..Default::default()
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    let echoed = loop {
        if terminal
            .emulator
            .lock()
            .text()
            .iter()
            .any(|line| line == "4b")
        {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    release.send(()).unwrap();
    out.blocking_send(EngineMsg::Shutdown).unwrap();
    worker.join().unwrap();
    wrote.unwrap();
    resized.unwrap();
    assert!(echoed, "PTY echo waited for the blocked actor");
    assert_eq!(terminal.emulator.lock().text().len(), 30);
    assert!(
        groups.lock().unwrap().is_empty(),
        "terminal group survived shutdown"
    );
    eprintln!("actor held: direct key K echoed as 4b, resize reached 96x30, owned PTY reaped");
}

#[test]
fn switch_idle_waits_for_exit_and_increments_epoch_once() {
    let mut f = Fixture::new(LiveState::Idle);
    f.command(Command::SwitchView {
        thread: f.thread,
        to: View::Terminal,
    })
    .unwrap();
    assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Shutdown);
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Stopping {
            then: AfterStop::StartTerminal
        }
    ));
    assert_eq!(f.actor.live[&f.thread].epoch, 1);
    f.event(ProviderEvent::Session(SessionEvent::Exited { crash: None }));
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Terminal { .. }
    ));
    assert_eq!(f.actor.live[&f.thread].epoch, 2);
    f.event_epoch(
        1,
        ProviderEvent::Session(SessionEvent::Exited { crash: None }),
    );
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Terminal { .. }
    ));
    assert_eq!(f.actor.live[&f.thread].epoch, 2);
}

#[test]
fn switch_quit_in_terminal_is_import_pending_on_recovery() {
    let mut f = Fixture::new(LiveState::Terminal);
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    let stored = f.loaded();
    assert_eq!(stored.threads[&f.thread].view, View::Terminal);
    let changes = crate::reconcile::decide(&stored, now());
    assert_eq!(changes.threads[&f.thread].view, View::Chat);
    assert!(changes.threads[&f.thread].import_pending);
    assert_eq!(
        changes.threads[&f.thread].native,
        stored.threads[&f.thread].native
    );
}

#[test]
fn switch_quit_processes_terminal_exit_without_deadline() {
    let mut f = Fixture::new(LiveState::Terminal);
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Shutdown);
    f.actor.handle(EngineMsg::Terminal {
        thread: f.thread,
        epoch: 1,
        event: TerminalEvent::Exited,
    });
    assert!(matches!(f.actor.live[&f.thread].live, Live::Parked));
    let row = &f.loaded().threads[&f.thread];
    assert_eq!(row.view, View::Chat);
    assert!(row.import_pending);
}

#[cfg(debug_assertions)]
#[test]
fn switch_idle_closes_stdin_before_ladder() {
    use serde_json::json;
    use std::{path::Path, time::Duration};
    let root = Path::new("/tmp").join(format!("veyra-m7s1-grace-{}", ThreadId::new()));
    std::fs::create_dir(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let records = [
        json!({"type":"transcript_start","provider":"claude","protocol":"stream-json","cli_version":"fake","scenario":"graceful close","source":"Veyra authored"}),
        json!({"type":"expect_outbound","line":{"type":"control_request","request_id":"<id:initialize>","request":{"subtype":"initialize"}}}),
        json!({"type":"emit_inbound","line":{"type":"control_response","response":{"subtype":"success","request_id":"<id:initialize>","response":{}}}}),
        json!({"type":"spin","ms":1600}),
        json!({"type":"touch","path":"graceful-exit"}),
        json!({"type":"exit","code":0}),
    ];
    std::fs::write(
        root.join("t1-s1.jsonl"),
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let mut env = env::ChildEnv::isolated().with_binary(ProviderKind::Claude, fake_binary());
    env.fixtures = Some(root.clone());
    let groups = env.groups.clone();
    Runtime::new().unwrap().block_on(async {
        let (out, mut events) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: root.clone(),
                config: config(),
                native: None,
                history: Default::default(),
                import: false,
                root: false,
            },
            env,
            out,
            1,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(6), async {
            while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
                match event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) => {
                        handle.control.send(SessionControl::Shutdown).unwrap()
                    }
                    ProviderEvent::Session(SessionEvent::Exited { crash }) => {
                        assert_eq!(crash, None);
                        return;
                    }
                    _ => {}
                }
            }
            panic!("session did not publish Exited");
        })
        .await
        .unwrap();
    });
    assert!(
        root.join("graceful-exit").is_file(),
        "idle process was signalled before its graceful exit"
    );
    assert!(groups.lock().unwrap().is_empty());
}

#[test]
fn import_takes_only_turns_after_cursor() {
    let mut f = Fixture::new(LiveState::Idle);
    let native = NativeSessionId("retained".into());
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.native = Some(native.clone());
    row.native_cursor = Some("before".into());
    row.import_pending = true;
    let mut c = ChangeSet::default();
    c.threads.insert(f.thread, row);
    f.save(c);
    let history = History {
        cursor: Some("after".into()),
        text: "typed\nreply".into(),
    };
    f.event(ProviderEvent::Session(SessionEvent::History {
        native: native.clone(),
        history: history.clone(),
    }));
    let p = f.loaded();
    assert_eq!(p.threads[&f.thread].native_cursor.as_deref(), Some("after"));
    assert!(!p.threads[&f.thread].import_pending);
    assert_eq!(p.items.values().filter(|item| matches!(&item.body, ItemBody::TerminalSpan(span) if span.text == "typed\nreply")).count(), 1);
    f.event(ProviderEvent::Session(SessionEvent::History {
        native,
        history,
    }));
    assert_eq!(f.loaded().items, p.items);
}

#[cfg(debug_assertions)]
#[test]
fn switch_back_imports_once() {
    use serde_json::json;
    use std::{fs, path::Path, time::Duration};
    let root = Path::new("/tmp").join(format!("veyra-m7s1-roundtrip-{}", ThreadId::new()));
    crate::platform::private_dir(&root).unwrap();
    let scratch = Scratch(root.clone());
    let f = Fixture::new(LiveState::Parked);
    let thread = f.thread;
    let native = NativeSessionId(ThreadId::new().to_string());
    let config_root = root.join("claude-config");
    fs::create_dir_all(config_root.join("projects/veyra-fake")).unwrap();
    let history_path = config_root.join(format!("projects/veyra-fake/{}.jsonl", native.0));
    fs::write(&history_path, format!("{}\n", json!({"sessionId":native.0,"uuid":"sdk-anchor","parentUuid":null,"isSidechain":false,"type":"assistant","entrypoint":"sdk-cli","message":{"content":[{"type":"text","text":"already in chat"}]}}))).unwrap();
    let db_path = root.join("test.db");
    let mut db = store::open(&db_path).unwrap();
    let mut c = ChangeSet {
        projects: f.actor.projection.projects.clone(),
        ..Default::default()
    };
    let mut row = f.actor.projection.threads[&thread].clone();
    row.cwd = root.clone();
    row.native = Some(native.clone());
    c.threads.insert(thread, row);
    let tx = db.transaction().unwrap();
    store::apply(&tx, &c, now()).unwrap();
    tx.commit().unwrap();
    db.execute_batch("CREATE TABLE cursor_updates (value TEXT); CREATE TRIGGER count_cursor AFTER UPDATE OF native_cursor ON threads WHEN OLD.native_cursor IS NOT NEW.native_cursor BEGIN INSERT INTO cursor_updates VALUES (NEW.native_cursor); END;").unwrap();
    let header = json!({"type":"transcript_start","provider":"claude","protocol":"stream-json","cli_version":"fake","scenario":"round trip","source":"Veyra authored"});
    let init = [
        header.clone(),
        json!({"type":"expect_outbound","line":{"type":"control_request","request_id":"<id:init>","request":{"subtype":"initialize"}}}),
        json!({"type":"emit_inbound","line":{"type":"control_response","response":{"subtype":"success","request_id":"<id:init>","response":{}}}}),
    ];
    let write = |name: &str, records: Vec<serde_json::Value>| {
        fs::write(
            root.join(name),
            records
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap()
    };
    for spawn in [1, 3, 5] {
        write(&format!("t1-s{spawn}.jsonl"), vec![header.clone()]);
    }
    for spawn in [2, 4] {
        let mut records = init.to_vec();
        records.push(json!({"type":"spin","ms":1600}));
        records.push(json!({"type":"exit","code":0}));
        write(&format!("t1-s{spawn}.jsonl"), records);
    }
    let mut resume = init.to_vec();
    resume.push(json!({"type":"expect_outbound","line":{"type":"user","uuid":"<id:prompt>","session_id":"<id:session>","parent_tool_use_id":null,"message":{"role":"user","content":"after recovery"}}}));
    resume.push(json!({"type":"emit_inbound","line":{"type":"result","subtype":"success","session_id":"<id:session>","is_error":false,"result":"ok","uuid":"result"}}));
    resume.push(json!({"type":"exit","code":0}));
    fs::create_dir(root.join("recovered")).unwrap();
    fs::write(
        root.join("recovered/t1-s1.jsonl"),
        resume
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let make_env = |fixtures: std::path::PathBuf| {
        let mut env = env::ChildEnv::isolated()
            .with_binary(ProviderKind::Claude, fake_binary())
            .with_variable("CLAUDE_CONFIG_DIR", config_root.as_os_str());
        env.fixtures = Some(fixtures);
        env
    };
    let read = || store::load(&Connection::open(&db_path).unwrap()).unwrap();
    let count = || {
        Connection::open(&db_path)
            .unwrap()
            .query_row("SELECT count(*) FROM cursor_updates", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    f.runtime.block_on(async {
        async fn ready(engine: &crate::Engine, thread: ThreadId) -> TerminalHandle {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(handle) = engine.terminal(thread)
                        && handle
                            .terminal
                            .as_ref()
                            .unwrap()
                            .emulator
                            .lock()
                            .text()
                            .iter()
                            .any(|line| line == "VEYRA_TUI_READY")
                    {
                        return handle;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap()
        }
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            db,
            Spawner::Cli(Box::new(make_env(root.clone()))),
            None,
        )
        .unwrap();
        engine
            .send(Command::SwitchView {
                thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        let handle = ready(&engine, thread).await;
        assert_eq!(
            read().threads[&thread].native_cursor.as_deref(),
            Some("sdk-anchor")
        );
        let before = count();
        handle.write(b"hello\x1b\r").unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if fs::read_to_string(&history_path).unwrap().lines().count() == 4 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            handle
                .terminal
                .as_ref()
                .unwrap()
                .emulator
                .lock()
                .text()
                .iter()
                .any(|line| line == "1b")
        );
        assert!(
            handle
                .terminal
                .as_ref()
                .unwrap()
                .emulator
                .lock()
                .text()
                .iter()
                .any(|line| line == "0d")
        );
        engine
            .send(Command::SwitchView {
                thread,
                to: View::Chat,
            })
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while read().threads[&thread].import_pending {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let p = read();
        assert_eq!(p.threads[&thread].native, Some(native.clone()));
        assert_eq!(count(), before + 1);
        assert_eq!(
            p.items
                .values()
                .filter_map(|item| if let ItemBody::TerminalSpan(span) = &item.body {
                    Some(span.text.as_str())
                } else {
                    None
                })
                .collect::<Vec<_>>(),
            ["hello\x1b\nfake reply: hello\x1b"]
        );
        tokio::time::sleep(Duration::from_millis(1700)).await;
        engine
            .send(Command::SwitchView {
                thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        ready(&engine, thread).await;
        let empty_before = count();
        engine
            .send(Command::SwitchView {
                thread,
                to: View::Chat,
            })
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while read().threads[&thread].import_pending {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(count(), empty_before);
        assert_eq!(read().items, p.items);
        tokio::time::sleep(Duration::from_millis(1700)).await;
        engine
            .send(Command::SwitchView {
                thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        let handle = ready(&engine, thread).await;
        handle.write(b"quit turn\r").unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while fs::read_to_string(&history_path).unwrap().lines().count() != 7 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        engine.on_app_quit().await;
        engine.shutdown_wait().await;
        assert!(read().threads[&thread].import_pending);
        Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE threads SET view='terminal', import_pending=0 WHERE id=?1",
                [thread.to_string()],
            )
            .unwrap();
        let recovery = crate::reconcile::decide(&read(), now());
        assert!(recovery.threads[&thread].import_pending);
        assert_eq!(recovery.threads[&thread].view, View::Chat);
        let mut events;
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            store::open(&db_path).unwrap(),
            Spawner::Cli(Box::new(make_env(root.join("recovered")))),
            None,
        )
        .unwrap();
        events = engine.subscribe();
        assert!(read().threads[&thread].import_pending);
        let before_quit_import = count();
        engine
            .send(Command::Send {
                thread,
                text: "after recovery".into(),
            })
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = events.recv().await {
                if matches!(
                    event,
                    UiEvent::Change {
                        event: crate::events::ChangeEvent::TurnStarted(_),
                        ..
                    }
                ) {
                    let p = read();
                    assert!(!p.threads[&thread].import_pending);
                    assert_eq!(count(), before_quit_import + 1);
                    assert_eq!(p.threads[&thread].native, Some(native.clone()));
                    let spans: Vec<_> = p
                        .items
                        .values()
                        .filter_map(|item| {
                            if let ItemBody::TerminalSpan(span) = &item.body {
                                Some(span.text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect();
                    assert_eq!(spans.len(), 2);
                    assert!(spans.contains(&"quit turn\nfake reply: quit turn"));
                    return;
                }
            }
            panic!("recovered session never started its first turn");
        })
        .await
        .unwrap();
        engine.shutdown_wait().await;
        assert!(engine.inner.groups.lock().unwrap().is_empty());
    });
    drop(scratch);
    eprintln!(
        "Claude round trip: raw ESC+CR echoed, one span/one cursor update, native retained; empty repeat added nothing; quit recovery imported before first turn"
    );
}

#[cfg(debug_assertions)]
#[test]
fn switch_stalled_sync_flushes_fake_tui_without_more_bytes() {
    use base64::Engine;
    use serde_json::json;
    use std::time::{Duration, Instant};
    let mut f = Fixture::new(LiveState::Parked);
    let root = std::path::Path::new("/tmp").join(format!("veyra-m7s1-sync-{}", ThreadId::new()));
    std::fs::create_dir(&root).unwrap();
    let _scratch = Scratch(root.clone());
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.native = Some(NativeSessionId(ThreadId::new().to_string()));
    c.threads.insert(f.thread, row);
    f.save(c);
    let bytes = b"\x1b[?2026hSYNC_STALLED\r\n";
    std::fs::write(root.join("t1-s1.jsonl"),format!("{}\n",json!({"type":"emit_raw","base64":base64::engine::general_purpose::STANDARD.encode(bytes)}))).unwrap();
    let mut env = env::ChildEnv::isolated()
        .with_binary(ProviderKind::Claude, fake_binary())
        .with_variable("CLAUDE_CONFIG_DIR", root.join("config"));
    env.fixtures = Some(root);
    let groups = env.groups.clone();
    f.runtime.block_on(async {
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            f.actor.db,
            Spawner::Cli(Box::new(env)),
            None,
        )
        .unwrap();
        let started = Instant::now();
        engine
            .send(Command::SwitchView {
                thread: f.thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        let flushed = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(handle) = engine.terminal(f.thread)
                    && handle
                        .terminal
                        .as_ref()
                        .unwrap()
                        .emulator
                        .lock()
                        .text()
                        .iter()
                        .any(|line| line == "SYNC_STALLED")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        engine.shutdown_wait().await;
        flushed.unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(groups.lock().unwrap().is_empty());
    });
}

#[test]
fn import_must_finish_before_ready_can_start_queued_work() {
    let mut f = Fixture::new(LiveState::Starting);
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.import_pending = true;
    let mut c = ChangeSet::default();
    c.threads.insert(f.thread, row);
    f.save(c);
    f.event(ProviderEvent::Session(SessionEvent::Ready {
        native: NativeSessionId("not-imported".into()),
        cli_version: None,
        pid: 1,
        pgid: 1,
        capabilities: vec![],
    }));
    assert_eq!(f.controls.try_recv().unwrap(), SessionControl::Shutdown);
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Starting { .. }
    ));
    assert!(f.loaded().threads[&f.thread].import_pending);
}
