use super::*;
use crate::Engine;
use std::time::Instant;

#[test]
fn claude_stream_keeps_seven_fixed_commits() {
    let mut f = Fixture::new(LiveState::Starting);
    let before = f.actor.test_commits;
    let Ack::Turn(turn) = f
        .command(Command::Send {
            thread: f.thread,
            text: "commit anatomy".into(),
        })
        .unwrap()
    else {
        panic!("missing turn")
    };
    let mut codec = claude::Codec::new(
        NativeSessionId(f.thread.to_string()),
        9,
        std::path::Path::new("/tmp"),
    );
    let mut matcher = crate::fixture::Matcher::default();
    for (name, value) in [
        ("<id:initialize>", claude::INITIALIZE_ID.to_owned()),
        ("<id:prompt>", turn.to_string()),
        ("<id:session>", f.thread.to_string()),
        ("<id:message>", "stream-message".into()),
    ] {
        matcher.bind(name, value.into());
    }
    let records = crate::fixture::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude/stream-2000.jsonl"),
    )
    .unwrap();
    let mut deltas = 0_usize;
    let mut text_writes = 0;
    let mut before_end = None;
    for record in records {
        let mut decoded = match record {
            crate::fixture::Record::ExpectOutbound { line } if line["type"] == "user" => {
                assert_eq!(
                    f.controls.try_recv().unwrap(),
                    SessionControl::StartTurn {
                        turn,
                        text: "commit anatomy".into()
                    }
                );
                codec.start(turn, "commit anatomy".into()).unwrap()
            }
            crate::fixture::Record::EmitInbound { line, .. } => {
                codec.value(matcher.expand(line)).unwrap()
            }
            crate::fixture::Record::Exit { code: 0, .. } => Decoded {
                events: vec![ProviderEvent::Session(SessionEvent::Exited { crash: None })],
                ..Default::default()
            },
            _ => continue,
        };
        for event in decoded.events {
            let text = matches!(
                &event,
                ProviderEvent::Turn {
                    event: TurnEvent::TextDelta { .. },
                    ..
                }
            );
            let dispatched = matches!(
                &event,
                ProviderEvent::Session(SessionEvent::Dispatched { .. })
            );
            let commits = f.actor.test_commits;
            {
                let _entered = f.runtime.enter();
                f.actor.handle(EngineMsg::Provider {
                    thread: f.thread,
                    epoch: 1,
                    cursor: decoded.cursor.take(),
                    event,
                });
            }
            if dispatched {
                assert_eq!(
                    f.loaded().threads[&f.thread].native_cursor.as_deref(),
                    Some(turn.to_string().as_str())
                );
                assert_eq!(
                    f.controls.try_recv().unwrap(),
                    SessionControl::DispatchCommitted { turn }
                );
            }
            if text {
                deltas += 1;
                if deltas == 1 {
                    println!("startup commits={}", f.actor.test_commits - before);
                    assert_eq!(f.actor.test_commits - before, 4);
                    assert_eq!(
                        f.loaded().threads[&f.thread].cli_version.as_deref(),
                        Some("2.1.294")
                    );
                } else {
                    assert_eq!(f.actor.test_commits, commits, "buffer-only delta committed");
                }
                if deltas.is_multiple_of(400) {
                    let commits = f.actor.test_commits;
                    let _entered = f.runtime.enter();
                    f.actor.handle(EngineMsg::TextWrite);
                    text_writes += f.actor.test_commits - commits;
                }
                before_end = Some(f.actor.test_commits);
            }
        }
    }
    assert_eq!(deltas, 2000);
    assert_eq!(text_writes, 5);
    assert_eq!(f.actor.test_commits - before_end.unwrap(), 3);
    assert_eq!(f.actor.test_commits - before - text_writes, 7);
    let stored = f.loaded();
    assert_eq!(
        stored.threads[&f.thread].cli_version.as_deref(),
        Some("2.1.294")
    );
    assert!(matches!(
        stored.turns[&turn].state,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            ..
        }
    ));
    assert!(stored.items_for(f.thread).any(|item| matches!(&item.body, ItemBody::AssistantText { text, done: true } if text == &"x".repeat(2000))));
    println!(
        "startup=4 ending=3 text_writes=5 total={}",
        f.actor.test_commits - before
    );
}

