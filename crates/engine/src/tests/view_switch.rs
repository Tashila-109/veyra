#![allow(clippy::disallowed_types)] // Import tests construct native history and provider wire envelopes.
use super::*;

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug)]
enum HeldCancellation {
    Interrupt,
    StopThread,
    StorageBlocked,
}

#[cfg(debug_assertions)]
struct HeldSessionCleanup {
    runtime: tokio::runtime::Handle,
    env: env::ChildEnv,
    session: SessionHandle,
}
#[cfg(debug_assertions)]
impl Drop for HeldSessionCleanup {
    fn drop(&mut self) {
        self.env.closing.close();
        let _ = self.session.control.send(SessionControl::Shutdown);
        for pid in self.env.groups.lock().unwrap().iter().copied() {
            let _ = crate::platform::signal_group(pid, crate::platform::Signal::Kill);
        }
        let cleaned = self.runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !self.env.groups.lock().unwrap().is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
        });
        if cleaned.is_err() && !std::thread::panicking() {
            panic!("held-prompt session cleanup did not reap its group");
        }
    }
}

#[cfg(debug_assertions)]
fn cancel_held_prompt(cancellation: HeldCancellation) {
    use serde_json::json;
    use std::{fs, time::Duration};
    let scratch = Scratch(
        std::path::Path::new("/tmp").join(format!("veyra-term-held-prompt-{}", ThreadId::new())),
    );
    fs::create_dir_all(scratch.0.join("projects/fixture")).unwrap();
    let binary = scratch.0.join("provider");
    fs::write(
        &binary,
        "#!/usr/bin/python3\nimport json,pathlib,sys\nroot=pathlib.Path.cwd()\nhistory=root/'projects/fixture/n\
            ative.jsonl'\nactive=None\nfor raw in sys.stdin:\n    wire=json.loads(raw)\n    with (root/'stdin.j\
                sonl').open('a') as out: out.write(raw)\n    if wire.get('type')=='user':\n        active=wire[\
                    'uuid']\n        with history.open('a') as out:\n            out.write(json.dumps({'type':'\
                        user','entrypoint':'sdk-cli','sessionId':'native','uuid':active,'message':wire['message\
                            ']})+'\\n')\n    elif wire.get('request',{}).get('subtype')=='initialize':\
                                \n        print(json.dumps({'type':'control_response','response':{'subtype':'su\
                                    ccess','request_id':wire['request_id'],'response':{}}}),flush=True)\n    el\
                                        if wire.get('request',{}).get('subtype')=='interrupt' and active:\
                                            \n        print(json.dumps({'type':'result','subtype':'error_during\
                                                _execution','session_id':'native','is_error':True,'uuid':'resul\
                                                t','result':'interrupted'}),flush=True)\n        active=None\n",
    )
    .unwrap();
    assert!(
        std::process::Command::new("/bin/chmod")
            .arg("700")
            .arg(&binary)
            .status()
            .unwrap()
            .success()
    );
    let native = NativeSessionId("native".into());
    let history_path = scratch.0.join("projects/fixture/native.jsonl");
    fs::write(
        &history_path,
        format!(
            "{}\n",
            json!({"type":"user","entrypoint":"sdk-cli","sessionId":"native","\
        uuid":"previous","message":{"content":"already shown"}})
        ),
    )
    .unwrap();
    let mut f = Fixture::new(LiveState::Idle);
    let mut changes = ChangeSet::default();
    let mut row = f.loaded().threads[&f.thread].clone();
    row.cwd = scratch.0.clone();
    row.native = Some(native.clone());
    row.native_cursor = Some("previous".into());
    changes.threads.insert(f.thread, row);
    f.save(changes);
    let env = env::ChildEnv::isolated()
        .with_binary(ProviderKind::Claude, binary)
        .with_variable("CLAUDE_CONFIG_DIR", scratch.0.as_os_str());
    let (out, mut events) = mpsc::channel(256);
    let session = {
        let _entered = f.runtime.enter();
        session::spawn(
            SessionSpec {
                thread: f.thread,
                cwd: scratch.0.clone(),
                config: config(),
                native: Some(native.clone()),
                import: None,
                root: false,
            },
            env.clone(),
            out,
            1,
        )
        .unwrap()
    };
    let cleanup = HeldSessionCleanup {
        runtime: f.runtime.handle().clone(),
        env,
        session: session.clone(),
    };
    f.actor.live.get_mut(&f.thread).unwrap().live = Live::Starting {
        session: Some(session.clone()),
    };
    let ready = f.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap()
    });
    {
        let _entered = f.runtime.enter();
        f.actor.handle(ready);
    }
    let Ack::Turn(turn) = f
        .command(Command::Send {
            thread: f.thread,
            text: "held prompt".into(),
        })
        .unwrap()
    else {
        panic!("missing turn")
    };
    let dispatched = f.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap()
    });
    assert!(
        matches!(&dispatched, EngineMsg::Provider { event: ProviderEvent::Session(SessionEvent::Dispatched {
            turn: id }), .. } if *id == turn)
    );
    let started = std::time::Instant::now();
    match cancellation {
        HeldCancellation::Interrupt => {
            f.command(Command::Interrupt {
                thread: f.thread,
                turn,
            })
            .unwrap();
        }
        HeldCancellation::StopThread => {
            f.command(Command::StopThread { thread: f.thread }).unwrap();
        }
        HeldCancellation::StorageBlocked => {
            f.actor.fail_write = true;
            f.event(ProviderEvent::Session(SessionEvent::Notice {
                text: "storage write".into(),
            }));
        }
    }
    {
        let _entered = f.runtime.enter();
        f.actor.handle(dispatched);
    }
    let ended = f.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                let message = events.recv().await.unwrap();
                let end = match &message {
                    EngineMsg::Provider {
                        event:
                            ProviderEvent::Turn {
                                event: TurnEvent::TurnEnded { outcome, park },
                                ..
                            },
                        ..
                    } => Some((outcome.clone(), *park)),
                    _ => None,
                };
                f.actor.handle(message);
                if let Some(end) = end {
                    return end;
                }
            }
        })
        .await
        .unwrap()
    });
    let elapsed = started.elapsed();
    let kept_process = !cleanup.env.groups.lock().unwrap().is_empty();
    f.actor.fail_write = false;
    {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Tick);
    }
    let cursor = f.loaded().threads[&f.thread].native_cursor.clone();
    session.control.send(SessionControl::Shutdown).ok();
    f.runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(message) = events.recv().await {
                if matches!(
                    message,
                    EngineMsg::Provider {
                        event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                        ..
                    }
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();
    });
    drop(cleanup);
    let wires: Vec<serde_json::Value> = fs::read_to_string(scratch.0.join("stdin.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let order: Vec<_> = wires
        .iter()
        .map(|wire| {
            wire["request"]["subtype"]
                .as_str()
                .unwrap_or(wire["type"].as_str().unwrap())
        })
        .collect();
    use std::io::Write;
    let mut history_file = fs::OpenOptions::new()
        .append(true)
        .open(&history_path)
        .unwrap();
    for entry in [
        json!({"type":"user","entrypoint":"cli","sessionId":"native","uuid":"terminal-user","message":{"content\
            ":"typed in terminal"}}),
        json!({"type":"assistant","entrypoint":"cli","sessionId":"native","uuid":"terminal-reply","message":{"c\
            ontent":[{"type":"text","text":"terminal reply"}]}}),
    ] {
        writeln!(history_file, "{entry}").unwrap();
    }
    let history = claude::session_file_import(
        &scratch.0,
        &native,
        &HistoryRequest {
            cursor: cursor.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    eprintln!(
        "{cancellation:?}: stdin={order:?} elapsed={elapsed:?} end={ended:?} kept_process={kept_process} cursor\
            ={cursor:?} import={history:?}"
    );
    assert_eq!(order, ["initialize", "user", "interrupt"]);
    assert_eq!(wires[1]["uuid"], turn.to_string());
    assert_eq!(wires[1]["message"]["content"], "held prompt");
    assert_eq!(cursor.as_deref(), Some(turn.to_string().as_str()));
    assert_eq!(
        ended,
        (
            TurnOutcome::Interrupted {
                reason: if matches!(cancellation, HeldCancellation::StorageBlocked) {
                    StopReason::StorageBlocked.text().into()
                } else {
                    StopReason::User.text().into()
                },
            },
            false
        )
    );
    assert!(kept_process && elapsed < Duration::from_secs(2));
    assert_eq!(
        history,
        History {
            cursor: Some("terminal-user".into()),
            text: "typed in terminal\nterminal reply".into(),
            failed: false,
        }
    );
}

#[cfg(debug_assertions)]
#[test]
fn interrupt_held_prompt_preserves_import_cursor() {
    cancel_held_prompt(HeldCancellation::Interrupt);
}

#[cfg(debug_assertions)]
#[test]
fn stop_thread_held_prompt_preserves_import_cursor() {
    cancel_held_prompt(HeldCancellation::StopThread);
}

#[cfg(debug_assertions)]
#[test]
fn storage_blocked_held_prompt_preserves_import_cursor() {
    cancel_held_prompt(HeldCancellation::StorageBlocked);
}

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
    let mut worker =
        ActorWorker::spawn(actor, receiver, runtime, out.clone(), Some(release.clone()));
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
    worker.stop();
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
        json!({"type":"transcript_start","provider":"claude","protocol":"stream-json","cli_version":"fake","sce\
            nario":"graceful close","source":"Veyra authored"}),
        json!({"type":"expect_outbound","line":{"type":"control_request","request_id":"<id:initialize>","reques\
            t":{"subtype":"initialize"}}}),
        json!({"type":"emit_inbound","line":{"type":"control_response","response":{"subtype":"success","request\
            _id":"<id:initialize>","response":{}}}}),
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
    assert_eq!(
        p.items
            .values()
            .filter(
                |item| matches!(&item.body, ItemBody::TerminalSpan(span) if span.text
        == "second\nreply second")
            )
            .count(),
        1
    );
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
    fs::write(&history_path, format!("{}\n", json!({"sessionId":native.0,"uuid":"sdk-anchor","parentUuid":null,
        "isSidechain":false,"type":"user","entrypoint":"sdk-cli","message":{"content":[{"type":"text","text":"a\
            lready in chat"}]}}))).unwrap();
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
    db.execute_batch("CREATE TABLE cursor_updates (value TEXT); CREATE TRIGGER count_cursor AFTER UPDATE OF nat\
        ive_cursor ON threads WHEN OLD.native_cursor IS NOT NEW.native_cursor BEGIN INSERT INTO cursor_updates \
            VALUES (NEW.native_cursor); END;").unwrap();
    let header = json!({"type":"transcript_start","provider":"claude","protocol":"stream-json","cli_version":"f\
        ake","scenario":"round trip","source":"Veyra authored"});
    let init = [
        header.clone(),
        json!({"type":"expect_outbound","line":{"type":"control_request","request_id":"<id:init>","request":{"s\
            ubtype":"initialize"}}}),
        json!({"type":"emit_inbound","line":{"type":"control_response","response":{"subtype":"success","request\
            _id":"<id:init>","response":{}}}}),
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
    resume.push(json!({"type":"expect_outbound","line":{"type":"user","uuid":"<id:prompt>","session_id":"<id:se\
        ssion>","parent_tool_use_id":null,"message":{"role":"user","content":"after recovery"}}}));
    resume.push(json!({"type":"emit_inbound","line":{"type":"result","subtype":"success","session_id":"<id:sess\
        ion>","is_error":false,"result":"ok","uuid":"result"}}));
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
            ["hello\nfake reply: hello"]
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
        "Claude round trip: raw ESC+CR echoed, one span/one cursor update, native retained; empty repeat added \
            nothing; quit recovery imported before first turn"
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
    std::fs::write(
        root.join("t1-s1.jsonl"),
        format!(
            "{}\n",
            json!({"type":"emit_raw","base64\
        ":base64::engine::general_purpose::STANDARD.encode(bytes)})
        ),
    )
    .unwrap();
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
    assert!(f.loaded().items.values().any(
        |item| matches!(&item.body, ItemBody::Notice(text) if text.text ==
        crate::provider::IMPORT_NOTICE)
    ));
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
    let root = std::path::Path::new("/tmp").join(format!("veyra-term-fixture-{}", ThreadId::new()));
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
    let raw = |bytes: &[u8]| {
        json!({"type":"emit_raw","base64\
        ":base64::engine::general_purpose::STANDARD.encode(bytes)})
    };
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
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if engine.terminal(f.thread).is_none() {
                    return engine.snapshot(f.thread).await.unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        engine.shutdown_wait().await;
        let snapshot = result.unwrap();
        assert!(
            !store::load(&Connection::open(_scratch.0.join("test.db")).unwrap())
                .unwrap()
                .threads[&f.thread]
                .import_pending
        );
        assert!(snapshot.items.iter().any(|item| matches!(&item.body,
                ItemBody::Notice(notice) if notice.text.contains("not installed"))));
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
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if engine.terminal(f.thread).is_none() {
                    return engine.snapshot(f.thread).await.unwrap();
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        engine.shutdown_wait().await;
        let snapshot = result.unwrap();
        assert!(
            store::load(&Connection::open(_scratch.0.join("test.db")).unwrap())
                .unwrap()
                .threads[&f.thread]
                .import_pending
        );
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| matches!(&item.body,ItemBody::Notice(notice) if
                    notice.text.contains("Sign in to Grok")))
        );
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
        dispatched_cursor(&mut f, turn, turn.to_string().into());
        let p = f.loaded();
        assert!(matches!(p.turns[&turn].state, TurnState::Running { .. }));
        assert_eq!(
            p.threads[&f.thread].native_cursor,
            Some(turn.to_string().into())
        );
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
    let mut open = codec
        .value(
            json!({"jsonrpc":"2.0","id":init["id"],"result":{"protocolVersion":1,"agentCapab\
        ilities":{},"agentInfo":{}}}),
        )
        .unwrap();
    let open = open.take_outbound();
    let crate::provider::Outbound::Message(load) = &open[0] else {
        panic!("load message")
    };
    assert_eq!(load["method"], "session/load");
    let mut fallback = codec
        .value(
            json!({"jsonrpc":"2.0","id":load["id"],"error":{"code":-32602,"message":"Ses\
        sion not found"}}),
        )
        .unwrap();
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
    assert!(p.items.values().any(
        |item| matches!(&item.body,ItemBody::Notice(notice) if notice.text.contains("CL\
        I still has them"))
    ));
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
    let result = codec.value(
        json!({"id":page["id"],"result":{"data":[{"id":"newest","items":[{"type":"userMessag\
        e","content":"invalid"}]}],"nextCursor":null}}),
    );
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
    std::fs::write(
        &script,
        "#!/usr/bin/python3\nimport signal,time\nsignal.signal(signal.SIGHUP,signal.SIG_IGN)\
        \nprint('VEYRA_TUI_READY',flush=True)\nwhile True: time.sleep(1)\n",
    )
    .unwrap();
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
        json!({"type":"transcript_start","provider":"codex","protocol":"json-rpc","cli_version":"fake","scenari\
            o":"structured cursor and failed display history","source":"Veyra authored"}),
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
        records.extend([
            json!({"type":"expect_outbound","line":{"id":"<id:page>","method":"thread/turns/list","\
            params":"<id:page-params>"}}),
            json!({"type":"emit_inbound","line":{"id":"<id:page>","result":{"data\
                ":[{"id":"newest","items":[{"type":"userMessage","content":"invalid"}]}]}}}),
        ]);
    }
    records.extend([json!({"type":"expect_outbound","line":{"id":"<id:resume>","method":"thread/resume","params\
        ":"<id:resume-params>"}}),json!({"type":"emit_inbound","line":{"id":"<id:resume>","result":{"thread":{"\
            id":native}}}}),json!({"type":"expect_outbound","line":{"id":"<id:turn>","method":"turn/start","par\
                ams":"<id:turn-params>"}}),json!({"type":"emit_inbound","line":{"id":"<id:turn>","result":{"tur\
                    n":{"id":"server-turn","status":"inProgress","items":[]}}}}),json!({"type":"emit_inbound","\
                        line":{"method":"turn/completed","params":{"threadId":native,"turn":{"id":"server-turn\
                            ","status":"completed","items":[]}}}}),json!({"type":"spin","ms":200}),json!({"type\
                                ":"exit","code":0})]);
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
        let Ack::Turn(turn) = engine
            .send(Command::Send {
                thread: f.thread,
                text: "queued wo\
                rk"
                .into(),
            })
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("turn")
        };
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let p = store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
                if matches!(p.turns[&turn].state, TurnState::Ended { .. }) {
                    return p;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        engine.shutdown_wait().await;
        let p = result.unwrap();
        assert!(matches!(
            p.turns[&turn].state,
            TurnState::Ended {
                outcome: TurnOutcome::Completed,
                ..
            }
        ));
        assert!(!p.threads[&f.thread].import_pending);
        if bad_history {
            assert!(
                p.items
                    .values()
                    .any(|item| matches!(&item.body,ItemBody::Notice(notice) if
            notice.text.contains("CLI still has them")))
            );
        }
        assert_eq!(
            p.threads[&f.thread].native_cursor.as_deref(),
            Some("server-turn")
        );
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
            json!({"type":"expect_outbound","line":{"type":"control_request","request_id":"<id:init>","request\
                ":{"subtype":"initialize"}}}),
            json!({"type":"emit_inbound","line":{"type":"control_response","response":{"subtype":"success","req\
                uest_id":"<id:init>","response":{}}}}),
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
    let Ack::Turn(turn) = f
        .command(Command::Send {
            thread: f.thread,
            text: "not acknowledged yet".into(),
        })
        .unwrap()
    else {
        panic!("user turn")
    };
    dispatched_cursor(&mut f, turn, CursorUpdate::Unknown);
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
    let message = |id: &str, text: &str| {
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","method":"session\
        /update","params":{"sessionId":native,"_meta":{"promptId":id},"update":{"sessionUpdate":"agent_message_\
            chunk","content":{"type":"text","text":text}}}}})
    };
    let complete = |id: &str| {
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","method":"_x.ai/session/prom\
        pt_complete","params":{"sessionId":native,"promptId":id,"stopReason":"end_turn"}}})
    };
    let records = [
        json!({"type":"expect_outbound","line":{"jsonrpc":"2.0","id":"<id:init>","method":"initialize","params\
            ":"<id:initparams>"}}),
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","id":"<id:init>","result":{"protocolVersion":1,"ag\
            entCapabilities":{"sessionCapabilities":{"resume":true}},"agentInfo":{}}}}),
        json!({"type":"expect_outbound","line":{"jsonrpc":"2.0","id":"<id:resume>","method":"session/resume","p\
            arams":"<id:resumeparams>"}}),
        json!({"type":"emit_inbound","line":{"jsonrpc":"2.0","id":"<id:resume>","result":{"sessionId":native}}}),
        json!({"type":"expect_outbound","line":{"jsonrpc":"2.0","id":"<id:rpc>","method":"session/prompt","para\
            ms":"<id:promptparams>"}}),
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
        result.unwrap();
        let p = store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
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
    assert!(p.items.values().any(
        |item| matches!(&item.body,ItemBody::Notice(notice) if notice.text=="PTY worke\
        r could not start")
    ));
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
            engine.on_app_quit().await;
            engine.shutdown_wait().await;
            let loaded =
                store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
            assert!(loaded.threads[&f.thread].import_pending);
            assert_eq!(loaded.threads[&f.thread].view, View::Chat);
            assert_eq!(
                loaded
                    .turns
                    .values()
                    .filter(|turn| matches!(
                        turn.state,
                        TurnState::Ended {
                            outcome: TurnOutcome::Failed { .. },
                            ..
                        }
                    ))
                    .count(),
                0
            );
            assert_eq!(
                loaded
                    .items
                    .values()
                    .filter(|item| matches!(&item.body,ItemBody::Notice(notice) if
                notice.text.contains("terminal exited")))
                    .count(),
                0
            );
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
        assert_eq!(
            p.items
                .values()
                .filter(
                    |item| matches!(&item.body, ItemBody::Notice(notice) if notice.text
            == crate::provider::IMPORT_NOTICE)
                )
                .count(),
            1,
            "{provider:?}"
        );
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
            matches!(f.controls.try_recv().unwrap(), SessionControl::StartTurn {turn,text,..} if turn ==
                queued && text == "continue after unknown cursor")
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
    f.actor
        .db
        .execute_batch(
            "CREATE TABLE thread_writes (value TEXT); CREATE TRIGGER count_thread_write AFTER \
        UPDATE ON threads BEGIN INSERT INTO thread_writes VALUES (NEW.native_cursor); END;",
        )
        .unwrap();
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
    assert_eq!(
        p.items_for(f.thread)
            .filter(
                |item| matches!(&item.body, ItemBody::AssistantText {text,..} if
        text == "xxxxxxxxxxxxxxxxxxxxz")
            )
            .count(),
        1
    );
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

#[cfg(debug_assertions)]
struct ActorWorker {
    out: mpsc::Sender<EngineMsg>,
    worker: Option<std::thread::JoinHandle<()>>,
    release: Option<std::sync::mpsc::SyncSender<()>>,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(debug_assertions)]
impl ActorWorker {
    fn spawn(
        actor: Actor,
        receiver: mpsc::Receiver<EngineMsg>,
        runtime: tokio::runtime::Handle,
        out: mpsc::Sender<EngineMsg>,
        release: Option<std::sync::mpsc::SyncSender<()>>,
    ) -> Self {
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ended = finished.clone();
        let worker = Some(std::thread::spawn(move || {
            actor.run(receiver, runtime);
            ended.store(true, std::sync::atomic::Ordering::Release);
        }));
        Self {
            out,
            worker,
            release,
            finished,
        }
    }
    fn stop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(worker) = self.worker.take() {
            let sent = self.out.blocking_send(EngineMsg::Shutdown);
            let joined = worker.join();
            if !std::thread::panicking() {
                sent.unwrap();
                joined.unwrap();
            }
        }
    }
}
#[cfg(debug_assertions)]
impl Drop for ActorWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn pending_terminal_import_survives_a_failed_launch() {
    let mut f = Fixture::new(LiveState::Terminal);
    let mut c = ChangeSet::default();
    let mut row = f.loaded().threads[&f.thread].clone();
    let native = NativeSessionId("retained-terminal".into());
    row.native = Some(native.clone());
    row.native_cursor = Some("before-terminal".into());
    c.threads.insert(f.thread, row);
    f.save(c);
    f.command(Command::StopThread { thread: f.thread }).unwrap();
    {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Terminal {
            thread: f.thread,
            epoch: 1,
            event: TerminalEvent::Exited {
                started: true,
                crash: None,
            },
        });
    }
    assert!(f.loaded().threads[&f.thread].import_pending);
    f.command(Command::SwitchView {
        thread: f.thread,
        to: View::Terminal,
    })
    .unwrap();
    let epoch = f.actor.live[&f.thread].epoch;
    {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Terminal {
            thread: f.thread,
            epoch,
            event: TerminalEvent::Exited {
                started: false,
                crash: Some("launch failed".into()),
            },
        });
    }
    assert!(
        f.loaded().threads[&f.thread].import_pending,
        "failed launch discarded earlier terminal history"
    );
    f.command(Command::Send {
        thread: f.thread,
        text: "continue".into(),
    })
    .unwrap();
    let epoch = f.actor.live[&f.thread].epoch;
    let mut c = ChangeSet::default();
    let (control, _) = mpsc::unbounded_channel();
    let mut live = f.actor.live[&f.thread].clone();
    live.live = Live::Starting {
        session: Some(SessionHandle { control }),
    };
    c.live.insert(f.thread, live);
    f.save(c);
    f.event_epoch(
        epoch,
        ProviderEvent::Session(SessionEvent::History {
            native,
            history: History {
                cursor: Some("typed-terminal".into()),
                text: "earlier terminal message".into(),
                failed: false,
            },
        }),
    );
    assert!(
        f.loaded()
            .items
            .values()
            .any(|item| matches!(&item.body, ItemBody::TerminalSpan(span) if
        span.text == "earlier terminal message"))
    );
    assert!(!f.loaded().threads[&f.thread].import_pending);
}

#[test]
fn grok_import_precedes_a_queued_prompt_on_a_timestamp_tie() {
    let mut f = Fixture::new(LiveState::Starting);
    let mut c = ChangeSet::default();
    let mut row = f.loaded().threads[&f.thread].clone();
    row.provider = ProviderKind::Grok;
    row.native = Some(NativeSessionId("grok-native".into()));
    row.native_cursor = Some("anchor".into());
    row.import_pending = true;
    c.threads.insert(f.thread, row);
    f.save(c);
    let queued = f.user("queued while switching");
    let prior = f.turn(
        TurnOrigin::Provider,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            started: Some(0),
            at: 1,
        },
    );
    let mut c = ChangeSet::default();
    crate::commands::add_item(
        &f.actor.projection,
        &mut c,
        Item {
            id: ItemId::new(),
            thread: f.thread,
            turn: prior,
            seq: 0,
            body: ItemBody::AssistantText {
                text: "already visible".into(),
                done: true,
            },
        },
    );
    f.save(c);
    f.event(ProviderEvent::Session(SessionEvent::History {
        native: NativeSessionId("grok-native".into()),
        history: History {
            cursor: Some("terminal-turn".into()),
            text: "terminal before queue".into(),
            failed: false,
        },
    }));
    f.event(ProviderEvent::Session(SessionEvent::Ready {
        native: NativeSessionId("grok-native".into()),
        cli_version: None,
        pid: 1,
        pgid: 1,
        capabilities: vec![],
    }));
    let p = f.loaded();
    let imported = p
        .items
        .values()
        .find(|item| matches!(item.body, ItemBody::TerminalSpan(_)))
        .unwrap()
        .turn;
    let TurnState::Ended { at, .. } = p.turns[&imported].state else {
        panic!("import ended")
    };
    let mut c = ChangeSet::default();
    let mut row = p.turns[&queued].clone();
    row.state = TurnState::Running { since: at };
    c.turns.insert(queued, row);
    f.save(c);
    let snapshot = terminal_test_snapshot(&mut f);
    let order: Vec<_> = snapshot.items.iter().map(|item| item.turn).collect();
    assert_eq!(
        order,
        [prior, imported, queued],
        "span must precede prompt even with equal timestamps"
    );
    let later = f.user("queued after import");
    let order: Vec<_> = terminal_test_snapshot(&mut f)
        .items
        .iter()
        .map(|item| item.turn)
        .collect();
    assert_eq!(order, [prior, imported, queued, later]);
}

