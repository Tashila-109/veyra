use super::*;
use crate::Engine;
use std::time::Instant;

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