#[test]
fn stream_pause_is_one_300ms_delay_after_1000_deltas() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude");
    let regular = crate::fixture::load(&root.join("stream-2000.jsonl")).unwrap();
    let paused = crate::fixture::load(&root.join("stream-pause.jsonl")).unwrap();
    assert_eq!(paused.len(), regular.len());
    let mut deltas = 0;
    let mut changes = vec![];
    for (index, (regular, paused)) in regular.iter().zip(&paused).enumerate() {
        let original = serde_json::to_value(regular).unwrap();
        let mut actual = serde_json::to_value(paused).unwrap();
        if actual["line"]["event"]["delta"]["type"] == "text_delta" {
            if actual["after_ms"] != original["after_ms"] {
                changes.push((deltas, actual["after_ms"].clone()));
                actual["after_ms"] = original["after_ms"].clone();
            }
            deltas += 1;
        }
        assert_eq!(actual, original, "fixture record {index} changed");
    }
    assert_eq!(deltas, 2000);
    assert_eq!(changes, [(1000, serde_json::json!(300))]);
}

fn seed_large_snapshot(db: &mut Connection, path: &std::path::Path) -> ThreadId {
    let project = ProjectId::new();
    let thread = ThreadId::new();
    let mut changes = ChangeSet::default();
    changes.projects.insert(
        project,
        Project {
            id: project,
            path: path.into(),
            name: "Snapshot project".into(),
        },
    );
    changes.threads.insert(
        thread,
        Thread {
            id: thread,
            project,
            title: "Five thousand transcript items".into(),
            lineage: Lineage::Root,
            provider: ProviderKind::Claude,
            config: config(),
            cwd: path.into(),
            native: None,
            native_cursor: None,
            cli_version: None,
            hold: None,
            view: View::Chat,
            import_pending: false,
            archived: false,
        },
    );
    for ordinal in 1..=50 {
        let turn = TurnId::new();
        let user = ItemId::new();
        changes.turns.insert(
            turn,
            Turn {
                id: turn,
                thread,
                ordinal,
                origin: TurnOrigin::User { item: user },
                state: TurnState::Ended {
                    started: Some(ordinal as i64 * 1000),
                    at: ordinal as i64 * 1000 + 500,
                    outcome: TurnOutcome::Completed,
                },
            },
        );
        for seq in 0..100 {
            let id = if seq == 0 { user } else { ItemId::new() };
            let body = match seq {
                0 => ItemBody::UserText(format!("Inspect batch {ordinal}")),
                99 => ItemBody::AssistantText {
                    text: format!("Batch {ordinal} complete"),
                    done: true,
                },
                _ => ItemBody::Tool(ToolItem {
                    call: ToolCall {
                        veyra_label: false,
                        name: "Read".into(),
                        arguments: serde_json::json!({"file_path":format!("batch-{ordinal}/item-{seq}.rs")}),
                        input_summary: None,
                    },
                    result: Some(ToolResult {
                        ok: true,
                        output_tail: Some(format!("Stored output for item {seq}")),
                    }),
                    output_tail: String::new(),
                }),
            };
            changes.items.insert(
                id,
                Item {
                    id,
                    thread,
                    turn,
                    seq: (ordinal - 1) * 100 + seq,
                    body,
                },
            );
        }
    }
    let tx = db.transaction().unwrap();
    store::apply(&tx, &changes, now()).unwrap();
    tx.commit().unwrap();
    thread
}

