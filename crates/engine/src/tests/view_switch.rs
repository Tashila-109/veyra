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
        f.command(Command::SwitchView {
            thread: f.thread,
            to: View::Terminal
        }),
        Err(CommandError::Busy(WaitReason::PendingRequest))
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
    row.native = Some(NativeSessionId("parked-session".into()));
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
        let (_scratch, env) = grok_terminal_env("parked-session");
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            f.actor.db,
            Spawner::Cli(Box::new(env)),
            None,
        )
        .unwrap();
        let groups = engine.inner.groups.clone();
        engine
            .send(Command::SwitchView {
                thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Some(handle) = engine.terminal(thread)
                    && handle
                        .emulator()
                        .unwrap()
                        .lock()
                        .text()
                        .iter()
                        .any(|line| line == "VEYRA_TUI_READY")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        engine.shutdown_wait().await;
        terminal.unwrap();
        assert!(groups.lock().unwrap().is_empty());
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
    let root =
        std::path::Path::new("/tmp").join(format!("veyra-term-terminal-{}", ThreadId::new()));
    std::fs::create_dir(&root).unwrap();
    let _ = native;
    std::fs::write(root.join("t1-s1.jsonl"), "").unwrap();
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
    row.native = Some(NativeSessionId("echo-session".into()));
    c.threads.insert(f.thread, row);
    f.save(c);
    let (out, mut receiver) = mpsc::channel(256);
    let (_scratch, env) = grok_terminal_env("echo-session");
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
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.native = Some(NativeSessionId("conversation".into()));
    c.threads.insert(f.thread, row);
    f.save(c);
    f.command(Command::SwitchView {
        thread: f.thread,
        to: View::Terminal,
    })
    .unwrap();
    assert_eq!(
        f.controls.try_recv().unwrap(),
        SessionControl::ShutdownForTerminal
    );
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
        event: TerminalEvent::Exited {
            started: true,
            crash: None,
        },
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
    let root = Path::new("/tmp").join(format!("veyra-term-grace-{}", ThreadId::new()));
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
    let mut f = Fixture::new(LiveState::Idle);
    let runtime = Runtime::new().unwrap();
    runtime.block_on(async {
        let (out, mut events) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: root.clone(),
                config: config(),
                native: None,

                import: None,
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
                        let mut c = ChangeSet::default();
                        let mut row = f.actor.projection.threads[&f.thread].clone();
                        row.native = Some(NativeSessionId("conversation".into()));
                        c.threads.insert(f.thread, row);
                        let mut live = f.actor.live[&f.thread].clone();
                        live.live = Live::Structured {
                            session: handle.clone(),
                            pid: 1,
                            pgid: 1,
                            draining: None,
                        };
                        c.live.insert(f.thread, live);
                        f.save(c);
                        let (reply, result) = oneshot::channel();
                        f.actor.handle(EngineMsg::Command {
                            command: Command::SwitchView {
                                thread: f.thread,
                                to: View::Terminal,
                            },
                            reply,
                        });
                        result.await.unwrap().unwrap();
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
    let root = std::path::Path::new("/tmp").join(format!("veyra-term-cursor-{}", ThreadId::new()));
    std::fs::create_dir_all(root.join("projects/isolated")).unwrap();
    let _scratch = Scratch(root.clone());
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude/session-file-import.jsonl"),
        root.join("projects/isolated/native-import.jsonl"),
    )
    .unwrap();
    let history = crate::provider::claude::session_file_import(
        &root,
        &NativeSessionId("native-import".into()),
        &HistoryRequest {
            cursor: Some("old-user".into()),
            sent: ["sent".into()].into(),
        },
    )
    .unwrap();
    f.event(ProviderEvent::Session(SessionEvent::History {
        native: native.clone(),
        history: history.clone(),
    }));
    let p = f.loaded();
    assert_eq!(
        p.threads[&f.thread].native_cursor.as_deref(),
        Some("new-user")
    );
    assert!(!p.threads[&f.thread].import_pending);
    assert_eq!(p.items.values().filter(|item| matches!(&item.body, ItemBody::TerminalSpan(span) if span.text == "second\nreply second")).count(), 1);
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
    let root = Path::new("/tmp").join(format!("veyra-term-roundtrip-{}", ThreadId::new()));
    crate::platform::private_dir(&root).unwrap();
    let scratch = Scratch(root.clone());
    let f = Fixture::new(LiveState::Parked);
    let thread = f.thread;
    let native = NativeSessionId(ThreadId::new().to_string());
    let config_root = root.join("claude-config");
    fs::create_dir_all(config_root.join("projects/veyra-fake")).unwrap();
    let history_path = config_root.join(format!("projects/veyra-fake/{}.jsonl", native.0));
    fs::write(&history_path, format!("{}\n", json!({"sessionId":native.0,"uuid":"sdk-anchor","parentUuid":null,"isSidechain":false,"type":"user","entrypoint":"sdk-cli","message":{"content":[{"type":"text","text":"already in chat"}]}}))).unwrap();
    let db_path = root.join("test.db");
    let mut db = store::open(&db_path).unwrap();
    let mut c = ChangeSet {
        projects: f.actor.projection.projects.clone(),
        ..Default::default()
    };
    let mut row = f.actor.projection.threads[&thread].clone();
    row.cwd = root.clone();
    row.native = Some(native.clone());
    row.native_cursor = Some("sdk-anchor".into());
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
                    assert_eq!(count(), before_quit_import + 2);
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
    let root = std::path::Path::new("/tmp").join(format!("veyra-term-sync-{}", ThreadId::new()));
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
fn import_failure_before_ready_clears_pending_and_starts_queued_work() {
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
    assert!(matches!(
        f.actor.live[&f.thread].live,
        Live::Structured { .. }
    ));
    assert!(!f.loaded().threads[&f.thread].import_pending);
    assert!(f.loaded().items.values().any(|item| matches!(&item.body, ItemBody::Notice(text) if text.text == crate::provider::IMPORT_NOTICE)));
}

#[test]
fn no_native_refuses_switch() {
    let mut f = Fixture::new(LiveState::Parked);
    let result = f.command(Command::SwitchView {
        thread: f.thread,
        to: View::Terminal,
    });
    assert!(matches!(&result, Err(CommandError::Busy(_))), "{result:?}");
    assert_eq!(
        result.unwrap_err().to_string(),
        "Send a message first; the terminal opens the CLI's conversation."
    );
    assert_eq!(f.loaded().threads[&f.thread].view, View::Chat);
}

#[test]
fn stop_before_terminal_has_no_import() {
    let mut f = Fixture::new(LiveState::Idle);
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.native = Some(NativeSessionId("conversation".into()));
    c.threads.insert(f.thread, row);
    f.save(c);
    f.command(Command::SwitchView {
        thread: f.thread,
        to: View::Terminal,
    })
    .unwrap();
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    assert_eq!(f.loaded().threads[&f.thread].view, View::Chat);
    assert!(!f.loaded().threads[&f.thread].import_pending);
}

#[test]
fn claude_truncated_line_is_skipped() {
    use std::io::Write;
    let root = std::path::Path::new("/tmp").join(format!("veyra-term-cut-{}", ThreadId::new()));
    std::fs::create_dir_all(root.join("projects/test")).unwrap();
    let scratch = Scratch(root);
    let file = scratch.0.join("projects/test/native-import.jsonl");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude/session-file-import.jsonl"),
        &file,
    )
    .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"{\"type\":")
        .unwrap();
    let history = claude::session_file_import(
        &scratch.0,
        &NativeSessionId("native-import".into()),
        &HistoryRequest {
            cursor: Some("anchor".into()),
            ..Default::default()
        },
    );
    assert!(history.is_ok(), "{history:?}");
    assert_eq!(history.unwrap().text, "second\nreply second");
}

#[test]
fn unknown_cursor_adopts_without_text() {
    let root = std::path::Path::new("/tmp").join(format!("veyra-term-unknown-{}", ThreadId::new()));
    std::fs::create_dir_all(root.join("projects/test")).unwrap();
    let scratch = Scratch(root);
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude/session-file-import.jsonl"),
        scratch.0.join("projects/test/native-import.jsonl"),
    )
    .unwrap();
    let history = claude::session_file_import(
        &scratch.0,
        &NativeSessionId("native-import".into()),
        &HistoryRequest::default(),
    )
    .unwrap();
    assert!(history.text.is_empty(), "{}", history.text);
    assert!(history.cursor.is_some());
}

