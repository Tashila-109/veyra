#![allow(clippy::disallowed_types)] // Shell tests construct fake terminal wire records.
use super::*;
use crate::{
    Engine,
    data_dir::DataDir,
    provider::TerminalHandle,
    terminal::{PtySize, ShellStatus},
};
use std::time::{Duration, Instant};

struct ShellRun {
    engine: Engine,
    env: env::ChildEnv,
    scratch: Scratch,
    runtime: Runtime,
    thread: ThreadId,
}
impl ShellRun {
    fn new(script: Option<&str>) -> Self {
        let root = std::path::Path::new("/tmp").join(format!("veyra-shell-{}", ThreadId::new()));
        std::fs::create_dir_all(root.join("project")).unwrap();
        let scratch = Scratch(root.clone());
        let shell = root.join("shell");
        let binary = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("veyra-fake-cli");
        let script = script.map(str::to_owned).unwrap_or_else(|| {
            format!(
                "printf '%s\\n%s' \"$1\" \"$PWD\" > launch\nexec '{}' shell\n",
                binary.display()
            )
        });
        std::fs::write(&shell, format!("#!/bin/sh\n{script}")).unwrap();
        assert!(
            std::process::Command::new("/bin/chmod")
                .arg("700")
                .arg(&shell)
                .status()
                .unwrap()
                .success()
        );
        let env = env::ChildEnv::isolated().with_variable("SHELL", shell.as_os_str());
        let runtime = Runtime::new().unwrap();
        let engine = Engine::start_with_env(
            runtime.handle(),
            DataDir::at(root.join("data")).unwrap(),
            env.clone(),
        )
        .unwrap();
        Self {
            engine,
            env,
            scratch,
            runtime,
            thread: ThreadId::new(),
        }
    }
    fn open(&self) -> TerminalHandle {
        let handle = self
            .engine
            .open_shell(
                self.thread,
                self.scratch.0.join("project"),
                PtySize::default(),
            )
            .unwrap();
        self.wait(|| !matches!(handle.status(), ShellStatus::Starting));
        handle
    }
    fn wait(&self, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !ready() {
            assert!(Instant::now() < deadline, "shell condition timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn text(&self, handle: &TerminalHandle, text: &str) {
        self.wait(|| handle.emulator().lock().text().join("\n").contains(text));
    }
}
impl Drop for ShellRun {
    fn drop(&mut self) {
        self.runtime.block_on(self.engine.shutdown_wait());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.env.groups.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        for pid in self.env.groups.lock().unwrap().iter().copied() {
            let _ = crate::platform::signal_group(pid, crate::platform::Signal::Kill);
        }
        assert!(
            self.env.groups.lock().unwrap().is_empty(),
            "shell cleanup left an owned group"
        );
    }
}

#[test]
fn shell_starts_login_shell_in_thread_folder() {
    let run = ShellRun::new(None);
    let handle = run.open();
    run.text(&handle, "VEYRA_TUI_READY");
    assert_eq!(
        std::fs::read_to_string(run.scratch.0.join("project/launch")).unwrap(),
        format!(
            "-l\n{}",
            run.scratch
                .0
                .join("project")
                .canonicalize()
                .unwrap()
                .display()
        )
    );
    assert!(!run.engine.awake());
}
#[test]
fn shell_env_strips_veyra_and_host_terminal() {
    let env = env::ChildEnv::isolated();
    let variables = env.shell_environment([
        ("VEYRA_MCP_TOKEN".into(), "private".into()),
        ("VEYRA_PARENT_PID".into(), "1".into()),
        ("TERM_PROGRAM".into(), "host".into()),
        ("KITTY_WINDOW_ID".into(), "host".into()),
        ("CLAUDECODE".into(), "1".into()),
        ("PATH".into(), "/original-path".into()),
        ("KEEP".into(), "kept".into()),
    ]);
    assert!(
        variables
            .keys()
            .all(|key| !key.to_string_lossy().starts_with("VEYRA_"))
    );
    for key in ["TERM_PROGRAM", "KITTY_WINDOW_ID", "CLAUDECODE"] {
        assert!(!variables.contains_key(std::ffi::OsStr::new(key)));
    }
    assert_eq!(variables[std::ffi::OsStr::new("PATH")], "/original-path");
    assert_eq!(variables[std::ffi::OsStr::new("TERM")], "xterm-256color");
    assert_eq!(variables[std::ffi::OsStr::new("COLORTERM")], "truecolor");
    assert_eq!(variables[std::ffi::OsStr::new("KEEP")], "kept");
}
#[test]
fn shell_close_asks_with_foreground_or_background_job() {
    for script in [
        "/bin/sleep 60 &\nprintf ready\\n\nwait\n",
        "/bin/sleep 60\nwait\n",
    ] {
        let run = ShellRun::new(Some(script));
        let handle = run.open();
        run.wait(|| {
            handle
                .running_processes()
                .iter()
                .any(|path| path.file_name().is_some_and(|name| name == "sleep"))
        });
        assert!(!handle.running_processes().is_empty());
        run.engine.close_shells(run.thread);
        run.wait(|| handle.status().is_finished());
    }
    let run = ShellRun::new(None);
    let handle = run.open();
    run.text(&handle, "VEYRA_TUI_READY");
    assert!(handle.running_processes().is_empty());
}
#[test]
fn shell_close_kills_group_after_grace() {
    let run = ShellRun::new(Some(
        "trap '' HUP\nprintf ready\\n\nwhile :; do /bin/sleep 1; done\n",
    ));
    let handle = run.open();
    run.text(&handle, "ready");
    let pid = handle.pid().unwrap();
    let start = Instant::now();
    run.engine.close_shells(run.thread);
    run.wait(|| handle.status().is_finished());
    assert!(start.elapsed() >= Duration::from_secs(3));
    assert!(start.elapsed() < Duration::from_secs(5));
    let deadline = Instant::now() + Duration::from_secs(1);
    while crate::platform::group_exists(pid).unwrap() {
        assert!(
            Instant::now() < deadline,
            "owned shell group outlived cleanup"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn shell_tab_limit_is_enforced_by_engine() {
    let run = ShellRun::new(None);
    let tabs: Vec<_> = (0..crate::terminal::SHELL_TAB_LIMIT)
        .map(|_| run.open())
        .collect();
    assert!(
        run.engine
            .open_shell(
                run.thread,
                run.scratch.0.join("project"),
                PtySize::default()
            )
            .is_err()
    );
    let other = run.engine.open_shell(
        ThreadId::new(),
        run.scratch.0.join("project"),
        PtySize::default(),
    );
    assert!(other.is_ok());
    tabs[0].close();
    assert!(
        run.engine
            .open_shell(
                run.thread,
                run.scratch.0.join("project"),
                PtySize::default()
            )
            .is_ok()
    );
}

#[test]
fn shell_foreground_job_is_killed_after_grace() {
    struct Job(std::path::PathBuf);
    impl Drop for Job {
        fn drop(&mut self) {
            if let Ok(pid) = std::fs::read_to_string(&self.0) {
                let _ = crate::platform::signal_group(
                    pid.trim().parse().unwrap(),
                    crate::platform::Signal::Kill,
                );
            }
        }
    }
    let run = ShellRun::new(Some(
        "set -m\n/bin/sh -c 'trap \"\" HUP; echo $$ > foreground; exec /bin/sleep 60'\n",
    ));
    let job = Job(run.scratch.0.join("project/foreground"));
    let handle = run.open();
    run.wait(|| {
        job.0.is_file()
            && handle
                .running_processes()
                .iter()
                .any(|path| path.file_name().is_some_and(|name| name == "sleep"))
    });
    let group: u32 = std::fs::read_to_string(&job.0)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(group, handle.pid().unwrap());
    let started = Instant::now();
    run.engine.close_shells(run.thread);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        crate::platform::group_exists(group).unwrap(),
        "foreground job did not ignore HUP"
    );
    run.wait(|| !crate::platform::group_exists(group).unwrap());
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert!(started.elapsed() < Duration::from_secs(5));
    run.wait(|| handle.status().is_finished());
}

#[test]
fn shell_missing_folder_starts_nothing() {
    let run = ShellRun::new(None);
    assert!(
        run.engine
            .open_shell(
                run.thread,
                run.scratch.0.join("missing"),
                PtySize::default()
            )
            .is_err()
    );
    assert!(run.env.groups.lock().unwrap().is_empty());
}
#[test]
fn close_shells_ends_every_tab_of_thread() {
    let run = ShellRun::new(None);
    let one = run.open();
    let two = run.open();
    let other = run
        .engine
        .open_shell(
            ThreadId::new(),
            run.scratch.0.join("project"),
            PtySize::default(),
        )
        .unwrap();
    run.wait(|| matches!(other.status(), ShellStatus::Running));
    run.engine.close_shells(run.thread);
    run.wait(|| {
        [one.status(), two.status()]
            .iter()
            .all(|status| status.is_finished())
    });
    assert!(matches!(other.status(), ShellStatus::Running));
}
#[test]
fn quit_hangs_up_every_shell() {
    let run = ShellRun::new(Some(
        "trap 'exit 0' HUP\nprintf ready\\n\nwhile :; do /bin/sleep 1; done\n",
    ));
    let one = run.open();
    let two = run.open();
    run.text(&one, "ready");
    run.text(&two, "ready");
    let start = Instant::now();
    run.runtime.block_on(run.engine.on_app_quit());
    assert!(start.elapsed() < Duration::from_millis(200));
    run.wait(|| {
        [one.status(), two.status()]
            .iter()
            .all(|status| status.is_finished())
    });
    assert!(start.elapsed() < Duration::from_secs(3));
}
#[test]
fn stored_terminal_view_opens_in_chat() {
    let mut f = Fixture::new(LiveState::Parked);
    let turn = f.turn(
        TurnOrigin::Provider,
        TurnState::Ended {
            outcome: TurnOutcome::Completed,
            started: Some(1),
            at: 2,
        },
    );
    let mut changes = ChangeSet::default();
    let item = Item {
        id: ItemId::new(),
        thread: f.thread,
        turn,
        seq: 1,
        body: ItemBody::TerminalSpan(TerminalSpan {
            text: "stored terminal text".into(),
        }),
    };
    changes.items.insert(item.id, item.clone());
    f.save(changes);
    f.actor
        .db
        .execute(store::LEGACY_TERMINAL_ROW, [f.thread.to_string()])
        .unwrap();
    let db = std::mem::replace(&mut f.actor.db, Connection::open_in_memory().unwrap());
    let (sender, _) = mpsc::channel(256);
    let mut actor = Actor::new(db, sender, Spawner::Scripted(Scripted::default())).unwrap();
    assert_eq!(actor.projection.threads[&f.thread].hold, None);
    assert!(matches!(actor.live[&f.thread].live, Live::Parked));
    assert_eq!(actor.projection.items[&item.id], item);
    let row: (String, bool, Option<String>) = actor
        .db
        .query_row(
            store::LEGACY_TERMINAL_COLUMNS,
            [f.thread.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("terminal".into(), true, Some("old".into())));
    let mut changes = ChangeSet::default();
    changes
        .threads
        .insert(f.thread, actor.projection.threads[&f.thread].clone());
    let tx = actor.db.transaction().unwrap();
    store::apply(&tx, &changes, now()).unwrap();
    tx.commit().unwrap();
    let row: (String, bool, Option<String>) = actor
        .db
        .query_row(
            store::LEGACY_TERMINAL_COLUMNS,
            [f.thread.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("chat".into(), false, None));
}

impl ShellRun {
    fn fixture(&self, records: &[serde_json::Value]) {
        let fixture = self.scratch.0.join("shell.jsonl");
        std::fs::write(
            &fixture,
            records
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let binary = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("veyra-fake-cli");
        std::fs::write(
            self.scratch.0.join("shell"),
            format!(
                "#!/bin/sh\nexport VEYRA_FAKE_FIXTURE='{}'\nexec '{}' shell\n",
                fixture.display(),
                binary.display()
            ),
        )
        .unwrap();
    }
}
fn raw(bytes: &[u8]) -> serde_json::Value {
    use base64::Engine;
    serde_json::json!({"type":"emit_raw","base64":base64::engine::general_purpose::STANDARD.encode(bytes)})
}
#[test]
fn shell_stalled_sync_flushes_without_more_bytes() {
    let run = ShellRun::new(None);
    run.fixture(&[
        raw(b"\x1b[?2026hSYNC_STALLED\r\n"),
        serde_json::json!({"type":"spin","ms":3000}),
    ]);
    let handle = run.open();
    let started = Instant::now();
    run.text(&handle, "SYNC_STALLED");
    assert!(started.elapsed() < Duration::from_secs(3));
}
#[test]
fn large_paste_keeps_reader_moving() {
    let run = ShellRun::new(None);
    run.fixture(&[
        raw(b"PASTE_READY\r\n"),
        serde_json::json!({"type":"spin","ms":300}),
        raw(b"\x1b[6n"),
        serde_json::json!({"type":"spin","ms":100}),
        raw(b"GRID_MOVED\r\n"),
        serde_json::json!({"type":"spin","ms":2000}),
    ]);
    let handle = run.open();
    run.text(&handle, "PASTE_READY");
    let started = Instant::now();
    handle.write(&vec![b'x'; 8 * 1024 * 1024]).unwrap();
    assert!(started.elapsed() < Duration::from_millis(200));
    run.text(&handle, "GRID_MOVED");
}
#[test]
fn worker_panic_still_exits() {
    let run = ShellRun::new(None);
    let gate = run.env.closing.clone();
    let _ = std::thread::spawn(move || {
        let _held = gate.lock.lock().unwrap();
        panic!("owned launch gate poison");
    })
    .join();
    let handle = run.open();
    gate_recover(&run.env.closing);
    assert!(matches!(handle.status(), ShellStatus::Failed { .. }));
    assert!(run.env.groups.lock().unwrap().is_empty());
}
fn gate_recover(gate: &crate::LaunchGate) {
    gate.lock.clear_poison();
}
#[test]
fn closing_shell_uses_one_second_ladder() {
    let run = ShellRun::new(Some(
        "trap '' HUP\nprintf ready\\n\nwhile :; do /bin/sleep 1; done\n",
    ));
    let handle = run.open();
    run.text(&handle, "ready");
    let started = Instant::now();
    run.runtime.block_on(run.engine.shutdown_wait());
    assert!(handle.status().is_finished());
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(2));
}
#[test]
fn hangup_ignoring_stand_in_is_reaped_after_assertion_failure() {
    let mut pid = None;
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let run = ShellRun::new(Some(
            "trap '' HUP\nprintf ready\\n\nwhile :; do /bin/sleep 1; done\n",
        ));
        let handle = run.open();
        run.text(&handle, "ready");
        pid = handle.pid();
        panic!("intentional stand-in assertion failure");
    }));
    assert!(failed.is_err());
    assert!(pid.is_some_and(|pid| !crate::platform::group_exists(pid).unwrap()));
}
#[test]
fn terminal_input_skips_actor_mailbox() {
    let run = ShellRun::new(None);
    let handle = run.open();
    run.text(&handle, "VEYRA_TUI_READY");
    let db = Connection::open(run.scratch.0.join("data/veyra.db")).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let engine = run.engine.clone();
    let folder = run.scratch.0.join("project");
    let command = run
        .runtime
        .spawn(async move { engine.send(Command::AddProject { path: folder }).await });
    #[cfg(debug_assertions)]
    run.wait(|| run.engine.perf().actor_busy_ms > 30.);
    let started = Instant::now();
    handle.write(b"K").unwrap();
    handle
        .resize(PtySize {
            cols: 96,
            rows: 30,
            ..Default::default()
        })
        .unwrap();
    run.text(&handle, "4b");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(handle.emulator().lock().text().len(), 30);
    db.execute_batch("COMMIT").unwrap();
    run.runtime.block_on(command).unwrap().unwrap().unwrap();
}

#[test]
fn worker_setup_error_before_handle_is_visible() {
    let run = ShellRun::new(None);
    std::fs::remove_file(run.scratch.0.join("shell")).unwrap();
    let handle = run.open();
    assert!(matches!(handle.status(), ShellStatus::Failed { .. }));
    assert_eq!(handle.pid(), None);
}
#[test]
fn failed_shell_keeps_exit_code_and_visible_reason() {
    let run = ShellRun::new(None);
    run.fixture(&[serde_json::json!({"type":"exit","code":7,"stderr":["Shell fixture ended"]})]);
    let handle = run.open();
    run.wait(|| handle.status().is_finished());
    assert!(matches!(handle.status(), ShellStatus::Exited { code: 7 }));
    assert!(
        handle
            .emulator()
            .lock()
            .text()
            .join("\n")
            .contains("Shell fixture ended")
    );
    let db = Connection::open(run.scratch.0.join("data/veyra.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM turns", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
fn escaped_pty_holder(closing: bool) {
    struct Escaped(std::path::PathBuf);
    impl Drop for Escaped {
        fn drop(&mut self) {
            if let Ok(text) = std::fs::read_to_string(&self.0)
                && let Ok(pid) = text.parse::<u32>()
            {
                let _ = crate::platform::signal_group(pid, crate::platform::Signal::Kill);
                let deadline = Instant::now() + Duration::from_secs(5);
                while crate::platform::is_alive(pid).unwrap_or(false) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(
                    !crate::platform::is_alive(pid).unwrap_or(false),
                    "escaped owned helper survived cleanup"
                );
            }
        }
    }
    let run = ShellRun::new(None);
    let helper = run.scratch.0.join("holder.py");
    std::fs::write(&helper, concat!(
        "import os,time\nchild=os.fork()\nif child==0:\n    os.setsid()\n",
        "    open('escaped-pid','w').write(str(os.getpid()))\n    time.sleep(60)\n    os._exit(0)\n",
        "print('HOLDER_READY',flush=True)\nwhile True: time.sleep(1)\n",
    )).unwrap();
    std::fs::write(
        run.scratch.0.join("shell"),
        format!("#!/bin/sh\nexec /usr/bin/python3 '{}'\n", helper.display()),
    )
    .unwrap();
    let cleanup = Escaped(run.scratch.0.join("project/escaped-pid"));
    let handle = run.open();
    run.text(&handle, "HOLDER_READY");
    run.wait(|| cleanup.0.is_file());
    let started = Instant::now();
    if closing {
        run.runtime.block_on(run.engine.on_app_quit());
    } else {
        run.engine.close_shells(run.thread);
    }
    run.wait(|| handle.status().is_finished());
    assert!(started.elapsed() < Duration::from_millis(if closing { 1800 } else { 3600 }));
    drop(cleanup);
}
#[test]
fn terminal_exit_is_bounded_with_an_escaped_pty_holder() {
    escaped_pty_holder(false);
}
#[test]
fn native_quit_bounds_a_terminal_reader_held_by_a_child() {
    escaped_pty_holder(true);
}