#[test]
fn large_snapshot_seed_keeps_fifty_complete_tool_heavy_turns() {
    let mut db = Connection::open_in_memory().unwrap();
    store::initialize(&mut db).unwrap();
    let thread = seed_large_snapshot(&mut db, std::path::Path::new("/tmp"));
    let rows = store::load(&db).unwrap();
    assert_eq!(rows.threads.len(), 1);
    assert_eq!(rows.turns.len(), 50);
    assert_eq!(rows.items.len(), 5000);
    assert_eq!(
        rows.items_for(thread)
            .filter(|item| matches!(item.body, ItemBody::Tool(_)))
            .count(),
        4900
    );
    for turn in rows.turns.values() {
        assert!(matches!(
            turn.state,
            TurnState::Ended {
                outcome: TurnOutcome::Completed,
                ..
            }
        ));
        assert_eq!(
            rows.items_for(thread)
                .filter(|item| item.turn == turn.id)
                .count(),
            100
        );
    }
}

#[test]
#[ignore]
fn perf_snapshot_large() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("perf-snapshot-{}", ThreadId::new()));
    std::fs::create_dir_all(&path).unwrap();
    let path = path.canonicalize().unwrap();
    let mut db = store::open(&path.join("veyra.db")).unwrap();
    let thread = seed_large_snapshot(&mut db, &path);
    let runtime = Runtime::new().unwrap();
    let (out, _messages) = mpsc::channel(256);
    let mut actor = Actor::new(db, out, Spawner::Scripted(Scripted::default())).unwrap();
    let (reply, snapshot) = oneshot::channel();
    let start = Instant::now();
    {
        let _enter = runtime.enter();
        actor.handle(EngineMsg::Snapshot { thread, reply });
    }
    let actor_ms = start.elapsed().as_secs_f64() * 1000.;
    assert_eq!(snapshot.blocking_recv().unwrap().items.len(), 5000);
    actor
        .db
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    println!(
        "perf_snapshot_large items=5000 turns=50 actor_ms={actor_ms:.3} thread={thread} database={}",
        path.join("veyra.db").display()
    );
}

#[test]
#[ignore]
fn perf_decide_commit() {
    let path =
        std::path::PathBuf::from(format!("/tmp/veyra-perf-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir(&path).unwrap();
    let _scratch = Scratch(path.clone());
    let db_path = path.join("veyra.db");
    let db = store::open(&db_path).unwrap();
    let runtime = Runtime::new().unwrap();
    let script = Scripted {
        auto_ready: true,
        auto_complete: true,
        ..Scripted::default()
    };
    let timings = script.timings.clone();
    let engine = Engine::scripted(runtime.handle(), db, script).unwrap();
    let total = Instant::now();
    runtime.block_on(async {
        let Ack::Project(project) = engine
            .send(Command::AddProject { path: path.clone() })
            .await
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        let Ack::Thread(thread) = engine
            .send(Command::CreateThread {
                project,
                provider: ProviderKind::Claude,
                config: config(),
                text: "first prompt".into(),
            })
            .await
            .unwrap()
            .unwrap()
        else {
            panic!()
        };
        for n in 0..10_000 {
            assert!(matches!(
                engine
                    .send(Command::Send {
                        thread,
                        text: format!("Prompt {n}")
                    })
                    .await
                    .unwrap()
                    .unwrap(),
                Ack::Turn(_)
            ));
        }
        let snapshot = engine.snapshot(thread).await.unwrap();
        assert_eq!(snapshot.items.len(), 10_001);
    });
    let samples = timings.lock().unwrap().clone();
    assert_eq!(samples.len(), 10_000);
    let db = Connection::open(&db_path).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM turns", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        10_001
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM items WHERE kind='user_text'",
            [],
            |r| r.get::<_, u32>(0)
        )
        .unwrap(),
        10_001
    );
    assert_eq!(
        db.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    let raw = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/perf-decide-commit-ns.txt");
    std::fs::write(
        &raw,
        samples.iter().map(|n| format!("{n}\n")).collect::<String>(),
    )
    .unwrap();
    let mut sorted = samples;
    sorted.sort_unstable();
    println!(
        concat!(
            "perf_decide_commit samples=10000 successful=10000 errors=0 user_items=10001 ",
            "p50={:.3} us p99={:.3} us end_to_end={:.3} s raw={}"
        ),
        sorted[4999] as f64 / 1000.0,
        sorted[9899] as f64 / 1000.0,
        total.elapsed().as_secs_f64(),
        raw.display()
    );
    drop(db);
    drop(engine);
    drop(runtime);
}