#[cfg(debug_assertions)]
fn terminal_fixture(
    provider: ProviderKind,
    records: &[serde_json::Value],
) -> (Fixture, Scratch, env::ChildEnv) {
    let mut f = Fixture::new(LiveState::Parked);
    let root = std::path::Path::new("/tmp").join(format!("veyra-term-repair-{}", ThreadId::new()));
    crate::platform::private_dir(&root).unwrap();
    std::fs::write(
        root.join("t1-s1.jsonl"),
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.provider = provider;
    row.cwd = root.clone();
    row.native = Some(NativeSessionId(ThreadId::new().to_string()));
    row.native_cursor = Some("anchor".into());
    c.threads.insert(f.thread, row);
    f.save(c);
    let mut db = store::open(&root.join("test.db")).unwrap();
    let tx = db.transaction().unwrap();
    store::apply(
        &tx,
        &ChangeSet {
            projects: f.actor.projection.projects.clone(),
            threads: f.actor.projection.threads.clone(),
            ..Default::default()
        },
        now(),
    )
    .unwrap();
    tx.commit().unwrap();
    f.actor.db = db;
    let mut env = env::ChildEnv::isolated()
        .with_binary(provider, fake_binary())
        .with_variable("CLAUDE_CONFIG_DIR", root.join("config"));
    env.fixtures = Some(root.clone());
    (f, Scratch(root), env)
}
#[cfg(debug_assertions)]
async fn terminal_grid_containing(
    engine: &crate::Engine,
    thread: ThreadId,
    text: &str,
) -> TerminalHandle {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
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
                    .any(|line| line.contains(text))
            {
                return handle;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
#[cfg(debug_assertions)]
#[test]
fn large_paste_keeps_reader_moving() {
    use base64::Engine;
    use serde_json::json;
    use std::time::{Duration, Instant};
    let raw = |bytes: &[u8]| json!({"type":"emit_raw","base64":base64::engine::general_purpose::STANDARD.encode(bytes)});
    let (f, _scratch, env) = terminal_fixture(
        ProviderKind::Claude,
        &[
            raw(b"PASTE_READY\r\n"),
            json!({"type":"spin","ms":300}),
            raw(b"\x1b[6n"),
            json!({"type":"spin","ms":100}),
            raw(b"GRID_MOVED\r\n"),
            json!({"type":"spin","ms":2000}),
        ],
    );
    f.runtime.block_on(async {
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            f.actor.db,
            Spawner::Cli(Box::new(env)),
            None,
        )
        .unwrap();
        engine
            .send(Command::SwitchView {
                thread: f.thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        let handle = terminal_grid_containing(&engine, f.thread, "PASTE_READY").await;
        let (sent, result) = std::sync::mpsc::channel();
        let start = Instant::now();
        let writing = std::thread::spawn(move || {
            let outcome = handle.write(&vec![b'x'; 8 * 1024 * 1024]);
            let _ = sent.send(outcome);
        });
        let returned = result.recv_timeout(Duration::from_millis(200)).is_ok();
        let grid = tokio::time::timeout(
            Duration::from_millis(1200),
            terminal_grid_containing(&engine, f.thread, "GRID_MOVED"),
        )
        .await
        .is_ok();
        engine.shutdown_wait().await;
        writing.join().unwrap();
        assert!(
            returned,
            "paste blocked its caller for {:?}",
            start.elapsed()
        );
        assert!(grid, "query reply stalled the reader");
        assert!(engine.inner.groups.lock().unwrap().is_empty());
    });
}
#[cfg(debug_assertions)]
#[test]
fn failed_terminal_shows_reason_without_import() {
    let (f, _scratch, mut env) = terminal_fixture(ProviderKind::Grok, &[]);
    env = env.with_binary(ProviderKind::Grok, "/nonexistent/terminal-test".into());
    f.runtime.block_on(async {
        let engine=crate::Engine::launch(&tokio::runtime::Handle::current(),f.actor.db,Spawner::Cli(Box::new(env)),None).unwrap();
        engine.send(Command::SwitchView {thread:f.thread,to:View::Terminal}).await.unwrap().unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(3),async {loop {if engine.terminal(f.thread).is_none() {return engine.snapshot(f.thread).await.unwrap();}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}}).await;
        engine.shutdown_wait().await;let snapshot=result.unwrap();
        assert!(!store::load(&Connection::open(_scratch.0.join("test.db")).unwrap()).unwrap().threads[&f.thread].import_pending);assert!(snapshot.items.iter().any(|item|matches!(&item.body,ItemBody::Notice(notice) if notice.text.contains("not installed"))));
    });
}
#[cfg(debug_assertions)]
#[test]
fn signed_out_terminal_keeps_sign_in_reason() {
    let (f, _scratch, env) = terminal_fixture(
        ProviderKind::Grok,
        &[
            serde_json::json!({"type":"exit","code":1,"stderr":["Sign in to Grok before continuing"]}),
        ],
    );
    f.runtime.block_on(async {
        let engine=crate::Engine::launch(&tokio::runtime::Handle::current(),f.actor.db,Spawner::Cli(Box::new(env)),None).unwrap();engine.send(Command::SwitchView {thread:f.thread,to:View::Terminal}).await.unwrap().unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(3),async {loop {if engine.terminal(f.thread).is_none() {return engine.snapshot(f.thread).await.unwrap();}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}}).await;
        engine.shutdown_wait().await;let snapshot=result.unwrap();assert!(store::load(&Connection::open(_scratch.0.join("test.db")).unwrap()).unwrap().threads[&f.thread].import_pending);assert!(snapshot.items.iter().any(|item|matches!(&item.body,ItemBody::Notice(notice) if notice.text.contains("Sign in to Grok"))));
    });
}
#[cfg(debug_assertions)]
#[test]
fn worker_panic_still_exits() {
    let (f, _scratch, env) = terminal_fixture(ProviderKind::Claude, &[]);
    let gate = env.closing.clone();
    let _ = std::thread::spawn(move || {
        let _locked = gate.lock.lock().unwrap();
        panic!("owned launch gate poison");
    })
    .join();
    let (out, mut events) = mpsc::channel(256);
    let row = &f.actor.projection.threads[&f.thread];
    let _entered = f.runtime.enter();
    let _handle = crate::terminal::spawn(
        ProviderKind::Claude,
        SessionSpec {
            thread: f.thread,
            cwd: row.cwd.clone(),
            config: config(),
            native: row.native.clone(),
            import: None,
            root: false,
        },
        env,
        out,
        1,
        crate::terminal::Colors::default(),
    )
    .unwrap();
    let exited = f.runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(message) = events.recv().await {
                if matches!(
                    message,
                    EngineMsg::Terminal {
                        event: TerminalEvent::Exited { .. },
                        ..
                    }
                ) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    });
    assert!(exited, "worker panic left the terminal live");
}
#[test]
fn structured_cursor_tracks_turn_state() {
    for provider in [ProviderKind::Claude, ProviderKind::Grok] {
        let mut f = Fixture::new(LiveState::Idle);
        let mut c = ChangeSet::default();
        let mut row = f.actor.projection.threads[&f.thread].clone();
        row.provider = provider;
        c.threads.insert(f.thread, row);
        f.save(c);
        let Ack::Turn(turn) = f
            .command(Command::Send {
                thread: f.thread,
                text: "sent prompt".into(),
            })
            .unwrap()
        else {
            panic!("turn")
        };
        let p = f.loaded();
        assert!(matches!(p.turns[&turn].state, TurnState::Running { .. }));
        assert_eq!(p.threads[&f.thread].native_cursor, Some(turn.to_string()));
    }
}

#[cfg(debug_assertions)]
fn rpc_terminal_roundtrip(
    provider: ProviderKind,
    fixture: &str,
    cursor: &str,
    expected: &str,
    typed: &[u8],
) {
    use std::time::Duration;
    let (mut f, scratch, env) = terminal_fixture(provider, &[]);
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.native = Some(NativeSessionId("native-import".into()));
    row.native_cursor = Some(cursor.into());
    c.threads.insert(f.thread, row);
    f.save(c);
    let folder = match provider {
        ProviderKind::Codex => "codex",
        ProviderKind::Grok => "grok",
        ProviderKind::Claude => unreachable!(),
    };
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/{folder}/{fixture}.jsonl")),
    )
    .unwrap();
    std::fs::write(scratch.0.join("t1-s2.jsonl"), text).unwrap();
    let read = || store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
    f.runtime.block_on(async {
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            f.actor.db,
            Spawner::Cli(Box::new(env)),
            None,
        )
        .unwrap();
        engine
            .send(Command::SwitchView {
                thread: f.thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        let ready = tokio::time::timeout(
            Duration::from_secs(3),
            terminal_grid_containing(&engine, f.thread, "VEYRA_TUI_READY"),
        )
        .await;
        if ready.is_err() {
            engine.shutdown_wait().await;
            panic!("Parked opened a structured process before its TUI");
        }
        ready.unwrap().write(typed).unwrap();
        terminal_grid_containing(&engine, f.thread, "0d").await;
        engine
            .send(Command::SwitchView {
                thread: f.thread,
                to: View::Chat,
            })
            .await
            .unwrap()
            .unwrap();
        let imported = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let p = read();
                if !p.threads[&f.thread].import_pending {
                    return p;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        engine.shutdown_wait().await;
        let p = imported.unwrap();
        assert_eq!(
            p.threads[&f.thread].native,
            Some(NativeSessionId("native-import".into()))
        );
        let spans = p
            .items
            .values()
            .filter_map(|item| {
                if let ItemBody::TerminalSpan(span) = &item.body {
                    Some(span.text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(spans, [expected]);
        assert!(engine.inner.groups.lock().unwrap().is_empty());
    });
}
#[cfg(debug_assertions)]
#[test]
fn codex_roundtrip_without_capture() {
    rpc_terminal_roundtrip(
        ProviderKind::Codex,
        "turns-list-import",
        "anchor",
        "second\nreply second\nthird\nreply third",
        b"second\rthird\r",
    );
}
#[cfg(debug_assertions)]
#[test]
fn grok_roundtrip_without_capture() {
    rpc_terminal_roundtrip(
        ProviderKind::Grok,
        "load-replay-import",
        "sent",
        "second\nreply second",
        b"second\r",
    );
}

#[test]
fn grok_not_found_import_runs_queued_work() {
    use serde_json::json;
    let mut f = Fixture::new(LiveState::Starting);
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.provider = ProviderKind::Grok;
    row.native = Some(NativeSessionId("missing".into()));
    row.native_cursor = Some("anchor".into());
    row.import_pending = true;
    c.threads.insert(f.thread, row);
    f.save(c);
    let turn = f.user("continue after terminal");
    let mut codec = crate::provider::grok::Codec::new(
        SessionSpec {
            thread: f.thread,
            cwd: "/tmp".into(),
            config: config(),
            native: Some(NativeSessionId("missing".into())),
            import: Some(HistoryRequest {
                cursor: Some("anchor".into()),
                ..Default::default()
            }),
            root: false,
        },
        1,
        None,
        false,
    );
    let init = codec.initialize();
    let mut open = codec.value(json!({"jsonrpc":"2.0","id":init["id"],"result":{"protocolVersion":1,"agentCapabilities":{},"agentInfo":{}}})).unwrap();
    let open = open.take_outbound();
    let crate::provider::Outbound::Message(load) = &open[0] else {
        panic!("load message")
    };
    assert_eq!(load["method"], "session/load");
    let mut fallback = codec.value(json!({"jsonrpc":"2.0","id":load["id"],"error":{"code":-32602,"message":"Session not found"}})).unwrap();
    let fallback = fallback.take_outbound();
    let crate::provider::Outbound::Message(new) = &fallback[0] else {
        panic!("new message")
    };
    assert_eq!(new["method"], "session/new");
    let ready = codec
        .value(json!({"jsonrpc":"2.0","id":new["id"],"result":{"sessionId":"replacement"}}))
        .unwrap();
    for event in ready.events {
        f.event(event);
    }
    let p = f.loaded();
    assert!(!p.threads[&f.thread].import_pending);
    assert_eq!(
        p.threads[&f.thread].native.as_ref().unwrap().0,
        "replacement"
    );
    assert!(matches!(p.turns[&turn].state, TurnState::Running { .. }));
    assert!(p.items.values().any(|item|matches!(&item.body,ItemBody::Notice(notice) if notice.text.contains("CLI still has them"))));
    assert!(
        matches!(f.controls.try_recv().unwrap(),SessionControl::StartTurn {turn:id,..} if id==turn)
    );
}

#[test]
fn codex_bad_page_still_resumes() {
    use serde_json::json;
    let mut codec = crate::provider::codex::Codec::new(
        SessionSpec {
            thread: ThreadId::new(),
            cwd: "/tmp".into(),
            config: config(),
            native: Some(NativeSessionId("native".into())),
            import: Some(HistoryRequest {
                cursor: Some("anchor".into()),
                ..Default::default()
            }),
            root: false,
        },
        1,
        None,
    );
    let init = codec.initialize();
    let opening = codec.value(json!({"id":init["id"],"result":{}})).unwrap();
    let page = opening
        .replies
        .iter()
        .find(|reply| reply["method"] == "thread/turns/list")
        .unwrap();
    let result=codec.value(json!({"id":page["id"],"result":{"data":[{"id":"newest","items":[{"type":"userMessage","content":"invalid"}]}],"nextCursor":null}}));
    assert!(result.is_ok(), "bad display history blocked resume");
    let decoded = result.unwrap();
    assert!(
        decoded
            .replies
            .iter()
            .any(|reply| reply["method"] == "thread/resume")
    );
    assert!(decoded.history.unwrap().failed);
}

#[cfg(debug_assertions)]
fn hangup_ignoring_terminal_fixture() -> (Fixture, Scratch, env::ChildEnv) {
    let (f, scratch, mut env) = terminal_fixture(ProviderKind::Claude, &[]);
    let script = scratch.0.join("terminal-fixture");
    std::fs::write(&script,"#!/usr/bin/python3\nimport signal,time\nsignal.signal(signal.SIGHUP,signal.SIG_IGN)\nprint('VEYRA_TUI_READY',flush=True)\nwhile True: time.sleep(1)\n").unwrap();
    assert!(
        std::process::Command::new("/bin/chmod")
            .env_clear()
            .args(["700"])
            .arg(&script)
            .status()
            .unwrap()
            .success()
    );
    env = env.with_binary(ProviderKind::Claude, script);
    (f, scratch, env)
}

#[cfg(debug_assertions)]
#[test]
fn closing_terminal_uses_one_second_ladder() {
    use std::time::{Duration, Instant};
    let (f, _scratch, env) = hangup_ignoring_terminal_fixture();
    let _cleanup = StandInGroups::new(&env);
    f.runtime.block_on(async {
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            f.actor.db,
            Spawner::Cli(Box::new(env)),
            None,
        )
        .unwrap();
        engine
            .send(Command::SwitchView {
                thread: f.thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        terminal_grid_containing(&engine, f.thread, "VEYRA_TUI_READY").await;
        let start = Instant::now();
        engine.shutdown_wait().await;
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(900));
        assert!(elapsed < Duration::from_secs(2), "closing took {elapsed:?}");
        assert!(engine.inner.groups.lock().unwrap().is_empty());
    });
}

#[cfg(debug_assertions)]
fn codex_import_before_resume(bad_history: bool) {
    use serde_json::json;
    use std::time::Duration;
    let (mut f, scratch, env) = terminal_fixture(ProviderKind::Codex, &[]);
    let native = f.loaded().threads[&f.thread]
        .native
        .as_ref()
        .unwrap()
        .0
        .clone();
    let mut records = vec![
        json!({"type":"transcript_start","provider":"codex","protocol":"json-rpc","cli_version":"fake","scenario":"structured cursor and failed display history","source":"Veyra authored"}),
        json!({"type":"expect_outbound","line":{"id":"<id:init>","method":"initialize","params":"<id:params>"}}),
        json!({"type":"emit_inbound","line":{"id":"<id:init>","result":{}}}),
        json!({"type":"expect_outbound","line":{"method":"initialized"}}),
    ];
    if bad_history {
        let mut c = ChangeSet::default();
        let mut row = f.loaded().threads[&f.thread].clone();
        row.import_pending = true;
        c.threads.insert(f.thread, row);
        f.save(c);
        records.extend([json!({"type":"expect_outbound","line":{"id":"<id:page>","method":"thread/turns/list","params":"<id:page-params>"}}),json!({"type":"emit_inbound","line":{"id":"<id:page>","result":{"data":[{"id":"newest","items":[{"type":"userMessage","content":"invalid"}]}]}}})]);
    }
    records.extend([json!({"type":"expect_outbound","line":{"id":"<id:resume>","method":"thread/resume","params":"<id:resume-params>"}}),json!({"type":"emit_inbound","line":{"id":"<id:resume>","result":{"thread":{"id":native}}}}),json!({"type":"expect_outbound","line":{"id":"<id:turn>","method":"turn/start","params":"<id:turn-params>"}}),json!({"type":"emit_inbound","line":{"id":"<id:turn>","result":{"turn":{"id":"server-turn","status":"inProgress","items":[]}}}}),json!({"type":"emit_inbound","line":{"method":"turn/completed","params":{"threadId":native,"turn":{"id":"server-turn","status":"completed","items":[]}}}}),json!({"type":"spin","ms":200}),json!({"type":"exit","code":0})]);
    std::fs::write(
        scratch.0.join("t1-s1.jsonl"),
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    f.runtime.block_on(async {
        let engine=crate::Engine::launch(&tokio::runtime::Handle::current(),f.actor.db,Spawner::Cli(Box::new(env)),None).unwrap();let Ack::Turn(turn)=engine.send(Command::Send {thread:f.thread,text:"queued work".into()}).await.unwrap().unwrap() else {panic!("turn")};
        let result=tokio::time::timeout(Duration::from_secs(5),async {loop {let p=store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();if matches!(p.turns[&turn].state,TurnState::Ended {..}) {return p;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await;engine.shutdown_wait().await;let p=result.unwrap();
        assert!(matches!(p.turns[&turn].state,TurnState::Ended {outcome:TurnOutcome::Completed,..}));assert!(!p.threads[&f.thread].import_pending);
        if bad_history {assert!(p.items.values().any(|item|matches!(&item.body,ItemBody::Notice(notice) if notice.text.contains("CLI still has them"))));}
        assert_eq!(p.threads[&f.thread].native_cursor.as_deref(),Some("server-turn"));
        assert!(engine.inner.groups.lock().unwrap().is_empty());
    });
}
#[cfg(debug_assertions)]
#[test]
fn codex_structured_cursor() {
    codex_import_before_resume(false);
}
#[cfg(debug_assertions)]
#[test]
fn codex_typed_bad_page_runs_queued_work() {
    codex_import_before_resume(true);
}

#[cfg(debug_assertions)]
#[test]
fn plain_shutdown_has_no_switch_grace() {
    use serde_json::json;
    use std::time::{Duration, Instant};
    let (f, scratch, env) = terminal_fixture(
        ProviderKind::Claude,
        &[
            json!({"type":"expect_outbound","line":{"type":"control_request","request_id":"<id:init>","request":{"subtype":"initialize"}}}),
            json!({"type":"emit_inbound","line":{"type":"control_response","response":{"subtype":"success","request_id":"<id:init>","response":{}}}}),
            json!({"type":"spin","ms":1600}),
            json!({"type":"touch","path":"graceful-exit"}),
            json!({"type":"exit","code":0}),
        ],
    );
    f.runtime.block_on(async {
        let (out, mut events) = mpsc::channel(256);
        let handle = session::spawn(
            SessionSpec {
                thread: f.thread,
                cwd: scratch.0.clone(),
                config: config(),
                native: None,
                import: None,
                root: false,
            },
            env,
            out,
            1,
        )
        .unwrap();
        let elapsed = tokio::time::timeout(Duration::from_secs(5), async {
            let mut started = None;
            while let Some(EngineMsg::Provider { event, .. }) = events.recv().await {
                match event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) => {
                        started = Some(Instant::now());
                        handle.control.send(SessionControl::Shutdown).unwrap();
                    }
                    ProviderEvent::Session(SessionEvent::Exited { crash }) => {
                        assert_eq!(crash, None);
                        return started.unwrap().elapsed();
                    }
                    _ => {}
                }
            }
            panic!("missing exit")
        })
        .await
        .unwrap();
        assert!(
            elapsed < Duration::from_millis(1300),
            "ordinary Shutdown took {elapsed:?}"
        );
        assert!(!scratch.0.join("graceful-exit").exists());
    });
}

#[test]
fn codex_unacknowledged_turn_has_unknown_cursor() {
    let mut f = Fixture::new(LiveState::Idle);
    let mut c = ChangeSet::default();
    let mut row = f.loaded().threads[&f.thread].clone();
    row.provider = ProviderKind::Codex;
    row.native = Some(NativeSessionId("native".into()));
    row.native_cursor = Some("previous-server-turn".into());
    c.threads.insert(f.thread, row);
    f.save(c);
    f.command(Command::Send {
        thread: f.thread,
        text: "not acknowledged yet".into(),
    })
    .unwrap();
    assert_eq!(f.loaded().threads[&f.thread].native_cursor, None);
    f.event(ProviderEvent::Session(SessionEvent::Exited {
        crash: Some("reply lost".into()),
    }));
    assert_eq!(f.loaded().threads[&f.thread].native_cursor, None);
}

#[cfg(debug_assertions)]
#[test]
fn grok_promoted_overlap_keeps_last_completed_cursor() {
    use serde_json::json;
    use std::time::Duration;
    let (f, scratch, env) = terminal_fixture(ProviderKind::Grok, &[]);
    let native = f.loaded().threads[&f.thread]
        .native
        .as_ref()
        .unwrap()
        .0
        .clone();
    let message = |id: &str, text: &str| json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":native,"_meta":{"promptId":id},"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}}});
    let complete = |id: &str| json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","method":"_x.ai/session/prompt_complete","params":{"sessionId":native,"promptId":id,"stopReason":"end_turn"}}});
    let records = [
        json!({"type":"expect_outbound","line":{"jsonrpc":"2.0","id":"<id:init>","method":"initialize","params":"<id:initparams>"}}),
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","id":"<id:init>","result":{"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{"resume":true}},"agentInfo":{}}}}),
        json!({"type":"expect_outbound","line":{"jsonrpc":"2.0","id":"<id:resume>","method":"session/resume","params":"<id:resumeparams>"}}),
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","id":"<id:resume>","result":{"sessionId":native}}}),
        json!({"type":"expect_outbound","line":{"jsonrpc":"2.0","id":"<id:rpc>","method":"session/prompt","params":"<id:promptparams>"}}),
        message("foreign-first", "skip before promotion"),
        message("foreign-newest", "skip before promotion"),
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","id":"<id:rpc>","result":{"stopReason":"end_turn"}}}),
        message("foreign-first", "first"),
        message("foreign-newest", "newest"),
        complete("foreign-newest"),
        message("foreign-first", " tail"),
        complete("foreign-first"),
        json!({"type":"spin","ms":200}),
        json!({"type":"exit","code":0}),
    ];
    std::fs::write(
        scratch.0.join("t1-s1.jsonl"),
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    f.runtime.block_on(async {
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            f.actor.db,
            Spawner::Cli(Box::new(env)),
            None,
        )
        .unwrap();
        engine
            .send(Command::Send {
                thread: f.thread,
                text: "own prompt".into(),
            })
            .await
            .unwrap()
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let p = store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
                if p.turns.values().any(|turn| {
                    turn.origin == TurnOrigin::Provider
                        && matches!(turn.state, TurnState::Ended { .. })
                }) {
                    return p;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        engine.shutdown_wait().await;
        let p = result.unwrap();
        assert_eq!(
            p.threads[&f.thread].native_cursor.as_deref(),
            Some("foreign-first")
        );
        assert_eq!(
            p.turns
                .values()
                .filter(|turn| turn.origin == TurnOrigin::Provider)
                .count(),
            1
        );
        assert!(p.items.values().any(
            |item| matches!(&item.body,ItemBody::AssistantText {text,..} if text=="first tail")
        ));
        assert!(
            p.items.values().any(
                |item| matches!(&item.body,ItemBody::AssistantText {text,..} if text=="newest")
            )
        );
        assert!(engine.inner.groups.lock().unwrap().is_empty());
    });
}

#[test]
fn stop_or_switch_before_ready_has_no_import() {
    for stop in [false, true] {
        let mut f = Fixture::new(LiveState::Terminal);
        let mut c = ChangeSet::default();
        let mut live = f.actor.live[&f.thread].clone();
        if let Live::Terminal { pid, .. } = &mut live.live {
            *pid = None;
        }
        c.live.insert(f.thread, live);
        f.save(c);
        let command = if stop {
            Command::StopThread { thread: f.thread }
        } else {
            Command::SwitchView {
                thread: f.thread,
                to: View::Chat,
            }
        };
        f.command(command).unwrap();
        let p = f.loaded();
        assert_eq!(p.threads[&f.thread].view, View::Chat);
        assert!(!p.threads[&f.thread].import_pending);
    }
}

#[test]
fn worker_setup_error_before_handle_is_visible() {
    let mut f = Fixture::new(LiveState::Parked);
    let mut c = ChangeSet::default();
    let mut row = f.loaded().threads[&f.thread].clone();
    row.view = View::Terminal;
    row.native = Some(NativeSessionId("conversation".into()));
    c.threads.insert(f.thread, row);
    f.save(c);
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Terminal {
        thread: f.thread,
        epoch: 1,
        event: TerminalEvent::Exited {
            started: false,
            crash: Some("PTY worker could not start".into()),
        },
    });
    let p = f.loaded();
    assert_eq!(p.threads[&f.thread].view, View::Chat);
    assert!(!p.threads[&f.thread].import_pending);
    assert!(p.items.values().any(|item|matches!(&item.body,ItemBody::Notice(notice) if notice.text=="PTY worker could not start")));
}