#[test]
fn claude_history_skips_malformed_typed_entries_and_accepts_unknown_kinds() {
    use serde_json::json;
    let scratch =
        Scratch(std::path::Path::new("/tmp").join(format!("veyra-term-typed-{}", ThreadId::new())));
    std::fs::create_dir_all(scratch.0.join("projects/fixture")).unwrap();
    let entry = |id: &str, content| {
        json!({"sessionId":"typed-native","uuid":id,"parentUuid":null,"isSidechain\
        ":false,"type":"user","entrypoint":"cli","message":{"content":content}})
    };
    let mut malformed = entry(
        "bad",
        json!([{"type":"text","text":17},{"type":"text","text":"must skip whole invalid entry"}]),
    );
    malformed["futureMetadata"] = json!({"ok":true});
    let records = [
        entry("anchor", json!("old")),
        malformed,
        entry(
            "good",
            json!([{"type":"future_block","anything":true},{"type":"text","text":"kept"}]),
        ),
    ];
    std::fs::write(
        scratch.0.join("projects/fixture/typed-native.jsonl"),
        records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let history = claude::session_file_import(
        &scratch.0,
        &NativeSessionId("typed-native".into()),
        &HistoryRequest {
            cursor: Some("anchor".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(history.text, "kept");
    assert_eq!(history.cursor.as_deref(), Some("good"));
    assert!(!history.failed);
}

#[cfg(debug_assertions)]
#[test]
fn failed_terminal_notice_has_only_ten_nonempty_screen_lines() {
    use base64::Engine;
    use serde_json::json;
    let text = (0..20)
        .map(|n| format!("  visible line {n}  \r\n\r\n"))
        .collect::<String>();
    let (f, scratch, env) = terminal_fixture(
        ProviderKind::Claude,
        &[
            json!({"type":"emit_raw","base64":base64::engine::general_purpose::STANDARD.encode(text)}),
            json!({"type":"spin","ms":50}),
            json!({"type":"exit","code":1}),
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
        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let p = store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
                if let Some(message) = p.turns.values().find_map(|turn| match &turn.state {
                    TurnState::Ended {
                        outcome: TurnOutcome::Failed { message },
                        ..
                    } => Some(message.clone()),
                    _ => None,
                }) {
                    return message;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        engine.shutdown_wait().await;
        let message = ended.unwrap();
        assert_eq!(message.lines().count(), 10, "{message}");
        assert!(message.ends_with("visible line 19"), "{message}");
        assert!(!message.contains("visible line 9"), "{message}");
        assert!(
            message.lines().all(|line| line.trim_end() == line),
            "{message}"
        );
    });
}

#[cfg(debug_assertions)]
#[test]
fn terminal_actor_shuts_down_and_joins_on_panic() {
    use std::time::{Duration, Instant};
    let (f, _scratch, env) = terminal_fixture(ProviderKind::Grok, &[]);
    let groups = env.groups.clone();
    let (out, receiver) = mpsc::channel(256);
    let actor = Actor::new(f.actor.db, out.clone(), Spawner::Cli(Box::new(env))).unwrap();
    let state = actor.state.clone();
    let worker = ActorWorker::spawn(
        actor,
        receiver,
        f.runtime.handle().clone(),
        out.clone(),
        None,
    );
    let finished = worker.finished.clone();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _worker = worker;
        let (reply, result) = oneshot::channel();
        out.blocking_send(EngineMsg::Command {
            command: Command::SwitchView {
                thread: f.thread,
                to: View::Terminal,
            },
            reply,
        })
        .unwrap();
        result.blocking_recv().unwrap().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(handle) = state.terminals.lock().unwrap().get(&f.thread)
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
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!(
            "owned_actor_groups_before_panic={:?}",
            groups.lock().unwrap()
        );
        panic!("forced PTY assertion failure");
    }));
    assert!(failed.is_err());
    let cleaned_on_unwind =
        finished.load(std::sync::atomic::Ordering::Acquire) && groups.lock().unwrap().is_empty();
    let _ = out.blocking_send(EngineMsg::Shutdown);
    let until = Instant::now() + Duration::from_secs(5);
    while !finished.load(std::sync::atomic::Ordering::Acquire) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        finished.load(std::sync::atomic::Ordering::Acquire),
        "emergency actor cleanup failed"
    );
    assert!(groups.lock().unwrap().is_empty());
    assert!(
        cleaned_on_unwind,
        "panicking PTY test left its actor or process running"
    );
}

#[cfg(debug_assertions)]
struct EscapedHolder {
    path: std::path::PathBuf,
    groups: env::Groups,
    closing: std::sync::Arc<crate::LaunchGate>,
    stopped: std::cell::Cell<bool>,
}
#[cfg(debug_assertions)]
impl EscapedHolder {
    fn stop(&self) -> bool {
        if self.stopped.get() {
            return true;
        }
        let mut reaped = true;
        self.closing.close();
        for &pid in self.groups.lock().unwrap().iter() {
            let _ = crate::platform::signal_group(pid, crate::platform::Signal::Kill);
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(&self.path)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                break Some(pid);
            }
            if std::time::Instant::now() >= until {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        if let Some(pid) = pid {
            let _ = crate::platform::signal_group(pid, crate::platform::Signal::Kill);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while crate::platform::is_alive(pid).is_ok_and(|alive| alive)
                && std::time::Instant::now() < until
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            reaped = crate::platform::is_alive(pid).is_ok_and(|alive| !alive);
        }
        let cleaned = stop_stand_in_groups(&self.groups) && reaped;
        self.stopped.set(cleaned);
        cleaned
    }
}
#[cfg(debug_assertions)]
impl Drop for EscapedHolder {
    fn drop(&mut self) {
        if !self.stop() {
            eprintln!("escaped terminal fixture cleanup did not finish");
        }
    }
}

#[cfg(debug_assertions)]
fn escaped_pty_holder(closing: bool) {
    use serde_json::json;
    use std::time::{Duration, Instant};
    let (f, scratch, mut env) = terminal_fixture(
        ProviderKind::Claude,
        &[
            json!({"type":"expect_outbound","line":{"type":"control_request","request_id":"<id:init>","request\
                ":{"subtype":"initialize"}}}),
            json!({"type":"emit_inbound","line":{"type":"control_response","response":{"subtype":"success","req\
                uest_id":"<id:init>","response":{}}}}),
            json!({"type":"spin","ms":10000}),
        ],
    );
    let native = f.loaded().threads[&f.thread].native.clone().unwrap();
    let history = scratch.0.join("config/projects/fixture");
    std::fs::create_dir_all(&history).unwrap();
    std::fs::write(history.join(format!("{}.jsonl",native.0)), json!({"sessionId":native.0,"uuid":"anchor","par\
        entUuid":null,"isSidechain":false,"type":"user","entrypoint":"sdk-cli","message":{"content":"old\
            "}}).to_string()+"\n").unwrap();
    let script = scratch.0.join("terminal-holder-fixture");
    std::fs::write(&script, "#!/usr/bin/python3\nimport json,os,pathlib,signal,sys,time\nif '-p' in sys.argv:\
        \n    os.execv(os.environ['OWN_FAKE_BINARY'],[os.environ['OWN_FAKE_BINARY']]+sys.argv[1:])\nnative=sys.\
            argv[sys.argv.index('--resume')+1]\npath=pathlib.Path(os.environ['CLAUDE_CONFIG_DIR'])/'projects/fi\
                xture'/(native+'.jsonl')\nwith path.open('a') as out:\n    out.write(json.dumps({'sessionId':na\
                    tive,'uuid':'terminal-child-message','parentUuid':'anchor','isSidechain':False,'type':'user\
                        ','entrypoint':'cli','message':{'content':'typed before escaped child'}})+'\\n')\ngate=\
                            os.pipe()\nchild=os.fork()\nif child==0:\n    os.read(gate[0],1)\n    os.setsid()\
                                \n    signal.signal(signal.SIGHUP,signal.SIG_IGN)\n    print('VEYRA_TUI_READY',\
                                    flush=True)\n    while True: time.sleep(1)\npathlib.Path('holder.pid').writ\
                                        e_text(str(child))\nos.write(gate[1],b'1')\nwhile True: time.sleep(1)\n\
                                            ").unwrap();
    assert!(
        std::process::Command::new("/bin/chmod")
            .env_clear()
            .arg("700")
            .arg(&script)
            .status()
            .unwrap()
            .success()
    );
    env = env
        .with_binary(ProviderKind::Claude, script)
        .with_variable("OWN_FAKE_BINARY", fake_binary());
    std::fs::copy(scratch.0.join("t1-s1.jsonl"), scratch.0.join("t1-s2.jsonl")).unwrap();
    let cleanup = EscapedHolder {
        path: scratch.0.join("holder.pid"),
        groups: env.groups.clone(),
        closing: env.closing.clone(),
        stopped: std::cell::Cell::new(false),
    };
    env.terminal_controlling_tty = false;
    let thread = f.thread;
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
                thread,
                to: View::Terminal,
            })
            .await
            .unwrap()
            .unwrap();
        terminal_grid_containing(&engine, thread, "VEYRA_TUI_READY").await;
        eprintln!(
            "owned_escaped_holder_pid={}",
            std::fs::read_to_string(&cleanup.path).unwrap().trim()
        );
        let start = Instant::now();
        if closing {
            engine.on_app_quit().await;
            engine.shutdown_wait().await;
            let elapsed = start.elapsed();
            let cleaned = cleanup.stop();
            let p = store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
            assert!(
                cleaned,
                "escaped terminal fixture survived native quit cleanup"
            );
            assert!(
                elapsed < Duration::from_millis(1600),
                "native quit waited for terminal reader: {elapsed:?}"
            );
            assert!(p.threads[&thread].import_pending);
            assert_eq!(p.threads[&thread].view, View::Chat);
            assert!(!p.turns.values().any(|turn| matches!(
                turn.state,
                TurnState::Ended {
                    outcome: TurnOutcome::Failed { .. },
                    ..
                }
            )));
            return;
        }
        engine
            .send(Command::SwitchView {
                thread,
                to: View::Chat,
            })
            .await
            .unwrap()
            .unwrap();
        let imported = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let p = store::load(&Connection::open(scratch.0.join("test.db")).unwrap()).unwrap();
                if !p.threads[&thread].import_pending
                    && p.items.values().any(|item| {
                        matches!(&item.body,
                    ItemBody::TerminalSpan(span) if span.text == "typed before escaped child")
                    })
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        let elapsed = start.elapsed();
        eprintln!("terminal_exit_import_elapsed_ms={}", elapsed.as_millis());
        let cleaned = cleanup.stop();
        engine.shutdown_wait().await;
        assert!(cleaned, "escaped terminal fixture survived cleanup");
        assert!(
            imported.is_ok(),
            "escaped child kept Exited and import waiting beyond the ladder"
        );
        assert!(
            elapsed < Duration::from_millis(3600),
            "switch back took {elapsed:?}"
        );
    });
}

