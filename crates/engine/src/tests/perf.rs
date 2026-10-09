use super::*;
use crate::Engine;
use std::time::Instant;

#[test]
#[ignore]
fn perf_decide_commit() {
    let path =
        std::path::PathBuf::from(format!("/tmp/veyra-perf-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir(&path).unwrap();
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
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