#[test]
fn switch_publishes_thread_view_to_window() {
    let mut f = Fixture::new(LiveState::Idle);
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.native = Some(NativeSessionId("window-conversation".into()));
    row.native_cursor = Some("window-anchor".into());
    let mut changes = ChangeSet::default();
    changes.threads.insert(f.thread, row.clone());
    f.save(changes);
    while f.events.try_recv().is_ok() {}
    f.command(Command::SwitchView {
        thread: f.thread,
        to: View::Terminal,
    })
    .unwrap();
    let mut shown = row;
    while let Ok(event) = f.events.try_recv() {
        if let UiEvent::Change {
            event:
                crate::events::ChangeEvent::ThreadCreated(row)
                | crate::events::ChangeEvent::ThreadUpdated(row),
            ..
        } = event
        {
            shown = *row;
        }
    }
    assert_eq!(shown.view, View::Terminal);
    assert_eq!(shown.native_cursor.as_deref(), Some("window-anchor"));
}

#[test]
fn claude_import_skips_lines_cut_inside_utf8() {
    let scratch =
        Scratch(std::path::Path::new("/tmp").join(format!("veyra-term-utf8-{}", ThreadId::new())));
    std::fs::create_dir_all(scratch.0.join("projects/fixture")).unwrap();
    let mut bytes = b"{\"type\":\"user\",\"message\":{\"content\":\"\xe2\x82\n".to_vec();
    bytes.extend(
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude/session-file-import.jsonl"),
        )
        .unwrap(),
    );
    bytes.extend_from_slice(b"\n{\"type\":\"user\",\"message\":{\"content\":\"\xe2\x82");
    std::fs::write(
        scratch.0.join("projects/fixture/native-import.jsonl"),
        bytes,
    )
    .unwrap();
    let history = claude::session_file_import(
        &scratch.0,
        &NativeSessionId("native-import".into()),
        &HistoryRequest {
            cursor: Some("anchor".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        history,
        History {
            cursor: Some("new-user".into()),
            text: "second\nreply second".into(),
            failed: false
        }
    );
}

#[cfg(debug_assertions)]
#[test]
fn native_quit_hangup_creates_no_failed_turn_or_crash_notice() {
    for _ in 0..5 {
        let (f, scratch, env) = terminal_fixture(ProviderKind::Claude, &[]);
        f.runtime.block_on(async {
            let engine=crate::Engine::launch(&tokio::runtime::Handle::current(),f.actor.db,Spawner::Cli(Box::new(env)),None).unwrap();
            engine.send(Command::SwitchView {thread:f.thread,to:View::Terminal}).await.unwrap().unwrap();
            terminal_grid_containing(&engine,f.thread,"VEYRA_TUI_READY").await;
            engine.on_app_quit().await;
            engine.shutdown_wait().await;
            let loaded=store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
            assert!(loaded.threads[&f.thread].import_pending);
            assert_eq!(loaded.threads[&f.thread].view,View::Chat);
            assert_eq!(loaded.turns.values().filter(|turn|matches!(turn.state,TurnState::Ended {outcome:TurnOutcome::Failed {..},..})).count(),0);
            assert_eq!(loaded.items.values().filter(|item|matches!(&item.body,ItemBody::Notice(notice) if notice.text.contains("terminal exited"))).count(),0);
            assert!(engine.inner.groups.lock().unwrap().is_empty());
        });
    }
}

#[test]
fn unknown_import_cursor_notifies_and_starts_queued_work() {
    for provider in [
        ProviderKind::Claude,
        ProviderKind::Codex,
        ProviderKind::Grok,
    ] {
        let mut f = Fixture::new(LiveState::Starting);
        let native = NativeSessionId("retained-native".into());
        let mut c = ChangeSet::default();
        let mut row = f.actor.projection.threads[&f.thread].clone();
        row.provider = provider;
        row.native = Some(native.clone());
        row.native_cursor = None;
        row.import_pending = true;
        c.threads.insert(f.thread, row);
        f.save(c);
        let queued = f.user("continue after unknown cursor");
        f.event(ProviderEvent::Session(SessionEvent::History {
            native: native.clone(),
            history: History {
                cursor: Some("latest-native-turn".into()),
                text: String::new(),
                failed: false,
            },
        }));
        let p = f.loaded();
        assert_eq!(p.items.values().filter(|item| matches!(&item.body, ItemBody::Notice(notice) if notice.text == crate::provider::IMPORT_NOTICE)).count(), 1, "{provider:?}");
        assert!(
            !p.items
                .values()
                .any(|item| matches!(item.body, ItemBody::TerminalSpan(_)))
        );
        assert_eq!(
            p.threads[&f.thread].native_cursor.as_deref(),
            Some("latest-native-turn")
        );
        assert!(!p.threads[&f.thread].import_pending);
        assert_eq!(p.threads[&f.thread].native, Some(native.clone()));
        assert!(matches!(p.turns[&queued].state, TurnState::Queued));
        f.event(ProviderEvent::Session(SessionEvent::Ready {
            native,
            cli_version: None,
            pid: 1,
            pgid: 1,
            capabilities: vec![],
        }));
        assert!(matches!(
            f.loaded().turns[&queued].state,
            TurnState::Running { .. }
        ));
        assert!(
            matches!(f.controls.try_recv().unwrap(), SessionControl::StartTurn {turn,text,..} if turn == queued && text == "continue after unknown cursor")
        );
    }
}

#[test]
fn streamed_cursor_writes_thread_only_when_it_changes() {
    let mut f = Fixture::new(LiveState::Running);
    let turn = f.running().unwrap();
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.provider = ProviderKind::Grok;
    row.native_cursor = Some("previous-turn".into());
    c.threads.insert(f.thread, row);
    f.save(c);
    f.actor.db.execute_batch("CREATE TABLE thread_writes (value TEXT); CREATE TRIGGER count_thread_write AFTER UPDATE ON threads BEGIN INSERT INTO thread_writes VALUES (NEW.native_cursor); END;").unwrap();
    for _ in 0..20 {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Provider {
            thread: f.thread,
            epoch: 1,
            cursor: Some("streamed-turn".into()),
            event: turn_event(
                Some(turn),
                TurnEvent::TextDelta {
                    streamed: true,
                    key: NativeItemKey("answer".into()),
                    text: "x".into(),
                },
            ),
        });
    }
    let writes: i64 = f
        .actor
        .db
        .query_row("SELECT count(*) FROM thread_writes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(writes, 1);
    {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Provider {
            thread: f.thread,
            epoch: 1,
            cursor: Some("next-turn".into()),
            event: turn_event(
                Some(turn),
                TurnEvent::TextDelta {
                    streamed: true,
                    key: NativeItemKey("answer".into()),
                    text: "z".into(),
                },
            ),
        });
        f.actor.handle(EngineMsg::TextWrite);
    }
    let writes: i64 = f
        .actor
        .db
        .query_row("SELECT count(*) FROM thread_writes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(writes, 2);
    let p = f.loaded();
    assert_eq!(
        p.threads[&f.thread].native_cursor.as_deref(),
        Some("next-turn")
    );
    assert_eq!(p.items_for(f.thread).filter(|item| matches!(&item.body, ItemBody::AssistantText {text,..} if text == "xxxxxxxxxxxxxxxxxxxxz")).count(), 1);
}

#[cfg(debug_assertions)]
struct StandInGroups {
    groups: env::Groups,
    closing: std::sync::Arc<crate::LaunchGate>,
}
#[cfg(debug_assertions)]
impl StandInGroups {
    fn new(env: &env::ChildEnv) -> Self {
        Self {
            groups: env.groups.clone(),
            closing: env.closing.clone(),
        }
    }
}
#[cfg(debug_assertions)]
impl Drop for StandInGroups {
    fn drop(&mut self) {
        self.closing.close();
        if !stop_stand_in_groups(&self.groups) {
            eprintln!("stand-in cleanup did not finish reaping its groups");
        }
    }
}

#[cfg(debug_assertions)]
fn stop_stand_in_groups(groups: &env::Groups) -> bool {
    let pids = groups.lock().unwrap().clone();
    for &pid in &pids {
        let _ = crate::platform::signal_group(pid, crate::platform::Signal::Kill);
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let reaped = groups.lock().unwrap().is_empty()
            && pids
                .iter()
                .all(|pid| crate::platform::group_exists(*pid).is_ok_and(|exists| !exists));
        if reaped || std::time::Instant::now() >= until {
            return reaped;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(debug_assertions)]
#[test]
fn hangup_ignoring_stand_in_is_reaped_after_assertion_failure() {
    for assertion in [true, false] {
        let (f, _scratch, env) = hangup_ignoring_terminal_fixture();
        let groups = env.groups.clone();
        let mut pid = None;
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _cleanup = StandInGroups::new(&env);
            f.runtime.block_on(async {
                let engine = crate::Engine::launch(
                    &tokio::runtime::Handle::current(),
                    f.actor.db,
                    Spawner::Cli(Box::new(env)),
                    None,
                )
                .unwrap();
                engine
                    .send(Command::SwitchView {
                        thread: f.thread,
                        to: View::Terminal,
                    })
                    .await
                    .unwrap()
                    .unwrap();
                let handle = terminal_grid_containing(&engine, f.thread, "VEYRA_TUI_READY").await;
                pid = handle.terminal.as_ref().unwrap().pid();
                assert!(!assertion, "intentional stand-in assertion failure");
                panic!("intentional stand-in panic");
            });
        }));
        let reaped = groups.lock().unwrap().is_empty()
            && pid.is_some_and(|pid| !crate::platform::group_exists(pid).unwrap());
        let cleaned = stop_stand_in_groups(&groups);
        eprintln!("stand_in_group={pid:?} reaped_on_unwind={reaped} forced_assertion={assertion}");
        assert!(cleaned, "emergency test cleanup failed");
        assert!(failed.is_err());
        assert!(
            reaped,
            "unwinding left the SIGHUP-ignoring stand-in group alive or unreaped"
        );
    }
}