#[cfg(debug_assertions)]
#[test]
fn terminal_exit_is_bounded_with_an_escaped_pty_holder() {
    escaped_pty_holder(false);
}
#[cfg(debug_assertions)]
#[test]
fn native_quit_bounds_a_terminal_reader_held_by_a_child() {
    escaped_pty_holder(true);
}

fn terminal_test_snapshot(f: &mut Fixture) -> crate::events::ThreadSnapshot {
    let (reply, result) = oneshot::channel();
    {
        let _entered = f.runtime.enter();
        f.actor.handle(EngineMsg::Snapshot {
            thread: f.thread,
            reply,
        });
    }
    result.blocking_recv().unwrap()
}

fn dispatched_cursor(f: &mut Fixture, turn: TurnId, cursor: CursorUpdate) {
    let _entered = f.runtime.enter();
    f.actor.handle(EngineMsg::Provider {
        thread: f.thread,
        epoch: f.actor.live[&f.thread].epoch,
        event: ProviderEvent::Session(SessionEvent::Dispatched { turn }),
        cursor: Some(cursor),
    });
}

#[test]
fn claude_history_tolerates_optional_field_and_block_variants() {
    use serde_json::json;
    let scratch = Scratch(
        std::path::Path::new("/tmp").join(format!("veyra-term-optional-{}", ThreadId::new())),
    );
    std::fs::create_dir_all(scratch.0.join("projects/fixture")).unwrap();
    let entry = |id: &str, kind: &str, text: &str| {
        json!({"sessionId":"optional-native","uuid":id,"parentUuid\
        ":null,"isSidechain":false,"type":kind,"entrypoint":"cli","message":{"content":[{"type":"text","text":text}]}})
    };
    let mut variants = Vec::new();
    for field in ["turnOrigin", "entrypoint", "sessionId", "isSidechain"] {
        for value in [
            json!(null),
            json!(17),
            json!([]),
            json!({}),
            json!("future-value"),
        ] {
            if field == "sessionId" && value.is_string()
                || field == "isSidechain" && value.is_boolean()
            {
                continue;
            }
            let mut user = entry("typed", "user", "typed prompt");
            user[field] = value;
            variants.push(user);
        }
    }
    let mut no_parent = entry("typed", "user", "typed prompt");
    no_parent.as_object_mut().unwrap().remove("parentUuid");
    variants.push(no_parent);
    for block in [
        json!(17),
        json!(null),
        json!([]),
        json!({}),
        json!({"type":17}),
        json!({"type":"future","text":17}),
    ] {
        let mut user = entry("typed", "user", "typed prompt");
        user["message"]["content"]
            .as_array_mut()
            .unwrap()
            .push(block);
        variants.push(user);
    }
    for user in variants {
        let path = scratch.0.join("projects/fixture/optional-native.jsonl");
        std::fs::write(
            path,
            [
                entry("anchor", "user", "old"),
                user.clone(),
                entry("reply", "assistant", "typed reply"),
            ]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        )
        .unwrap();
        let history = claude::session_file_import(
            &scratch.0,
            &NativeSessionId("optional-native".into()),
            &HistoryRequest {
                cursor: Some("anchor".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            history.text, "typed prompt\ntyped reply",
            "optional input: {user}"
        );
        assert_eq!(history.cursor.as_deref(), Some("typed"));
        assert!(!history.failed);
    }
}

#[test]
fn imported_spans_keep_display_text_for_every_provider() {
    use serde_json::json;
    for provider in ProviderKind::ALL {
        let mut f = Fixture::new(LiveState::Idle);
        let mut row = f.actor.projection.threads[&f.thread].clone();
        row.provider = provider;
        row.native = Some(NativeSessionId("display-native".into()));
        row.native_cursor = Some("anchor".into());
        row.import_pending = true;
        let mut changes = ChangeSet::default();
        changes.threads.insert(f.thread, row);
        f.save(changes);
        let fixture_line =
            json!({"text":"terminal\u{1b}\u{7}\u{0}\u{1f}\u{7f}\n\tC1: \u{85} é 🦢"}).to_string();
        let line: serde_json::Value = serde_json::from_str(&fixture_line).unwrap();
        f.event(ProviderEvent::Session(SessionEvent::History {
            native: NativeSessionId("display-native".into()),
            history: History {
                cursor: Some("typed".into()),
                text: line["text"].as_str().unwrap().into(),
                failed: false,
            },
        }));
        let imported: Vec<_> = f
            .loaded()
            .items
            .values()
            .filter_map(|item| match &item.body {
                ItemBody::TerminalSpan(span) => Some(span.text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(imported, ["terminal\n\tC1: \u{85} é 🦢"], "{provider:?}");
    }
}

#[cfg(debug_assertions)]
#[test]
fn dispatch_cursor_commits_before_prompt_delivery() {
    use std::time::Duration;
    let mut f = Fixture::new(LiveState::Idle);
    let scratch = Scratch(
        std::path::Path::new("/tmp").join(format!("veyra-term-dispatch-{}", ThreadId::new())),
    );
    std::fs::create_dir(&scratch.0).unwrap();
    let binary = scratch.0.join("owned-cli");
    std::fs::write(
        &binary,
        "#!/bin/sh\nread initialize\nprintf ready > initialized\nread prompt\nprintf '%s' \
        \"$prompt\" > delivered\nwhile read ignored; do :; done\n",
    )
    .unwrap();
    assert!(
        std::process::Command::new("/bin/chmod")
            .args(["700"])
            .arg(&binary)
            .status()
            .unwrap()
            .success()
    );
    let env = env::ChildEnv::isolated().with_binary(ProviderKind::Claude, binary);
    let groups = env.groups.clone();
    let turn = f.user("dispatch payload");
    let mut c = ChangeSet::default();
    let mut row = f.actor.projection.threads[&f.thread].clone();
    row.native_cursor = Some("previous-server-turn".into());
    c.threads.insert(f.thread, row);
    f.save(c);
    let (out, mut events) = mpsc::channel(256);
    let handle = {
        let _entered = f.runtime.enter();
        session::spawn(
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
        .unwrap()
    };
    let mut c = ChangeSet::default();
    let mut live = f.actor.live[&f.thread].clone();
    live.live = Live::Structured {
        session: handle.clone(),
        pid: 1,
        pgid: 1,
        draining: None,
    };
    c.live.insert(f.thread, live);
    f.save(c);
    let runtime = f.runtime.handle().clone();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
        handle.control.send(SessionControl::StartTurn {turn,text:"dispatch payload".into()}).unwrap();
        let message = tokio::time::timeout(Duration::from_secs(3), events.recv()).await.unwrap().unwrap();
        assert!(matches!(&message, EngineMsg::Provider {event:ProviderEvent::Session(SessionEvent::Dispatched {
            turn:owner}),..} if *owner==turn));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !scratch.0.join("initialized").exists() {tokio::time::sleep(Duration::from_millis(5)).await;}
        }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let delivered_before_commit = scratch.0.join("delivered").exists();
        f.actor.handle(message);
        assert_eq!(f.loaded().threads[&f.thread].native_cursor.as_deref(), Some(turn.to_string().as_str()));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !scratch.0.join("delivered").exists() {tokio::time::sleep(Duration::from_millis(5)).await;}
        }).await.unwrap();
        let delivered:serde_json::Value = serde_json::from_str(&std::fs::read_to_string(scratch.0.join("deliver\
            ed")).unwrap()).unwrap();
        assert_eq!(delivered["message"]["content"], "dispatch payload");
        assert!(!delivered_before_commit, "fake CLI received the prompt before the actor committed its dispatc\
            h cursor");
    })
    }));
    let _ = handle.control.send(SessionControl::Shutdown);
    let cleanup = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !groups.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    });
    assert!(cleanup.is_ok(), "owned dispatch fixture did not reap");
    outcome.unwrap();
}
